//! Shared **radio** state: the link-status snapshot the UI reads, the Bluetooth switch, the BAS
//! battery cell, and the CoC arming channel the control plane ([`super::control`]) and the CoC data
//! plane ([`super::data_plane`]) coordinate through. Everything here is `pub(crate)` at most — it
//! lives entirely within the `ble` module tree, read/written across the four planes but never wider.
//!
//! Anything a *second transport* would also need — the command handler, descriptor classification,
//! the cross-transport one-transfer gate, the identity blobs — lives in [`crate::link`] instead.

use core::cell::Cell;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::signal::Signal;

use crate::link::Armed;

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
    /// The radio is switched **off** (the Bluetooth setting, #455): not advertising, no link, and
    /// the lifecycle loop parked until [`set_radio_enabled`] re-arms it.
    Off,
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
    /// A bond is stored in the RRAM slot (#455): seeded at boot from `load_bond`, raised when a
    /// fresh pairing persists, cleared by Forget phone. Drives the app's "Paired" row **and** the
    /// reject-when-bonded pairing policy (the control plane refuses new pairing attempts while set).
    pub paired: bool,
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
        paired: false,
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

/// Wait for the next link-edge [`publish`] — the wake the event-driven map loop selects on so a link
/// change (connect/disconnect, and crucially the pairing `PassKeyDisplay`) pulls it out of warm
/// sleep to feed `set_ble_status` and render the passkey card (epic #447, P2). Reuses the existing
/// `publish` edge (`STATUS_EDGE`); it invents no new wake path. `Signal` is level/coalescing, so a
/// burst of publishes wakes the loop once and the next snapshot read carries the latest state.
pub fn wait_status_change() -> impl core::future::Future<Output = ()> {
    STATUS_EDGE.wait()
}

/// The link distilled into the app-facing [`obc_app::BleStatus`] (epic #447, P1 + P8): the
/// three-state link, the pairing passkey, and the stored-bond flag. The ride loop reads this each
/// pass and feeds it through [`App::set_ble_status`](obc_app::App::set_ble_status), so `obc-app`
/// sees the link in its own vocabulary without any `ble` type crossing the seam. `Init` reads as
/// `Advertising` (the UI never needs "stack coming up").
pub fn app_ble_status() -> obc_app::BleStatus {
    let s = status();
    let link = match s.state {
        LinkState::Off => obc_app::BleLink::Off,
        LinkState::Init | LinkState::Advertising => obc_app::BleLink::Advertising,
        LinkState::Connected => obc_app::BleLink::Connected,
    };
    obc_app::BleStatus { link, passkey: s.passkey, paired: s.paired }
}

// ============================ Settings → radio control (#455) ============================

/// The rider's Bluetooth switch, mirrored across the plane boundary: the ride loop owns the
/// persisted [`Settings`](obc_app::Settings) and pushes the value here each pass
/// ([`set_radio_enabled`]); the lifecycle loop in [`super::run`] reads it and parks the radio while
/// off. Defaults **on** so a BLE build without the ride loop's seed still advertises; `run` re-seeds
/// it from the persisted settings at boot, before the first advertise.
static RADIO_ENABLED: AtomicBool = AtomicBool::new(true);

/// Edge for the lifecycle loop: signalled whenever [`RADIO_ENABLED`] *changes*, so the advertise /
/// serve phases can wake, re-check the level, and wind the radio up or down. Level + edge (not just
/// a `Signal` payload) so a toggle bounced off-and-on between polls degrades to a harmless
/// re-advertise, never a stuck state.
static RADIO_EDGE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// The Bluetooth screen's **Forget phone** (#455), rung by the ride loop after
/// [`App::drain_host_commands`](obc_app::App::drain_host_commands): the lifecycle loop clears the RRAM bond
/// slot + the host's bond table and drops the bonded connection. Latching, so a request raised
/// between phases is picked up at the next loop top.
pub(crate) static FORGET_BOND: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Push the rider's Bluetooth switch to the radio plane (called by the ride loop once per pass —
/// one atomic swap; the edge fires only on a change). `false` stops advertising and drops a live
/// connection; `true` resumes the normal advertising lifecycle (policy unchanged). Also pulses the
/// **sensor** manager's work edge (SE6, #707) so the central-role task winds sensor links up/down
/// with the phone link — its own dedicated wake so it never contends on `RADIO_EDGE`'s single waiter.
pub fn set_radio_enabled(enabled: bool) {
    if RADIO_ENABLED.swap(enabled, Ordering::Relaxed) != enabled {
        RADIO_EDGE.signal(());
        super::sensors::wake_work();
    }
}

/// Boot seed for [`RADIO_ENABLED`] from the persisted settings — no edge (the lifecycle loop hasn't
/// started; it reads the level at its first pass).
pub(crate) fn seed_radio_enabled(enabled: bool) {
    RADIO_ENABLED.store(enabled, Ordering::Relaxed);
}

/// The current radio switch level.
pub(crate) fn radio_enabled() -> bool {
    RADIO_ENABLED.load(Ordering::Relaxed)
}

/// Resolve once the radio switch reads **off** — the advertise / serve phases' wind-down arm.
/// Consumes [`RADIO_EDGE`] signals while enabled, so a bounced toggle just re-checks the level.
pub(crate) async fn radio_disabled() {
    loop {
        if !radio_enabled() {
            return;
        }
        RADIO_EDGE.wait().await;
    }
}

/// Resolve once the radio switch reads **on** — the parked Off phase's wake.
pub(crate) async fn radio_enabled_wait() {
    loop {
        if radio_enabled() {
            return;
        }
        RADIO_EDGE.wait().await;
    }
}

/// Ring the Forget-phone request (called by the ride loop; the BLE lifecycle honours it in any
/// phase — parked, advertising, or connected).
pub fn request_forget_bond() {
    FORGET_BOND.signal(());
}

/// The lifetime link counters + the last disconnect reason, for the §7.5 diagnostics blob
/// ([`crate::link::diag_input`]). Kept here because they are the *radio* link's history; the
/// diagnostics object is a device fact, so a USB reader gets the same three numbers.
pub(crate) fn link_counters() -> (u32, u32, u8) {
    let s = status();
    (s.connects, s.disconnects, s.last_disconnect_reason)
}

/// The battery percent for the BAS characteristic, read by `battery_task` to seed + notify. A
/// constant [`StubFuelGauge`]-matching 75 % until the real nPM1300 fuel gauge is wired across the
/// plane seam (the ride loop owns the gauge; feeding it into BAS is a #270 follow-up).
static BATTERY: AtomicU8 = AtomicU8::new(75);

/// The latest battery percent (BAS seed + notify).
pub(crate) fn battery() -> u8 {
    BATTERY.load(Ordering::Relaxed)
}

// ============================ CoC data-plane arming ============================

/// The GATT control plane → CoC data plane hand-off: `serve_connection` decodes a
/// `transfer_control` write, [`classify_transfer`](crate::link::transfer::classify_transfer)
/// validates it against the `ObjectStore`, and the [`Armed`] transfer is signalled here; `serve_coc`
/// wakes on it and drives the CoC. A `Signal` (latest-value) suffices because exactly one transfer
/// is in flight at a time — the *cross-transport* [`TRANSFER_ACTIVE`](crate::link::TRANSFER_ACTIVE)
/// gate turns a second open into a typed `busy` instead of a silent overwrite. The signal itself is
/// per-transport: each data plane waits on its own.
pub(crate) static TRANSFER_ARM: Signal<CriticalSectionRawMutex, Armed> = Signal::new();

/// An abort aimed at the in-flight CoC transfer: the control plane signals, the data plane consumes
/// it at its next step (between SDUs / chunks), discards, and answers `aborted` with the durable
/// offset. Latched — an abort that races the transfer's own completion is drained at the same
/// boundary that clears `TRANSFER_ACTIVE`, before the terminal result is notified, so it can't leak
/// into the next one.
pub(crate) static TRANSFER_ABORT: Signal<CriticalSectionRawMutex, ()> = Signal::new();
