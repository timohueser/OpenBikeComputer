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
//! state across a park and the uploader's next call flips it back. [`storage_session`] exists so
//! that async users announce that intent up front and wait out a live scan by yielding instead of
//! spinning; it is deliberately not a lock.
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

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use defmt::{error, warn};
use embassy_time::{Duration, Instant, Timer};

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

/// Poll interval for the async wait in [`storage_session`]. Fine enough to be invisible against a
/// 44 ms scan, coarse enough that a full frame costs about 88 wakes rather than pegging the executor
/// with `yield_now`.
const SCAN_POLL: Duration = Duration::from_micros(500);

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
/// sequentially, so this is free. A chunk landing during a push is the case it exists for, and
/// [`storage_session`] is the async front door that turns that spin into a yield.
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
            error!("flpr_mux: sEMMC would not take the hart ({}) — storage is down for this operation", e);
            MODE.store(Mode::Unknown as u8, Ordering::Relaxed);
            false
        }
        None => false,
    }
}

/// Run one synchronous storage operation with the FLPR in storage mode.
///
/// The single door the `BlockDevice` impl uses: it pairs the mode guarantee with the driver borrow,
/// so neither can be taken without the other.
pub fn with_storage<R>(f: impl FnOnce(&mut Semmc) -> R) -> Result<R, SemmcError> {
    if !ensure_storage() {
        return Err(SemmcError::NoBoot);
    }
    with_semmc(f).ok_or(SemmcError::NotInitialised)
}

/// The async front door for a batch of storage work, acquired by
/// [`SharedStoreMutex::lock`](crate::SharedStoreMutex), so every plane that reaches the card through
/// the shared store gets it for free.
///
/// It waits out a live scan by yielding, so a chunk arriving mid-frame costs the executor nothing
/// while the FLPR finishes drawing. It does not switch the mode — that is [`with_storage`]'s job,
/// lazily, at the point of use — and it is not a lock: holding it does not stop the map plane
/// pushing a frame. That is deliberate, because a session that blocked the panel would turn a
/// multi-megabyte upload into a frozen screen.
///
/// It deliberately does not switch eagerly. An eager switch charged every acquirer a 29 µs park and
/// warm boot, plus 138 µs to get the panel back on the next frame, whether or not it ever touched
/// the card — and `SharedStoreMutex::lock` has about 50 call sites, plenty of which never reach the
/// device. On a soft peripheral that will not boot it was a 500 ms boot-deadline spin per lock.
///
/// The scan wait is bounded by [`FRAME_DEADLINE`](crate::ls021_flpr::FRAME_DEADLINE), because no
/// wait in this subsystem is unbounded and this one is held inside the store mutex, so an FLPR that
/// never finishes a frame would wedge every plane that wants the card. On expiry it proceeds with a
/// log rather than failing: the session carries no capability, so proceeding only means it stops
/// yielding. Each operation's [`with_storage`] then applies the same bound synchronously.
pub async fn storage_session() -> StorageSession {
    let deadline = Instant::now() + crate::ls021_flpr::FRAME_DEADLINE;
    while crate::ls021_flpr::scan_in_flight() {
        if Instant::now() >= deadline {
            warn!(
                "flpr_mux: storage session waited out {=u64} ms of scan and gave up yielding — the operation's own guard takes it from here",
                crate::ls021_flpr::FRAME_DEADLINE.as_millis()
            );
            break;
        }
        Timer::after(SCAN_POLL).await;
    }
    StorageSession(())
}

/// The zero-sized marker [`storage_session`] hands back. It carries no capability — the mode is
/// re-checked at each operation — so dropping it early cannot make anything unsound. It exists to
/// make the intent visible at the call sites.
pub struct StorageSession(());

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
