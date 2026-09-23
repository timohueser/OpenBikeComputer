//! The ordinary Ride Assistant question list.
use super::{
    palette::*,
    vocab::{
        chrome::{title_frame, LIST_TOP},
        fmt::write_distance_coarse,
        list::{self, scrollbar},
        rows::{self, Line2, ROW_GAP},
    },
    Ctx, Render, Transition,
};
use crate::{navigator::VisitUnavailable, Gesture, Msg};
use core::fmt::Write;
use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

/// The live questions, then the placeholders. A placeholder stays in the list, inert, as a
/// reminder of what the Assistant will answer.
pub(crate) const QUESTIONS: [Msg; 7] = [
    Msg::AssistantFind,
    Msg::AssistantNext,
    Msg::AssistantEasier,
    Msg::AssistantLandmarks,
    Msg::AssistantBlocked,
    Msg::AssistantBackRoute,
    Msg::AssistantDetour,
];
/// The hint under each live question, index for index. Every question with a hint is live.
const HINTS: [Msg; 4] =
    [Msg::AssistantFindHint, Msg::AssistantNextHint, Msg::AssistantEasierHint, Msg::AssistantLandmarksHint];

#[derive(Debug, Default)]
pub struct AssistantScreen {
    pub(crate) selected: usize,
    pub(crate) error: Option<VisitUnavailable>,
}
impl AssistantScreen {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn handle(&mut self, g: Gesture, _: &mut Ctx) -> Transition {
        match g {
            Gesture::Back if self.error.is_some() => self.error = None,
            Gesture::Back => return Transition::Pop,
            Gesture::Step(n) if self.error.is_none() => {
                self.selected = list::step_selection(self.selected, n, QUESTIONS.len())
            }
            _ => {}
        }
        Transition::None
    }
    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        title_frame(cv, rx.w, rx.h, rx.t(Msg::AssistantTitle), "");
        if let Some(error) = self.error {
            let message = match error {
                VisitUnavailable::NoFix => Msg::AssistantNoFix,
                VisitUnavailable::Unmatched => Msg::AssistantNoRoute,
                VisitUnavailable::Busy => Msg::AssistantBusy,
                _ => Msg::AssistantUnavailable,
            };
            cv.text(rx.t(message), Point::new(rx.w / 2, 124), Font::Label, TextAlign::Center, INK);
            cv.text(rx.t(Msg::AssistantRetry), Point::new(rx.w / 2, 164), Font::Label, TextAlign::Center, SUBTEXT);
            return;
        }
        let heights: [i32; QUESTIONS.len()] = core::array::from_fn(|i| rows::row_height(i < HINTS.len()));
        let avail = rx.h - LIST_TOP - 6;
        let (first, end) = rows::window_by_height(&heights, self.selected, avail);
        let mut next = heapless::String::<24>::new();
        let mut y = LIST_TOP;
        for i in first..end {
            let area = rows::row_rect(y, rx.w, heights[i]);
            let hint = match HINTS.get(i) {
                Some(Msg::AssistantNextHint) => Some(next_hint(rx, &mut next).unwrap_or(rx.t(Msg::AssistantNextHint))),
                Some(hint) => Some(rx.t(*hint)),
                None => None,
            };
            // No chevron: its column would leave the hint too few characters.
            rows::nav_row(
                cv,
                area,
                rx.t(QUESTIONS[i]),
                hint.map(Line2::text),
                i == self.selected,
                hint.is_some(),
                false,
            );
            y += heights[i] + ROW_GAP;
        }
        scrollbar(cv, rx.w - 8, LIST_TOP, avail, QUESTIONS.len(), first, end - first);
    }
}

/// What's next's first answer, from the resident climbs over the same window its overview shows.
/// It reads no card, so the first frame has it. `None` leaves the static hint.
fn next_hint<'a>(rx: &Render<'a>, buf: &'a mut heapless::String<24>) -> Option<&'a str> {
    let nav = rx.navigation;
    if nav.active_route.is_none() {
        return Some(rx.t(Msg::AssistantNoRoute));
    }
    let start_m = nav.progress_m;
    let end_m = start_m.saturating_add(rx.ahead.range.meters()).min(nav.route_total_m);
    let climb = rx.climbs.ahead(start_m, end_m)?;
    if climb.start_m <= start_m {
        return Some(rx.t(Msg::AheadOnClimb));
    }
    let _ = write!(buf, "{} ", rx.t(Msg::AssistantClimbIn));
    write_distance_coarse(buf, "", climb.start_m - start_m, rx.settings.units);
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{input::Chord, screen::Screen, settings::Language, App, AppState};

    fn open() -> App {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        assert!(app.apply_chord(Chord::Assistant));
        assert!(matches!(app.top_screen(), Screen::Assistant(_)));
        app
    }
    #[test]
    fn held_shortcut_reaches_real_questions_and_keeps_placeholders_inert() {
        for (selected, expected) in [(0, "FindPlace"), (1, "WhatsNext"), (3, "Landmarks")] {
            let mut app = open();
            app.apply_gesture(Gesture::Step(selected));
            app.apply_gesture(Gesture::Press);
            assert_eq!(app.top_screen().name(), expected);
            app.apply_gesture(Gesture::Back);
            assert!(matches!(app.top_screen(), Screen::Assistant(s) if s.selected == selected as usize));
            assert!(app.settings().ble_enabled);
        }
        let mut app = open();
        app.apply_gesture(Gesture::Step(2));
        app.apply_gesture(Gesture::Press);
        assert!(matches!(app.top_screen(), Screen::Assistant(s) if s.error.is_some()));
        app.apply_gesture(Gesture::Back);
        assert!(matches!(app.top_screen(), Screen::Assistant(s) if s.error.is_none()));
        for selected in [4, 5, 6] {
            let mut app = open();
            app.apply_gesture(Gesture::Step(selected));
            app.apply_gesture(Gesture::Press);
            assert!(
                matches!(app.top_screen(), Screen::Assistant(s) if s.selected == selected as usize && s.error.is_none())
            );
            assert!(app.active_route_index().is_none());
            assert!(!app.recording());
        }
    }
    #[test]
    fn question_and_photo_labels_fit_all_supported_languages() {
        for language in Language::ALL {
            // A row without a chevron: wider, and the label drops to the Label cut.
            for message in QUESTIONS {
                let text = crate::i18n::t(message, language);
                assert!(obc_render::text::text_width(text, Font::Body) <= 202, "{language:?}: {text}");
            }
            for message in [Msg::AssistantArrival, Msg::AssistantResumeNavigation] {
                let text = crate::i18n::t(message, language);
                assert!(obc_render::text::text_width(text, Font::Body) <= 212, "{language:?}: {text}");
            }
            // A hint line has the row's width less the text inset; the longest distance is 6 cells.
            let climb = std::format!("{} 5279ft", crate::i18n::t(Msg::AssistantClimbIn, language));
            let hints = HINTS.map(|m| crate::i18n::t(m, language));
            for text in hints.iter().copied().chain([
                climb.as_str(),
                crate::i18n::t(Msg::AheadOnClimb, language),
                crate::i18n::t(Msg::AssistantNoRoute, language),
            ]) {
                assert!(obc_render::text::text_width(text, Font::Label) <= 202, "{language:?}: {text}");
            }
            for message in [Msg::AssistantMorePlaces, Msg::AssistantSearchChanged] {
                let text = crate::i18n::t(message, language);
                assert!(obc_render::text::text_width(text, Font::Body) <= 212, "{language:?}: {text}");
            }
            let text = crate::i18n::t(Msg::AssistantBrowsePlaces, language);
            assert!(obc_render::text::text_width(text, Font::Label) <= 204, "{language:?}: {text}");
            for message in [
                Msg::AssistantPhotoVisit,
                Msg::AssistantPhotoNoAccess,
                Msg::AssistantPhotoUnavailableHint,
                Msg::AssistantPhotoMissing,
                Msg::AssistantPhotoUnavailable,
            ] {
                let text = crate::i18n::t(message, language);
                assert!(obc_render::text::text_width(text, Font::Label) <= 228, "{language:?}: {text}");
            }
            for message in [
                Msg::AssistantCalculating,
                Msg::AssistantAddStop,
                Msg::AssistantGoHere,
                Msg::AssistantSaving,
                Msg::AssistantBackMap,
                Msg::AssistantCancelVisit,
                Msg::AssistantResumeRoute,
                Msg::AssistantVisitUnavailable,
                Msg::AssistantUseRoute,
                Msg::AssistantPreviewRoute,
            ] {
                let text = crate::i18n::t(message, language);
                assert!(obc_render::text::text_width(text, Font::Body) <= 216, "{language:?}: {text}");
            }
            for message in [Msg::AssistantAddStop, Msg::AssistantGoHere] {
                let text = crate::i18n::t(message, language);
                assert!(obc_render::text::text_width(text, Font::Body) <= 180, "{language:?}: {text}");
            }
            for message in
                [Msg::AssistantVisit, Msg::AssistantClosed, Msg::AssistantNoAccess, Msg::AssistantAccessUnavailable]
            {
                let text = crate::i18n::t(message, language);
                assert!(
                    obc_render::text::text_width(text, Font::Label) + 4 * Font::Label.char_width() <= 228,
                    "{language:?}: {text} plus page count"
                );
            }
        }
    }
}
