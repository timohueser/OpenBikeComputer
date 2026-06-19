//! A plain in-memory `DrawTarget` — a packed RGB888 pixel buffer the host owns.
//!
//! This is the simulator's replacement for embedded-graphics-simulator's
//! `SimulatorDisplay`: the shared [`obc_render::MapRenderer`] runs the exact same
//! rendering code as the firmware, but draws into a buffer we control, with no SDL
//! uploads the buffer to a GPU texture (the eframe screen window) or encodes it
//! to a PNG (`--png`). The device firmware draws into its real LS021B7DD02 driver
//! instead; only this host-side target differs.

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};

/// An owned `width`×`height` RGB888 framebuffer (3 bytes/pixel, row-major).
pub struct Framebuffer {
    width: u32,
    height: u32,
    buf: Vec<u8>,
}

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        Framebuffer { width, height, buf: vec![0u8; (width * height * 3) as usize] }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The raw packed RGB888 bytes, `width * height * 3` long — ready to hand to
    /// `image::RgbImage::from_raw` or `egui::ColorImage`.
    pub fn as_rgb888(&self) -> &[u8] {
        &self.buf
    }

    /// Write one pixel, clipping silently to the buffer bounds (the renderer
    /// projects geometry that can land off-screen).
    #[inline]
    fn put(&mut self, x: i32, y: i32, c: Rgb888) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let i = ((y as u32 * self.width + x as u32) * 3) as usize;
        self.buf[i] = c.r();
        self.buf[i + 1] = c.g();
        self.buf[i + 2] = c.b();
    }
}

impl OriginDimensions for Framebuffer {
    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

impl DrawTarget for Framebuffer {
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

    /// Fast path for the renderer's scanline fills (it calls this per polygon row
    /// and to clear): fill a clipped rectangle directly instead of per-pixel.
    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let clipped = area.intersection(&self.bounding_box());
        if let Some(br) = clipped.bottom_right() {
            for y in clipped.top_left.y..=br.y {
                for x in clipped.top_left.x..=br.x {
                    self.put(x, y, color);
                }
            }
        }
        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        for px in self.buf.chunks_exact_mut(3) {
            px[0] = color.r();
            px[1] = color.g();
            px[2] = color.b();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(fb: &Framebuffer, x: u32, y: u32) -> (u8, u8, u8) {
        let i = ((y * fb.width + x) * 3) as usize;
        let b = fb.as_rgb888();
        (b[i], b[i + 1], b[i + 2])
    }

    #[test]
    fn new_is_all_black() {
        let fb = Framebuffer::new(4, 3);
        assert_eq!(fb.as_rgb888().len(), 4 * 3 * 3);
        assert!(fb.as_rgb888().iter().all(|&b| b == 0));
    }

    #[test]
    fn clear_sets_every_pixel() {
        let mut fb = Framebuffer::new(4, 3);
        fb.clear(Rgb888::new(10, 20, 30)).unwrap();
        for y in 0..3 {
            for x in 0..4 {
                assert_eq!(pixel(&fb, x, y), (10, 20, 30));
            }
        }
    }

    #[test]
    fn fill_solid_fills_subrect_and_clips() {
        let mut fb = Framebuffer::new(4, 4);
        // Rectangle straddling the right/bottom edge: only the in-bounds part fills.
        fb.fill_solid(
            &Rectangle::new(Point::new(2, 2), Size::new(10, 10)),
            Rgb888::new(255, 0, 0),
        )
        .unwrap();
        assert_eq!(pixel(&fb, 1, 1), (0, 0, 0)); // outside
        assert_eq!(pixel(&fb, 2, 2), (255, 0, 0)); // inside corner
        assert_eq!(pixel(&fb, 3, 3), (255, 0, 0)); // last in-bounds pixel
    }

    #[test]
    fn draw_iter_sets_pixels_and_ignores_out_of_bounds() {
        let mut fb = Framebuffer::new(2, 2);
        let green = Rgb888::new(0, 255, 0);
        fb.draw_iter([
            Pixel(Point::new(0, 1), green),
            Pixel(Point::new(5, 5), green), // off-screen: silently dropped
            Pixel(Point::new(-1, 0), green), // off-screen: silently dropped
        ])
        .unwrap();
        assert_eq!(pixel(&fb, 0, 1), (0, 255, 0));
        assert_eq!(pixel(&fb, 0, 0), (0, 0, 0));
    }
}
