mod endpoints;
mod leg;

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, unbounded, Receiver, RecvTimeoutError, Sender};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

use crate::loopback_binder::{Binder, Endpoint, EndpointDevices};
use crate::loopback_mix::{EndpointMixer, MAX_SOURCE_LEAD_SAMPLES};
use endpoints::EndpointWatch;
use leg::EndpointLeg;

type CaptureResult<T> = Result<T, Box<dyn std::error::Error>>;
type WasapiBinder = Binder<WasapiDevices>;

const REBIND_SETTLE: Duration = Duration::from_millis(300);

pub(super) enum Control {
    EndpointsChanged,
    LegFailed,
    Stop,
}

pub(super) struct Fanin {
    mixer: EndpointMixer,
    on_audio_data: Box<dyn FnMut(Vec<i16>) + Send>,
}

impl Fanin {
    fn push(&mut self, source: usize, chunk: &[i16]) {
        let mixed = self.mixer.push(source, chunk);
        if !mixed.is_empty() {
            (self.on_audio_data)(mixed);
        }
    }

    fn reset(&mut self, sources: usize) {
        self.mixer.reset(sources);
    }
}

struct Worker {
    control: Sender<Control>,
    thread: JoinHandle<()>,
}

pub struct LoopbackCapture {
    fanin: Arc<Mutex<Fanin>>,
    worker: Option<Worker>,
    stop_error: Arc<Mutex<Option<String>>>,
}

pub fn build_system_output_stream_streaming(
    on_audio_data: impl FnMut(Vec<i16>) + Send + 'static,
) -> CaptureResult<LoopbackCapture> {
    Ok(LoopbackCapture {
        fanin: Arc::new(Mutex::new(Fanin {
            mixer: EndpointMixer::new(MAX_SOURCE_LEAD_SAMPLES),
            on_audio_data: Box::new(on_audio_data),
        })),
        worker: None,
        stop_error: Arc::new(Mutex::new(None)),
    })
}

impl LoopbackCapture {
    pub fn start(&mut self) -> CaptureResult<()> {
        if self.worker.is_some() {
            return Ok(());
        }
        let (control_tx, control_rx) = unbounded();
        let (ready_tx, ready_rx) = bounded(1);
        let devices = WasapiDevices {
            fanin: Arc::clone(&self.fanin),
            control: control_tx.clone(),
        };
        let binder = Binder::new(devices, Arc::clone(&self.stop_error));
        let watch_control = control_tx.clone();
        let thread = std::thread::Builder::new()
            .name("sw-system-loopback".into())
            .spawn(move || run_binder(binder, watch_control, control_rx, ready_tx))?;
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
                Err("system loopback thread exited during start".into())
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

impl Drop for LoopbackCapture {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

struct WasapiDevices {
    fanin: Arc<Mutex<Fanin>>,
    control: Sender<Control>,
}

impl EndpointDevices for WasapiDevices {
    type Leg = EndpointLeg;

    fn open(
        &self,
        role: &'static str,
        endpoint: &Endpoint,
        source: usize,
    ) -> Result<EndpointLeg, String> {
        EndpointLeg::open(role, endpoint, source, &self.fanin, &self.control)
    }

    fn play(&self, leg: &EndpointLeg) -> Result<(), String> {
        leg.play()
    }

    fn describe(&self, leg: &EndpointLeg) -> String {
        leg.format().to_string()
    }

    fn reset_sources(&self, sources: usize) {
        if let Ok(mut fanin) = self.fanin.lock() {
            fanin.reset(sources);
        }
    }
}

fn run_binder(
    binder: WasapiBinder,
    control: Sender<Control>,
    control_rx: Receiver<Control>,
    ready_tx: Sender<Result<(), String>>,
) {
    if let Err(err) = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok() {
        let _ = ready_tx.send(Err(format!("CoInitializeEx failed: {err}")));
        return;
    }
    bind_and_watch(binder, control, &control_rx, &ready_tx);
    unsafe { CoUninitialize() };
}

fn bind_and_watch(
    mut binder: WasapiBinder,
    control: Sender<Control>,
    control_rx: &Receiver<Control>,
    ready_tx: &Sender<Result<(), String>>,
) {
    let watch = match EndpointWatch::register(control) {
        Ok(watch) => watch,
        Err(err) => {
            let _ = ready_tx.send(Err(format!("endpoint notifications unavailable: {err}")));
            return;
        }
    };
    let defaults = watch.defaults();
    log::info!("[audio] default output at capture start: {defaults}");
    if let Err(err) = binder.bind(defaults, Instant::now()) {
        let _ = ready_tx.send(Err(err));
        return;
    }
    let _ = ready_tx.send(Ok(()));

    loop {
        let received = match binder.repair_due() {
            Some(due) => control_rx.recv_deadline(due),
            None => control_rx
                .recv()
                .map_err(|_| RecvTimeoutError::Disconnected),
        };
        let leg_failed = match received {
            Err(RecvTimeoutError::Timeout) => {
                binder.repair_missing(Instant::now());
                continue;
            }
            Err(RecvTimeoutError::Disconnected) | Ok(Control::Stop) => break,
            Ok(Control::EndpointsChanged) => false,
            Ok(Control::LegFailed) => true,
        };
        match settle(control_rx, leg_failed) {
            Some(leg_failed) => binder.rebind(watch.defaults(), leg_failed, Instant::now()),
            None => break,
        }
    }
    binder.release();
    drop(watch);
}

fn settle(control_rx: &Receiver<Control>, mut leg_failed: bool) -> Option<bool> {
    loop {
        match control_rx.recv_timeout(REBIND_SETTLE) {
            Ok(Control::Stop) | Err(RecvTimeoutError::Disconnected) => return None,
            Ok(Control::LegFailed) => leg_failed = true,
            Ok(Control::EndpointsChanged) => {}
            Err(RecvTimeoutError::Timeout) => return Some(leg_failed),
        }
    }
}
