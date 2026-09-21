//! EP0 vendor request: the firmware, hardware and serial strings, which a host reads before it
//! exchanges a record. The firmware revision is available here and nowhere else on the link.

use embassy_usb::control::{InResponse, Request, RequestType};
use embassy_usb::types::InterfaceNumber;
use embassy_usb::Handler;

use crate::link::identity;

/// The vendor `bRequest`. `0x01` is taken by the MS OS 2.0 descriptor request the same device
/// answers.
pub(crate) const GET_DEVICE_INFO: u8 = 0x20;

/// Three strings of at most 48 bytes, each behind a length byte.
pub(crate) const MAX_DEVICE_INFO: usize = 3 * (1 + 48);

/// Answers the device-info request. It is a device-level handler because embassy routes every
/// request the standard stack does not handle there; the interface filter below makes it an
/// interface request.
pub(crate) struct DeviceInfoHandler {
    pub(crate) interface: InterfaceNumber,
}

impl Handler for DeviceInfoHandler {
    fn control_in<'a>(&'a mut self, req: Request, buf: &'a mut [u8]) -> Option<InResponse<'a>> {
        if req.request_type != RequestType::Vendor
            || req.request != GET_DEVICE_INFO
            || req.index != u16::from(u8::from(self.interface))
        {
            // `None` lets the stack keep asking the other handlers.
            return None;
        }
        let len = encode(buf);
        Some(InResponse::Accepted(&buf[..len]))
    }
}

/// `len u8 · UTF-8`, three times: firmware, hardware, serial. Each field is clamped to 48 bytes.
fn encode(out: &mut [u8]) -> usize {
    let firmware = identity::firmware_revision();
    let serial = identity::serial_string();
    let mut at = 0;
    for s in [firmware.as_str(), identity::HARDWARE_REVISION, serial.as_str()] {
        if at >= out.len() {
            break;
        }
        let n = s.len().min(out.len() - at - 1).min(48);
        out[at] = n as u8;
        at += 1;
        out[at..at + n].copy_from_slice(&s.as_bytes()[..n]);
        at += n;
    }
    at
}
