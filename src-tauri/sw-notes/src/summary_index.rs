use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sw_crypto::{decrypt_bytes, encrypt_bytes};

use crate::store::{
    unique_tmp_path, write_via_tmp, InsightNote, SessionRecord, SessionStore, SessionSummary,
    SESSION_EXT,
};

pub(crate) const INDEX_FILE: &str = "summary-index.swindex";
const INDEX_VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
struct IndexEntry {
    file_id: String,
    len: u64,
    modified_ms: u64,
    summary: SessionSummary,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    insight_feedback_unpublished: bool,
}

struct Summarized {
    summary: SessionSummary,
    insight_feedback_unpublished: bool,
}

#[derive(Serialize)]
struct IndexOut<'a> {
    version: u32,
    entries: &'a [IndexEntry],
}

#[derive(Deserialize)]
struct IndexIn {
    version: u32,
    entries: serde_json::Value,
}

fn summarize(record: SessionRecord) -> Summarized {
    let insight_feedback_unpublished = record.backend_session_id.is_some()
        && record
            .insights
            .iter()
            .any(InsightNote::feedback_unpublished);
    Summarized {
        summary: SessionSummary {
            session_id: record.session_id,
            title: record.title,
            ended_at: record.ended_at,
            has_notes: record.notes_markdown.is_some(),
            notes_status: record.notes_status,
            outcome: record.outcome,
        },
        insight_feedback_unpublished,
    }
}

impl SessionStore {
    pub(crate) fn summaries_via_index(&self) -> Vec<SessionSummary> {
        let mut summaries: Vec<SessionSummary> = self
            .refreshed_index_entries()
            .into_iter()
            .map(|entry| entry.summary)
            .collect();
        summaries.sort_by(|a, b| b.ended_at.cmp(&a.ended_at));
        summaries
    }

    pub(crate) fn sessions_with_unpublished_insight_feedback(&self) -> Vec<String> {
        self.refreshed_index_entries()
            .into_iter()
            .filter(|entry| entry.insight_feedback_unpublished)
            .map(|entry| entry.summary.session_id)
            .collect()
    }

    fn refreshed_index_entries(&self) -> Vec<IndexEntry> {
        let loaded = self.load_index();
        let index_was_usable = loaded.is_some();
        let mut known: HashMap<String, IndexEntry> = loaded
            .unwrap_or_default()
            .into_iter()
            .map(|entry| (entry.file_id.clone(), entry))
            .collect();

        let stats = self.session_file_stats();
        let mut entries: Vec<IndexEntry> = Vec::with_capacity(stats.len());
        let mut changed = !index_was_usable;
        for stat in stats {
            let reusable = known
                .remove(&stat.session_id)
                .filter(|entry| entry.len == stat.len && entry.modified_ms == stat.modified_ms);
            if let Some(entry) = reusable {
                entries.push(entry);
                continue;
            }
            changed = true;
            if let Some(summarized) = self.summarize_file(&stat.session_id) {
                entries.push(IndexEntry {
                    file_id: stat.session_id,
                    len: stat.len,
                    modified_ms: stat.modified_ms,
                    summary: summarized.summary,
                    insight_feedback_unpublished: summarized.insight_feedback_unpublished,
                });
            }
        }
        changed = changed || !known.is_empty();
        if changed {
            self.store_index(&entries);
        }
        entries
    }

    fn summarize_file(&self, file_id: &str) -> Option<Summarized> {
        let path = self.dir().join(format!("{file_id}.{SESSION_EXT}"));
        let data = std::fs::read(path).ok()?;
        #[cfg(test)]
        self.decrypted_session_files
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let plain = decrypt_bytes(&self.file_key, &data).ok()?;
        let record: SessionRecord = serde_json::from_slice(&plain).ok()?;
        Some(summarize(record))
    }

    fn load_index(&self) -> Option<Vec<IndexEntry>> {
        let data = std::fs::read(self.dir().join(INDEX_FILE)).ok()?;
        let plain = decrypt_bytes(&self.file_key, &data).ok()?;
        let parsed: IndexIn = serde_json::from_slice(&plain).ok()?;
        if parsed.version != INDEX_VERSION {
            return None;
        }
        serde_json::from_value(parsed.entries).ok()
    }

    fn store_index(&self, entries: &[IndexEntry]) {
        let out = IndexOut {
            version: INDEX_VERSION,
            entries,
        };
        let Ok(plain) = serde_json::to_vec(&out) else {
            return;
        };
        let Ok(encrypted) = encrypt_bytes(&self.file_key, &plain) else {
            return;
        };
        let path = self.dir().join(INDEX_FILE);
        let tmp = unique_tmp_path(&path);
        let _ = write_via_tmp(&tmp, &path, &encrypted);
    }
}
