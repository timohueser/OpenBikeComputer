//! Fixed route facts for the opt-in information-design study, independent of the visit map.

use super::super::{climb::grade_color, palette::*, poi_menu::draw_category_icon, vocab::list};
use crate::{corridor::UpAheadScope, Gesture};
use core::fmt::Write;
use embedded_graphics::prelude::Point;
use obc_reader::{PoiCategory, PoiCategorySet};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

// Distance in metres, elevation relative to the start. The main climb gains 210 m over 3 km.
const PROFILE: &[(u32, i32)] = &[
    (0, 0),
    (800, 40),
    (1300, 30),
    (2000, 50),
    (2600, 74),
    (3200, 110),
    (3800, 170),
    (4400, 224),
    (5000, 260),
    (7000, 120),
    (8500, 190),
    (10000, 120),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Place(PoiCategory),
    Waypoint,
    Climb,
}

struct Entry {
    name: &'static str,
    distance: u32,
    kind: Kind,
    open_now: Option<bool>,
    offset: u32,
}

const ENTRIES: &[Entry] = &[
    Entry { name: "Village water", distance: 800, kind: Kind::Place(PoiCategory::Water), open_now: None, offset: 80 },
    Entry {
        name: "Closed shop",
        distance: 1200,
        kind: Kind::Place(PoiCategory::Resupply),
        open_now: Some(false),
        offset: 0,
    },
    Entry { name: "Climb: 210m", distance: 2000, kind: Kind::Climb, open_now: None, offset: 0 },
    Entry {
        name: "Village shop",
        distance: 6400,
        kind: Kind::Place(PoiCategory::Resupply),
        open_now: Some(true),
        offset: 0,
    },
    Entry { name: "Lunch", distance: 7000, kind: Kind::Waypoint, open_now: None, offset: 0 },
    Entry { name: "Spring", distance: 8600, kind: Kind::Place(PoiCategory::Water), open_now: None, offset: 120 },
    Entry {
        name: "Campsite",
        distance: 9200,
        kind: Kind::Place(PoiCategory::Campsite),
        open_now: Some(true),
        offset: 0,
    },
    Entry { name: "Old bridge", distance: 9800, kind: Kind::Waypoint, open_now: None, offset: 0 },
];

fn height(distance: u32) -> i32 {
    let pair = PROFILE.windows(2).find(|p| distance <= p[1].0).unwrap();
    let (a, z) = pair[0];
    let (b, q) = pair[1];
    z + (q - z) * (distance - a) as i32 / (b - a) as i32
}

fn totals(end: u32) -> (u32, u32) {
    PROFILE.windows(2).take_while(|p| p[0].0 < end).fold((0, 0), |(up, down), p| {
        let delta = height(p[1].0.min(end)) - p[0].1;
        (up + delta.max(0) as u32, down + (-delta).max(0) as u32)
    })
}

fn rows(range: u32, scope: UpAheadScope) -> impl Iterator<Item = &'static Entry> + Clone {
    ENTRIES.iter().filter(move |e| {
        e.distance <= range
            && e.open_now != Some(false)
            && match e.kind {
                Kind::Place(c) => scope.source.shows_pois() && scope.filter.contains(c),
                Kind::Waypoint => scope.source.shows_waypoints() && scope.filter == PoiCategorySet::ALL,
                Kind::Climb => {
                    scope.source == crate::settings::UpAheadSource::Both && scope.filter == PoiCategorySet::ALL
                }
            }
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Brief,
    Timeline,
    Detail,
}

#[derive(Debug)]
pub(super) struct View {
    page: Page,
    range: u32,
    selected: Option<(usize, UpAheadScope)>,
}

impl View {
    pub(super) fn new(timeline: bool) -> Self {
        Self { page: if timeline { Page::Timeline } else { Page::Brief }, range: 10000, selected: None }
    }

    pub(super) fn is_timeline(&self) -> bool {
        self.page == Page::Timeline
    }

    fn cursor(&self, scope: UpAheadScope) -> usize {
        self.selected
            .filter(|(_, s)| *s == scope)
            .map_or(0, |(i, _)| i)
            .min(rows(self.range, scope).count().saturating_sub(1))
    }

    // True returns to the Assistant questions. The containing screen owns that transition.
    pub(super) fn handle(&mut self, g: Gesture, scope: UpAheadScope) -> bool {
        match (self.page, g) {
            (Page::Brief, Gesture::Step(n)) if n != 0 => {
                self.range = if n < 0 { 5000 } else { 10000 };
                self.selected = None;
            }
            (Page::Brief, Gesture::Press) => self.page = Page::Timeline,
            (Page::Brief, Gesture::Back) => return true,
            (Page::Timeline, Gesture::Step(n)) => {
                self.selected =
                    Some((list::step_selection(self.cursor(scope), n, rows(self.range, scope).count().max(1)), scope));
            }
            (Page::Timeline, Gesture::Press) if rows(self.range, scope).next().is_some() => {
                self.selected = Some((self.cursor(scope), scope));
                self.page = Page::Detail;
            }
            (Page::Timeline, Gesture::Back) => self.page = Page::Brief,
            (Page::Detail, Gesture::Back) => self.page = Page::Timeline,
            _ => {}
        }
        false
    }

    pub(super) fn draw(&self, cv: &mut impl Surface, scope: UpAheadScope) {
        cv.clear(PARCHMENT);
        let mut title = heapless::String::<24>::new();
        let _ = write!(title, "{} {}km", if self.page == Page::Brief { "Next" } else { "Ahead" }, self.range / 1000);
        cv.round(rect(4, 4, 232, 34), 6, WOOD);
        label(cv, &title, 12, 6, Font::Body, PARCHMENT);
        match self.page {
            Page::Brief => self.brief(cv),
            Page::Timeline => self.timeline(cv, scope),
            Page::Detail => {
                let Some(e) = rows(self.range, scope).nth(self.cursor(scope)) else { return };
                icon(cv, e.kind, 22, 65);
                label(cv, e.name, 40, 50, Font::Body, INK);
                let d = distance(e.distance);
                label(cv, &d, 14, 88, Font::Body, INK);
                let (up, _) = totals(e.distance);
                let mut line = heapless::String::<32>::new();
                match e.kind {
                    Kind::Climb => {
                        label(cv, "3km at 7%", 14, 126, Font::Body, INK);
                        label(cv, "+210m", 14, 162, Font::Body, INK);
                    }
                    Kind::Waypoint | Kind::Place(_) => {
                        let _ = write!(line, "{up}m climbing");
                        label(cv, &line, 14, 126, Font::Label, INK);
                        label(
                            cv,
                            if e.kind == Kind::Waypoint {
                                "Custom waypoint"
                            } else if e.open_now == Some(true) {
                                "Open now"
                            } else {
                                "Hours unknown"
                            },
                            14,
                            164,
                            Font::Label,
                            SUBTEXT,
                        );
                        if e.offset > 0 {
                            line.clear();
                            let _ = write!(line, "{}m off route", e.offset);
                            label(cv, &line, 14, 204, Font::Label, INK);
                            label(cv, "Access unknown", 14, 236, Font::Label, SUBTEXT);
                        }
                    }
                }
            }
        }
    }

    fn brief(&self, cv: &mut impl Surface) {
        cv.triangle(Point::new(217, 18), Point::new(227, 18), Point::new(222, 11), PARCHMENT);
        cv.triangle(Point::new(217, 25), Point::new(227, 25), Point::new(222, 32), PARCHMENT);
        let (up, down) = totals(self.range);
        arrow_stat(cv, up, 12, 44, false);
        arrow_stat(cv, down, 130, 44, true);
        label(cv, "Climb in 2km", 12, 76, Font::Body, INK);
        label(cv, "3km at 7%", 12, 106, Font::Label, INK);
        label(cv, "+210m", 166, 106, Font::Label, INK);
        let x = |d| 12 + (216 * d / self.range) as i32;
        let y = |d| 194 - height(d) * 65 / 300;
        for column in 0..216u32 {
            let a = self.range * column / 216;
            let b = self.range * (column + 1) / 216;
            let slope = PROFILE.windows(2).find(|p| a < p[1].0).unwrap();
            let grade = (slope[1].1 - slope[0].1) * 100 / (slope[1].0 - slope[0].0) as i32;
            let top = y(a);
            cv.vline(12 + column as i32, top, 195 - top, 1, grade_color(grade));
            cv.line(Point::new(x(a), top), Point::new(x(b), y(b)), INK);
        }
        for e in ENTRIES.iter().filter(|e| e.distance <= self.range && (e.distance == 800 || e.kind == Kind::Waypoint))
        {
            let p = Point::new(x(e.distance), y(e.distance));
            cv.line(p, Point::new(p.x, p.y - 14), SUBTEXT);
            icon(cv, e.kind, p.x, p.y - 20);
        }
        icon(cv, Kind::Waypoint, 20, 240);
        label(cv, "Lunch", 38, 226, Font::Body, INK);
        label(cv, "7km", 192, 228, Font::Label, INK);
        for (category, x) in [(PoiCategory::Water, 18), (PoiCategory::Resupply, 133)] {
            icon(cv, Kind::Place(category), x, 268);
            let next = ENTRIES
                .iter()
                .find(|e| e.kind == Kind::Place(category) && e.open_now != Some(false) && e.distance <= self.range);
            let value = next.map_or_else(|| heapless::String::try_from("none").unwrap(), |e| distance(e.distance));
            label(cv, &value, x + 16, 256, Font::Label, INK);
        }
        super::button(cv, "Explore ahead", 284, true);
    }

    fn timeline(&self, cv: &mut impl Surface, scope: UpAheadScope) {
        if scope.filter != PoiCategorySet::ALL || scope.source != crate::settings::UpAheadSource::Both {
            cv.triangle(Point::new(156, 13), Point::new(172, 13), Point::new(164, 22), AMBER);
            cv.fill(rect(162, 20, 4, 8), AMBER);
        }
        let total = rows(self.range, scope).count();
        let selected = self.cursor(scope);
        let mut count = heapless::String::<12>::new();
        let _ = write!(count, "{}/{}", if total == 0 { 0 } else { selected + 1 }, total);
        cv.text(&count, Point::new(226, 8), Font::Label, TextAlign::Right, PARCHMENT);
        if total == 0 {
            label(cv, "No matches", 14, 108, Font::Body, INK);
            label(cv, "Change filters", 14, 144, Font::Label, SUBTEXT);
            return;
        }
        let first = list::window_start(selected, 4, total);
        for (slot, e) in rows(self.range, scope).skip(first).take(4).enumerate() {
            let y = 44 + slot as i32 * 67;
            if first + slot == selected {
                cv.round(rect(8, y, 224, 63), 6, AMBER);
            }
            icon(cv, e.kind, 23, y + 19);
            label(cv, e.name, 40, y + 3, Font::Body, INK);
            if e.kind == Kind::Climb {
                label(cv, "in 2km; 3km at 7%", 12, y + 33, Font::Label, INK);
            } else {
                label(cv, &distance(e.distance), 16, y + 33, Font::Label, INK);
                arrow_stat(cv, totals(e.distance).0, 94, y + 33, false);
                if e.offset > 0 {
                    cv.triangle(Point::new(187, y + 39), Point::new(187, y + 49), Point::new(194, y + 44), INK);
                    cv.text(&distance(e.offset), Point::new(226, y + 33), Font::Label, TextAlign::Right, INK);
                }
            }
        }
        cv.vline(235, 46, 264, 2, RULE);
        cv.vline(235, 46 + first as i32 * 264 / total as i32, 264 * 4 / total.max(4) as i32, 2, WOOD);
    }
}

fn distance(m: u32) -> heapless::String<16> {
    let mut s = heapless::String::new();
    if m < 1000 {
        let _ = write!(s, "{m}m");
    } else if m.is_multiple_of(1000) {
        let _ = write!(s, "{}km", m / 1000);
    } else {
        let _ = write!(s, "{}.{:01}km", m / 1000, m % 1000 / 100);
    }
    s
}

fn label(cv: &mut impl Surface, s: &str, x: i32, y: i32, font: Font, color: u16) {
    cv.text(s, Point::new(x, y), font, TextAlign::Left, color);
}

fn arrow_stat(cv: &mut impl Surface, value: u32, x: i32, y: i32, down: bool) {
    let (base, tip) = if down { (y + 5, y + 17) } else { (y + 17, y + 5) };
    cv.triangle(Point::new(x, base), Point::new(x + 10, base), Point::new(x + 5, tip), INK);
    let mut s = heapless::String::<16>::new();
    let _ = write!(s, "{value}m");
    label(cv, &s, x + 18, y, Font::Label, INK);
}

fn icon(cv: &mut impl Surface, kind: Kind, x: i32, y: i32) {
    match kind {
        Kind::Place(c) => draw_category_icon(cv, c, Point::new(x, y), INK, PARCHMENT),
        Kind::Climb => cv.triangle(Point::new(x - 9, y + 8), Point::new(x + 9, y + 8), Point::new(x, y - 9), INK),
        Kind::Waypoint => {
            cv.triangle(Point::new(x - 8, y), Point::new(x + 8, y), Point::new(x, y - 9), INK);
            cv.triangle(Point::new(x - 8, y), Point::new(x + 8, y), Point::new(x, y + 9), INK);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::UpAheadSource;
    fn scope() -> UpAheadScope {
        UpAheadScope { filter: PoiCategorySet::ALL, source: UpAheadSource::Both }
    }

    #[test]
    fn ranges_use_matching_terrain_and_exclude_closed_places_but_keep_unknowns() {
        assert_eq!(totals(5000), (270, 10));
        assert_eq!(totals(10000), (340, 220));
        assert_eq!(rows(5000, scope()).map(|e| e.name).collect::<std::vec::Vec<_>>(), ["Village water", "Climb: 210m"]);
        assert_eq!(rows(10000, scope()).count(), 7);
        assert!(!rows(10000, scope()).any(|e| e.open_now == Some(false)));
        let water = UpAheadScope { filter: PoiCategorySet::only(PoiCategory::Water), ..scope() };
        assert_eq!(rows(10000, water).count(), 2);
        assert_eq!(rows(10000, UpAheadScope { source: UpAheadSource::WaypointsOnly, ..scope() }).count(), 2);
    }

    #[test]
    fn details_restore_selection_and_filters_rehome_without_changing_the_brief_range() {
        let mut view = View::new(false);
        view.handle(Gesture::Step(-1), scope());
        assert_eq!(view.range, 5000);
        view.handle(Gesture::Step(1), scope());
        view.handle(Gesture::Press, scope());
        view.handle(Gesture::Step(4), scope());
        let selected = view.cursor(scope());
        view.handle(Gesture::Press, scope());
        assert_eq!(view.page, Page::Detail);
        view.handle(Gesture::Back, scope());
        assert_eq!(view.cursor(scope()), selected);
        let water = UpAheadScope { filter: PoiCategorySet::only(PoiCategory::Water), ..scope() };
        assert_eq!(view.cursor(water), 0);
        assert_eq!(view.range, 10000);
        view.handle(Gesture::Back, water);
        assert_eq!(view.page, Page::Brief);
        assert!(view.handle(Gesture::Back, water));
    }
}
