//! **The ST7789 `DisplayDriver` backend** (issue #174) — the opt-in `tft` TFT panel behind the
//! board-agnostic [`DisplayDriver`](super::DisplayDriver) seam.
//!
//! A thin adapter over the low-level [`St7789`](crate::st7789::St7789) driver (the command / address-
//! window / RAMWR-stream transport stays at the crate root, shared with nothing else). The map plane
//! renders the whole frame into the resident RGB222 [`fb`](Display::fb) plane, then [`present`] bands
//! it to GRAM; [`present_overlay`] composites the hold bulge over the clean backdrop with the shared
//! [`composite_overlay_window`] and DMAs just that column window. The only ST7789-specific code is the
//! RGB222 → 12-bit wire-pack inside those two pushes — everything else is the shared core.
//!
//! [`present`]: Display::present
//! [`present_overlay`]: Display::present_overlay

use embassy_nrf::gpio::Output;
use embassy_nrf::spim::Spim;
use embassy_time::Delay;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use obc_platform::{composite_overlay_window, Band};

use super::{DisplayDriver, OverlayRegion};
use crate::st7789::{self, St7789, HEIGHT, WIDTH};

/// The concrete display panel type behind the bus mutex: the ST7789 over the SERIAL00 SPIM, its three
/// GPIO control lines, and the `'static` RGB565 band scratch. Named so [`Display`] + the mutex can be
/// `'static`. (`Spim`/`Output` aren't generic over the instance, so this fully specifies the type.)
pub type DisplayPanel = St7789<'static, Spim<'static>, Output<'static>, Output<'static>, Output<'static>, Delay>;

/// The shared display the two planes split: the ST7789 panel + the resident RGB222 framebuffer it
/// pushes. Both reach both only through the bus mutex (`main.rs`), so the map render's framebuffer
/// write is serialised against the input plane's overlay-window read — the bulge backdrop is never
/// torn. `main.rs` builds it (`Display { panel, fb }`) and drives it only through [`DisplayDriver`].
pub struct Display {
    pub panel: DisplayPanel,
    pub fb: &'static mut [u8],
}

impl DisplayDriver for Display {
    fn fb_mut(&mut self) -> &mut [u8] {
        self.fb
    }

    /// Push the whole RGB222 framebuffer to the ST7789, band by band, **straight from RGB222 to the
    /// panel's 12-bit RGB444 wire** ([`flush_band_rgb222`](St7789::flush_band_rgb222) — the fast path,
    /// no RGB565 intermediate). The transient hold bulge is **not** drawn here: it rides its own
    /// overlay re-push on the input plane (issue #126/#163), so the map present stays a single clean
    /// pack with no overlay coupling. ST7789 GRAM writes don't fault, so always `true`.
    fn present(&mut self) -> bool {
        let Display { panel, fb } = self;
        st7789::reset_push_timers();
        let rows = panel.band_rows();
        let mut y0 = 0u16;
        while y0 < HEIGHT {
            let h = rows.min(HEIGHT - y0);
            let row0 = y0 as usize * WIDTH as usize;
            let n = WIDTH as usize * h as usize;
            panel.flush_band_rgb222(y0, h, &fb[row0..row0 + n]);
            y0 += h;
        }
        let (fill_us, pack_us, spi_us) = st7789::push_timers();
        defmt::info!("ST7789 push: fill {=u32} + pack {=u32} + spi {=u32} us", fill_us, pack_us, spi_us);
        true
    }

    /// Re-push just the overlay rectangle: the ST7789 is column-addressable, so it addresses exactly
    /// the `region` window (`flush_window`). The shared [`composite_overlay_window`] fills the scratch
    /// from the clean framebuffer backdrop (RGB222 → RGB565) + composites the bulge via `draw_overlay`;
    /// the panel then DMAs that window — no map re-render, no torn frame (the caller holds the bus).
    /// GRAM writes don't fault, so always `true`.
    fn present_overlay(&mut self, region: OverlayRegion, draw_overlay: &mut dyn FnMut(&mut Band)) -> bool {
        let Display { panel, fb } = self;
        let fb: &[u8] = fb;
        panel.flush_window(region.x0, region.y0, region.w, region.rows, |scratch| {
            let window = Rectangle::new(
                Point::new(region.x0 as i32, region.y0 as i32),
                Size::new(region.w as u32, region.rows as u32),
            );
            composite_overlay_window(fb, Size::new(WIDTH as u32, HEIGHT as u32), window, scratch, draw_overlay);
        });
        true
    }
}
