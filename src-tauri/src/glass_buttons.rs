#![allow(clippy::let_unit_value)]

#[cfg(all(target_os = "macos", has_swift_glass))]
#[allow(dead_code)]
mod macos {
    use std::sync::{Mutex, OnceLock};

    use tauri::{AppHandle, Manager};
    use tauri_nspanel::objc2::msg_send;
    use tauri_nspanel::objc2::runtime::{AnyClass, AnyObject};
    use tauri_nspanel::objc2_foundation::{NSPoint, NSRect, NSSize};

    use crate::state::app_state::AppState;
    use crate::state::session::SessionState;

    extern "C" {
        fn sw_glass_create_control_bar() -> *mut AnyObject;

        fn sw_glass_update_state(is_listening: bool, is_ready: bool);

        fn sw_glass_set_callbacks(
            listen: extern "C" fn(),
            settings: extern "C" fn(),
            connect_ai: extern "C" fn(),
        );
    }

    static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

    struct HostingViewRef {
        #[allow(dead_code)]
        view: *mut AnyObject,
    }
    unsafe impl Send for HostingViewRef {}

    static HOSTING_VIEW: OnceLock<Mutex<HostingViewRef>> = OnceLock::new();

    const PANEL_WIDTH: f64 = 300.0;
    const PANEL_HEIGHT: f64 = 44.0;

    extern "C" fn on_listen_clicked() {
        if let Some(app) = APP_HANDLE.get() {
            let app_clone = app.clone();
            std::thread::spawn(move || {
                crate::state::session::toggle_session_state(app_clone);
            });
        }
    }

    extern "C" fn on_settings_clicked() {
        if let Some(app) = APP_HANDLE.get() {
            let _ = crate::panels::open_settings_window(app.clone());
        }
    }

    extern "C" fn on_connect_ai_clicked() {
        if let Some(app) = APP_HANDLE.get() {
            let _ = crate::panels::open_settings_window(app.clone());
        }
    }

    fn push_state_to_swift(app_handle: &AppHandle) {
        let is_listening = {
            let data = app_handle.state::<Mutex<AppState>>();
            let state = data.lock().unwrap();
            state.session_state == SessionState::Listening
        };
        let is_ready = crate::state::local_llm::engine_ready(app_handle);

        unsafe {
            sw_glass_update_state(is_listening, is_ready);
        }
    }

    unsafe fn hide_webviews_in(content_view: *mut AnyObject) {
        let wk_class = AnyClass::get(c"WKWebView");
        let Some(wk_cls) = wk_class else {
            return;
        };

        let subviews: *mut AnyObject = msg_send![content_view, subviews];
        let count: usize = msg_send![subviews, count];

        for i in 0..count {
            let subview: *mut AnyObject = msg_send![subviews, objectAtIndex: i];
            let is_webview: bool = msg_send![subview, isKindOfClass: wk_cls];
            if is_webview {
                let _: () = msg_send![subview, setHidden: true];
            }
        }
    }

    pub fn inject_glass_buttons(app_handle: &AppHandle) {
        APP_HANDLE.set(app_handle.clone()).ok();

        unsafe {
            sw_glass_set_callbacks(
                on_listen_clicked,
                on_settings_clicked,
                on_connect_ai_clicked,
            );

            let hosting_view = sw_glass_create_control_bar();
            if hosting_view.is_null() {
                eprintln!("info: sw_glass_create_control_bar returned null — HTML fallback active");
                return;
            }

            let control_window = app_handle
                .get_webview_window("main")
                .expect("main window must exist");
            let ns_window: *mut AnyObject =
                msg_send![control_window.ns_window().unwrap() as *mut AnyObject, self];
            let content_view: *mut AnyObject = msg_send![ns_window, contentView];

            hide_webviews_in(content_view);

            let panel_frame = NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(PANEL_WIDTH, PANEL_HEIGHT),
            );
            let _: () = msg_send![hosting_view, setFrame: panel_frame];

            let _: () = msg_send![content_view, addSubview: hosting_view];

            let _: () =
                msg_send![hosting_view, setTranslatesAutoresizingMaskIntoConstraints: false];

            let content_leading: *mut AnyObject = msg_send![content_view, leadingAnchor];
            let hosting_leading: *mut AnyObject = msg_send![hosting_view, leadingAnchor];
            let leading_constraint: *mut AnyObject =
                msg_send![hosting_leading, constraintEqualToAnchor: content_leading];
            let _: () = msg_send![leading_constraint, setActive: true];

            let content_trailing: *mut AnyObject = msg_send![content_view, trailingAnchor];
            let hosting_trailing: *mut AnyObject = msg_send![hosting_view, trailingAnchor];
            let trailing_constraint: *mut AnyObject =
                msg_send![hosting_trailing, constraintEqualToAnchor: content_trailing];
            let _: () = msg_send![trailing_constraint, setActive: true];

            let content_top: *mut AnyObject = msg_send![content_view, topAnchor];
            let hosting_top: *mut AnyObject = msg_send![hosting_view, topAnchor];
            let top_constraint: *mut AnyObject =
                msg_send![hosting_top, constraintEqualToAnchor: content_top];
            let _: () = msg_send![top_constraint, setActive: true];

            let content_bottom: *mut AnyObject = msg_send![content_view, bottomAnchor];
            let hosting_bottom: *mut AnyObject = msg_send![hosting_view, bottomAnchor];
            let bottom_constraint: *mut AnyObject =
                msg_send![hosting_bottom, constraintEqualToAnchor: content_bottom];
            let _: () = msg_send![bottom_constraint, setActive: true];

            push_state_to_swift(app_handle);

            HOSTING_VIEW
                .set(Mutex::new(HostingViewRef { view: hosting_view }))
                .ok();
        }
    }

    pub fn sync_button_appearance(app_handle: &AppHandle) {
        push_state_to_swift(app_handle);
    }

    pub fn set_glass_onboarding_visible(_visible: bool) {}
}

#[cfg(all(target_os = "macos", has_swift_glass))]
#[allow(unused_imports)]
pub use macos::{inject_glass_buttons, set_glass_onboarding_visible, sync_button_appearance};

#[cfg(not(all(target_os = "macos", has_swift_glass)))]
pub fn inject_glass_buttons(_app_handle: &tauri::AppHandle) {}

#[cfg(not(all(target_os = "macos", has_swift_glass)))]
pub fn sync_button_appearance(_app_handle: &tauri::AppHandle) {}

#[cfg(not(all(target_os = "macos", has_swift_glass)))]
pub fn set_glass_onboarding_visible(_visible: bool) {}
