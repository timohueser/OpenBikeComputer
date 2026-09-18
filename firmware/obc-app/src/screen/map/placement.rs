//! Screen-space occupancy for map point marks.
//!
//! The reserved chrome goes in first. A mark is then accepted only when its box clears the chrome
//! and — grown by the caller's margin — clears every mark already placed. A mark that does not fit
//! is dropped, never moved. There are no leader lines, no alternative positions around an anchor
//! and no second pass.
//!
//! This is occupancy only: it holds no panel and refuses nothing for reaching past one. A mark may
//! hang over an edge, and the panel clips it. The whole unclipped box is what the tests use, so a
//! clipped mark still holds its space.
//!
//! The stored box is the raw one, not the grown one. The clear space a mark keeps is the margin of
//! its own call, so two marks that use different margins are held apart by the later one. The
//! margin holds marks apart from each other only: reserved chrome is the ink it protects, so a mark
//! may sit flush against it.
//!
//! The result depends only on the order of the calls, so the caller offers its marks best first and
//! the same camera always gives the same frame. The arithmetic is integer only, with the edges
//! taken in `i64` so a box near `i32::MAX` cannot wrap. Nothing is allocated.

use embedded_graphics::primitives::Rectangle;

/// How many boxes one frame can hold: 6 for the chrome and 6 for the labels, with room to spare.
/// The point icons keep their own occupancy today, so they take no slot here. The list is 384 bytes
/// of transient stack inside the draw pass.
const MAX_PLACED: usize = 24;

/// `(left, top, right, bottom)` of a box, in `i64`.
type Edges = (i64, i64, i64, i64);

/// The boxes one frame has given away.
pub(crate) struct PointPlacement {
    /// How many leading `occupied` boxes are reserved chrome, which takes no margin.
    reserved: usize,
    occupied: heapless::Vec<Rectangle, MAX_PLACED>,
}

impl PointPlacement {
    /// Start with the boxes that the map chrome owns. A reserved box past the capacity is dropped,
    /// because a full list already refuses every mark.
    pub(crate) fn new(reserved: &[Rectangle]) -> Self {
        let mut occupied = heapless::Vec::new();
        for r in reserved {
            if occupied.push(*r).is_err() {
                break;
            }
        }
        Self { reserved: occupied.len(), occupied }
    }

    /// Accept `r` and keep it, or refuse it.
    ///
    /// An empty box, and every box once the list is full, are refused. Against another mark the
    /// test grows `r` by `margin` pixels on every side (a negative margin counts as zero); against
    /// reserved chrome it uses `r` itself, because the margin is there to keep marks legible beside
    /// each other, not to push them off the chrome.
    pub(crate) fn try_place(&mut self, r: Rectangle, margin: i32) -> bool {
        if self.occupied.is_full() || r.size.width == 0 || r.size.height == 0 {
            return false;
        }
        let (l, t, right, bottom) = edges(r);
        let m = i64::from(margin.max(0));
        let probe = (l - m, t - m, right + m, bottom + m);
        let hits = self.occupied.iter().enumerate().any(|(i, o)| {
            let test = if i < self.reserved { (l, t, right, bottom) } else { probe };
            overlaps(test, edges(*o))
        });
        if hits {
            return false;
        }
        self.occupied.push(r).is_ok()
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
        let mut p = PointPlacement::new(&[]);
        assert!(p.try_place(r(10, 10, 40, 24), 12));
    }

    #[test]
    fn a_touching_box_is_refused() {
        let mut p = PointPlacement::new(&[]);
        assert!(p.try_place(r(0, 0, 10, 10), 0));
        // One shared pixel column is an overlap.
        assert!(!p.try_place(r(9, 0, 10, 10), 0));
        // A shared edge is not: the box starts where the first one ends.
        assert!(p.try_place(r(10, 0, 10, 10), 0));
    }

    #[test]
    fn the_margin_holds_the_boxes_apart() {
        let mut tight = PointPlacement::new(&[]);
        assert!(tight.try_place(r(0, 0, 10, 10), 12));
        assert!(!tight.try_place(r(21, 0, 10, 10), 12), "11 px of clear space is inside a 12 px margin");

        let mut clear = PointPlacement::new(&[]);
        assert!(clear.try_place(r(0, 0, 10, 10), 12));
        assert!(clear.try_place(r(22, 0, 10, 10), 12), "12 px of clear space holds the margin exactly");
    }

    #[test]
    fn the_later_margin_alone_holds_the_gap() {
        let mut p = PointPlacement::new(&[]);
        assert!(p.try_place(r(0, 0, 10, 10), 12));
        // The stored box ends at x = 10, not at the grown x = 22, so the first margin protects
        // nothing: a second mark that asks for no margin comes within one pixel.
        assert!(p.try_place(r(11, 0, 10, 10), 0));
    }

    #[test]
    fn reserved_chrome_refuses_a_mark() {
        let clock = r(94, 6, 56, 32);
        let mut p = PointPlacement::new(&[clock]);
        assert!(!p.try_place(r(100, 10, 20, 20), 0), "a mark inside the clock box is refused");
        assert!(p.try_place(r(100, 60, 20, 20), 0), "a mark below it is free");
    }

    #[test]
    fn a_mark_sits_flush_against_reserved_chrome() {
        let chrome = r(0, 100, 240, 100);
        let mut p = PointPlacement::new(&[chrome]);
        // The margin keeps marks legible beside each other; the chrome box is already its own ink.
        assert!(p.try_place(r(0, 76, 40, 24), 12), "a box ending where the chrome starts is placed");
        assert!(!p.try_place(r(100, 99, 40, 24), 12), "a box reaching into the chrome is still refused");
    }

    #[test]
    fn a_box_reaching_past_the_panel_is_placed_and_holds_its_whole_width() {
        let mut p = PointPlacement::new(&[]);
        assert!(p.try_place(r(-20, 100, 40, 24), 0), "a box hanging over the left edge is placed");
        // Its off-panel half still holds space, so a second mark cannot overprint the visible part.
        assert!(!p.try_place(r(-40, 100, 40, 24), 0), "the part past the edge is still occupied");
    }

    #[test]
    fn a_full_placer_refuses_every_mark() {
        let mut p = PointPlacement::new(&[]);
        for i in 0..MAX_PLACED as i32 {
            assert!(p.try_place(r(0, i * 20, 10, 10), 0), "box {i} fits");
        }
        assert!(!p.try_place(r(200, 200, 10, 10), 0), "a clear box is refused once the list is full");
    }

    #[test]
    fn a_zero_sized_box_is_refused() {
        let mut p = PointPlacement::new(&[]);
        assert!(!p.try_place(r(10, 10, 0, 24), 0));
        assert!(!p.try_place(r(10, 10, 40, 0), 0));
    }

    #[test]
    fn a_negative_margin_is_treated_as_zero() {
        let mut p = PointPlacement::new(&[]);
        assert!(p.try_place(r(0, 0, 10, 10), -50));
        assert!(p.try_place(r(10, 0, 10, 10), -50), "the shared edge still clears");
        assert!(!p.try_place(r(19, 0, 10, 10), -50), "the overlap still refuses");
    }

    #[test]
    fn edges_near_i32_max_do_not_wrap() {
        let mut p = PointPlacement::new(&[]);
        assert!(p.try_place(r(i32::MAX - 5, i32::MAX - 5, 10, 10), 0));
        assert!(!p.try_place(r(i32::MAX - 3, i32::MAX - 3, 10, 10), 0), "the overlap is still seen");
        assert!(!p.try_place(r(i32::MAX - 100, i32::MAX - 100, 10, 10), i32::MAX), "a huge margin does not wrap");
    }

    #[test]
    fn placement_depends_only_on_the_call_order() {
        // Three boxes on one row: the outer two are clear of each other, the middle one touches both.
        let (left, middle, right) = (r(0, 0, 20, 10), r(15, 0, 20, 10), r(30, 0, 20, 10));
        let run = |order: [Rectangle; 3]| {
            let mut p = PointPlacement::new(&[]);
            order.map(|b| p.try_place(b, 0))
        };
        assert_eq!(run([left, middle, right]), [true, false, true]);
        assert_eq!(run([middle, left, right]), [true, false, false]);
    }
}
