use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sw_crypto::{decrypt_bytes, encrypt_bytes};

use crate::accumulate::TranscriptSegment;

const SESSION_EXT: &str = "swsession";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("io: {0}")]
    Io(String),
    #[error("crypto: {0}")]
    Crypto(String),
    #[error("serde: {0}")]
    Serde(String),
    #[error("invalid session id")]
    InvalidId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatMsg {
    pub id: String,
    pub role: String,
    pub content: String,
    pub status: String,
    #[serde(default)]
    pub parent_message_id: Option<String>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub error_message: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub relay_session_id: String,
    pub started_at: String,
    pub ended_at: String,
    #[serde(default)]
    pub title: Option<String>,
    pub segments: Vec<TranscriptSegment>,
    #[serde(default)]
    pub notes_markdown: Option<String>,
    #[serde(default)]
    pub notes_status: Option<String>,
    #[serde(default)]
    pub notes_error: Option<String>,
    #[serde(default)]
    pub notes_root_message_id: Option<String>,
    #[serde(default)]
    pub chat: Vec<ChatMsg>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    #[serde(default)]
    pub title: Option<String>,
    pub ended_at: String,
    pub has_notes: bool,
    #[serde(default)]
    pub notes_status: Option<String>,
}

pub struct SessionStore {
    dir: PathBuf,
    file_key: [u8; 32],
    lock: Mutex<()>,
}

impl SessionStore {
    pub fn new(dir: PathBuf, file_key: [u8; 32]) -> Result<Self, StoreError> {
        std::fs::create_dir_all(&dir).map_err(|e| StoreError::Io(e.to_string()))?;
        Ok(Self {
            dir,
            file_key,
            lock: Mutex::new(()),
        })
    }

    pub fn save(&self, record: &SessionRecord) -> Result<(), StoreError> {
        let _guard = self.lock.lock().unwrap();
        self.write_raw(record)
    }

    pub fn load(&self, session_id: &str) -> Result<Option<SessionRecord>, StoreError> {
        let _guard = self.lock.lock().unwrap();
        self.read_raw(session_id)
    }

    pub fn list(&self) -> Result<Vec<SessionSummary>, StoreError> {
        let _guard = self.lock.lock().unwrap();
        let mut summaries: Vec<SessionSummary> = Vec::new();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(_) => return Ok(summaries),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(SESSION_EXT) {
                continue;
            }
            let data = match std::fs::read(&path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let plain = match decrypt_bytes(&self.file_key, &data) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let record: SessionRecord = match serde_json::from_slice(&plain) {
                Ok(r) => r,
                Err(_) => continue,
            };
            summaries.push(SessionSummary {
                session_id: record.session_id,
                title: record.title,
                ended_at: record.ended_at,
                has_notes: record.notes_markdown.is_some(),
                notes_status: record.notes_status,
            });
        }
        summaries.sort_by(|a, b| b.ended_at.cmp(&a.ended_at));
        Ok(summaries)
    }

    pub fn append_chat(&self, session_id: &str, msg: ChatMsg) -> Result<(), StoreError> {
        let _guard = self.lock.lock().unwrap();
        let mut record = match self.read_raw(session_id)? {
            Some(r) => r,
            None => return Ok(()),
        };
        if !record.chat.iter().any(|m| m.id == msg.id) {
            record.chat.push(msg);
            self.write_raw(&record)?;
        }
        Ok(())
    }

    pub fn update_chat_status(
        &self,
        session_id: &str,
        message_id: &str,
        status: &str,
        error_message: Option<&str>,
    ) -> Result<(), StoreError> {
        let _guard = self.lock.lock().unwrap();
        let mut record = match self.read_raw(session_id)? {
            Some(r) => r,
            None => return Ok(()),
        };
        if let Some(msg) = record.chat.iter_mut().find(|m| m.id == message_id) {
            msg.status = status.to_string();
            msg.error_message = error_message.map(|s| s.to_string());
        }
        self.write_raw(&record)
    }

    pub fn record_reply(
        &self,
        session_id: &str,
        user_message_id: Option<&str>,
        task_id: &str,
        content: &str,
        status: &str,
        error_code: Option<&str>,
        error_message: Option<&str>,
        created_at: &str,
    ) -> Result<(), StoreError> {
        let _guard = self.lock.lock().unwrap();
        let mut record = match self.read_raw(session_id)? {
            Some(r) => r,
            None => return Ok(()),
        };

        let is_notes = record.notes_root_message_id.is_some()
            && record.notes_root_message_id.as_deref() == user_message_id
            && record.notes_markdown.is_none();

        if is_notes {
            match status {
                "completed" => {
                    record.notes_markdown = Some(content.to_string());
                    record.notes_status = Some("completed".to_string());
                    record.notes_error = None;
                }
                "errored" => {
                    record.notes_status = Some("errored".to_string());
                    record.notes_error = error_message
                        .or(error_code)
                        .map(|s| s.to_string())
                        .or(Some("Assistant returned an error".to_string()));
                }
                "cancelled" | "silent" => {
                    record.notes_status = Some("cancelled".to_string());
                }
                _ => {}
            }
        } else if !record.chat.iter().any(|m| m.id == task_id) {
            record.chat.push(ChatMsg {
                id: task_id.to_string(),
                role: "assistant".to_string(),
                content: content.to_string(),
                status: status.to_string(),
                parent_message_id: user_message_id.map(|s| s.to_string()),
                error_code: error_code.map(|s| s.to_string()),
                error_message: error_message.map(|s| s.to_string()),
                created_at: created_at.to_string(),
            });
        }

        self.write_raw(&record)
    }

    pub fn delete(&self, session_id: &str) -> Result<(), StoreError> {
        let _guard = self.lock.lock().unwrap();
        let path = self.path_for(session_id)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StoreError::Io(e.to_string())),
        }
    }

    fn path_for(&self, session_id: &str) -> Result<PathBuf, StoreError> {
        if session_id.is_empty() || session_id.contains(['/', '\\']) || session_id.contains("..") {
            return Err(StoreError::InvalidId);
        }
        Ok(self.dir.join(format!("{session_id}.{SESSION_EXT}")))
    }

    fn read_raw(&self, session_id: &str) -> Result<Option<SessionRecord>, StoreError> {
        let path = self.path_for(session_id)?;
        if !path.exists() {
            return Ok(None);
        }
        let data = std::fs::read(&path).map_err(|e| StoreError::Io(e.to_string()))?;
        let plain =
            decrypt_bytes(&self.file_key, &data).map_err(|e| StoreError::Crypto(e.to_string()))?;
        let record =
            serde_json::from_slice(&plain).map_err(|e| StoreError::Serde(e.to_string()))?;
        Ok(Some(record))
    }

    fn write_raw(&self, record: &SessionRecord) -> Result<(), StoreError> {
        let path = self.path_for(&record.session_id)?;
        let plain = serde_json::to_vec(record).map_err(|e| StoreError::Serde(e.to_string()))?;
        let encrypted =
            encrypt_bytes(&self.file_key, &plain).map_err(|e| StoreError::Crypto(e.to_string()))?;
        let tmp = path.with_extension(format!("{SESSION_EXT}.tmp"));
        std::fs::write(&tmp, &encrypted).map_err(|e| StoreError::Io(e.to_string()))?;
        std::fs::rename(&tmp, &path).map_err(|e| StoreError::Io(e.to_string()))?;
        Ok(())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

#[cfg(test)]
mod tests {
    use crate::accumulate::TranscriptSource;

    use super::*;

    fn sample(session_id: &str) -> SessionRecord {
        SessionRecord {
            session_id: session_id.to_string(),
            relay_session_id: session_id.to_string(),
            started_at: "2026-05-25T10:00:00Z".to_string(),
            ended_at: "2026-05-25T10:30:00Z".to_string(),
            title: None,
            segments: vec![TranscriptSegment {
                source: TranscriptSource::You,
                utterance: "hello world".to_string(),
            }],
            notes_markdown: None,
            notes_status: Some("pending".to_string()),
            notes_error: None,
            notes_root_message_id: Some("root-1".to_string()),
            chat: vec![],
        }
    }

    fn temp_store(key: [u8; 32], tag: &str) -> SessionStore {
        let dir = std::env::temp_dir().join(format!("sw_notes{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        SessionStore::new(dir, key).unwrap()
    }

    #[test]
    fn save_load_roundtrip_encrypted() {
        let store = temp_store([0x11; 32], "roundtrip");
        let rec = sample("sess-a");
        store.save(&rec).unwrap();

        let raw = std::fs::read(store.dir().join("sess-a.swsession")).unwrap();
        assert!(
            !raw.windows(5).any(|w| w == b"hello"),
            "on-disk bytes must be encrypted"
        );

        let loaded = store.load("sess-a").unwrap().unwrap();
        assert_eq!(loaded.segments[0].utterance, "hello world");
        assert_eq!(loaded.notes_root_message_id.as_deref(), Some("root-1"));
    }

    #[test]
    fn delete_removes_file_and_is_idempotent() {
        let store = temp_store([0x55; 32], "delete");
        store.save(&sample("sess-d")).unwrap();
        assert!(store.load("sess-d").unwrap().is_some());

        store.delete("sess-d").unwrap();
        assert!(store.load("sess-d").unwrap().is_none());
        assert!(!store.dir().join("sess-d.swsession").exists());

        store.delete("sess-d").unwrap();
        assert!(store.delete("../escape").is_err());
    }

    #[test]
    fn wrong_key_cannot_decrypt() {
        let dir = std::env::temp_dir().join(format!("sw_notes{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = SessionStore::new(dir.clone(), [0x22; 32]).unwrap();
        store.save(&sample("sess-b")).unwrap();

        let other = SessionStore::new(dir, [0x33; 32]).unwrap();
        assert!(other.load("sess-b").is_err());
    }

    #[test]
    fn record_reply_sets_notes_then_appends_chat() {
        let store = temp_store([0x44; 32], "reply");
        store.save(&sample("sess-c")).unwrap();

        store
            .record_reply(
                "sess-c",
                Some("root-1"),
                "task-1",
                "# Notes",
                "completed",
                None,
                None,
                "2026-05-25T10:31:00Z",
            )
            .unwrap();
        let after_notes = store.load("sess-c").unwrap().unwrap();
        assert_eq!(after_notes.notes_markdown.as_deref(), Some("# Notes"));
        assert_eq!(after_notes.notes_status.as_deref(), Some("completed"));
        assert!(after_notes.chat.is_empty());

        store
            .record_reply(
                "sess-c",
                Some("umsg-2"),
                "task-2",
                "follow up reply",
                "completed",
                None,
                None,
                "2026-05-25T10:32:00Z",
            )
            .unwrap();
        let after_chat = store.load("sess-c").unwrap().unwrap();
        assert_eq!(after_chat.chat.len(), 1);
        assert_eq!(after_chat.chat[0].content, "follow up reply");
        assert_eq!(
            after_chat.chat[0].parent_message_id.as_deref(),
            Some("umsg-2")
        );
    }

    #[test]
    fn update_chat_status_marks_pending_message() {
        let store = temp_store([0x77; 32], "status");
        store.save(&sample("sess-st")).unwrap();
        store
            .append_chat(
                "sess-st",
                ChatMsg {
                    id: "umsg-1".to_string(),
                    role: "user".to_string(),
                    content: "hello".to_string(),
                    status: "pending".to_string(),
                    parent_message_id: None,
                    error_code: None,
                    error_message: None,
                    created_at: "2026-05-25T10:31:00Z".to_string(),
                },
            )
            .unwrap();

        store
            .update_chat_status("sess-st", "umsg-1", "errored", Some("relay rejected"))
            .unwrap();
        let after = store.load("sess-st").unwrap().unwrap();
        assert_eq!(after.chat[0].status, "errored");
        assert_eq!(
            after.chat[0].error_message.as_deref(),
            Some("relay rejected")
        );

        store
            .update_chat_status("sess-st", "umsg-1", "completed", None)
            .unwrap();
        let after = store.load("sess-st").unwrap().unwrap();
        assert_eq!(after.chat[0].status, "completed");
        assert_eq!(after.chat[0].error_message, None);
    }

    #[test]
    fn list_sorts_by_ended_at_desc() {
        let store = temp_store([0x55; 32], "list");
        let mut a = sample("sess-old");
        a.ended_at = "2026-05-20T10:00:00Z".to_string();
        let mut b = sample("sess-new");
        b.ended_at = "2026-05-25T10:00:00Z".to_string();
        store.save(&a).unwrap();
        store.save(&b).unwrap();
        let list = store.list().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].session_id, "sess-new");
    }

    #[test]
    fn rejects_path_traversal_ids() {
        let store = temp_store([0x66; 32], "traversal");
        assert!(store.load("../etc/passwd").is_err());
    }
}
