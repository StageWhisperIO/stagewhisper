use crate::panels::{hide_panels, is_control_panel_visible, show_panels};
use tauri::AppHandle;

pub fn init_shortcuts(app_handle: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(desktop)]
    {
        use tauri_plugin_global_shortcut::{
            Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState,
        };

        let cmd_slash = Shortcut::new(Some(Modifiers::META), Code::Backslash);
        let app_handle_clone = app_handle.clone();

        app_handle.plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |_app, shortcut, event| {
                    if shortcut == &cmd_slash && event.state() == ShortcutState::Pressed {
                        if is_control_panel_visible(&app_handle_clone) {
                            hide_panels(&app_handle_clone);
                        } else {
                            show_panels(&app_handle_clone);
                        }
                    }
                })
                .build(),
        )?;

        app_handle.global_shortcut().register(cmd_slash)?;
    }

    Ok(())
}
