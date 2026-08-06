//! **The LS021B7DD02 FLPR display backend, driving the real app's map/ride render.**
//!
//! `main.rs` runs the real [`obc_app::App`](obc_app) on the reflective LS021 panel through the
//! generic display contracts (`obc_display::display_contracts`): the board's composition edge
//! pairs [`Frame64`] (the resident device-64 [`Device64Frame`]) with this crate's [`Ls021Flpr`]
//! presenter — no panel-specific code in the map plane's callers. The simulator presenter is the
//! contracts' other live backend.
//!
//! The contract impls ([`Presenter`] / [`OverlayPresenter`]) live at the bottom of this file — thin
//! adapters over the panel methods below, so the whole LS021 backend is one module.
//!
//! What lives here is everything that talks to the **FLPR** (the nRF54L15's VPR RISC-V coprocessor):
//!   - the cross-core [`Control`] block + the dirty-row span list (**contract v2**, issue #347 — the
//!     normative contract with the C blob `src/flpr/flpr_scan.c`, kept byte-for-byte in sync, both
//!     static-assert 96 bytes);
//!   - [`launch_flpr`] — copy the blob into FLPR RAM, arm the control block, release the core, and
//!     wait for its `ALIVE` stamp;
//!   - [`Ls021Flpr`] — the resident-framebuffer backend. The FLPR **scans the framebuffer
//!     directly** (`fb_addr` in the control block): a push publishes the span list + rings one
//!     masked `CMD_RUN_FRAME`, and the FLPR packs each dirty row to the wire itself, straight from
//!     the resident device-64 plane ([`push_spans`](Ls021Flpr::push_spans) = only the listed rows,
//!     the FLPR fast-forwarding the gate over the rest; [`push_frame`](Ls021Flpr::push_frame) = the
//!     whole frame, the degenerate one-span case).
//!     The contracts' [`Presenter::present`] is the **self-diffing present**: it derives those
//!     spans automatically from a per-row hash of the last-pushed frame, so an idle redraw pushes
//!     only the rows that actually changed.
//!
//! The contract adapters (present / present_overlay) are folded in at the bottom of this module;
//! the rest owns the FLPR transport they call into.
//!
//! **COM stays on the M33** (`com::com_task`) and **is not here** — if the FLPR ever faults, COM must
//! keep alternating so the panel never takes a DC bias. The caller owns COM + the high-priority
//! `InterruptExecutor` it free-runs on.
//!
//! ## Whole-frame render, masked push
//!
//! The app still **renders** the whole frame into the resident RGB222 plane each redraw — the screens
//! are immediate-mode, they `clear()` and redraw (the map path writes device-64
//! ([`FbDevice64`](obc_display::FbDevice64)) straight into [`Frame64`]'s bytes). What the
//! self-diffing **present** then changes is the *push*: a single `CMD_RUN_FRAME` masked to the changed
//! rows, the FLPR fast-forwarding its gate scan over the unchanged ones and early-stopping after the
//! last. Render CPU is unchanged (a full draw), but the push — the dominant ~44 ms cost (#348; ~97 ms before its timing pass) — scales to
//! the changed-row span. A full redraw is just the degenerate "every row changed" case.
//!
//! ## Why the FLPR reads the framebuffer, not a hand-off buffer
//!
//! The retired ping-pong write buffers (F4, `flpr_pingpong.c`) predated the resident framebuffer:
//! with no fb to read, the M33 packed every row into two shared buffers under a per-row
//! `ready`/`consumed` handshake, busy-polling for the whole frame. The resident fb made that
//! moot: it is a stable byte-per-pixel plane in shared SRAM the FLPR can read, and the map plane
//! owns it for the duration of a push anyway (it presents, then renders — never both at once). So
//! the M33's only per-frame work is publishing `fb_addr` + the span list and ringing the doorbell —
//! the pack (~20 RISC-V integer ops/word, ported from the host-tested
//! [`ls021::wire`](obc_display::ls021::wire)) rides inside the panel's mandatory data-setup windows
//! on the FLPR, where the old blob just busy-spun.
//!
//! ## Async push — the M33 is freed for the whole frame
//!
//! The push is **async**: [`push_frame`](Ls021Flpr::push_frame) / [`push_spans`](Ls021Flpr::push_spans)
//! ring the doorbell and **`await` the FLPR's EGU20 frame ack** (the blob pokes
//! `EGU20.TASKS_TRIGGER[0]` after every frame; [`launch_flpr`] arms its `TRIGGERED[0]` IRQ, whose
//! ISR signals [`FRAME_ACK`]). So the ~44 ms a full frame takes on the wire costs the M33
//! **nothing**: thread mode runs other futures (SD I/O, sensor ticks, the next frame's prep) while
//! the FLPR scans. The wait is bounded by [`FRAME_DEADLINE`]; a timeout returns `false` exactly
//! like a transport fault — the caller keeps the last frame, re-arms the diff, and retries (and
//! #349's relaunch escalation builds on this hook).
//!
//! The one exception is the **overlay push, which deliberately blocks**
//! ([`run_spans_blocking`](Ls021Flpr::run_spans_blocking)): its ~9 KB composite/save scratch must
//! stay a stack transient — alive across an `await` it becomes task-future state in a *static*,
//! permanently shrinking the residual stack (the on-glass boot HardFault that taught this).
//!
//! ## The FLPR is shared with storage (epic #1158)
//!
//! Since the storage pivot the same coprocessor also runs Nordic's sEMMC soft peripheral — the two
//! images are **time-multiplexed**, and [`crate::flpr_mux`] owns which one has the hart. Two seams
//! reach out of this module into it:
//!
//! - every push funnels through [`ring_spans`](Ls021Flpr::ring_spans), which calls
//!   [`flpr_mux::ensure_display`](crate::flpr_mux::ensure_display) first — so a push after a
//!   storage burst pays the measured 138 µs to take the hart back and nothing else in the display
//!   path has to know the mux exists;
//! - a scan **in flight** must never be parked ([`scan_in_flight`] / [`wait_scan_settled`]) — that
//!   is the one window in which handing the FLPR to storage would abandon a half-drawn frame. The
//!   flag is a *sequence*, not a boolean, so a late ack of the previous frame cannot clear the
//!   current one's.

use core::ptr::{addr_of, addr_of_mut};
use core::sync::atomic::{AtomicU32, Ordering};

use defmt::{debug, error, info};
// The `#[interrupt]` attribute + the `EGU20` vector both live under this name (the module carries
// the interrupt enum, the macro registers the handler) — the same import `main.rs` uses for SWI01.
use embassy_nrf::interrupt;
use embassy_nrf::interrupt::{InterruptExt, Priority};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{with_deadline, Duration, Instant, Timer};
// The panel width the wire pack is defined over — the C blob's pack is the line-for-line port of
// `ls021_wire::pack_pair`/`pack_row` (the host-tested normative reference), so the seam geometry is
// pinned to it below.
use obc_display::ls021::wire::WIDTH;
// The generic display contracts this backend pairs `Frame64` with, and the LS021 pairing's shared
// substance: `RowDiff` (the self-diffing present store — a per-row hash of the last-pushed frame so
// a present pushes only the rows that actually changed), the `RowDamage`/`RowWindow` vocabulary,
// and `composite_into_resident` (the shared save→composite→push→restore overlay engine — the FLPR
// scans the resident frame directly, so the composited window must transiently live in it).
use obc_display::display_contracts::{Device64Frame, OverlayPresenter, PresentStats, Presenter};
use obc_display::ls021::{composite_into_resident, OverlayScratch, RowDamage, RowDiff, RowWindow};
use obc_display::Band;

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

/// The FLPR program image, cross-compiled by `build.rs` into `$OUT_DIR/flpr.bin`.
static FLPR_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flpr.bin"));

// ── The M33↔FLPR cross-core contract (issue #346): the shared addresses, magic/status stamps,
// command codes, and the span cap are **generated by build.rs** (its `contract` module is the single
// definition site — it also derives the carved memory.x and the FLPR's flpr.ld from the same values,
// so no side can fork). This include! splices `FLPR_RAM_BASE`, `CONTROL_ADDR`, `MAX_DIRTY_SPANS`,
// `LAYOUT_MAGIC`, `FLPR_ALIVE`, `FLPR_BADMAG`, and `CMD_RUN_FRAME` in here; the C blob includes the
// equivalent generated `flpr_contract.h`. The production carve-out is **8 KB**: the blob is ~820 B +
// a shallow leaf stack (a 4 KB `FLPR_RAM`), so the M33 links 248 KB; `SHARED` (the 4 KB handshake page
// at `CONTROL_ADDR`) holds just the control block — the ping-pong row buffers that used to follow it
// are gone (contract v2, #347: the FLPR scans the framebuffer directly). ──
include!(concat!(env!("OUT_DIR"), "/flpr_contract.rs"));

/// Shared control block at the `SHARED` page base — **contract v2** (issue #347): the ping-pong
/// `buf[2]` descriptors left the block; `fb_addr` (the resident framebuffer the FLPR scans
/// directly, stride = [`FB_W`] bytes/row by contract) took their place. Layout is normative and
/// identical to the C `flpr_control_t` in `src/flpr/flpr_scan.c` — keep them in sync
/// (`firmware/docs/ls021-flpr.md`). All fields `u32`, little-endian; `#[repr(C)]` + all-`u32`
/// members ⇒ deterministic offsets, no padding. Accessed only through raw volatile field
/// reads/writes (never as a `&` reference) since the FLPR mutates it concurrently.
#[repr(C)]
#[allow(dead_code)] // fields are touched only through raw `addr_of` field projections, never as `.field`.
struct Control {
    magic: u32,                    // 0x00 M33: layout/version tag, checked by the FLPR before acting
    m33_seq: u32,                  // 0x04 M33: command sequence counter (the per-frame command doorbell)
    cmd: u32,                      // 0x08 M33: command word (a CMD_* code)
    flpr_seq: u32,                 // 0x0C FLPR: echoes the m33_seq it serviced (the ack the M33 awaits)
    status: u32,                   // 0x10 FLPR: ack/result (dirty rows scanned; boot: FLPR_ALIVE)
    frame_count: u32,              // 0x14 FLPR: frames drained (bumped per CMD_RUN_FRAME)
    fb_addr: u32,                  // 0x18 M33: resident device-64 framebuffer base (stride FB_W bytes/row)
    n_spans: u32,                  // 0x1C M33: #dirty-row spans (1 = a full frame `(0, FB_H)`)
    spans: [u32; MAX_DIRTY_SPANS], // 0x20 M33: packed `(start_row << 16) | count`, ascending + disjoint
}
// (`MAX_DIRTY_SPANS` — the spans[] length on both sides — comes from the generated contract above;
// 16 disjoint regions is far more than any UI produces: a full frame is one span, the bulge is one.)
/// Lock the cross-language contract: the C `flpr_control_t` `_Static_assert`s the same 96 bytes
/// (the two structs alias the same shared-RAM bytes — with `MAX_DIRTY_SPANS` single-sourced the
/// sizes can no longer diverge via the array length, so this guards field-order/type drift).
const _: () = assert!(core::mem::size_of::<Control>() == 96);

const CONTROL: *mut Control = CONTROL_ADDR as *mut Control;

// ── VPR00 control (secure alias base 0x5004_C000): the M33 only launches the FLPR here. ──
const VPR00_INITPC: *mut u32 = 0x5004_C808 as *mut u32; // initial PC at core start
const VPR00_CPURUN: *mut u32 = 0x5004_C800 as *mut u32; // CPURUN.EN bit0 = run

// ── VPR00 RISC-V Debug Module (`DEBUGIF`, VPR00 + 0x400) — the **force-stop** lever a wedged FLPR
// needs (#349). `CPURUN.EN = 0` does *not* stop a running VPR core — it only parks one that
// reaches a WFI, which neither the scan blob's poll loop nor a hung/corrupted core ever executes
// (verified on glass: with `EN` cleared mid-run the blob kept servicing frames). The DM's
// `haltreq` halts the hart at the next instruction boundary regardless of what it is running, and
// `ndmreset` then resets it — so the relaunch can rebuild the blob under a genuinely stopped
// core. Standard RISC-V DM v0.13 register layout. ──
const VPR00_DMCONTROL: *mut u32 = 0x5004_C440 as *mut u32;
const VPR00_DMSTATUS: *mut u32 = 0x5004_C444 as *mut u32;
const DM_DMACTIVE: u32 = 1 << 0; // DMCONTROL: debug module enable (its own reset, active low-ish: 0 = DM reset)
const DM_NDMRESET: u32 = 1 << 1; // DMCONTROL: reset line to everything but the DM (the hart)
const DM_HALTREQ: u32 = 1 << 31; // DMCONTROL: halt the selected hart at the next instruction boundary
const DMSTATUS_ALLHALTED: u32 = 1 << 9; // DMSTATUS: the hart acknowledged the halt
/// How long a haltreq gets before the relaunch stops waiting and just fires `ndmreset` — the
/// halt is a courtesy (a clean instruction boundary), the reset is the actual guarantee.
const HALT_DEADLINE: Duration = Duration::from_millis(10);

// ── Frame geometry. The framebuffer is the LS021 pairing's `ls021::FRAME_W × FRAME_H` frame; the
//    wire-word counts (`WIDTH` 240, `BCK_PER_SUBLINE` 124, `ROW_WORDS` 248) come from
//    `obc_display::ls021::wire`. The static asserts pin the frame geometry to the protocol constants
//    it must equal, so the frame the app renders can never silently fork from the frame this backend
//    scans. ──
/// Visible pixel rows the FLPR scans per frame — the `status` the M33 cross-checks, and the
/// framebuffer height. The blob's gate scan is hard-wired to 320 rows, so the seam's `FRAME_H` must
/// equal it (asserted below).
pub const ROWS_PER_FRAME: u32 = 320;
/// Framebuffer width = the seam's frame width (re-exported for the resident-plane sizing in the app).
pub const FB_W: usize = obc_display::ls021::FRAME_W;
/// Framebuffer height = the seam's frame height = the visible row count.
pub const FB_H: usize = obc_display::ls021::FRAME_H;
// The wire pack consumes exactly one `ls021::wire::WIDTH`-pixel row per framebuffer row, and the FLPR
// blob scans exactly `ROWS_PER_FRAME` gate lines — the seam's frame must match both. (`obc_display`
// itself already asserts `ls021::wire::WIDTH == FRAME_W`; this pins the FLPR gate-scan height too.)
const _: () = assert!(FB_W == WIDTH, "ls021::FRAME_W diverged from the LS021 wire row width");
const _: () =
    assert!(FB_H == ROWS_PER_FRAME as usize, "ls021::FRAME_H diverged from the FLPR blob's 320-row gate scan");

/// The board's native frame: the one resident RGB222 plane (`main`'s `FB` static) wrapped as the
/// contracts' [`Device64Frame`] at the panel geometry. Owned by the map plane *next to* the
/// [`Ls021Flpr`] presenter — render borrows it mutably, a base present shares it for the whole
/// FLPR scan (the borrow that statically keeps a render off the bytes the coprocessor is reading).
pub type Frame64 = Device64Frame<'static, FB_W, FB_H>;

/// Max overlay region the overlay present's composite scratch holds — the hold
/// bulge's right-edge window (16 cols × 192 rows). A region must fit this (asserted);
/// the `[u16; COLS×ROWS]` RGB565 scratch is the only extra RAM the FLPR overlay path needs (~6 KB,
/// transient on the overlay-only frame's shallow stack — never live during a deep map render).
const MAX_OVERLAY_COLS: usize = 16;
const MAX_OVERLAY_ROWS: usize = 192;

// ── The EGU20 frame-ack doorbell (issue #347). The blob pokes `EGU20.TASKS_TRIGGER[0]` after every
// frame; the M33 arms `TRIGGERED[0]`'s IRQ and awaits it instead of busy-polling `flpr_seq`. Raw
// secure-alias MMIO (offsets from the nRF54L15 PAC: EVENTS_TRIGGERED[0] +0x100, INTENSET +0x304 —
// bit n = TRIGGERED[n]) because embassy-nrf's `pac` re-export is `pub(crate)`; same precedent as
// the VPR00 registers above and the FICR reads in `ble::gatt`. ──
const EGU20_EVENTS_TRIGGERED0: *mut u32 = 0x500C_9100 as *mut u32;
const EGU20_INTENSET: *mut u32 = 0x500C_9304 as *mut u32;

/// The frame ack the EGU20 ISR signals — what [`Ls021Flpr`]'s pushes `await` instead of spinning.
/// A `Signal` (not a waker) so an ack that lands *before* the await starts is not lost.
static FRAME_ACK: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// EGU20 ISR — the FLPR's per-frame doorbell: clear the latched `TRIGGERED[0]` event (or the IRQ
/// re-fires forever) and signal the awaiting push. Runs at P1 (set in [`launch_flpr`]): below the
/// P0 GRTC time-driver, above the P3 input plane — the ack is a one-store wake, so priority is
/// uncritical as long as it stays under P0.
#[interrupt]
unsafe fn EGU20() {
    EGU20_EVENTS_TRIGGERED0.write_volatile(0);
    FRAME_ACK.signal(());
}

/// **The sequence of the scan currently on the wire, or 0 when the panel is idle** (epic #1158).
///
/// A *sequence* rather than a flag, because the two are not equivalent: the EGU20 ISR fires per
/// frame and a late ack of frame *N* can land after [`Ls021Flpr::ring_spans`] has already rung
/// *N+1*, so a boolean cleared by "an ack arrived" would report the panel idle with a frame
/// half-drawn. Comparing against the FLPR's own `flpr_seq` echo cannot get that wrong, and it is
/// readable from **any** task — which matters, because the whole point is that a task other than
/// the one awaiting the ack can ask the question (see [`scan_in_flight`]).
static SCAN_SEQ: AtomicU32 = AtomicU32::new(0);

/// **Is a frame on the wire right now?** — the one window in which the FLPR must not be handed to
/// storage (epic #1158 / #1145 §3: *never park mid-scan*). Parking here abandons a half-scanned
/// frame; every other instant is fair game, because the display side only ever touches the
/// coprocessor from the map plane and the storage side's transfers are synchronous.
///
/// Reads the FLPR's `flpr_seq` echo directly instead of consulting the pushing task, so it stays
/// truthful while that task is suspended on [`FRAME_ACK`] — which is exactly the situation it
/// exists for.
pub fn scan_in_flight() -> bool {
    let s = SCAN_SEQ.load(Ordering::Relaxed);
    // SAFETY: a volatile read of one `u32` in the SHARED handshake page, which no Rust object
    // aliases and the FLPR writes concurrently.
    s != 0 && unsafe { addr_of!((*CONTROL).flpr_seq).read_volatile() } != s
}

/// Spin until [`scan_in_flight`] clears — the synchronous half of the mux's *never park mid-scan*
/// rule, for the paths that have no `await` (the `BlockDevice` seam).
///
/// Bounded by [`FRAME_DEADLINE`], the same window the pushing task gives the ack: past it the
/// coprocessor is wedged, not busy, and the caller is better served by taking the hart (the display
/// path's own timeout then escalates to a relaunch) than by waiting forever. Returns whether the
/// scan settled.
///
/// This spins rather than yields on purpose: it is reached from synchronous storage code, and the
/// task it is waiting on cannot make progress by being polled anyway — the FLPR finishes the frame
/// on its own and stamps `flpr_seq`, which this reads directly. Async callers should wait through
/// [`crate::flpr_mux::storage_session`] instead, which yields.
pub fn wait_scan_settled() -> bool {
    let deadline = Instant::now() + FRAME_DEADLINE;
    while scan_in_flight() {
        if Instant::now() >= deadline {
            error!(
                "LS021 FLPR: scan never settled within {=u64} ms — taking the hart anyway",
                FRAME_DEADLINE.as_millis()
            );
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

/// Deadline for one frame ack: the worst full-frame scan is ~44 ms (#348), so 250 ms (>5×)
/// only ever fires when the FLPR has genuinely stalled — turning a hang into a reported error the
/// caller retries (and #349's relaunch escalation hooks).
const FRAME_DEADLINE: Duration = Duration::from_millis(250);

/// Deadline for the boot/relaunch `ALIVE` stamp: the FLPR stamps within a few of its 5 ms poll
/// periods normally, so a full second only ever expires when the core genuinely didn't come up
/// (no boot, or shared RAM unreachable). `Instant`-based like every other display-path wait (#349
/// — no iteration-count spins left).
const ALIVE_DEADLINE: Duration = Duration::from_millis(1000);

/// Why [`launch_flpr`] gave up — surfaced to the caller so a panel that can't come up degrades to a
/// heartbeat idle rather than driving an un-launched FLPR (the same "never fault on bad hardware"
/// contract the SD/map path keeps).
#[derive(defmt::Format)]
pub enum FlprError {
    /// The FLPR booted but the control-block magic mismatched — a memory-map drift between [`Control`]
    /// here and `flpr_control_t` in the C blob.
    BadMagic,
    /// No `ALIVE` stamp within the boot window — the FLPR didn't boot or can't reach shared RAM.
    NoBoot,
}

/// Copy the blob into FLPR RAM, set the boot vector, and release the core from reset.
/// `INITPC` = the entry address; `CPURUN.EN = 1` starts execution there.
///
/// **The caller owns parking the hart first.** The boot path reaches a core that never ran; the
/// mode-switch path reaches one [`crate::semmc::park_hart`] has just reset. Re-copying the blob
/// under a live core is the one thing this must never do.
fn start_flpr() {
    unsafe {
        core::ptr::copy_nonoverlapping(FLPR_BLOB.as_ptr(), FLPR_RAM_BASE as *mut u8, FLPR_BLOB.len());
        // Make the blob + the control block visible to the other core before we release it.
        cortex_m::asm::dsb();
        VPR00_INITPC.write_volatile(FLPR_RAM_BASE as u32);
        VPR00_CPURUN.write_volatile(1); // EN = 1 → FLPR runs from INITPC
    }
}

/// Zero + arm the control block (the layout magic the FLPR checks first), then release the core —
/// the shared front half of [`launch_flpr`] and [`launch_flpr_blocking`].
fn arm_and_start() {
    // SAFETY: CONTROL is the SHARED-page control block at a fixed address no Rust object aliases; the
    // FLPR is not yet running, so this pre-launch zero/arm races nothing.
    unsafe {
        core::ptr::write_bytes(CONTROL as *mut u8, 0, core::mem::size_of::<Control>());
        addr_of_mut!((*CONTROL).magic).write_volatile(LAYOUT_MAGIC); // FLPR reads magic first thing
    }
    // A relaunch (recovery or a storage→display mode switch) hands the core a control block whose
    // `flpr_seq` has just been zeroed; the in-flight marker must go with it, or `scan_in_flight`
    // would compare a live sequence against a counter that will never reach it again.
    SCAN_SEQ.store(0, Ordering::Relaxed);
    start_flpr();
}

/// Arm the frame-ack doorbell (#347): EGU20 TRIGGERED[0] → its IRQ → [`FRAME_ACK`]. P1 = the
/// default peripheral lane (below the P0 GRTC time-driver; the ISR is one store + a signal). Armed
/// before the first push so no ack is ever missed, and re-run on every relaunch — every step is
/// idempotent (`INTENSET` is a set-mask write).
fn arm_frame_ack() {
    // SAFETY: fixed EGU20 secure-alias MMIO; set-mask writes only.
    unsafe {
        EGU20_INTENSET.write_volatile(1); // bit0 = TRIGGERED[0]
        interrupt::EGU20.set_priority(Priority::P1);
        interrupt::EGU20.enable();
    }
}

/// Bring the FLPR up: zero + arm the control block (write the layout magic the FLPR checks first),
/// launch the core, and poll for its `ALIVE` stamp. Returns once the FLPR has booted and agreed on
/// the control-block layout — after this the [`Ls021Flpr`] backend can drive frames.
///
/// The **boot / recovery** path: it yields between polls because a panel that is going to come up
/// late (a cold core, a stalled relaunch) may take a chunk of the 1 s [`ALIVE_DEADLINE`], and there
/// is nothing to gain from holding the executor for it. The per-switch path is
/// [`launch_flpr_blocking`].
pub async fn launch_flpr() -> Result<(), FlprError> {
    arm_and_start();
    info!("LS021 FLPR: released (INITPC=0x{=u32:08x}) — waiting for alive", FLPR_RAM_BASE as u32);

    let deadline = Instant::now() + ALIVE_DEADLINE;
    loop {
        match unsafe { addr_of!((*CONTROL).status).read_volatile() } {
            FLPR_ALIVE => {
                arm_frame_ack();
                crate::flpr_mux::note_display_live();
                return Ok(());
            }
            FLPR_BADMAG => return Err(FlprError::BadMagic),
            _ if Instant::now() >= deadline => return Err(FlprError::NoBoot),
            _ => Timer::after_millis(5).await,
        }
    }
}

/// **The mode-switch launch** (epic #1158): [`launch_flpr`]'s contract with a spin instead of a
/// yield.
///
/// [`crate::flpr_mux::ensure_display`] runs from synchronous code — the overlay push and the
/// `BlockDevice` seam both reach it with no `await` available — so the storage→display half of the
/// mux cannot use the async launch. Spinning is also simply the right shape here: the whole switch
/// measured **138 µs** on glass (the blob stamps `ALIVE` within a few of its poll periods, and it
/// has just been re-copied into RAM that was never powered down), so a yield would cost more in
/// scheduler round-trips than the wait itself. The deadline stays [`ALIVE_DEADLINE`] — reaching it
/// means the coprocessor is gone, not slow, and the caller's push then fails into the existing
/// relaunch escalation.
pub fn launch_flpr_blocking() -> Result<(), FlprError> {
    arm_and_start();
    let deadline = Instant::now() + ALIVE_DEADLINE;
    loop {
        match unsafe { addr_of!((*CONTROL).status).read_volatile() } {
            FLPR_ALIVE => {
                arm_frame_ack();
                return Ok(());
            }
            FLPR_BADMAG => return Err(FlprError::BadMagic),
            _ if Instant::now() >= deadline => return Err(FlprError::NoBoot),
            _ => core::hint::spin_loop(),
        }
    }
}

/// **Full FLPR relaunch** — the recovery step #349's escalation runs on a wedged coprocessor:
/// force-halt the hart through its Debug Module, hold it in `ndmreset`, then rerun the entire
/// [`launch_flpr`] bring-up — re-copy the blob (a hung FLPR may have been executing corrupted
/// instructions), zero + re-arm the control block, release the core, and await a fresh `ALIVE`.
///
/// The DM dance exists because **`CPURUN.EN = 0` cannot stop a wedged core** — it only parks one
/// that executes a WFI, which the busy-polling scan blob (and any hung variant of it) never does
/// (verified on glass). `haltreq` stops the hart wherever it is; `ndmreset` then resets it, so
/// the blob re-copy never races live execution. The halt wait is deadline-bounded and advisory —
/// if even the DM can't halt the hart, the reset is still the guarantee.
///
/// **COM is unaffected either way — the panel stays DC-bias-safe through a dead FLPR and through
/// this relaunch.** COM never ran on the FLPR: the M33 `com_task` (or the `com-hw` TIMER+DPPI+GPIOTE
/// generator) free-runs on its own plane, and the M33 keeps every gate/source GPIO configured
/// (`MapDisplay::_gate_bus`/`_src_bus`), so the glass just holds its last image while the FLPR is
/// down. That property is load-bearing — keep COM out of the FLPR whatever else moves there.
///
/// After an `Ok(())` the caller must force a **full repaint** (`reset_diff()` + a whole-frame
/// present): the fresh FLPR has no history, and rows the diff store thinks are on glass may have
/// missed it while the old FLPR was wedged. The M33-side `Ls021Flpr::seq` deliberately keeps
/// counting across the relaunch — the blob services any `m33_seq` different from the last one it
/// saw (zeroed to 0 here), so a stale-ack/fresh-ack mixup is impossible.
pub async fn relaunch_flpr() -> Result<(), FlprError> {
    // 0. If storage happens to hold the hart, quiesce the sEMMC peripheral first (epic #1158) —
    //    latched completions cleared, the shared VPR00 interrupt gate disarmed, the pads handed
    //    back — so the halt below lands on a coprocessor nothing is still talking to. A no-op on
    //    the path this is actually reached from (a failed display push, mode already Display).
    crate::flpr_mux::quiesce_storage_if_active();
    // 1. Force-halt via the DM (works mid-busy-loop, unlike CPURUN), then bounded-wait for the ack.
    unsafe {
        VPR00_DMCONTROL.write_volatile(DM_DMACTIVE); // wake the DM first (its fields gate on dmactive)
        VPR00_DMCONTROL.write_volatile(DM_DMACTIVE | DM_HALTREQ);
    }
    let deadline = Instant::now() + HALT_DEADLINE;
    while unsafe { VPR00_DMSTATUS.read_volatile() } & DMSTATUS_ALLHALTED == 0 {
        if Instant::now() >= deadline {
            debug!("LS021 FLPR: relaunch — hart ignored haltreq; proceeding to ndmreset");
            break; // the reset below is the real guarantee
        }
        Timer::after_micros(100).await;
    }
    // 2. Park the start gate, then pulse the hart reset while it's down — the blob re-copy in
    //    `launch_flpr` must never race a live core.
    unsafe {
        VPR00_CPURUN.write_volatile(0); // the reset release must not restart stale code
        VPR00_DMCONTROL.write_volatile(DM_DMACTIVE | DM_NDMRESET);
        cortex_m::asm::dsb();
        VPR00_DMCONTROL.write_volatile(DM_DMACTIVE); // release the hart reset (still parked: CPURUN=0)
        VPR00_DMCONTROL.write_volatile(0); // DM back off — leave the block as boot found it
    }
    launch_flpr().await
}

/// Start a frame: write `cmd = CMD_RUN_FRAME`, **then** a `dsb`, **then** bump `m33_seq` (the
/// command doorbell guard). The barrier sits *between* the payload and the doorbell so the FLPR can
/// never observe the new sequence before the command word, the span list ([`set_spans`]), the
/// `fb_addr`, and the framebuffer pixels it guards (issue #346). The trailing `dsb` just drains the
/// doorbell store promptly for the polling FLPR.
fn ring_cmd(seq: u32) {
    unsafe {
        addr_of_mut!((*CONTROL).cmd).write_volatile(CMD_RUN_FRAME);
        cortex_m::asm::dsb(); // cmd + spans + descriptors complete BEFORE the doorbell below
        addr_of_mut!((*CONTROL).m33_seq).write_volatile(seq); // seq last = the command doorbell guard
        cortex_m::asm::dsb();
    }
}

/// Publish the **dirty-row span list** the FLPR's masked scan reads (issue #163): each span a packed
/// `(start_row << 16) | count`, written ascending. Capped at `MAX_DIRTY_SPANS` (a UI never produces
/// more — a full frame is one span `(0, FB_H)`, the bulge one). Ordering: these plain volatile
/// stores carry no barrier of their own — they are ordered before the FLPR sees the frame by the
/// `dsb` [`ring_cmd`] issues *between* them and its `m33_seq` doorbell bump.
fn set_spans(spans: &[(u16, u16)]) {
    let n = spans.len().min(MAX_DIRTY_SPANS);
    unsafe {
        for (s, &(start, count)) in spans.iter().take(n).enumerate() {
            let packed = ((start as u32) << 16) | count as u32;
            addr_of_mut!((*CONTROL).spans[s]).write_volatile(packed);
        }
        addr_of_mut!((*CONTROL).n_spans).write_volatile(n as u32);
    }
}

/// Total rows across `spans` — the FLPR's dirty-row count, which `status` must echo back.
fn span_row_total(spans: &[(u16, u16)]) -> usize {
    spans.iter().take(MAX_DIRTY_SPANS).map(|&(_, count)| count as usize).sum()
}

/// Dump the full cross-core handshake state when a frame stalls — the read-back that tells *how* the
/// FLPR died. `m33_seq` ought to equal the `seq` we just rang; `flpr_seq`
/// is where the FLPR's poll loop last got to (≪ `seq` ⇒ it never saw our doorbell); `magic` should
/// still be `LAYOUT_MAGIC` (a different value = the SHARED page was clobbered); `frame_count` is how
/// many frames the FLPR has *ever* run (frozen ⇒ it stopped servicing). `CPURUN.EN` reads whether
/// the core is still released from reset. Together these separate "FLPR hung mid-loop" (CPURUN=1,
/// flpr_seq stale, magic intact) from "FLPR reset/faulted" (CPURUN=0) from "shared RAM corrupted"
/// (magic wrong) — which the bare "consumed/ready" line can't.
fn dump_flpr_state(seq: u32) {
    let (magic, m33_seq, flpr_seq, status, frames) = unsafe {
        (
            addr_of!((*CONTROL).magic).read_volatile(),
            addr_of!((*CONTROL).m33_seq).read_volatile(),
            addr_of!((*CONTROL).flpr_seq).read_volatile(),
            addr_of!((*CONTROL).status).read_volatile(),
            addr_of!((*CONTROL).frame_count).read_volatile(),
        )
    };
    let cpurun = unsafe { VPR00_CPURUN.read_volatile() };
    error!(
        "LS021 FLPR: state @ stall — magic=0x{=u32:08x} (want 0x{=u32:08x}) m33_seq={=u32} (rang {=u32}) flpr_seq={=u32} status={=u32} frame_count={=u32} CPURUN.EN={=u32}",
        magic, LAYOUT_MAGIC, m33_seq, seq, flpr_seq, status, frames, cpurun & 1
    );
}

/// A transport fault: the FLPR stalled (no frame ack / a status mismatch within the deadline). The
/// caller keeps the last frame and retries — the contracts' `Presenter::Error`.
#[derive(Debug)]
pub struct Stalled;

/// The LS021-over-FLPR presenter. The resident RGB222 frame plane lives *next to* it as [`Frame64`]
/// (the map plane owns both); the app renders the whole frame into the frame, and
/// [`push_frame`](Self::push_frame) / [`push_spans`](Self::push_spans) hand its bytes to the FLPR to
/// scan directly (`fb_addr` + the span list, then await the ack — #347). The contract impls
/// ([`Presenter`] / [`OverlayPresenter`]) are at the bottom of this file.
pub struct Ls021Flpr<'b> {
    /// The **self-diffing present** store: a per-row hash of the last-pushed framebuffer, so a
    /// present re-hashes the frame and pushes only the rows that actually changed — a Home clock
    /// tick repaints its clock band (a few ms) instead of all 320 rows (~44 ms). Borrowed from a
    /// `.bss` static (`main`), parallel to the frame (it must outlive the pushes); `FB_H` rows =
    /// 1.28 KB. The hashes track the clean frame, never the composited bulge (the bulge rides its
    /// own overlay-present plane), so the store stays the source of truth for what the map present
    /// last put on glass.
    diff: &'b mut RowDiff<FB_H>,
    /// Per-frame command sequence — bumped each push, echoed back by the FLPR as the ack.
    seq: u32,
}

impl<'b> Ls021Flpr<'b> {
    /// Build the presenter for the resident **device-64 map/ride plane** (`main.rs` wraps it as
    /// [`Frame64`]): the app quantises to the device-64 gamut itself
    /// ([`FbDevice64`](obc_display::FbDevice64)) and renders straight into the frame, then the
    /// self-diffing present diffs + drives it. The FLPR packs the frame straight to the wire, so
    /// there is no RGB565 band scratch (the ~7.5 KB an intermediate band push would need is never
    /// allocated). `diff` is the `FB_H`-row [`RowDiff`] store the self-diffing present compares
    /// against.
    pub fn new(diff: &'b mut RowDiff<FB_H>) -> Self {
        Self { diff, seq: 0 }
    }

    /// Drive a **span-masked** frame to glass — the engine behind [`push_spans`](Self::push_spans)
    /// and [`push_frame`](Self::push_frame). Publishes the span list + the framebuffer address,
    /// rings the FLPR, and **awaits the EGU20 frame ack** — the M33's entire per-frame work: the
    /// FLPR packs every dirty row to the wire itself, straight from the resident fb (#347), while
    /// thread mode runs other futures. The fb must not be written until the ack returns; the
    /// contracts' shared borrow of the frame across the present's `await` guarantees that
    /// statically (and the map plane is suspended inside this `await` for the whole push anyway).
    /// Returns `true` if the ack checks out (`status == #dirty rows && flpr_seq == seq`) within
    /// [`FRAME_DEADLINE`]; `false` = a stalled FLPR (the caller keeps the last frame and retries).
    async fn run_spans(&mut self, fb: &[u8], spans: &[(u16, u16)]) -> bool {
        let total = span_row_total(spans);
        if total == 0 {
            return true; // nothing dirty — a no-op frame (defensive; callers pass ≥1 row)
        }
        let (s, t_frame) = self.ring_spans(fb, spans);
        let deadline = t_frame + FRAME_DEADLINE;

        // Await the FLPR's EGU20 frame ack; on a stale echo (`flpr_seq` ≠ our seq — a late ack of
        // an older frame racing the reset in `ring_spans`) keep waiting for ours until the deadline.
        loop {
            if with_deadline(deadline, FRAME_ACK.wait()).await.is_err() {
                error!(
                    "LS021 FLPR: frame TIMEOUT — no EGU20 ack for seq {=u32} within {=u64} ms (the scan stalled)",
                    s,
                    FRAME_DEADLINE.as_millis()
                );
                dump_flpr_state(s);
                SCAN_SEQ.store(0, Ordering::Relaxed); // the panel is wedged, not scanning — release storage
                return false;
            }
            // The FLPR writes `status`/`frame_count` *before* the `flpr_seq` ack (its fence orders
            // them); mirror that on the read side so the ack is never reordered ahead of the values
            // it guards (issue #346).
            cortex_m::asm::dmb();
            if unsafe { addr_of!((*CONTROL).flpr_seq).read_volatile() } == s {
                break;
            }
        }
        SCAN_SEQ.store(0, Ordering::Relaxed); // the frame is on glass — storage may take the hart
        self.check_ack(total, t_frame)
    }

    /// The **blocking** twin of [`run_spans`](Self::run_spans), for the overlay path only: identical
    /// ring + ack contract, but spin-waits `flpr_seq` (deadline-bounded) instead of awaiting EGU20.
    ///
    /// **Why it exists — the #347 stack lesson.** The overlay present's composite
    /// scratch + save window (~9 KB) must stay **stack transients**: alive across an `await` they
    /// become part of the map-plane task's *static* future, permanently shrinking the residual
    /// stack by that much (on glass: total 36.6 → 24.7 KB, and the first deep map render overflowed
    /// into `.bss` — a BusFault at boot). An overlay push is a few ms (≤192 rows, gate
    /// fast-forwarded), so briefly spinning costs what the pre-#347 path always cost, while the
    /// ~44 ms map frames keep the async ack. The EGU20 ISR still fires per frame; the stale signal
    /// it leaves is dropped by the next async push's `FRAME_ACK.reset()`.
    fn run_spans_blocking(&mut self, fb: &[u8], spans: &[(u16, u16)]) -> bool {
        let total = span_row_total(spans);
        if total == 0 {
            return true;
        }
        let (s, t_frame) = self.ring_spans(fb, spans);
        let deadline = t_frame + FRAME_DEADLINE;
        while unsafe { addr_of!((*CONTROL).flpr_seq).read_volatile() } != s {
            if Instant::now() > deadline {
                error!(
                    "LS021 FLPR: overlay frame TIMEOUT — no ack for seq {=u32} within {=u64} ms (the scan stalled)",
                    s,
                    FRAME_DEADLINE.as_millis()
                );
                dump_flpr_state(s);
                SCAN_SEQ.store(0, Ordering::Relaxed);
                return false;
            }
            core::hint::spin_loop();
        }
        SCAN_SEQ.store(0, Ordering::Relaxed);
        cortex_m::asm::dmb(); // ack-read ordering, as in the async path (issue #346)
        self.check_ack(total, t_frame)
    }

    /// Publish the span list + the framebuffer address, drop any stale ack signal, and ring one
    /// `CMD_RUN_FRAME` — the shared front half of [`run_spans`](Self::run_spans) /
    /// [`run_spans_blocking`](Self::run_spans_blocking). Returns the rung sequence + the frame's t0.
    ///
    /// **This is the display side's single seam onto the mode mux** (epic #1158): both pushes go
    /// through here, so taking the coprocessor back from storage is one call in one place. It is
    /// deliberately *not* fallible — a failed relaunch rings the doorbell at a dead core, the ack
    /// times out, and the existing `MapDisplay` escalation (`relaunch_flpr` after
    /// `PUSH_FAILS_PER_RELAUNCH`) handles it, rather than this growing a second recovery ladder.
    fn ring_spans(&mut self, fb: &[u8], spans: &[(u16, u16)]) -> (u32, Instant) {
        crate::flpr_mux::ensure_display();
        self.seq += 1;
        let s = self.seq;
        // Publish before the doorbell: from here until the ack the FLPR owns the hart, and storage
        // must wait (`scan_in_flight`). Cleared by both run paths, success or timeout.
        SCAN_SEQ.store(s, Ordering::Relaxed);
        set_spans(spans);
        // SAFETY: plain volatile store into the SHARED-page control block (no Rust object aliases
        // it); the FLPR reads it only after the `ring_cmd` doorbell below orders it.
        unsafe { addr_of_mut!((*CONTROL).fb_addr).write_volatile(fb.as_ptr() as u32) };
        // Drop any stale ack (a late echo of an older frame, or a blocking overlay push's unclaimed
        // signal) so an async wait can't be satisfied by history.
        FRAME_ACK.reset();
        let t_frame = Instant::now();
        ring_cmd(s);
        (s, t_frame)
    }

    /// The shared back half of a push: cross-check the FLPR's `status` against the dirty-row total
    /// and emit the per-push `debug` breakdown. The caller has already matched `flpr_seq` (+ `dmb`).
    fn check_ack(&self, total: usize, t_frame: Instant) -> bool {
        let frame_us = t_frame.elapsed().as_micros();
        let status = unsafe { addr_of!((*CONTROL).status).read_volatile() };
        let frames = unsafe { addr_of!((*CONTROL).frame_count).read_volatile() };
        if status != total as u32 {
            error!("LS021 FLPR: frame MISMATCH — status={=u32} (want {=usize} dirty rows)", status, total);
            return false;
        }
        // The loop's `map frame` / `ui frame` line already reports the push time at `info`; this is
        // the per-push internal breakdown (dirty rows, µs/row), kept at `debug` so a build can opt
        // into it for perf-tuning without flooding the default log every frame.
        debug!(
            "LS021 FLPR: frame OK — FLPR scanned {=usize} dirty rows (frame #{=u32}) in {=u64} µs (~{=u64} µs/row)",
            total,
            frames,
            frame_us,
            frame_us / total as u64,
        );
        true
    }

    /// Drive **only the rows in `spans`** to glass — the FLPR packs each straight from the resident
    /// framebuffer, fast-forwards the gate over the gaps, and early-stops after the last dirty row,
    /// so a vertically-compact region costs a fraction of a full frame. Spans are
    /// `(start_row, count)`, ascending + disjoint, ≤ `MAX_DIRTY_SPANS`. A full frame is
    /// `&[(0, FB_H)]` (= [`push_frame`](Self::push_frame)).
    pub async fn push_spans(&mut self, frame: &Frame64, spans: &[(u16, u16)]) -> bool {
        self.run_spans(frame.bytes(), spans).await
    }

    /// Drive the **whole** resident frame to glass — the degenerate one-span frame `&[(0, FB_H)]`
    /// (the boot init-black frame). Does **not** touch the [`RowDiff`] store (it's a raw push, not
    /// a diff), so the first self-diffing present still re-seeds the store from a clean comparison
    /// and pushes the whole frame — the exact boot behavior the datasheet's Initial #0 sequence
    /// relies on.
    pub async fn push_frame(&mut self, frame: &Frame64) -> bool {
        self.push_spans(frame, &[(0, FB_H as u16)]).await
    }

    /// Re-arm the self-diffing present so the next present pushes the **whole** frame again and
    /// re-seeds the store — the recovery path when a push failed to reach glass (a stalled FLPR).
    /// The present advances the row-hash store *before* the push, so after a fault the store
    /// already records the (un-pushed) current frame; a plain retry would then diff an identical
    /// frame against an up-to-date store and re-push **nothing**, stranding the rows that missed
    /// glass. Resetting forces the retry to repaint every row. Delegates to
    /// [`RowDiff::reset`](obc_display::ls021::RowDiff::reset). (The contracts' `damage_full`
    /// collapses this + the push into one present; the board keeps the two-step shape because the
    /// reset happens at the fault site and the repaint on the ride loop's next latched redraw.)
    pub fn reset_diff(&mut self) {
        self.diff.reset();
    }
}

// ── The display contracts. Thin adapters over the panel methods above: the FLPR launch + direct-fb
//    transport stay this crate's business, but the map/ride app reaches glass only through the
//    (`Frame64`, `Ls021Flpr`) pairing — the panel-swap point; the simulator presenter is the
//    contracts' other live backend. The only LS021-specific code is the device-64 → 6-line wire
//    pack the pushes drive (on the FLPR); the diff/clip/exclusion policy and the overlay composite
//    are the shared, host-conformance-tested `obc_display::ls021` strategy pieces. ──

impl Presenter<Frame64> for Ls021Flpr<'_> {
    type Damage = RowDamage;
    type Error = Stalled;

    fn damage_full() -> RowDamage {
        RowDamage::Full
    }

    fn damage_unknown() -> RowDamage {
        RowDamage::SelfDiff { exclude: None }
    }

    /// The **self-diffing present**: re-hash every frame row against the [`RowDiff`] store and push
    /// only the rows that actually changed, optionally clipping the live hold bulge's rows out
    /// (`RowDamage::SelfDiff { exclude }` — [`damage_around`](OverlayPresenter::damage_around)) so
    /// the overlay present owns them; an idle Home clock tick repaints just its clock band.
    /// `RowDamage::Full` re-seeds the store and pushes the whole frame; the first present after
    /// boot / a [`reset_diff`](Ls021Flpr::reset_diff) does the same through the store's priming
    /// flag.
    ///
    /// The diff/clip/fallback skeleton is the shared [`RowDiff::diff_clipped`] (see its docs for
    /// the exclude semantics — the store is updated for the clipped rows too, so the trailing clear
    /// finds it already agreeing). Awaits the FLPR's EGU20 frame ack — the M33 runs other futures
    /// for the whole ~44 ms scan (#347), and the shared `&Frame64` borrow held across that await is
    /// what statically keeps the renderer off the bytes the FLPR is scanning. `Err(Stalled)` = a
    /// transport fault: the caller keeps the last frame and retries.
    async fn present(&mut self, frame: &Frame64, damage: RowDamage) -> Result<PresentStats, Stalled> {
        let exclude = match damage {
            RowDamage::Full => {
                self.diff.reset(); // full = re-seed the store + push every row (recovery/re-init)
                None
            }
            RowDamage::SelfDiff { exclude } => exclude,
        };
        let mut scratch = [(0u16, 0u16); MAX_DIRTY_SPANS];
        let spans = self.diff.diff_clipped(frame.bytes(), FB_W, exclude, &mut scratch);
        if spans.is_empty() {
            // Nothing changed outside the bulge — push nothing (the whole point).
            return Ok(PresentStats { pushed_units: 0, total_units: FB_H as u32, regions: 0 });
        }
        let pushed: u32 = spans.iter().map(|&(_, n)| n as u32).sum();
        let regions = spans.len() as u32;
        if self.run_spans(frame.bytes(), spans).await {
            Ok(PresentStats { pushed_units: pushed, total_units: FB_H as u32, regions })
        } else {
            Err(Stalled)
        }
    }
}

impl OverlayPresenter<Frame64> for Ls021Flpr<'_> {
    type Region = RowWindow;
    type OverlayTarget<'t> = Band<'t>;

    fn region(rect: Rectangle) -> RowWindow {
        RowWindow::from_rect(rect, FB_W as u32, FB_H as u32)
    }

    fn damage_around(region: RowWindow) -> RowDamage {
        RowDamage::SelfDiff { exclude: Some(region.exclude_span()) }
    }

    /// Re-present **only the rows of the overlay region** with `draw` composited over the clean
    /// frame backdrop — the few-ms partial push the hold bulge rides over a static map. The LS021
    /// is row-addressed (touching a row re-latches all 240 columns), so this rewrites the
    /// full-width rows `[y0, y0+rows)` while only the `[x0, x0+w)` columns carry the overlay; the
    /// FLPR fast-forwards the gate to `y0` and early-stops after `y0+rows`, so the cost scales to
    /// the region's row span, not the whole frame.
    ///
    /// **Composite-into-fb with save/restore** (#347): the FLPR scans the frame directly, so the
    /// composited window must transiently *be* in it — the shared
    /// [`composite_into_resident`] engine saves the clean window bytes (≤3 KB), writes the
    /// composited window in (re-quantised to device-64 by the same host-tested packer the frame
    /// itself was rendered with, so the overlay lands on the panel's RGB222 gamut exactly as the map
    /// style cards do), pushes the rows, and restores. Sound because
    /// the map plane owns the frame and is inside this call for the whole push (the `&mut Frame64`
    /// borrow — it can't render mid-push), and the input plane never touches the frame. The
    /// [`RowDiff`] store keeps tracking the **clean** frame throughout — after the restore the
    /// frame is byte-identical to before (the contracts' clean-frame postcondition, checked by the
    /// conformance harness against this same engine), so the store is never touched and the
    /// trailing clear finds it already agreeing. On a push fault the window is still restored (the
    /// caller retries; a mid-scan FLPR reading restored clean bytes just paints clean rows — the
    /// next overlay tick repaints them).
    ///
    /// **Stack + lock discipline**: the overlay is rendered into a small RGB565 window scratch
    /// **once** (one `draw` call ⇒ the caller's `InputPlane` lock is taken once per overlay frame,
    /// not per row); the transient cost is the ~6 KB RGB565 scratch + the ~3 KB save window on this
    /// call's stack — an overlay-only frame, never live during a deep map render.
    ///
    /// **Deliberately blocking** (`run_spans_blocking`) with **no await point in this body**: the
    /// scratches stay poll-stack transients exactly as before — if those ~9 KB lived across an
    /// `await` they would move into the map-plane task's *static* future and permanently shrink the
    /// residual stack (the on-glass boot HardFault that forced this shape). A bulge push is a few
    /// ms; the ~44 ms map presents are the ones that await (see [`run_spans_blocking`]'s doc).
    async fn present_overlay(
        &mut self,
        frame: &mut Frame64,
        region: RowWindow,
        draw: impl for<'t> FnOnce(&mut Band<'t>),
    ) -> Result<PresentStats, Stalled> {
        let (x0, y0, w, rows) = (region.x0 as usize, region.y0 as usize, region.w as usize, region.rows as usize);
        assert!(
            w <= MAX_OVERLAY_COLS && rows <= MAX_OVERLAY_ROWS && x0 + w <= FB_W && y0 + rows <= FB_H,
            "overlay region out of bounds / larger than the composite scratch"
        );
        let mut win = [0u16; MAX_OVERLAY_COLS * MAX_OVERLAY_ROWS];
        let mut save = [0u8; MAX_OVERLAY_COLS * MAX_OVERLAY_ROWS];
        let mut draw = Some(draw);
        composite_into_resident(
            frame.bytes_mut(),
            Size::new(FB_W as u32, FB_H as u32),
            region,
            OverlayScratch { win: &mut win, save: &mut save },
            &mut |band| {
                if let Some(d) = draw.take() {
                    d(band)
                }
            },
            |fb| if self.run_spans_blocking(fb, &[(region.y0, region.rows)]) { Ok(()) } else { Err(Stalled) },
        )
        .map(|()| PresentStats { pushed_units: region.rows as u32, total_units: FB_H as u32, regions: 1 })
    }
}
