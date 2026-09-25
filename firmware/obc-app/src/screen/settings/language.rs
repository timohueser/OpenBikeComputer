//! The Language screen: a pick list of the four languages, each by its own name and flag, so it
//! reads to a speaker who cannot read the current UI language. A press commits and returns.

use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::vocab::chrome::{title_frame, LIST_TOP};
use crate::screen::vocab::flags::{draw_flag, FLAG_H};
use crate::screen::vocab::rows::{row_cursor, row_rect, ROW_GAP, ROW_ONE};
use crate::screen::vocab::sheet::committed_tick;
use crate::screen::{palette, Ctx, Render, Transition};
use crate::settings::Language;
use crate::Msg;

/// The cursor opens on the committed language.
#[derive(Debug)]
pub struct LanguageScreen {
    selected: usize,
}

impl LanguageScreen {
    pub fn new(current: Language) -> Self {
        LanguageScreen { selected: current as usize }
    }

    /// The language under the cursor.
    pub(crate) fn cursor(&self) -> Language {
        Language::ALL[self.selected]
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => {
                self.selected = crate::screen::vocab::list::step_selection(self.selected, n, Language::COUNT);
                Transition::None
            }
            Gesture::Press => {
                cx.settings.language = self.cursor();
                Transition::Pop
            }
            Gesture::Back => Transition::Pop,
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::LanguageTitle), "");
        let committed = rx.settings.language;
        for (i, lang) in Language::ALL.iter().enumerate() {
            let y = LIST_TOP + i as i32 * (ROW_ONE + ROW_GAP);
            let area = row_rect(y, w, ROW_ONE);
            row_cursor(cv, area, i == self.selected, false);
            let x = area.top_left.x + 10;
            let cy = y + ROW_ONE / 2;
            draw_flag(cv, x + 1, cy - FLAG_H / 2, *lang);
            cv.text_vcentered(
                lang.name(),
                x + 30,
                (y, ROW_ONE),
                Font::Body,
                TextAlign::Left,
                if i == self.selected { palette::ON_ACCENT } else { palette::INK },
            );
            if *lang == committed {
                let tx = area.top_left.x + area.size.width as i32 - 22;
                // The committed tick of the drawer editor, at row scale.
                for k in 0..2 {
                    committed_tick(cv, tx + k, cy - k, palette::WOOD);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::screen::test_ctx;
    use crate::{AppState, Mode, Settings};

    fn run(scr: &mut LanguageScreen, s: &mut Settings, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut cx = test_ctx(&mut st, &mut act, s);
        scr.handle(g, &mut cx)
    }

    #[test]
    fn opens_on_the_committed_language_and_a_press_commits_and_returns() {
        let mut s = Settings { language: Language::De, ..Settings::default() };
        let mut scr = LanguageScreen::new(s.language);
        assert_eq!(scr.selected, 1, "the cursor starts on Deutsch");
        run(&mut scr, &mut s, Gesture::Step(1));
        assert_eq!(s.language, Language::De, "a step commits nothing");
        assert!(matches!(run(&mut scr, &mut s, Gesture::Press), Transition::Pop));
        assert_eq!(s.language, Language::Fr, "press commits Français and returns");
        run(&mut scr, &mut s, Gesture::Step(-3));
        assert_eq!(scr.selected, 3, "the list wraps");
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop));
        assert_eq!(s.language, Language::Fr, "Back leaves the committed language alone");
    }
}
