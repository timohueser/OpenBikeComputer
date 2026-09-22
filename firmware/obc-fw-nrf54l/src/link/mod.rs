//! The transport-free companion-link core: everything the protocol does that is not about which
//! wire carried it.
//!
//! The bulk channel is a raw byte pipe with no per-chunk framing, which is why a USB bulk endpoint
//! pair slots in beneath the same object model as BLE's L2CAP CoC. That stays true only if the two
//! transports share the semantics instead of each growing its own copy, so this module owns what
//! would otherwise be duplicated:
//!
//! - [`command::run_command`] — the imperatives (`installFw`, `forgetBond`, `setClock`). It takes
//!   the store and returns a typed outcome.
//! - [`identity`] — the FICR-derived serial and name, the DIS strings, and the Config and
//!   `protocolVersion` blob codecs, in plain bytes. BLE's GATT table wraps them into its
//!   attribute-value types; USB writes the same bytes into a control frame.
//! - The single [`ObjectStore`] itself ([`init_store`]). One card, one catalog, one revision
//!   counter, so one store, built once in `main` and handed to every plane.
//!
//! What stays transport-specific: how a control message is addressed (a GATT characteristic handle,
//! or an EP0 vendor request) and the link lifecycle (advertising and bonding, or enumeration and
//! VBUS). The object surface is not here at all: both links speak protocol v4 into the one engine in
//! `crate::flat_store`.

pub(crate) mod command;
pub(crate) mod identity;

use core::cell::RefCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use obc_ble::StatusMessage;
use obc_link::flat::ObjectKind;

use crate::init_static;
use crate::object_store::ObjectStore;

/// The single [`ObjectStore`]: catalog, upload, download and revision semantics behind a `RefCell`
/// that every plane borrows synchronously, never across an `await`. The SD card and RRAM settings it
/// operates on live in [`crate::SharedStore`], locked per call and passed into each store method.
static mut STORE: MaybeUninit<RefCell<ObjectStore>> = MaybeUninit::uninit();

/// Size of the resident store, for the resource report. It is reported under the
/// `ble_object_store` name, which the pinned resource baseline uses.
pub(crate) const OBJECT_STORE_BYTES: usize = core::mem::size_of::<RefCell<ObjectStore>>();

#[inline(never)]
pub(crate) fn init_store(shared: &mut crate::SharedStore) -> &'static RefCell<ObjectStore> {
    /// The fully-wrapped initial value as a named constant: `ptr::write` of a constant lowers to a
    /// `.rodata`-to-slot memcpy, with no `RefCell<ObjectStore>`-sized stack value anywhere. The
    /// `declare_interior_mutable_const` lint warns that every use of such a const is a fresh copy
    /// that forgets mutations; here that copy-on-use is the mechanism, so the hazard cannot arise.
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: RefCell<ObjectStore> = RefCell::new(ObjectStore::EMPTY);
    let cell = unsafe { init_static(core::ptr::addr_of_mut!(STORE), INIT) };
    cell.borrow_mut().hydrate(shared);
    cell
}

/// The storage handles a link plane is composed with. They always travel together, so they are
/// handed over as one value rather than as three parallel parameters threaded through each
/// transport's spawn trampoline.
#[derive(Clone, Copy)]
pub(crate) struct LinkStores {
    /// The SD card + RRAM settings, behind the async mutex the ride loop shares. Locked per store
    /// call and released before the next channel `await`, so the map render interleaves.
    pub shared: &'static crate::SharedStoreMutex,
    pub objects: &'static RefCell<ObjectStore>,
    /// The boot mint pass's store-epoch outcome, the value the identity read serves. `None` means no
    /// mounted store. It is never re-derived by a plane, so a card swap cannot silently change what
    /// a plane reports.
    pub epoch: Option<u32>,
}

/// Whether a ride is recording, mirrored across the plane boundary: the ride loop pushes
/// `app.recording()` here each pass, and the `installFw` command handler reads it as the `busy`
/// gate, because that arm ends in a reboot. Defaults false; the ride loop seeds the real value on
/// its first pass. `Relaxed` is enough: every plane is a cooperative future on the one executor, and
/// a stale read is at worst one pass late.
static RECORDING: AtomicBool = AtomicBool::new(false);

/// Push the ride-recording state to the link planes; the ride loop calls it once per pass.
pub fn set_recording(recording: bool) {
    RECORDING.store(recording, Ordering::Relaxed);
}

/// Whether a ride is recording (the `installFw` `busy` gate).
pub(crate) fn recording() -> bool {
    RECORDING.load(Ordering::Relaxed)
}

/// The deepest stack use seen so far, in bytes, published by the status loop from its
/// [`stackmeter`](crate::stackmeter) paint scan and surfaced in the diagnostics blob, so a soak rig
/// can post the stack high-water without RTT. 0 = not measured yet.
static STACK_HIGH_WATER: AtomicU32 = AtomicU32::new(0);

/// Publish a new stack high-water peak (called by the ride loop when the mark grows).
pub fn publish_stack_high_water(bytes: usize) {
    STACK_HIGH_WATER.store(bytes as u32, Ordering::Relaxed);
}

// A map upload writes for minutes. The ride loop owns the `App` and is the only task that may touch
// it, and the USB data plane must not block on it, so progress crosses the plane boundary as plain
// atomics the ride loop reads once per pass and feeds through `App::set_map_transfer`. Nothing here
// is a queue: the value is a state, always re-readable, and a missed intermediate is simply a frame
// that showed the previous percentage.

/// The transfer phase, as a `u8` so it fits an atomic: 0 = idle, 1 = receiving, 2 = installed,
/// 3 = storage failure, 4 = damaged (CRC), 5 = not a readable map, 6 = a file of a volume set was
/// refused before it streamed.
static MAP_PHASE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// Kibibytes received so far, and the announced total. KiB rather than bytes so a 4 GiB map still
/// fits a `u32` with room, and finer than any bar 240 px can resolve.
static MAP_RX_KIB: AtomicU32 = AtomicU32::new(0);
static MAP_TOTAL_KIB: AtomicU32 = AtomicU32::new(0);

/// The engine's view of an upload, as a screen — called from `flat_store::serve` after every engine
/// call, on whichever link made it. One function rather than one per edge, because the engine is a
/// state machine that one execution context holds, so the honest shape is "here is everything that
/// is true now" rather than a sequence of notifications a plane has to remember to send.
///
/// The three inputs collapse to three outcomes: a verdict wins, whatever else is true, because it is
/// the terminal card; otherwise a live map upload is a progress bar; otherwise, if a bar is on the
/// glass and nothing is live, the transfer went away without a verdict — a pulled cable, a dropped
/// connection, a `CANCEL` — which clears the card rather than raising one.
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
                // no, a revision moved underneath, or the store is read-only.
                UploadEnd::Refused(_) => 5,
            },
            Ordering::Relaxed,
        );
        return;
    }
    match live {
        Some(progress) if progress.kind == ObjectKind::MapShard => {
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

/// The app-facing map-transfer state, or `None` when there is nothing to show. Read once per pass
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

/// Why a committed map failed the structure check that runs before its card says installed.
pub(crate) enum MapVerifyFault {
    /// The bytes do not parse as a map this firmware reads. Send a map this build agrees with.
    NotAMap,
    /// The card could not be read back. The map may be good; the medium is the suspect.
    Storage,
}

/// A committed map failed its structure check. Published right after [`publish_map_transfer`] has
/// stored the installed phase, so the terminal card becomes the failure instead of the success.
///
/// The map stays committed either way. The previous map is already gone: the publish frees it in the
/// same commit that writes the new one, so there is nothing to roll back to. The rider re-sends, and
/// a reboot before they do lands on MAP UNREADABLE with USB recovery running.
pub(crate) fn publish_map_verify_failure(fault: MapVerifyFault) {
    MAP_PHASE.store(
        match fault {
            MapVerifyFault::NotAMap => 5,
            MapVerifyFault::Storage => 3,
        },
        Ordering::Relaxed,
    );
}

/// Clear the map-transfer state when the rider dismisses the terminal card, so the ride loop's next
/// pass does not push it back.
///
/// Clears only a terminal state. The dismissal is observed a pass after it happened, so a fresh
/// transfer could have started in the gap, and clearing that would leave a multi-minute write with
/// no card and no way to raise one, because progress updates only touch the byte counters.
pub fn clear_map_transfer() {
    if MAP_PHASE.load(Ordering::Relaxed) >= 2 {
        MAP_PHASE.store(0, Ordering::Relaxed);
    }
}

/// A `status` message's bytes, ready to hand to a transport (`&buf[..len]`). Each plane keeps one
/// small stack buffer per message rather than a heapless alloc — every status message fits.
pub(crate) type StatusBytes = ([u8; StatusMessage::MAX_ENCODED_LEN], usize);
