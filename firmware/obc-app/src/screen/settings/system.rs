//! The System settings menu: a nav list of the standalone pages. Each row opens its own page.
//! The destructive Reset page stays last.

use obc_render::Surface;

use crate::input::Gesture;
use crate::screen::vocab::list;
use crate::screen::{
    AboutScreen, Ctx, DateTimeScreen, FirmwareScreen, LanguageScreen, Render, ResetScreen, Screen, Transition,
    UnitsScreen,
};
use crate::Msg;

const UNITS: usize = 0;
const DATETIME: usize = 1;
const LANGUAGE: usize = 2;
const FIRMWARE: usize = 3;
const ABOUT: usize = 4;
const RESET: usize = 5;
const N_ITEMS: usize = 6;

#[derive(Debug, Default)]
pub struct SystemScreen {
    selected: usize,
}

impl SystemScreen {
    pub fn new() -> Self {
        SystemScreen { selected: 0 }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => list::on_step(&mut self.selected, n, N_ITEMS),
            Gesture::Press => match self.selected {
                UNITS => Transition::Push(Screen::Units(UnitsScreen::new())),
                DATETIME => Transition::Push(Screen::DateTime(DateTimeScreen::new())),
                LANGUAGE => Transition::Push(Screen::Language(LanguageScreen::new())),
                FIRMWARE => {
                    // The Firmware page shows the free space, so start the card scan on entry.
                    cx.storage.admit_intent(crate::device_core::storage_info::StorageInfoIntent::RefreshRequested);
                    Transition::Push(Screen::Firmware(FirmwareScreen::new()))
                }
                ABOUT => Transition::Push(Screen::About(AboutScreen::new())),
                RESET => Transition::Push(Screen::Reset(ResetScreen::new())),
                _ => Transition::None,
            },
            Gesture::Back => Transition::Pop,
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let items: [&str; N_ITEMS] = [
            rx.t(Msg::SystemUnits),
            rx.t(Msg::SystemDatetime),
            rx.t(Msg::SystemLanguage),
            rx.t(Msg::SystemUpdate),
            rx.t(Msg::SystemAbout),
            rx.t(Msg::SystemReset),
        ];
        list::nav_list(cv, rx.w, rx.h, rx.t(Msg::SystemTitle), &items, self.selected);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::device_core::storage_info::StorageInfo;
    use crate::screen::test_ctx;
    use crate::{AppState, Mode, Settings};

    fn run(scr: &mut SystemScreen, act: &mut Activity, g: Gesture) -> Transition {
        run_with(scr, act, &mut StorageInfo::new(), g)
    }

    fn run_with(scr: &mut SystemScreen, act: &mut Activity, storage: &mut StorageInfo, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut s = Settings::default();
        let mut cx = Ctx { storage, ..test_ctx(&mut st, act, &mut s) };
        scr.handle(g, &mut cx)
    }

    #[test]
    fn rows_open_their_pages() {
        let mut act = Activity::new(Mode::Idle);
        let mut scr = SystemScreen::new();
        assert!(matches!(run(&mut scr, &mut act, Gesture::Press), Transition::Push(Screen::Units(_))));
        run(&mut scr, &mut act, Gesture::Step(1));
        assert!(matches!(run(&mut scr, &mut act, Gesture::Press), Transition::Push(Screen::DateTime(_))));
        run(&mut scr, &mut act, Gesture::Step(1));
        assert!(matches!(run(&mut scr, &mut act, Gesture::Press), Transition::Push(Screen::Language(_))));
        run(&mut scr, &mut act, Gesture::Step(1));
        assert_eq!(scr.selected, FIRMWARE);
        let mut storage = StorageInfo::new();
        assert!(matches!(
            run_with(&mut scr, &mut act, &mut storage, Gesture::Press),
            Transition::Push(Screen::Firmware(_))
        ));
        assert!(storage.next_effect().is_some(), "opening Firmware arms the free-space measurement");
        run(&mut scr, &mut act, Gesture::Step(1));
        assert_eq!(scr.selected, ABOUT);
        assert!(matches!(run(&mut scr, &mut act, Gesture::Press), Transition::Push(Screen::About(_))));
        run(&mut scr, &mut act, Gesture::Step(1));
        assert!(matches!(run(&mut scr, &mut act, Gesture::Press), Transition::Push(Screen::Reset(_))));
        assert!(matches!(run(&mut scr, &mut act, Gesture::Back), Transition::Pop));
    }
}
