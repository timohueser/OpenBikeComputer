//! The Language screen: one value row that cycles the four languages. It shows each language by its
//! own name, so it reads to a speaker who cannot read the current UI language.

use obc_render::Surface;

use crate::input::Gesture;
use crate::screen::vocab::chrome::{title_frame, LIST_TOP};
use crate::screen::vocab::rows::value_row_with_arrows;
use crate::screen::{Ctx, Render, Transition};
use crate::Msg;

/// Stateless. The value lives in [`Settings`](crate::Settings), and the one row is always the cursor.
#[derive(Debug, Default)]
pub struct LanguageScreen;

impl LanguageScreen {
    pub fn new() -> Self {
        LanguageScreen
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // The ring is short, so there is no separate edit mode.
            Gesture::Press => {
                cx.settings.language = cx.settings.language.cycled();
                Transition::None
            }
            Gesture::Step(n) => {
                cx.settings.language = cx.settings.language.stepped(n);
                Transition::None
            }
            Gesture::Back => Transition::Pop,
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let (w, h) = (rx.w, rx.h);
        let language = rx.settings.language;
        title_frame(cv, w, h, rx.t(Msg::LanguageTitle), "");

        value_row_with_arrows(cv, LIST_TOP + 8, w, language.name());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::screen::test_ctx;
    use crate::settings::Language;
    use crate::{AppState, Mode, Settings};

    fn run(scr: &mut LanguageScreen, s: &mut Settings, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut cx = test_ctx(&mut st, &mut act, s);
        scr.handle(g, &mut cx)
    }

    #[test]
    fn press_cycles_and_turn_walks() {
        let mut s = Settings { language: Language::En, ..Settings::default() };
        let mut scr = LanguageScreen::new();
        run(&mut scr, &mut s, Gesture::Press);
        assert_eq!(s.language, Language::De, "press cycles English → Deutsch");
        run(&mut scr, &mut s, Gesture::Step(1));
        assert_eq!(s.language, Language::Fr, "a step walks Deutsch → Français");
        run(&mut scr, &mut s, Gesture::Step(-1));
        assert_eq!(s.language, Language::De, "and back");
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop));
    }
}
