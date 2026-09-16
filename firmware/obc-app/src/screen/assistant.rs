//! The ordinary Ride Assistant question list.
use super::{
    palette::*,
    vocab::{chrome::title_frame, list},
    Ctx, Render, Transition,
};
use crate::{navigator::VisitUnavailable, Gesture, Msg};
use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

pub(crate) const QUESTIONS: [Msg; 7] = [
    Msg::AssistantFind,
    Msg::AssistantNext,
    Msg::AssistantEasier,
    Msg::AssistantBlocked,
    Msg::AssistantBackRoute,
    Msg::AssistantLandmarks,
    Msg::AssistantDetour,
];
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
        let first = list::window_start(self.selected, 6, QUESTIONS.len());
        for (slot, &label) in QUESTIONS.iter().skip(first).take(6).enumerate() {
            let i = first + slot;
            let y = 43 + slot as i32 * 44;
            if i == self.selected {
                cv.round(rect(10, y, rx.w - 20, 40), 6, AMBER);
            }
            let ink = if matches!(i, 0 | 1 | 2 | 5) { INK } else { SUBTEXT };
            cv.text(rx.t(label), Point::new(18, y + 5), Font::Body, TextAlign::Left, ink);
        }
        cv.vline(rx.w - 7, 44, 261, 2, RULE);
        cv.vline(rx.w - 7, 44 + first as i32 * 36, 225, 2, WOOD);
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
        for (selected, expected) in [(0, "FindPlace"), (1, "WhatsNext"), (5, "Landmarks")] {
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
        for selected in [3, 4, 6] {
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
            for message in QUESTIONS.into_iter().chain([Msg::AssistantArrival, Msg::AssistantResumeJourney]) {
                let text = crate::i18n::t(message, language);
                let width = obc_render::text::text_width(text, Font::Body);
                assert!(width <= 212, "{language:?}: {text}");
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
