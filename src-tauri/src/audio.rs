use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};

use crate::state::app_state::{AppState, PipelineMode};
use crate::state::session::{SessionState, SESSION_STATE_EVENT};

pub enum AudioFrame {
    System(Vec<i16>, Instant),
    Mic(Vec<i16>, Instant),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioStopReason {
    Complete,
}

pub type AudioStopSignal = Sender<AudioStopReason>;

pub(crate) const SYSTEM_AUDIO_STALL_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const STALL_RESTART_MAX_ATTEMPTS: u32 = 3;
pub(crate) const CAPTURE_PANIC_MAX_RETRIES: u32 = 2;
pub(crate) const CAPTURE_PANIC_RETRY_DELAY: Duration = Duration::from_secs(2);

pub(crate) fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

pub fn start_system_audio_capture(app: AppHandle) -> Result<AudioStopSignal, String> {
    crate::audio_local::start_local_capture(app)
}

pub(crate) fn reset_session_state_keep_panel(app: &AppHandle) {
    let data = app.state::<std::sync::Mutex<AppState>>();
    if let Ok(mut state) = data.lock() {
        state.session_state = SessionState::Stopped;
        state.audio_stop_tx = None;
        state.session_id = None;
        state.pipeline_mode = PipelineMode::Local;
    }
    let _ = app.emit(SESSION_STATE_EVENT, "stopped");
    let _ = app.emit("current-session-changed", Option::<String>::None);
    let _ = app.emit("pipeline-loading", false);

    let app_buttons = app.clone();
    let _ = app.run_on_main_thread(move || {
        crate::glass_buttons::sync_button_appearance(&app_buttons);
    });
}
