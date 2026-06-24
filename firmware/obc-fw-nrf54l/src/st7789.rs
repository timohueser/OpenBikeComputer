//! Minimal ST7789 driver for the Adafruit 2.0" 240×320 EYESPI stand-in panel (#122).
//!
//! Hand-rolled (no external display crate) on purpose: this same command / address-window /
//! RAMWR-stream path becomes the nRF `Panel` backend's `flush_band` in later phases, so we
//! want to own it rather than wrap someone else's lifecycle. Write-only — the panel never
//! talks back, so there is no MISO and CS is held low by the caller (single device on the
//! bus, the same trick the STM32 port uses). Blocking SPI + busy-wait delays are fine here:
//! init runs once and the test-pattern push is the only hot path until N4 wires the real
//! framebuffer through.
//!
//! Generic over embedded-hal 1.0 `SpiBus` / `OutputPin` / `DelayNs`, which embassy-nrf's
//! `Spim` + `Output` and embassy-time's `Delay` all implement.

use embedded_hal::delay::DelayNs;
use embedded_hal::digital::OutputPin;
use embedded_hal::spi::SpiBus;

/// Native panel geometry with `MADCTL = 0x00` (portrait): 240 columns × 320 rows, no offset.
/// (The 240×240 ST7789 variants need a row offset; the full 240×320 panel starts at 0,0.)
pub const WIDTH: u16 = 240;
pub const HEIGHT: u16 = 320;

/// ST7789 command set (the subset this bring-up uses).
mod cmd {
    pub const SWRESET: u8 = 0x01;
    pub const SLPOUT: u8 = 0x11;
    pub const NORON: u8 = 0x13;
    pub const INVON: u8 = 0x21;
    pub const DISPON: u8 = 0x29;
    pub const CASET: u8 = 0x2A;
    pub const RASET: u8 = 0x2B;
    pub const RAMWR: u8 = 0x2C;
    pub const MADCTL: u8 = 0x36;
    pub const COLMOD: u8 = 0x3A;
    // Power / timing / gamma block — the bias config that makes pixels actually visible.
    pub const PORCTRL: u8 = 0xB2;
    pub const GCTRL: u8 = 0xB7;
    pub const VCOMS: u8 = 0xBB;
    pub const LCMCTRL: u8 = 0xC0;
    pub const VDVVRHEN: u8 = 0xC2;
    pub const VRHS: u8 = 0xC3;
    pub const VDVS: u8 = 0xC4;
    pub const FRCTRL2: u8 = 0xC6;
    pub const PWCTRL1: u8 = 0xD0;
    pub const PVGAMCTRL: u8 = 0xE0;
    pub const NVGAMCTRL: u8 = 0xE1;
}

/// ST7789 over SPI: data/command line `dc`, hardware-reset line `rst`, and a `delay` for the
/// datasheet-mandated power-on waits. CS is the caller's responsibility (held low).
pub struct St7789<SPI, DC, RST, DELAY> {
    spi: SPI,
    dc: DC,
    rst: RST,
    delay: DELAY,
}

impl<SPI, DC, RST, DELAY> St7789<SPI, DC, RST, DELAY>
where
    SPI: SpiBus,
    DC: OutputPin,
    RST: OutputPin,
    DELAY: DelayNs,
{
    pub fn new(spi: SPI, dc: DC, rst: RST, delay: DELAY) -> Self {
        Self { spi, dc, rst, delay }
    }

    /// DC low marks a command byte. Each command and its data are separate SPI transactions
    /// (SCK idles between them), so DC is always stable across a whole byte — no DC/clock race.
    fn cmd(&mut self, c: u8) {
        self.dc.set_low().ok();
        self.spi.write(&[c]).ok();
    }

    /// DC high marks parameter/pixel bytes.
    fn data(&mut self, bytes: &[u8]) {
        self.dc.set_high().ok();
        self.spi.write(bytes).ok();
    }

    /// Convenience: a command immediately followed by its parameter bytes.
    fn cmd_data(&mut self, c: u8, params: &[u8]) {
        self.cmd(c);
        self.data(params);
    }

    /// Hardware reset + full power-on init. Delays follow the ST7789 datasheet (SLPOUT needs
    /// ≥120 ms before the next command). The porch/gate/**VCOM/power/gamma** block is the part
    /// that drives the LCD bias — a bare SLPOUT+DISPON can leave these IPS panels black even
    /// with valid pixel data on the bus (which is exactly the failure we hit on glass). Tunables:
    /// `INVON` because these IPS modules are normally-black (flip to `INVOFF` if the image is a
    /// colour negative); flip the `MADCTL` BGR bit (0x08) if red/blue read swapped.
    pub fn init(&mut self) {
        self.rst.set_high().ok();
        self.delay.delay_ms(10);
        self.rst.set_low().ok();
        self.delay.delay_ms(10);
        self.rst.set_high().ok();
        self.delay.delay_ms(120);

        self.cmd(cmd::SWRESET);
        self.delay.delay_ms(150);
        self.cmd(cmd::SLPOUT);
        self.delay.delay_ms(120);

        self.cmd_data(cmd::COLMOD, &[0x55]); // 16 bits/pixel, RGB565
        self.cmd_data(cmd::MADCTL, &[0x00]); // portrait, RGB order

        // Canonical ST7789 240×320 power/timing/gamma values (shared by most ST7789 drivers).
        self.cmd_data(cmd::PORCTRL, &[0x0C, 0x0C, 0x00, 0x33, 0x33]);
        self.cmd_data(cmd::GCTRL, &[0x35]);
        self.cmd_data(cmd::VCOMS, &[0x19]);
        self.cmd_data(cmd::LCMCTRL, &[0x2C]);
        self.cmd_data(cmd::VDVVRHEN, &[0x01]);
        self.cmd_data(cmd::VRHS, &[0x12]);
        self.cmd_data(cmd::VDVS, &[0x20]);
        self.cmd_data(cmd::FRCTRL2, &[0x0F]); // ~60 Hz
        self.cmd_data(cmd::PWCTRL1, &[0xA4, 0xA1]);
        self.cmd_data(
            cmd::PVGAMCTRL,
            &[0xD0, 0x04, 0x0D, 0x11, 0x13, 0x2B, 0x3F, 0x54, 0x4C, 0x18, 0x0D, 0x0B, 0x1F, 0x23],
        );
        self.cmd_data(
            cmd::NVGAMCTRL,
            &[0xD0, 0x04, 0x0C, 0x11, 0x13, 0x2C, 0x3F, 0x44, 0x51, 0x2F, 0x1F, 0x1F, 0x20, 0x23],
        );

        self.cmd(cmd::INVON);
        self.delay.delay_ms(10);
        self.cmd(cmd::NORON);
        self.delay.delay_ms(10);
        self.cmd(cmd::DISPON);
        self.delay.delay_ms(120);
    }

    /// Set the GRAM write window (inclusive bounds) and leave the panel in RAMWR/data mode so
    /// the caller can stream pixels straight into it with [`push`](Self::push).
    pub fn set_window(&mut self, x0: u16, y0: u16, x1: u16, y1: u16) {
        self.cmd_data(cmd::CASET, &[(x0 >> 8) as u8, x0 as u8, (x1 >> 8) as u8, x1 as u8]);
        self.cmd_data(cmd::RASET, &[(y0 >> 8) as u8, y0 as u8, (y1 >> 8) as u8, y1 as u8]);
        self.cmd(cmd::RAMWR);
        self.dc.set_high().ok();
    }

    /// Stream raw big-endian RGB565 bytes into the current window (call after `set_window`).
    pub fn push(&mut self, bytes: &[u8]) {
        self.spi.write(bytes).ok();
    }
}
