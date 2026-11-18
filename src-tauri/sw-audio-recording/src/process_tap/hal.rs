use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::NonNull;

use objc2_core_audio::{
    kAudioDevicePropertyDeviceUID, kAudioHardwarePropertyDefaultOutputDevice,
    kAudioHardwarePropertyTranslatePIDToProcessObject, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
    kAudioObjectUnknown, AudioObjectAddPropertyListener, AudioObjectGetPropertyData, AudioObjectID,
    AudioObjectPropertyAddress, AudioObjectPropertySelector, AudioObjectRemovePropertyListener,
    AudioObjectSetPropertyData,
};
use objc2_core_foundation::{CFRetained, CFString};

pub(super) const SYSTEM_OBJECT: AudioObjectID = kAudioObjectSystemObject as AudioObjectID;

pub(super) type ListenerProc = unsafe extern "C-unwind" fn(
    AudioObjectID,
    u32,
    NonNull<AudioObjectPropertyAddress>,
    *mut c_void,
) -> i32;

#[derive(Clone, Copy)]
pub(super) struct ListenerTarget {
    pub(super) proc_: ListenerProc,
    pub(super) client_data: *mut c_void,
}

pub(super) fn address(selector: AudioObjectPropertySelector) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

pub(super) fn status_error(status: i32, action: &str) -> String {
    let code = status.to_be_bytes();
    if code.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        format!(
            "{action} failed (status '{}')",
            String::from_utf8_lossy(&code)
        )
    } else {
        format!("{action} failed (status {status})")
    }
}

pub(super) fn check(status: i32, action: &str) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(status_error(status, action))
    }
}

fn read_with_qualifier<T>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    qualifier: Option<&i32>,
    mut value: T,
) -> Result<T, i32> {
    let mut size = size_of::<T>() as u32;
    let (qualifier_size, qualifier_data) = match qualifier {
        Some(qualifier) => (
            size_of::<i32>() as u32,
            (qualifier as *const i32).cast::<c_void>(),
        ),
        None => (0, std::ptr::null()),
    };
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address(selector)),
            qualifier_size,
            qualifier_data,
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    };
    if status != 0 {
        return Err(status);
    }
    if size as usize != size_of::<T>() {
        return Err(-1);
    }
    Ok(value)
}

pub(super) fn read_value<T>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    initial: T,
) -> Result<T, i32> {
    read_with_qualifier(object, selector, None, initial)
}

pub(super) fn read_string(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
) -> Option<String> {
    let raw: *const CFString = read_value(object, selector, std::ptr::null()).ok()?;
    let string = unsafe { CFRetained::from_raw(NonNull::new(raw.cast_mut())?) };
    Some(string.to_string())
}

pub(super) fn write_value<T>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    value: &T,
) -> i32 {
    unsafe {
        AudioObjectSetPropertyData(
            object,
            NonNull::from(&address(selector)),
            0,
            std::ptr::null(),
            size_of::<T>() as u32,
            NonNull::from(value).cast(),
        )
    }
}

pub(super) fn own_process_object() -> Result<AudioObjectID, i32> {
    let pid = std::process::id() as i32;
    read_with_qualifier(
        SYSTEM_OBJECT,
        kAudioHardwarePropertyTranslatePIDToProcessObject,
        Some(&pid),
        kAudioObjectUnknown,
    )
}

pub(super) fn describe_default_output() -> String {
    let Some(device) = read_value(
        SYSTEM_OBJECT,
        kAudioHardwarePropertyDefaultOutputDevice,
        kAudioObjectUnknown,
    )
    .ok()
    .filter(|&id| id != kAudioObjectUnknown) else {
        return "none".to_string();
    };
    let name = read_string(device, kAudioObjectPropertyName).unwrap_or_else(|| "unnamed".into());
    let uid =
        read_string(device, kAudioDevicePropertyDeviceUID).unwrap_or_else(|| "unknown".into());
    format!("\"{name}\" (uid {uid}, id {device})")
}

pub(super) struct PropertyListener {
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    target: ListenerTarget,
}

impl PropertyListener {
    pub(super) fn add(
        object: AudioObjectID,
        selector: AudioObjectPropertySelector,
        target: ListenerTarget,
    ) -> Result<Self, String> {
        let status = unsafe {
            AudioObjectAddPropertyListener(
                object,
                NonNull::from(&address(selector)),
                Some(target.proc_),
                target.client_data,
            )
        };
        check(status, "watching an audio property")?;
        Ok(Self {
            object,
            selector,
            target,
        })
    }
}

impl Drop for PropertyListener {
    fn drop(&mut self) {
        let status = unsafe {
            AudioObjectRemovePropertyListener(
                self.object,
                NonNull::from(&address(self.selector)),
                Some(self.target.proc_),
                self.target.client_data,
            )
        };
        if status != 0 {
            log::warn!(
                "[audio] {}",
                status_error(status, "removing an audio property listener")
            );
        }
    }
}
