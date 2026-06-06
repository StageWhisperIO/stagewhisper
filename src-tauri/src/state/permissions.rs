use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

#[cfg(all(target_os = "macos", has_swift_microphone))]
extern "C" {
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

#[cfg(target_vendor = "apple")]
const AV_MEDIA_TYPE_AUDIO: &str = "soun";

#[cfg(target_vendor = "apple")]
fn microphone_auth_status() -> i32 {
    use objc2::msg_send;
    use objc2::runtime::AnyClass;
    use objc2_foundation::NSString;

    let Some(capture_device) = AnyClass::get(c"AVCaptureDevice") else {
        return 0;
    };
    let audio_media_type = NSString::from_str(AV_MEDIA_TYPE_AUDIO);
    let status: isize =
        unsafe { msg_send![capture_device, authorizationStatusForMediaType: &*audio_media_type] };
    status as i32
}

#[cfg(not(target_vendor = "apple"))]
fn microphone_auth_status() -> i32 {
    0
}

fn swift_microphone_supported() -> bool {
    cfg!(all(target_os = "macos", has_swift_microphone))
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

fn poke_and_await_microphone() {
    let stream = match sw_audio_recording::build_mic_input_stream(|_: Vec<i16>| {}) {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("[permissions] microphone access request failed: {err}");
            return;
        }
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while std::time::Instant::now() < deadline && microphone_auth_status() == 0 {
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    drop(stream);
}

fn resolve_microphone_permission() -> PermissionState {
    if swift_microphone_supported() {
        let _ = microphone_request_access();
    } else {
        poke_and_await_microphone();
    }
    map_microphone_status(microphone_auth_status())
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

#[cfg(all(test, target_vendor = "apple"))]
mod tests {
    #[test]
    fn microphone_status_query_does_not_crash() {
        let status = super::microphone_auth_status();
        assert!(
            (0..=3).contains(&status),
            "AVCaptureDevice authorization status out of range: {status}"
        );
    }
}

#[tauri::command]
pub async fn request_microphone_permission(app: AppHandle) -> Result<PermissionState, String> {
    let state = tokio::task::spawn_blocking(resolve_microphone_permission)
        .await
        .map_err(|err| format!("microphone permission task failed: {err}"))?;
    let _ = app.emit(
        PERMISSIONS_STATUS_CHANGED_EVENT,
        &PermissionsStatus { microphone: state },
    );
    Ok(state)
}
