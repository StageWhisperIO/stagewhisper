use super::*;
use ::windows::Win32::Foundation::HWND;
use ::windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
    DWM_SYSTEMBACKDROP_TYPE,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    GWL_EXSTYLE, HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SWP_SHOWWINDOW, SW_HIDE, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW,
};
use tauri::{Emitter, Runtime, WebviewUrl};

fn window_hwnd<R: Runtime>(window: &tauri::WebviewWindow<R>) -> Option<HWND> {
    window
        .hwnd()
        .ok()
        .map(|handle| HWND(handle.0 as *mut core::ffi::c_void))
}

fn apply_overlay_styles<R: Runtime>(window: &tauri::WebviewWindow<R>, no_activate: bool) {
    let Some(hwnd) = window_hwnd(window) else {
        return;
    };
    let mut added = WS_EX_TOOLWINDOW.0 as isize;
    if no_activate {
        added |= WS_EX_NOACTIVATE.0 as isize;
    }
    unsafe {
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, current | added);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

fn apply_capture_affinity<R: Runtime>(window: &tauri::WebviewWindow<R>, private: bool) {
    let Some(hwnd) = window_hwnd(window) else {
        return;
    };
    let affinity = if private {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    unsafe {
        if let Err(err) = SetWindowDisplayAffinity(hwnd, affinity) {
            eprintln!("[panels] failed to set display affinity: {err}");
        }
    }
}

fn apply_acrylic_backdrop<R: Runtime>(window: &tauri::WebviewWindow<R>) {
    let Some(hwnd) = window_hwnd(window) else {
        return;
    };
    let backdrop = DWMSBT_TRANSIENTWINDOW;
    let applied = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const DWM_SYSTEMBACKDROP_TYPE as *const core::ffi::c_void,
            std::mem::size_of::<DWM_SYSTEMBACKDROP_TYPE>() as u32,
        )
    }
    .is_ok();

    if !applied {
        eprintln!(
            "[panels] DWM system backdrop unavailable (Windows 10); panels paint their own surface"
        );
    }
}

fn hide_window_without_activation<R: Runtime>(window: &tauri::WebviewWindow<R>) {
    match window_hwnd(window) {
        Some(hwnd) => unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        },
        None => {
            let _ = window.hide();
        }
    }
}

fn force_transparent_webview<R: Runtime>(window: &tauri::WebviewWindow<R>) {
    if let Err(err) = window.set_background_color(Some(tauri::window::Color(0, 0, 0, 0))) {
        eprintln!("[panels] failed to force transparent webview background: {err}");
    }
}

fn force_dark_theme<R: Runtime>(window: &tauri::WebviewWindow<R>) {
    let _ = window.set_theme(Some(tauri::Theme::Dark));
}

fn show_window_without_activation(window: &tauri::WebviewWindow) {
    match window_hwnd(window) {
        Some(hwnd) => unsafe {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        },
        None => {
            let _ = window.show();
        }
    }
}

fn position_session_below_control<R: Runtime>(
    control: &tauri::WebviewWindow<R>,
    session: &tauri::WebviewWindow<R>,
) {
    let (Ok(control_pos), Ok(control_size), Ok(session_size)) = (
        control.outer_position(),
        control.outer_size(),
        session.outer_size(),
    ) else {
        return;
    };
    let scale = control.scale_factor().unwrap_or(1.0);
    let gap = (PANEL_GAP * scale).round() as i32;
    let x = control_pos.x + (control_size.width as i32 - session_size.width as i32) / 2;
    let y = control_pos.y + control_size.height as i32 + gap;
    let _ = session.set_position(tauri::PhysicalPosition::new(x, y));
}

fn reposition_visible_session(app_handle: &AppHandle) {
    let Some(session) = app_handle.get_webview_window(SESSION_LABEL) else {
        return;
    };
    if !session.is_visible().unwrap_or(false) {
        return;
    }
    let Some(control) = app_handle.get_webview_window(CONTROL_LABEL) else {
        return;
    };
    position_session_below_control(&control, &session);
}

pub fn init_panels(app_handle: &AppHandle) {
    let control_window = app_handle
        .get_webview_window(CONTROL_LABEL)
        .expect("main window must exist in tauri.conf.json");

    let _ = control_window.set_skip_taskbar(true);
    let _ = control_window.set_shadow(false);
    force_dark_theme(&control_window);
    apply_acrylic_backdrop(&control_window);
    apply_overlay_styles(&control_window, true);
    apply_capture_affinity(&control_window, true);
    apply_saved_size(app_handle, CONTROL_LABEL, &CONTROL_CONSTRAINTS);
    remember_size_on_resize(app_handle, CONTROL_LABEL);

    let session_window = tauri::WebviewWindowBuilder::new(
        app_handle,
        SESSION_LABEL,
        WebviewUrl::App("session.html".into()),
    )
    .title("Session")
    .inner_size(SESSION_WIDTH, SESSION_HEIGHT)
    .min_inner_size(
        SESSION_CONSTRAINTS.min_width,
        SESSION_CONSTRAINTS.min_height,
    )
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(true)
    .focused(false)
    .visible(false)
    .build()
    .expect("failed to create session window");

    apply_saved_size(app_handle, SESSION_LABEL, &SESSION_CONSTRAINTS);
    remember_size_on_resize(app_handle, SESSION_LABEL);

    let realigner = app_handle.clone();
    session_window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Resized(_)) {
            reposition_visible_session(&realigner);
        }
    });

    let _ = session_window.set_shadow(false);
    force_transparent_webview(&session_window);
    force_dark_theme(&session_window);
    apply_acrylic_backdrop(&session_window);
    apply_overlay_styles(&session_window, false);
    apply_capture_affinity(&session_window, true);

    let follower = app_handle.clone();
    control_window.on_window_event(move |event| {
        if matches!(
            event,
            tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_)
        ) {
            reposition_visible_session(&follower);
        }
    });

    let _ = control_window.show();

    sync_session_panel_visibility(app_handle);
}

fn show_session_panel(app_handle: &AppHandle) {
    revive_webview_if_empty(app_handle, SESSION_LABEL);
    let Some(control) = app_handle.get_webview_window(CONTROL_LABEL) else {
        return;
    };
    let Some(session) = app_handle.get_webview_window(SESSION_LABEL) else {
        return;
    };
    position_session_below_control(&control, &session);
    show_window_without_activation(&session);
}

fn hide_session_panel(app_handle: &AppHandle) {
    if let Some(session) = app_handle.get_webview_window(SESSION_LABEL) {
        hide_window_without_activation(&session);
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

pub fn show_panels(app_handle: &AppHandle) {
    revive_webview_if_empty(app_handle, CONTROL_LABEL);
    if let Some(control) = app_handle.get_webview_window(CONTROL_LABEL) {
        show_window_without_activation(&control);
    }
    sync_session_panel_visibility(app_handle);
}

pub fn hide_panels(app_handle: &AppHandle) {
    hide_session_panel(app_handle);
    if let Some(control) = app_handle.get_webview_window(CONTROL_LABEL) {
        hide_window_without_activation(&control);
    }
}

pub fn is_control_panel_visible(app_handle: &AppHandle) -> bool {
    app_handle
        .get_webview_window(CONTROL_LABEL)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

#[tauri::command]
pub fn toggle_screen_share_visibility<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<bool, String> {
    let data = app.state::<Mutex<AppState>>();
    let new_private = {
        let mut state = data.lock().unwrap();
        state.is_screen_sharing_private = !state.is_screen_sharing_private;
        state.is_screen_sharing_private
    };

    for label in [CONTROL_LABEL, SESSION_LABEL] {
        if let Some(window) = app.get_webview_window(label) {
            apply_capture_affinity(&window, new_private);
        }
    }

    let _ = app.emit("privacy-state-changed", new_private);

    Ok(new_private)
}
