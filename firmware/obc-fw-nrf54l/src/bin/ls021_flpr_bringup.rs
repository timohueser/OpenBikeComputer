//! **LS021 FLPR bring-up F1** (issue #151, epic #149) — M33↔FLPR comms over a shared control
//! block + a doorbell each way.
//!
//! Successor to **F0** (#150), which proved the build/boot path: the M33 cross-compiles a C blob,
//! copies it to FLPR RAM, releases the core, and the FLPR blinks LED0 + answers a single-word
//! handshake. F1 promotes that crude one-word handshake into the real **bidirectional control
//! channel** the ping-pong write-buffer handoff (F4) needs — a structured shared-RAM control
//! block + a doorbell each way, round-trip verified. Still no panel signal: the comms analog of
//! the M33 bring-up's L1. See `firmware/docs/ls021-flpr.md`.
//!
//! The round-trip, per command `N`:
//!   1. M33 writes `cmd = N` + bumps `m33_seq` (seq last), `dsb`. The bumped sequence IS the
//!      M33→FLPR doorbell.
//!   2. The FLPR polls `m33_seq` in shared RAM; on a change it reads `cmd`, writes
//!      `status = N ^ 0xA11E` and `flpr_seq = m33_seq` (seq last), pokes `EGU20.TASKS_TRIGGER[0]`,
//!      then blinks LED0 (P2.09) `N` times as a by-eye/LA marker.
//!   3. `EGU20.EVENTS_TRIGGERED[0]` fires the M33's **`EGU20` IRQ (#201)**. The ISR signals
//!      `main`, which reads back `status`/`flpr_seq` and checks the round-trip over RTT.
//!
//! **Why shared RAM + EGU, not VEVIF (the bring-up lesson).** The epic named the VPR's VEVIF
//! mailboxes, but on this bare-metal setup both VEVIF directions are walled: a VEVIF *task* the
//! M33 rings never latches into the FLPR's TASKS CSR (even after unlocking RT-peripheral CSR
//! access + enabling INTEN), and a VEVIF *event* the FLPR raises reaches the app's
//! `EVENTS_TRIGGERED` but can't be gated to the NVIC — the app-side VPR00 `INTEN` refuses writes
//! (reads back 0) without SoC-level init we don't replicate. So M33→FLPR rides the shared-RAM
//! sequence (the FLPR is a dedicated core — polling is correct, and is exactly F4's ping-pong
//! handshake), and FLPR→M33 is a real interrupt bounced off **EGU20** (a normal peripheral whose
//! `INTEN` *is* writable). No pin, no busy-wait on the M33. Full story in `ls021-flpr.md`.
//!
//! Build/flash (the bin only compiles with its feature; needs a RISC-V gcc — `brew install
//! riscv64-elf-gcc`):
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

/// Shared control block at the `SHARED` page base. Layout is normative and identical to the C
/// `flpr_control_t` in `src/flpr/flpr_comms.c` — keep them in sync (`firmware/docs/ls021-flpr.md`).
/// All fields `u32`, little-endian; `#[repr(C)]` + all-`u32` members ⇒ deterministic offsets, no
/// padding. Accessed only through raw volatile field reads/writes (never as a `&` reference) since
/// the FLPR mutates it concurrently.
#[repr(C)]
#[allow(dead_code)] // frame_count/buf/reserved are the F4 ping-pong contract — defined now, unused here.
struct Control {
    magic: u32,         // 0x00 M33: layout/version tag, checked by the FLPR before acting
    m33_seq: u32,       // 0x04 M33: command sequence counter (the doorbell payload id)
    cmd: u32,           // 0x08 M33: command word (F1: the value N)
    flpr_seq: u32,      // 0x0C FLPR: echoes the m33_seq it serviced (round-trip proof)
    status: u32,        // 0x10 FLPR: ack/result (F1: cmd ^ 0xA11E; boot: FLPR_ALIVE)
    frame_count: u32,   // 0x14 FLPR: frames drained (F4)
    buf: [BufDesc; 2],  // 0x18, 0x28 ping-pong write-buffer descriptors (F4)
    reserved: [u32; 2], // 0x38 forward-compat headroom
}
#[repr(C)]
#[allow(dead_code)]
struct BufDesc {
    ptr: u32,
    len: u32,
    ready: u32,
    consumed: u32,
}
/// Lock the cross-language contract: the C `flpr_control_t` `_Static_assert`s the same size.
const _: () = assert!(core::mem::size_of::<Control>() == 64);

const CONTROL: *mut Control = 0x2003_F000 as *mut Control;
const LAYOUT_MAGIC: u32 = 0xF1C0_0001; // "F1 control block" — the FLPR refuses to act otherwise
const FLPR_ALIVE: u32 = 0x0000_A11E; // FLPR boot confirmation (also the ack XOR key)
const FLPR_BADMAG: u32 = 0x0BAD_CAFE; // FLPR booted but saw the wrong magic (memory-map drift)

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
/// re-fire, then wake `main`; `main` reads `status`/`flpr_seq` from the control block (the FLPR
/// wrote them, fenced, before ringing, so they're visible here).
#[interrupt]
unsafe fn EGU20() {
    EGU20_EVENTS_TRIGGERED0.write_volatile(0);
    let _ = EGU20_EVENTS_TRIGGERED0.read_volatile(); // read-back: ensure the clear lands before return
    ACK.signal(());
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

    // The M33 owns the pin configuration for LED0 (P2.09); the FLPR only ever pulses OUTSET/OUTCLR
    // (atomic, never an OUT read-modify-write) so the two cores never collide on shared port P2.
    // Kept alive for the life of the program so the pin stays configured as the FLPR drives it.
    let _led0 = Output::new(p.P2_09, Level::Low, OutputDrive::Standard);
    // LED1 (P1.10) is the M33's own heartbeat — proves the M33 keeps running alongside the FLPR.
    let mut led1 = Output::new(p.P1_10, Level::Low, OutputDrive::Standard);

    info!("LS021 FLPR F1: launcher up — blob is {=usize} bytes", FLPR_BLOB.len());

    // Arm the control block: zero it, then write the layout magic the FLPR checks. The FLPR reads
    // `magic` first thing, so it must be set before release.
    unsafe {
        core::ptr::write_bytes(CONTROL as *mut u8, 0, core::mem::size_of::<Control>());
        addr_of_mut!((*CONTROL).magic).write_volatile(LAYOUT_MAGIC);
    }

    // Arm the FLPR→M33 return doorbell on EGU20 before releasing the FLPR: enable EGU channel 0's
    // interrupt and unmask the EGU20 IRQ in the NVIC. P3 keeps the ISR snappy (it only flips a
    // Signal). Read INTEN back to confirm it latched (it does — EGU INTEN is writable, the whole
    // reason we bounce off EGU instead of VPR00's unwritable INTEN).
    unsafe { EGU20_INTENSET.write_volatile(1 << ACK_EGU_CH) };
    interrupt::EGU20.set_priority(Priority::P3);
    unsafe { interrupt::EGU20.enable() };
    info!(
        "LS021 FLPR F1: EGU20 armed ch{=u32} — INTEN=0x{=u32:08x}, nvic_enabled={=bool}",
        ACK_EGU_CH,
        unsafe { EGU20_INTEN.read_volatile() },
        interrupt::EGU20.is_enabled()
    );

    start_flpr();
    info!("LS021 FLPR F1: FLPR released (INITPC=0x{=u32:08x}) — waiting for alive", FLPR_RAM_BASE as u32);

    // Boot confirmation has no doorbell (the FLPR isn't running yet when we'd arm one), so poll
    // the control block briefly for the FLPR's ALIVE stamp (~1 s budget).
    let mut alive = false;
    for _ in 0..200 {
        match unsafe { addr_of!((*CONTROL).status).read_volatile() } {
            FLPR_ALIVE => {
                alive = true;
                break;
            }
            FLPR_BADMAG => {
                error!(
                    "LS021 FLPR F1: FLPR booted but control-block magic mismatched — memory-map drift? (ls021-flpr.md)"
                );
                break;
            }
            _ => Timer::after_millis(5).await,
        }
    }
    if !alive {
        warn!("LS021 FLPR F1: no alive stamp — FLPR didn't boot or can't reach shared RAM; skipping round-trip");
        loop {
            led1.toggle();
            Timer::after_millis(500).await;
        }
    }
    info!("LS021 FLPR F1: FLPR alive — running doorbell round-trip (cmd N=1..=5)");

    // Round-trip sweep: prove the bidirectional channel is reliable, not a one-shot. For each N:
    // write the command (the m33_seq bump is the doorbell), await the EGU return interrupt, check
    // the echo + sequence.
    let mut seq: u32 = 0;
    let mut all_ok = true;
    for n in 1..=5u32 {
        seq += 1;
        // Clear any stale signal *before* ringing, so a late ACK from a timed-out earlier round
        // can't be mistaken for this command's response.
        ACK.reset();
        unsafe {
            addr_of_mut!((*CONTROL).cmd).write_volatile(n);
            addr_of_mut!((*CONTROL).m33_seq).write_volatile(seq); // seq last = the doorbell guard
            cortex_m::asm::dsb(); // command visible before the FLPR can observe the new sequence
        }
        match with_timeout(Duration::from_millis(100), ACK.wait()).await {
            Ok(()) => {
                let status = unsafe { addr_of!((*CONTROL).status).read_volatile() };
                let flpr_seq = unsafe { addr_of!((*CONTROL).flpr_seq).read_volatile() };
                let want = n ^ FLPR_ALIVE;
                if status == want && flpr_seq == seq {
                    info!(
                        "LS021 FLPR F1: round-trip {=u32}/5 OK — cmd={=u32} → status=0x{=u32:08x}, seq matched",
                        n, n, status
                    );
                } else {
                    all_ok = false;
                    error!(
                        "LS021 FLPR F1: round-trip {=u32}/5 MISMATCH — status=0x{=u32:08x} (want 0x{=u32:08x}), flpr_seq={=u32} (want {=u32})",
                        n, status, want, flpr_seq, seq
                    );
                }
            }
            Err(_) => {
                all_ok = false;
                // Localize the failure: did the FLPR service the command (ack fields advanced) but
                // the EGU return interrupt not arrive, or did it never service at all?
                let status = unsafe { addr_of!((*CONTROL).status).read_volatile() };
                let flpr_seq = unsafe { addr_of!((*CONTROL).flpr_seq).read_volatile() };
                if flpr_seq == seq {
                    error!(
                        "LS021 FLPR F1: round-trip {=u32}/5 TIMEOUT — FLPR serviced it (status=0x{=u32:08x}, seq matched) but no EGU20 IRQ (EVENTS[0]={=u32}, INTEN=0x{=u32:08x})",
                        n,
                        status,
                        unsafe { EGU20_EVENTS_TRIGGERED0.read_volatile() },
                        unsafe { EGU20_INTEN.read_volatile() }
                    );
                } else {
                    error!(
                        "LS021 FLPR F1: round-trip {=u32}/5 TIMEOUT — FLPR did NOT service (status=0x{=u32:08x}, flpr_seq={=u32}≠{=u32}) → M33→FLPR path is the problem",
                        n, status, flpr_seq, seq
                    );
                }
            }
        }
        // Spacing so the FLPR's LED0 pulse bursts are distinguishable by eye / on the LA.
        Timer::after_millis(400).await;
    }
    if all_ok {
        info!("LS021 FLPR F1: all round-trips OK — bidirectional M33↔FLPR channel verified");
    } else {
        warn!("LS021 FLPR F1: some round-trips failed — see errors above");
    }

    // Keep exercising the channel forever so the activity is observable on the bench: LED0 (P2.09,
    // FLPR-driven) blinks `cmd` times per command, LED1 (P1.10, M33) toggles each command. The
    // FLPR only drives LED0 *while servicing*, so without this loop the verification sweep above is
    // a single ~1 s burst that's easy to miss on a scope. Commands cycle 1..=5 so the burst length
    // varies visibly. RTT logs only on a fault here (no per-command spam).
    info!("LS021 FLPR F1: looping round-trips forever — LED0 = FLPR response, LED1 = M33 heartbeat");
    let mut n = 1u32;
    loop {
        seq += 1;
        ACK.reset();
        unsafe {
            addr_of_mut!((*CONTROL).cmd).write_volatile(n);
            addr_of_mut!((*CONTROL).m33_seq).write_volatile(seq);
            cortex_m::asm::dsb();
        }
        if with_timeout(Duration::from_millis(200), ACK.wait()).await.is_err() {
            warn!("LS021 FLPR F1: keepalive round-trip (cmd={=u32}) timed out", n);
        }
        led1.toggle();
        n = if n >= 5 { 1 } else { n + 1 };
        Timer::after_millis(500).await;
    }
}
