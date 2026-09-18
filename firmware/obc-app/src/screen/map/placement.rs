//! Screen-space occupancy for map point marks.
//!
//! Reserved chrome goes in first. Each mark is then accepted only when its box, grown by the
//! caller's margin, is clear of everything already placed. The order of the calls is the priority
//! rule: the caller offers its marks best-first, and a mark that does not fit is dropped, never
//! moved. There are no leader lines, no alternative positions around an anchor and no second pass.
//!
//! The arithmetic is integer only, with the right and bottom edges taken in `i64` so a box near
//! `i32::MAX` cannot wrap. Nothing is allocated, so the same camera always gives the same frame.

// The point icon and settlement label overlays are the callers. The helper lands on its own, so
// both go through one collision path instead of two.
#![allow(dead_code)]

use embedded_graphics::primitives::Rectangle;

/// How many boxes one frame can hold — reserved chrome, icons and labels together.
pub(crate) const MAX_PLACED: usize = 24;

/// `(left, top, right, bottom)` of a box, in `i64`.
type Edges = (i64, i64, i64, i64);

/// The boxes one frame has given away.
pub(crate) struct PointPlacement {
    occupied: heapless::Vec<Rectangle, MAX_PLACED>,
}

impl PointPlacement {
    /// Start with the boxes that the map chrome owns. A box past the capacity is dropped, because
    /// a full list already refuses every mark.
    pub(crate) fn with_reserved(reserved: &[Rectangle]) -> Self {
        let mut occupied = heapless::Vec::new();
        for r in reserved {
            if occupied.push(*r).is_err() {
                break;
            }
        }
        Self { occupied }
    }

    /// Accept `r` and keep it, or refuse it.
    ///
    /// The test grows `r` by `margin` pixels on every side; a negative margin counts as zero. The
    /// stored box is `r` itself, so the clear space between two accepted marks is `margin`, not
    /// twice it. An empty box is always refused, and so is every box once the list is full.
    pub(crate) fn try_place(&mut self, r: Rectangle, margin: i32) -> bool {
        if self.occupied.is_full() || r.size.width == 0 || r.size.height == 0 {
            return false;
        }
        let m = i64::from(margin.max(0));
        let (l, t, right, bottom) = edges(r);
        let probe = (l - m, t - m, right + m, bottom + m);
        if self.occupied.iter().any(|o| overlaps(probe, edges(*o))) {
            return false;
        }
        self.occupied.push(r).is_ok()
    }

    /// Whether the list is full, so every further mark is refused.
    pub(crate) fn is_full(&self) -> bool {
        self.occupied.is_full()
    }

    /// How many boxes are held, the reserved ones included.
    pub(crate) fn placed(&self) -> usize {
        self.occupied.len()
    }
}

fn edges(r: Rectangle) -> Edges {
    let (l, t) = (i64::from(r.top_left.x), i64::from(r.top_left.y));
    (l, t, l + i64::from(r.size.width), t + i64::from(r.size.height))
}

/// The standard axis-aligned test: boxes that only share an edge do not overlap.
fn overlaps(a: Edges, b: Edges) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::prelude::{Point, Size};

    fn r(x: i32, y: i32, w: u32, h: u32) -> Rectangle {
        Rectangle::new(Point::new(x, y), Size::new(w, h))
    }

    #[test]
    fn a_clear_box_is_placed() {
        let mut p = PointPlacement::with_reserved(&[]);
        assert_eq!(p.placed(), 0);
        assert!(p.try_place(r(10, 10, 40, 24), 12));
        assert_eq!(p.placed(), 1);
        assert!(!p.is_full());
    }

    #[test]
    fn a_touching_box_is_refused() {
        let mut p = PointPlacement::with_reserved(&[]);
        assert!(p.try_place(r(0, 0, 10, 10), 0));
        // One shared pixel column is an overlap.
        assert!(!p.try_place(r(9, 0, 10, 10), 0));
        // A shared edge is not: the box starts where the first one ends.
        assert!(p.try_place(r(10, 0, 10, 10), 0));
    }

    #[test]
    fn the_margin_holds_the_boxes_apart() {
        let mut wide = PointPlacement::with_reserved(&[]);
        assert!(wide.try_place(r(0, 0, 10, 10), 12));
        assert!(!wide.try_place(r(18, 0, 10, 10), 12), "8 px apart is inside a 12 px margin");

        let mut tight = PointPlacement::with_reserved(&[]);
        assert!(tight.try_place(r(0, 0, 10, 10), 4));
        assert!(tight.try_place(r(18, 0, 10, 10), 4), "8 px apart clears a 4 px margin");
    }

    #[test]
    fn the_stored_box_is_not_grown() {
        let mut p = PointPlacement::with_reserved(&[]);
        assert!(p.try_place(r(0, 0, 10, 10), 12));
        // The stored box ends at x = 10, not at the inflated x = 22.
        assert!(p.try_place(r(23, 0, 10, 10), 0));
    }

    #[test]
    fn reserved_chrome_refuses_a_mark() {
        let clock = r(94, 6, 56, 32);
        let mut p = PointPlacement::with_reserved(&[clock]);
        assert_eq!(p.placed(), 1);
        assert!(!p.try_place(r(100, 10, 20, 20), 0), "a mark inside the clock box is refused");
        assert!(p.try_place(r(100, 60, 20, 20), 0), "a mark below it is free");
    }

    #[test]
    fn a_full_placer_refuses_every_mark() {
        let mut p = PointPlacement::with_reserved(&[]);
        for i in 0..MAX_PLACED as i32 {
            assert!(p.try_place(r(0, i * 20, 10, 10), 0), "box {i} fits");
        }
        assert!(p.is_full());
        assert_eq!(p.placed(), MAX_PLACED);
        assert!(!p.try_place(r(200, 200, 10, 10), 0), "a clear box is refused once the list is full");
    }

    #[test]
    fn a_zero_sized_box_is_refused() {
        let mut p = PointPlacement::with_reserved(&[]);
        assert!(!p.try_place(r(10, 10, 0, 24), 0));
        assert!(!p.try_place(r(10, 10, 40, 0), 0));
        assert_eq!(p.placed(), 0);
    }

    #[test]
    fn a_negative_margin_is_treated_as_zero() {
        let mut p = PointPlacement::with_reserved(&[]);
        assert!(p.try_place(r(0, 0, 10, 10), -50));
        assert!(p.try_place(r(10, 0, 10, 10), -50), "the shared edge still clears");
        assert!(!p.try_place(r(19, 0, 10, 10), -50), "the overlap still refuses");
    }

    #[test]
    fn placement_depends_only_on_the_call_order() {
        // Three boxes on one row: the outer two are clear of each other, the middle one touches both.
        let (left, middle, right) = (r(0, 0, 20, 10), r(15, 0, 20, 10), r(30, 0, 20, 10));
        let run = |order: [Rectangle; 3]| {
            let mut p = PointPlacement::with_reserved(&[]);
            order.map(|b| p.try_place(b, 0))
        };
        assert_eq!(run([left, middle, right]), [true, false, true]);
        assert_eq!(run([middle, left, right]), [true, false, false]);
        // Each order repeated gives the same set.
        assert_eq!(run([left, middle, right]), run([left, middle, right]));
        assert_eq!(run([middle, left, right]), run([middle, left, right]));
    }
}
