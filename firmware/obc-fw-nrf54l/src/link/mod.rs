//! The **transport-free companion-link core**: everything the protocol does that is not about which
//! wire carried it.
//!
//! `obc-ble-interface-spec.md` principle #2 — the bulk channel is a raw byte pipe with *no*
//! per-chunk framing — is why a USB bulk endpoint pair slots in beneath the same object model as
//! BLE's L2CAP CoC. That claim only stays true if the two transports genuinely share the semantics
//! rather than each growing its own copy, so this module owns everything that would otherwise be
//! duplicated:
//!
//! - [`command::run_command`] — the §4.4 imperatives (`deleteObject`, `ackRides`, `installFw`,
//!   `forgetBond`, `setClock`, `setRouteRetention`). Takes the store, returns a typed outcome; it
//!   has never had a radio in it.
//! - [`transfer::classify_transfer`] — decode + validate a §4.2 descriptor against the store, and
//!   say whether it arms, is rejected outright, or aborts what is running.
//! - [`Armed`] + [`TRANSFER_ACTIVE`] — the one-transfer-at-a-time gate. **Deliberately shared
//!   across transports**: both planes drive the *same* [`ObjectStore`], which has exactly one
//!   upload temp and one open download source, so a BLE transfer in flight must answer a USB
//!   `transferControl` with `busy` and vice versa. A per-transport gate would let two uploads
//!   interleave into one temp file.
//! - [`identity`] — the FICR-derived serial/name, the DIS strings, and the Config /
//!   `protocolVersion` blob codecs, in plain bytes. BLE's GATT table wraps them into its
//!   attribute-value types; USB writes the same bytes into a control frame.
//! - The single [`ObjectStore`] itself ([`init_store`]). One card, one catalog, one revision
//!   counter — so one store, built once in `main` and handed to every plane.
//!
//! What stays transport-specific: how a control message is *addressed* (a GATT characteristic
//! handle vs. a USB selector byte), how a device → host message is *delivered* (an ATT notify vs. a
//! bulk-IN frame), and the link lifecycle (advertising/bonding vs. enumeration/VBUS).
//!
//! Compiled whenever the companion link exists at all. The USB plane is unconditional and the
//! radio is in every build that compiles, so gating on `ble` and gating on "either link" are the
//! same set today; the module is named for the concept rather than the radio so a future
//! radio-less build is a cfg rename, not a redesign.

pub(crate) mod command;
pub(crate) mod identity;
pub(crate) mod transfer;

use core::cell::RefCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use obc_ble::{Receiver, StatusMessage, TransferControl, TransferResult, TransferStatus};

use crate::init_static;
use crate::object_store::{DiagInput, ObjectStore};

// ============================ The one object store ============================

/// The single [`ObjectStore`]: catalog / upload / download / revision semantics behind a `RefCell`
/// that every plane borrows **synchronously, never across an `await`**. The SD card + RRAM settings
/// it operates on live in [`crate::SharedStore`] (the async mutex the ride loop shares), locked per
/// call and passed into each store method.
static mut STORE: MaybeUninit<RefCell<ObjectStore>> = MaybeUninit::uninit();

/// Size of the resident store, for the `main.rs` budget assert + the resource report. (Reported
/// under the historical `ble_object_store` name — the allocation did not change, only its module.)
pub(crate) const OBJECT_STORE_BYTES: usize = core::mem::size_of::<RefCell<ObjectStore>>();

/// Build the object store into its `.bss` slot and hand out the one `&'static` reference.
///
/// `#[inline(never)]` is load-bearing: the ~13.5 KB construction temporary must land in **this**
/// transient frame — popped immediately, at boot's shallow depth — and not become a permanent slot
/// in a caller's async poll frame, which is allocated at entry on *every* poll (#677). Measured on
/// the shipping ELF this frame is ~27.6 KB, which is exactly why it must not be inlined into
/// `main`'s or a task's steady-state frame.
///
/// # Safety
/// Sole writer of [`STORE`]; called exactly once, from `main`, before any plane is spawned.
#[inline(never)]
pub(crate) fn init_store(shared: &mut crate::SharedStore) -> &'static RefCell<ObjectStore> {
    unsafe { init_static(core::ptr::addr_of_mut!(STORE), RefCell::new(ObjectStore::new(shared))) }
}

/// The storage handles a link plane is composed with. They always travel together — every control
/// and data plane needs all three — so they are handed over as one value rather than as three
/// parallel parameters threaded through each transport's spawn trampoline.
#[derive(Clone, Copy)]
pub(crate) struct LinkStores {
    /// The SD card + RRAM settings, behind the async mutex the ride loop shares. Locked per store
    /// call and released before the next channel `await`, so the map render interleaves.
    pub shared: &'static crate::SharedStoreMutex,
    /// The one object store (see [`init_store`]).
    pub objects: &'static RefCell<ObjectStore>,
    /// The boot mint pass's store-epoch outcome — the value the §1 identity read serves. `None`
    /// (no mounted store) means the version-only form; it is never re-derived by a plane, so a card
    /// swap cannot silently change what a plane reports.
    pub epoch: Option<u32>,
}

// ============================ Cross-plane mirrors ============================

/// Whether a ride is recording, mirrored across the plane boundary: the ride loop owns the `App`
/// and pushes `app.activity.is_tracking()` here each pass ([`set_recording`]); the `installFw`
/// command handler reads it as the `busy` gate's "a ride is recording" input (spec §4.4) — the arm
/// ends in a reboot, so an install must never be requested mid-ride. Defaults **false**; the ride
/// loop seeds the real value on its first pass. `Relaxed`: every plane is a cooperative future on
/// the one executor, and a stale read is at worst one pass late (the on-device guard still refuses).
static RECORDING: AtomicBool = AtomicBool::new(false);

/// Push the ride-recording state to the link planes (ride loop, once per pass — one atomic store).
pub fn set_recording(recording: bool) {
    RECORDING.store(recording, Ordering::Relaxed);
}

/// Whether a ride is recording (the `installFw` `busy` gate).
pub(crate) fn recording() -> bool {
    RECORDING.load(Ordering::Relaxed)
}

/// The deepest stack use seen so far (bytes), published by the status loop from its
/// [`stackmeter`](crate::stackmeter) paint-scan and surfaced in the diagnostics blob (§7.5) so a
/// soak rig can post the stack high-water without RTT. 0 = not measured yet.
static STACK_HIGH_WATER: AtomicU32 = AtomicU32::new(0);

/// Publish a new stack high-water peak (called by the ride loop when the mark grows).
pub fn publish_stack_high_water(bytes: usize) {
    STACK_HIGH_WATER.store(bytes as u32, Ordering::Relaxed);
}

/// The latest stack high-water mark (bytes) for the diagnostics blob.
pub(crate) fn stack_high_water() -> u32 {
    STACK_HIGH_WATER.load(Ordering::Relaxed)
}

/// Assemble the §7.5 diagnostics input both data planes hand to
/// [`ObjectStore::download_open`](crate::object_store::ObjectStore::download_open).
///
/// The link counters are the **BLE** link's on purpose: the diagnostics object describes the
/// *device*, not the transport that asked for it, and a USB reader wants the radio's connect /
/// disconnect history exactly as the phone does. Callers own the two identity strings because
/// `DiagInput` borrows them.
pub(crate) fn diag_input<'a>(firmware: &'a str, serial: &'a str, uptime_s: u32) -> DiagInput<'a> {
    let s = crate::ble::link_counters();
    DiagInput {
        firmware,
        hardware: identity::HARDWARE_REVISION,
        serial,
        uptime_s,
        connects: s.0,
        disconnects: s.1,
        last_disconnect_reason: s.2,
        stack_hw: stack_high_water(),
    }
}

// ============================ Data-plane arming ============================

/// A transfer a control plane validated and handed to its data plane: the echo loopback, an upload
/// with its ready fresh [`Receiver`] (the store opened the temp), or a download (the data plane
/// opens the source itself; opening may be slow — a CRC pre-pass — and belongs off the reply path).
#[derive(Clone, Copy)]
pub(crate) enum Armed {
    Echo(TransferControl),
    Upload(TransferControl, Receiver),
    Download(TransferControl),
}

/// Which wire a descriptor arrived on — the *only* place transport identity crosses into the shared
/// classifier, and it exists for exactly one rule: **a map is USB-only** (spec §10). A map is
/// hundreds of megabytes; over BLE it would be days, which is why the type did not exist before a
/// cable did. Rather than leave that to a comment, [`transfer::classify_transfer`] takes this and a
/// map descriptor on the radio is refused with a typed, logged `error` — the same shape as any other
/// unsupported op/type pair, rather than silently falling into the route commit path.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transport {
    /// The BLE link: GATT control plane, L2CAP CoC data plane.
    Ble,
    /// The USB device plane: framed control endpoint, bulk data endpoints.
    Usb,
}

// ============================ Map-transfer progress mirror (issue #927) ============================
//
// A map upload writes for **minutes**. The ride loop owns the `App` and is the only task that may
// touch it, and the USB data plane must not block on it, so progress crosses the plane boundary the
// same way `RECORDING` does — plain atomics the ride loop reads once per pass and feeds through
// `App::set_map_transfer`. Nothing here is a queue: the value is a *state*, always re-readable, and
// a missed intermediate is simply a frame that showed the previous percentage.

/// The transfer phase, as a `u8` so it fits an atomic: 0 = idle, 1 = receiving, 2 = installed,
/// 3 = storage failure, 4 = damaged (CRC), 5 = not a readable map.
static MAP_PHASE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// Kibibytes received so far, and the announced total. KiB rather than bytes so a 4 GiB map still
/// fits a `u32` with room, and finer than any bar 240 px can resolve.
static MAP_RX_KIB: AtomicU32 = AtomicU32::new(0);
static MAP_TOTAL_KIB: AtomicU32 = AtomicU32::new(0);

/// Publish "a map transfer has started, `total_len` bytes announced" (USB data plane).
pub(crate) fn map_transfer_started(total_len: u32) {
    MAP_RX_KIB.store(0, Ordering::Relaxed);
    MAP_TOTAL_KIB.store(total_len / 1024, Ordering::Relaxed);
    MAP_PHASE.store(1, Ordering::Relaxed);
}

/// Publish the running byte count (USB data plane, once per chunk — two relaxed stores).
pub(crate) fn map_transfer_progress(received: u32) {
    MAP_RX_KIB.store(received / 1024, Ordering::Relaxed);
}

/// Publish the transfer's outcome. `None` clears the state entirely — which is what an abort or an
/// unplug does, deliberately: the rider caused those and needs no card explaining it back to them.
pub(crate) fn map_transfer_ended(outcome: Option<TransferStatus>) {
    MAP_PHASE.store(
        match outcome {
            None => 0,
            Some(TransferStatus::Committed) => 2,
            Some(TransferStatus::CrcMismatch) => 4,
            // `error` after the bytes landed is the commit's verdict: either the card refused the
            // write or the payload wasn't a readable OBCM. The store logs which; the card says the
            // one the rider can act on — re-send from a builder that targets this OBCM version.
            Some(_) => 5,
        },
        Ordering::Relaxed,
    );
}

/// Publish a storage failure (the file could not be opened or a chunk could not be appended) —
/// distinct from a bad payload, because the fix is different: free space or a different card.
pub(crate) fn map_transfer_storage_failed() {
    MAP_PHASE.store(3, Ordering::Relaxed);
}

/// The app-facing map-transfer state, or `None` when there is nothing to show — read once per pass
/// by the ride loop and handed to [`obc_app::App::set_map_transfer`].
pub fn map_transfer_state() -> Option<obc_app::screen::MapTransfer> {
    use obc_app::screen::{MapTransfer, MapTransferError};
    Some(match MAP_PHASE.load(Ordering::Relaxed) {
        1 => MapTransfer::Receiving {
            received_kib: MAP_RX_KIB.load(Ordering::Relaxed),
            total_kib: MAP_TOTAL_KIB.load(Ordering::Relaxed),
        },
        2 => MapTransfer::Installed,
        3 => MapTransfer::Failed(MapTransferError::Storage),
        4 => MapTransfer::Failed(MapTransferError::Damaged),
        5 => MapTransfer::Failed(MapTransferError::NotAMap),
        _ => return None,
    })
}

/// Clear the map-transfer state — called when the rider dismisses the terminal card, so the ride
/// loop's next pass doesn't immediately push it back.
///
/// Clears **only a terminal state**. The dismissal is observed a pass after it happened (the card
/// pops itself; nothing tells the board), so in the gap a fresh transfer could have started — and
/// clearing *that* would leave a multi-minute write with no card and no way to raise one, since
/// progress updates only touch the byte counters. Every plane runs as a cooperative future on the
/// one executor and this holds no `await`, so the read-modify-write needs no stronger ordering.
pub fn clear_map_transfer() {
    if MAP_PHASE.load(Ordering::Relaxed) >= 2 {
        MAP_PHASE.store(0, Ordering::Relaxed);
    }
}

/// One transfer at a time, **across every transport**: set by whichever control plane armed it,
/// cleared by that transport's data plane when the transfer concludes (answered, aborted, or the
/// channel dropped). While set, any further `transferControl` open — BLE or USB — is answered
/// `busy`.
///
/// Shared rather than per-transport because the resource being arbitrated is the store, not the
/// wire: [`ObjectStore`] holds exactly one upload temp file and one open download source. Two
/// simultaneous uploads would interleave into the same temp and commit a corrupt object.
pub(crate) static TRANSFER_ACTIVE: AtomicBool = AtomicBool::new(false);

// ============================ Status-message vocabulary ============================

/// A `status` message's bytes, ready to hand to a transport (`&buf[..len]`). Each plane keeps one
/// small stack buffer per message rather than a heapless alloc — every status message fits.
pub(crate) type StatusBytes = ([u8; StatusMessage::MAX_ENCODED_LEN], usize);

/// A `transferResult` status message with a zero `committed_offset` — the shape for every result a
/// control plane answers directly (nothing durable is being reported).
pub(crate) fn transfer_result(object_id: u16, status: TransferStatus) -> StatusBytes {
    transfer_result_at(object_id, status, 0)
}

/// A `transferResult` carrying a real durable byte count — a committed transfer reports its
/// `total_len`.
pub(crate) fn transfer_result_at(object_id: u16, status: TransferStatus, committed_offset: u32) -> StatusBytes {
    StatusMessage::TransferResult(TransferResult::new(object_id, status, committed_offset)).encode()
}
