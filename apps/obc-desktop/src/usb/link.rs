//! One claimed device: endpoint discovery, and the two byte pipes over it.
//!
//! Everything here moves **bytes**. Nothing in this file knows what a transfer descriptor is, what
//! a status envelope means, or which object an upload belongs to — that is
//! `packer/web_builder/frontend/src/lib/usb/`, once, for both tiers (see [`super`]).

use std::sync::Arc;
use std::time::Duration;

use nusb::transfer::{Buffer, Bulk, Completion, In, Out, TransferError};
use nusb::{Device, DeviceInfo, Endpoint, Interface};
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Mutex};

/// USB vendor-specific interface class — the class the device's one interface declares
/// (`firmware/obc-fw-nrf54l/src/usb/mod.rs`), and the class a WebUSB-reachable interface must use.
const VENDOR_CLASS: u8 = 0xff;

/// How much of a bulk IN stream one transfer asks for.
///
/// **This is the read size the terminating ZLP bought us.** A USB IN transfer ends when the request
/// is filled *or* a short packet arrives, so asking for more than one packet from a device that
/// stops on an exact packet boundary would wait forever — which is why C3's WebUSB pipe reads
/// exactly one max packet. #889 closed that hole from the device side: `run_download` and `run_echo`
/// send an explicit zero-length packet when an object is an exact multiple of the max packet, so a
/// larger request always terminates. 16 KB is 32 packets, which cuts the URB count per megabyte
/// from ~2000 to ~64 without making a mid-object pause any more visible (the device streams
/// continuously; a pause simply lets the transfer keep accumulating).
///
/// Rounded down to a whole number of packets at construction: nusb rejects an IN request that is
/// not a multiple of the endpoint's max packet size.
const BULK_READ_TARGET: usize = 16 * 1024;

/// Which of the two planes a call means. The names match `lib/usb/pipe.ts`'s `DeviceLink`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Plane {
    Control,
    Bulk,
}

/// Which half of an endpoint pair. Cancellation is per-direction: an abort on a download must not
/// also tear down the write the caller is about to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    In,
    Out,
}

/// A transport failure, in the vocabulary `lib/usb/pipe.ts`'s `PipeError` already speaks.
///
/// The `code` is the contract — the TS side switches on it — and the `message` is for a human.
/// `unsupported` never appears here: it means "this browser has no WebUSB", which is exactly the
/// condition the desktop app exists to not have.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PipeFault {
    /// `closed` | `aborted` | `device-error`.
    pub code: &'static str,
    pub message: String,
}

impl PipeFault {
    pub fn closed(message: impl Into<String>) -> Self {
        Self { code: "closed", message: message.into() }
    }

    pub fn aborted(message: impl Into<String>) -> Self {
        Self { code: "aborted", message: message.into() }
    }

    pub fn device(message: impl Into<String>) -> Self {
        Self { code: "device-error", message: message.into() }
    }

    /// Map a nusb transfer failure onto the pipe vocabulary.
    ///
    /// `Disconnected` is `closed` rather than an error: a pulled cable is the ordinary end of a
    /// link, and the UI's sentence for it is "plug it back in", not "something went wrong".
    pub(super) fn from_transfer(what: &str, error: TransferError) -> Self {
        match error {
            TransferError::Cancelled => Self::aborted(format!("The {what} was cancelled.")),
            TransferError::Disconnected => Self::closed("The device was disconnected."),
            other => Self::device(format!("The {what} failed: {other}.")),
        }
    }
}

impl std::fmt::Display for PipeFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

// ============================ Endpoint discovery ============================

/// One endpoint, reduced to the three facts the layout rule uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointFacts {
    /// The full address, direction bit included (`0x81` is IN #1).
    pub address: u8,
    pub is_in: bool,
    pub max_packet: usize,
}

/// Where the two pipes live on the device's vendor interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointLayout {
    pub interface: u8,
    pub control: (u8, u8, usize),
    pub bulk: (u8, u8, usize),
}

/// Split a vendor interface's endpoints into a control pair and a bulk pair.
///
/// **Allocation order is the wire contract.** The firmware allocates
/// control-IN, control-OUT, bulk-IN, bulk-OUT in that order and says so
/// (`firmware/obc-fw-nrf54l/src/usb/mod.rs`), and both hosts read them back the same mechanical
/// way: lowest-numbered IN/OUT pair is control, the next is bulk. This is deliberately the *same*
/// rule as `lib/usb/webusb.ts::discoverLayout` — descriptor topology, not protocol, which is why it
/// is the one thing this backend re-derives rather than borrowing from the TS client.
pub fn split_layout(interface: u8, endpoints: &[EndpointFacts]) -> Result<EndpointLayout, PipeFault> {
    let mut ins: Vec<EndpointFacts> = endpoints.iter().copied().filter(|e| e.is_in).collect();
    let mut outs: Vec<EndpointFacts> = endpoints.iter().copied().filter(|e| !e.is_in).collect();
    ins.sort_by_key(|e| e.address);
    outs.sort_by_key(|e| e.address);
    if ins.len() < 2 || outs.len() < 2 {
        return Err(PipeFault::device(format!(
            "The device's interface exposes {} IN and {} OUT endpoints; two of each are needed \
             (a control pair and a bulk pair).",
            ins.len(),
            outs.len()
        )));
    }
    Ok(EndpointLayout {
        interface,
        control: (ins[0].address, outs[0].address, ins[0].max_packet),
        bulk: (ins[1].address, outs[1].address, ins[1].max_packet),
    })
}

// ============================ The pipes ============================

/// One endpoint pair as a byte pipe: `read`, `write`, `reset`, and a cancel that reaches the
/// transport.
///
/// ## Why cancellation is a first-class thing here and not on the web
///
/// WebUSB cannot cancel a submitted `transferIn` at all, so C3's pipe releases the *caller* and
/// leaves the transfer to settle into nothing. That works there because a browser tab can afford an
/// orphan. Here it would wedge: an orphaned read holds the endpoint's `&mut`, the next read queues
/// behind it, and after an abort the device stops sending by design — so the orphan never completes
/// and the pipe is dead while looking alive. nusb *can* cancel, so this pipe does.
///
/// The mechanism is a `watch` channel rather than a `Notify` because the race matters: a cancel
/// that arrives between "the caller decided to abort" and "the read parked itself" must not be
/// lost. A monotonic epoch the reader snapshots before it parks cannot miss one.
pub struct Pipe {
    pub max_packet: usize,
    read_len: usize,
    ep_in: Mutex<Endpoint<Bulk, In>>,
    ep_out: Mutex<Endpoint<Bulk, Out>>,
    cancel_in: watch::Sender<u64>,
    cancel_out: watch::Sender<u64>,
}

impl Pipe {
    fn new(interface: &Interface, in_addr: u8, out_addr: u8, target_read: usize) -> Result<Self, PipeFault> {
        let ep_in = interface
            .endpoint::<Bulk, In>(in_addr)
            .map_err(|e| PipeFault::device(format!("endpoint {in_addr:#04x} could not be opened: {e}")))?;
        let ep_out = interface
            .endpoint::<Bulk, Out>(out_addr)
            .map_err(|e| PipeFault::device(format!("endpoint {out_addr:#04x} could not be opened: {e}")))?;
        let max_packet = ep_in.max_packet_size().max(1);
        // nusb rejects an IN request that is not a nonzero multiple of the max packet size, and the
        // OS would fail the URB anyway. Round down, floor at one packet.
        let read_len = (target_read / max_packet).max(1) * max_packet;
        Ok(Self {
            max_packet,
            read_len,
            ep_in: Mutex::new(ep_in),
            ep_out: Mutex::new(ep_out),
            cancel_in: watch::Sender::new(0),
            cancel_out: watch::Sender::new(0),
        })
    }

    /// Wait for the next bytes.
    ///
    /// Never resolves empty: a zero-length packet is a USB-level marker (the object terminator
    /// #889 added), not data, and a caller could not tell an empty array from a spurious wakeup —
    /// so it is absorbed and the read goes round again.
    pub async fn read(&self) -> Result<Vec<u8>, PipeFault> {
        let mut ep = self.ep_in.lock().await;
        let mut cancel = self.cancel_in.subscribe();
        cancel.borrow_and_update();
        loop {
            let buf = Buffer::new(self.read_len);
            ep.submit(buf);
            let mut done: Option<Completion> = None;
            tokio::select! {
                // Biased so a transfer that completed in the same wakeup as a cancel still delivers
                // its bytes — dropping data we already have would be a needless retransmit.
                biased;
                completion = ep.next_complete() => done = Some(completion),
                _ = cancel.changed() => {}
            }
            let Some(completion) = done else {
                cancel_and_drain(&mut ep).await;
                return Err(PipeFault::aborted("The read was cancelled."));
            };
            completion.status.map_err(|e| PipeFault::from_transfer("read", e))?;
            if !completion.buffer.is_empty() {
                return Ok(completion.buffer.to_vec());
            }
        }
    }

    /// Hand `bytes` to the endpoint, resolving only once the device has taken them.
    ///
    /// That resolution *is* the backpressure the `BytePipe` contract promises: the device NAKs an
    /// endpoint it has not drained, so a writer that awaits every call cannot outrun an SD card
    /// that tops out in the high hundreds of KB/s.
    pub async fn write(&self, bytes: &[u8]) -> Result<(), PipeFault> {
        // An empty write would put a zero-length packet on the wire — a terminator, not data. No
        // caller means one, and sending one could close a stream the device is still counting.
        if bytes.is_empty() {
            return Ok(());
        }
        let mut ep = self.ep_out.lock().await;
        let mut cancel = self.cancel_out.subscribe();
        cancel.borrow_and_update();
        let mut buf = Buffer::new(bytes.len());
        buf.extend_from_slice(bytes);
        ep.submit(buf);
        let mut done: Option<Completion> = None;
        tokio::select! {
            biased;
            completion = ep.next_complete() => done = Some(completion),
            _ = cancel.changed() => {}
        }
        let Some(completion) = done else {
            cancel_and_drain(&mut ep).await;
            return Err(PipeFault::aborted("The write was cancelled."));
        };
        completion.status.map_err(|e| PipeFault::from_transfer("write", e))?;
        if completion.actual_len != bytes.len() {
            return Err(PipeFault::device(format!(
                "the device took {} of {} bytes.",
                completion.actual_len,
                bytes.len()
            )));
        }
        Ok(())
    }

    /// Discard everything in flight and clear both halves of the pair.
    ///
    /// Interface spec §4.1: an exchange that does not reach its correlated close leaves the channel
    /// at an unknown offset and is not reusable. Over BLE the app closes and reopens the CoC; here
    /// it is cancel-everything plus `CLEAR_FEATURE(ENDPOINT_HALT)`, which is also what the firmware
    /// reads as "the host has given up on this exchange".
    ///
    /// The cancel comes first and the locks second, deliberately: a read parked on an endpoint the
    /// device has stopped feeding holds the `&mut` this needs, and bumping its epoch is what makes
    /// it let go.
    pub async fn reset(&self) -> Result<(), PipeFault> {
        self.cancel(None);
        {
            let mut ep = self.ep_in.lock().await;
            cancel_and_drain(&mut ep).await;
            // Best-effort: an endpoint that was never halted may refuse, and that is not a failure
            // of the reset.
            let _ = ep.clear_halt().await;
        }
        {
            let mut ep = self.ep_out.lock().await;
            cancel_and_drain(&mut ep).await;
            let _ = ep.clear_halt().await;
        }
        Ok(())
    }

    /// Release whatever is parked on this pipe. `None` means both directions.
    pub fn cancel(&self, dir: Option<Dir>) {
        if dir != Some(Dir::Out) {
            self.cancel_in.send_modify(|epoch| *epoch += 1);
        }
        if dir != Some(Dir::In) {
            self.cancel_out.send_modify(|epoch| *epoch += 1);
        }
    }

    /// The OUT endpoint and its cancel signal, for the file streamer ([`super::sendfile`]).
    ///
    /// Handing out the mutex rather than a per-transfer method is the point: a file send holds the
    /// endpoint for the whole object, which is what keeps several transfers in flight and is also
    /// exactly the §4.1 "one transfer at a time" rule the client already enforces above it.
    pub(super) fn out_for_streaming(&self) -> (&Mutex<Endpoint<Bulk, Out>>, watch::Receiver<u64>) {
        (&self.ep_out, self.cancel_out.subscribe())
    }
}

/// Cancel every pending transfer on an endpoint and consume the completions.
///
/// The drain is not tidiness: `clear_halt` must not run with transfers pending, and `next_complete`
/// panics when there are none — so both callers need the queue provably empty.
async fn cancel_and_drain<D: nusb::transfer::EndpointDirection>(ep: &mut Endpoint<Bulk, D>) {
    if ep.pending() == 0 {
        return;
    }
    ep.cancel_all();
    while ep.pending() > 0 {
        let _ = ep.next_complete().await;
    }
}

// ============================ One open device ============================

/// A device this app has opened and claimed: the interface, and the two pipes over it.
///
/// The `Interface` is held for its whole life because dropping it releases the claim — nothing
/// reads it directly.
pub struct OpenLink {
    /// The stable key the frontend knows this device by; a hot-plug `Disconnected` carries the same
    /// one, which is how a pulled cable reaches the pipes without waiting for a transfer to time
    /// out.
    pub device_id: String,
    pub control: Pipe,
    pub bulk: Pipe,
    _interface: Interface,
}

impl OpenLink {
    pub fn plane(&self, plane: Plane) -> &Pipe {
        match plane {
            Plane::Control => &self.control,
            Plane::Bulk => &self.bulk,
        }
    }

    /// Fail everything parked on both planes. Called on an unplug and on close.
    pub fn cancel_all(&self) {
        self.control.cancel(None);
        self.bulk.cancel(None);
    }
}

/// What `usb_open` tells the frontend about the link it just got.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedLink {
    pub handle: u32,
    pub device_id: String,
    pub interface_number: u8,
    pub control_packet_size: usize,
    pub bulk_packet_size: usize,
    pub product: Option<String>,
    pub serial_number: Option<String>,
}

/// Open, configure and claim a device, returning its two pipes.
pub async fn open(info: &DeviceInfo, device_id: String) -> Result<(Arc<OpenLink>, OpenedLink), PipeFault> {
    let device: Device = info
        .open()
        .await
        .map_err(|e| PipeFault::device(format!("The device could not be opened: {e}{}", permission_hint(&e))))?;

    // Only if the OS has not already configured it. Re-issuing SET_CONFIGURATION on a live device
    // resets its endpoints, which would tear down a link another window is using.
    if device.active_configuration().is_err() {
        device
            .set_configuration(1)
            .await
            .map_err(|e| PipeFault::device(format!("The device would not select its configuration: {e}")))?;
    }
    let configuration = device
        .active_configuration()
        .map_err(|e| PipeFault::device(format!("The device offers no usable configuration: {e}")))?;

    let vendor = configuration
        .interface_alt_settings()
        .find(|alt| alt.class() == VENDOR_CLASS)
        .ok_or_else(|| PipeFault::device("This device has no vendor-specific interface."))?;
    let facts: Vec<EndpointFacts> = vendor
        .endpoints()
        .filter(|e| e.transfer_type() == nusb::descriptors::TransferType::Bulk)
        .map(|e| EndpointFacts {
            address: e.address(),
            is_in: e.direction() == nusb::transfer::Direction::In,
            max_packet: e.max_packet_size(),
        })
        .collect();
    let layout = split_layout(vendor.interface_number(), &facts)?;

    let interface = device.claim_interface(layout.interface).await.map_err(|e| {
        PipeFault::device(format!("Interface {} could not be claimed: {e}{}", layout.interface, permission_hint(&e)))
    })?;

    // The control plane is message-oriented: one frame is exactly one transfer, so it reads exactly
    // one max packet and a short packet delimits the frame. The bulk plane is a stream, so it reads
    // as much as the ZLP contract allows.
    let control = Pipe::new(&interface, layout.control.0, layout.control.1, layout.control.2)?;
    let bulk = Pipe::new(&interface, layout.bulk.0, layout.bulk.1, BULK_READ_TARGET)?;

    let opened = OpenedLink {
        handle: 0, // filled in by the registry
        device_id: device_id.clone(),
        interface_number: layout.interface,
        control_packet_size: control.max_packet,
        bulk_packet_size: bulk.max_packet,
        product: info.product_string().map(str::to_owned),
        serial_number: info.serial_number().map(str::to_owned),
    };
    Ok((Arc::new(OpenLink { device_id, control, bulk, _interface: interface }), opened))
}

/// The two failures whose remedy is not "try again", spelled out where they happen.
///
/// On Linux the usbfs node is root-owned unless a udev rule says otherwise, and the error the user
/// would otherwise read is a bare `Permission denied` with nothing to suggest that a one-line file
/// fixes it (`apps/obc-desktop/linux/`). macOS needs nothing for a vendor interface; Windows
/// binds WinUSB from the MS OS 2.0 descriptors the firmware already ships (#889).
///
/// `Busy` is the other one, and it is not platform-specific: an interface may be claimed once, so a
/// second window — or a browser tab that already took the device over WebUSB — is a real and
/// entirely recoverable situation the message should name.
fn permission_hint(error: &nusb::Error) -> &'static str {
    match error.kind() {
        nusb::ErrorKind::PermissionDenied if cfg!(target_os = "linux") => {
            " — install the udev rule from the app's `linux/` folder, then unplug and re-plug the device."
        }
        nusb::ErrorKind::PermissionDenied => " — this account is not allowed to open the device.",
        nusb::ErrorKind::Busy => " — something else has it open. Close the other app or browser tab and try again.",
        _ => "",
    }
}

/// How long the send loop waits between progress reports. See [`super::sendfile`].
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(80);

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(address: u8, max_packet: usize) -> EndpointFacts {
        EndpointFacts { address, is_in: address & 0x80 != 0, max_packet }
    }

    #[test]
    fn the_lowest_pair_is_control_and_the_next_is_bulk() {
        // Exactly what firmware/obc-fw-nrf54l/src/usb/mod.rs allocates, in the order it allocates
        // it — and deliberately shuffled here, because the rule sorts rather than trusting order.
        let layout = split_layout(0, &[ep(0x02, 512), ep(0x81, 512), ep(0x01, 512), ep(0x82, 512)]).unwrap();
        assert_eq!(layout.control, (0x81, 0x01, 512));
        assert_eq!(layout.bulk, (0x82, 0x02, 512));
    }

    #[test]
    fn an_interface_with_one_pair_is_refused_rather_than_guessed_at() {
        let fault = split_layout(0, &[ep(0x81, 64), ep(0x01, 64)]).unwrap_err();
        assert_eq!(fault.code, "device-error");
        assert!(fault.message.contains("1 IN and 1 OUT"), "{}", fault.message);
    }

    #[test]
    fn a_read_request_is_a_whole_number_of_packets() {
        // The rounding `Pipe::new` does, stated as arithmetic: nusb rejects an IN request that is
        // not a nonzero multiple of the max packet size.
        for max_packet in [64usize, 512] {
            let read_len = (BULK_READ_TARGET / max_packet).max(1) * max_packet;
            assert_eq!(read_len % max_packet, 0);
            assert!(read_len > 0 && read_len <= BULK_READ_TARGET);
        }
        // A hypothetical endpoint larger than the target still gets one whole packet, never zero.
        let huge = BULK_READ_TARGET * 4;
        assert_eq!((BULK_READ_TARGET / huge).max(1) * huge, huge);
    }
}
