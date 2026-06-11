mod app_settings;
mod audio;
mod audio_local;
mod chat_reply_listener;
mod glass_buttons;
mod notes;
mod pairing;
mod panels;
mod relay;
mod relay_settings;
mod shortcuts;
mod state;
mod updater;

use std::sync::{Arc, Mutex};

use panels::{
    close_settings_window, get_screen_share_privacy, init_panels, open_settings_window, quit_app,
    set_onboarding_visible, setup_settings_close_intercept, toggle_screen_share_visibility,
};
use state::app_state::AppState;
use state::local_llm::{
    cancel_local_llm_download, delete_local_llm_model, download_local_llm_model,
    get_local_llm_status, get_responder_preference, list_local_llm_models, set_local_llm_model,
    set_responder_preference, use_hf_cache_model, use_local_llm_folder, LocalLlmDownloads,
    LocalLlmRuntime,
};
use state::local_pipeline::{
    check_models_ready, download_models, get_model_status, get_pipeline_mode,
};
use state::permissions::{
    get_permissions_status, request_microphone_permission, request_screen_recording_permission,
};
use state::session::{
    complete_session, get_current_session_id, get_session_state, toggle_session_state,
};
use tauri::{Emitter, Listener, Manager};
use tauri_plugin_opener::OpenerExt;

const RELAY_SETTINGS_CHANGED_EVENT: &str = "relay-settings-changed";

fn notify_relay_changed(app: &tauri::AppHandle, settings: &relay_settings::RelaySettings) {
    let _ = app.emit(RELAY_SETTINGS_CHANGED_EVENT, settings.ready());
    glass_buttons::sync_button_appearance(app);
}

#[derive(serde::Deserialize)]
struct SaveRelaySettingsArgs {
    relay_url: Option<String>,
    relay_token: Option<String>,
}

#[derive(serde::Deserialize)]
struct SaveCallbackSettingsArgs {
    callback_url: Option<String>,
    callback_port: Option<u16>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveAppSettingsArgs {
    mic_enabled: Option<bool>,
}

#[tauri::command]
fn get_relay_settings(app: tauri::AppHandle) -> relay_settings::RelaySettings {
    let store = app
        .state::<Arc<relay_settings::RelaySettingsStore>>()
        .inner()
        .clone();
    store.snapshot()
}

#[tauri::command]
fn get_app_settings(app: tauri::AppHandle) -> app_settings::AppSettings {
    let store = app
        .state::<Arc<app_settings::AppSettingsStore>>()
        .inner()
        .clone();
    store.snapshot()
}

#[tauri::command]
fn save_app_settings(
    app: tauri::AppHandle,
    args: SaveAppSettingsArgs,
) -> Result<app_settings::AppSettings, String> {
    let store = app
        .state::<Arc<app_settings::AppSettingsStore>>()
        .inner()
        .clone();
    let updated = store
        .update(|s| {
            if let Some(v) = args.mic_enabled {
                s.mic_enabled = v;
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(updated)
}

#[tauri::command]
async fn save_relay_settings(
    app: tauri::AppHandle,
    args: SaveRelaySettingsArgs,
) -> Result<relay_settings::RelaySettings, String> {
    let store = app
        .state::<Arc<relay_settings::RelaySettingsStore>>()
        .inner()
        .clone();
    let rotated_token = chat_reply_listener::generate_callback_token();
    let updated = store
        .update(|s| {
            if let Some(v) = args.relay_url {
                s.relay_url = v;
            }
            if let Some(v) = args.relay_token {
                s.relay_token = v;
            }
            s.callback_token = Some(rotated_token.clone());
            s.paired_verified = false;
        })
        .map_err(|e| e.to_string())?;
    apply_callback_token(&app, &updated, rotated_token).await;
    notify_relay_changed(&app, &updated);
    Ok(updated)
}

type ReplyListenerSlot = Arc<tokio::sync::Mutex<Option<chat_reply_listener::ChatReplyListener>>>;

fn resolve_advertised_callback(
    settings: &relay_settings::RelaySettings,
    env_callback_url: Option<String>,
    loopback_port: u16,
) -> String {
    if let Some(url) = &settings.callback_url {
        return url.clone();
    }
    if let Some(env) = env_callback_url {
        let trimmed = env.trim().trim_end_matches('/');
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    format!("http://127.0.0.1:{loopback_port}")
}

async fn apply_callback_token(
    app: &tauri::AppHandle,
    settings: &relay_settings::RelaySettings,
    token: String,
) {
    let slot = app.state::<ReplyListenerSlot>().inner().clone();
    let guard = slot.lock().await;
    let Some(listener) = guard.as_ref() else {
        return;
    };
    listener.set_token(token.clone());
    let advertised = resolve_advertised_callback(
        settings,
        std::env::var("STAGEWHISPER_CALLBACK_URL").ok(),
        listener.local_port(),
    );
    app.state::<Arc<tokio::sync::RwLock<relay::RelayClient>>>()
        .inner()
        .clone()
        .read()
        .await
        .set_callback(advertised, token);
}

#[tauri::command]
async fn save_callback_settings(
    app: tauri::AppHandle,
    args: SaveCallbackSettingsArgs,
) -> Result<relay_settings::RelaySettings, String> {
    let store = app
        .state::<Arc<relay_settings::RelaySettingsStore>>()
        .inner()
        .clone();
    let previous = store.snapshot();

    let desired_url = args
        .callback_url
        .as_ref()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty());
    let desired_port = args.callback_port;
    let changed =
        previous.callback_url != desired_url || previous.callback_port != desired_port;

    let slot = app.state::<ReplyListenerSlot>().inner().clone();
    let mut guard = slot.lock().await;
    let token = chat_reply_listener::ensure_callback_token(&store)
        .await
        .map_err(|e| e.to_string())?;
    let needs_new = previous.callback_port != desired_port || guard.is_none();

    let fresh = if needs_new {
        Some(
            chat_reply_listener::ChatReplyListener::start(
                app.app_handle().clone(),
                token.clone(),
                desired_port,
                desired_url.clone(),
            )
            .await
            .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };

    let updated = store
        .update(|s| {
            s.callback_url = desired_url.clone();
            s.callback_port = desired_port;
            if changed {
                s.paired_verified = false;
            }
        })
        .map_err(|e| e.to_string())?;

    if let Some(listener) = fresh {
        *guard = Some(listener);
    }

    let advertised = match guard.as_ref() {
        Some(listener) => resolve_advertised_callback(
            &updated,
            std::env::var("STAGEWHISPER_CALLBACK_URL").ok(),
            listener.local_port(),
        ),
        None => return Err("Reply listener is not ready yet.".to_string()),
    };

    app.state::<Arc<tokio::sync::RwLock<relay::RelayClient>>>()
        .inner()
        .clone()
        .read()
        .await
        .set_callback(advertised, token);
    notify_relay_changed(&app, &updated);
    Ok(updated)
}

#[tauri::command]
async fn pair_with_code(
    app: tauri::AppHandle,
    code: String,
) -> Result<relay_settings::RelaySettings, String> {
    let relay = pairing::parse_pairing_code(&code)?;
    let store = app
        .state::<Arc<relay_settings::RelaySettingsStore>>()
        .inner()
        .clone();
    let rotated_token = chat_reply_listener::generate_callback_token();
    let updated = store
        .update(|s| {
            s.relay_url = relay.url;
            s.relay_token = relay.token;
            s.callback_token = Some(rotated_token.clone());
            s.paired_verified = false;
        })
        .map_err(|e| e.to_string())?;
    apply_callback_token(&app, &updated, rotated_token).await;
    notify_relay_changed(&app, &updated);
    Ok(updated)
}

const PROBE_TEXT: &str = "StageWhisper connection check. Please reply with \"ok\".";

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeResult {
    reachable: bool,
    reply: Option<String>,
}

async fn run_relay_probe(
    app: &tauri::AppHandle,
) -> Result<chat_reply_listener::ProbeOutcome, String> {
    let settings = app
        .state::<Arc<relay_settings::RelaySettingsStore>>()
        .inner()
        .clone()
        .snapshot();
    if !settings.has_relay() {
        return Err("Relay not configured".to_string());
    }

    let relay = app
        .state::<Arc<tokio::sync::RwLock<relay::RelayClient>>>()
        .inner()
        .clone();
    {
        let guard = relay.read().await;
        if !guard.has_callback() {
            return Err("Reply listener is not ready yet. Try again in a moment.".to_string());
        }
    }

    let registry = app
        .state::<Arc<chat_reply_listener::ProbeRegistry>>()
        .inner()
        .clone();

    let probe_session = uuid::Uuid::new_v4();
    let task_id = uuid::Uuid::new_v4();
    let user_message_id = uuid::Uuid::new_v4().to_string();
    let rx = registry.register(task_id.to_string());

    let send_result = {
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
    };
    if let Err(err) = send_result {
        registry.cancel(&task_id.to_string());
        return Err(err.to_string());
    }

    match tokio::time::timeout(std::time::Duration::from_secs(20), rx).await {
        Ok(Ok(outcome)) => Ok(outcome),
        Ok(Err(_)) => {
            Err("Reply listener dropped the probe before a response arrived.".to_string())
        }
        Err(_) => {
            registry.cancel(&task_id.to_string());
            Err("Timed out waiting for your assistant to respond.".to_string())
        }
    }
}

fn probe_error_message(outcome: &chat_reply_listener::ProbeOutcome) -> String {
    outcome
        .error_message
        .clone()
        .unwrap_or_else(|| "Your assistant couldn't be reached. Make sure it's running and has approved this device, then try again.".to_string())
}

#[tauri::command]
async fn probe_agent_pairing(app: tauri::AppHandle) -> Result<ProbeResult, String> {
    let outcome = run_relay_probe(&app).await?;
    if outcome.status == "completed" {
        Ok(ProbeResult {
            reachable: true,
            reply: outcome.reply_text,
        })
    } else {
        Err(probe_error_message(&outcome))
    }
}

#[tauri::command]
async fn confirm_device_approved(
    app: tauri::AppHandle,
) -> Result<relay_settings::RelaySettings, String> {
    let outcome = run_relay_probe(&app).await?;
    if outcome.status != "completed" {
        return Err(probe_error_message(&outcome));
    }
    let store = app
        .state::<Arc<relay_settings::RelaySettingsStore>>()
        .inner()
        .clone();
    let updated = store
        .update(|s| s.paired_verified = true)
        .map_err(|e| e.to_string())?;
    if let Err(err) = ensure_session_storage(&app) {
        eprintln!("[device-key] deferred session storage init failed: {err}");
    }
    notify_relay_changed(&app, &updated);
    Ok(updated)
}

fn ensure_session_storage(app: &tauri::AppHandle) -> Result<(), String> {
    if app.try_state::<Arc<sw_notes::SessionStore>>().is_some() {
        return Ok(());
    }
    let device_key = state::device_key::DeviceKeyManager::load_or_create()?;
    let file_key = device_key.file_key()?;
    let sessions_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data dir: {e}"))?
        .join("sessions");
    let store = sw_notes::SessionStore::new(sessions_dir, file_key)
        .map_err(|e| format!("failed to init session store: {e}"))?;
    if let Ok(mut guard) = app.state::<Mutex<AppState>>().lock() {
        guard.device_key = Some(device_key);
    }
    app.manage(Arc::new(store));
    Ok(())
}

fn session_store(app: &tauri::AppHandle) -> Result<Arc<sw_notes::SessionStore>, String> {
    app.try_state::<Arc<sw_notes::SessionStore>>()
        .map(|s| s.inner().clone())
        .ok_or_else(|| "session library unavailable".to_string())
}

#[tauri::command]
fn list_sessions(app: tauri::AppHandle) -> Result<Vec<sw_notes::SessionSummary>, String> {
    session_store(&app)?.list().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_session(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<Option<sw_notes::SessionRecord>, String> {
    session_store(&app)?
        .load(&session_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_session(app: tauri::AppHandle, session_id: String) -> Result<(), String> {
    session_store(&app)?
        .delete(&session_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn send_session_chat_message(
    app: tauri::AppHandle,
    session_id: String,
    text: String,
    parent_message_id: Option<String>,
) -> Result<sw_notes::ChatMsg, String> {
    if text.trim().is_empty() {
        return Err("message is empty".to_string());
    }
    let store = session_store(&app)?;
    let settings = app
        .state::<Arc<relay_settings::RelaySettingsStore>>()
        .inner()
        .clone()
        .snapshot();
    let relay_available = settings.has_relay();
    let use_local = state::local_llm::local_ready(&app)
        && (state::local_llm::prefers_local(&app) || !relay_available);
    if !relay_available && !use_local {
        return Err("Relay not configured".to_string());
    }

    let record = store
        .load(&session_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "session not found".to_string())?;

    let user_message_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let mut user_msg = sw_notes::ChatMsg {
        id: user_message_id.clone(),
        role: "user".to_string(),
        content: text.clone(),
        status: "pending".to_string(),
        parent_message_id,
        error_code: None,
        error_message: None,
        created_at: now,
    };
    store
        .append_chat(&session_id, user_msg.clone())
        .map_err(|e| e.to_string())?;

    if use_local {
        let _ = store.update_chat_status(&session_id, &user_message_id, "completed", None);
        user_msg.status = "completed".to_string();
        spawn_local_reply(app.clone(), session_id.clone(), text, user_message_id);
        return Ok(user_msg);
    }

    let relay_session = uuid::Uuid::parse_str(&record.relay_session_id)
        .map_err(|_| "session has invalid relay id".to_string())?;
    let task_id = uuid::Uuid::new_v4();
    let relay = app
        .state::<Arc<tokio::sync::RwLock<relay::RelayClient>>>()
        .inner()
        .clone();

    if let Some(pending) = app.try_state::<Arc<chat_reply_listener::PendingReplies>>() {
        pending.register(task_id.to_string(), relay_session.to_string());
    }

    let send_result = {
        let guard = relay.read().await;
        guard
            .send_session_chat(
                &settings,
                relay_session,
                text,
                Some(user_message_id.clone()),
                task_id,
            )
            .await
    };

    match send_result {
        Ok(()) => {
            let _ = store.update_chat_status(&session_id, &user_message_id, "completed", None);
            user_msg.status = "completed".to_string();
            Ok(user_msg)
        }
        Err(err) => {
            let message = err.to_string();
            let _ =
                store.update_chat_status(&session_id, &user_message_id, "errored", Some(&message));
            Err(message)
        }
    }
}

const LOCAL_LLM_SYSTEM_PROMPT: &str = "You are StageWhisper, a concise real-time call assistant. \
Answer the user's request directly and briefly, in plain language suitable for reading mid-call.";

fn spawn_local_reply(
    app: tauri::AppHandle,
    session_id: String,
    prompt: String,
    parent_id: String,
) {
    tauri::async_runtime::spawn(async move {
        let Ok(store) = session_store(&app) else {
            return;
        };
        let result =
            state::local_llm::generate_reply(&app, Some(LOCAL_LLM_SYSTEM_PROMPT), &prompt).await;
        let now = chrono::Utc::now().to_rfc3339();
        let reply_id = uuid::Uuid::new_v4().to_string();

        match result {
            Ok(content) => {
                let msg = sw_notes::ChatMsg {
                    id: reply_id.clone(),
                    role: "assistant".to_string(),
                    content: content.clone(),
                    status: "completed".to_string(),
                    parent_message_id: Some(parent_id.clone()),
                    error_code: None,
                    error_message: None,
                    created_at: now.clone(),
                };
                let _ = store.append_chat(&session_id, msg);
                let payload = chat_reply_listener::ChatMessagePayload {
                    id: reply_id,
                    session_id: session_id.clone(),
                    role: "assistant".to_string(),
                    content,
                    status: "completed".to_string(),
                    tool_calls: None,
                    tool_result_payload: None,
                    parent_message_id: Some(parent_id),
                    suggestion_id: None,
                    error_code: None,
                    error_message: None,
                    created_at: now.clone(),
                    updated_at: now.clone(),
                    finalized_at: Some(now),
                };
                let _ = app.emit("chat-message-created", &payload);
            }
            Err(err) => {
                let msg = sw_notes::ChatMsg {
                    id: reply_id.clone(),
                    role: "assistant".to_string(),
                    content: String::new(),
                    status: "errored".to_string(),
                    parent_message_id: Some(parent_id.clone()),
                    error_code: Some("local_llm_failed".to_string()),
                    error_message: Some(err.clone()),
                    created_at: now.clone(),
                };
                let _ = store.append_chat(&session_id, msg);
                let payload = chat_reply_listener::ChatMessagePayload {
                    id: reply_id,
                    session_id: session_id.clone(),
                    role: "assistant".to_string(),
                    content: String::new(),
                    status: "errored".to_string(),
                    tool_calls: None,
                    tool_result_payload: None,
                    parent_message_id: Some(parent_id.clone()),
                    suggestion_id: None,
                    error_code: Some("local_llm_failed".to_string()),
                    error_message: Some(err.clone()),
                    created_at: now.clone(),
                    updated_at: now.clone(),
                    finalized_at: Some(now),
                };
                let _ = app.emit("chat-message-created", &payload);
                let event = serde_json::json!({
                    "session_id": session_id,
                    "user_message_id": parent_id,
                    "error_code": "local_llm_failed",
                    "error_message": err,
                });
                let _ = app.emit("chat-message-errored", &event);
            }
        }
    });
}

fn finish_setup(app_handle: &tauri::AppHandle) {
    init_panels(app_handle);
    setup_settings_close_intercept(app_handle);
    glass_buttons::inject_glass_buttons(app_handle);
    refresh_control_bar_on_local_llm_changes(app_handle);
    if let Err(err) = shortcuts::init_shortcuts(app_handle) {
        eprintln!("[shortcuts] failed to register global shortcuts: {err}");
    }
    updater::spawn_update_check(app_handle);
}

fn open_engine_on_first_run(app_handle: &tauri::AppHandle) {
    if crate::state::local_llm::engine_ready(app_handle) {
        return;
    }
    let Ok(dir) = app_handle.path().app_config_dir() else {
        return;
    };
    let marker = dir.join("engine_intro_shown");
    if marker.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(&marker, b"1");
    let handle = app_handle.clone();
    let _ = app_handle.run_on_main_thread(move || {
        let _ = crate::panels::open_settings_window(handle);
    });
}

fn refresh_control_bar_on_local_llm_changes(app_handle: &tauri::AppHandle) {
    for event in [
        "local-llm-download-complete",
        "local-llm-status-changed",
        "responder-preference-changed",
        "engine-readiness-changed",
        "model-download-complete",
    ] {
        let handle = app_handle.clone();
        app_handle.listen(event, move |_| {
            let handle_main = handle.clone();
            let _ = handle.run_on_main_thread(move || {
                glass_buttons::sync_button_appearance(&handle_main);
            });
        });
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {}))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_nspanel::init())
        .invoke_handler(tauri::generate_handler![
            toggle_screen_share_visibility,
            get_screen_share_privacy,
            open_settings_window,
            close_settings_window,
            set_onboarding_visible,
            quit_app,
            get_session_state,
            get_current_session_id,
            toggle_session_state,
            complete_session,
            get_model_status,
            download_models,
            get_pipeline_mode,
            check_models_ready,
            list_local_llm_models,
            get_local_llm_status,
            download_local_llm_model,
            cancel_local_llm_download,
            delete_local_llm_model,
            set_local_llm_model,
            use_local_llm_folder,
            use_hf_cache_model,
            get_responder_preference,
            set_responder_preference,
            get_relay_settings,
            save_relay_settings,
            save_callback_settings,
            get_app_settings,
            save_app_settings,
            pair_with_code,
            probe_agent_pairing,
            confirm_device_approved,
            list_sessions,
            get_session,
            delete_session,
            send_session_chat_message,
            get_permissions_status,
            request_microphone_permission,
            request_screen_recording_permission,
            open_microphone_privacy_settings,
            open_screen_recording_privacy_settings
        ])
        .setup(|app| {
            let initial_models_ready = sw_audio_recording::download::models_ready(
                &sw_audio_recording::download::default_model_dir(),
            );
            let mut state = AppState::default();
            state.models_ready = initial_models_ready;
            state.local_llm_prefs = state::local_llm::load_prefs();
            state.local_llm_ready = sw_local_llm::resolve(&state.local_llm_prefs.selected_model_id)
                .map(|entry| sw_local_llm::model_ready(&sw_local_llm::default_llm_dir(), &entry))
                .unwrap_or(false);
            app.manage(Mutex::new(state));
            app.manage(LocalLlmRuntime::default());
            app.manage(LocalLlmDownloads::default());

            let relay_settings_store = Arc::new(
                relay_settings::RelaySettingsStore::load(app.app_handle())
                    .expect("failed to load relay settings store"),
            );
            let already_connected = relay_settings_store.snapshot().has_relay();
            app.manage(relay_settings_store);

            let sessions_dir_exists = app
                .path()
                .app_data_dir()
                .map(|dir| dir.join("sessions").exists())
                .unwrap_or(false);
            if already_connected || sessions_dir_exists {
                if let Err(err) = ensure_session_storage(app.app_handle()) {
                    eprintln!("[device-key] startup session storage init failed: {err}");
                }
            }

            let app_settings_store = Arc::new(
                app_settings::AppSettingsStore::load(app.app_handle())
                    .expect("failed to load app settings store"),
            );
            app.manage(app_settings_store);

            let relay_client = Arc::new(tokio::sync::RwLock::new(
                relay::RelayClient::new().expect("failed to build relay client"),
            ));
            app.manage(relay_client.clone());

            let pending_replies = match app.path().app_data_dir() {
                Ok(dir) => chat_reply_listener::PendingReplies::load(dir.join("pending_tasks.json")),
                Err(err) => {
                    eprintln!("[callback] failed to resolve app data dir for pending tasks: {err}");
                    chat_reply_listener::PendingReplies::default()
                }
            };
            app.manage(Arc::new(pending_replies));
            app.manage(Arc::new(chat_reply_listener::ProbeRegistry::default()));

            let listener_slot: ReplyListenerSlot =
                Arc::new(tokio::sync::Mutex::new(None));
            app.manage(listener_slot.clone());

            let app_handle_for_listener = app.app_handle().clone();
            let relay_settings_for_listener = app
                .state::<Arc<relay_settings::RelaySettingsStore>>()
                .inner()
                .clone();
            let relay_client_for_listener = relay_client.clone();
            tauri::async_runtime::spawn(async move {
                let mut guard = listener_slot.lock().await;
                let token =
                    match chat_reply_listener::ensure_callback_token(&relay_settings_for_listener)
                        .await
                    {
                        Ok(t) => t,
                        Err(err) => {
                            eprintln!("[chat-reply-listener] failed to ensure token: {err}");
                            return;
                        }
                    };
                let settings = relay_settings_for_listener.snapshot();
                match chat_reply_listener::ChatReplyListener::start(
                    app_handle_for_listener.clone(),
                    token.clone(),
                    settings.callback_port,
                    settings.callback_url.clone(),
                )
                .await
                {
                    Ok(listener) => {
                        let callback_url = listener.callback_url();
                        relay_client_for_listener
                            .read()
                            .await
                            .set_callback(callback_url, token);
                        *guard = Some(listener);
                    }
                    Err(err) => {
                        eprintln!("[chat-reply-listener] failed to start: {err}");
                    }
                }
            });

            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            finish_setup(app.app_handle());

            open_engine_on_first_run(app.app_handle());

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build tauri app");

    app.run(|_app_handle, _event| {});
}

#[cfg(target_os = "macos")]
#[tauri::command]
fn open_microphone_privacy_settings(app: tauri::AppHandle) -> Result<(), String> {
    app.opener()
        .open_url(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
            None::<&str>,
        )
        .map_err(|e| format!("failed to open system settings: {e}"))
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
fn open_microphone_privacy_settings() -> Result<(), String> {
    Err("Microphone privacy settings are only available on macOS".to_string())
}

#[cfg(target_os = "macos")]
#[tauri::command]
fn open_screen_recording_privacy_settings(app: tauri::AppHandle) -> Result<(), String> {
    app.opener()
        .open_url(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
            None::<&str>,
        )
        .map_err(|e| format!("failed to open system settings: {e}"))
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
fn open_screen_recording_privacy_settings() -> Result<(), String> {
    Err("Screen recording privacy settings are only available on macOS".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with_callback(url: Option<&str>) -> relay_settings::RelaySettings {
        relay_settings::RelaySettings {
            callback_url: url.map(|u| u.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn persisted_callback_url_wins_over_env_and_loopback() {
        let settings = settings_with_callback(Some("https://saved.ts.net"));
        let advertised = resolve_advertised_callback(
            &settings,
            Some("https://env.ts.net".to_string()),
            8788,
        );
        assert_eq!(advertised, "https://saved.ts.net");
    }

    #[test]
    fn env_callback_url_preserved_when_no_persisted_url() {
        let settings = settings_with_callback(None);
        let advertised = resolve_advertised_callback(
            &settings,
            Some("https://env.ts.net/".to_string()),
            8788,
        );
        assert_eq!(advertised, "https://env.ts.net");
    }

    #[test]
    fn falls_back_to_loopback_when_nothing_configured() {
        let settings = settings_with_callback(None);
        let advertised = resolve_advertised_callback(&settings, None, 8788);
        assert_eq!(advertised, "http://127.0.0.1:8788");
    }
}
