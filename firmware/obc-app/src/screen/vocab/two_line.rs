//! The two-line list row of the Route menu and the Rides list: the name on line 1, and line 2 one
//! size under it, olive on every row, the cursor's too.

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use super::list::{ListGeometry, RowCtx, Separators};
use super::marquee::MarqueeFrame;
use crate::screen::palette;

pub(crate) const NAME_FONT: Font = Font::Label;
pub(crate) const LINE2_FONT: Font = Font::Caption;
pub(crate) const LINE2: u16 = palette::SUBTEXT;

/// The list's margin from the panel edge, and the text's inset from the row area's left edge.
pub(crate) const SIDE_INSET: i32 = 12;
pub(crate) const NAME_INSET: i32 = 12;

/// The row pane: two lines, sized so four rows fill the list area under the title bar.
const ROW_H: i32 = 66;
const NAME_Y: i32 = 9;
const LINE2_Y: i32 = 36;

/// The list under the title bar, with a rule under every row but the cursor's.
pub(crate) fn geometry(w: i32, h: i32) -> ListGeometry {
    ListGeometry::below_title(w, h, ROW_H, 8, SIDE_INSET, Separators::Unselected)
}

/// The x where both lines start.
pub(crate) fn text_x(row: &RowCtx) -> i32 {
    row.area.top_left.x + NAME_INSET
}

fn area_right(row: &RowCtx) -> i32 {
    row.area.top_left.x + row.area.size.width as i32
}

/// The right edge of the name when nothing sits right of it.
pub(crate) fn name_right(row: &RowCtx) -> i32 {
    area_right(row) - 8
}

pub(crate) fn line2_right(row: &RowCtx) -> i32 {
    area_right(row) - 4
}

/// The centre of a mark of half-width `half` on line 1, `gap` px in from the row area's right edge.
pub(crate) fn right_mark(row: &RowCtx, half: i32, gap: i32) -> Point {
    mark_at(row, area_right(row) - gap - half)
}

/// The centre of a mark at `x` on line 1's cap.
pub(crate) fn mark_at(row: &RowCtx, x: i32) -> Point {
    Point::new(x, row.area.top_left.y + NAME_Y + NAME_FONT.cap_mid() as i32)
}

/// Line 1: `name` from `x` to `right`, cut, or scrolled on the cursor row.
pub(crate) fn name_line(
    cv: &mut impl Surface,
    row: &RowCtx,
    marquee: &MarqueeFrame,
    name: &str,
    (x, right): (i32, i32),
    color: u16,
) {
    let color = row_color(row, color);
    let name = marquee.fit(name, right - x, NAME_FONT, row.scroll());
    cv.text(&name, Point::new(x, row.area.top_left.y + NAME_Y), NAME_FONT, TextAlign::Left, color);
}

/// The top of line 2.
pub(crate) fn line2_y(row: &RowCtx) -> i32 {
    row.area.top_left.y + LINE2_Y
}

/// Text on the amber cursor keeps its contrast in both themes.
pub(crate) fn row_color(row: &RowCtx, color: u16) -> u16 {
    match (row.selected, color) {
        (true, palette::INK) => palette::ON_ACCENT,
        (true, palette::SUBTEXT) => palette::SUBTEXT_ON_ACCENT,
        _ => color,
    }
}
