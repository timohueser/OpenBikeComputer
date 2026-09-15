//! Source-derived nearby cards and pre-paginated reading, with shared Visit detail.
use super::{palette::*, vocab::list, Ctx, RenderFrame, Screen, Transition};
use crate::{landmarks::Status, Gesture};
use core::fmt::Write;
use embedded_graphics::{draw_target::DrawTarget, prelude::Point};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, Surface,
};
#[derive(Debug)]
pub struct LandmarksScreen;
#[derive(Debug)]
pub struct LandmarkSourcesScreen;
impl LandmarksScreen {
    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let state = &mut cx.landmarks;
        match g {
            Gesture::Back if state.reading => {
                state.reading = false;
            }
            Gesture::Back => return Transition::Pop,
            Gesture::Step(n) if state.reading => {
                let Some(record) = state.record else {
                    return Transition::None;
                };
                let count = record.text_pages as usize + usize::from(!record.photo.is_absent());
                let next = list::step_selection(state.page as usize, n, count);
                if next == record.text_pages as usize {
                    if let Some(selection) = state.selection() {
                        return Transition::Push(Screen::LandmarkPhoto(
                            super::LandmarkPhotoScreen::new(selection, &state.name).linked(),
                        ));
                    }
                } else {
                    state.page = next as u16;
                }
            }
            Gesture::Step(n) => {
                state.selected =
                    list::step_selection(state.selected, n, state.rows.len() + usize::from(state.more) + 1);
                state.invalidate_selection();
            }
            Gesture::Press if !state.reading && state.selected >= state.rows.len() => {
                let next = state.more && state.selected == state.rows.len();
                if !next {
                    if let Some(fix) = cx.state.user_fix {
                        state.origin = (fix.lon, fix.lat);
                        state.generation = None;
                    } else {
                        state.status = Status::NoFix;
                        return Transition::None;
                    }
                }
                state.restart(next);
            }
            Gesture::Press if state.ready() && state.record.is_some() => {
                if !state.reading {
                    state.reading = true;
                    state.page = 0;
                } else if let Some(poi) = detail(state) {
                    return Transition::Push(Screen::PoiDetail(super::PoiDetailScreen::new(poi)));
                }
            }
            _ => {}
        }
        Transition::None
    }
    pub fn draw<D, F, S>(&self, cv: &mut Canvas<D, F>, rx: &mut RenderFrame<'_, S>)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: obc_map_scene::MapScene,
    {
        if rx.landmarks.reading {
            reading(cv, rx, false);
            return;
        }
        let state = rx.landmarks;
        let (mut min, mut max) = (state.origin, state.origin);
        for row in &state.rows {
            min = (min.0.min(row.position.0), min.1.min(row.position.1));
            max = (max.0.max(row.position.0), max.1.max(row.position.1));
        }
        let vp = super::find_place::fit(min, max, rx.w, rx.h, 208);
        let _ = super::map::draw_map_scene(cv, rx, &vp, None);
        for (i, row) in rx.landmarks.rows.iter().enumerate() {
            let (x, y) = vp.to_screen(row.position.0, row.position.1);
            cv.round(rect(x - 11, y - 12, 23, 24), 4, if i == rx.landmarks.selected { AMBER } else { PARCHMENT });
            cv.text(letter(i), Point::new(x, y - 12), Font::Label, TextAlign::Center, INK);
        }
        let (x, y) = vp.to_screen(rx.landmarks.origin.0, rx.landmarks.origin.1);
        cv.disc(Point::new(x, y), 5, INK);
        header(cv, "Landmarks");
        cv.fill(rect(0, 208, 240, 112), PARCHMENT);
        cv.round(rect(6, 210, 228, 104), 6, AMBER);
        let state = rx.landmarks;
        if state.selected >= state.rows.len() && state.ready() {
            cv.text(
                if state.more && state.selected == state.rows.len() { "More landmarks" } else { "Refresh" },
                Point::new(12, 238),
                Font::Body,
                TextAlign::Left,
                INK,
            );
            cv.text("Within 10km", Point::new(12, 272), Font::Label, TextAlign::Left, SUBTEXT);
            return;
        }
        if let Some(record) = state.record.filter(|_| state.ready()) {
            let mut label = heapless::String::<40>::new();
            let _ = write!(label, "{}  {}", letter(state.selected), kind(record.category));
            cv.text(&label, Point::new(12, 212), Font::Label, TextAlign::Left, SUBTEXT);
            cv.text(&super::poi_list::fit(&state.name, 18), Point::new(12, 238), Font::Label, TextAlign::Left, INK);
            label.clear();
            super::vocab::fmt::write_distance_coarse(
                &mut label,
                "",
                state.selected().unwrap().key.distance_m,
                rx.settings.units,
            );
            let _ = label.push_str(" straight line");
            cv.text(&label, Point::new(12, 262), Font::Label, TextAlign::Left, INK);
            cv.text(
                if state.status == Status::Partial { "Partial data" } else { "Select to read" },
                Point::new(120, 288),
                Font::Label,
                TextAlign::Center,
                SUBTEXT,
            );
        } else {
            cv.text(status(state.status), Point::new(12, 238), Font::Label, TextAlign::Left, INK);
            if state.status != Status::Loading {
                cv.text("Select to refresh", Point::new(12, 288), Font::Label, TextAlign::Left, SUBTEXT);
            }
        }
    }
}
impl LandmarkSourcesScreen {
    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Back | Gesture::Press => return Transition::Pop,
            Gesture::Step(n) => {
                cx.landmarks.source_page = list::step_selection(
                    cx.landmarks.source_page as usize,
                    n,
                    cx.landmarks.source_pages.max(1) as usize,
                ) as u16
            }
            _ => {}
        }
        Transition::None
    }
    pub fn draw<D, F, S>(&self, cv: &mut Canvas<D, F>, rx: &mut RenderFrame<'_, S>)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: obc_map_scene::MapScene,
    {
        reading(cv, rx, true);
    }
}
pub(crate) fn detail(state: &crate::landmarks::Landmarks) -> Option<obc_reader::Poi> {
    let record = state.record?;
    let metadata = record.osm?;
    let mut name = heapless::String::new();
    for c in state.name.chars() {
        if name.push(c).is_err() {
            break;
        }
    }
    Some(obc_reader::Poi {
        opening: obc_reader::hours::OpeningStatus::Unknown,
        metadata,
        lon: record.lon,
        lat: record.lat,
        subtype: 0,
        name,
        hours_ref: record.hours_ref,
        distance_m: state.selected()?.key.distance_m,
    })
}
fn reading<D, F, S>(cv: &mut Canvas<D, F>, rx: &RenderFrame<'_, S>, sources: bool)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
    S: obc_map_scene::MapScene,
{
    cv.clear(PARCHMENT);
    let state = rx.landmarks;
    header(cv, if sources { "Sources" } else { &state.name });
    if !state.ready() || state.record.is_none() {
        cv.text(status(state.status), Point::new(12, 100), Font::Label, TextAlign::Left, INK);
        return;
    }
    for (i, line) in state.text.lines().enumerate() {
        cv.text(
            line,
            Point::new(12, 40 + i as i32 * Font::Label.line_height() as i32),
            Font::Label,
            TextAlign::Left,
            INK,
        );
    }
    let mut label = heapless::String::<40>::new();
    if sources {
        let _ = write!(label, "Back  {}/{}", state.source_page + 1, state.source_pages);
    } else {
        let record = state.record.unwrap();
        let action = if !rx.poi_scratch.detail_valid {
            "Access unavailable"
        } else if rx
            .poi_scratch
            .detail_schedule
            .is_some_and(|s| s.status(rx.place_local) == obc_reader::hours::OpeningStatus::Closed)
        {
            "Closed"
        } else if record.osm.and_then(|m| m.approach).is_some() {
            "Visit"
        } else {
            "No mapped access"
        };
        let _ = write!(
            label,
            "{} {}/{}",
            action,
            state.page + 1,
            record.text_pages as u16 + u16::from(!record.photo.is_absent())
        );
    }
    cv.round(rect(4, 282, 232, 34), 6, AMBER);
    cv.text(&label, Point::new(120, 286), Font::Label, TextAlign::Center, INK);
}
fn header(cv: &mut impl Surface, title: &str) {
    cv.fill(rect(0, 0, 240, 40), PARCHMENT);
    cv.round(rect(4, 4, 232, 34), 6, WOOD);
    cv.text(&super::poi_list::fit(title, 18), Point::new(12, 9), Font::Label, TextAlign::Left, PARCHMENT);
}
fn letter(i: usize) -> &'static str {
    ["A", "B", "C", "D"][i.min(3)]
}
fn kind(category: u8) -> &'static str {
    match category {
        1 => "Natural site",
        2 => "Castle / ruin",
        3 => "Archaeology",
        4 => "Abbey",
        5 => "Cathedral",
        6 => "Pass",
        _ => "Landmark",
    }
}
fn status(status: Status) -> &'static str {
    match status {
        Status::Loading => "Loading landmarks",
        Status::Missing => "No landmark content",
        Status::Unsupported => "Unsupported content",
        Status::Failed => "Content unavailable",
        Status::Partial => "Partial data",
        Status::NoFix => "GPS required",
        Status::NoMap => "No map",
        Status::Stale => "Map changed: refresh",
        Status::Empty => "None within 10km",
        _ => "Select a landmark",
    }
}
