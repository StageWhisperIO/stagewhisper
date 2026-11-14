use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::chat_stream::{ActivityData, UiChunk};
use crate::store::{ChatAppendOutcome, ChatMsg};

use super::chat_turns_types::{
    turn_reply_id, ChunkSink, TerminationOutcome, TurnOutcome, TurnPersistence, TurnSnapshot,
};

const REPLY_PERSIST_FAILED_ERROR_TEXT: &str = "Failed to save the assistant's reply.";
const PART_SEPARATOR: &str = "\n\n";

struct TurnState {
    buffer: String,
    sink: Option<Box<dyn ChunkSink>>,
}

pub struct TurnHandle {
    turn_id: String,
    reply_id: String,
    session_id: String,
    parent_message_id: String,
    text_id: String,
    binding: Option<String>,
    cancelled: AtomicBool,
    state: Mutex<TurnState>,
}

impl TurnHandle {
    pub(super) fn new<S: ChunkSink>(
        turn_id: String,
        session_id: String,
        parent_message_id: String,
        text_id: String,
        binding: Option<String>,
        sink: S,
    ) -> Self {
        let reply_id = turn_reply_id(&turn_id);
        Self {
            turn_id,
            reply_id,
            session_id,
            parent_message_id,
            text_id,
            binding,
            cancelled: AtomicBool::new(false),
            state: Mutex::new(TurnState {
                buffer: String::new(),
                sink: Some(Box::new(sink)),
            }),
        }
    }

    pub fn turn_id(&self) -> &str {
        &self.turn_id
    }

    pub fn reply_id(&self) -> &str {
        &self.reply_id
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn parent_message_id(&self) -> &str {
        &self.parent_message_id
    }

    pub fn text_id(&self) -> &str {
        &self.text_id
    }

    pub(super) fn binding(&self) -> Option<&str> {
        self.binding.as_deref()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub(super) fn mark_cancelled(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn buffered_text(&self) -> String {
        self.state.lock().unwrap().buffer.clone()
    }

    pub fn emit(&self, chunk: UiChunk) {
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        let state = self.state.lock().unwrap();
        if let Some(sink) = state.sink.as_ref() {
            sink.send(chunk);
        }
    }

    pub fn push_text_delta(&self, delta: &str) {
        if delta.is_empty() || self.cancelled.load(Ordering::Acquire) {
            return;
        }
        let mut state = self.state.lock().unwrap();
        if state.sink.is_none() {
            return;
        }
        state.buffer.push_str(delta);
        if let Some(sink) = state.sink.as_ref() {
            sink.send(UiChunk::TextDelta {
                id: self.text_id.clone(),
                delta: delta.to_string(),
            });
        }
    }

    pub fn push_message_part(&self, part: &str) {
        if part.is_empty() || self.cancelled.load(Ordering::Acquire) {
            return;
        }
        let continuation = {
            let state = self.state.lock().unwrap();
            if state.sink.is_none() {
                return;
            }
            let streamed = state.buffer.trim();
            if streamed.is_empty() {
                Some(part.to_string())
            } else {
                match part.trim().strip_prefix(streamed) {
                    Some(remainder) if remainder.trim().is_empty() => None,
                    Some(remainder) => Some(remainder.to_string()),
                    None => Some(format!("{PART_SEPARATOR}{part}")),
                }
            }
        };
        if let Some(continuation) = continuation {
            self.push_text_delta(&continuation);
        }
    }

    pub fn push_activity(&self, label: &str) {
        self.emit(UiChunk::Activity {
            data: ActivityData {
                label: label.to_string(),
            },
            transient: true,
        });
    }

    pub(super) fn resume_with_sink<S: ChunkSink>(&self, sink: S) {
        let mut state = self.state.lock().unwrap();
        state.sink = Some(Box::new(sink));
        let replay = state.buffer.clone();
        let sink_ref = state.sink.as_ref().expect("sink was just installed");
        sink_ref.send(UiChunk::Start {
            message_id: self.reply_id.clone(),
        });
        sink_ref.send(UiChunk::TextStart {
            id: self.text_id.clone(),
        });
        if !replay.is_empty() {
            sink_ref.send(UiChunk::TextDelta {
                id: self.text_id.clone(),
                delta: replay,
            });
        }
    }

    fn snapshot(&self) -> TurnSnapshot {
        TurnSnapshot {
            session_id: self.session_id.clone(),
            parent_message_id: self.parent_message_id.clone(),
        }
    }

    fn reply_message(&self, content: &str) -> ChatMsg {
        ChatMsg {
            id: self.reply_id.clone(),
            role: "assistant".to_string(),
            content: content.to_string(),
            status: "completed".to_string(),
            parent_message_id: Some(self.parent_message_id.clone()),
            error_code: None,
            error_message: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    fn error_message(&self, error_text: &str) -> ChatMsg {
        ChatMsg {
            id: self.reply_id.clone(),
            role: "assistant".to_string(),
            content: String::new(),
            status: "errored".to_string(),
            parent_message_id: Some(self.parent_message_id.clone()),
            error_code: None,
            error_message: Some(error_text.to_string()),
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    fn emit_reply(&self, sink: &dyn ChunkSink, buffered: &str, content: &str) {
        let missing_suffix = content
            .strip_prefix(buffered)
            .filter(|suffix| !suffix.is_empty());
        if let Some(suffix) = missing_suffix {
            sink.send(UiChunk::TextDelta {
                id: self.text_id.clone(),
                delta: suffix.to_string(),
            });
        }
        sink.send(UiChunk::TextEnd {
            id: self.text_id.clone(),
        });
        sink.send(UiChunk::Finish {
            finish_reason: "stop".to_string(),
        });
    }

    fn emit_error(&self, sink: &dyn ChunkSink, error_text: &str) {
        sink.send(UiChunk::Error {
            error_text: error_text.to_string(),
        });
        sink.send(UiChunk::Finish {
            finish_reason: "error".to_string(),
        });
    }

    pub(super) fn terminate_with_outcome(
        &self,
        outcome: TurnOutcome,
        persistence: &dyn TurnPersistence,
    ) -> TerminationOutcome {
        let (sink, buffered) = {
            let mut state = self.state.lock().unwrap();
            let sink = state
                .sink
                .take()
                .expect("the registry hands out a taken turn entry exactly once");
            (sink, state.buffer.clone())
        };
        let snapshot = self.snapshot();
        match outcome {
            TurnOutcome::Reply(content) => {
                let content = if content.is_empty() {
                    buffered.clone()
                } else {
                    content
                };
                match persistence.persist_reply(&self.session_id, self.reply_message(&content)) {
                    Ok(ChatAppendOutcome::Inserted) | Ok(ChatAppendOutcome::IdenticalDuplicate) => {
                        self.emit_reply(&*sink, &buffered, &content);
                        TerminationOutcome::Replied(snapshot)
                    }
                    Ok(ChatAppendOutcome::ConflictingDuplicate)
                    | Ok(ChatAppendOutcome::MissingSession)
                    | Err(_) => {
                        self.emit_error(&*sink, REPLY_PERSIST_FAILED_ERROR_TEXT);
                        TerminationOutcome::Failed(snapshot)
                    }
                }
            }
            TurnOutcome::Failure(error_text) => {
                let _ =
                    persistence.persist_reply(&self.session_id, self.error_message(&error_text));
                self.emit_error(&*sink, &error_text);
                TerminationOutcome::Failed(snapshot)
            }
        }
    }

    pub(super) fn terminate_with_error(&self, error_text: &str) -> bool {
        let sink = {
            let mut state = self.state.lock().unwrap();
            match state.sink.take() {
                Some(sink) => sink,
                None => return false,
            }
        };
        self.emit_error(&*sink, error_text);
        true
    }
}
