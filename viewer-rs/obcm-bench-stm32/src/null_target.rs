//! A `DrawTarget` that stores no framebuffer — it consumes whatever the renderer
//! draws so the rasterizer's per-pixel coordinate math runs (and isn't optimized
//! away) while costing ~0 RAM. Identical to the RP2040 bench's null target so the
//! two boards measure exactly the same work. See obcm-bench-rp2040 for the full
//! rationale (draw_iter/fill_contiguous are drained; clear/fill_solid are O(1)).

use core::convert::Infallible;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

pub struct NullTarget {
    pub pixels: u32,
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
