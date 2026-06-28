//! **The LS021B7DD02 FLPR `Panel` backend, shared between the bring-up bin and the real app.**
//!
//! Lifted out of `src/bin/ls021_flpr_bringup.rs` (epic #149, F5/PR #162) so `main.rs` can run the
//! real [`obc_app::App`](obc_app) on the reflective LS021 panel through the *same* board-agnostic
//! [`obc_platform::Panel`] seam the ST7789 uses (issue #165). The bring-up bin keeps driving test
//! patterns through it; `main.rs` drives the live map/ride render through it — no panel-specific code
//! in either.
//!
//! What lives here is everything that talks to the **FLPR** (the nRF54L15's VPR RISC-V coprocessor):
//!   - the cross-core [`Control`] block + ping-pong [`BufDesc`]s (the normative contract with the C
//!     blob `src/flpr/flpr_pingpong.c` — kept byte-for-byte in sync, both static-assert 64 bytes);
//!   - [`launch_flpr`] — copy the blob into FLPR RAM, arm the control block, release the core, and
//!     wait for its `ALIVE` stamp;
//!   - [`Ls021Flpr`] — the resident-framebuffer [`Panel`] backend whose `end_frame` packs the whole
//!     RGB222 plane through the two ping-pong buffers and busy-waits one `CMD_RUN_FRAME`.
//!
//! **COM stays on the M33** (`ls021::com_task`) and **is not here** — if the FLPR ever faults, COM
//! must keep alternating so the panel never takes a DC bias (the epic's safety rule). The caller
//! owns COM + the high-priority `InterruptExecutor` it free-runs on.
//!
//! ## Full-frame push per `end_frame` (the design choice)
//!
//! The FLPR scans the *whole* frame top-to-bottom in **one** `CMD_RUN_FRAME`, so a band push can't
//! reach glass on its own: the seam is **full-frame push per `end_frame`**. The app renders the whole
//! frame into the resident RGB222 plane first — the map path writes it directly as device-64
//! ([`FbDevice64`](obc_platform::FbDevice64)) via [`fb_mut`](Ls021Flpr::fb_mut); the glass-demo draws
//! it band-by-band through [`flush_band`](Panel::flush_band), which quantises each RGB565 band into
//! the plane — and then `end_frame` drives all 320 rows once. This matches how the FLPR works and
//! keeps the ping-pong (M33 packs row N+1 while the FLPR scans row N) exactly as F4 proved it.
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
use obc_platform::{ls021_pack_row, Band, Panel};
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
#[allow(dead_code)] // reserved / EGU-era fields are forward-compat headroom kept in the contract.
struct Control {
    magic: u32,         // 0x00 M33: layout/version tag, checked by the FLPR before acting
    m33_seq: u32,       // 0x04 M33: command sequence counter (the per-frame command doorbell)
    cmd: u32,           // 0x08 M33: command word (a CMD_* code)
    flpr_seq: u32,      // 0x0C FLPR: echoes the m33_seq it serviced (the ack the M33 polls)
    status: u32,        // 0x10 FLPR: ack/result (rows scanned; boot: FLPR_ALIVE)
    frame_count: u32,   // 0x14 FLPR: frames drained (bumped per CMD_RUN_FRAME)
    buf: [BufDesc; 2],  // 0x18, 0x28 ping-pong row-buffer descriptors (buf[0] even rows, buf[1] odd)
    reserved: [u32; 2], // 0x38 forward-compat headroom
}
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
/// Lock the cross-language contract: the C `flpr_control_t` `_Static_assert`s the same size.
const _: () = assert!(core::mem::size_of::<Control>() == 64);

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
}

impl<'b> Ls021Flpr<'b> {
    /// Backend for the **device-64 map/ride path** (`main.rs`): the app quantises to the device-64
    /// gamut itself ([`FbDevice64`](obc_platform::FbDevice64)) and renders straight into `fb`, then
    /// [`push_frame`](Self::push_frame) drives it. No RGB565 band scratch — `flush_band` is unused on
    /// this path (the empty `band` is never touched), which is what frees the ~7.5 KB the ST7789 band
    /// push needs (issue #165). `fb` must be `FB_W × FB_H` device-64 bytes.
    pub fn new_fb(fb: &'b mut [u8]) -> Self {
        Self { fb, band: &mut [], seq: 0 }
    }

    /// Backend for **whole-frame RGB565 generators** (the bring-up glass-demo): [`flush_band`] hands
    /// the generator a `band` of RGB565 scratch and quantises each band into `fb`. `band` sizes the
    /// band height (`band.len() / FB_W` rows). (Bin-only — the app uses [`new_fb`](Self::new_fb).)
    pub fn new_banded(fb: &'b mut [u8], band: &'b mut [u16]) -> Self {
        Self { fb, band, seq: 0 }
    }

    /// The resident RGB222 plane, for the map path to render into (device-64, `0b00_RR_GG_BB` per
    /// pixel) before [`push_frame`](Self::push_frame). The ST7789 path keeps its framebuffer beside
    /// the panel; the FLPR backend owns it, so this is how the app reaches it.
    pub fn fb_mut(&mut self) -> &mut [u8] {
        self.fb
    }

    /// Drive the resident framebuffer to glass through the **ping-pong** path and busy-wait the
    /// frame ack. Pre-packs rows 0/1 into `buf[0]`/`buf[1]`, rings the FLPR, then packs each
    /// remaining row into whichever buffer the FLPR just freed (`buf[row & 1]`) — the M33 stays one
    /// buffer ahead while the FLPR scans. Returns `true` if the ack checks out (`status ==
    /// ROWS_PER_FRAME && flpr_seq == seq`). Logs the **pack-vs-frame overlap** + the frame time
    /// (the speed-tune metric — drive the blob clocks down until this nears the spec ~53 ms).
    pub fn push_frame(&mut self) -> bool {
        self.seq += 1;
        let s = self.seq;

        // Reset + pre-fill both buffers while the FLPR is idle (it starts only on the m33_seq bump).
        reset_descriptors();
        publish_row(self.fb, 0, 0); // row 0 → buf[0] (even)
        publish_row(self.fb, 1, 1); // row 1 → buf[1] (odd)

        let t_frame = Instant::now();
        ring_cmd(s);

        // Pack the remaining 318 rows, each paced by the FLPR freeing its buffer (the ping-pong).
        let mut pack_total_us: u64 = 0;
        for row in 2..FB_H {
            let i = row & 1;
            if !spin_until(|| buf_consumed(i) == buf_ready(i)) {
                error!(
                    "LS021 FLPR: STALLED at row {=usize} — FLPR didn't free buf[{=usize}] (consumed={=u32}, ready={=u32})",
                    row, i, buf_consumed(i), buf_ready(i)
                );
                return false;
            }
            let t0 = Instant::now();
            publish_row(self.fb, i, row);
            pack_total_us += t0.elapsed().as_micros();
        }

        // Every row packed — busy-wait the FLPR's whole-frame ack (`flpr_seq` echoes our seq).
        if !spin_until(|| unsafe { addr_of!((*CONTROL).flpr_seq).read_volatile() } == s) {
            error!(
                "LS021 FLPR: frame TIMEOUT — FLPR never echoed seq {=u32} (the scan or a ping-pong wait stalled)",
                s
            );
            return false;
        }
        let frame_us = t_frame.elapsed().as_micros();
        let status = unsafe { addr_of!((*CONTROL).status).read_volatile() };
        let frames = unsafe { addr_of!((*CONTROL).frame_count).read_volatile() };
        if status != ROWS_PER_FRAME {
            error!("LS021 FLPR: frame MISMATCH — status={=u32} (want {=u32})", status, ROWS_PER_FRAME);
            return false;
        }
        info!(
            "LS021 FLPR: frame OK — FLPR scanned {=u32} rows (frame #{=u32}) in {=u64} µs (~{=u64} µs/row); M33 packed {=u32} rows in {=u64} µs",
            status,
            frames,
            frame_us,
            frame_us / ROWS_PER_FRAME as u64,
            ROWS_PER_FRAME - 2,
            pack_total_us
        );
        true
    }
}

impl Panel for Ls021Flpr<'_> {
    /// Band height = however many full `WIDTH`-pixel rows the RGB565 scratch holds (0 for the
    /// `new_fb` map path, which never bands).
    fn band_rows(&self) -> u16 {
        (self.band.len() / FB_W) as u16
    }

    /// Nothing to set up — the resident plane is filled band-by-band, then driven by `end_frame`.
    fn begin_frame(&mut self) {}

    /// Render one band into the RGB565 scratch, then **quantise it into the resident RGB222 plane**
    /// at rows `[y0, y0 + rows)`: each pixel is snapped to the device-64 gamut by the host-tested
    /// [`rgb565_to_device64`] (the same quantiser the glass-demo's swatches are drawn from) and
    /// stored as a `0b00_RR_GG_BB` byte. No panel signal here — `end_frame` drives the whole plane.
    /// Only the glass-demo path uses this; the map path renders device-64 directly via [`fb_mut`].
    fn flush_band(&mut self, y0: u16, rows: u16, fill: impl FnOnce(&mut [u16])) {
        let n = FB_W * rows as usize;
        fill(&mut self.band[..n]);
        let base = y0 as usize * FB_W;
        for (i, &px) in self.band[..n].iter().enumerate() {
            // rgb565_to_device64 returns 0/85/170/255 per channel; /85 recovers the 2-bit level.
            let (r, g, b) = rgb565_to_device64(px);
            self.fb[base + i] = ((r / 85) << 4) | ((g / 85) << 2) | (b / 85);
        }
    }

    /// Drive the now-filled resident plane to glass over the ping-pong path (one `CMD_RUN_FRAME`),
    /// then busy-wait the ack. The full-frame push the seam is built around — see the module doc.
    fn end_frame(&mut self) {
        self.push_frame();
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
