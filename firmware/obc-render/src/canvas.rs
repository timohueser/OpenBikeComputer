//! A thin drawing surface over a [`DrawTarget`] + the host `color_fn`, so screen
//! layout code reads like a description of the screen instead of a wall of
//! `embedded-graphics` builders.
//!
//! Every method takes a palette **RGB565** and resolves it through `color_fn`
//! (quantizing to the panel exactly like the map and [`draw_text`] do), and
//! rectangles are written as plain `(x, y, w, h)` via [`rect`]. Construct one
//! [`Canvas`] per `draw` call:
//!
//! ```ignore
//! let mut cv = Canvas::new(target, color_fn);
//! cv.clear(palette::HUD);
//! cv.round(rect(8, 8, w - 16, h - 16), 6, palette::PARCHMENT);
//! cv.text("MENU", Point::new(w / 2, 20), Font::Body, TextAlign::Center, palette::INK);
//! ```

use embedded_graphics::{
    prelude::*,
    primitives::{Circle, Line, PointsIter, PrimitiveStyle, Rectangle, RoundedRectangle, Triangle},
};

use crate::text::{draw_text, Font, TextAlign};

/// A [`Rectangle`] from top-left `(x, y)` and size `(w, h)` — terser than
/// `Rectangle::new(Point::new(x, y), Size::new(w, h))`. Negative sizes clamp to 0.
#[inline]
pub fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle {
    Rectangle::new(Point::new(x, y), Size::new(w.max(0) as u32, h.max(0) as u32))
}

/// A drawing surface bundling a target with the host color policy. Every color
/// argument is a palette RGB565 resolved through `color_fn`. Draw errors are
/// swallowed (host targets are infallible; a real display can't recover mid-frame
/// anyway), matching the rest of the renderer.
pub struct Canvas<'a, D, F> {
    target: &'a mut D,
    color_fn: &'a F,
}

impl<'a, D, F> Canvas<'a, D, F>
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    pub fn new(target: &'a mut D, color_fn: &'a F) -> Self {
        Canvas { target, color_fn }
    }

    #[inline]
    fn c(&self, rgb565: u16) -> D::Color {
        (self.color_fn)(rgb565)
    }

    /// Clear the whole target to `color`.
    pub fn clear(&mut self, color: u16) {
        let _ = self.target.clear(self.c(color));
    }

    /// Fill a rectangle.
    pub fn fill(&mut self, area: Rectangle, color: u16) {
        let _ = self.target.fill_solid(&area, self.c(color));
    }

    /// Fill a rounded rectangle (equal corner radius).
    pub fn round(&mut self, area: Rectangle, radius: u32, color: u16) {
        let style = PrimitiveStyle::with_fill(self.c(color));
        let _ =
            RoundedRectangle::with_equal_corners(area, Size::new(radius, radius)).into_styled(style).draw(self.target);
    }

    /// Draw the 1px outline of a rounded rectangle.
    pub fn round_outline(&mut self, area: Rectangle, radius: u32, color: u16) {
        let style = PrimitiveStyle::with_stroke(self.c(color), 1);
        let _ =
            RoundedRectangle::with_equal_corners(area, Size::new(radius, radius)).into_styled(style).draw(self.target);
    }

    /// A horizontal hairline `len` px wide starting at `(x, y)`.
    pub fn hline(&mut self, x: i32, y: i32, len: i32, color: u16) {
        self.fill(rect(x, y, len, 1), color);
    }

    /// A vertical hairline `len` px tall starting at `(x, y)` — e.g. a cursor / marker
    /// line. `w` widens it to a solid bar.
    pub fn vline(&mut self, x: i32, y: i32, len: i32, w: i32, color: u16) {
        self.fill(rect(x, y, w.max(1), len), color);
    }

    /// A 1px straight line between two points — e.g. one marching-squares contour
    /// segment on the Home background.
    ///
    /// Draws the bare Bresenham pixel stream (`points()`), **not** a styled 1px stroke: the Home
    /// contour emits *thousands* of tiny segments per frame, and the styled-stroke path rebuilds
    /// its thick-line (perpendicular-extent) machinery on every one — pure per-segment overhead at
    /// width 1. Walking the points straight into `draw_iter` skips all of it.
    pub fn line(&mut self, a: Point, b: Point, color: u16) {
        let color = self.c(color);
        let _ = self.target.draw_iter(Line::new(a, b).points().map(|p| Pixel(p, color)));
    }

    /// A filled triangle (e.g. a list pointer bullet).
    pub fn triangle(&mut self, a: Point, b: Point, c: Point, color: u16) {
        let style = PrimitiveStyle::with_fill(self.c(color));
        let _ = Triangle::new(a, b, c).into_styled(style).draw(self.target);
    }

    /// A filled circle of `radius` centered at `center` — e.g. a position dot.
    pub fn disc(&mut self, center: Point, radius: u32, color: u16) {
        let style = PrimitiveStyle::with_fill(self.c(color));
        let top_left = Point::new(center.x - radius as i32, center.y - radius as i32);
        let _ = Circle::new(top_left, radius * 2 + 1).into_styled(style).draw(self.target);
    }

    /// Text anchored at `at`, aligned `align`, top baseline. Returns the position
    /// just past the string (see [`draw_text`]).
    pub fn text(&mut self, s: &str, at: Point, font: Font, align: TextAlign, color: u16) -> Point {
        draw_text(self.target, s, at, font, align, self.c(color))
    }
}
