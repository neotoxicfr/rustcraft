//! System notifications of the audio endpoints (Windows): a device added, removed, enabled or
//! disabled, a new default device, a property changed (a rename) raise a flag that the
//! "audio-devices" thread reads at its next poll, instead of enumerating every device and reading
//! each name every second (`RUSTCRAFT_OPT_AUDIO_DEVICE_EVENTS`).
//!
//! The callbacks run on a system thread: they only store the flag (they must neither block nor
//! wait), the poll thread does the reading.

// The subscription goes through COM (endpoint enumerator, callback object): FFI only.
#![allow(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{PROPERTYKEY, RPC_E_CHANGED_MODE};
use windows::Win32::Media::Audio::{
    DEVICE_STATE, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl,
    MMDeviceEnumerator,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows_core::PCWSTR;

/// Callback object: every notification sets the flag.
#[windows_core::implement(IMMNotificationClient)]
struct Notifier {
    changed: Arc<AtomicBool>,
}

impl Notifier {
    fn raise(&self) -> windows_core::Result<()> {
        self.changed.store(true, Ordering::Release);
        Ok(())
    }
}

impl IMMNotificationClient_Impl for Notifier_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> windows_core::Result<()> {
        self.raise()
    }

    fn OnDeviceAdded(&self, _id: &PCWSTR) -> windows_core::Result<()> {
        self.raise()
    }

    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> windows_core::Result<()> {
        self.raise()
    }

    fn OnDefaultDeviceChanged(&self, _flow: EDataFlow, _role: ERole, _id: &PCWSTR) -> windows_core::Result<()> {
        self.raise()
    }

    /// Any property: the name is one of them, and a spurious read costs only the old poll.
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> windows_core::Result<()> {
        self.raise()
    }
}

/// Registered notifications, unregistered when dropped. The thread that subscribes stays in the
/// multithreaded apartment for its whole life (the poll thread lives as long as the client).
pub(crate) struct Subscription {
    registered: Option<(IMMDeviceEnumerator, IMMNotificationClient)>,
    changed: Arc<AtomicBool>,
}

impl Subscription {
    /// Subscribes from the calling thread, which joins the multithreaded apartment first (cpal,
    /// which asks for a single-threaded one afterwards on the same thread, accepts it).
    /// `None` if the system refuses: the caller falls back to probing.
    pub(crate) fn new() -> Option<Subscription> {
        // SAFETY: plain COM calls on this thread; the interfaces are kept in the returned value.
        unsafe {
            let init = CoInitializeEx(None, COINIT_MULTITHREADED);
            if init.is_err() && init != RPC_E_CHANGED_MODE {
                return None;
            }
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let changed = Arc::new(AtomicBool::new(false));
            let client: IMMNotificationClient = Notifier { changed: changed.clone() }.into();
            enumerator.RegisterEndpointNotificationCallback(&client).ok()?;
            Some(Subscription { registered: Some((enumerator, client)), changed })
        }
    }

    /// True if a notification arrived since the last call.
    pub(crate) fn take(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some((enumerator, client)) = self.registered.take() {
            // SAFETY: the objects of the registration, still alive.
            unsafe {
                let _ = enumerator.UnregisterEndpointNotificationCallback(&client);
            }
        }
    }
}
