mod commands;
mod local_producer;
mod relay_producer;

pub use commands::{
    cancel_session_chat_turn, resume_session_chat_turn, stream_session_chat_message,
};
pub use sw_notes::TurnRegistry;

use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::Manager;

use sw_notes::{TerminalResult, TurnOutcome, TurnPersistence, UiChunk};

pub(crate) const LOCAL_IDENTITY_BINDING: &str = "desktop-free-local-identity";

pub(super) fn channel_sink(on_chunk: Channel<UiChunk>) -> impl sw_notes::ChunkSink {
    move |chunk: UiChunk| {
        let _ = on_chunk.send(chunk);
    }
}

pub(crate) struct StorePersistence<'a> {
    pub(crate) store: &'a sw_notes::SessionStore,
}

impl TurnPersistence for StorePersistence<'_> {
    fn persist_reply(
        &self,
        session_id: &str,
        message: sw_notes::ChatMsg,
    ) -> Result<sw_notes::ChatAppendOutcome, String> {
        self.store
            .append_chat(session_id, message)
            .map_err(|err| err.to_string())
    }
}

pub(crate) fn log_non_terminated_result(site: &str, turn_id: &str, result: &TerminalResult) {
    match result {
        TerminalResult::Terminated(_) => {}
        TerminalResult::AlreadyTerminated => {
            eprintln!(
                "[session_chat] {site}: turn_id={turn_id} was already terminated by another source"
            );
        }
        TerminalResult::Unauthorized => {
            eprintln!(
                "[session_chat] {site}: turn_id={turn_id} refused to terminate, binding mismatch"
            );
        }
        TerminalResult::Unknown => {
            eprintln!("[session_chat] {site}: turn_id={turn_id} was never registered");
        }
    }
}

pub(crate) fn apply_relay_turn_outcome(
    registry: &TurnRegistry,
    store: &sw_notes::SessionStore,
    task_id: &str,
    status: &str,
    reply_text: Option<&str>,
    error_code: Option<&str>,
    error_message: Option<&str>,
) -> bool {
    let outcome = match status {
        "completed" | "silent" => TurnOutcome::Reply(reply_text.unwrap_or_default().to_string()),
        "errored" => TurnOutcome::Failure(
            error_message
                .or(error_code)
                .unwrap_or("Your assistant returned an error.")
                .to_string(),
        ),
        _ => return false,
    };

    let persistence = StorePersistence { store };
    let result = registry.terminate(task_id, outcome, Some(LOCAL_IDENTITY_BINDING), &persistence);
    log_non_terminated_result("relay turn outcome", task_id, &result);
    match result {
        TerminalResult::Terminated(_) => true,
        TerminalResult::AlreadyTerminated => true,
        TerminalResult::Unauthorized => true,
        TerminalResult::Unknown => false,
    }
}

fn relay_turn_registry(app: &tauri::AppHandle) -> Option<Arc<TurnRegistry>> {
    app.try_state::<Arc<TurnRegistry>>()
        .map(|registry| registry.inner().clone())
}

pub(crate) fn push_relay_turn_part(app: &tauri::AppHandle, task_id: &str, text: &str) -> bool {
    relay_turn_registry(app).is_some_and(|registry| {
        registry.push_message_part(task_id, Some(LOCAL_IDENTITY_BINDING), text)
    })
}

pub(crate) fn push_relay_turn_text_delta(
    app: &tauri::AppHandle,
    task_id: &str,
    delta: &str,
) -> bool {
    relay_turn_registry(app).is_some_and(|registry| {
        registry.push_text_delta(task_id, Some(LOCAL_IDENTITY_BINDING), delta)
    })
}

pub(crate) fn relay_turn_is_retired(app: &tauri::AppHandle, task_id: &str) -> bool {
    relay_turn_registry(app).is_some_and(|registry| registry.is_retired(task_id))
}

pub(crate) fn complete_relay_turn(
    app: &tauri::AppHandle,
    task_id: &str,
    status: &str,
    reply_text: Option<&str>,
    error_code: Option<&str>,
    error_message: Option<&str>,
) -> bool {
    let Some(registry) = app.try_state::<Arc<TurnRegistry>>() else {
        return false;
    };
    let registry = registry.inner().clone();
    let Ok(store) = crate::session_store(app) else {
        if !registry.contains(task_id) {
            return false;
        }
        registry.cancel(task_id);
        return true;
    };
    apply_relay_turn_outcome(
        &registry,
        &store,
        task_id,
        status,
        reply_text,
        error_code,
        error_message,
    )
}

#[cfg(test)]
#[path = "session_chat_tests.rs"]
mod tests;
