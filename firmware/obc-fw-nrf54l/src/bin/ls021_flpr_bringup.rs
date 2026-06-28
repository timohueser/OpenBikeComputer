//! **LS021 FLPR bring-up F5** (issue #155, epic #149) — **the bridge to running the app.** The
//! FLPR-driven frame push now lives behind the board-agnostic [`obc_platform::Panel`] seam, so the
//! whole-frame generators that already drive the ST7789 (`demo::font_palette_demo`, and ultimately
//! [`App::render_frame`](obc_app::App::render_frame)) put pixels on the real LS021 with **no
//! panel-specific code** — and the waveform clocks step toward the panel's real ~53 ms full-frame
//! speed (vs the ~1.8 s bring-up rate).
//!
//! Successor to **F4** (#154, the FLPR driving a frame from the RGB222 framebuffer over two
//! ping-pong write buffers). F5 keeps the complete waveform + ping-pong pipeline F4 built and adds:
//!   - **[`Ls021Flpr`]** — an `obc_platform::Panel` backend whose `begin_frame` / `flush_band` /
//!     `end_frame` route a band's RGB565 pixels onto glass through the F4 path (framebuffer → pack →
//!     ping-pong → FLPR). It owns the resident 75 KB RGB222 [`FB`] plane; the generator draws the
//!     frame band-by-band through [`obc_platform::Band`], `flush_band` quantises each band into the
//!     plane, and **`end_frame` runs the whole-frame FLPR push**.
//!   - the **glass-demo** (`demo::font_palette_demo` — the font ladder + 64-colour gamut the ST7789
//!     `--features glass-demo` build renders) driven through that backend, on the real panel.
//!   - a first **speed step** in the C blob (`src/flpr/flpr_pingpong.c`) toward the spec frame, with
//!     the remaining bench dial-in (LA edges clean at `BCK ≤ 0.758 MHz`) documented there.
//!
//! ## Full-frame push per `end_frame` (the design choice)
//!
//! The FLPR scans the *whole* frame top-to-bottom in **one** `CMD_RUN_FRAME`, so there is no natural
//! "band-incremental feed" to the panel: a band push can't reach glass on its own. Hence the seam is
//! **full-frame push per `end_frame`** — `flush_band` only *fills* the resident framebuffer plane
//! (RGB565 → device-64), and `end_frame` packs all 320 rows through the ping-pong buffers and drives
//! the frame once. This matches how the FLPR works and keeps the ping-pong (M33 packs row N+1 while
//! the FLPR scans row N) exactly as F4 proved it.
//!
//! ## Blocking push (sync `Panel` seam)
//!
//! [`Panel`] is a synchronous trait, so [`Ls021Flpr::end_frame`] **busy-polls** rather than awaiting:
//! it spins on each ping-pong buffer's `consumed == ready` (the M33 is a dedicated packer here) and
//! on the FLPR's `flpr_seq` ack — no EGU20 IRQ needed (the F4 async return doorbell). COM still
//! free-runs on its own high-priority `InterruptExecutor`, so blocking the thread-mode M33 for a
//! frame is benign (the same shape as the ST7789 path blocking on its SPI-DMA write). The blob still
//! pokes `EGU20` after each frame; with its IRQ unarmed here that write is a harmless no-op.
//!
//! Power-on sequence (datasheet §6-2, mirroring the L3/F4 bins):
//!   1. **Settle (~2 s)** — rails up, all inputs `Lo`, COM `Lo`. Hands-free for a deterministic LA
//!      capture at reset; LED1 blinks.
//!   2. **Launch the FLPR**, wait for its `ALIVE` stamp (F1/F2 boot).
//!   3. **Init #0 — `INTB`-framed all-black frame** through the `Panel` seam (a black `clear`). COM `Lo`.
//!   4. **Wait `T4 ≥ 30 µs`, then start COM** on a high-priority `InterruptExecutor` — runs forever.
//!   5. **BTN0 steps** the screen: GLASS-DEMO → white → red → green → blue → wrap. Each is drawn
//!      through the `Panel`/`Band` seam and FLPR-driven once (MIP retains it). The glass-demo card
//!      must look identical to the ST7789 `--features glass-demo` build; the solids give clean
//!      single-value waveforms for the LA speed-tune.
//!
//! **Both cores on P2 at once (since F3).** The FLPR drives the source bus on `P2.00..06`; the M33
//! drives COM on `P2.07/08/10` from `com_task`. Safe because every GPIO touch on either core is an
//! atomic `OUTSET`/`OUTCLR` of disjoint pin masks. The gate lines (`GSP`/`GCK`/`GEN`/`INTB`) + `BSP`
//! are all on **P1**, FLPR-driven.
//!
//! Build/flash (needs a RISC-V gcc for the blob — `brew install riscv64-elf-gcc`; and the
//! Board-Configurator ext-memory-off / 3.3 V-VDDM settings the `ls021_bringup` epic already needs):
//! ```sh
//! cargo run --release --bin ls021_flpr_bringup --features ls021-flpr
//! ```

#![no_std]
#![no_main]

use core::ptr::{addr_of, addr_of_mut};

use defmt::{error, info, warn};
use embassy_executor::{InterruptExecutor, Spawner};
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::interrupt;
use embassy_nrf::interrupt::{InterruptExt, Priority};
use embassy_time::{Instant, Timer};
use embedded_graphics::{pixelcolor::Rgb565, prelude::*};
// The board-agnostic display seam + its frame-absolute band view, and the host-tested RGB222 →
// LS021-wire pack (#154) with its sub-line/row word counts.
use obc_platform::ls021_wire::{BCK_PER_SUBLINE, ROW_WORDS, WIDTH};
use obc_platform::{ls021_pack_row, Band, Panel};
// The host-tested RGB565 → device-64 quantiser — the same one the glass-demo's gamut is drawn from,
// so `flush_band` lands a band on the panel's RGB222 gamut exactly as the ST7789 stand-in shows it.
use obc_reader::rgb565_to_device64;
use {defmt_rtt as _, panic_probe as _};

// The free-running COM driver is panel-board-agnostic infrastructure (not the M33-direct PanelBus,
// which the FLPR replaces). Pull in `com_task`; the rest of the module is unused here (module-level
// allow). The glass-demo generator is shared verbatim with the ST7789 `--features glass-demo` build.
#[path = "../demo.rs"]
mod demo;
#[path = "../ls021.rs"]
#[allow(dead_code)]
mod ls021;
use demo::font_palette_demo;
use ls021::com_task;

/// The FLPR program image, cross-compiled by `build.rs` into `$OUT_DIR/flpr.bin`.
static FLPR_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flpr.bin"));

// ── FLPR memory map (must match src/flpr/flpr.ld + the carved memory.x in build.rs) ──
/// FLPR execution base: the M33 copies the blob here and points `INITPC` at it.
const FLPR_RAM_BASE: usize = 0x2003_8000;
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
const ROWS_PER_FRAME: u32 = 320;
/// Framebuffer width = the panel width.
const FB_W: usize = WIDTH;
/// Framebuffer height = the visible row count.
const FB_H: usize = ROWS_PER_FRAME as usize;
/// Resident RGB222 (device-64) framebuffer, one byte per pixel (`0b00_RR_GG_BB`, the
/// [`FbDevice64`](obc_platform::FbDevice64)/`PackDevice64` format). 75 KB; fits the 224 KB M33
/// region. This is the production map plane's type/size — F5 fills it via the `Panel` seam, N7 plugs
/// the real `App` render in behind the *same* seam. `ls021_pack_row` reads one row per buffer fill.
static mut FB: [u8; FB_W * FB_H] = [0u8; FB_W * FB_H];

/// One band's worth of RGB565 scratch the [`Panel`] seam hands the generator (`BAND_ROWS` full
/// `WIDTH`-pixel rows). The frame is resident in [`FB`]; this is only the transient per-band buffer
/// the generator draws into before `flush_band` quantises it into the plane, so it can be small.
const BAND_ROWS: usize = 16;
static mut BAND: [u16; FB_W * BAND_ROWS] = [0u16; FB_W * BAND_ROWS];

/// High-priority executor the COM driver runs on, pended from the unused SWI00 software-interrupt
/// vector. COM at P3 preempts thread-mode so it free-runs CPU-independently while the M33 busy-polls
/// the FLPR's per-frame ack.
static EXECUTOR_COM: InterruptExecutor = InterruptExecutor::new();

#[interrupt]
unsafe fn SWI00() {
    EXECUTOR_COM.on_interrupt();
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

/// Busy-poll cap while waiting on the FLPR (a free ping-pong buffer or the frame ack). A spin cap,
/// not a duration — sized large enough that even a full slow frame never trips it, so it only fires
/// if the FLPR has genuinely stalled, turning a hang into a reported error.
const SPIN_CAP: u32 = 50_000_000;

/// The board-agnostic [`Panel`] backend for the LS021 over the FLPR. Owns the resident RGB222
/// framebuffer plane; the generator draws the frame band-by-band into the RGB565 `band` scratch via
/// [`flush_band`](Panel::flush_band), which quantises each band into the plane, and
/// [`end_frame`](Panel::end_frame) drives the whole frame to glass through the F4 ping-pong path.
struct Ls021Flpr<'b> {
    /// Resident RGB222 (device-64) frame plane, `FB_W × FB_H`. `flush_band` writes it, `end_frame`
    /// packs + pushes it.
    fb: &'b mut [u8],
    /// One band of RGB565 scratch the seam hands the generator; quantised into `fb` per band.
    band: &'b mut [u16],
    /// Per-frame command sequence — bumped each push, echoed back by the FLPR as the ack.
    seq: u32,
}

impl Ls021Flpr<'_> {
    /// Drive the resident framebuffer to glass through the **ping-pong** path and busy-wait the
    /// frame ack. Pre-packs rows 0/1 into `buf[0]`/`buf[1]`, rings the FLPR, then packs each
    /// remaining row into whichever buffer the FLPR just freed (`buf[row & 1]`) — the M33 stays one
    /// buffer ahead while the FLPR scans. Returns `true` if the ack checks out (`status ==
    /// ROWS_PER_FRAME && flpr_seq == seq`). Logs the **pack-vs-frame overlap** + the frame time
    /// (the F5 speed-tune metric — drive the blob clocks down until this nears the spec ~53 ms).
    fn push_frame(&mut self) -> bool {
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
                    "LS021 FLPR F5: STALLED at row {=usize} — FLPR didn't free buf[{=usize}] (consumed={=u32}, ready={=u32})",
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
                "LS021 FLPR F5: frame TIMEOUT — FLPR never echoed seq {=u32} (the scan or a ping-pong wait stalled)",
                s
            );
            return false;
        }
        let frame_us = t_frame.elapsed().as_micros();
        let status = unsafe { addr_of!((*CONTROL).status).read_volatile() };
        let frames = unsafe { addr_of!((*CONTROL).frame_count).read_volatile() };
        if status != ROWS_PER_FRAME {
            error!("LS021 FLPR F5: frame MISMATCH — status={=u32} (want {=u32})", status, ROWS_PER_FRAME);
            return false;
        }
        info!(
            "LS021 FLPR F5: frame OK — FLPR scanned {=u32} rows (frame #{=u32}) in {=u64} µs (~{=u64} µs/row); M33 packed {=u32} rows in {=u64} µs → pack ≪ drain, pipeline overlaps",
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

impl Panel for Ls021Flpr<'_> {
    /// Band height = however many full `WIDTH`-pixel rows the RGB565 scratch holds.
    fn band_rows(&self) -> u16 {
        (self.band.len() / FB_W) as u16
    }

    /// Nothing to set up — the resident plane is filled band-by-band, then driven by `end_frame`.
    fn begin_frame(&mut self) {}

    /// Render one band into the RGB565 scratch, then **quantise it into the resident RGB222 plane**
    /// at rows `[y0, y0 + rows)`: each pixel is snapped to the device-64 gamut by the host-tested
    /// [`rgb565_to_device64`] (the same quantiser the glass-demo's swatches are drawn from) and
    /// stored as a `0b00_RR_GG_BB` byte. No panel signal here — `end_frame` drives the whole plane.
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

/// Draw a whole-frame generator onto the panel through the [`Panel`]/[`Band`] seam: clear/fill the
/// resident plane band-by-band (each band gets the *whole* frame drawn into it, clipped to its rows
/// by [`Band`], so it reassembles seam-free), then drive the frame. The exact loop `main.rs`'s
/// ST7789 glass-demo uses — proof the same generator drives both panels unchanged.
fn show(panel: &mut Ls021Flpr, name: &str, gen: impl Fn(&mut Band)) {
    info!("LS021 FLPR F5: SHOW {=str} — press BTN0 for next", name);
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

/// Wait for one clean BTN0 press: drain any still-held press (so a held button can't double-step),
/// then poll every ~5 ms for a debounced press edge. (Same as the L3/F4 bins.)
async fn wait_for_press(btn: &Input<'_>) {
    while btn.is_low() {
        Timer::after_millis(5).await;
    }
    Timer::after_millis(20).await;
    loop {
        if btn.is_low() {
            Timer::after_millis(15).await;
            if btn.is_low() {
                return;
            }
        }
        Timer::after_millis(5).await;
    }
}

/// Clear `t` to a solid RGB565 colour — a whole-frame generator for the BTN0 solid cards (the clean
/// single-value waveforms the LA speed-tune reads). The device snaps each to its RGB222 gamut.
fn solid(t: &mut Band, c: Rgb565) {
    t.clear(c).ok();
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // Match the rest of the nRF firmware: run the M33 at its full 128 MHz.
    let p = {
        let mut config = embassy_nrf::config::Config::default();
        config.clock_speed = embassy_nrf::config::ClockSpeed::CK128;
        embassy_nrf::init(config)
    };

    // The M33 owns pin *configuration* for every line the FLPR drives; the FLPR only ever pulses
    // OUTSET/OUTCLR (atomic, never an OUT read-modify-write) so the two cores never collide on the
    // shared ports. Kept alive for the life of the program so the pins stay configured. All boot
    // `Output(Lo)` (the datasheet boot-safe state). DK pins are the L0 harness map in
    // `firmware/docs/ls021-bringup.md`.
    //   • Gate + frame (P1): GSP P1.11, GCK P1.12, GEN P1.04, INTB P1.06.
    //   • Source bus: BSP P1.07 (P1) + BCK P2.06 + the 6 data lines P2.00..05 (P2).
    //   • LED0 P2.09: the FLPR's by-eye "drained a frame" marker.
    let _gate_bus = [
        Output::new(p.P1_11, Level::Low, OutputDrive::Standard), // GSP  (gate start pulse)
        Output::new(p.P1_12, Level::Low, OutputDrive::Standard), // GCK  (gate clock / area-plane select)
        Output::new(p.P1_04, Level::Low, OutputDrive::Standard), // GEN  (gate output enable)
        Output::new(p.P1_06, Level::Low, OutputDrive::Standard), // INTB (frame envelope)
    ];
    let _src_bus = [
        Output::new(p.P1_07, Level::Low, OutputDrive::Standard), // BSP  (P1.07 — the lone P1 source line)
        Output::new(p.P2_06, Level::Low, OutputDrive::Standard), // BCK  (P2.06)
        Output::new(p.P2_00, Level::Low, OutputDrive::Standard), // R0   (P2.00, odd)
        Output::new(p.P2_01, Level::Low, OutputDrive::Standard), // R1   (P2.01, even)
        Output::new(p.P2_02, Level::Low, OutputDrive::Standard), // G0   (P2.02, odd)
        Output::new(p.P2_03, Level::Low, OutputDrive::Standard), // G1   (P2.03, even)
        Output::new(p.P2_04, Level::Low, OutputDrive::Standard), // B0   (P2.04, odd)
        Output::new(p.P2_05, Level::Low, OutputDrive::Standard), // B1   (P2.05, even)
    ];
    let _led0 = Output::new(p.P2_09, Level::Low, OutputDrive::Standard); // FLPR's frame marker

    // COM lines as high-drive GPIO (56–77 nF load each), boot `Lo` (safe state); held `Lo` through
    // the init frame, then moved into `com_task`. VCOM=P2.07, VB=P2.08, VA=P2.10 (M33-driven).
    let vcom = Output::new(p.P2_07, Level::Low, OutputDrive::HighDrive);
    let vb = Output::new(p.P2_08, Level::Low, OutputDrive::HighDrive);
    let va = Output::new(p.P2_10, Level::Low, OutputDrive::HighDrive);

    // LED1 (P1.10) = the M33's heartbeat — proves the M33 keeps running alongside the FLPR.
    let mut led1 = Output::new(p.P1_10, Level::Low, OutputDrive::Standard);
    let btn0 = Input::new(p.P1_13, Pull::Up); // DK BTN0 — active-LOW (pressed = Lo); steps the screen

    info!("LS021 FLPR F5: launcher up — blob is {=usize} bytes", FLPR_BLOB.len());

    // 1. Settle window (~2 s, LED1 ~2 Hz): all inputs `Lo`, COM `Lo`. Meter window; hands-free so the
    //    bench LA capture at reset is deterministic.
    info!("LS021 FLPR F5: SETTLE (~2s, all inputs Lo, COM held Lo) — then FLPR boot, init-black, COM, glass-demo");
    for _ in 0..8 {
        led1.toggle();
        Timer::after_millis(250).await;
    }

    // 2. Arm the control block, launch the FLPR, poll for its ALIVE stamp.
    unsafe {
        core::ptr::write_bytes(CONTROL as *mut u8, 0, core::mem::size_of::<Control>());
        addr_of_mut!((*CONTROL).magic).write_volatile(LAYOUT_MAGIC); // FLPR reads magic first thing
    }
    start_flpr();
    info!("LS021 FLPR F5: FLPR released (INITPC=0x{=u32:08x}) — waiting for alive", FLPR_RAM_BASE as u32);

    let mut alive = false;
    for _ in 0..200 {
        match unsafe { addr_of!((*CONTROL).status).read_volatile() } {
            FLPR_ALIVE => {
                alive = true;
                break;
            }
            FLPR_BADMAG => {
                error!(
                    "LS021 FLPR F5: FLPR booted but control-block magic mismatched — memory-map drift? (ls021-flpr.md)"
                );
                break;
            }
            _ => Timer::after_millis(5).await,
        }
    }
    if !alive {
        warn!("LS021 FLPR F5: no alive stamp — FLPR didn't boot or can't reach shared RAM; halting (LED1 blink)");
        loop {
            led1.toggle();
            Timer::after_millis(500).await;
        }
    }
    info!("LS021 FLPR F5: FLPR alive — building the Panel backend, driving the init-black frame (COM still Lo)");

    // Build the Panel backend over the resident framebuffer + the band scratch.
    // SAFETY: the sole references taken to FB/BAND; held by `panel` for the rest of the program and
    // this single-executor build never aliases them (COM/SWI touch neither).
    let mut panel =
        Ls021Flpr { fb: unsafe { &mut *addr_of_mut!(FB) }, band: unsafe { &mut *addr_of_mut!(BAND) }, seq: 0 };

    // 3. Init #0 — an INTB-framed all-black frame, FLPR-driven through the Panel seam. COM not yet up.
    led1.set_high(); // LED1 steady-on marks the init frame on the bench
    show(&mut panel, "INIT-BLACK", |t| solid(t, Rgb565::BLACK));
    led1.set_low();

    // 4. T4 ≥ 30 µs, then start COM on the high-priority interrupt executor (P3). From here COM
    //    free-runs forever — VCOM≡VB in phase, VA inverse, ~60 Hz / 50 %.
    Timer::after_micros(50).await;
    interrupt::SWI00.set_priority(Priority::P3);
    let com_spawner = EXECUTOR_COM.start(interrupt::SWI00);
    com_spawner.spawn(defmt::unwrap!(com_task(vcom, vb, va)));
    info!("LS021 FLPR F5: COM RUNNING — BTN0 steps GLASS-DEMO → LINE-TEST → WHITE → RED → GREEN → BLUE (MIP retains each frame)");

    // 5. BTN0 steps the screen through the Panel seam: the glass-demo (the F5 deliverable — font
    //    ladder + 64-colour gamut, identical to the ST7789 `--features glass-demo` build), the
    //    line/box diagnostic card (tells panel area-gradation texture apart from a pixel bug), then
    //    four solids (clean single-value waveforms for the LA speed-tune). MIP retains each; COM toggles.
    let mut i = 0usize;
    loop {
        match i {
            0 => show(&mut panel, "GLASS-DEMO", |t| {
                font_palette_demo(t).ok();
            }),
            1 => show(&mut panel, "LINE-TEST", |t| {
                demo::line_test_card(t).ok();
            }),
            2 => show(&mut panel, "WHITE", |t| solid(t, Rgb565::WHITE)),
            3 => show(&mut panel, "RED", |t| solid(t, Rgb565::RED)),
            4 => show(&mut panel, "GREEN", |t| solid(t, Rgb565::GREEN)),
            _ => show(&mut panel, "BLUE", |t| solid(t, Rgb565::BLUE)),
        }
        wait_for_press(&btn0).await;
        led1.toggle();
        i = (i + 1) % 6;
    }
}
