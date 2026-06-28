//! **LS021 FLPR bring-up F4** (issue #154, epic #149) — the FLPR drives a frame from the **real
//! RGB222 framebuffer** through **two ping-pong write buffers**: a uniform **black** init frame,
//! then **solid white / R / G / B**, the **64-colour palette**, and the **black-on-white shapes**
//! card. **The epic's headline deliverable.**
//!
//! Successor to **F3** (#153, the FLPR driving a whole *solid* frame from one reused row buffer).
//! F4 keeps the complete waveform F3 built (gate scan + `INTB` envelope + source shift, ported from
//! the analyzer-verified M33 [`PanelBus`](../ls021.rs), epic #139) and changes only the **source**:
//!   - the M33 renders a pattern into a resident **75 KB RGB222 framebuffer**
//!     ([`FbDevice64`](obc_platform::FbDevice64)-format bytes, `0b00_RR_GG_BB`);
//!   - it packs that, **one row at a time**, into the LS021 source-bus wire words via the
//!     host-tested [`obc_platform::ls021_pack_row`] (the trickiest logic — area-gradation split +
//!     odd/even column interleave + GPIO pre-shift — lives in a unit-tested Rust fn, not the C blob);
//!   - the two row buffers **ping-pong**: the M33 packs row N+1 into one buffer while the FLPR scans
//!     row N out of the other, swapping on the FLPR's per-buffer "drained" echo.
//!
//! **The ping-pong unit is one gate line = MSB sub-line + LSB sub-line** (a 248-word row buffer);
//! `buf[0]` carries even rows, `buf[1]` odd. Per-buffer back-pressure runs both ways: the FLPR waits
//! for `ready != consumed` before scanning a buffer (never a half-filled one), the M33 waits for
//! `consumed == ready` before refilling it (never one the FLPR is mid-scan on). COM (`VCOM`/`VB`/`VA`)
//! free-runs on the M33 the whole time. See `firmware/docs/ls021-flpr.md`.
//!
//! Power-on sequence (datasheet §6-2, mirroring the L3 `ls021_bringup` bin):
//!   1. **Settle (~2 s)** — rails up, all inputs `Lo`, COM `Lo`. Hands-free so a power-cycle gives a
//!      deterministic LA capture; LED1 blinks.
//!   2. **Launch the FLPR**, arm the EGU20 return doorbell, wait for its `ALIVE` stamp (F1/F2 boot).
//!   3. **Init #0 — `INTB`-framed all-black frame**, FLPR-driven from a black framebuffer. COM `Lo`.
//!   4. **Wait `T4 ≥ 30 µs`, then start COM** on a high-priority `InterruptExecutor` — runs forever.
//!   5. **BTN0 steps** the pattern: white → R → G → B → 64-colour palette → shapes → wrap. Each is
//!      rendered into the framebuffer then FLPR-driven once (MIP retains it); BTN0 is polled
//!      responsively for the next press. The on-glass result must be **identical to the M33-direct
//!      L3 (#148)** — both paths render the same shared [`palette`]/[`shapes`] source.
//!
//! Per frame: the M33 pre-packs rows 0/1 into `buf[0]`/`buf[1]`, writes `cmd = CMD_RUN_FRAME` + bumps
//! `m33_seq` (the command doorbell), then packs the remaining rows into whichever buffer the FLPR
//! just freed; the FLPR runs the gate scan, draining the ping-pong buffers, and acks via the `EGU20`
//! IRQ (#201). The M33 checks `status == 320 && flpr_seq == m33_seq` and logs the **pack-vs-drain
//! overlap** (the M33 pack time is a tiny fraction of the FLPR's frame time → the pipeline overlaps).
//! The waveform is verified on the logic analyzer against the M33 golden frame, the result on glass
//! via the webcam (`/tmp/obc-cam/panel.jpg`).
//!
//! **Both cores on P2 at once (since F3).** The FLPR drives the source bus on `P2.00..06`; the M33
//! drives COM on `P2.07/08/10` from `com_task`. Safe because every GPIO touch on either core is an
//! atomic `OUTSET`/`OUTCLR` of disjoint pin masks (never an `OUT` read-modify-write). The gate lines
//! (`GSP`/`GCK`/`GEN`/`INTB`) + `BSP` are all on **P1**, FLPR-driven.
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
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration, Instant, Timer};
// The host-tested RGB222 → LS021-wire pack (issue #154) + its sub-line/row word counts.
use obc_platform::ls021_pack_row;
use obc_platform::ls021_wire::{BCK_PER_SUBLINE, ROW_WORDS, WIDTH};
use {defmt_rtt as _, panic_probe as _};

// The free-running COM driver + the shared test patterns are panel-board-agnostic infrastructure
// (not the M33-direct PanelBus, which the FLPR replaces). Pull in `com_task` + `palette`/`shapes`;
// the rest of the module (PanelBus etc.) is unused here, hence the module-level allow.
#[path = "../ls021.rs"]
#[allow(dead_code)]
mod ls021;
use ls021::{com_task, palette, shapes};

/// The FLPR program image, cross-compiled by `build.rs` into `$OUT_DIR/flpr.bin`.
static FLPR_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flpr.bin"));

// ── FLPR memory map (must match src/flpr/flpr.ld + the carved memory.x in build.rs) ──
/// FLPR execution base: the M33 copies the blob here and points `INITPC` at it.
const FLPR_RAM_BASE: usize = 0x2003_8000;
/// The **two ping-pong row buffers** the FLPR drains, parked in the SHARED page (which both cores
/// already reach — the FLPR reads the control block there). Each row buffer is `ROW_WORDS` (248)
/// u32 = MSB sub-line [0..124) then LSB sub-line [124..248); `buf[0]` is at the first address,
/// `buf[1]` one row buffer above it. Both sit clear of the 64-byte control block at the page base,
/// and together (2 × 992 B) fit the 4 KB SHARED page with room to spare. `buf[0]` carries even
/// rows, `buf[1]` odd — the M33 packs one while the FLPR scans the other.
const WRITE_BUF_ADDR: [usize; 2] = [0x2003_F100, 0x2003_F100 + ROW_WORDS * 4];

/// Shared control block at the `SHARED` page base. Layout is normative and identical to the C
/// `flpr_control_t` in `src/flpr/flpr_pingpong.c` — keep them in sync (`firmware/docs/ls021-flpr.md`).
/// All fields `u32`, little-endian; `#[repr(C)]` + all-`u32` members ⇒ deterministic offsets, no
/// padding. Accessed only through raw volatile field reads/writes (never as a `&` reference) since
/// the FLPR mutates it concurrently.
#[repr(C)]
#[allow(dead_code)] // reserved is forward-compat headroom — defined now, unused here.
struct Control {
    magic: u32,         // 0x00 M33: layout/version tag, checked by the FLPR before acting
    m33_seq: u32,       // 0x04 M33: command sequence counter (the per-frame command doorbell)
    cmd: u32,           // 0x08 M33: command word (a CMD_* code)
    flpr_seq: u32,      // 0x0C FLPR: echoes the m33_seq it serviced (round-trip proof)
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

// ── Frame geometry. The wire-word counts (`WIDTH` 240, `BCK_PER_SUBLINE` 124, `ROW_WORDS` 248)
//    come from `obc_platform::ls021_wire` — the same constants the host-tested pack uses and the
//    FLPR reads as `buf[i].len` (a row buffer is `2 × BCK_PER_SUBLINE = ROW_WORDS` words). ──
/// Visible pixel rows the FLPR scans per frame — the `status` the M33 cross-checks, and the
/// framebuffer height.
const ROWS_PER_FRAME: u32 = 320;

// ── Resident RGB222 framebuffer (the F4 source). 240×320 device-64 bytes (`0b00_RR_GG_BB`, the
//    [`FbDevice64`](obc_platform::FbDevice64)/`PackDevice64` format) = 75 KB in the M33's `.bss` —
//    the production map plane's type/size, here filled with the bring-up test patterns instead of a
//    rendered map (F5 plugs the real `App` render in behind the `Panel` seam). `ls021_pack_row`
//    reads one row of it per ping-pong buffer fill. ──
/// Framebuffer width = the panel width.
const FB_W: usize = WIDTH;
/// Framebuffer height = the visible row count.
const FB_H: usize = ROWS_PER_FRAME as usize;
/// Resident RGB222 (device-64) framebuffer, one byte per pixel. 75 KB; fits the 224 KB M33 region.
static mut FB: [u8; FB_W * FB_H] = [0u8; FB_W * FB_H];

// ── VPR00 control (secure alias base 0x5004_C000): the M33 only launches the FLPR here. NB:
//    VPR00's own EVENTS→app-IRQ is unusable on bare metal — the app-side INTEN won't accept writes
//    from the M33 (reads back 0) — so the FLPR→M33 return doorbell uses EGU20 instead. ──
const VPR00_INITPC: *mut u32 = 0x5004_C808 as *mut u32; // initial PC at core start
const VPR00_CPURUN: *mut u32 = 0x5004_C800 as *mut u32; // CPURUN.EN bit0 = run

// ── EGU20 (secure 0x500C_9000, IRQ #201): the FLPR→M33 return doorbell. The FLPR pokes
//    TASKS_TRIGGER[0] (a plain peripheral write, like it drives GPIO); EGU20.EVENTS_TRIGGERED[0]
//    then raises the EGU20 IRQ on the M33. EGU is a normal peripheral whose INTEN *is* writable. ──
const EGU20_EVENTS_TRIGGERED0: *mut u32 = 0x500C_9100 as *mut u32;
const EGU20_INTEN: *mut u32 = 0x500C_9300 as *mut u32;
const EGU20_INTENSET: *mut u32 = 0x500C_9304 as *mut u32;
const ACK_EGU_CH: u32 = 0; // EGU channel 0

/// Set by the `EGU20` ISR when the FLPR rings the return doorbell; awaited by `main`.
static ACK: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// High-priority executor the COM driver runs on, pended from the unused SWI00 software-interrupt
/// vector (we only borrow its vector as the pend line). COM at P3 preempts thread-mode so it
/// free-runs CPU-independently while the M33 awaits the FLPR's per-frame ack.
static EXECUTOR_COM: InterruptExecutor = InterruptExecutor::new();

#[interrupt]
unsafe fn SWI00() {
    EXECUTOR_COM.on_interrupt();
}

/// FLPR → M33 return doorbell. The FLPR poked `EGU20.TASKS_TRIGGER[0]`, raising
/// `EGU20.EVENTS_TRIGGERED[0]` and (via `INTEN[0]`) this IRQ. Clear the event so it doesn't
/// re-fire, then wake `main`; `main` reads the ack fields from the control block (the FLPR wrote
/// them, fenced, before ringing, so they're visible here).
#[interrupt]
unsafe fn EGU20() {
    EGU20_EVENTS_TRIGGERED0.write_volatile(0);
    let _ = EGU20_EVENTS_TRIGGERED0.read_volatile(); // read-back: ensure the clear lands before return
    ACK.signal(());
}

/// A cycleable F4 test pattern rendered into the framebuffer: a uniform RGB222 `Solid`, or a
/// `Spatial` per-pixel pattern (the [`palette`], the [`shapes`] card). Both fill the *same*
/// resident `FbDevice64` source the ping-pong pack then reads — solids exercise the path, palette +
/// shapes exercise per-column (spatial) data.
#[derive(Clone, Copy)]
enum Pattern {
    Solid(u8, u8, u8),
    Spatial(fn(u16, u16) -> (u8, u8, u8)),
}

/// Render one pattern into the resident RGB222 framebuffer ([`FB`]) as device-64 bytes
/// (`0b00_RR_GG_BB`, the `FbDevice64`/`PackDevice64` format). A `Solid` is one `fill`; a `Spatial`
/// evaluates the shared pattern fn per pixel — the same source the M33-direct L3 path renders, so
/// the FLPR-driven card must look identical.
fn render_into_fb(pat: Pattern) {
    // SAFETY: single-threaded bench use — FB is touched only here (fill) and in `pack_row_into`
    // (read), never concurrently (the COM/ACK interrupts don't touch it).
    let fb = unsafe { &mut *addr_of_mut!(FB) };
    match pat {
        Pattern::Solid(r, g, b) => fb.fill((r << 4) | (g << 2) | b),
        Pattern::Spatial(f) => {
            for (y, row) in fb.chunks_mut(FB_W).enumerate() {
                for (x, px) in row.iter_mut().enumerate() {
                    let (r, g, b) = f(x as u16, y as u16);
                    *px = (r << 4) | (g << 2) | b;
                }
            }
        }
    }
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
fn publish_row(i: usize, row: usize) {
    // SAFETY: `fb_row` reads the framebuffer (filled by render_into_fb); `out` is the SHARED-page
    // write buffer at a fixed address no Rust object aliases. Disjoint regions, single-threaded.
    let fb_row = unsafe { core::slice::from_raw_parts((addr_of!(FB) as *const u8).add(row * FB_W), FB_W) };
    let out = unsafe { core::slice::from_raw_parts_mut(WRITE_BUF_ADDR[i] as *mut u32, ROW_WORDS) };
    ls021_pack_row(fb_row, out);
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

/// Poll interval / cap while waiting for the FLPR to free a ping-pong buffer. A bring-up-slow row
/// drains in ~5.6 ms, so a free buffer is usually seen within a poll or two; the cap (`* WAIT_POLLS`
/// ≈ 5 s) only fires if the FLPR has stalled, turning a hang into a reported error.
const WAIT_POLL_US: u64 = 100;
const WAIT_POLLS: u32 = 50_000;

/// Wait (async, non-blocking) for the FLPR to drain buf[i] — i.e. `consumed == ready`, the buffer is
/// free for the M33 to refill. Returns `false` on the timeout cap (a stalled FLPR). Async so the
/// executor keeps running (COM is on its own interrupt executor regardless).
async fn wait_buffer_free(i: usize) -> bool {
    for _ in 0..WAIT_POLLS {
        if buf_consumed(i) == buf_ready(i) {
            return true;
        }
        Timer::after_micros(WAIT_POLL_US).await;
    }
    false
}

/// Drive one frame from the framebuffer through the FLPR over the two **ping-pong** buffers, and
/// wait for the frame-done ack. Pre-packs rows 0/1 into `buf[0]`/`buf[1]`, rings the FLPR, then packs
/// each remaining row into whichever buffer the FLPR just freed (`buf[row & 1]`) — the M33 stays one
/// buffer ahead while the FLPR scans. Returns `true` if the ack checks out (`status ==
/// ROWS_PER_FRAME && flpr_seq == seq`); the waveform/glass proof is on the LA + webcam.
///
/// Logs the **pack-vs-drain overlap**: the M33's summed pack time is a tiny fraction of the FLPR's
/// frame time, so the M33 spends the frame mostly waiting on the FLPR — the pipeline genuinely
/// overlaps (the M33 is never the bottleneck), the property the F4 issue asks to demonstrate.
async fn push_frame(name: &str, seq: &mut u32) -> bool {
    *seq += 1;
    let s = *seq;

    // Reset + pre-fill both buffers while the FLPR is idle (it starts only on the m33_seq bump).
    reset_descriptors();
    publish_row(0, 0); // row 0 → buf[0] (even)
    publish_row(1, 1); // row 1 → buf[1] (odd)

    ACK.reset(); // clear any stale signal before ringing
    let t_frame = Instant::now();
    ring_cmd(s);

    // Pack the remaining 318 rows, each paced by the FLPR freeing its buffer (the ping-pong).
    let mut pack_total_us: u64 = 0;
    let mut pack_max_us: u64 = 0;
    for row in 2..ROWS_PER_FRAME as usize {
        let i = row & 1;
        if !wait_buffer_free(i).await {
            error!(
                "LS021 FLPR F4: {=str} STALLED at row {=usize} — FLPR didn't free buf[{=usize}] (consumed={=u32}, ready={=u32})",
                name, row, i, buf_consumed(i), buf_ready(i)
            );
            return false;
        }
        let t0 = Instant::now();
        publish_row(i, row);
        let dt = t0.elapsed().as_micros();
        pack_total_us += dt;
        pack_max_us = pack_max_us.max(dt);
    }

    // The M33 has packed every row; wait for the FLPR's whole-frame ack (a bring-up-slow frame is
    // ~1.8 s, so the timeout is generous). COM free-runs on its interrupt executor throughout.
    match with_timeout(Duration::from_secs(8), ACK.wait()).await {
        Ok(()) => {
            let status = unsafe { addr_of!((*CONTROL).status).read_volatile() };
            let flpr_seq = unsafe { addr_of!((*CONTROL).flpr_seq).read_volatile() };
            let frames = unsafe { addr_of!((*CONTROL).frame_count).read_volatile() };
            if status == ROWS_PER_FRAME && flpr_seq == s {
                let frame_us = t_frame.elapsed().as_micros();
                let packed = ROWS_PER_FRAME - 2; // rows packed inside the frame (2 were pre-filled)
                let avg_drain = frame_us / ROWS_PER_FRAME as u64; // FLPR per-row scan time
                let avg_pack = pack_total_us / packed as u64; // M33 per-row pack time
                info!(
                    "LS021 FLPR F4: {=str} frame OK — FLPR scanned {=u32} rows (frame #{=u32}) in {=u64} µs (~{=u64} µs/row); M33 packed {=u32} rows in {=u64} µs (avg {=u64} / max {=u64} µs/row) → pack ≪ drain, pipeline overlaps",
                    name, status, frames, frame_us, avg_drain, packed, pack_total_us, avg_pack, pack_max_us
                );
                true
            } else {
                error!(
                    "LS021 FLPR F4: {=str} frame MISMATCH — status={=u32} (want {=u32}), flpr_seq={=u32} (want {=u32})",
                    name, status, ROWS_PER_FRAME, flpr_seq, s
                );
                false
            }
        }
        Err(_) => {
            let flpr_seq = unsafe { addr_of!((*CONTROL).flpr_seq).read_volatile() };
            if flpr_seq == s {
                error!(
                    "LS021 FLPR F4: {=str} frame TIMEOUT — FLPR serviced it (flpr_seq matched) but no EGU20 IRQ (EVENTS[0]={=u32})",
                    name,
                    unsafe { EGU20_EVENTS_TRIGGERED0.read_volatile() }
                );
            } else {
                error!(
                    "LS021 FLPR F4: {=str} frame TIMEOUT — FLPR did NOT finish (flpr_seq={=u32}≠{=u32}) → the scan or a ping-pong wait stalled",
                    name, flpr_seq, s
                );
            }
            false
        }
    }
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
/// then poll every ~5 ms for a debounced press edge. (Same as the L3 `ls021_bringup` helper.)
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
    let btn0 = Input::new(p.P1_13, Pull::Up); // DK BTN0 — active-LOW (pressed = Lo); steps the colour

    info!("LS021 FLPR F4: launcher up — blob is {=usize} bytes", FLPR_BLOB.len());

    // 1. Settle window (~2 s, LED1 ~2 Hz): all inputs `Lo`, COM `Lo`. Meter window; hands-free so the
    //    bench LA capture at reset is deterministic.
    info!("LS021 FLPR F4: SETTLE (~2s, all inputs Lo, COM held Lo) — then FLPR boot, init-black, COM, colours");
    for _ in 0..8 {
        led1.toggle();
        Timer::after_millis(250).await;
    }

    // 2. Arm the control block, launch the FLPR, wait for its ALIVE stamp.
    unsafe {
        core::ptr::write_bytes(CONTROL as *mut u8, 0, core::mem::size_of::<Control>());
        addr_of_mut!((*CONTROL).magic).write_volatile(LAYOUT_MAGIC); // FLPR reads magic first thing
    }
    unsafe { EGU20_INTENSET.write_volatile(1 << ACK_EGU_CH) }; // arm the FLPR→M33 return doorbell
    interrupt::EGU20.set_priority(Priority::P3);
    unsafe { interrupt::EGU20.enable() };
    info!(
        "LS021 FLPR F4: EGU20 armed ch{=u32} — INTEN=0x{=u32:08x}, nvic_enabled={=bool}",
        ACK_EGU_CH,
        unsafe { EGU20_INTEN.read_volatile() },
        interrupt::EGU20.is_enabled()
    );
    start_flpr();
    info!("LS021 FLPR F4: FLPR released (INITPC=0x{=u32:08x}) — waiting for alive", FLPR_RAM_BASE as u32);

    // Boot confirmation has no doorbell (the FLPR isn't running yet when we'd arm one), so poll the
    // control block briefly for the FLPR's ALIVE stamp (~1 s budget).
    let mut alive = false;
    for _ in 0..200 {
        match unsafe { addr_of!((*CONTROL).status).read_volatile() } {
            FLPR_ALIVE => {
                alive = true;
                break;
            }
            FLPR_BADMAG => {
                error!(
                    "LS021 FLPR F4: FLPR booted but control-block magic mismatched — memory-map drift? (ls021-flpr.md)"
                );
                break;
            }
            _ => Timer::after_millis(5).await,
        }
    }
    if !alive {
        warn!("LS021 FLPR F4: no alive stamp — FLPR didn't boot or can't reach shared RAM; halting (LED1 blink)");
        loop {
            led1.toggle();
            Timer::after_millis(500).await;
        }
    }
    info!("LS021 FLPR F4: FLPR alive — driving the init-black frame (COM still Lo)");

    // 3. Init #0 — the FLPR drives an INTB-framed all-black frame from a black framebuffer, over the
    //    ping-pong path (every frame, init included, runs the same pack → ping-pong → FLPR pipeline).
    //    COM is not running yet.
    let mut seq: u32 = 0;
    led1.set_high(); // LED1 steady-on marks the init frame on the bench
    render_into_fb(Pattern::Solid(0, 0, 0));
    if !push_frame("INIT-BLACK", &mut seq).await {
        warn!("LS021 FLPR F4: init-black frame failed — see error above; continuing to COM + patterns anyway");
    }
    led1.set_low();

    // 4. T4 ≥ 30 µs, then start COM on the high-priority interrupt executor (P3). From here COM
    //    free-runs forever — VCOM≡VB in phase, VA inverse, ~60 Hz / 50 %.
    Timer::after_micros(50).await;
    interrupt::SWI00.set_priority(Priority::P3);
    let com_spawner = EXECUTOR_COM.start(interrupt::SWI00);
    com_spawner.spawn(defmt::unwrap!(com_task(vcom, vb, va)));
    info!(
        "LS021 FLPR F4: COM RUNNING — BTN0 steps WHITE → R → G → B → PALETTE → SHAPES (MIP retains each frame; COM toggles)"
    );

    // 5. BTN0 steps the pattern: white → red → green → blue → 64-colour palette → shapes → wrap.
    //    Each is rendered into the framebuffer then FLPR-driven once over the ping-pong path (MIP
    //    retains it — no refresh), then BTN0 is polled responsively for the next press. LED1 toggles
    //    on each accepted press. The palette + shapes are the *same* shared pattern fns the
    //    M33-direct L3 (#148) draws, so the on-glass cards must match it pixel-for-pixel.
    const PATTERNS: [(&str, Pattern); 6] = [
        ("WHITE", Pattern::Solid(3, 3, 3)),
        ("RED", Pattern::Solid(3, 0, 0)),
        ("GREEN", Pattern::Solid(0, 3, 0)),
        ("BLUE", Pattern::Solid(0, 0, 3)),
        ("PALETTE", Pattern::Spatial(palette)),
        ("SHAPES", Pattern::Spatial(shapes)),
    ];
    let mut i = 0usize;
    loop {
        let (name, pat) = PATTERNS[i];
        info!("LS021 FLPR F4: SHOW {=str} — press BTN0 for next", name);
        render_into_fb(pat);
        push_frame(name, &mut seq).await;
        wait_for_press(&btn0).await;
        led1.toggle();
        i = (i + 1) % PATTERNS.len();
    }
}
