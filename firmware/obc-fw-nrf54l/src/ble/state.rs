//! Shared BLE state: the link-status snapshot the UI reads, the BAS battery cell, and the
//! one-transfer-at-a-time arming channel the control plane ([`super::control`]) and the CoC data
//! plane ([`super::data_plane`]) coordinate through. Everything here is `pub(crate)` at most —
//! it lives entirely within the `ble` module tree, read/written across the four planes but never
//! wider.

use core::cell::Cell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::signal::Signal;
use obc_ble::{Receiver, StatusMessage, TransferControl, TransferResult, TransferStatus};

// ============================ Link status → the status UI ============================

/// The link state the status UI shows.
#[derive(Clone, Copy, PartialEq, Eq, defmt::Format)]
pub(crate) enum LinkState {
    /// Stack still coming up (the boot instant, before the first advertise).
    Init,
    /// Advertising, connectable — the powered-and-unconnected steady state.
    Advertising,
    /// A central holds the (single) link.
    Connected,
}

/// One coherent snapshot of the link for the status UI — published by the BLE plumbing below,
/// drained by `run_status` in `main.rs`. `Copy` so it crosses the mutex as a value.
#[derive(Clone, Copy)]
pub(crate) struct Status {
    pub state: LinkState,
    /// The connected central's address (little-endian, as the wire carries it), while connected.
    pub peer: Option<[u8; 6]>,
    /// The live connection interval (ms), once the central negotiated one; 0 = not reported yet.
    pub conn_interval_ms: u32,
    /// The negotiated ATT MTU (target 247); 0 = not exchanged yet.
    pub att_mtu: u16,
    /// True once the link runs on the 2M PHY.
    pub phy_2m: bool,
    /// Lifetime counters — the soak's at-a-glance health line.
    pub connects: u32,
    pub disconnects: u32,
    /// The HCI reason (status) code of the most recent disconnect; 0 = none yet. Logged in full
    /// (named) over RTT on each disconnect — this is the at-a-glance byte for the status screen.
    pub last_disconnect_reason: u8,
    /// The 6-digit LESC passkey to show on glass while pairing — `Some` between `PassKeyDisplay` and
    /// pairing completing/failing, `None` otherwise. When set, the status screen becomes the big-font
    /// passkey card the rider types into the phone.
    pub passkey: Option<u32>,
    /// True once the link is encrypted (a fresh pairing or a resumed bond) — the status screen's
    /// "secured" marker and the CoC's open-gate.
    pub secured: bool,
}

impl Status {
    const INIT: Status = Status {
        state: LinkState::Init,
        peer: None,
        conn_interval_ms: 0,
        att_mtu: 0,
        phy_2m: false,
        connects: 0,
        disconnects: 0,
        last_disconnect_reason: 0,
        passkey: None,
        secured: false,
    };
}

/// The published snapshot ([`Status`] is `Copy`, so a plain `Cell` under the blocking mutex).
static STATUS: BlockingMutex<CriticalSectionRawMutex, Cell<Status>> = BlockingMutex::new(Cell::new(Status::INIT));
/// Edge the status UI sleeps on: signalled on every [`publish`], consumed by [`wait_status_change`].
static STATUS_EDGE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Read-modify-write the published status + wake the status UI.
pub(crate) fn publish(f: impl FnOnce(&mut Status)) {
    STATUS.lock(|c| {
        let mut s = c.get();
        f(&mut s);
        c.set(s);
    });
    STATUS_EDGE.signal(());
}

/// The status UI's snapshot read (any time, non-blocking).
pub(crate) fn status() -> Status {
    STATUS.lock(|c| c.get())
}

/// The link distilled into the app-facing [`obc_app::BleStatus`] (epic #447, P1): connected + the
/// pairing passkey. The ride loop reads this each pass and feeds it through
/// [`App::set_ble_status`](obc_app::App::set_ble_status), so `obc-app` sees the link in its own
/// vocabulary without any `ble` type crossing the seam. `Init`/`Advertising` both read as *not*
/// connected.
pub fn app_ble_status() -> obc_app::BleStatus {
    let s = status();
    obc_app::BleStatus { connected: s.state == LinkState::Connected, passkey: s.passkey }
}

/// The battery percent for the BAS characteristic, read by `battery_task` to seed + notify. A
/// constant [`StubFuelGauge`]-matching 75 % until the real nPM1300 fuel gauge is wired across the
/// plane seam (the ride loop owns the gauge; feeding it into BAS is a #270 follow-up).
static BATTERY: AtomicU8 = AtomicU8::new(75);

/// The latest battery percent (BAS seed + notify).
pub(crate) fn battery() -> u8 {
    BATTERY.load(Ordering::Relaxed)
}

/// The deepest stack use seen so far (bytes), published by the status loop from its
/// [`stackmeter`](crate::stackmeter) paint-scan and surfaced in the diagnostics blob (§7.5) so the
/// A9 soak rig can post the stack high-water without RTT. 0 = not measured yet.
static STACK_HIGH_WATER: AtomicU32 = AtomicU32::new(0);

/// Publish a new stack high-water peak (called by `run_status` when the mark grows).
pub fn publish_stack_high_water(bytes: usize) {
    STACK_HIGH_WATER.store(bytes as u32, Ordering::Relaxed);
}

/// The latest stack high-water mark (bytes) for the diagnostics blob.
pub(crate) fn stack_high_water() -> u32 {
    STACK_HIGH_WATER.load(Ordering::Relaxed)
}

// ============================ Data-plane arming ============================

/// A transfer the control plane validated and handed to the data plane: the echo loopback, a route
/// upload with its ready fresh [`Receiver`] (the store opened the temp), or a download (the data plane
/// opens the source itself; opening may be slow — a CRC pre-pass — and belongs off the GATT reply
/// path).
#[derive(Clone, Copy)]
pub(crate) enum Armed {
    Echo(TransferControl),
    Upload(TransferControl, Receiver),
    Download(TransferControl),
}

/// The control plane → data plane hand-off: `serve_connection` decodes a `transfer_control` write,
/// validates it against the `ObjectStore`, and signals the [`Armed`] transfer here; `serve_coc` wakes
/// on it and drives the CoC. A `Signal` (latest-value) suffices because exactly one transfer is in
/// flight at a time — [`TRANSFER_ACTIVE`] turns a second open into a typed `busy` instead of a silent
/// overwrite.
pub(crate) static TRANSFER_ARM: Signal<CriticalSectionRawMutex, Armed> = Signal::new();

/// One-transfer-at-a-time: set by the control plane when it arms, cleared by the data plane when the
/// transfer concludes (answered, aborted, or the channel dropped). While set, another
/// `transferControl` open is answered `busy`.
pub(crate) static TRANSFER_ACTIVE: AtomicBool = AtomicBool::new(false);

/// An abort aimed at the in-flight transfer: the control plane signals, the data plane consumes it at
/// its next step (between SDUs / chunks), discards, and answers `aborted` with the durable offset.
/// Latched — an abort that races the transfer's own completion is drained by `serve_coc` after each
/// transfer, so it can't leak into the next one.
pub(crate) static TRANSFER_ABORT: Signal<CriticalSectionRawMutex, ()> = Signal::new();

// ============================ Status-message vocabulary ============================

/// A `status` notification's bytes, ready to hand to `server.notify` (`&buf[..len]`). The board keeps
/// one small stack buffer per message rather than a heapless alloc — every status message fits.
pub(crate) type StatusBytes = ([u8; StatusMessage::MAX_ENCODED_LEN], usize);

/// A `transferResult` status message with a zero `committed_offset` — the shape for every result the
/// control plane answers directly (nothing durable is being reported).
pub(crate) fn transfer_result(object_id: u16, status: TransferStatus) -> StatusBytes {
    transfer_result_at(object_id, status, 0)
}

/// A `transferResult` carrying a real durable byte count — a committed transfer reports its
/// `total_len`.
pub(crate) fn transfer_result_at(object_id: u16, status: TransferStatus, committed_offset: u32) -> StatusBytes {
    StatusMessage::TransferResult(TransferResult::new(object_id, status, committed_offset)).encode()
}
