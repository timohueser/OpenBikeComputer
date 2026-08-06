//! **THROWAWAY integration demo for issue #1145** — the Option A architecture end to end on the
//! NEW harness: LS021 display on the rehomed pins + microSD over sEMMC, **time-multiplexed on the
//! one FLPR**, switching every round exactly the way the real firmware will.
//!
//! ⚠️ DELETE THIS FILE (and its `[[bin]]` stanza) before the implementation merges.
//! ⚠️ `include_bytes!`s Nordic's blob from `~/.cache/obc/semmc_fw.bin` (see `sd_semmc_bringup`).
//!
//!     cargo run --release --bin sd_display_mux
//!
//! ## The new pin map this runs on (issue #1145 §1, wired 2026-08-06)
//!
//! ```text
//! SD (sEMMC, fixed):  P2.00=D3  P2.01=CLK  P2.02=D0  P2.03=D2  P2.04=D1  P2.05=CMD
//! Display data (new): R0=P2.06  R1=P2.08  G0=P2.09  G1=P2.10  B0=P2.00* B1=P2.04*
//!                     (* time-shared with SD D3/D1 — CTRLSEL flips per mode)
//! Unchanged:          BCK=P2.07, BSP=P1.14, GSP/GCK/GEN/INTB=P1.10–13, COM=P1.22–24
//! ```
//!
//! The FLPR scan blob (`flpr_scan.c`) carries the matching remapped `DATA_MASK 0x751` — the
//! normative host-side pack (`ls021_wire`) + goldens follow in the production PR.
//!
//! ## What a round does (the ride loop in miniature)
//!
//! 1. **→ storage**: park the hart, SD pads → `CTRLSEL=VPR`, warm-boot the resident sEMMC image
//!    (~22 µs), power on. `CMD13` must report the card still in `tran` **without re-init** — the
//!    card-side state survives the swap (bench item B1).
//! 2. Read 512 KiB at 32 MHz / 4-bit, plus re-verify sector 0 against the boot golden.
//! 3. **→ display**: park the hart, shared pads → `CTRLSEL=GPIO`, relaunch the display blob,
//!    push a repainted frame (colour bars + a moving marker so the change is visible).
//! 4. COM never stops — it free-runs on the M33 the whole time, both modes (the load-bearing
//!    property from the feasibility study).
//!
//! Watch the glass: the image must update every round AND hold rock-steady during the storage
//! phases (SD traffic toggles the two shared B0/B1 lines while BCK/GEN are idle — the display
//! must not care; that is blocker 5's display-direction half).
#![no_std]
#![no_main]

#[allow(dead_code)]
#[path = "../com.rs"]
mod com;
#[allow(dead_code)]
#[path = "../ls021_flpr.rs"]
mod ls021_flpr;

use core::ptr::addr_of_mut;

use defmt::{error, info, warn};
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::pac;
use embassy_nrf::pac::gpio::vals::{Ctrlsel, Dir, Drive, Input, Pull};
use embassy_time::{Instant, Timer};
use nrf_mpsl as _;
use panic_probe as _;

use ls021_flpr::{launch_flpr, Frame64, Ls021Flpr, FB_H, FB_W};
use obc_display::ls021::RowDiff;

/// Nordic's sEMMC v0.1.1 PIC firmware (regeneration one-liner in `sd_semmc_bringup`'s docs).
static SEMMC_FW: &[u8] = include_bytes!("/Users/timo/.cache/obc/semmc_fw.bin");

// ── sEMMC memory + VRI map (established in sd_semmc_bringup; see there for provenance) ──
const FW_CODE_BYTES: usize = 15360;
const VRI_OFFSET: usize = 16896;
const CARVE_BYTES: usize = 17408;
#[repr(C, align(4096))]
struct Carve([u8; CARVE_BYTES]);
static mut SEMMC_CARVE: Carve = Carve([0; CARVE_BYTES]);

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
const VRI_CMD_CMD: usize = 0x44;
const VRI_CMD_ARG: usize = 0x48;
const VRI_CMD_RESPONSEADDR: usize = 0x4C;
const VRI_CMD_RESPONSE0: usize = 0x50;
const VRI_DATA_BUFFERADDR: usize = 0x64;
const VRI_DATA_BLOCKSIZE: usize = 0x68;
const VRI_DATA_BLOCKNUM: usize = 0x6C;
const VRI_SPSYNC_AUX: usize = 0x74;

const RESP_NONE: u32 = 0;
const RESP_R1: u32 = 1;
const RESP_R1B: u32 = 2;
const RESP_R2: u32 = 3;
const RESP_R3: u32 = 4;
const PROC_PROCESS: u32 = 0;
const PROC_IGNORE: u32 = 1;

const T_DPPI0: usize = 16;
const T_CONFIG: usize = 17;
const T_ACTION: usize = 18;
const T_STOP: usize = 19;

const VPR00_TASKS_TRIGGER: *mut u32 = 0x5004_C000 as *mut u32;
const VPR00_CPURUN: *mut u32 = 0x5004_C800 as *mut u32;
const VPR00_INITPC: *mut u32 = 0x5004_C808 as *mut u32;
const VPR00_DMCONTROL: *mut u32 = 0x5004_C440 as *mut u32;
const DM_DMACTIVE: u32 = 1 << 0;
const DM_NDMRESET: u32 = 1 << 1;

/// The six sEMMC pads. Adaptive pulls, hardcoded from `sd_semmc_bringup`'s S0 on THIS breakout:
/// CLK/D0/D2/CMD carry breakout pull-ups (internal pull off — 13k∥10k would leave the SD spec
/// window); D3/D1 have none (internal pull-up on).
const SD_PADS: [usize; 6] = [0, 1, 2, 3, 4, 5];
/// The two pads time-shared with display B0/B1.
const SHARED_PADS: [usize; 2] = [0, 4];

/// 256-block transfer buffer, 32-bit aligned.
const XFER_BLOCKS: usize = 256;
#[repr(C, align(4))]
struct XferBuf([u8; XFER_BLOCKS * 512]);
static mut XFER_BUF: XferBuf = XferBuf([0; XFER_BLOCKS * 512]);
static mut GOLDEN0: [u8; 512] = [0; 512];
static mut RESP_RAW: [u32; 16] = [0; 16];

static mut FB: [u8; FB_W * FB_H] = [0; FB_W * FB_H];
static mut ROW_DIFF: RowDiff<FB_H> = RowDiff::new();

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
fn xfer_buf() -> &'static mut [u8] {
    unsafe { &mut (*(&raw mut XFER_BUF)).0 }
}
fn golden0() -> &'static mut [u8] {
    unsafe { &mut *(&raw mut GOLDEN0) }
}

fn park_hart() {
    unsafe {
        VPR00_CPURUN.write_volatile(0);
        VPR00_DMCONTROL.write_volatile(DM_NDMRESET | DM_DMACTIVE);
        VPR00_DMCONTROL.write_volatile(DM_DMACTIVE);
        VPR00_DMCONTROL.write_volatile(0);
    }
}

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

/// Storage mode: all six SD pads → Nordic reference config + `CTRLSEL=VPR`.
fn storage_pads() {
    for pin in SD_PADS {
        let pull = if pin == 0 || pin == 4 { Pull::Pullup } else { Pull::Disabled };
        cfg_pin(pin, Dir::Output, Input::Disconnect, pull, true, Ctrlsel::Vpr);
    }
    pac::GPIOHSPADCTRL_S.bias().modify(|w| w.set_hsbias(2));
}

/// Display mode: the two shared pads become display data outputs under GPIO control (the FLPR
/// blob drives them via OUTSET/OUTCLR); the four card-only pads are parked as inputs — the
/// breakout's own pull-ups hold CLK/CMD/D0/D2 high, which is exactly the SD idle-bus state.
fn display_pads() {
    for pin in SHARED_PADS {
        cfg_pin(pin, Dir::Output, Input::Disconnect, Pull::Disabled, false, Ctrlsel::Gpio);
    }
    for pin in [1usize, 2, 3, 5] {
        cfg_pin(pin, Dir::Input, Input::Disconnect, Pull::Disabled, false, Ctrlsel::Gpio);
    }
}

// ── The minimal sEMMC host (condensed from sd_semmc_bringup, same laws: power-on after boot,
//    transaction-close ack after every completion, R2 words LSW-first). ──

#[derive(Clone, Copy, PartialEq, defmt::Format)]
enum SdErr {
    BarrierTimeout,
    Timeout,
    Aborted(u32),
}

struct Semmc {
    clk_hz: u32,
    bus_width: u32,
    read_delay: u32,
    num_retries: u32,
    counter: u32,
    rca: u32,
    ccs: bool,
}

impl Semmc {
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
                return false;
            }
        }
        vri_write(VRI_INTEN, 0x7);
        true
    }

    fn power_on(&mut self) -> bool {
        vri_write(VRI_ENABLE, 1);
        let ok = self.barrier(T_ACTION).is_ok();
        vri_write(VRI_EV_XFERCOMPLETE, 0);
        vri_write(VRI_EV_ABORTED, 0);
        vri_write(VRI_EV_READYTOTRANSFER, 0);
        ok
    }

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

    fn recover(&mut self) {
        let _ = self.barrier(T_STOP);
        if !self.boot_firmware(false) {
            self.boot_firmware(true);
        }
        self.power_on();
    }

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
        vri_write(VRI_CFG_NUMRETRIES, self.num_retries);
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
                let status = vri_read(0x70);
                vri_write(VRI_EV_ABORTED, 0);
                vri_write(VRI_EV_XFERCOMPLETE, 0);
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
        vri_write(VRI_CFG_READYTOTRANSFER, 0);
        self.barrier(T_ACTION)?;
        Ok([
            vri_read(VRI_CMD_RESPONSE0),
            vri_read(VRI_CMD_RESPONSE0 + 4),
            vri_read(VRI_CMD_RESPONSE0 + 8),
            vri_read(VRI_CMD_RESPONSE0 + 12),
        ])
    }

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

    fn read_blocks(&mut self, lba: u32, n: u32, buf: *mut u8) -> Result<(), SdErr> {
        let data = Some((buf as u32, 512, n));
        if n == 1 {
            self.cmd(17, self.block_arg(lba), RESP_R1, PROC_IGNORE, data, 1000)?;
        } else {
            self.cmd(18, self.block_arg(lba), RESP_R1, PROC_IGNORE, data, 2000)?;
            self.cmd(12, 0, RESP_R1B, PROC_PROCESS, None, 500)?;
        }
        self.card_state()?;
        Ok(())
    }

    fn cmd8_deliver_abort(&mut self) -> Result<(), SdErr> {
        self.cmd_start(8, 0x1AA, RESP_R1, PROC_PROCESS, None)?;
        cortex_m::asm::delay(3000 * 128);
        let stopped = self.barrier(T_STOP).is_ok();
        let mut acked = false;
        if stopped {
            let t0 = Instant::now();
            while t0.elapsed().as_millis() < 50 {
                if vri_read(VRI_EV_ABORTED) != 0 || vri_read(VRI_EV_XFERCOMPLETE) != 0 {
                    acked = true;
                    break;
                }
            }
        }
        vri_write(VRI_EV_ABORTED, 0);
        vri_write(VRI_EV_XFERCOMPLETE, 0);
        vri_write(VRI_CFG_READYTOTRANSFER, 0);
        if !(stopped && acked && self.barrier(T_ACTION).is_ok()) {
            self.recover();
        }
        Ok(())
    }

    /// The full boot-time card bring-up, with all three laws + both blob-wart workarounds baked
    /// in: CMD0 → CMD8 deliver-abort → ACMD41 → CMD2/3/7 → ACMD6 4-bit → CMD6+drain-read High
    /// Speed → 32 MHz verified. Fills `golden0` with sector 0.
    fn init_card(&mut self) -> bool {
        self.clk_hz = 400_000;
        self.bus_width = 1;
        for _ in 0..3 {
            if self.cmd(0, 0, RESP_NONE, PROC_PROCESS, None, 300).is_ok() {
                break;
            }
        }
        let _ = self.cmd8_deliver_abort();
        let t0 = Instant::now();
        loop {
            match self.acmd(41, 0x4030_0000 | 0x00FF_8000, RESP_R3, PROC_PROCESS, None, 500) {
                Ok(r) if r[0] & 0x8000_0000 != 0 => {
                    self.ccs = r[0] & 0x4000_0000 != 0;
                    break;
                }
                _ if t0.elapsed().as_millis() > 1500 => {
                    error!("init: ACMD41 never left busy");
                    return false;
                }
                _ => cortex_m::asm::delay(20_000 * 128),
            }
        }
        if self.cmd(2, 0, RESP_R2, PROC_PROCESS, None, 500).is_err() {
            return false;
        }
        match self.cmd(3, 0, RESP_R1, PROC_PROCESS, None, 500) {
            Ok(r) => self.rca = (r[0] >> 16) & 0xFFFF,
            Err(_) => return false,
        }
        if self.cmd(7, self.rca << 16, RESP_R1B, PROC_PROCESS, None, 500).is_err() {
            return false;
        }
        if self.acmd(6, 0b10, RESP_R1, PROC_PROCESS, None, 500).is_err() {
            return false;
        }
        self.bus_width = 4;
        self.clk_hz = 21_333_333;
        if self.read_blocks(0, 1, golden0().as_mut_ptr()).is_err() {
            error!("init: 4-bit golden read failed");
            return false;
        }
        // High Speed via the drain-read trick, then verify 32 MHz against the golden.
        if self.cmd(6, 0x80FF_FFF1, RESP_R1B, PROC_PROCESS, None, 500).is_ok() {
            self.num_retries = 0;
            let buf = xfer_buf();
            let _ = self.cmd(17, self.block_arg(0), RESP_R1, PROC_IGNORE, Some((buf.as_mut_ptr() as u32, 512, 1)), 300);
            self.num_retries = 3;
            let mut in_tran = false;
            for _ in 0..4 {
                if let Ok((_, s)) = self.card_state() {
                    if s == 4 {
                        in_tran = true;
                        break;
                    }
                }
            }
            if in_tran {
                self.clk_hz = 32_000_000;
                let buf = xfer_buf();
                buf[..512].fill(0);
                if self.read_blocks(0, 1, buf.as_mut_ptr()).is_ok() && buf[..512] == golden0()[..] {
                    info!("init: High Speed on — 32 MHz verified");
                } else {
                    warn!("init: 32 MHz not stable — staying at 21.3 MHz");
                    self.clk_hz = 21_333_333;
                }
            }
        }
        let sig = u16::from_le_bytes([golden0()[510], golden0()[511]]);
        info!(
            "init: card up — {=u32}-bit @ {=u32} Hz, CCS={=bool}, sector0 sig 0x{=u16:04x}",
            self.bus_width, self.clk_hz, self.ccs, sig
        );
        true
    }
}

/// Paint the round's frame: six colour bars, a moving white marker band (so every round visibly
/// changes), and a thin frame counter strip whose parity inverts each round.
fn paint(frame: &mut Frame64, round: u32) {
    const BANDS: [u8; 6] = [0x03, 0x0C, 0x30, 0x0F, 0x3C, 0x3F];
    let bytes = frame.bytes_mut();
    for (y, row) in bytes.chunks_exact_mut(FB_W).enumerate() {
        for (x, px) in row.iter_mut().enumerate() {
            *px = if y < 16 {
                // Parity strip: alternating checker whose phase flips each round.
                if ((x / 8) + round as usize) % 2 == 0 {
                    0x3F
                } else {
                    0x00
                }
            } else if (40..104).contains(&y) {
                // Marker band: black with a white block that walks right each round.
                let pos = (round as usize * 24) % (FB_W - 32);
                if x >= pos && x < pos + 32 {
                    0x3F
                } else {
                    0x00
                }
            } else {
                BANDS[x * BANDS.len() / FB_W]
            };
        }
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = {
        let mut config = embassy_nrf::config::Config::default();
        config.clock_speed = embassy_nrf::config::ClockSpeed::CK128;
        embassy_nrf::init(config)
    };
    let mut led = Output::new(p.P1_25, Level::Low, OutputDrive::Standard);
    Timer::after_millis(1500).await; // RTT settle

    info!("");
    info!("╔════════════════════════════════════════════════════════════════════════════╗");
    info!("║  sd_display_mux — display (NEW pins) + SD over sEMMC, time-multiplexed      ║");
    info!("║  {=str}+{=str}", env!("CARGO_PKG_VERSION"), env!("OBC_FW_GIT"));
    info!("╚════════════════════════════════════════════════════════════════════════════╝");

    // ── Pin claims. Display: gate + BSP as before; data on the REHOMED pins. The two shared
    //    pads (P2.00/P2.04) are claimed as outputs too — the per-mode pad functions retarget
    //    their PIN_CNF (drive/pull/ctrlsel) around embassy's claim, same pattern as main.rs's
    //    E0E1 pokes on the old SD pins. ──
    let _gate_bus = [
        Output::new(p.P1_10, Level::Low, OutputDrive::Standard), // GSP
        Output::new(p.P1_11, Level::Low, OutputDrive::Standard), // GCK
        Output::new(p.P1_12, Level::Low, OutputDrive::Standard), // GEN
        Output::new(p.P1_13, Level::Low, OutputDrive::Standard), // INTB
    ];
    let _src_bus = [
        Output::new(p.P1_14, Level::Low, OutputDrive::Standard), // BSP
        Output::new(p.P2_07, Level::Low, OutputDrive::Standard), // BCK (unchanged)
        Output::new(p.P2_06, Level::Low, OutputDrive::Standard), // R0  (was SD-SPI SCK)
        Output::new(p.P2_08, Level::Low, OutputDrive::Standard), // R1  (was SD-SPI MOSI)
        Output::new(p.P2_09, Level::Low, OutputDrive::Standard), // G0  (was SD-SPI MISO)
        Output::new(p.P2_10, Level::Low, OutputDrive::Standard), // G1  (was SD-SPI CS)
        Output::new(p.P2_00, Level::Low, OutputDrive::Standard), // B0  (shared: sEMMC D3)
        Output::new(p.P2_04, Level::Low, OutputDrive::Standard), // B1  (shared: sEMMC D1)
    ];
    let (vcom, vb, va) = (
        Output::new(p.P1_22, Level::Low, OutputDrive::HighDrive),
        Output::new(p.P1_23, Level::Low, OutputDrive::HighDrive),
        Output::new(p.P1_24, Level::Low, OutputDrive::HighDrive),
    );

    // ── Phase 1: storage first — boot the sEMMC firmware and bring the card all the way up. ──
    info!("═══ phase 1 — storage: card init over sEMMC (new harness) ═══");
    storage_pads();
    let mut sd = Semmc { clk_hz: 400_000, bus_width: 1, read_delay: 0, num_retries: 3, counter: 0, rca: 0, ccs: false };
    if !sd.boot_firmware(true) || !sd.power_on() {
        error!("sEMMC firmware did not come up — check the six SD pads");
        loop {
            led.toggle();
            Timer::after_millis(100).await;
        }
    }
    if !sd.init_card() {
        error!("card init failed — bench stopped (is the card seated?)");
        loop {
            led.toggle();
            Timer::after_millis(100).await;
        }
    }

    // ── Phase 2: display — relaunch the FLPR with the (remapped) scan blob, first frame, COM. ──
    info!("═══ phase 2 — display: first frame on the REHOMED pins ═══");
    park_hart();
    display_pads();
    if let Err(e) = launch_flpr().await {
        error!("display blob did not come up ({}) — check gate/BSP wiring", e);
        loop {
            led.toggle();
            Timer::after_millis(100).await;
        }
    }
    let fb: &'static mut [u8] = unsafe { &mut *addr_of_mut!(FB) };
    let diff: &'static mut RowDiff<FB_H> = unsafe { &mut *addr_of_mut!(ROW_DIFF) };
    let mut frame = Frame64::new(fb);
    let mut panel = Ls021Flpr::new(diff);

    panel.push_frame(&frame).await; // datasheet Initial #0: black frame, COM still low
    Timer::after_micros(50).await;
    spawner.spawn(defmt::unwrap!(com::com_task(vcom, vb, va)));
    paint(&mut frame, 0);
    let ok = panel.push_frame(&frame).await;
    info!("first image pushed (ok={=bool}) — colour bars + marker should be on glass NOW", ok);
    Timer::after_secs(2).await;

    // ── Phase 3: the mux loop — the ride loop in miniature. ──
    info!("═══ phase 3 — 12 rounds of storage↔display, the real-firmware pattern ═══");
    let mut all_ok = true;
    for round in 1..=12u32 {
        // → storage
        let t0 = Instant::now();
        park_hart();
        storage_pads();
        let boot_ok = sd.boot_firmware(false) && sd.power_on();
        let sw_store_us = t0.elapsed().as_micros();

        // Card must still be in tran — NO re-init (bench item B1: state survives the swap).
        let state = sd.card_state().map(|(_, s)| s).unwrap_or(99);
        // 512 KiB read at the initialised clock/width + golden re-verify.
        let t1 = Instant::now();
        let mut read_ok = true;
        for i in 0..4u32 {
            if sd
                .read_blocks(
                    2_097_152 + (round * 4 + i) * XFER_BLOCKS as u32,
                    XFER_BLOCKS as u32,
                    xfer_buf().as_mut_ptr(),
                )
                .is_err()
            {
                read_ok = false;
                break;
            }
        }
        let read_us = t1.elapsed().as_micros().max(1);
        let kbs = if read_ok { (4 * XFER_BLOCKS as u64 * 512 * 1000) / read_us } else { 0 };
        let golden_ok = {
            let buf = xfer_buf();
            buf[..512].fill(0);
            sd.read_blocks(0, 1, buf.as_mut_ptr()).is_ok() && buf[..512] == golden0()[..]
        };

        // → display
        let t2 = Instant::now();
        park_hart();
        display_pads();
        let disp_ok = launch_flpr().await.is_ok();
        let sw_disp_us = t2.elapsed().as_micros();
        paint(&mut frame, round);
        let t3 = Instant::now();
        let frame_ok = disp_ok && panel.push_frame(&frame).await;
        let frame_ms = t3.elapsed().as_millis();

        let round_ok = boot_ok && state == 4 && read_ok && golden_ok && frame_ok;
        all_ok &= round_ok;
        info!(
            "round {=u32:02}: →store {=u64} µs | state={=u32}{=str} | read {=u64} KB/s golden={=bool} | →disp {=u64} µs | frame {=u64} ms | {=str}",
            round,
            sw_store_us,
            state,
            if state == 4 { " (tran, NO re-init)" } else { " ⚠" },
            kbs,
            golden_ok,
            sw_disp_us,
            frame_ms,
            if round_ok { "OK" } else { "FAIL" }
        );
        led.toggle();
        Timer::after_millis(300).await; // let the eye catch the frame
    }

    info!("");
    info!("═══ SUMMARY — {=str} ═══", if all_ok { "ALL 12 ROUNDS CLEAN" } else { "FAILURES — see rounds above" });
    info!("• card stayed in tran across every swap (B1) — no re-init ever issued");
    info!("• visual checklist: did the marker walk right each round, and did the image hold");
    info!("  perfectly still during the storage phases (SD traffic on shared B0/B1)?");
    info!("bench idle — reset to re-run.");
    loop {
        led.toggle();
        Timer::after_millis(500).await;
    }
}
