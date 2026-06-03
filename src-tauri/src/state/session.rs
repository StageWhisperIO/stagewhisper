use std::sync::{Arc, Mutex};

use crate::audio::{start_system_audio_capture, AudioStopReason};
use crate::panels::sync_session_panel_visibility;
use crate::relay_settings::RelaySettingsStore;
use crate::state::app_state::AppState;
use tauri::Emitter;
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionState {
    Stopped,
    Listening,
}

pub const SESSION_STATE_EVENT: &str = "session-state-changed";

fn session_state_to_str(state: SessionState) -> &'static str {
    match state {
        SessionState::Stopped => "stopped",
        SessionState::Listening => "listening",
    }
}

fn emit_session_events(app: &tauri::AppHandle, state: SessionState) {
    let _ = app.emit(SESSION_STATE_EVENT, session_state_to_str(state));
}

fn sync_panel_on_main_thread(app: &tauri::AppHandle) {
    let app_clone = app.clone();
    let _ = app.run_on_main_thread(move || {
        sync_session_panel_visibility(&app_clone);
    });
}

fn emit_session_start_error(app: &tauri::AppHandle, message: impl Into<String>) {
    let message = message.into();
    let _ = app.emit("session-start-error", message.clone());
    let _ = app
        .notification()
        .builder()
        .title("Couldn't start the session")
        .body(&message)
        .show();
}

pub fn force_stop_session(app: &tauri::AppHandle, reason: AudioStopReason) {
    let stop_tx = {
        let data = app.state::<Mutex<AppState>>();
        let mut state = data.lock().unwrap();
        state.session_state = SessionState::Stopped;
        state.session_id = None;
        state.audio_stop_tx.take()
    };

    if let Some(tx) = stop_tx {
        let _ = tx.send(reason);
    }

    let _ = app.emit("current-session-changed", Option::<String>::None);
    emit_session_events(app, SessionState::Stopped);
}

#[tauri::command]
pub fn complete_session(app: tauri::AppHandle) {
    let (current_state, has_audio_thread) = {
        let data = app.state::<Mutex<AppState>>();
        let state = data.lock().unwrap();
        (state.session_state, state.audio_stop_tx.is_some())
    };

    if !matches!(current_state, SessionState::Listening) {
        return;
    }

    if has_audio_thread {
        force_stop_session(&app, AudioStopReason::Complete);
    }

    sync_panel_on_main_thread(&app);
}

#[tauri::command]
pub fn get_session_state(app: tauri::AppHandle) -> String {
    let data = app.state::<Mutex<AppState>>();
    let state = data.lock().unwrap();
    session_state_to_str(state.session_state).to_string()
}

#[tauri::command]
pub fn get_current_session_id(app: tauri::AppHandle) -> Option<String> {
    let data = app.state::<Mutex<AppState>>();
    let state = data.lock().unwrap();
    state.session_id.clone()
}

#[tauri::command]
pub fn toggle_session_state(app: tauri::AppHandle) -> bool {
    let current_state = {
        let data = app.state::<Mutex<AppState>>();
        let state = data.lock().unwrap();
        state.session_state
    };

    if matches!(current_state, SessionState::Stopped) {
        let relay_settings = app.state::<Arc<RelaySettingsStore>>().snapshot();
        if !relay_settings.has_relay() {
            emit_session_start_error(
                &app,
                "Connect your AI assistant in Settings before starting a session.",
            );
            sync_panel_on_main_thread(&app);
            return false;
        }
        if relay_settings.pairing_pending() {
            emit_session_start_error(
                &app,
                "Your assistant hasn't approved this device yet. Finish pairing in Connection settings before starting a session.",
            );
            let _ = crate::panels::open_settings_window(app.clone());
            sync_panel_on_main_thread(&app);
            return false;
        }

        let model_dir = sw_audio_recording::download::default_model_dir();
        if !sw_audio_recording::download::models_ready(&model_dir) {
            emit_session_start_error(
                &app,
                "Transcription models are required. Download them in Settings → Model before starting.",
            );
            sync_panel_on_main_thread(&app);
            return false;
        }
    }

    match current_state {
        SessionState::Stopped => match start_system_audio_capture(app.clone()) {
            Ok(stop_tx) => {
                {
                    let data = app.state::<Mutex<AppState>>();
                    let mut state = data.lock().unwrap();
                    state.audio_stop_tx = Some(stop_tx);
                    state.session_state = SessionState::Listening;
                }
                emit_session_events(&app, SessionState::Listening);
                sync_panel_on_main_thread(&app);
                true
            }
            Err(err) => {
                eprintln!("failed to start listening: {err}");
                emit_session_start_error(&app, err);
                {
                    let data = app.state::<Mutex<AppState>>();
                    let mut state = data.lock().unwrap();
                    state.audio_stop_tx = None;
                    state.session_state = SessionState::Stopped;
                }
                emit_session_events(&app, SessionState::Stopped);
                sync_panel_on_main_thread(&app);
                false
            }
        },
        SessionState::Listening => {
            force_stop_session(&app, AudioStopReason::Complete);
            sync_panel_on_main_thread(&app);
            false
        }
    }
}
