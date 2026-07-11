//! The Display screen — the Map's chrome overlays plus the idle-return timeout. `Clock` and
//! `Scale bar` are click-to-flip toggles that show/hide the Map's `HH:MM` pill and the bottom-left
//! scale bar. `Idle` is a left/right value picker (15 s / 30 s / 1 min / 5 min / Never) — the same
//! [`IdleReturn`](crate::settings::IdleReturn) picker that used to live on the Power page, moved here
//! so all the "how the screen behaves" settings sit together.

use obc_render::{rect, text::Font, Surface};

use crate::input::Gesture;
use crate::screen::{title_frame, Ctx, Render, Transition, LIST_TOP};
use crate::Msg;

/// Row height — fits a two-line label (Body + sub-caption) plus a toggle / value cell with arrow room.
const ROW_H: i32 = 58;

/// The rows: the two Map-overlay toggles, then the idle-return picker.
const CLOCK: usize = 0;
const SCALE_BAR: usize = 1;
const IDLE_RETURN: usize = 2;
const ROWS: usize = 3;

/// The Display screen. `selected` is the highlighted row; `editing` is set only while the
/// idle-return picker is open (the toggles have no edit sub-mode).
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
            Gesture::Turn(n) => {
                if self.editing {
                    // Only the idle-return row has an editable value; a turn walks it in place.
                    if self.selected == IDLE_RETURN {
                        cx.settings.idle_return = cx.settings.idle_return.stepped(n);
                    }
                } else {
                    self.selected = crate::screen::list::step_selection(self.selected, n, ROWS);
                }
                Transition::None
            }
            Gesture::Press => {
                match self.selected {
                    CLOCK => cx.settings.map_clock = !cx.settings.map_clock,
                    SCALE_BAR => cx.settings.map_scale_bar = !cx.settings.map_scale_bar,
                    // The value row: press enters the picker, press again (there's one field) steps
                    // back out — so press just toggles editing.
                    IDLE_RETURN => self.editing = !self.editing,
                    _ => {}
                }
                Transition::None
            }
            // Back steps out of an open field first, else climbs to the Settings list.
            Gesture::Back => {
                if self.editing {
                    self.editing = false;
                    Transition::None
                } else {
                    Transition::Pop
                }
            }
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::DisplayTitle), "");

        // Row 0 — Clock (toggle). Label + sub kept as short as the GPS-fix row's so they clear the
        // right-hand toggle slider.
        let r0 = super::row_rect(LIST_TOP + 8, w, ROW_H);
        super::row_cursor(cv, r0, self.selected == CLOCK, false);
        super::row_label(cv, r0, rx.t(Msg::DisplayClock), Some(rx.t(Msg::DisplayClockSub)));
        super::toggle_slider(cv, r0, rx.settings.map_clock);

        // Row 1 — Scale bar (toggle).
        let r1 = super::row_rect(LIST_TOP + 8 + ROW_H + 6, w, ROW_H);
        super::row_cursor(cv, r1, self.selected == SCALE_BAR, false);
        super::row_label(cv, r1, rx.t(Msg::DisplayScaleBar), Some(rx.t(Msg::DisplayScaleBarSub)));
        super::toggle_slider(cv, r1, rx.settings.map_scale_bar);

        // Row 2 — Idle return (value picker: 15 s / 30 s / 1 min / 5 min / Never).
        let r2 = super::row_rect(LIST_TOP + 8 + 2 * (ROW_H + 6), w, ROW_H);
        let editing = self.editing && self.selected == IDLE_RETURN;
        super::row_cursor(cv, r2, self.selected == IDLE_RETURN, editing);
        super::row_label(cv, r2, rx.t(Msg::DisplayIdle), Some(rx.t(Msg::DisplayIdleSub)));
        let val = rx.settings.idle_return.name(rx.settings.language);
        let (cw, ch) = (76, 32);
        let cell = rect(r2.top_left.x + r2.size.width as i32 - cw - 6, r2.top_left.y + (ROW_H - ch) / 2, cw, ch);
        super::stepper_field(cv, cell, val, editing, Font::Label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::settings::IdleReturn;
    use crate::{AppState, Mode, Settings};

    fn run(scr: &mut DisplayScreen, s: &mut Settings, g: Gesture) -> Transition {
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
            sensor_scan_hits: &[],
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// The two overlay toggles are click-to-flip; the row cursor walks all three rows.
    #[test]
    fn clock_and_scale_bar_toggle() {
        let mut s = Settings::default();
        let mut scr = DisplayScreen::new();
        assert!(s.map_clock && s.map_scale_bar, "both default on");
        run(&mut scr, &mut s, Gesture::Press); // flip Clock
        assert!(!s.map_clock, "press flips the clock toggle");
        run(&mut scr, &mut s, Gesture::Turn(1)); // → Scale bar row
        assert_eq!(scr.selected, SCALE_BAR);
        run(&mut scr, &mut s, Gesture::Press);
        assert!(!s.map_scale_bar, "press flips the scale-bar toggle");
    }

    /// The idle-return row moved here verbatim: press opens its picker, a turn walks the values in
    /// place, and Back closes an open picker before it pops the screen.
    #[test]
    fn idle_return_picker() {
        let mut s = Settings { idle_return: IdleReturn::S30, ..Settings::default() };
        let mut scr = DisplayScreen::new();
        run(&mut scr, &mut s, Gesture::Turn(2)); // Clock → Scale bar → Idle
        assert_eq!(scr.selected, IDLE_RETURN);
        run(&mut scr, &mut s, Gesture::Press); // open the picker
        assert!(scr.editing);
        run(&mut scr, &mut s, Gesture::Turn(1));
        assert_eq!(s.idle_return, IdleReturn::M1, "a turn walks 30 s → 1 min");
        run(&mut scr, &mut s, Gesture::Turn(-1));
        assert_eq!(s.idle_return, IdleReturn::S30, "and back");
        // Back closes the open picker first (no pop), then a second Back pops the screen.
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::None));
        assert!(!scr.editing);
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop));
    }
}
