use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

static SCREEN_RECORDING_REQUESTED: AtomicBool = AtomicBool::new(false);

#[cfg(all(target_os = "macos", has_swift_microphone))]
extern "C" {
    fn sw_microphone_request_access() -> bool;
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
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
    pub screen_recording: PermissionState,
    pub system_audio: PermissionState,
}

const PERMISSIONS_STATUS_CHANGED_EVENT: &str = "permissions-status-changed";

#[cfg(target_os = "macos")]
const SYSTEM_AUDIO_ANSWER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

#[cfg(target_os = "macos")]
pub(crate) fn system_audio_status() -> PermissionState {
    use sw_audio_recording::system_audio_access::{system_audio_access, SystemAudioAccess};
    match system_audio_access() {
        SystemAudioAccess::Granted => PermissionState::Granted,
        SystemAudioAccess::Denied => PermissionState::Denied,
        SystemAudioAccess::Unknown => PermissionState::Undetermined,
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn system_audio_status() -> PermissionState {
    PermissionState::Granted
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) fn system_audio_status() -> PermissionState {
    PermissionState::Unsupported
}

#[cfg(target_os = "macos")]
pub(crate) fn ask_for_system_audio(on_answer: impl FnOnce(bool) + Send + 'static) -> bool {
    sw_audio_recording::system_audio_access::request_system_audio_access(move |granted| {
        log::info!(
            "[permissions] system audio recording {}",
            if granted { "allowed" } else { "declined" }
        );
        on_answer(granted);
    })
}

#[cfg(target_os = "macos")]
fn resolve_system_audio_permission() -> PermissionState {
    let state = system_audio_status();
    if state != PermissionState::Undetermined {
        return state;
    }
    let (answered_tx, answered_rx) = std::sync::mpsc::channel();
    if ask_for_system_audio(move |granted| {
        let _ = answered_tx.send(granted);
    }) {
        let _ = answered_rx.recv_timeout(SYSTEM_AUDIO_ANSWER_TIMEOUT);
    }
    system_audio_status()
}

#[cfg(not(target_os = "macos"))]
fn resolve_system_audio_permission() -> PermissionState {
    system_audio_status()
}

pub(crate) fn emit_permissions_status(app: &AppHandle) {
    let _ = app.emit(
        PERMISSIONS_STATUS_CHANGED_EVENT,
        &build_permissions_status(),
    );
}

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

#[cfg(not(any(target_vendor = "apple", target_os = "windows")))]
fn microphone_auth_status() -> i32 {
    0
}

#[cfg(target_os = "windows")]
const WINDOWS_MIC_GUIDANCE: &str =
    "allow microphone access in Windows Settings > Privacy & security > Microphone";

#[cfg(target_os = "windows")]
static WINDOWS_MIC_PROBE_STATUS: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(0);

#[cfg(target_os = "windows")]
fn windows_capability_microphone_status() -> i32 {
    use windows::Security::Authorization::AppCapabilityAccess::{
        AppCapability, AppCapabilityAccessStatus,
    };
    let Ok(capability) = AppCapability::Create(&windows::core::HSTRING::from("microphone")) else {
        return 0;
    };
    match capability.CheckAccess() {
        Ok(status) if status == AppCapabilityAccessStatus::Allowed => 3,
        Ok(status) if status == AppCapabilityAccessStatus::DeniedByUser => 2,
        Ok(status) if status == AppCapabilityAccessStatus::DeniedBySystem => 1,
        _ => 0,
    }
}

#[cfg(target_os = "windows")]
fn microphone_auth_status() -> i32 {
    let live = windows_capability_microphone_status();
    if live != 0 {
        return live;
    }
    WINDOWS_MIC_PROBE_STATUS.load(Ordering::Relaxed)
}

#[cfg(not(target_os = "windows"))]
fn swift_microphone_supported() -> bool {
    cfg!(all(target_os = "macos", has_swift_microphone))
}

#[cfg(not(target_os = "windows"))]
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

#[cfg(not(target_os = "windows"))]
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

#[cfg(not(target_os = "windows"))]
fn resolve_microphone_permission() -> PermissionState {
    if swift_microphone_supported() {
        let _ = microphone_request_access();
    } else {
        poke_and_await_microphone();
    }
    map_microphone_status(microphone_auth_status())
}

#[cfg(target_os = "windows")]
fn resolve_microphone_permission() -> PermissionState {
    let capability_status = windows_capability_microphone_status();
    let status = match sw_audio_recording::build_mic_input_stream(|_: Vec<i16>| {}) {
        Ok(stream) => {
            drop(stream);
            if capability_status == 1 || capability_status == 2 {
                capability_status
            } else {
                3
            }
        }
        Err(err) => {
            eprintln!("[permissions] microphone probe failed: {err}; {WINDOWS_MIC_GUIDANCE}");
            2
        }
    };
    WINDOWS_MIC_PROBE_STATUS.store(status, Ordering::Relaxed);
    map_microphone_status(status)
}

fn map_microphone_status(code: i32) -> PermissionState {
    match code {
        3 => PermissionState::Granted,
        2 => PermissionState::Denied,
        1 => PermissionState::Denied,
        _ => PermissionState::Undetermined,
    }
}

#[cfg(target_os = "macos")]
fn screen_capture_preflight() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }
    unsafe { CGPreflightScreenCaptureAccess() }
}

#[cfg(target_os = "macos")]
fn screen_capture_request() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGRequestScreenCaptureAccess() -> bool;
    }
    unsafe { CGRequestScreenCaptureAccess() }
}

fn screen_recording_status() -> PermissionState {
    #[cfg(target_os = "macos")]
    {
        if screen_capture_preflight() {
            PermissionState::Granted
        } else if SCREEN_RECORDING_REQUESTED.load(Ordering::Relaxed) {
            PermissionState::Denied
        } else {
            PermissionState::Undetermined
        }
    }
    #[cfg(target_os = "windows")]
    {
        PermissionState::Granted
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        PermissionState::Unsupported
    }
}

fn build_permissions_status() -> PermissionsStatus {
    PermissionsStatus {
        microphone: map_microphone_status(microphone_auth_status()),
        screen_recording: screen_recording_status(),
        system_audio: system_audio_status(),
    }
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

    #[test]
    fn system_audio_status_query_does_not_prompt_or_crash() {
        assert_ne!(
            super::system_audio_status(),
            super::PermissionState::Unsupported
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
        &build_permissions_status(),
    );
    Ok(state)
}

#[tauri::command]
pub async fn request_screen_recording_permission(
    app: AppHandle,
) -> Result<PermissionState, String> {
    let state = tokio::task::spawn_blocking(|| {
        #[cfg(target_os = "macos")]
        {
            let _ = screen_capture_request();
            SCREEN_RECORDING_REQUESTED.store(true, Ordering::Relaxed);
        }
        screen_recording_status()
    })
    .await
    .map_err(|err| format!("screen recording permission task failed: {err}"))?;
    let _ = app.emit(
        PERMISSIONS_STATUS_CHANGED_EVENT,
        &build_permissions_status(),
    );
    Ok(state)
}

#[tauri::command]
pub async fn request_system_audio_permission(app: AppHandle) -> Result<PermissionState, String> {
    let state = tokio::task::spawn_blocking(resolve_system_audio_permission)
        .await
        .map_err(|err| format!("system audio permission task failed: {err}"))?;
    emit_permissions_status(&app);
    Ok(state)
}
