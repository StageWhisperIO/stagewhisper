mod app_settings;
mod audio;
mod audio_local;
#[cfg(feature = "diarization")]
mod diarization;
mod glass_buttons;
mod notes;
mod pairing;
mod panels;
mod relay;
mod relay_probe;
mod relay_settings;
mod reply_router;
mod reply_stream;
mod shortcuts;
mod state;
#[cfg(target_os = "windows")]
mod tray;
mod updater;

use std::sync::{Arc, Mutex};

use panels::{
    close_settings_window, get_screen_share_privacy, init_panels, open_settings_window, quit_app,
    set_onboarding_visible, setup_settings_close_intercept, toggle_screen_share_visibility,
    webview_mounted,
};
use relay_probe::{confirm_device_approved, probe_agent_pairing};
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
use state::session_chat::{
    cancel_session_chat_turn, resume_session_chat_turn, stream_session_chat_message, TurnRegistry,
};
use tauri::{Emitter, Listener, Manager};
use tauri_plugin_opener::OpenerExt;

const RELAY_SETTINGS_CHANGED_EVENT: &str = "relay-settings-changed";

pub(crate) async fn notify_relay_changed(
    app: &tauri::AppHandle,
    settings: &relay_settings::RelaySettings,
) {
    let _ = app.emit(RELAY_SETTINGS_CHANGED_EVENT, settings.ready());
    glass_buttons::sync_button_appearance(app);
    app.state::<Arc<reply_stream::ReplyStreamManager>>()
        .inner()
        .clone()
        .cancel_all()
        .await;
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

#[derive(serde::Serialize)]
struct Capabilities {
    diarization: bool,
}

#[tauri::command]
fn get_capabilities() -> Capabilities {
    Capabilities {
        diarization: cfg!(feature = "diarization"),
    }
}

#[cfg(feature = "diarization")]
#[tauri::command]
fn rename_speaker(
    app: tauri::AppHandle,
    speaker_id: String,
    label: Option<String>,
) -> Result<(), String> {
    diarization::rename_speaker(&app, &speaker_id, label)
}

#[cfg(not(feature = "diarization"))]
#[tauri::command]
fn rename_speaker(
    _app: tauri::AppHandle,
    _speaker_id: String,
    _label: Option<String>,
) -> Result<(), String> {
    Err("not available in this build".to_string())
}

#[cfg(feature = "diarization")]
#[tauri::command]
fn list_session_speakers(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<Vec<diarization::SessionSpeaker>, String> {
    diarization::list_session_speakers(&app, &session_id)
}

#[cfg(not(feature = "diarization"))]
#[tauri::command]
fn list_session_speakers(_app: tauri::AppHandle, _session_id: String) -> Result<Vec<()>, String> {
    Err("not available in this build".to_string())
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
    if let Some(url) = args.relay_url.as_deref() {
        if !url.trim().is_empty() {
            pairing::validate_relay_url(url)?;
        }
    }
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
    notify_relay_changed(&app, &updated).await;
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
    let updated = store
        .update(|s| {
            s.relay_url = relay.url;
            s.relay_token = relay.token;
            s.paired_verified = false;
        })
        .map_err(|e| e.to_string())?;
    notify_relay_changed(&app, &updated).await;
    Ok(updated)
}

pub(crate) fn ensure_session_storage(app: &tauri::AppHandle) -> Result<(), String> {
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
fn update_session_title(
    app: tauri::AppHandle,
    session_id: String,
    title: Option<String>,
) -> Result<(), String> {
    session_store(&app)?
        .update_title(&session_id, title)
        .map_err(|e| e.to_string())
}

fn finish_setup(app_handle: &tauri::AppHandle) {
    init_panels(app_handle);
    setup_settings_close_intercept(app_handle);
    glass_buttons::inject_glass_buttons(app_handle);
    refresh_control_bar_on_local_llm_changes(app_handle);
    if let Err(err) = shortcuts::init_shortcuts(app_handle) {
        eprintln!("[shortcuts] failed to register global shortcuts: {err}");
    }
    #[cfg(target_os = "windows")]
    if let Err(err) = tray::init_tray(app_handle) {
        eprintln!("[tray] failed to initialize: {err}");
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
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {
            #[cfg(target_os = "windows")]
            tray::focus_control_panel(_app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build());

    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());

    let app = builder
        .invoke_handler(tauri::generate_handler![
            toggle_screen_share_visibility,
            get_screen_share_privacy,
            open_settings_window,
            close_settings_window,
            webview_mounted,
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
            get_app_settings,
            save_app_settings,
            get_capabilities,
            rename_speaker,
            list_session_speakers,
            pair_with_code,
            probe_agent_pairing,
            confirm_device_approved,
            list_sessions,
            get_session,
            delete_session,
            update_session_title,
            stream_session_chat_message,
            cancel_session_chat_turn,
            resume_session_chat_turn,
            get_permissions_status,
            request_microphone_permission,
            request_screen_recording_permission,
            open_microphone_privacy_settings,
            open_screen_recording_privacy_settings
        ])
        .setup(|app| {
            let initial_models_ready = state::local_pipeline::all_models_ready(
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
            app.manage(state::local_llm::LocalTurnQueue::new());
            app.manage(LocalLlmDownloads::default());
            app.manage(Arc::new(TurnRegistry::default()));

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

            app.manage(Arc::new(tokio::sync::RwLock::new(
                relay::RelayClient::new().expect("failed to build relay client"),
            )));
            app.manage(Arc::new(reply_stream::ReplyStreamManager::default()));

            let pending_replies = match app.path().app_data_dir() {
                Ok(dir) => reply_router::PendingReplies::load(dir.join("pending_tasks.json")),
                Err(err) => {
                    eprintln!(
                        "[reply-router] failed to resolve app data dir for pending tasks: {err}"
                    );
                    reply_router::PendingReplies::default()
                }
            };
            app.manage(Arc::new(pending_replies));
            app.manage(Arc::new(reply_router::ProbeRegistry::default()));

            #[cfg(target_os = "macos")]
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

#[cfg(target_os = "windows")]
#[tauri::command]
fn open_microphone_privacy_settings(app: tauri::AppHandle) -> Result<(), String> {
    app.opener()
        .open_url("ms-settings:privacy-microphone", None::<&str>)
        .map_err(|e| format!("failed to open system settings: {e}"))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
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

#[cfg(target_os = "windows")]
#[tauri::command]
fn open_screen_recording_privacy_settings() -> Result<(), String> {
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[tauri::command]
fn open_screen_recording_privacy_settings() -> Result<(), String> {
    Err("Screen recording privacy settings are only available on macOS".to_string())
}
