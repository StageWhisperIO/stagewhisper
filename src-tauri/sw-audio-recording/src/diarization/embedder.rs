use anyhow::{anyhow, Result};
use sherpa_rs::diarize::{Diarize, DiarizeConfig};
use sherpa_rs::speaker_id::{EmbeddingExtractor, ExtractorConfig};
use std::path::Path;

pub const SAMPLE_RATE: u32 = 16000;

pub struct SpeakerEmbedder {
    extractor: EmbeddingExtractor,
}

impl SpeakerEmbedder {
    pub fn new(model: &Path) -> Result<Self> {
        let config = ExtractorConfig {
            model: model.to_string_lossy().to_string(),
            provider: None,
            num_threads: Some(1),
            debug: false,
        };
        let extractor =
            EmbeddingExtractor::new(config).map_err(|e| anyhow!("embedding extractor: {e}"))?;
        Ok(Self { extractor })
    }

    pub fn dimension(&self) -> usize {
        self.extractor.embedding_size
    }

    pub fn embed(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        self.extractor
            .compute_speaker_embedding(samples.to_vec(), SAMPLE_RATE)
            .map_err(|e| anyhow!("compute embedding: {e}"))
    }
}

#[derive(Debug, Clone)]
pub struct OfflineSegment {
    pub start: f32,
    pub end: f32,
    pub speaker: i32,
}

pub fn diarize_offline(
    segmentation_model: &Path,
    embedding_model: &Path,
    samples: &[f32],
    num_speakers: Option<i32>,
    threshold: Option<f32>,
) -> Result<Vec<OfflineSegment>> {
    let config = DiarizeConfig {
        num_clusters: num_speakers,
        threshold: if num_speakers.is_some() {
            None
        } else {
            Some(threshold.unwrap_or(0.5))
        },
        min_duration_on: Some(0.3),
        min_duration_off: Some(0.5),
        provider: None,
        debug: false,
    };

    let mut diarize = Diarize::new(segmentation_model, embedding_model, config)
        .map_err(|e| anyhow!("offline diarization init: {e}"))?;

    let segments = diarize
        .compute(samples.to_vec(), None)
        .map_err(|e| anyhow!("offline diarization compute: {e}"))?;

    Ok(segments
        .into_iter()
        .map(|s| OfflineSegment {
            start: s.start,
            end: s.end,
            speaker: s.speaker,
        })
        .collect())
}
