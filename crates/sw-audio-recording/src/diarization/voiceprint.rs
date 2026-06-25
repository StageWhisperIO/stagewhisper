use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::{cosine_similarity, mean_embedding};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoicePrint {
    pub speaker_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub centroid_embedding: Vec<f32>,
    pub sample_count: usize,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct VoicePrintFile {
    #[serde(default)]
    prints: Vec<VoicePrint>,
    #[serde(default)]
    next_id: u64,
}

pub struct VoicePrintStore {
    path: PathBuf,
    key: [u8; 32],
    prints: Vec<VoicePrint>,
    next_id: u64,
}

impl VoicePrintStore {
    pub fn load(path: &Path, key: [u8; 32]) -> Result<Self> {
        let mut store = Self {
            path: path.to_path_buf(),
            key,
            prints: Vec::new(),
            next_id: 1,
        };

        if path.exists() {
            let decrypted = sw_crypto::decrypt_file(&key, path)
                .with_context(|| format!("decrypt voiceprint store: {}", path.display()))?;
            let parsed: VoicePrintFile =
                serde_json::from_slice(&decrypted).context("parse voiceprint store")?;
            store.prints = parsed.prints;
            store.next_id = parsed.next_id.max(1);
        }

        Ok(store)
    }

    pub fn prints(&self) -> &[VoicePrint] {
        &self.prints
    }

    pub fn rename(&mut self, speaker_id: &str, label: Option<String>) -> Result<bool> {
        let mut changed = false;
        for print in &mut self.prints {
            if print.speaker_id == speaker_id {
                print.label = label.clone();
                changed = true;
            }
        }
        if changed {
            self.persist()?;
        }
        Ok(changed)
    }

    pub fn match_or_create(
        &mut self,
        embedding: &[f32],
        match_threshold: f32,
        naming_confidence: f32,
    ) -> (String, Option<String>) {
        let mut best: Option<(usize, f32)> = None;
        for (index, print) in self.prints.iter().enumerate() {
            let score = cosine_similarity(&print.centroid_embedding, embedding);
            if score >= match_threshold {
                match best {
                    Some((_, best_score)) if best_score >= score => {}
                    _ => best = Some((index, score)),
                }
            }
        }

        if let Some((index, score)) = best {
            let print = &self.prints[index];
            let label = if score >= naming_confidence {
                print.label.clone()
            } else {
                None
            };
            return (print.speaker_id.clone(), label);
        }

        let speaker_id = format!("spk_{}", self.next_id);
        self.next_id += 1;
        self.prints.push(VoicePrint {
            speaker_id: speaker_id.clone(),
            label: None,
            centroid_embedding: embedding.to_vec(),
            sample_count: 0,
        });
        (speaker_id, None)
    }

    pub fn accumulate(&mut self, speaker_id: &str, embedding: &[f32], samples_added: usize) {
        for print in &mut self.prints {
            if print.speaker_id == speaker_id {
                let total = print.sample_count + samples_added.max(1);
                let blended = mean_embedding(&[
                    scaled(&print.centroid_embedding, print.sample_count as f32),
                    scaled(embedding, samples_added.max(1) as f32),
                ]);
                let renorm = if total > 0 {
                    scaled(&blended, 2.0 / total as f32)
                } else {
                    blended
                };
                print.centroid_embedding = renorm;
                print.sample_count = total;
                return;
            }
        }
    }

    pub fn persist(&self) -> Result<()> {
        let file = VoicePrintFile {
            prints: self.prints.clone(),
            next_id: self.next_id,
        };
        let plaintext = serde_json::to_vec(&file).context("serialize voiceprint store")?;
        let encrypted =
            sw_crypto::encrypt_bytes(&self.key, &plaintext).context("encrypt voiceprint store")?;

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create voiceprint dir: {}", parent.display()))?;
        }

        let tmp_path = temp_path(&self.path);
        let _ = std::fs::remove_file(&tmp_path);
        std::fs::write(&tmp_path, &encrypted)
            .with_context(|| format!("write voiceprint tmp: {}", tmp_path.display()))?;
        std::fs::rename(&tmp_path, &self.path)
            .with_context(|| format!("rename voiceprint store: {}", self.path.display()))?;
        Ok(())
    }
}

fn scaled(embedding: &[f32], factor: f32) -> Vec<f32> {
    embedding.iter().map(|v| v * factor).collect()
}

fn temp_path(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    let name = tmp
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    tmp.set_file_name(format!(".{name}.tmp"));
    tmp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("sw_voiceprint_test");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn new_speaker_is_created_then_matched() {
        let path = temp_store_path("create_match.bin");
        let _ = std::fs::remove_file(&path);
        let mut store = VoicePrintStore::load(&path, [7u8; 32]).unwrap();

        let embedding = vec![1.0, 0.0, 0.0];
        let (id_a, _) = store.match_or_create(&embedding, 0.6, 0.7);
        store.accumulate(&id_a, &embedding, 1);

        let (id_b, _) = store.match_or_create(&vec![0.99, 0.01, 0.0], 0.6, 0.7);
        assert_eq!(id_a, id_b);

        let (id_c, _) = store.match_or_create(&vec![0.0, 0.0, 1.0], 0.6, 0.7);
        assert_ne!(id_a, id_c);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn label_only_returned_above_confidence() {
        let path = temp_store_path("label_conf.bin");
        let _ = std::fs::remove_file(&path);
        let mut store = VoicePrintStore::load(&path, [9u8; 32]).unwrap();

        let embedding = vec![1.0, 0.0];
        let (id, _) = store.match_or_create(&embedding, 0.6, 0.95);
        store.accumulate(&id, &embedding, 1);
        store.rename(&id, Some("Alice".to_string())).unwrap();

        let (_, label_high) = store.match_or_create(&vec![0.999, 0.001], 0.6, 0.95);
        assert_eq!(label_high.as_deref(), Some("Alice"));

        let (_, label_low) = store.match_or_create(&vec![0.7, 0.71], 0.6, 0.95);
        assert_eq!(label_low, None);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn persist_and_reload_roundtrip() {
        let path = temp_store_path("roundtrip.bin");
        let _ = std::fs::remove_file(&path);
        let key = [3u8; 32];
        {
            let mut store = VoicePrintStore::load(&path, key).unwrap();
            let embedding = vec![0.5, 0.5, 0.5];
            let (id, _) = store.match_or_create(&embedding, 0.6, 0.7);
            store.accumulate(&id, &embedding, 4);
            store.rename(&id, Some("Bob".to_string())).unwrap();
            store.persist().unwrap();
        }

        let reloaded = VoicePrintStore::load(&path, key).unwrap();
        assert_eq!(reloaded.prints().len(), 1);
        assert_eq!(reloaded.prints()[0].label.as_deref(), Some("Bob"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn legacy_file_without_label_deserializes() {
        let json = r#"{"prints":[{"speaker_id":"spk_1","centroid_embedding":[1.0,2.0],"sample_count":3}],"next_id":2}"#;
        let parsed: VoicePrintFile = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.prints.len(), 1);
        assert_eq!(parsed.prints[0].label, None);
        assert_eq!(parsed.next_id, 2);
    }
}
