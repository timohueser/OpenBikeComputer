//! A thin drawing surface over a [`DrawTarget`](embedded_graphics::draw_target::DrawTarget) and
//! the host `color_fn`, so screen layout code reads like a description of the screen instead of a
//! wall of `embedded-graphics` builders.
//!
//! The drawing vocabulary lives on the [`Surface`](crate::Surface) trait, which `Canvas` is the
//! one implementor of, so helpers take `&mut impl Surface` and never see the generics. The host
//! constructs one [`Canvas`] per frame.

use embedded_graphics::{prelude::*, primitives::Rectangle};

/// A [`Rectangle`] from top-left `(x, y)` and size `(w, h)`. Negative sizes clamp to 0.
#[inline]
pub fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle {
    Rectangle::new(Point::new(x, y), Size::new(w.max(0) as u32, h.max(0) as u32))
}

/// A drawing surface bundling a target with the host colour policy, the one implementor of
/// [`Surface`](crate::Surface). Every colour argument is a palette RGB565.
pub struct Canvas<'a, D, F> {
    target: &'a mut D,
    color_fn: &'a F,
    /// Region-scoped repaint bound, `None` on a normal full frame. When set, the
    /// [`Surface`](crate::Surface) impl rejects whole primitives whose bounds miss it before any
    /// rasterizing runs, which is what a pixel-level clip cannot skip. Rejection only, never pixel
    /// clipping: a primitive that touches the clip draws in full and the host's clipped
    /// framebuffer discards the out-of-region writes.
    clip: Option<Rectangle>,
}

impl<'a, D, F> Canvas<'a, D, F> {
    pub fn new(target: &'a mut D, color_fn: &'a F) -> Self {
        Canvas { target, color_fn, clip: None }
    }

    /// Set or clear the region-scoped repaint bound for the draws that follow. The caller owns the
    /// contract that everything it wants changed lies inside `clip`.
    pub fn set_clip(&mut self, clip: Option<Rectangle>) {
        self.clip = clip;
    }

    /// Whether a primitive with bounding box `bbox` can be skipped outright. `false` on a full
    /// frame, so the normal path draws everything.
    #[inline]
    pub(crate) fn rejects(&self, bbox: &Rectangle) -> bool {
        self.clip.is_some_and(|c| bbox.intersection(&c).is_zero_sized())
    }

    /// Ring-aware rejection for hollow primitives: skip when the clip lies entirely inside `hole`,
    /// the largest rectangle the stroke can never enter. A full-frame outline has a whole-screen
    /// bbox, so [`rejects`](Canvas::rejects) keeps it, yet its ring cannot touch an interior clip.
    #[inline]
    pub(crate) fn rejects_ring(&self, hole: &Rectangle) -> bool {
        self.clip.is_some_and(|c| hole.intersection(&c) == c)
    }

    /// The raw-target escape hatch, for the one consumer that must hand the target and colour
    /// policy to [`RenderScratch`](crate::RenderScratch) directly.
    pub fn split(&mut self) -> (&mut D, &F) {
        (self.target, self.color_fn)
    }
}
