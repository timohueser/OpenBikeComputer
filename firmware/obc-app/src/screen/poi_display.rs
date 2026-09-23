//! Shared names, climb figures and side arrows for place displays.
use embedded_graphics::prelude::Point;
use obc_formats::obcm::poi_label_of;
use obc_render::text::{Font, TextAlign};
use obc_render::Surface;

pub const OFF_ROUTE_HINT_M: i32 = 50;
pub(super) const ARROW_W: i32 = 7;
pub(super) const ARROW_GAP: i32 = 4;

pub(crate) fn poi_row_name(poi: &obc_reader::Poi) -> &str {
    if poi.name.is_empty() {
        poi_label_of(poi.subtype).unwrap_or("POI")
    } else {
        poi.name.as_str()
    }
}

pub(super) fn draw_side_arrow(cv: &mut impl Surface, at: Point, to_right: bool, color: u16) {
    let (h, y) = (5, at.y);
    if to_right {
        cv.triangle(Point::new(at.x, y - h), Point::new(at.x, y + h), Point::new(at.x + ARROW_W, y), color);
    } else {
        cv.triangle(Point::new(at.x + ARROW_W, y - h), Point::new(at.x + ARROW_W, y + h), Point::new(at.x, y), color);
    }
}

/// The climb figure on a Label line at `y`: a filled up-triangle at `x`, then `value`. The triangle
/// is drawn because the device font has no arrow glyph.
pub(super) fn draw_climb_figure(cv: &mut impl Surface, x: i32, y: i32, value: &str) {
    use super::palette::INK;
    cv.triangle(Point::new(x, y + 18), Point::new(x + 7, y + 6), Point::new(x + 14, y + 18), INK);
    cv.text(value, Point::new(x + 21, y), Font::Label, TextAlign::Left, INK);
}
