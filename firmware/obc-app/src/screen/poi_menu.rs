//! The POI **category** screen — the first step of the POIs browser (Menu → POIs). Six rows in
//! fixed category-id order ([`PoiCategory::ALL`]), each a small icon in the main-menu pixel style
//! plus the category's [`name()`](PoiCategory::name). Selecting one opens the
//! [`PoiListScreen`](super::PoiListScreen) for that category; `back` returns to the Menu.
//!
//! Names only, no counts (house style — a count would also cost a `nearest_pois` query per row on
//! entry). The list itself reuses the shared [`list`](super::list) widget; only the per-row body
//! (icon + name) and the Press semantics are local, mirroring the Route menu.

use embedded_graphics::prelude::Point;
use obc_reader::PoiCategory;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::Msg;

use super::list::{self, ListGeometry, Separators};
use super::{palette, Ctx, PoiListScreen, Render, Screen, Transition};

/// Per-category row height — a Body-tier row with an amber highlight + padding, matching the nav
/// menus. Six rows fit the list area at this pitch.
const ROW_H: i32 = 52;

/// The category list. State is just the highlighted category.
#[derive(Debug, Default)]
pub struct PoiMenuScreen {
    selected: usize,
}

impl PoiMenuScreen {
    pub fn new() -> Self {
        PoiMenuScreen { selected: 0 }
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        let len = PoiCategory::ALL.len();
        match g {
            Gesture::Step(n) => list::on_step(&mut self.selected, n, len),
            Gesture::Press => {
                let cat = PoiCategory::ALL[self.selected.min(len - 1)];
                Transition::Push(Screen::PoiList(PoiListScreen::new(cat)))
            }
            Gesture::Back => Transition::Pop, // return to the Menu
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        let total = PoiCategory::ALL.len();
        let geo = ListGeometry::below_title(w, h, ROW_H, 8, 16, Separators::All);
        list::list_frame(cv, w, h, rx.t(Msg::PoiMenuTitle), self.selected + 1, total, geo.visible);

        let first = list::window_start(self.selected, geo.visible, total) as i32;
        list::draw_rows(cv, geo, total, self.selected, first, |cv, row| {
            let cat = PoiCategory::ALL[row.index];
            let a = row.area;
            let mid = a.top_left.y + a.size.height as i32 / 2;
            // Icon in a fixed left gutter, then the name — same rhythm as the nav-menu bullet+label.
            let ink = if row.selected { INK } else { SUBTEXT };
            let bg = if row.selected { AMBER } else { PARCHMENT };
            draw_category_icon(cv, cat, Point::new(a.top_left.x + 22, mid), ink, bg);
            cv.text(rx.t(category_msg(cat)), Point::new(a.top_left.x + 44, mid - 14), Font::Body, TextAlign::Left, INK);
        });
    }
}

/// The catalog key for a category's name (epic #602 + #946). [`PoiCategory::name`] is the
/// format crate's English label — fine for a spec dump, wrong on glass — so every screen that
/// *shows* a category (this menu, the POI list's title, the Up-ahead picker) resolves it here, and
/// the three can't drift into three spellings.
pub(super) fn category_msg(cat: PoiCategory) -> Msg {
    match cat {
        PoiCategory::Water => Msg::PoiCatWater,
        PoiCategory::Campsite => Msg::PoiCatCampsite,
        PoiCategory::Accommodation => Msg::PoiCatAccommodation,
        PoiCategory::Resupply => Msg::PoiCatResupply,
        PoiCategory::Pharmacy => Msg::PoiCatPharmacy,
        PoiCategory::BikeShop => Msg::PoiCatBikeShop,
    }
}

/// Dispatch a category's pixel icon, centred at `c`. `bg` is the surface behind it, for punched-out
/// details (the same authoring path as the main Menu's [`draw_icon`](super::menu)). Every glyph is
/// hand-drawn from `Surface` primitives at a fixed ~20 px box, sized for one list row — which is
/// also a Body line's height, so the [POI detail](super::PoiDetailScreen)'s name row and the
/// create-route confirm's glyph slot reuse these exact fns unscaled (#685).
pub(super) fn draw_category_icon(cv: &mut impl Surface, cat: PoiCategory, c: Point, color: u16, bg: u16) {
    match cat {
        PoiCategory::Water => icon_water(cv, c, color, bg),
        PoiCategory::Campsite => icon_campsite(cv, c, color),
        PoiCategory::Accommodation => icon_accommodation(cv, c, color, bg),
        PoiCategory::Resupply => icon_resupply(cv, c, color, bg),
        PoiCategory::Pharmacy => icon_pharmacy(cv, c, color, bg),
        PoiCategory::BikeShop => icon_bike(cv, c, color, bg),
    }
}

/// A water drop: a disc base with a tapering tip, and a small punched highlight.
fn icon_water(cv: &mut impl Surface, c: Point, color: u16, bg: u16) {
    let base = Point::new(c.x, c.y + 3);
    cv.disc(base, 7, color);
    cv.triangle(Point::new(c.x - 6, c.y + 1), Point::new(c.x + 6, c.y + 1), Point::new(c.x, c.y - 9), color);
    // Highlight glint, upper-left of the drop.
    cv.disc(Point::new(c.x - 2, c.y + 1), 2, bg);
}

/// A tent: two roof slopes to a ridge peak, with a punched door notch drawn in the fill colour as a
/// solid triangle over the parchment ground line.
fn icon_campsite(cv: &mut impl Surface, c: Point, color: u16) {
    let base_y = c.y + 8;
    let peak = Point::new(c.x, c.y - 9);
    // Left and right roof panels meeting at the peak.
    cv.triangle(peak, Point::new(c.x - 11, base_y), Point::new(c.x - 1, base_y), color);
    cv.triangle(peak, Point::new(c.x + 11, base_y), Point::new(c.x + 1, base_y), color);
    // Ground line.
    cv.line(Point::new(c.x - 12, base_y), Point::new(c.x + 12, base_y), color);
}

/// A bed: a headboard post, a mattress bar, and two legs — the lodging glyph.
fn icon_accommodation(cv: &mut impl Surface, c: Point, color: u16, _bg: u16) {
    let (l, r) = (c.x - 11, c.x + 11);
    let top = c.y - 2;
    // Headboard (left), pillow bump, mattress top bar.
    cv.vline(l, c.y - 8, 12, 2, color);
    cv.fill(rect(l, top, r - l, 4), color);
    cv.disc(Point::new(l + 6, top - 1), 3, color);
    // Two legs.
    cv.vline(l, c.y + 2, 6, 2, color);
    cv.vline(r - 1, c.y + 2, 6, 2, color);
}

/// A shopping basket: a trapezoid body with a punched interior and a small handle arc — resupply.
fn icon_resupply(cv: &mut impl Surface, c: Point, color: u16, bg: u16) {
    // Handle arc: two short strokes rising from the rim.
    cv.line(Point::new(c.x - 5, c.y - 5), Point::new(c.x - 3, c.y - 10), color);
    cv.line(Point::new(c.x + 5, c.y - 5), Point::new(c.x + 3, c.y - 10), color);
    cv.line(Point::new(c.x - 3, c.y - 10), Point::new(c.x + 3, c.y - 10), color);
    // Basket body: a filled trapezoid (wide rim, narrow base), then punched hollow.
    cv.triangle(Point::new(c.x - 11, c.y - 4), Point::new(c.x + 11, c.y - 4), Point::new(c.x - 7, c.y + 9), color);
    cv.triangle(Point::new(c.x + 11, c.y - 4), Point::new(c.x + 7, c.y + 9), Point::new(c.x - 7, c.y + 9), color);
    cv.triangle(Point::new(c.x - 8, c.y - 1), Point::new(c.x + 8, c.y - 1), Point::new(c.x - 5, c.y + 6), bg);
    cv.triangle(Point::new(c.x + 8, c.y - 1), Point::new(c.x + 5, c.y + 6), Point::new(c.x - 5, c.y + 6), bg);
}

/// A medical cross in a rounded tile — pharmacy. The plus is punched out of a filled square.
fn icon_pharmacy(cv: &mut impl Surface, c: Point, color: u16, bg: u16) {
    cv.round(rect(c.x - 10, c.y - 10, 20, 20), 4, color);
    // Punched cross: a vertical and a horizontal bar in the background colour.
    cv.fill(rect(c.x - 2, c.y - 7, 5, 15), bg);
    cv.fill(rect(c.x - 7, c.y - 2, 15, 5), bg);
}

/// A bicycle: two spoked wheels and a proper diamond frame with a saddle + handlebar — the bike-shop
/// glyph (echoes the project's bike-computer identity).
fn icon_bike(cv: &mut impl Surface, c: Point, color: u16, bg: u16) {
    // Wheels as rims: a filled disc with the hub punched out, plus a small hub dot.
    let rear = Point::new(c.x - 8, c.y + 6);
    let front = Point::new(c.x + 8, c.y + 6);
    for wheel in [rear, front] {
        cv.disc(wheel, 5, color);
        cv.disc(wheel, 3, bg);
        cv.disc(wheel, 1, color);
    }
    // Diamond frame: bottom bracket low-centre, saddle up-and-back, head tube up-and-front.
    let bb = Point::new(c.x - 1, c.y + 6);
    let saddle = Point::new(c.x - 5, c.y - 5);
    let head = Point::new(c.x + 5, c.y - 4);
    cv.line(rear, bb, color); // chainstay
    cv.line(bb, saddle, color); // seat tube
    cv.line(saddle, head, color); // top tube
    cv.line(head, bb, color); // down tube
    cv.line(head, front, color); // head tube + fork
    cv.line(rear, saddle, color); // seat stay
                                  // Saddle bar + handlebar stub.
    cv.line(Point::new(saddle.x - 3, saddle.y), Point::new(saddle.x + 2, saddle.y), color);
    cv.line(Point::new(head.x - 1, head.y - 2), Point::new(head.x + 4, head.y - 3), color);
}
