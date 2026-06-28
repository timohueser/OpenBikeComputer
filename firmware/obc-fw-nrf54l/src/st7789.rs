//! Minimal ST7789 driver for the Adafruit 2.0" 240×320 EYESPI stand-in panel (#122).
//!
//! Hand-rolled (no external display crate) on purpose: this command / address-window /
//! RAMWR-stream path backs the board-agnostic [`obc_platform::Panel`] `flush_band` (impl below),
//! so we want to own it rather than wrap someone else's lifecycle. Write-only — the panel never
//! talks back, so there is no MISO. CS is **framed per transaction** (pulsed low around each
//! command/data write, idle high) rather than tied low: the CSX rising edge re-aligns the
//! panel's input shift register, so a warm MCU reset recovers instead of needing a power cycle
//! — see [`St7789::transaction`]. Blocking SPI + busy-wait delays are fine here: init runs once
//! and the test-pattern push is the only hot path until N4 wires the real framebuffer through.
//!
//! Generic over embedded-hal 1.0 `SpiBus` / `OutputPin` / `DelayNs`, which embassy-nrf's
//! `Spim` + `Output` and embassy-time's `Delay` all implement.

// The FLPR build (`panel-ls021`, issue #165) still compiles this module for its `WIDTH`/`HEIGHT`
// geometry, but replaces the ST7789 driver with the LS021 FLPR backend — so the whole driver (the
// `cmd` set, the `St7789` type, the push fast paths) is unused there. Allow it only in that build;
// the default + glass-demo builds keep dead-code enforced.
#![cfg_attr(feature = "panel-ls021", allow(dead_code))]

use core::sync::atomic::{AtomicU32, Ordering};

use embassy_time::Instant;
use embedded_hal::delay::DelayNs;
use embedded_hal::digital::OutputPin;
use embedded_hal::spi::SpiBus;
use obc_platform::Panel;

// Strippable per-stage timing for the banded push (issue #126: pin down what makes the visible fill
// slow — CPU format-conversion vs the SPI/DMA). Each [`flush_window`](St7789::flush_window) adds the
// microseconds it spent in the three stages; the map loop [`reset_push_timers`]s before a frame's
// push and reads [`push_timers`] after, so the totals are that frame's push only.
static FILL_US: AtomicU32 = AtomicU32::new(0); // the caller's fill closure (RGB222->RGB565 expand + bulge composite)
static PACK_US: AtomicU32 = AtomicU32::new(0); // RGB565 -> 12-bit RGB444 pack
static SPI_US: AtomicU32 = AtomicU32::new(0); // set_window (CASET/RASET/RAMWR) + the data DMA

/// Zero the per-stage push timers — call before a frame's band-push loop. Map-path only (the
/// `glass-demo` panel bring-up draws a single static frame, so it never reads the push timing).
#[cfg(not(feature = "glass-demo"))]
pub fn reset_push_timers() {
    FILL_US.store(0, Ordering::Relaxed);
    PACK_US.store(0, Ordering::Relaxed);
    SPI_US.store(0, Ordering::Relaxed);
}

/// `(fill_us, pack_us, spi_us)` accumulated since the last [`reset_push_timers`]. Map-path only
/// (see [`reset_push_timers`]).
#[cfg(not(feature = "glass-demo"))]
pub fn push_timers() -> (u32, u32, u32) {
    (FILL_US.load(Ordering::Relaxed), PACK_US.load(Ordering::Relaxed), SPI_US.load(Ordering::Relaxed))
}

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

/// ST7789 over SPI: data/command line `dc`, hardware-reset line `rst`, chip-select `cs` (pulsed
/// low per transaction by [`transaction`](Self::transaction)), a `delay` for the
/// datasheet-mandated power-on waits, and a borrowed RGB565 `band` scratch the [`Panel`] impl
/// fills + DMAs (its length picks the band height).
pub struct St7789<'b, SPI, DC, RST, CS, DELAY> {
    spi: SPI,
    dc: DC,
    rst: RST,
    cs: CS,
    delay: DELAY,
    /// One band's worth of RGB565 pixels (`WIDTH * band_rows`), borrowed from a board-owned
    /// `'static` buffer so it lives in `.bss`, never on the stack. [`Panel::flush_band`] fills it
    /// (native LE `u16`), then [`flush_window`](Self::flush_window) packs it **in place** to the
    /// ST7789's 12-bit RGB444 RAMWR stream (2 px → 3 bytes) and SPIM-DMAs it. `band_rows()` is
    /// `band.len() / WIDTH`.
    band: &'b mut [u16],
}

impl<'b, SPI, DC, RST, CS, DELAY> St7789<'b, SPI, DC, RST, CS, DELAY>
where
    SPI: SpiBus,
    DC: OutputPin,
    RST: OutputPin,
    CS: OutputPin,
    DELAY: DelayNs,
{
    pub fn new(spi: SPI, dc: DC, rst: RST, cs: CS, delay: DELAY, band: &'b mut [u16]) -> Self {
        Self { spi, dc, rst, cs, delay, band }
    }

    /// Frame one transaction between a CS low→high pulse. The ST7789's serial interface is gated
    /// by CSX, not tied low: the rising edge at the end resets the panel's input shift register so
    /// the next transaction starts byte-aligned. That re-alignment is what lets a **warm MCU
    /// reset** recover — held low, a reset mid-stream would strand the bit counter mid-byte and
    /// every later boot would clock its init in misaligned (black panel until a power cycle). DC
    /// is set inside the frame, while CS is asserted.
    fn transaction<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.cs.set_low().ok();
        let r = f(self);
        self.cs.set_high().ok();
        r
    }

    /// One command byte (DC low) in its own CS frame.
    fn cmd(&mut self, c: u8) {
        self.transaction(|s| {
            s.dc.set_low().ok();
            s.spi.write(&[c]).ok();
        });
    }

    /// A command (DC low) immediately followed by its parameter bytes (DC high), in one CS frame.
    /// The ST7789 keys command-vs-data off the DC level of each byte, so DC can toggle mid-frame.
    fn cmd_data(&mut self, c: u8, params: &[u8]) {
        self.transaction(|s| {
            s.dc.set_low().ok();
            s.spi.write(&[c]).ok();
            s.dc.set_high().ok();
            s.spi.write(params).ok();
        });
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

        self.cmd_data(cmd::COLMOD, &[0x53]); // 12 bits/pixel, RGB444 (2 px per 3 bytes — ~25% less
                                             // wire data than RGB565, and the device-64 gamut fits
                                             // 4 bits/channel losslessly; see `flush_window`)
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

    /// Set the GRAM write window (inclusive bounds) and issue RAMWR so the caller can stream
    /// pixels with [`push`](Self::push). RAMWR mode and the auto-incrementing address pointer
    /// survive the CS pulses between here and the pushes — CSX only gates bus access, it neither
    /// exits the memory write nor resets the pointer — so the window fills correctly across many
    /// separately-framed `push` calls.
    pub fn set_window(&mut self, x0: u16, y0: u16, x1: u16, y1: u16) {
        self.cmd_data(cmd::CASET, &[(x0 >> 8) as u8, x0 as u8, (x1 >> 8) as u8, x1 as u8]);
        self.cmd_data(cmd::RASET, &[(y0 >> 8) as u8, y0 as u8, (y1 >> 8) as u8, y1 as u8]);
        self.cmd(cmd::RAMWR);
    }

    /// Stream raw big-endian RGB565 bytes (DC high) into the current window, in its own CS frame;
    /// continues the RAMWR opened by [`set_window`](Self::set_window).
    pub fn push(&mut self, bytes: &[u8]) {
        self.transaction(|s| {
            s.dc.set_high().ok();
            s.spi.write(bytes).ok();
        });
    }

    /// Render + push an arbitrary `w × rows` window at `(x0, y0)`: hand `fill` a `w * rows` RGB565
    /// scratch (a prefix of `band`) to draw into, **pack it to the panel's 12-bit RGB444 RAMWR
    /// stream** (2 px → 3 bytes), address the `[x0, x0+w) × [y0, y0+rows)` window, and SPIM-DMA it.
    /// The full-width [`Panel::flush_band`] is the `x0 = 0, w = WIDTH` case (it delegates here); the
    /// narrow case backs the composite-on-push hold bulge, which re-pushes only the right-edge
    /// columns it touches without a map re-render (issue #126). `w * rows` must be **even** and fit
    /// `band` (every nRF window is — full bands are `WIDTH`-wide and the bulge window is 16-wide).
    pub fn flush_window(&mut self, x0: u16, y0: u16, w: u16, rows: u16, fill: impl FnOnce(&mut [u16])) {
        let n = w as usize * rows as usize;
        let t = Instant::now();
        fill(&mut self.band[..n]);
        FILL_US.fetch_add(t.elapsed().as_micros() as u32, Ordering::Relaxed);
        let t = Instant::now();
        // Pack the RGB565 scratch in place to the ST7789's 12-bit (RGB444) wire format: each pixel
        // keeps the top 4 bits of every channel (the device-64 gamut only uses the top 2, so this is
        // lossless), and two pixels share three bytes — `[Ra:Ga][Ba:Rb][Gb:Bb]`. The 3-byte output
        // for pair `i` lands at byte `i*3`, which always trails the still-unread `u16`s at index
        // `≥ 2i+2` (byte `≥ 4i+4`), so packing forward over the same buffer never clobbers a pixel it
        // hasn't read yet. Saves ~25% of the wire bytes vs RGB565 (no endian swap needed either).
        let p = self.band.as_mut_ptr() as *mut u8;
        let mut i = 0;
        let mut o = 0;
        while i + 1 < n {
            let (ra, ga, ba) = rgb565_to_rgb444(self.band[i]);
            let (rb, gb, bb) = rgb565_to_rgb444(self.band[i + 1]);
            // SAFETY: `o + 2 < 2n` (since `o = (i/2)*3 ≤ 1.5n` and the buffer is `2n` bytes), and the
            // write at `o..=o+2` precedes every later pixel read (see the index argument above).
            unsafe {
                *p.add(o) = (ra << 4) | ga;
                *p.add(o + 1) = (ba << 4) | rb;
                *p.add(o + 2) = (gb << 4) | bb;
            }
            i += 2;
            o += 3;
        }
        PACK_US.fetch_add(t.elapsed().as_micros() as u32, Ordering::Relaxed);
        let t = Instant::now();
        self.set_window(x0, y0, x0 + w - 1, y0 + rows - 1);
        // The packed 12-bit stream is `n * 3 / 2` bytes at the base of `band`.
        // SAFETY: a `[u16]` is always validly aligned for `[u8]`; the view is `n*3/2` bytes over the
        // same allocation, the buffer outlives this blocking write, and nothing mutates it during.
        let bytes = unsafe { core::slice::from_raw_parts(self.band.as_ptr() as *const u8, n * 3 / 2) };
        self.push(bytes);
        SPI_US.fetch_add(t.elapsed().as_micros() as u32, Ordering::Relaxed);
    }

    /// Push a full-width band **straight from the RGB222 framebuffer** — the map plane's fast path.
    /// Packs `src` (`WIDTH * rows` device-64 bytes, `0b00_RR_GG_BB`) directly to the panel's 12-bit
    /// RGB444 stream (2 px → 3 bytes), **skipping the RGB565 intermediate** the generic
    /// [`flush_window`](Self::flush_window) goes through. `RGB222 → RGB444` is exact 2→4-bit
    /// replication (`c<<2 | c` → the 0/5/10/15 levels), *identical* output to the two-hop
    /// `RGB222 → RGB565 → RGB444` but ~half the CPU — the two-hop expand+pack was ~71% of the push
    /// (issue #126 perf). No bulge composite here: the input plane repaints the bulge on its own
    /// narrow window push (the generic `flush_window`), so the hot map path stays a single pack.
    /// Map-path only — the `glass-demo` build has no RGB222 framebuffer and pushes RGB565 bands
    /// through the generic [`flush_window`](Self::flush_window).
    #[cfg(not(feature = "glass-demo"))]
    pub fn flush_band_rgb222(&mut self, y0: u16, rows: u16, src: &[u8]) {
        let n = WIDTH as usize * rows as usize;
        let t = Instant::now();
        // Pack RGB222 `src` → 12-bit into the band scratch (`≥ 2n` bytes). `src` (the framebuffer) is
        // a *separate* buffer from `band`, so the reads never alias the writes; `o = (i/2)*3 ≤ 1.5n`.
        let out = self.band.as_mut_ptr() as *mut u8;
        let mut i = 0;
        let mut o = 0;
        while i + 1 < n {
            let (ra, ga, ba) = rgb222_to_rgb444(src[i]);
            let (rb, gb, bb) = rgb222_to_rgb444(src[i + 1]);
            // SAFETY: in-bounds per the index argument above; `band` is `[u16]` so `2n` bytes wide.
            unsafe {
                *out.add(o) = (ra << 4) | ga;
                *out.add(o + 1) = (ba << 4) | rb;
                *out.add(o + 2) = (gb << 4) | bb;
            }
            i += 2;
            o += 3;
        }
        PACK_US.fetch_add(t.elapsed().as_micros() as u32, Ordering::Relaxed);
        let t = Instant::now();
        self.set_window(0, y0, WIDTH - 1, y0 + rows - 1);
        // SAFETY: `[u16]` is aligned for `[u8]`; `n*3/2` bytes over the same allocation, held for the
        // blocking write, unmutated during it.
        let bytes = unsafe { core::slice::from_raw_parts(self.band.as_ptr() as *const u8, n * 3 / 2) };
        self.push(bytes);
        SPI_US.fetch_add(t.elapsed().as_micros() as u32, Ordering::Relaxed);
    }
}

/// Pack one RGB565 word to RGB444 channels (top 4 bits each). The device-64 gamut only sets the top
/// 2 bits per channel, so 4 bits hold it losslessly — re-expanding gives back the same colour.
#[inline]
fn rgb565_to_rgb444(c: u16) -> (u8, u8, u8) {
    let r = ((c >> 12) & 0xF) as u8; // top 4 of the 5-bit red
    let g = ((c >> 7) & 0xF) as u8; // top 4 of the 6-bit green
    let b = ((c >> 1) & 0xF) as u8; // top 4 of the 5-bit blue
    (r, g, b)
}

/// Pack one RGB222 (device-64) byte `0b00_RR_GG_BB` straight to RGB444 channels: replicate each
/// 2-bit channel to 4 bits (`c<<2 | c` → levels 0/5/10/15). This is exactly the two-hop
/// `RGB222 → RGB565 → RGB444` result (both land on those levels), so [`flush_band_rgb222`] produces
/// byte-identical output to the generic path while doing one conversion instead of two. Used only
/// by [`flush_band_rgb222`], so it's gated off the `glass-demo` (RGB565-only) build with it.
#[inline]
#[cfg(not(feature = "glass-demo"))]
fn rgb222_to_rgb444(byte: u8) -> (u8, u8, u8) {
    let r = (byte >> 4) & 0x3;
    let g = (byte >> 2) & 0x3;
    let b = byte & 0x3;
    ((r << 2) | r, (g << 2) | g, (b << 2) | b)
}

/// The board-agnostic [`Panel`] seam (issue #122). The caller renders a band of the frame into
/// the RGB565 scratch; this backend addresses the band's rows and streams them to GRAM. The same
/// seam the future FLPR/LS021B7DD02 backend implements — only the reformat + transport differ.
impl<SPI, DC, RST, CS, DELAY> Panel for St7789<'_, SPI, DC, RST, CS, DELAY>
where
    SPI: SpiBus,
    DC: OutputPin,
    RST: OutputPin,
    CS: OutputPin,
    DELAY: DelayNs,
{
    /// Band height = however many full `WIDTH`-pixel rows the board-owned scratch holds.
    fn band_rows(&self) -> u16 {
        (self.band.len() / WIDTH as usize) as u16
    }

    /// Nothing to set up — each `flush_band` addresses its own window, so a frame is just a run
    /// of band pushes. (A latching panel would arm its frame buffer here.)
    fn begin_frame(&mut self) {}

    /// Render one band into the scratch, then put it on glass: the full-width `[y0, y0+rows)`
    /// case of [`flush_window`](Self::flush_window).
    fn flush_band(&mut self, y0: u16, rows: u16, fill: impl FnOnce(&mut [u16])) {
        self.flush_window(0, y0, WIDTH, rows, fill);
    }

    /// Nothing to present — pixels are live in GRAM as each band is pushed. (A latching panel
    /// would toggle VCOM / present here.)
    fn end_frame(&mut self) {}
}
