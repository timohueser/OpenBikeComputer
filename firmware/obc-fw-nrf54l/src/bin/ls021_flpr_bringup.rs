//! **LS021 FLPR bring-up F3** (issue #153, epic #149) — the FLPR drives a **whole frame** and
//! puts it on glass: a uniform **black** init frame, then **solid white / R / G / B**. **The "FLPR
//! drives the panel" milestone.**
//!
//! Successor to **F2** (#152, the FLPR clocking out one source sub-line). F2 isolated the inner
//! data-shift loop; F3 wraps it in the two pieces that make a frame — the **gate scan**
//! (`GSP`/`GCK`/`GEN`) and the **frame envelope** (`INTB`) — so the FLPR now owns the *complete*
//! LS021 waveform, ported from the analyzer-verified M33 [`PanelBus`](../ls021.rs) (epic #139).
//! The M33's only panel job is to **pack one row buffer** (the solid colour's MSB then LSB sub-line
//! words) and ring the FLPR once per frame; **COM** (`VCOM`/`VB`/`VA`) free-runs on the M33 the
//! whole time, exactly as in the M33-direct bring-up. See `firmware/docs/ls021-flpr.md`.
//!
//! Power-on sequence (datasheet §6-2, mirroring the L3 `ls021_bringup` bin):
//!   1. **Settle (~2 s)** — rails up, all inputs `Lo`, COM `Lo`. Hands-free so a power-cycle gives a
//!      deterministic LA capture; LED1 blinks.
//!   2. **Launch the FLPR**, arm the EGU20 return doorbell, wait for its `ALIVE` stamp (F1/F2 boot).
//!   3. **Init #0 — `INTB`-framed all-black frame**, driven by the FLPR (`CMD_RUN_FRAME` with a black
//!      row buffer). COM is still held `Lo`.
//!   4. **Wait `T4 ≥ 30 µs`, then start COM** on a high-priority `InterruptExecutor` — runs forever.
//!   5. **BTN0 steps** the colour: white → R → G → B → wrap. Each frame is FLPR-driven once (MIP
//!      retains it), then BTN0 is polled responsively for the next press. (Palette + shapes = F4.)
//!
//! Per frame the M33 packs `WRITE_BUF` (`buf[0]` row buffer: MSB sub-line then LSB sub-line), writes
//! `cmd = CMD_RUN_FRAME`, bumps `m33_seq` (the doorbell); the FLPR runs the whole gate scan and acks
//! via the `EGU20` IRQ (#201); the M33 checks `consumed == ready && status == 320 && flpr_seq ==
//! m33_seq`. The waveform itself is verified on the logic analyzer against the M33 golden frame, and
//! the result on glass via the webcam (`/tmp/obc-cam/panel.jpg`).
//!
//! **Both cores on P2 at once (new in F3).** The FLPR drives the source bus on `P2.00..06`; the M33
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
use embassy_time::{with_timeout, Duration, Timer};
use {defmt_rtt as _, panic_probe as _};

// The free-running COM driver is the proven L1 task — shared, panel-board-agnostic infrastructure
// (not the M33-direct PanelBus, which the FLPR replaces). Pull in just `com_task`; the rest of the
// module (PanelBus etc.) is unused here, hence the module-level allow.
#[path = "../ls021.rs"]
#[allow(dead_code)]
mod ls021;
use ls021::com_task;

/// The FLPR program image, cross-compiled by `build.rs` into `$OUT_DIR/flpr.bin`.
static FLPR_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flpr.bin"));

// ── FLPR memory map (must match src/flpr/flpr.ld + the carved memory.x in build.rs) ──
/// FLPR execution base: the M33 copies the blob here and points `INITPC` at it.
const FLPR_RAM_BASE: usize = 0x2003_8000;
/// The **row buffer** the FLPR drains, parked in the SHARED page (which both cores already reach —
/// the FLPR reads the control block there). One row = MSB sub-line (124 words) then LSB sub-line
/// (124 words); sits well clear of the 64-byte control block at the page base. F4 moves these to
/// M33-side ping-pong buffers (one per row).
const WRITE_BUF_ADDR: usize = 0x2003_F100;

/// Shared control block at the `SHARED` page base. Layout is normative and identical to the C
/// `flpr_control_t` in `src/flpr/flpr_frame.c` — keep them in sync (`firmware/docs/ls021-flpr.md`).
/// All fields `u32`, little-endian; `#[repr(C)]` + all-`u32` members ⇒ deterministic offsets, no
/// padding. Accessed only through raw volatile field reads/writes (never as a `&` reference) since
/// the FLPR mutates it concurrently.
#[repr(C)]
#[allow(dead_code)] // reserved is forward-compat headroom — defined now, unused here.
struct Control {
    magic: u32,         // 0x00 M33: layout/version tag, checked by the FLPR before acting
    m33_seq: u32,       // 0x04 M33: command sequence counter (the doorbell payload id)
    cmd: u32,           // 0x08 M33: command word (F3: a CMD_* code)
    flpr_seq: u32,      // 0x0C FLPR: echoes the m33_seq it serviced (round-trip proof)
    status: u32,        // 0x10 FLPR: ack/result (F3: rows scanned; boot: FLPR_ALIVE)
    frame_count: u32,   // 0x14 FLPR: frames drained (bumped per CMD_RUN_FRAME)
    buf: [BufDesc; 2],  // 0x18, 0x28 row-buffer descriptors (F3 uses buf[0]; F4 ping-pongs)
    reserved: [u32; 2], // 0x38 forward-compat headroom
}
#[repr(C)]
#[allow(dead_code)]
struct BufDesc {
    ptr: u32,      // row-buffer base (MSB sub-line at [0..len), LSB sub-line at [len..2·len))
    len: u32,      // words per sub-line = BCK per sub-line; a row is 2·len words
    ready: u32,    // M33 set when filled — a token the FLPR echoes into `consumed`
    consumed: u32, // FLPR set when drained (= the serviced `ready` token)
}
/// Lock the cross-language contract: the C `flpr_control_t` `_Static_assert`s the same size.
const _: () = assert!(core::mem::size_of::<Control>() == 64);

const CONTROL: *mut Control = 0x2003_F000 as *mut Control;
const LAYOUT_MAGIC: u32 = 0xF1C0_0001; // "F1 control block" — the FLPR refuses to act otherwise
const FLPR_ALIVE: u32 = 0x0000_A11E; // FLPR boot confirmation
const FLPR_BADMAG: u32 = 0x0BAD_CAFE; // FLPR booted but saw the wrong magic (memory-map drift)
const CMD_RUN_FRAME: u32 = 0x0000_0002; // drive one full frame from buf[0]

// ── Frame geometry (matches `PanelBus` in src/ls021.rs and the C blob). ──
/// Data columns per sub-line: 240 columns ÷ 2 pixels-per-`BCK` = **120**.
const COLS_PER_SUBLINE: usize = 120;
/// `BCK` per sub-line: 120 data + 4 trailing dummy/flush = **124**. Also the per-sub-line word count
/// the FLPR reads from `buf[0].len` (a row buffer is `2 × BCK_PER_SUBLINE` words).
const BCK_PER_SUBLINE: usize = 124;
/// Visible pixel rows the FLPR scans per frame — the `status` the M33 cross-checks.
const ROWS_PER_FRAME: u32 = 320;

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

/// Pack one solid-colour source word for the given RGB222 level triple and area plane: the **MSB**
/// plane carries each channel's 2/3-area bit (`l >> 1`), the **LSB** plane the 1/3-area bit
/// (`l & 1`). For a solid fill the **odd** (`R0/G0/B0`, bits 0/2/4) and **even** (`R1/G1/B1`, bits
/// 1/3/5) pixels carry the same bit, so a swap or a stuck odd/even line shows on the LA. This is the
/// uniform stand-in for F4's real host-tested RGB222-framebuffer→wire pack.
fn pack_solid(level: (u8, u8, u8), msb: bool) -> u32 {
    let shift = if msb { 1 } else { 0 }; // MSB plane = the 2/3-area bit (l>>1), LSB plane = 1/3-area (l>>0)
    let bit = |l: u8| ((l >> shift) & 1) as u32;
    let (r, g, b) = (bit(level.0), bit(level.1), bit(level.2));
    r | (r << 1) | (g << 2) | (g << 3) | (b << 4) | (b << 5)
}

/// Fill the SHARED-page **row buffer** with one solid colour: the MSB sub-line (`BCK_PER_SUBLINE`
/// words: 120 data columns of the MSB word + 4 trailing black dummies) followed by the LSB sub-line,
/// laid out exactly as the FLPR reads them (`buf[0]` = MSB at `[0..124)`, LSB at `[124..248)`). For a
/// solid colour every row is identical, so this single buffer feeds all 320 rows.
fn fill_solid_buffer(r: u8, g: u8, b: u8) {
    let buf = WRITE_BUF_ADDR as *mut u32;
    let msb = pack_solid((r, g, b), true);
    let lsb = pack_solid((r, g, b), false);
    for col in 0..BCK_PER_SUBLINE {
        let (m, l) = if col < COLS_PER_SUBLINE { (msb, lsb) } else { (0, 0) }; // 4 dummy cols = black
        unsafe {
            buf.add(col).write_volatile(m); // MSB sub-line [0..124)
            buf.add(BCK_PER_SUBLINE + col).write_volatile(l); // LSB sub-line [124..248)
        }
    }
}

/// Publish the row buffer + ring the doorbell for one frame: publish the descriptor (`ptr/len/
/// ready`), the command, then bump `m33_seq` **last** (the guard), with a `dsb` so the FLPR never
/// sees the new sequence before the buffer + descriptor it guards.
fn ring_frame(seq: u32, ready: u32) {
    unsafe {
        addr_of_mut!((*CONTROL).buf[0].ptr).write_volatile(WRITE_BUF_ADDR as u32);
        addr_of_mut!((*CONTROL).buf[0].len).write_volatile(BCK_PER_SUBLINE as u32); // words per sub-line
        addr_of_mut!((*CONTROL).buf[0].ready).write_volatile(ready);
        addr_of_mut!((*CONTROL).cmd).write_volatile(CMD_RUN_FRAME);
        addr_of_mut!((*CONTROL).m33_seq).write_volatile(seq); // seq last = the doorbell guard
        cortex_m::asm::dsb();
    }
}

/// Pack `(r, g, b)` into the row buffer, ring the FLPR, and wait for its per-frame ack. The FLPR
/// runs the whole bring-up-slow gate scan (~1.8 s/frame), so the timeout is generous; COM keeps
/// free-running on its interrupt executor throughout. Returns `true` if the ack checks out
/// (`consumed == ready && status == ROWS_PER_FRAME && flpr_seq == seq`). The waveform/glass proof is
/// on the LA + webcam — this just confirms the round-trip and the row count.
async fn run_frame(name: &str, r: u8, g: u8, b: u8, seq: &mut u32) -> bool {
    *seq += 1;
    let s = *seq;
    let ready = 0xF300_0000 | s; // distinct token (not the seq) so the consumed echo is meaningful
    fill_solid_buffer(r, g, b);
    ACK.reset(); // clear any stale signal before ringing
    ring_frame(s, ready);
    match with_timeout(Duration::from_secs(5), ACK.wait()).await {
        Ok(()) => {
            let consumed = unsafe { addr_of!((*CONTROL).buf[0].consumed).read_volatile() };
            let status = unsafe { addr_of!((*CONTROL).status).read_volatile() };
            let flpr_seq = unsafe { addr_of!((*CONTROL).flpr_seq).read_volatile() };
            let frames = unsafe { addr_of!((*CONTROL).frame_count).read_volatile() };
            if consumed == ready && status == ROWS_PER_FRAME && flpr_seq == s {
                info!(
                    "LS021 FLPR F3: {=str} frame OK — FLPR scanned {=u32} rows (frame #{=u32}), consumed=0x{=u32:08x}",
                    name, status, frames, consumed
                );
                true
            } else {
                error!(
                    "LS021 FLPR F3: {=str} frame MISMATCH — status={=u32} (want {=u32}), consumed=0x{=u32:08x} (want 0x{=u32:08x}), flpr_seq={=u32} (want {=u32})",
                    name, status, ROWS_PER_FRAME, consumed, ready, flpr_seq, s
                );
                false
            }
        }
        Err(_) => {
            let flpr_seq = unsafe { addr_of!((*CONTROL).flpr_seq).read_volatile() };
            if flpr_seq == s {
                error!(
                    "LS021 FLPR F3: {=str} frame TIMEOUT — FLPR serviced it (flpr_seq matched) but no EGU20 IRQ (EVENTS[0]={=u32})",
                    name,
                    unsafe { EGU20_EVENTS_TRIGGERED0.read_volatile() }
                );
            } else {
                error!(
                    "LS021 FLPR F3: {=str} frame TIMEOUT — FLPR did NOT finish (flpr_seq={=u32}≠{=u32}) → M33→FLPR path or the scan stalled",
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

    info!("LS021 FLPR F3: launcher up — blob is {=usize} bytes", FLPR_BLOB.len());

    // 1. Settle window (~2 s, LED1 ~2 Hz): all inputs `Lo`, COM `Lo`. Meter window; hands-free so the
    //    bench LA capture at reset is deterministic.
    info!("LS021 FLPR F3: SETTLE (~2s, all inputs Lo, COM held Lo) — then FLPR boot, init-black, COM, colours");
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
        "LS021 FLPR F3: EGU20 armed ch{=u32} — INTEN=0x{=u32:08x}, nvic_enabled={=bool}",
        ACK_EGU_CH,
        unsafe { EGU20_INTEN.read_volatile() },
        interrupt::EGU20.is_enabled()
    );
    start_flpr();
    info!("LS021 FLPR F3: FLPR released (INITPC=0x{=u32:08x}) — waiting for alive", FLPR_RAM_BASE as u32);

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
                    "LS021 FLPR F3: FLPR booted but control-block magic mismatched — memory-map drift? (ls021-flpr.md)"
                );
                break;
            }
            _ => Timer::after_millis(5).await,
        }
    }
    if !alive {
        warn!("LS021 FLPR F3: no alive stamp — FLPR didn't boot or can't reach shared RAM; halting (LED1 blink)");
        loop {
            led1.toggle();
            Timer::after_millis(500).await;
        }
    }
    info!("LS021 FLPR F3: FLPR alive — driving the init-black frame (COM still Lo)");

    // 3. Init #0 — the FLPR drives an INTB-framed all-black frame. COM is not running yet.
    let mut seq: u32 = 0;
    led1.set_high(); // LED1 steady-on marks the init frame on the bench
    if !run_frame("INIT-BLACK", 0, 0, 0, &mut seq).await {
        warn!("LS021 FLPR F3: init-black frame failed — see error above; continuing to COM + colours anyway");
    }
    led1.set_low();

    // 4. T4 ≥ 30 µs, then start COM on the high-priority interrupt executor (P3). From here COM
    //    free-runs forever — VCOM≡VB in phase, VA inverse, ~60 Hz / 50 %.
    Timer::after_micros(50).await;
    interrupt::SWI00.set_priority(Priority::P3);
    let com_spawner = EXECUTOR_COM.start(interrupt::SWI00);
    com_spawner.spawn(defmt::unwrap!(com_task(vcom, vb, va)));
    info!("LS021 FLPR F3: COM RUNNING — BTN0 steps WHITE → R → G → B (MIP retains each frame; COM toggles)");

    // 5. BTN0 steps the colour: white → red → green → blue → wrap. Each frame is FLPR-driven once
    //    (MIP retains it — no refresh), then BTN0 is polled responsively for the next press. LED1
    //    toggles on each accepted press. (Palette + shapes are F4's per-column data.)
    const COLOURS: [(&str, u8, u8, u8); 4] =
        [("WHITE", 3, 3, 3), ("RED", 3, 0, 0), ("GREEN", 0, 3, 0), ("BLUE", 0, 0, 3)];
    let mut i = 0usize;
    loop {
        let (name, r, g, b) = COLOURS[i];
        info!("LS021 FLPR F3: SHOW {=str} — press BTN0 for next", name);
        run_frame(name, r, g, b, &mut seq).await;
        wait_for_press(&btn0).await;
        led1.toggle();
        i = (i + 1) % COLOURS.len();
    }
}
