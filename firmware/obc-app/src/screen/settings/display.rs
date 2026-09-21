//! The Display screen: the idle-return timeout, and nothing else. The timeout controls when the
//! whole UI returns to Home or the Map, so it is a device setting. The switches for what the Map
//! draws are on the map's own contextual sheet.

use obc_render::{rect, text::Font, Surface};

use crate::input::Gesture;
use crate::screen::vocab::chrome::{title_frame, LIST_TOP};
use crate::screen::vocab::rows::{row_cursor, row_rect};
use crate::screen::{Ctx, Render, Transition};
use crate::Msg;

/// Row height. It fits a two-line label and a value cell with room for the arrows.
const ROW_H: i32 = 58;

const IDLE_RETURN: usize = 0;
const ROWS: usize = 1;

/// `editing` is set only while the idle-return picker is open.
#[derive(Debug, Default)]
pub struct DisplayScreen {
    selected: usize,
    editing: bool,
}

impl DisplayScreen {
    pub fn new() -> Self {
        DisplayScreen::default()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => {
                if self.editing {
                    if self.selected == IDLE_RETURN {
                        cx.settings.idle_return = cx.settings.idle_return.stepped(n);
                    }
                } else {
                    self.selected = crate::screen::vocab::list::step_selection(self.selected, n, ROWS);
                }
                Transition::None
            }
            Gesture::Press => {
                self.editing = !self.editing;
                Transition::None
            }
            Gesture::Back => super::back_out_of_field(self.editing, || self.editing = false),
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::DisplayTitle), "");

        let r0 = row_rect(LIST_TOP + 8, w, ROW_H);
        let editing = self.editing && self.selected == IDLE_RETURN;
        row_cursor(cv, r0, self.selected == IDLE_RETURN, editing);
        super::row_label(cv, r0, rx.t(Msg::DisplayIdle), Some(rx.t(Msg::DisplayIdleSub)));
        let val = rx.settings.idle_return.name(rx.settings.language);
        let (cw, ch) = (76, 32);
        let cell = rect(r0.top_left.x + r0.size.width as i32 - cw - 6, r0.top_left.y + (ROW_H - ch) / 2, cw, ch);
        super::stepper_field(cv, cell, val, editing, Font::Label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::screen::test_ctx;
    use crate::settings::IdleReturn;
    use crate::{AppState, Mode, Settings};

    fn run(scr: &mut DisplayScreen, s: &mut Settings, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut cx = test_ctx(&mut st, &mut act, s);
        scr.handle(g, &mut cx)
    }

    #[test]
    fn idle_return_picker() {
        let mut s = Settings { idle_return: IdleReturn::S30, ..Settings::default() };
        let mut scr = DisplayScreen::new();
        assert_eq!(scr.selected, IDLE_RETURN, "the picker is the screen's first and only row");
        run(&mut scr, &mut s, Gesture::Press);
        assert!(scr.editing);
        run(&mut scr, &mut s, Gesture::Step(1));
        assert_eq!(s.idle_return, IdleReturn::M1, "a step walks 30 s → 1 min");
        run(&mut scr, &mut s, Gesture::Step(-1));
        assert_eq!(s.idle_return, IdleReturn::S30, "and back");
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::None));
        assert!(!scr.editing);
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop));
    }
}
