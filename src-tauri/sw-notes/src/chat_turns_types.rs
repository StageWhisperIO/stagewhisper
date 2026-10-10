use uuid::Uuid;

use crate::chat_stream::UiChunk;
use crate::store::{ChatAppendOutcome, ChatMsg};

pub trait ChunkSink: Send + Sync + 'static {
    fn send(&self, chunk: UiChunk);
}

impl<F> ChunkSink for F
where
    F: Fn(UiChunk) + Send + Sync + 'static,
{
    fn send(&self, chunk: UiChunk) {
        self(chunk)
    }
}

pub trait TurnPersistence {
    fn persist_reply(
        &self,
        session_id: &str,
        message: ChatMsg,
    ) -> Result<ChatAppendOutcome, String>;
}

#[derive(Debug)]
pub enum TurnOutcome {
    Reply(String),
    Failure(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TurnSnapshot {
    pub session_id: String,
    pub parent_message_id: String,
}

#[derive(Debug, PartialEq)]
pub enum TerminationOutcome {
    Replied(TurnSnapshot),
    Failed(TurnSnapshot),
}

#[derive(Debug, PartialEq)]
pub enum TerminalResult {
    Terminated(TerminationOutcome),
    AlreadyTerminated,
    Unauthorized,
    Unknown,
}

pub fn turn_reply_id(turn_id: &str) -> String {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, turn_id.as_bytes()).to_string()
}
