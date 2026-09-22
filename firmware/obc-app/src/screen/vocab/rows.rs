//! The shared row vocabulary: the settings grammar's rows (a door, a value, a switch, an action,
//! an info line), the stat-ledger row, and the guarded-action option rows.
//!
//! One grammar for the settings pages and both drawers. A row is one line, or a label with a line
//! under it. Every text starts at the same x, and every chevron is one size in one column, so a
//! list of mixed rows still reads as one grid.

use embedded_graphics::{prelude::Point, primitives::Rectangle};
use obc_reader::PoiCategory;
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::screen::palette;
use crate::settings::Language;

/// Left inset of every settings row (clears the framed outline).
pub(crate) const ROW_X: i32 = 14;
/// A one-line row: a door, a switch, or an action.
pub(crate) const ROW_ONE: i32 = 40;
/// A two-line row: a label with a value or a hint under it.
pub(crate) const ROW_TWO: i32 = 52;
/// The gap between rows.
pub(crate) const ROW_GAP: i32 = 4;
/// Where a row's text starts, relative to the row's left edge.
const TEXT_DX: i32 = 10;
/// The column a chevron takes at the right of a row.
const CHEVRON_W: i32 = 28;
/// The column a switch takes at the right of a row.
const SWITCH_W: i32 = 58;
/// The two-line block: the Body label's cell top, then the Label line's cell top, so the block
/// sits centred in a [`ROW_TWO`] row.
const LINE1_DY: i32 = 2;
const LINE2_DY: i32 = 26;

pub(crate) fn row_rect(y: i32, w: i32, h: i32) -> Rectangle {
    rect(ROW_X, y, w - 2 * ROW_X, h)
}

/// Paint a row's amber focus cursor. A no-op while editing, because the field's `▲▼` box is the
/// cursor then.
pub(crate) fn row_cursor(cv: &mut impl Surface, area: Rectangle, selected: bool, editing: bool) {
    if selected && !editing {
        cv.round(area, 6, palette::AMBER);
    }
}

/// A glyph a row's second line can carry before its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowIcon {
    Flag(Language),
    Poi(PoiCategory),
}

/// A row's second line: a value or a hint, in the muted colour, with an optional glyph.
#[derive(Clone, Copy)]
pub(crate) struct Line2<'a> {
    pub icon: Option<RowIcon>,
    pub text: &'a str,
}

impl<'a> Line2<'a> {
    pub(crate) fn text(text: &'a str) -> Self {
        Line2 { icon: None, text }
    }
}

/// The row height a body needs.
pub(crate) fn row_height(two_lines: bool) -> i32 {
    if two_lines {
        ROW_TWO
    } else {
        ROW_ONE
    }
}

/// The right-hand chevron: the mark of a row that opens something, a page or an editor.
pub(crate) fn chevron(cv: &mut impl Surface, area: Rectangle, color: u16) {
    let cx0 = area.top_left.x + area.size.width as i32 - 18;
    let cy = area.top_left.y + area.size.height as i32 / 2;
    cv.triangle(Point::new(cx0, cy - 8), Point::new(cx0, cy + 8), Point::new(cx0 + 9, cy), color);
}

/// The font a label fits in: Body, or the Label cut when Body would run into the right-hand
/// column. The drawers' rule, so a long translation shrinks rather than collides.
fn label_font(label: &str, room: i32) -> Font {
    if text_width(label, Font::Body) as i32 > room {
        Font::Label
    } else {
        Font::Body
    }
}

/// The label and the optional second line of a row. `right_w` is the column the label must clear.
fn row_text(cv: &mut impl Surface, area: Rectangle, label: &str, line2: Option<Line2>, right_w: i32, ink: u16) {
    let x = area.top_left.x + TEXT_DX;
    let room = area.size.width as i32 - TEXT_DX - right_w;
    // A label the catalog breaks over two lines takes both lines of a two-line row, in the Label
    // cut, and states nothing under itself.
    if let Some((first, second)) = label.split_once('\n') {
        let y = area.top_left.y;
        cv.text(first, Point::new(x, y + LINE1_DY), Font::Label, TextAlign::Left, ink);
        cv.text(second, Point::new(x, y + LINE2_DY), Font::Label, TextAlign::Left, ink);
        return;
    }
    let font = label_font(label, room);
    match line2 {
        Some(line) => {
            let y = area.top_left.y;
            cv.text(label, Point::new(x, y + LINE1_DY), font, TextAlign::Left, ink);
            let sub_ink = if ink == palette::CONTOUR { palette::CONTOUR } else { palette::SUBTEXT };
            let mut tx = x;
            if let Some(icon) = line.icon {
                let cy = y + LINE2_DY + Font::Label.cap_mid() as i32;
                match icon {
                    RowIcon::Flag(lang) => super::flags::draw_flag(cv, tx + 1, cy - super::flags::FLAG_H / 2, lang),
                    RowIcon::Poi(cat) => crate::screen::poi_menu::draw_category_icon(
                        cv,
                        cat,
                        Point::new(tx + 9, cy),
                        ink,
                        palette::PARCHMENT,
                    ),
                }
                tx += 28;
            }
            // The line is cut to the column, so a long status never runs under the chevron.
            let budget = area.top_left.x + area.size.width as i32 - right_w - tx;
            let text = super::marquee::fit(line.text, budget, Font::Label);
            cv.text(&text, Point::new(tx, y + LINE2_DY), Font::Label, TextAlign::Left, sub_ink);
        }
        None => {
            let (top, h) = (area.top_left.y, area.size.height as i32);
            cv.text_vcentered(label, x, (top, h), font, TextAlign::Left, ink);
        }
    }
}

/// A row that opens something: a page (a door) or an editor (a value). `line2` is the value or
/// the hint under the label; `chevron` is drawn on a live row. An inert row is recessed.
pub(crate) fn nav_row(
    cv: &mut impl Surface,
    area: Rectangle,
    label: &str,
    line2: Option<Line2>,
    selected: bool,
    live: bool,
    with_chevron: bool,
) {
    row_cursor(cv, area, selected, false);
    let ink = if live { palette::INK } else { palette::CONTOUR };
    row_text(cv, area, label, line2, if with_chevron { CHEVRON_W } else { 0 }, ink);
    if with_chevron && live {
        chevron(cv, area, ink);
    }
}

/// A door onto a destructive page, lettered in warning red until the cursor lands on it. A press
/// opens it like any door; the confirm and its hold live on the page.
pub(crate) fn danger_door_row(cv: &mut impl Surface, area: Rectangle, label: &str, selected: bool) {
    row_cursor(cv, area, selected, false);
    let ink = if selected { palette::INK } else { palette::WARNING };
    row_text(cv, area, label, None, CHEVRON_W, ink);
    chevron(cv, area, ink);
}

/// A row that flips a bool in place: the label and the slider.
pub(crate) fn switch_row(cv: &mut impl Surface, area: Rectangle, label: &str, on: bool, selected: bool) {
    row_cursor(cv, area, selected, false);
    row_text(cv, area, label, None, SWITCH_W, palette::INK);
    toggle_slider(cv, area, on);
}

/// A row that does something. A destructive one is lettered in warning red and, while the cursor
/// is on it, sits on the shaded hold base that fills red with `hold`. `hint` is the line under the
/// label, for a row that cannot act right now.
#[allow(clippy::too_many_arguments)] // the row's whole state, spelled out
pub(crate) fn action_row(
    cv: &mut impl Surface,
    area: Rectangle,
    label: &str,
    hint: Option<&str>,
    selected: bool,
    live: bool,
    danger: bool,
    hold: f32,
) {
    if danger {
        confirm_row(cv, area, selected, true, hold, palette::WARNING, 6);
    } else {
        row_cursor(cv, area, selected, false);
    }
    let ink = match (live, danger, selected) {
        (false, _, _) => palette::CONTOUR,
        (true, true, false) => palette::WARNING,
        _ => palette::INK,
    };
    row_text(cv, area, label, hint.map(Line2::text), 0, ink);
}

/// A read-only row: a label with its value under it. The cursor never lands on it.
pub(crate) fn info_row(cv: &mut impl Surface, area: Rectangle, label: &str, line2: Line2) {
    row_text(cv, area, label, Some(line2), 0, palette::INK);
}

/// The window of rows to draw when rows have their own heights: the first and one-past-last index
/// so `selected` is on the panel and as many rows as fit follow. `avail` is the panel height the
/// rows may use, gaps included.
pub(crate) fn window_by_height(heights: &[i32], selected: usize, avail: i32) -> (usize, usize) {
    let end_from = |first: usize| {
        let mut used = 0;
        let mut end = first;
        while end < heights.len() && used + heights[end] <= avail {
            used += heights[end] + ROW_GAP;
            end += 1;
        }
        end
    };
    let mut first = 0;
    loop {
        let end = end_from(first);
        if selected < end || first >= selected {
            return (first, end);
        }
        first += 1;
    }
}

/// Draw a slider toggle at the right of `area`: a white knob sliding left (off) or right (on), on
/// a track that is dark for off and green for on.
pub(crate) fn toggle_slider(cv: &mut impl Surface, area: Rectangle, on: bool) {
    let (tw, th) = (50, 28);
    let tx = area.top_left.x + area.size.width as i32 - tw - 4;
    let ty = area.top_left.y + (area.size.height as i32 - th) / 2;
    cv.round(rect(tx, ty, tw, th), 6, if on { palette::ON } else { palette::INK });
    let m = 4;
    let k = th - 2 * m;
    let kx = if on { tx + tw - m - k } else { tx + m };
    cv.round(rect(kx, ty + m, k, k), 4, palette::PARCHMENT);
}

/// One stat-ledger row: caption on the left, the value right-aligned with a small unit suffix, and
/// an optional climb/descent triangle left of the value (`Some(true)` = up). The text sits on the
/// parchment with no pane, because that look is reserved for the riding grid's live tiles.
pub(crate) fn ledger_row(
    cv: &mut impl Surface,
    w: i32,
    y: i32,
    caption: &str,
    value: &str,
    unit: &str,
    arrow: Option<bool>,
) {
    use palette::*;
    // The Display and Label caps both bottom out at `y + 32`, so the baselines agree.
    cv.text(caption, Point::new(16, y + 14), Font::Label, TextAlign::Left, SUBTEXT);
    cv.text(unit, Point::new(w - 16, y + 14), Font::Label, TextAlign::Right, SUBTEXT);
    let unit_w = unit.chars().count() as i32 * Font::Label.char_width() as i32;
    let vx = w - 16 - unit_w - 6;
    cv.text(value, Point::new(vx, y + 6), Font::Display, TextAlign::Right, INK);
    if let Some(up) = arrow {
        let value_w = value.chars().count() as i32 * Font::Display.char_width() as i32;
        let ax = vx - value_w - 18;
        let (flat, tip) = if up { (y + 30, y + 12) } else { (y + 12, y + 30) };
        cv.triangle(Point::new(ax, flat), Point::new(ax + 13, flat), Point::new(ax + 6, tip), INK);
    }
}

/// One option in a guarded-action menu. `guard` marks an irreversible option, which needs a hold
/// instead of a press.
pub(crate) struct MenuItem {
    pub label: &'static str,
    pub guard: bool,
}

/// Draw a selected option row's background: a plain amber fill, or, when `guard` is set, a shaded
/// base that fills in `fill` to track `hold_progress` (0.0–1.0). The caller draws the label.
pub(crate) fn confirm_row(
    cv: &mut impl Surface,
    row: Rectangle,
    selected: bool,
    guard: bool,
    hold_progress: f32,
    fill: u16,
    radius: u32,
) {
    if !selected {
        return;
    }
    if guard {
        cv.round(row, radius, palette::PARCHMENT_SHADE);
        let fill_w = (row.size.width as f32 * hold_progress.clamp(0.0, 1.0)) as i32;
        if fill_w > 0 {
            cv.round(rect(row.top_left.x, row.top_left.y, fill_w, row.size.height as i32), radius, fill);
        }
    } else {
        cv.round(row, radius, palette::AMBER);
    }
}

/// The per-screen geometry [`draw_guarded_rows`] lays [`MenuItem`]s out with.
pub(crate) struct GuardedRowsGeometry {
    pub x: i32,
    pub w: i32,
    /// Top of the first row.
    pub top: i32,
    pub row_h: i32,
    pub gap: i32,
    /// The label anchor, relative to the row's top-left.
    pub label_dx: i32,
    pub label_dy: i32,
}

impl GuardedRowsGeometry {
    /// The rows of a full-bleed confirm card. Only `top` differs between the cards.
    pub(crate) fn card(w: i32, top: i32) -> Self {
        GuardedRowsGeometry { x: 12, w: w - 24, top, row_h: 46, gap: 8, label_dx: 16, label_dy: 11 }
    }

    /// The action rows inside a framed panel. The row height and the gap stay the caller's,
    /// because each panel sizes its block to the space it has.
    pub(crate) fn panel(w: i32, top: i32, row_h: i32, gap: i32) -> Self {
        GuardedRowsGeometry { x: 14, w: w - 28, top, row_h, gap, label_dx: 12, label_dy: 5 }
    }
}

/// Draw a guarded-action menu's option rows: each [`MenuItem`] gets its [`confirm_row`] background
/// and its label. The caller draws the chrome around them.
pub(crate) fn draw_guarded_rows(
    cv: &mut impl Surface,
    items: &[MenuItem],
    selected: usize,
    hold_progress: f32,
    fill: u16,
    geo: GuardedRowsGeometry,
) {
    for (i, item) in items.iter().enumerate() {
        let y = geo.top + i as i32 * (geo.row_h + geo.gap);
        let row = rect(geo.x, y, geo.w, geo.row_h);
        confirm_row(cv, row, i == selected, item.guard, hold_progress, fill, 6);
        cv.text(
            item.label,
            Point::new(geo.x + geo.label_dx, y + geo.label_dy),
            Font::Body,
            TextAlign::Left,
            palette::INK,
        );
    }
}
