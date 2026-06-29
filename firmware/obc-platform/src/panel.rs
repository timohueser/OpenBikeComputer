//! The board-agnostic [`Panel`] display seam + the [`Band`] draw helper.
//!
//! A display with a hardware scan-out engine (an LCD controller that continuously rescans a full
//! framebuffer from memory) needs only a [`Framebuffer565`] `DrawTarget` — write a pixel and it's
//! on glass. The boards this project ships on have no such luxury: the nRF54L has no external RAM
//! (256 KB total) and no scan-out engine, so a frame is pushed to the panel a **band** (a few
//! rows) at a time over SPI/DMA. [`Panel`] is the seam that hides that difference: the caller
//! renders into a small RGB565 band the backend hands it, and the backend reformats + transports
//! the band however its panel wants (ST7789: byte-swap to big-endian RGB565 + SPIM-DMA a
//! CASET/RASET window; the FLPR/LS021B7DD02: pack RGB222 → 6-line wire bytes; the simulator: blit
//! the band into its image). **No board/panel types appear in the trait**, so the same generator
//! drives every backend.
//!
//! [`Band`] is the small piece that makes "render the whole frame, band at a time" invisible to
//! the drawing code: it wraps one `flush_band` scratch slice as a [`Framebuffer565`] with the
//! band's `y0` baked in, yet reports the **full frame** size. A whole-frame generator
//! ([`App::render_frame`](obc_app::App::render_frame)) draws in absolute
//! frame coordinates exactly as it would against a full-frame scan-out plane; [`Band`] shifts each draw up
//! by `y0` and the inner framebuffer clips away whatever falls outside this band's rows — so the
//! frame reassembles seam-free across the bands with the generator none the wiser.

use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};

use crate::framebuffer::{device64_to_rgb565, Framebuffer565};

/// A banded display, pushed a few rows at a time. The board-agnostic seam between a whole-frame
/// generator and a concrete panel's wire format + transport — the banded analog of a full-frame
/// scan-out plane, for boards that have to stream the frame themselves.
///
/// One frame is: [`begin_frame`](Self::begin_frame), then [`flush_band`](Self::flush_band) for
/// each band of [`band_rows`](Self::band_rows) rows from `y0 = 0` upward, then
/// [`end_frame`](Self::end_frame). The caller fills the RGB565 band the backend passes to
/// `fill`; the backend owns how that band reaches glass. Wrap the band in a [`Band`] to draw it
/// in frame-absolute coordinates.
pub trait Panel {
    /// Rows per band — the backend's choice, sized to its scratch buffer. The frame height need
    /// not be a multiple of it; the last band is whatever rows remain (passed as `rows`).
    fn band_rows(&self) -> u16;

    /// Start a frame (e.g. reset internal band state). May be a no-op.
    fn begin_frame(&mut self);

    /// Render + transport one band. `fill` receives this band's RGB565 scratch — exactly
    /// `width * rows` pixels, row-major — to draw into; on return the backend reformats it to
    /// the panel's wire format and pushes it to rows `[y0, y0 + rows)`. `rows ≤ band_rows()`.
    fn flush_band(&mut self, y0: u16, rows: u16, fill: impl FnOnce(&mut [u16]));

    /// Finish a frame (e.g. latch / VCOM toggle / present). May be a no-op.
    fn end_frame(&mut self);
}

/// A frame-absolute [`DrawTarget`] view of one [`Panel::flush_band`] scratch band — or, more
/// generally, of any rectangular **window** of the frame.
///
/// Wraps the scratch slice as a [`Framebuffer565`] sized to just this window (`w × rows`), but
/// **reports the full frame size** ([`OriginDimensions`]) and **offsets every draw by `(-x0, -y0)`**.
/// So a generator that lays out the whole 240×320 frame — reading `target.bounding_box()` for its
/// dimensions, drawing at absolute `(x, y)` — lands its pixels for this window in the scratch and
/// has everything else clipped away by the inner framebuffer. Drawing the frame once per band thus
/// reassembles it seam-free, and the *same* generator works against a full-frame scan-out plane
/// (where there is one band == the whole frame).
///
/// A **full-width band** ([`new`](Band::new), `x0 = 0`, `w = frame.width`) is the common case used
/// by [`Panel::flush_band`]. A **narrow window** ([`new_window`](Band::new_window)) lets a banded
/// backend re-push just a sub-rectangle — the nRF's composite-on-push hold bulge re-fills only the
/// right-edge columns it touches (issue #126), reusing the same scratch + clip path.
///
/// Reuses [`framebuffer::RawFb`](crate::framebuffer::RawFb)/`Pack`, so a band pixel travels the
/// exact same pack + clip path as a full-frame-plane pixel.
pub struct Band<'a> {
    /// The window's own RGB565 buffer, `w × rows`. Draws land here in window-local coords.
    fb: Framebuffer565<'a>,
    /// This window's first frame column — subtracted from every incoming `x` so absolute coords
    /// map into the window (columns outside `[0, w)` clip in `fb`).
    x0: i32,
    /// This window's first frame row — subtracted from every incoming `y` (rows outside `[0, rows)`
    /// clip in `fb`).
    y0: i32,
    /// The full frame size, reported to the generator so its layout spans the whole panel.
    frame: Size,
}

impl<'a> Band<'a> {
    /// View `scratch` (this band's `frame.width × rows` RGB565 pixels) as the full-width frame band
    /// at `y0`. Panics if `scratch` is shorter than `frame.width * rows` (a backend wiring bug).
    pub fn new(scratch: &'a mut [u16], frame: Size, y0: u16, rows: u16) -> Self {
        Self::new_window(scratch, frame, 0, y0, frame.width as u16, rows)
    }

    /// View `scratch` (`w × rows` RGB565 pixels) as the frame window at `(x0, y0)`, sized `w × rows`,
    /// reporting the full `frame`. Frame-absolute draws land offset by `(-x0, -y0)` and anything
    /// outside the window clips. Panics if `scratch` is shorter than `w * rows`.
    pub fn new_window(scratch: &'a mut [u16], frame: Size, x0: u16, y0: u16, w: u16, rows: u16) -> Self {
        Self { fb: Framebuffer565::new(scratch, w as u32, rows as u32), x0: x0 as i32, y0: y0 as i32, frame }
    }
}

impl OriginDimensions for Band<'_> {
    /// The full frame — so a generator sizing itself off `bounding_box()` lays out the whole
    /// panel, not just this window.
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

    /// Offset the fill rectangle into window-local space; the inner framebuffer intersects it with
    /// the window's bounds, so a rect spanning rows/columns outside this window fills only its slice
    /// (this is the renderer's hot path — a per-row `fill`, kept fast by forwarding straight to
    /// `RawFb`).
    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let shifted = Rectangle::new(Point::new(area.top_left.x - self.x0, area.top_left.y - self.y0), area.size);
        self.fb.fill_solid(&shifted, color)
    }

    /// Clear *this window's* pixels to `color`. The generator calls it once per frame (per band);
    /// the pixels it doesn't subsequently draw stay this colour, matching the full-frame clear.
    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        self.fb.clear(color)
    }
}

/// Composite a transient overlay over the **clean RGB222 framebuffer backdrop** into `scratch`
/// (issue #163) — the one piece of the partial-update path that is byte-for-byte the same on every
/// banded backend, so it lives here beside [`Band`] rather than being re-implemented per panel.
///
/// Fills `scratch[..w * rows]` with the framebuffer window `[x0, x0+w) × [y0, y0+rows)` expanded
/// device-64 → RGB565 ([`device64_to_rgb565`]), then runs `draw_overlay` over it through a
/// frame-absolute [`Band::new_window`] — so the drawer paints the chrome (the hold bulge) in whole-
/// frame coordinates and anything outside the window clips. On return `scratch` holds the composited
/// region; `fb` (the resident clean map, the source of truth) is **never** written, so the overlay
/// costs no map re-render to clear again.
///
/// Backend-agnostic: a column-addressable panel (ST7789) DMAs `scratch` straight to its `window`; a
/// row-addressed panel (LS021/FLPR) re-quantizes `scratch` back into its full-width dirty rows. The
/// only per-panel code left is that final wire-pack.
///
/// `fb` is the resident device-64 (`0b00_RR_GG_BB`) plane, `frame` its full size, `window` the dirty
/// rectangle within it. Panics if `scratch` is shorter than `window`'s area or the window runs past
/// `frame` (a caller wiring bug).
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
    let mut band = Band::new_window(&mut scratch[..wu * rows as usize], frame, x0, y0, w, rows);
    draw_overlay(&mut band);
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::pixelcolor::raw::RawU16;

    fn rgb(raw: u16) -> Rgb565 {
        Rgb565::from(RawU16::new(raw))
    }

    /// A `Band` reports the whole frame but writes only its own rows: a frame-absolute fill that
    /// straddles the band's top edge lands offset by `-y0` and is clipped to the band.
    #[test]
    fn band_reports_full_frame_offsets_and_clips() {
        // Frame 4×8; band covers rows [4, 8) (y0=4, rows=4), scratch = 4×4.
        let mut scratch = [0u16; 4 * 4];
        {
            let mut band = Band::new(&mut scratch, Size::new(4, 8), 4, 4);
            assert_eq!(band.size(), Size::new(4, 8), "reports the FULL frame, not the band");
            // Absolute rows 2..6 → only frame rows 4,5 fall in this band → band-local rows 0,1.
            band.fill_solid(&Rectangle::new(Point::new(0, 2), Size::new(4, 4)), rgb(0x07E0)).unwrap();
        }
        assert!(scratch[0..8].iter().all(|&p| p == 0x07E0), "frame rows 4-5 filled (band-local 0-1)");
        assert!(scratch[8..16].iter().all(|&p| p == 0x0000), "frame rows 6-7 untouched (rect ended at 5)");
    }

    /// A narrow [`Band::new_window`] reports the whole frame but writes only its own sub-rect: a
    /// frame-absolute fill straddling the window's bounds lands offset by `(-x0, -y0)` and clips to
    /// the window — the path the nRF composite-on-push bulge re-pushes its right-edge columns by.
    #[test]
    fn window_reports_full_frame_offsets_xy_and_clips() {
        // Frame 8×8; window covers cols [4,8) × rows [2,6) (x0=4,y0=2,w=4,rows=4), scratch = 4×4.
        let mut scratch = [0u16; 4 * 4];
        {
            let mut win = Band::new_window(&mut scratch, Size::new(8, 8), 4, 2, 4, 4);
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

    /// The load-bearing property: drawing a whole-frame generator band-by-band reconstructs the
    /// exact image a single full-frame draw produces — i.e. no band seams. Renders the same
    /// generator into a 4×8 full framebuffer and into two 4-row bands, and asserts byte-equality.
    #[test]
    fn bands_reconstruct_full_frame() {
        const W: u32 = 4;
        const H: u32 = 8;

        // A whole-frame generator that sizes off the target and draws in absolute coords: clear,
        // then paint each row a distinct colour — so a y-offset bug or a seam shows as a mismatch.
        fn gen<D: DrawTarget<Color = Rgb565>>(t: &mut D) {
            t.clear(rgb(0x1234)).ok();
            let size = t.bounding_box().size;
            for y in 0..size.height as i32 {
                let c = rgb((y as u16 + 1) * 0x0101);
                t.fill_solid(&Rectangle::new(Point::new(0, y), Size::new(size.width, 1)), c).ok();
            }
        }

        let mut full = [0u16; (W * H) as usize];
        gen(&mut Framebuffer565::new(&mut full, W, H));

        let mut assembled = [0u16; (W * H) as usize];
        let mut scratch = [0u16; (W * 4) as usize]; // one 4-row band at a time
        let rows = 4u16;
        let mut y0 = 0u16;
        while (y0 as u32) < H {
            let r = rows.min(H as u16 - y0);
            let n = (W * r as u32) as usize;
            {
                let mut band = Band::new(&mut scratch[..n], Size::new(W, H), y0, r);
                gen(&mut band);
            }
            let start = (y0 as u32 * W) as usize;
            assembled[start..start + n].copy_from_slice(&scratch[..n]);
            y0 += r;
        }

        assert_eq!(full, assembled, "banded render must be byte-identical to the full-frame render");
    }

    /// [`composite_overlay_window`] fills the scratch with the clean framebuffer window expanded to
    /// RGB565, then lets the drawer paint over it in frame-absolute coords — and never touches `fb`.
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
