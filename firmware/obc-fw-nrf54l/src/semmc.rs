//! **The microSD host: Nordic's sEMMC soft peripheral on the FLPR** (epic #1158, proven in #1145).
//!
//! The nRF54L has no SD host controller. Nordic ships one as a *soft peripheral*: a small
//! position-independent RISC-V image that the FLPR (the VPR00 coprocessor) executes, turning six
//! GPIOs into a real 4-bit SD bus. The M33 does not bit-bang anything — it fills in a register
//! block (the **VRI**) that lives in the image's own RAM carve and pokes VPR tasks; the FLPR does
//! the clocking, CRC and framing. Measured on glass 2026-08-05/06: **14.7 MB/s read** (CMD18, 256
//! blocks, 32 MHz, 4-bit) and **8.2 MB/s write** (CMD25, 21.3 MHz) against 1.07 MB/s over the SPI
//! transport this replaces.
//!
//! The image is vendored at `vendor/semmc/` (`LicenseRef-Nordic-5-Clause` — provenance, license
//! text and the regeneration one-liner are in that directory's `README.md`). Its carve —
//! 17,408 B in a 20 KiB 4 KiB-aligned region immediately below the display FLPR's — is defined
//! once in `build.rs`'s `contract` module, which emits the constants included below *and* shrinks
//! the M33's `RAM` region to match, and which cross-checks them against the image's own metadata
//! header so a blob update cannot silently mis-size the carve.
//!
//! ## Not wired up yet
//!
//! This module is **build-only in this PR**: nothing in `main.rs` constructs a [`Semmc`] and the
//! `VPR00` interrupt vector is unclaimed. The integration PR of epic #1158 swaps `sd.rs`'s
//! transport onto it, adds the display/storage mode scheduler around
//! [`enter_storage_mode`](Semmc::enter_storage_mode) / [`leave_storage_mode`](Semmc::leave_storage_mode),
//! binds the vector to [`on_vpr00_irq`], deletes the SPI path, and updates
//! `firmware/docs/ls021-flpr.md` + the board README. Until then the module carries an
//! `#[allow(dead_code)]` at its declaration in `main.rs`.
//!
//! The public surface is deliberately the whole sequence and nothing below it —
//! [`Semmc::start`] once per power-on (async: it is the only path that waits in milliseconds),
//! then `enter_storage_mode` / `leave_storage_mode` per handover and the two sync transfer calls.
//! Boot, enable and card identification are private on purpose: law 1 is only a law if a caller
//! cannot boot the firmware without powering it on.
//!
//! ## The three laws (violating any = a silent hang; each one cost real on-glass debugging)
//!
//! 1. **Boot leaves the firmware INITIALIZED, not powered on.** It ignores start triggers until
//!    `ENABLE = 1` followed by an `__ASB` barrier ([`Semmc::enable`], Nordic's `nrf_semmc_enable`).
//!    Symptom of skipping it: a dead bus, zero clock activity, and nothing to see in the VRI.
//! 2. **Every completion needs the transaction-close ack**: `CONFIG.READYTOTRANSFER = 0` *plus*
//!    an `__ASB` ([`Semmc::close_transaction`]) — the tail of Nordic's own IRQ handler. Symptom of
//!    skipping it: the first command after a boot works and the second times out, forever.
//! 3. **R2 responses land LSW-first**: `RESPONSE[0]` is bits 31:0 and `RESPONSE[3]` is bits 127:96.
//!    Every CID/CSD field decode here is written against that order.
//!
//! ## The barrier
//!
//! Every state change is fenced by one of the soft-peripheral `__XSBx` macros
//! (nrfxlib `softperipheral_regif.h`): write the host counter into `SPSYNC.AUX[0]`, trigger the
//! matching VPR task (`__CSB` 17 = config, `__ASB` 18 = action, `__SSB` 19 = stop), spin until the
//! firmware echoes the counter into `AUX[1]`. ~2.2 µs measured. Start is task 16 (the DPPI start),
//! and completion raises the VRI's `EVENTS_XFERCOMPLETE`/`ABORTED` plus VEVIF event 20 →
//! `VPR00_IRQn` (see [`on_vpr00_irq`]).
//!
//! ## The two blob warts, and the workarounds that beat them
//!
//! Root cause (established on glass, not card-dependent): the image's per-index transfer table is
//! hard-coded to **eMMC** semantics, and data descriptors on indexes it thinks carry no data are
//! ignored. Two SD commands fall in the gap:
//!
//! - **CMD8** — eMMC index 8 is `SEND_EXT_CSD`, a 512 B read; SD's `SEND_IF_COND` is
//!   response-only, so the host wait never completes. *Deliver and abort*: send it, wait ~3 ms
//!   (the card's R7 is on the wire by then, which is all the SD spec needs from CMD8 — it arms
//!   ACMD41's HCS handling), abandon the wait via `__SSB`, ack, continue. See
//!   [`Semmc::cmd8_deliver_abort`].
//! - **CMD6** — eMMC index 6 is `SWITCH`, an R1b with no data; SD's `SWITCH_FUNC` streams a 64 B
//!   status block that is then orphaned, leaving the card stuck in `data`. *Drain read*: follow it
//!   with a CMD17 (`NUMRETRIES = 0`, or the firmware's retry hunts for a second start bit
//!   forever). The card ignores the CMD17 — it is illegal in `data` — while the firmware's 512 B
//!   read engine supplies exactly the clocks the orphaned block needs, landing it in our buffer,
//!   where `byte16 & 0xF == 1` is the card's own High-Speed confirmation. The card returns to
//!   `tran`; the read itself fails length/CRC and aborts, which we ack normally. See
//!   [`Semmc::cmd6_high_speed`].
//!
//! Both disappear if Nordic ships a VRI data-phase override (direction + block count beating the
//! index table); a DevZone request for that is tracked in #1158.
//!
//! ## What this module does *not* do
//!
//! It never panics and never waits without a deadline — and the deadline is reachable with **no
//! interrupts arriving at all**, which is the whole reason [`Semmc::wait_completion`] is a bounded
//! poll with an interrupt fast path rather than the `WFE` sleep it looks like it should be (that
//! version could sleep past its own deadline; read the note there before changing it back). Every
//! failure is a [`SemmcError`]. Storage that cannot be brought up is the caller's decision to
//! present (the boot-fault honesty rule lives above this layer, in `obc-app`).

use core::sync::atomic::{AtomicBool, Ordering};

use defmt::warn;
use embassy_nrf::pac;
use embassy_nrf::pac::gpio::vals::{Ctrlsel, Dir, Drive, Input, Pull};
use embassy_time::{Duration, Instant, Timer};

// The sEMMC carve — generated by build.rs's `contract` module (the single definition site: it also
// derives the carved `memory.x` RAM shrink from these values and asserts them against the vendored
// image's metadata header). Splices in `SEMMC_RAM_BASE`, `SEMMC_CODE_BYTES`, `SEMMC_VRI_OFFSET`,
// `SEMMC_VRI_BYTES`, `SEMMC_IMAGE_BYTES` and `SEMMC_CARVE_BYTES`.
include!(concat!(env!("OUT_DIR"), "/semmc_contract.rs"));

/// Nordic's sEMMC v0.1.1 image — position-independent RISC-V for the FLPR.
/// `LicenseRef-Nordic-5-Clause`; see `vendor/semmc/README.md` for provenance + regeneration.
static SEMMC_FW: &[u8] = include_bytes!("../vendor/semmc/semmc_firmware_v0.1.1.bin");

const _: () = assert!(SEMMC_IMAGE_BYTES <= SEMMC_CARVE_BYTES);
const _: () = assert!(SEMMC_RAM_BASE.is_multiple_of(4096), "the image base must stay 4 KiB aligned");

// ── VRI register offsets (nrfxlib `sEMMC/include/nrf_sp_emmc.h`, `NRF_SP_EMMC_Type`) ───────────
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
const VRI_CMD_CMD: usize = 0x44; // IDX | RESPTYPE << 16 | RESPPROC << 24
const VRI_CMD_ARG: usize = 0x48;
const VRI_CMD_RESPONSEADDR: usize = 0x4C;
const VRI_CMD_RESPONSE0: usize = 0x50; // [4] processed response words, **LSW first** (law 3)
const VRI_DATA_BUFFERADDR: usize = 0x64;
const VRI_DATA_BLOCKSIZE: usize = 0x68;
const VRI_DATA_BLOCKNUM: usize = 0x6C;
const VRI_STATUS: usize = 0x70;
const VRI_SPSYNC_AUX: usize = 0x74; // [6]; barrier: host counter in [0], firmware echo in [1]

/// `INTEN` mask: XFERCOMPLETE | ABORTED | READYTOTRANSFER → VEVIF event 20.
const VRI_INTEN_ALL: u32 = 0x7;

// `STATUS.STATUS` bit positions (same header) — the diagnosis carried out of an abort.
const STATUS_CMDTIMEOUT: u32 = 1 << 0;
const STATUS_CMDCRCERROR: u32 = 1 << 1;
const STATUS_DATACRCERROR: u32 = 1 << 2;
const STATUS_RETRYEXCEEDED: u32 = 1 << 3;
const STATUS_PROTOCOLERR: u32 = 1 << 4;

// Response types (`SP_EMMC_COMMAND_CMD_RESPTYPE_*`). SD's R6 and R7 are R1-shaped on the wire, so
// they ride `RESP_R1`.
const RESP_NONE: u32 = 0;
const RESP_R1: u32 = 1;
const RESP_R1B: u32 = 2;
const RESP_R2: u32 = 3;
const RESP_R3: u32 = 4;
// Response processing (`RESPPROC`). `IGNORE` exists because the firmware cannot process a response
// and a data phase at the same time — reads therefore ignore the response and ask again via CMD13.
const PROC_PROCESS: u32 = 0;
const PROC_IGNORE: u32 = 1;

// ── Soft-peripheral VPR task / event indices (`softperipheral_regif.h`, the nRF54L row) ────────
const T_START: usize = 16; // start a prepared transfer (the DPPI start task)
const T_CONFIG: usize = 17; // __CSB — configuration barrier
const T_ACTION: usize = 18; // __ASB — action barrier
const T_STOP: usize = 19; // __SSB — stop/abort barrier
const EV_COMPLETION: usize = 20; // VEVIF completion event → VPR00_IRQn

// ── VPR00 (secure alias). Same register block `ls021_flpr` launches the display blob through;
//    raw MMIO for the same reason (these are not in embassy-nrf's PAC surface). ──
const VPR00_TASKS_TRIGGER: *mut u32 = 0x5004_C000 as *mut u32;
const VPR00_EVENTS_TRIGGERED: *mut u32 = 0x5004_C100 as *mut u32;
const VPR00_INTENSET: *mut u32 = 0x5004_C304 as *mut u32;
const VPR00_INTENCLR: *mut u32 = 0x5004_C308 as *mut u32;
const VPR00_CPURUN: *mut u32 = 0x5004_C800 as *mut u32;
const VPR00_INITPC: *mut u32 = 0x5004_C808 as *mut u32;
const VPR00_DMCONTROL: *mut u32 = 0x5004_C440 as *mut u32;
const DM_DMACTIVE: u32 = 1 << 0;
const DM_NDMRESET: u32 = 1 << 1;

// ── Pins. The order is Nordic's, not sequential: see the pin map in issue #1158. ──
/// `(P2 pin, sEMMC role)` for the six card pads.
const SD_PADS: [(usize, &str); 6] = [(0, "D3"), (1, "CLK"), (2, "D0"), (3, "D2"), (4, "D1"), (5, "CMD")];
/// The two pads time-shared with the display's B0/B1 data lines — the only ones the display side
/// drives, and the only ones that need an internal pull-up in storage mode (this breakout carries
/// its own resistors on CLK/D0/D2/CMD; the production board fits external 10–100 kΩ pull-ups on
/// CMD/DAT0–3 and then runs *all* internal pulls off).
const SHARED_PADS: [usize; 2] = [0, 4];
/// The four pads only storage uses — parked as inputs in display mode, where the external
/// pull-ups hold the bus in its SD idle-high state.
const CARD_ONLY_PADS: [usize; 4] = [1, 2, 3, 5];

// ── Clocks. Legal = 128 MHz / an even divisor ≥ 4, so 32 / 21.3 / 16 MHz … and 400 kHz at 320. ──
/// SD card-identification clock (divisor 320 — the divider does reach it, verified).
const CLK_INIT_HZ: u32 = 400_000;
/// Default Speed data clock: what the card runs at before the High-Speed switch, and the write
/// clock afterwards.
const CLK_DS_HZ: u32 = 21_333_333;
/// High Speed read clock, after CMD6.
const CLK_HS_HZ: u32 = 32_000_000;
/// **Writes cap at 21.3 MHz.** 32 MHz writes fail card-side CRC on the jumper harness (a clean
/// failure — nothing is programmed) while 32 MHz reads are spotless, so reads ride the HS clock and
/// writes drop a rung. The clock is per-transaction config, which makes mixed-rate free. Re-test on
/// soldered hardware (#1158's on-glass checklist).
const CLK_WRITE_MAX_HZ: u32 = CLK_DS_HZ;

/// Retries the firmware makes per transaction. Zero only for the CMD6 drain read, where a retry
/// would hunt for a second start bit forever.
const NUM_RETRIES: u32 = 3;

// ── Deadlines. Every wait in this module is bounded by one of these; none of them is ever hit in
//    normal operation, so they are generous rather than tight — their job is to turn a wedge into a
//    reported error. ──
const BARRIER_DEADLINE: Duration = Duration::from_millis(50);
const BOOT_DEADLINE: Duration = Duration::from_millis(500);
const CMD_DEADLINE: Duration = Duration::from_millis(500);
const STATUS_DEADLINE: Duration = Duration::from_millis(250);
const READ_DEADLINE: Duration = Duration::from_millis(2000);
const WRITE_DEADLINE: Duration = Duration::from_millis(4000);
/// How long the card may stay in `prg` after a write before we call it stuck.
const PROGRAM_DEADLINE: Duration = Duration::from_millis(1000);
/// ACMD41 power-up poll window.
const POWERUP_DEADLINE: Duration = Duration::from_millis(1500);
/// How long the card's R7 gets to reach the wire in the CMD8 deliver-and-abort workaround.
const CMD8_DELIVER: Duration = Duration::from_micros(3000);
/// Card power-up settle before the first CMD0. The SD spec wants ≥1 ms of stable supply plus 74
/// clocks; the bringup bench used 100 ms and the mux bench 0 (its card had been powered for
/// minutes). 10 ms is an order of magnitude over the spec's floor and invisible next to boot —
/// this only ever runs once, at [`Semmc::start`], and is deliberately **absent** from the per-switch
/// [`Semmc::enter_storage_mode`] path, where 12/12 measured rounds show the card needs nothing.
const CARD_SETTLE: Duration = Duration::from_millis(10);
/// ACMD41 poll interval while the card powers up.
const POWERUP_POLL: Duration = Duration::from_millis(10);
/// Re-check granularity in [`Semmc::wait_completion`]: ~5 µs at the CK128 core clock. See that
/// function's note for why this is a bounded slice rather than a `WFE`.
const WAIT_SLICE_CYCLES: u32 = 640;

/// SD bytes per block. Fixed: the OBCM/FAT layers above assume it and SDHC/SDXC cards cannot be
/// told otherwise.
pub const BLOCK_BYTES: usize = 512;
/// `CURRENT_STATE` in an R1: the transfer state. Anything else means a transfer must not start.
pub const CARD_STATE_TRAN: u8 = 4;
/// `CURRENT_STATE`: the card is programming (the completion signal for a write).
const CARD_STATE_PRG: u8 = 7;
/// R1 **error** bits, per the SD Physical Layer spec's card-status table — derived per bit rather
/// than copied, because the difference between an error and a status flag is the difference between
/// a failed read and a working one.
///
/// ⚠️ **Deliberately stricter than the bench**, which only logged these: a driver that shrugs off an
/// address/CRC/ECC complaint is how corrupted map bytes reach the renderer. It is on #1158's
/// on-glass checklist to confirm no flag trips it during a long ride.
///
/// Deliberately **excluded** — these are informational, and failing a read on any of them would be
/// a bug: `CARD_ECC_DISABLED` (14), `ERASE_RESET` (13, set by any command that interrupts a queued
/// erase — routine), `AKE_SEQ_ERROR` (3, authentication sequence, which this driver never uses), and
/// `CARD_IS_LOCKED` (25, a state, not a fault).
const R1_ERROR_MASK: u32 = (1 << 31)  // OUT_OF_RANGE
    | (1 << 30)                       // ADDRESS_ERROR
    | (1 << 29)                       // BLOCK_LEN_ERROR
    | (1 << 28)                       // ERASE_SEQ_ERROR
    | (1 << 27)                       // ERASE_PARAM
    | (1 << 26)                       // WP_VIOLATION
    | (1 << 24)                       // LOCK_UNLOCK_FAILED
    | (1 << 23)                       // COM_CRC_ERROR
    | (1 << 22)                       // ILLEGAL_COMMAND
    | (1 << 21)                       // CARD_ECC_FAILED
    | (1 << 20)                       // CC_ERROR (internal controller error)
    | (1 << 19)                       // ERROR (generic, unknown)
    | (1 << 16)                       // CSD_OVERWRITE
    | (1 << 15); // WP_ERASE_SKIP

/// Set by [`on_vpr00_irq`]. The completion wait sleeps on this rather than spinning on the VRI —
/// but the VRI events stay the *authority* (see [`Semmc::wait_completion`]).
static COMPLETION: AtomicBool = AtomicBool::new(false);
/// One-shot latch so "no completion interrupt ever arrived" is diagnosed once, not per block.
static WARNED_NO_IRQ: AtomicBool = AtomicBool::new(false);

/// **The `VPR00_IRQn` handler body** — call this from the vector.
///
/// Kept as a plain function rather than an `#[interrupt] fn VPR00` because the integration PR owns
/// the vector table alongside the display side's `EGU20`; it declares
///
/// ```ignore
/// #[interrupt]
/// unsafe fn VPR00() {
///     semmc::on_vpr00_irq();
/// }
/// ```
///
/// and arms the NVIC line (`interrupt::VPR00.set_priority(Priority::P1); .enable()`) once, next to
/// the display's. The VRI-side gate (`INTEN`) and the VEVIF gate (`VPR00.INTENSET` bit 20) are
/// armed by [`Semmc::boot_firmware`] on every boot — **`INTENSET` writes are silently dropped while
/// the VPR core is stopped** (measured), which is exactly why they live there and not here.
///
/// The latched VEVIF event must be cleared or the level-triggered IRQ re-fires forever.
pub fn on_vpr00_irq() {
    // SAFETY: a fixed MMIO address in the VPR00 secure alias; no Rust object aliases it.
    unsafe { VPR00_EVENTS_TRIGGERED.add(EV_COMPLETION).write_volatile(0) };
    COMPLETION.store(true, Ordering::Release);
}

/// Why a storage operation failed. Every variant is *returned*, never panicked or hung on.
#[derive(Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum SemmcError {
    /// The firmware never echoed a barrier counter — it is wedged (or was never booted).
    Barrier,
    /// The image never stamped ready after a boot.
    NoBoot,
    /// No completion within the operation's deadline. The firmware has been recovered (warm
    /// reboot + power-on) before this returns, so the next command starts from a known state.
    Timeout,
    /// The firmware raised `EVENTS_ABORTED`; the payload is its `STATUS` register
    /// (`CMDTIMEOUT` / `CMDCRCERROR` / `DATACRCERROR` / `RETRYEXCEEDED` / `PROTOCOLERR`).
    Aborted(u32),
    /// The card answered, but its R1 carries error bits.
    CardStatus(u32),
    /// The card did not leave `prg` (or reach `tran`) within the deadline.
    CardBusy,
    /// Card identification did not complete — no card, an unpowered socket, or a broken bus.
    NoCard,
    /// The card is not an SDHC/SDXC (CSD version 2) card. SDSC is byte-addressed and caps at 2 GB;
    /// nothing this device stores fits on one, so it is rejected rather than half-supported.
    UnsupportedCard,
    /// The caller's buffer is not a whole number of 512 B blocks, is empty, or is not 32-bit
    /// aligned — the firmware's DMA requires all three.
    BadBuffer,
    /// The requested block span runs past the card's capacity.
    OutOfRange,
    /// A transfer was attempted before [`Semmc::init_card`] succeeded.
    NotInitialised,
}

impl SemmcError {
    /// Human-ish decode of an [`Aborted`](Self::Aborted) status word, for logging.
    pub fn abort_reason(status: u32) -> &'static str {
        if status & STATUS_CMDTIMEOUT != 0 {
            "command timeout (card silent)"
        } else if status & STATUS_CMDCRCERROR != 0 {
            "command CRC"
        } else if status & STATUS_DATACRCERROR != 0 {
            "data CRC (clock too high for the wiring?)"
        } else if status & STATUS_RETRYEXCEEDED != 0 {
            "retries exceeded"
        } else if status & STATUS_PROTOCOLERR != 0 {
            "protocol error"
        } else {
            "unclassified"
        }
    }
}

/// What card identification found out.
#[derive(Clone, Copy, defmt::Format)]
pub struct CardInfo {
    /// Relative card address, from CMD3.
    pub rca: u16,
    /// Capacity in 512 B blocks, from the CSD.
    pub blocks: u32,
    /// Whether the CMD6 High-Speed switch took (and the bus therefore reads at 32 MHz).
    pub high_speed: bool,
    /// The read clock the bus settled on.
    pub read_clk_hz: u32,
}

// ═════════════════════════════ VRI + VPR primitives ═════════════════════════════

const VRI_BASE: usize = SEMMC_RAM_BASE + SEMMC_VRI_OFFSET;

#[inline(always)]
fn vri_read(off: usize) -> u32 {
    // SAFETY: the VRI page is inside the carve — RAM the linker does not hand to the M33 and no
    // Rust object aliases. The firmware mutates it concurrently, hence volatile.
    unsafe { ((VRI_BASE + off) as *const u32).read_volatile() }
}

#[inline(always)]
fn vri_write(off: usize, v: u32) {
    // SAFETY: as above.
    unsafe { ((VRI_BASE + off) as *mut u32).write_volatile(v) }
}

/// Stop the FLPR hart, whatever it is doing.
///
/// `CPURUN = 0` alone does **not** stop a running VPR core (it only parks one that reaches a WFI,
/// which neither soft-peripheral image ever executes) — the pulsed `ndmreset` through the RISC-V
/// Debug Module is the actual guarantee.
///
/// This is Nordic's `nrf_semmc_uninit` recipe (`CPURUN = 0`, then `DMCONTROL` pulsed
/// `ndmreset|dmactive` → `dmactive` → 0), and it is **not** the same sequence as
/// `ls021_flpr::relaunch_flpr`, which additionally issues a `haltreq` and waits for `DMSTATUS.
/// allhalted` before resetting. Both stop the hart; the display side's is the more careful one
/// (a clean instruction boundary), this one is the 29 µs one the mode-switch numbers were measured
/// with. ⚠️ The integration PR owns the question of whether the scheduler should use one recipe for
/// both directions — they are equivalent for a park, but only one of them has been measured at
/// switch cadence.
pub fn park_hart() {
    // SAFETY: fixed VPR00 MMIO; parking the coprocessor cannot corrupt M33 state.
    unsafe {
        VPR00_CPURUN.write_volatile(0);
        VPR00_DMCONTROL.write_volatile(DM_NDMRESET | DM_DMACTIVE);
        VPR00_DMCONTROL.write_volatile(DM_DMACTIVE);
        VPR00_DMCONTROL.write_volatile(0);
    }
}

fn cfg_pad(pin: usize, dir: Dir, input: Input, pull: Pull, drive: Drive, ctrl: Ctrlsel) {
    pac::P2_S.pin_cnf(pin).modify(|w| {
        w.set_dir(dir);
        w.set_input(input);
        w.set_pull(pull);
        w.set_drive0(drive);
        w.set_drive1(drive);
        w.set_ctrlsel(ctrl);
    });
}

/// **Storage mode pads** (issue #1158's table): all six card pads become VPR-controlled outputs
/// with the input buffer disconnected and the extra-high E0/E1 drive 32 MHz needs, plus the
/// high-speed pad bias. Internal pull-ups go on D3/D1 only — the other four carry the breakout's
/// own resistors, and 13 kΩ ∥ 10 kΩ would sit under the SD spec's 10 kΩ floor.
pub fn configure_storage_pads() {
    for (pin, _) in SD_PADS {
        let pull = if SHARED_PADS.contains(&pin) { Pull::Pullup } else { Pull::Disabled };
        cfg_pad(pin, Dir::Output, Input::Disconnect, pull, Drive::E, Ctrlsel::Vpr);
    }
    // High-speed pad bias for 32 MHz (Nordic's porting guide: BIAS = 2).
    pac::GPIOHSPADCTRL_S.bias().modify(|w| w.set_hsbias(2));
}

/// **Display mode pads**: hand the two shared pads back to the display blob (which drives them as
/// B0/B1 through plain `OUTSET`/`OUTCLR`, so `CTRLSEL = GPIO` and the standard drive), and park the
/// four card-only pads as high-Z inputs — the external pull-ups then hold CLK/CMD/D0/D2 in the SD
/// idle-high state while the card is not being talked to. The card keeps its `tran` + High-Speed
/// state across the whole excursion (12/12 rounds measured); nothing here re-initialises it.
pub fn configure_display_pads() {
    for pin in SHARED_PADS {
        cfg_pad(pin, Dir::Output, Input::Disconnect, Pull::Disabled, Drive::S, Ctrlsel::Gpio);
    }
    for pin in CARD_ONLY_PADS {
        cfg_pad(pin, Dir::Input, Input::Disconnect, Pull::Disabled, Drive::S, Ctrlsel::Gpio);
    }
}

/// Response landing zone. Nordic documents four words; twice that is 32 B of `.bss` bought as
/// insurance against a firmware that writes past its own contract.
#[repr(C, align(4))]
struct RespRaw([u32; 8]);
static mut RESP_RAW: RespRaw = RespRaw([0; 8]);

/// A 32-bit-aligned single block — the shape every buffer handed to the firmware must have.
#[repr(C, align(4))]
struct AlignedBlock([u8; BLOCK_BYTES]);

// ═════════════════════════════════ the host driver ═════════════════════════════════

/// The sEMMC host. One instance owns the FLPR while it is in storage mode; the integration PR's
/// mode scheduler owns *when* that is.
pub struct Semmc {
    /// Clock the next command is configured with.
    clk_hz: u32,
    /// 1 or 4. Mirrors `CONFIG.BUSWIDTH`.
    bus_width: u32,
    /// Firmware data-sampling offset in FLPR clock cycles. 0 sufficed on the jumper harness at
    /// 32 MHz; a marginal harness may need 1–3 (sweep it against a known-good block). Exposed via
    /// [`set_read_delay`](Self::set_read_delay) rather than auto-tuned — a silent retune would hide
    /// a wiring fault the on-glass checklist wants to see.
    read_delay: u32,
    num_retries: u32,
    /// Barrier handshake counter (wraps; only equality with the firmware's echo matters).
    counter: u32,
    rca: u32,
    card: Option<CardInfo>,
    read_clk_hz: u32,
    write_clk_hz: u32,
}

impl Default for Semmc {
    fn default() -> Self {
        Self::new()
    }
}

impl Semmc {
    /// A host that has not booted the firmware yet.
    pub const fn new() -> Self {
        Self {
            clk_hz: CLK_INIT_HZ,
            bus_width: 1,
            read_delay: 0,
            num_retries: NUM_RETRIES,
            counter: 0,
            rca: 0,
            card: None,
            read_clk_hz: CLK_DS_HZ,
            write_clk_hz: CLK_WRITE_MAX_HZ,
        }
    }

    /// What [`init_card`](Self::init_card) found, once it has run.
    pub fn card(&self) -> Option<CardInfo> {
        self.card
    }

    /// Capacity in 512 B blocks — what `embedded_sdmmc::BlockDevice::num_blocks` reports.
    pub fn num_blocks(&self) -> Result<u32, SemmcError> {
        self.card.map(|c| c.blocks).ok_or(SemmcError::NotInitialised)
    }

    /// Override the firmware's data-sampling delay (default 0). See [`read_delay`](Self::read_delay).
    pub fn set_read_delay(&mut self, cycles: u32) {
        self.read_delay = cycles;
    }

    // ── boot / mode ──────────────────────────────────────────────────────────────────────────

    /// **The one-call bring-up**: pads → storage, cold-boot the image, power it on, let the card's
    /// supply settle, identify it. Run this once per power-on, with the FLPR already parked and the
    /// display side idle; every later handover is [`enter_storage_mode`](Self::enter_storage_mode) /
    /// [`leave_storage_mode`](Self::leave_storage_mode), which never re-identify the card.
    ///
    /// **`async` on purpose.** Card identification is the one storage path that waits in
    /// milliseconds rather than microseconds — the ACMD41 power-up poll alone is bounded at 1.5 s —
    /// and on the default build the panel's anti-DC-bias COM square wave is an M33 60 Hz task. A
    /// blocking bring-up would starve it for long enough to be a DC-bias hazard on the glass, so
    /// every long wait in here yields. The per-transfer paths stay synchronous: they are
    /// microsecond-scale and `embedded_sdmmc`'s `BlockDevice` is a sync trait.
    pub async fn start(&mut self) -> Result<CardInfo, SemmcError> {
        park_hart();
        configure_storage_pads();
        self.cold_boot()?;
        self.enable()?;
        Timer::after(CARD_SETTLE).await;
        self.init_card().await
    }

    /// **Cold boot**: copy the vendored image into the carve and start it. ~47 µs measured.
    fn cold_boot(&mut self) -> Result<(), SemmcError> {
        self.boot_firmware(true)
    }

    /// **Warm boot**: the image is already resident, so this is just park → `INITPC` → `CPURUN`
    /// → ready stamp. ~12 µs; the whole storage-ward mode switch (park + pads + this + power-on)
    /// measured 29 µs.
    fn warm_boot(&mut self) -> Result<(), SemmcError> {
        self.boot_firmware(false)
    }

    /// Take the FLPR for storage: pads → VPR, warm-boot the resident image, power it on.
    ///
    /// The caller must already have brought the display side to a safe point — never park mid-scan
    /// (await the EGU20 frame ack first, ≤44 ms bound). The card's own state survives untouched, so
    /// there is deliberately no re-initialisation here.
    pub fn enter_storage_mode(&mut self) -> Result<(), SemmcError> {
        park_hart();
        configure_storage_pads();
        self.warm_boot()?;
        self.enable()
    }

    /// Hand the FLPR back to the display: quiesce the peripheral, park the hart, flip the pads.
    ///
    /// Infallible by construction — every transfer path closes its own transaction before
    /// returning, so this only has to clear latched events and stop the core. The caller relaunches
    /// the display blob afterwards (`ls021_flpr::launch_flpr`).
    pub fn leave_storage_mode(&mut self) {
        // Plain VRI writes, no barrier: nothing is in flight (the transfer paths ack their own
        // completions), and the hart is about to be reset anyway.
        vri_write(VRI_CFG_READYTOTRANSFER, 0);
        vri_write(VRI_EV_XFERCOMPLETE, 0);
        vri_write(VRI_EV_ABORTED, 0);
        vri_write(VRI_EV_READYTOTRANSFER, 0);
        COMPLETION.store(false, Ordering::Relaxed);
        // Disarm our VEVIF gate and drop any latched event before the display blob owns the hart:
        // the two images share one VPR00 interrupt line, and a completion event left pending would
        // fire `on_vpr00_irq` against a peripheral that no longer exists. Re-armed by the next
        // `boot_firmware` — which is where it has to be, since `INTENSET` writes are dropped while
        // the core is stopped.
        // SAFETY: fixed VPR00 MMIO; `INTENCLR` is a clear-mask write.
        unsafe {
            VPR00_INTENCLR.write_volatile(1 << EV_COMPLETION);
            VPR00_EVENTS_TRIGGERED.add(EV_COMPLETION).write_volatile(0);
        }
        park_hart();
        configure_display_pads();
    }

    /// Boot (or re-boot) the firmware: park, optionally re-copy the image, zero the VRI, set
    /// `ENABLE`, point `INITPC` at the carve, run, and wait for the firmware to clear `ENABLE` —
    /// its ready stamp.
    ///
    /// One deliberate difference from the bench: the cold path zeroes the **whole**
    /// [`SEMMC_IMAGE_BYTES`], where the bench zeroed only the code region. The firmware clears its
    /// own `.bss`, so this is not load-bearing — but the exec/data window between the code region
    /// and the VRI was the one part of the carve nobody was clearing, and starting the coprocessor
    /// on RAM of known content costs one `memset` at boot and nothing thereafter.
    fn boot_firmware(&mut self, copy_image: bool) -> Result<(), SemmcError> {
        park_hart();
        if copy_image {
            // SAFETY: the carve is RAM outside the M33's linked region (build.rs shrinks `RAM` to
            // end at `SEMMC_RAM_BASE`), so nothing aliases it, and the hart is parked.
            unsafe {
                core::ptr::write_bytes(SEMMC_RAM_BASE as *mut u8, 0, SEMMC_IMAGE_BYTES);
                core::ptr::copy_nonoverlapping(SEMMC_FW.as_ptr(), SEMMC_RAM_BASE as *mut u8, SEMMC_FW.len());
            }
        }
        // SAFETY: as above — the VRI page, with the hart parked.
        unsafe { core::ptr::write_bytes(VRI_BASE as *mut u8, 0, SEMMC_VRI_BYTES) };
        vri_write(VRI_ENABLE, 1);
        // SAFETY: fixed VPR00 MMIO. The `dsb` publishes the image + VRI before the core is released.
        unsafe {
            cortex_m::asm::dsb();
            VPR00_INITPC.write_volatile(SEMMC_RAM_BASE as u32);
            VPR00_CPURUN.write_volatile(1);
        }
        let deadline = Instant::now() + BOOT_DEADLINE;
        while vri_read(VRI_ENABLE) != 0 {
            if Instant::now() >= deadline {
                warn!("sEMMC: firmware never stamped ready (ENABLE stayed 1)");
                return Err(SemmcError::NoBoot);
            }
        }
        vri_write(VRI_INTEN, VRI_INTEN_ALL);
        // Arm the VEVIF gate for the completion event. **Must** happen with the core RUNNING —
        // `INTENSET` writes are silently dropped while the VPR is stopped (measured 2026-08-05),
        // which is why this re-runs on every boot instead of once at init.
        // SAFETY: fixed VPR00 MMIO; `INTENSET` is a set-mask write, so it is idempotent.
        unsafe { VPR00_INTENSET.write_volatile(1 << EV_COMPLETION) };
        COMPLETION.store(false, Ordering::Relaxed);
        Ok(())
    }

    /// **Law 1** — `nrf_semmc_enable()`. After a boot the firmware is *initialised* but not
    /// *powered on*, and it ignores start triggers until `ENABLE = 1` is followed by an `__ASB`.
    ///
    /// Private, and the boot entry points are too: law 1 is only a law if it cannot be skipped, and
    /// a `pub cold_boot` without a `pub enable` beside it is an invitation to a dead bus. The two
    /// public sequences ([`start`](Self::start), [`enter_storage_mode`](Self::enter_storage_mode))
    /// both end here.
    fn enable(&mut self) -> Result<(), SemmcError> {
        vri_write(VRI_ENABLE, 1);
        let r = self.barrier(T_ACTION);
        vri_write(VRI_EV_XFERCOMPLETE, 0);
        vri_write(VRI_EV_ABORTED, 0);
        vri_write(VRI_EV_READYTOTRANSFER, 0);
        r
    }

    /// Wedge recovery: try a stop barrier, warm re-boot (the image stays resident), power back on;
    /// if even that fails, re-copy the image. The **card** is untouched — it keeps its RCA, bus
    /// width and speed mode, so the caller can simply retry the failed command.
    ///
    /// Stays synchronous even though [`BOOT_DEADLINE`] is 500 ms: this is the rare error path, it
    /// is reached from sync transfer code, and the normal cost is the ~600 µs a warm reboot
    /// actually takes — only a firmware that will not boot at all spends the deadline, and at that
    /// point storage is gone regardless.
    fn recover(&mut self) {
        let _ = self.barrier(T_STOP);
        if self.boot_firmware(false).is_err() {
            warn!("sEMMC: warm re-boot failed — re-copying the image");
            if self.boot_firmware(true).is_err() {
                warn!("sEMMC: firmware will not boot at all");
                return;
            }
        }
        let _ = self.enable();
    }

    // ── the barrier + one command ────────────────────────────────────────────────────────────

    /// A barrier on the command path, with the recovery a failed one has earned.
    ///
    /// [`SemmcError::Barrier`] *is* "the firmware is wedged" — leaving it unrecovered kills storage
    /// for the rest of the session, and the second barrier in [`cmd_start`] can additionally fail
    /// with `CONFIG.READYTOTRANSFER` already set and no start trigger issued: the law-2 poison state
    /// where the firmware considers a transaction open forever. A warm reboot costs ~600 µs against
    /// a 50 ms barrier timeout, so it is always the right trade. Used by every barrier on the
    /// command path; [`recover`](Self::recover) itself uses the raw [`barrier`](Self::barrier) so
    /// this cannot recurse.
    fn barrier_or_recover(&mut self, task: usize) -> Result<(), SemmcError> {
        match self.barrier(task) {
            Ok(()) => Ok(()),
            Err(e) => {
                warn!("sEMMC: barrier timeout — warm-rebooting the firmware");
                self.recover();
                Err(e)
            }
        }
    }

    /// One `__XSBx` barrier: publish the counter into `SPSYNC.AUX[0]`, trigger the task, wait for
    /// the firmware's echo in `AUX[1]`. ~2.2 µs; the deadline only ever fires on a wedge.
    fn barrier(&mut self, task: usize) -> Result<(), SemmcError> {
        self.counter = self.counter.wrapping_add(1);
        vri_write(VRI_SPSYNC_AUX, self.counter);
        // SAFETY: fixed VPR00 MMIO; the task index is one of the four soft-peripheral constants.
        unsafe { VPR00_TASKS_TRIGGER.add(task).write_volatile(1) };
        let deadline = Instant::now() + BARRIER_DEADLINE;
        while vri_read(VRI_SPSYNC_AUX) != vri_read(VRI_SPSYNC_AUX + 4) {
            if Instant::now() >= deadline {
                return Err(SemmcError::Barrier);
            }
        }
        Ok(())
    }

    /// **Law 2** — close the transaction: `CONFIG.READYTOTRANSFER = 0` **plus** the `__ASB` ack.
    /// Without the barrier the firmware still considers the old transaction open and ignores the
    /// next start trigger (the measured "first command works, second times out" alternation).
    fn close_transaction(&mut self) -> Result<(), SemmcError> {
        vri_write(VRI_CFG_READYTOTRANSFER, 0);
        self.barrier_or_recover(T_ACTION)
    }

    /// The front half of `nrf_semmc_cmd()`: fill CONFIG + COMMAND + DATA, `__CSB`, arm
    /// `READYTOTRANSFER`, `__ASB`, trigger the start task.
    fn cmd_start(
        &mut self,
        idx: u32,
        arg: u32,
        resp: u32,
        proc: u32,
        data: Option<(u32, u32, u32)>,
    ) -> Result<(), SemmcError> {
        vri_write(VRI_CFG_CLKFREQHZ, self.clk_hz);
        vri_write(VRI_CFG_BUSWIDTH, self.bus_width);
        vri_write(VRI_CFG_NUMRETRIES, self.num_retries);
        vri_write(VRI_CFG_READDELAY, self.read_delay);
        vri_write(VRI_CMD_CMD, (idx & 0xFFFF) | (resp << 16) | (proc << 24));
        vri_write(VRI_CMD_ARG, arg);
        vri_write(VRI_CMD_RESPONSEADDR, &raw const RESP_RAW as u32);
        let (buf, block_size, block_num) = data.unwrap_or((0, 0, 0));
        vri_write(VRI_DATA_BUFFERADDR, buf);
        vri_write(VRI_DATA_BLOCKSIZE, block_size);
        vri_write(VRI_DATA_BLOCKNUM, block_num);
        self.barrier_or_recover(T_CONFIG)?;
        vri_write(VRI_CFG_READYTOTRANSFER, 1);
        self.barrier_or_recover(T_ACTION)?;
        COMPLETION.store(false, Ordering::Relaxed);
        // SAFETY: fixed VPR00 MMIO.
        unsafe { VPR00_TASKS_TRIGGER.add(T_START).write_volatile(1) };
        Ok(())
    }

    /// Wait for the transfer to end, and close the transaction (law 2) either way.
    ///
    /// **Bounded polling with an interrupt fast path, not a sleep.** The obvious shape here is
    /// `WFE` woken by [`on_vpr00_irq`], and that was the first cut — but it is not sound: WFE only
    /// returns when the event register is set, and nothing in this system guarantees that ever
    /// happens. Embassy's cortex-m platform does not set `SCB.SCR.SEVONPEND`, the GRTC time driver
    /// only interrupts when a timer is actually scheduled, and the two cases this wait exists to
    /// survive — an unbound `VPR00` vector (which is the state until the integration PR binds it)
    /// and a firmware that wedged mid-transfer and will never raise its completion event — are
    /// exactly the cases where no interrupt arrives at all. The deadline below would then never be
    /// evaluated and the "every wait is bounded" promise would be a lie.
    ///
    /// So the loop re-checks on a fixed [`WAIT_SLICE_CYCLES`] slice, which makes the deadline
    /// reachable with **zero** interrupts arriving. The tradeoff, stated plainly: the core spins
    /// for the duration of a transfer instead of idling. That is the same profile the SPI transport
    /// it replaces had (a blocking `Spim` transfer held the CPU too), and the transfers are ~30×
    /// shorter now. The completion ISR still matters — it is what lets the slice be a *polling*
    /// granularity rather than the mechanism, and [`COMPLETION`] short-circuits the VRI read — but
    /// correctness no longer depends on it firing. A completion that arrives with no interrupt at
    /// all is diagnosed once, loudly, because that is a wiring-up bug and not a card fault.
    fn wait_completion(&mut self, timeout: Duration) -> Result<(), SemmcError> {
        let deadline = Instant::now() + timeout;
        loop {
            let signalled = COMPLETION.swap(false, Ordering::Acquire);
            if let Some(outcome) = self.take_completion() {
                // The second load closes the race where the interrupt lands between the swap and
                // the event read — otherwise a perfectly healthy vector could earn the warning.
                if !signalled && !COMPLETION.load(Ordering::Relaxed) && !WARNED_NO_IRQ.swap(true, Ordering::Relaxed) {
                    warn!("sEMMC: completion seen without a VPR00 interrupt — is the vector bound?");
                }
                return outcome;
            }
            if Instant::now() >= deadline {
                vri_write(VRI_CFG_READYTOTRANSFER, 0);
                self.recover();
                return Err(SemmcError::Timeout);
            }
            // A bounded slice, not a sleep — see the note above. It also keeps the M33 off the
            // SRAM bus the FLPR is DMA-ing across, which a tight re-read loop would not.
            cortex_m::asm::delay(WAIT_SLICE_CYCLES);
        }
    }

    /// Read the VRI completion events. `Some` once the transfer has ended, with the transaction
    /// already closed.
    ///
    /// **Every raised event is serviced and cleared in one pass**, the shape of Nordic's
    /// `nrf_semmc_irq_handler`: clearing only the one we matched would leave a latched `ABORTED`
    /// behind to be mistaken for the *next* command's outcome. An abort wins over a completion when
    /// both are set — the transfer did not deliver what was asked for.
    fn take_completion(&mut self) -> Option<Result<(), SemmcError>> {
        let complete = vri_read(VRI_EV_XFERCOMPLETE) != 0;
        let aborted = vri_read(VRI_EV_ABORTED) != 0;
        if !complete && !aborted {
            return None;
        }
        let status = vri_read(VRI_STATUS);
        vri_write(VRI_EV_XFERCOMPLETE, 0);
        vri_write(VRI_EV_ABORTED, 0);
        vri_write(VRI_EV_READYTOTRANSFER, 0);
        // The FLPR's DMA writes into the caller's buffer are finished as of the completion event.
        // `dsb` orders this core's accesses behind the event reads above, and the compiler fence
        // stops LLVM hoisting a buffer load past them — the buffer is plain memory, so nothing else
        // would stop it. (There is no data cache to invalidate on this part.)
        cortex_m::asm::dsb();
        core::sync::atomic::compiler_fence(Ordering::Acquire);
        // Nordic's abort path closes the transaction the same way a completion does; without the
        // barrier the firmware never re-arms.
        let closed = self.close_transaction();
        if aborted {
            return Some(Err(SemmcError::Aborted(status)));
        }
        Some(closed)
    }

    /// Run one SD command to completion. `data = (buffer, block_size, block_count)`; the direction
    /// comes from the firmware's command-index table (17/18 read, 24/25 write — indexes SD and eMMC
    /// agree on). Returns the four response words, **LSW first** (law 3).
    fn cmd(
        &mut self,
        idx: u32,
        arg: u32,
        resp: u32,
        proc: u32,
        data: Option<(u32, u32, u32)>,
        timeout: Duration,
    ) -> Result<[u32; 4], SemmcError> {
        self.cmd_start(idx, arg, resp, proc, data)?;
        self.wait_completion(timeout)?;
        Ok([
            vri_read(VRI_CMD_RESPONSE0),
            vri_read(VRI_CMD_RESPONSE0 + 4),
            vri_read(VRI_CMD_RESPONSE0 + 8),
            vri_read(VRI_CMD_RESPONSE0 + 12),
        ])
    }

    /// An application command: CMD55 with the current RCA, then the ACMD itself.
    fn acmd(&mut self, idx: u32, arg: u32, resp: u32, timeout: Duration) -> Result<[u32; 4], SemmcError> {
        self.cmd(55, self.rca << 16, RESP_R1, PROC_PROCESS, None, timeout)?;
        self.cmd(idx, arg, resp, PROC_PROCESS, None, timeout)
    }

    /// CMD13 → the card's `CURRENT_STATE` (4 = `tran`). Also the read path's response fetch, since
    /// the firmware cannot process a response and a data phase at the same time.
    pub fn card_state(&mut self) -> Result<u8, SemmcError> {
        self.card_status().map(|(_, state)| state)
    }

    /// CMD13 → `(raw R1, state)`.
    fn card_status(&mut self) -> Result<(u32, u8), SemmcError> {
        let r = self.cmd(13, self.rca << 16, RESP_R1, PROC_PROCESS, None, STATUS_DEADLINE)?;
        Ok((r[0], ((r[0] >> 9) & 0xF) as u8))
    }

    // ── card identification ──────────────────────────────────────────────────────────────────

    /// **The CMD8 workaround.** Deliver `SEND_IF_COND`, give the card time to put its R7 on the
    /// wire, then abandon the host-side wait with Nordic's own abort barrier (`__SSB`) rather than
    /// a reboot. The R7 payload is never read — the card's side of CMD8 is complete the moment it
    /// answers, and that is what arms ACMD41's HCS handling.
    async fn cmd8_deliver_abort(&mut self) -> Result<(), SemmcError> {
        // A future blob with a fixed index table would simply complete this; try that first, so the
        // workaround starts looking like dead weight the day it becomes dead weight.
        if let Ok(r) = self.cmd(8, 0x1AA, RESP_R1, PROC_PROCESS, None, Duration::from_millis(100)) {
            if r[0] & 0xFFF == 0x1AA {
                return Ok(());
            }
        }
        self.cmd_start(8, 0x1AA, RESP_R1, PROC_PROCESS, None)?;
        Timer::after(CMD8_DELIVER).await;
        let stopped = self.barrier(T_STOP).is_ok();
        let mut acked = false;
        if stopped {
            let deadline = Instant::now() + BARRIER_DEADLINE;
            while Instant::now() < deadline {
                if vri_read(VRI_EV_ABORTED) != 0 || vri_read(VRI_EV_XFERCOMPLETE) != 0 {
                    acked = true;
                    break;
                }
            }
        }
        vri_write(VRI_EV_ABORTED, 0);
        vri_write(VRI_EV_XFERCOMPLETE, 0);
        if stopped && acked {
            // `close_transaction` recovers itself if its barrier times out.
            let _ = self.close_transaction();
        } else {
            // The firmware ignored the stop: a warm re-boot cleans up, and the card is unaffected.
            self.recover();
        }
        Ok(())
    }

    /// **The CMD6 workaround.** Ask for High Speed as the firmware expects (`SWITCH`, R1b, no data
    /// descriptor), then drain the orphaned 64 B status block with a CMD17 the card ignores.
    /// `byte16 & 0xF == 1` in what lands is the card's own confirmation that it switched.
    ///
    /// Returns whether the switch is confirmed. Failure here is not fatal — the bus simply stays at
    /// Default Speed.
    fn cmd6_high_speed(&mut self) -> Result<bool, SemmcError> {
        self.cmd(6, 0x80FF_FFF1, RESP_R1B, PROC_PROCESS, None, CMD_DEADLINE)?;

        let mut drain = AlignedBlock([0xEE; BLOCK_BYTES]);
        let addr = drain.0.as_mut_ptr() as u32;
        self.num_retries = 0; // a retry would hunt for a second start bit forever
        let data = Some((addr, BLOCK_BYTES as u32, 1));
        // Expected to end in an abort — the orphan is 64 B, not 512. The drain is the point.
        let _ = self.cmd(17, self.block_arg(0), RESP_R1, PROC_IGNORE, data, Duration::from_millis(300));
        self.num_retries = NUM_RETRIES;

        let landed = drain.0[..64].iter().any(|&b| b != 0xEE);
        let switched = !landed || drain.0[16] & 0xF == 1;

        // Let the card finish and come back to `tran` (CMD13 also clears the CMD17 illegal flag).
        //
        // ⚠️ **Tolerant on purpose — do not `?` this loop.** The drain read ends in an abort by
        // design, and it can also time out and warm-reboot the host; either way the CMD13 right
        // after it is the single most likely command in the whole driver to return an error. An
        // early return here would skip the CMD12 below and leave the card stranded in `data`, where
        // every later CMD17/CMD18 is illegal — and no amount of host-side recovery fixes a
        // *card*-side state. This is the bench's shape, and the bench was right.
        let mut in_tran = false;
        for _ in 0..4 {
            if let Ok(state) = self.card_state() {
                if state == CARD_STATE_TRAN {
                    in_tran = true;
                    break;
                }
            }
        }
        if !in_tran {
            // The rescue, and it must be unconditional: STOP_TRANSMISSION is what walks a card out
            // of `data` when the drain did not.
            let _ = self.cmd(12, 0, RESP_R1B, PROC_PROCESS, None, CMD_DEADLINE);
            return Ok(false);
        }
        Ok(switched)
    }

    /// Bring the card from power-on to ready: 4-bit, High Speed if it takes, 32 MHz reads.
    ///
    /// The full verified ladder — CMD0 ×2 @400 kHz → CMD8 deliver-and-abort → CMD55+ACMD41 →
    /// CMD2 → CMD3 → CMD9 → CMD7 → ACMD6 + `CONFIG.BUSWIDTH = 4` → CMD6 drain-read → 32 MHz,
    /// verified against a Default-Speed golden read of sector 0 before it is trusted.
    ///
    /// Runs **once per power-on**, not per mode switch: the card keeps its state across a park.
    /// `async` for the reason given on [`start`](Self::start) — the millisecond-scale waits in here
    /// must not starve the panel's COM task.
    async fn init_card(&mut self) -> Result<CardInfo, SemmcError> {
        self.card = None;
        self.rca = 0;
        self.bus_width = 1;
        self.clk_hz = CLK_INIT_HZ;
        self.num_retries = NUM_RETRIES;

        // CMD0 GO_IDLE_STATE, twice — the card may miss the first while its supply is still
        // settling, and it carries no response to tell us so.
        let mut idle = false;
        for _ in 0..2 {
            idle |= self.cmd(0, 0, RESP_NONE, PROC_PROCESS, None, CMD_DEADLINE).is_ok();
        }
        if !idle {
            return Err(SemmcError::NoCard);
        }

        self.cmd8_deliver_abort().await?;

        // ACMD41 until the card reports powered up. HCS asks for block addressing; 0xFF8000 is the
        // full 2.7–3.6 V window.
        let deadline = Instant::now() + POWERUP_DEADLINE;
        let ocr = loop {
            match self.acmd(41, 0x4030_0000 | 0x00FF_8000, RESP_R3, CMD_DEADLINE) {
                Ok(r) if r[0] & 0x8000_0000 != 0 => break r[0],
                Ok(_) => {}
                Err(e) => return Err(e),
            }
            if Instant::now() >= deadline {
                return Err(SemmcError::NoCard);
            }
            Timer::after(POWERUP_POLL).await; // a poll interval, and a yield for the COM task
        };
        if ocr & 0x4000_0000 == 0 {
            // CCS = 0: a byte-addressed SDSC card. Rejected — see `UnsupportedCard`.
            return Err(SemmcError::UnsupportedCard);
        }

        self.cmd(2, 0, RESP_R2, PROC_PROCESS, None, CMD_DEADLINE)?; // CID
        let r = self.cmd(3, 0, RESP_R1, PROC_PROCESS, None, CMD_DEADLINE)?; // RCA (R6)
        self.rca = (r[0] >> 16) & 0xFFFF;
        let csd = self.cmd(9, self.rca << 16, RESP_R2, PROC_PROCESS, None, CMD_DEADLINE)?;
        let blocks = csd_v2_blocks(&csd)?;
        self.cmd(7, self.rca << 16, RESP_R1B, PROC_PROCESS, None, CMD_DEADLINE)?; // select
        if self.card_state()? != CARD_STATE_TRAN {
            return Err(SemmcError::CardBusy);
        }

        // 4-bit: the card first (ACMD6 arg 2), then the host — in that order, or the next command
        // is spoken on a bus the two sides disagree about.
        self.acmd(6, 0b10, RESP_R1, CMD_DEADLINE)?;
        self.bus_width = 4;
        self.clk_hz = CLK_DS_HZ;
        self.read_clk_hz = CLK_DS_HZ;
        self.write_clk_hz = CLK_WRITE_MAX_HZ;

        // A Default-Speed golden read is what the High-Speed rung is checked against — 32 MHz that
        // returns different bytes is worse than 21.3 MHz that returns the right ones.
        let mut golden = AlignedBlock([0; BLOCK_BYTES]);
        self.read_one(0, &mut golden)?;

        let high_speed = self.cmd6_high_speed().unwrap_or(false);
        if high_speed {
            self.clk_hz = CLK_HS_HZ;
            let mut check = AlignedBlock([0; BLOCK_BYTES]);
            if self.read_one(0, &mut check).is_ok() && check.0 == golden.0 {
                self.read_clk_hz = CLK_HS_HZ;
            } else {
                warn!("sEMMC: High Speed accepted but 32 MHz reads are not stable — staying at 21.3 MHz");
                self.clk_hz = CLK_DS_HZ;
            }
        }

        let info = CardInfo {
            rca: self.rca as u16,
            blocks,
            high_speed: self.read_clk_hz == CLK_HS_HZ,
            read_clk_hz: self.read_clk_hz,
        };
        self.card = Some(info);
        Ok(info)
    }

    // ── transfers ────────────────────────────────────────────────────────────────────────────

    /// Block address for `lba`. SDHC/SDXC only (rejected otherwise), so this is the identity —
    /// kept as a named step because the byte-addressed alternative is what every SD driver bug
    /// about "reads land 512× too far in" comes from.
    fn block_arg(&self, lba: u32) -> u32 {
        lba
    }

    /// Best-effort STOP_TRANSMISSION after a failed transfer.
    ///
    /// Needed on **every** failed data command, not only the multi-block ones: the recovery path
    /// resets the *host*, and a card that was mid-block when we walked away sits in `data`/`rcv`
    /// waiting for clocks that are not coming. The card is the one piece of state a warm reboot
    /// cannot repair, so it gets told to stop even when we are already returning an error.
    fn stop_transmission(&mut self) {
        let _ = self.cmd(12, 0, RESP_R1B, PROC_PROCESS, None, CMD_DEADLINE);
    }

    fn read_one(&mut self, lba: u32, block: &mut AlignedBlock) -> Result<(), SemmcError> {
        let addr = block.0.as_mut_ptr() as u32;
        let data = Some((addr, BLOCK_BYTES as u32, 1));
        if let Err(e) = self.cmd(17, self.block_arg(lba), RESP_R1, PROC_IGNORE, data, READ_DEADLINE) {
            self.stop_transmission();
            return Err(e);
        }
        self.check_after_transfer()
    }

    /// **Read `buf.len() / 512` blocks starting at `lba`.**
    ///
    /// CMD17 for one block, CMD18 + CMD12 for more. Reads run with `PROC_IGNORE` and a following
    /// CMD13 — the firmware cannot process a response and a data phase at the same time — so the
    /// card's R1 for the transfer is fetched afterwards and its error bits surfaced as
    /// [`SemmcError::CardStatus`].
    ///
    /// `buf` must be a non-empty whole number of 512 B blocks and **32-bit aligned** (the
    /// firmware's DMA requirement); otherwise [`SemmcError::BadBuffer`]. ⚠️ Note for the
    /// integration PR: `embedded_sdmmc::Block` has no alignment attribute, so the `BlockDevice`
    /// impl must either align it in the fork we already carry or bounce through an aligned buffer.
    ///
    /// Blocking, and it holds the core while the transfer runs — the same profile the SPI transport
    /// had, and what `embedded_sdmmc`'s synchronous `BlockDevice` needs. See
    /// [`wait_completion`](Self::wait_completion) for why the wait is a bounded poll with an
    /// interrupt fast path rather than a sleep.
    pub fn read_blocks(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), SemmcError> {
        let n = self.check_request(buf.as_ptr() as usize, buf.len(), lba)?;
        self.clk_hz = self.read_clk_hz;
        let data = Some((buf.as_mut_ptr() as u32, BLOCK_BYTES as u32, n));
        if n == 1 {
            if let Err(e) = self.cmd(17, self.block_arg(lba), RESP_R1, PROC_IGNORE, data, READ_DEADLINE) {
                self.stop_transmission();
                return Err(e);
            }
        } else {
            // A failed CMD18 leaves the **card** streaming — the timeout path recovers the host
            // (warm reboot), not the card — so STOP_TRANSMISSION goes out either way, or every
            // later command talks to a card stuck in `data`.
            let r = self.cmd(18, self.block_arg(lba), RESP_R1, PROC_IGNORE, data, READ_DEADLINE);
            let stop = self.cmd(12, 0, RESP_R1B, PROC_PROCESS, None, CMD_DEADLINE);
            r?;
            stop?;
        }
        self.check_after_transfer()
    }

    /// **Write `buf.len() / 512` blocks starting at `lba`.**
    ///
    /// CMD24 for one block, CMD25 + CMD12 for more, then CMD13 until the card leaves `prg` — the
    /// program cycle *is* the completion signal for a write. Runs at [`CLK_WRITE_MAX_HZ`]
    /// regardless of the read clock (see that constant).
    ///
    /// Same buffer rules as [`read_blocks`](Self::read_blocks).
    pub fn write_blocks(&mut self, lba: u32, buf: &[u8]) -> Result<(), SemmcError> {
        let n = self.check_request(buf.as_ptr() as usize, buf.len(), lba)?;
        self.clk_hz = self.write_clk_hz;
        let data = Some((buf.as_ptr() as u32, BLOCK_BYTES as u32, n));
        if n == 1 {
            if let Err(e) = self.cmd(24, self.block_arg(lba), RESP_R1, PROC_PROCESS, data, WRITE_DEADLINE) {
                self.stop_transmission();
                return Err(e);
            }
        } else {
            // Same reasoning as the read path: the card must be told to stop even when the
            // multi-block write failed, or it stays in `rcv` waiting for data that never comes.
            let r = self.cmd(25, self.block_arg(lba), RESP_R1, PROC_PROCESS, data, WRITE_DEADLINE);
            let stop = self.cmd(12, 0, RESP_R1B, PROC_PROCESS, None, CMD_DEADLINE);
            r?;
            stop?;
        }
        let deadline = Instant::now() + PROGRAM_DEADLINE;
        loop {
            let (r1, state) = self.card_status()?;
            if state == CARD_STATE_TRAN {
                return check_r1(r1);
            }
            if state != CARD_STATE_PRG && r1 & R1_ERROR_MASK != 0 {
                return Err(SemmcError::CardStatus(r1));
            }
            if Instant::now() >= deadline {
                return Err(SemmcError::CardBusy);
            }
        }
    }

    /// The card's own verdict on the transfer that just ran (reads ignore the in-band response).
    fn check_after_transfer(&mut self) -> Result<(), SemmcError> {
        let (r1, _) = self.card_status()?;
        check_r1(r1)
    }

    /// Validate a caller's buffer against the firmware's DMA rules and the span against the card's
    /// capacity, returning the block count.
    ///
    /// The span check is ours rather than the card's `OUT_OF_RANGE`: a run that walks off the end
    /// should be refused before it is clocked onto the bus, not diagnosed from an error bit after a
    /// partial transfer has already landed in the caller's buffer.
    fn check_request(&self, addr: usize, len: usize, lba: u32) -> Result<u32, SemmcError> {
        let card = self.card.ok_or(SemmcError::NotInitialised)?;
        if len == 0 || !len.is_multiple_of(BLOCK_BYTES) || !addr.is_multiple_of(4) {
            return Err(SemmcError::BadBuffer);
        }
        let n = (len / BLOCK_BYTES) as u32;
        if u64::from(lba) + u64::from(n) > u64::from(card.blocks) {
            return Err(SemmcError::OutOfRange);
        }
        Ok(n)
    }
}

/// Fail on any error bit the card reports in an R1.
fn check_r1(r1: u32) -> Result<(), SemmcError> {
    if r1 & R1_ERROR_MASK != 0 {
        Err(SemmcError::CardStatus(r1))
    } else {
        Ok(())
    }
}

/// Capacity in 512 B blocks from a CSD **version 2** (SDHC/SDXC). Words arrive LSW-first (law 3),
/// so `csd[3]` holds bits 127:96 and `CSD_STRUCTURE` is its top two bits; `C_SIZE` (bits 69:48)
/// straddles `csd[1]`'s high half and `csd[2]`'s low six bits, and the capacity is
/// `(C_SIZE + 1) × 512 KiB`.
fn csd_v2_blocks(csd: &[u32; 4]) -> Result<u32, SemmcError> {
    if csd[3] >> 30 != 1 {
        return Err(SemmcError::UnsupportedCard);
    }
    let c_size = ((csd[2] & 0x3F) << 16) | (csd[1] >> 16);
    // 1024 blocks per 512 KiB unit. `c_size` is 22 bits, so the product reaches 2^32 exactly at the
    // all-ones value — one block past what a u32 block count can express, and a capacity no SD card
    // has (it is the 2 TiB SDUC ceiling). `saturating_mul` clamps that one unreachable case to
    // `u32::MAX` rather than wrapping to zero; everything a real card reports is exact.
    Ok(c_size.saturating_add(1).saturating_mul(1024))
}
