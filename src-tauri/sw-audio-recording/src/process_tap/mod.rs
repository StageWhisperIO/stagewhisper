mod convert;
mod hal;
mod io;
mod session;

use std::ffi::c_void;
use std::fmt;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{bounded, unbounded, Receiver, RecvTimeoutError, Sender};
use objc2_core_audio::{
    kAudioHardwarePropertyDefaultOutputDevice, kAudioTapPropertyFormat, AudioObjectID,
    AudioObjectPropertyAddress,
};

use crate::system_audio_access::{system_audio_access, SystemAudioAccess};
use hal::{describe_default_output, ListenerTarget, PropertyListener, SYSTEM_OBJECT};
use session::TapSession;

type SharedAudioCallback = Arc<Mutex<dyn FnMut(Vec<i16>) + Send>>;
type CaptureResult<T> = Result<T, Box<dyn std::error::Error>>;

const REBUILD_SETTLE: Duration = Duration::from_millis(300);
const ACCESS_POLL: Duration = Duration::from_millis(500);
const SYSTEM_AUDIO_DENIED: &str = "macOS denied StageWhisper permission to record system audio";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RebuildCause {
    OutputDeviceChanged,
    FormatChanged,
    AccessGranted,
}

impl fmt::Display for RebuildCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RebuildCause::OutputDeviceChanged => "the default output device changed",
            RebuildCause::FormatChanged => "the system audio format changed",
            RebuildCause::AccessGranted => "macOS allowed system audio recording",
        })
    }
}

enum Control {
    Rebuild(RebuildCause),
    AccessDenied,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TapEvent {
    Rebuild(RebuildCause),
    AccessDenied,
}

struct Worker {
    control: Sender<Control>,
    thread: JoinHandle<()>,
}

pub struct ProcessTapCapture {
    on_audio_data: SharedAudioCallback,
    worker: Option<Worker>,
    stop_error: Arc<Mutex<Option<String>>>,
}

pub fn build_system_output_stream_streaming(
    on_audio_data: impl FnMut(Vec<i16>) + Send + 'static,
) -> CaptureResult<ProcessTapCapture> {
    Ok(ProcessTapCapture {
        on_audio_data: Arc::new(Mutex::new(on_audio_data)),
        worker: None,
        stop_error: Arc::new(Mutex::new(None)),
    })
}

impl ProcessTapCapture {
    pub fn start(&mut self) -> CaptureResult<()> {
        if self.worker.is_some() {
            return Ok(());
        }
        if system_audio_access() == SystemAudioAccess::Denied {
            return Err(SYSTEM_AUDIO_DENIED.into());
        }
        if let Ok(mut stop_error) = self.stop_error.lock() {
            stop_error.take();
        }
        let (control_tx, control_rx) = unbounded();
        let (ready_tx, ready_rx) = bounded(1);
        let tap = TapWorker {
            on_audio_data: Arc::clone(&self.on_audio_data),
            stop_error: Arc::clone(&self.stop_error),
            control: control_tx.clone(),
        };
        let thread = std::thread::Builder::new()
            .name("sw-system-tap".into())
            .spawn(move || tap.run(control_rx, ready_tx))?;
        match ready_rx.recv() {
            Ok(Ok(())) => {
                self.worker = Some(Worker {
                    control: control_tx,
                    thread,
                });
                Ok(())
            }
            Ok(Err(err)) => {
                let _ = thread.join();
                Err(err.into())
            }
            Err(_) => {
                let _ = thread.join();
                Err("system audio tap thread exited during start".into())
            }
        }
    }

    pub fn stop(&mut self) -> CaptureResult<()> {
        if let Some(worker) = self.worker.take() {
            let _ = worker.control.send(Control::Stop);
            let _ = worker.thread.join();
        }
        Ok(())
    }

    pub fn take_stop_error(&self) -> Option<String> {
        self.stop_error
            .lock()
            .ok()
            .and_then(|mut stop_error| stop_error.take())
    }
}

impl Drop for ProcessTapCapture {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

struct TapWorker {
    on_audio_data: SharedAudioCallback,
    stop_error: Arc<Mutex<Option<String>>>,
    control: Sender<Control>,
}

impl TapWorker {
    fn run(self, control_rx: Receiver<Control>, ready_tx: Sender<Result<(), String>>) {
        let control = Box::new(self.control.clone());
        let target = ListenerTarget {
            proc_: property_changed,
            client_data: (&*control as *const Sender<Control>).cast_mut().cast(),
        };
        let output_listener = PropertyListener::add(
            SYSTEM_OBJECT,
            kAudioHardwarePropertyDefaultOutputDevice,
            target,
        )
        .map_err(|err| log::warn!("[audio] cannot watch the default output device: {err}"))
        .ok();

        let mut session = match TapSession::open(Arc::clone(&self.on_audio_data), target) {
            Ok(session) => {
                let _ = ready_tx.send(Ok(()));
                Some(session)
            }
            Err(err) => {
                drop(output_listener);
                let _ = ready_tx.send(Err(err));
                return;
            }
        };

        let mut access = AccessWatch::new(system_audio_access);
        while let Some(event) = next_event(&control_rx, &mut access) {
            let cause = match event {
                TapEvent::Rebuild(cause) => cause,
                TapEvent::AccessDenied => {
                    self.record_stop(SYSTEM_AUDIO_DENIED.to_string());
                    break;
                }
            };
            match cause {
                RebuildCause::OutputDeviceChanged => log::info!(
                    "[audio] default output device changed to {}; rebuilding the system audio tap",
                    describe_default_output()
                ),
                RebuildCause::FormatChanged => {
                    log::info!(
                        "[audio] system audio format changed; rebuilding the system audio tap"
                    )
                }
                RebuildCause::AccessGranted => log::info!(
                    "[audio] system audio recording was allowed; rebuilding the system audio tap"
                ),
            }
            drop(session.take());
            session = TapSession::open(Arc::clone(&self.on_audio_data), target)
                .map_err(|err| self.record_failure(cause, err))
                .ok();
        }

        drop(session);
        drop(output_listener);
        drop(control);
        log::info!("[audio] system audio tap stopped");
    }

    fn record_failure(&self, cause: RebuildCause, err: String) {
        self.record_stop(format!(
            "the system audio tap could not be rebuilt after {cause}: {err}"
        ));
    }

    fn record_stop(&self, reason: String) {
        log::warn!("[audio] {reason}");
        if let Ok(mut stop_error) = self.stop_error.lock() {
            stop_error.get_or_insert(reason);
        }
    }
}

struct AccessWatch<F: FnMut() -> SystemAudioAccess> {
    check: F,
    undecided: bool,
}

impl<F: FnMut() -> SystemAudioAccess> AccessWatch<F> {
    fn new(mut check: F) -> Self {
        let undecided = check() == SystemAudioAccess::Unknown;
        Self { check, undecided }
    }

    fn next_control(&mut self, control_rx: &Receiver<Control>) -> Option<Control> {
        while self.undecided {
            match control_rx.recv_timeout(ACCESS_POLL) {
                Ok(control) => return Some(control),
                Err(RecvTimeoutError::Disconnected) => return None,
                Err(RecvTimeoutError::Timeout) => match (self.check)() {
                    SystemAudioAccess::Unknown => {}
                    SystemAudioAccess::Granted => {
                        self.undecided = false;
                        return Some(Control::Rebuild(RebuildCause::AccessGranted));
                    }
                    SystemAudioAccess::Denied => {
                        self.undecided = false;
                        return Some(Control::AccessDenied);
                    }
                },
            }
        }
        control_rx.recv().ok()
    }
}

fn next_event<F: FnMut() -> SystemAudioAccess>(
    control_rx: &Receiver<Control>,
    access: &mut AccessWatch<F>,
) -> Option<TapEvent> {
    let mut cause = match access.next_control(control_rx) {
        Some(Control::Rebuild(cause)) => cause,
        Some(Control::AccessDenied) => return Some(TapEvent::AccessDenied),
        Some(Control::Stop) | None => return None,
    };
    loop {
        match control_rx.recv_timeout(REBUILD_SETTLE) {
            Ok(Control::Rebuild(next)) => {
                if next != RebuildCause::FormatChanged {
                    cause = next;
                }
            }
            Ok(Control::AccessDenied) => return Some(TapEvent::AccessDenied),
            Ok(Control::Stop) | Err(RecvTimeoutError::Disconnected) => return None,
            Err(RecvTimeoutError::Timeout) => return Some(TapEvent::Rebuild(cause)),
        }
    }
}

unsafe extern "C-unwind" fn property_changed(
    _object: AudioObjectID,
    address_count: u32,
    addresses: NonNull<AudioObjectPropertyAddress>,
    client_data: *mut c_void,
) -> i32 {
    let Some(control) = NonNull::new(client_data.cast::<Sender<Control>>()) else {
        return 0;
    };
    let addresses =
        unsafe { std::slice::from_raw_parts(addresses.as_ptr(), address_count as usize) };
    for address in addresses {
        if let Some(cause) = rebuild_cause(address.mSelector) {
            let _ = unsafe { control.as_ref() }.send(Control::Rebuild(cause));
        }
    }
    0
}

fn rebuild_cause(selector: u32) -> Option<RebuildCause> {
    if selector == kAudioHardwarePropertyDefaultOutputDevice {
        Some(RebuildCause::OutputDeviceChanged)
    } else if selector == kAudioTapPropertyFormat {
        Some(RebuildCause::FormatChanged)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decided() -> AccessWatch<fn() -> SystemAudioAccess> {
        AccessWatch::new(|| SystemAudioAccess::Granted)
    }

    #[test]
    fn a_burst_of_changes_settles_into_one_rebuild() {
        let (tx, rx) = unbounded();
        tx.send(Control::Rebuild(RebuildCause::FormatChanged))
            .unwrap();
        tx.send(Control::Rebuild(RebuildCause::OutputDeviceChanged))
            .unwrap();
        tx.send(Control::Rebuild(RebuildCause::FormatChanged))
            .unwrap();
        assert_eq!(
            next_event(&rx, &mut decided()),
            Some(TapEvent::Rebuild(RebuildCause::OutputDeviceChanged))
        );
        assert!(rx.is_empty());
    }

    #[test]
    fn stop_wins_over_a_pending_rebuild() {
        let (tx, rx) = unbounded();
        tx.send(Control::Rebuild(RebuildCause::OutputDeviceChanged))
            .unwrap();
        tx.send(Control::Stop).unwrap();
        assert_eq!(next_event(&rx, &mut decided()), None);
    }

    #[test]
    fn a_tap_opened_before_access_was_decided_rebuilds_once_it_is_granted() {
        let (_tx, rx) = unbounded::<Control>();
        let mut answers = vec![
            SystemAudioAccess::Granted,
            SystemAudioAccess::Unknown,
            SystemAudioAccess::Unknown,
        ];
        let mut access =
            AccessWatch::new(move || answers.pop().unwrap_or(SystemAudioAccess::Granted));
        assert!(access.undecided);
        assert_eq!(
            next_event(&rx, &mut access),
            Some(TapEvent::Rebuild(RebuildCause::AccessGranted))
        );
        assert!(!access.undecided);
    }

    #[test]
    fn a_declined_prompt_stops_the_tap_instead_of_recording_silence() {
        let (_tx, rx) = unbounded::<Control>();
        let mut answers = vec![SystemAudioAccess::Denied, SystemAudioAccess::Unknown];
        let mut access =
            AccessWatch::new(move || answers.pop().unwrap_or(SystemAudioAccess::Denied));
        assert_eq!(next_event(&rx, &mut access), Some(TapEvent::AccessDenied));
        assert!(!access.undecided);
    }

    #[test]
    fn listener_notifications_become_rebuild_requests() {
        let (tx, rx) = unbounded();
        let addresses = [
            hal::address(kAudioHardwarePropertyDefaultOutputDevice),
            hal::address(kAudioTapPropertyFormat),
            hal::address(0),
        ];
        let client_data = (&tx as *const Sender<Control>).cast_mut().cast();
        let status = unsafe {
            property_changed(
                SYSTEM_OBJECT,
                addresses.len() as u32,
                NonNull::from(&addresses).cast(),
                client_data,
            )
        };
        assert_eq!(status, 0);
        let causes: Vec<_> = rx
            .try_iter()
            .filter_map(|control| match control {
                Control::Rebuild(cause) => Some(cause),
                Control::AccessDenied | Control::Stop => None,
            })
            .collect();
        assert_eq!(
            causes,
            vec![
                RebuildCause::OutputDeviceChanged,
                RebuildCause::FormatChanged
            ]
        );
    }
}
