mod defaults;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::loopback_repair::EndpointRepair;
pub(crate) use defaults::{DefaultEndpoints, Endpoint};

const REPEATED_FAILURE_WINDOW: Duration = Duration::from_secs(10);
pub(crate) const PROBE_SOURCE: usize = usize::MAX;

pub(crate) trait EndpointDevices {
    type Leg;

    fn open(
        &self,
        role: &'static str,
        endpoint: &Endpoint,
        source: usize,
    ) -> Result<Self::Leg, String>;
    fn play(&self, leg: &Self::Leg) -> Result<(), String>;
    fn describe(&self, leg: &Self::Leg) -> String;
    fn reset_sources(&self, sources: usize);
}

pub(crate) struct Binder<D: EndpointDevices> {
    devices: D,
    stop_error: Arc<Mutex<Option<String>>>,
    legs: Vec<D::Leg>,
    bound: DefaultEndpoints,
    missing: Vec<(&'static str, Endpoint)>,
    repair: EndpointRepair,
    last_failure_rebind: Option<Instant>,
}

impl<D: EndpointDevices> Binder<D> {
    pub(crate) fn new(devices: D, stop_error: Arc<Mutex<Option<String>>>) -> Self {
        Self {
            devices,
            stop_error,
            legs: Vec::new(),
            bound: DefaultEndpoints::default(),
            missing: Vec::new(),
            repair: EndpointRepair::default(),
            last_failure_rebind: None,
        }
    }

    pub(crate) fn repair_due(&self) -> Option<Instant> {
        self.repair.due_at()
    }

    pub(crate) fn release(&mut self) {
        self.legs.clear();
    }

    pub(crate) fn bind(&mut self, defaults: DefaultEndpoints, now: Instant) -> Result<(), String> {
        self.legs.clear();
        self.bound = DefaultEndpoints::default();
        self.missing.clear();
        self.repair.clear();
        let mut failures = Vec::new();
        let mut missing = Vec::new();
        let mut opened = Vec::new();
        for (role, endpoint) in defaults.to_capture() {
            match self.devices.open(role, &endpoint, opened.len()) {
                Ok(leg) => opened.push((role, endpoint, leg)),
                Err(err) => {
                    failures.push(err);
                    missing.push((role, endpoint));
                }
            }
        }
        self.devices.reset_sources(opened.len());
        for (role, endpoint, leg) in opened {
            match self.devices.play(&leg) {
                Ok(()) => {
                    log::info!(
                        "[audio] system loopback bound to {role} endpoint {endpoint}: {}",
                        self.devices.describe(&leg)
                    );
                    self.legs.push(leg);
                }
                Err(err) => {
                    failures.push(format!("{role} endpoint {endpoint}: {err}"));
                    missing.push((role, endpoint));
                }
            }
        }
        for failure in &failures {
            log::warn!("[audio] system loopback: {failure}");
        }
        if self.legs.is_empty() {
            return Err(failures
                .pop()
                .unwrap_or_else(|| "no system audio output device available".to_string()));
        }
        self.bound = defaults;
        if !missing.is_empty() {
            log::warn!(
                "[audio] system loopback is running without {}; retrying it",
                describe_missing(&missing)
            );
            self.repair.schedule(now);
        }
        self.missing = missing;
        Ok(())
    }

    pub(crate) fn repair_missing(&mut self, now: Instant) {
        if self.missing.is_empty() {
            self.repair.clear();
            return;
        }
        let probe = self.missing.iter().try_for_each(|(role, endpoint)| {
            self.devices.open(role, endpoint, PROBE_SOURCE).map(drop)
        });
        match probe {
            Ok(()) => {
                log::info!(
                    "[audio] {} can be opened again; rebinding system loopback",
                    describe_missing(&self.missing)
                );
                let defaults = self.bound.clone();
                if let Err(err) = self.bind(defaults, now) {
                    self.report_stop(format!("could not rebind: {err}"));
                }
            }
            Err(err) => {
                let retry_in = self.repair.attempt_failed(now);
                log::warn!(
                    "[audio] system loopback still cannot open {}: {err}; retrying in {}s",
                    describe_missing(&self.missing),
                    retry_in.as_secs()
                );
            }
        }
    }

    pub(crate) fn rebind(&mut self, defaults: DefaultEndpoints, leg_failed: bool, now: Instant) {
        let same_endpoints = defaults == self.bound;
        if same_endpoints && !leg_failed {
            if !self.missing.is_empty() {
                self.repair_missing(now);
            }
            return;
        }
        if same_endpoints {
            let failed_again = self
                .last_failure_rebind
                .is_some_and(|at| now.saturating_duration_since(at) < REPEATED_FAILURE_WINDOW);
            if failed_again {
                self.report_stop(format!("the output endpoint keeps failing ({defaults})"));
                return;
            }
            self.last_failure_rebind = Some(now);
            log::info!("[audio] system loopback stream failed; rebinding to {defaults}");
        } else {
            log::info!("[audio] default output changed; rebinding system loopback to {defaults}");
        }
        if let Err(err) = self.bind(defaults, now) {
            self.report_stop(format!("could not rebind: {err}"));
        }
    }

    fn report_stop(&mut self, reason: String) {
        log::warn!("[audio] system loopback stopped: {reason}");
        self.legs.clear();
        self.bound = DefaultEndpoints::default();
        self.missing.clear();
        self.repair.clear();
        if let Ok(mut stop_error) = self.stop_error.lock() {
            stop_error.get_or_insert(reason);
        }
    }
}

fn describe_missing(missing: &[(&'static str, Endpoint)]) -> String {
    missing
        .iter()
        .map(|(role, endpoint)| format!("the {role} endpoint {endpoint}"))
        .collect::<Vec<_>>()
        .join(" and ")
}

#[cfg(test)]
mod tests;
