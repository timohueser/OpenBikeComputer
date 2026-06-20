//! A `DrawTarget` over a raw `&mut [u16]` RGB565 framebuffer.
//!
//! The on-device counterpart of the simulator's `obc-sim/src/framebuffer.rs`: the
//! shared [`obc_render::MapRenderer`](../../obc_render) (driven through
//! [`obc_app::App::render_frame`](../../obc_app)) runs the exact same rendering
//! code on the host and on the MCU, drawing into a buffer the board owns. Here the
//! buffer is the LTDC-scanned SDRAM framebuffer: a flat `width * height` array of
//! native RGB565 pixels, hardware-rescanned to the panel every frame, so a written
//! pixel is on glass with no blit.
//!
//! The panel is native RGB565, so the `color_fn` the app is rendered with is the
//! **identity** `RGB565 -> Rgb565` (`|c| Rgb565::from(RawU16::new(c))`). The
//! device-64 quantization (`obc_reader::rgb565_to_device64`) the simulator applies
//! is a host concern — a preview of the final LS021B7DD02's RGB222 gamut — and has
//! no place on the device path.

use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};

/// A `DrawTarget` wrapping a borrowed `width * height` RGB565 buffer (one `u16`
/// per pixel, row-major). The buffer is the board's — typically a slice over the
/// SDRAM framebuffer the LTDC scans — so this owns nothing and only writes pixels.
pub struct Framebuffer565<'a> {
    width: u32,
    height: u32,
    buf: &'a mut [u16],
}

impl<'a> Framebuffer565<'a> {
    /// Wrap `buf` as a `width`×`height` RGB565 target. `buf` must hold at least
    /// `width * height` pixels; a shorter slice is a board wiring bug, so it panics
    /// (this is bring-up code — better a clear panic over RTT than a silent
    /// out-of-bounds later).
    pub fn new(buf: &'a mut [u16], width: u32, height: u32) -> Self {
        assert!(
            buf.len() >= (width * height) as usize,
            "framebuffer slice smaller than width*height"
        );
        Framebuffer565 { width, height, buf }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The raw RGB565 pixels — `width * height` long. Handy for tests and for a
    /// board that wants to DMA the buffer somewhere itself.
    pub fn as_u16(&self) -> &[u16] {
        self.buf
    }

    /// Write one pixel, clipping silently to the buffer bounds (the renderer
    /// projects geometry that can land off-screen).
    #[inline]
    fn put(&mut self, x: i32, y: i32, raw: u16) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        self.buf[y as usize * self.width as usize + x as usize] = raw;
    }
}

impl OriginDimensions for Framebuffer565<'_> {
    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

impl DrawTarget for Framebuffer565<'_> {
    type Color = Rgb565;
    // The buffer can't fail to accept a pixel; out-of-bounds writes are clipped.
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            self.put(p.x, p.y, c.into_storage());
        }
        Ok(())
    }

    /// Fast path for the renderer's scanline fills (it calls this per polygon row
    /// and to clear): fill a clipped rectangle row-by-row instead of per-pixel.
    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let clipped = area.intersection(&self.bounding_box());
        if let Some(br) = clipped.bottom_right() {
            let raw = color.into_storage();
            let w = self.width as usize;
            for y in clipped.top_left.y..=br.y {
                let row = y as usize * w;
                for x in clipped.top_left.x as usize..=br.x as usize {
                    self.buf[row + x] = raw;
                }
            }
        }
        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        let raw = color.into_storage();
        for px in self.buf.iter_mut() {
            *px = raw;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::pixelcolor::raw::RawU16;

    fn fb(buf: &mut [u16], w: u32, h: u32) -> Framebuffer565<'_> {
        Framebuffer565::new(buf, w, h)
    }

    #[test]
    fn put_writes_native_rgb565_and_clips() {
        let mut buf = [0u16; 2 * 2];
        let mut fb = fb(&mut buf, 2, 2);
        let red = Rgb565::from(RawU16::new(0xF800));
        fb.draw_iter([
            Pixel(Point::new(0, 1), red),
            Pixel(Point::new(5, 5), red),  // off-screen: dropped
            Pixel(Point::new(-1, 0), red), // off-screen: dropped
        ])
        .unwrap();
        assert_eq!(buf, [0x0000, 0x0000, 0xF800, 0x0000]);
    }

    #[test]
    fn clear_sets_every_pixel() {
        let mut buf = [0u16; 4 * 3];
        let raw = 0x07E0; // green
        fb(&mut buf, 4, 3).clear(Rgb565::from(RawU16::new(raw))).unwrap();
        assert!(buf.iter().all(|&p| p == raw));
    }

    #[test]
    fn fill_solid_fills_subrect_and_clips() {
        let mut buf = [0u16; 4 * 4];
        {
            let mut fb = fb(&mut buf, 4, 4);
            // Rectangle straddling the right/bottom edge: only the in-bounds part fills.
            fb.fill_solid(
                &Rectangle::new(Point::new(2, 2), Size::new(10, 10)),
                Rgb565::from(RawU16::new(0x001F)),
            )
            .unwrap();
        }
        let at = |x: usize, y: usize| buf[y * 4 + x];
        assert_eq!(at(1, 1), 0x0000); // outside
        assert_eq!(at(2, 2), 0x001F); // inside corner
        assert_eq!(at(3, 3), 0x001F); // last in-bounds pixel
    }

    #[test]
    #[should_panic]
    fn too_small_buffer_panics() {
        let mut buf = [0u16; 3];
        let _ = Framebuffer565::new(&mut buf, 2, 2); // needs 4
    }
}
