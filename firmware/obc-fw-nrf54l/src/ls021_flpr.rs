//! **The LS021B7DD02 FLPR display backend, driving the real app's map/ride render.**
//!
//! Lifted out of the (since-retired) `ls021_flpr_bringup` bench bin (epic #149, F5/PR #162) so
//! `main.rs` can run the real [`obc_app::App`](obc_app) on the reflective LS021 panel through the
//! board-agnostic [`DisplayDriver`](crate::display::DisplayDriver) seam (issue #174) the ST7789 also
//! implements — no panel-specific code in the map plane.
//!
//! What lives here is everything that talks to the **FLPR** (the nRF54L15's VPR RISC-V coprocessor):
//!   - the cross-core [`Control`] block + ping-pong [`BufDesc`]s + the dirty-row span list (the
//!     normative contract with the C blob `src/flpr/flpr_pingpong.c` — kept byte-for-byte in sync,
//!     both static-assert 124 bytes);
//!   - [`launch_flpr`] — copy the blob into FLPR RAM, arm the control block, release the core, and
//!     wait for its `ALIVE` stamp;
//!   - [`Ls021Flpr`] — the resident-framebuffer backend whose pushes pack the dirty rows of the
//!     RGB222 plane through the two ping-pong buffers and busy-wait one masked `CMD_RUN_FRAME`
//!     ([`push_spans`](Ls021Flpr::push_spans) = only the listed rows, the FLPR fast-forwarding the
//!     gate over the rest — issue #163; [`push_frame`](Ls021Flpr::push_frame) = the whole frame, the
//!     degenerate one-span case). [`present_within`](Ls021Flpr::present_within) is the **self-diffing
//!     present** (issue #201): it derives those spans automatically from a per-row hash of the
//!     last-pushed frame, so an idle redraw pushes only the rows that actually changed.
//!
//! The `DisplayDriver` adapter (present / present_overlay) lives in `display::ls021_flpr` (issue
//! #174); this root module owns the FLPR transport it calls into.
//!
//! **COM stays on the M33** (`com::com_task`) and **is not here** — if the FLPR ever faults, COM
//! must keep alternating so the panel never takes a DC bias (the epic's safety rule). The caller
//! owns COM + the high-priority `InterruptExecutor` it free-runs on.
//!
//! ## Whole-frame render, masked push (the design choice)
//!
//! The app still **renders** the whole frame into the resident RGB222 plane each redraw — the screens
//! are immediate-mode, they `clear()` and redraw (the map path writes device-64
//! ([`FbDevice64`](obc_platform::FbDevice64)) straight via [`fb_mut`](Ls021Flpr::fb_mut)). What the
//! self-diffing **present** then changes is the *push*: a single `CMD_RUN_FRAME` masked to the changed
//! rows, the FLPR fast-forwarding its gate scan over the unchanged ones and early-stopping after the
//! last. Render CPU is unchanged (a full draw), but the push — the dominant ~97 ms cost — scales to the
//! changed-row span. The ping-pong (M33 packs the next dirty row while the FLPR scans the current) is
//! exactly as F4 proved it; a full redraw is just the degenerate "every row changed" case.
//!
//! ## Blocking push
//!
//! The push is synchronous: [`push_frame`](Ls021Flpr::push_frame) / [`push_spans`](Ls021Flpr::push_spans)
//! **busy-poll** rather than awaiting — they spin on each ping-pong buffer's `consumed == ready` (the
//! M33 is a dedicated packer here) and on the FLPR's `flpr_seq` ack (no EGU20 IRQ needed — the F4 async
//! return doorbell). COM free-runs on its own high-priority `InterruptExecutor`, so blocking the
//! thread-mode M33 for a frame is benign (the same shape as the ST7789 path blocking on its SPI-DMA
//! write). The blob still pokes `EGU20` after each frame; with its IRQ unarmed here that write is a
//! harmless no-op.

use core::ptr::{addr_of, addr_of_mut};

use defmt::{error, info};
use embassy_time::{Instant, Timer};
// The host-tested RGB222 → LS021-wire pack (#154) with its sub-line/row word counts.
use obc_platform::ls021_wire::{BCK_PER_SUBLINE, ROW_WORDS, WIDTH};
// `composite_overlay_window` is the shared overlay-composite core (#174): fill a window scratch from
// the clean framebuffer (device-64 → RGB565) + draw the hold bulge over it through a `Band` — the same
// step the ST7789 backend runs, before this backend re-quantises it back to the wire. `RowDiff` is the
// self-diffing present store (#200): a per-row hash of the last-pushed frame so a present pushes only
// the rows that actually changed (issue #201).
use obc_platform::{clip_span, composite_overlay_window, ls021_pack_row, Band, RowDiff};
// The host-tested RGB565 → device-64 quantiser — the same one the map style table is tuned to, so the
// re-quantised overlay window lands on the panel's RGB222 gamut exactly as the ST7789 stand-in shows it.
use obc_reader::rgb565_to_device64;

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

/// The FLPR program image, cross-compiled by `build.rs` into `$OUT_DIR/flpr.bin`.
static FLPR_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flpr.bin"));

// ── FLPR memory map (must match src/flpr/flpr.ld + the carved memory.x in build.rs) ──
// The production carve-out is **12 KB** (issue #165): the blob is ~660 B + a shallow stack, so the
// 28 KB F0 bring-up headroom shrank to an 8 KB `FLPR_RAM`, handing ~20 KB back to the M33 (it now
// links 244 KB instead of 224 KB). `SHARED` (the 4 KB handshake page) is unchanged, so the control
// block + ping-pong buffer addresses below did not move.
/// FLPR execution base: the M33 copies the blob here and points `INITPC` at it (top of SRAM − 12 KB).
const FLPR_RAM_BASE: usize = 0x2003_D000;
/// The **two ping-pong row buffers** the FLPR drains, parked in the SHARED page (which both cores
/// already reach — the FLPR reads the control block there). Each row buffer is `ROW_WORDS` (248)
/// u32 = MSB sub-line [0..124) then LSB sub-line [124..248); `buf[0]` carries even rows, `buf[1]`
/// odd — the M33 packs one while the FLPR scans the other.
const WRITE_BUF_ADDR: [usize; 2] = [0x2003_F100, 0x2003_F100 + ROW_WORDS * 4];

/// Shared control block at the `SHARED` page base. Layout is normative and identical to the C
/// `flpr_control_t` in `src/flpr/flpr_pingpong.c` — keep them in sync (`firmware/docs/ls021-flpr.md`).
/// All fields `u32`, little-endian; `#[repr(C)]` + all-`u32` members ⇒ deterministic offsets, no
/// padding. Accessed only through raw volatile field reads/writes (never as a `&` reference) since
/// the FLPR mutates it concurrently.
#[repr(C)]
#[allow(dead_code)] // fields are touched only through raw `addr_of` field projections, never as `.field`.
struct Control {
    magic: u32,                    // 0x00 M33: layout/version tag, checked by the FLPR before acting
    m33_seq: u32,                  // 0x04 M33: command sequence counter (the per-frame command doorbell)
    cmd: u32,                      // 0x08 M33: command word (a CMD_* code)
    flpr_seq: u32,                 // 0x0C FLPR: echoes the m33_seq it serviced (the ack the M33 polls)
    status: u32,                   // 0x10 FLPR: ack/result (#163: dirty rows scanned; boot: FLPR_ALIVE)
    frame_count: u32,              // 0x14 FLPR: frames drained (bumped per CMD_RUN_FRAME)
    buf: [BufDesc; 2],             // 0x18, 0x28 ping-pong row-buffer descriptors (toggled per DIRTY row, #163)
    n_spans: u32,                  // 0x38 M33: #dirty-row spans (1 = a full frame `(0, FB_H)`)
    spans: [u32; MAX_DIRTY_SPANS], // 0x3C M33: packed `(start_row << 16) | count`, ascending + disjoint
}
/// Dirty-row span-list cap (issue #163) — **must equal `MAX_DIRTY_SPANS` in the C `flpr_pingpong.c`**.
/// 16 disjoint regions is far more than any UI produces (a full frame is one span, the bulge is one).
const MAX_DIRTY_SPANS: usize = 16;
/// One ping-pong row-buffer descriptor. `ready`/`consumed` are the per-buffer handshake counters:
/// the M33 bumps `ready` after packing a fresh row into the buffer; the FLPR sets `consumed = ready`
/// after it has finished scanning that row out. `ready != consumed` ⇒ a row is waiting to be
/// scanned; `consumed == ready` ⇒ the buffer is free for the M33 to refill.
#[repr(C)]
#[allow(dead_code)]
struct BufDesc {
    ptr: u32,      // row-buffer base (MSB sub-line at [0..len), LSB sub-line at [len..2·len))
    len: u32,      // words per sub-line = BCK per sub-line; a row is 2·len words
    ready: u32,    // M33: bumped after packing a row into this buffer
    consumed: u32, // FLPR: set = ready after draining this buffer
}
/// Lock the cross-language contract: the C `flpr_control_t` `_Static_assert`s the same 124 bytes (the
/// two structs alias the same shared-RAM bytes), which also stays below the ping-pong buffer base at
/// `control + 0x100` (see [`WRITE_BUF_ADDR`]) so the span list never moves the buffers.
const _: () = assert!(core::mem::size_of::<Control>() == 124);

const CONTROL: *mut Control = 0x2003_F000 as *mut Control;
const LAYOUT_MAGIC: u32 = 0xF1C0_0001; // "F1 control block" — the FLPR refuses to act otherwise
const FLPR_ALIVE: u32 = 0x0000_A11E; // FLPR boot confirmation
const FLPR_BADMAG: u32 = 0x0BAD_CAFE; // FLPR booted but saw the wrong magic (memory-map drift)
const CMD_RUN_FRAME: u32 = 0x0000_0002; // drive one full frame, ping-ponging buf[0]/buf[1]

// ── VPR00 control (secure alias base 0x5004_C000): the M33 only launches the FLPR here. ──
const VPR00_INITPC: *mut u32 = 0x5004_C808 as *mut u32; // initial PC at core start
const VPR00_CPURUN: *mut u32 = 0x5004_C800 as *mut u32; // CPURUN.EN bit0 = run

// ── Frame geometry. The wire-word counts (`WIDTH` 240, `BCK_PER_SUBLINE` 124, `ROW_WORDS` 248) come
//    from `obc_platform::ls021_wire`. The framebuffer is `WIDTH × ROWS_PER_FRAME` device-64 bytes. ──
/// Visible pixel rows the FLPR scans per frame — the `status` the M33 cross-checks, and the
/// framebuffer height.
pub const ROWS_PER_FRAME: u32 = 320;
/// Framebuffer width = the panel width (re-exported for the resident-plane sizing in the app).
pub const FB_W: usize = WIDTH;
/// Framebuffer height = the visible row count.
pub const FB_H: usize = ROWS_PER_FRAME as usize;

/// Max overlay region [`push_overlay`](Ls021Flpr::push_overlay)'s composite scratch holds — the hold
/// bulge's right-edge window (16 cols × 192 rows, issue #126/#163). A region must fit this (asserted);
/// the `[u16; COLS×ROWS]` RGB565 scratch is the only extra RAM the FLPR overlay path needs (~6 KB,
/// transient on the overlay-only frame's shallow stack — never live during a deep map render).
const MAX_OVERLAY_COLS: usize = 16;
const MAX_OVERLAY_ROWS: usize = 192;

/// Busy-poll cap while waiting on the FLPR (a free ping-pong buffer or the frame ack). A spin cap,
/// not a duration — sized large enough that even a full slow frame never trips it, so it only fires
/// if the FLPR has genuinely stalled, turning a hang into a reported error.
const SPIN_CAP: u32 = 50_000_000;

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
fn start_flpr() {
    unsafe {
        core::ptr::copy_nonoverlapping(FLPR_BLOB.as_ptr(), FLPR_RAM_BASE as *mut u8, FLPR_BLOB.len());
        // Make the blob + the control block visible to the other core before we release it.
        cortex_m::asm::dsb();
        VPR00_INITPC.write_volatile(FLPR_RAM_BASE as u32);
        VPR00_CPURUN.write_volatile(1); // EN = 1 → FLPR runs from INITPC
    }
}

/// Bring the FLPR up: zero + arm the control block (write the layout magic the FLPR checks first),
/// launch the core, and poll for its `ALIVE` stamp. Returns once the FLPR has booted and agreed on
/// the control-block layout — after this the [`Ls021Flpr`] backend can drive frames.
pub async fn launch_flpr() -> Result<(), FlprError> {
    // SAFETY: CONTROL is the SHARED-page control block at a fixed address no Rust object aliases; the
    // FLPR is not yet running, so this pre-launch zero/arm races nothing.
    unsafe {
        core::ptr::write_bytes(CONTROL as *mut u8, 0, core::mem::size_of::<Control>());
        addr_of_mut!((*CONTROL).magic).write_volatile(LAYOUT_MAGIC); // FLPR reads magic first thing
    }
    start_flpr();
    info!("LS021 FLPR: released (INITPC=0x{=u32:08x}) — waiting for alive", FLPR_RAM_BASE as u32);

    for _ in 0..200 {
        match unsafe { addr_of!((*CONTROL).status).read_volatile() } {
            FLPR_ALIVE => return Ok(()),
            FLPR_BADMAG => return Err(FlprError::BadMagic),
            _ => Timer::after_millis(5).await,
        }
    }
    Err(FlprError::NoBoot)
}

// ── Per-buffer ping-pong descriptor access (volatile; the FLPR mutates `consumed` concurrently). ──
fn buf_ready(i: usize) -> u32 {
    unsafe { addr_of!((*CONTROL).buf[i].ready).read_volatile() }
}
fn buf_consumed(i: usize) -> u32 {
    unsafe { addr_of!((*CONTROL).buf[i].consumed).read_volatile() }
}

/// Reset both ping-pong descriptors for a new frame: (re)publish `ptr`/`len` and zero the
/// `ready`/`consumed` counters. Safe to do unsynchronised because the FLPR is idle here — it only
/// enters a frame on the `m33_seq` bump (`ring_cmd`), which happens after the pre-fill.
fn reset_descriptors() {
    for (i, &addr) in WRITE_BUF_ADDR.iter().enumerate() {
        unsafe {
            addr_of_mut!((*CONTROL).buf[i].ptr).write_volatile(addr as u32);
            addr_of_mut!((*CONTROL).buf[i].len).write_volatile(BCK_PER_SUBLINE as u32); // words per sub-line
            addr_of_mut!((*CONTROL).buf[i].ready).write_volatile(0);
            addr_of_mut!((*CONTROL).buf[i].consumed).write_volatile(0);
        }
    }
}

/// Pack framebuffer row `row` into ping-pong buffer `i` and publish it: the host-tested
/// [`ls021_pack_row`] writes the 248 wire words (MSB sub-line then LSB sub-line), a `dsb` orders
/// those stores before the `ready` bump, then `ready += 1` hands the buffer to the FLPR. The FLPR
/// `fence`s on seeing `ready` change, so it never reads the words before they land.
fn publish_row(fb: &[u8], i: usize, row: usize) {
    // SAFETY: `out` is the SHARED-page write buffer at a fixed address no Rust object aliases.
    let out = unsafe { core::slice::from_raw_parts_mut(WRITE_BUF_ADDR[i] as *mut u32, ROW_WORDS) };
    ls021_pack_row(&fb[row * FB_W..row * FB_W + FB_W], out);
    cortex_m::asm::dsb(); // buffer words complete before the ready bump the FLPR waits on
    let next = buf_ready(i).wrapping_add(1);
    unsafe { addr_of_mut!((*CONTROL).buf[i].ready).write_volatile(next) };
}

/// Start a frame: write `cmd = CMD_RUN_FRAME` then bump `m33_seq` **last** (the command doorbell
/// guard), with a `dsb` so the FLPR never sees the new sequence before the pre-filled buffers +
/// descriptors it guards.
fn ring_cmd(seq: u32) {
    unsafe {
        addr_of_mut!((*CONTROL).cmd).write_volatile(CMD_RUN_FRAME);
        addr_of_mut!((*CONTROL).m33_seq).write_volatile(seq); // seq last = the command doorbell guard
        cortex_m::asm::dsb();
    }
}

/// Publish the **dirty-row span list** the FLPR's masked scan reads (issue #163): each span a packed
/// `(start_row << 16) | count`, written ascending. Capped at [`MAX_DIRTY_SPANS`] (a UI never produces
/// more — a full frame is one span `(0, FB_H)`, the bulge one). Written before [`ring_cmd`] bumps
/// `m33_seq` (its `dsb` orders the whole pre-fill, including these words, before the doorbell).
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

/// Spin (bounded by [`SPIN_CAP`]) until `cond` holds. Returns `false` if the cap trips first (a
/// stalled FLPR). Pure busy-poll — correct here because the M33 is a dedicated packer and COM runs
/// on its own interrupt executor, so there is nothing else for this core to yield to.
fn spin_until(cond: impl Fn() -> bool) -> bool {
    for _ in 0..SPIN_CAP {
        if cond() {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// Dump the full cross-core handshake state when a frame stalls — the read-back that tells *how* the
/// FLPR died (issue #165 freeze debug). `m33_seq` ought to equal the `seq` we just rang; `flpr_seq`
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

/// The LS021-over-FLPR display backend. Owns the resident RGB222 framebuffer plane; the app renders
/// the whole frame into it (the map path writes it directly as device-64 via [`fb_mut`](Self::fb_mut)),
/// and [`push_frame`](Self::push_frame) / [`push_spans`](Self::push_spans) drive it to glass over the
/// ping-pong path. The board-agnostic [`DisplayDriver`](crate::display::DisplayDriver) impl
/// (present / present_overlay) lives in `display::ls021_flpr` (issue #174).
pub struct Ls021Flpr<'b> {
    /// Resident RGB222 (device-64) frame plane, `FB_W × FB_H`. `fb_mut` writes it,
    /// `push_frame`/`push_spans` pack + push it.
    fb: &'b mut [u8],
    /// The **self-diffing present** store (issue #201/#200): a per-row hash of the last-pushed
    /// framebuffer, so [`present_within`](Self::present_within) re-hashes the frame and pushes only the
    /// rows that actually changed — a Home clock tick repaints its clock band (a few ms) instead of all
    /// 320 rows (~97 ms). Borrowed from a `.bss` static (`main`), parallel to `fb` (it must outlive the
    /// pushes); `FB_H` rows = 1.28 KB. The hashes track the clean `fb`, never the composited bulge (the
    /// bulge rides its own [`push_overlay`](Self::push_overlay) plane), so the store stays the source of
    /// truth for what the map present last put on glass.
    diff: &'b mut RowDiff<FB_H>,
    /// Per-frame command sequence — bumped each push, echoed back by the FLPR as the ack.
    seq: u32,
}

impl<'b> Ls021Flpr<'b> {
    /// Build the backend over the resident **device-64 map/ride plane** (`main.rs`): the app quantises
    /// to the device-64 gamut itself ([`FbDevice64`](obc_platform::FbDevice64)) and renders straight
    /// into `fb`, then [`present_within`](Self::present_within) diffs + drives it. The FLPR packs `fb`
    /// straight to the wire, so there is no RGB565 band scratch (the ~7.5 KB the ST7789 band push needs
    /// is freed here, issue #165). `fb` must be `FB_W × FB_H` device-64 bytes; `diff` is the
    /// `FB_H`-row [`RowDiff`] store the self-diffing present compares against (issue #201).
    pub fn new_fb(fb: &'b mut [u8], diff: &'b mut RowDiff<FB_H>) -> Self {
        Self { fb, diff, seq: 0 }
    }

    /// The resident RGB222 plane, for the map path to render into (device-64, `0b00_RR_GG_BB` per
    /// pixel) before [`push_frame`](Self::push_frame). The ST7789 path keeps its framebuffer beside
    /// the panel; the FLPR backend owns it, so this is how the app reaches it.
    pub fn fb_mut(&mut self) -> &mut [u8] {
        self.fb
    }

    /// Drive a **span-masked** frame to glass through the ping-pong path and busy-wait the ack — the
    /// engine behind [`push_spans`](Self::push_spans) and [`push_frame`](Self::push_frame) (issue
    /// #163). Publishes the span list, pre-packs the first two **dirty** rows into `buf[0]`/`buf[1]`,
    /// rings the FLPR, then packs each remaining dirty row into whichever buffer the FLPR just freed —
    /// the M33 stays one buffer ahead while the FLPR fast-forwards the clean rows and scans the dirty
    /// ones. `publish(fb, buf_i, abs_row)` packs absolute `abs_row` into `buf[buf_i]` + bumps `ready`
    /// (straight from `fb`, or composited for the bulge — issue #163). The ping-pong index toggles
    /// per **dirty** row (ascending across spans), matching the FLPR's `drain_row(dirty & 1)`.
    /// Returns `true` if the ack checks out (`status == #dirty rows && flpr_seq == seq`). Logs the
    /// **pack-vs-frame overlap** + frame time (the speed-tune metric).
    fn run_masked(&mut self, spans: &[(u16, u16)], mut publish: impl FnMut(&[u8], usize, usize)) -> bool {
        let total = span_row_total(spans);
        if total == 0 {
            return true; // nothing dirty — a no-op frame (defensive; callers pass ≥1 row)
        }
        self.seq += 1;
        let s = self.seq;

        // Reset + publish the span list + pre-fill up to two dirty rows while the FLPR is idle (it
        // starts only on the m33_seq bump). Dirty rows walk ascending across spans; the buffer index
        // toggles per dirty row (`k & 1`), exactly as the FLPR's `drain_row(dirty & 1)` consumes them.
        reset_descriptors();
        set_spans(spans);
        let mut rows = spans
            .iter()
            .take(MAX_DIRTY_SPANS)
            .flat_map(|&(start, count)| start as usize..start as usize + count as usize)
            .enumerate();
        for _ in 0..2 {
            if let Some((k, row)) = rows.next() {
                publish(self.fb, k & 1, row);
            }
        }

        let t_frame = Instant::now();
        ring_cmd(s);

        // Pack the remaining dirty rows, each paced by the FLPR freeing its buffer (the ping-pong).
        let mut pack_total_us: u64 = 0;
        for (k, row) in rows {
            let i = k & 1;
            if !spin_until(|| buf_consumed(i) == buf_ready(i)) {
                error!(
                    "LS021 FLPR: STALLED at dirty row {=usize} (abs {=usize}) — FLPR didn't free buf[{=usize}] (consumed={=u32}, ready={=u32})",
                    k, row, i, buf_consumed(i), buf_ready(i)
                );
                dump_flpr_state(s);
                return false;
            }
            let t0 = Instant::now();
            publish(self.fb, i, row);
            pack_total_us += t0.elapsed().as_micros();
        }

        // Every dirty row packed — busy-wait the FLPR's frame ack (`flpr_seq` echoes our seq).
        if !spin_until(|| unsafe { addr_of!((*CONTROL).flpr_seq).read_volatile() } == s) {
            error!(
                "LS021 FLPR: frame TIMEOUT — FLPR never echoed seq {=u32} (the scan or a ping-pong wait stalled)",
                s
            );
            dump_flpr_state(s);
            return false;
        }
        let frame_us = t_frame.elapsed().as_micros();
        let status = unsafe { addr_of!((*CONTROL).status).read_volatile() };
        let frames = unsafe { addr_of!((*CONTROL).frame_count).read_volatile() };
        if status != total as u32 {
            error!("LS021 FLPR: frame MISMATCH — status={=u32} (want {=usize} dirty rows)", status, total);
            return false;
        }
        info!(
            "LS021 FLPR: frame OK — FLPR scanned {=usize} dirty rows (frame #{=u32}) in {=u64} µs (~{=u64} µs/row); M33 packed {=usize} rows in {=u64} µs",
            total,
            frames,
            frame_us,
            frame_us / total as u64,
            total.saturating_sub(2),
            pack_total_us
        );
        true
    }

    /// Drive **only the rows in `spans`** to glass (issue #163), packing each straight from the clean
    /// framebuffer. The FLPR fast-forwards the gate over the gaps and early-stops after the last dirty
    /// row, so a vertically-compact region costs a fraction of a full frame. Spans are
    /// `(start_row, count)`, ascending + disjoint, ≤ [`MAX_DIRTY_SPANS`]. A full frame is
    /// `&[(0, FB_H)]` (= [`push_frame`](Self::push_frame)).
    pub fn push_spans(&mut self, spans: &[(u16, u16)]) -> bool {
        self.run_masked(spans, publish_row)
    }

    /// Drive the **whole** resident framebuffer to glass — the degenerate one-span frame
    /// `&[(0, FB_H)]` (init-black + a forced full repaint), byte-identical to the F5 full-frame scan.
    /// Does **not** touch the [`RowDiff`] store (it's a raw push, not a diff), so the next
    /// [`present_within`](Self::present_within) still re-seeds the store from a clean comparison.
    pub fn push_frame(&mut self) -> bool {
        self.push_spans(&[(0, FB_H as u16)])
    }

    /// The **self-diffing present** (issue #201): re-hash every framebuffer row against the
    /// [`RowDiff`] store and push only the rows that actually changed, optionally clipping the live
    /// hold bulge's rows out (`exclude`) so [`push_overlay`](Self::push_overlay) owns them (the
    /// `map_rows_around` discipline, now inside the present path — issue #163). The first call after
    /// boot / a [`RowDiff::reset`](obc_platform::RowDiff::reset) pushes the whole frame and seeds the
    /// store; thereafter an idle Home clock tick repaints just its clock band.
    ///
    /// `exclude = None` ⇒ the whole frame is eligible (no bulge); `Some((y0, rows))` ⇒ the rows
    /// `[y0, y0+rows)` are left for the overlay composite. The store is updated for **every** row
    /// (including the clipped ones) — it tracks the clean `fb`, so when the bulge later goes quiet the
    /// trailing clear re-pushes those rows clean and the store already agrees (no stale row). Returns
    /// `false` on a transport fault (a stalled FLPR) so the caller keeps the last frame and retries.
    pub fn present_within(&mut self, exclude: Option<(u16, u16)>) -> bool {
        // Half-open bulge interval [e0, e1) the clip removes from each changed span.
        let ex = exclude.map(|(y0, rows)| (y0, y0 + rows));
        let mut spans: heapless::Vec<(u16, u16), MAX_DIRTY_SPANS> = heapless::Vec::new();
        let mut overflow = false;
        // Diff the whole frame (the store is updated for every row), emitting each changed span clipped
        // around the bulge. The diff piggybacks on a single per-row hash pass over `fb`.
        self.diff.diff(self.fb, FB_W, |y0, n| {
            clip_span(y0, n, ex, &mut |s, c| {
                if spans.push((s, c)).is_err() {
                    overflow = true;
                }
            });
        });
        if overflow {
            // Pathological fragmentation (> MAX_DIRTY_SPANS disjoint changed regions — a UI never
            // produces this): fall back to the whole frame minus the bulge (the `map_rows_around`
            // ceiling) rather than silently dropping spans and stranding rows.
            spans.clear();
            clip_span(0, FB_H as u16, ex, &mut |s, c| {
                let _ = spans.push((s, c));
            });
        }
        if spans.is_empty() {
            return true; // nothing changed outside the bulge — push nothing (the whole point).
        }
        self.push_spans(&spans)
    }

    /// Re-present **only the rows of an overlay region** with `draw_overlay` composited over the clean
    /// framebuffer backdrop (issue #163) — the few-ms partial push the hold bulge rides over a static
    /// map, keeping `fb` the clean map (never mutated). The LS021 is row-addressed (touching a row
    /// re-latches all 240 columns), so this rewrites the full-width rows `[y0, y0+rows)` while only the
    /// `[x0, x0+w)` columns carry the overlay; the FLPR fast-forwards the gate to `y0` and early-stops
    /// after `y0+rows`, so the cost scales to the region's row span, not the whole frame.
    ///
    /// **Stack-frugal + lock-light** — the explicit fix for the old per-row overflow: the overlay is
    /// rendered into a small RGB565 window scratch **once** (one `draw_overlay` call ⇒ the caller's
    /// `InputPlane` lock is taken once per overlay frame, not per row), then each dirty row is packed
    /// as the clean `fb` columns with the `[x0, x0+w)` columns swapped for the composited window
    /// (re-quantised inline) — no per-row lock and no per-row re-render.
    pub fn push_overlay(
        &mut self,
        x0: u16,
        y0: u16,
        w: u16,
        rows: u16,
        draw_overlay: &mut dyn FnMut(&mut Band),
    ) -> bool {
        let (x0, y0, w, rows) = (x0 as usize, y0 as usize, w as usize, rows as usize);
        assert!(
            w <= MAX_OVERLAY_COLS && rows <= MAX_OVERLAY_ROWS && x0 + w <= FB_W && y0 + rows <= FB_H,
            "overlay region out of bounds / larger than the composite scratch"
        );

        // 1. Composite the overlay ONCE into a window scratch over the clean `fb` backdrop, via the
        //    shared `composite_overlay_window` (#174): it fills the window from `fb` (device-64 → RGB565)
        //    and lets `draw_overlay` paint the bulge over it through a frame-absolute `Band`. `win` then
        //    holds the composited region; `fb` is untouched. This is the exact step the ST7789 backend
        //    runs — only the re-quantising wire-pack below is LS021-specific.
        let mut win = [0u16; MAX_OVERLAY_COLS * MAX_OVERLAY_ROWS];
        let window = Rectangle::new(Point::new(x0 as i32, y0 as i32), Size::new(w as u32, rows as u32));
        composite_overlay_window(self.fb, Size::new(FB_W as u32, FB_H as u32), window, &mut win, draw_overlay);

        // 2. Drive the full-width span `[y0, y0+rows)`: each dirty row = the clean `fb` columns with the
        //    `[x0, x0+w)` columns replaced by the composited window (re-quantised to device-64). One
        //    reused 240-byte row, packed straight to the ping-pong buffer — no lock, no re-render.
        self.run_masked(&[(y0 as u16, rows as u16)], |fb, buf_i, abs_row| {
            let srow = abs_row - y0;
            let mut row_dev = [0u8; FB_W];
            row_dev.copy_from_slice(&fb[abs_row * FB_W..abs_row * FB_W + FB_W]);
            for c in 0..w {
                // rgb565_to_device64 returns 0/85/170/255 per channel; /85 recovers the 2-bit level.
                let (r, g, b) = rgb565_to_device64(win[srow * w + c]);
                row_dev[x0 + c] = ((r / 85) << 4) | ((g / 85) << 2) | (b / 85);
            }
            // SAFETY: `out` is the SHARED-page write buffer at a fixed address no Rust object aliases.
            let out = unsafe { core::slice::from_raw_parts_mut(WRITE_BUF_ADDR[buf_i] as *mut u32, ROW_WORDS) };
            ls021_pack_row(&row_dev, out);
            cortex_m::asm::dsb(); // buffer words complete before the ready bump the FLPR waits on
            let next = buf_ready(buf_i).wrapping_add(1);
            unsafe { addr_of_mut!((*CONTROL).buf[buf_i].ready).write_volatile(next) };
        })
    }
}
