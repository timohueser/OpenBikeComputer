//! **Who owns the FLPR right now** — the display/storage mode scheduler (epic #1158, §3 of #1145).
//!
//! There is one coprocessor and two soft-peripheral images that want it: the LS021 scan blob
//! ([`crate::ls021_flpr`]) and Nordic's sEMMC SD host ([`crate::semmc`]). Both are **resident** —
//! the display blob in its 4 KiB carve, the sEMMC image in its 20 KiB one — so a handover is not a
//! load, it is a park + a pad flip + a warm boot. Measured on glass 2026-08-06:
//!
//! | switch | cost |
//! | :-- | --: |
//! | → storage (park, pads → `CTRLSEL=VPR`, warm boot, power-on) | **29 µs** |
//! | → display (quiesce, park, pads → GPIO, blob relaunch, `ALIVE`) | **138 µs** |
//! | card state across a switch | stays `tran` + High-Speed, **zero** re-inits (12/12 rounds) |
//!
//! ## The design: lazy mode, one gate, no mutex
//!
//! The obvious shape is an async owner mutex, and it is the wrong one here — it would deadlock the
//! first time a task held a storage session across an `await` while the map plane wanted to push,
//! and it would buy nothing, because **the exclusion the hardware actually needs is one-directional
//! and much narrower than "who holds the FLPR"**:
//!
//! 1. *Never park mid-scan.* Between ringing the doorbell and the FLPR's ack, the panel is being
//!    drawn; parking there abandons a half-drawn frame. This is the only display-side window that
//!    matters — [`crate::ls021_flpr::scan_in_flight`] names it exactly, and every storage entry
//!    here waits it out.
//! 2. *Never switch mid-transfer.* Every sEMMC entry point — transfers **and** card bring-up — is
//!    **synchronous** and never yields, so on the one thread-mode executor no other task can even
//!    run while one is in flight, and the `&mut Semmc` borrow enforces the rest. PR #1160 shipped
//!    `Semmc::start` as `async` and stated the contract that came with it (*hold the mode across the
//!    entire `start().await`*); this PR made it synchronous instead, so that contract is now
//!    satisfied by there being no yield to hold across. The reasons — the COM task it was written to
//!    protect runs on a **preempting** P3 executor, and the coroutine cost 6.4 KB of permanent
//!    `main` task frame — are on `Semmc::start`.
//! 3. *The mode must match the work.* Ensured lazily at the point of use:
//!    [`ensure_display`] from the display side's one push funnel, [`ensure_storage`] from the
//!    `BlockDevice` seam.
//!
//! With those three, mutual exclusion falls out of the executor's cooperative scheduling and no
//! lock is needed at all — which is also why nothing here can deadlock.
//!
//! ## The hold policy
//!
//! **A hold is one synchronous burst long, and is never held across an `await`.** The mode is
//! *lazy*: neither side switches back when it is done, so a run of storage operations pays 29 µs
//! once, and a run of frames pays 138 µs once. A frame interleaved between two storage bursts costs
//! both — 167 µs against a 44 ms full-frame push, **0.4 %**, and less than half of one 430 µs
//! single-block read.
//!
//! That is what keeps the panel alive during a long transfer: a BLE or USB upload does not *own*
//! storage for the transfer, it owns it for each chunk's synchronous FAT call. Between chunks the
//! map plane pushes a frame, flipping the mode out from under the uploader — harmlessly, because
//! the card keeps its `tran` + High-Speed state across a park and the uploader's next call flips it
//! back. [`storage_session`] exists so that the *async* users announce that intent up front and
//! wait for a live scan by yielding instead of spinning; it is deliberately **not** a lock.
//!
//! **COM is not in this file and must never be.** The panel's anti-DC-bias square wave runs on the
//! M33 (`com::com_task` on the P3 executor) and free-runs through both modes, through a wedged
//! FLPR, and through every handover — the glass just holds its last image. That property is
//! load-bearing; it is why a storage burst can take the coprocessor at all.
//!
//! ## Reconciling the two park recipes
//!
//! Two different sequences stop the hart, and this module picks deliberately:
//!
//! - [`semmc::park_hart`](crate::semmc::park_hart) — Nordic's `nrf_semmc_uninit` shape: `CPURUN = 0`
//!   then a pulsed `ndmreset | dmactive` → `dmactive` → `0`. **This is the routine switch recipe,
//!   both directions**, because it is the one the 29 µs / 138 µs numbers were measured with, at
//!   switch cadence, 12/12 rounds. Both `semmc::enter_storage_mode` and `semmc::leave_storage_mode`
//!   call it internally, so a switch through this module always uses it.
//! - [`ls021_flpr::relaunch_flpr`](crate::ls021_flpr::relaunch_flpr) — the #349 escalation: DM
//!   `haltreq`, wait for `DMSTATUS.allhalted` (≤10 ms), *then* `ndmreset`. **Kept for recovery
//!   only.** Its extra courtesy is a clean instruction boundary, which is worth 10 ms when the
//!   question is "why is this core wedged" and worth nothing when the answer is already known. The
//!   force-stop capability is unchanged and still reachable — a wedged FLPR is still halted through
//!   its Debug Module, not through `CPURUN` (which does not stop a busy-polling core).
//!
//! Both are a park; only one has been measured at frame cadence, and that is the one the fast path
//! uses.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use defmt::{error, warn};
use embassy_time::{Duration, Timer};

use crate::semmc::{self, CardInfo, Semmc, SemmcError};

/// Which image has the hart.
#[derive(Clone, Copy, PartialEq, Eq, defmt::Format)]
#[repr(u8)]
enum Mode {
    /// Neither, yet — before the display blob's first launch. Reached only during boot.
    Unknown = 0,
    /// The LS021 scan blob.
    Display = 1,
    /// Nordic's sEMMC soft peripheral.
    Storage = 2,
}

impl Mode {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Mode::Display,
            2 => Mode::Storage,
            _ => Mode::Unknown,
        }
    }
}

static MODE: AtomicU8 = AtomicU8::new(Mode::Unknown as u8);

/// Guards the one `&mut Semmc`. The driver is not re-entrant (a transfer is a synchronous state
/// machine over one register block), and nothing in the design re-enters it — this is the assertion
/// that says so, not a lock.
static SEMMC_BUSY: AtomicBool = AtomicBool::new(false);

/// The host driver. A `static mut` rather than a `RefCell` in a `Mutex` because the access rule is
/// structural, not dynamic: every user is a synchronous call on the one thread-mode executor and no
/// ISR touches it ([`semmc::on_vpr00_irq`] only stores one atomic). [`with_semmc`] is the sole
/// door, and it carries the re-entrancy assertion.
static mut SEMMC: Semmc = Semmc::new();

fn mode() -> Mode {
    Mode::from_u8(MODE.load(Ordering::Relaxed))
}

/// Poll interval for the async wait in [`storage_session`]. Fine enough to be invisible against a
/// 44 ms scan (≈1 % of one), coarse enough that a full frame costs ~88 wakes rather than pegging the
/// executor with `yield_now`.
const SCAN_POLL: Duration = Duration::from_micros(500);

/// Borrow the sEMMC driver for one synchronous operation.
///
/// Returns `None` if it is already borrowed — which cannot happen by construction (see
/// [`SEMMC_BUSY`]) and is reported rather than silently aliased if the construction ever changes.
fn with_semmc<R>(f: impl FnOnce(&mut Semmc) -> R) -> Option<R> {
    if SEMMC_BUSY.swap(true, Ordering::Acquire) {
        error!("flpr_mux: re-entrant sEMMC borrow — the storage transport is not re-entrant");
        return None;
    }
    // SAFETY: `SEMMC_BUSY` was clear, so no other borrow is live; every caller is a synchronous
    // call on the one thread-mode executor and no interrupt handler touches this static.
    let r = f(unsafe { &mut *core::ptr::addr_of_mut!(SEMMC) });
    SEMMC_BUSY.store(false, Ordering::Release);
    Some(r)
}

/// **Take the FLPR for the display**, if it does not already have it. Called from
/// [`Ls021Flpr::ring_spans`](crate::ls021_flpr::Ls021Flpr) — every push, both the async and the
/// blocking one — so this is the storage→display half of the mux and the only place the display
/// side knows the mux exists.
///
/// Synchronous, because the overlay push is (its ~9 KB composite scratch must stay a stack
/// transient, #347) — see [`crate::ls021_flpr::launch_flpr_blocking`].
///
/// A failed relaunch is logged and returns anyway: the caller then rings a dead core, the ack times
/// out, and `MapDisplay`'s existing escalation runs a full [`relaunch_flpr`](crate::ls021_flpr::relaunch_flpr).
/// Growing a second recovery ladder here would only give the two ways to disagree.
pub fn ensure_display() {
    if mode() == Mode::Display {
        return;
    }
    quiesce_storage_if_active();
    // **Park unconditionally, not only when the mode says `Storage`.** `Mode::Unknown` does not mean
    // "the hart is idle" — it is also where a *failed* bring-up leaves things, with the sEMMC image
    // copied into its carve and quite possibly running (`Semmc::start` boots the firmware before it
    // ever talks to a card, so a `NoCard` return leaves a live coprocessor behind). Re-copying the
    // display blob and writing `INITPC`/`CPURUN` under a running core is undefined; a second park
    // costs microseconds and is idempotent. The pad flip likewise: it restores exactly what `main`
    // claimed for the two shared pads and parks the four card-only ones as inputs.
    semmc::park_hart();
    semmc::configure_display_pads();
    match crate::ls021_flpr::launch_flpr_blocking() {
        Ok(()) => MODE.store(Mode::Display as u8, Ordering::Relaxed),
        Err(e) => error!("flpr_mux: display blob did not relaunch ({}) — the push will time out", e),
    }
}

/// Hand the sEMMC peripheral back if it holds the hart: latched completions cleared, the shared
/// `VPR00` interrupt gate disarmed, the hart parked, the pads returned to the display map.
///
/// Idempotent and safe to call in any mode — a no-op unless storage is actually live. Used by
/// [`ensure_display`] and by [`relaunch_flpr`](crate::ls021_flpr::relaunch_flpr), whose Debug-Module
/// halt should never land on a coprocessor the M33 is still mid-conversation with.
pub fn quiesce_storage_if_active() {
    if mode() != Mode::Storage {
        return;
    }
    with_semmc(|sd| sd.leave_storage_mode());
    MODE.store(Mode::Unknown as u8, Ordering::Relaxed);
}

/// **Take the FLPR for storage**, if it does not already have it — the synchronous seam every
/// `BlockDevice` operation goes through.
///
/// Waits out a live scan first ([`wait_scan_settled`](crate::ls021_flpr::wait_scan_settled)): rule
/// 1, *never park mid-scan*. In the map plane's own storage phase there is never a scan in flight
/// (that task renders and presents sequentially), so this is free; a BLE/USB chunk landing during a
/// push is the case it exists for, and [`storage_session`] is the async front door that turns that
/// spin into a yield.
///
/// Returns whether storage has the hart. `false` means the sEMMC firmware would not boot — the
/// transport then fails the operation rather than clocking a bus that is not there.
pub fn ensure_storage() -> bool {
    if mode() == Mode::Storage {
        return true;
    }
    crate::ls021_flpr::wait_scan_settled();
    match with_semmc(|sd| sd.enter_storage_mode()) {
        Some(Ok(())) => {
            MODE.store(Mode::Storage as u8, Ordering::Relaxed);
            true
        }
        Some(Err(e)) => {
            error!("flpr_mux: sEMMC would not take the hart ({}) — storage is down for this operation", e);
            MODE.store(Mode::Unknown as u8, Ordering::Relaxed);
            false
        }
        None => false,
    }
}

/// Run one synchronous storage operation with the FLPR in storage mode.
///
/// The single door the `BlockDevice` impl uses: it pairs the mode guarantee with the driver borrow
/// so neither can be taken without the other.
pub fn with_storage<R>(f: impl FnOnce(&mut Semmc) -> R) -> Result<R, SemmcError> {
    if !ensure_storage() {
        return Err(SemmcError::NoBoot);
    }
    with_semmc(f).ok_or(SemmcError::NotInitialised)
}

/// **The async front door for a batch of storage work** — acquired by
/// [`SharedStoreMutex::lock`](crate::SharedStoreMutex), so every plane that reaches the card
/// through the shared store gets it for free and no call site changed.
///
/// What it does, and does not do:
///
/// - it **waits out a live scan by yielding**, so a BLE or USB chunk arriving mid-frame costs the
///   executor nothing while the FLPR finishes drawing (the synchronous [`ensure_storage`] would
///   spin for the same wall-clock time);
/// - it puts the FLPR in storage mode up front, so the burst's first block read does not pay a mode
///   check inside the FAT layer;
/// - it is **not a lock**. Holding it does not stop the map plane pushing a frame — see the hold
///   policy in this module's docs. That is deliberate: a session that blocked the panel would turn
///   a multi-megabyte upload into a frozen screen.
pub async fn storage_session() -> StorageSession {
    while crate::ls021_flpr::scan_in_flight() {
        Timer::after(SCAN_POLL).await;
    }
    ensure_storage();
    StorageSession(())
}

/// The (zero-sized) marker [`storage_session`] hands back. Carries no capability — the mode is
/// re-checked at each operation — so dropping it early cannot make anything unsound; it exists to
/// make the intent visible at the call sites and to give the guard a place to grow if the policy
/// ever needs one.
pub struct StorageSession(());

/// **Bring the card up.** Runs once per power-on, from `main`, before any other plane exists.
///
/// PR #1160 shipped `Semmc::start` as `async` with a contract attached: *the mode must be held
/// across the entire `start().await`*, because it yielded three times with the sEMMC image live and,
/// at the CMD8 deliver-and-abort, a command actually on the wire. **That contract is now
/// structural**: `start` is synchronous, so there is no yield to hand the coprocessor away at, and
/// this whole function is one uninterruptible stretch on the one thread-mode executor. It is also
/// called from `main` before the ride loop, the BLE stack and the USB plane exist, so there is
/// nothing that *could* want the FLPR meanwhile.
pub fn bring_up_storage() -> Result<CardInfo, SemmcError> {
    if !crate::ls021_flpr::wait_scan_settled() {
        warn!("flpr_mux: bringing storage up with the panel mid-scan — the frame is lost");
    }
    let r = with_semmc(|sd| sd.start()).unwrap_or(Err(SemmcError::NotInitialised));
    MODE.store(if r.is_ok() { Mode::Storage as u8 } else { Mode::Unknown as u8 }, Ordering::Relaxed);
    r
}

/// Record that the display blob is live and owns the hart — called by
/// [`launch_flpr`](crate::ls021_flpr::launch_flpr) on a successful `ALIVE` stamp, so the boot launch
/// and the #349 recovery relaunch both publish the mode without the display path having to know
/// about this module beyond its one push-funnel call.
pub fn note_display_live() {
    MODE.store(Mode::Display as u8, Ordering::Relaxed);
}

/// The mode, for the boot log line.
pub fn mode_name() -> &'static str {
    match mode() {
        Mode::Unknown => "none",
        Mode::Display => "display",
        Mode::Storage => "storage",
    }
}
