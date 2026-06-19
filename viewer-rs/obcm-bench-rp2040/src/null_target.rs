//! A `DrawTarget` that stores no framebuffer — it just *consumes* whatever the
//! renderer draws so the rasterizer's per-pixel coordinate math actually runs
//! (and isn't optimized away), while costing ~0 RAM. That lets the full ~199 KB
//! renderer scratch fit in the RP2040's 264 KB without also paying for a 150 KB
//! 240x320 framebuffer.
//!
//! What it (deliberately) does and doesn't time:
//!   * `draw_iter` / `fill_contiguous` — fully drained pixel-by-pixel with a
//!     `black_box`, so polygon scanline fills and thick-polyline strokes pay
//!     their real coordinate-generation cost. THIS is the work we want to measure.
//!   * `clear` / `fill_solid` — treated as O(1) (a memset on real hardware): we
//!     just tally the area instead of iterating, so a full-screen clear doesn't
//!     drown the geometry cost in 76 800 no-op increments.
//!
//! The only thing skipped vs. a real device is the final store of each pixel into
//! a framebuffer array — a cheap write next to the soft-float geometry on an M0+.

use core::convert::Infallible;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

pub struct NullTarget {
    /// Pixels emitted via `draw_iter` / `fill_contiguous` (the rasterized path).
    pub pixels: u32,
    /// Pixels covered by `fill_solid` / `clear` (counted, not iterated).
    pub solid_pixels: u32,
    size: Size,
}

impl NullTarget {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { pixels: 0, solid_pixels: 0, size: Size::new(width, height) }
    }
}

impl OriginDimensions for NullTarget {
    fn size(&self) -> Size {
        self.size
    }
}

impl DrawTarget for NullTarget {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        // Drain the iterator: this is where the renderer's scanline/stroke
        // coordinate computation happens. `black_box` stops LLVM from eliding it.
        for pixel in pixels {
            self.pixels = self.pixels.wrapping_add(1);
            core::hint::black_box(pixel);
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let a = area.size;
        self.solid_pixels = self.solid_pixels.wrapping_add(a.width * a.height);
        core::hint::black_box(color);
        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        let a = self.size;
        self.solid_pixels = self.solid_pixels.wrapping_add(a.width * a.height);
        core::hint::black_box(color);
        Ok(())
    }
}
