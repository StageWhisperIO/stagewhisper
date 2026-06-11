use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

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
            .message(format!(
                "StageWhisper {version} is available. Install it and restart now?"
            ))
            .title("Update available")
            .kind(MessageDialogKind::Info)
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Install".to_string(),
                "Later".to_string(),
            ))
            .blocking_show();

        if !confirmed {
            return;
        }

        if let Err(err) = update.download_and_install(|_, _| {}, || {}).await {
            eprintln!("[updater] install failed: {err}");
            return;
        }

        app.restart();
    });
}
