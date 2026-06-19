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

pub fn can_listen(
    relay: &crate::relay_settings::RelaySettings,
    local_ready: bool,
    prefers_local: bool,
    stt_ready: bool,
) -> bool {
    if !stt_ready {
        return false;
    }
    let local_intended = prefers_local || !relay.has_relay();
    if local_intended {
        return local_ready;
    }
    if relay.pairing_pending() {
        return false;
    }
    true
}

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
        let local_llm_ready = crate::state::local_llm::local_ready(&app);
        let use_local_reasoning = local_llm_ready
            && (crate::state::local_llm::prefers_local(&app) || !relay_settings.has_relay());
        if !relay_settings.has_relay() && !local_llm_ready {
            emit_session_start_error(
                &app,
                "Connect your AI assistant or set up a local model in Settings before starting a session.",
            );
            sync_panel_on_main_thread(&app);
            return false;
        }
        if relay_settings.pairing_pending() && !use_local_reasoning {
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

#[cfg(test)]
mod tests {
    use super::can_listen;
    use crate::relay_settings::RelaySettings;

    fn relay(url: &str, token: &str, verified: bool) -> RelaySettings {
        RelaySettings {
            relay_url: url.to_string(),
            relay_token: token.to_string(),
            paired_verified: verified,
            ..RelaySettings::default()
        }
    }

    #[test]
    fn blocks_without_speech_to_text() {
        assert!(!can_listen(&relay("http://x", "t", true), true, true, false));
    }

    #[test]
    fn blocks_with_neither_relay_nor_local() {
        assert!(!can_listen(&RelaySettings::default(), false, false, true));
    }

    #[test]
    fn local_only_is_ready() {
        assert!(can_listen(&RelaySettings::default(), true, true, true));
    }

    #[test]
    fn verified_relay_is_ready() {
        assert!(can_listen(&relay("http://x", "t", true), false, false, true));
    }

    #[test]
    fn pending_relay_without_local_is_not_ready() {
        assert!(!can_listen(&relay("http://x", "t", false), false, false, true));
    }

    #[test]
    fn pending_relay_with_local_default_falls_back_to_local() {
        assert!(can_listen(&relay("http://x", "t", false), true, true, true));
    }

    #[test]
    fn pending_relay_with_local_preferring_external_stays_blocked() {
        assert!(!can_listen(&relay("http://x", "t", false), true, false, true));
    }
}
