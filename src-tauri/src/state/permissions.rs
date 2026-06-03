use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

#[cfg(all(target_os = "macos", has_swift_microphone))]
extern "C" {
    fn sw_microphone_auth_status() -> i32;
    fn sw_microphone_request_access() -> bool;
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum PermissionState {
    Granted,
    Denied,
    Undetermined,
    Unsupported,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsStatus {
    pub microphone: PermissionState,
}

const PERMISSIONS_STATUS_CHANGED_EVENT: &str = "permissions-status-changed";

fn microphone_auth_status() -> i32 {
    #[cfg(all(target_os = "macos", has_swift_microphone))]
    {
        unsafe { sw_microphone_auth_status() }
    }
    #[cfg(not(all(target_os = "macos", has_swift_microphone)))]
    {
        0
    }
}

fn microphone_request_access() -> bool {
    #[cfg(all(target_os = "macos", has_swift_microphone))]
    {
        unsafe { sw_microphone_request_access() }
    }
    #[cfg(not(all(target_os = "macos", has_swift_microphone)))]
    {
        false
    }
}

fn map_microphone_status(code: i32) -> PermissionState {
    match code {
        3 => PermissionState::Granted,
        2 => PermissionState::Denied,
        1 => PermissionState::Denied,
        _ => PermissionState::Undetermined,
    }
}

fn build_permissions_status() -> PermissionsStatus {
    let microphone = map_microphone_status(microphone_auth_status());

    PermissionsStatus { microphone }
}

#[tauri::command]
pub async fn get_permissions_status() -> Result<PermissionsStatus, String> {
    Ok(build_permissions_status())
}

#[tauri::command]
pub async fn request_microphone_permission(app: AppHandle) -> Result<PermissionState, String> {
    let _ = microphone_request_access();
    let state = map_microphone_status(microphone_auth_status());
    let status = build_permissions_status();
    let _ = app.emit(PERMISSIONS_STATUS_CHANGED_EVENT, &status);
    Ok(state)
}
