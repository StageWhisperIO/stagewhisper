#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SystemAudioGate {
    Open,
    Denied,
    Asking,
}

#[cfg(target_os = "macos")]
pub(crate) use macos::system_audio_gate;

#[cfg(not(target_os = "macos"))]
pub(crate) fn system_audio_gate(_app: &tauri::AppHandle) -> SystemAudioGate {
    SystemAudioGate::Open
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Default)]
struct PromptSlot {
    shown_at: Option<std::time::Instant>,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl PromptSlot {
    fn claim(&mut self, now: std::time::Instant, answer_timeout: std::time::Duration) -> bool {
        let waiting = self
            .shown_at
            .is_some_and(|at| now.saturating_duration_since(at) < answer_timeout);
        if waiting {
            return false;
        }
        self.shown_at = Some(now);
        true
    }

    fn release(&mut self) {
        self.shown_at = None;
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use tauri::{AppHandle, Manager};
    use tauri_plugin_opener::OpenerExt;

    use super::{PromptSlot, SystemAudioGate};
    use crate::state::app_state::AppState;
    use crate::state::permissions::{
        ask_for_system_audio, emit_permissions_status, system_audio_status, PermissionState,
    };
    use crate::state::session::{
        refuse_start_without_system_audio, toggle_session_state, SessionState,
    };

    const SYSTEM_AUDIO_SETTINGS_URL: &str =
        "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";
    const PROMPT_ANSWER_TIMEOUT: Duration = Duration::from_secs(120);

    static PROMPT: Mutex<PromptSlot> = Mutex::new(PromptSlot { shown_at: None });
    static ALLOWED_IN_PROMPT: AtomicBool = AtomicBool::new(false);

    pub(crate) fn system_audio_gate(app: &AppHandle) -> SystemAudioGate {
        match system_audio_status() {
            PermissionState::Denied => {
                log::warn!(
                    "[permissions] system audio recording is denied; refusing to listen to silence"
                );
                if let Err(err) = app
                    .opener()
                    .open_url(SYSTEM_AUDIO_SETTINGS_URL, None::<&str>)
                {
                    log::warn!("[permissions] failed to open system settings: {err}");
                }
                SystemAudioGate::Denied
            }
            PermissionState::Undetermined if !ALLOWED_IN_PROMPT.load(Ordering::SeqCst) => {
                ask_then_continue(app)
            }
            _ => SystemAudioGate::Open,
        }
    }

    fn ask_then_continue(app: &AppHandle) -> SystemAudioGate {
        let claimed = PROMPT
            .lock()
            .map(|mut prompt| prompt.claim(Instant::now(), PROMPT_ANSWER_TIMEOUT))
            .unwrap_or(false);
        if !claimed {
            return SystemAudioGate::Asking;
        }
        let answered_app = app.clone();
        let asked = ask_for_system_audio(move |granted| {
            release_prompt();
            ALLOWED_IN_PROMPT.store(granted, Ordering::SeqCst);
            emit_permissions_status(&answered_app);
            let continued_app = answered_app.clone();
            if let Err(err) =
                answered_app.run_on_main_thread(move || continue_start(&continued_app, granted))
            {
                log::warn!("[permissions] could not continue the session start: {err}");
            }
        });
        if !asked {
            release_prompt();
            return SystemAudioGate::Open;
        }
        log::info!("[permissions] waiting for the system audio recording answer before listening");
        SystemAudioGate::Asking
    }

    fn release_prompt() {
        if let Ok(mut prompt) = PROMPT.lock() {
            prompt.release();
        }
    }

    fn continue_start(app: &AppHandle, granted: bool) {
        let session_state = app.state::<Mutex<AppState>>().lock().unwrap().session_state;
        if session_state != SessionState::Stopped {
            return;
        }
        if granted {
            toggle_session_state(app.clone());
        } else {
            refuse_start_without_system_audio(app);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::PromptSlot;

    const TIMEOUT: Duration = Duration::from_secs(120);

    #[test]
    fn a_second_listen_while_the_prompt_is_open_does_not_ask_again() {
        let now = Instant::now();
        let mut prompt = PromptSlot::default();
        assert!(prompt.claim(now, TIMEOUT));
        assert!(!prompt.claim(now + Duration::from_secs(5), TIMEOUT));
    }

    #[test]
    fn an_answer_frees_the_prompt_for_the_next_listen() {
        let now = Instant::now();
        let mut prompt = PromptSlot::default();
        assert!(prompt.claim(now, TIMEOUT));
        prompt.release();
        assert!(prompt.claim(now + Duration::from_secs(1), TIMEOUT));
    }

    #[test]
    fn a_prompt_that_never_answers_stops_blocking_listen() {
        let now = Instant::now();
        let mut prompt = PromptSlot::default();
        assert!(prompt.claim(now, TIMEOUT));
        assert!(prompt.claim(now + TIMEOUT, TIMEOUT));
    }
}
