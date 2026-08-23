//! The shared **row** vocabulary — the settings row rectangle and cursor, the value picker, the
//! stat-ledger row, and the guarded-action option rows the confirm cards and panels draw.

use embedded_graphics::{prelude::Point, primitives::Rectangle};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::screen::palette;

/// Left inset of every settings row (clears the framed outline).
pub(crate) const ROW_X: i32 = 14;

/// The full-width settings-row rectangle at `y` of height `h`.
pub(crate) fn row_rect(y: i32, w: i32, h: i32) -> Rectangle {
    rect(ROW_X, y, w - 2 * ROW_X, h)
}

/// Paint a row's amber row-focus cursor. A no-op while editing (the field's `▲▼` box is the cursor
/// then) or when unselected, so the two focus levels never both light up.
pub(crate) fn row_cursor(cv: &mut impl Surface, area: Rectangle, selected: bool, editing: bool) {
    if selected && !editing {
        cv.round(area, 6, palette::AMBER);
    }
}

/// One centred value row flanked by ◄ ► triangles — the "rotate to switch" picker row shared by the
/// single-row settings screens ([`Units`](crate::screen::UnitsScreen),
/// [`Language`](crate::screen::LanguageScreen)). Always drawn as the cursor (these screens have one
/// row, always focused). Returns the row rectangle so a caller can lay out further content beneath
/// it (Units' consequence-preview rows). `y` is the row's top; the row is a fixed 50 px tall.
pub(crate) fn value_row_with_arrows(cv: &mut impl Surface, y: i32, w: i32, text: &str) -> Rectangle {
    let area = row_rect(y, w, 50);
    row_cursor(cv, area, true, false);
    let midy = area.top_left.y + area.size.height as i32 / 2;
    cv.text_vcentered(text, w / 2, (area.top_left.y, 50), Font::Body, TextAlign::Center, palette::INK);
    // ◄ and ► as filled triangles, inset from the row edges.
    let ax = area.top_left.x + 18;
    cv.triangle(Point::new(ax, midy - 9), Point::new(ax, midy + 9), Point::new(ax - 11, midy), palette::INK);
    let bx = area.top_left.x + area.size.width as i32 - 18;
    cv.triangle(Point::new(bx, midy - 9), Point::new(bx, midy + 9), Point::new(bx + 11, midy), palette::INK);
    area
}

/// One stat-ledger row — olive caption on the left, the Display value right-aligned with a small
/// unit suffix (baselines shared), and an optional climb/descent triangle just left of the value
/// (`Some(true)` = up). All text sits on the parchment — no pane; that look is reserved for the
/// riding grid's live tiles. Shared by the Route overview and the Paused page.
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
    // Display cap is 26 from `y + 6`, Label cap 18 from `y + 14` — both bottom out at `y + 32`.
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

/// One option in a guarded-action menu (Ride control, Route swap): a static label and a
/// `guard` flag marking the irreversible options that need a hold-to-confirm instead of a
/// plain press.
pub(crate) struct MenuItem {
    pub label: &'static str,
    pub guard: bool,
}

/// Draw a selected option row's background for the guarded-action menus: a plain `AMBER` fill for
/// an instant option, or — when `guard` is set — a `PARCHMENT_SHADE` base that fills in `fill`
/// tracking `hold_progress` (0.0–1.0). The caller draws the label. A no-op for an unselected row.
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

/// Layout of a guarded-action menu's option rows — the per-screen geometry
/// [`draw_guarded_rows`] lays [`MenuItem`]s out with. The label offsets are from the row's
/// top-left, hand-tuned per screen (the two panels frame their rows differently).
pub(crate) struct GuardedRowsGeometry {
    /// Left edge and width of every row.
    pub x: i32,
    pub w: i32,
    /// Top of the first row.
    pub top: i32,
    /// Row height and the vertical gap between rows.
    pub row_h: i32,
    pub gap: i32,
    /// The label anchor, relative to the row's top-left.
    pub label_dx: i32,
    pub label_dy: i32,
}

impl GuardedRowsGeometry {
    /// The **card** family — the option rows of a full-bleed confirm card (Route received /
    /// updated, Trip received, Route swap, Trip delete, Nav route): a 12 px side inset, 46 px rows
    /// 8 apart, the label 16 in and 11 down. Only where the block starts differs between them.
    pub(crate) fn card(w: i32, top: i32) -> Self {
        GuardedRowsGeometry { x: 12, w: w - 24, top, row_h: 46, gap: 8, label_dx: 16, label_dy: 11 }
    }

    /// The **panel** family — action rows inside a framed panel (Pause menu, Route overview, Ride
    /// detail): the wider 14 px inset the frame wants, and a tighter label at 12 in / 5 down. Row
    /// height and gap stay the caller's, since each panel sizes its block to the space it has.
    pub(crate) fn panel(w: i32, top: i32, row_h: i32, gap: i32) -> Self {
        GuardedRowsGeometry { x: 14, w: w - 28, top, row_h, gap, label_dx: 12, label_dy: 5 }
    }
}

/// Draw a guarded-action menu's option rows (Ride control, Route swap): each [`MenuItem`] gets its
/// [`confirm_row`] background — the amber cursor, or the hold-progress fill in `fill` on a guarded
/// row — and its Body label. The caller draws its chrome (the PAUSED panel / the full-frame prompt)
/// and keeps its `handle` semantics.
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
