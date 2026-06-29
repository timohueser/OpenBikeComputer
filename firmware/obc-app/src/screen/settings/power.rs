//! The Power screen — GPS fix interval + a power-saver toggle. Same two-level model as the rest:
//! highlight a row, press to act. `GPS Fix` is a value row whose stepper opens in place; the
//! interval **step adapts** (1 s up to 10 s, then 5 s) so a long interval is a few detents, not
//! dozens. `Power Saver` is a click-to-flip toggle.

use core::fmt::Write;

use embedded_graphics::prelude::DrawTarget;
use obc_render::{rect, text::Font, Canvas, RenderStats};

use crate::input::Gesture;
use crate::screen::{title_frame, Ctx, Render, Transition, LIST_TOP};
use crate::settings::{FIX_INTERVAL_MAX, FIX_INTERVAL_MIN};

/// Row height — fits a two-line label (Body + sub-caption) plus a stepper field with arrow room.
const ROW_H: i32 = 58;

/// The two rows: the interval value row, then the power-saver toggle.
const GPS_FIX: usize = 0;
const POWER_SAVER: usize = 1;
const ROWS: usize = 2;

/// Step the fix interval by `n` detents with the **adaptive** granularity: 1 s while under 10 s,
/// 5 s at/above it, clamped to the configured bounds. Applied per detent so a multi-detent flick
/// compounds, and the boundary is re-checked each detent so a sweep up reads 1…10, 15, 20, ….
fn step_interval(v: u16, n: i32) -> u16 {
    let dir = n.signum();
    let mut x = v as i32;
    for _ in 0..n.unsigned_abs() {
        let step = if x < 10 { 1 } else { 5 };
        x = (x + dir * step).clamp(FIX_INTERVAL_MIN as i32, FIX_INTERVAL_MAX as i32);
    }
    x as u16
}

/// The Power screen. `selected` is the highlighted row; `editing` is set only while the GPS-fix
/// stepper is open (the toggle has no edit sub-mode).
#[derive(Debug, Default)]
pub struct PowerScreen {
    selected: usize,
    editing: bool,
}

impl PowerScreen {
    pub fn new() -> Self {
        PowerScreen { selected: 0, editing: false }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => {
                if self.editing {
                    cx.settings.fix_interval_s = step_interval(cx.settings.fix_interval_s, n);
                } else {
                    self.selected = crate::screen::step_selection(self.selected, n, ROWS);
                }
                Transition::None
            }
            Gesture::Press => {
                match self.selected {
                    // The interval row's one field: press enters the stepper, press again (off the
                    // single field) steps back out — so press just toggles editing.
                    GPS_FIX => self.editing = !self.editing,
                    POWER_SAVER => cx.settings.power_saver = !cx.settings.power_saver,
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

    pub fn draw<D, F>(&self, target: &mut D, rx: &mut Render, color_fn: &F) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let (w, h) = (rx.w as i32, rx.h as i32);
        let mut cv = Canvas::new(target, color_fn);
        title_frame(&mut cv, w, h, "POWER", "");

        // Row 0 — GPS Fix interval (value row).
        let r0 = super::row_rect(0, LIST_TOP + 8, w, ROW_H);
        let editing = self.editing && self.selected == GPS_FIX;
        super::row_cursor(&mut cv, r0, self.selected == GPS_FIX, editing);
        super::row_label(&mut cv, r0, "GPS Fix", Some("interval"));
        let mut val: heapless::String<8> = heapless::String::new();
        let _ = write!(val, "{} s", rx.settings.fix_interval_s);
        let (cw, ch) = (76, 32);
        let cell = rect(r0.top_left.x + r0.size.width as i32 - cw - 6, r0.top_left.y + (ROW_H - ch) / 2, cw, ch);
        super::stepper_field(&mut cv, cell, &val, editing, Font::Label);

        // Row 1 — Power Saver (toggle).
        let r1 = super::row_rect(1, LIST_TOP + 8 + ROW_H + 6, w, ROW_H);
        super::row_cursor(&mut cv, r1, self.selected == POWER_SAVER, false);
        super::row_label(&mut cv, r1, "Power save", Some("low power"));
        super::toggle_slider(&mut cv, r1, rx.settings.power_saver);

        RenderStats::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::{AppState, Mode, Settings};

    /// The adaptive interval step: 1 s granularity under 10 s, 5 s at/above, clamped at the bounds.
    #[test]
    fn interval_step_is_adaptive_and_clamped() {
        assert_eq!(step_interval(5, 1), 6, "1 s steps below 10 s");
        assert_eq!(step_interval(10, 1), 15, "5 s steps from 10 s up");
        assert_eq!(step_interval(1, -1), 1, "can't go below the minimum");
        assert_eq!(step_interval(FIX_INTERVAL_MAX, 1), FIX_INTERVAL_MAX, "can't exceed the maximum");
        // A multi-detent flick compounds with the boundary re-checked each detent: 1→…→10→15.
        assert_eq!(step_interval(8, 4), 20, "8→9→10→15→20 across the boundary");
    }

    fn run(scr: &mut PowerScreen, s: &mut Settings, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut cx = Ctx { state: &mut st, activity: &mut act, settings: s, routes: &[], now_ms: 0 };
        scr.handle(g, &mut cx)
    }

    /// Press opens the interval stepper, a turn edits it live, and press steps back out; the
    /// toggle row is a click-to-flip.
    #[test]
    fn stepper_and_toggle_behaviour() {
        let mut s = Settings { fix_interval_s: 5, power_saver: false, ..Settings::default() };
        let mut scr = PowerScreen::new();
        run(&mut scr, &mut s, Gesture::Press); // enter the interval stepper
        assert!(scr.editing);
        run(&mut scr, &mut s, Gesture::Turn(1));
        assert_eq!(s.fix_interval_s, 6, "rotating the open stepper edits the interval live");
        run(&mut scr, &mut s, Gesture::Press); // off the single field → out
        assert!(!scr.editing);
        run(&mut scr, &mut s, Gesture::Turn(1)); // → Power Saver row
        assert_eq!(scr.selected, POWER_SAVER);
        run(&mut scr, &mut s, Gesture::Press);
        assert!(s.power_saver, "press flips the toggle");
    }
}
