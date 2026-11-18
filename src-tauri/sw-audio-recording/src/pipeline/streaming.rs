use std::collections::VecDeque;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::pipeline::PipelineEvent;
use crate::transcriber::{FinalTiming, Transcriber};
use crate::vad::{SileroVad, VadConfig, VadEvent, VadSegmenter, VadState, VAD_CHUNK_SAMPLES};

const PREROLL_SAMPLES: usize = 3_200;
pub const DEFAULT_PARTIAL_INTERVAL: Duration = Duration::from_millis(500);
pub const DEFAULT_FINISH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamedTranscript {
    pub text: String,
    pub is_final: bool,
}

pub trait TranscriptStream: Send {
    fn send_audio(&mut self, pcm_i16: &[i16]);
    fn commit(&mut self);
    fn drain(&mut self) -> Vec<StreamedTranscript>;
    fn finish(&mut self, timeout: Duration) -> Vec<StreamedTranscript>;
}

pub trait SpeechProbability: Send {
    fn probability(&mut self, chunk: &[f32]) -> Result<f32>;
    fn reset(&mut self);
}

impl SpeechProbability for SileroVad {
    fn probability(&mut self, chunk: &[f32]) -> Result<f32> {
        self.process_chunk(chunk)
    }

    fn reset(&mut self) {
        SileroVad::reset(self)
    }
}

pub struct StreamingPipeline<S, V> {
    stream: S,
    vad: V,
    segmenter: VadSegmenter,
    vad_buf: Vec<i16>,
    preroll: VecDeque<i16>,
    sent_since_commit: bool,
    partial_interval: Duration,
    last_partial_at: Option<Instant>,
    finish_timeout: Duration,
    capture_ms: u64,
    pending_commit_ms: VecDeque<u64>,
    final_captured_at_ms: Vec<u64>,
}

impl<S: TranscriptStream, V: SpeechProbability> StreamingPipeline<S, V> {
    pub fn new(stream: S, vad: V) -> Self {
        Self {
            stream,
            vad,
            segmenter: VadSegmenter::new(VadConfig::default()),
            vad_buf: Vec::new(),
            preroll: VecDeque::with_capacity(PREROLL_SAMPLES),
            sent_since_commit: false,
            partial_interval: DEFAULT_PARTIAL_INTERVAL,
            last_partial_at: None,
            finish_timeout: DEFAULT_FINISH_TIMEOUT,
            capture_ms: 0,
            pending_commit_ms: VecDeque::new(),
            final_captured_at_ms: Vec::new(),
        }
    }

    pub fn with_partial_interval(mut self, interval: Duration) -> Self {
        self.partial_interval = interval;
        self
    }

    pub fn with_finish_timeout(mut self, timeout: Duration) -> Self {
        self.finish_timeout = timeout;
        self
    }

    fn run_vad(&mut self, events: &mut Vec<PipelineEvent>, emit_boundaries: bool) {
        let mut offset = 0;
        while offset + VAD_CHUNK_SAMPLES <= self.vad_buf.len() {
            let chunk: Vec<i16> = self.vad_buf[offset..offset + VAD_CHUNK_SAMPLES].to_vec();
            offset += VAD_CHUNK_SAMPLES;
            let samples: Vec<f32> = chunk.iter().map(|&s| s as f32 / 32768.0).collect();
            let speech_prob = match self.vad.probability(&samples) {
                Ok(prob) => prob,
                Err(err) => {
                    log::error!("VAD processing error: {err}");
                    continue;
                }
            };
            self.push_preroll(&chunk);
            match self.segmenter.process(speech_prob) {
                VadEvent::SpeechStart => {
                    let preroll: Vec<i16> = self.preroll.drain(..).collect();
                    self.send(&preroll);
                    if emit_boundaries {
                        events.push(PipelineEvent::SpeechStart);
                    }
                }
                VadEvent::SpeechEnd => {
                    self.commit();
                    if emit_boundaries {
                        events.push(PipelineEvent::SpeechEnd);
                    }
                }
                VadEvent::ForceSegment => {
                    self.send(&chunk);
                    self.commit();
                }
                VadEvent::None => {
                    if self.segmenter.state() == VadState::Speaking {
                        self.send(&chunk);
                    }
                }
            }
        }
        if offset > 0 {
            self.vad_buf.drain(..offset);
        }
    }

    fn send(&mut self, pcm: &[i16]) {
        if pcm.is_empty() {
            return;
        }
        self.stream.send_audio(pcm);
        self.sent_since_commit = true;
    }

    fn commit(&mut self) {
        if self.sent_since_commit {
            self.stream.commit();
            self.pending_commit_ms.push_back(self.capture_ms);
            self.sent_since_commit = false;
        }
    }

    fn push_preroll(&mut self, chunk: &[i16]) {
        self.preroll.extend(chunk.iter().copied());
        while self.preroll.len() > PREROLL_SAMPLES {
            self.preroll.pop_front();
        }
    }

    fn collect(&mut self, transcripts: Vec<StreamedTranscript>, events: &mut Vec<PipelineEvent>) {
        let mut latest_partial = None;
        for transcript in transcripts {
            if transcript.is_final {
                latest_partial = None;
                self.last_partial_at = None;
                let spoken_at_ms = self
                    .pending_commit_ms
                    .pop_front()
                    .unwrap_or(self.capture_ms);
                if !transcript.text.trim().is_empty() {
                    events.push(PipelineEvent::Transcript {
                        text: transcript.text.trim().to_string(),
                        is_final: true,
                    });
                    self.final_captured_at_ms.push(spoken_at_ms);
                }
            } else {
                latest_partial = Some(transcript.text);
            }
        }
        let Some(text) = latest_partial else {
            return;
        };
        let now = Instant::now();
        let due = self
            .last_partial_at
            .is_none_or(|at| now.saturating_duration_since(at) >= self.partial_interval);
        if due && !text.trim().is_empty() {
            self.last_partial_at = Some(now);
            events.push(PipelineEvent::Transcript {
                text: text.trim().to_string(),
                is_final: false,
            });
        }
    }
}

impl<S: TranscriptStream, V: SpeechProbability> Transcriber for StreamingPipeline<S, V> {
    fn process_audio_chunk(&mut self, pcm_i16: &[i16]) -> Vec<PipelineEvent> {
        let mut events = Vec::new();
        self.vad_buf.extend_from_slice(pcm_i16);
        self.run_vad(&mut events, true);
        let transcripts = self.stream.drain();
        self.collect(transcripts, &mut events);
        events
    }

    fn flush(&mut self) -> Vec<PipelineEvent> {
        let mut events = Vec::new();
        self.run_vad(&mut events, false);
        if self.segmenter.state() == VadState::Speaking {
            let rest = std::mem::take(&mut self.vad_buf);
            self.send(&rest);
        }
        self.commit();
        let transcripts = self.stream.finish(self.finish_timeout);
        self.collect(transcripts, &mut events);
        events.retain(|event| {
            !matches!(
                event,
                PipelineEvent::Transcript {
                    is_final: false,
                    ..
                }
            )
        });
        events
    }

    fn process_audio_chunk_at(
        &mut self,
        pcm_i16: &[i16],
        captured_at_ms: u64,
    ) -> Vec<PipelineEvent> {
        self.capture_ms = captured_at_ms;
        self.process_audio_chunk(pcm_i16)
    }

    fn final_timing(&mut self) -> FinalTiming {
        FinalTiming {
            final_captured_at_ms: std::mem::take(&mut self.final_captured_at_ms),
            pending_since_ms: self.pending_commit_ms.front().copied(),
        }
    }

    fn reset(&mut self) {
        self.commit();
        self.vad.reset();
        self.segmenter.reset();
        self.vad_buf.clear();
        self.preroll.clear();
    }
}

#[cfg(test)]
#[path = "streaming_tests.rs"]
mod tests;
