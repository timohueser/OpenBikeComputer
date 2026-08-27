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
//! - [`identity`] — the FICR-derived serial/name, the DIS strings, and the Config /
//!   `protocolVersion` blob codecs, in plain bytes. BLE's GATT table wraps them into its
//!   attribute-value types; USB writes the same bytes into a control frame.
//! - The single [`ObjectStore`] itself ([`init_store`]). One card, one catalog, one revision
//!   counter — so one store, built once in `main` and handed to every plane.
//!
//! What stays transport-specific: how a control message is *addressed* (a GATT characteristic
//! handle vs. an EP0 vendor request, `FLAT_Store_Protocol.md` §5.2.1) and the link lifecycle
//! (advertising/bonding vs. enumeration/VBUS). The object surface is not here at all any more: both
//! links speak protocol v4 into the one engine in `crate::flat_store`.
//!
//! Compiled in every build: the USB plane and the radio both are. The module is named for the
//! concept rather than the radio so a future radio-less build is a cfg addition, not a redesign.

pub(crate) mod command;
pub(crate) mod identity;

use core::cell::RefCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use obc_ble::StatusMessage;
use obc_link::flat::ObjectKind;

use crate::init_static;
use crate::object_store::ObjectStore;

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
/// in a caller's async poll frame, which is allocated at entry on *every* poll (#677).
///
/// Construction is two-phase on purpose: the **const** empty store ([`ObjectStore::EMPTY`], a
/// `.rodata` image the slot write memcpys from — no stack temporary can exist for a constant),
/// then [`ObjectStore::hydrate`] loads settings and scans the card **in place**. History, because
/// this shape has bitten twice: the original `RefCell::new(ObjectStore::new(shared))` stacked two
/// ~13.5 KB copies (the `new` return slot + the wrapper argument) into a measured ~27.6 KB frame
/// — which overran the residual main stack the moment EL7 grew the ride task's poll frame by 2 KB
/// (STKOF HardFault at this function's prologue, every boot, 2026-08-03). The fix then was a
/// by-value `empty()` hop the optimizer collapsed to one copy — until WX12 (#1197) grew
/// `Settings` by 96 B and rustc 1.96 stopped collapsing it, re-stacking both temporaries (the
/// boot-chain guard caught it before any glass did). The const image ends the optimizer's say in
/// the matter, at the price of the empty store's bytes in flash.
///
/// # Safety
/// Sole writer of [`STORE`]; called exactly once, from `main`, before any plane is spawned.
#[inline(never)]
pub(crate) fn init_store(shared: &mut crate::SharedStore) -> &'static RefCell<ObjectStore> {
    /// The fully-wrapped initial value as a named constant: `ptr::write` of a constant lowers to
    /// a `.rodata` -> slot memcpy, with no `RefCell<ObjectStore>`-sized stack value anywhere.
    /// The `declare_interior_mutable_const` lint warns that every use of such a const is a fresh
    /// copy that forgets mutations — here that copy-on-use IS the mechanism (one write into the
    /// slot, never mutated as a const), so the lint's hazard cannot arise.
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: RefCell<ObjectStore> = RefCell::new(ObjectStore::EMPTY);
    let cell = unsafe { init_static(core::ptr::addr_of_mut!(STORE), INIT) };
    cell.borrow_mut().hydrate(shared);
    cell
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
/// and pushes `app.recording()` here each pass ([`set_recording`]); the `installFw`
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

// ============================ Map-transfer progress mirror (issue #927) ============================
//
// A map upload writes for **minutes**. The ride loop owns the `App` and is the only task that may
// touch it, and the USB data plane must not block on it, so progress crosses the plane boundary the
// same way `RECORDING` does — plain atomics the ride loop reads once per pass and feeds through
// `App::set_map_transfer`. Nothing here is a queue: the value is a *state*, always re-readable, and
// a missed intermediate is simply a frame that showed the previous percentage.

/// The transfer phase, as a `u8` so it fits an atomic: 0 = idle, 1 = receiving, 2 = installed,
/// 3 = storage failure, 4 = damaged (CRC), 5 = not a readable map, 6 = a file of a volume set was
/// refused before it streamed (#1044).
static MAP_PHASE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// Kibibytes received so far, and the announced total. KiB rather than bytes so a 4 GiB map still
/// fits a `u32` with room, and finer than any bar 240 px can resolve.
static MAP_RX_KIB: AtomicU32 = AtomicU32::new(0);
static MAP_TOTAL_KIB: AtomicU32 = AtomicU32::new(0);

/// **The engine's view of an upload, as a screen** — called from `flat_store::serve` after every
/// engine call, on whichever link made it.
///
/// One function rather than the five edges the v1 planes published, because there are no longer five
/// moments to publish at: the engine is a state machine one execution context holds, so the honest
/// shape is "here is everything that is true now" rather than a sequence of notifications a plane
/// has to remember to send on every path. That sequence is exactly how the v1 version grew a
/// `map_transfer_refused` for the one case a plane forgot.
///
/// The three inputs collapse to three outcomes:
///
/// - a **verdict** wins, whatever else is true: it is the terminal card and it was latched precisely
///   because it is true for one instant;
/// - otherwise a **live map upload** is a progress bar;
/// - otherwise, if a bar is on the glass and nothing is live, the transfer went away without a
///   verdict — a pulled cable, a dropped connection, a `CANCEL`. That clears the card rather than
///   raising one. The rider caused all three and needs no card explaining it back to them.
pub(crate) fn publish_map_transfer(
    live: Option<obc_link::flat::UploadProgress>,
    ended: Option<(ObjectKind, obc_link::flat::UploadEnd)>,
) {
    use obc_link::flat::UploadEnd;
    if let Some((ObjectKind::MapShard, end)) = ended {
        MAP_PHASE.store(
            match end {
                UploadEnd::Committed { .. } => 2,
                // The payload arrived damaged: the whole-object CRC the `PUT` declared did not match
                // what landed. Re-sending is the fix, and it is the one the card names.
                UploadEnd::Refused(obc_link::flat::ErrorCode::ChecksumFailure) => 4,
                // The card could not take the bytes — out of space, or media that refused a write.
                // A different fix (free space, another card), so a different card.
                UploadEnd::Refused(obc_link::flat::ErrorCode::NoSpace | obc_link::flat::ErrorCode::MediaIo) => 3,
                // Everything else is the object being wrong for this device: a kind validator said
                // no, a revision moved underneath, the store is read-only. Re-send from a builder
                // that targets this firmware.
                UploadEnd::Refused(_) => 5,
            },
            Ordering::Relaxed,
        );
        return;
    }
    match live {
        Some(progress) if progress.kind == ObjectKind::MapShard => {
            // KiB rather than bytes so a 4 GiB map still fits a `u32` with room, and finer than any
            // bar 240 px can resolve.
            MAP_RX_KIB.store((progress.received / 1024) as u32, Ordering::Relaxed);
            MAP_TOTAL_KIB.store((progress.declared / 1024) as u32, Ordering::Relaxed);
            MAP_PHASE.store(1, Ordering::Relaxed);
        }
        // Nothing live and nothing ended: if a bar is up, its transfer is gone.
        _ => {
            if MAP_PHASE.load(Ordering::Relaxed) == 1 {
                MAP_PHASE.store(0, Ordering::Relaxed);
            }
        }
    }
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
        6 => MapTransfer::Failed(MapTransferError::Refused),
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

// ============================ Status-message vocabulary ============================

/// A `status` message's bytes, ready to hand to a transport (`&buf[..len]`). Each plane keeps one
/// small stack buffer per message rather than a heapless alloc — every status message fits.
pub(crate) type StatusBytes = ([u8; StatusMessage::MAX_ENCODED_LEN], usize);
