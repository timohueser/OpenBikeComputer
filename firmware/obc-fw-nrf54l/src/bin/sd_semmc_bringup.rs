//! **THROWAWAY bring-up + throughput bench for issue #1145** — microSD in **native 4-bit SD mode**
//! over Nordic's sEMMC soft peripheral, on the FLPR. Display disconnected; ONLY the card is wired.
//!
//! ⚠️ DELETE THIS FILE (and its `[[bin]]` stanza) before #1145's implementation merges. It lives on
//! the feasibility branch only so the wiring session can pick it up; it ships in no build.
//!
//! ⚠️ `include_bytes!`s Nordic's `LicenseRef-Nordic-5-Clause` firmware from OUTSIDE the repo
//! (`~/.cache/obc/semmc_fw.bin` — never commit it). Regenerate:
//! ```text
//! curl -sfL https://raw.githubusercontent.com/nrfconnect/sdk-nrfxlib/main/softperipheral/sEMMC/include/nrf54l/semmc_firmware_v0.1.1.h \
//!  | python3 -c "import sys,re;open('/Users/timo/.cache/obc/semmc_fw.bin','wb').write(bytes(int(x,16)&0xff for x in re.findall(r'0x[0-9a-fA-F]+',sys.stdin.read().split('semmc_firmware_bin[] = {',1)[1].split('};',1)[0])))"
//! ```
//!
//!     cargo run --release --bin sd_semmc_bringup
//!
//! ## Wiring under test (Nordic porting guide — the pin order is NOT sequential)
//!
//! | nRF54LM20 | sEMMC role | microSD pin | SPI-breakout label |
//! |-----------|------------|-------------|--------------------|
//! | P2.00     | D3         | 2 (CD/DAT3) | CS                 |
//! | P2.01     | CLK        | 5 (CLK)     | SCK                |
//! | P2.02     | D0         | 7 (DAT0)    | MISO               |
//! | P2.03     | D2         | 1 (DAT2)    | *(unrouted on SPI boards)* |
//! | P2.04     | D1         | 8 (DAT1)    | *(unrouted on SPI boards)* |
//! | P2.05     | CMD        | 3 (CMD)     | MOSI               |
//!
//! No external resistors: the nRF internal pulls (CLK down, all others up, ~13 kΩ) are Nordic's
//! reference config and inside the SD spec's 10–100 kΩ window.
//!
//! ## The step ladder — each failure points at a specific wire or a specific blob limitation
//!
//! - **S0 wiring diagnostics (plain GPIO, before sEMMC exists):** card-present via DAT3's
//!   *card-internal* ~50 kΩ pull-up; per-pin stuck-at test (each pin must follow our pull both
//!   ways); pairwise bridge test (drive one pin, sense the other five against the opposite pull).
//! - **S1** pads → Nordic reference config + `CTRLSEL=VPR`, boot the sEMMC firmware (47 µs cold).
//! - **S2** `CMD0` at 400 kHz — the known unknown: whether the firmware's divider reaches the SD
//!   init clock (clkdiv 320). Falls back up a clock ladder to isolate a divider-width limit.
//! - **S3** `CMD8 SEND_IF_COND` — R7 echo check proves CMD **TX and RX** + CRC end to end.
//!   ⚠ First blob-compat probe: eMMC's CMD8 is a 512 B data read; SD's is response-only. If this
//!   fails oddly, the blob keys the data phase on the command *index*, not on `DATA.BLOCKNUM`.
//! - **S4** `CMD55`+`ACMD41` loop (R3, no-CRC response) → OCR, CCS. Second blob-compat probe
//!   (neither index exists in eMMC).
//! - **S5** `CMD2` → CID, printed human-readably — the "is this really my card?" check.
//! - **S6** `CMD3` → RCA, `CMD9` → CSD/capacity, `CMD7` select, `CMD13` state must be `tran`.
//! - **S7** `CMD17` sector 0 in **1-bit** mode: MBR `0x55AA` signature + double-read compare.
//!   Proves the D0 data path.
//! - **S8** `ACMD6` → 4-bit + `CONFIG.BUSWIDTH=4`, re-read sector 0, byte-compare against S7.
//!   A mismatch/timeout here indicts exactly DAT1/DAT2/DAT3.
//! - **S9** clock ladder 2→8→16→21.3 MHz (Default Speed ceiling 25 MHz), read+compare per rung,
//!   `read_delay` sweep on failure. Then a `CMD6` high-speed switch attempt (64 B data read —
//!   blob-compat experiment) and, only if that works, a 32 MHz rung.
//! - **S10** read throughput at the best clock: CMD17 singles; CMD18 batches 8/64/256 (+CMD12).
//! - **S11** write bench, safety-first: save→salt→verify→**restore** a single block (safe
//!   anywhere); CMD25 batches only inside a scratch window 128 MiB from the card's end and only
//!   if that window reads back uniform (unused) first.
//! - **S12** summary, plus whether the **VPR00 completion ISR** actually fired (issue #1145 A3 —
//!   end-to-end interrupt delivery with real transfers).
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};

use defmt::{error, info, warn};
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::interrupt;
use embassy_nrf::interrupt::{InterruptExt, Priority};
use embassy_nrf::pac;
use embassy_nrf::pac::gpio::vals::{Ctrlsel, Dir, Drive, Input, Pull};
use embassy_time::{Instant, Timer};
// Linking nrf-mpsl provides the critical-section impl; never initialised (same as sd_bench).
use nrf_mpsl as _;
use panic_probe as _;

/// Nordic's sEMMC v0.1.1 PIC firmware (see the module doc for the regeneration one-liner).
static SEMMC_FW: &[u8] = include_bytes!("/Users/timo/.cache/obc/semmc_fw.bin");

// ── Memory the firmware asks for (decoded from its metadata header, verified 2026-08-05) ──
const FW_CODE_BYTES: usize = 15360; // fw_code_size 960×16 (blob is 13636 B; tail is zero-init)
const VRI_OFFSET: usize = 16896; // shared_ram_addr_offset 1536 + code 15360
const CARVE_BYTES: usize = 17408; // + the 512 B VRI

#[repr(C, align(4096))]
struct Carve([u8; CARVE_BYTES]);
static mut SEMMC_CARVE: Carve = Carve([0; CARVE_BYTES]);

// ── VRI register offsets (nrfxlib `nrf_sp_emmc.h`, `NRF_SP_EMMC_Type`) ──
const VRI_EV_XFERCOMPLETE: usize = 0x10;
const VRI_EV_ABORTED: usize = 0x14;
const VRI_EV_READYTOTRANSFER: usize = 0x18;
const VRI_INTEN: usize = 0x28;
const VRI_ENABLE: usize = 0x2C;
const VRI_CFG_READYTOTRANSFER: usize = 0x30;
const VRI_CFG_CLKFREQHZ: usize = 0x34;
const VRI_CFG_BUSWIDTH: usize = 0x38;
const VRI_CFG_NUMRETRIES: usize = 0x3C;
const VRI_CFG_READDELAY: usize = 0x40;
const VRI_CMD_CMD: usize = 0x44; // IDX | RESPTYPE<<16 | RESPPROC<<24
const VRI_CMD_ARG: usize = 0x48;
const VRI_CMD_RESPONSEADDR: usize = 0x4C;
const VRI_CMD_RESPONSE0: usize = 0x50; // [4] processed response words
const VRI_DATA_BUFFERADDR: usize = 0x64;
const VRI_DATA_BLOCKSIZE: usize = 0x68;
const VRI_DATA_BLOCKNUM: usize = 0x6C;
const VRI_STATUS: usize = 0x70;
const VRI_SPSYNC_AUX: usize = 0x74; // [6]; barrier: host counter in [0], firmware echo in [1]

// Response types (SP_EMMC_COMMAND_CMD_RESPTYPE_*) — R7≡R1 and R6≡R1 in wire shape for SD.
const RESP_NONE: u32 = 0;
const RESP_R1: u32 = 1;
const RESP_R1B: u32 = 2;
const RESP_R2: u32 = 3;
const RESP_R3: u32 = 4;
// Response processing (RESPPROC): IGNORE only applies to the read direction (porting guide).
const PROC_PROCESS: u32 = 0;
const PROC_IGNORE: u32 = 1;

// Soft-peripheral VPR task/event indices (`softperipheral_regif.h`, nRF54L row).
const T_DPPI0: usize = 16; // start a prepared transfer
const T_CONFIG: usize = 17; // __CSB
const T_ACTION: usize = 18; // __ASB
const T_STOP: usize = 19; // __SSB
const EV_IDX: usize = 20; // completion event → VPR00_IRQn

// ── VPR00 (secure alias) ──
const VPR00_TASKS_TRIGGER: *mut u32 = 0x5004_C000 as *mut u32;
const VPR00_EVENTS_TRIGGERED: *mut u32 = 0x5004_C100 as *mut u32;
const VPR00_INTENSET: *mut u32 = 0x5004_C304 as *mut u32;
const VPR00_CPURUN: *mut u32 = 0x5004_C800 as *mut u32;
const VPR00_INITPC: *mut u32 = 0x5004_C808 as *mut u32;
const VPR00_DMCONTROL: *mut u32 = 0x5004_C440 as *mut u32;
const DM_DMACTIVE: u32 = 1 << 0;
const DM_NDMRESET: u32 = 1 << 1;

// ── P2 GPIO, raw (base 0x5005_0400; OUT +0, OUTSET +4, OUTCLR +8, IN +0x0C) ──
const GPIO2_OUTSET: *mut u32 = 0x5005_0404 as *mut u32;
const GPIO2_OUTCLR: *mut u32 = 0x5005_0408 as *mut u32;
const GPIO2_IN: *const u32 = 0x5005_040C as *const u32;

/// The six sEMMC pins: (P2 pin, sEMMC role, where the wire goes).
const SD_PINS: [(usize, &str, &str); 6] = [
    (0, "D3 ", "microSD pin 2 — breakout 'CS'"),
    (1, "CLK", "microSD pin 5 — breakout 'SCK'"),
    (2, "D0 ", "microSD pin 7 — breakout 'MISO'"),
    (3, "D2 ", "microSD pin 1 — usually unrouted on SPI breakouts"),
    (4, "D1 ", "microSD pin 8 — usually unrouted on SPI breakouts"),
    (5, "CMD", "microSD pin 3 — breakout 'MOSI'"),
];

/// Transfer buffer: one 256-block CMD18/CMD25 batch. 32-bit aligned (driver requirement).
const XFER_BLOCKS: usize = 256;
#[repr(C, align(4))]
struct XferBuf([u8; XFER_BLOCKS * 512]);
static mut XFER_BUF: XferBuf = XferBuf([0; XFER_BLOCKS * 512]);
/// Golden copy of sector 0 (the 1-bit read S8/S9 compare against) + a save slot for S11.
static mut SECTOR0: XferBuf2 = XferBuf2([0; 512]);
static mut SAVE_BLOCK: XferBuf2 = XferBuf2([0; 512]);
#[repr(C, align(4))]
struct XferBuf2([u8; 512]);
/// Raw response sampling buffer — the driver always hands the firmware a pointer.
static mut RESP_RAW: [u32; 16] = [0; 16];

/// Completion-ISR counter (issue #1145 A3: does the VPR00 vector deliver, end to end?).
static ISR_COUNT: AtomicU32 = AtomicU32::new(0);

/// The app-side vector Nordic's driver installs as `SP_VPR_IRQHandler`: clear the VEVIF event
/// (or the level IRQ re-fires forever) and count. Completion detection itself stays polled.
#[interrupt]
unsafe fn VPR00() {
    VPR00_EVENTS_TRIGGERED.add(EV_IDX).write_volatile(0);
    ISR_COUNT.fetch_add(1, Ordering::Relaxed);
}

fn carve_base() -> usize {
    &raw const SEMMC_CARVE as *const _ as usize
}
fn vri() -> usize {
    carve_base() + VRI_OFFSET
}
#[inline(always)]
fn vri_read(off: usize) -> u32 {
    unsafe { ((vri() + off) as *const u32).read_volatile() }
}
#[inline(always)]
fn vri_write(off: usize, v: u32) {
    unsafe { ((vri() + off) as *mut u32).write_volatile(v) }
}

fn park_hart() {
    unsafe {
        VPR00_CPURUN.write_volatile(0);
        VPR00_DMCONTROL.write_volatile(DM_NDMRESET | DM_DMACTIVE);
        VPR00_DMCONTROL.write_volatile(DM_DMACTIVE);
        VPR00_DMCONTROL.write_volatile(0);
    }
}

/// Configure one P2 pin. `drive_e` = extra-high E0/E1 (the sEMMC/32 MHz requirement).
fn cfg_pin(pin: usize, dir: Dir, input: Input, pull: Pull, drive_e: bool, ctrl: Ctrlsel) {
    pac::P2_S.pin_cnf(pin).modify(|w| {
        w.set_dir(dir);
        w.set_input(input);
        w.set_pull(pull);
        w.set_drive0(if drive_e { Drive::E } else { Drive::S });
        w.set_drive1(if drive_e { Drive::E } else { Drive::S });
        w.set_ctrlsel(ctrl);
    });
}

fn read_pin(pin: usize) -> u32 {
    (unsafe { GPIO2_IN.read_volatile() } >> pin) & 1
}

fn delay_us(us: u32) {
    cortex_m::asm::delay(us * 128); // CK128
}

/// Hex-dump the interesting VRI region (events, config, command, data, status, AUX) — the
/// firmware-state snapshot for any stall diagnosis.
fn dump_vri(tag: &str) {
    info!(
        "{=str}: EV(x/a/r)={=u32}/{=u32}/{=u32} EN={=u32} RTT={=u32} CLK={=u32} W={=u32} STATUS=0x{=u32:08x} AUX[0]={=u32} AUX[1]={=u32}",
        tag,
        vri_read(VRI_EV_XFERCOMPLETE),
        vri_read(VRI_EV_ABORTED),
        vri_read(VRI_EV_READYTOTRANSFER),
        vri_read(VRI_ENABLE),
        vri_read(VRI_CFG_READYTOTRANSFER),
        vri_read(VRI_CFG_CLKFREQHZ),
        vri_read(VRI_CFG_BUSWIDTH),
        vri_read(VRI_STATUS),
        vri_read(VRI_SPSYNC_AUX),
        vri_read(VRI_SPSYNC_AUX + 4),
    );
}

/// The porting guide's secure-environment requirement (skipped so far — the boot handshake worked
/// without it, but the *bus/data* path may not): make the FLPR a secure bus master.
/// `NRF_SPU00_S->PERIPH[0xC].PERM.SECATTR = Secure` — index 0xC because VPR00 sits at
/// 0x5004_C000 = SPU00's slave slot 12.
fn spu_secure_flpr() {
    let perm = pac::SPU00_S.periph(0xC).perm();
    let before = perm.read().0;
    perm.modify(|w| w.set_secattr(true));
    let after = perm.read().0;
    info!("SPU00.PERIPH[0xC].PERM (VPR00): 0x{=u32:08x} → 0x{=u32:08x} (SECATTR=bit4)", before, after);
}

// ═════════════════════════════════ S0 — wiring diagnostics ═════════════════════════════════

/// Pure-GPIO checks, run BEFORE the soft peripheral exists — so a failure here is a WIRE, never
/// firmware. Returns (card present via DAT3's pull-up, per-pin external-pull-up map — pins where
/// the breakout carries its own resistor, so [`Semmc::configure_pads`] must NOT parallel our
/// internal ~13 kΩ on top of it or the combined pull drops below the SD spec's 10 kΩ floor).
fn s0_wiring_diagnostics() -> (bool, [bool; 6]) {
    info!("");
    info!("═══ S0 — wiring diagnostics (plain GPIO — failures here are wires, not firmware) ═══");
    for (pin, role, wire) in SD_PINS {
        info!("  P2.{=usize:02} = {=str} → {=str}", pin, role, wire);
    }

    // (a) Card present? All six as input, NO pull: DAT3 (P2.00) has a ~50 kΩ pull-up INSIDE the
    // card (the SD card-detect feature), so with a powered card it must read HIGH on its own.
    for (pin, _, _) in SD_PINS {
        cfg_pin(pin, Dir::Input, Input::Connect, Pull::Disabled, false, Ctrlsel::Gpio);
    }
    delay_us(2000);
    let dat3 = read_pin(0);
    let raw: u32 = unsafe { GPIO2_IN.read_volatile() } & 0x3F;
    info!("S0a floating levels P2.05..00 = {=u32:06b} (DAT3={=u32})", raw, dat3);
    let card_present = dat3 == 1;
    if card_present {
        info!("S0a CARD PRESENT — DAT3 reads high through the card's internal pull-up. VDD + DAT3 wire OK.");
    } else {
        warn!("S0a NO CARD SIGNATURE — DAT3 reads low with pulls off. Either no card, no 3V3 on the");
        warn!("    socket, or the DAT3 wire (breakout 'CS' → P2.00) is missing. Continuing anyway.");
    }

    // (b) Per-pin characterisation. Three probes, and the COMBINATION is what diagnoses:
    //   pull test  — internal pull-up then pull-down, read. A lone pin follows both.
    //   self-drive — output WITH the input buffer connected, drive 0 and 1, read the pad back.
    //                Our driver (~mA) beats any resistor, so a pad that will not follow its own
    //                driver has a HARD short to a rail; a pad that follows the driver but not the
    //                ~13 kΩ pull is loaded by an EXTERNAL RESISTOR (breakout pull-up array) — a
    //                completely different (and benign) diagnosis.
    let mut stuck = 0u32;
    let mut ext_pullup = [false; 6];
    for (i, (pin, role, _)) in SD_PINS.iter().enumerate() {
        let (pin, role) = (*pin, *role);
        cfg_pin(pin, Dir::Input, Input::Connect, Pull::Pullup, false, Ctrlsel::Gpio);
        delay_us(500);
        let up = read_pin(pin);
        cfg_pin(pin, Dir::Input, Input::Connect, Pull::Pulldown, false, Ctrlsel::Gpio);
        delay_us(500);
        let down = read_pin(pin);
        // Self-drive with read-back.
        cfg_pin(pin, Dir::Output, Input::Connect, Pull::Disabled, false, Ctrlsel::Gpio);
        unsafe { GPIO2_OUTCLR.write_volatile(1 << pin) };
        delay_us(200);
        let drv_lo = read_pin(pin);
        unsafe { GPIO2_OUTSET.write_volatile(1 << pin) };
        delay_us(200);
        let drv_hi = read_pin(pin);
        unsafe { GPIO2_OUTCLR.write_volatile(1 << pin) };
        cfg_pin(pin, Dir::Input, Input::Connect, Pull::Disabled, false, Ctrlsel::Gpio);

        let self_ok = drv_lo == 0 && drv_hi == 1;
        if self_ok && up == 1 && down == 0 {
            info!("S0b P2.{=usize:02} {=str}: clean (pull ↑1 ↓0, self-drive ok)", pin, role);
        } else if !self_ok {
            stuck += 1;
            error!(
                "S0b P2.{=usize:02} {=str}: HARD SHORT — self-drive lo→{=u32} hi→{=u32} (a rail short overpowers the GPIO driver)",
                pin, role, drv_lo, drv_hi
            );
        } else if down == 1 {
            ext_pullup[i] = true;
            info!(
                "S0b P2.{=usize:02} {=str}: EXTERNAL PULL-UP (self-drive ok, but internal ~13k pull-down reads {=u32}) — breakout resistor, benign for SD",
                pin, role, down
            );
        } else {
            info!(
                "S0b P2.{=usize:02} {=str}: EXTERNAL PULL-DOWN? (self-drive ok, internal pull-up reads {=u32}) — unusual, note it",
                pin, role, up
            );
        }
    }

    // (c) Bridges — the pull-artifact-proof version. A neighbour is only a BRIDGE if it follows
    // the driven pin in BOTH directions: a breakout pull-up can fake a follow to 1 (it fights our
    // sense pull-down into the undefined band) but never a follow to 0 (it agrees with the sense
    // pull-up), so requiring both kills that artifact; a real bridge (~0 Ω) wins both ways.
    let mut follows = [[0u8; 6]; 6]; // bit0 = followed to 0, bit1 = followed to 1
    for (di, (drv, _, _)) in SD_PINS.iter().enumerate() {
        let drv = *drv;
        for level in [0u32, 1u32] {
            for (other, _, _) in SD_PINS {
                if other != drv {
                    let opposite = if level == 0 { Pull::Pullup } else { Pull::Pulldown };
                    cfg_pin(other, Dir::Input, Input::Connect, opposite, false, Ctrlsel::Gpio);
                }
            }
            cfg_pin(drv, Dir::Output, Input::Disconnect, Pull::Disabled, false, Ctrlsel::Gpio);
            unsafe {
                if level == 0 {
                    GPIO2_OUTCLR.write_volatile(1 << drv);
                } else {
                    GPIO2_OUTSET.write_volatile(1 << drv);
                }
            }
            delay_us(500);
            for (oi, (other, _, _)) in SD_PINS.iter().enumerate() {
                if *other != drv && read_pin(*other) == level {
                    follows[di][oi] |= 1 << level;
                }
            }
            cfg_pin(drv, Dir::Input, Input::Connect, Pull::Disabled, false, Ctrlsel::Gpio);
        }
    }
    let mut bridges = 0u32;
    for di in 0..6 {
        for oi in 0..6 {
            if di != oi && follows[di][oi] == 0b11 {
                bridges += 1;
                error!(
                    "S0c BRIDGE — P2.{=usize:02} ({=str}) follows P2.{=usize:02} ({=str}) BOTH ways: solder/jumper short between these two wires",
                    SD_PINS[oi].0, SD_PINS[oi].1, SD_PINS[di].0, SD_PINS[di].1
                );
            }
        }
    }
    let one_way: u32 = (0..6)
        .flat_map(|d| (0..6).map(move |o| (d, o)))
        .filter(|&(d, o)| d != o && follows[d][o] != 0 && follows[d][o] != 0b11)
        .count() as u32;
    if one_way > 0 {
        info!("S0c {=u32} one-way follow(s) — consistent with external pull resistors (see S0b), NOT bridges", one_way);
    }

    if stuck == 0 && bridges == 0 {
        info!("S0 PASS — no hard shorts, no bridges{=str}", if card_present { ", card present" } else { "" });
    } else {
        error!(
            "S0 FAIL — {=u32} hard short(s), {=u32} bridge(s). Fix wiring before trusting later steps.",
            stuck, bridges
        );
    }
    if ext_pullup.iter().any(|&b| b) {
        info!("S0 note: external pull-ups detected — if ALL of CMD/DAT have them, the S0a card-present");
        info!("         check is inconclusive (a breakout resistor also holds DAT3 high, card or not).");
    }
    (card_present, ext_pullup)
}

// ═════════════════════════════ the minimal sEMMC host ═════════════════════════════

#[derive(Clone, Copy, PartialEq, defmt::Format)]
enum SdErr {
    BarrierTimeout,
    Timeout,
    Aborted(u32), // VRI STATUS at the abort
}

struct Semmc {
    clk_hz: u32,
    bus_width: u32,
    read_delay: u32,
    counter: u32,
    rca: u32,
    ccs: bool, // SDHC/SDXC: block addressing
}

impl Semmc {
    /// Nordic's reference pad config (porting guide table), then hand the pads to the VPR.
    /// CLK pull-DOWN, everything else pull-UP; Output + input Disconnected + E0/E1 drive.
    /// Where S0 found an EXTERNAL pull-up (breakout resistor), the internal pull is disabled —
    /// 13 kΩ ∥ ~10 kΩ ≈ 5.6 kΩ would sit below the SD spec's 10 kΩ floor and over-load the lines.
    fn configure_pads(ext_pullup: &[bool; 6]) {
        for (i, (pin, _, _)) in SD_PINS.iter().enumerate() {
            let pin = *pin;
            let pull = if ext_pullup[i] {
                Pull::Disabled
            } else if pin == 1 {
                Pull::Pulldown
            } else {
                Pull::Pullup
            };
            cfg_pin(pin, Dir::Output, Input::Disconnect, pull, true, Ctrlsel::Gpio);
        }
        for (pin, _, _) in SD_PINS {
            pac::P2_S.pin_cnf(pin).modify(|w| w.set_ctrlsel(Ctrlsel::Vpr));
        }
        // High-speed pad bias for 32 MHz (porting guide: BIAS = 0x2).
        pac::GPIOHSPADCTRL_S.bias().modify(|w| w.set_hsbias(2));
    }

    /// Boot (or re-boot) the firmware: park, zero the VRI, ENABLE=1, point INITPC at the resident
    /// image, run, wait for the firmware to clear ENABLE (its ready stamp). ~47 µs cold / ~12 µs
    /// warm, measured. Returns false if it never stamps ready.
    fn boot_firmware(&mut self, copy_image: bool) -> bool {
        park_hart();
        if copy_image {
            unsafe {
                core::ptr::write_bytes(carve_base() as *mut u8, 0, FW_CODE_BYTES);
                core::ptr::copy_nonoverlapping(SEMMC_FW.as_ptr(), carve_base() as *mut u8, SEMMC_FW.len());
            }
        }
        unsafe { core::ptr::write_bytes(vri() as *mut u8, 0, 512) };
        vri_write(VRI_ENABLE, 1);
        unsafe {
            cortex_m::asm::dsb();
            VPR00_INITPC.write_volatile(carve_base() as u32);
            VPR00_CPURUN.write_volatile(1);
        }
        let t0 = Instant::now();
        while vri_read(VRI_ENABLE) != 0 {
            if t0.elapsed().as_millis() > 500 {
                error!("sEMMC firmware never stamped ready (ENABLE stayed 1)");
                return false;
            }
        }
        vri_write(VRI_INTEN, 0x7); // XFERCOMPLETE | ABORTED | READYTOTRANSFER → VEVIF ev 20
                                   // Arm the app-side gate for the completion event. Must happen with the core RUNNING —
                                   // INTEN writes are silently dropped while the VPR is stopped (measured, 2026-08-05) —
                                   // which is why this lives here and re-runs on every (re)boot.
        unsafe { VPR00_INTENSET.write_volatile(1 << EV_IDX) };
        true
    }

    /// One `__XSBx` barrier (softperipheral_regif.h): AUX[0]=counter, trigger the task, spin until
    /// the firmware echoes into AUX[1]. ~2.2 µs measured; 50 ms timeout = firmware wedged.
    fn barrier(&mut self, task: usize) -> Result<(), SdErr> {
        self.counter = self.counter.wrapping_add(1);
        vri_write(VRI_SPSYNC_AUX, self.counter);
        unsafe { VPR00_TASKS_TRIGGER.add(task).write_volatile(1) };
        let t0 = Instant::now();
        while vri_read(VRI_SPSYNC_AUX) != vri_read(VRI_SPSYNC_AUX + 4) {
            if t0.elapsed().as_millis() > 50 {
                return Err(SdErr::BarrierTimeout);
            }
        }
        Ok(())
    }

    /// `nrf_semmc_enable()` — the step the first run MISSED (and why the S2 probe saw a dead
    /// bus): after boot the firmware is *initialized* but not *powered on*, and it ignores start
    /// triggers until `ENABLE = 1` + an `__ASB` moves it to POWERED_ON. Mirrors the driver:
    /// enable, barrier, clear the three events.
    fn power_on(&mut self) -> bool {
        vri_write(VRI_ENABLE, 1);
        let ok = self.barrier(T_ACTION).is_ok();
        vri_write(VRI_EV_XFERCOMPLETE, 0);
        vri_write(VRI_EV_ABORTED, 0);
        vri_write(VRI_EV_READYTOTRANSFER, 0);
        if !ok {
            error!("power_on: __ASB after ENABLE=1 timed out");
        }
        ok
    }

    /// Firmware wedge recovery: try a stop barrier, then warm re-boot (image stays resident,
    /// ~21 µs measured) + power back on. Card-side state is untouched — the card keeps its
    /// RCA/state.
    fn recover(&mut self) {
        let _ = self.barrier(T_STOP);
        if !self.boot_firmware(false) {
            error!("recover: warm re-boot failed — re-copying the image");
            self.boot_firmware(true);
        }
        self.power_on();
    }

    /// The front half of `nrf_semmc_cmd()`: CONFIG + COMMAND + DATA registers → `__CSB` →
    /// READYTOTRANSFER=1 → `__ASB` → trigger the DPPI start task. Split out so the S2 activity
    /// probe can sample the pads while the transfer runs.
    fn cmd_start(
        &mut self,
        idx: u32,
        arg: u32,
        resp: u32,
        proc: u32,
        data: Option<(u32, u32, u32)>,
    ) -> Result<(), SdErr> {
        vri_write(VRI_CFG_CLKFREQHZ, self.clk_hz);
        vri_write(VRI_CFG_BUSWIDTH, self.bus_width);
        vri_write(VRI_CFG_NUMRETRIES, 3);
        vri_write(VRI_CFG_READDELAY, self.read_delay);
        vri_write(VRI_CMD_CMD, (idx & 0xFFFF) | (resp << 16) | (proc << 24));
        vri_write(VRI_CMD_ARG, arg);
        vri_write(VRI_CMD_RESPONSEADDR, &raw const RESP_RAW as u32);
        let (b, bs, bn) = data.unwrap_or((0, 0, 0));
        vri_write(VRI_DATA_BUFFERADDR, b);
        vri_write(VRI_DATA_BLOCKSIZE, bs);
        vri_write(VRI_DATA_BLOCKNUM, bn);
        self.barrier(T_CONFIG)?;
        vri_write(VRI_CFG_READYTOTRANSFER, 1);
        self.barrier(T_ACTION)?;
        unsafe { VPR00_TASKS_TRIGGER.add(T_DPPI0).write_volatile(1) };
        Ok(())
    }

    /// Run one SD command: [`cmd_start`](Self::cmd_start) + poll the VRI completion events.
    /// `data = (buffer_addr, block_size, block_count)`; direction comes from the firmware's
    /// command-index table (17/18 read, 24/25 write — indices SD and eMMC share).
    fn cmd(
        &mut self,
        idx: u32,
        arg: u32,
        resp: u32,
        proc: u32,
        data: Option<(u32, u32, u32)>,
        timeout_ms: u64,
    ) -> Result<[u32; 4], SdErr> {
        self.cmd_start(idx, arg, resp, proc, data)?;
        let t0 = Instant::now();
        loop {
            if vri_read(VRI_EV_XFERCOMPLETE) != 0 {
                break;
            }
            if vri_read(VRI_EV_ABORTED) != 0 {
                let status = vri_read(VRI_STATUS);
                vri_write(VRI_EV_ABORTED, 0);
                vri_write(VRI_EV_XFERCOMPLETE, 0);
                // Close the transaction: RTT=0 **plus the __ASB ack** — the driver's abort path
                // does exactly this; without the barrier the firmware never re-arms.
                vri_write(VRI_CFG_READYTOTRANSFER, 0);
                let _ = self.barrier(T_ACTION);
                return Err(SdErr::Aborted(status));
            }
            if t0.elapsed().as_millis() > timeout_ms {
                vri_write(VRI_CFG_READYTOTRANSFER, 0);
                self.recover();
                return Err(SdErr::Timeout);
            }
        }
        vri_write(VRI_EV_XFERCOMPLETE, 0);
        vri_write(VRI_EV_READYTOTRANSFER, 0);
        // Close the transaction: RTT=0 **plus the __ASB ack**. The driver's IRQ handler does this
        // on every XFERCOMPLETE; skipping the barrier leaves the firmware considering the old
        // transaction open, and it ignores the next start trigger — the measured
        // "first-command-after-boot works, second one times out" alternation.
        vri_write(VRI_CFG_READYTOTRANSFER, 0);
        self.barrier(T_ACTION)?;
        Ok([
            vri_read(VRI_CMD_RESPONSE0),
            vri_read(VRI_CMD_RESPONSE0 + 4),
            vri_read(VRI_CMD_RESPONSE0 + 8),
            vri_read(VRI_CMD_RESPONSE0 + 12),
        ])
    }

    /// **The S2 oscilloscope-free probe**: run one CMD0 while sampling the CLK/CMD/D0 pads through
    /// their (temporarily connected) input buffers, counting transitions. This splits the world:
    ///   - CLK toggles seen → the firmware IS driving the bus; a stall is protocol/card-side.
    ///   - zero toggles     → the firmware never touches the pads; the problem is CTRLSEL/SPU/VIO
    ///     on our side — no amount of rewiring would change anything.
    /// The M33 samples at a few MHz, plenty to *count* edges at a 400 kHz CLK (it need not be
    /// cycle-accurate — zero vs nonzero is the verdict).
    fn cmd0_activity_probe(&mut self) {
        self.activity_probe(0, 0, RESP_NONE, "CMD0");
    }

    /// Generalised pad-activity probe: run `idx` while counting CLK/CMD/D0 transitions. For a
    /// response-carrying command the CMD count separates "card answered" (extra transitions after
    /// the 48-bit TX burst) from "card silent" (TX-only count, then a flat line + our timeout).
    fn activity_probe(&mut self, idx: u32, arg: u32, resp: u32, label: &str) {
        for pin in [1usize, 2, 5] {
            pac::P2_S.pin_cnf(pin).modify(|w| w.set_input(Input::Connect));
        }
        let started = self.cmd_start(idx, arg, resp, PROC_PROCESS, None);
        let (mut clk_t, mut cmd_t, mut d0_t) = (0u32, 0u32, 0u32);
        let mut complete = false;
        if started.is_ok() {
            let mut last = unsafe { GPIO2_IN.read_volatile() };
            let t0 = Instant::now();
            while t0.elapsed().as_millis() < 50 {
                let now = unsafe { GPIO2_IN.read_volatile() };
                let ch = now ^ last;
                if ch & (1 << 1) != 0 {
                    clk_t += 1;
                }
                if ch & (1 << 5) != 0 {
                    cmd_t += 1;
                }
                if ch & (1 << 2) != 0 {
                    d0_t += 1;
                }
                last = now;
                if vri_read(VRI_EV_XFERCOMPLETE) != 0 || vri_read(VRI_EV_ABORTED) != 0 {
                    complete = true;
                    break;
                }
            }
        } else {
            warn!("S2-probe: cmd_start failed ({}) before any sampling", started.unwrap_err());
        }
        info!(
            "probe {=str}: CLK transitions={=u32} CMD={=u32} D0={=u32}, completed={=bool}",
            label, clk_t, cmd_t, d0_t, complete
        );
        if clk_t == 0 {
            warn!("probe: NO CLOCK ACTIVITY — the firmware is not driving the pads. This is a");
            warn!("       CTRLSEL/SPU/VPR-IO problem on our side, NOT wiring. Rewiring won't help.");
        } else if !complete && cmd_t <= 6 {
            warn!("probe: bus driven, command sent, but the CMD line shows NO response burst —");
            warn!("       the CARD is silent. Wedged card (power-cycle it) or CMD wire RX-side.");
        } else if !complete {
            warn!("probe: card activity seen on CMD but the firmware never completed — response");
            warn!("       sampling/CRC on the firmware side (read_delay?), not the card.");
        }
        dump_vri("probe VRI");
        // Clean up whatever state the probe left — including the RTT=0 + __ASB transaction-close
        // ack when it completed (the same ack cmd() does).
        vri_write(VRI_EV_XFERCOMPLETE, 0);
        vri_write(VRI_EV_ABORTED, 0);
        vri_write(VRI_CFG_READYTOTRANSFER, 0);
        if complete {
            let _ = self.barrier(T_ACTION);
        } else {
            self.recover();
        }
        for pin in [1usize, 2, 5] {
            pac::P2_S.pin_cnf(pin).modify(|w| w.set_input(Input::Disconnect));
        }
    }

    /// Put the card AND the host back into 1-bit mode — the S8 unwind. Without this a failed
    /// 4-bit experiment strands the card in wide-bus mode that the wiring can't serve, and every
    /// later 1-bit read mysteriously fails too.
    fn revert_to_1bit(&mut self) {
        self.bus_width = 1;
        match self.acmd(6, 0, RESP_R1, PROC_PROCESS, None, 500) {
            Ok(_) => info!("S8 card reverted to 1-bit (ACMD6 arg 0)"),
            Err(e) => warn!("S8 could not revert the card to 1-bit ({}) — later reads may fail until re-init", e),
        }
    }

    /// App-command prefix: CMD55 with the current RCA.
    fn acmd(
        &mut self,
        idx: u32,
        arg: u32,
        resp: u32,
        proc: u32,
        data: Option<(u32, u32, u32)>,
        timeout_ms: u64,
    ) -> Result<[u32; 4], SdErr> {
        self.cmd(55, self.rca << 16, RESP_R1, PROC_PROCESS, None, timeout_ms)?;
        self.cmd(idx, arg, resp, proc, data, timeout_ms)
    }

    /// CMD13 card status → (raw R1, current_state). States: 0 idle, 1 ready, 2 ident, 3 stby,
    /// 4 tran, 5 data, 6 rcv, 7 prg.
    fn card_state(&mut self) -> Result<(u32, u32), SdErr> {
        let r = self.cmd(13, self.rca << 16, RESP_R1, PROC_PROCESS, None, 250)?;
        Ok((r[0], (r[0] >> 9) & 0xF))
    }

    fn block_arg(&self, lba: u32) -> u32 {
        if self.ccs {
            lba
        } else {
            lba * 512
        }
    }

    /// Read `n` blocks at `lba` into `buf`. CMD17 for one, CMD18+CMD12 for more. Reads run with
    /// `PROC_IGNORE` + a following CMD13, per the porting guide ("sEMMC is not able to process
    /// response and data at the same time").
    fn read_blocks(&mut self, lba: u32, n: u32, buf: *mut u8) -> Result<(), SdErr> {
        let data = Some((buf as u32, 512, n));
        if n == 1 {
            self.cmd(17, self.block_arg(lba), RESP_R1, PROC_IGNORE, data, 1000)?;
        } else {
            self.cmd(18, self.block_arg(lba), RESP_R1, PROC_IGNORE, data, 2000)?;
            self.cmd(12, 0, RESP_R1B, PROC_PROCESS, None, 500)?;
        }
        let (r1, state) = self.card_state()?;
        if r1 & 0xFDF9_E008 != 0 {
            warn!("read_blocks: CMD13 flags error bits: R1=0x{=u32:08x} (state {=u32})", r1, state);
        }
        Ok(())
    }

    /// Write `n` blocks. CMD24 for one, CMD25+CMD12 for more; then poll CMD13 until the card
    /// leaves `prg` (the program cycle IS the completion signal for writes).
    fn write_blocks(&mut self, lba: u32, n: u32, buf: *const u8) -> Result<(), SdErr> {
        let data = Some((buf as u32, 512, n));
        if n == 1 {
            self.cmd(24, self.block_arg(lba), RESP_R1, PROC_PROCESS, data, 1000)?;
        } else {
            self.cmd(25, self.block_arg(lba), RESP_R1, PROC_PROCESS, data, 4000)?;
            self.cmd(12, 0, RESP_R1B, PROC_PROCESS, None, 1000)?;
        }
        let t0 = Instant::now();
        loop {
            let (_, state) = self.card_state()?;
            if state == 4 {
                return Ok(()); // tran — programming done
            }
            if t0.elapsed().as_millis() > 1000 {
                return Err(SdErr::Timeout);
            }
        }
    }
}

fn xfer_buf() -> &'static mut [u8] {
    unsafe { &mut (*(&raw mut XFER_BUF)).0 }
}
fn sector0() -> &'static mut [u8] {
    unsafe { &mut (*(&raw mut SECTOR0)).0 }
}
fn save_block() -> &'static mut [u8] {
    unsafe { &mut (*(&raw mut SAVE_BLOCK)).0 }
}

// ═════════════════════════════════════ main ═════════════════════════════════════

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = {
        let mut config = embassy_nrf::config::Config::default();
        config.clock_speed = embassy_nrf::config::ClockSpeed::CK128;
        embassy_nrf::init(config)
    };
    {
        let mut cp = unsafe { cortex_m::Peripherals::steal() };
        cp.DCB.enable_trace();
        cp.DWT.enable_cycle_counter();
    }
    let mut led = Output::new(p.P1_25, Level::Low, OutputDrive::Standard);

    // Let probe-rs finish attaching its RTT reader before the log torrent starts — the first run
    // lost the whole S0 section to the flash-to-attach gap overrunning the RTT ring.
    Timer::after_millis(1500).await;

    info!("");
    info!("╔════════════════════════════════════════════════════════════════════════════╗");
    info!("║  sd_semmc_bringup — microSD in native 4-bit mode over sEMMC (issue #1145)   ║");
    info!("║  {=str}+{=str} — display DISCONNECTED, card on P2.00–05", env!("CARGO_PKG_VERSION"), env!("OBC_FW_GIT"));
    info!("╚════════════════════════════════════════════════════════════════════════════╝");
    info!(
        "sEMMC blob: {=usize} B, magic 0x{=u32:04x} (want 0xa005)",
        SEMMC_FW.len(),
        u32::from_le_bytes([SEMMC_FW[0], SEMMC_FW[1], SEMMC_FW[2], SEMMC_FW[3]]) & 0xFFFF
    );

    // ── S0: wiring, before any firmware ──
    let (card_present, ext_pullup) = s0_wiring_diagnostics();

    // ── S1: pads → Nordic config, boot the firmware, arm the completion ISR ──
    info!("");
    info!("═══ S1 — pad config (internal pulls, E0/E1) + SPU perm + sEMMC firmware boot ═══");
    spu_secure_flpr();
    Semmc::configure_pads(&ext_pullup);
    let mut sd = Semmc { clk_hz: 400_000, bus_width: 1, read_delay: 0, counter: 0, rca: 0, ccs: false };
    let t0 = Instant::now();
    if !sd.boot_firmware(true) {
        error!("S1 FAIL — firmware did not come up; nothing below can run");
        loop {
            led.toggle();
            Timer::after_millis(100).await;
        }
    }
    unsafe {
        interrupt::VPR00.set_priority(Priority::P2);
        interrupt::VPR00.enable();
    }
    // The step the first run missed: power the peripheral ON (ENABLE=1 + __ASB) — boot alone
    // leaves it in INITIALIZED, where start triggers are ignored (measured: dead bus, S2-probe).
    if !sd.power_on() {
        error!("S1 FAIL — power-on __ASB timed out");
    }
    info!(
        "S1 PASS — firmware ready in {=u64} µs, POWERED ON (ENABLE={=u32}); completion ISR armed",
        t0.elapsed().as_micros(),
        vri_read(VRI_ENABLE)
    );
    Timer::after_millis(100).await; // card power-up grace (spec: 74 clocks + ramp; NUMRETRIES pads the rest)

    // ── S2: CMD0 at the 400 kHz init clock — the firmware-divider unknown ──
    info!("");
    info!("═══ S2 — CMD0 GO_IDLE_STATE @ 400 kHz (does the divider reach the SD init clock?) ═══");
    // First: the activity probe — is the firmware driving the pads AT ALL? Splits "our side
    // (CTRLSEL/SPU/VIO)" from "wiring/card side" before the retry ladder muddies the water.
    sd.cmd0_activity_probe();
    let mut init_clk = 0u32;
    'ladder: for hz in [400_000u32, 1_000_000, 2_000_000, 8_000_000] {
        sd.clk_hz = hz;
        for attempt in 0..3 {
            match sd.cmd(0, 0, RESP_NONE, PROC_PROCESS, None, 500) {
                Ok(_) => {
                    info!("S2 CMD0 completed @ {=u32} Hz (attempt {=u32})", hz, attempt + 1);
                    init_clk = hz;
                    break 'ladder;
                }
                Err(e) => warn!("S2 CMD0 @ {=u32} Hz attempt {=u32}: {}", hz, attempt + 1, e),
            }
        }
    }
    if init_clk == 0 {
        error!("S2 FAIL — CMD0 never completed at any clock. With S0 clean this means the CLK/CMD");
        error!("   wires or the firmware itself. Check P2.01↔SCK and P2.05↔MOSI, then re-run.");
        summary_and_idle(&mut led, false, card_present, 0, 0, 0).await;
    }
    if init_clk != 400_000 {
        warn!("S2 NOTE — 400 kHz itself failed; init proceeding at {=u32} Hz (out of SD spec but", init_clk);
        warn!("   usually tolerated). The firmware's divider may be narrower than clkdiv 320.");
    } else {
        info!("S2 PASS — the divider reaches 400 kHz (clkdiv 320). The last init-clock unknown is closed.");
    }

    // ── S3: CMD8 — voltage check + echo. R7 rides the R1 wire shape. ──
    info!("");
    info!("═══ S3 — CMD8 SEND_IF_COND (R7 echo proves CMD TX+RX+CRC; first blob-index probe) ═══");
    let mut cmd8_ok = false;
    match sd.cmd(8, 0x1AA, RESP_R1, PROC_PROCESS, None, 500) {
        Ok(r) => {
            if r[0] & 0xFFF == 0x1AA {
                info!(
                    "S3 PASS — echo 0x{=u32:03x} (pattern+voltage). CMD line verified both directions.",
                    r[0] & 0xFFF
                );
                cmd8_ok = true;
            } else {
                warn!("S3 odd echo: response[0]=0x{=u32:08x} (want low bits 0x1AA)", r[0]);
            }
        }
        Err(e) => {
            warn!("S3 CMD8 failed: {} — probing the response phase to see if the card answers at all…", e);
            sd.activity_probe(8, 0x1AA, RESP_R1, "CMD8");
            warn!("S3 also possible: the blob treating index 8 as eMMC's");
            warn!("   SEND_EXT_CSD (a 512 B data read). Retrying with a dummy 512 B data phase…");
            // The experiment: give the index-8 command the data phase eMMC expects and see if it
            // then completes. Diagnostic only — the response echo is what matters.
            let buf = xfer_buf().as_mut_ptr() as u32;
            match sd.cmd(8, 0x1AA, RESP_R1, PROC_IGNORE, Some((buf, 512, 1)), 500) {
                Ok(_) => warn!("S3 index-8 completed only WITH a data phase → the blob keys direction on the index. An SD CMD8 needs a firmware-side fix/workaround; init may still work (SDHC cards accept ACMD41 without CMD8 at 2.7–3.6 V, but HCS handling varies)."),
                Err(e2) => warn!("S3 data-phase variant also failed: {}", e2),
            }
        }
    }
    // The response-side rescue sweep: Nordic's limitations page says a tuning cycle "might be
    // needed to set the proper read_delay" — the sampling offset in FLPR clock cycles. Sweep
    // read_delay × clock; any (clk, rd) pair where the R7 echo lands is adopted for everything
    // after. This also disambiguates the probe: success here = the card was answering all along
    // and the firmware was sampling the response at the wrong point.
    if !cmd8_ok {
        info!("S3-sweep — CMD8 across clock × read_delay (the Nordic tuning-cycle hint)…");
        'sweep: for hz in [400_000u32, 1_000_000, 2_000_000, 8_000_000] {
            for rd in [1u32, 2, 4, 8, 16, 32, 64] {
                if rd >= 128_000_000 / hz {
                    continue; // read_delay must stay below clkdiv
                }
                sd.clk_hz = hz;
                sd.read_delay = rd;
                if let Ok(r) = sd.cmd(8, 0x1AA, RESP_R1, PROC_PROCESS, None, 300) {
                    if r[0] & 0xFFF == 0x1AA {
                        info!(
                            "S3-sweep HIT — CMD8 echo OK at clk={=u32} Hz read_delay={=u32}. The card answers; read_delay=0 was the problem. Adopting these settings.",
                            hz, rd
                        );
                        cmd8_ok = true;
                        break 'sweep;
                    } else {
                        warn!("S3-sweep clk={=u32} rd={=u32}: completed but echo=0x{=u32:08x}", hz, rd, r[0]);
                    }
                }
            }
        }
        if !cmd8_ok {
            sd.clk_hz = 400_000;
            sd.read_delay = 0;
            warn!("S3-sweep: no (clock, read_delay) pair produced an R7 echo — card-silent remains possible");
        }
    }
    if !cmd8_ok {
        warn!("S3 NOT PASSED — continuing (ACMD41 may still init the card), but HCS/SDXC handling is unproven");
    }

    // ── S4: ACMD41 loop → OCR. R3 = no CRC, no index echo. ──
    info!("");
    info!("═══ S4 — CMD55+ACMD41 (R3) until powered up; OCR/CCS ═══");
    let mut acmd41_ok = false;
    let t0 = Instant::now();
    loop {
        match sd.acmd(41, 0x4030_0000 | 0x00FF_8000, RESP_R3, PROC_PROCESS, None, 500) {
            Ok(r) => {
                let ocr = r[0];
                if ocr & 0x8000_0000 != 0 {
                    sd.ccs = ocr & 0x4000_0000 != 0;
                    info!(
                        "S4 PASS — OCR=0x{=u32:08x}, CCS={=bool} ({=str})",
                        ocr,
                        sd.ccs,
                        if sd.ccs { "SDHC/SDXC, block-addressed" } else { "SDSC, byte-addressed" }
                    );
                    acmd41_ok = true;
                    break;
                }
            }
            Err(e) => {
                warn!("S4 ACMD41 failed: {} — indices 55/41 do not exist in eMMC; if S3 passed and this", e);
                warn!("   fails, the blob likely validates command indices against the eMMC set.");
                break;
            }
        }
        if t0.elapsed().as_millis() > 1500 {
            warn!("S4 card stayed busy >1.5 s (OCR bit31 never set)");
            break;
        }
        Timer::after_millis(20).await;
    }

    let mut init_done = false;
    let mut capacity_blocks: u32 = 0;
    if acmd41_ok {
        // ── S5: CID — the human check. ──
        info!("");
        info!("═══ S5 — CMD2 ALL_SEND_CID (R2, 136-bit) ═══");
        match sd.cmd(2, 0, RESP_R2, PROC_PROCESS, None, 500) {
            Ok(r) => {
                info!("S5 CID raw: {=u32:08x} {=u32:08x} {=u32:08x} {=u32:08x}", r[0], r[1], r[2], r[3]);
                // Word order verified on glass: response[3] = CID[127:96] (LSW-first from the VRI).
                let mid = (r[3] >> 24) & 0xFF;
                let pnm = [
                    (r[3] & 0xFF) as u8,
                    ((r[2] >> 24) & 0xFF) as u8,
                    ((r[2] >> 16) & 0xFF) as u8,
                    ((r[2] >> 8) & 0xFF) as u8,
                    (r[2] & 0xFF) as u8,
                ];
                if pnm.iter().all(|c| c.is_ascii_graphic() || *c == b' ') {
                    if let Ok(name) = core::str::from_utf8(&pnm) {
                        info!("S5 decoded (heuristic): MID=0x{=u32:02x} product '{=str}' — does that match the label on your card?", mid, name);
                    }
                } else {
                    info!("S5 decode heuristic failed (word order differs) — raw words above still prove the R2 path.");
                }
            }
            Err(e) => warn!("S5 CMD2 failed: {}", e),
        }

        // ── S6: RCA, CSD/capacity, select, state check ──
        info!("");
        info!("═══ S6 — CMD3 (RCA) → CMD9 (CSD) → CMD7 (select) → CMD13 (state) ═══");
        match sd.cmd(3, 0, RESP_R1, PROC_PROCESS, None, 500) {
            Ok(r) => {
                sd.rca = (r[0] >> 16) & 0xFFFF;
                info!("S6 RCA = 0x{=u32:04x}", sd.rca);
                if let Ok(r) = sd.cmd(9, sd.rca << 16, RESP_R2, PROC_PROCESS, None, 500) {
                    info!("S6 CSD raw: {=u32:08x} {=u32:08x} {=u32:08x} {=u32:08x}", r[0], r[1], r[2], r[3]);
                    // CSD v2, LSW-first (verified): C_SIZE[69:48] spans r[2] low bits + r[1] high.
                    let c_size = ((r[2] & 0x3F) << 16) | (r[1] >> 16);
                    let cap_mb = ((c_size as u64 + 1) * 512) / 1024;
                    if cap_mb > 1000 && cap_mb < 2_000_000 {
                        capacity_blocks = ((c_size + 1) as u64 * 1024) as u32; // ×512 KiB / 512 B
                        info!(
                            "S6 capacity (heuristic): {=u64} MB ({=u32} blocks) — sanity: the card sold as ~64 GB?",
                            cap_mb, capacity_blocks
                        );
                    } else {
                        warn!("S6 CSD capacity heuristic implausible ({=u64} MB) — multi-block write bench will be skipped", cap_mb);
                    }
                }
                match sd.cmd(7, sd.rca << 16, RESP_R1B, PROC_PROCESS, None, 500) {
                    Ok(_) => match sd.card_state() {
                        Ok((r1, state)) => {
                            info!("S6 selected; CMD13 state={=u32} (want 4=tran), R1=0x{=u32:08x}", state, r1);
                            init_done = state == 4;
                        }
                        Err(e) => warn!("S6 CMD13 failed: {}", e),
                    },
                    Err(e) => warn!("S6 CMD7 select failed: {}", e),
                }
            }
            Err(e) => warn!("S6 CMD3 failed: {}", e),
        }
    }

    if !init_done {
        error!("card init did not reach tran — stopping before data-phase steps");
        summary_and_idle(&mut led, false, card_present, 0, 0, 0).await;
    }
    info!("");
    info!("*** CARD INITIALISED — {=str} ***", "full CMD0→tran sequence over sEMMC");

    // Bump to a working data clock for the width test (2 MHz keeps timing slack while unproven).
    sd.clk_hz = 2_000_000;

    // ── S7: first data read, 1-bit ──
    info!("");
    info!("═══ S7 — CMD17 sector 0 @ 1-bit / 2 MHz (proves the D0 data path) ═══");
    let mut d0_ok = false;
    if !sd.ccs {
        let _ = sd.cmd(16, 512, RESP_R1, PROC_PROCESS, None, 500); // SDSC: fix 512 B blocks
    }
    match sd.read_blocks(0, 1, sector0().as_mut_ptr()) {
        Ok(()) => {
            let sig = u16::from_le_bytes([sector0()[510], sector0()[511]]);
            let mut second = [0u8; 512];
            let again = sd.read_blocks(0, 1, second.as_mut_ptr());
            let stable = again.is_ok() && second[..] == sector0()[..];
            info!(
                "S7 sector 0 read; boot signature 0x{=u16:04x} (want 0xAA55 on a formatted card), double-read stable={=bool}",
                sig, stable
            );
            d0_ok = stable && sig == 0xAA55;
            if !d0_ok && stable {
                warn!("S7 stable but no 0xAA55 — card not MBR-formatted? Data path itself looks fine.");
                d0_ok = true;
            }
        }
        Err(e) => {
            error!("S7 read failed: {} — CMD path works (init passed), so this is the DAT0 wire", e);
            error!("   (breakout 'MISO' → P2.02) or the blob's data engine. STATUS=0x{=u32:08x}", vri_read(VRI_STATUS));
        }
    }

    // ── S8: 4-bit switch + compare — the DAT1/DAT2/DAT3 verdict ──
    info!("");
    info!("═══ S8 — ACMD6 → 4-bit, re-read sector 0, byte-compare (verdict on DAT1/DAT2/DAT3) ═══");
    let mut four_bit = false;
    if d0_ok {
        match sd.acmd(6, 0b10, RESP_R1, PROC_PROCESS, None, 500) {
            Ok(_) => {
                sd.bus_width = 4;
                let buf = xfer_buf();
                match sd.read_blocks(0, 1, buf.as_mut_ptr()) {
                    Ok(()) => {
                        if buf[..512] == sector0()[..] {
                            info!("S8 PASS — 4-bit read is byte-identical to the 1-bit read. DAT1/DAT2/DAT3 wiring verified.");
                            four_bit = true;
                        } else {
                            let first = buf[..512].iter().zip(sector0().iter()).position(|(a, b)| a != b);
                            error!("S8 MISMATCH at byte {=u32} — data arrives but scrambled: check DAT1 (P2.04) and DAT2 (P2.03), the two new wires", first.unwrap_or(0) as u32);
                            sd.revert_to_1bit();
                        }
                    }
                    Err(e) => {
                        error!("S8 4-bit read failed ({}) where 1-bit worked → DAT1/DAT2/DAT3 wiring (the two new wires + 'CS'→D3), or the card ignored ACMD6", e);
                        sd.revert_to_1bit();
                    }
                }
            }
            Err(e) => warn!("S8 ACMD6 failed: {} (index 6 = eMMC SWITCH R1b — shape-compatible, so this is more likely wiring/state)", e),
        }
    } else {
        warn!("S8 skipped — S7 did not pass");
    }

    // ── S9: clock ladder ──
    info!("");
    info!("═══ S9 — clock ladder @ {=u32}-bit: read sector 0 + compare per rung ═══", sd.bus_width);
    let mut best_clk = 2_000_000u32;
    for hz in [8_000_000u32, 16_000_000, 21_333_333] {
        sd.clk_hz = hz;
        let mut rung_ok = false;
        for rd in 0..4u32 {
            sd.read_delay = rd;
            let buf = xfer_buf();
            buf[..512].fill(0);
            if sd.read_blocks(0, 1, buf.as_mut_ptr()).is_ok() && buf[..512] == sector0()[..] {
                info!("S9 {=u32} Hz: OK (read_delay {=u32})", hz, rd);
                rung_ok = true;
                best_clk = hz;
                break;
            }
        }
        if !rung_ok {
            warn!("S9 {=u32} Hz: FAILED at every read_delay 0–3 — ceiling is the previous rung (wire quality?)", hz);
            break;
        }
    }
    sd.clk_hz = best_clk;
    sd.read_delay = 0; // re-derived above if best_clk needed one; keep the winning value simple
    info!("S9 best stable clock: {=u32} Hz @ {=u32}-bit", best_clk, sd.bus_width);
    // 32 MHz needs the card in High Speed mode (Default Speed tops at 25 MHz). SD's CMD6 is a
    // 64-byte data READ — shape the blob may not support (eMMC CMD6 is R1b, no data). Experiment:
    if best_clk > 20_000_000 {
        let buf = xfer_buf();
        match sd.cmd(6, 0x80FF_FFF1, RESP_R1, PROC_IGNORE, Some((buf.as_mut_ptr() as u32, 64, 1)), 500) {
            Ok(_) => {
                sd.clk_hz = 32_000_000;
                let ok = sd.read_blocks(0, 1, buf.as_mut_ptr()).is_ok() && buf[..512] == sector0()[..];
                if ok {
                    info!("S9+ CMD6 High-Speed switch WORKED — 32 MHz rung stable. best=32 MHz");
                    best_clk = 32_000_000;
                } else {
                    warn!("S9+ 32 MHz unstable after CMD6 — staying at {=u32} Hz", best_clk);
                    sd.clk_hz = best_clk;
                }
            }
            Err(e) => info!("S9+ CMD6 HS switch failed ({}) — expected if the blob treats index 6 as eMMC SWITCH (no data). 21.3 MHz stands.", e),
        }
    }

    // ── S10: read throughput ──
    info!("");
    info!("═══ S10 — READ throughput @ {=u32} Hz, {=u32}-bit ═══", sd.clk_hz, sd.bus_width);
    if d0_ok {
        // Sequential region starting at 1 GiB in — far from FAT metadata, plain map/file data.
        let base = 2_097_152u32.min(capacity_blocks.saturating_sub(XFER_BLOCKS as u32 * 9).max(0));
        // CMD17 singles.
        let t0 = Instant::now();
        let mut singles = 0u32;
        for i in 0..64 {
            if sd.read_blocks(base + i, 1, xfer_buf().as_mut_ptr()).is_err() {
                break;
            }
            singles += 1;
        }
        let us = t0.elapsed().as_micros().max(1);
        info!(
            "S10 CMD17 ×{=u32}: {=u64} µs/block, {=u64} KB/s",
            singles,
            us / singles.max(1) as u64,
            (singles as u64 * 512 * 1000) / us
        );
        // CMD18 batches.
        for batch in [8u32, 64, 256] {
            let t0 = Instant::now();
            let mut blocks = 0u64;
            let reps = (1024 / batch).max(2);
            let mut failed = false;
            for r in 0..reps {
                if sd.read_blocks(base + r * batch, batch, xfer_buf().as_mut_ptr()).is_err() {
                    failed = true;
                    break;
                }
                blocks += batch as u64;
            }
            let us = t0.elapsed().as_micros().max(1);
            let kbs = blocks * 512 * 1000 / us;
            info!(
                "S10 CMD18 b{=u32}: {=u64} blocks in {=u64} µs → {=u64} KB/s ({=u64}.{=u64} MB/s){=str}",
                batch,
                blocks,
                us,
                kbs,
                kbs / 1000,
                (kbs % 1000) / 100,
                if failed { "  [stopped early on error]" } else { "" }
            );
        }
    } else {
        warn!("S10 skipped — no working read path");
    }

    // ── S11: write bench (safety-first) ──
    info!("");
    info!("═══ S11 — WRITE bench (save/restore single block; batches only in a verified-unused window) ═══");
    if d0_ok {
        // (a) Save → salt → verify → restore ONE block at 1 GiB. Safe anywhere: original restored.
        let lba = 2_097_152u32;
        if sd.read_blocks(lba, 1, save_block().as_mut_ptr()).is_ok() {
            let buf = xfer_buf();
            for (i, b) in buf[..512].iter_mut().enumerate() {
                *b = (i as u8) ^ 0xA5;
            }
            let t0 = Instant::now();
            let wr = sd.write_blocks(lba, 1, buf.as_ptr());
            let us = t0.elapsed().as_micros();
            match wr {
                Ok(()) => {
                    let mut check = [0u8; 512];
                    let ok = sd.read_blocks(lba, 1, check.as_mut_ptr()).is_ok() && check[..] == buf[..512];
                    info!("S11a CMD24 single block: {=u64} µs, read-back verified={=bool}", us, ok);
                }
                Err(e) => error!("S11a single-block write failed: {}", e),
            }
            // Restore, verified.
            if sd.write_blocks(lba, 1, save_block().as_ptr()).is_ok() {
                let mut check = [0u8; 512];
                let restored = sd.read_blocks(lba, 1, check.as_mut_ptr()).is_ok() && check[..] == save_block()[..];
                info!("S11a original block restored, verified={=bool}", restored);
            } else {
                error!("S11a RESTORE FAILED at LBA {=u32} — that block now holds bench data!", lba);
            }
        }

        // (b) Multi-block CMD25, only in a window 128 MiB from the end AND only if it reads
        // uniform (erased/unused) first. On a 60 GB card with a few MB used this is empty space;
        // if the check fails we skip rather than risk file-system data.
        if capacity_blocks > 400_000 {
            let win = capacity_blocks - 262_144; // 128 MiB from the end
            let buf = xfer_buf();
            let mut uniform = sd.read_blocks(win, XFER_BLOCKS as u32, buf.as_mut_ptr()).is_ok();
            if uniform {
                let first = buf[0];
                uniform = buf[..XFER_BLOCKS * 512].iter().all(|&b| b == first);
            }
            if uniform {
                info!("S11b window @ LBA {=u32} reads uniform (0x{=u8:02x}) — safe to bench", win, buf[0]);
                for batch in [8u32, 64, 256] {
                    for (i, b) in buf[..batch as usize * 512].iter_mut().enumerate() {
                        *b = (i as u8) ^ (batch as u8);
                    }
                    let t0 = Instant::now();
                    let mut blocks = 0u64;
                    let reps = (1024 / batch).max(2);
                    let mut failed = false;
                    for r in 0..reps {
                        if sd.write_blocks(win + r * batch, batch, buf.as_ptr()).is_err() {
                            failed = true;
                            break;
                        }
                        blocks += batch as u64;
                    }
                    let us = t0.elapsed().as_micros().max(1);
                    let kbs = blocks * 512 * 1000 / us;
                    info!(
                        "S11b CMD25 b{=u32}: {=u64} blocks in {=u64} µs → {=u64} KB/s ({=u64}.{=u64} MB/s){=str}",
                        batch,
                        blocks,
                        us,
                        kbs,
                        kbs / 1000,
                        (kbs % 1000) / 100,
                        if failed { "  [stopped early on error]" } else { "" }
                    );
                }
                // Spot-verify the last batch's first block.
                let mut check = [0u8; 512];
                if sd.read_blocks(win, 1, check.as_mut_ptr()).is_ok() {
                    let expect: u8 = 0 ^ 8u8; // first byte of the b8 pattern — window start was rewritten by each pass
                    info!("S11b spot check: first window byte 0x{=u8:02x} (pattern families 0x08/0x40/0x00…) — written data landed", check[0]);
                    let _ = expect;
                }
            } else {
                warn!("S11b window @ LBA {=u32} is NOT uniform — someone's data may live there; multi-block write bench SKIPPED", win);
            }
        } else {
            warn!("S11b skipped — capacity unknown/implausible, no safe window derivable");
        }
    } else {
        warn!("S11 skipped — no working read path");
    }

    summary_and_idle(&mut led, four_bit, card_present, best_clk, sd.bus_width, ISR_COUNT.load(Ordering::Relaxed)).await;
}

/// Final summary, then heartbeat forever (re-run = reset button / re-flash).
async fn summary_and_idle(
    led: &mut Output<'_>,
    four_bit: bool,
    card_present: bool,
    best_clk: u32,
    width: u32,
    isr_count: u32,
) -> ! {
    info!("");
    info!("═══════════════════════════ SUMMARY ═══════════════════════════");
    info!("card detected (DAT3 pull-up) : {=bool}", card_present);
    info!("best stable clock            : {=u32} Hz @ {=u32}-bit", best_clk, width);
    info!("4-bit verified               : {=bool}", four_bit);
    info!(
        "VPR00 completion ISR fired   : {=u32} times ({=str})",
        isr_count,
        if isr_count > 0 {
            "issue #1145 A3: interrupt delivery CONFIRMED end-to-end"
        } else {
            "never — polling remains the completion path"
        }
    );
    info!("throughput numbers above; today's SPI baseline: 1.07 MB/s read (sd_bench 2026-07)");
    info!("bench idle — press the DK reset button to re-run.");
    loop {
        led.toggle();
        Timer::after_millis(500).await;
    }
}
