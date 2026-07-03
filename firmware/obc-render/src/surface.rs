//! [`Surface`] — the palette-565 drawing vocabulary every screen draws with.
//!
//! Screen layout code never needs the raw [`DrawTarget`] or the host colour policy; it needs
//! "fill / round / text / triangle / disc in a palette RGB565". This trait is that vocabulary, and
//! the [`Canvas`] impl below is the **single** place the
//! `D: DrawTarget, F: Fn(u16) -> D::Color` bound lives — so a drawing helper is
//! `fn f(cv: &mut impl Surface, ...)`: one bound, no `where` clause, no `(target, color_fn)` pair
//! threading.
//!
//! Every colour argument is a palette **RGB565**, resolved through the host `color_fn` (quantizing
//! to the panel exactly like the map and [`draw_text`] do). Draw errors are swallowed (host targets
//! are infallible; a real display can't recover mid-frame anyway), matching the rest of the renderer.

use embedded_graphics::{
    prelude::*,
    primitives::{Circle, Line, PointsIter, PrimitiveStyle, Rectangle, RoundedRectangle, Triangle},
};

use crate::canvas::{rect, Canvas};
use crate::text::{draw_text, Font, TextAlign};

/// A drawing surface in the palette-565 vocabulary. See the [module docs](self) for the contract;
/// [`Canvas`] is the one implementor.
pub trait Surface {
    /// Clear the whole target to `color`.
    fn clear(&mut self, color: u16);

    /// Fill a rectangle.
    fn fill(&mut self, area: Rectangle, color: u16);

    /// Fill a rounded rectangle (equal corner radius).
    fn round(&mut self, area: Rectangle, radius: u32, color: u16);

    /// Draw the 1px outline of a rounded rectangle.
    fn round_outline(&mut self, area: Rectangle, radius: u32, color: u16);

    /// A horizontal hairline `len` px wide starting at `(x, y)`.
    fn hline(&mut self, x: i32, y: i32, len: i32, color: u16) {
        self.fill(rect(x, y, len, 1), color);
    }

    /// A vertical hairline `len` px tall starting at `(x, y)`; `w` widens it to a solid bar.
    fn vline(&mut self, x: i32, y: i32, len: i32, w: i32, color: u16) {
        self.fill(rect(x, y, w.max(1), len), color);
    }

    /// A 1px straight line between two points.
    fn line(&mut self, a: Point, b: Point, color: u16);

    /// A filled triangle.
    fn triangle(&mut self, a: Point, b: Point, c: Point, color: u16);

    /// A filled circle of `radius` centered at `center`.
    fn disc(&mut self, center: Point, radius: u32, color: u16);

    /// Text anchored at `at`, aligned `align`, top baseline. Returns the position
    /// just past the string (see [`draw_text`]).
    fn text(&mut self, s: &str, at: Point, font: Font, align: TextAlign, color: u16) -> Point;
}

impl<D, F> Surface for Canvas<'_, D, F>
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    fn clear(&mut self, color: u16) {
        let (target, c) = self.split();
        let _ = target.clear(c(color));
    }

    fn fill(&mut self, area: Rectangle, color: u16) {
        let (target, c) = self.split();
        let _ = target.fill_solid(&area, c(color));
    }

    fn round(&mut self, area: Rectangle, radius: u32, color: u16) {
        let (target, c) = self.split();
        let style = PrimitiveStyle::with_fill(c(color));
        let _ = RoundedRectangle::with_equal_corners(area, Size::new(radius, radius)).into_styled(style).draw(target);
    }

    fn round_outline(&mut self, area: Rectangle, radius: u32, color: u16) {
        let (target, c) = self.split();
        let style = PrimitiveStyle::with_stroke(c(color), 1);
        let _ = RoundedRectangle::with_equal_corners(area, Size::new(radius, radius)).into_styled(style).draw(target);
    }

    /// Draws the bare Bresenham pixel stream (`points()`), **not** a styled 1px stroke: the Home
    /// contour emits thousands of tiny segments per frame, and the styled-stroke path rebuilds its
    /// thick-line machinery on every one — pure per-segment overhead at width 1.
    fn line(&mut self, a: Point, b: Point, color: u16) {
        let (target, c) = self.split();
        let color = c(color);
        let _ = target.draw_iter(Line::new(a, b).points().map(|p| Pixel(p, color)));
    }

    fn triangle(&mut self, a: Point, b: Point, c: Point, color: u16) {
        let (target, cf) = self.split();
        let style = PrimitiveStyle::with_fill(cf(color));
        let _ = Triangle::new(a, b, c).into_styled(style).draw(target);
    }

    fn disc(&mut self, center: Point, radius: u32, color: u16) {
        let (target, c) = self.split();
        let style = PrimitiveStyle::with_fill(c(color));
        let top_left = Point::new(center.x - radius as i32, center.y - radius as i32);
        let _ = Circle::new(top_left, radius * 2 + 1).into_styled(style).draw(target);
    }

    fn text(&mut self, s: &str, at: Point, font: Font, align: TextAlign, color: u16) -> Point {
        let (target, c) = self.split();
        draw_text(target, s, at, font, align, c(color))
    }
}
