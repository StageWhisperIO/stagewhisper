use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

use crate::ui_locale::{fill, text, NativeText};

pub fn spawn_update_check(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let updater = match app.updater() {
            Ok(updater) => updater,
            Err(err) => {
                eprintln!("[updater] not configured: {err}");
                return;
            }
        };

        let update = match updater.check().await {
            Ok(Some(update)) => update,
            Ok(None) => return,
            Err(err) => {
                eprintln!("[updater] check failed: {err}");
                return;
            }
        };

        let version = update.version.clone();
        let confirmed = app
            .dialog()
            .message(fill(
                text(NativeText::UpdateAvailableMessage),
                &[("version", &version)],
            ))
            .title(text(NativeText::UpdateAvailableTitle))
            .kind(MessageDialogKind::Info)
            .buttons(MessageDialogButtons::OkCancelCustom(
                text(NativeText::UpdateInstall).to_string(),
                text(NativeText::UpdateLater).to_string(),
            ))
            .blocking_show();

        if !confirmed {
            return;
        }

        if let Err(err) = update.download_and_install(|_, _| {}, || {}).await {
            eprintln!("[updater] install failed: {err}");
            return;
        }

        #[cfg(not(target_os = "windows"))]
        app.restart();
    });
}
