//! The Weather settings screen (WX11, epic #1185): the scheduled refresh interval —
//! Off / 15 / 30 / 60 / 120 minutes, default 30 — in the standard picker-row anatomy
//! (the Display screen's Idle row). The WX8 due scheduler consumes the persisted value;
//! scheduled requests run only during an active ride, and opening Weather always refreshes
//! urgently regardless of this interval.

use obc_render::{rect, text::Font, Surface};

use crate::input::Gesture;
use crate::Msg;

use super::super::{Ctx, Render, Transition};
use crate::screen::vocab::chrome::title_frame;
use crate::screen::vocab::chrome::LIST_TOP;
use crate::screen::vocab::rows::{row_cursor, row_rect};

/// Row height — the Display screen's picker-row pitch.
const ROW_H: i32 = 58;

/// The Weather settings screen: one picker row (Refresh). State is whether the picker is open.
#[derive(Debug, Default)]
pub struct WeatherSettingsScreen {
    editing: bool,
}

impl WeatherSettingsScreen {
    pub fn new() -> Self {
        WeatherSettingsScreen::default()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => {
                if self.editing {
                    cx.settings.weather_refresh = cx.settings.weather_refresh.stepped(n);
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
        title_frame(cv, w, h, rx.t(Msg::WeatherSettingsTitle), "");

        let row = row_rect(LIST_TOP + 8, w, ROW_H);
        row_cursor(cv, row, true, self.editing);
        super::row_label(cv, row, rx.t(Msg::WeatherSettingsRefresh), Some(rx.t(Msg::WeatherSettingsRefreshSub)));
        let value = rx.settings.weather_refresh.name(rx.settings.language);
        let (cw, ch) = (88, 32);
        let cell = rect(row.top_left.x + row.size.width as i32 - cw - 6, row.top_left.y + (ROW_H - ch) / 2, cw, ch);
        super::stepper_field(cv, cell, value, self.editing, Font::Label);
    }
}

// Deliberately no extra rows: refresh cadence is the one device-side weather setting the epic
// locks; provider/attribution/diagnostics live on the phone (no provider badges on device).

#[cfg(test)]
mod tests {
    use super::super::super::test_ctx;
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::settings::{Settings, WeatherRefresh};
    use crate::AppState;

    fn run(scr: &mut WeatherSettingsScreen, settings: &mut Settings, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut cx = test_ctx(&mut st, &mut act, settings);
        scr.handle(g, &mut cx)
    }

    /// The picker cycle: press opens the field, steps walk Off → 15 → 30 → 60 → 120 with wrap,
    /// Back closes the field first and only then climbs out — the Display screen's exact model.
    #[test]
    fn refresh_picker_cycles_and_backs_out_in_two_steps() {
        let mut s = Settings::default();
        let mut scr = WeatherSettingsScreen::new();
        // Closed: steps change nothing.
        run(&mut scr, &mut s, Gesture::Step(1));
        assert_eq!(s.weather_refresh, WeatherRefresh::Every30, "closed picker leaves the value");
        // Open, step forward twice: 30 → 60 → 120.
        run(&mut scr, &mut s, Gesture::Press);
        run(&mut scr, &mut s, Gesture::Step(1));
        run(&mut scr, &mut s, Gesture::Step(1));
        assert_eq!(s.weather_refresh, WeatherRefresh::Every120);
        // Wrap past the end lands on Off.
        run(&mut scr, &mut s, Gesture::Step(1));
        assert_eq!(s.weather_refresh, WeatherRefresh::Off);
        // First Back closes the field (stays put), second climbs out.
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::None));
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop));
    }
}
