use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sw_crypto::{decrypt_bytes, encrypt_bytes};
use uuid::Uuid;

use crate::accumulate::{TranscriptSegment, TranscriptSource};
use crate::blocks::BlockSummary;

const SESSION_EXT: &str = "swsession";
const LIVE_EXT: &str = "swlive";

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
    #[error("session is live")]
    LiveSessionActive,
    #[error("notes are still generating")]
    NotesPending,
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

impl ChatMsg {
    fn has_same_payload_as(&self, other: &ChatMsg) -> bool {
        self.role == other.role
            && self.content == other.content
            && self.status == other.status
            && self.parent_message_id == other.parent_message_id
            && self.error_code == other.error_code
            && self.error_message == other.error_message
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatAppendOutcome {
    Inserted,
    IdenticalDuplicate,
    ConflictingDuplicate,
    MissingSession,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InsightNote {
    pub offset_ms: u64,
    pub severity: String,
    pub message: String,
    #[serde(default)]
    pub segment_index: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub relay_session_id: String,
    pub started_at: String,
    pub ended_at: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub title_is_auto: bool,
    pub segments: Vec<TranscriptSegment>,
    #[serde(default)]
    pub insights: Vec<InsightNote>,
    #[serde(default)]
    pub blocks: Vec<BlockSummary>,
    #[serde(default)]
    pub notes_markdown: Option<String>,
    #[serde(default)]
    pub notes_status: Option<String>,
    #[serde(default)]
    pub notes_error: Option<String>,
    #[serde(default)]
    pub notes_root_message_id: Option<String>,
    #[serde(default)]
    pub notes_pending_root_message_id: Option<String>,
    #[serde(default)]
    pub chat: Vec<ChatMsg>,
    #[serde(default)]
    pub attendees: Vec<String>,
    #[serde(default)]
    pub calendar_event_id: Option<String>,
    #[serde(default)]
    pub playbook: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LiveSessionRecord {
    pub session_id: String,
    pub started_at_ms: u64,
    #[serde(default)]
    pub updated_at_ms: u64,
    #[serde(default)]
    pub segments: Vec<TranscriptSegment>,
    #[serde(default)]
    pub insights: Vec<InsightNote>,
    #[serde(default)]
    pub blocks: Vec<BlockSummary>,
    #[serde(default)]
    pub attendees: Vec<String>,
    #[serde(default)]
    pub calendar_event_id: Option<String>,
    #[serde(default)]
    pub playbook: Option<String>,
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

pub fn derive_title(record: &SessionRecord) -> String {
    if let Some(notes) = record.notes_markdown.as_deref() {
        if let Some(line) = first_meaningful_line(notes) {
            return line;
        }
    }
    let date = record
        .ended_at
        .split('T')
        .next()
        .unwrap_or(&record.ended_at);
    if !record.attendees.is_empty() {
        let people = record.attendees.join(", ");
        if date.is_empty() {
            return people;
        }
        return format!("{people} ({date})");
    }
    if !date.is_empty() {
        return date.to_string();
    }
    record.ended_at.clone()
}

const MAX_TITLE_LEN: usize = 80;

fn first_meaningful_line(markdown: &str) -> Option<String> {
    for raw_line in markdown.lines() {
        let stripped = raw_line.trim_start_matches('#').trim();
        if stripped.is_empty() {
            continue;
        }
        return Some(clean_title(stripped));
    }
    None
}

fn clean_title(line: &str) -> String {
    let unwrapped = line
        .trim_matches(|c| matches!(c, '*' | '_' | '`' | '"' | '#'))
        .trim();
    let collapsed = unwrapped.split_whitespace().collect::<Vec<_>>().join(" ");
    cap_title(&collapsed)
}

fn cap_title(title: &str) -> String {
    if title.chars().count() <= MAX_TITLE_LEN {
        return title.to_string();
    }
    let truncated: String = title.chars().take(MAX_TITLE_LEN).collect();
    let head = match truncated.rfind(' ') {
        Some(idx) if idx >= MAX_TITLE_LEN / 2 => truncated[..idx].to_string(),
        _ => truncated,
    };
    format!("{}…", head.trim_end())
}

fn blocks_still_describe(
    blocks: &[BlockSummary],
    previous: &[TranscriptSegment],
    replacement: &[TranscriptSegment],
) -> bool {
    let covered = crate::blocks::covered_to(blocks);
    if covered == 0 {
        return true;
    }
    covered <= previous.len()
        && covered <= replacement.len()
        && previous[..covered]
            .iter()
            .zip(&replacement[..covered])
            .all(|(before, after)| {
                before.source == after.source && before.utterance == after.utterance
            })
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

    pub fn append_chat(
        &self,
        session_id: &str,
        msg: ChatMsg,
    ) -> Result<ChatAppendOutcome, StoreError> {
        let _guard = self.lock.lock().unwrap();
        let mut record = match self.read_raw(session_id)? {
            Some(r) => r,
            None => return Ok(ChatAppendOutcome::MissingSession),
        };
        if let Some(existing) = record.chat.iter().find(|m| m.id == msg.id) {
            return Ok(if existing.has_same_payload_as(&msg) {
                ChatAppendOutcome::IdenticalDuplicate
            } else {
                ChatAppendOutcome::ConflictingDuplicate
            });
        }
        record.chat.push(msg);
        self.write_raw(&record)?;
        Ok(ChatAppendOutcome::Inserted)
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

        let is_pending_regeneration = record.notes_pending_root_message_id.is_some()
            && record.notes_pending_root_message_id.as_deref() == user_message_id;
        let is_first_notes_attempt = !is_pending_regeneration
            && record.notes_root_message_id.is_some()
            && record.notes_root_message_id.as_deref() == user_message_id
            && record.notes_markdown.is_none();
        let is_notes = is_pending_regeneration || is_first_notes_attempt;

        if is_notes {
            match status {
                "completed" => {
                    if let Ok(Some(live)) = self.read_live_raw(session_id) {
                        crate::insights::append_missing_insights(
                            &mut record.insights,
                            &live.insights,
                        );
                    }
                    if is_pending_regeneration {
                        let superseded_root = record.notes_root_message_id.take();
                        record.chat.retain(|m| {
                            m.parent_message_id.as_deref() != superseded_root.as_deref()
                        });
                        record.notes_root_message_id = record.notes_pending_root_message_id.take();
                    } else {
                        record.notes_pending_root_message_id = None;
                    }
                    record.notes_markdown = Some(content.to_string());
                    record.notes_status = Some("completed".to_string());
                    record.notes_error = None;
                    if !record.chat.iter().any(|m| m.id == task_id) {
                        record.chat.push(ChatMsg {
                            id: task_id.to_string(),
                            role: "assistant".to_string(),
                            content: content.to_string(),
                            status: "completed".to_string(),
                            parent_message_id: user_message_id.map(|s| s.to_string()),
                            error_code: None,
                            error_message: None,
                            created_at: created_at.to_string(),
                        });
                    }
                }
                "errored" => {
                    record.notes_pending_root_message_id = None;
                    if !is_pending_regeneration {
                        record.notes_status = Some("errored".to_string());
                        record.notes_error = error_message
                            .or(error_code)
                            .map(|s| s.to_string())
                            .or(Some("Assistant returned an error".to_string()));
                    }
                }
                "cancelled" | "silent" => {
                    record.notes_pending_root_message_id = None;
                    if !is_pending_regeneration {
                        record.notes_status = Some("cancelled".to_string());
                    }
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

    pub fn update_title(&self, session_id: &str, title: Option<String>) -> Result<(), StoreError> {
        let _guard = self.lock.lock().unwrap();
        self.ensure_not_live(session_id)?;
        let Some(mut record) = self.read_raw(session_id)? else {
            return Ok(());
        };
        record.title = title
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        record.title_is_auto = false;
        self.write_raw(&record)
    }

    pub fn apply_auto_title(&self, session_id: &str) -> Result<bool, StoreError> {
        let _guard = self.lock.lock().unwrap();
        let Some(mut record) = self.read_raw(session_id)? else {
            return Ok(false);
        };
        if record.title.is_some() || record.notes_markdown.is_none() {
            return Ok(false);
        }
        record.title = Some(derive_title(&record));
        record.title_is_auto = true;
        self.write_raw(&record)?;
        Ok(true)
    }

    pub fn replace_segments(
        &self,
        session_id: &str,
        segments: Vec<TranscriptSegment>,
    ) -> Result<(), StoreError> {
        let _guard = self.lock.lock().unwrap();
        self.ensure_not_live(session_id)?;
        let Some(mut record) = self.read_raw(session_id)? else {
            return Ok(());
        };
        Self::ensure_notes_settled(&record)?;
        if record.title_is_auto {
            record.title = None;
            record.title_is_auto = false;
        }
        record.segments = segments;
        record.insights = Vec::new();
        record.blocks = Vec::new();
        record.notes_markdown = None;
        record.notes_status = None;
        record.notes_error = None;
        self.write_raw(&record)
    }

    pub fn split(&self, session_id: &str, at_index: usize) -> Result<(String, String), StoreError> {
        let _guard = self.lock.lock().unwrap();
        self.ensure_not_live(session_id)?;
        let Some(record) = self.read_raw(session_id)? else {
            return Err(StoreError::InvalidId);
        };
        Self::ensure_notes_settled(&record)?;
        if at_index == 0 || at_index >= record.segments.len() {
            return Err(StoreError::InvalidId);
        }

        let mut segments_a = record.segments.clone();
        let segments_b = segments_a.split_off(at_index);
        let (insights_a, insights_b) = crate::insights::split_insights(&record.insights, at_index);
        let (blocks_a, blocks_b) = crate::blocks::split_blocks(&record.blocks, at_index);

        let id_a = Uuid::new_v4().to_string();
        let id_b = Uuid::new_v4().to_string();

        let inherited_title = if record.title_is_auto {
            None
        } else {
            record.title.clone()
        };

        let part_a = SessionRecord {
            session_id: id_a.clone(),
            relay_session_id: id_a.clone(),
            started_at: record.started_at.clone(),
            ended_at: record.ended_at.clone(),
            title: inherited_title.clone(),
            title_is_auto: false,
            segments: segments_a,
            insights: insights_a,
            blocks: blocks_a,
            notes_markdown: None,
            notes_status: None,
            notes_error: None,
            notes_root_message_id: record.notes_root_message_id.clone(),
            notes_pending_root_message_id: None,
            chat: record.chat.clone(),
            attendees: record.attendees.clone(),
            calendar_event_id: record.calendar_event_id.clone(),
            playbook: record.playbook.clone(),
        };
        let part_b = SessionRecord {
            session_id: id_b.clone(),
            relay_session_id: id_b.clone(),
            started_at: record.started_at.clone(),
            ended_at: record.ended_at.clone(),
            title: inherited_title,
            title_is_auto: false,
            segments: segments_b,
            insights: insights_b,
            blocks: blocks_b,
            notes_markdown: None,
            notes_status: None,
            notes_error: None,
            notes_root_message_id: None,
            notes_pending_root_message_id: None,
            chat: Vec::new(),
            attendees: record.attendees.clone(),
            calendar_event_id: record.calendar_event_id.clone(),
            playbook: record.playbook.clone(),
        };

        self.write_raw(&part_a)?;
        if let Err(err) = self.write_raw(&part_b) {
            let _ = self.remove_session_file(&id_a);
            return Err(err);
        }
        if let Err(err) = self.remove_session_file(session_id) {
            eprintln!("[store] split left original {session_id} after writing children: {err}");
        }
        Ok((id_a, id_b))
    }

    pub fn merge(&self, id_a: &str, id_b: &str) -> Result<String, StoreError> {
        if id_a == id_b {
            return Err(StoreError::InvalidId);
        }
        let _guard = self.lock.lock().unwrap();
        self.ensure_not_live(id_a)?;
        self.ensure_not_live(id_b)?;
        let Some(first) = self.read_raw(id_a)? else {
            return Err(StoreError::InvalidId);
        };
        let Some(second) = self.read_raw(id_b)? else {
            return Err(StoreError::InvalidId);
        };
        Self::ensure_notes_settled(&first)?;
        Self::ensure_notes_settled(&second)?;
        let (first, second) = if first.started_at <= second.started_at {
            (first, second)
        } else {
            (second, first)
        };

        let merged_id = Uuid::new_v4().to_string();
        let mut segments = first.segments.clone();
        segments.extend(second.segments.iter().cloned());
        let insights = crate::insights::merge_insights(
            &first.insights,
            &second.insights,
            first.segments.len(),
            crate::insights::rfc3339_delta_ms(&first.started_at, &second.started_at),
        );

        let mut attendees = first.attendees.clone();
        for attendee in &second.attendees {
            if !attendees.contains(attendee) {
                attendees.push(attendee.clone());
            }
        }

        let started_at = first.started_at.min(second.started_at.clone());
        let ended_at = first.ended_at.max(second.ended_at.clone());

        let mut chat = first.chat.clone();
        chat.extend(second.chat.iter().cloned());

        let notes_markdown = match (first.notes_markdown.clone(), second.notes_markdown.clone()) {
            (Some(a), Some(b)) => Some(format!("{a}\n\n---\n\n{b}")),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };

        let merged = SessionRecord {
            session_id: merged_id.clone(),
            relay_session_id: merged_id.clone(),
            started_at,
            ended_at,
            title: first.title.clone().or_else(|| second.title.clone()),
            title_is_auto: if first.title.is_some() {
                first.title_is_auto
            } else {
                second.title_is_auto
            },
            segments,
            insights,
            blocks: crate::blocks::merge_blocks(
                &first.blocks,
                &second.blocks,
                first.segments.len(),
            ),
            notes_markdown,
            notes_status: first
                .notes_status
                .clone()
                .or_else(|| second.notes_status.clone()),
            notes_error: first
                .notes_error
                .clone()
                .or_else(|| second.notes_error.clone()),
            notes_root_message_id: first
                .notes_root_message_id
                .clone()
                .or_else(|| second.notes_root_message_id.clone()),
            notes_pending_root_message_id: None,
            chat,
            attendees,
            calendar_event_id: first
                .calendar_event_id
                .clone()
                .or_else(|| second.calendar_event_id.clone()),
            playbook: Self::merge_playbook(first.playbook.as_deref(), second.playbook.as_deref()),
        };

        self.write_raw(&merged)?;
        if let Err(err) = self.remove_session_file(id_a) {
            eprintln!("[store] merge left input {id_a} after committing merge: {err}");
        }
        if let Err(err) = self.remove_session_file(id_b) {
            eprintln!("[store] merge left input {id_b} after committing merge: {err}");
        }
        Ok(merged_id)
    }

    fn ensure_not_live(&self, session_id: &str) -> Result<(), StoreError> {
        let path = self.live_path_for(session_id)?;
        if path.exists() {
            return Err(StoreError::LiveSessionActive);
        }
        Ok(())
    }

    fn ensure_notes_settled(record: &SessionRecord) -> Result<(), StoreError> {
        if record.notes_status.as_deref() == Some("pending")
            || record.notes_pending_root_message_id.is_some()
        {
            return Err(StoreError::NotesPending);
        }
        Ok(())
    }

    fn merge_playbook(first: Option<&str>, second: Option<&str>) -> Option<String> {
        match (first, second) {
            (None, None) => None,
            (Some(value), None) | (None, Some(value)) => Some(value.to_string()),
            (Some(a), Some(b)) => (a.trim() == b.trim()).then(|| a.to_string()),
        }
    }

    fn remove_session_file(&self, session_id: &str) -> Result<(), StoreError> {
        let path = self.path_for(session_id)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StoreError::Io(e.to_string())),
        }
    }

    pub fn live_begin(&self, session_id: &str, started_at_ms: u64) -> Result<(), StoreError> {
        let _guard = self.lock.lock().unwrap();
        if self.read_live_raw(session_id)?.is_some() {
            return Ok(());
        }
        self.write_live_raw(&LiveSessionRecord {
            session_id: session_id.to_string(),
            started_at_ms,
            updated_at_ms: started_at_ms,
            segments: Vec::new(),
            insights: Vec::new(),
            blocks: Vec::new(),
            attendees: Vec::new(),
            calendar_event_id: None,
            playbook: None,
        })
    }

    pub fn live_set_playbook(
        &self,
        session_id: &str,
        playbook: Option<String>,
    ) -> Result<(), StoreError> {
        let _guard = self.lock.lock().unwrap();
        let Some(mut record) = self.read_live_raw(session_id)? else {
            return Ok(());
        };
        record.playbook = playbook;
        self.write_live_raw(&record)
    }

    pub fn live_append_block(
        &self,
        session_id: &str,
        start_index: usize,
        end_index: usize,
        summary: &str,
        now_ms: u64,
    ) -> Result<bool, StoreError> {
        let trimmed = summary.trim();
        if trimmed.is_empty() || end_index <= start_index {
            return Ok(false);
        }
        let _guard = self.lock.lock().unwrap();
        let Some(mut record) = self.read_live_raw(session_id)? else {
            return Ok(false);
        };
        if record
            .blocks
            .iter()
            .any(|block| block.end_index >= end_index)
        {
            return Ok(false);
        }
        record.blocks.push(BlockSummary {
            start_index,
            end_index,
            summary: crate::blocks::clamp_block_summary(trimmed),
            created_at_ms: now_ms,
        });
        record.updated_at_ms = now_ms;
        self.write_live_raw(&record)?;
        Ok(true)
    }

    pub fn live_load(&self, session_id: &str) -> Result<Option<LiveSessionRecord>, StoreError> {
        let _guard = self.lock.lock().unwrap();
        self.read_live_raw(session_id)
    }

    pub fn live_append_segment(
        &self,
        session_id: &str,
        source: TranscriptSource,
        utterance: &str,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let trimmed = utterance.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        let _guard = self.lock.lock().unwrap();
        let Some(mut record) = self.read_live_raw(session_id)? else {
            return Ok(());
        };
        record.segments.push(TranscriptSegment {
            source,
            utterance: trimmed.to_string(),
            speaker_id: None,
            speaker_label: None,
        });
        record.updated_at_ms = now_ms;
        self.write_live_raw(&record)
    }

    pub fn live_append_insight(
        &self,
        session_id: &str,
        severity: &str,
        message: &str,
        segment_index: Option<usize>,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let trimmed = message.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        let _guard = self.lock.lock().unwrap();
        let Some(record) = self.read_live_raw(session_id)? else {
            let Some(session) = self.read_raw(session_id)? else {
                return Ok(());
            };
            return self.push_insight_session_locked(
                session,
                severity,
                trimmed,
                segment_index,
                now_ms,
            );
        };
        self.push_insight_locked(record, severity, trimmed, segment_index, now_ms)
    }

    fn push_insight_locked(
        &self,
        mut record: LiveSessionRecord,
        severity: &str,
        trimmed: &str,
        segment_index: Option<usize>,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        if record
            .insights
            .last()
            .is_some_and(|last| last.message == trimmed)
        {
            return Ok(());
        }
        record.insights.push(InsightNote {
            offset_ms: now_ms.saturating_sub(record.started_at_ms),
            severity: severity.to_string(),
            message: trimmed.to_string(),
            segment_index,
        });
        record.updated_at_ms = now_ms;
        self.write_live_raw(&record)
    }

    fn push_insight_session_locked(
        &self,
        mut record: SessionRecord,
        severity: &str,
        trimmed: &str,
        segment_index: Option<usize>,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        if record
            .insights
            .last()
            .is_some_and(|last| last.message == trimmed)
        {
            return Ok(());
        }
        let started_ms = crate::insights::rfc3339_epoch_ms(&record.started_at).unwrap_or(now_ms);
        record.insights.push(InsightNote {
            offset_ms: now_ms.saturating_sub(started_ms),
            severity: severity.to_string(),
            message: trimmed.to_string(),
            segment_index,
        });
        Self::refresh_notes_markdown(&mut record);
        self.write_raw(&record)
    }

    fn refresh_notes_markdown(record: &mut SessionRecord) {
        if record.notes_markdown.is_none() {
            return;
        }
        let Some(base) = record
            .chat
            .iter()
            .find(|m| {
                m.role == "assistant"
                    && m.status == "completed"
                    && m.parent_message_id.is_some()
                    && m.parent_message_id == record.notes_root_message_id
            })
            .map(|m| m.content.clone())
        else {
            return;
        };
        record.notes_markdown = Some(base);
    }

    pub fn live_merge_insights_then_delete(&self, session_id: &str) -> Result<(), StoreError> {
        let _guard = self.lock.lock().unwrap();
        let Some(mut session) = self.read_raw(session_id)? else {
            return Ok(());
        };
        if let Some(live) = self.read_live_raw(session_id)? {
            if crate::insights::append_missing_insights(&mut session.insights, &live.insights) {
                Self::refresh_notes_markdown(&mut session);
                self.write_raw(&session)?;
            }
        }
        let path = self.live_path_for(session_id)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StoreError::Io(e.to_string())),
        }
    }

    pub fn live_replace_segments(
        &self,
        session_id: &str,
        started_at_ms: u64,
        segments: Vec<TranscriptSegment>,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let _guard = self.lock.lock().unwrap();
        let mut record =
            self.read_live_raw(session_id)
                .ok()
                .flatten()
                .unwrap_or(LiveSessionRecord {
                    session_id: session_id.to_string(),
                    started_at_ms,
                    updated_at_ms: now_ms,
                    segments: Vec::new(),
                    insights: Vec::new(),
                    blocks: Vec::new(),
                    attendees: Vec::new(),
                    calendar_event_id: None,
                    playbook: None,
                });
        if !blocks_still_describe(&record.blocks, &record.segments, &segments) {
            record.blocks.clear();
        }
        record.segments = segments;
        record.updated_at_ms = now_ms;
        self.write_live_raw(&record)
    }

    pub fn live_set_participants(
        &self,
        session_id: &str,
        attendees: Vec<String>,
        calendar_event_id: Option<String>,
    ) -> Result<(), StoreError> {
        let _guard = self.lock.lock().unwrap();
        let Some(mut record) = self.read_live_raw(session_id)? else {
            return Ok(());
        };
        record.attendees = attendees;
        record.calendar_event_id = calendar_event_id;
        self.write_live_raw(&record)
    }

    pub fn live_list(&self) -> Result<Vec<LiveSessionRecord>, StoreError> {
        let _guard = self.lock.lock().unwrap();
        let mut records: Vec<LiveSessionRecord> = Vec::new();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(_) => return Ok(records),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(LIVE_EXT) {
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
            if let Ok(record) = serde_json::from_slice::<LiveSessionRecord>(&plain) {
                records.push(record);
            }
        }
        Ok(records)
    }

    pub fn live_delete(&self, session_id: &str) -> Result<(), StoreError> {
        let _guard = self.lock.lock().unwrap();
        let path = self.live_path_for(session_id)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StoreError::Io(e.to_string())),
        }
    }

    fn read_live_raw(&self, session_id: &str) -> Result<Option<LiveSessionRecord>, StoreError> {
        let path = self.live_path_for(session_id)?;
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

    fn write_live_raw(&self, record: &LiveSessionRecord) -> Result<(), StoreError> {
        let path = self.live_path_for(&record.session_id)?;
        let plain = serde_json::to_vec(record).map_err(|e| StoreError::Serde(e.to_string()))?;
        let encrypted =
            encrypt_bytes(&self.file_key, &plain).map_err(|e| StoreError::Crypto(e.to_string()))?;
        let tmp = unique_tmp_path(&path);
        write_via_tmp(&tmp, &path, &encrypted)
    }

    fn live_path_for(&self, session_id: &str) -> Result<PathBuf, StoreError> {
        if session_id.is_empty() || session_id.contains(['/', '\\']) || session_id.contains("..") {
            return Err(StoreError::InvalidId);
        }
        Ok(self.dir.join(format!("{session_id}.{LIVE_EXT}")))
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
        let tmp = unique_tmp_path(&path);
        write_via_tmp(&tmp, &path, &encrypted)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

fn unique_tmp_path(path: &Path) -> PathBuf {
    let mut tmp_os = path.as_os_str().to_owned();
    tmp_os.push(format!(".tmp.{}.{}", std::process::id(), Uuid::new_v4()));
    PathBuf::from(tmp_os)
}

fn write_via_tmp(tmp: &Path, path: &Path, contents: &[u8]) -> Result<(), StoreError> {
    let result = std::fs::write(tmp, contents)
        .map_err(|e| StoreError::Io(e.to_string()))
        .and_then(|_| std::fs::rename(tmp, path).map_err(|e| StoreError::Io(e.to_string())));
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
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
            title_is_auto: false,
            segments: vec![TranscriptSegment {
                source: TranscriptSource::You,
                utterance: "hello world".to_string(),
                speaker_id: None,
                speaker_label: None,
            }],
            insights: vec![],
            blocks: vec![],
            notes_markdown: None,
            notes_status: Some("pending".to_string()),
            notes_error: None,
            notes_root_message_id: Some("root-1".to_string()),
            notes_pending_root_message_id: None,
            chat: vec![],
            attendees: vec![],
            calendar_event_id: None,
            playbook: None,
        }
    }

    #[test]
    fn save_removes_the_leftover_temp_file_when_rename_fails() {
        let store = temp_store([0x91; 32], "write_raw_cleanup");
        let record = sample("sess-cleanup");
        let dest = store.path_for(&record.session_id).unwrap();
        std::fs::create_dir_all(&dest).unwrap();

        let result = store.save(&record);

        assert!(result.is_err());
        let leftovers: Vec<_> = std::fs::read_dir(store.dir())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|p| p.to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty());
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn unique_tmp_path_never_returns_the_same_path_twice_for_the_same_destination() {
        let base = PathBuf::from("/does/not/matter/sess-1.swsession");
        let mut seen = std::collections::HashSet::new();
        for _ in 0..64 {
            assert!(seen.insert(unique_tmp_path(&base)));
        }
    }

    fn temp_store(key: [u8; 32], tag: &str) -> SessionStore {
        let dir = std::env::temp_dir().join(format!("sw_notes{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        SessionStore::new(dir, key).unwrap()
    }

    #[test]
    fn live_insights_persist_on_record_not_in_notes() {
        let store = temp_store([0x77; 32], "insights_flow");
        store.live_begin("sess-i", 1_000).unwrap();
        store
            .live_append_segment(
                "sess-i",
                TranscriptSource::Others,
                "we use spreadsheets",
                5_000,
            )
            .unwrap();
        store
            .live_append_insight(
                "sess-i",
                "green",
                "Ask about their process cost",
                Some(0),
                61_000,
            )
            .unwrap();
        store
            .live_append_insight(
                "sess-i",
                "green",
                "Ask about their process cost",
                Some(0),
                62_000,
            )
            .unwrap();

        let live = store.live_load("sess-i").unwrap().unwrap();
        assert_eq!(live.insights.len(), 1);
        assert_eq!(live.insights[0].offset_ms, 60_000);
        assert_eq!(live.insights[0].segment_index, Some(0));

        let mut rec = sample("sess-i");
        rec.insights = live.insights;
        store.save(&rec).unwrap();
        store
            .record_reply(
                "sess-i",
                Some("root-1"),
                "task-1",
                "# Call Notes\n\nOverview.",
                "completed",
                None,
                None,
                "2026-05-25T10:31:00Z",
            )
            .unwrap();

        let saved = store.load("sess-i").unwrap().unwrap();
        assert_eq!(
            saved.notes_markdown.as_deref(),
            Some("# Call Notes\n\nOverview.")
        );
        assert_eq!(saved.insights.len(), 1);
        assert_eq!(saved.insights[0].message, "Ask about their process cost");
    }

    #[test]
    fn live_set_playbook_round_trips_through_store() {
        let store = temp_store([0x81; 32], "playbook_roundtrip");
        store.live_begin("sess-pb", 1_000).unwrap();
        store
            .live_set_playbook("sess-pb", Some("Ask about budget".to_string()))
            .unwrap();

        let live = store.live_load("sess-pb").unwrap().unwrap();
        assert_eq!(live.playbook.as_deref(), Some("Ask about budget"));
    }

    #[test]
    fn live_session_record_without_playbook_field_loads_as_none() {
        let json = r#"{"session_id":"sess-old","started_at_ms":1000}"#;
        let record: LiveSessionRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.playbook, None);
    }

    #[test]
    fn session_record_without_playbook_field_loads_as_none() {
        let json = r#"{
            "session_id": "sess-old",
            "relay_session_id": "sess-old",
            "started_at": "2026-05-25T10:00:00Z",
            "ended_at": "2026-05-25T10:30:00Z",
            "segments": []
        }"#;
        let record: SessionRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.playbook, None);
    }

    #[test]
    fn replace_segments_clears_insights() {
        let store = temp_store([0x7b; 32], "insights_replace");
        let mut rec = sample("sess-rs");
        rec.notes_markdown = Some("# Old notes".to_string());
        rec.notes_status = Some("completed".to_string());
        rec.notes_root_message_id = None;
        rec.insights = vec![InsightNote {
            offset_ms: 60_000,
            severity: "green".to_string(),
            message: "Based on a removed utterance".to_string(),
            segment_index: Some(0),
        }];
        store.save(&rec).unwrap();

        store
            .replace_segments(
                "sess-rs",
                vec![TranscriptSegment {
                    source: TranscriptSource::Others,
                    utterance: "entirely new transcript".to_string(),
                    speaker_id: None,
                    speaker_label: None,
                }],
            )
            .unwrap();

        let saved = store.load("sess-rs").unwrap().unwrap();
        assert!(saved.insights.is_empty());
        assert!(saved.notes_markdown.is_none());
    }

    #[test]
    fn merge_after_completed_notes_updates_insights_only() {
        let store = temp_store([0x7d; 32], "insights_after_notes");
        store.live_begin("sess-ln", 1_000).unwrap();
        store
            .live_append_segment(
                "sess-ln",
                TranscriptSource::Others,
                "we use spreadsheets",
                5_000,
            )
            .unwrap();
        store.save(&sample("sess-ln")).unwrap();
        store
            .record_reply(
                "sess-ln",
                Some("root-1"),
                "task-1",
                "# Call Notes\n\nOverview.",
                "completed",
                None,
                None,
                "2026-05-25T10:31:00Z",
            )
            .unwrap();
        store
            .live_append_insight("sess-ln", "green", "Arrived after summary", Some(0), 61_000)
            .unwrap();

        store.live_merge_insights_then_delete("sess-ln").unwrap();
        assert!(store.live_load("sess-ln").unwrap().is_none());
        let saved = store.load("sess-ln").unwrap().unwrap();
        assert_eq!(saved.insights.len(), 1);
        assert_eq!(saved.insights[0].message, "Arrived after summary");
        assert_eq!(
            saved.notes_markdown.as_deref(),
            Some("# Call Notes\n\nOverview.")
        );

        store
            .live_append_insight("sess-ln", "green", "Even later insight", None, 62_000)
            .unwrap();
        let saved = store.load("sess-ln").unwrap().unwrap();
        assert_eq!(saved.insights.len(), 2);
        assert_eq!(saved.insights[1].message, "Even later insight");
        assert_eq!(
            saved.notes_markdown.as_deref(),
            Some("# Call Notes\n\nOverview.")
        );
    }

    #[test]
    fn recovery_merge_unions_live_insights_with_saved_session() {
        let store = temp_store([0x7c; 32], "insights_recovery");
        store.live_begin("sess-rec", 1_000).unwrap();
        store
            .live_append_segment(
                "sess-rec",
                TranscriptSource::Others,
                "we use spreadsheets",
                5_000,
            )
            .unwrap();
        store
            .live_append_insight("sess-rec", "green", "Snapshot insight", Some(0), 11_000)
            .unwrap();
        let live = store.live_load("sess-rec").unwrap().unwrap();

        let mut rec = sample("sess-rec");
        rec.insights = live.insights.clone();
        store.save(&rec).unwrap();

        store
            .live_append_insight(
                "sess-rec",
                "green",
                "Post-snapshot insight",
                Some(0),
                12_000,
            )
            .unwrap();

        store.live_merge_insights_then_delete("sess-rec").unwrap();
        assert!(store.live_load("sess-rec").unwrap().is_none());
        let saved = store.load("sess-rec").unwrap().unwrap();
        assert_eq!(saved.insights.len(), 2);
        assert_eq!(saved.insights[0].message, "Snapshot insight");
        assert_eq!(saved.insights[1].message, "Post-snapshot insight");
    }

    #[test]
    fn late_insights_survive_finalize_handoff() {
        let store = temp_store([0x79; 32], "insights_late");
        store.live_begin("sess-late", 1_000).unwrap();
        store
            .live_append_segment(
                "sess-late",
                TranscriptSource::Others,
                "we use spreadsheets",
                5_000,
            )
            .unwrap();
        store.save(&sample("sess-late")).unwrap();
        store
            .live_append_insight(
                "sess-late",
                "green",
                "Landed before cleanup",
                Some(0),
                6_000,
            )
            .unwrap();
        store.live_merge_insights_then_delete("sess-late").unwrap();
        assert!(store.live_load("sess-late").unwrap().is_none());
        store
            .live_append_insight("sess-late", "green", "Landed after cleanup", Some(0), 7_000)
            .unwrap();

        store
            .record_reply(
                "sess-late",
                Some("root-1"),
                "task-1",
                "# Call Notes\n\nOverview.",
                "completed",
                None,
                None,
                "2026-05-25T10:31:00Z",
            )
            .unwrap();
        let saved = store.load("sess-late").unwrap().unwrap();
        let messages: Vec<&str> = saved
            .insights
            .iter()
            .map(|note| note.message.as_str())
            .collect();
        assert!(messages.contains(&"Landed before cleanup"));
        assert!(messages.contains(&"Landed after cleanup"));
        assert!(!saved.notes_markdown.unwrap().contains("Landed"));
    }

    #[test]
    fn record_reply_merges_pending_live_insights() {
        let store = temp_store([0x7a; 32], "insights_reply_merge");
        store.live_begin("sess-rm", 1_000).unwrap();
        store
            .live_append_segment(
                "sess-rm",
                TranscriptSource::Others,
                "we use spreadsheets",
                5_000,
            )
            .unwrap();
        store.save(&sample("sess-rm")).unwrap();
        store
            .live_append_insight("sess-rm", "green", "Arrived mid-handoff", Some(0), 61_000)
            .unwrap();

        store
            .record_reply(
                "sess-rm",
                Some("root-1"),
                "task-1",
                "# Call Notes\n\nOverview.",
                "completed",
                None,
                None,
                "2026-05-25T10:31:00Z",
            )
            .unwrap();
        let saved = store.load("sess-rm").unwrap().unwrap();
        assert!(saved
            .insights
            .iter()
            .any(|note| note.message == "Arrived mid-handoff"));
        assert!(!saved
            .notes_markdown
            .unwrap()
            .contains("Arrived mid-handoff"));

        store.live_merge_insights_then_delete("sess-rm").unwrap();
        let saved = store.load("sess-rm").unwrap().unwrap();
        assert_eq!(
            saved
                .insights
                .iter()
                .filter(|n| n.message == "Arrived mid-handoff")
                .count(),
            1
        );
    }

    #[test]
    fn insight_keeps_dispatch_time_segment_index() {
        let store = temp_store([0x78; 32], "insights_dispatch");
        store.live_begin("sess-dc", 1_000).unwrap();
        store
            .live_append_segment(
                "sess-dc",
                TranscriptSource::Others,
                "we use spreadsheets",
                5_000,
            )
            .unwrap();
        let dispatch_segment_index = Some(0);
        store
            .live_append_segment(
                "sess-dc",
                TranscriptSource::You,
                "unrelated later remark",
                8_000,
            )
            .unwrap();
        store
            .live_append_insight(
                "sess-dc",
                "green",
                "Ask about their process cost",
                dispatch_segment_index,
                9_000,
            )
            .unwrap();
        store
            .live_append_insight("sess-dc", "yellow", "Slow down the pitch", None, 10_000)
            .unwrap();

        let live = store.live_load("sess-dc").unwrap().unwrap();
        assert_eq!(live.segments.len(), 2);
        assert_eq!(live.insights.len(), 2);
        assert_eq!(live.insights[0].segment_index, Some(0));
        assert_eq!(live.insights[1].segment_index, None);
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
        assert_eq!(after_notes.chat.len(), 1);
        assert_eq!(after_notes.chat[0].content, "# Notes");
        assert_eq!(after_notes.chat[0].role, "assistant");

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
        assert_eq!(after_chat.chat.len(), 2);
        assert_eq!(after_chat.chat[1].content, "follow up reply");
        assert_eq!(
            after_chat.chat[1].parent_message_id.as_deref(),
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

    #[test]
    fn old_record_without_participants_deserializes_to_defaults() {
        let legacy = serde_json::json!({
            "session_id": "legacy-1",
            "relay_session_id": "legacy-1",
            "started_at": "2026-05-25T10:00:00Z",
            "ended_at": "2026-05-25T10:30:00Z",
            "segments": [],
        });
        let record: SessionRecord = serde_json::from_value(legacy).unwrap();
        assert!(record.attendees.is_empty());
        assert_eq!(record.calendar_event_id, None);
        assert_eq!(record.title, None);
    }

    #[test]
    fn live_record_roundtrip_append_and_delete() {
        let store = temp_store([0x99; 32], "live");
        store.live_begin("live-1", 1000).unwrap();
        store.live_begin("live-1", 2000).unwrap();
        store
            .live_append_segment("live-1", TranscriptSource::You, "  hello  ", 1500)
            .unwrap();
        store
            .live_append_segment("live-1", TranscriptSource::Others, "   ", 1600)
            .unwrap();
        store
            .live_set_participants("live-1", vec!["a@b.c".to_string()], Some("evt".to_string()))
            .unwrap();

        let record = store.live_load("live-1").unwrap().unwrap();
        assert_eq!(record.started_at_ms, 1000);
        assert_eq!(record.updated_at_ms, 1500);
        assert_eq!(record.segments.len(), 1);
        assert_eq!(record.segments[0].utterance, "hello");
        assert_eq!(record.attendees, vec!["a@b.c"]);
        assert_eq!(record.calendar_event_id.as_deref(), Some("evt"));

        assert_eq!(store.live_list().unwrap().len(), 1);

        store.live_delete("live-1").unwrap();
        assert!(store.live_load("live-1").unwrap().is_none());
        store.live_delete("live-1").unwrap();
    }

    #[test]
    fn live_append_without_begin_is_silent_noop() {
        let store = temp_store([0xcc; 32], "livenobegin");
        store
            .live_append_segment("live-orphan", TranscriptSource::Others, "lost", 100)
            .unwrap();
        assert!(store.live_load("live-orphan").unwrap().is_none());
    }

    #[test]
    fn live_replace_segments_overwrites_and_creates() {
        let store = temp_store([0xbb; 32], "livereplace");
        store.live_begin("live-r", 1000).unwrap();
        store
            .live_append_segment("live-r", TranscriptSource::You, "one", 1100)
            .unwrap();
        store
            .live_set_participants("live-r", vec!["a@b.c".to_string()], None)
            .unwrap();

        let replacement = vec![
            TranscriptSegment {
                source: TranscriptSource::You,
                utterance: "one".to_string(),
                speaker_id: None,
                speaker_label: None,
            },
            TranscriptSegment {
                source: TranscriptSource::Others,
                utterance: "two".to_string(),
                speaker_id: None,
                speaker_label: None,
            },
        ];
        store
            .live_replace_segments("live-r", 1000, replacement, 1200)
            .unwrap();
        let record = store.live_load("live-r").unwrap().unwrap();
        assert_eq!(record.segments.len(), 2);
        assert_eq!(record.updated_at_ms, 1200);
        assert_eq!(record.attendees, vec!["a@b.c"]);

        store
            .live_replace_segments(
                "live-missing",
                500,
                vec![TranscriptSegment {
                    source: TranscriptSource::You,
                    utterance: "solo".to_string(),
                    speaker_id: None,
                    speaker_label: None,
                }],
                600,
            )
            .unwrap();
        let created = store.live_load("live-missing").unwrap().unwrap();
        assert_eq!(created.started_at_ms, 500);
        assert_eq!(created.segments.len(), 1);
    }

    #[test]
    fn live_replace_keeps_blocks_when_the_summarized_prefix_is_untouched() {
        let store = temp_store([0xb7; 32], "liveprefixkeep");
        store.live_begin("live-k", 1000).unwrap();
        for word in ["one", "two", "three"] {
            store
                .live_append_segment("live-k", TranscriptSource::You, word, 1100)
                .unwrap();
        }
        store
            .live_append_block("live-k", 0, 2, "- they covered one and two", 1150)
            .unwrap();

        let replacement = ["one", "two", "three", "four"]
            .iter()
            .map(|word| TranscriptSegment {
                source: TranscriptSource::You,
                utterance: word.to_string(),
                speaker_id: None,
                speaker_label: None,
            })
            .collect();
        store
            .live_replace_segments("live-k", 1000, replacement, 1200)
            .unwrap();

        let record = store.live_load("live-k").unwrap().unwrap();
        assert_eq!(record.segments.len(), 4);
        assert_eq!(record.blocks.len(), 1);
    }

    #[test]
    fn live_replace_keeps_blocks_when_diarization_labels_the_prefix() {
        let store = temp_store([0xba; 32], "livediarize");
        store.live_begin("live-dz", 1000).unwrap();
        for word in ["one", "two", "three"] {
            store
                .live_append_segment("live-dz", TranscriptSource::Others, word, 1100)
                .unwrap();
        }
        store
            .live_append_block("live-dz", 0, 3, "- they covered everything", 1150)
            .unwrap();

        let labelled = ["one", "two", "three"]
            .iter()
            .enumerate()
            .map(|(index, word)| TranscriptSegment {
                source: TranscriptSource::Others,
                utterance: word.to_string(),
                speaker_id: Some(format!("spk-{index}")),
                speaker_label: Some(format!("Speaker {index}")),
            })
            .collect();
        store
            .live_replace_segments("live-dz", 1000, labelled, 1200)
            .unwrap();

        let record = store.live_load("live-dz").unwrap().unwrap();
        assert_eq!(record.blocks.len(), 1);
        assert_eq!(
            record.segments[0].speaker_label.as_deref(),
            Some("Speaker 0")
        );
    }

    #[test]
    fn live_replace_drops_blocks_when_the_summarized_prefix_changed() {
        let store = temp_store([0xb8; 32], "liveprefixdrop");
        store.live_begin("live-d", 1000).unwrap();
        for word in ["one", "two", "three"] {
            store
                .live_append_segment("live-d", TranscriptSource::You, word, 1100)
                .unwrap();
        }
        store
            .live_append_block("live-d", 0, 2, "- they covered one and two", 1150)
            .unwrap();

        let rewritten = ["ONE rewritten", "two", "three"]
            .iter()
            .map(|word| TranscriptSegment {
                source: TranscriptSource::You,
                utterance: word.to_string(),
                speaker_id: None,
                speaker_label: None,
            })
            .collect();
        store
            .live_replace_segments("live-d", 1000, rewritten, 1200)
            .unwrap();
        assert!(store
            .live_load("live-d")
            .unwrap()
            .unwrap()
            .blocks
            .is_empty());
    }

    #[test]
    fn live_replace_drops_blocks_when_the_transcript_shrinks_below_coverage() {
        let store = temp_store([0xb9; 32], "liveprefixshrink");
        store.live_begin("live-s", 1000).unwrap();
        for word in ["one", "two", "three"] {
            store
                .live_append_segment("live-s", TranscriptSource::You, word, 1100)
                .unwrap();
        }
        store
            .live_append_block("live-s", 0, 3, "- the whole stretch", 1150)
            .unwrap();

        let shorter = vec![TranscriptSegment {
            source: TranscriptSource::You,
            utterance: "one".to_string(),
            speaker_id: None,
            speaker_label: None,
        }];
        store
            .live_replace_segments("live-s", 1000, shorter, 1200)
            .unwrap();
        assert!(store
            .live_load("live-s")
            .unwrap()
            .unwrap()
            .blocks
            .is_empty());
    }

    #[test]
    fn live_append_block_reports_whether_it_appended() {
        let store = temp_store([0xbb; 32], "liveappendreport");
        store.live_begin("live-r", 1000).unwrap();
        for word in ["one", "two", "three"] {
            store
                .live_append_segment("live-r", TranscriptSource::You, word, 1100)
                .unwrap();
        }
        let appended = store
            .live_append_block("live-r", 0, 2, "- they covered one and two", 1150)
            .unwrap();
        assert!(appended);
        assert_eq!(store.live_load("live-r").unwrap().unwrap().blocks.len(), 1);

        let appended_again = store
            .live_append_block("live-r", 0, 2, "- they covered one and two", 1200)
            .unwrap();
        assert!(!appended_again);
        assert_eq!(store.live_load("live-r").unwrap().unwrap().blocks.len(), 1);
    }

    #[test]
    fn live_records_do_not_appear_in_session_list() {
        let store = temp_store([0xaa; 32], "livelist");
        store.live_begin("live-2", 1000).unwrap();
        assert!(store.list().unwrap().is_empty());
        assert!(store.load("live-2").unwrap().is_none());
    }

    #[test]
    fn participants_roundtrip_through_store() {
        let store = temp_store([0x88; 32], "participants");
        let mut rec = sample("sess-p");
        rec.attendees = vec![
            "alice@example.com".to_string(),
            "bob@example.com".to_string(),
        ];
        rec.calendar_event_id = Some("evt-123".to_string());
        store.save(&rec).unwrap();

        let loaded = store.load("sess-p").unwrap().unwrap();
        assert_eq!(
            loaded.attendees,
            vec!["alice@example.com", "bob@example.com"]
        );
        assert_eq!(loaded.calendar_event_id.as_deref(), Some("evt-123"));
    }

    #[test]
    fn derive_title_prefers_first_notes_heading() {
        let mut rec = sample("title-notes");
        rec.notes_markdown = Some("\n\n#  Quarterly planning sync \n\nbody".to_string());
        assert_eq!(derive_title(&rec), "Quarterly planning sync");
    }

    #[test]
    fn derive_title_falls_back_to_attendees_and_date() {
        let mut rec = sample("title-attendees");
        rec.notes_markdown = None;
        rec.attendees = vec![
            "alice@example.com".to_string(),
            "bob@example.com".to_string(),
        ];
        assert_eq!(
            derive_title(&rec),
            "alice@example.com, bob@example.com (2026-05-25)"
        );
    }

    #[test]
    fn derive_title_falls_back_to_date_when_no_notes_or_attendees() {
        let mut rec = sample("title-date");
        rec.notes_markdown = None;
        rec.attendees = vec![];
        assert_eq!(derive_title(&rec), "2026-05-25");
    }

    #[test]
    fn derive_title_strips_emphasis_and_caps_length() {
        let mut rec = sample("title-clean");
        rec.notes_markdown = Some("# **Acme Q3 pricing review**\n\nbody".to_string());
        assert_eq!(derive_title(&rec), "Acme Q3 pricing review");

        let long = "Discussion about the upcoming product launch and the very detailed go to market plan for next quarter and well beyond";
        rec.notes_markdown = Some(format!("# {long}\n\nbody"));
        let title = derive_title(&rec);
        assert!(title.chars().count() <= MAX_TITLE_LEN + 1);
        assert!(title.ends_with('…'));
        assert!(!title.contains("  "));
    }

    #[test]
    fn apply_auto_title_sets_once_only_with_notes_and_survives_later_writes() {
        let store = temp_store([0xd1; 32], "autotitle");
        let mut rec = sample("sess-at");
        rec.notes_markdown = None;
        rec.notes_status = Some("pending".to_string());
        store.save(&rec).unwrap();

        assert_eq!(store.apply_auto_title("sess-at").unwrap(), false);
        assert_eq!(store.load("sess-at").unwrap().unwrap().title, None);

        store
            .record_reply(
                "sess-at",
                Some("root-1"),
                "task-1",
                "# Acme pricing review\n\nbody",
                "completed",
                None,
                None,
                "2026-05-25T10:31:00Z",
            )
            .unwrap();

        assert_eq!(store.apply_auto_title("sess-at").unwrap(), true);
        assert_eq!(
            store.load("sess-at").unwrap().unwrap().title.as_deref(),
            Some("Acme pricing review")
        );

        assert_eq!(store.apply_auto_title("sess-at").unwrap(), false);

        store
            .record_reply(
                "sess-at",
                Some("umsg-2"),
                "task-2",
                "follow up",
                "completed",
                None,
                None,
                "2026-05-25T10:32:00Z",
            )
            .unwrap();
        assert_eq!(
            store.load("sess-at").unwrap().unwrap().title.as_deref(),
            Some("Acme pricing review")
        );
    }

    #[test]
    fn regeneration_drops_auto_title_but_keeps_manual_title() {
        let store = temp_store([0xd5; 32], "regen-title");
        let edit = vec![TranscriptSegment {
            source: TranscriptSource::You,
            utterance: "new transcript".to_string(),
            speaker_id: None,
            speaker_label: None,
        }];

        let mut auto = sample("auto");
        auto.notes_markdown = Some("# Acme pricing review\n\nbody".to_string());
        auto.notes_status = Some("completed".to_string());
        auto.title = Some(derive_title(&auto));
        auto.title_is_auto = true;
        store.save(&auto).unwrap();
        store.replace_segments("auto", edit.clone()).unwrap();
        assert_eq!(store.load("auto").unwrap().unwrap().title, None);

        let mut manual = sample("manual");
        manual.notes_markdown = Some("# Acme pricing review\n\nbody".to_string());
        manual.notes_status = Some("completed".to_string());
        manual.title = Some("Board prep".to_string());
        store.save(&manual).unwrap();
        store.replace_segments("manual", edit.clone()).unwrap();
        assert_eq!(
            store.load("manual").unwrap().unwrap().title.as_deref(),
            Some("Board prep")
        );

        let mut manual_matches_derived = sample("manual-derived");
        manual_matches_derived.notes_markdown = Some("# Acme pricing review\n\nbody".to_string());
        manual_matches_derived.notes_status = Some("completed".to_string());
        manual_matches_derived.title = Some(derive_title(&manual_matches_derived));
        store.save(&manual_matches_derived).unwrap();
        store.replace_segments("manual-derived", edit).unwrap();
        assert_eq!(
            store
                .load("manual-derived")
                .unwrap()
                .unwrap()
                .title
                .as_deref(),
            Some("Acme pricing review")
        );
    }

    #[test]
    fn update_title_roundtrips_and_trims() {
        let store = temp_store([0xc1; 32], "updatetitle");
        store.save(&sample("sess-ut")).unwrap();

        store
            .update_title("sess-ut", Some("  Renamed call  ".to_string()))
            .unwrap();
        assert_eq!(
            store.load("sess-ut").unwrap().unwrap().title.as_deref(),
            Some("Renamed call")
        );

        store
            .update_title("sess-ut", Some("   ".to_string()))
            .unwrap();
        assert_eq!(store.load("sess-ut").unwrap().unwrap().title, None);

        store.update_title("sess-ut", None).unwrap();
        assert_eq!(store.load("sess-ut").unwrap().unwrap().title, None);
    }

    #[test]
    fn replace_segments_overwrites_finalized_record() {
        let store = temp_store([0xc2; 32], "replaceseg");
        let mut rec = sample("sess-rs");
        rec.notes_markdown = Some("# Recap\nfrom old segments".to_string());
        rec.notes_status = Some("completed".to_string());
        rec.notes_error = Some("stale".to_string());
        rec.chat = vec![ChatMsg {
            id: "c1".to_string(),
            role: "assistant".to_string(),
            content: "old summary".to_string(),
            status: "completed".to_string(),
            parent_message_id: None,
            error_code: None,
            error_message: None,
            created_at: "2026-05-25T10:31:00Z".to_string(),
        }];
        store.save(&rec).unwrap();

        let replacement = vec![
            TranscriptSegment {
                source: TranscriptSource::You,
                utterance: "first".to_string(),
                speaker_id: Some("spk-1".to_string()),
                speaker_label: Some("Alice".to_string()),
            },
            TranscriptSegment {
                source: TranscriptSource::Others,
                utterance: "second".to_string(),
                speaker_id: None,
                speaker_label: None,
            },
        ];
        store.replace_segments("sess-rs", replacement).unwrap();

        let loaded = store.load("sess-rs").unwrap().unwrap();
        assert_eq!(loaded.segments.len(), 2);
        assert_eq!(loaded.segments[0].utterance, "first");
        assert_eq!(loaded.segments[0].speaker_label.as_deref(), Some("Alice"));
        assert_eq!(loaded.segments[1].utterance, "second");
        assert!(loaded.notes_markdown.is_none());
        assert!(loaded.notes_status.is_none());
        assert!(loaded.notes_error.is_none());
        assert_eq!(loaded.notes_root_message_id.as_deref(), Some("root-1"));
        assert_eq!(loaded.chat.len(), 1);
    }

    #[test]
    fn replace_segments_drops_block_summaries_of_the_old_transcript() {
        let store = temp_store([0xd4; 32], "replaceblocks");
        let mut rec = sample("sess-rb");
        rec.notes_status = Some("completed".to_string());
        rec.blocks = vec![BlockSummary {
            start_index: 0,
            end_index: 1,
            summary: "- they discussed the old transcript".to_string(),
            created_at_ms: 1,
        }];
        rec.insights = vec![InsightNote {
            offset_ms: 1000,
            severity: "green".to_string(),
            message: "old cue".to_string(),
            segment_index: Some(0),
        }];
        store.save(&rec).unwrap();

        store
            .replace_segments(
                "sess-rb",
                vec![TranscriptSegment {
                    source: TranscriptSource::You,
                    utterance: "a completely different conversation".to_string(),
                    speaker_id: None,
                    speaker_label: None,
                }],
            )
            .unwrap();

        let loaded = store.load("sess-rb").unwrap().unwrap();
        assert!(
            loaded.blocks.is_empty(),
            "stale blocks would summarize the replaced transcript"
        );
        assert!(loaded.insights.is_empty());
    }

    #[test]
    fn merge_normalizes_chronological_order_regardless_of_arg_order() {
        let store = temp_store([0xc7; 32], "mergeorder");
        let mut early = sample("early");
        early.started_at = "2026-05-25T09:00:00Z".to_string();
        early.ended_at = "2026-05-25T09:30:00Z".to_string();
        early.notes_markdown = Some("# Early".to_string());
        early.notes_status = Some("completed".to_string());
        early.segments = vec![TranscriptSegment {
            source: TranscriptSource::You,
            utterance: "earlier".to_string(),
            speaker_id: None,
            speaker_label: None,
        }];
        let mut late = sample("late");
        late.started_at = "2026-05-25T11:00:00Z".to_string();
        late.ended_at = "2026-05-25T11:30:00Z".to_string();
        late.notes_markdown = Some("# Late".to_string());
        late.notes_status = Some("completed".to_string());
        late.segments = vec![TranscriptSegment {
            source: TranscriptSource::Others,
            utterance: "later".to_string(),
            speaker_id: None,
            speaker_label: None,
        }];
        store.save(&early).unwrap();
        store.save(&late).unwrap();

        let merged_id = store.merge("late", "early").unwrap();
        let merged = store.load(&merged_id).unwrap().unwrap();
        assert_eq!(merged.started_at, "2026-05-25T09:00:00Z");
        assert_eq!(merged.ended_at, "2026-05-25T11:30:00Z");
        assert_eq!(merged.segments[0].utterance, "earlier");
        assert_eq!(merged.segments[1].utterance, "later");
        let notes = merged.notes_markdown.unwrap();
        assert!(notes.find("# Early").unwrap() < notes.find("# Late").unwrap());
    }

    #[test]
    fn edits_rejected_while_notes_pending() {
        let store = temp_store([0xc8; 32], "pendingedit");
        let mut pending = sample("pending-a");
        pending.notes_status = Some("pending".to_string());
        pending.segments = vec![
            TranscriptSegment {
                source: TranscriptSource::You,
                utterance: "one".to_string(),
                speaker_id: None,
                speaker_label: None,
            },
            TranscriptSegment {
                source: TranscriptSource::Others,
                utterance: "two".to_string(),
                speaker_id: None,
                speaker_label: None,
            },
        ];
        store.save(&pending).unwrap();
        let mut settled = sample("settled-b");
        settled.notes_status = Some("completed".to_string());
        store.save(&settled).unwrap();

        assert!(matches!(
            store.split("pending-a", 1),
            Err(StoreError::NotesPending)
        ));
        assert!(matches!(
            store.replace_segments("pending-a", vec![]),
            Err(StoreError::NotesPending)
        ));
        assert!(matches!(
            store.merge("pending-a", "settled-b"),
            Err(StoreError::NotesPending)
        ));

        assert!(store.load("pending-a").unwrap().is_some());
        assert!(store.load("settled-b").unwrap().is_some());
        assert_eq!(store.load("pending-a").unwrap().unwrap().segments.len(), 2);
    }

    #[test]
    fn merge_rejects_identical_ids_without_mutating() {
        let store = temp_store([0xc6; 32], "selfmerge");
        store.save(&sample("solo")).unwrap();

        assert!(matches!(
            store.merge("solo", "solo"),
            Err(StoreError::InvalidId)
        ));

        let loaded = store.load("solo").unwrap().unwrap();
        assert_eq!(loaded.segments.len(), 1);
        assert_eq!(loaded.segments[0].utterance, "hello world");
    }

    #[test]
    fn split_creates_two_records_and_removes_original() {
        let store = temp_store([0xc3; 32], "splitsess");
        let mut rec = sample("sess-split");
        rec.attendees = vec!["alice@example.com".to_string()];
        rec.notes_markdown = Some("# Recap\nkey points".to_string());
        rec.notes_status = Some("completed".to_string());
        rec.notes_root_message_id = Some("root-split".to_string());
        rec.chat = vec![ChatMsg {
            id: "m1".to_string(),
            role: "assistant".to_string(),
            content: "summary".to_string(),
            status: "completed".to_string(),
            parent_message_id: None,
            error_code: None,
            error_message: None,
            created_at: "2026-05-25T10:31:00Z".to_string(),
        }];
        rec.segments = vec![
            TranscriptSegment {
                source: TranscriptSource::You,
                utterance: "one".to_string(),
                speaker_id: None,
                speaker_label: None,
            },
            TranscriptSegment {
                source: TranscriptSource::Others,
                utterance: "two".to_string(),
                speaker_id: None,
                speaker_label: None,
            },
            TranscriptSegment {
                source: TranscriptSource::You,
                utterance: "three".to_string(),
                speaker_id: None,
                speaker_label: None,
            },
        ];
        store.save(&rec).unwrap();

        let (id_a, id_b) = store.split("sess-split", 1).unwrap();
        assert_ne!(id_a, id_b);
        assert_ne!(id_a, "sess-split");
        assert!(store.load("sess-split").unwrap().is_none());

        let part_a = store.load(&id_a).unwrap().unwrap();
        let part_b = store.load(&id_b).unwrap().unwrap();
        assert_eq!(part_a.relay_session_id, id_a);
        assert_eq!(part_b.relay_session_id, id_b);
        assert_ne!(part_a.relay_session_id, "sess-split");
        assert_ne!(part_b.relay_session_id, "sess-split");
        assert_eq!(part_a.segments.len(), 1);
        assert_eq!(part_a.segments[0].utterance, "one");
        assert_eq!(part_b.segments.len(), 2);
        assert_eq!(part_b.segments[0].utterance, "two");
        assert_eq!(part_a.attendees, vec!["alice@example.com"]);
        assert_eq!(part_b.attendees, vec!["alice@example.com"]);
        assert_eq!(part_a.started_at, rec.started_at);
        assert_eq!(part_b.ended_at, rec.ended_at);

        assert!(part_a.notes_markdown.is_none());
        assert_eq!(part_a.notes_root_message_id.as_deref(), Some("root-split"));
        assert_eq!(part_a.chat.len(), 1);
        assert!(part_b.notes_markdown.is_none());
        assert!(part_b.notes_root_message_id.is_none());
        assert!(part_b.chat.is_empty());

        assert!(store.split(&id_a, 0).is_err());
        assert!(store.split(&id_a, 5).is_err());
    }

    #[test]
    fn merge_unions_segments_attendees_and_spans_timestamps() {
        let store = temp_store([0xc4; 32], "mergesess");
        let mut a = sample("merge-a");
        a.started_at = "2026-05-25T09:00:00Z".to_string();
        a.ended_at = "2026-05-25T09:30:00Z".to_string();
        a.attendees = vec!["alice@example.com".to_string()];
        a.notes_markdown = Some("# A recap".to_string());
        a.notes_status = Some("completed".to_string());
        a.chat = vec![ChatMsg {
            id: "ma".to_string(),
            role: "assistant".to_string(),
            content: "a summary".to_string(),
            status: "completed".to_string(),
            parent_message_id: None,
            error_code: None,
            error_message: None,
            created_at: "2026-05-25T09:31:00Z".to_string(),
        }];
        a.segments = vec![TranscriptSegment {
            source: TranscriptSource::You,
            utterance: "alpha".to_string(),
            speaker_id: None,
            speaker_label: None,
        }];
        let mut b = sample("merge-b");
        b.started_at = "2026-05-25T11:00:00Z".to_string();
        b.ended_at = "2026-05-25T11:45:00Z".to_string();
        b.attendees = vec![
            "alice@example.com".to_string(),
            "bob@example.com".to_string(),
        ];
        b.notes_markdown = Some("# B recap".to_string());
        b.notes_status = Some("completed".to_string());
        b.chat = vec![ChatMsg {
            id: "mb".to_string(),
            role: "assistant".to_string(),
            content: "b summary".to_string(),
            status: "completed".to_string(),
            parent_message_id: None,
            error_code: None,
            error_message: None,
            created_at: "2026-05-25T11:46:00Z".to_string(),
        }];
        b.segments = vec![TranscriptSegment {
            source: TranscriptSource::Others,
            utterance: "beta".to_string(),
            speaker_id: None,
            speaker_label: None,
        }];
        store.save(&a).unwrap();
        store.save(&b).unwrap();

        let merged_id = store.merge("merge-a", "merge-b").unwrap();
        assert_ne!(merged_id, "merge-a");
        assert_ne!(merged_id, "merge-b");
        assert!(store.load("merge-a").unwrap().is_none());
        assert!(store.load("merge-b").unwrap().is_none());

        let merged = store.load(&merged_id).unwrap().unwrap();
        assert_eq!(merged.relay_session_id, merged_id);
        assert_ne!(merged.relay_session_id, "merge-a");
        assert_ne!(merged.relay_session_id, "merge-b");
        assert_eq!(merged.segments.len(), 2);
        assert_eq!(merged.segments[0].utterance, "alpha");
        assert_eq!(merged.segments[1].utterance, "beta");
        assert_eq!(
            merged.attendees,
            vec!["alice@example.com", "bob@example.com"]
        );
        assert_eq!(merged.started_at, "2026-05-25T09:00:00Z");
        assert_eq!(merged.ended_at, "2026-05-25T11:45:00Z");
        let merged_notes = merged.notes_markdown.unwrap();
        assert!(merged_notes.contains("# A recap"));
        assert!(merged_notes.contains("# B recap"));
        assert_eq!(merged.notes_status.as_deref(), Some("completed"));
        assert_eq!(merged.chat.len(), 2);
    }

    #[test]
    fn merge_playbook_both_absent_is_none() {
        assert_eq!(SessionStore::merge_playbook(None, None), None);
    }

    #[test]
    fn merge_playbook_one_side_present_is_adopted() {
        assert_eq!(
            SessionStore::merge_playbook(Some("Discovery framework"), None),
            Some("Discovery framework".to_string())
        );
        assert_eq!(
            SessionStore::merge_playbook(None, Some("Discovery framework")),
            Some("Discovery framework".to_string())
        );
    }

    #[test]
    fn merge_playbook_equal_values_are_kept() {
        assert_eq!(
            SessionStore::merge_playbook(Some("Discovery framework"), Some("Discovery framework")),
            Some("Discovery framework".to_string())
        );
    }

    #[test]
    fn merge_playbook_equal_after_trim_is_kept() {
        assert_eq!(
            SessionStore::merge_playbook(
                Some("Discovery framework\n"),
                Some("  Discovery framework")
            ),
            Some("Discovery framework\n".to_string())
        );
    }

    #[test]
    fn merge_playbook_differing_values_drop_to_none() {
        assert_eq!(
            SessionStore::merge_playbook(
                Some("Discovery framework"),
                Some("Objection handling playbook")
            ),
            None
        );
    }

    #[test]
    fn merge_keeps_playbook_when_both_sessions_share_it() {
        let store = temp_store([0xd6; 32], "mergeplaybooksame");
        let mut a = sample("merge-pb-a");
        a.started_at = "2026-05-25T09:00:00Z".to_string();
        a.notes_status = Some("completed".to_string());
        a.playbook = Some("Discovery framework".to_string());
        let mut b = sample("merge-pb-b");
        b.started_at = "2026-05-25T11:00:00Z".to_string();
        b.notes_status = Some("completed".to_string());
        b.playbook = Some("Discovery framework".to_string());
        store.save(&a).unwrap();
        store.save(&b).unwrap();

        let merged_id = store.merge("merge-pb-a", "merge-pb-b").unwrap();
        let merged = store.load(&merged_id).unwrap().unwrap();
        assert_eq!(merged.playbook.as_deref(), Some("Discovery framework"));
    }

    #[test]
    fn merge_drops_playbook_when_sessions_disagree() {
        let store = temp_store([0xd7; 32], "mergeplaybookdiff");
        let mut a = sample("merge-pbdiff-a");
        a.started_at = "2026-05-25T09:00:00Z".to_string();
        a.notes_status = Some("completed".to_string());
        a.playbook = Some("Discovery framework".to_string());
        let mut b = sample("merge-pbdiff-b");
        b.started_at = "2026-05-25T11:00:00Z".to_string();
        b.notes_status = Some("completed".to_string());
        b.playbook = Some("Objection handling playbook".to_string());
        store.save(&a).unwrap();
        store.save(&b).unwrap();

        let merged_id = store.merge("merge-pbdiff-a", "merge-pbdiff-b").unwrap();
        let merged = store.load(&merged_id).unwrap().unwrap();
        assert_eq!(merged.playbook, None);
    }

    #[test]
    fn mutation_disallowed_while_live_exists() {
        let store = temp_store([0xc5; 32], "livegate");
        store.save(&sample("sess-live")).unwrap();
        store.live_begin("sess-live", 1000).unwrap();

        assert!(matches!(
            store.update_title("sess-live", Some("x".to_string())),
            Err(StoreError::LiveSessionActive)
        ));
        assert!(matches!(
            store.replace_segments("sess-live", vec![]),
            Err(StoreError::LiveSessionActive)
        ));
        assert!(matches!(
            store.split("sess-live", 1),
            Err(StoreError::LiveSessionActive)
        ));

        store.live_delete("sess-live").unwrap();
        store
            .update_title("sess-live", Some("ok now".to_string()))
            .unwrap();
        assert_eq!(
            store.load("sess-live").unwrap().unwrap().title.as_deref(),
            Some("ok now")
        );
    }
}

#[cfg(test)]
#[path = "store_append_chat_tests.rs"]
mod append_chat_tests;
