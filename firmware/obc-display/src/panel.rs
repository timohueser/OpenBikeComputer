//! [`Band`], a frame-absolute draw view of one window of the frame, plus the
//! [`composite_overlay_window`] overlay helper.
//!
//! An overlay is composited in a small RGB565 window. Each presenter backend owns the push to
//! glass; this module supplies the pieces they share.
//!
//! [`Band`] makes the window invisible to the drawing code: it wraps the window's scratch slice as
//! a [`Framebuffer565`], yet reports the full frame size. An overlay draws in absolute coordinates,
//! and [`Band`] shifts each draw by the window's origin and clips whatever falls outside.

use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};

use crate::framebuffer::{device64_to_rgb565, Framebuffer565};

/// A frame-absolute [`DrawTarget`] view of one rectangular window of the frame.
///
/// It wraps the scratch as a [`Framebuffer565`] sized to just this window, but reports the full
/// frame size and offsets every draw by `(-x0, -y0)`, so a generator laying out the whole frame
/// lands its pixels for this window in the scratch and has the rest clipped away.
pub struct Band<'a> {
    /// The window's own RGB565 buffer, `w × rows`. Draws land here in window-local coords.
    fb: Framebuffer565<'a>,
    /// This window's first frame column, subtracted from every incoming `x`; columns outside
    /// `[0, w)` clip in `fb`.
    x0: i32,
    /// This window's first frame row, subtracted from every incoming `y`; rows outside
    /// `[0, rows)` clip in `fb`.
    y0: i32,
    /// The full frame size, reported to the generator so its layout spans the whole panel.
    frame: Size,
}

impl<'a> Band<'a> {
    /// View `scratch` as the frame window at `(x0, y0)`, sized `w × rows`, reporting the full
    /// `frame`. Frame-absolute draws land offset by `(-x0, -y0)` and anything outside the window
    /// clips. Panics if `scratch` is shorter than `w * rows`.
    pub fn new(scratch: &'a mut [u16], frame: Size, x0: u16, y0: u16, w: u16, rows: u16) -> Self {
        Self { fb: Framebuffer565::new(scratch, w as u32, rows as u32), x0: x0 as i32, y0: y0 as i32, frame }
    }
}

impl OriginDimensions for Band<'_> {
    /// The full frame, so a generator sizing itself off `bounding_box()` lays out the whole panel
    /// rather than just this window.
    fn size(&self) -> Size {
        self.frame
    }
}

impl DrawTarget for Band<'_> {
    type Color = Rgb565;
    // The inner framebuffer can't fail; out-of-window pixels are clipped, not errors.
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        let (x0, y0) = (self.x0, self.y0);
        self.fb.draw_iter(pixels.into_iter().map(move |Pixel(p, c)| Pixel(Point::new(p.x - x0, p.y - y0), c)))
    }

    /// Offset the fill rectangle into window-local space. The inner framebuffer intersects it
    /// with the window's bounds, so a rect spanning rows or columns outside this window fills
    /// only its slice. This is the renderer's hot path, so it forwards straight to `RawFb`.
    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let shifted = Rectangle::new(Point::new(area.top_left.x - self.x0, area.top_left.y - self.y0), area.size);
        self.fb.fill_solid(&shifted, color)
    }

    /// Clear this window's pixels to `color`, matching the full-frame clear.
    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        self.fb.clear(color)
    }
}

/// Composite a transient overlay over the clean RGB222 framebuffer backdrop into `scratch`: the
/// one piece of the partial-update path that is byte-for-byte identical on every backend.
///
/// It fills `scratch[..w * rows]` with the framebuffer window expanded device-64 to RGB565, then
/// runs `draw_overlay` over it through a frame-absolute [`Band`]. `fb`, the resident
/// clean map, is never written, so the overlay costs no map re-render to clear again.
///
/// Panics if `scratch` is shorter than `window`'s area, or if the window runs past `frame`.
pub fn composite_overlay_window(
    fb: &[u8],
    frame: Size,
    window: Rectangle,
    scratch: &mut [u16],
    draw_overlay: &mut dyn FnMut(&mut Band),
) {
    let (x0, y0) = (window.top_left.x as u16, window.top_left.y as u16);
    let (w, rows) = (window.size.width as u16, window.size.height as u16);
    let (fw, x0u, y0u, wu) = (frame.width as usize, x0 as usize, y0 as usize, w as usize);
    for row in 0..rows as usize {
        let fb_base = (y0u + row) * fw + x0u;
        let dst = &mut scratch[row * wu..row * wu + wu];
        for (px, &byte) in dst.iter_mut().zip(&fb[fb_base..fb_base + wu]) {
            *px = device64_to_rgb565(byte);
        }
    }
    let mut band = Band::new(&mut scratch[..wu * rows as usize], frame, x0, y0, w, rows);
    draw_overlay(&mut band);
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::pixelcolor::raw::RawU16;

    fn rgb(raw: u16) -> Rgb565 {
        Rgb565::from(RawU16::new(raw))
    }

    /// A [`Band`] reports the whole frame but writes only its own sub-rect: a
    /// frame-absolute fill straddling the window's bounds lands offset by `(-x0, -y0)` and clips
    /// to the window.
    #[test]
    fn window_reports_full_frame_offsets_xy_and_clips() {
        // Frame 8×8; window covers cols [4,8) × rows [2,6) (x0=4,y0=2,w=4,rows=4), scratch = 4×4.
        let mut scratch = [0u16; 4 * 4];
        {
            let mut win = Band::new(&mut scratch, Size::new(8, 8), 4, 2, 4, 4);
            assert_eq!(win.size(), Size::new(8, 8), "reports the FULL frame, not the window");
            // Absolute cols 6..10 × rows 4..8 → only cols 6,7 / rows 4,5 fall in the window →
            // window-local cols 2,3 / rows 2,3.
            win.fill_solid(&Rectangle::new(Point::new(6, 4), Size::new(4, 4)), rgb(0xF800)).unwrap();
        }
        let at = |x: usize, y: usize| scratch[y * 4 + x];
        assert_eq!(at(1, 1), 0x0000, "outside the filled sub-rect → untouched");
        assert_eq!(at(2, 2), 0xF800, "frame (6,4) → window-local (2,2) filled");
        assert_eq!(at(3, 3), 0xF800, "frame (7,5) → window-local (3,3), last in-window pixel");
    }

    /// [`composite_overlay_window`] fills the scratch with the clean framebuffer window expanded
    /// to RGB565, lets the drawer paint over it in frame-absolute coords, and never touches `fb`.
    #[test]
    fn composite_overlay_reads_backdrop_then_draws_over_it() {
        // Frame 8×8; every fb pixel a distinct device-64 byte (its index, masked to 6 bits).
        let mut fb = [0u8; 8 * 8];
        for (i, px) in fb.iter_mut().enumerate() {
            *px = (i as u8) & 0b0011_1111;
        }
        let fb_snapshot = fb;
        let frame = Size::new(8, 8);
        // Window cols [4,8) × rows [2,6); the drawer paints frame-absolute (5,3) red.
        let mut scratch = [0u16; 4 * 4];
        let window = Rectangle::new(Point::new(4, 2), Size::new(4, 4));
        composite_overlay_window(&fb, frame, window, &mut scratch, &mut |band| {
            band.fill_solid(&Rectangle::new(Point::new(5, 3), Size::new(1, 1)), rgb(0xF800)).ok();
        });

        let at = |x: usize, y: usize| scratch[y * 4 + x];
        // Backdrop: window-local (0,0) = frame (4,2) = fb byte 2*8+4 = 20, expanded to RGB565.
        assert_eq!(at(0, 0), device64_to_rgb565(20), "backdrop = clean fb expanded to RGB565");
        assert_eq!(at(3, 3), device64_to_rgb565(5 * 8 + 7), "backdrop window corner = frame (7,5)");
        // Overlay: frame (5,3) → window-local (1,1) painted red over the backdrop.
        assert_eq!(at(1, 1), 0xF800, "drawer painted frame-absolute (5,3) into the window");
        assert_eq!(fb, fb_snapshot, "the clean framebuffer is never written");
    }
}
