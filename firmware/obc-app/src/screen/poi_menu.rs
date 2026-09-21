//! Shared place-category labels and icons.
use crate::Msg;
use embedded_graphics::prelude::Point;
use obc_reader::PoiCategory;
use obc_render::{rect, Surface};

/// The catalog key for the name of a category. `PoiCategory::name` is the English label of the
/// format crate, so every screen that shows a category resolves it here instead.
pub(crate) fn category_msg(cat: PoiCategory) -> Msg {
    match cat {
        PoiCategory::Water => Msg::PoiCatWater,
        PoiCategory::Campsite => Msg::PoiCatCampsite,
        PoiCategory::Accommodation => Msg::PoiCatAccommodation,
        PoiCategory::Resupply => Msg::PoiCatResupply,
        PoiCategory::Pharmacy => Msg::PoiCatPharmacy,
        PoiCategory::BikeShop => Msg::PoiCatBikeShop,
        PoiCategory::Train => Msg::PoiCatTrain,
    }
}

/// Draw the icon of a category, centred at `c`. `bg` is the surface behind it, for punched
/// details. Each glyph fills a fixed 20 px box, which is the height of one list row and of a Body
/// line, so the other screens use these functions unscaled.
pub(super) fn draw_category_icon(cv: &mut impl Surface, cat: PoiCategory, c: Point, color: u16, bg: u16) {
    match cat {
        PoiCategory::Water => icon_water(cv, c, color, bg),
        PoiCategory::Campsite => icon_campsite(cv, c, color),
        PoiCategory::Accommodation => icon_accommodation(cv, c, color, bg),
        PoiCategory::Resupply => icon_resupply(cv, c, color, bg),
        PoiCategory::Pharmacy => icon_pharmacy(cv, c, color, bg),
        PoiCategory::BikeShop => icon_bike(cv, c, color, bg),
        PoiCategory::Train => icon_train(cv, c, color, bg),
    }
}

/// A water drop: a disc base with a tapering tip, and a small punched highlight.
fn icon_water(cv: &mut impl Surface, c: Point, color: u16, bg: u16) {
    let base = Point::new(c.x, c.y + 3);
    cv.disc(base, 7, color);
    cv.triangle(Point::new(c.x - 6, c.y + 1), Point::new(c.x + 6, c.y + 1), Point::new(c.x, c.y - 9), color);
    cv.disc(Point::new(c.x - 2, c.y + 1), 2, bg);
}

/// A tent: two roof slopes to a ridge peak, over a ground line.
fn icon_campsite(cv: &mut impl Surface, c: Point, color: u16) {
    let base_y = c.y + 8;
    let peak = Point::new(c.x, c.y - 9);
    cv.triangle(peak, Point::new(c.x - 11, base_y), Point::new(c.x - 1, base_y), color);
    cv.triangle(peak, Point::new(c.x + 11, base_y), Point::new(c.x + 1, base_y), color);
    cv.line(Point::new(c.x - 12, base_y), Point::new(c.x + 12, base_y), color);
}

/// A bed: a headboard post, a mattress bar, and two legs — the lodging glyph.
fn icon_accommodation(cv: &mut impl Surface, c: Point, color: u16, _bg: u16) {
    let (l, r) = (c.x - 11, c.x + 11);
    let top = c.y - 2;
    cv.vline(l, c.y - 8, 12, 2, color);
    cv.fill(rect(l, top, r - l, 4), color);
    cv.disc(Point::new(l + 6, top - 1), 3, color);
    cv.vline(l, c.y + 2, 6, 2, color);
    cv.vline(r - 1, c.y + 2, 6, 2, color);
}

/// A shopping basket: a trapezoid body with a punched interior and a small handle arc — resupply.
fn icon_resupply(cv: &mut impl Surface, c: Point, color: u16, bg: u16) {
    cv.line(Point::new(c.x - 5, c.y - 5), Point::new(c.x - 3, c.y - 10), color);
    cv.line(Point::new(c.x + 5, c.y - 5), Point::new(c.x + 3, c.y - 10), color);
    cv.line(Point::new(c.x - 3, c.y - 10), Point::new(c.x + 3, c.y - 10), color);
    // The body is a filled trapezoid with the hollow punched out of it.
    cv.triangle(Point::new(c.x - 11, c.y - 4), Point::new(c.x + 11, c.y - 4), Point::new(c.x - 7, c.y + 9), color);
    cv.triangle(Point::new(c.x + 11, c.y - 4), Point::new(c.x + 7, c.y + 9), Point::new(c.x - 7, c.y + 9), color);
    cv.triangle(Point::new(c.x - 8, c.y - 1), Point::new(c.x + 8, c.y - 1), Point::new(c.x - 5, c.y + 6), bg);
    cv.triangle(Point::new(c.x + 8, c.y - 1), Point::new(c.x + 5, c.y + 6), Point::new(c.x - 5, c.y + 6), bg);
}

/// A medical cross in a rounded tile — pharmacy. The plus is punched out of a filled square.
fn icon_pharmacy(cv: &mut impl Surface, c: Point, color: u16, bg: u16) {
    cv.round(rect(c.x - 10, c.y - 10, 20, 20), 4, color);
    cv.fill(rect(c.x - 2, c.y - 7, 5, 15), bg);
    cv.fill(rect(c.x - 7, c.y - 2, 15, 5), bg);
}

/// A bicycle: two wheels and a diamond frame with a saddle and a handlebar.
fn icon_bike(cv: &mut impl Surface, c: Point, color: u16, bg: u16) {
    // Each wheel is a disc with the rim punched out, plus a hub dot.
    let rear = Point::new(c.x - 8, c.y + 6);
    let front = Point::new(c.x + 8, c.y + 6);
    for wheel in [rear, front] {
        cv.disc(wheel, 5, color);
        cv.disc(wheel, 3, bg);
        cv.disc(wheel, 1, color);
    }
    let bb = Point::new(c.x - 1, c.y + 6);
    let saddle = Point::new(c.x - 5, c.y - 5);
    let head = Point::new(c.x + 5, c.y - 4);
    cv.line(rear, bb, color); // chainstay
    cv.line(bb, saddle, color); // seat tube
    cv.line(saddle, head, color); // top tube
    cv.line(head, bb, color); // down tube
    cv.line(head, front, color); // head tube + fork
    cv.line(rear, saddle, color); // seat stay
    cv.line(Point::new(saddle.x - 3, saddle.y), Point::new(saddle.x + 2, saddle.y), color);
    cv.line(Point::new(head.x - 1, head.y - 2), Point::new(head.x + 4, head.y - 3), color);
}

fn icon_train(cv: &mut impl Surface, c: Point, color: u16, bg: u16) {
    cv.round(rect(c.x - 8, c.y - 10, 16, 18), 3, color);
    cv.fill(rect(c.x - 5, c.y - 7, 10, 7), bg);
    cv.disc(Point::new(c.x - 4, c.y + 4), 2, bg);
    cv.disc(Point::new(c.x + 4, c.y + 4), 2, bg);
    cv.line(Point::new(c.x - 4, c.y + 8), Point::new(c.x - 7, c.y + 11), color);
    cv.line(Point::new(c.x + 4, c.y + 8), Point::new(c.x + 7, c.y + 11), color);
}
