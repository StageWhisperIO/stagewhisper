use std::ffi::{c_int, c_void, CStr};
use std::sync::{Mutex, OnceLock};

use block2::{Block, RcBlock};
use objc2_core_foundation::{CFDictionary, CFString};

const TCC_FRAMEWORK: &CStr = c"/System/Library/PrivateFrameworks/TCC.framework/Versions/A/TCC";
const AUDIO_CAPTURE_SERVICE: &str = "kTCCServiceAudioCapture";
const PREFLIGHT_GRANTED: c_int = 0;
const PREFLIGHT_DENIED: c_int = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemAudioAccess {
    Granted,
    Denied,
    Unknown,
}

type PreflightFn = unsafe extern "C" fn(*const CFString, *const CFDictionary) -> c_int;
type RequestFn =
    unsafe extern "C" fn(*const CFString, *const CFDictionary, *const Block<dyn Fn(u8)>);

struct TccFunctions {
    preflight: Option<PreflightFn>,
    request: Option<RequestFn>,
}

fn tcc() -> &'static TccFunctions {
    static FUNCTIONS: OnceLock<TccFunctions> = OnceLock::new();
    FUNCTIONS.get_or_init(load_tcc)
}

fn load_tcc() -> TccFunctions {
    let handle = unsafe { libc::dlopen(TCC_FRAMEWORK.as_ptr(), libc::RTLD_NOW) };
    if handle.is_null() {
        log::warn!("[permissions] TCC is unavailable; system audio access cannot be checked");
        return TccFunctions {
            preflight: None,
            request: None,
        };
    }
    let preflight = symbol(handle, c"TCCAccessPreflight")
        .map(|address| unsafe { std::mem::transmute::<*mut c_void, PreflightFn>(address) });
    let request = symbol(handle, c"TCCAccessRequest")
        .map(|address| unsafe { std::mem::transmute::<*mut c_void, RequestFn>(address) });
    if preflight.is_none() || request.is_none() {
        log::warn!("[permissions] TCC lacks the system audio access functions; access is unknown");
    }
    TccFunctions { preflight, request }
}

fn symbol(handle: *mut c_void, name: &CStr) -> Option<*mut c_void> {
    let address = unsafe { libc::dlsym(handle, name.as_ptr()) };
    (!address.is_null()).then_some(address)
}

fn access_from_preflight(result: c_int) -> SystemAudioAccess {
    match result {
        PREFLIGHT_GRANTED => SystemAudioAccess::Granted,
        PREFLIGHT_DENIED => SystemAudioAccess::Denied,
        _ => SystemAudioAccess::Unknown,
    }
}

pub fn system_audio_access() -> SystemAudioAccess {
    let Some(preflight) = tcc().preflight else {
        return SystemAudioAccess::Unknown;
    };
    let service = CFString::from_static_str(AUDIO_CAPTURE_SERVICE);
    access_from_preflight(unsafe { preflight(&*service, std::ptr::null()) })
}

pub fn request_system_audio_access(on_answer: impl FnOnce(bool) + Send + 'static) -> bool {
    let Some(request) = tcc().request else {
        return false;
    };
    let on_answer = Mutex::new(Some(on_answer));
    let completion = RcBlock::new(move |granted: u8| {
        let answer = on_answer.lock().ok().and_then(|mut pending| pending.take());
        if let Some(answer) = answer {
            answer(granted != 0);
        }
    });
    let service = CFString::from_static_str(AUDIO_CAPTURE_SERVICE);
    unsafe { request(&*service, std::ptr::null(), &*completion) };
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_codes_map_to_access() {
        assert_eq!(access_from_preflight(0), SystemAudioAccess::Granted);
        assert_eq!(access_from_preflight(1), SystemAudioAccess::Denied);
        assert_eq!(access_from_preflight(2), SystemAudioAccess::Unknown);
        assert_eq!(access_from_preflight(-1), SystemAudioAccess::Unknown);
    }

    #[test]
    fn checking_access_never_prompts_or_crashes() {
        let access = system_audio_access();
        println!("system audio access for this test process: {access:?}");
        assert!(matches!(
            access,
            SystemAudioAccess::Granted | SystemAudioAccess::Denied | SystemAudioAccess::Unknown
        ));
    }
}
