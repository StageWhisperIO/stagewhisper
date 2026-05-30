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
use state::local_pipeline::{
    check_models_ready, download_models, get_model_status, get_pipeline_mode,
};
use state::permissions::{get_permissions_status, request_microphone_permission};
use state::session::{
    complete_session, get_current_session_id, get_session_state, toggle_session_state,
};
use tauri::{Emitter, Manager};
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
fn save_relay_settings(
    app: tauri::AppHandle,
    args: SaveRelaySettingsArgs,
) -> Result<relay_settings::RelaySettings, String> {
    let store = app
        .state::<Arc<relay_settings::RelaySettingsStore>>()
        .inner()
        .clone();
    let updated = store
        .update(|s| {
            if let Some(v) = args.relay_url {
                s.relay_url = v;
            }
            if let Some(v) = args.relay_token {
                s.relay_token = v;
            }
            s.paired_verified = false;
        })
        .map_err(|e| e.to_string())?;
    notify_relay_changed(&app, &updated);
    Ok(updated)
}

#[tauri::command]
fn pair_with_code(
    app: tauri::AppHandle,
    code: String,
) -> Result<relay_settings::RelaySettings, String> {
    let relay = pairing::parse_pairing_code(&code)?;
    let store = app
        .state::<Arc<relay_settings::RelaySettingsStore>>()
        .inner()
        .clone();
    let updated = store
        .update(|s| {
            s.relay_url = relay.url;
            s.relay_token = relay.token;
            s.paired_verified = false;
        })
        .map_err(|e| e.to_string())?;
    notify_relay_changed(&app, &updated);
    Ok(updated)
}

const PROBE_TEXT: &str = "StageWhisper connection check. Please reply with \"ok\".";

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeResult {
    reachable: bool,
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
        Ok(ProbeResult { reachable: true })
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
    notify_relay_changed(&app, &updated);
    Ok(updated)
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
    if !settings.has_relay() {
        return Err("Relay not configured".to_string());
    }

    let record = store
        .load(&session_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "session not found".to_string())?;
    let relay_session = uuid::Uuid::parse_str(&record.relay_session_id)
        .map_err(|_| "session has invalid relay id".to_string())?;

    let relay = app
        .state::<Arc<tokio::sync::RwLock<relay::RelayClient>>>()
        .inner()
        .clone();

    let user_message_id = uuid::Uuid::new_v4().to_string();
    let task_id = uuid::Uuid::new_v4();
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

    if let Some(pending) = app.try_state::<Arc<chat_reply_listener::PendingReplies>>() {
        pending.register(task_id.to_string());
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

fn finish_setup(app_handle: &tauri::AppHandle) {
    init_panels(app_handle);
    setup_settings_close_intercept(app_handle);
    glass_buttons::inject_glass_buttons(app_handle);
    if let Err(err) = shortcuts::init_shortcuts(app_handle) {
        eprintln!("[shortcuts] failed to register global shortcuts: {err}");
    }
    updater::spawn_update_check(app_handle);
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
            get_relay_settings,
            save_relay_settings,
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
            open_microphone_privacy_settings
        ])
        .setup(|app| {
            let initial_models_ready = sw_audio_recording::download::models_ready(
                &sw_audio_recording::download::default_model_dir(),
            );
            let mut state = AppState::default();
            state.models_ready = initial_models_ready;
            match state::device_key::DeviceKeyManager::load_or_create() {
                Ok(dkm) => {
                    state.device_key = Some(dkm);
                }
                Err(e) => {
                    return Err(format!(
                        "Device key initialization failed: {e}. \
                         Cannot start without encryption keys."
                    )
                    .into());
                }
            }
            let file_key = state
                .device_key
                .as_ref()
                .and_then(|dkm| match dkm.file_key() {
                    Ok(key) => Some(key),
                    Err(e) => {
                        eprintln!("[device-key] WARNING: could not derive file key: {e}");
                        None
                    }
                });
            app.manage(Mutex::new(state));

            if let Some(fk) = file_key {
                match app.path().app_data_dir() {
                    Ok(dir) => {
                        let sessions_dir = dir.join("sessions");
                        match sw_notes::SessionStore::new(sessions_dir, fk) {
                            Ok(store) => {
                                app.manage(Arc::new(store));
                            }
                            Err(err) => {
                                eprintln!("[notes] failed to init session store: {err}");
                            }
                        }
                    }
                    Err(err) => {
                        eprintln!("[notes] failed to resolve app data dir: {err}");
                    }
                }
            }

            let relay_settings_store = Arc::new(
                relay_settings::RelaySettingsStore::load(app.app_handle())
                    .expect("failed to load relay settings store"),
            );
            app.manage(relay_settings_store);

            let app_settings_store = Arc::new(
                app_settings::AppSettingsStore::load(app.app_handle())
                    .expect("failed to load app settings store"),
            );
            app.manage(app_settings_store);

            let relay_client = Arc::new(tokio::sync::RwLock::new(
                relay::RelayClient::new().expect("failed to build relay client"),
            ));
            app.manage(relay_client.clone());

            app.manage(Arc::new(chat_reply_listener::PendingReplies::default()));
            app.manage(Arc::new(chat_reply_listener::ProbeRegistry::default()));

            let app_handle_for_listener = app.app_handle().clone();
            let relay_settings_for_listener = app
                .state::<Arc<relay_settings::RelaySettingsStore>>()
                .inner()
                .clone();
            let relay_client_for_listener = relay_client.clone();
            tauri::async_runtime::spawn(async move {
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
                match chat_reply_listener::ChatReplyListener::start(
                    app_handle_for_listener.clone(),
                    token.clone(),
                )
                .await
                {
                    Ok(listener) => {
                        let callback_url = listener.callback_url();
                        relay_client_for_listener
                            .read()
                            .await
                            .set_callback(callback_url, token);
                        app_handle_for_listener.manage(Arc::new(tokio::sync::Mutex::new(listener)));
                    }
                    Err(err) => {
                        eprintln!("[chat-reply-listener] failed to start: {err}");
                    }
                }
            });

            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            finish_setup(app.app_handle());

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
