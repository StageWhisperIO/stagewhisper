use std::sync::Arc;

use serde_json::Value;
use sw_notes::SessionStore;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::app_state::AppState;

use super::pending::{PendingReplies, ReserveResult};
use super::probe::{ProbeOutcome, ProbeRegistry};
use super::ChatMessagePayload;

fn is_notes_turn_settled(
    status: &str,
    notes_root_message_id: Option<&str>,
    notes_pending_root_message_id: Option<&str>,
    parent_message_id: Option<&str>,
) -> bool {
    matches!(
        status,
        "completed" | "errored" | "cancelled" | "silent" | "message"
    ) && parent_message_id.is_some()
        && (notes_root_message_id == parent_message_id
            || notes_pending_root_message_id == parent_message_id)
}

pub trait ReplySink: Send + Sync + 'static {
    fn current_session_id(&self) -> Option<String>;
    fn session_known(&self, session_id: &str) -> bool;
    fn append_message(&self, message: ChatMessagePayload) -> bool;
    fn emit_created(&self, payload: &ChatMessagePayload);
    fn emit_errored(&self, payload: &Value);
    fn reserve_terminal(&self, task_id: &str, session_id: &str) -> ReserveResult;
    fn release_terminal(&self, task_id: &str);
    fn complete_terminal(&self, task_id: &str);
    fn touch_pending(&self, _task_id: &str) {}
    fn validate_task_session(&self, _task_id: &str, _session_id: &str) -> ReserveResult {
        ReserveResult::Reserved
    }
    fn resolve_probe(&self, _task_id: &str, _outcome: ProbeOutcome) -> bool {
        false
    }
    fn task_session(&self, _task_id: &str) -> Option<String> {
        None
    }
    fn route_turn_part(&self, _task_id: &str, _text: &str) -> bool {
        false
    }
    fn route_turn_text_delta(&self, _task_id: &str, _delta: &str) -> bool {
        false
    }
    fn turn_is_retired(&self, _task_id: &str) -> bool {
        false
    }
    fn forward_to_turn(
        &self,
        _task_id: &str,
        _status: &str,
        _reply_text: Option<&str>,
        _error_code: Option<&str>,
        _error_message: Option<&str>,
    ) -> bool {
        false
    }
}

pub struct TauriReplySink {
    pub app: AppHandle,
}

impl ReplySink for TauriReplySink {
    fn current_session_id(&self) -> Option<String> {
        let state = self.app.try_state::<std::sync::Mutex<AppState>>()?;
        let guard = state.lock().ok()?;
        guard.session_id.clone()
    }

    fn session_known(&self, session_id: &str) -> bool {
        self.app
            .try_state::<Arc<SessionStore>>()
            .map(|s| s.inner().clone())
            .and_then(|store| store.load(session_id).ok().flatten())
            .is_some()
    }

    fn append_message(&self, message: ChatMessagePayload) -> bool {
        let store = self
            .app
            .try_state::<Arc<SessionStore>>()
            .map(|s| s.inner().clone());

        let notes_turn_settled = store
            .as_ref()
            .and_then(|s| s.load(&message.session_id).ok().flatten())
            .map(|record| {
                is_notes_turn_settled(
                    &message.status,
                    record.notes_root_message_id.as_deref(),
                    record.notes_pending_root_message_id.as_deref(),
                    message.parent_message_id.as_deref(),
                )
            })
            .unwrap_or(false);

        let persisted = match store.as_ref() {
            Some(store) => store
                .record_reply(
                    &message.session_id,
                    message.parent_message_id.as_deref(),
                    &message.id,
                    &message.content,
                    &message.status,
                    message.error_code.as_deref(),
                    message.error_message.as_deref(),
                    &message.created_at,
                )
                .is_ok(),
            None => true,
        };
        if persisted && notes_turn_settled {
            crate::notes::apply_auto_title(&self.app, &message.session_id);
        }
        persisted
    }

    fn emit_created(&self, payload: &ChatMessagePayload) {
        let _ = self.app.emit("chat-message-created", payload);
    }

    fn emit_errored(&self, payload: &Value) {
        let _ = self.app.emit("chat-message-errored", payload);
    }

    fn reserve_terminal(&self, task_id: &str, session_id: &str) -> ReserveResult {
        match self.app.try_state::<Arc<PendingReplies>>() {
            Some(pending) => pending.reserve(task_id, session_id),
            None => ReserveResult::Reserved,
        }
    }

    fn release_terminal(&self, task_id: &str) {
        if let Some(pending) = self.app.try_state::<Arc<PendingReplies>>() {
            pending.release(task_id);
        }
    }

    fn complete_terminal(&self, task_id: &str) {
        if let Some(pending) = self.app.try_state::<Arc<PendingReplies>>() {
            pending.complete(task_id);
        }
    }

    fn touch_pending(&self, task_id: &str) {
        if let Some(pending) = self.app.try_state::<Arc<PendingReplies>>() {
            pending.touch(task_id);
        }
    }

    fn validate_task_session(&self, task_id: &str, session_id: &str) -> ReserveResult {
        match self.app.try_state::<Arc<PendingReplies>>() {
            Some(pending) => pending.validate(task_id, session_id),
            None => ReserveResult::Reserved,
        }
    }

    fn resolve_probe(&self, task_id: &str, outcome: ProbeOutcome) -> bool {
        match self.app.try_state::<Arc<ProbeRegistry>>() {
            Some(registry) => match registry.take(task_id) {
                Some(tx) => {
                    let _ = tx.send(outcome);
                    true
                }
                None => false,
            },
            None => false,
        }
    }

    fn task_session(&self, task_id: &str) -> Option<String> {
        self.app
            .try_state::<Arc<PendingReplies>>()
            .and_then(|pending| pending.session_for(task_id))
    }

    fn route_turn_part(&self, task_id: &str, text: &str) -> bool {
        crate::state::session_chat::push_relay_turn_part(&self.app, task_id, text)
    }

    fn route_turn_text_delta(&self, task_id: &str, delta: &str) -> bool {
        crate::state::session_chat::push_relay_turn_text_delta(&self.app, task_id, delta)
    }

    fn turn_is_retired(&self, task_id: &str) -> bool {
        crate::state::session_chat::relay_turn_is_retired(&self.app, task_id)
    }

    fn forward_to_turn(
        &self,
        task_id: &str,
        status: &str,
        reply_text: Option<&str>,
        error_code: Option<&str>,
        error_message: Option<&str>,
    ) -> bool {
        crate::state::session_chat::complete_relay_turn(
            &self.app,
            task_id,
            status,
            reply_text,
            error_code,
            error_message,
        )
    }
}

#[cfg(test)]
#[path = "sink_tests.rs"]
mod tests;
