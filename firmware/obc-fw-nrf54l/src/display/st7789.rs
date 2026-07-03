//! **The ST7789 `DisplayDriver` backend** — the opt-in `tft` TFT panel behind the board-agnostic
//! [`DisplayDriver`](super::DisplayDriver) seam.
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
use obc_platform::{composite_overlay_window, Band, RowDiff};

use super::{DisplayDriver, OverlayRegion};
use crate::st7789::{self, St7789, HEIGHT, WIDTH};

/// The concrete display panel type behind the bus mutex: the ST7789 over the SERIAL00 SPIM, its three
/// GPIO control lines, and the `'static` RGB565 band scratch. Named so [`Display`] + the mutex can be
/// `'static`. (`Spim`/`Output` aren't generic over the instance, so this fully specifies the type.)
pub type DisplayPanel = St7789<'static, Spim<'static>, Output<'static>, Output<'static>, Output<'static>, Delay>;

/// The shared display the two planes split: the ST7789 panel + the resident RGB222 framebuffer it
/// pushes + the self-diffing present store. Both reach both only through the bus mutex (`main.rs`), so
/// the map render's framebuffer write is serialised against the input plane's overlay-window read — the
/// bulge backdrop is never torn. `main.rs` builds it (`Display { panel, fb, diff }`) and drives it only
/// through [`DisplayDriver`].
pub struct Display {
    pub panel: DisplayPanel,
    pub fb: &'static mut [u8],
    /// The **self-diffing present** store: a per-row hash of the last-pushed framebuffer, so
    /// [`present`](DisplayDriver::present) RASET-windows only the rows that changed (a Home clock tick
    /// repaints its clock band, not all 320 rows). Borrowed from a `.bss` static.
    pub diff: &'static mut RowDiff<{ HEIGHT as usize }>,
}

impl DisplayDriver for Display {
    fn fb_mut(&mut self) -> &mut [u8] {
        self.fb
    }

    /// **Self-diffing present**: re-hash each framebuffer row against the [`RowDiff`] store and push
    /// only the changed rows — each contiguous changed span addressed as its own RASET window and banded
    /// **straight from RGB222 to the panel's 12-bit RGB444 wire**
    /// ([`flush_band_rgb222`](St7789::flush_band_rgb222) — the fast path, no RGB565 intermediate). The
    /// first present after boot / a [`RowDiff::reset`](obc_platform::RowDiff::reset) pushes the whole
    /// frame and seeds the store; an idle redraw then repaints just its changed band.
    ///
    /// The transient hold bulge is **not** drawn here: it rides its own overlay re-push on the input
    /// plane, so the map present stays a single clean pack with no overlay coupling. A live bulge's
    /// rows are clipped out of the push (`exclude` — the shared
    /// [`diff_clipped`](RowDiff::diff_clipped) skeleton the FLPR present also runs), so a `dirty.map`
    /// redraw landing mid-hold no longer blanks the bulge; the excluded rows keep their on-glass
    /// content until the overlay plane's next ~8 ms tick repaints them. ST7789 GRAM writes don't
    /// fault, so always `true`.
    fn present(&mut self, exclude: Option<(u16, u16)>) -> bool {
        let Display { panel, fb, diff } = self;
        st7789::reset_push_timers();
        let band_rows = panel.band_rows();
        let fb: &[u8] = fb;
        // Same span cap as the FLPR path's `MAX_DIRTY_SPANS`: >16 disjoint changed regions is
        // pathological fragmentation a UI never produces; `diff_clipped` then falls back to the whole
        // frame minus the exclude rather than dropping rows.
        let mut scratch = [(0u16, 0u16); 16];
        for &(sy, sn) in diff.diff_clipped(fb, WIDTH as usize, exclude, &mut scratch) {
            // Band the changed span [sy, sy+sn) in `band_rows`-tall chunks; each `flush_band_rgb222`
            // sets its own RASET window, so a partial span pushes only its rows.
            let mut y0 = sy;
            while y0 < sy + sn {
                let h = band_rows.min(sy + sn - y0);
                let row0 = y0 as usize * WIDTH as usize;
                let n = WIDTH as usize * h as usize;
                panel.flush_band_rgb222(y0, h, &fb[row0..row0 + n]);
                y0 += h;
            }
        }
        // Per-stage push breakdown (per present) — `debug` so the loop's frame line stays the one
        // info-level per-frame log; opt in with `DEFMT_LOG=debug` for push perf-tuning.
        let (fill_us, pack_us, spi_us) = st7789::push_timers();
        defmt::debug!("ST7789 push: fill {=u32} + pack {=u32} + spi {=u32} us", fill_us, pack_us, spi_us);
        true
    }

    /// Re-push just the overlay rectangle: the ST7789 is column-addressable, so it addresses exactly
    /// the `region` window (`flush_window`). The shared [`composite_overlay_window`] fills the scratch
    /// from the clean framebuffer backdrop (RGB222 → RGB565) + composites the bulge via `draw_overlay`;
    /// the panel then DMAs that window — no map re-render, no torn frame (the caller holds the bus).
    /// GRAM writes don't fault, so always `true`.
    fn present_overlay(&mut self, region: OverlayRegion, draw_overlay: &mut dyn FnMut(&mut Band)) -> bool {
        // The overlay reads the clean backdrop only; the row-hash store is the map present's concern.
        let Display { panel, fb, .. } = self;
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
