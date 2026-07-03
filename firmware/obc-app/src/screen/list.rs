//! The shared scrolling-list widget. Five screens (Menu, Settings, Route menu, Add field,
//! Fields) are "a title bar over a windowed row list"; this module owns everything they have
//! in common — the wrapping cursor ([`on_turn`]), the window math ([`window_start`] /
//! [`pinned_first`]), and [`draw_rows`], which walks the visible slots, paints the amber
//! row cursor and the separators, and finishes with the scrollbar. Each screen keeps only
//! its per-row body (bullet + label, two-line route pane, span badge, …) and its Press
//! semantics.

use core::fmt::Write;

use embedded_graphics::{prelude::Point, primitives::Rectangle};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use super::{palette, title_frame, Transition, LIST_TOP};

/// Advance a wrapping list selection by `n` detents over `len` items. Wraps at both ends; a no-op
/// on an empty list.
pub(crate) fn step_selection(selected: usize, n: i32, len: usize) -> usize {
    if len == 0 {
        return selected;
    }
    (selected as i32 + n).rem_euclid(len as i32) as usize
}

/// The shared `Gesture::Turn` arm: step a wrapping cursor by `n` detents over `len` rows.
/// Always [`Transition::None`] — turning never navigates.
pub(crate) fn on_turn(selected: &mut usize, n: i32, len: usize) -> Transition {
    *selected = step_selection(*selected, n, len);
    Transition::None
}

/// First visible index of a scrolling list that keeps `selected` on screen within `visible` rows
/// of `total` items. Stateless — a pure function of the selection — so list screens need no scroll
/// state: the highlight moves down to the last visible row, then the window follows it. Cast to
/// `i32` this is the default `first` for [`draw_rows`].
pub fn window_start(selected: usize, visible: usize, total: usize) -> usize {
    if total <= visible || selected < visible {
        0
    } else {
        (selected + 1 - visible).min(total - visible)
    }
}

/// The grab-pinning alternative to [`window_start`]: anchor `selected` at the middle slot for
/// *every* position by scrolling the window virtually — so near the list ends the pinned row
/// stays centred with empty space above/below rather than drifting to the edge. The offset can
/// run past either end; [`draw_rows`] skips the out-of-range slots. Used by the Fields screen
/// while a row is grabbed.
pub(crate) fn pinned_first(selected: usize, visible: usize) -> i32 {
    selected as i32 - (visible / 2) as i32
}

/// When [`draw_rows`] draws the hairline rule under a row (never under the last one).
#[derive(Clone, Copy)]
pub(crate) enum Separators {
    /// No rules — the Fields / Add-field rows carry enough shape on their own.
    None,
    /// A rule under every row (the nav menus).
    All,
    /// A rule under every row except the highlighted one (the Route menu — the amber pane
    /// reads cleaner without a line hugging it).
    Unselected,
}

/// Layout of a scrolling list: the panel width, where rows start, their pitch and inset, the
/// separator policy, and how many slots fit the windowed area.
#[derive(Clone, Copy)]
pub(crate) struct ListGeometry {
    pub w: i32,
    /// Top of the first row slot.
    pub top: i32,
    /// Row pitch. The row *area* (the amber cursor fill) is `row_h - row_gap` tall, leaving a
    /// breathing gap between rows.
    pub row_h: i32,
    pub row_gap: i32,
    /// Left/right inset of the row area from the panel edges.
    pub side_inset: i32,
    pub separators: Separators,
    /// Slots the windowed area fits.
    pub visible: usize,
}

impl ListGeometry {
    /// Geometry for a list filling the frame below the title bar: rows start at [`LIST_TOP`] and
    /// fit down to the 6 px margin above the bottom outline. A screen reserving a footer (Fields'
    /// delete bar) passes `h` minus it.
    pub fn below_title(w: i32, h: i32, row_h: i32, row_gap: i32, side_inset: i32, separators: Separators) -> Self {
        let visible = ((h - LIST_TOP - 6) / row_h).max(1) as usize;
        ListGeometry { w, top: LIST_TOP, row_h, row_gap, side_inset, separators, visible }
    }
}

/// What [`draw_rows`] hands the row body: which item this slot shows, the row area (the same
/// rectangle the amber cursor filled), and whether it is the highlighted row.
pub(crate) struct RowCtx {
    pub index: usize,
    pub area: Rectangle,
    pub selected: bool,
}

/// Draw a windowed list: for each visible slot, the amber cursor fill (on the selected row),
/// the screen's row body, and the separator rule; then the right-edge scrollbar. `first` is
/// signed — [`window_start`]`as i32` normally, [`pinned_first`] while a Fields row is grabbed —
/// and slots mapping outside `0..total` draw as empty space. The scrollbar clamps the virtual
/// offset back into range.
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

/// The nav-menu row body shared by Menu and Settings: a pointer triangle and a Body-tier label,
/// vertically centred in the row area (the highlight makes the bullet ink, unselected rows muted).
pub(crate) fn nav_row(cv: &mut impl Surface, area: Rectangle, label: &str, selected: bool) {
    let x = area.top_left.x;
    let mid = area.top_left.y + area.size.height as i32 / 2;
    let bullet = if selected { palette::INK } else { palette::SUBTEXT };
    cv.triangle(Point::new(x + 14, mid - 9), Point::new(x + 14, mid + 9), Point::new(x + 27, mid), bullet);
    cv.text(label, Point::new(x + 38, mid - 14), Font::Body, TextAlign::Left, palette::INK);
}

/// A whole nav-menu draw — the chrome plus [`nav_row`]s with hairline separators — so Menu and
/// Settings (identical apart from title and items) are each a single call.
pub(crate) fn nav_list(cv: &mut impl Surface, w: i32, h: i32, title: &str, items: &[&str], selected: usize) {
    /// Per-row height — fits a Body-tier row with an amber highlight + padding.
    const ROW_H: i32 = 52;
    let geo = ListGeometry::below_title(w, h, ROW_H, 8, 16, Separators::All);
    list_frame(cv, w, h, title, selected + 1, items.len(), geo.visible);
    let first = window_start(selected, geo.visible, items.len()) as i32;
    draw_rows(cv, geo, items.len(), selected, first, |cv, row| nav_row(cv, row.area, items[row.index], row.selected));
}

/// [`title_frame`] with a `pos / total` counter on the right — but only when the list can
/// scroll (`total > visible`): a `1 / 2` counter on a static two-item menu is noise, while
/// an overflowing list (Routes with many routes, a full Fields list) needs the position cue.
pub(crate) fn list_frame(cv: &mut impl Surface, w: i32, h: i32, title: &str, pos: usize, total: usize, visible: usize) {
    if total > visible {
        let mut counter: heapless::String<12> = heapless::String::new();
        let _ = write!(counter, "{pos} / {total}");
        title_frame(cv, w, h, title, &counter);
    } else {
        title_frame(cv, w, h, title, "");
    }
}

/// Draw a list scrollbar — a faint track with a proportional thumb — at the right
/// edge, or nothing when everything fits. `top`/`height` is the windowed list
/// area; `first` is [`window_start`]'s result.
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

    // `step_selection` wrapping: a `%` regression is negative for a backward turn at the top, which
    // would hand back a garbage index and highlight nothing or panic on the row lookup.

    /// Backward off the top: `Turn(-1)` from index 0 wraps to the last item, not a negative index.
    #[test]
    fn step_selection_wraps_backward_past_the_top() {
        assert_eq!(step_selection(0, -1, 4), 3, "up from the first item lands on the last");
        assert_eq!(step_selection(0, -1, 1), 0, "a single-item list stays put");
    }

    /// Forward off the bottom: `Turn(1)` from the last item wraps to the first.
    #[test]
    fn step_selection_wraps_forward_past_the_bottom() {
        assert_eq!(step_selection(3, 1, 4), 0, "down from the last item lands on the first");
    }

    /// A multi-detent turn larger than the list wraps cleanly, not off the end.
    #[test]
    fn step_selection_wraps_multiple_turns() {
        assert_eq!(step_selection(0, 5, 3), 2, "a long forward flick wraps modulo the length");
        assert_eq!(step_selection(0, -5, 3), 1, "a long backward flick wraps without going negative");
        assert_eq!(step_selection(2, 3, 3), 2, "exactly one lap is a no-op");
    }

    /// An empty list is a no-op for any turn — the `len == 0` guard must short-circuit before the
    /// `% 0` that would panic.
    #[test]
    fn step_selection_on_empty_list_is_a_noop() {
        assert_eq!(step_selection(0, 1, 0), 0, "a forward turn on an empty list stays at 0");
        assert_eq!(step_selection(0, -1, 0), 0, "a backward turn on an empty list stays at 0");
        assert_eq!(step_selection(7, 3, 0), 7, "the selection is returned unchanged, not modulo'd");
    }

    // `draw_rows` windowing with a signed `first` — the Fields grab-pinning contract: slots
    // mapping outside the list draw nothing, in-range items land in the *slot* positions, not
    // clamped back to the top.

    /// A draw target that swallows every primitive — the windowing tests only observe which rows
    /// the body callback is invoked for and where.
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

    /// A window scrolled past the top (`first < 0`, the grabbed-at-the-start case): the leading
    /// slots stay empty and the real rows keep their slot positions further down.
    #[test]
    fn draw_rows_skips_slots_before_the_list() {
        let g = geo(5);
        let seen = drawn(g, 14, -2);
        let y = |slot: i32| g.top + slot * g.row_h;
        assert_eq!(&seen[..], [(0, y(2)), (1, y(3)), (2, y(4))], "slots 0–1 empty, items 0–2 in slots 2–4");
    }

    /// A window scrolled past the end (grabbed-at-the-bottom): the trailing slots stay empty.
    #[test]
    fn draw_rows_skips_slots_past_the_list() {
        let g = geo(5);
        let seen = drawn(g, 10, 8);
        let y = |slot: i32| g.top + slot * g.row_h;
        assert_eq!(&seen[..], [(8, y(0)), (9, y(1))], "only the last two items draw; slots 2–4 empty");
    }
}
