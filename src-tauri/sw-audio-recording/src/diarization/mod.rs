mod embedder;
mod voiceprint;

pub use embedder::{diarize_offline, OfflineSegment, SpeakerEmbedder, SAMPLE_RATE};
pub use voiceprint::{VoicePrint, VoicePrintStore};

use anyhow::Result;
use std::path::Path;

pub const DEFAULT_MATCH_THRESHOLD: f32 = 0.55;
pub const DEFAULT_NAMING_CONFIDENCE: f32 = 0.7;

#[derive(Debug, Clone)]
pub struct Utterance {
    pub index: usize,
    pub samples: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct SpeakerAssignment {
    pub index: usize,
    pub speaker_id: String,
    pub speaker_label: Option<String>,
}

pub struct Diarizer {
    embedder: SpeakerEmbedder,
    store: VoicePrintStore,
    match_threshold: f32,
    naming_confidence: f32,
}

impl Diarizer {
    pub fn new(
        embedding_model: &Path,
        store: VoicePrintStore,
    ) -> Result<Self> {
        let embedder = SpeakerEmbedder::new(embedding_model)?;
        Ok(Self {
            embedder,
            store,
            match_threshold: DEFAULT_MATCH_THRESHOLD,
            naming_confidence: DEFAULT_NAMING_CONFIDENCE,
        })
    }

    pub fn with_thresholds(mut self, match_threshold: f32, naming_confidence: f32) -> Self {
        self.match_threshold = match_threshold;
        self.naming_confidence = naming_confidence;
        self
    }

    pub fn store(&self) -> &VoicePrintStore {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut VoicePrintStore {
        &mut self.store
    }

    pub fn assign_speakers(
        &mut self,
        utterances: &[Utterance],
    ) -> Result<Vec<SpeakerAssignment>> {
        let (assignments, _) = self.assign_speakers_with_embeddings(utterances)?;
        Ok(assignments)
    }

    pub fn assign_speakers_with_embeddings(
        &mut self,
        utterances: &[Utterance],
    ) -> Result<(Vec<SpeakerAssignment>, Vec<(usize, Vec<f32>)>)> {
        let mut embeddings: Vec<(usize, Vec<f32>)> = Vec::new();
        for utterance in utterances {
            if utterance.samples.is_empty() {
                continue;
            }
            match self.embedder.embed(&utterance.samples) {
                Ok(embedding) => embeddings.push((utterance.index, embedding)),
                Err(err) => log::warn!("speaker embedding failed: {err}"),
            }
        }

        let clusters = cluster_within_session(&embeddings, self.match_threshold);
        let mut assignments = Vec::with_capacity(clusters.len());

        for cluster in &clusters {
            let (speaker_id, label) =
                self.store
                    .match_or_create(&cluster.centroid, self.match_threshold, self.naming_confidence);

            self.store
                .accumulate(&speaker_id, &cluster.centroid, cluster.members.len());

            for &index in &cluster.members_indices {
                assignments.push(SpeakerAssignment {
                    index,
                    speaker_id: speaker_id.clone(),
                    speaker_label: label.clone(),
                });
            }
        }

        self.store.persist()?;
        assignments.sort_by_key(|a| a.index);
        Ok((assignments, embeddings))
    }

    pub fn embedder_mut(&mut self) -> &mut SpeakerEmbedder {
        &mut self.embedder
    }
}

struct Cluster {
    centroid: Vec<f32>,
    members: Vec<Vec<f32>>,
    members_indices: Vec<usize>,
}

fn cluster_within_session(embeddings: &[(usize, Vec<f32>)], threshold: f32) -> Vec<Cluster> {
    let mut clusters: Vec<Cluster> = Vec::new();

    for (index, embedding) in embeddings {
        let mut best: Option<(usize, f32)> = None;
        for (cluster_index, cluster) in clusters.iter().enumerate() {
            let score = cosine_similarity(&cluster.centroid, embedding);
            if score >= threshold {
                match best {
                    Some((_, best_score)) if best_score >= score => {}
                    _ => best = Some((cluster_index, score)),
                }
            }
        }

        match best {
            Some((cluster_index, _)) => {
                let cluster = &mut clusters[cluster_index];
                cluster.members.push(embedding.clone());
                cluster.members_indices.push(*index);
                cluster.centroid = mean_embedding(&cluster.members);
            }
            None => clusters.push(Cluster {
                centroid: embedding.clone(),
                members: vec![embedding.clone()],
                members_indices: vec![*index],
            }),
        }
    }

    clusters
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

pub fn mean_embedding(embeddings: &[Vec<f32>]) -> Vec<f32> {
    if embeddings.is_empty() {
        return Vec::new();
    }
    let dim = embeddings[0].len();
    let mut sum = vec![0.0f32; dim];
    for embedding in embeddings {
        for i in 0..dim.min(embedding.len()) {
            sum[i] += embedding[i];
        }
    }
    let count = embeddings.len() as f32;
    for value in &mut sum {
        *value /= count;
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_of_identical_vectors_is_one() {
        let v = vec![0.2, 0.4, 0.6];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn cosine_of_orthogonal_vectors_is_zero() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-5);
    }

    #[test]
    fn mean_is_centroid() {
        let a = vec![0.0, 0.0];
        let b = vec![2.0, 4.0];
        let mean = mean_embedding(&[a, b]);
        assert_eq!(mean, vec![1.0, 2.0]);
    }

    #[test]
    fn clustering_groups_similar_embeddings() {
        let embeddings = vec![
            (0usize, vec![1.0, 0.0, 0.0]),
            (1usize, vec![0.98, 0.02, 0.0]),
            (2usize, vec![0.0, 1.0, 0.0]),
        ];
        let clusters = cluster_within_session(&embeddings, 0.8);
        assert_eq!(clusters.len(), 2);
    }
}
