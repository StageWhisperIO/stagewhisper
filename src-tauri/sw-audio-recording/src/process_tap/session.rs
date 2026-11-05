use std::ffi::CStr;
use std::ptr::NonNull;

use objc2::AnyThread;
use objc2_core_audio::{
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey,
    kAudioAggregateDeviceNameKey, kAudioAggregateDevicePropertyTapList,
    kAudioAggregateDeviceSubDeviceListKey, kAudioAggregateDeviceUIDKey, kAudioObjectUnknown,
    kAudioTapPropertyFormat, kAudioTapPropertyUID, AudioDeviceCreateIOProcID,
    AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart, AudioDeviceStop,
    AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap,
    AudioHardwareDestroyAggregateDevice, AudioHardwareDestroyProcessTap, AudioObjectID,
    CATapDescription, CATapMuteBehavior,
};
use objc2_core_audio_types::AudioStreamBasicDescription;
use objc2_core_foundation::{CFArray, CFBoolean, CFDictionary, CFRetained, CFString, CFType};
use objc2_foundation::{NSArray, NSNumber, NSString};

use super::hal::{
    check, describe_default_output, own_process_object, read_string, read_value, status_error,
    write_value, ListenerTarget, PropertyListener,
};
use super::io::{io_proc, IoContext, TapFormat};
use super::SharedAudioCallback;

const TAP_NAME: &str = "StageWhisper system audio";
const AGGREGATE_NAME: &str = "StageWhisper system audio tap";

pub(super) struct TapSession {
    tap: AudioObjectID,
    aggregate: AudioObjectID,
    io_proc: AudioDeviceIOProcID,
    io_context: Option<NonNull<IoContext>>,
    format_listener: Option<PropertyListener>,
    running: bool,
}

impl TapSession {
    pub(super) fn open(
        on_audio_data: SharedAudioCallback,
        listener: ListenerTarget,
    ) -> Result<Self, String> {
        let excluded = excluded_processes();
        let mut session = Self {
            tap: create_tap(&excluded)?,
            aggregate: kAudioObjectUnknown,
            io_proc: None,
            io_context: None,
            format_listener: None,
            running: false,
        };
        let format = read_format(session.tap)?;
        let tap_uid = read_string(session.tap, kAudioTapPropertyUID)
            .ok_or("the system audio tap has no UID")?;
        session.aggregate = create_aggregate(&tap_uid)?;
        attach_tap(session.aggregate, &tap_uid)?;

        let context = NonNull::from(Box::leak(Box::new(IoContext::new(on_audio_data, format))));
        session.io_context = Some(context);
        check(
            unsafe {
                AudioDeviceCreateIOProcID(
                    session.aggregate,
                    Some(io_proc),
                    context.as_ptr().cast(),
                    NonNull::from(&mut session.io_proc),
                )
            },
            "creating the system audio callback",
        )?;
        session.format_listener =
            PropertyListener::add(session.tap, kAudioTapPropertyFormat, listener)
                .map_err(|err| log::warn!("[audio] cannot watch the system audio format: {err}"))
                .ok();
        check(
            unsafe { AudioDeviceStart(session.aggregate, session.io_proc) },
            "starting the system audio tap",
        )?;
        session.running = true;
        log::info!(
            "[audio] system audio tap started for default output {}: {format}; excluding {} StageWhisper process object(s)",
            describe_default_output(),
            excluded.len()
        );
        Ok(session)
    }
}

impl Drop for TapSession {
    fn drop(&mut self) {
        if self.running {
            warn_on_failure(
                unsafe { AudioDeviceStop(self.aggregate, self.io_proc) },
                "stopping the system audio tap",
            );
        }
        if self.io_proc.is_some() {
            warn_on_failure(
                unsafe { AudioDeviceDestroyIOProcID(self.aggregate, self.io_proc) },
                "removing the system audio callback",
            );
        }
        self.format_listener.take();
        if self.aggregate != kAudioObjectUnknown {
            warn_on_failure(
                unsafe { AudioHardwareDestroyAggregateDevice(self.aggregate) },
                "destroying the system audio aggregate device",
            );
        }
        warn_on_failure(
            unsafe { AudioHardwareDestroyProcessTap(self.tap) },
            "destroying the system audio tap",
        );
        if let Some(context) = self.io_context.take() {
            drop(unsafe { Box::from_raw(context.as_ptr()) });
        }
    }
}

fn warn_on_failure(status: i32, action: &str) {
    if status != 0 {
        log::warn!("[audio] {}", status_error(status, action));
    }
}

fn excluded_processes() -> Vec<AudioObjectID> {
    match own_process_object() {
        Ok(object) if object != kAudioObjectUnknown => vec![object],
        Ok(_) => Vec::new(),
        Err(status) => {
            log::warn!(
                "[audio] {}; StageWhisper's own sounds may reach the transcript",
                status_error(status, "looking up StageWhisper's audio process")
            );
            Vec::new()
        }
    }
}

fn create_tap(excluded: &[AudioObjectID]) -> Result<AudioObjectID, String> {
    let numbers: Vec<_> = excluded.iter().map(|&id| NSNumber::new_u32(id)).collect();
    let processes = NSArray::from_retained_slice(&numbers);
    let description = unsafe {
        CATapDescription::initStereoGlobalTapButExcludeProcesses(
            CATapDescription::alloc(),
            &processes,
        )
    };
    unsafe {
        description.setName(&NSString::from_str(TAP_NAME));
        description.setPrivate(true);
        description.setMuteBehavior(CATapMuteBehavior::Unmuted);
    }
    let mut tap = kAudioObjectUnknown;
    check(
        unsafe { AudioHardwareCreateProcessTap(Some(&description), &mut tap) },
        "creating the system audio tap",
    )?;
    if tap == kAudioObjectUnknown {
        return Err("macOS did not create the system audio tap".to_string());
    }
    Ok(tap)
}

fn read_format(tap: AudioObjectID) -> Result<TapFormat, String> {
    let empty = AudioStreamBasicDescription {
        mSampleRate: 0.0,
        mFormatID: 0,
        mFormatFlags: 0,
        mBytesPerPacket: 0,
        mFramesPerPacket: 0,
        mBytesPerFrame: 0,
        mChannelsPerFrame: 0,
        mBitsPerChannel: 0,
        mReserved: 0,
    };
    let description = read_value(tap, kAudioTapPropertyFormat, empty)
        .map_err(|status| status_error(status, "reading the system audio format"))?;
    let format = TapFormat::from_description(&description);
    if !format.is_supported() {
        return Err(format!("unsupported system audio format: {format}"));
    }
    Ok(format)
}

fn cf_key(key: &CStr) -> CFRetained<CFString> {
    CFString::from_str(&key.to_string_lossy())
}

fn create_aggregate(tap_uid: &str) -> Result<AudioObjectID, String> {
    let keys = [
        cf_key(kAudioAggregateDeviceNameKey),
        cf_key(kAudioAggregateDeviceUIDKey),
        cf_key(kAudioAggregateDeviceSubDeviceListKey),
        cf_key(kAudioAggregateDeviceIsPrivateKey),
        cf_key(kAudioAggregateDeviceIsStackedKey),
    ];
    let name = CFString::from_str(AGGREGATE_NAME);
    let uid = CFString::from_str(&format!("{tap_uid}.aggregate"));
    let no_sub_devices = CFArray::<CFType>::empty();
    let values: [&CFType; 5] = [
        &name,
        &uid,
        &no_sub_devices,
        CFBoolean::new(true),
        CFBoolean::new(false),
    ];
    let key_refs: Vec<&CFString> = keys.iter().map(|key| &**key).collect();
    let description = CFDictionary::<CFString, CFType>::from_slices(&key_refs, &values);
    let mut aggregate = kAudioObjectUnknown;
    check(
        unsafe {
            AudioHardwareCreateAggregateDevice(
                description.as_opaque(),
                NonNull::from(&mut aggregate),
            )
        },
        "creating the system audio aggregate device",
    )?;
    Ok(aggregate)
}

fn attach_tap(aggregate: AudioObjectID, tap_uid: &str) -> Result<(), String> {
    let uid = CFString::from_str(tap_uid);
    let taps = CFArray::<CFString>::from_objects(&[&*uid]);
    let taps_ref: *const CFArray<CFString> = &*taps;
    check(
        write_value(aggregate, kAudioAggregateDevicePropertyTapList, &taps_ref),
        "adding the tap to the system audio aggregate device",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "talks to coreaudiod; run by hand, it never starts IO so it never prompts"]
    fn a_tap_and_its_aggregate_device_build_and_tear_down_without_starting() {
        let excluded = excluded_processes();
        let tap = create_tap(&excluded).expect("tap");
        let format = read_format(tap).expect("format");
        let tap_uid = read_string(tap, kAudioTapPropertyUID).expect("tap uid");
        let aggregate = create_aggregate(&tap_uid).expect("aggregate");
        attach_tap(aggregate, &tap_uid).expect("attach");
        println!(
            "tap {tap} ({tap_uid}), aggregate {aggregate}, {format}, excluded {excluded:?}, default output {}",
            describe_default_output()
        );
        assert_eq!(unsafe { AudioHardwareDestroyAggregateDevice(aggregate) }, 0);
        assert_eq!(unsafe { AudioHardwareDestroyProcessTap(tap) }, 0);
    }
}
