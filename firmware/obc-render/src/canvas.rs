//! A thin drawing surface over a [`DrawTarget`](embedded_graphics::draw_target::DrawTarget) + the
//! host `color_fn`, so screen layout code reads like a description of the screen instead of a wall
//! of `embedded-graphics` builders.
//!
//! The drawing vocabulary lives on the [`Surface`](crate::Surface) trait (which `Canvas` is the one
//! implementor of); helpers take `&mut impl Surface` and never see the target/colour generics.
//! Rectangles are written as plain `(x, y, w, h)` via [`rect`]. The host constructs one [`Canvas`]
//! per frame:
//!
//! ```ignore
//! let mut cv = Canvas::new(target, color_fn);
//! cv.clear(palette::HUD);
//! cv.round(rect(8, 8, w - 16, h - 16), 6, palette::PARCHMENT);
//! cv.text("MENU", Point::new(w / 2, 20), Font::Body, TextAlign::Center, palette::INK);
//! ```

use embedded_graphics::{prelude::*, primitives::Rectangle};

/// A [`Rectangle`] from top-left `(x, y)` and size `(w, h)` — terser than
/// `Rectangle::new(Point::new(x, y), Size::new(w, h))`. Negative sizes clamp to 0.
#[inline]
pub fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle {
    Rectangle::new(Point::new(x, y), Size::new(w.max(0) as u32, h.max(0) as u32))
}

/// A drawing surface bundling a target with the host color policy — the one implementor of
/// [`Surface`](crate::Surface). Every color argument is a palette RGB565 resolved through
/// `color_fn`.
pub struct Canvas<'a, D, F> {
    target: &'a mut D,
    color_fn: &'a F,
    /// Region-scoped repaint bound (#500 follow-up), `None` on a normal full frame. When set, the
    /// [`Surface`](crate::Surface) impl **rejects whole primitives whose bounds miss it** before
    /// any rasterizing runs — the per-pixel machinery (glyph decode, scanline iterators) is what a
    /// pixel-level clip can't skip. Rejection only, never pixel clipping: a primitive that
    /// *touches* the clip draws in full, and the host's clipped framebuffer discards the
    /// out-of-region writes — so inside the clip the output is byte-identical to a full draw.
    clip: Option<Rectangle>,
}

impl<'a, D, F> Canvas<'a, D, F> {
    pub fn new(target: &'a mut D, color_fn: &'a F) -> Self {
        Canvas { target, color_fn, clip: None }
    }

    /// Set (or clear) the region-scoped repaint bound for the draws that follow — see the field
    /// doc. The caller owns the correctness contract: everything it wants changed lies inside
    /// `clip`, and the target discards writes outside it.
    pub fn set_clip(&mut self, clip: Option<Rectangle>) {
        self.clip = clip;
    }

    /// Whether a primitive with bounding box `bbox` can be skipped outright: a clip is set and
    /// `bbox` doesn't touch it. `false` on a full frame, so the normal path draws everything.
    #[inline]
    pub(crate) fn rejects(&self, bbox: &Rectangle) -> bool {
        self.clip.is_some_and(|c| bbox.intersection(&c).is_zero_sized())
    }

    /// Ring-aware rejection for hollow primitives (the 1 px outlines): skip when the clip lies
    /// entirely inside `hole` — the largest rectangle the primitive's stroke can never enter.
    /// The full-frame outline every framed screen draws has a whole-screen bbox (so
    /// [`rejects`](Canvas::rejects) keeps it), yet its ring can't touch an interior region clip.
    #[inline]
    pub(crate) fn rejects_ring(&self, hole: &Rectangle) -> bool {
        self.clip.is_some_and(|c| hole.intersection(&c) == c)
    }

    /// The raw-target escape hatch: the underlying target + colour policy, for the one consumer
    /// (the Map screen) that must hand them to [`RenderScratch`](crate::RenderScratch) directly. Every
    /// other caller draws through [`Surface`](crate::Surface) and never needs this.
    pub fn split(&mut self) -> (&mut D, &F) {
        (self.target, self.color_fn)
    }
}
