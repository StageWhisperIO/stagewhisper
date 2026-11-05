use std::time::{Duration, Instant};

use crate::aec::EchoPathPrior;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OutputRoute {
    InternalSpeakers,
    Headphones,
    Bluetooth,
    Usb,
    ExternalDisplay,
    AirPlay,
    Other,
}

impl OutputRoute {
    pub fn echo_path_prior(self) -> EchoPathPrior {
        match self {
            OutputRoute::InternalSpeakers | OutputRoute::ExternalDisplay | OutputRoute::AirPlay => {
                EchoPathPrior::EchoProne
            }
            OutputRoute::Headphones => EchoPathPrior::Clear,
            OutputRoute::Bluetooth | OutputRoute::Usb | OutputRoute::Other => {
                EchoPathPrior::Unknown
            }
        }
    }
}

const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

const TRANSPORT_BUILT_IN: u32 = fourcc(b"bltn");
const TRANSPORT_BLUETOOTH: u32 = fourcc(b"blue");
const TRANSPORT_BLUETOOTH_LE: u32 = fourcc(b"blea");
const TRANSPORT_USB: u32 = fourcc(b"usb ");
const TRANSPORT_HDMI: u32 = fourcc(b"hdmi");
const TRANSPORT_DISPLAY_PORT: u32 = fourcc(b"dprt");
const TRANSPORT_THUNDERBOLT: u32 = fourcc(b"thun");
const TRANSPORT_AIRPLAY: u32 = fourcc(b"airp");
const DATA_SOURCE_HEADPHONES: u32 = fourcc(b"hdpn");

fn route_from_transport(transport: u32, data_source: Option<u32>) -> OutputRoute {
    match transport {
        TRANSPORT_BUILT_IN => match data_source {
            Some(DATA_SOURCE_HEADPHONES) => OutputRoute::Headphones,
            _ => OutputRoute::InternalSpeakers,
        },
        TRANSPORT_BLUETOOTH | TRANSPORT_BLUETOOTH_LE => OutputRoute::Bluetooth,
        TRANSPORT_USB => OutputRoute::Usb,
        TRANSPORT_HDMI | TRANSPORT_DISPLAY_PORT | TRANSPORT_THUNDERBOLT => {
            OutputRoute::ExternalDisplay
        }
        TRANSPORT_AIRPLAY => OutputRoute::AirPlay,
        _ => OutputRoute::Other,
    }
}

#[cfg(target_os = "macos")]
mod sys {
    use super::*;
    use coreaudio_sys::{AudioObjectGetPropertyData, AudioObjectPropertyAddress};

    const SYSTEM_OBJECT: u32 = 1;
    const SELECTOR_DEFAULT_OUTPUT: u32 = fourcc(b"dOut");
    const SELECTOR_TRANSPORT: u32 = fourcc(b"tran");
    const SELECTOR_DATA_SOURCE: u32 = fourcc(b"ssrc");
    const SCOPE_GLOBAL: u32 = fourcc(b"glob");
    const SCOPE_OUTPUT: u32 = fourcc(b"outp");

    fn get_u32(object: u32, selector: u32, scope: u32) -> Option<u32> {
        let address = AudioObjectPropertyAddress {
            mSelector: selector,
            mScope: scope,
            mElement: 0,
        };
        let mut value: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                &mut value as *mut u32 as *mut std::ffi::c_void,
            )
        };
        (status == 0 && size == std::mem::size_of::<u32>() as u32).then_some(value)
    }

    pub fn default_output_route() -> OutputRoute {
        let Some(device) = get_u32(SYSTEM_OBJECT, SELECTOR_DEFAULT_OUTPUT, SCOPE_GLOBAL) else {
            return OutputRoute::Other;
        };
        if device == 0 {
            return OutputRoute::Other;
        }
        let Some(transport) = get_u32(device, SELECTOR_TRANSPORT, SCOPE_GLOBAL) else {
            return OutputRoute::Other;
        };
        let data_source = get_u32(device, SELECTOR_DATA_SOURCE, SCOPE_OUTPUT);
        route_from_transport(transport, data_source)
    }
}

#[cfg(target_os = "macos")]
pub fn default_output_route() -> OutputRoute {
    sys::default_output_route()
}

#[cfg(not(target_os = "macos"))]
pub fn default_output_route() -> OutputRoute {
    OutputRoute::Other
}

const ROUTE_POLL_INTERVAL: Duration = Duration::from_secs(1);

pub struct RouteWatcher {
    current: OutputRoute,
    last_check: Instant,
}

impl RouteWatcher {
    pub fn start() -> Self {
        Self {
            current: default_output_route(),
            last_check: Instant::now(),
        }
    }

    pub fn current(&self) -> OutputRoute {
        self.current
    }

    pub fn poll_change(&mut self) -> Option<OutputRoute> {
        self.poll_change_with(default_output_route)
    }

    fn poll_change_with(
        &mut self,
        read_route: impl FnOnce() -> OutputRoute,
    ) -> Option<OutputRoute> {
        if self.last_check.elapsed() < ROUTE_POLL_INTERVAL {
            return None;
        }
        self.last_check = Instant::now();
        let route = read_route();
        if route == self.current {
            return None;
        }
        self.current = route;
        Some(route)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watcher_last_checked(current: OutputRoute, ago: Duration) -> RouteWatcher {
        RouteWatcher {
            current,
            last_check: Instant::now() - ago,
        }
    }

    #[test]
    fn a_route_change_is_reported_once_the_poll_interval_has_passed() {
        let mut watcher = watcher_last_checked(OutputRoute::Headphones, ROUTE_POLL_INTERVAL);
        let change = watcher.poll_change_with(|| OutputRoute::InternalSpeakers);
        assert_eq!(change, Some(OutputRoute::InternalSpeakers));
        assert_eq!(watcher.current(), OutputRoute::InternalSpeakers);
    }

    #[test]
    fn the_route_is_not_read_again_before_the_poll_interval_has_passed() {
        let mut watcher = watcher_last_checked(OutputRoute::Headphones, Duration::ZERO);
        let change = watcher.poll_change_with(|| panic!("route read too early"));
        assert_eq!(change, None);
    }

    #[test]
    fn an_unchanged_route_reports_nothing() {
        let mut watcher = watcher_last_checked(OutputRoute::Headphones, ROUTE_POLL_INTERVAL);
        assert_eq!(watcher.poll_change_with(|| OutputRoute::Headphones), None);
    }

    #[test]
    fn builtin_speaker_is_echo_prone() {
        let route = route_from_transport(TRANSPORT_BUILT_IN, Some(fourcc(b"ispk")));
        assert_eq!(route, OutputRoute::InternalSpeakers);
        assert_eq!(route.echo_path_prior(), EchoPathPrior::EchoProne);
    }

    #[test]
    fn builtin_headphone_jack_is_clear() {
        let route = route_from_transport(TRANSPORT_BUILT_IN, Some(DATA_SOURCE_HEADPHONES));
        assert_eq!(route, OutputRoute::Headphones);
        assert_eq!(route.echo_path_prior(), EchoPathPrior::Clear);
    }

    #[test]
    fn bluetooth_is_ambiguous() {
        for transport in [TRANSPORT_BLUETOOTH, TRANSPORT_BLUETOOTH_LE] {
            let route = route_from_transport(transport, None);
            assert_eq!(route, OutputRoute::Bluetooth);
            assert_eq!(route.echo_path_prior(), EchoPathPrior::Unknown);
        }
    }

    #[test]
    fn display_and_airplay_are_echo_prone() {
        for transport in [TRANSPORT_HDMI, TRANSPORT_DISPLAY_PORT, TRANSPORT_AIRPLAY] {
            assert_eq!(
                route_from_transport(transport, None).echo_path_prior(),
                EchoPathPrior::EchoProne
            );
        }
    }

    #[test]
    fn unknown_transport_falls_back_to_unknown_prior() {
        let route = route_from_transport(fourcc(b"zzzz"), None);
        assert_eq!(route, OutputRoute::Other);
        assert_eq!(route.echo_path_prior(), EchoPathPrior::Unknown);
    }
}
