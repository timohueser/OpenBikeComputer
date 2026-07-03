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
}

impl<'a, D, F> Canvas<'a, D, F> {
    pub fn new(target: &'a mut D, color_fn: &'a F) -> Self {
        Canvas { target, color_fn }
    }

    /// The raw-target escape hatch: the underlying target + colour policy, for the one consumer
    /// (the Map screen) that must hand them to [`MapRenderer`](crate::MapRenderer) directly. Every
    /// other caller draws through [`Surface`](crate::Surface) and never needs this.
    pub fn split(&mut self) -> (&mut D, &F) {
        (self.target, self.color_fn)
    }
}
