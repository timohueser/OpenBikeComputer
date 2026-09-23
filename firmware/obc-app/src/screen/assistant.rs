//! The ordinary Ride Assistant question list.
mod icons;
use super::{
    palette::*,
    vocab::{
        chrome::{title_frame, LIST_TOP},
        list, rows,
    },
    Ctx, Render, Transition,
};
use crate::{navigator::VisitUnavailable, Gesture, Msg};
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
const LIVE_QUESTIONS: usize = 4;
const ROW_H: i32 = 36;
const ROW_PITCH: i32 = 38;
const TEXT_X: i32 = 52;

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
        for (i, question) in QUESTIONS.iter().enumerate() {
            let y = LIST_TOP + i as i32 * ROW_PITCH;
            let area = rows::row_rect(y, rx.w, ROW_H);
            let selected = i == self.selected;
            rows::row_cursor(cv, area, selected, false);
            let ink = if i >= LIVE_QUESTIONS {
                CONTOUR
            } else if selected {
                ON_ACCENT
            } else {
                INK
            };
            icons::draw(cv, *question, Point::new(22, y + 7), ink);
            cv.text(rx.t(*question), Point::new(TEXT_X, y + 7), Font::Label, TextAlign::Left, ink);
        }
    }
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
            // The icon column leaves the same label width in every language.
            for message in QUESTIONS {
                let text = crate::i18n::t(message, language);
                assert!(
                    obc_render::text::text_width(text, Font::Label) <= (240 - rows::ROW_X - TEXT_X) as u32,
                    "{language:?}: {text}"
                );
            }
            for message in [Msg::AssistantArrival, Msg::AssistantResumeNavigation] {
                let text = crate::i18n::t(message, language);
                assert!(obc_render::text::text_width(text, Font::Body) <= 212, "{language:?}: {text}");
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
