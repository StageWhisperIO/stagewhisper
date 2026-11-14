use crate::pipeline::{LocalPipeline, PipelineEvent};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FinalTiming {
    pub final_captured_at_ms: Vec<u64>,
    pub pending_since_ms: Option<u64>,
}

pub trait Transcriber: Send {
    fn process_audio_chunk(&mut self, pcm_i16: &[i16]) -> Vec<PipelineEvent>;
    fn flush(&mut self) -> Vec<PipelineEvent>;
    fn reset(&mut self);

    fn process_audio_chunk_at(
        &mut self,
        pcm_i16: &[i16],
        _captured_at_ms: u64,
    ) -> Vec<PipelineEvent> {
        self.process_audio_chunk(pcm_i16)
    }

    fn final_timing(&mut self) -> FinalTiming {
        FinalTiming::default()
    }
}

impl Transcriber for LocalPipeline {
    fn process_audio_chunk(&mut self, pcm_i16: &[i16]) -> Vec<PipelineEvent> {
        LocalPipeline::process_audio_chunk(self, pcm_i16)
    }

    fn flush(&mut self) -> Vec<PipelineEvent> {
        LocalPipeline::flush(self)
    }

    fn reset(&mut self) {
        LocalPipeline::reset(self)
    }
}

impl<T: Transcriber + ?Sized> Transcriber for Box<T> {
    fn process_audio_chunk(&mut self, pcm_i16: &[i16]) -> Vec<PipelineEvent> {
        (**self).process_audio_chunk(pcm_i16)
    }

    fn flush(&mut self) -> Vec<PipelineEvent> {
        (**self).flush()
    }

    fn reset(&mut self) {
        (**self).reset()
    }

    fn process_audio_chunk_at(
        &mut self,
        pcm_i16: &[i16],
        captured_at_ms: u64,
    ) -> Vec<PipelineEvent> {
        (**self).process_audio_chunk_at(pcm_i16, captured_at_ms)
    }

    fn final_timing(&mut self) -> FinalTiming {
        (**self).final_timing()
    }
}
