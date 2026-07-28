//! Discovery and hot-plug: what makes plugging a cable in light the window up.
//!
//! The UX contract is C3's, not a new one (#902): *plugging in lights up the UI within about a
//! second, and unplugging is handled without a stuck spinner*. The browser gets that from
//! `navigator.usb`'s `connect` / `disconnect` events; here it comes from the OS notification
//! streams nusb wraps (netlink on Linux, IOKit on macOS, `WM_DEVICECHANGE` on Windows), which are
//! edge-driven rather than polled — so the latency budget is spent almost entirely on the device
//! side, where the firmware's VBUS gate re-reads at 2 Hz before it asserts its pull-up (#934).
//!
//! ## The one thing the desktop app does *not* inherit
//!
//! WebUSB's chooser may only open from a user gesture, which is why C3's session exists before any
//! device is known and why the hosted site must draw a Connect button it can never retire. A native
//! host has no chooser and no permission prompt: it can see the device the moment it appears. The
//! session shape stays identical anyway — `requestDevice()` here is simply "look again now", so the
//! same button keeps working and no UI has to branch on which transport it got.

use std::sync::Arc;

use futures_core::Stream;
use nusb::hotplug::HotplugEvent;
use nusb::DeviceInfo;
use serde::Serialize;

use super::{PRODUCT_ID, VENDOR_ID};

/// A device the app is willing to talk to, as the frontend sees it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSummary {
    /// Opaque and stable for as long as the device stays plugged in — the key `usb_open` takes and
    /// the key a `disconnected` event carries. Never parsed by the frontend.
    pub id: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub product: Option<String>,
    /// The nRF `FICR.DEVICEID`, which is what tells two boards apart.
    pub serial_number: Option<String>,
}

/// What the watch task tells the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum UsbEvent {
    Connected {
        device: DeviceSummary,
    },
    Disconnected {
        id: String,
    },
    /// The OS notification stream itself ended or could not be started. Reported rather than
    /// swallowed: a watch that silently died looks exactly like "nothing is ever plugged in".
    WatchFailed {
        message: String,
    },
}

/// The frontend's handle on a device, stable for the life of the connection.
///
/// `DeviceId` is opaque by design and platform-specific in shape, so it is stringified rather than
/// interpreted — the only property anything depends on is that the same physical connection
/// produces the same string in a `connected` event, in `usb_list`, and in the matching
/// `disconnected`.
pub fn device_key(id: nusb::DeviceId) -> String {
    format!("{id:?}")
}

/// Is this one of ours?
///
/// `1209:0001` is pid.codes' prototype/testing pair, and the firmware declares the same one
/// (`firmware/obc-fw-nrf54l/src/usb/mod.rs`). Allocating a real product id is an owner action; when
/// it happens, this constant, the firmware's `PRODUCT_ID` and `OBC_USB_FILTERS` in
/// `lib/usb/webusb.ts` move together.
pub fn matches(info: &DeviceInfo) -> bool {
    info.vendor_id() == VENDOR_ID && info.product_id() == PRODUCT_ID
}

pub fn summarize(info: &DeviceInfo) -> DeviceSummary {
    DeviceSummary {
        id: device_key(info.id()),
        vendor_id: info.vendor_id(),
        product_id: info.product_id(),
        product: info.product_string().map(str::to_owned),
        serial_number: info.serial_number().map(str::to_owned),
    }
}

/// Every matching device attached right now.
pub async fn list() -> Result<Vec<DeviceSummary>, String> {
    let devices = nusb::list_devices().await.map_err(|e| format!("USB devices could not be listed: {e}"))?;
    Ok(devices.filter(matches).map(|info| summarize(&info)).collect())
}

/// What the watch task hands its events to.
///
/// A closure rather than a [`Channel`] because the channel is *replaceable*: a window that reloads
/// opens a new one, and the watch — which outlives the page — has to end up talking to the current
/// one. The indirection lives in [`super::UsbState`], where the sink is stored.
pub type Emit = Arc<dyn Fn(UsbEvent) + Send + Sync>;

/// Follow hot-plug forever.
///
/// `on_disconnect` runs **before** the event reaches the frontend, and that ordering is the whole
/// point: it is where the pipes of a link on the vanished device are failed, so an in-flight
/// transfer's UI says "unplugged" now instead of spinning until a timeout. C3's WebUSB watcher does
/// exactly the same thing for the same reason.
pub fn spawn(emit: Emit, on_disconnect: Arc<dyn Fn(&str) + Send + Sync>) {
    // Created *before* the caller lists devices (nusb's own advice): a device attached in the
    // window between listing and watching would otherwise be missed entirely.
    let mut watch = match nusb::watch_devices() {
        Ok(watch) => watch,
        Err(e) => {
            emit(UsbEvent::WatchFailed { message: format!("USB hot-plug is unavailable: {e}") });
            return;
        }
    };
    tauri::async_runtime::spawn(async move {
        loop {
            let event = std::future::poll_fn(|cx| std::pin::Pin::new(&mut watch).poll_next(cx)).await;
            let Some(event) = event else {
                emit(UsbEvent::WatchFailed { message: "The system stopped reporting USB device changes.".into() });
                return;
            };
            match event {
                HotplugEvent::Connected(info) if matches(&info) => {
                    emit(UsbEvent::Connected { device: summarize(&info) });
                }
                HotplugEvent::Connected(_) => {}
                HotplugEvent::Disconnected(id) => {
                    // Not filtered by VID/PID: a `Disconnected` carries only an id, and an id we do
                    // not hold a link for is a no-op on both sides anyway. Filtering here would
                    // mean keeping a device table purely to discard events we already ignore.
                    let key = device_key(id);
                    on_disconnect(&key);
                    emit(UsbEvent::Disconnected { id: key });
                }
            }
        }
    });
}
