//! The shared row vocabulary: the settings row rectangle and cursor, the value picker, the
//! stat-ledger row, and the guarded-action option rows.

use embedded_graphics::{prelude::Point, primitives::Rectangle};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::screen::palette;

/// Left inset of every settings row (clears the framed outline).
pub(crate) const ROW_X: i32 = 14;

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

/// One centred value row flanked by ◄ ► triangles, always drawn as the cursor. `y` is the row's
/// top and the row is 50 px tall. Returns the row rectangle, for content laid out below it.
pub(crate) fn value_row_with_arrows(cv: &mut impl Surface, y: i32, w: i32, text: &str) -> Rectangle {
    let area = row_rect(y, w, 50);
    row_cursor(cv, area, true, false);
    let midy = area.top_left.y + area.size.height as i32 / 2;
    cv.text_vcentered(text, w / 2, (area.top_left.y, 50), Font::Body, TextAlign::Center, palette::INK);
    let ax = area.top_left.x + 18;
    cv.triangle(Point::new(ax, midy - 9), Point::new(ax, midy + 9), Point::new(ax - 11, midy), palette::INK);
    let bx = area.top_left.x + area.size.width as i32 - 18;
    cv.triangle(Point::new(bx, midy - 9), Point::new(bx, midy + 9), Point::new(bx + 11, midy), palette::INK);
    area
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
