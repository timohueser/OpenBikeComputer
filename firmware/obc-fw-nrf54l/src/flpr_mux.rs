//! Who owns the FLPR right now: the display and storage mode scheduler.
//!
//! There is one coprocessor and two soft-peripheral images that want it: the LS021 scan blob
//! ([`crate::ls021_flpr`]) and Nordic's sEMMC SD host ([`crate::semmc`]). Both images stay resident,
//! so a handover is not a load: it is a park, a pad flip and a warm boot. Measured on glass:
//!
//! | switch | cost |
//! | :-- | --: |
//! | to storage (park, pads to `CTRLSEL=VPR`, warm boot, power-on) | 29 µs |
//! | to display (quiesce, park, pads to GPIO, blob relaunch, `ALIVE`) | 138 µs |
//! | card state across a switch | stays `tran` and High-Speed, with no re-init |
//!
//! ## Lazy mode, one gate, no mutex
//!
//! An async owner mutex would deadlock the first time a task held a storage session across an
//! `await` while the map plane wanted to push, and it would buy nothing, because the exclusion the
//! hardware needs is much narrower than "who holds the FLPR":
//!
//! 1. Never park mid-scan. Between ringing the doorbell and the FLPR's ack the panel is being drawn,
//!    and parking there abandons a half-drawn frame. [`crate::ls021_flpr::scan_in_flight`] names
//!    that window exactly, and every storage entry here waits it out.
//! 2. Never switch mid-transfer. Every sEMMC entry point, transfers and card bring-up alike, is
//!    synchronous and never yields, so on the one thread-mode executor no other task can run while
//!    one is in flight, and the `&mut Semmc` borrow enforces the rest.
//! 3. The mode must match the work, ensured lazily at the point of use: [`ensure_display`] from the
//!    display side's one push funnel, [`ensure_storage`] from the `BlockDevice` seam.
//!
//! With those three, mutual exclusion falls out of the executor's cooperative scheduling, no lock is
//! needed, and nothing here can deadlock.
//!
//! ## The hold policy
//!
//! A hold is one synchronous burst long and is never held across an `await`. The mode is lazy:
//! neither side switches back when it is done, so a run of storage operations pays 29 µs once and a
//! run of frames pays 138 µs once. A frame between two storage bursts costs both, 167 µs against a
//! 44 ms full-frame push.
//!
//! That is what keeps the panel alive during a long transfer: an upload does not own storage for the
//! transfer, it owns it for each chunk's synchronous call. Between chunks the map plane pushes a
//! frame and flips the mode out from under the uploader, harmlessly, because the card keeps its
//! state across a park and the uploader's next call flips it back.
//!
//! COM is not in this file and must never be. The panel's anti-DC-bias square wave runs on the M33
//! and free-runs through both modes, through a wedged FLPR and through every handover, so the glass
//! holds its last image. That property is what lets a storage burst take the coprocessor at all.
//!
//! ## The two park recipes
//!
//! [`semmc::park_hart`](crate::semmc::park_hart) is the routine switch recipe in both directions,
//! because it is the one the 29 µs and 138 µs numbers were measured with, at switch cadence. Both
//! `semmc::enter_storage_mode` and `semmc::leave_storage_mode` call it internally.
//!
//! [`ls021_flpr::relaunch_flpr`](crate::ls021_flpr::relaunch_flpr) halts through the Debug Module
//! and waits for `DMSTATUS.allhalted` before resetting. It is kept for recovery only: its extra
//! courtesy is a clean instruction boundary, which is worth 10 ms when the question is why the core
//! is wedged and worth nothing when the answer is known. A wedged FLPR is still halted through its
//! Debug Module and not through `CPURUN`, which does not stop a busy-polling core.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use defmt::{error, warn};
use embassy_time::Instant;

use obc_storage::health::{Breaker, Outcome};

use crate::semmc::{self, CardInfo, Semmc, SemmcError};

/// Which image has the hart.
#[derive(Clone, Copy, PartialEq, Eq, defmt::Format)]
#[repr(u8)]
enum Mode {
    /// Neither, yet: before the display blob's first launch. Reached only during boot.
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

/// Guards the one `&mut Semmc`. The driver is not re-entrant — a transfer is a synchronous state
/// machine over one register block — and nothing in the design re-enters it. This is the assertion
/// that says so, not a lock.
static SEMMC_BUSY: AtomicBool = AtomicBool::new(false);

/// The host driver. A `static mut` rather than a `RefCell` in a `Mutex`, because the access rule is
/// structural and not dynamic: every user is a synchronous call on the one thread-mode executor and
/// no ISR touches it. [`with_semmc`] is the sole door, and it carries the re-entrancy assertion.
static mut SEMMC: Semmc = Semmc::new();

fn mode() -> Mode {
    Mode::from_u8(MODE.load(Ordering::Relaxed))
}

/// Borrow the sEMMC driver for one synchronous operation.
///
/// Returns `None` if it is already borrowed, which cannot happen by construction and is reported
/// rather than silently aliased if the construction ever changes.
fn with_semmc<R>(f: impl FnOnce(&mut Semmc) -> R) -> Option<R> {
    if SEMMC_BUSY.swap(true, Ordering::Acquire) {
        error!("flpr_mux: re-entrant sEMMC borrow — the storage transport is not re-entrant");
        return None;
    }
    // SAFETY: `SEMMC_BUSY` was clear, so no other borrow is live. Every caller is a synchronous call
    // on the one thread-mode executor, and no interrupt handler touches this static.
    let r = f(unsafe { &mut *core::ptr::addr_of_mut!(SEMMC) });
    SEMMC_BUSY.store(false, Ordering::Release);
    Some(r)
}

/// Take the FLPR for the display, if it does not already have it. Every push calls it, so this is
/// the storage-to-display half of the mux and the only place the display side knows the mux exists.
///
/// Synchronous, because the overlay push is: its composite scratch must stay a stack transient.
///
/// A failed relaunch is logged and returns anyway. The caller then rings a dead core, the ack times
/// out, and `MapDisplay`'s existing escalation runs a full relaunch; a second recovery ladder here
/// would only give the two ways to disagree.
pub fn ensure_display() {
    if mode() == Mode::Display {
        return;
    }
    quiesce_storage_if_active();
    // Park unconditionally, not only when the mode says `Storage`. `Mode::Unknown` does not mean the
    // hart is idle: it is also where a failed bring-up leaves things, with the sEMMC image copied
    // into its carve and quite possibly running, because `Semmc::start` boots the firmware before it
    // ever talks to a card. Re-copying the display blob and writing `INITPC` under a running core is
    // undefined; a second park costs microseconds and is idempotent.
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
/// Idempotent and safe to call in any mode, and a no-op only when the display already holds the
/// hart. It is used by [`ensure_display`] and by the recovery relaunch, whose Debug-Module halt
/// should never land on a coprocessor the M33 is still mid-conversation with.
///
/// It is gated on `!= Display`, not `== Storage`, and that distinction is load-bearing.
/// `Semmc::start` arms the shared `VPR00` gate inside `boot_firmware`, before `enable` or
/// `init_card` can fail, so a failed bring-up leaves `Mode::Unknown` with the completion gate armed
/// and possibly a latched event. Skipping the quiesce there would let [`ensure_display`]'s
/// unconditional park launch the display blob with the sEMMC gate still live, and a completion event
/// left pending would fire `on_vpr00_irq` against a peripheral that no longer exists.
pub fn quiesce_storage_if_active() {
    if mode() == Mode::Display {
        return;
    }
    with_semmc(|sd| sd.leave_storage_mode());
    MODE.store(Mode::Unknown as u8, Ordering::Relaxed);
}

/// Take the FLPR for storage, if it does not already have it: the synchronous seam every
/// `BlockDevice` operation goes through.
///
/// It waits out a live scan first, which is the never-park-mid-scan rule. In the map plane's own
/// storage phase there is never a scan in flight, because that task renders and presents
/// sequentially, so this is free. A chunk landing during a push is the case it exists for.
///
/// Returns whether storage has the hart. `false` means the sEMMC firmware would not boot, and the
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
            error!("flpr_mux: sEMMC would not take the hart ({}) — this operation fails", e);
            MODE.store(Mode::Unknown as u8, Ordering::Relaxed);
            false
        }
        None => false,
    }
}

/// The transport's health, as [`Breaker`] stores it: the consecutive-fault count and when the
/// breaker last opened. Two words beside the driver, under the same access rule as everything else
/// here, but atomics because [`storage_latched`] reads them from the ride loop between operations.
static FAILURES: AtomicU8 = AtomicU8::new(0);
static OPENED_AT_MS: AtomicU32 = AtomicU32::new(0);

// One dedicated recovery pass: the panel's declared scan deadline, Default-Speed re-identification,
// and four successful single-attempt reads (two superblocks and two catalog gates). The 24 s value
// is the board watchdog contract also checked by `obc_storage::health`.
const RECOVERY_SUCCESS_BOUND_MS: u64 = crate::ls021_flpr::FRAME_DEADLINE.as_millis()
    + crate::semmc::RECOVERY_REIDENTIFY_BOUND_MS as u64
    + 4 * crate::semmc::RECOVERY_BLOCK_READ_BOUND_MS as u64;
/// Do not start another identity command after this point in the dedicated pass. A worst failed
/// command then ends by 21.7 s, leaving 2.3 s for loop and interrupt overhead before the watchdog.
pub const RECOVERY_READ_START_CUTOFF_MS: u64 = 18_000;
const _: () = assert!(RECOVERY_SUCCESS_BOUND_MS < 24_000);
const _: () = assert!(RECOVERY_READ_START_CUTOFF_MS + (crate::semmc::RECOVERY_BLOCK_FAILURE_BOUND_MS as u64) < 24_000);

fn breaker() -> Breaker {
    Breaker::restore(FAILURES.load(Ordering::Relaxed), OPENED_AT_MS.load(Ordering::Relaxed))
}

fn store_breaker(b: Breaker) {
    FAILURES.store(b.failures(), Ordering::Relaxed);
    OPENED_AT_MS.store(b.opened_at_ms(), Ordering::Relaxed);
}

/// Whether the transport has given up on the card, for the one warning the ride loop raises. It
/// stays true through a cool-down and clears when a probe finds the card working again.
pub fn storage_latched() -> bool {
    breaker().open()
}

/// Whether a background read may become the transport's next operation.
///
/// It stays false for the complete latch, including the half-open edge. Only the dedicated ride
/// recovery pass may probe after the cool-down.
pub fn storage_admitted() -> bool {
    breaker().background_admitted()
}

/// Whether a watchdog-fed ride pass must be reserved for one recovery probe.
pub fn storage_recovery_due(watchdog_fed: bool) -> bool {
    let b = breaker();
    b.recovery_due(watchdog_fed, Instant::now().as_millis() as u32)
}

/// Re-identify the card and validate the retained store inside its dedicated ride pass.
///
/// Success clears the breaker only after `validate` accepts the media. Every failure restarts the
/// cool-down. The caller must end the pass either way, so no retained operation follows this work
/// before the watchdog is fed again.
pub fn recover_storage(validate: impl FnOnce(&mut Semmc, Instant) -> Result<(), SemmcError>) -> Result<(), SemmcError> {
    let before = breaker();
    if !before.open() || !before.admits(Instant::now().as_millis() as u32) {
        return Err(SemmcError::Unhealthy);
    }

    let started = Instant::now();
    crate::ls021_flpr::wait_scan_settled();
    let result = with_semmc(|sd| {
        sd.reidentify()?;
        validate(sd, started)
    })
    .unwrap_or(Err(SemmcError::NotInitialised));

    let after = before.record(
        if result.is_ok() { Outcome::Success } else { Outcome::TransportFault },
        Instant::now().as_millis() as u32,
    );
    store_breaker(after);
    match result {
        Ok(()) => {
            MODE.store(Mode::Storage as u8, Ordering::Relaxed);
            warn!("flpr_mux: the card and retained store match — the transport is back");
        }
        Err(error) => {
            MODE.store(Mode::Unknown as u8, Ordering::Relaxed);
            warn!("flpr_mux: storage recovery failed ({}) — cool-down restarted", error);
        }
    }
    result
}

/// Run one synchronous storage operation with the FLPR in storage mode.
///
/// The single door the `BlockDevice` impl uses: it pairs the mode guarantee with the driver borrow,
/// so neither can be taken without the other, and it is the one place every card operation's
/// outcome is visible, so it is where the [`Breaker`] lives.
///
/// The closure returns a `Result` because the breaker must see the transfer's verdict and not only
/// whether the mode switch worked. A dead card and a soft peripheral that stopped booting are the
/// same fact to a rider — storage is gone — so they share one latch. A caller-side refusal
/// ([`SemmcError::is_transport_fault`]) is weighed as [`Outcome::Refused`] and moves nothing.
///
/// While the breaker is open every call returns [`SemmcError::Unhealthy`] at once, without a mode
/// switch, a boot attempt or a bus cycle. The ride loop owns the one recovery probe after each
/// cool-down, in a pass that cannot also run an ordinary operation.
pub fn with_storage<R>(f: impl FnOnce(&mut Semmc) -> Result<R, SemmcError>) -> Result<R, SemmcError> {
    let before = breaker();
    if before.open() {
        return Err(SemmcError::Unhealthy);
    }
    let ready = ensure_storage();
    let r = if ready { with_semmc(f).unwrap_or(Err(SemmcError::NotInitialised)) } else { Err(SemmcError::NoBoot) };
    let outcome = match &r {
        Ok(_) => Outcome::Success,
        Err(e) if e.is_transport_fault() => Outcome::TransportFault,
        Err(_) => Outcome::Refused,
    };
    // The clock is read again here, not reused from the admission check: a failed operation can
    // have spent seconds of its deadline ladder, and the cool-down starts when it ended.
    let after = before.record(outcome, Instant::now().as_millis() as u32);
    store_breaker(after);
    if after.opened_from(before) {
        error!(
            "flpr_mux: {=u8} storage operations failed in a row — the transport is off until a probe {=u32} s from now finds the card again",
            Breaker::LIMIT,
            Breaker::COOL_DOWN_MS / 1000
        );
    }
    r
}

/// Bring the card up. It runs once per power-on, from `main`, before any other plane exists.
///
/// The mode must be held across the whole of `Semmc::start`, and that is structural rather than a
/// convention: `start` is synchronous, so there is no yield to hand the coprocessor away at, and
/// this whole function is one uninterruptible stretch on the one thread-mode executor.
pub fn bring_up_storage() -> Result<CardInfo, SemmcError> {
    if !crate::ls021_flpr::wait_scan_settled() {
        warn!("flpr_mux: bringing storage up with the panel mid-scan — the frame is lost");
    }
    let r = with_semmc(|sd| sd.start()).unwrap_or(Err(SemmcError::NotInitialised));
    MODE.store(if r.is_ok() { Mode::Storage as u8 } else { Mode::Unknown as u8 }, Ordering::Relaxed);
    r
}

/// Record that the display blob is live and owns the hart. The successful `ALIVE` stamp calls it,
/// so the boot launch and the recovery relaunch both publish the mode without the display path
/// having to know about this module beyond its one push-funnel call.
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
