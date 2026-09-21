//! Discovery and hot-plug: what makes plugging a cable in light the window up.
//!
//! Plugging in must light up the UI within about a second, and unplugging must not leave a stuck
//! spinner. The browser gets that from `navigator.usb`'s events; here it comes from the OS
//! notification streams nusb wraps, which are edge-driven rather than polled, so the latency budget
//! is spent on the device side.
//!
//! A native host has no chooser and no permission prompt, so it sees the device the moment it
//! appears. The session shape stays the same as the browser's anyway: `requestDevice()` here is
//! looking again, so the same button keeps working and no UI branches on the transport.

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
    /// Opaque and stable for as long as the device stays plugged in: the key `usb_open` takes and
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
    /// The OS notification stream ended or could not be started. Reported rather than swallowed: a
    /// watch that died silently looks like nothing ever being plugged in.
    WatchFailed {
        message: String,
    },
}

/// The frontend's handle on a device, stable for the life of the connection.
///
/// `DeviceId` is opaque and platform-specific in shape, so it is stringified rather than
/// interpreted. The only property anything depends on is that one physical connection produces the
/// same string in a `connected` event, in `usb_list` and in the matching `disconnected`.
pub fn device_key(id: nusb::DeviceId) -> String {
    format!("{id:?}")
}

/// Is this one of ours?
///
/// The pair is pid.codes' prototype and testing range, and the firmware declares the same one.
/// Allocating a real product id moves this constant, the firmware's `PRODUCT_ID` and the web
/// filters together.
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
/// A closure rather than a [`Channel`], because the channel is replaceable: a window that reloads
/// opens a new one, and the watch outlives the page.
pub type Emit = Arc<dyn Fn(UsbEvent) + Send + Sync>;

/// Follow hot-plug forever.
///
/// `on_disconnect` runs before the event reaches the frontend, and that ordering matters: it is
/// where the pipes of a link on the vanished device are failed, so an in-flight transfer's UI says
/// unplugged now instead of spinning until a timeout.
pub fn spawn(emit: Emit, on_disconnect: Arc<dyn Fn(&str) + Send + Sync>) {
    // Created before the caller lists devices: a device attached in the window between listing
    // and watching would otherwise be missed.
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
                    // Not filtered by vendor and product id: a disconnect carries only an id, and
                    // an id with no link is a no-op on both sides. Filtering here would mean
                    // keeping a device table only to discard events that are already ignored.
                    let key = device_key(id);
                    on_disconnect(&key);
                    emit(UsbEvent::Disconnected { id: key });
                }
            }
        }
    });
}
