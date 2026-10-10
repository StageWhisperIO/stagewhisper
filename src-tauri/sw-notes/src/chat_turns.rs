use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::chat_stream::UiChunk;

#[path = "chat_turns_types.rs"]
mod chat_turns_types;

#[path = "chat_turns_handle.rs"]
mod chat_turns_handle;

pub use chat_turns_handle::TurnHandle;
pub use chat_turns_types::{
    ChunkSink, TerminalResult, TerminationOutcome, TurnOutcome, TurnPersistence, TurnSnapshot,
};

const RETIRED_TURN_ID_CAPACITY: usize = 256;
const CANCELLED_TURN_ERROR_TEXT: &str = "This response was cancelled.";

struct TurnEntry {
    handle: Arc<TurnHandle>,
    sequence: u64,
}

enum TakeOutcome {
    Taken(TurnEntry),
    AlreadyRetired,
    NeverRegistered,
}

#[derive(Default)]
struct RetiredTurnIds {
    order: VecDeque<String>,
    members: HashSet<String>,
}

impl RetiredTurnIds {
    fn contains(&self, turn_id: &str) -> bool {
        self.members.contains(turn_id)
    }

    fn record(&mut self, turn_id: String) {
        if !self.members.insert(turn_id.clone()) {
            return;
        }
        self.order.push_back(turn_id);
        if self.order.len() > RETIRED_TURN_ID_CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.members.remove(&oldest);
            }
        }
    }
}

#[derive(Default)]
struct RegistryState {
    turns: HashMap<String, TurnEntry>,
    retired: RetiredTurnIds,
}

#[derive(Default)]
pub struct TurnRegistry {
    state: Mutex<RegistryState>,
    next_sequence: AtomicU64,
}

impl TurnRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use = "a rejected registration means the caller owns no turn and must not stream to it"]
    pub fn register<S: ChunkSink>(
        &self,
        turn_id: impl Into<String>,
        session_id: impl Into<String>,
        parent_message_id: impl Into<String>,
        text_id: impl Into<String>,
        binding: Option<String>,
        sink: S,
    ) -> Option<Arc<TurnHandle>> {
        let turn_id = turn_id.into();
        let mut state = self.state.lock().unwrap();
        if state.retired.contains(&turn_id) {
            return None;
        }
        let vacant = match state.turns.entry(turn_id.clone()) {
            Entry::Occupied(_) => return None,
            Entry::Vacant(vacant) => vacant,
        };
        let handle = Arc::new(TurnHandle::new(
            turn_id,
            session_id.into(),
            parent_message_id.into(),
            text_id.into(),
            binding,
            sink,
        ));
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        vacant.insert(TurnEntry {
            handle: handle.clone(),
            sequence,
        });
        drop(state);
        handle.emit(UiChunk::Start {
            message_id: handle.reply_id().to_string(),
        });
        handle.emit(UiChunk::TextStart {
            id: handle.text_id().to_string(),
        });
        Some(handle)
    }

    pub fn get(&self, turn_id: &str) -> Option<Arc<TurnHandle>> {
        self.state
            .lock()
            .unwrap()
            .turns
            .get(turn_id)
            .map(|entry| entry.handle.clone())
    }

    pub fn contains(&self, turn_id: &str) -> bool {
        self.state.lock().unwrap().turns.contains_key(turn_id)
    }

    pub fn is_retired(&self, turn_id: &str) -> bool {
        self.state.lock().unwrap().retired.contains(turn_id)
    }

    fn take_entry(&self, turn_id: &str) -> TakeOutcome {
        let mut state = self.state.lock().unwrap();
        if let Some(entry) = state.turns.remove(turn_id) {
            state.retired.record(turn_id.to_string());
            return TakeOutcome::Taken(entry);
        }
        if state.retired.contains(turn_id) {
            TakeOutcome::AlreadyRetired
        } else {
            TakeOutcome::NeverRegistered
        }
    }

    fn authorized(&self, turn_id: &str, binding: Option<&str>) -> bool {
        let state = self.state.lock().unwrap();
        match state.turns.get(turn_id) {
            Some(entry) => matches!(
                (entry.handle.binding(), binding),
                (Some(captured), Some(current)) if captured == current
            ),
            None => true,
        }
    }

    pub fn cancel(&self, turn_id: &str) -> bool {
        let entry = match self.take_entry(turn_id) {
            TakeOutcome::Taken(entry) => entry,
            TakeOutcome::AlreadyRetired | TakeOutcome::NeverRegistered => return false,
        };
        entry.handle.mark_cancelled();
        entry.handle.terminate_with_error(CANCELLED_TURN_ERROR_TEXT)
    }

    pub fn resume<S: ChunkSink>(
        &self,
        session_id: &str,
        binding: Option<&str>,
        sink: S,
    ) -> Option<String> {
        let state = self.state.lock().unwrap();
        let (turn_id, handle) = state
            .turns
            .iter()
            .filter(|(_, entry)| entry.handle.session_id() == session_id)
            .max_by_key(|(_, entry)| entry.sequence)
            .map(|(id, entry)| (id.clone(), entry.handle.clone()))?;
        let authorized = matches!(
            (handle.binding(), binding),
            (Some(captured), Some(current)) if captured == current
        );
        drop(state);
        if !authorized {
            return None;
        }
        handle.resume_with_sink(sink);
        Some(turn_id)
    }

    pub fn abandon(&self, turn_id: &str) -> bool {
        matches!(self.take_entry(turn_id), TakeOutcome::Taken(_))
    }

    pub fn has_reply_to(&self, session_id: &str, parent_message_id: &str) -> bool {
        self.state.lock().unwrap().turns.values().any(|entry| {
            entry.handle.session_id() == session_id
                && entry.handle.parent_message_id() == parent_message_id
        })
    }

    pub fn resume_reply_to<S: ChunkSink>(
        &self,
        session_id: &str,
        parent_message_id: &str,
        binding: Option<&str>,
        sink: S,
    ) -> Option<String> {
        let state = self.state.lock().unwrap();
        let (turn_id, handle) = state
            .turns
            .iter()
            .filter(|(_, entry)| {
                entry.handle.session_id() == session_id
                    && entry.handle.parent_message_id() == parent_message_id
            })
            .max_by_key(|(_, entry)| entry.sequence)
            .map(|(id, entry)| (id.clone(), entry.handle.clone()))?;
        let authorized = matches!(
            (handle.binding(), binding),
            (Some(captured), Some(current)) if captured == current
        );
        drop(state);
        if !authorized {
            return None;
        }
        handle.resume_with_sink(sink);
        Some(turn_id)
    }

    pub fn push_text_delta(&self, turn_id: &str, binding: Option<&str>, delta: &str) -> bool {
        if !self.authorized(turn_id, binding) {
            return false;
        }
        match self.get(turn_id) {
            Some(handle) => {
                handle.push_text_delta(delta);
                true
            }
            None => false,
        }
    }

    pub fn push_message_part(&self, turn_id: &str, binding: Option<&str>, part: &str) -> bool {
        if !self.authorized(turn_id, binding) {
            return false;
        }
        match self.get(turn_id) {
            Some(handle) => {
                handle.push_message_part(part);
                true
            }
            None => false,
        }
    }

    pub fn push_activity(&self, turn_id: &str, label: &str) -> bool {
        match self.get(turn_id) {
            Some(handle) => {
                handle.push_activity(label);
                true
            }
            None => false,
        }
    }

    pub fn terminate(
        &self,
        turn_id: &str,
        outcome: TurnOutcome,
        binding: Option<&str>,
        persistence: &dyn TurnPersistence,
    ) -> TerminalResult {
        if !self.authorized(turn_id, binding) {
            return TerminalResult::Unauthorized;
        }
        match self.take_entry(turn_id) {
            TakeOutcome::NeverRegistered => TerminalResult::Unknown,
            TakeOutcome::AlreadyRetired => TerminalResult::AlreadyTerminated,
            TakeOutcome::Taken(entry) => TerminalResult::Terminated(
                entry.handle.terminate_with_outcome(outcome, persistence),
            ),
        }
    }
}

#[cfg(test)]
#[path = "chat_turns_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "chat_turns_reply_tests.rs"]
mod reply_tests;

#[cfg(test)]
#[path = "chat_turns_registration_tests.rs"]
mod registration_tests;

#[cfg(test)]
#[path = "chat_turns_cancel_tests.rs"]
mod cancel_tests;

#[cfg(test)]
#[path = "chat_turns_terminate_tests.rs"]
mod terminate_tests;
