//! The Display screen — the idle-return timeout, and nothing else. `Idle` is a left/right value
//! picker (15 s / 30 s / 1 min / 5 min / Never) governing when the **whole UI** returns to Home or
//! the Map, which is a device-global behaviour rather than map chrome, so it stays central.
//!
//! The Map's three chrome switches — the `HH:MM` pill, the scale bar and the terrain layer — left
//! this page in #1515 D4c. They are rows of the map's own contextual sheet now
//! ([`MAP_DISPLAY`](crate::screen::context_drawer::MAP_DISPLAY)), which is the only home they have:
//! a switch for what the Map draws belongs on the Map, not two levels inside a settings tree the
//! rider can only reach by leaving it.

use obc_render::{rect, text::Font, Surface};

use crate::input::Gesture;
use crate::screen::vocab::chrome::{title_frame, LIST_TOP};
use crate::screen::vocab::rows::{row_cursor, row_rect};
use crate::screen::{Ctx, Render, Transition};
use crate::Msg;

/// Row height — fits a two-line label (Body + sub-caption) plus a value cell with arrow room.
const ROW_H: i32 = 58;

/// The rows: the idle-return picker, alone.
const IDLE_RETURN: usize = 0;
const ROWS: usize = 1;

/// The Display screen. `selected` is the highlighted row; `editing` is set only while the
/// idle-return picker is open.
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
                    // Only the idle-return row has an editable value; a turn walks it in place.
                    if self.selected == IDLE_RETURN {
                        cx.settings.idle_return = cx.settings.idle_return.stepped(n);
                    }
                } else {
                    self.selected = crate::screen::vocab::list::step_selection(self.selected, n, ROWS);
                }
                Transition::None
            }
            // The value row: press enters the picker, press again (there's one field) steps back
            // out — so press just toggles editing.
            Gesture::Press => {
                self.editing = !self.editing;
                Transition::None
            }
            // Back steps out of an open field first, else climbs to the Settings list.
            Gesture::Back => super::back_out_of_field(self.editing, || self.editing = false),
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::DisplayTitle), "");

        // Row 0 — Idle return (value picker: 15 s / 30 s / 1 min / 5 min / Never).
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

    /// The screen's one row: press opens its picker, a turn walks the values in place, and Back
    /// closes an open picker before it pops the screen.
    #[test]
    fn idle_return_picker() {
        let mut s = Settings { idle_return: IdleReturn::S30, ..Settings::default() };
        let mut scr = DisplayScreen::new();
        assert_eq!(scr.selected, IDLE_RETURN, "the picker is the screen's first and only row");
        run(&mut scr, &mut s, Gesture::Press); // open the picker
        assert!(scr.editing);
        run(&mut scr, &mut s, Gesture::Step(1));
        assert_eq!(s.idle_return, IdleReturn::M1, "a step walks 30 s → 1 min");
        run(&mut scr, &mut s, Gesture::Step(-1));
        assert_eq!(s.idle_return, IdleReturn::S30, "and back");
        // Back closes the open picker first (no pop), then a second Back pops the screen.
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::None));
        assert!(!scr.editing);
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop));
    }
}
