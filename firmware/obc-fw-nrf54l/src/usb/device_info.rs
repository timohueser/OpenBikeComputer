//! **§5.2.1's EP0 vendor request**: the device information a host reads before it exchanges a
//! record.
//!
//! ## Why this is on EP0 and not on the wire
//!
//! §5.2 gives the device's one control bulk endpoint pair wholly to `FLAT_Store_Protocol.md` §3, and
//! §3.2 forbids a generic forwarding path, so the non-object control surface has to live somewhere
//! else. On BLE it already does: a set of GATT characteristics, separately addressed by the
//! transport and outside the store spec's scope. EP0 is USB's equivalent of exactly that — the place
//! every USB device's identity already lives, below the record framing, readable the moment the
//! interface is claimed. §5.2 also already names enumeration as this link's authorization boundary,
//! so nothing about the trust decision moves.
//!
//! One request, and there is deliberately no second. The protocol major is settled by descriptor
//! matching (`bInterfaceProtocol = 4`, `bcdDevice = 0x0400`) before a record moves, and the store's
//! identity is `LIST`'s `StoreId` and commit sequence — which every client reads first anyway. A
//! third answer to a question the wire answers twice is the duplication the major bump removed.
//!
//! ## The one thing a host cannot get anywhere else
//!
//! The **firmware revision**, which is what "an update is available" compares against. The running
//! image's version lives here and nowhere else, and that is why this request survived the cut when
//! the config blob, the command envelope and the card-space read did not.

use embassy_usb::control::{InResponse, Request, RequestType};
use embassy_usb::types::InterfaceNumber;
use embassy_usb::Handler;

use crate::link::identity;

/// §5.2.1's `bRequest`.
///
/// `0x01` is taken by the device-level MS OS 2.0 descriptor request this same device answers for
/// Windows, and the recipient below already separates the two — but a distinct number costs nothing
/// and removes the need to reason about that separation every time either is edited.
pub(crate) const GET_DEVICE_INFO: u8 = 0x20;

/// The largest payload §5.2.1 permits: three strings of at most 48 bytes, each behind a length byte.
pub(crate) const MAX_DEVICE_INFO: usize = 3 * (1 + 48);

/// Answers §5.2.1 and nothing else.
///
/// Registered as a device-level handler because that is where embassy routes every request the
/// standard stack does not handle itself; the recipient and interface filter below is what makes it
/// an *interface* request in the sense §5.2.1 specifies.
pub(crate) struct DeviceInfoHandler {
    /// The interface §5.2.1 says `wIndex` names. Captured at bring-up rather than assumed, since
    /// embassy assigns it.
    pub(crate) interface: InterfaceNumber,
}

impl Handler for DeviceInfoHandler {
    fn control_in<'a>(&'a mut self, req: Request, buf: &'a mut [u8]) -> Option<InResponse<'a>> {
        if req.request_type != RequestType::Vendor
            || req.request != GET_DEVICE_INFO
            || req.index != u16::from(u8::from(self.interface))
        {
            // Not ours. `None` lets the stack keep asking other handlers, which is the contract.
            return None;
        }
        let len = encode(buf);
        Some(InResponse::Accepted(&buf[..len]))
    }
}

/// `len u8 · UTF-8`, three times, firmware · hardware · serial (§5.2.1).
///
/// Every source is a fixed-capacity string well under 48 bytes by construction; the clamp is what
/// keeps a revision string that ever grows from truncating mid-field rather than at the end.
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
