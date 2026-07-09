//! The Language screen — the UI language (epic #602). [`Language`](crate::settings::Language) will
//! re-translate every user-facing string once the catalog lands; today it only persists the choice.
//! A single value row cycling the four languages by their **endonyms**, so it reads to a speaker who
//! can't yet read the current UI language — press (or a turn) walks it in place, no field sub-mode.

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::{palette, title_frame, Ctx, Render, Transition, LIST_TOP};

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
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        let language = rx.settings.language;
        title_frame(cv, w, h, "LANGUAGE", "");

        // The single value row — the current language's endonym centred, flanked by left/right arrows
        // to read as "rotate to switch".
        let area = super::row_rect(LIST_TOP + 8, w, 50);
        super::row_cursor(cv, area, true, false);
        let midy = area.top_left.y + area.size.height as i32 / 2;
        cv.text_vcentered(language.name(), w / 2, (area.top_left.y, 50), Font::Body, TextAlign::Center, INK);
        // ◄ and ► as filled triangles, inset from the row edges.
        let ax = area.top_left.x + 18;
        cv.triangle(Point::new(ax, midy - 9), Point::new(ax, midy + 9), Point::new(ax - 11, midy), INK);
        let bx = area.top_left.x + area.size.width as i32 - 18;
        cv.triangle(Point::new(bx, midy - 9), Point::new(bx, midy + 9), Point::new(bx + 11, midy), INK);
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
            nav_profiles: &crate::NavProfiles::EMPTY,
            poi_scratch: &scratch,
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
