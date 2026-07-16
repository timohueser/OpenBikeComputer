//! Raw-block SD access for the install engine — the app's SD stack **minus the FAT layer**.
//!
//! `embedded_sdmmc::SdCard` over a blocking `embassy_nrf::spim::Spim` gives card init +
//! `read(BlockIdx)`; there is **no `VolumeManager`** (and must never be — extents in the boot
//! state are absolute 512-byte blocks, pre-resolved by the armer, so the bootloader needs no
//! filesystem inside its 32 KB budget; the linker keeps the FAT code out because nothing here
//! references it).
//!
//! Pins + frequencies are deliberate **duplicates** of the board crate's SD bring-up — the
//! source of truth is `../obc-fw-nrf54l/src/sd.rs` (`SD_INIT_HZ`/`SD_FAST_HZ`, the `NoCs`
//! held-low-CS workaround) and the SPIM/CS construction in `../obc-fw-nrf54l/src/main.rs`
//! (SERIAL22: SCK P1_11 · MISO P1_07 · MOSI P1_06 · CS **P0_00** held low for the session).
//! The crates don't share a pins module today; keep the copies in lockstep by hand.
//!
//! The one genuine difference from the app: there is no executor and no `embassy_time`, so the
//! `DelayNs` the card driver wants is a cycle-counted busy-wait ([`BusyDelay`]).

use embassy_embedded_hal::SetConfig;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::spim::{Config as SpiConfig, Frequency, Spim};
use embassy_nrf::{bind_interrupts, peripherals, spim, Peri};
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::{Block, BlockDevice, BlockIdx, SdCard};

// SERIAL22 backs the microSD SPIM, exactly as in the board crate. The handler is registered but
// the blocking transfer path never enables its NVIC line under load we care about; `jump_to_app`
// quiesces the NVIC regardless.
bind_interrupts!(struct Irqs {
    SERIAL22 => spim::InterruptHandler<peripherals::SERIAL22>;
});

/// SD clock during the init handshake — ≤400 kHz per the SD spec; `K250` is embassy-nrf's
/// fastest in-spec ladder step. Copied from `obc-fw-nrf54l/src/sd.rs::SD_INIT_HZ` (source of
/// truth) — keep in lockstep.
const SD_INIT_HZ: Frequency = Frequency::K250;

/// SD clock for bulk transfer once the card is up — 8 MHz, the fastest SERIAL22 reaches on the
/// PERI-domain P1 header. Copied from `obc-fw-nrf54l/src/sd.rs::SD_FAST_HZ` — keep in lockstep.
const SD_FAST_HZ: Frequency = Frequency::M8;

/// `DelayNs` as a cycle-counted busy-wait at the 64 MHz boot clock — the bootloader has no
/// timers and no executor, and the card driver only uses this for handshake pacing/timeouts,
/// where a busy-wait is exactly right.
pub struct BusyDelay;

impl embedded_hal::delay::DelayNs for BusyDelay {
    fn delay_ns(&mut self, ns: u32) {
        // 64 cycles/µs; +1 rounds up so a nonzero request never becomes a zero-cycle delay.
        cortex_m::asm::delay(((ns as u64 * 64) / 1000) as u32 + 1);
    }
}

/// A no-op chip-select for [`ExclusiveDevice`] — the held-low-CS workaround, copied from
/// `obc-fw-nrf54l/src/sd.rs::NoCs` (see its doc for the full story): embedded-sdmmc issues each
/// byte as its own `SpiDevice` op, and a real CS toggling between a command and its reply drops
/// the card off the bus. The *real* CS (P0_00) is held LOW for the whole session instead.
struct NoCs;
impl embedded_hal::digital::ErrorType for NoCs {
    type Error = core::convert::Infallible;
}
impl embedded_hal::digital::OutputPin for NoCs {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn set_high(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

type BootSd = SdCard<ExclusiveDevice<Spim<'static>, NoCs, BusyDelay>, BusyDelay>;

/// The bootloader's raw-block card handle: init + absolute block reads, nothing else.
pub struct SdBlocks {
    card: BootSd,
    /// The real chip-select (P0_00), driven HIGH for the wake clocks and LOW for the session —
    /// never handed to the card driver (see [`NoCs`]).
    cs: Output<'static>,
}

impl SdBlocks {
    /// Build the SPI → card stack (pins/instance mirror the board crate's SD bring-up — see the
    /// module doc). Does **not** talk to the card yet; call [`try_init`](Self::try_init) until
    /// it succeeds.
    pub fn new(
        serial: Peri<'static, peripherals::SERIAL22>,
        sck: Peri<'static, peripherals::P1_11>,
        miso: Peri<'static, peripherals::P1_07>,
        mosi: Peri<'static, peripherals::P1_06>,
        cs: Peri<'static, peripherals::P0_00>,
    ) -> SdBlocks {
        let mut cfg = SpiConfig::default();
        cfg.frequency = SD_INIT_HZ;
        cfg.orc = 0xFF; // over-reads clock the SD idle byte on MOSI
        let spi = Spim::new(serial, Irqs, sck, miso, mosi, cfg);
        let cs = Output::new(cs, Level::High, OutputDrive::Standard);
        // Infallible by construction (NoCs can't fail) — destructure rather than unwrap so this
        // crate keeps its no-panic property without trusting a panic path away.
        let dev = match ExclusiveDevice::new(spi, NoCs, BusyDelay) {
            Ok(dev) => dev,
            Err(never) => match never {},
        };
        SdBlocks { card: SdCard::new(dev, BusyDelay), cs }
    }

    /// One card bring-up attempt: wake clocks (≥74 with CS high, SD spec), re-acquire from
    /// scratch at the init clock, and on success re-clock the bus to [`SD_FAST_HZ`]. `false` =
    /// no card / init failed — the caller owns the retry-forever-with-backoff policy (the card
    /// is life-support; recovery is reinsert + power cycle).
    pub fn try_init(&mut self) -> bool {
        self.card.mark_card_uninit();
        self.set_speed(SD_INIT_HZ);
        self.cs.set_high();
        self.card.spi(|dev| {
            let _ = dev.bus_mut().blocking_write(&[0xFFu8; 10]);
        });
        self.cs.set_low();
        // `num_bytes` forces the init sequence (same probe the app's `sd::init` uses).
        if self.card.num_bytes().is_err() {
            return false;
        }
        self.set_speed(SD_FAST_HZ);
        true
    }

    fn set_speed(&self, f: Frequency) {
        self.card.spi(|dev| {
            let mut cfg = SpiConfig::default();
            cfg.frequency = f;
            cfg.orc = 0xFF;
            let _ = dev.bus_mut().set_config(&cfg);
        });
    }

    /// Read `out.len() / 512` whole blocks starting at absolute block `start` — the engine's
    /// `read_blocks` (`out` is always a non-zero multiple of 512). Multi-block CMD18 reads via
    /// an 8-block scratch (the driver wants `[Block]`, whose alignment we can't guarantee for a
    /// borrowed `&mut [u8]`; the copy is noise next to the SPI transfer itself).
    pub fn read_blocks(&self, start: u32, out: &mut [u8]) -> Result<(), ()> {
        let mut scratch: [Block; 8] = core::array::from_fn(|_| Block::new());
        let total = (out.len() / Block::LEN) as u32;
        let mut done = 0u32;
        while done < total {
            let n = ((total - done) as usize).min(scratch.len());
            self.card.read(&mut scratch[..n], BlockIdx(start + done)).map_err(|_| ())?;
            for (i, block) in scratch[..n].iter().enumerate() {
                let at = (done as usize + i) * Block::LEN;
                out[at..at + Block::LEN].copy_from_slice(&block.contents);
            }
            done += n as u32;
        }
        Ok(())
    }
}
