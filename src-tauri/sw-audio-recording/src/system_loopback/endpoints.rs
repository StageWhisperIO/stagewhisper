use std::ffi::c_void;

use crossbeam_channel::Sender;
use windows::core::{implement, Result as WinResult, PCWSTR, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::{
    eCommunications, eConsole, eRender, EDataFlow, ERole, IMMDevice, IMMDeviceEnumerator,
    IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator, DEVICE_STATE,
};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL, STGM_READ};

use super::Control;
use crate::loopback_binder::{DefaultEndpoints, Endpoint};

#[implement(IMMNotificationClient)]
struct EndpointNotifier {
    control: Sender<Control>,
}

impl IMMNotificationClient_Impl for EndpointNotifier_Impl {
    fn OnDeviceStateChanged(&self, _device_id: &PCWSTR, _new_state: DEVICE_STATE) -> WinResult<()> {
        let _ = self.control.send(Control::EndpointsChanged);
        Ok(())
    }

    fn OnDeviceAdded(&self, _device_id: &PCWSTR) -> WinResult<()> {
        Ok(())
    }

    fn OnDeviceRemoved(&self, _device_id: &PCWSTR) -> WinResult<()> {
        let _ = self.control.send(Control::EndpointsChanged);
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        _default_device_id: &PCWSTR,
    ) -> WinResult<()> {
        if flow == eRender && (role == eConsole || role == eCommunications) {
            let _ = self.control.send(Control::EndpointsChanged);
        }
        Ok(())
    }

    fn OnPropertyValueChanged(&self, _device_id: &PCWSTR, _key: &PROPERTYKEY) -> WinResult<()> {
        Ok(())
    }
}

pub(super) struct EndpointWatch {
    enumerator: IMMDeviceEnumerator,
    notifier: IMMNotificationClient,
}

impl EndpointWatch {
    pub(super) fn register(control: Sender<Control>) -> WinResult<Self> {
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }?;
        let notifier: IMMNotificationClient = EndpointNotifier { control }.into();
        unsafe { enumerator.RegisterEndpointNotificationCallback(&notifier) }?;
        Ok(Self {
            enumerator,
            notifier,
        })
    }

    pub(super) fn defaults(&self) -> DefaultEndpoints {
        DefaultEndpoints {
            console: self.default_endpoint(eConsole),
            communications: self.default_endpoint(eCommunications),
        }
    }

    fn default_endpoint(&self, role: ERole) -> Option<Endpoint> {
        let device = unsafe { self.enumerator.GetDefaultAudioEndpoint(eRender, role) }.ok()?;
        let id = unsafe { device.GetId() }.ok().map(take_string)?;
        let name = friendly_name(&device).unwrap_or_else(|| "unnamed".to_string());
        Some(Endpoint { id, name })
    }
}

impl Drop for EndpointWatch {
    fn drop(&mut self) {
        let _ = unsafe {
            self.enumerator
                .UnregisterEndpointNotificationCallback(&self.notifier)
        };
    }
}

fn take_string(value: PWSTR) -> String {
    let text = unsafe { value.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(value.0 as *const c_void)) };
    text
}

fn friendly_name(device: &IMMDevice) -> Option<String> {
    let store = unsafe { device.OpenPropertyStore(STGM_READ) }.ok()?;
    let mut value = unsafe { store.GetValue(&PKEY_Device_FriendlyName) }.ok()?;
    let text = unsafe { PropVariantToStringAlloc(&value) };
    let _ = unsafe { PropVariantClear(&mut value) };
    text.ok().map(take_string)
}
