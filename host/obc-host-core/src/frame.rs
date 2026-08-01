//! A plain in-memory RGBA8888 `DrawTarget` — the buffer `ctx.putImageData` reads.
//!
//! The shared render path draws the firmware-identical frame into this buffer; the page then
//! wraps the bytes in an `ImageData` and blits them to the `<canvas>`. Full-frame transport at
//! the replay cadence (240×320×4 ≈ 300 KB memcpy) is plenty on a browser — the device's
//! dirty-row diff machinery stays in the hosts that protect glass (`obc-sim`'s `Present` + its
//! exact-diff oracle), deliberately **not** duplicated here.
//!
//! Lives here rather than in one shell because **both** browser hosts draw into it: the landing
//! demo (`obc-web-demo`, which renders the whole app) and the builder's preset previews
//! (for example, the simulator). One framebuffer, one set of clipping and
//! alpha invariants, one test.

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};

/// An owned `width`×`height` RGBA framebuffer (4 bytes/pixel, row-major, alpha always opaque).
pub struct RgbaFrame {
    width: u32,
    height: u32,
    buf: Vec<u8>,
}

impl RgbaFrame {
    pub fn new(width: u32, height: u32) -> Self {
        let mut buf = vec![0u8; (width * height * 4) as usize];
        // Pre-set every alpha to opaque; pixel writes below only touch RGB.
        for a in buf.iter_mut().skip(3).step_by(4) {
            *a = 0xFF;
        }
        RgbaFrame { width, height, buf }
    }

    /// The raw RGBA bytes, `width * height * 4` long — exactly the layout `ImageData` expects.
    pub fn as_rgba(&self) -> &[u8] {
        &self.buf
    }

    /// Write one pixel, clipping silently to the buffer bounds (the renderer projects geometry
    /// that can land off-screen).
    #[inline]
    fn put(&mut self, x: i32, y: i32, c: Rgb888) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let i = ((y as u32 * self.width + x as u32) * 4) as usize;
        self.buf[i] = c.r();
        self.buf[i + 1] = c.g();
        self.buf[i + 2] = c.b();
        // buf[i + 3] stays 0xFF from `new`.
    }
}

impl OriginDimensions for RgbaFrame {
    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

impl DrawTarget for RgbaFrame {
    type Color = Rgb888;
    // The buffer can't fail to accept a pixel; out-of-bounds writes are clipped.
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            self.put(p.x, p.y, c);
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        // Row-sliced fill: the renderer clears + fills large rects every frame, so don't go
        // pixel-by-pixel through `draw_iter`.
        let x0 = area.top_left.x.max(0);
        let y0 = area.top_left.y.max(0);
        let x1 = (area.top_left.x + area.size.width as i32).min(self.width as i32);
        let y1 = (area.top_left.y + area.size.height as i32).min(self.height as i32);
        if x0 >= x1 || y0 >= y1 {
            return Ok(());
        }
        let px = [color.r(), color.g(), color.b(), 0xFF];
        for y in y0..y1 {
            let start = ((y as u32 * self.width + x0 as u32) * 4) as usize;
            let row = &mut self.buf[start..start + ((x1 - x0) as usize) * 4];
            for chunk in row.chunks_exact_mut(4) {
                chunk.copy_from_slice(&px);
            }
        }
        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        self.fill_solid(&Rectangle::new(Point::zero(), Size::new(self.width, self.height)), color)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pixel writes clip, fills stay rectangular, and alpha is opaque everywhere — the exact
    /// invariants `putImageData` relies on.
    #[test]
    fn writes_clip_and_alpha_stays_opaque() {
        let mut f = RgbaFrame::new(4, 3);
        f.draw_iter([
            Pixel(Point::new(1, 1), Rgb888::new(10, 20, 30)),
            Pixel(Point::new(-1, 0), Rgb888::new(1, 1, 1)), // clipped
            Pixel(Point::new(4, 2), Rgb888::new(1, 1, 1)),  // clipped
        ])
        .unwrap();
        f.fill_solid(&Rectangle::new(Point::new(2, 0), Size::new(99, 1)), Rgb888::new(5, 6, 7)).unwrap();
        let b = f.as_rgba();
        let px = (4 + 1) * 4; // row 1, col 1
        assert_eq!(&b[px..px + 4], &[10, 20, 30, 0xFF]);
        assert_eq!(&b[2 * 4..2 * 4 + 4], &[5, 6, 7, 0xFF], "fill clipped to the right edge");
        assert!(b.iter().skip(3).step_by(4).all(|&a| a == 0xFF), "alpha opaque everywhere");
        assert_eq!(&b[0..3], &[0, 0, 0], "the clipped writes landed nowhere");
    }
}
