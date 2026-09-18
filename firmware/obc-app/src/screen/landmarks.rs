//! Source-derived nearby cards and pre-paginated reading, with shared Visit detail.
use super::{palette::*, vocab::list, Ctx, RenderFrame, Screen, Transition};
use crate::{landmarks::Status, Gesture, Msg};
use core::fmt::Write;
use embedded_graphics::{draw_target::DrawTarget, prelude::Point};
/// Top row of the opaque bottom panel, and the camera's bottom bound.
const PANEL_TOP: i32 = 208;

use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
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
                let Some(article) = state.article else {
                    return Transition::None;
                };
                let count = article.text_pages as usize + usize::from(state.photo_available);
                let next = list::step_selection(state.page as usize, n, count);
                if next == article.text_pages as usize {
                    if let Some(selection) = state.selection() {
                        return Transition::Push(Screen::LandmarkPhoto(
                            super::LandmarkPhotoScreen::content(selection, &state.name).linked(),
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
            Gesture::Press
                if (!state.ready() && state.status != Status::Loading)
                    || (!state.reading && state.selected >= state.rows.len()) =>
            {
                let next = state.ready() && state.more && state.selected == state.rows.len();
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
                    return Transition::Push(Screen::PoiDetail(
                        super::PoiDetailScreen::new(poi).landmark(state.record.unwrap().category),
                    ));
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
        let vp = super::find_place::fit(min, max, rx.w, rx.h, PANEL_TOP);
        let _ = super::map::draw_map_scene(cv, rx, &vp, None, &[super::find_place::panel(rx.w, rx.h, PANEL_TOP)]);
        for (i, row) in rx.landmarks.rows.iter().enumerate() {
            let (x, y) = vp.to_screen(row.position.0, row.position.1);
            cv.round(rect(x - 11, y - 12, 23, 24), 4, if i == rx.landmarks.selected { AMBER } else { PARCHMENT });
            cv.text(letter(i), Point::new(x, y - 12), Font::Label, TextAlign::Center, INK);
        }
        let (x, y) = vp.to_screen(rx.landmarks.origin.0, rx.landmarks.origin.1);
        cv.disc(Point::new(x, y), 5, INK);
        header(cv, rx.t(Msg::AssistantLandmarks), None, None);
        cv.fill(super::find_place::panel(rx.w, rx.h, PANEL_TOP), PARCHMENT);
        cv.round(rect(6, 210, 228, 104), 6, AMBER);
        let state = rx.landmarks;
        if state.selected >= state.rows.len() && state.status == Status::Ready {
            cv.text(
                if state.more && state.selected == state.rows.len() {
                    rx.t(Msg::AssistantMoreLandmarks)
                } else {
                    rx.t(Msg::AssistantRefresh)
                },
                Point::new(12, 238),
                Font::Body,
                TextAlign::Left,
                INK,
            );
            let mut range = heapless::String::<32>::new();
            let _ = write!(range, "{} ", rx.t(Msg::AssistantWithin));
            super::vocab::fmt::write_distance_coarse(&mut range, "", 10_000, rx.settings.units);
            cv.text(&range, Point::new(12, 272), Font::Label, TextAlign::Left, SUBTEXT);
            return;
        }
        if let Some(record) = state.record.filter(|_| state.ready()) {
            let mut label = heapless::String::<40>::new();
            let _ = write!(label, "{}  {}", letter(state.selected), rx.t(kind(record.category)));
            cv.text(&label, Point::new(12, 212), Font::Label, TextAlign::Left, SUBTEXT);
            let name_row = rect(12, 238, 18 * Font::Label.char_width() as i32, Font::Label.line_height() as i32);
            let name = rx.marquee.fit(&state.name, 18, Some(name_row));
            cv.text(&name, Point::new(12, 238), Font::Label, TextAlign::Left, INK);
            label.clear();
            super::vocab::fmt::write_distance_coarse(
                &mut label,
                "",
                state.selected().unwrap().key.distance_m,
                rx.settings.units,
            );
            let _ = write!(label, " {}", rx.t(Msg::AssistantStraight));
            cv.text(&label, Point::new(12, 262), Font::Label, TextAlign::Left, INK);
            cv.text(
                if state.status == Status::Partial {
                    rx.t(Msg::AssistantPartialData)
                } else {
                    rx.t(Msg::AssistantSelectRead)
                },
                Point::new(120, 288),
                Font::Label,
                TextAlign::Center,
                SUBTEXT,
            );
        } else {
            cv.text(rx.t(status(state.status)), Point::new(12, 238), Font::Label, TextAlign::Left, INK);
            if state.status != Status::Loading {
                cv.text(rx.t(Msg::AssistantSelectRefresh), Point::new(12, 288), Font::Label, TextAlign::Left, SUBTEXT);
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
pub(super) fn reading<D, F, S>(cv: &mut Canvas<D, F>, rx: &RenderFrame<'_, S>, sources: bool)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
    S: obc_map_scene::MapScene,
{
    cv.clear(PARCHMENT);
    let state = rx.landmarks;
    let page = state.article.filter(|_| state.ready()).map(|_| {
        if sources {
            (state.source_page + 1, state.source_pages)
        } else {
            (state.page + 1, state.article.map_or(0, |a| a.text_pages as u16) + u16::from(state.photo_available))
        }
    });
    header(
        cv,
        if sources { rx.t(Msg::RideContextSources) } else { &state.name },
        page,
        (!sources).then_some(&rx.marquee),
    );
    if !state.ready() || state.article.is_none() {
        cv.text(rx.t(status(state.status)), Point::new(12, 100), Font::Label, TextAlign::Left, INK);
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
    let label = if sources || state.peak.is_some() {
        rx.t(Msg::AssistantBack)
    } else {
        rx.t(visit_action(state, rx.poi_scratch, rx.settings.bike_profile_idx))
    };
    cv.round(rect(4, 282, 232, 34), 6, AMBER);
    cv.text(label, Point::new(120, 286), Font::Label, TextAlign::Center, INK);
}
pub(super) fn visit_action(
    state: &crate::landmarks::Landmarks,
    scratch: &super::poi_list::PoiScratch,
    profile: u8,
) -> Msg {
    if !state.ready()
        || state.record.is_none()
        || !scratch.detail_valid
        || scratch.detail_source != state.record.and_then(|r| r.osm).map_or(0, |m| m.source.0)
    {
        Msg::AssistantAccessUnavailable
    } else if state
        .record
        .and_then(|r| r.osm)
        .and_then(|m| m.approach)
        .is_some_and(|a| a.profile_mask & (1 << profile.min(7)) != 0)
    {
        Msg::AssistantVisit
    } else {
        Msg::AssistantNoAccess
    }
}
pub(super) fn header(
    cv: &mut impl Surface,
    title: &str,
    page: Option<(u16, u16)>,
    marquee: Option<&super::vocab::marquee::MarqueeFrame>,
) {
    cv.fill(rect(0, 0, 240, 40), PARCHMENT);
    cv.round(rect(4, 4, 232, 34), 6, WOOD);
    let mut count = heapless::String::<12>::new();
    if let Some((current, total)) = page {
        let _ = write!(count, "{current}/{total}");
    }
    let reserved = if count.is_empty() { 0 } else { text_width(&count, Font::Label) as usize + 12 };
    let title_chars = (216 - reserved) / Font::Label.char_width() as usize;
    let title = match marquee {
        Some(marquee) => marquee.fit(title, title_chars, Some(rect(4, 4, 232, 34))),
        None => super::vocab::marquee::fit(title, title_chars),
    };
    cv.text(&title, Point::new(12, 9), Font::Label, TextAlign::Left, PARCHMENT);
    cv.text(&count, Point::new(228, 9), Font::Label, TextAlign::Right, PARCHMENT);
}
fn letter(i: usize) -> &'static str {
    ["A", "B", "C", "D"][i.min(3)]
}
pub(super) fn kind(category: u8) -> Msg {
    match category {
        1 => Msg::AssistantNatural,
        2 => Msg::AssistantCastle,
        3 => Msg::AssistantArchaeology,
        4 => Msg::AssistantAbbey,
        5 => Msg::AssistantCathedral,
        6 => Msg::AssistantPass,
        _ => Msg::AssistantLandmark,
    }
}
fn status(status: Status) -> Msg {
    match status {
        Status::Loading => Msg::AssistantLandmarksLoading,
        Status::Missing => Msg::AssistantLandmarksMissing,
        Status::Unsupported => Msg::AssistantUnsupported,
        Status::Failed => Msg::AssistantContentUnavailable,
        Status::Partial => Msg::AssistantPartialData,
        Status::NoFix => Msg::AssistantNoFix,
        Status::NoMap => Msg::AssistantNoMap,
        Status::Stale => Msg::AssistantRefreshNeeded,
        Status::Empty => Msg::AssistantNoneFound,
        _ => Msg::AssistantSelectLandmark,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::obcm::{landmarks::*, PoiApproach, PoiMetadata, SourceId};

    #[test]
    fn article_and_photo_visit_availability_require_the_selected_sources_access() {
        let mut state = crate::landmarks::Landmarks::new();
        state.status = Status::Ready;
        state.record = Some(LandmarkRecord {
            qid: 1,
            lon: 0,
            lat: 0,
            category: 2,
            hours_ref: 0,
            osm: None,
            name: ContentRef::default(),
            articles: ContentRef::default(),
            photo: ContentRef::default(),
            photo_attribution: ContentRef::default(),
        });
        let mut scratch = super::super::PoiScratch::new();
        scratch.detail_valid = true;
        assert!(matches!(visit_action(&state, &scratch, 0), Msg::AssistantNoAccess));
        state.record.as_mut().unwrap().osm = Some(PoiMetadata {
            source: SourceId(42),
            approach: Some(PoiApproach { source: SourceId(43), lat: 0, lon: 0, profile_mask: 1 }),
        });
        assert!(matches!(visit_action(&state, &scratch, 0), Msg::AssistantAccessUnavailable));
        scratch.detail_source = 42;
        assert!(matches!(visit_action(&state, &scratch, 0), Msg::AssistantVisit));
        assert!(matches!(visit_action(&state, &scratch, 1), Msg::AssistantNoAccess));
        scratch.detail_schedule = obc_reader::WeeklySchedule::decode(&[0; 29]);
        assert!(matches!(visit_action(&state, &scratch, 0), Msg::AssistantVisit));
        assert!(matches!(visit_action(&state, &scratch, 0), Msg::AssistantVisit));
        state.invalidate();
        assert!(matches!(visit_action(&state, &scratch, 0), Msg::AssistantAccessUnavailable));
    }
}
