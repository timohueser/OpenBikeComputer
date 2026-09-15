//! Production Find card geometry, backed by prepared place and visit facts.
use super::{
    palette::*,
    vocab::{chrome::title_frame, list},
    Ctx, RenderFrame, Screen, Transition,
};
use crate::{
    find_place::{Action, Costs, State},
    navigator::ReviewStatus,
    Gesture,
};
use core::fmt::Write;
use embedded_graphics::{draw_target::DrawTarget, prelude::Point};
use obc_reader::PoiCategory;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, Surface, Viewport,
};

#[derive(Debug)]
pub struct FindPlaceScreen {
    category: Option<PoiCategory>,
    selected: usize,
}
impl Default for FindPlaceScreen {
    fn default() -> Self {
        Self::new()
    }
}
impl FindPlaceScreen {
    pub fn new() -> Self {
        Self { category: None, selected: 0 }
    }
    pub(crate) fn choices(&self) -> bool {
        self.category.is_some()
    }
    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        if let Some(category) = self.category {
            let len = cx.find.results.len().max(1);
            match g {
                Gesture::Step(n) => {
                    self.selected = list::step_selection(self.selected, n, len + 2);
                }
                Gesture::Back => {
                    self.category = None;
                    self.selected = 0;
                }
                Gesture::Press if self.selected == len => {
                    cx.find.action = Action::More;
                }
                Gesture::Press if self.selected == len + 1 => {
                    cx.find.action = Action::Refresh;
                    self.selected = 0;
                }
                Gesture::Press if cx.find.state == State::Ready => {
                    if let Some(poi) = cx
                        .find
                        .selected(self.selected, cx.poi_scratch, cx.corridor)
                        .filter(|p| p.opening != obc_reader::hours::OpeningStatus::Closed)
                    {
                        let mut poi = poi.clone();
                        poi.distance_m = obc_map_scene::ground_dist_m(cx.find.origin, (poi.lon, poi.lat)) as u32;
                        return Transition::Push(Screen::PoiDetail(super::PoiDetailScreen::new(poi)));
                    }
                }
                _ => {}
            }
            cx.find.category = category;
        } else {
            match g {
                Gesture::Step(n) => self.selected = list::step_selection(self.selected, n, PoiCategory::ALL.len()),
                Gesture::Press => {
                    let cat = PoiCategory::ALL[self.selected];
                    self.category = Some(cat);
                    cx.find.category = cat;
                    cx.find.action = Action::Refresh;
                    self.selected = 0;
                }
                Gesture::Back => return Transition::Pop,
                _ => {}
            }
        }
        Transition::None
    }
    pub fn draw<D, F, S>(&self, cv: &mut Canvas<D, F>, rx: &mut RenderFrame<'_, S>)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: obc_map_scene::MapScene,
    {
        let Some(category) = self.category else {
            title_frame(cv, rx.w, rx.h, "Find a place", "");
            let first = list::window_start(self.selected, 6, PoiCategory::ALL.len());
            for (slot, cat) in PoiCategory::ALL.iter().skip(first).take(6).enumerate() {
                let y = 43 + slot as i32 * 44;
                if first + slot == self.selected {
                    cv.round(rect(10, y, rx.w - 20, 40), 6, AMBER);
                }
                cv.text(
                    rx.t(super::poi_menu::category_msg(*cat)),
                    Point::new(18, y + 5),
                    Font::Body,
                    TextAlign::Left,
                    INK,
                );
            }
            return;
        };
        let mut min = rx.find.origin;
        let mut max = min;
        for i in 0..rx.find.results.len() {
            if let Some(p) = rx.find.selected(i, rx.poi_scratch, rx.corridor) {
                min = (min.0.min(p.lon), min.1.min(p.lat));
                max = (max.0.max(p.lon), max.1.max(p.lat));
            }
        }
        let vp = fit(min, max, rx.w, rx.h, 208);
        let _ = super::map::draw_map_scene(cv, rx, &vp, None);
        for i in 0..rx.find.results.len() {
            if let Some(p) = rx.find.selected(i, rx.poi_scratch, rx.corridor) {
                let (x, y) = vp.to_screen(p.lon, p.lat);
                cv.round(rect(x - 11, y - 12, 23, 24), 4, if i == self.selected { AMBER } else { PARCHMENT });
                cv.text(letter(i), Point::new(x, y - 12), Font::Label, TextAlign::Center, INK);
            }
        }
        cv.fill(rect(0, 0, rx.w, 40), PARCHMENT);
        cv.round(rect(4, 4, rx.w - 8, 34), 6, WOOD);
        cv.text(
            rx.t(super::poi_menu::category_msg(category)),
            Point::new(14, 8),
            Font::Body,
            TextAlign::Left,
            PARCHMENT,
        );
        cv.fill(rect(0, 208, rx.w, rx.h - 208), PARCHMENT);
        cv.round(rect(10, 210, rx.w - 20, 104), 6, AMBER);
        let count = rx.find.results.len();
        if self.selected >= count.max(1) {
            let more = self.selected == count.max(1);
            cv.text(
                if more { "More places" } else { "Refresh" },
                Point::new(18, 236),
                Font::Body,
                TextAlign::Left,
                INK,
            );
            cv.text(
                if more {
                    if rx.route.is_some() {
                        "10km / 20km ahead"
                    } else {
                        "10km shortlist"
                    }
                } else {
                    "Search again"
                },
                Point::new(18, 264),
                Font::Label,
                TextAlign::Left,
                SUBTEXT,
            );
            if more {
                cv.text("More: 50km partial", Point::new(18, 288), Font::Label, TextAlign::Left, SUBTEXT);
            }
            return;
        }
        let Some(poi) =
            rx.find.selected(self.selected, rx.poi_scratch, rx.corridor).filter(|_| rx.find.state == State::Ready)
        else {
            let title = match rx.find.state {
                State::NoFix => "GPS required",
                State::NoMap => "No map",
                State::NoAccess => "No graph",
                State::Failed => "Data error",
                State::Stale => "Refresh needed",
                State::Ready => "No choices",
                State::Empty => "None found",
                _ => "Finding places",
            };
            cv.text(title, Point::new(18, 236), Font::Body, TextAlign::Left, INK);
            cv.text(
                if matches!(rx.find.state, State::Empty | State::Ready) { "Partial search" } else { "More / Refresh" },
                Point::new(18, 270),
                Font::Label,
                TextAlign::Left,
                SUBTEXT,
            );
            return;
        };
        let Some(cost) = rx.find.costs(self.selected) else { return };
        let mut role = heapless::String::<32>::new();
        let _ = write!(role, "{} {}", letter(self.selected), if cost.on_way() { "On the way" } else { "Nearby" });
        cv.text(&role, Point::new(18, 212), Font::Label, TextAlign::Left, SUBTEXT);
        let mut number = heapless::String::<8>::new();
        let _ = write!(number, "{}/{}", self.selected + 1, count);
        cv.text(&number, Point::new(rx.w - 18, 212), Font::Label, TextAlign::Right, INK);
        let name = if poi.name.is_empty() {
            obc_formats::obcm::poi_label_of(poi.subtype).unwrap_or("Place")
        } else {
            poi.name.as_str()
        };
        cv.text(&super::poi_list::fit(name, 15), Point::new(18, 236), Font::Body, TextAlign::Left, INK);
        if poi.opening == obc_reader::hours::OpeningStatus::Closed {
            cv.text("Closed", Point::new(18, 264), Font::Label, TextAlign::Left, WARNING);
            return;
        }
        figures(cv, cost.arrival_m, cost.arrival_ascent_m, 264, false);
        let mut line = heapless::String::<32>::new();
        if let Some(extra) = cost.added_m {
            super::vocab::fmt::write_distance_coarse(&mut line, "+", extra, rx.settings.units);
            let _ = line.push_str(" extra");
        } else {
            let _ = line.push_str("Destination");
        }
        cv.text(&line, Point::new(18, 288), Font::Label, TextAlign::Left, INK);
    }
}

#[derive(Debug)]
pub struct VisitReviewScreen {
    name: heapless::String<32>,
}
impl VisitReviewScreen {
    pub fn new(name: &str) -> Self {
        let mut title = heapless::String::new();
        for c in name.chars() {
            if title.push(c).is_err() {
                break;
            }
        }
        Self { name: title }
    }
    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Back => {
                cx.find.action = Action::Cancel;
                Transition::Pop
            }
            Gesture::Press if cx.find.review == ReviewStatus::Preview => {
                cx.find.action = Action::Accept;
                Transition::None
            }
            Gesture::Press if cx.find.review == ReviewStatus::Accepted => {
                Transition::Root(Screen::Map(super::MapScreen::new()))
            }
            _ => Transition::None,
        }
    }
    pub fn draw<D, F, S>(&self, cv: &mut Canvas<D, F>, rx: &mut RenderFrame<'_, S>)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: obc_map_scene::MapScene,
    {
        let points = rx.nav_preview;
        let vp = if !points.is_empty() && matches!(rx.find.review, ReviewStatus::Preview | ReviewStatus::Saving) {
            let (mut min, mut max) = (points[0], points[0]);
            for &(lon, lat) in points {
                min = (min.0.min(lon), min.1.min(lat));
                max = (max.0.max(lon), max.1.max(lat));
            }
            fit(min, max, rx.w, rx.h, 192)
        } else {
            rx.state.viewport(rx.w as f32, rx.h as f32)
        };
        let _ = super::map::draw_map_scene(cv, rx, &vp, None);
        if matches!(rx.find.review, ReviewStatus::Preview | ReviewStatus::Saving) {
            if let Some(scratch) = rx.scratch.as_deref_mut() {
                let (target, color) = cv.split();
                scratch.stroke_path(target, &vp, points.iter().copied(), color(DETOUR), super::ROUTE_WEIGHT);
            }
        }
        cv.fill(rect(0, 0, rx.w, 40), PARCHMENT);
        cv.round(rect(4, 4, rx.w - 8, 34), 6, WOOD);
        cv.text(&super::poi_list::fit(&self.name, 18), Point::new(14, 8), Font::Label, TextAlign::Left, PARCHMENT);
        cv.fill(rect(0, 192, rx.w, rx.h - 192), PARCHMENT);
        if let Some(Costs { arrival_m, arrival_ascent_m, added_m, added_ascent_m }) = rx.find.review_costs {
            figures(cv, arrival_m, arrival_ascent_m, 196, false);
            cv.text(
                if added_m.is_some() { "Extra incl. return" } else { "Direct destination" },
                Point::new(14, 222),
                Font::Label,
                TextAlign::Left,
                SUBTEXT,
            );
            if let Some(m) = added_m {
                figures(cv, m, added_ascent_m, 248, true);
            }
        }
        let label = match rx.find.review {
            ReviewStatus::Planning => "Calculating",
            ReviewStatus::Preview if rx.find.review_costs.is_some_and(|c| c.added_m.is_some()) => "Add stop",
            ReviewStatus::Preview => "Go here",
            ReviewStatus::Saving => "Saving",
            ReviewStatus::Accepted => "Back to map",
            _ => "Visit unavailable",
        };
        cv.round(rect(12, 280, 216, 32), 6, AMBER);
        cv.text(label, Point::new(120, 282), Font::Body, TextAlign::Center, INK);
    }
}
fn letter(i: usize) -> &'static str {
    ["A", "B", "C", "D"][i.min(3)]
}
fn fit(min: (i32, i32), max: (i32, i32), w: i32, h: i32, bottom: i32) -> Viewport {
    let lat = min.1 + (max.1 - min.1) / 2;
    let aspect = obc_map_scene::cos_lat(lat);
    let zoom = ((w - 48) as f32 / ((max.0 - min.0).max(200) as f32 * aspect))
        .min((bottom - 84) as f32 / (max.1 - min.1).max(200) as f32);
    Viewport::new(
        w as f32,
        h as f32,
        min.0 + (max.0 - min.0) / 2,
        lat - ((h as f32 / 2.0 - (40 + bottom) as f32 / 2.0) / zoom) as i32,
        zoom,
    )
}
fn figures(cv: &mut impl Surface, distance: u32, climb: Option<u32>, y: i32, extra: bool) {
    let mut d = heapless::String::<20>::new();
    if extra {
        let _ = d.push('+');
    }
    if distance < 1000 {
        let _ = write!(d, "{distance}m");
    } else {
        let _ = write!(d, "{}.{:01}km", distance / 1000, distance % 1000 / 100);
    }
    cv.text(&d, Point::new(18, y), Font::Label, TextAlign::Left, INK);
    cv.triangle(Point::new(125, y + 18), Point::new(132, y + 6), Point::new(139, y + 18), INK);
    let mut c = heapless::String::<16>::new();
    if let Some(n) = climb {
        let _ = write!(c, "{n}m");
    } else {
        let _ = c.push_str("Unknown");
    }
    cv.text(&c, Point::new(146, y), Font::Label, TextAlign::Left, INK);
}
