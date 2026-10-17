use crate::audio_processing::AudioBuffer;

use super::PipelineEvent;

pub(super) const MIN_UTTERANCE_SAMPLES: usize = 1600;
const BUFFER_LIMIT_SECS: f32 = 60.0;

pub(super) struct UtteranceAssembler {
    piece_audio: AudioBuffer,
    segment_audio: AudioBuffer,
    piece_samples: usize,
    samples_since_piece: usize,
    piece_drained_in_segment: bool,
    segment_piece_texts: Vec<String>,
    finished_segment_texts: Vec<String>,
}

impl UtteranceAssembler {
    pub(super) fn new(piece_samples: usize) -> Self {
        Self {
            piece_audio: AudioBuffer::new(BUFFER_LIMIT_SECS),
            segment_audio: AudioBuffer::new(BUFFER_LIMIT_SECS),
            piece_samples,
            samples_since_piece: 0,
            piece_drained_in_segment: false,
            segment_piece_texts: Vec::new(),
            finished_segment_texts: Vec::new(),
        }
    }

    pub(super) fn begin(&mut self, preroll: &[f32]) {
        self.clear();
        self.append(preroll);
    }

    pub(super) fn push_speech<F>(
        &mut self,
        chunk: &[f32],
        transcribe: &mut F,
    ) -> Option<PipelineEvent>
    where
        F: FnMut(&[f32]) -> Option<String>,
    {
        self.append(chunk);
        self.samples_since_piece += chunk.len();
        if self.samples_since_piece < self.piece_samples {
            return None;
        }
        self.samples_since_piece = 0;
        self.piece_drained_in_segment = true;
        let piece = self.piece_audio.drain();
        self.decode_piece(&piece, transcribe).map(interim_event)
    }

    pub(super) fn force_boundary<F>(
        &mut self,
        next_chunk: &[f32],
        transcribe: &mut F,
    ) -> Option<PipelineEvent>
    where
        F: FnMut(&[f32]) -> Option<String>,
    {
        let tail_event = self.close_segment(transcribe);
        self.append(next_chunk);
        self.samples_since_piece = next_chunk.len();
        tail_event
    }

    pub(super) fn finish<F>(&mut self, transcribe: &mut F) -> Vec<PipelineEvent>
    where
        F: FnMut(&[f32]) -> Option<String>,
    {
        let mut events: Vec<PipelineEvent> = self.close_segment(transcribe).into_iter().collect();
        let finished_texts = std::mem::take(&mut self.finished_segment_texts);
        if !finished_texts.is_empty() {
            events.push(PipelineEvent::Transcript {
                text: finished_texts.join(" "),
                is_final: true,
            });
        }
        self.samples_since_piece = 0;
        events
    }

    pub(super) fn clear(&mut self) {
        self.piece_audio.clear();
        self.segment_audio.clear();
        self.samples_since_piece = 0;
        self.piece_drained_in_segment = false;
        self.segment_piece_texts.clear();
        self.finished_segment_texts.clear();
    }

    fn append(&mut self, samples: &[f32]) {
        self.piece_audio.push(samples);
        self.segment_audio.push(samples);
    }

    fn close_segment<F>(&mut self, transcribe: &mut F) -> Option<PipelineEvent>
    where
        F: FnMut(&[f32]) -> Option<String>,
    {
        let tail = self.piece_audio.drain();
        let segment = self.segment_audio.drain();
        let tail_is_whole_segment = !self.piece_drained_in_segment;

        let tail_text = self.decode_piece(&tail, transcribe);
        let whole_segment_text = if tail_is_whole_segment {
            tail_text.clone()
        } else {
            decode_long_enough(&segment, transcribe)
        };

        let segment_text = whole_segment_text.or_else(|| self.joined_pieces_fallback());
        self.finished_segment_texts.extend(segment_text);
        self.segment_piece_texts.clear();
        self.piece_drained_in_segment = false;
        tail_text.map(interim_event)
    }

    fn decode_piece<F>(&mut self, piece: &[f32], transcribe: &mut F) -> Option<String>
    where
        F: FnMut(&[f32]) -> Option<String>,
    {
        let text = decode_long_enough(piece, transcribe)?;
        self.segment_piece_texts.push(text.clone());
        Some(text)
    }

    fn joined_pieces_fallback(&self) -> Option<String> {
        if self.segment_piece_texts.is_empty() {
            return None;
        }
        log::debug!(
            "whole-utterance decode returned no text, keeping {} live pieces",
            self.segment_piece_texts.len()
        );
        Some(self.segment_piece_texts.join(" "))
    }
}

fn interim_event(text: String) -> PipelineEvent {
    PipelineEvent::Transcript {
        text,
        is_final: false,
    }
}

fn decode_long_enough<F>(samples: &[f32], transcribe: &mut F) -> Option<String>
where
    F: FnMut(&[f32]) -> Option<String>,
{
    if samples.len() <= MIN_UTTERANCE_SAMPLES {
        return None;
    }
    transcribe(samples).filter(|text| !text.trim().is_empty())
}

#[cfg(test)]
#[path = "utterance_tests.rs"]
mod tests;
