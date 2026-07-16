//! The Language screen — the UI language (epic #602). [`Language`](crate::settings::Language) will
//! re-translate every user-facing string once the catalog lands; today it only persists the choice.
//! A single value row cycling the four languages by their **endonyms**, so it reads to a speaker who
//! can't yet read the current UI language — press (or a turn) walks it in place, no field sub-mode.

use obc_render::Surface;

use crate::input::Gesture;
use crate::screen::{title_frame, Ctx, Render, Transition, LIST_TOP};
use crate::Msg;

/// The Language screen. Stateless — the value lives in [`Settings`](crate::Settings); the one row is
/// always the cursor.
#[derive(Debug, Default)]
pub struct LanguageScreen;

impl LanguageScreen {
    pub fn new() -> Self {
        LanguageScreen
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // A short ring, so — like Units — there's no separate edit mode: press cycles one
            // forward, a turn walks the ring in place.
            Gesture::Press => {
                cx.settings.language = cx.settings.language.cycled();
                Transition::None
            }
            Gesture::Turn(n) => {
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

        // The single value row — the current language's endonym centred, flanked by left/right arrows
        // to read as "rotate to switch". Shared with the Units picker (`value_row_with_arrows`).
        super::value_row_with_arrows(cv, LIST_TOP + 8, w, language.name());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::settings::Language;
    use crate::{AppState, Mode, Settings};

    fn run(scr: &mut LanguageScreen, s: &mut Settings, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let scratch = crate::screen::PoiScratch::new();
        let mut cx = Ctx {
            state: &mut st,
            activity: &mut act,
            settings: s,
            routes: &[],
            rides: &[],
            trips: &[],
            nav_profiles: &crate::NavProfiles::EMPTY,
            poi_scratch: &scratch,
            sensor_scan_hits: &[],
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// Press cycles one language forward and a turn walks the ring in place (no edit sub-mode);
    /// Back pops the screen.
    #[test]
    fn press_cycles_and_turn_walks() {
        let mut s = Settings { language: Language::En, ..Settings::default() };
        let mut scr = LanguageScreen::new();
        run(&mut scr, &mut s, Gesture::Press);
        assert_eq!(s.language, Language::De, "press cycles English → Deutsch");
        run(&mut scr, &mut s, Gesture::Turn(1));
        assert_eq!(s.language, Language::Fr, "a turn walks Deutsch → Français");
        run(&mut scr, &mut s, Gesture::Turn(-1));
        assert_eq!(s.language, Language::De, "and back");
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop));
    }
}
