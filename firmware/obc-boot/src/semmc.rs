//! Raw-block SD access for the install engine — **the sEMMC soft peripheral, bootloader shape**
//! (epic #1158; replaces the deleted SD-SPI transport, whose pins are the display's now).
//!
//! The nRF54L has no SD host controller; Nordic ships one as a position-independent RISC-V image
//! the FLPR executes. The app carries that image in its flash — this crate cannot (the 13.6 KB
//! blob alone would blow the 32 KB budget), and it must not read it out of the app slot it is
//! about to rewrite. Instead the **armer staged it into the `SEMMC_STAGE` RRAM carve**
//! (`OBCU_Spec.md` §3) before writing `Armed`, and [`staged_blob`] validates it — the CRC frame
//! plus the image's own metadata header, through the shared, host-tested
//! `obc_dfu::blobstage` — before a single FLPR register is touched. The VRI base comes from that
//! validated metadata, never from a hard-coded offset.
//!
//! This is a deliberate **port, not a copy**, of the app's driver (`../obc-fw-nrf54l/src/semmc.rs`
//! — read its module doc first: the three laws, the barrier, and the CMD8 wart are all inherited
//! verbatim and are not re-explained here). The differences are all subtractions, and each is a
//! bootloader fact:
//!
//! - **No `embassy_time`** — deadlines are DWT `CYCCNT` arithmetic at the 64 MHz boot clock (the
//!   same counter `com.rs` paces the COM wave with), and the settle waits are cycle-counted
//!   busy-waits. Main gates card construction on the cycle counter actually running, so every
//!   wait here stays genuinely bounded.
//! - **No interrupt at all** — the app's `wait_completion` is already a bounded poll that treats
//!   the VPR00 IRQ as diagnostic-only; here the vector is simply never bound and the poll is the
//!   whole story.
//! - **No mode scheduler** — the display blob never runs while this crate owns the machine
//!   (`com.rs` keeps the glass alive from the M33), so the FLPR is taken once per boot and handed
//!   back (parked, pads reset) just before the jump. The card-pad configuration is otherwise the
//!   app's, measured shape: `CTRLSEL = VPR`, E-drive, pulls on D3/D1 only, `HSBIAS = 2`.
//! - **Reads only, Default Speed only** — no write path (the engine only reads the card), and no
//!   CMD6 High-Speed switch, which drops the drain-read workaround and its rescue ladder wholesale.
//!   21.3 MHz 4-bit is ~8× the SPI transport this replaces; the install is RRAM-write-bound anyway.
//! - **No CMD9/capacity** — extents were resolved by the armer on this very card, and a garbage
//!   extent past the end comes back as the card's own `OUT_OF_RANGE` R1 error bit, which fails the
//!   read like any other; a range pre-check would spend bytes to convert one typed error into
//!   another.
//!
//! One genuine addition: [`BootSemmc::read_blocks`] accepts **unaligned** output slices by
//! bouncing through an aligned block, because the engine's `ExtentStream` hands out mid-buffer
//! slices the firmware's 32-bit-aligned DMA cannot take directly. The aligned fast path is the
//! common case.
//!
//! Clock note: the boot core runs at the reset 64 MHz, not the app's 128 MHz. If the firmware
//! derives its bus dividers from an assumed 128 MHz core clock, every rate below lands at half the
//! requested value — 400 kHz → 200 kHz identification, 21.3 → 10.7 MHz data, both squarely legal
//! (the SD spec's identification window is 100–400 kHz) — and if it reads its real clock they land
//! exactly. Either way correct; the install is seconds long regardless.

use embassy_nrf::pac;
use embassy_nrf::pac::gpio::vals::{Ctrlsel, Dir, Drive, Input, Pull};
use obc_dfu::blobstage::{sp_geometry, validate_stage, SpImageGeometry};

/// The RAM execution carve, mirroring the app's `build.rs` contract (`SEMMC_CARVE_BYTES` — the
/// stage carve in flash is sized to match, pinned by `obc_dfu::STAGE_LEN`).
const SEMMC_RAM_CARVE: usize = 20_480;

/// Base of the flash stage carve (`__semmc_stage_base`, `memory.x`) — the armer's handoff.
fn stage_base() -> usize {
    extern "C" {
        static __semmc_stage_base: u8;
    }
    core::ptr::addr_of!(__semmc_stage_base) as usize
}

/// Base of the RAM execution carve (`__semmc_ram_base`, `memory.x`) — one past this crate's RAM.
fn ram_base() -> usize {
    extern "C" {
        static __semmc_ram_base: u8;
    }
    core::ptr::addr_of!(__semmc_ram_base) as usize
}

// ── VRI register offsets (nrfxlib `sEMMC/include/nrf_sp_emmc.h`) — the app driver's constants. ──
const VRI_EV_XFERCOMPLETE: usize = 0x10;
const VRI_EV_ABORTED: usize = 0x14;
const VRI_EV_READYTOTRANSFER: usize = 0x18;
const VRI_ENABLE: usize = 0x2C;
const VRI_CFG_READYTOTRANSFER: usize = 0x30;
const VRI_CFG_CLKFREQHZ: usize = 0x34;
const VRI_CFG_BUSWIDTH: usize = 0x38;
const VRI_CFG_NUMRETRIES: usize = 0x3C;
const VRI_CFG_READDELAY: usize = 0x40;
const VRI_CMD_CMD: usize = 0x44;
const VRI_CMD_ARG: usize = 0x48;
const VRI_CMD_RESPONSEADDR: usize = 0x4C;
const VRI_CMD_RESPONSE0: usize = 0x50; // [4] processed response words, LSW first (law 3)
const VRI_DATA_BUFFERADDR: usize = 0x64;
const VRI_DATA_BLOCKSIZE: usize = 0x68;
const VRI_DATA_BLOCKNUM: usize = 0x6C;
const VRI_STATUS: usize = 0x70;
const VRI_SPSYNC_AUX: usize = 0x74; // [6]; barrier: host counter in [0], firmware echo in [1]

// Response types / processing (`SP_EMMC_COMMAND_CMD_*`).
const RESP_NONE: u32 = 0;
const RESP_R1: u32 = 1;
const RESP_R1B: u32 = 2;
const RESP_R2: u32 = 3;
const RESP_R3: u32 = 4;
const PROC_PROCESS: u32 = 0;
const PROC_IGNORE: u32 = 1;

// ── Soft-peripheral VPR task indices (`softperipheral_regif.h`, the nRF54L row). ──
const T_START: usize = 16;
const T_CONFIG: usize = 17; // __CSB
const T_ACTION: usize = 18; // __ASB
const T_STOP: usize = 19; // __SSB

// ── VPR00 (secure alias) — raw MMIO, same addresses as the app driver. ──
const VPR00_TASKS_TRIGGER: *mut u32 = 0x5004_C000 as *mut u32;
const VPR00_CPURUN: *mut u32 = 0x5004_C800 as *mut u32;
const VPR00_INITPC: *mut u32 = 0x5004_C808 as *mut u32;
const VPR00_DMCONTROL: *mut u32 = 0x5004_C440 as *mut u32;
const DM_DMACTIVE: u32 = 1 << 0;
const DM_NDMRESET: u32 = 1 << 1;

// ── Pins (issue #1158's table; order is Nordic's). ──
/// The six card pads on P2: `(pin, role)` — D3, CLK, D0, D2, D1, CMD.
const SD_PADS: [usize; 6] = [0, 1, 2, 3, 4, 5];
/// The two pads time-shared with the display's B0/B1 — internal pull-ups in storage mode.
const SHARED_PADS: [usize; 2] = [0, 4];

// ── Clocks. See the module doc's 64 MHz note — every value is legal at half rate too. ──
const CLK_INIT_HZ: u32 = 400_000;
/// Default Speed — the only data clock here (no CMD6 High-Speed switch in the bootloader).
const CLK_DS_HZ: u32 = 21_333_333;
/// Firmware retries per transaction (the app's number).
const NUM_RETRIES: u32 = 3;

// ── Deadlines, in milliseconds of the 64 MHz DWT clock. Generous on purpose: their job is to
//    turn a wedge into a reported error, and none is ever hit in normal operation. ──
const CYC_PER_MS: u32 = 64_000;
const BARRIER_MS: u32 = 50;
const BOOT_MS: u32 = 500;
const CMD_MS: u32 = 500;
const STATUS_MS: u32 = 250;
const READ_MS: u32 = 2_000;
const POWERUP_MS: u32 = 1_500;
/// Card power-up settle before the first CMD0 (the app's 10 ms).
const CARD_SETTLE_MS: u32 = 10;
/// ACMD41 poll interval.
const POWERUP_POLL_MS: u32 = 10;
/// CMD8 deliver-and-abort: how long the card's R7 gets to reach the wire.
const CMD8_DELIVER_US: u32 = 3_000;
/// Completion-poll re-check slice, ~5 µs at 64 MHz — keeps the M33 off the SRAM bus the FLPR is
/// DMA-ing across (the app driver's reasoning, halved for the halved clock).
const WAIT_SLICE_CYCLES: u32 = 320;

/// SD bytes per block.
pub const BLOCK_BYTES: usize = 512;
/// `CURRENT_STATE` in an R1: the transfer state.
const CARD_STATE_TRAN: u8 = 4;
/// R1 error bits — the app driver's mask, derivation comments and all (see it before editing).
const R1_ERROR_MASK: u32 = (1 << 31)
    | (1 << 30)
    | (1 << 29)
    | (1 << 28)
    | (1 << 27)
    | (1 << 26)
    | (1 << 24)
    | (1 << 23)
    | (1 << 22)
    | (1 << 21)
    | (1 << 20)
    | (1 << 19)
    | (1 << 16)
    | (1 << 15);

/// Why a storage operation failed. Returned, never panicked or hung on; the payloads exist for
/// the `rtt` diagnostics and cost a handful of bytes.
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "rtt", derive(defmt::Format))]
pub enum SemmcError {
    /// The firmware never echoed a barrier counter — wedged (or never booted).
    Barrier,
    /// The image never stamped ready after a boot.
    NoBoot,
    /// No completion within the deadline (the firmware has been warm-rebooted before this returns).
    Timeout,
    /// `EVENTS_ABORTED`, payload = the firmware's `STATUS` word.
    Aborted(u32),
    /// The card's R1 carries error bits.
    CardStatus(u32),
    /// The card did not reach `tran` when it had to.
    CardBusy,
    /// Card identification did not complete — no card, or a broken bus.
    NoCard,
    /// CCS = 0: a byte-addressed SDSC card (the armer-side stack rejects these too).
    UnsupportedCard,
}

// ── DWT-cycle deadlines. CYCCNT is enabled by main before any of this runs (and main refuses to
//    build the card at all if the counter isn't ticking, so "bounded" stays true). ──

#[inline(always)]
fn now() -> u32 {
    cortex_m::peripheral::DWT::cycle_count()
}

/// A wrap-safe deadline: `expired` once `now` has advanced past the end mark. All deadlines here
/// are ≤ 2 s = 128 M cycles, far inside the i32 half-range the comparison needs.
struct Deadline(u32);

impl Deadline {
    fn after_ms(ms: u32) -> Deadline {
        Deadline(now().wrapping_add(ms.saturating_mul(CYC_PER_MS)))
    }
    fn expired(&self) -> bool {
        (now().wrapping_sub(self.0) as i32) >= 0
    }
}

fn delay_us(us: u32) {
    // 64 cycles/µs at the boot clock; +1 so a nonzero request never rounds to zero.
    cortex_m::asm::delay(us.saturating_mul(64) + 1);
}

fn delay_ms(ms: u32) {
    delay_us(ms.saturating_mul(1_000));
}

/// Validate the armer-staged blob (`OBCU_Spec.md` §3.4): the CRC frame, then the image's own
/// metadata — magic, header version, REGIF, not self-boot, the sEMMC id, and that it fits the
/// execution carve. `None` = the carve does not hold a bootable sEMMC image; the caller owns what
/// that means per decision (abandon an untouched `Armed`, park a `Rollback`).
pub fn staged_blob() -> Option<(&'static [u8], SpImageGeometry)> {
    // SAFETY: the linker reserves the SEMMC_STAGE region; RRAM is memory-mapped, always readable,
    // and nothing writes it while the bootloader runs.
    let carve = unsafe { core::slice::from_raw_parts(stage_base() as *const u8, obc_dfu::STAGE_LEN) };
    let blob = validate_stage(carve)?;
    let geom = sp_geometry(blob, SEMMC_RAM_CARVE)?;
    Some((blob, geom))
}

// ═════════════════════════════ the bootloader's card handle ═════════════════════════════

/// The sEMMC host, bootloader shape: one instance owns the FLPR from construction to
/// [`shutdown`](Self::shutdown). Init + absolute block reads, nothing else.
pub struct BootSemmc {
    /// The staged image (validated flash bytes — `recover`'s re-copy source, always present).
    blob: &'static [u8],
    /// VRI base = carve base + the *staged metadata's* VRI offset (never a pinned constant).
    vri_base: usize,
    /// Bytes to reserve + zero at a cold boot (code + exec/data + VRI, from the metadata).
    image_bytes: usize,
    clk_hz: u32,
    bus_width: u32,
    num_retries: u32,
    /// Barrier handshake counter (only equality with the firmware's echo matters).
    counter: u32,
    rca: u32,
    ready: bool,
}

impl BootSemmc {
    /// Build the handle from a [`staged_blob`] result. Touches nothing yet — call
    /// [`try_init`](Self::try_init) until it succeeds (the caller owns the retry/backoff policy,
    /// exactly as with the SPI transport this replaces).
    pub fn new(blob: &'static [u8], geom: SpImageGeometry) -> BootSemmc {
        BootSemmc {
            blob,
            vri_base: ram_base() + geom.vri_offset,
            image_bytes: geom.image_bytes,
            clk_hz: CLK_INIT_HZ,
            bus_width: 1,
            num_retries: NUM_RETRIES,
            counter: 0,
            rca: 0,
            ready: false,
        }
    }

    /// One full bring-up attempt: park the hart, pads → storage, cold-boot the staged image,
    /// power it on, settle, run card identification to 4-bit Default Speed, and probe-read block
    /// 0 so the whole data path is proven before the engine trusts it. `false` = no card / a
    /// failed step — retry later; every failure path has already parked or recovered the FLPR.
    pub fn try_init(&mut self) -> bool {
        self.ready = false;
        park_hart();
        configure_storage_pads();
        let ok = self.bring_up().is_ok();
        if !ok {
            // Leave the hart parked rather than half-running a peripheral we won't talk to.
            park_hart();
        }
        self.ready = ok;
        ok
    }

    fn bring_up(&mut self) -> Result<(), SemmcError> {
        self.boot_firmware(true)?;
        self.enable()?;
        delay_ms(CARD_SETTLE_MS);
        self.init_card()?;
        // The probe: one real block through the real path (also what `num_bytes` did for SPI).
        let mut probe = AlignedBlock([0; BLOCK_BYTES]);
        self.read_one(0, &mut probe)
    }

    /// Read `out.len() / 512` whole blocks starting at absolute block `start` — the engine's
    /// `read_blocks` (`out` is always a non-zero multiple of 512). CMD17/CMD18 straight into the
    /// caller's buffer when it is 32-bit aligned (the firmware's DMA requirement); the engine's
    /// mid-buffer slices bounce per-block through an aligned scratch.
    pub fn read_blocks(&mut self, start: u32, out: &mut [u8]) -> Result<(), SemmcError> {
        if !self.ready {
            return Err(SemmcError::NoCard);
        }
        if (out.as_ptr() as usize).is_multiple_of(4) {
            return self.read_span(start, out);
        }
        let mut scratch = AlignedBlock([0; BLOCK_BYTES]);
        for (i, chunk) in out.chunks_mut(BLOCK_BYTES).enumerate() {
            self.read_one(start + i as u32, &mut scratch)?;
            chunk.copy_from_slice(&scratch.0);
        }
        Ok(())
    }

    /// Hand the FLPR back before the jump: quiesce the VRI, park the hart, reset the card pads to
    /// their power-on shape. The app's own bring-up re-takes the coprocessor from scratch.
    pub fn shutdown(&mut self) {
        self.ready = false;
        vri_write(self.vri_base, VRI_CFG_READYTOTRANSFER, 0);
        vri_write(self.vri_base, VRI_EV_XFERCOMPLETE, 0);
        vri_write(self.vri_base, VRI_EV_ABORTED, 0);
        vri_write(self.vri_base, VRI_EV_READYTOTRANSFER, 0);
        park_hart();
        reset_pads();
    }

    // ── boot / recovery ──────────────────────────────────────────────────────────────────────

    /// Boot (or re-boot) the firmware: park, optionally re-copy the staged image into the carve,
    /// zero the VRI, `ENABLE`, `INITPC`, run, wait for the ready stamp (firmware clears `ENABLE`).
    fn boot_firmware(&mut self, copy_image: bool) -> Result<(), SemmcError> {
        park_hart();
        let base = ram_base();
        if copy_image {
            // SAFETY: the carve is RAM above this crate's linked `RAM` region (memory.x ends it
            // at `__semmc_ram_base`), nothing aliases it, and the hart is parked.
            unsafe {
                core::ptr::write_bytes(base as *mut u8, 0, self.image_bytes);
                core::ptr::copy_nonoverlapping(self.blob.as_ptr(), base as *mut u8, self.blob.len());
            }
        }
        let vri_bytes = self.image_bytes - (self.vri_base - base);
        // SAFETY: the VRI page is inside the carve, hart parked.
        unsafe { core::ptr::write_bytes(self.vri_base as *mut u8, 0, vri_bytes) };
        vri_write(self.vri_base, VRI_ENABLE, 1);
        // SAFETY: fixed VPR00 MMIO; the `dsb` publishes image + VRI before the core is released.
        unsafe {
            cortex_m::asm::dsb();
            VPR00_INITPC.write_volatile(base as u32);
            VPR00_CPURUN.write_volatile(1);
        }
        let deadline = Deadline::after_ms(BOOT_MS);
        while vri_read(self.vri_base, VRI_ENABLE) != 0 {
            if deadline.expired() {
                #[cfg(feature = "rtt")]
                defmt::warn!("obc-boot: sEMMC firmware never stamped ready");
                return Err(SemmcError::NoBoot);
            }
        }
        Ok(())
    }

    /// **Law 1** — after a boot the firmware is initialised, not powered on: `ENABLE = 1` + `__ASB`.
    fn enable(&mut self) -> Result<(), SemmcError> {
        vri_write(self.vri_base, VRI_ENABLE, 1);
        let r = self.barrier(T_ACTION);
        vri_write(self.vri_base, VRI_EV_XFERCOMPLETE, 0);
        vri_write(self.vri_base, VRI_EV_ABORTED, 0);
        vri_write(self.vri_base, VRI_EV_READYTOTRANSFER, 0);
        r
    }

    /// Wedge recovery: stop barrier, warm re-boot (image resident), power on; if even that fails,
    /// re-copy the image from the stage carve (flash — always present, unlike the app slot). The
    /// card is untouched and keeps its RCA/bus state, so the caller simply retries.
    fn recover(&mut self) {
        let _ = self.barrier(T_STOP);
        if self.boot_firmware(false).is_err() && self.boot_firmware(true).is_err() {
            #[cfg(feature = "rtt")]
            defmt::warn!("obc-boot: sEMMC firmware will not boot at all");
            return;
        }
        let _ = self.enable();
    }

    // ── the barrier + one command ────────────────────────────────────────────────────────────

    /// One `__XSBx` barrier (~2.2 µs): counter into `SPSYNC.AUX[0]`, trigger, spin for the echo.
    fn barrier(&mut self, task: usize) -> Result<(), SemmcError> {
        self.counter = self.counter.wrapping_add(1);
        vri_write(self.vri_base, VRI_SPSYNC_AUX, self.counter);
        // SAFETY: fixed VPR00 MMIO; the task index is one of the soft-peripheral constants.
        unsafe { VPR00_TASKS_TRIGGER.add(task).write_volatile(1) };
        let deadline = Deadline::after_ms(BARRIER_MS);
        while vri_read(self.vri_base, VRI_SPSYNC_AUX) != vri_read(self.vri_base, VRI_SPSYNC_AUX + 4) {
            if deadline.expired() {
                return Err(SemmcError::Barrier);
            }
        }
        Ok(())
    }

    /// A barrier on the command path, with the recovery a failed one has earned (a failed barrier
    /// = a wedged firmware; a warm reboot costs ~600 µs against a 50 ms timeout). `recover` uses
    /// the raw [`barrier`](Self::barrier) so this cannot recurse.
    fn barrier_or_recover(&mut self, task: usize) -> Result<(), SemmcError> {
        match self.barrier(task) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.recover();
                Err(e)
            }
        }
    }

    /// **Law 2** — close the transaction: `CONFIG.READYTOTRANSFER = 0` **plus** the `__ASB` ack.
    fn close_transaction(&mut self) -> Result<(), SemmcError> {
        vri_write(self.vri_base, VRI_CFG_READYTOTRANSFER, 0);
        self.barrier_or_recover(T_ACTION)
    }

    /// Front half of a command: CONFIG + COMMAND + DATA, `__CSB`, arm `READYTOTRANSFER`, `__ASB`,
    /// trigger the start task.
    fn cmd_start(
        &mut self,
        idx: u32,
        arg: u32,
        resp: u32,
        proc: u32,
        data: Option<(u32, u32, u32)>,
    ) -> Result<(), SemmcError> {
        let vri = self.vri_base;
        vri_write(vri, VRI_CFG_CLKFREQHZ, self.clk_hz);
        vri_write(vri, VRI_CFG_BUSWIDTH, self.bus_width);
        vri_write(vri, VRI_CFG_NUMRETRIES, self.num_retries);
        vri_write(vri, VRI_CFG_READDELAY, 0);
        vri_write(vri, VRI_CMD_CMD, (idx & 0xFFFF) | (resp << 16) | (proc << 24));
        vri_write(vri, VRI_CMD_ARG, arg);
        vri_write(vri, VRI_CMD_RESPONSEADDR, &raw const RESP_RAW as u32);
        let (buf, block_size, block_num) = data.unwrap_or((0, 0, 0));
        vri_write(vri, VRI_DATA_BUFFERADDR, buf);
        vri_write(vri, VRI_DATA_BLOCKSIZE, block_size);
        vri_write(vri, VRI_DATA_BLOCKNUM, block_num);
        self.barrier_or_recover(T_CONFIG)?;
        vri_write(vri, VRI_CFG_READYTOTRANSFER, 1);
        self.barrier_or_recover(T_ACTION)?;
        // SAFETY: fixed VPR00 MMIO.
        unsafe { VPR00_TASKS_TRIGGER.add(T_START).write_volatile(1) };
        Ok(())
    }

    /// Wait for the transfer to end and close the transaction (law 2) either way. A pure bounded
    /// poll — see the module doc; the interrupt fast path of the app's version does not exist here.
    fn wait_completion(&mut self, deadline_ms: u32) -> Result<(), SemmcError> {
        let deadline = Deadline::after_ms(deadline_ms);
        loop {
            let complete = vri_read(self.vri_base, VRI_EV_XFERCOMPLETE) != 0;
            let aborted = vri_read(self.vri_base, VRI_EV_ABORTED) != 0;
            if complete || aborted {
                let status = vri_read(self.vri_base, VRI_STATUS);
                vri_write(self.vri_base, VRI_EV_XFERCOMPLETE, 0);
                vri_write(self.vri_base, VRI_EV_ABORTED, 0);
                vri_write(self.vri_base, VRI_EV_READYTOTRANSFER, 0);
                // Order this core's later buffer reads behind the event reads (no D-cache here).
                cortex_m::asm::dsb();
                core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Acquire);
                let closed = self.close_transaction();
                if aborted {
                    // An abort wins over a completion — the transfer did not deliver.
                    return Err(SemmcError::Aborted(status));
                }
                return closed;
            }
            if deadline.expired() {
                vri_write(self.vri_base, VRI_CFG_READYTOTRANSFER, 0);
                self.recover();
                return Err(SemmcError::Timeout);
            }
            // A bounded slice keeps the M33 off the SRAM bus the FLPR is DMA-ing across.
            cortex_m::asm::delay(WAIT_SLICE_CYCLES);
        }
    }

    /// Run one command to completion; returns the four response words, LSW first (law 3).
    fn cmd(
        &mut self,
        idx: u32,
        arg: u32,
        resp: u32,
        proc: u32,
        data: Option<(u32, u32, u32)>,
        deadline_ms: u32,
    ) -> Result<[u32; 4], SemmcError> {
        self.cmd_start(idx, arg, resp, proc, data)?;
        self.wait_completion(deadline_ms)?;
        Ok([
            vri_read(self.vri_base, VRI_CMD_RESPONSE0),
            vri_read(self.vri_base, VRI_CMD_RESPONSE0 + 4),
            vri_read(self.vri_base, VRI_CMD_RESPONSE0 + 8),
            vri_read(self.vri_base, VRI_CMD_RESPONSE0 + 12),
        ])
    }

    /// CMD55 + the ACMD.
    fn acmd(&mut self, idx: u32, arg: u32, resp: u32) -> Result<[u32; 4], SemmcError> {
        self.cmd(55, self.rca << 16, RESP_R1, PROC_PROCESS, None, CMD_MS)?;
        self.cmd(idx, arg, resp, PROC_PROCESS, None, CMD_MS)
    }

    /// CMD13 → `(raw R1, CURRENT_STATE)` — also the read path's response fetch (the firmware
    /// cannot process a response and a data phase at once, so reads run `PROC_IGNORE` + this).
    fn card_status(&mut self) -> Result<(u32, u8), SemmcError> {
        let r = self.cmd(13, self.rca << 16, RESP_R1, PROC_PROCESS, None, STATUS_MS)?;
        Ok((r[0], ((r[0] >> 9) & 0xF) as u8))
    }

    // ── card identification ──────────────────────────────────────────────────────────────────

    /// **The CMD8 workaround** — the app driver's deliver-and-abort, verbatim (its doc has the
    /// story): send `SEND_IF_COND`, give the R7 3 ms to reach the wire (which is all ACMD41's HCS
    /// handling needs), abandon the host-side wait via `__SSB`, ack, continue.
    fn cmd8_deliver_abort(&mut self) -> Result<(), SemmcError> {
        // A future blob with a fixed index table would simply complete this; try that first.
        if let Ok(r) = self.cmd(8, 0x1AA, RESP_R1, PROC_PROCESS, None, 100) {
            if r[0] & 0xFFF == 0x1AA {
                return Ok(());
            }
        }
        self.cmd_start(8, 0x1AA, RESP_R1, PROC_PROCESS, None)?;
        delay_us(CMD8_DELIVER_US);
        let stopped = self.barrier(T_STOP).is_ok();
        let mut acked = false;
        if stopped {
            let deadline = Deadline::after_ms(BARRIER_MS);
            while !deadline.expired() {
                if vri_read(self.vri_base, VRI_EV_ABORTED) != 0 || vri_read(self.vri_base, VRI_EV_XFERCOMPLETE) != 0 {
                    acked = true;
                    break;
                }
            }
        }
        vri_write(self.vri_base, VRI_EV_ABORTED, 0);
        vri_write(self.vri_base, VRI_EV_XFERCOMPLETE, 0);
        if stopped && acked {
            let _ = self.close_transaction();
        } else {
            self.recover();
        }
        Ok(())
    }

    /// Power-on to ready: CMD0 ×2 → CMD8 → ACMD41(HCS) → CMD2 → CMD3 → CMD7 → ACMD6 4-bit,
    /// Default Speed. No CMD9, no CMD6 — see the module doc.
    fn init_card(&mut self) -> Result<(), SemmcError> {
        self.rca = 0;
        self.bus_width = 1;
        self.clk_hz = CLK_INIT_HZ;
        self.num_retries = NUM_RETRIES;

        // CMD0 twice — the card may miss the first while its supply settles (no response to say so).
        let mut idle = false;
        for _ in 0..2 {
            idle |= self.cmd(0, 0, RESP_NONE, PROC_PROCESS, None, CMD_MS).is_ok();
        }
        if !idle {
            return Err(SemmcError::NoCard);
        }

        self.cmd8_deliver_abort()?;

        // ACMD41 until powered up; HCS for block addressing, 0xFF8000 = the full voltage window.
        let deadline = Deadline::after_ms(POWERUP_MS);
        let ocr = loop {
            match self.acmd(41, 0x4030_0000 | 0x00FF_8000, RESP_R3) {
                Ok(r) if r[0] & 0x8000_0000 != 0 => break r[0],
                Ok(_) => {}
                Err(e) => return Err(e),
            }
            if deadline.expired() {
                return Err(SemmcError::NoCard);
            }
            delay_ms(POWERUP_POLL_MS);
        };
        if ocr & 0x4000_0000 == 0 {
            return Err(SemmcError::UnsupportedCard);
        }

        self.cmd(2, 0, RESP_R2, PROC_PROCESS, None, CMD_MS)?; // CID
        let r = self.cmd(3, 0, RESP_R1, PROC_PROCESS, None, CMD_MS)?; // RCA (R6)
        self.rca = (r[0] >> 16) & 0xFFFF;
        self.cmd(7, self.rca << 16, RESP_R1B, PROC_PROCESS, None, CMD_MS)?; // select
        if self.card_status()?.1 != CARD_STATE_TRAN {
            return Err(SemmcError::CardBusy);
        }

        // 4-bit: the card first (ACMD6 arg 2), then the host, in that order.
        self.acmd(6, 0b10, RESP_R1)?;
        self.bus_width = 4;
        self.clk_hz = CLK_DS_HZ;
        Ok(())
    }

    // ── transfers ────────────────────────────────────────────────────────────────────────────

    /// Best-effort STOP_TRANSMISSION after any failed data command — the host recovery path
    /// cannot repair a *card* left streaming in `data`.
    fn stop_transmission(&mut self) {
        let _ = self.cmd(12, 0, RESP_R1B, PROC_PROCESS, None, CMD_MS);
    }

    fn read_one(&mut self, lba: u32, block: &mut AlignedBlock) -> Result<(), SemmcError> {
        let data = Some((block.0.as_mut_ptr() as u32, BLOCK_BYTES as u32, 1));
        if let Err(e) = self.cmd(17, lba, RESP_R1, PROC_IGNORE, data, READ_MS) {
            self.stop_transmission();
            return Err(e);
        }
        self.check_after_transfer()
    }

    /// The aligned read path: CMD17 for one block, CMD18 + CMD12 for more.
    fn read_span(&mut self, lba: u32, out: &mut [u8]) -> Result<(), SemmcError> {
        let n = (out.len() / BLOCK_BYTES) as u32;
        let data = Some((out.as_mut_ptr() as u32, BLOCK_BYTES as u32, n));
        if n == 1 {
            if let Err(e) = self.cmd(17, lba, RESP_R1, PROC_IGNORE, data, READ_MS) {
                self.stop_transmission();
                return Err(e);
            }
        } else {
            // A failed CMD18 leaves the *card* streaming — the timeout path recovers the host,
            // not the card — so STOP_TRANSMISSION goes out either way.
            let r = self.cmd(18, lba, RESP_R1, PROC_IGNORE, data, READ_MS);
            let stop = self.cmd(12, 0, RESP_R1B, PROC_PROCESS, None, CMD_MS);
            r?;
            stop?;
        }
        self.check_after_transfer()
    }

    /// The card's own verdict on the transfer that just ran (reads ignore the in-band response).
    fn check_after_transfer(&mut self) -> Result<(), SemmcError> {
        let (r1, _) = self.card_status()?;
        if r1 & R1_ERROR_MASK != 0 {
            return Err(SemmcError::CardStatus(r1));
        }
        Ok(())
    }
}

// ── VRI + VPR primitives ──

#[inline(always)]
fn vri_read(base: usize, off: usize) -> u32 {
    // SAFETY: the VRI page is inside the carve — RAM the linker does not hand to the M33, no Rust
    // object aliases it, and the firmware mutates it concurrently (hence volatile).
    unsafe { ((base + off) as *const u32).read_volatile() }
}

#[inline(always)]
fn vri_write(base: usize, off: usize, v: u32) {
    // SAFETY: as above.
    unsafe { ((base + off) as *mut u32).write_volatile(v) }
}

/// Stop the FLPR hart whatever it is doing — `CPURUN = 0` alone does NOT stop a running VPR core;
/// the pulsed `ndmreset` through the Debug Module is the guarantee (Nordic's `nrf_semmc_uninit`).
fn park_hart() {
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

/// Storage-mode pads (#1158's measured table): all six card pads VPR-controlled, input
/// disconnected, E-drive; internal pull-ups on D3/D1 only (the other four carry external
/// resistors); the high-speed pad bias the app also sets.
fn configure_storage_pads() {
    for pin in SD_PADS {
        let pull = if SHARED_PADS.contains(&pin) { Pull::Pullup } else { Pull::Disabled };
        cfg_pad(pin, Dir::Output, Input::Disconnect, pull, Drive::E, Ctrlsel::Vpr);
    }
    pac::GPIOHSPADCTRL_S.bias().modify(|w| w.set_hsbias(2));
}

/// Reset the six card pads to their power-on shape (input, disconnected, GPIO, standard drive) —
/// the handoff courtesy before the jump, so the app's bring-up starts from what reset would give.
fn reset_pads() {
    for pin in SD_PADS {
        cfg_pad(pin, Dir::Input, Input::Disconnect, Pull::Disabled, Drive::S, Ctrlsel::Gpio);
    }
}

/// Response landing zone — four documented words, doubled as insurance (the app's shape).
#[repr(C, align(4))]
struct RespRaw([u32; 8]);
static mut RESP_RAW: RespRaw = RespRaw([0; 8]);

/// A 32-bit-aligned block — the shape every buffer handed to the firmware must have.
#[repr(C, align(4))]
struct AlignedBlock([u8; BLOCK_BYTES]);
