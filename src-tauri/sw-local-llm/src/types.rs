use serde::Serialize;

#[derive(Debug, Clone, PartialEq)]
pub enum ModelKind {
    Gguf { file: String },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum ModelSource {
    #[default]
    Remote,
    Local,
}

#[derive(Debug, Clone)]
pub struct ModelEntry {
    pub id: String,
    pub repo_id: String,
    pub revision: String,
    pub label: String,
    pub ram_hint_gb: f32,
    pub recommended: bool,
    pub kind: ModelKind,
    pub source: ModelSource,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmDownloadProgress {
    pub file_name: String,
    pub bytes_downloaded: u64,
    pub bytes_total: u64,
    pub files_completed: usize,
    pub files_total: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::Assistant,
            content: content.into(),
        }
    }
}

pub const CHAT_HISTORY_MAX_MESSAGES: usize = 16;
pub const CHAT_HISTORY_MAX_CHARS: usize = 8000;

pub fn prepare_chat_history(
    history: &[ChatMessage],
    max_messages: usize,
    max_chars: usize,
) -> Vec<ChatMessage> {
    let mut start = history.len();
    let mut chars = 0usize;
    while start > 0 {
        let candidate = &history[start - 1];
        let candidate_chars = candidate.content.chars().count();
        if history.len() - start >= max_messages {
            break;
        }
        if chars + candidate_chars > max_chars && start < history.len() {
            break;
        }
        chars += candidate_chars;
        start -= 1;
    }
    while start < history.len() && history[start].role == ChatRole::Assistant {
        start += 1;
    }
    let mut prepared: Vec<ChatMessage> = Vec::with_capacity(history.len() - start);
    for message in &history[start..] {
        match prepared.last_mut() {
            Some(last) if last.role == message.role => {
                last.content.push_str("\n\n");
                last.content.push_str(&message.content);
            }
            _ => prepared.push(message.clone()),
        }
    }
    prepared
}

#[derive(Debug, Clone)]
pub struct GenerationChunk {
    pub text: String,
    pub done: bool,
}

#[derive(Debug, Clone)]
pub struct InferenceParams {
    pub max_tokens: usize,
    pub temperature: f32,
    pub top_p: f32,
    pub disable_reasoning: bool,
}

impl Default for InferenceParams {
    fn default() -> Self {
        Self {
            max_tokens: 1024,
            temperature: 0.7,
            top_p: 0.95,
            disable_reasoning: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("download failed: {0}")]
    Download(String),
    #[error("download canceled")]
    Cancelled,
    #[error("model not found: {0}")]
    ModelNotFound(String),
    #[error("failed to load model: {0}")]
    Load(String),
    #[error("inference failed: {0}")]
    Inference(String),
    #[error("reasoning suppression ignored: {0}")]
    ReasoningNotSuppressed(String),
    #[error("inference timed out: {0}")]
    Timeout(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: ChatRole, len: usize) -> ChatMessage {
        ChatMessage {
            role,
            content: "x".repeat(len),
        }
    }

    fn alternating(pairs: usize, last_user_len: usize) -> Vec<ChatMessage> {
        let mut history = Vec::new();
        for i in 0..pairs {
            history.push(msg(ChatRole::User, 10 + i));
            history.push(msg(ChatRole::Assistant, 20 + i));
        }
        history.push(msg(ChatRole::User, last_user_len));
        history
    }

    fn assert_valid_shape(prepared: &[ChatMessage]) {
        assert!(!prepared.is_empty());
        assert_eq!(prepared[0].role, ChatRole::User);
        assert_eq!(prepared.last().unwrap().role, ChatRole::User);
        for pair in prepared.windows(2) {
            assert_ne!(pair[0].role, pair[1].role);
        }
    }

    #[test]
    fn keeps_everything_under_budget() {
        let history = alternating(1, 10);
        let prepared = prepare_chat_history(&history, 16, 8000);
        assert_eq!(prepared.len(), 3);
        assert_valid_shape(&prepared);
    }

    #[test]
    fn count_cap_never_orphans_an_assistant_turn() {
        let history = alternating(9, 7);
        let prepared = prepare_chat_history(&history, 16, 8000);
        assert!(prepared.len() <= 16);
        assert_valid_shape(&prepared);
        assert_eq!(prepared.last().unwrap().content.chars().count(), 7);
    }

    #[test]
    fn char_cap_never_orphans_an_assistant_turn() {
        let history = vec![
            msg(ChatRole::User, 500),
            msg(ChatRole::Assistant, 500),
            msg(ChatRole::User, 100),
        ];
        let prepared = prepare_chat_history(&history, 16, 550);
        assert_eq!(prepared.len(), 1);
        assert_valid_shape(&prepared);
        assert_eq!(prepared[0].content.chars().count(), 100);
    }

    #[test]
    fn char_cap_landing_on_assistant_drops_it() {
        let history = vec![
            msg(ChatRole::User, 5000),
            msg(ChatRole::Assistant, 100),
            msg(ChatRole::User, 100),
        ];
        let prepared = prepare_chat_history(&history, 16, 250);
        assert_valid_shape(&prepared);
        assert_eq!(prepared.len(), 1);
    }

    #[test]
    fn consecutive_same_role_messages_are_merged() {
        let history = vec![
            msg(ChatRole::User, 10),
            msg(ChatRole::User, 20),
            msg(ChatRole::Assistant, 30),
            msg(ChatRole::User, 40),
        ];
        let prepared = prepare_chat_history(&history, 16, 8000);
        assert_eq!(prepared.len(), 3);
        assert_valid_shape(&prepared);
        assert!(prepared[0].content.chars().count() > 30);
    }

    #[test]
    fn latest_message_survives_even_if_over_budget() {
        let history = vec![msg(ChatRole::User, 9000)];
        let prepared = prepare_chat_history(&history, 16, 8000);
        assert_eq!(prepared.len(), 1);
        assert_valid_shape(&prepared);
    }

    #[test]
    fn empty_history_stays_empty() {
        let history: Vec<ChatMessage> = Vec::new();
        assert!(prepare_chat_history(&history, 16, 8000).is_empty());
    }

    #[test]
    fn every_cap_boundary_produces_a_valid_shape() {
        let history = alternating(12, 9);
        for max_messages in 1..=25 {
            for max_chars in [1, 40, 80, 200, 8000] {
                let prepared = prepare_chat_history(&history, max_messages, max_chars);
                assert_valid_shape(&prepared);
            }
        }
    }
}
