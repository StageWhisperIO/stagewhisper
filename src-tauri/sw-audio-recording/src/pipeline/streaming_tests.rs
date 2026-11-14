use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;

#[derive(Default)]
struct Recorded {
    sent_samples: usize,
    commits: usize,
    finish_timeouts: Vec<Duration>,
    queued: VecDeque<StreamedTranscript>,
    on_finish: Vec<StreamedTranscript>,
}

#[derive(Clone, Default)]
struct FakeStream(Arc<Mutex<Recorded>>);

impl FakeStream {
    fn queue(&self, text: &str, is_final: bool) {
        self.0.lock().unwrap().queued.push_back(StreamedTranscript {
            text: text.to_string(),
            is_final,
        });
    }

    fn answer_finish_with(&self, text: &str) {
        self.0.lock().unwrap().on_finish.push(StreamedTranscript {
            text: text.to_string(),
            is_final: true,
        });
    }

    fn sent_samples(&self) -> usize {
        self.0.lock().unwrap().sent_samples
    }

    fn commits(&self) -> usize {
        self.0.lock().unwrap().commits
    }
}

impl TranscriptStream for FakeStream {
    fn send_audio(&mut self, pcm_i16: &[i16]) {
        self.0.lock().unwrap().sent_samples += pcm_i16.len();
    }

    fn commit(&mut self) {
        self.0.lock().unwrap().commits += 1;
    }

    fn drain(&mut self) -> Vec<StreamedTranscript> {
        self.0.lock().unwrap().queued.drain(..).collect()
    }

    fn finish(&mut self, timeout: Duration) -> Vec<StreamedTranscript> {
        let mut recorded = self.0.lock().unwrap();
        recorded.finish_timeouts.push(timeout);
        let mut answered: Vec<_> = recorded.queued.drain(..).collect();
        answered.append(&mut recorded.on_finish);
        answered
    }
}

struct ScriptedVad {
    speaking: Arc<Mutex<bool>>,
}

impl SpeechProbability for ScriptedVad {
    fn probability(&mut self, _chunk: &[f32]) -> Result<f32> {
        Ok(if *self.speaking.lock().unwrap() {
            0.9
        } else {
            0.1
        })
    }

    fn reset(&mut self) {}
}

struct Harness {
    stream: FakeStream,
    speaking: Arc<Mutex<bool>>,
    pipeline: StreamingPipeline<FakeStream, ScriptedVad>,
}

fn harness() -> Harness {
    let stream = FakeStream::default();
    let speaking = Arc::new(Mutex::new(false));
    let pipeline = StreamingPipeline::new(
        stream.clone(),
        ScriptedVad {
            speaking: Arc::clone(&speaking),
        },
    )
    .with_partial_interval(Duration::ZERO);
    Harness {
        stream,
        speaking,
        pipeline,
    }
}

fn vad_chunks(count: usize) -> Vec<i16> {
    vec![0; VAD_CHUNK_SAMPLES * count]
}

impl Harness {
    fn speak(&mut self, chunks: usize) -> Vec<PipelineEvent> {
        *self.speaking.lock().unwrap() = true;
        self.pipeline.process_audio_chunk(&vad_chunks(chunks))
    }

    fn pause(&mut self, chunks: usize) -> Vec<PipelineEvent> {
        *self.speaking.lock().unwrap() = false;
        self.pipeline.process_audio_chunk(&vad_chunks(chunks))
    }
}

fn finals(events: &[PipelineEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            PipelineEvent::Transcript {
                text,
                is_final: true,
            } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn partials(events: &[PipelineEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            PipelineEvent::Transcript {
                text,
                is_final: false,
            } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn silence_is_never_sent_to_the_cloud() {
    let mut h = harness();

    let events = h.pause(40);

    assert!(events.is_empty());
    assert_eq!(h.stream.sent_samples(), 0);
    assert_eq!(h.stream.commits(), 0);
}

#[test]
fn speech_is_streamed_from_its_start_and_committed_once_the_speaker_pauses() {
    let mut h = harness();

    let started = h.speak(10);
    let ended = h.pause(30);

    assert!(matches!(started.first(), Some(PipelineEvent::SpeechStart)));
    assert!(ended.iter().any(|e| matches!(e, PipelineEvent::SpeechEnd)));
    assert!(h.stream.sent_samples() >= VAD_CHUNK_SAMPLES * 10);
    assert_eq!(h.stream.commits(), 1);
}

#[test]
fn a_transcript_arriving_later_is_reported_on_the_next_chunk() {
    let mut h = harness();
    h.speak(10);
    h.pause(30);

    h.stream.queue("Hello", false);
    h.stream.queue("Hello there!", true);
    let events = h.pause(1);

    assert_eq!(finals(&events), vec!["Hello there!".to_string()]);
    assert!(partials(&events).is_empty());
}

#[test]
fn only_the_newest_partial_of_a_burst_is_reported() {
    let mut h = harness();
    h.speak(4);

    h.stream.queue("Hel", false);
    h.stream.queue("Hello", false);
    h.stream.queue("Hello th", false);
    let events = h.speak(1);

    assert_eq!(partials(&events), vec!["Hello th".to_string()]);
}

#[test]
fn partials_are_throttled_to_the_configured_interval() {
    let mut h = harness();
    h.pipeline = StreamingPipeline::new(
        h.stream.clone(),
        ScriptedVad {
            speaking: Arc::clone(&h.speaking),
        },
    )
    .with_partial_interval(Duration::from_secs(60));
    h.speak(2);

    h.stream.queue("one", false);
    let first = h.speak(1);
    h.stream.queue("one two", false);
    let second = h.speak(1);

    assert_eq!(partials(&first), vec!["one".to_string()]);
    assert!(partials(&second).is_empty());
}

#[test]
fn an_empty_final_is_not_reported() {
    let mut h = harness();
    h.stream.queue("  ", true);

    assert!(finals(&h.pause(1)).is_empty());
}

#[test]
fn a_long_monologue_is_committed_at_the_forced_boundary() {
    let mut h = harness();
    let max_speech_chunks = (30_000 / 32) as usize;

    h.speak(max_speech_chunks + 2);

    assert_eq!(h.stream.commits(), 1);
}

#[test]
fn flushing_mid_sentence_commits_and_waits_for_the_last_transcript() {
    let mut h = harness();
    h.speak(10);
    h.stream.queue("partial words", false);
    h.stream.answer_finish_with("the last sentence");

    let events = h.pipeline.flush();

    assert_eq!(h.stream.commits(), 1);
    assert_eq!(finals(&events), vec!["the last sentence".to_string()]);
    assert!(partials(&events).is_empty());
    assert_eq!(
        h.stream.0.lock().unwrap().finish_timeouts,
        vec![DEFAULT_FINISH_TIMEOUT]
    );
}

#[test]
fn flushing_after_silence_does_not_commit_an_empty_buffer() {
    let mut h = harness();
    h.pause(10);

    h.pipeline.flush();

    assert_eq!(h.stream.commits(), 0);
}

#[test]
fn resetting_closes_the_utterance_that_was_being_streamed() {
    let mut h = harness();
    h.speak(10);

    h.pipeline.reset();
    h.pause(30);

    assert_eq!(h.stream.commits(), 1);
}

#[test]
fn a_final_carries_the_time_its_utterance_was_spoken_not_when_it_came_back() {
    let mut h = harness();
    *h.speaking.lock().unwrap() = true;
    h.pipeline.process_audio_chunk_at(&vad_chunks(10), 1_000);
    *h.speaking.lock().unwrap() = false;
    h.pipeline.process_audio_chunk_at(&vad_chunks(30), 2_000);

    assert_eq!(h.pipeline.final_timing().pending_since_ms, Some(2_000));

    h.stream.queue("Is that okay?", true);
    let events = h.pipeline.process_audio_chunk_at(&vad_chunks(1), 9_000);
    let timing = h.pipeline.final_timing();

    assert_eq!(finals(&events), vec!["Is that okay?".to_string()]);
    assert_eq!(timing.final_captured_at_ms, vec![2_000]);
    assert_eq!(timing.pending_since_ms, None);
}

#[test]
fn an_utterance_heard_as_silence_still_settles_its_place_in_line() {
    let mut h = harness();
    *h.speaking.lock().unwrap() = true;
    h.pipeline.process_audio_chunk_at(&vad_chunks(10), 1_000);
    *h.speaking.lock().unwrap() = false;
    h.pipeline.process_audio_chunk_at(&vad_chunks(30), 2_000);
    *h.speaking.lock().unwrap() = true;
    h.pipeline.process_audio_chunk_at(&vad_chunks(10), 3_000);
    *h.speaking.lock().unwrap() = false;
    h.pipeline.process_audio_chunk_at(&vad_chunks(30), 4_000);

    h.stream.queue("", true);
    h.stream.queue("Yes.", true);
    h.pipeline.process_audio_chunk_at(&vad_chunks(1), 5_000);

    assert_eq!(h.pipeline.final_timing().final_captured_at_ms, vec![4_000]);
}
