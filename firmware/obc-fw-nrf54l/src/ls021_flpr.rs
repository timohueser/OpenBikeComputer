//! **The LS021B7DD02 FLPR `Panel` backend, shared between the bring-up bin and the real app.**
//!
//! Lifted out of `src/bin/ls021_flpr_bringup.rs` (epic #149, F5/PR #162) so `main.rs` can run the
//! real [`obc_app::App`](obc_app) on the reflective LS021 panel through the *same* board-agnostic
//! [`obc_platform::Panel`] seam the ST7789 uses (issue #165). The bring-up bin keeps driving test
//! patterns through it; `main.rs` drives the live map/ride render through it — no panel-specific code
//! in either.
//!
//! What lives here is everything that talks to the **FLPR** (the nRF54L15's VPR RISC-V coprocessor):
//!   - the cross-core [`Control`] block + ping-pong [`BufDesc`]s + dirty-row span list (the normative
//!     contract with the C blob `src/flpr/flpr_pingpong.c` — kept byte-for-byte in sync, both
//!     static-assert 124 bytes);
//!   - [`launch_flpr`] — copy the blob into FLPR RAM, arm the control block, release the core, and
//!     wait for its `ALIVE` stamp;
//!   - [`Ls021Flpr`] — the resident-framebuffer [`Panel`] backend whose pushes pack the dirty rows of
//!     the RGB222 plane through the two ping-pong buffers and busy-wait one masked `CMD_RUN_FRAME`.
//!
//! **COM stays on the M33** (`ls021::com_task`) and **is not here** — if the FLPR ever faults, COM
//! must keep alternating so the panel never takes a DC bias (the epic's safety rule). The caller
//! owns COM + the high-priority `InterruptExecutor` it free-runs on.
//!
//! ## Span-masked push per `end_frame` (the design choice)
//!
//! The FLPR scans a frame in **one** `CMD_RUN_FRAME` driven by a **dirty-row span list** (issue #163):
//! it fast-forwards the gate over the clean rows and shifts+latches only the spanned ones, so a band
//! push can't reach glass on its own — the seam is **a masked push per `end_frame`**. The app renders
//! into the resident RGB222 plane first — the map path writes it directly as device-64
//! ([`FbDevice64`](obc_platform::FbDevice64)) via [`fb_mut`](Ls021Flpr::fb_mut) and drives spans with
//! [`push_spans`](Ls021Flpr::push_spans) / [`push_overlay_spans`](Ls021Flpr::push_overlay_spans); the
//! glass-demo draws it band-by-band through [`flush_band`](Panel::flush_band), which quantises each
//! RGB565 band into the plane *and records its rows*, and `end_frame` drives the recorded spans. A
//! whole-frame draw bands the frame contiguously → one `(0, FB_H)` span = a full frame (the F5
//! behaviour); a backend that bands only the changed rows gets a partial frame for free. The ping-pong
//! (M33 packs the next dirty row while the FLPR scans this one) is exactly as F4 proved it, only the
//! buffer index now toggles per **dirty** row.
//!
//! ## Blocking push (sync `Panel` seam)
//!
//! [`Panel`] is synchronous, so [`Ls021Flpr::end_frame`] **busy-polls** rather than awaiting: it
//! spins on each ping-pong buffer's `consumed == ready` (the M33 is a dedicated packer here) and on
//! the FLPR's `flpr_seq` ack — no EGU20 IRQ needed (the F4 async return doorbell). COM free-runs on
//! its own high-priority `InterruptExecutor`, so blocking the thread-mode M33 for a frame is benign
//! (the same shape as the ST7789 path blocking on its SPI-DMA write). The blob still pokes `EGU20`
//! after each frame; with its IRQ unarmed here that write is a harmless no-op.

// This module is consumed two ways: the bring-up bin drives whole-frame RGB565 generators through the
// banded `Panel` path (`new_banded` + `show`), while the app renders device-64 straight into the plane
// (`new_fb` + `fb_mut`). Each binary leaves the *other's* constructor/path unused, so allow dead code
// here rather than tag the constructors per-consumer.
#![allow(dead_code)]

use core::ptr::{addr_of, addr_of_mut};

use defmt::{error, info};
use embassy_time::{Instant, Timer};
// The host-tested RGB222 → LS021-wire pack (#154) with its sub-line/row word counts.
use obc_platform::ls021_wire::{BCK_PER_SUBLINE, ROW_WORDS, WIDTH};
// `device64_to_rgb565` expands a clean framebuffer row to RGB565 so the overlay-composite path
// (#163) can draw the hold bulge over it through the `Band` window before re-quantising to the wire.
use obc_platform::{device64_to_rgb565, ls021_pack_row, Band, Panel};
// The host-tested RGB565 → device-64 quantiser — the same one the glass-demo's gamut is drawn from,
// so `flush_band` lands a band on the panel's RGB222 gamut exactly as the ST7789 stand-in shows it.
use obc_reader::rgb565_to_device64;

use embedded_graphics::prelude::*;

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
/// Dirty-row span list cap (issue #163) — **must equal `MAX_DIRTY_SPANS` in the C `flpr_pingpong.c`**.
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
/// Framebuffer width = the panel width (re-exported for the resident-plane sizing in the bin/app).
pub const FB_W: usize = WIDTH;
/// Framebuffer height = the visible row count.
pub const FB_H: usize = ROWS_PER_FRAME as usize;

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

/// Pack framebuffer row `row` into ping-pong buffer `i` **with an overlay composited on top** and
/// publish it — the partial-update path the hold bulge rides (issue #163), keeping `fb` itself the
/// clean-map source of truth. Per row: expand the clean `fb` row to an RGB565 scratch
/// ([`device64_to_rgb565`]), let `composite(row, scratch)` draw the bulge over it (the caller wraps
/// the scratch in a 1-row [`Band`] window and calls [`InputPlane::render_overlay`]), then re-quantise
/// each pixel back to the device-64 gamut and pack. The round-trip is **lossless on untouched
/// pixels** (`device64_to_rgb565` → re-quantise is identity on the gamut), so the rows outside the
/// bulge reproduce the clean map seam-free — no readback, no `fb` mutation.
fn publish_row_overlay(fb: &[u8], i: usize, row: usize, composite: &mut dyn FnMut(u16, &mut [u16])) {
    let mut scratch = [0u16; FB_W];
    let base = row * FB_W;
    for (px, &byte) in scratch.iter_mut().zip(&fb[base..base + FB_W]) {
        *px = device64_to_rgb565(byte);
    }
    composite(row as u16, &mut scratch);
    // SAFETY: `out` is the SHARED-page write buffer at a fixed address no Rust object aliases.
    let out = unsafe { core::slice::from_raw_parts_mut(WRITE_BUF_ADDR[i] as *mut u32, ROW_WORDS) };
    let mut dev = [0u8; FB_W];
    for (d, &px) in dev.iter_mut().zip(scratch.iter()) {
        // rgb565_to_device64 returns 0/85/170/255 per channel; /85 recovers the 2-bit level (the
        // same quantise `flush_band` uses, i.e. `rgb565_to_device64_byte`).
        let (r, g, b) = rgb565_to_device64(px);
        *d = ((r / 85) << 4) | ((g / 85) << 2) | (b / 85);
    }
    ls021_pack_row(&dev, out);
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

/// The board-agnostic [`Panel`] backend for the LS021 over the FLPR. Owns the resident RGB222
/// framebuffer plane; the app renders the whole frame into it (the map path writes it directly as
/// device-64 via [`fb_mut`](Self::fb_mut); a whole-frame RGB565 generator draws it band-by-band
/// through [`flush_band`](Panel::flush_band)), and [`end_frame`](Panel::end_frame) /
/// [`push_frame`](Self::push_frame) drive the whole frame to glass over the ping-pong path.
pub struct Ls021Flpr<'b> {
    /// Resident RGB222 (device-64) frame plane, `FB_W × FB_H`. `flush_band`/`fb_mut` write it,
    /// `end_frame`/`push_frame` pack + push it.
    fb: &'b mut [u8],
    /// One band of RGB565 scratch the seam hands a whole-frame generator; quantised into `fb` per
    /// band. **Empty for the map path** ([`new_fb`](Self::new_fb)) — the app renders device-64
    /// straight into `fb`, never through `flush_band`, so no RGB565 band scratch is allocated (the
    /// ~7.5 KB the ST7789 path needs is freed here, issue #165).
    band: &'b mut [u16],
    /// Per-frame command sequence — bumped each push, echoed back by the FLPR as the ack.
    seq: u32,
    /// Dirty-row spans recorded across one `begin_frame`/`flush_band`*/`end_frame` (the banded
    /// [`Panel`] seam, issue #163): each [`flush_band`](Panel::flush_band) appends/coalesces its
    /// `(y0, rows)`, and [`end_frame`](Panel::end_frame) drives exactly those rows via
    /// [`push_spans`](Self::push_spans). A backend that bands only the changed rows gets a partial
    /// frame for free; the bring-up `show()` bands the whole frame contiguously → one `(0, FB_H)` span
    /// = a full frame, unchanged. (Unused on the `new_fb` map path, which drives spans directly.)
    dirty: [(u16, u16); MAX_DIRTY_SPANS],
    dirty_n: usize,
}

impl<'b> Ls021Flpr<'b> {
    /// Backend for the **device-64 map/ride path** (`main.rs`): the app quantises to the device-64
    /// gamut itself ([`FbDevice64`](obc_platform::FbDevice64)) and renders straight into `fb`, then
    /// [`push_frame`](Self::push_frame) drives it. No RGB565 band scratch — `flush_band` is unused on
    /// this path (the empty `band` is never touched), which is what frees the ~7.5 KB the ST7789 band
    /// push needs (issue #165). `fb` must be `FB_W × FB_H` device-64 bytes.
    pub fn new_fb(fb: &'b mut [u8]) -> Self {
        Self { fb, band: &mut [], seq: 0, dirty: [(0, 0); MAX_DIRTY_SPANS], dirty_n: 0 }
    }

    /// Backend for **whole-frame RGB565 generators** (the bring-up glass-demo): [`flush_band`] hands
    /// the generator a `band` of RGB565 scratch and quantises each band into `fb`. `band` sizes the
    /// band height (`band.len() / FB_W` rows). (Bin-only — the app uses [`new_fb`](Self::new_fb).)
    pub fn new_banded(fb: &'b mut [u8], band: &'b mut [u16]) -> Self {
        Self { fb, band, seq: 0, dirty: [(0, 0); MAX_DIRTY_SPANS], dirty_n: 0 }
    }

    /// The resident RGB222 plane, for the map path to render into (device-64, `0b00_RR_GG_BB` per
    /// pixel) before [`push_frame`](Self::push_frame). The ST7789 path keeps its framebuffer beside
    /// the panel; the FLPR backend owns it, so this is how the app reaches it.
    pub fn fb_mut(&mut self) -> &mut [u8] {
        self.fb
    }

    /// Drive a **span-masked** frame to glass through the ping-pong path and busy-wait the ack — the
    /// engine behind [`push_frame`](Self::push_frame), [`push_spans`](Self::push_spans) and
    /// [`push_overlay_spans`](Self::push_overlay_spans) (issue #163). Publishes the span list, pre-packs
    /// the first two **dirty** rows into `buf[0]`/`buf[1]`, rings the FLPR, then packs each remaining
    /// dirty row into whichever buffer the FLPR just freed — the M33 stays one buffer ahead while the
    /// FLPR fast-forwards the clean rows and scans the dirty ones. `publish(fb, buf_i, abs_row)` packs
    /// absolute `abs_row` into `buf[buf_i]` + bumps `ready` (straight from `fb`, or composited for the
    /// bulge). The ping-pong index toggles per **dirty** row (ascending across spans), matching the
    /// FLPR's `drain_row`. Returns `true` if the ack checks out (`status == #dirty rows && flpr_seq ==
    /// seq`). Logs the **pack-vs-frame overlap** + frame time (the speed-tune metric).
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
                publish(&*self.fb, k & 1, row);
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
            publish(&*self.fb, i, row);
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
    /// `fb`. The FLPR fast-forwards the gate over the gaps and early-stops after the last dirty row,
    /// so a vertically-compact region costs a fraction of a full frame. Spans are `(start_row, count)`,
    /// ascending + disjoint, ≤ [`MAX_DIRTY_SPANS`]. A full frame is `&[(0, FB_H)]` (= [`push_frame`]).
    pub fn push_spans(&mut self, spans: &[(u16, u16)]) -> bool {
        self.run_masked(spans, publish_row)
    }

    /// Drive `spans` to glass with the hold **bulge composited on top** (issue #163), keeping `fb` the
    /// clean-map source of truth. For each dirty row `composite(row, scratch)` is handed that row's
    /// RGB565 pixels (pre-filled from the clean `fb`) to draw the bulge over — see
    /// [`publish_row_overlay`] for the seam-free round-trip. The map plane uses this for the bulge
    /// span over a static map, and for a full map frame while the bulge is up.
    pub fn push_overlay_spans(&mut self, spans: &[(u16, u16)], mut composite: impl FnMut(u16, &mut [u16])) -> bool {
        self.run_masked(spans, |fb, i, row| publish_row_overlay(fb, i, row, &mut composite))
    }

    /// Drive the **whole** resident framebuffer to glass — the degenerate one-span frame
    /// `&[(0, FB_H)]` (init-black + every full map redraw). Returns `true` if the ack checks out.
    pub fn push_frame(&mut self) -> bool {
        self.push_spans(&[(0, FB_H as u16)])
    }
}

impl Panel for Ls021Flpr<'_> {
    /// Band height = however many full `WIDTH`-pixel rows the RGB565 scratch holds (0 for the
    /// `new_fb` map path, which never bands).
    fn band_rows(&self) -> u16 {
        (self.band.len() / FB_W) as u16
    }

    /// Clear the recorded dirty-row span set for a new frame (issue #163) — `flush_band` appends to
    /// it, `end_frame` drives exactly those rows.
    fn begin_frame(&mut self) {
        self.dirty_n = 0;
    }

    /// Render one band into the RGB565 scratch, **quantise it into the resident RGB222 plane** at rows
    /// `[y0, y0 + rows)`, **and record `(y0, rows)` as a dirty span** (issue #163). Each pixel is
    /// snapped to the device-64 gamut by the host-tested [`rgb565_to_device64`] (the same quantiser
    /// the glass-demo's swatches are drawn from) and stored as a `0b00_RR_GG_BB` byte. No panel signal
    /// here — `end_frame` drives the recorded spans. Bands arrive ascending, so a contiguous run
    /// coalesces into one span (extend the last when `y0` meets its end); a backend that only bands the
    /// changed rows thus drives a minimal partial frame. Only the glass-demo path bands; the map path
    /// renders device-64 directly via [`fb_mut`](Self::fb_mut) and drives spans itself.
    fn flush_band(&mut self, y0: u16, rows: u16, fill: impl FnOnce(&mut [u16])) {
        let n = FB_W * rows as usize;
        fill(&mut self.band[..n]);
        let base = y0 as usize * FB_W;
        for (i, &px) in self.band[..n].iter().enumerate() {
            // rgb565_to_device64 returns 0/85/170/255 per channel; /85 recovers the 2-bit level.
            let (r, g, b) = rgb565_to_device64(px);
            self.fb[base + i] = ((r / 85) << 4) | ((g / 85) << 2) | (b / 85);
        }
        // Record/coalesce the span. Bands ascend, so extend the last when this one abuts it; else
        // append (clamped to MAX_DIRTY_SPANS — over-cap bands fold into the last, an over-redraw at
        // worst, never a missed row).
        match self.dirty[..self.dirty_n].last_mut() {
            Some((start, count)) if *start + *count == y0 => *count += rows,
            _ if self.dirty_n < MAX_DIRTY_SPANS => {
                self.dirty[self.dirty_n] = (y0, rows);
                self.dirty_n += 1;
            }
            Some((start, count)) => *count = y0 + rows - *start,
            None => unreachable!(),
        }
    }

    /// Drive the recorded dirty-row spans to glass over the ping-pong path, then busy-wait the ack
    /// (issue #163). A whole-frame generator bands the frame contiguously → one `(0, FB_H)` span = a
    /// full frame; a backend that bands only changed rows gets a partial frame for free.
    fn end_frame(&mut self) {
        let n = self.dirty_n;
        let mut spans = [(0u16, 0u16); MAX_DIRTY_SPANS];
        spans[..n].copy_from_slice(&self.dirty[..n]);
        self.push_spans(&spans[..n]);
    }
}

/// Draw a whole-frame RGB565 generator onto the panel through the [`Panel`]/[`Band`] seam: clear/fill
/// the resident plane band-by-band (each band gets the *whole* frame drawn into it, clipped to its
/// rows by [`Band`], so it reassembles seam-free), then drive the frame. The exact loop the ST7789
/// `glass-demo` uses — proof the same generator drives both panels unchanged. (Map/ride frames skip
/// this and render device-64 straight into [`fb_mut`](Ls021Flpr::fb_mut).)
pub fn show(panel: &mut Ls021Flpr, gen: impl Fn(&mut Band)) {
    panel.begin_frame();
    let rows = panel.band_rows();
    let frame = Size::new(FB_W as u32, FB_H as u32);
    let mut y0 = 0u16;
    while (y0 as usize) < FB_H {
        let h = rows.min(FB_H as u16 - y0);
        panel.flush_band(y0, h, |scratch| {
            let mut t = Band::new(scratch, frame, y0, h);
            gen(&mut t);
        });
        y0 += h;
    }
    panel.end_frame();
}
