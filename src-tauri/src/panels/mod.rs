#![allow(clippy::unused_unit)]

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

mod sizes;
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    allow(unused_imports)
)]
use sizes::{apply_saved_size, remember_size_on_resize, PanelConstraints};

use crate::state::app_state::AppState;
use crate::state::session::SessionState;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

const CONTROL_LABEL: &str = "main";
pub const SESSION_LABEL: &str = "session";
const SETTINGS_LABEL: &str = "settings";
const SESSION_WIDTH: f64 = 540.0;
const SESSION_HEIGHT: f64 = 260.0;
const PANEL_GAP: f64 = 16.0;

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
const CONTROL_CONSTRAINTS: PanelConstraints = PanelConstraints {
    min_width: 240.0,
    min_height: 44.0,
    max_height: Some(44.0),
};
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
const SESSION_CONSTRAINTS: PanelConstraints = PanelConstraints {
    min_width: 480.0,
    min_height: 200.0,
    max_height: None,
};

/// Returns whether the session panel should currently be visible
/// (session state is Listening or Paused).
pub fn is_session_active(app_handle: &AppHandle) -> bool {
    let data = app_handle.state::<Mutex<AppState>>();
    let state = data.lock().unwrap();
    matches!(state.session_state, SessionState::Listening)
}

static MOUNTED_WEBVIEWS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn mounted_webviews() -> &'static Mutex<HashSet<String>> {
    MOUNTED_WEBVIEWS.get_or_init(|| Mutex::new(HashSet::new()))
}

#[tauri::command]
pub fn webview_mounted(window: tauri::WebviewWindow) {
    let mut mounted = mounted_webviews().lock().unwrap();
    mounted.insert(window.label().to_string());
}

fn webview_has_mounted(label: &str) -> bool {
    mounted_webviews().lock().unwrap().contains(label)
}

static PENDING_UNMOUNTED_REVIVES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

const UNMOUNTED_REVIVE_GRACE: Duration = Duration::from_secs(5);
const HOST_RELOAD_DEBOUNCE: Duration = Duration::from_secs(10);

static LAST_HOST_RELOADS: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

fn host_reload_permitted(label: &str) -> bool {
    let mut last = LAST_HOST_RELOADS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
    if let Some(at) = last.get(label) {
        if at.elapsed() < HOST_RELOAD_DEBOUNCE {
            return false;
        }
    }
    last.insert(label.to_string(), Instant::now());
    true
}

/// A window shown before its frontend ever reported a mount has no JS-side
/// recovery to lean on (its content process may have died pre-mount). Give a
/// healthy boot a generous grace period, then force a host-side reload if the
/// mount signal still never arrived.
fn schedule_unmounted_revive(app_handle: &AppHandle, label: &str) {
    {
        let mut pending = PENDING_UNMOUNTED_REVIVES
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .unwrap();
        if !pending.insert(label.to_string()) {
            return;
        }
    }
    let app = app_handle.clone();
    let label = label.to_string();
    std::thread::spawn(move || {
        std::thread::sleep(UNMOUNTED_REVIVE_GRACE);
        PENDING_UNMOUNTED_REVIVES
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .unwrap()
            .remove(&label);
        if webview_has_mounted(&label) {
            return;
        }
        if let Some(window) = app.get_webview_window(&label) {
            if !host_reload_permitted(&label) {
                return;
            }
            eprintln!(
                "[panels] webview '{label}' never reported mount within {UNMOUNTED_REVIVE_GRACE:?} of show — forcing reload"
            );
            if let Err(err) = window.reload() {
                eprintln!("[panels] pre-mount reload failed for '{label}': {err}");
            }
        }
    });
}

/// Reload a window's webview when its content process died while hidden
/// (macOS reclaims WKWebView content processes under memory pressure,
/// leaving the panel blank on the next show).
///
/// Only armed after the window's frontend has reported a successful mount
/// (`webview_mounted`), so an empty root during first boot or slow hydration
/// is never mistaken for a dead process.
///
/// Deliberately fire-and-forget: callers show the panel immediately and the
/// content fills in when the reload lands. Gating visibility on a
/// load-complete signal would need a timeout fallback to avoid re-introducing
/// an unopenable panel, which is a worse failure mode than a sub-second blank
/// glass surface.
pub fn revive_webview_if_empty(app_handle: &AppHandle, label: &str) {
    if !webview_has_mounted(label) {
        schedule_unmounted_revive(app_handle, label);
        return;
    }
    let Some(window) = app_handle.get_webview_window(label) else {
        return;
    };
    let eval_result = window.eval(
        "(function(){if(document.readyState!=='complete'){return;}var r=document.getElementById('root');if(r===null||r.childElementCount===0){location.reload();}})();",
    );
    if let Err(eval_err) = eval_result {
        if !host_reload_permitted(label) {
            return;
        }
        eprintln!("[panels] revive eval failed for '{label}': {eval_err} — forcing reload");
        if let Err(reload_err) = window.reload() {
            eprintln!("[panels] reload fallback failed for '{label}': {reload_err}");
        }
    }
}

/// Get the current privacy state.
/// Returns true if panels are hidden from screen sharing.
#[tauri::command]
pub fn get_screen_share_privacy(app: tauri::AppHandle) -> bool {
    let data = app.state::<Mutex<AppState>>();
    let state = data.lock().unwrap();
    state.is_screen_sharing_private
}

#[tauri::command]
pub fn open_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window(SETTINGS_LABEL)
        .ok_or("settings window not found — is it defined in tauri.conf.json?")?;
    revive_webview_if_empty(&app, SETTINGS_LABEL);
    let _ = window.show();
    let _ = window.set_focus();
    Ok(())
}

#[tauri::command]
pub fn set_onboarding_visible(visible: bool) {
    crate::glass_buttons::set_glass_onboarding_visible(visible);
}

#[tauri::command]
pub fn close_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window(SETTINGS_LABEL)
        .ok_or("settings window not found")?;
    let _ = window.hide();
    Ok(())
}

#[tauri::command]
pub fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

/// Intercept the close event on the settings window so that it hides instead
/// of being destroyed. This keeps the WebView alive for instant re-opening.
pub fn setup_settings_close_intercept(app_handle: &AppHandle) {
    if let Some(window) = app_handle.get_webview_window(SETTINGS_LABEL) {
        let win = window.clone();
        window.on_window_event(move |event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = win.hide();
            }
        });
    }
}
