use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::asr::ParakeetModel;
use crate::audio_processing::{compute_mel_spectrogram, MelConfig};
use crate::vad::{SileroVad, VadConfig, VadEvent, VadSegmenter, VadState, VAD_CHUNK_SAMPLES};

mod utterance;

use utterance::UtteranceAssembler;

const PREROLL_SECONDS: f32 = 0.2;
const SAMPLE_RATE: f32 = 16000.0;
const DEFAULT_INTERIM_SECS: f32 = 5.0;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", content = "data")]
pub enum PipelineEvent {
    SpeechStart,
    SpeechEnd,
    Transcript { text: String, is_final: bool },
}

pub struct LocalPipeline {
    vad: SileroVad,
    segmenter: VadSegmenter,
    model: Arc<Mutex<ParakeetModel>>,
    utterance: UtteranceAssembler,
    mel_config: MelConfig,
    vad_buf: Vec<f32>,
    preroll: VecDeque<f32>,
    preroll_samples: usize,
}

impl LocalPipeline {
    fn mel_config_for_features(features_size: usize) -> MelConfig {
        MelConfig {
            n_mels: features_size,
            ..MelConfig::default()
        }
    }

    fn new_utterance() -> UtteranceAssembler {
        UtteranceAssembler::new((DEFAULT_INTERIM_SECS * SAMPLE_RATE) as usize)
    }

    pub fn load(
        model_dir: &Path,
        use_accel: bool,
        verbose: bool,
        cancelled: &AtomicBool,
    ) -> Result<Self> {
        let vad_path = model_dir.join("silero_vad.onnx");
        let vad = SileroVad::load(&vad_path, verbose)?;
        let segmenter = VadSegmenter::new(VadConfig::default());

        if cancelled.load(Ordering::Acquire) {
            anyhow::bail!("pipeline load cancelled");
        }

        let model = ParakeetModel::load(model_dir, use_accel, verbose, cancelled)?;
        let mel_config = Self::mel_config_for_features(model.features_size);
        let model = Arc::new(Mutex::new(model));
        let preroll_samples = (PREROLL_SECONDS * SAMPLE_RATE) as usize;

        Ok(Self {
            vad,
            segmenter,
            model,
            utterance: Self::new_utterance(),
            mel_config,
            vad_buf: Vec::new(),
            preroll: VecDeque::new(),
            preroll_samples,
        })
    }

    pub fn shared_model(&self) -> Arc<Mutex<ParakeetModel>> {
        self.model.clone()
    }

    pub fn set_vad_config(&mut self, config: VadConfig) {
        self.segmenter = VadSegmenter::new(config);
    }

    pub fn load_with_shared_model(
        model_dir: &Path,
        shared_model: Arc<Mutex<ParakeetModel>>,
        verbose: bool,
        cancelled: &AtomicBool,
    ) -> Result<Self> {
        let vad_path = model_dir.join("silero_vad.onnx");
        let vad = SileroVad::load(&vad_path, verbose)?;
        let segmenter = VadSegmenter::new(VadConfig::default());

        if cancelled.load(Ordering::Acquire) {
            anyhow::bail!("pipeline load cancelled");
        }

        let mel_config = Self::mel_config_for_features(
            shared_model
                .lock()
                .map_err(|_| anyhow::anyhow!("model mutex poisoned"))?
                .features_size,
        );
        let preroll_samples = (PREROLL_SECONDS * SAMPLE_RATE) as usize;

        Ok(Self {
            vad,
            segmenter,
            model: shared_model,
            utterance: Self::new_utterance(),
            mel_config,
            vad_buf: Vec::new(),
            preroll: VecDeque::new(),
            preroll_samples,
        })
    }

    pub fn process_audio_chunk(&mut self, pcm_i16: &[i16]) -> Vec<PipelineEvent> {
        let samples_f32: Vec<f32> = pcm_i16.iter().map(|&s| s as f32 / 32768.0).collect();
        self.process_f32_samples(&samples_f32)
    }

    fn process_f32_samples(&mut self, samples: &[f32]) -> Vec<PipelineEvent> {
        let mut events = Vec::new();
        let model = &self.model;
        let mel_config = &self.mel_config;
        let mut transcribe = |audio: &[f32]| transcribe_samples(model, mel_config, audio);

        self.vad_buf.extend_from_slice(samples);

        let mut offset = 0;
        while offset + VAD_CHUNK_SAMPLES <= self.vad_buf.len() {
            let mut vad_chunk = [0.0f32; VAD_CHUNK_SAMPLES];
            vad_chunk.copy_from_slice(&self.vad_buf[offset..offset + VAD_CHUNK_SAMPLES]);
            offset += VAD_CHUNK_SAMPLES;

            let speech_prob = match self.vad.process_chunk(&vad_chunk) {
                Ok(prob) => prob,
                Err(e) => {
                    log::error!("VAD processing error: {e}");
                    continue;
                }
            };

            Self::push_preroll(&mut self.preroll, &vad_chunk, self.preroll_samples);

            let event = self.segmenter.process(speech_prob);

            match event {
                VadEvent::SpeechStart => {
                    let preroll_vec: Vec<f32> = self.preroll.iter().copied().collect();
                    self.utterance.begin(&preroll_vec);
                    self.preroll.clear();
                    events.push(PipelineEvent::SpeechStart);
                }
                VadEvent::SpeechEnd => {
                    events.extend(self.utterance.finish(&mut transcribe));
                    events.push(PipelineEvent::SpeechEnd);
                }
                VadEvent::ForceSegment => {
                    events.extend(self.utterance.force_boundary(&vad_chunk, &mut transcribe));
                }
                VadEvent::None => {
                    if self.segmenter.state() == VadState::Speaking {
                        events.extend(self.utterance.push_speech(&vad_chunk, &mut transcribe));
                    }
                }
            }
        }

        if offset > 0 {
            self.vad_buf.drain(..offset);
        }

        events
    }

    pub fn flush(&mut self) -> Vec<PipelineEvent> {
        let mut events = Vec::new();
        let model = &self.model;
        let mel_config = &self.mel_config;
        let mut transcribe = |audio: &[f32]| transcribe_samples(model, mel_config, audio);

        let mut offset = 0;
        while offset + VAD_CHUNK_SAMPLES <= self.vad_buf.len() {
            let mut vad_chunk = [0.0f32; VAD_CHUNK_SAMPLES];
            vad_chunk.copy_from_slice(&self.vad_buf[offset..offset + VAD_CHUNK_SAMPLES]);
            offset += VAD_CHUNK_SAMPLES;

            if let Ok(speech_prob) = self.vad.process_chunk(&vad_chunk) {
                Self::push_preroll(&mut self.preroll, &vad_chunk, self.preroll_samples);
                let event = self.segmenter.process(speech_prob);
                match event {
                    VadEvent::SpeechStart => {
                        let preroll_vec: Vec<f32> = self.preroll.iter().copied().collect();
                        self.utterance.begin(&preroll_vec);
                        self.preroll.clear();
                    }
                    VadEvent::SpeechEnd | VadEvent::ForceSegment => {}
                    VadEvent::None => {
                        if self.segmenter.state() == VadState::Speaking {
                            events.extend(self.utterance.push_speech(&vad_chunk, &mut transcribe));
                        }
                    }
                }
            }
        }

        if offset > 0 {
            self.vad_buf.drain(..offset);
        }

        events.extend(self.utterance.finish(&mut transcribe));
        events
    }

    pub fn reset(&mut self) {
        self.vad.reset();
        self.segmenter.reset();
        self.utterance.clear();
        self.vad_buf.clear();
        self.preroll.clear();
    }

    fn push_preroll(preroll: &mut VecDeque<f32>, chunk: &[f32], max_samples: usize) {
        preroll.extend(chunk.iter().copied());
        while preroll.len() > max_samples {
            preroll.pop_front();
        }
    }
}

fn transcribe_samples(
    model: &Mutex<ParakeetModel>,
    mel_config: &MelConfig,
    samples: &[f32],
) -> Option<String> {
    let ilat_started = std::time::Instant::now();
    let features = compute_mel_spectrogram(samples, mel_config);

    match model.lock().unwrap().transcribe(&features) {
        Ok(text) => {
            let trimmed = text.trim().to_string();
            if cfg!(feature = "ilat") {
                eprintln!(
                    "[ilat] stt ms={} audio_ms={} chars={}",
                    ilat_started.elapsed().as_millis(),
                    samples.len() / 16,
                    trimmed.len()
                );
            }
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        }
        Err(e) => {
            log::error!("Transcription error: {e:#}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_preroll_caps_at_max() {
        let mut preroll = VecDeque::new();
        let chunk = vec![1.0; 100];
        LocalPipeline::push_preroll(&mut preroll, &chunk, 50);
        assert_eq!(preroll.len(), 50);
        assert_eq!(*preroll.front().unwrap(), 1.0);
    }

    #[test]
    fn test_push_preroll_accumulates() {
        let mut preroll = VecDeque::new();
        LocalPipeline::push_preroll(&mut preroll, &[1.0, 2.0], 10);
        LocalPipeline::push_preroll(&mut preroll, &[3.0, 4.0], 10);
        assert_eq!(preroll.len(), 4);
    }
}
