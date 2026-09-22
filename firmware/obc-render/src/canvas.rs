//! A thin drawing surface over a [`DrawTarget`](embedded_graphics::draw_target::DrawTarget) and
//! the host `color_fn`, so screen layout code reads like a description of the screen instead of a
//! wall of `embedded-graphics` builders.
//!
//! The drawing vocabulary lives on the [`Surface`](crate::Surface) trait, which `Canvas` is the
//! one implementor of, so helpers take `&mut impl Surface` and never see the generics. The host
//! constructs one [`Canvas`] per frame.

use core::cell::Cell;

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
    /// Optional switch owned by the host's colour policy. Authored raster and map content turn
    /// it off while it uses the same target conversion, then UI chrome turns it back on.
    policy_enabled: Option<&'a Cell<bool>>,
    /// Region-scoped repaint bound, `None` on a normal full frame. When set, the
    /// [`Surface`](crate::Surface) impl rejects whole primitives whose bounds miss it before any
    /// rasterizing runs, which is what a pixel-level clip cannot skip. Rejection only, never pixel
    /// clipping: a primitive that touches the clip draws in full and the host's clipped
    /// framebuffer discards the out-of-region writes.
    clip: Option<Rectangle>,
}

impl<'a, D, F> Canvas<'a, D, F> {
    pub fn new(target: &'a mut D, color_fn: &'a F) -> Self {
        Canvas { target, color_fn, policy_enabled: None, clip: None }
    }

    /// Construct a canvas whose host colour policy can be bypassed for authored content.
    pub fn with_policy_switch(target: &'a mut D, color_fn: &'a F, policy_enabled: &'a Cell<bool>) -> Self {
        Canvas { target, color_fn, policy_enabled: Some(policy_enabled), clip: None }
    }

    /// Select whether the host's optional UI colour policy applies to subsequent draws.
    ///
    /// This does not change the target's required colour conversion. It only controls policy
    /// captured by `color_fn`, and is a no-op for canvases constructed with [`Canvas::new`].
    pub fn set_color_policy_enabled(&self, enabled: bool) {
        if let Some(switch) = self.policy_enabled {
            switch.set(enabled);
        }
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

    /// Split out the target, colour conversion, and optional host policy switch.
    pub fn split_with_policy_switch(&mut self) -> (&mut D, &F, Option<&Cell<bool>>) {
        (self.target, self.color_fn, self.policy_enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Surface;
    use embedded_graphics::{
        mock_display::MockDisplay,
        pixelcolor::{raw::RawU16, Rgb565},
    };

    #[test]
    fn authored_color_can_bypass_a_ui_policy_collision() {
        let enabled = Cell::new(true);
        let authored = Rgb565::new(3, 7, 3);
        let themed = Rgb565::new(28, 56, 28);
        let policy = |color| {
            if enabled.get() && color == authored.into_storage() {
                themed
            } else {
                Rgb565::from(RawU16::new(color))
            }
        };
        let mut target = MockDisplay::new();
        target.set_allow_overdraw(true);
        let mut canvas = Canvas::with_policy_switch(&mut target, &policy, &enabled);

        canvas.set_color_policy_enabled(false);
        canvas.fill(rect(0, 0, 1, 1), authored.into_storage());
        canvas.set_color_policy_enabled(true);
        canvas.fill(rect(1, 0, 1, 1), authored.into_storage());

        assert_eq!(target.get_pixel(Point::new(0, 0)), Some(authored));
        assert_eq!(target.get_pixel(Point::new(1, 0)), Some(themed));
    }
}
