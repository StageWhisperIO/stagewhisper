#![allow(clippy::unused_unit)]

use crate::state::app_state::AppState;
use crate::state::session::SessionState;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::Emitter;
use tauri::{AppHandle, Manager, Runtime, Size, WebviewUrl};
use tauri_nspanel::{
    tauri_panel, CollectionBehavior, ManagerExt, PanelBuilder, PanelLevel, StyleMask,
    TrackingAreaOptions, WebviewWindowExt,
};
#[cfg(target_os = "macos")]
use window_vibrancy::{
    apply_liquid_glass, apply_vibrancy, NSGlassEffectViewStyle, NSVisualEffectMaterial,
    NSVisualEffectState,
};

tauri_panel! {
    panel!(HoverActivatePanel {
        config: {
            can_become_key_window: true,
            can_become_main_window: false,
            is_floating_panel: true,
            has_key_appearance: true,
            becomes_key_only_if_needed: true,
        }
        with: {
            tracking_area: {
                options: TrackingAreaOptions::new()
                    .active_always()
                    .mouse_entered_and_exited()
                    .mouse_moved()
                    .cursor_update(),
                auto_resize: true
            }
        }
    })

    panel_event!(HoverPanelEventHandler {})
}

const CONTROL_LABEL: &str = "main";
pub const SESSION_LABEL: &str = "session";
const SETTINGS_LABEL: &str = "settings";
const SESSION_WIDTH: f64 = 540.0;
const SESSION_HEIGHT: f64 = 260.0;
const PANEL_GAP: f64 = 16.0;

#[cfg(target_os = "macos")]
const DARK_TINT: (u8, u8, u8, u8) = (0, 0, 0, 204);

// ── Public init ─────────────────────────────────────────────────────────────

pub fn init_panels(app_handle: &AppHandle) {
    // ── 1. Control panel (from "main" window defined in tauri.conf.json) ────
    let control_window = app_handle
        .get_webview_window(CONTROL_LABEL)
        .expect("main window must exist in tauri.conf.json");

    #[cfg(target_os = "macos")]
    {
        force_dark_appearance_on_window(&control_window);
        apply_glass_or_vibrancy(&control_window, 22.0);
    }

    let control_panel = control_window
        .to_panel::<HoverActivatePanel>()
        .expect("failed to convert main window to panel");

    configure_panel_common(&*control_panel);
    control_panel.set_movable_by_window_background(true);

    let handler = HoverPanelEventHandler::new();
    let handle = app_handle.clone();

    handler.on_mouse_entered(move |_event| {
        if let Ok(panel) = handle.get_webview_panel(CONTROL_LABEL) {
            panel.make_key_window();
        }
    });

    let handle = app_handle.clone();
    handler.on_mouse_exited(move |_event| {
        if let Ok(panel) = handle.get_webview_panel(CONTROL_LABEL) {
            panel.resign_key_window();
        }
    });

    control_panel.set_event_handler(Some(handler.as_ref()));

    // ── 2. Session panel (created via PanelBuilder) ─────────────────────────
    //
    // Use PanelBuilder from tauri-nspanel for proper panel creation.
    // The session panel starts hidden — it is only shown when session state
    // becomes Listening or Paused. We do NOT attach it as a child window here;
    // that happens each time the session panel is shown (see `show_session_panel`),
    // because macOS removes the child relationship when a child is hidden via
    // `orderOut:`.
    let session_panel = PanelBuilder::<_, HoverActivatePanel>::new(app_handle, SESSION_LABEL)
        .url(WebviewUrl::App("session.html".into()))
        .title("Session")
        .size(Size::Logical(tauri::LogicalSize::new(
            SESSION_WIDTH,
            SESSION_HEIGHT,
        )))
        .level(PanelLevel::Floating)
        .style_mask(StyleMask::empty().borderless().nonactivating_panel())
        .collection_behavior(
            CollectionBehavior::new()
                .full_screen_auxiliary()
                .can_join_all_spaces()
                .stationary()
                .ignores_cycle(),
        )
        .hides_on_deactivate(false)
        .works_when_modal(true)
        .movable_by_window_background(false) // NOT draggable
        .no_activate(true) // Don't steal focus during creation
        .corner_radius(12.0)
        .with_window(|w| w.decorations(false).transparent(true).visible(false))
        .build()
        .expect("failed to create session panel");

    let session_handler = HoverPanelEventHandler::new();
    let handle = app_handle.clone();
    session_handler.on_mouse_entered(move |_event| {
        if let Ok(panel) = handle.get_webview_panel(SESSION_LABEL) {
            panel.make_key_window();
        }
    });

    let handle = app_handle.clone();
    session_handler.on_mouse_exited(move |_event| {
        if let Ok(panel) = handle.get_webview_panel(SESSION_LABEL) {
            panel.resign_key_window();
        }
    });

    session_panel.set_event_handler(Some(session_handler.as_ref()));

    // Apply glass effect via the Tauri WebviewWindow (NOT session_panel.to_window()
    // which is destructive — it removes the panel from the nspanel store).
    if let Some(session_window) = app_handle.get_webview_window(SESSION_LABEL) {
        #[cfg(target_os = "macos")]
        {
            force_dark_appearance_on_window(&session_window);
            apply_glass_or_vibrancy(&session_window, 12.0);
        }
    }

    // Make session panel invisible to screen sharing (inherit privacy state)
    unsafe {
        use tauri_nspanel::objc2::msg_send;
        let _: () = msg_send![session_panel.as_panel(), setSharingType: 0u64];
    }

    // ── 3. Show control, then sync secondary panels based on state ──────────
    control_panel.show();
    sync_session_panel_visibility(app_handle);
}

// ── Session panel visibility sync ───────────────────────────────────────────

/// Show the session panel: position it below the control panel, re-attach it
/// as a child window, then make it visible.
///
/// macOS removes the child window relationship when a window is hidden
/// (`orderOut:`), so we must re-establish it every time we show the panel.
fn show_session_panel(app_handle: &AppHandle) {
    revive_webview_if_empty(app_handle, SESSION_LABEL);
    let control = app_handle.get_webview_panel(CONTROL_LABEL);
    let session = app_handle.get_webview_panel(SESSION_LABEL);
    if control.is_err() || session.is_err() {
        return;
    }
    let control = control.unwrap();
    let session = session.unwrap();

    position_panel_below_control(&*control, &*session, SESSION_WIDTH, SESSION_HEIGHT);

    // Re-attach as child window so it follows the control panel
    add_child_window(&*control, &*session);

    session.show();
}

/// Hide the session panel. macOS will automatically detach the child window
/// relationship when `orderOut:` is called.
fn hide_session_panel(app_handle: &AppHandle) {
    if let Ok(panel) = app_handle.get_webview_panel(SESSION_LABEL) {
        panel.hide();
    }
}

pub fn sync_session_panel_visibility(app_handle: &AppHandle) {
    crate::glass_buttons::sync_button_appearance(app_handle);

    if is_session_active(app_handle) {
        show_session_panel(app_handle);
    } else {
        hide_session_panel(app_handle);
    }
}

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

pub fn show_panels(app_handle: &AppHandle) {
    revive_webview_if_empty(app_handle, CONTROL_LABEL);
    if let Ok(panel) = app_handle.get_webview_panel(CONTROL_LABEL) {
        panel.show();
    }
    sync_session_panel_visibility(app_handle);
}

pub fn hide_panels(app_handle: &AppHandle) {
    hide_session_panel(app_handle);
    if let Ok(panel) = app_handle.get_webview_panel(CONTROL_LABEL) {
        panel.hide();
    }
}

pub fn is_control_panel_visible(app_handle: &AppHandle) -> bool {
    app_handle
        .get_webview_panel(CONTROL_LABEL)
        .map(|p| p.is_visible())
        .unwrap_or(false)
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Used for the control panel which is converted from a tauri.conf.json window.
fn configure_panel_common(panel: &dyn tauri_nspanel::Panel) {
    panel.set_level(PanelLevel::Floating.value());

    panel.set_style_mask(StyleMask::new().borderless().nonactivating_panel().into());

    // Invisible to screen sharing (NSWindowSharingNone = 0)
    unsafe {
        use tauri_nspanel::objc2::msg_send;
        let _: () = msg_send![panel.as_panel(), setSharingType: 0u64];
    }

    panel.set_collection_behavior(
        CollectionBehavior::new()
            .full_screen_auxiliary()
            .can_join_all_spaces()
            .stationary()
            .ignores_cycle()
            .into(),
    );

    // Don't hide when app loses focus
    panel.set_hides_on_deactivate(false);

    panel.set_works_when_modal(true);
}

/// Apply the best available translucent backdrop to a window.
///
/// On macOS 26+ (Tahoe): uses Liquid Glass (`NSGlassEffectView`) with a dark
/// tint color for the new native glass aesthetic.
///
/// On older macOS: falls back to `NSVisualEffectView` vibrancy with `Popover`
/// material. Combined with `force_dark_appearance_on_window` this still gives
/// a consistently dark translucent look.
#[cfg(target_os = "macos")]
fn apply_glass_or_vibrancy(window: &tauri::WebviewWindow, radius: f64) {
    // Try liquid glass first (macOS 26+).
    if apply_liquid_glass(
        window,
        NSGlassEffectViewStyle::Clear,
        Some(DARK_TINT),
        Some(radius),
    )
    .is_ok()
    {
        return;
    }

    // Fallback: classic vibrancy for macOS < 26.
    apply_vibrancy(
        window,
        NSVisualEffectMaterial::Popover,
        NSVisualEffectState::Active.into(),
        Some(radius),
    )
    .expect("Unsupported platform! 'apply_vibrancy' is only supported on macOS");
}

/// Force the window to always use dark appearance, regardless of system theme.
/// This ensures the vibrancy / liquid glass effect always renders as a dark
/// translucent backdrop so we can safely use white/light text at all times.
#[cfg(target_os = "macos")]
fn force_dark_appearance_on_window(window: &tauri::WebviewWindow) {
    unsafe {
        use tauri_nspanel::objc2::msg_send;
        use tauri_nspanel::objc2::runtime::{AnyClass, AnyObject};
        use tauri_nspanel::objc2_foundation::NSString;

        let ns_window: *mut AnyObject =
            msg_send![window.ns_window().unwrap() as *mut AnyObject, self];
        let name = NSString::from_str("NSAppearanceNameVibrantDark");
        let cls = AnyClass::get(c"NSAppearance").unwrap();
        let appearance: *mut AnyObject = msg_send![cls, appearanceNamed: &*name];
        let _: () = msg_send![ns_window, setAppearance: appearance];
    }
}

/// Position a child panel directly below the control panel with a 16px gap.
/// Uses macOS coordinate system (origin at bottom-left of screen).
fn position_panel_below_control(
    control: &dyn tauri_nspanel::Panel,
    panel: &dyn tauri_nspanel::Panel,
    panel_width: f64,
    panel_height: f64,
) {
    unsafe {
        use tauri_nspanel::objc2::msg_send;
        use tauri_nspanel::objc2_foundation::{NSPoint, NSRect};

        let frame: NSRect = msg_send![control.as_panel(), frame];

        // In macOS coords (bottom-left origin), "below" means smaller Y
        let panel_origin = NSPoint::new(
            frame.origin.x - (panel_width - frame.size.width) / 2.0,
            frame.origin.y - panel_height - PANEL_GAP,
        );

        let _: () = msg_send![panel.as_panel(), setFrameOrigin: panel_origin];
    }
}

/// Make the session panel a child of the control panel so it follows movement
/// automatically (macOS child window behavior).
///
/// This must be called every time the session panel is shown, because macOS
/// removes the child relationship when a window is ordered out (hidden).
fn add_child_window(parent: &dyn tauri_nspanel::Panel, child: &dyn tauri_nspanel::Panel) {
    unsafe {
        use tauri_nspanel::objc2::msg_send;
        // NSWindowOrderingMode.above = 1
        let _: () = msg_send![
            parent.as_panel(),
            addChildWindow: child.as_panel(),
            ordered: 1_isize
        ];
    }
}

// ── Commands ────────────────────────────────────────────────────────────────

/// Toggle whether both panels are visible to screen sharing.
/// Returns the new state: true = private (hidden from screen share), false = visible.
#[tauri::command]
pub fn toggle_screen_share_visibility<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<bool, String> {
    // Toggle the state
    let data = app.state::<Mutex<AppState>>();
    let new_private = {
        let mut state = data.lock().unwrap();
        state.is_screen_sharing_private = !state.is_screen_sharing_private;
        state.is_screen_sharing_private
    };

    // NSWindowSharingNone = 0 (invisible to screen share)
    // NSWindowSharingReadOnly = 1 (visible to screen share)
    let sharing_type: u64 = if new_private { 0 } else { 1 };

    // Apply to both panels
    for label in &[CONTROL_LABEL, SESSION_LABEL] {
        if let Ok(panel) = app.get_webview_panel(label) {
            unsafe {
                use tauri_nspanel::objc2::msg_send;
                let _: () = msg_send![panel.as_panel(), setSharingType: sharing_type];
            }
        }
    }

    // Emit event to update UI in both windows
    let _ = app.emit("privacy-state-changed", new_private);

    Ok(new_private)
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
