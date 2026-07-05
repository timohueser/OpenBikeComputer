//! The Stats screen — the riding [`Statistics`](crate::screen) page's configuration. **Page cycle**
//! (how fast the grid auto-flips) is a stepper here; **Fields** opens the
//! [`StatFields`](super::StatFieldsScreen) sub-screen for the panel selection + order. The cycle
//! period is kept out of the field list deliberately — mixed among the panels it read as just
//! another draggable row.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{rect, text::Font, Surface};

use crate::input::Gesture;
use crate::screen::{title_frame, Ctx, Render, Screen, Transition, LIST_TOP};
use crate::settings::{STAT_CYCLE_MAX, STAT_CYCLE_MIN};

use super::StatFieldsScreen;

/// Row height — fits a main label + sub-caption with the stepper / chevron on the right. The label
/// is kept short ("Pages") so the big Body glyphs clear the value box.
const ROW_H: i32 = 58;

const PAGE_CYCLE: usize = 0;
const FIELDS: usize = 1;
const ROWS: usize = 2;

/// Step the page-cycle period by `n` detents (1 s each), clamped to the configured bounds.
fn step_cycle(v: u16, n: i32) -> u16 {
    (v as i32 + n).clamp(STAT_CYCLE_MIN as i32, STAT_CYCLE_MAX as i32) as u16
}

/// The Stats screen. `selected` is the highlighted row; `editing_cycle` is set only while the
/// page-cycle stepper is open (the Fields row has no edit sub-mode — it navigates).
#[derive(Debug, Default)]
pub struct StatsScreen {
    selected: usize,
    editing_cycle: bool,
}

impl StatsScreen {
    pub fn new() -> Self {
        StatsScreen::default()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => {
                if self.editing_cycle {
                    cx.settings.stat_cycle_s = step_cycle(cx.settings.stat_cycle_s, n);
                } else {
                    self.selected = crate::screen::list::step_selection(self.selected, n, ROWS);
                }
                Transition::None
            }
            Gesture::Press => match self.selected {
                // The cycle row's single field: press toggles the stepper open/closed.
                PAGE_CYCLE => {
                    self.editing_cycle = !self.editing_cycle;
                    Transition::None
                }
                // Fields → the panel manager.
                _ => Transition::Push(Screen::StatFields(StatFieldsScreen::new())),
            },
            // Back steps out of an open field first, else climbs to the Settings list.
            Gesture::Back => {
                if self.editing_cycle {
                    self.editing_cycle = false;
                    Transition::None
                } else {
                    Transition::Pop
                }
            }
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use crate::screen::palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, "STATS", "");

        // Row 0 — Page cycle (single-line value row with a stepper on the right).
        let r0 = super::row_rect(LIST_TOP + 8, w, ROW_H);
        let editing = self.editing_cycle && self.selected == PAGE_CYCLE;
        super::row_cursor(cv, r0, self.selected == PAGE_CYCLE, editing);
        super::row_label(cv, r0, "Pages", Some("auto-flip"));
        let mut val: heapless::String<8> = heapless::String::new();
        let _ = write!(val, "{} s", rx.settings.stat_cycle_s);
        let (cw, ch) = (76, 32);
        let cell = rect(r0.top_left.x + r0.size.width as i32 - cw - 6, r0.top_left.y + (ROW_H - ch) / 2, cw, ch);
        super::stepper_field(cv, cell, &val, editing, Font::Label);

        // Row 1 — Fields (opens the panel manager).
        let r1 = super::row_rect(LIST_TOP + 8 + ROW_H + 6, w, ROW_H);
        super::row_cursor(cv, r1, self.selected == FIELDS, false);
        super::row_label(cv, r1, "Fields", Some("panels & order"));
        // A right-pointing chevron says "enters a sub-screen".
        let cx0 = r1.top_left.x + r1.size.width as i32 - 22;
        let midy = r1.top_left.y + r1.size.height as i32 / 2;
        cv.triangle(Point::new(cx0, midy - 9), Point::new(cx0, midy + 9), Point::new(cx0 + 11, midy), INK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::{AppState, Mode, Settings};

    fn run(scr: &mut StatsScreen, s: &mut Settings, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let scratch = crate::screen::PoiScratch::new();
        let mut cx = Ctx {
            state: &mut st,
            activity: &mut act,
            settings: s,
            routes: &[],
            rides: &[],
            poi_scratch: &scratch,
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// Press opens the cycle stepper; a turn edits the period live and clamps; press steps back out.
    #[test]
    fn cycle_stepper_edits_live_and_clamps() {
        let mut s = Settings { stat_cycle_s: 5, ..Settings::default() };
        let mut scr = StatsScreen::new();
        run(&mut scr, &mut s, Gesture::Press); // cursor starts on Page cycle → open stepper
        assert!(scr.editing_cycle);
        run(&mut scr, &mut s, Gesture::Turn(2));
        assert_eq!(s.stat_cycle_s, 7);
        run(&mut scr, &mut s, Gesture::Turn(100));
        assert_eq!(s.stat_cycle_s, STAT_CYCLE_MAX, "clamps to the max");
        run(&mut scr, &mut s, Gesture::Turn(-100));
        assert_eq!(s.stat_cycle_s, STAT_CYCLE_MIN, "and to the min");
        run(&mut scr, &mut s, Gesture::Press);
        assert!(!scr.editing_cycle, "press steps back out");
    }

    /// The Fields row pushes the panel manager; Back from the bare screen climbs to Settings.
    #[test]
    fn fields_row_pushes_manager_and_back_pops() {
        let mut s = Settings::default();
        let mut scr = StatsScreen::new();
        run(&mut scr, &mut s, Gesture::Turn(1)); // → Fields row
        assert_eq!(scr.selected, FIELDS);
        assert!(matches!(run(&mut scr, &mut s, Gesture::Press), Transition::Push(Screen::StatFields(_))));
        run(&mut scr, &mut s, Gesture::Turn(-1)); // back to Page cycle row
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop));
    }

    /// Back closes an open stepper before it pops — the staged escape.
    #[test]
    fn back_closes_stepper_first() {
        let mut s = Settings::default();
        let mut scr = StatsScreen::new();
        run(&mut scr, &mut s, Gesture::Press); // open the stepper
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::None));
        assert!(!scr.editing_cycle, "back closed the stepper, not the screen");
    }
}
