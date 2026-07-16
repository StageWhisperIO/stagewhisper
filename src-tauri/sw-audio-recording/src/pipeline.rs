use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::asr::ParakeetModel;
use crate::audio_processing::{compute_mel_spectrogram, AudioBuffer, MelConfig};
use crate::vad::{SileroVad, VadConfig, VadEvent, VadSegmenter, VadState, VAD_CHUNK_SAMPLES};

const PREROLL_SECONDS: f32 = 0.2;
const SAMPLE_RATE: f32 = 16000.0;
const MIN_UTTERANCE_SAMPLES: usize = 1600;
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
    utterance_buffer: AudioBuffer,
    mel_config: MelConfig,
    vad_buf: Vec<f32>,
    preroll: VecDeque<f32>,
    preroll_samples: usize,
    interim_samples: usize,
    samples_since_emit: usize,
    interim_texts: Vec<String>,
}

impl LocalPipeline {
    pub fn load(
        model_dir: &Path,
        use_coreml: bool,
        verbose: bool,
        cancelled: &AtomicBool,
    ) -> Result<Self> {
        let vad_path = model_dir.join("silero_vad.onnx");
        let vad = SileroVad::load(&vad_path, verbose)?;
        let segmenter = VadSegmenter::new(VadConfig::default());

        if cancelled.load(Ordering::Acquire) {
            anyhow::bail!("pipeline load cancelled");
        }

        let model = ParakeetModel::load(model_dir, use_coreml, verbose, cancelled)?;
        let model = Arc::new(Mutex::new(model));
        let utterance_buffer = AudioBuffer::new(60.0);
        let mel_config = MelConfig::default();
        let preroll_samples = (PREROLL_SECONDS * SAMPLE_RATE) as usize;
        let interim_samples = (DEFAULT_INTERIM_SECS * SAMPLE_RATE) as usize;

        Ok(Self {
            vad,
            segmenter,
            model,
            utterance_buffer,
            mel_config,
            vad_buf: Vec::new(),
            preroll: VecDeque::new(),
            preroll_samples,
            interim_samples,
            samples_since_emit: 0,
            interim_texts: Vec::new(),
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

        let utterance_buffer = AudioBuffer::new(60.0);
        let mel_config = MelConfig::default();
        let preroll_samples = (PREROLL_SECONDS * SAMPLE_RATE) as usize;
        let interim_samples = (DEFAULT_INTERIM_SECS * SAMPLE_RATE) as usize;

        Ok(Self {
            vad,
            segmenter,
            model: shared_model,
            utterance_buffer,
            mel_config,
            vad_buf: Vec::new(),
            preroll: VecDeque::new(),
            preroll_samples,
            interim_samples,
            samples_since_emit: 0,
            interim_texts: Vec::new(),
        })
    }

    pub fn process_audio_chunk(&mut self, pcm_i16: &[i16]) -> Vec<PipelineEvent> {
        let samples_f32: Vec<f32> = pcm_i16.iter().map(|&s| s as f32 / 32768.0).collect();
        self.process_f32_samples(&samples_f32)
    }

    fn process_f32_samples(&mut self, samples: &[f32]) -> Vec<PipelineEvent> {
        let mut events = Vec::new();

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
                    self.utterance_buffer.clear();
                    self.interim_texts.clear();
                    let preroll_vec: Vec<f32> = self.preroll.iter().copied().collect();
                    self.utterance_buffer.push(&preroll_vec);
                    self.preroll.clear();
                    self.samples_since_emit = 0;
                    events.push(PipelineEvent::SpeechStart);
                }
                VadEvent::SpeechEnd => {
                    let samples = self.utterance_buffer.drain();
                    let tail_text = if samples.len() > MIN_UTTERANCE_SAMPLES {
                        self.transcribe_samples(&samples)
                    } else {
                        None
                    };

                    if let Some(ref tail) = tail_text {
                        events.push(PipelineEvent::Transcript {
                            text: tail.clone(),
                            is_final: false,
                        });
                        self.interim_texts.push(tail.clone());
                    }

                    if !self.interim_texts.is_empty() {
                        let combined = self.interim_texts.join(" ");
                        self.interim_texts.clear();
                        if !combined.is_empty() {
                            events.push(PipelineEvent::Transcript {
                                text: combined,
                                is_final: true,
                            });
                        }
                    }
                    self.samples_since_emit = 0;
                    events.push(PipelineEvent::SpeechEnd);
                }
                VadEvent::ForceSegment => {
                    let samples = self.utterance_buffer.drain();
                    if samples.len() > MIN_UTTERANCE_SAMPLES {
                        if let Some(text) = self.transcribe_samples(&samples) {
                            self.interim_texts.push(text.clone());
                            events.push(PipelineEvent::Transcript {
                                text,
                                is_final: false,
                            });
                        }
                    }
                    self.utterance_buffer.push(&vad_chunk);
                    self.samples_since_emit = VAD_CHUNK_SAMPLES;
                }
                VadEvent::None => {
                    if self.segmenter.state() == VadState::Speaking {
                        self.utterance_buffer.push(&vad_chunk);
                        self.samples_since_emit += VAD_CHUNK_SAMPLES;

                        if self.samples_since_emit >= self.interim_samples {
                            let samples = self.utterance_buffer.drain();
                            if samples.len() > MIN_UTTERANCE_SAMPLES {
                                if let Some(text) = self.transcribe_samples(&samples) {
                                    self.interim_texts.push(text.clone());
                                    events.push(PipelineEvent::Transcript {
                                        text,
                                        is_final: false,
                                    });
                                }
                            }
                            self.samples_since_emit = 0;
                        }
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
                        self.utterance_buffer.clear();
                        let preroll_vec: Vec<f32> = self.preroll.iter().copied().collect();
                        self.utterance_buffer.push(&preroll_vec);
                        self.preroll.clear();
                    }
                    VadEvent::SpeechEnd | VadEvent::ForceSegment => {}
                    VadEvent::None => {
                        if self.segmenter.state() == VadState::Speaking {
                            self.utterance_buffer.push(&vad_chunk);
                        }
                    }
                }
            }
        }

        if offset > 0 {
            self.vad_buf.drain(..offset);
        }

        if self.utterance_buffer.duration_secs() > 0.1 {
            let samples = self.utterance_buffer.drain();
            if let Some(text) = self.transcribe_samples(&samples) {
                events.push(PipelineEvent::Transcript {
                    text: text.clone(),
                    is_final: false,
                });
                self.interim_texts.push(text);
            }
        } else {
            self.utterance_buffer.clear();
        }

        if !self.interim_texts.is_empty() {
            let combined = self.interim_texts.join(" ");
            self.interim_texts.clear();
            if !combined.is_empty() {
                events.push(PipelineEvent::Transcript {
                    text: combined,
                    is_final: true,
                });
            }
        }

        events
    }

    pub fn reset(&mut self) {
        self.vad.reset();
        self.segmenter.reset();
        self.utterance_buffer.clear();
        self.vad_buf.clear();
        self.preroll.clear();
        self.samples_since_emit = 0;
        self.interim_texts.clear();
    }

    fn transcribe_samples(&mut self, samples: &[f32]) -> Option<String> {
        let ilat_started = std::time::Instant::now();
        let features = compute_mel_spectrogram(samples, &self.mel_config);

        match self.model.lock().unwrap().transcribe(&features) {
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
                log::error!("Transcription error: {e}");
                None
            }
        }
    }

    fn push_preroll(preroll: &mut VecDeque<f32>, chunk: &[f32], max_samples: usize) {
        preroll.extend(chunk.iter().copied());
        while preroll.len() > max_samples {
            preroll.pop_front();
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
