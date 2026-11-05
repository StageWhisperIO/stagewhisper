use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;
use sw_reply_stream::{subscribe, ReplyStreamConfig, ReplyStreamEvent, ReplyStreamHandle};
use tauri::{AppHandle, Manager};

use crate::relay_settings::RelaySettings;
use crate::reply_router::{
    route_reply, PendingReplies, ReplyBody, ReplyDisposition, ReplySink, TauriReplySink,
};

const INITIAL_CONNECTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);
const CONNECTION_ERROR_MESSAGE: &str =
    "Couldn't open the assistant reply connection. Check Connection settings and try again.";

struct SessionSubscription {
    owners: HashSet<String>,
    handle: ReplyStreamHandle,
    ready: tokio::sync::watch::Receiver<bool>,
}

#[derive(Default)]
pub struct ReplyStreamManager {
    active: tokio::sync::Mutex<HashMap<String, SessionSubscription>>,
}

impl ReplyStreamManager {
    async fn acquire<F>(
        &self,
        settings: &RelaySettings,
        session_id: String,
        owner_id: String,
        on_event: F,
    ) where
        F: Fn(ReplyStreamEvent) + Send + Sync + 'static,
    {
        let mut guard = self.active.lock().await;
        if guard
            .get(&session_id)
            .is_some_and(|subscription| !subscription.handle.has_stopped())
        {
            if let Some(subscription) = guard.get_mut(&session_id) {
                subscription.owners.insert(owner_id);
            }
            return;
        }
        let mut owners = match guard.remove(&session_id) {
            Some(stopped) => stopped.owners,
            None => HashSet::new(),
        };
        owners.insert(owner_id);
        let config = ReplyStreamConfig {
            base_url: settings.relay_url.clone(),
            token: settings.relay_token.clone(),
            session_id: session_id.clone(),
        };
        let (ready_tx, ready) = tokio::sync::watch::channel(false);
        guard.insert(
            session_id,
            SessionSubscription {
                owners,
                handle: subscribe(config, move |event| {
                    if matches!(&event, ReplyStreamEvent::Connected) {
                        let _ = ready_tx.send(true);
                    } else if matches!(&event, ReplyStreamEvent::Disconnected { .. }) {
                        let _ = ready_tx.send(false);
                    }
                    on_event(event);
                }),
                ready,
            },
        );
    }

    async fn wait_ready(&self, session_id: &str) -> Result<(), String> {
        let mut ready = self
            .active
            .lock()
            .await
            .get(session_id)
            .map(|subscription| subscription.ready.clone())
            .ok_or_else(|| CONNECTION_ERROR_MESSAGE.to_string())?;
        if *ready.borrow() {
            return Ok(());
        }
        let connected = async {
            loop {
                ready
                    .changed()
                    .await
                    .map_err(|_| CONNECTION_ERROR_MESSAGE.to_string())?;
                if *ready.borrow() {
                    return Ok(());
                }
            }
        };
        tokio::time::timeout(INITIAL_CONNECTION_TIMEOUT, connected)
            .await
            .map_err(|_| CONNECTION_ERROR_MESSAGE.to_string())?
    }

    pub async fn release(&self, session_id: &str, owner_id: &str) {
        let removed = {
            let mut guard = self.active.lock().await;
            let Some(subscription) = guard.get_mut(session_id) else {
                return;
            };
            subscription.owners.remove(owner_id);
            if subscription.owners.is_empty() {
                guard.remove(session_id)
            } else {
                None
            }
        };
        if let Some(subscription) = removed {
            subscription.handle.cancel().await;
        }
    }

    pub async fn cancel_all(&self) {
        let removed: Vec<SessionSubscription> = self
            .active
            .lock()
            .await
            .drain()
            .map(|(_, value)| value)
            .collect();
        for subscription in removed {
            subscription.handle.cancel().await;
        }
    }

    #[cfg(test)]
    pub async fn session_count(&self) -> usize {
        self.active.lock().await.len()
    }

    #[cfg(test)]
    pub async fn owner_count(&self, session_id: &str) -> usize {
        self.active
            .lock()
            .await
            .get(session_id)
            .map(|subscription| subscription.owners.len())
            .unwrap_or_default()
    }
}

pub async fn acquire_task(
    app: &AppHandle,
    settings: &RelaySettings,
    session_id: &str,
    task_id: &str,
) -> Result<(), String> {
    if !settings.has_relay() {
        return Err(CONNECTION_ERROR_MESSAGE.to_string());
    }
    let Some(manager) = app.try_state::<Arc<ReplyStreamManager>>() else {
        return Err(CONNECTION_ERROR_MESSAGE.to_string());
    };
    let app_for_events = app.clone();
    let event_session_id = session_id.to_string();
    manager
        .inner()
        .acquire(
            settings,
            session_id.to_string(),
            task_id.to_string(),
            move |event| dispatch_event(&app_for_events, &event_session_id, event),
        )
        .await;
    if let Err(error) = manager.inner().wait_ready(session_id).await {
        manager.inner().release(session_id, task_id).await;
        return Err(error);
    }
    let Some(pending) = app.try_state::<Arc<PendingReplies>>() else {
        manager.inner().release(session_id, task_id).await;
        return Err(CONNECTION_ERROR_MESSAGE.to_string());
    };
    let app = app.clone();
    let pending = pending.inner().clone();
    let session_id = session_id.to_string();
    let task_id = task_id.to_string();
    tauri::async_runtime::spawn(async move {
        while pending.session_for(&task_id).is_some() {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        release_task(&app, &session_id, &task_id).await;
    });
    Ok(())
}

pub async fn release_task(app: &AppHandle, session_id: &str, task_id: &str) {
    if let Some(manager) = app.try_state::<Arc<ReplyStreamManager>>() {
        manager.inner().release(session_id, task_id).await;
    }
}

fn dispatch_event(app: &AppHandle, session_id: &str, event: ReplyStreamEvent) {
    match event {
        ReplyStreamEvent::Connected => {}
        ReplyStreamEvent::Reply { payload, .. } => {
            let turn_ended = payload
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(ends_turn);
            let task_id = payload
                .get("task_id")
                .and_then(Value::as_str)
                .map(str::to_string);
            let sink = TauriReplySink { app: app.clone() };
            let disposition = route_reply_payload(&sink, payload);
            let settled = disposition.as_ref().is_some_and(reply_task_is_settled);
            if turn_ended && settled {
                if let Some(task_id) = task_id {
                    spawn_release(app.clone(), session_id.to_string(), task_id);
                }
            }
        }
        ReplyStreamEvent::Disconnected { reason } => {
            eprintln!("[reply-stream] session={session_id} disconnected: {reason}");
        }
    }
}

fn reply_task_is_settled(disposition: &ReplyDisposition) -> bool {
    matches!(
        disposition,
        ReplyDisposition::Accepted
            | ReplyDisposition::AlreadyFinalized
            | ReplyDisposition::ProbeResolved
    )
}

pub(crate) fn route_reply_payload(
    sink: &dyn ReplySink,
    payload: Value,
) -> Option<ReplyDisposition> {
    let body: ReplyBody = match serde_json::from_value(payload) {
        Ok(body) => body,
        Err(err) => {
            eprintln!("[reply-stream] skipping reply frame that failed to parse: {err}");
            return None;
        }
    };
    let Some(task_id) = body.task_id.clone() else {
        eprintln!("[reply-stream] skipping reply frame missing a task id");
        return None;
    };
    Some(route_reply(sink, &task_id, body))
}

fn ends_turn(status: &str) -> bool {
    matches!(status, "completed" | "errored" | "silent")
}

fn spawn_release(app: AppHandle, session_id: String, task_id: String) {
    tauri::async_runtime::spawn(async move {
        release_task(&app, &session_id, &task_id).await;
    });
}

#[cfg(test)]
#[path = "reply_stream_tests.rs"]
mod tests;
