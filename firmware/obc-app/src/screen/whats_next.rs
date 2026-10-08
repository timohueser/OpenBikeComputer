//! The route as a service line, with distance on the left and places on the right.
use super::{palette::*, vocab::spinner::Spinner, Ctx, PoiDetailScreen, Render, Screen, ScreenTick, Transition};
use crate::{
    whats_next::{AheadState, Item, Page, Row, ROWS, SERVICES},
    Gesture, Msg,
};
use core::fmt::Write;
use embedded_graphics::prelude::Point;
use obc_formats::obcm::SettlementClass;
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

const TOP: i32 = 42;
const LINE_X: i32 = 48;
const NAME_X: i32 = 62;
const NAME_W: i32 = 174;

#[derive(Debug, Default)]
pub struct WhatsNextScreen {
    spin: Spinner,
}
impl WhatsNextScreen {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn tick_timers(&mut self, now_ms: u32, w: i32, h: i32, pending: bool) -> ScreenTick {
        if pending {
            self.spin.tick_at_cadence(now_ms, w, h, 166)
        } else {
            self.spin = Spinner::default();
            ScreenTick::idle()
        }
    }
    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let a = &mut cx.ahead;
        match (a.page, g) {
            (_, Gesture::Hold) => a.refresh(cx.navigator.route_state().progress_m),
            (Page::Timeline, Gesture::Back) => return Transition::Pop,
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
                    if let Item::Place(p) = &row.item {
                        let mut detail = p.poi.clone();
                        detail.distance_m = row.key.distance().saturating_sub(cx.navigator.route_state().progress_m);
                        return Transition::Push(Screen::PoiDetail(PoiDetailScreen::new(detail).off_route(p.offset_m)));
                    }
                    a.open_detail();
                }
            }
            (Page::Detail, Gesture::Step(n)) if !a.pending() && n != 0 => {
                if n > 0 && a.detail_selected + 1 >= a.detail_rows.len() {
                    if a.detail_more {
                        a.detail_page(false);
                    }
                } else if n < 0 && a.detail_selected == 0 {
                    if a.detail_previous {
                        a.detail_page(true);
                    }
                } else {
                    a.detail_selected =
                        (a.detail_selected as i32 + n).clamp(0, a.detail_rows.len().saturating_sub(1) as i32) as usize;
                }
            }
            (Page::Detail, Gesture::Press) if !a.pending() => {
                if let Some(p) = a.detail_rows.get(a.detail_selected) {
                    let mut poi = p.poi.clone();
                    poi.distance_m = p.dist_along_m.saturating_sub(cx.navigator.route_state().progress_m);
                    return Transition::Push(Screen::PoiDetail(PoiDetailScreen::new(poi).off_route(p.offset_m)));
                }
            }
            _ => {}
        }
        if !a.pending() && !a.rows.is_empty() {
            a.scroll = scroll_for(a);
        }
        Transition::None
    }
    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        cv.clear(PARCHMENT);
        let a = rx.ahead;
        if a.window.is_none() {
            message(cv, if a.stale { rx.t(Msg::AheadChanged) } else { rx.t(Msg::AheadNoRoute) });
        } else if a.pending() {
            self.spin.draw_needle(cv, rx.w, rx.h);
            super::vocab::chrome::wrapped(
                cv,
                rx.t(Msg::AheadLoading),
                rx.w / 2,
                rx.h * 72 / 100,
                rx.w - 24,
                Font::Label,
                INK,
            );
        } else if a.rows.is_empty() {
            message(cv, rx.t(Msg::AheadReadFailed));
        } else if a.page == Page::Timeline {
            timeline(cv, rx);
        } else {
            detail(cv, rx);
        }
        cv.fill(rect(0, 0, rx.w, TOP), PARCHMENT);
        cv.round(rect(4, 4, rx.w - 8, 34), 6, WOOD);
        if a.page == Page::Detail {
            let name = a.rows.get(a.selected).map_or(rx.t(Msg::AheadAhead), |r| row_name(r, rx));
            fitted(cv, name, 12, 6, rx.w - 24, BAR_TEXT);
        } else {
            label(cv, rx.t(Msg::AheadAhead), 12, 6, Font::Body, BAR_TEXT);
            if let Some(window) = a.window {
                let dist = distance(window.end_m.saturating_sub(rx.navigation.progress_m), rx.settings.units);
                cv.text(&dist, Point::new(228, 8), Font::Label, TextAlign::Right, BAR_TEXT);
            }
        }
    }
}
fn label(cv: &mut impl Surface, text: &str, x: i32, y: i32, font: Font, color: u16) {
    cv.text(text, Point::new(x, y), font, TextAlign::Left, color);
}
fn message(cv: &mut impl Surface, text: &str) {
    super::vocab::chrome::wrapped(cv, text, 120, 110, 212, Font::Body, INK);
}
pub(crate) fn distance(m: u32, units: crate::settings::Units) -> heapless::String<24> {
    let mut s = heapless::String::new();
    super::vocab::fmt::write_distance_coarse(&mut s, "", m, units);
    s
}
fn font_for(name: &str, width: i32) -> Font {
    if text_width(name, Font::Body) as i32 <= width {
        Font::Body
    } else {
        Font::Label
    }
}
fn fitted(cv: &mut impl Surface, name: &str, x: i32, y: i32, width: i32, ink: u16) {
    let font = font_for(name, width);
    let name = super::vocab::marquee::fit(name, width, font);
    label(cv, &name, x, y + if font == Font::Label { 2 } else { 0 }, font, ink);
}
fn row_name<'a>(row: &'a Row, rx: &'a Render) -> &'a str {
    match &row.item {
        Item::End => rx.t(Msg::AheadRouteEnd),
        Item::Place(p) if p.poi.name.is_empty() => super::poi_display::poi_row_name(&p.poi),
        Item::Waypoint(w) if w.name.is_empty() => rx.t(Msg::AheadWaypoint),
        _ => row.name(),
    }
}
fn row_height(row: &Row) -> i32 {
    if matches!(row.item, Item::Place(_)) || row.services.is_empty() {
        34
    } else {
        62
    }
}
fn positions(a: &AheadState) -> ([i32; ROWS], i32) {
    let mut positions = [0; ROWS];
    let mut y = if a.has_previous() { 0 } else { 18 };
    let mut last_d = a.window.map_or(0, |w| w.start_m);
    for (i, row) in a.rows.iter().enumerate() {
        if (i > 0 || !a.has_previous()) && row.key.distance().saturating_sub(last_d) >= 7_000 {
            y += 18;
        }
        positions[i] = y;
        y += row_height(row);
        last_d = row.key.distance();
    }
    (positions, y)
}
fn scroll_for(a: &AheadState) -> i32 {
    let (ys, total) = positions(a);
    let selected = a.selected.min(a.rows.len() - 1);
    let bottom = ys[selected] + row_height(&a.rows[selected]);
    let height = 320 - TOP;
    (if bottom - a.scroll > height {
        bottom - height
    } else if ys[selected] < a.scroll {
        ys[selected]
    } else {
        a.scroll
    })
    .clamp(0, (total - height).max(0))
}
fn timeline(cv: &mut impl Surface, rx: &Render) {
    let a = rx.ahead;
    let (ys, _) = positions(a);
    let selected = a.selected.min(a.rows.len() - 1);
    let scroll = scroll_for(a);
    let sy = |i: usize| TOP + ys[i] - scroll;
    cv.round(rect(4, sy(selected), 232, row_height(&a.rows[selected]) - 2), 5, AMBER);
    let first = sy(0) + 15;
    let end = sy(a.rows.len() - 1) + 15;
    let start_y = if a.has_previous() { TOP } else { (first - 20).max(TOP) };
    let end_y = if a.has_next() { 320 } else { end };
    cv.vline(LINE_X - 4, start_y, end_y - start_y + 1, 9, INK);
    let mut colors = [RULE; 320];
    for y in start_y.max(TOP)..end_y.min(319) + 1 {
        let i = (0..a.rows.len()).find(|&i| sy(i) + 15 >= y).unwrap_or(a.rows.len() - 1);
        let (d0, y0) =
            if i == 0 { (a.window.unwrap().start_m, start_y) } else { (a.rows[i - 1].key.distance(), sy(i - 1) + 15) };
        let d1 = a.rows[i].key.distance();
        let y1 = sy(i) + 15;
        let d = d0 + ((u64::from(d1.saturating_sub(d0)) * (y - y0).max(0) as u64) / (y1 - y0).max(1) as u64) as u32;
        let km = d / 1_000 * 1_000 + 500;
        colors[y as usize] = rx
            .profile
            .and_then(|p| {
                let total = a.window.unwrap().end_m.max(1);
                let start = km - 500;
                let length = total.saturating_sub(start).clamp(1, 1_000);
                let mut sum = 0;
                for sample in 0..8 {
                    sum += p.grade_at((start + length * (2 * sample + 1) / 16) as f32 / total as f32)?;
                }
                Some(libm::roundf(sum as f32 / 8.0) as i32)
            })
            .map_or(RULE, super::climb::grade_color);
    }
    merge_short_runs(&mut colors[start_y.max(TOP) as usize..(end_y.min(319) + 1).max(start_y.max(TOP)) as usize]);
    for y in start_y.max(TOP)..end_y.min(319) + 1 {
        cv.hline(LINE_X - 3, y, 7, colors[y as usize]);
    }
    if !a.has_previous() && first - 20 >= TOP {
        cv.triangle(
            Point::new(LINE_X - 6, first - 22),
            Point::new(LINE_X + 6, first - 22),
            Point::new(LINE_X, first - 10),
            WARNING,
        );
    }
    for (i, row) in a.rows.iter().enumerate() {
        let y = sy(i);
        if y + row_height(row) <= TOP || y >= 320 {
            continue;
        }
        let chosen = i == selected;
        let bg = if chosen { AMBER } else { PARCHMENT };
        let ink = if chosen { ON_ACCENT } else { INK };
        let mut km = heapless::String::<16>::new();
        let m = row.key.distance().saturating_sub(rx.navigation.progress_m);
        let amount = match rx.settings.units {
            crate::settings::Units::Metric => m as f32 / 1000.0,
            crate::settings::Units::Imperial => m as f32 / 1609.344,
        };
        if amount < 1.0 && amount > 0.0 {
            let _ = write!(km, "{amount:.1}");
        } else {
            let _ = write!(km, "{}", libm::roundf(amount) as u32);
        }
        cv.text(&km, Point::new(35, y + 4), Font::Caption, TextAlign::Right, ink);
        marker(cv, row, Point::new(LINE_X, y + 15), ink, bg);
        fitted(cv, row_name(row, rx), NAME_X, y + 1, NAME_W, ink);
        if !matches!(row.item, Item::Place(_)) {
            let mut x = NAME_X + 10;
            for cat in SERVICES.into_iter().filter(|c| row.services.contains(*c)) {
                super::poi_menu::draw_category_icon(cv, cat, Point::new(x, y + 44), ink, bg);
                x += 24;
            }
        }
    }
}
fn merge_short_runs(colors: &mut [u16]) {
    let mut i = 0;
    while i < colors.len() {
        let mut end = i + 1;
        while end < colors.len() && colors[end] == colors[i] {
            end += 1;
        }
        if end - i < 5 && (i > 0 || end < colors.len()) {
            let color = if i > 0 { colors[i - 1] } else { colors[end] };
            colors[i..end].fill(color);
        }
        i = end;
    }
}
fn marker(cv: &mut impl Surface, row: &Row, p: Point, ink: u16, bg: u16) {
    match &row.item {
        Item::Settlement(_, SettlementClass::Hamlet) => cv.fill(rect(p.x - 8, p.y - 2, 17, 5), ink),
        Item::Settlement(_, class) => {
            let r = if matches!(class, SettlementClass::City | SettlementClass::Town) { 9 } else { 7 };
            cv.disc(p, r, ink);
            cv.disc(p, r - 2, bg);
        }
        Item::Waypoint(_) => {
            cv.triangle(Point::new(p.x, p.y - 9), Point::new(p.x - 9, p.y), Point::new(p.x + 9, p.y), ink);
            cv.triangle(Point::new(p.x, p.y + 9), Point::new(p.x - 9, p.y), Point::new(p.x + 9, p.y), ink);
            cv.triangle(Point::new(p.x, p.y - 5), Point::new(p.x - 5, p.y), Point::new(p.x + 5, p.y), bg);
            cv.triangle(Point::new(p.x, p.y + 5), Point::new(p.x - 5, p.y), Point::new(p.x + 5, p.y), bg);
        }
        Item::End => {
            cv.fill(rect(p.x - 9, p.y - 4, 19, 9), ink);
            cv.fill(rect(p.x - 7, p.y - 2, 15, 5), bg);
        }
        Item::Place(s) => {
            cv.disc(p, 12, bg);
            if let Some(cat) = obc_formats::obcm::poi_category_of(s.poi.subtype) {
                super::poi_menu::draw_category_icon(cv, cat, p, ink, bg);
            }
        }
    }
}
fn detail(cv: &mut impl Surface, rx: &Render) {
    let a = rx.ahead;
    let Some(row) = a.rows.get(a.selected) else {
        return;
    };
    let dist = distance(row.key.distance().saturating_sub(rx.navigation.progress_m), rx.settings.units);
    label(cv, &dist, 12, 44, Font::Body, INK);
    if let Some(up) = row.facts.and_then(|f| f.ascent_m).map(|up| {
        up.saturating_sub(rx.profile.map_or(0, |p| {
            p.ascent_between_m(a.window.unwrap().start_m, rx.navigation.progress_m, a.window.unwrap().end_m)
        }))
    }) {
        let climb = super::vocab::fmt::elevation_short(Some(up), rx.settings.units);
        super::poi_display::draw_climb_figure(cv, 122, 46, &climb, INK);
    }
    if let Some(ele) = row.facts.and_then(|f| f.elevation_m) {
        let mut text = heapless::String::<32>::new();
        let _ = write!(text, "{}{}", rx.settings.units.elev(ele as f32) as i32, rx.settings.units.elev_label());
        label(cv, &text, 12, 76, Font::Label, SUBTEXT);
    }
    let mut y = 108;
    cv.hline(8, y, 224, RULE);
    y += 4;
    if a.detail_rows.is_empty() {
        label(
            cv,
            if matches!(row.item, Item::Waypoint(_)) {
                rx.t(Msg::AheadCustom)
            } else if matches!(row.item, Item::End) {
                &a.route_name
            } else {
                rx.t(Msg::AheadNoMatches)
            },
            12,
            y + 12,
            Font::Label,
            SUBTEXT,
        );
        return;
    }
    let slots = ((310 - y) / 52).max(1) as usize;
    if a.detail_previous || a.detail_selected >= slots {
        cv.triangle(Point::new(224, y - 5), Point::new(218, y + 1), Point::new(230, y + 1), INK);
    }
    if a.detail_more || a.detail_selected + 1 < a.detail_rows.len() {
        cv.triangle(Point::new(224, 316), Point::new(218, 310), Point::new(230, 310), INK);
    }
    let first = a.detail_selected.saturating_sub(slots - 1);
    for (i, p) in a.detail_rows.iter().enumerate().skip(first).take(slots) {
        let selected = i == a.detail_selected;
        let bg = if selected { AMBER } else { PARCHMENT };
        let ink = if selected { ON_ACCENT } else { INK };
        if selected {
            cv.round(rect(4, y, 232, 50), 5, bg);
        }
        if let Some(cat) = obc_formats::obcm::poi_category_of(p.poi.subtype) {
            super::poi_menu::draw_category_icon(cv, cat, Point::new(21, y + 25), ink, bg);
            let subtype = obc_formats::obcm::poi_label_of(p.poi.subtype).unwrap_or("");
            label(cv, subtype, 42, y + 26, Font::Caption, if selected { SUBTEXT_ON_ACCENT } else { SUBTEXT });
        }
        fitted(cv, super::poi_display::poi_row_name(&p.poi), 42, y, 190, ink);
        y += 52;
    }
}
