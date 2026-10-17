use std::sync::Arc;
use std::time::Duration;

use tauri::Manager;

use sw_notes::{TurnHandle, TurnOutcome, TurnRegistry};

use crate::reply_router::{PendingReplies, TimeoutCheck};

use super::{StorePersistence, LOCAL_IDENTITY_BINDING};

const RELAY_REPLY_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(90);
const RELAY_REPLY_TIMEOUT_MESSAGE: &str = "No reply received before timeout";

pub(super) fn parse_relay_target(
    record: &sw_notes::SessionRecord,
    turn_id: &str,
) -> Result<(uuid::Uuid, uuid::Uuid), String> {
    let relay_session = uuid::Uuid::parse_str(&record.relay_session_id)
        .map_err(|_| "session has invalid relay id".to_string())?;
    let task_id =
        uuid::Uuid::parse_str(turn_id).map_err(|_| "turn id must be a uuid".to_string())?;
    Ok((relay_session, task_id))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn send_relay_turn(
    app: &tauri::AppHandle,
    store: &Arc<sw_notes::SessionStore>,
    registry: &Arc<TurnRegistry>,
    handle: Arc<TurnHandle>,
    settings: &crate::relay_settings::RelaySettings,
    record: &sw_notes::SessionRecord,
    relay_session: uuid::Uuid,
    task_id: uuid::Uuid,
    session_id: &str,
    turn_id: &str,
    user_message_id: &str,
    text: &str,
) -> Result<(), String> {
    let relay = app
        .state::<Arc<tokio::sync::RwLock<crate::relay::RelayClient>>>()
        .inner()
        .clone();
    let pending = app
        .try_state::<Arc<PendingReplies>>()
        .map(|s| s.inner().clone());
    if let Some(pending) = pending.as_ref() {
        pending.register(task_id.to_string(), relay_session.to_string());
    }

    let stream_result = crate::reply_stream::acquire_task(
        app,
        settings,
        &relay_session.to_string(),
        &task_id.to_string(),
    )
    .await;

    let outbound_text = sw_notes::build_relay_chat_outbound(
        &record.segments,
        &record.chat,
        record.notes_markdown.as_deref(),
        text,
    );

    let send_result = match stream_result {
        Ok(()) => {
            let guard = relay.read().await;
            guard
                .send_session_chat(
                    settings,
                    relay_session,
                    outbound_text,
                    Some(user_message_id.to_string()),
                    task_id,
                )
                .await
                .map_err(|error| error.to_string())
        }
        Err(error) => Err(error),
    };

    let outcome = send_result;
    if outcome.is_err() {
        crate::reply_stream::release_task(app, &relay_session.to_string(), &task_id.to_string())
            .await;
    }
    let status = if outcome.is_ok() {
        "completed"
    } else {
        "errored"
    };
    let _ = store.update_chat_status(
        session_id,
        user_message_id,
        status,
        outcome.as_ref().err().map(String::as_str),
    );
    let result = apply_send_outcome(registry, store.as_ref(), &handle, turn_id, outcome);
    if result.is_ok() {
        spawn_relay_reply_watchdog(
            registry.clone(),
            store.clone(),
            pending,
            turn_id.to_string(),
        );
    }
    result
}

fn apply_send_outcome(
    registry: &TurnRegistry,
    store: &sw_notes::SessionStore,
    handle: &TurnHandle,
    turn_id: &str,
    outcome: Result<(), String>,
) -> Result<(), String> {
    match outcome {
        Ok(()) => {
            handle.push_activity("Waiting for a reply...");
            Ok(())
        }
        Err(message) => {
            let persistence = StorePersistence { store };
            let result = registry.terminate(
                turn_id,
                TurnOutcome::Failure(message.clone()),
                Some(LOCAL_IDENTITY_BINDING),
                &persistence,
            );
            super::log_non_terminated_result("relay send failure", turn_id, &result);
            Err(message)
        }
    }
}

async fn run_relay_reply_watchdog(
    registry: Arc<TurnRegistry>,
    store: Arc<sw_notes::SessionStore>,
    pending: Option<Arc<PendingReplies>>,
    turn_id: String,
    timeout: Duration,
) {
    let Some(pending) = pending else {
        tokio::time::sleep(timeout).await;
        crate::state::session_chat::apply_relay_turn_outcome(
            &registry,
            &store,
            &turn_id,
            "errored",
            None,
            Some("reply_timeout"),
            Some(RELAY_REPLY_TIMEOUT_MESSAGE),
        );
        return;
    };

    loop {
        match pending.check_or_claim_timeout(&turn_id, timeout) {
            TimeoutCheck::StillFresh { remaining } => {
                tokio::time::sleep(remaining).await;
            }
            TimeoutCheck::NoLongerTracked => return,
            TimeoutCheck::Claimed => break,
        }
    }

    crate::state::session_chat::apply_relay_turn_outcome(
        &registry,
        &store,
        &turn_id,
        "errored",
        None,
        Some("reply_timeout"),
        Some(RELAY_REPLY_TIMEOUT_MESSAGE),
    );
    pending.complete(&turn_id);
}

fn spawn_relay_reply_watchdog(
    registry: Arc<TurnRegistry>,
    store: Arc<sw_notes::SessionStore>,
    pending: Option<Arc<PendingReplies>>,
    turn_id: String,
) {
    tokio::spawn(run_relay_reply_watchdog(
        registry,
        store,
        pending,
        turn_id,
        RELAY_REPLY_WATCHDOG_TIMEOUT,
    ));
}

#[cfg(test)]
#[path = "relay_producer_tests.rs"]
mod tests;
