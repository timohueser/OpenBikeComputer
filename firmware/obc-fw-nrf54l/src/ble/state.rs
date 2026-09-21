//! Shared radio state: the link-status snapshot the UI reads, the Bluetooth switch, the BAS battery
//! cell, and the bond-removal request slot the control plane and the lifecycle loop coordinate
//! through. Everything here is `pub(crate)` at most and lives inside the `ble` module tree.
//!
//! Anything a second transport would also need — the command handler, descriptor classification, the
//! identity blobs — lives in [`crate::link`] instead.

use core::cell::Cell;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::signal::Signal;

#[derive(Clone, Copy, PartialEq, Eq, defmt::Format)]
pub(crate) enum LinkState {
    /// Stack still coming up: the boot instant, before the first advertise.
    Init,
    Advertising,
    Connected,
    /// The radio is switched off: not advertising, no link, and the lifecycle loop parked until
    /// [`set_radio_enabled`] re-arms it.
    Off,
}

/// One coherent snapshot of the link for the status UI, published by the BLE plumbing below and
/// drained by `run_status` in `main.rs`.
#[derive(Clone, Copy)]
pub(crate) struct Status {
    pub state: LinkState,
    /// The connected central's address (little-endian, as the wire carries it), while connected.
    pub peer: Option<[u8; 6]>,
    /// The live connection interval in ms, once the central negotiated one; 0 = not reported yet.
    pub conn_interval_ms: u32,
    /// The negotiated ATT MTU (target 247); 0 = not exchanged yet.
    pub att_mtu: u16,
    pub phy_2m: bool,
    pub connects: u32,
    pub disconnects: u32,
    /// The HCI reason code of the most recent disconnect; 0 = none yet.
    pub last_disconnect_reason: u8,
    /// The 6-digit LESC passkey to show on glass while pairing. `Some` between `PassKeyDisplay` and
    /// pairing completing or failing. When set, the status screen becomes the passkey card the rider
    /// types into the phone.
    pub passkey: Option<u32>,
    pub secured: bool,
    /// A bond is stored in the RRAM slot: seeded at boot, raised when a fresh pairing persists,
    /// cleared by Forget phone. Drives the app's "Paired" row and the reject-when-bonded pairing
    /// policy.
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

static STATUS: BlockingMutex<CriticalSectionRawMutex, Cell<Status>> = BlockingMutex::new(Cell::new(Status::INIT));
/// Edge the status UI sleeps on: signalled on every [`publish`], consumed by [`wait_status_change`].
static STATUS_EDGE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

pub(crate) fn publish(f: impl FnOnce(&mut Status)) {
    STATUS.lock(|c| {
        let mut s = c.get();
        f(&mut s);
        c.set(s);
    });
    wake_status();
}

pub(crate) fn wake_status() {
    STATUS_EDGE.signal(());
}

pub(crate) fn status() -> Status {
    STATUS.lock(|c| c.get())
}

/// Wait for the next link-edge [`publish`]. The event-driven map loop selects on this, so a link
/// change — connect, disconnect, and the pairing `PassKeyDisplay` — pulls it out of warm sleep to
/// feed `set_ble_status` and render the passkey card. `Signal` coalesces, so a burst of publishes
/// wakes the loop once and the next snapshot read carries the latest state.
pub fn wait_status_change() -> impl core::future::Future<Output = ()> {
    STATUS_EDGE.wait()
}

/// The link distilled into the app-facing [`obc_app::BleStatus`]: the three-state link, the pairing
/// passkey and the stored-bond flag. The ride loop reads this each pass and feeds it through
/// [`App::set_ble_status`](obc_app::App::set_ble_status), so no `ble` type crosses the seam. `Init`
/// reads as `Advertising`.
pub fn app_ble_status() -> obc_app::BleStatus {
    let s = status();
    let link = match s.state {
        LinkState::Off => obc_app::BleLink::Off,
        LinkState::Init | LinkState::Advertising => obc_app::BleLink::Advertising,
        LinkState::Connected => obc_app::BleLink::Connected,
    };
    obc_app::BleStatus { link, passkey: s.passkey, paired: s.paired }
}

/// The rider's Bluetooth switch, mirrored across the plane boundary: the ride loop pushes the
/// persisted value here each pass, and the lifecycle loop in [`super::run`] parks the radio while
/// off. Defaults on, so a BLE build without the ride loop's seed still advertises; `run` re-seeds it
/// from the persisted settings at boot, before the first advertise.
static RADIO_ENABLED: AtomicBool = AtomicBool::new(true);

/// Cable-level radio interlock. Active BLE radio work and the nRF54L sEMMC card engine have been
/// seen to corrupt card commands when they overlap, so USB map transfers own the radio for the whole
/// time J3 has VBUS. Starts inhibited, so a cable present at boot cannot race the first
/// advertisement; the USB task releases it once it has sampled VBUS low.
static USB_RADIO_INHIBITED: AtomicBool = AtomicBool::new(true);

/// Edge for the lifecycle loop, signalled whenever [`RADIO_ENABLED`] changes, so the advertise and
/// serve phases can wake, re-check the level, and wind the radio up or down. The level is the
/// authority, so a toggle bounced off and on between polls degrades to a harmless re-advertise,
/// never a stuck state.
static RADIO_EDGE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// The Bluetooth screen's Forget phone, rung by the ride loop: the lifecycle loop clears the RRAM
/// bond slot and the host's bond table, then drops the bonded connection. Latching, so a request
/// raised between phases is picked up at the next loop top.
pub(crate) static FORGET_BOND: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Push the rider's Bluetooth switch to the radio plane, called by the ride loop once per pass. The
/// edge fires only on a change. `false` stops advertising and drops a live connection; `true`
/// resumes the normal advertising lifecycle. Also pulses the sensor manager's own work edge, so the
/// central-role task winds sensor links up and down with the phone link and never contends on
/// `RADIO_EDGE`'s single waiter.
pub fn set_radio_enabled(enabled: bool) {
    if RADIO_ENABLED.swap(enabled, Ordering::Relaxed) != enabled && !USB_RADIO_INHIBITED.load(Ordering::Relaxed) {
        RADIO_EDGE.signal(());
        super::sensors::wake_work();
    }
}

/// Boot seed for [`RADIO_ENABLED`] from the persisted settings. No edge: the lifecycle loop has not
/// started, and it reads the level at its first pass.
pub(crate) fn seed_radio_enabled(enabled: bool) {
    RADIO_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Inhibit active phone advertising/connections and sensor scanning while USB has VBUS. This does
/// not overwrite the rider's persisted switch: removing the cable restores its effective level.
pub(crate) fn set_usb_radio_inhibited(inhibited: bool) {
    if USB_RADIO_INHIBITED.swap(inhibited, Ordering::Relaxed) != inhibited && RADIO_ENABLED.load(Ordering::Relaxed) {
        RADIO_EDGE.signal(());
        super::sensors::wake_work();
    }
}

/// The effective radio level after applying the rider switch and USB interlock.
pub(crate) fn radio_enabled() -> bool {
    RADIO_ENABLED.load(Ordering::Relaxed) && !USB_RADIO_INHIBITED.load(Ordering::Relaxed)
}

/// Resolve once the radio switch reads off — the advertise and serve phases' wind-down arm. It
/// consumes [`RADIO_EDGE`] signals while enabled, so a bounced toggle just re-checks the level.
pub(crate) async fn radio_disabled() {
    loop {
        if !radio_enabled() {
            return;
        }
        RADIO_EDGE.wait().await;
    }
}

pub(crate) async fn radio_enabled_wait() {
    loop {
        if radio_enabled() {
            return;
        }
        RADIO_EDGE.wait().await;
    }
}

/// Ring the Forget-phone request. The BLE lifecycle honours it in any phase.
pub(crate) fn request_forget_bond() {
    FORGET_BOND.signal(());
}

/// The battery percent for the BAS characteristic, read by `battery_task` to seed and notify. A
/// constant 75 % until the real fuel gauge is wired across the plane seam.
static BATTERY: AtomicU8 = AtomicU8::new(75);

pub(crate) fn battery() -> u8 {
    BATTERY.load(Ordering::Relaxed)
}

// Retained request/result; FORGET_BOND and STATUS_EDGE only coalesce wakeups.
static BOND_DELIVERY: BlockingMutex<CriticalSectionRawMutex, core::cell::RefCell<obc_app::ble::BondDelivery>> =
    BlockingMutex::new(core::cell::RefCell::new(obc_app::ble::BondDelivery::new()));

pub fn try_forget_bond(effect: obc_app::ble::BondEffect) -> Result<(), obc_app::ble::BondError> {
    BOND_DELIVERY.lock(|slot| slot.borrow_mut().submit(effect))?;
    FORGET_BOND.signal(());
    Ok(())
}

pub(crate) fn begin_bond_removal() -> Option<obc_app::ble::BondEffect> {
    BOND_DELIVERY.lock(|slot| slot.borrow_mut().begin())
}

pub(crate) fn finish_bond_removal(outcome: obc_app::ble::BondOutcome) {
    let accepted = BOND_DELIVERY.lock(|slot| slot.borrow_mut().finish(outcome));
    debug_assert!(accepted, "bond result must match the running request");
    STATUS_EDGE.signal(());
}

pub fn take_bond_outcome() -> Option<obc_app::ble::BondOutcome> {
    BOND_DELIVERY.lock(|slot| slot.borrow_mut().take_outcome())
}
