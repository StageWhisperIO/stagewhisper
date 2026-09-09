use tauri::{
    image::Image,
    menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

use crate::panels::{hide_panels, is_control_panel_visible, show_panels};

const TRAY_ID: &str = "stagewhisper-tray";
const ITEM_OPEN_PANEL: &str = "tray.open_panel";
const ITEM_OPEN_SETTINGS: &str = "tray.open_settings";
const ITEM_QUIT: &str = "tray.quit";

const TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/32x32.png");

pub fn init_tray(app: &AppHandle) -> tauri::Result<()> {
    if app.tray_by_id(TRAY_ID).is_some() {
        return Ok(());
    }

    let menu = build_menu(app)?;
    let icon = match app.default_window_icon() {
        Some(icon) => icon.clone(),
        None => Image::from_bytes(TRAY_ICON_BYTES)?,
    };

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("StageWhisper")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(handle_tray_icon_event)
        .build(app)?;

    Ok(())
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let open_panel = MenuItem::with_id(
        app,
        ITEM_OPEN_PANEL,
        "Open Control Panel",
        true,
        None::<&str>,
    )?;
    let open_settings =
        MenuItem::with_id(app, ITEM_OPEN_SETTINGS, "Open Settings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, ITEM_QUIT, "Quit StageWhisper", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;

    let items: Vec<&dyn IsMenuItem<tauri::Wry>> =
        vec![&open_panel, &open_settings, &separator, &quit];

    Menu::with_items(app, &items)
}

fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    match event.id.as_ref() {
        ITEM_OPEN_PANEL => focus_control_panel(app),
        ITEM_OPEN_SETTINGS => open_settings(app),
        ITEM_QUIT => app.exit(0),
        _ => {}
    }
}

fn handle_tray_icon_event(tray: &tauri::tray::TrayIcon, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        toggle_control_panel(tray.app_handle());
    }
}

fn toggle_control_panel(app: &AppHandle) {
    let app_handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if is_control_panel_visible(&app_handle) {
            hide_panels(&app_handle);
        } else {
            show_panels(&app_handle);
        }
    });
}

pub fn focus_control_panel(app: &AppHandle) {
    let app_handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        show_panels(&app_handle);
        if let Some(window) = app_handle.get_webview_window("main") {
            let _ = window.set_focus();
        }
    });
}

fn open_settings(app: &AppHandle) {
    let app_handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Err(err) = crate::panels::open_settings_window(app_handle) {
            eprintln!("[tray] failed to open settings: {err}");
        }
    });
}
