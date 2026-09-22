//! The shared scrolling-list widget: the wrapping cursor ([`on_step`]), the window math
//! ([`window_start`]), and [`draw_rows`], which walks the visible slots, paints the row cursor and
//! the separators, and finishes with the scrollbar. Each screen keeps only its per-row body and
//! its Press semantics. A settings page is not one of these: it is a [`rows`](super::rows) list.

use core::fmt::Write;

use embedded_graphics::primitives::Rectangle;
use obc_render::{rect, Surface};

use super::chrome::{title_frame, LIST_TOP};
use crate::screen::{palette, Transition};

/// Advance a wrapping list selection by `n` steps over `len` items. Wraps at both ends; a no-op
/// on an empty list.
pub(crate) fn step_selection(selected: usize, n: i32, len: usize) -> usize {
    if len == 0 {
        return selected;
    }
    (selected as i32 + n).rem_euclid(len as i32) as usize
}

/// The shared `Gesture::Step` arm. Always [`Transition::None`], because turning never navigates.
pub(crate) fn on_step(selected: &mut usize, n: i32, len: usize) -> Transition {
    *selected = step_selection(*selected, n, len);
    Transition::None
}

/// First visible index of a scrolling list that keeps `selected` on screen within `visible` rows
/// of `total` items. It is a pure function of the selection, so list screens keep no scroll state.
/// Cast to `i32`, this is the default `first` for [`draw_rows`].
pub(crate) fn window_start(selected: usize, visible: usize, total: usize) -> usize {
    if total <= visible || selected < visible {
        0
    } else {
        (selected + 1 - visible).min(total - visible)
    }
}

/// When [`draw_rows`] draws the hairline rule under a row (never under the last one).
#[derive(Clone, Copy)]
pub(crate) enum Separators {
    None,
    All,
    /// A rule under every row except the highlighted one.
    Unselected,
}

/// Layout of a scrolling list.
#[derive(Clone, Copy)]
pub(crate) struct ListGeometry {
    pub w: i32,
    /// Top of the first row slot.
    pub top: i32,
    /// Row pitch. The row area is `row_h - row_gap` tall, which leaves a gap between rows.
    pub row_h: i32,
    pub row_gap: i32,
    /// Left and right inset of the row area from the panel edges.
    pub side_inset: i32,
    pub separators: Separators,
    /// Slots the windowed area fits.
    pub visible: usize,
}

impl ListGeometry {
    /// Geometry for a list below the title bar: rows start at [`LIST_TOP`] and fit down to the
    /// 6 px margin above the bottom outline. A screen with a footer passes `h` minus it.
    pub fn below_title(w: i32, h: i32, row_h: i32, row_gap: i32, side_inset: i32, separators: Separators) -> Self {
        let visible = ((h - LIST_TOP - 6) / row_h).max(1) as usize;
        ListGeometry { w, top: LIST_TOP, row_h, row_gap, side_inset, separators, visible }
    }

    /// [`below_title`](Self::below_title), but the rows consume the whole viewport: the visible
    /// count comes from `nominal_row_h`, then the leftover is folded back into the pitch, so the
    /// last row lands flush with the bottom margin.
    pub fn filling_below_title(
        w: i32,
        h: i32,
        nominal_row_h: i32,
        row_gap: i32,
        side_inset: i32,
        separators: Separators,
    ) -> Self {
        let avail = h - LIST_TOP - 6;
        let visible = (avail / nominal_row_h).max(1);
        let row_h = avail / visible;
        ListGeometry { w, top: LIST_TOP, row_h, row_gap, side_inset, separators, visible: visible as usize }
    }
}

/// What [`draw_rows`] hands the row body. `area` is the rectangle the row cursor filled.
pub(crate) struct RowCtx {
    pub index: usize,
    pub area: Rectangle,
    pub selected: bool,
}

impl RowCtx {
    /// The row area on the highlighted row, where a list scrolls its one long name. `None` on
    /// every other row, which keeps the `..` cut.
    pub(crate) fn scroll(&self) -> Option<Rectangle> {
        self.selected.then_some(self.area)
    }
}

/// Draw a windowed list: for each visible slot, the cursor fill, the screen's row body and the
/// separator rule; then the scrollbar. `first` is signed, because a window can start before the
/// list; slots outside `0..total` draw as empty space.
pub(crate) fn draw_rows<S: Surface>(
    cv: &mut S,
    geo: ListGeometry,
    total: usize,
    selected: usize,
    first: i32,
    mut row_fn: impl FnMut(&mut S, RowCtx),
) {
    for slot in 0..geo.visible {
        let idx = first + slot as i32;
        if idx < 0 || idx as usize >= total {
            continue;
        }
        let idx = idx as usize;
        let y = geo.top + slot as i32 * geo.row_h;
        let area = rect(geo.side_inset, y, geo.w - 2 * geo.side_inset, geo.row_h - geo.row_gap);
        let is_selected = idx == selected;

        if is_selected {
            cv.round(area, 6, palette::AMBER);
        }
        row_fn(cv, RowCtx { index: idx, area, selected: is_selected });

        let rule = match geo.separators {
            Separators::None => false,
            Separators::All => true,
            Separators::Unselected => !is_selected,
        };
        if rule && slot + 1 < geo.visible && idx + 1 < total {
            let sx = geo.side_inset + 4;
            cv.hline(sx, y + geo.row_h - 4, geo.w - 2 * sx, palette::RULE);
        }
    }

    let sb_first = first.clamp(0, total.saturating_sub(geo.visible) as i32) as usize;
    scrollbar(cv, geo.w - 8, geo.top, geo.visible as i32 * geo.row_h, total, sb_first, geo.visible);
}

/// [`title_frame`] with a `pos / total` counter on the right, but only when the list can scroll.
/// A counter on a list that fits is noise.
pub(crate) fn list_frame(cv: &mut impl Surface, w: i32, h: i32, title: &str, pos: usize, total: usize, visible: usize) {
    if total > visible {
        let mut counter: heapless::String<12> = heapless::String::new();
        let _ = write!(counter, "{pos} / {total}");
        title_frame(cv, w, h, title, &counter);
    } else {
        title_frame(cv, w, h, title, "");
    }
}

/// Draw a list scrollbar at the right edge, or nothing when everything fits. `top` and `height`
/// are the windowed list area; `first` is [`window_start`]'s result.
pub(crate) fn scrollbar(
    cv: &mut impl Surface,
    x: i32,
    top: i32,
    height: i32,
    total: usize,
    first: usize,
    visible: usize,
) {
    if total <= visible || total == 0 {
        return;
    }
    cv.round(rect(x, top, 3, height), 1, palette::RULE);
    let thumb_h = (height * visible as i32 / total as i32).max(10);
    let thumb_y = top + height * first as i32 / total as i32;
    cv.round(rect(x, thumb_y, 3, thumb_h), 1, palette::WOOD);
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::prelude::Point;
    use obc_render::text::{Font, TextAlign};

    #[test]
    fn step_selection_wraps_backward_past_the_top() {
        assert_eq!(step_selection(0, -1, 4), 3, "up from the first item lands on the last");
        assert_eq!(step_selection(0, -1, 1), 0, "a single-item list stays put");
    }

    #[test]
    fn step_selection_wraps_forward_past_the_bottom() {
        assert_eq!(step_selection(3, 1, 4), 0, "down from the last item lands on the first");
    }

    #[test]
    fn step_selection_wraps_multiple_steps() {
        assert_eq!(step_selection(0, 5, 3), 2, "a long forward flick wraps modulo the length");
        assert_eq!(step_selection(0, -5, 3), 1, "a long backward flick wraps without going negative");
        assert_eq!(step_selection(2, 3, 3), 2, "exactly one lap is a no-op");
    }

    #[test]
    fn step_selection_on_empty_list_is_a_noop() {
        assert_eq!(step_selection(0, 1, 0), 0, "a forward step on an empty list stays at 0");
        assert_eq!(step_selection(0, -1, 0), 0, "a backward step on an empty list stays at 0");
        assert_eq!(step_selection(7, 3, 0), 7, "the selection is returned unchanged, not modulo'd");
    }

    /// A draw target that swallows every primitive.
    struct NullSurface;
    impl Surface for NullSurface {
        fn clear(&mut self, _color: u16) {}
        fn fill(&mut self, _area: Rectangle, _color: u16) {}
        fn round(&mut self, _area: Rectangle, _radius: u32, _color: u16) {}
        fn round_outline(&mut self, _area: Rectangle, _radius: u32, _color: u16) {}
        fn line(&mut self, _a: Point, _b: Point, _color: u16) {}
        fn triangle(&mut self, _a: Point, _b: Point, _c: Point, _color: u16) {}
        fn disc(&mut self, _center: Point, _radius: u32, _color: u16) {}
        fn text(&mut self, _s: &str, at: Point, _font: Font, _align: TextAlign, _color: u16) -> Point {
            at
        }
    }

    fn geo(visible: usize) -> ListGeometry {
        ListGeometry {
            w: 240,
            top: LIST_TOP,
            row_h: 46,
            row_gap: 6,
            side_inset: 14,
            separators: Separators::None,
            visible,
        }
    }

    /// Rows drawn for a signed window: `(index, y)` per body invocation.
    fn drawn(g: ListGeometry, total: usize, first: i32) -> heapless::Vec<(usize, i32), 8> {
        let mut seen = heapless::Vec::new();
        draw_rows(&mut NullSurface, g, total, 0, first, |_, row| {
            let _ = seen.push((row.index, row.area.top_left.y));
        });
        seen
    }

    #[test]
    fn draw_rows_skips_slots_before_the_list() {
        let g = geo(5);
        let seen = drawn(g, 14, -2);
        let y = |slot: i32| g.top + slot * g.row_h;
        assert_eq!(&seen[..], [(0, y(2)), (1, y(3)), (2, y(4))], "slots 0–1 empty, items 0–2 in slots 2–4");
    }

    #[test]
    fn draw_rows_skips_slots_past_the_list() {
        let g = geo(5);
        let seen = drawn(g, 10, 8);
        let y = |slot: i32| g.top + slot * g.row_h;
        assert_eq!(&seen[..], [(8, y(0)), (9, y(1))], "only the last two items draw; slots 2–4 empty");
    }
}
