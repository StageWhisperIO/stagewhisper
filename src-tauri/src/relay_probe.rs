use std::sync::Arc;
use std::time::Duration;

use sw_reply_stream::{subscribe, ReplyStreamConfig, ReplyStreamEvent, ReplyStreamHandle};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::relay::RelayClient;
use crate::relay_settings::RelaySettingsStore;
use crate::reply_router::{ProbeOutcome, ProbeRegistry, ReplySink, TauriReplySink};
use crate::reply_stream::route_reply_payload;

const PROBE_TEXT: &str = "StageWhisper connection check. Please reply with \"ok\".";
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

const PROBE_TIMEOUT_MESSAGE: &str =
    "Your assistant didn't answer in time. Check that it's running, then try again.";
const PROBE_DROPPED_MESSAGE: &str =
    "Couldn't get an answer from your assistant. Check that it's running, then try again.";

pub async fn run_relay_probe(app: &AppHandle) -> Result<ProbeOutcome, String> {
    let settings = app
        .state::<Arc<RelaySettingsStore>>()
        .inner()
        .clone()
        .snapshot();
    if !settings.has_relay() {
        return Err("Relay not configured".to_string());
    }

    let relay = app
        .state::<Arc<tokio::sync::RwLock<RelayClient>>>()
        .inner()
        .clone();
    let registry = app.state::<Arc<ProbeRegistry>>().inner().clone();

    let probe_session = Uuid::new_v4();
    let task_id = Uuid::new_v4();
    let user_message_id = Uuid::new_v4().to_string();

    let sink = Arc::new(TauriReplySink { app: app.clone() });
    let config = ReplyStreamConfig {
        base_url: settings.relay_url.clone(),
        token: settings.relay_token.clone(),
        session_id: probe_session.to_string(),
    };

    probe_over_stream(
        sink,
        config,
        registry.as_ref(),
        task_id,
        PROBE_TIMEOUT,
        move || async move {
            let guard = relay.read().await;
            guard
                .send_session_chat(
                    &settings,
                    probe_session,
                    PROBE_TEXT.to_string(),
                    Some(user_message_id),
                    task_id,
                )
                .await
                .map_err(|err| err.to_string())
        },
    )
    .await
}

pub(crate) async fn probe_over_stream<S, F, Fut>(
    sink: Arc<S>,
    config: ReplyStreamConfig,
    registry: &ProbeRegistry,
    task_id: Uuid,
    timeout: Duration,
    send: F,
) -> Result<ProbeOutcome, String>
where
    S: ReplySink + 'static,
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let rx = registry.register(task_id.to_string());
    let handle = subscribe_probe_stream(sink, config);

    if let Err(err) = send().await {
        registry.cancel(&task_id.to_string());
        handle.cancel().await;
        return Err(err);
    }

    let received = tokio::time::timeout(timeout, rx).await;
    handle.cancel().await;

    match received {
        Ok(Ok(outcome)) => Ok(outcome),
        Ok(Err(_)) => Err(PROBE_DROPPED_MESSAGE.to_string()),
        Err(_) => {
            registry.cancel(&task_id.to_string());
            Err(PROBE_TIMEOUT_MESSAGE.to_string())
        }
    }
}

fn subscribe_probe_stream<S: ReplySink + 'static>(
    sink: Arc<S>,
    config: ReplyStreamConfig,
) -> ReplyStreamHandle {
    subscribe(config, move |event| {
        if let ReplyStreamEvent::Reply { payload, .. } = event {
            route_reply_payload(sink.as_ref(), payload);
        }
    })
}

pub fn probe_error_message(outcome: &ProbeOutcome) -> String {
    outcome.error_message.clone().unwrap_or_else(|| {
        "Your assistant couldn't be reached. Make sure it's running and has approved this device, then try again."
            .to_string()
    })
}

pub fn probe_indicates_reachable(status: &str) -> bool {
    matches!(status, "completed" | "message")
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    reachable: bool,
    reply: Option<String>,
}

#[tauri::command]
pub async fn probe_agent_pairing(app: AppHandle) -> Result<ProbeResult, String> {
    let outcome = run_relay_probe(&app).await?;
    if probe_indicates_reachable(&outcome.status) {
        Ok(ProbeResult {
            reachable: true,
            reply: outcome.reply_text,
        })
    } else {
        Err(probe_error_message(&outcome))
    }
}

#[tauri::command]
pub async fn confirm_device_approved(
    app: AppHandle,
) -> Result<crate::relay_settings::RelaySettings, String> {
    let outcome = run_relay_probe(&app).await?;
    if !probe_indicates_reachable(&outcome.status) {
        return Err(probe_error_message(&outcome));
    }
    let store = app.state::<Arc<RelaySettingsStore>>().inner().clone();
    let updated = store
        .update(|s| s.paired_verified = true)
        .map_err(|e| e.to_string())?;
    if let Err(err) = crate::ensure_session_storage(&app) {
        eprintln!("[device-key] deferred session storage init failed: {err}");
    }
    crate::notify_relay_changed(&app, &updated).await;
    Ok(updated)
}

#[cfg(test)]
#[path = "relay_probe_tests.rs"]
mod tests;
