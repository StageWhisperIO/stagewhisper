use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use super::*;
use crate::loopback_repair::FIRST_RETRY;

#[derive(Default)]
struct DeviceState {
    unavailable: HashSet<String>,
    fails_to_start: HashSet<String>,
    opened: Vec<(String, usize)>,
    mixed_sources: Vec<usize>,
    next_serial: u32,
}

#[derive(Clone, Default)]
struct FakeDevices(Rc<RefCell<DeviceState>>);

impl FakeDevices {
    fn make_unavailable(&self, id: &str) {
        self.0.borrow_mut().unavailable.insert(id.to_string());
    }

    fn make_available(&self, id: &str) {
        self.0.borrow_mut().unavailable.remove(id);
    }

    fn make_fail_to_start(&self, id: &str) {
        self.0.borrow_mut().fails_to_start.insert(id.to_string());
    }

    fn opens_of(&self, id: &str) -> usize {
        self.0
            .borrow()
            .opened
            .iter()
            .filter(|(opened, _)| opened == id)
            .count()
    }

    fn last_open(&self) -> Option<(String, usize)> {
        self.0.borrow().opened.last().cloned()
    }

    fn mixed_sources(&self) -> Vec<usize> {
        self.0.borrow().mixed_sources.clone()
    }
}

#[derive(Debug)]
struct FakeLeg {
    endpoint: String,
    source: usize,
    serial: u32,
}

impl EndpointDevices for FakeDevices {
    type Leg = FakeLeg;

    fn open(
        &self,
        _role: &'static str,
        endpoint: &Endpoint,
        source: usize,
    ) -> Result<FakeLeg, String> {
        let mut state = self.0.borrow_mut();
        state.opened.push((endpoint.id.clone(), source));
        if state.unavailable.contains(&endpoint.id) {
            return Err(format!("{} is in use by another app", endpoint.id));
        }
        state.next_serial += 1;
        Ok(FakeLeg {
            endpoint: endpoint.id.clone(),
            source,
            serial: state.next_serial,
        })
    }

    fn play(&self, leg: &FakeLeg) -> Result<(), String> {
        if self.0.borrow().fails_to_start.contains(&leg.endpoint) {
            return Err("loopback did not start".to_string());
        }
        Ok(())
    }

    fn describe(&self, _leg: &FakeLeg) -> String {
        "48000 Hz, 2 ch, F32".to_string()
    }

    fn reset_sources(&self, sources: usize) {
        self.0.borrow_mut().mixed_sources.push(sources);
    }
}

fn endpoint(id: &str) -> Endpoint {
    Endpoint {
        id: id.to_string(),
        name: id.to_string(),
    }
}

fn defaults(console: &str, communications: &str) -> DefaultEndpoints {
    DefaultEndpoints {
        console: Some(endpoint(console)),
        communications: Some(endpoint(communications)),
    }
}

fn binder(devices: &FakeDevices) -> (Binder<FakeDevices>, Arc<Mutex<Option<String>>>) {
    let stop_error = Arc::new(Mutex::new(None));
    (
        Binder::new(devices.clone(), Arc::clone(&stop_error)),
        stop_error,
    )
}

fn running(binder: &Binder<FakeDevices>) -> Vec<&str> {
    binder
        .legs
        .iter()
        .map(|leg| leg.endpoint.as_str())
        .collect()
}

fn partially_bound(devices: &FakeDevices, now: Instant) -> Binder<FakeDevices> {
    devices.make_unavailable("headset");
    let (mut binder, _) = binder(devices);
    binder.bind(defaults("speakers", "headset"), now).unwrap();
    binder
}

#[test]
fn a_partial_bind_keeps_the_working_endpoint_and_schedules_a_retry() {
    let devices = FakeDevices::default();
    let now = Instant::now();
    let binder = partially_bound(&devices, now);

    assert_eq!(running(&binder), ["speakers"]);
    assert_eq!(binder.repair_due(), Some(now + FIRST_RETRY));
}

#[test]
fn retries_leave_the_working_endpoint_running_while_the_other_stays_unavailable() {
    let devices = FakeDevices::default();
    let now = Instant::now();
    let mut binder = partially_bound(&devices, now);
    let speakers_leg = binder.legs[0].serial;

    for _ in 0..3 {
        binder.repair_missing(now);
    }

    assert_eq!(running(&binder), ["speakers"]);
    assert_eq!(binder.legs[0].serial, speakers_leg);
    assert_eq!(devices.opens_of("speakers"), 1);
    assert_eq!(devices.opens_of("headset"), 4);
    assert_eq!(devices.mixed_sources(), [1]);
    assert_eq!(binder.repair_due(), Some(now + Duration::from_secs(16)));
}

#[test]
fn a_retry_probes_with_a_source_the_mixer_ignores() {
    let devices = FakeDevices::default();
    let now = Instant::now();
    let mut binder = partially_bound(&devices, now);

    binder.repair_missing(now);

    assert_eq!(
        devices.last_open(),
        Some(("headset".to_string(), PROBE_SOURCE))
    );
}

#[test]
fn a_recovered_endpoint_is_bound_alongside_the_working_one() {
    let devices = FakeDevices::default();
    let now = Instant::now();
    let mut binder = partially_bound(&devices, now);

    devices.make_available("headset");
    binder.repair_missing(now);

    assert_eq!(running(&binder), ["speakers", "headset"]);
    let sources: Vec<usize> = binder.legs.iter().map(|leg| leg.source).collect();
    assert_eq!(sources, [0, 1]);
    assert_eq!(devices.mixed_sources(), [1, 2]);
    assert_eq!(binder.repair_due(), None);
}

#[test]
fn a_device_notification_retries_at_once_when_the_defaults_are_unchanged() {
    let devices = FakeDevices::default();
    let now = Instant::now();
    let mut binder = partially_bound(&devices, now);

    devices.make_available("headset");
    binder.rebind(defaults("speakers", "headset"), false, now);

    assert_eq!(running(&binder), ["speakers", "headset"]);
    assert_eq!(binder.repair_due(), None);
}

#[test]
fn a_notification_with_everything_bound_changes_nothing() {
    let devices = FakeDevices::default();
    let now = Instant::now();
    let (mut binder, _) = binder(&devices);
    binder.bind(defaults("speakers", "headset"), now).unwrap();

    binder.rebind(defaults("speakers", "headset"), false, now);

    assert_eq!(running(&binder), ["speakers", "headset"]);
    assert_eq!(devices.opens_of("speakers"), 1);
    assert_eq!(devices.opens_of("headset"), 1);
    assert_eq!(binder.repair_due(), None);
}

#[test]
fn an_endpoint_that_opens_but_does_not_start_is_retried() {
    let devices = FakeDevices::default();
    devices.make_fail_to_start("headset");
    let now = Instant::now();
    let (mut binder, _) = binder(&devices);

    binder.bind(defaults("speakers", "headset"), now).unwrap();

    assert_eq!(running(&binder), ["speakers"]);
    assert_eq!(binder.repair_due(), Some(now + FIRST_RETRY));
}

#[test]
fn a_recovery_that_loses_the_other_endpoint_keeps_what_it_can_and_retries() {
    let devices = FakeDevices::default();
    let now = Instant::now();
    let mut binder = partially_bound(&devices, now);

    devices.make_available("headset");
    devices.make_unavailable("speakers");
    binder.repair_missing(now);

    assert_eq!(running(&binder), ["headset"]);
    assert_eq!(binder.repair_due(), Some(now + FIRST_RETRY));
}

#[test]
fn nothing_opening_is_an_error_without_a_retry() {
    let devices = FakeDevices::default();
    devices.make_unavailable("speakers");
    devices.make_unavailable("headset");
    let (mut binder, _) = binder(&devices);

    assert!(binder
        .bind(defaults("speakers", "headset"), Instant::now())
        .is_err());
    assert!(running(&binder).is_empty());
    assert_eq!(binder.repair_due(), None);
}

#[test]
fn changed_defaults_rebind_to_the_new_endpoints() {
    let devices = FakeDevices::default();
    let now = Instant::now();
    let (mut binder, _) = binder(&devices);
    binder.bind(defaults("speakers", "headset"), now).unwrap();

    binder.rebind(defaults("speakers", "usb-headset"), false, now);

    assert_eq!(running(&binder), ["speakers", "usb-headset"]);
}

#[test]
fn a_stream_that_keeps_failing_stops_capture_and_cancels_the_retry() {
    let devices = FakeDevices::default();
    let now = Instant::now();
    devices.make_unavailable("headset");
    let (mut binder, stop_error) = binder(&devices);
    binder.bind(defaults("speakers", "headset"), now).unwrap();

    binder.rebind(defaults("speakers", "headset"), true, now);
    assert_eq!(running(&binder), ["speakers"]);
    binder.rebind(
        defaults("speakers", "headset"),
        true,
        now + Duration::from_secs(1),
    );

    assert!(running(&binder).is_empty());
    assert_eq!(binder.repair_due(), None);
    assert!(stop_error.lock().unwrap().is_some());
}

#[test]
fn the_same_device_in_both_roles_is_captured_once() {
    let devices = FakeDevices::default();
    let (mut binder, _) = binder(&devices);

    binder
        .bind(defaults("speakers", "speakers"), Instant::now())
        .unwrap();

    assert_eq!(running(&binder), ["speakers"]);
    assert_eq!(devices.opens_of("speakers"), 1);
    assert_eq!(binder.repair_due(), None);
}
