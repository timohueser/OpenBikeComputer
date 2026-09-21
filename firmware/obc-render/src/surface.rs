//! [`Surface`]: the palette-565 drawing vocabulary every screen draws with.
//!
//! Screen layout code needs "fill, round, text, triangle, disc in a palette RGB565", not the raw
//! [`DrawTarget`] and the host colour policy. This trait is that vocabulary, and the [`Canvas`]
//! impl is the single place the `D: DrawTarget, F: Fn(u16) -> D::Color` bound lives, so a drawing
//! helper takes one `impl Surface` instead of threading a `(target, color_fn)` pair.
//!
//! Every colour argument is a palette RGB565, resolved through the host `color_fn`. Draw errors
//! are swallowed: host targets are infallible and a real display cannot recover mid-frame.

use embedded_graphics::{
    prelude::*,
    primitives::{Circle, Line, PointsIter, PrimitiveStyle, Rectangle, RoundedRectangle, Triangle},
};

use crate::canvas::{rect, Canvas};
use crate::text::{draw_text, draw_text_ccw, text_width, Font, TextAlign};

/// The bounding box of a point set — for the primitives given as vertices (line, triangle).
fn points_bbox(pts: &[Point]) -> Rectangle {
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for p in pts {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    rect(x0, y0, x1 - x0 + 1, y1 - y0 + 1)
}

/// A drawing surface in the palette-565 vocabulary. [`Canvas`] is the one implementor.
pub trait Surface {
    fn clear(&mut self, color: u16);

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

    fn triangle(&mut self, a: Point, b: Point, c: Point, color: u16);

    /// A filled circle of `radius` centered at `center`.
    fn disc(&mut self, center: Point, radius: u32, color: u16);

    /// Text anchored at `at`, aligned `align`, top baseline. Returns the position past the string.
    fn text(&mut self, s: &str, at: Point, font: Font, align: TextAlign, color: u16) -> Point;

    /// Draw text counter-clockwise from a bottom-left anchor. `divisor = 2` halves both glyph
    /// dimensions while retaining any lit source pixel in each 2×2 block.
    fn text_ccw(&mut self, s: &str, bottom_left: Point, font: Font, divisor: u32, color: u16) {
        // A recording surface that does not care about orientation still sees the text event.
        let _ = divisor;
        self.text(s, bottom_left, font, TextAlign::Left, color);
    }

    /// [`text`](Surface::text) vertically centred in the `v_span = (top, height)` span: the capital
    /// ink is centred after subtracting the font's top bearing.
    fn text_vcentered(
        &mut self,
        s: &str,
        x: i32,
        v_span: (i32, i32),
        font: Font,
        align: TextAlign,
        color: u16,
    ) -> Point {
        let (top, h) = v_span;
        let y = top + (h - font.cap_height() as i32) / 2 - font.cap_top() as i32;
        self.text(s, Point::new(x, y), font, align, color)
    }
}

impl<D, F> Surface for Canvas<'_, D, F>
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    fn clear(&mut self, color: u16) {
        // Never rejected: on a clipped repaint the target's own clip bounds the cost, and the clip
        // region must still be cleared for the replayed draw on top of it.
        let (target, c) = self.split();
        let _ = target.clear(c(color));
    }

    fn fill(&mut self, area: Rectangle, color: u16) {
        if self.rejects(&area) {
            return;
        }
        let (target, c) = self.split();
        let _ = target.fill_solid(&area, c(color));
    }

    fn round(&mut self, area: Rectangle, radius: u32, color: u16) {
        if self.rejects(&area) {
            return;
        }
        let (target, c) = self.split();
        let style = PrimitiveStyle::with_fill(c(color));
        let _ = RoundedRectangle::with_equal_corners(area, Size::new(radius, radius)).into_styled(style).draw(target);
    }

    fn round_outline(&mut self, area: Rectangle, radius: u32, color: u16) {
        // The 1 px stroke stays within `radius + 1` of the boundary, so a clip wholly inside that
        // ring's hole can also skip it. The full-frame outline every framed screen draws would
        // otherwise never reject.
        let inset = radius as i32 + 1;
        let hole = rect(
            area.top_left.x + inset,
            area.top_left.y + inset,
            area.size.width as i32 - 2 * inset,
            area.size.height as i32 - 2 * inset,
        );
        if self.rejects(&area) || self.rejects_ring(&hole) {
            return;
        }
        let (target, c) = self.split();
        let style = PrimitiveStyle::with_stroke(c(color), 1);
        let _ = RoundedRectangle::with_equal_corners(area, Size::new(radius, radius)).into_styled(style).draw(target);
    }

    /// Draws the bare Bresenham pixel stream, not a styled 1 px stroke: the Home contour emits
    /// thousands of tiny segments per frame, and the styled path rebuilds its thick-line machinery
    /// on every one.
    fn line(&mut self, a: Point, b: Point, color: u16) {
        if self.rejects(&points_bbox(&[a, b])) {
            return;
        }
        let (target, c) = self.split();
        let color = c(color);
        let _ = target.draw_iter(Line::new(a, b).points().map(|p| Pixel(p, color)));
    }

    fn triangle(&mut self, a: Point, b: Point, c: Point, color: u16) {
        if self.rejects(&points_bbox(&[a, b, c])) {
            return;
        }
        let (target, cf) = self.split();
        let style = PrimitiveStyle::with_fill(cf(color));
        let _ = Triangle::new(a, b, c).into_styled(style).draw(target);
    }

    fn disc(&mut self, center: Point, radius: u32, color: u16) {
        let r = radius as i32;
        if self.rejects(&rect(center.x - r, center.y - r, 2 * r + 1, 2 * r + 1)) {
            return;
        }
        let (target, c) = self.split();
        let style = PrimitiveStyle::with_fill(c(color));
        let top_left = Point::new(center.x - radius as i32, center.y - radius as i32);
        let _ = Circle::new(top_left, radius * 2 + 1).into_styled(style).draw(target);
    }

    fn text(&mut self, s: &str, at: Point, font: Font, align: TextAlign, color: u16) -> Point {
        // The glyph cell box is exact for the monospace face, so a string outside it decodes no
        // glyphs at all. A multi-line string would break the single-cell-row math, so those always
        // draw; no screen passes one. The rejected return is `at`, `draw_text`'s own fallback.
        if !s.contains('\n') {
            let w = text_width(s, font) as i32;
            let x0 = match align {
                TextAlign::Left => at.x,
                TextAlign::Center => at.x - w / 2,
                TextAlign::Right => at.x - w,
            };
            if self.rejects(&rect(x0, at.y, w, font.line_height() as i32)) {
                return at;
            }
        }
        let (target, c) = self.split();
        draw_text(target, s, at, font, align, c(color))
    }

    fn text_ccw(&mut self, s: &str, bottom_left: Point, font: Font, divisor: u32, color: u16) {
        let divisor = divisor.max(1);
        let width = text_width(s, font).div_ceil(divisor) as i32;
        let height = font.line_height().div_ceil(divisor) as i32;
        if self.rejects(&rect(bottom_left.x, bottom_left.y - width, height, width)) {
            return;
        }
        let (target, c) = self.split();
        draw_text_ccw(target, s, bottom_left, font, divisor, c(color));
    }
}
