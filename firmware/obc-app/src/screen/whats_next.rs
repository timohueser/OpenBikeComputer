//! What comes next on the frozen accepted journey.
use super::{palette::*, Ctx, PoiDetailScreen, Render, Screen, Transition};
use crate::{
    whats_next::{Item, Page, Row},
    Gesture, Msg,
};
use core::fmt::Write;
use embedded_graphics::prelude::Point;
use obc_reader::{hours::OpeningStatus, reader::places::QueryProgress, PoiCategory};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};
use obc_route::window::AheadRange;

#[derive(Debug, Default)]
pub struct WhatsNextScreen;
impl WhatsNextScreen {
    pub fn new() -> Self {
        Self
    }
    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let a = &mut cx.ahead;
        match (a.page, g) {
            (_, Gesture::Hold) => a.refresh(cx.navigator.route_state().progress_m),
            (Page::Overview, Gesture::Step(n)) if n != 0 => {
                a.range(if n < 0 { AheadRange::FiveKm } else { AheadRange::TenKm })
            }
            (Page::Overview, Gesture::Press) => a.explore(),
            (Page::Overview, Gesture::Back) => return Transition::Pop,
            (Page::Timeline, Gesture::Back) => a.back(),
            (Page::Detail, Gesture::Back) => a.page = Page::Timeline,
            (Page::Timeline, Gesture::Step(n)) if !a.pending() && n != 0 => {
                if n > 0 && a.selected + 1 >= a.rows.len() {
                    if a.has_next() {
                        a.turn_page(false);
                    }
                } else if n < 0 && a.selected == 0 {
                    if a.has_previous() {
                        a.turn_page(true);
                    }
                } else {
                    a.selected = (a.selected as i32 + n).clamp(0, a.rows.len().saturating_sub(1) as i32) as usize;
                }
            }
            (Page::Timeline, Gesture::Press) if !a.pending() => {
                if let Some(row) = a.rows.get(a.selected) {
                    if let Item::Place(i) = row.item {
                        if let Some(p) = cx.corridor.get(i as usize).filter(|p| p.poi.opening != OpeningStatus::Closed)
                        {
                            return Transition::Push(Screen::PoiDetail(
                                PoiDetailScreen::new(p.poi.clone()).off_route(p.offset_m),
                            ));
                        }
                    } else {
                        a.page = Page::Detail;
                    }
                }
            }
            _ => {}
        }
        Transition::None
    }
    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        cv.clear(PARCHMENT);
        let a = rx.ahead;
        let caption = if a.page == Page::Overview { rx.t(Msg::AheadNext) } else { rx.t(Msg::AheadAhead) };
        let range = distance(a.range.meters(), rx.settings.units);
        let mut title = heapless::String::<24>::new();
        // Timeline status markers start at x=202; keep an eight-pixel gap before them.
        let budget = if a.page == Page::Timeline { 182 } else { rx.w - 24 };
        let title_width = obc_render::text::text_width(caption, Font::Body)
            + Font::Body.char_width()
            + obc_render::text::text_width(&range, Font::Body);
        let font = if title_width as i32 <= budget {
            let _ = write!(title, "{caption} {range}");
            Font::Body
        } else {
            // One cell of the budget is the space between the caption and the range.
            let room =
                budget - obc_render::text::text_width(&range, Font::Label) as i32 - Font::Label.char_width() as i32;
            let caption = super::vocab::marquee::fit(caption, room, Font::Label);
            let _ = write!(title, "{caption} {range}");
            Font::Label
        };
        cv.round(rect(4, 4, rx.w - 8, 34), 6, WOOD);
        label(cv, &title, 12, if font == Font::Body { 6 } else { 8 }, font, PARCHMENT);
        if a.window.is_none() {
            empty_message(
                cv,
                if a.stale {
                    rx.t(Msg::AheadChanged)
                } else if matches!(a.status, QueryProgress::Failed(_)) {
                    rx.t(Msg::AheadReadFailed)
                } else {
                    rx.t(Msg::AheadNoRoute)
                },
                rx.w,
            );
            return;
        }
        match a.page {
            Page::Overview => overview(cv, rx),
            Page::Timeline => timeline(cv, rx),
            Page::Detail => detail(cv, rx),
        }
    }
}
fn label(cv: &mut impl Surface, text: &str, x: i32, y: i32, font: Font, color: u16) {
    cv.text(text, Point::new(x, y), font, TextAlign::Left, color);
}
fn empty_message(cv: &mut impl Surface, text: &str, width: i32) {
    if obc_render::text::text_width(text, Font::Body) as i32 > width - 28 {
        super::vocab::chrome::wrapped(cv, text, width / 2, 108, width - 28, Font::Body, INK);
    } else {
        label(cv, text, 14, 108, Font::Body, INK);
    }
}
fn distance(m: u32, units: crate::settings::Units) -> heapless::String<24> {
    let mut s = heapless::String::new();
    super::vocab::fmt::write_distance_coarse(&mut s, "", m, units);
    s
}
fn icon(cv: &mut impl Surface, category: Option<PoiCategory>, x: i32, y: i32, bg: u16) {
    if let Some(c) = category {
        super::poi_menu::draw_category_icon(cv, c, Point::new(x, y), INK, bg);
    } else {
        cv.triangle(Point::new(x, y - 6), Point::new(x - 5, y), Point::new(x + 5, y), WOOD);
        cv.triangle(Point::new(x, y + 6), Point::new(x - 5, y), Point::new(x + 5, y), WOOD);
    }
}
fn overview(cv: &mut impl Surface, rx: &Render) {
    let a = rx.ahead;
    let w = a.window.unwrap();
    if a.totals.is_none() && matches!(a.status, QueryProgress::Failed(_)) {
        label(cv, rx.t(Msg::AheadReadFailed), 14, 108, Font::Body, INK);
        return;
    }
    cv.triangle(Point::new(217, 18), Point::new(227, 18), Point::new(222, 11), PARCHMENT);
    cv.triangle(Point::new(217, 25), Point::new(227, 25), Point::new(222, 32), PARCHMENT);
    for (x, down) in [(12, false), (130, true)] {
        let mut s = heapless::String::<24>::new();
        if let Some((up, dn)) = a.totals {
            let _ = write!(
                s,
                "{}{}{}",
                if down { "-" } else { "+" },
                rx.settings.units.elev((if down { dn } else { up }) as f32) as u32,
                rx.settings.units.elev_label()
            );
        } else {
            let _ = write!(s, "? {}", rx.settings.units.elev_label());
        }
        label(cv, &s, x, 44, Font::Body, INK);
    }
    if let Some(c) = a.climb {
        let mut s = heapless::String::<32>::new();
        if c.start_m <= w.start_m {
            let _ = s.push_str(rx.t(Msg::AheadOnClimb));
        } else {
            let _ = write!(s, "{} {}", rx.t(Msg::AheadClimbIn), distance(c.start_m - w.start_m, rx.settings.units));
        }
        label(cv, &s, 12, 76, Font::Body, INK);
        s.clear();
        let _ = write!(
            s,
            "{} {} {}%",
            distance(c.end_m - c.start_m, rx.settings.units),
            rx.t(Msg::AheadAt),
            c.avg_grade_pct
        );
        label(cv, &s, 12, 106, Font::Label, INK);
        s.clear();
        let _ = write!(s, "+{}{}", rx.settings.units.elev(c.gain_m as f32) as u32, rx.settings.units.elev_label());
        label(cv, &s, 166, 106, Font::Label, INK);
    } else {
        label(cv, rx.t(Msg::AheadNoClimb), 12, 76, Font::Body, INK);
    }
    if let Some(p) = rx.profile {
        let total = rx.navigation.route_total_m.max(1) as f32;
        let f = |x: u32| (w.start_m as f32 + (w.end_m - w.start_m) as f32 * x as f32 / 215.0) / total;
        let (mut low, mut high) = (i16::MAX, i16::MIN);
        for x in 0..216 {
            let (lo, hi) = p.sample(0, f(x));
            if lo <= hi {
                low = low.min(lo);
                high = high.max(hi);
            }
        }
        let span = (i32::from(high) - i32::from(low)).max(100) as f32;
        for x in 0..216 {
            let (lo, hi) = p.sample(0, f(x));
            if lo > hi {
                continue;
            }
            let y = 194 - ((i32::from(hi) - i32::from(low)) as f32 * 65.0 / span) as i32;
            let Some(grade) = p.grade_at(f(x)) else {
                continue;
            };
            let color = super::climb::grade_color(grade);
            cv.vline(12 + x as i32, y, (195 - y).max(1), 1, color);
        }
        cv.hline(12, 195, 216, RULE);
        for d in
            [a.water, a.next_waypoint.as_ref().map(|w| w.dist_along_m)].into_iter().flatten().filter(|d| w.contains(*d))
        {
            let x = 12 + (216u64 * (d - w.start_m) as u64 / (w.end_m - w.start_m).max(1) as u64) as i32;
            cv.vline(x, 186, 16, 1, WOOD);
        }
    }
    if a.totals.is_none() {
        label(cv, rx.t(Msg::AheadIncomplete), 12, 201, Font::Label, SUBTEXT);
    }
    if let Some(wpt) = &a.next_waypoint {
        icon(cv, wpt.category, 20, 240, PARCHMENT);
        let distance = distance(wpt.dist_along_m.saturating_sub(w.start_m), rx.settings.units);
        let budget = 228 - obc_render::text::text_width(&distance, Font::Label) as i32 - 8 - 38;
        let name = super::vocab::marquee::fit(
            if wpt.name.is_empty() { rx.t(Msg::AheadWaypoint) } else { &wpt.name },
            budget,
            Font::Body,
        );
        label(cv, &name, 38, 226, Font::Body, INK);
        cv.text(&distance, Point::new(228, 228), Font::Label, TextAlign::Right, INK);
    } else {
        label(cv, rx.t(Msg::AheadNoWaypoint), 12, 226, Font::Label, SUBTEXT);
    }
    for (category, x, next) in [(PoiCategory::Water, 18, a.water), (PoiCategory::Resupply, 133, a.shop)] {
        icon(cv, Some(category), x, 268, PARCHMENT);
        let d = next.map(|d| distance(d.saturating_sub(w.start_m), rx.settings.units));
        let text = d.as_deref().unwrap_or(match a.status {
            QueryProgress::Ready { more: false, coverage_complete: true } => rx.t(Msg::AheadNone),
            QueryProgress::Pending => "...",
            _ => "?",
        });
        label(cv, text, x + 16, 256, Font::Label, INK);
    }
    cv.round(rect(8, 284, 224, 32), 6, AMBER);
    cv.text(rx.t(Msg::AheadExplore), Point::new(120, 286), Font::Body, TextAlign::Center, INK);
}
fn row_name<'a>(row: &'a Row, rx: &'a Render) -> &'a str {
    match &row.item {
        Item::Waypoint(w) => {
            if w.name.is_empty() {
                rx.t(Msg::AheadWaypoint)
            } else {
                &w.name
            }
        }
        Item::Climb(_) => rx.t(Msg::AheadClimb),
        Item::Place(i) => rx
            .corridor
            .get(*i as usize)
            .map_or(rx.t(Msg::AheadUnavailable), |p| super::poi_display::poi_row_name(&p.poi)),
    }
}
fn timeline(cv: &mut impl Surface, rx: &Render) {
    let a = rx.ahead;
    let scope = rx.up_ahead_scope();
    if scope.filter != obc_reader::PoiCategorySet::ALL || scope.source != crate::settings::UpAheadSource::Both {
        cv.triangle(Point::new(202, 13), Point::new(218, 13), Point::new(210, 22), AMBER);
        cv.fill(rect(208, 20, 4, 8), AMBER);
    }
    if !matches!(a.status, QueryProgress::Ready { coverage_complete: true, .. }) {
        cv.text("?", Point::new(228, 8), Font::Label, TextAlign::Right, PARCHMENT);
    }
    if a.pending() {
        empty_message(cv, rx.t(Msg::AheadLoading), rx.w);
        return;
    }
    if a.rows.is_empty() {
        let text = match a.status {
            QueryProgress::Ready { coverage_complete: true, .. } => rx.t(Msg::AheadNoMatches),
            QueryProgress::Failed(_) => rx.t(Msg::AheadReadFailed),
            QueryProgress::Ready { coverage_complete: false, .. } => rx.t(Msg::AheadPartial),
            _ => rx.t(Msg::AheadMapUnavailable),
        };
        empty_message(cv, text, rx.w);
        return;
    }
    for (i, row) in a.rows.iter().enumerate() {
        let y = 44 + i as i32 * 67;
        let selected = i == a.selected;
        let bg = if selected { AMBER } else { PARCHMENT };
        if selected {
            cv.round(rect(8, y, 224, 63), 6, bg);
        }
        let category = match &row.item {
            Item::Waypoint(w) => w.category,
            Item::Place(i) => {
                rx.corridor.get(*i as usize).and_then(|p| obc_formats::obcm::poi_category_of(p.poi.subtype))
            }
            Item::Climb(_) => None,
        };
        icon(cv, category, 23, y + 19, bg);
        let name_row = selected.then(|| rect(40, y + 3, 184, Font::Body.line_height() as i32));
        let name = rx.marquee.fit(row_name(row, rx), 184, Font::Body, name_row);
        label(cv, &name, 40, y + 3, Font::Body, INK);
        let passed = row.key.distance() < rx.navigation.progress_m;
        if passed {
            label(cv, rx.t(Msg::AheadPassed), 12, y + 33, Font::Label, SUBTEXT);
        } else {
            label(
                cv,
                &distance(row.key.distance().saturating_sub(a.window.unwrap().start_m), rx.settings.units),
                12,
                y + 33,
                Font::Label,
                INK,
            );
        }
        let offset = match &row.item {
            Item::Waypoint(w) => w.lateral_offset_m as i32,
            Item::Place(i) => rx.corridor.get(*i as usize).map_or(0, |p| p.offset_m),
            Item::Climb(_) => 0,
        };
        let offset = (offset.abs() > super::OFF_ROUTE_HINT_M)
            .then(|| (distance(offset.unsigned_abs(), rx.settings.units), offset > 0));
        if let Some((d, right)) = &offset {
            let at = Point::new(offset_left(d), y + 33 + Font::Label.cap_mid() as i32);
            super::poi_display::draw_side_arrow(cv, at, *right, INK);
            cv.text(d, Point::new(FIGURE_RIGHT, y + 33), Font::Label, TextAlign::Right, INK);
        }
        // A climb under 5 m is noise, an unknown climb shows nothing, and the climb to a passed
        // place means nothing.
        if let Some(m) = row.ascent_m.filter(|m| *m >= 5 && !passed) {
            let climb = super::vocab::fmt::elevation_short(Some(m), rx.settings.units);
            if climb_fits(&climb, offset.as_ref().map(|(d, _)| d.as_str())) {
                super::poi_display::draw_climb_figure(cv, CLIMB_X, y + 33, &climb);
            }
        }
    }
    if a.has_next() {
        cv.triangle(Point::new(234, 298), Point::new(228, 290), Point::new(239, 290), WOOD);
    }
}
/// The climb figure's column on line 2 of a timeline row. It is 6 px clear of the widest imperial
/// distance, and a 3-digit metric climb is 6 px clear of a 3-digit offset.
const CLIMB_X: i32 = 90;
/// The right edge of the offset figure on line 2.
const FIGURE_RIGHT: i32 = 224;

/// The left edge of the offset figure, which is its side arrow.
fn offset_left(offset: &str) -> i32 {
    use super::poi_display::{ARROW_GAP, ARROW_W};
    FIGURE_RIGHT - obc_render::text::text_width(offset, Font::Label) as i32 - ARROW_GAP - ARROW_W
}

/// Whether the climb figure clears the offset. When both do not fit, the offset stays, because it
/// warns that the place is not on the route.
fn climb_fits(climb: &str, offset: Option<&str>) -> bool {
    let climb_right =
        CLIMB_X + super::poi_display::CLIMB_TEXT_DX + obc_render::text::text_width(climb, Font::Label) as i32;
    offset.is_none_or(|o| climb_right + 6 <= offset_left(o))
}

fn detail(cv: &mut impl Surface, rx: &Render) {
    let a = rx.ahead;
    let Some(row) = a.rows.get(a.selected) else {
        return;
    };
    label(cv, row_name(row, rx), 12, 50, Font::Body, INK);
    label(
        cv,
        &distance(row.key.distance().saturating_sub(rx.navigation.progress_m), rx.settings.units),
        14,
        88,
        Font::Body,
        INK,
    );
    match &row.item {
        Item::Climb(c) => {
            let mut s = heapless::String::<40>::new();
            let _ = write!(
                s,
                "{} {} {}%",
                distance(c.end_m - c.start_m, rx.settings.units),
                rx.t(Msg::AheadAt),
                c.avg_grade_pct
            );
            label(cv, &s, 14, 126, Font::Body, INK);
            s.clear();
            let _ = write!(
                s,
                "+{}{} ({})",
                rx.settings.units.elev(c.gain_m as f32) as u32,
                rx.settings.units.elev_label(),
                rx.t(Msg::AheadWholeClimb)
            );
            label(cv, &s, 14, 162, Font::Label, INK);
        }
        Item::Waypoint(w) => {
            label(cv, rx.t(Msg::AheadCustom), 14, 126, Font::Label, SUBTEXT);
            if w.lateral_offset_m != 0 {
                label(
                    cv,
                    &distance(w.lateral_offset_m.unsigned_abs() as u32, rx.settings.units),
                    14,
                    164,
                    Font::Label,
                    INK,
                );
                label(cv, rx.t(Msg::AheadAccess), 14, 200, Font::Label, SUBTEXT);
            }
        }
        Item::Place(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Language, Units};
    use obc_render::text::text_width;

    /// The widest string `format` gives for any distance up to 50 km.
    fn widest(format: impl Fn(u32) -> heapless::String<24>) -> heapless::String<24> {
        (0..50_000).step_by(3).map(format).max_by_key(|s| text_width(s, Font::Label)).unwrap()
    }

    /// Line 2 never overprints in any unit or language: the distance or the "Passed" caption ends
    /// before the climb and the widest offset, and the climb shows only where it clears the offset.
    #[test]
    fn line_two_figures_clear_each_other_in_every_unit_and_language() {
        for units in [Units::Metric, Units::Imperial] {
            let dist = widest(|m| distance(m, units));
            let offset = offset_left(&dist);
            let dist_right = 12 + text_width(&dist, Font::Label) as i32;
            assert!(dist_right + 6 <= CLIMB_X, "{units:?}: {dist}");
            for language in Language::ALL {
                let passed = crate::i18n::t(Msg::AheadPassed, language);
                assert!(12 + text_width(passed, Font::Label) as i32 + 6 <= offset, "{language:?}: {passed}");
            }
            let climb = super::super::vocab::fmt::elevation_short(Some(2_000), units);
            assert!(climb_fits(&climb, None), "{units:?}: {climb} alone");
            assert!(!climb_fits(&climb, Some(&dist)), "{units:?}: {climb} beside {dist} must give way");
        }
        assert!(climb_fits("120m", Some("300m")), "a metric climb shows beside a corridor offset");
    }
}
