//! Production Find card geometry, backed by prepared place and visit facts.
use super::{
    palette::*,
    vocab::{chrome::title_frame, list},
    Ctx, RenderFrame, Screen, Transition,
};
use crate::{
    find_place::{Action, Costs, State},
    navigator::ReviewStatus,
    Gesture, Msg,
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
    pub(crate) fn refresh_selection(&mut self) {
        self.selected = 0;
    }
    pub(crate) fn choices(&self) -> bool {
        self.category.is_some()
    }
    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        if let Some(category) = self.category {
            let len = cx.find.results.len().max(1);
            match g {
                Gesture::Step(n) => {
                    self.selected = list::step_selection(self.selected, n, len + 1);
                }
                Gesture::Back => {
                    self.category = None;
                    self.selected = 0;
                }
                Gesture::Press if self.selected == len => {
                    cx.find.action = Action::More;
                    self.selected = 0;
                }
                Gesture::Press
                    if cx.find.state == State::Ready
                        && cx.find.selected(self.selected, cx.poi_scratch, cx.corridor).is_some() =>
                {
                    cx.find.action = Action::Preview(self.selected as u8);
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
                    if cx.find.category != cat || cx.find.state != State::Ready {
                        cx.find.action = Action::Refresh;
                    }
                    cx.find.category = cat;
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
            title_frame(cv, rx.w, rx.h, rx.t(Msg::AssistantFind), "");
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
            cv.text(rx.t(Msg::AssistantMorePlaces), Point::new(rx.w / 2, 232), Font::Body, TextAlign::Center, INK);
            cv.text(
                rx.t(Msg::AssistantBrowsePlaces),
                Point::new(rx.w / 2, 270),
                Font::Label,
                TextAlign::Center,
                SUBTEXT,
            );
            return;
        }
        let Some(poi) =
            rx.find.selected(self.selected, rx.poi_scratch, rx.corridor).filter(|_| rx.find.state == State::Ready)
        else {
            let title = match rx.find.state {
                State::NoFix => rx.t(Msg::AssistantNoFix),
                State::NoMap => rx.t(Msg::AssistantNoMap),
                State::NoAccess => rx.t(Msg::AssistantNoGraph),
                State::Failed => rx.t(Msg::AssistantDataError),
                State::Stale => rx.t(Msg::AssistantSearchChanged),
                State::Ready => rx.t(Msg::AssistantNoChoices),
                State::Empty => rx.t(Msg::AssistantNoneFound),
                _ => rx.t(Msg::AssistantFinding),
            };
            cv.text(title, Point::new(18, 236), Font::Body, TextAlign::Left, INK);
            cv.text(
                if matches!(rx.find.state, State::Empty | State::Ready) {
                    rx.t(Msg::AssistantPartial)
                } else {
                    rx.t(Msg::AssistantMorePlaces)
                },
                Point::new(18, 270),
                Font::Label,
                TextAlign::Left,
                SUBTEXT,
            );
            return;
        };
        let Some(cost) = rx.find.costs(self.selected) else { return };
        cv.text(letter(self.selected), Point::new(18, 212), Font::Label, TextAlign::Left, SUBTEXT);
        let mut number = heapless::String::<8>::new();
        let _ = write!(number, "{}/{}", self.selected + 1, count);
        cv.text(&number, Point::new(rx.w - 18, 212), Font::Label, TextAlign::Right, INK);
        let name = if poi.name.is_empty() {
            obc_formats::obcm::poi_label_of(poi.subtype).unwrap_or(rx.t(Msg::AssistantPlace))
        } else {
            poi.name.as_str()
        };
        let name_row = rect(18, 236, 15 * Font::Body.char_width() as i32, Font::Body.line_height() as i32);
        let name = rx.marquee.fit(name, 15, Some(name_row));
        cv.text(&name, Point::new(18, 236), Font::Body, TextAlign::Left, INK);
        if poi.opening == obc_reader::hours::OpeningStatus::Closed {
            cv.text(rx.t(Msg::AssistantClosed), Point::new(48, 212), Font::Label, TextAlign::Left, WARNING);
        }
        figures(cv, cost.arrival_m, cost.arrival_ascent_m, 264, false, rx.settings.units);
        let mut line = heapless::String::<32>::new();
        if let Some(extra) = cost.added_m {
            super::vocab::fmt::write_distance_coarse(&mut line, "+", extra, rx.settings.units);
            let _ = write!(line, " {}", rx.t(Msg::AssistantExtra));
        } else {
            let _ = line.push_str(rx.t(Msg::AssistantDestination));
        }
        cv.text(&line, Point::new(18, 288), Font::Label, TextAlign::Left, INK);
    }
}

#[derive(Debug)]
pub struct VisitReviewScreen {
    pub(crate) accepted: bool,
    pub(crate) returning: bool,
    pub(crate) error: Option<crate::navigator::VisitUnavailable>,
    cancel_selected: bool,
    route_choices: bool,
    pub(crate) destination: bool,
    pub(crate) pending_target: Option<obc_route::visit::VisitTarget>,
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
        Self {
            name: title,
            accepted: false,
            returning: false,
            error: None,
            cancel_selected: false,
            route_choices: false,
            destination: false,
            pending_target: None,
        }
    }
    pub(crate) fn route_choices(mut self, available: bool) -> Self {
        self.route_choices = available;
        self
    }
    pub(crate) fn accepted(name: &str) -> Self {
        let mut screen = Self::new(name);
        screen.accepted = true;
        screen
    }
    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Back if self.error.is_some() => {
                self.error = None;
                Transition::None
            }
            Gesture::Back if self.accepted => Transition::Pop,
            Gesture::Step(n) if self.route_choices && cx.find.review == ReviewStatus::Preview && n % 2 != 0 => {
                cx.find.action = Action::RouteMode(!self.destination);
                Transition::None
            }
            Gesture::Step(_) if self.accepted && cx.find.review == ReviewStatus::Accepted => {
                self.cancel_selected = !self.cancel_selected;
                Transition::None
            }
            Gesture::Press if self.accepted && self.cancel_selected && cx.find.review == ReviewStatus::Accepted => {
                cx.find.action = Action::CancelVisit;
                Transition::None
            }
            Gesture::Back
                if self.returning && matches!(cx.find.review, ReviewStatus::Saving | ReviewStatus::Unresolved) =>
            {
                Transition::Pop
            }
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
        let visible = matches!(rx.find.review, ReviewStatus::Preview | ReviewStatus::Saving)
            || (self.accepted && rx.find.review == ReviewStatus::Accepted);
        let gap = rx.visit_target.and_then(|target| {
            let approach = target.metadata.approach?;
            let endpoint = (approach.lon, approach.lat);
            let meters = obc_map_scene::ground_dist_m(target.display, endpoint) as u32;
            (!self.accepted && meters > 100).then_some((endpoint, meters))
        });
        let vp = if !points.is_empty() && visible {
            let (mut min, mut max) = (points[0], points[0]);
            for &(lon, lat) in points {
                min = (min.0.min(lon), min.1.min(lat));
                max = (max.0.max(lon), max.1.max(lat));
            }
            if let Some(target) = rx.visit_target {
                min = (min.0.min(target.display.0), min.1.min(target.display.1));
                max = (max.0.max(target.display.0), max.1.max(target.display.1));
            }
            fit(min, max, rx.w, rx.h, if self.accepted || self.returning { 192 } else { 168 })
        } else {
            rx.state.viewport(rx.w as f32, rx.h as f32)
        };
        let marker_color = super::map::draw_map_scene(cv, rx, &vp, None);
        if visible {
            if let Some(scratch) = rx.scratch.as_deref_mut() {
                let (target, color) = cv.split();
                scratch.stroke_path(target, &vp, points.iter().copied(), color(DETOUR), 9);
            }
            if let Some(target) = rx.visit_target {
                let (x, y) = vp.to_screen(target.display.0, target.display.1);
                let place = Point::new(x, y);
                if let Some((endpoint, _)) = gap {
                    let (x, y) = vp.to_screen(endpoint.0, endpoint.1);
                    let end = Point::new(x, y);
                    dotted_connector(cv, end, place);
                    cv.disc(end, 7, PARCHMENT);
                    cv.disc(end, 5, DETOUR);
                    cv.disc(end, 2, PARCHMENT);
                }
                destination_pin(cv, place);
            }
            if let (Some(fix), Some(color), Some(scratch)) =
                (rx.state.user_fix, marker_color, rx.scratch.as_deref_mut())
            {
                let (target, colors) = cv.split();
                scratch.draw_marker(target, &vp, fix.lon, fix.lat, fix.course, colors(color));
            }
            if !self.accepted && !self.returning {
                let hours = opening_hours(rx.poi_scratch.detail_schedule.as_ref(), rx.place_local);
                let closed = rx
                    .poi_scratch
                    .detail_schedule
                    .is_some_and(|s| s.status(rx.place_local) == obc_reader::hours::OpeningStatus::Closed);
                let missing = rx.t(if closed { Msg::AssistantClosed } else { Msg::PoiDetailHoursNotListed });
                let label = hours.as_ref().map_or(missing, |hours| hours.as_str());
                let width = label.chars().count() as i32 * Font::Label.char_width() as i32 + 12;
                cv.round(rect(8, 164, width, 26), 4, PARCHMENT);
                cv.text(label, Point::new(14, 164), Font::Label, TextAlign::Left, INK);
            }
        }
        cv.fill(rect(0, 0, rx.w, 40), PARCHMENT);
        cv.round(rect(4, 4, rx.w - 8, 34), 6, WOOD);
        let title = rx.marquee.fit(&self.name, 18, Some(rect(4, 4, rx.w - 8, 34)));
        cv.text(&title, Point::new(14, 8), Font::Label, TextAlign::Left, PARCHMENT);
        cv.fill(rect(0, 192, rx.w, rx.h - 192), PARCHMENT);
        if let Some(Costs { arrival_m, arrival_ascent_m, added_m, added_ascent_m }) = rx.find.review_costs {
            figures(cv, arrival_m, arrival_ascent_m, 196, false, rx.settings.units);
            cv.text(
                if self.accepted {
                    rx.t(Msg::AssistantCurrentLeg)
                } else if added_m.is_some() {
                    rx.t(Msg::AssistantReturnExtra)
                } else {
                    rx.t(Msg::AssistantDirect)
                },
                Point::new(14, 222),
                Font::Label,
                TextAlign::Left,
                SUBTEXT,
            );
            if let Some(m) = added_m {
                figures(cv, m, added_ascent_m, 248, true, rx.settings.units);
            }
        }
        if self.accepted && rx.find.review == ReviewStatus::Accepted && self.error.is_none() {
            for (index, label) in [Msg::AssistantBackMap, Msg::AssistantCancelVisit].iter().enumerate() {
                let y = 240 + index as i32 * 38;
                cv.round(rect(12, y, 216, 32), 6, if self.cancel_selected == (index == 1) { AMBER } else { PARCHMENT });
                cv.text(rx.t(*label), Point::new(120, y + 2), Font::Body, TextAlign::Center, INK);
            }
            return;
        }
        let label = if self.error.is_some() {
            rx.t(if self.returning { Msg::AssistantUnavailable } else { Msg::AssistantVisitUnavailable })
        } else {
            match rx.find.review {
                ReviewStatus::Planning => rx.t(Msg::AssistantCalculating),
                ReviewStatus::Preview if self.returning => rx.t(Msg::AssistantUseRoute),
                ReviewStatus::Preview if rx.find.review_costs.is_some_and(|c| c.added_m.is_some()) => {
                    rx.t(Msg::AssistantAddStop)
                }
                ReviewStatus::Preview => rx.t(Msg::AssistantGoHere),
                ReviewStatus::Saving => rx.t(Msg::AssistantSaving),
                ReviewStatus::Unresolved => rx.t(Msg::AssistantSaveUnknown),
                ReviewStatus::Accepted => rx.t(Msg::AssistantBackMap),
                _ => rx.t(Msg::AssistantVisitUnavailable),
            }
        };
        cv.round(rect(12, 280, 216, 32), 6, AMBER);
        cv.text(label, Point::new(120, 282), Font::Body, TextAlign::Center, INK);
        if self.route_choices && rx.find.review == ReviewStatus::Preview && self.error.is_none() {
            cv.triangle(Point::new(18, 296), Point::new(24, 290), Point::new(24, 302), INK);
            cv.triangle(Point::new(222, 296), Point::new(216, 290), Point::new(216, 302), INK);
        }
    }
}
fn opening_hours(
    schedule: Option<&obc_reader::WeeklySchedule>,
    local: Option<(u8, u16)>,
) -> Option<heapless::String<16>> {
    let schedule = schedule.filter(|s| s.status(local) == obc_reader::hours::OpeningStatus::Open)?;
    let (day, minute) = local?;
    let ranges = schedule.intervals_on_day(day);
    let range =
        ranges.iter().find(|range| u16::from(range.open_q) * 15 <= minute && minute < u16::from(range.close_q) * 15)?;
    let (open, close) = (u16::from(range.open_q) * 15, u16::from(range.close_q) * 15);
    let mut label = heapless::String::new();
    let _ = write!(label, "{:02}:{:02}-{:02}:{:02}", open / 60, open % 60, close / 60, close % 60);
    Some(label)
}

fn destination_pin(cv: &mut impl Surface, point: Point) {
    let head = Point::new(point.x, point.y - 10);
    cv.triangle(point, Point::new(point.x - 7, point.y - 10), Point::new(point.x + 7, point.y - 10), INK);
    cv.disc(head, 9, PARCHMENT);
    cv.disc(head, 7, INK);
    cv.disc(head, 3, PARCHMENT);
}

fn dotted_connector(cv: &mut impl Surface, start: Point, end: Point) {
    let delta = end - start;
    let steps = delta.x.abs().max(delta.y.abs()).max(1);
    for step in (0..steps).step_by(6) {
        cv.disc(Point::new(start.x + delta.x * step / steps, start.y + delta.y * step / steps), 1, INK);
    }
}

fn letter(i: usize) -> &'static str {
    ["A", "B", "C", "D", "E", "F"][i.min(5)]
}
pub(super) fn fit(min: (i32, i32), max: (i32, i32), w: i32, h: i32, bottom: i32) -> Viewport {
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
fn figures(
    cv: &mut impl Surface,
    distance: u32,
    climb: Option<u32>,
    y: i32,
    extra: bool,
    units: crate::settings::Units,
) {
    let mut d = heapless::String::<20>::new();
    super::vocab::fmt::write_distance_coarse(&mut d, if extra { "+" } else { "" }, distance, units);
    cv.text(&d, Point::new(18, y), Font::Label, TextAlign::Left, INK);
    cv.triangle(Point::new(125, y + 18), Point::new(132, y + 6), Point::new(139, y + 18), INK);
    let c = super::vocab::fmt::elevation_short(climb, units);
    cv.text(&c, Point::new(146, y), Font::Label, TextAlign::Left, INK);
}

#[cfg(test)]
mod tests {
    use super::opening_hours;

    #[test]
    fn hours_pill_uses_the_current_interval_and_requires_trusted_hours() {
        let mut bytes = [0; 29];
        bytes[1..5].copy_from_slice(&[32, 48, 56, 72]);
        let schedule = obc_reader::WeeklySchedule::decode(&bytes).unwrap();
        assert_eq!(opening_hours(Some(&schedule), Some((0, 600))).unwrap(), "08:00-12:00");
        assert_eq!(opening_hours(Some(&schedule), Some((0, 900))).unwrap(), "14:00-18:00");
        assert!(opening_hours(Some(&schedule), Some((0, 780))).is_none());
        assert!(opening_hours(Some(&schedule), None).is_none());
        assert!(opening_hours(None, Some((0, 600))).is_none());
        bytes[0] = 1;
        let uncertain = obc_reader::WeeklySchedule::decode(&bytes).unwrap();
        assert!(opening_hours(Some(&uncertain), Some((0, 600))).is_none());
        bytes[0] = 0;
        bytes[1..5].copy_from_slice(&[88, 8, 0, 0]);
        let overnight = obc_reader::WeeklySchedule::decode(&bytes).unwrap();
        assert_eq!(opening_hours(Some(&overnight), Some((1, 60))).unwrap(), "00:00-02:00");
    }
}
