//! **LS021 FLPR bring-up F2** (issue #152, epic #149) — the FLPR clocks out one **source sub-line**
//! from a write buffer, diffed on the logic analyzer against the M33 golden capture.
//!
//! Successor to **F1** (#151, the M33↔FLPR comms channel). F2 keeps that channel — a shared control
//! block + a doorbell each way — and adds the **single most timing-critical piece of the epic**,
//! isolated: the inner data-shift loop. The M33 fills one SHARED-page write buffer with a known test
//! sub-line, hands it to the FLPR through the buffer descriptor, and rings the doorbell; the FLPR
//! drains it — pulse `BSP`, then 124 `BCK` (120 data + 4 dummy) presenting the 6 data lines from the
//! buffer — then acks. No gate scan, no frame envelope, no glass. See `firmware/docs/ls021-flpr.md`.
//!
//! Per round:
//!   1. M33 fills the write buffer (a distinctive per-column test pattern — see [`fill_test_subline`]),
//!      sets `buf[0].ptr/len/ready`, writes `cmd = CMD_SHIFT_SUBLINE`, bumps `m33_seq` (seq last),
//!      `dsb`. The bumped sequence IS the doorbell.
//!   2. The FLPR polls `m33_seq`; on a change it reads `cmd` + the descriptor, drives the sub-line on
//!      `BSP`/`BCK`/`R0..B1`, echoes `buf[0].ready` into `buf[0].consumed`, writes `status = columns
//!      driven` + `flpr_seq = m33_seq` (seq last), pokes `EGU20.TASKS_TRIGGER[0]`, toggles LED0.
//!   3. `EGU20.EVENTS_TRIGGERED[0]` fires the M33's **`EGU20` IRQ (#201)**. The ISR signals `main`,
//!      which checks `consumed == ready` + `status == len` + `flpr_seq == m33_seq`.
//!
//! **The F2 variable — two ports.** The timing-critical bus (6 data + `BCK`) is all on **P2**
//! (`P2.00..06`, the FLPR fast trace domain), so the hot 124× loop is single-port. `BSP` (one pulse
//! per sub-line) is on **P1.07** — so F2 is also the first low-stakes test that the FLPR can drive a
//! non-P2 (P1) GPIO, which F3's gate scan (`GSP`/`GCK`/`GEN`/`INTB`, all on P1) will depend on. The
//! M33 owns pin *configuration* (these `Output`s); the FLPR only ever pulses `OUTSET`/`OUTCLR`.
//!
//! Build/flash (needs a RISC-V gcc for the blob — `brew install riscv64-elf-gcc`; and the
//! Board-Configurator ext-memory-off / 3.3 V-VDDM settings the `ls021_bringup` epic already needs,
//! so `P2.00..05` are free GPIO):
//! ```sh
//! cargo run --release --bin ls021_flpr_bringup --features ls021-flpr
//! ```

#![no_std]
#![no_main]

use core::ptr::{addr_of, addr_of_mut};

use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::interrupt;
use embassy_nrf::interrupt::{InterruptExt, Priority};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration, Timer};
use {defmt_rtt as _, panic_probe as _};

/// The FLPR program image, cross-compiled by `build.rs` into `$OUT_DIR/flpr.bin`.
static FLPR_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flpr.bin"));

// ── FLPR memory map (must match src/flpr/flpr.ld + the carved memory.x in build.rs) ──
/// FLPR execution base: the M33 copies the blob here and points `INITPC` at it.
const FLPR_RAM_BASE: usize = 0x2003_8000;
/// The write buffer the FLPR drains, parked in the SHARED page (which both cores already reach —
/// the FLPR reads the control block there). One source sub-line of pre-packed GPIO words; sits well
/// clear of the 64-byte control block at the page base. F4 moves these to M33-side ping-pong buffers.
const WRITE_BUF_ADDR: usize = 0x2003_F100;

/// Shared control block at the `SHARED` page base. Layout is normative and identical to the C
/// `flpr_control_t` in `src/flpr/flpr_source.c` — keep them in sync (`firmware/docs/ls021-flpr.md`).
/// All fields `u32`, little-endian; `#[repr(C)]` + all-`u32` members ⇒ deterministic offsets, no
/// padding. Accessed only through raw volatile field reads/writes (never as a `&` reference) since
/// the FLPR mutates it concurrently.
#[repr(C)]
#[allow(dead_code)] // frame_count/reserved are the F4 contract — defined now, unused here.
struct Control {
    magic: u32,         // 0x00 M33: layout/version tag, checked by the FLPR before acting
    m33_seq: u32,       // 0x04 M33: command sequence counter (the doorbell payload id)
    cmd: u32,           // 0x08 M33: command word (F2: a CMD_* code)
    flpr_seq: u32,      // 0x0C FLPR: echoes the m33_seq it serviced (round-trip proof)
    status: u32,        // 0x10 FLPR: ack/result (F2: columns driven; boot: FLPR_ALIVE)
    frame_count: u32,   // 0x14 FLPR: frames drained (F4)
    buf: [BufDesc; 2],  // 0x18, 0x28 ping-pong write-buffer descriptors (F2 uses buf[0])
    reserved: [u32; 2], // 0x38 forward-compat headroom
}
#[repr(C)]
#[allow(dead_code)]
struct BufDesc {
    ptr: u32,      // write-buffer base
    len: u32,      // length in words = BCK per sub-line
    ready: u32,    // M33 set when filled — a token the FLPR echoes into `consumed`
    consumed: u32, // FLPR set when drained (= the serviced `ready` token)
}
/// Lock the cross-language contract: the C `flpr_control_t` `_Static_assert`s the same size.
const _: () = assert!(core::mem::size_of::<Control>() == 64);

const CONTROL: *mut Control = 0x2003_F000 as *mut Control;
const LAYOUT_MAGIC: u32 = 0xF1C0_0001; // "F1 control block" — the FLPR refuses to act otherwise
const FLPR_ALIVE: u32 = 0x0000_A11E; // FLPR boot confirmation
const FLPR_BADMAG: u32 = 0x0BAD_CAFE; // FLPR booted but saw the wrong magic (memory-map drift)
const CMD_SHIFT_SUBLINE: u32 = 0x0000_0001; // drain buf[0] as one source sub-line

// ── Source sub-line geometry (matches `PanelBus` in src/ls021.rs / the datasheet horizontal chart):
//    240 columns ÷ 2 pixels-per-BCK = 120 data clocks + 4 trailing dummy/flush = 124 BCK. ──
const COLS_PER_SUBLINE: usize = 120;
const BCK_PER_SUBLINE: usize = 124;
/// The 6 source data bits within a write-buffer word, pre-shifted to their **P2 pin positions**:
/// bit0 `R0`(P2.00) bit1 `R1`(P2.01) bit2 `G0`(P2.02) bit3 `G1`(P2.03) bit4 `B0`(P2.04) bit5
/// `B1`(P2.05). The FLPR stores the word straight to the port (`OUTCLR` the zeros, `OUTSET` the
/// ones); `BCK`(P2.06) is the FLPR's own pulse and is never in the word. (`R0/G0/B0` = odd pixel,
/// `R1/G1/B1` = even.)
const DATA_MASK: u32 = 0x3F;

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

/// Fill the SHARED-page write buffer with one **distinctive** test sub-line: column `c`'s word is
/// `c & 0x3F`, so each of the 6 data lines becomes a clean divide-by-2ⁿ square wave clocked by the
/// columns — `R0` toggles every column, `R1` every 2, `G0` every 4, … `B1` every 32. On the LA each
/// data line has its own distinct frequency, so a **bit swap** (e.g. `R0`↔`R1`), a **stuck line**,
/// or an **odd/even-interleave** error shows immediately. The 4 trailing dummy columns present
/// black. (This stands in for F4's real RGB222→wire pack — here we just need a known pattern.)
fn fill_test_subline() {
    let buf = WRITE_BUF_ADDR as *mut u32;
    for col in 0..BCK_PER_SUBLINE {
        let word = if col < COLS_PER_SUBLINE { (col as u32) & DATA_MASK } else { 0 };
        unsafe { buf.add(col).write_volatile(word) };
    }
}

/// Hand the buffer to the FLPR and ring the doorbell: publish the descriptor (`ptr/len/ready`), the
/// command, then bump `m33_seq` **last** (the guard), with a `dsb` so the FLPR never sees the new
/// sequence before the buffer + descriptor it guards.
fn ring_shift(seq: u32, ready: u32) {
    unsafe {
        addr_of_mut!((*CONTROL).buf[0].ptr).write_volatile(WRITE_BUF_ADDR as u32);
        addr_of_mut!((*CONTROL).buf[0].len).write_volatile(BCK_PER_SUBLINE as u32);
        addr_of_mut!((*CONTROL).buf[0].ready).write_volatile(ready);
        addr_of_mut!((*CONTROL).cmd).write_volatile(CMD_SHIFT_SUBLINE);
        addr_of_mut!((*CONTROL).m33_seq).write_volatile(seq); // seq last = the doorbell guard
        cortex_m::asm::dsb();
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
    // shared ports. Kept alive for the life of the program so the pins stay configured.
    //   • LED0 (P2.09): the FLPR's by-eye "serviced a sub-line" marker.
    //   • The source bus: BSP (P1.07) + the 6 data lines (P2.00..05) + BCK (P2.06). Boot Lo (the
    //     panel-safe state). All Standard drive, matching the M33-direct `ls021_bringup` source bus.
    let _led0 = Output::new(p.P2_09, Level::Low, OutputDrive::Standard);
    let _src_bus = [
        Output::new(p.P1_07, Level::Low, OutputDrive::Standard), // BSP  (P1.07 — the only P1 line)
        Output::new(p.P2_06, Level::Low, OutputDrive::Standard), // BCK  (P2.06)
        Output::new(p.P2_00, Level::Low, OutputDrive::Standard), // R0   (P2.00, odd)
        Output::new(p.P2_01, Level::Low, OutputDrive::Standard), // R1   (P2.01, even)
        Output::new(p.P2_02, Level::Low, OutputDrive::Standard), // G0   (P2.02, odd)
        Output::new(p.P2_03, Level::Low, OutputDrive::Standard), // G1   (P2.03, even)
        Output::new(p.P2_04, Level::Low, OutputDrive::Standard), // B0   (P2.04, odd)
        Output::new(p.P2_05, Level::Low, OutputDrive::Standard), // B1   (P2.05, even)
    ];
    // LED1 (P1.10) is the M33's own heartbeat — proves the M33 keeps running alongside the FLPR.
    let mut led1 = Output::new(p.P1_10, Level::Low, OutputDrive::Standard);

    info!("LS021 FLPR F2: launcher up — blob is {=usize} bytes", FLPR_BLOB.len());

    // Arm the control block: zero it, then write the layout magic the FLPR checks. The FLPR reads
    // `magic` first thing, so it must be set before release.
    unsafe {
        core::ptr::write_bytes(CONTROL as *mut u8, 0, core::mem::size_of::<Control>());
        addr_of_mut!((*CONTROL).magic).write_volatile(LAYOUT_MAGIC);
    }

    // Arm the FLPR→M33 return doorbell on EGU20 before releasing the FLPR (see F1 / ls021-flpr.md).
    unsafe { EGU20_INTENSET.write_volatile(1 << ACK_EGU_CH) };
    interrupt::EGU20.set_priority(Priority::P3);
    unsafe { interrupt::EGU20.enable() };
    info!(
        "LS021 FLPR F2: EGU20 armed ch{=u32} — INTEN=0x{=u32:08x}, nvic_enabled={=bool}",
        ACK_EGU_CH,
        unsafe { EGU20_INTEN.read_volatile() },
        interrupt::EGU20.is_enabled()
    );

    start_flpr();
    info!("LS021 FLPR F2: FLPR released (INITPC=0x{=u32:08x}) — waiting for alive", FLPR_RAM_BASE as u32);

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
                    "LS021 FLPR F2: FLPR booted but control-block magic mismatched — memory-map drift? (ls021-flpr.md)"
                );
                break;
            }
            _ => Timer::after_millis(5).await,
        }
    }
    if !alive {
        warn!("LS021 FLPR F2: no alive stamp — FLPR didn't boot or can't reach shared RAM; skipping sub-line");
        loop {
            led1.toggle();
            Timer::after_millis(500).await;
        }
    }

    // Fill the write buffer once — the test pattern is constant, so each round re-uses it and the LA
    // sees a stable, repeating sub-line. (F4 re-packs the buffer per frame from the framebuffer.)
    fill_test_subline();
    info!(
        "LS021 FLPR F2: FLPR alive — buffer filled ({=usize} BCK: {=usize} data + dummy). Driving sub-lines.",
        BCK_PER_SUBLINE, COLS_PER_SUBLINE
    );

    // Verification sweep: drive the sub-line a few times and check the ack each round (consumed echo,
    // column count, sequence). The waveform itself is verified on the logic analyzer — capture BSP +
    // BCK + the 6 data lines and diff against the M33 golden sub-line (firmware/docs/ls021-flpr.md).
    let mut seq: u32 = 0;
    let mut all_ok = true;
    for n in 1..=5u32 {
        seq += 1;
        let ready = 0xBEEF_0000 | n; // distinct token (not the seq) so the consumed echo is meaningful
        ACK.reset(); // clear any stale signal before ringing
        ring_shift(seq, ready);
        match with_timeout(Duration::from_millis(100), ACK.wait()).await {
            Ok(()) => {
                let consumed = unsafe { addr_of!((*CONTROL).buf[0].consumed).read_volatile() };
                let status = unsafe { addr_of!((*CONTROL).status).read_volatile() };
                let flpr_seq = unsafe { addr_of!((*CONTROL).flpr_seq).read_volatile() };
                if consumed == ready && status == BCK_PER_SUBLINE as u32 && flpr_seq == seq {
                    info!(
                        "LS021 FLPR F2: sub-line {=u32}/5 OK — drove {=u32} BCK, consumed=0x{=u32:08x}, seq matched",
                        n, status, consumed
                    );
                } else {
                    all_ok = false;
                    error!(
                        "LS021 FLPR F2: sub-line {=u32}/5 MISMATCH — status={=u32} (want {=usize}), consumed=0x{=u32:08x} (want 0x{=u32:08x}), flpr_seq={=u32} (want {=u32})",
                        n, status, BCK_PER_SUBLINE, consumed, ready, flpr_seq, seq
                    );
                }
            }
            Err(_) => {
                all_ok = false;
                let consumed = unsafe { addr_of!((*CONTROL).buf[0].consumed).read_volatile() };
                let flpr_seq = unsafe { addr_of!((*CONTROL).flpr_seq).read_volatile() };
                if flpr_seq == seq {
                    error!(
                        "LS021 FLPR F2: sub-line {=u32}/5 TIMEOUT — FLPR serviced it (consumed=0x{=u32:08x}, seq matched) but no EGU20 IRQ (EVENTS[0]={=u32})",
                        n,
                        consumed,
                        unsafe { EGU20_EVENTS_TRIGGERED0.read_volatile() }
                    );
                } else {
                    error!(
                        "LS021 FLPR F2: sub-line {=u32}/5 TIMEOUT — FLPR did NOT service (flpr_seq={=u32}≠{=u32}) → M33→FLPR path is the problem",
                        n, flpr_seq, seq
                    );
                }
            }
        }
        // Spacing so each sub-line is a distinct, easily-triggered capture on the analyzer.
        Timer::after_millis(400).await;
    }
    if all_ok {
        info!("LS021 FLPR F2: all sub-lines acked OK — capture BSP/BCK/R0..B1 on the LA and diff vs M33");
    } else {
        warn!("LS021 FLPR F2: some sub-lines failed — see errors above");
    }

    // Keep driving the sub-line forever so the analyzer sees a steady stream to trigger on: each
    // round the FLPR re-drives the (constant) buffer, LED0 blinks per sub-line, LED1 = M33 heartbeat.
    info!("LS021 FLPR F2: looping sub-lines forever — LED0 = FLPR serviced, LED1 = M33 heartbeat");
    let mut n = 6u32;
    loop {
        seq += 1;
        let ready = 0xBEEF_0000 | (n & 0xFFFF);
        ACK.reset();
        ring_shift(seq, ready);
        if with_timeout(Duration::from_millis(200), ACK.wait()).await.is_err() {
            warn!("LS021 FLPR F2: keepalive sub-line (seq={=u32}) timed out", seq);
        }
        led1.toggle();
        n = n.wrapping_add(1);
        Timer::after_millis(500).await;
    }
}
