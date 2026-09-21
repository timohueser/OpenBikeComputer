//! The Paused page: the ride so far as a stat ledger, over the Resume, Finish and Discard rows.
//!
//! Resume is not guarded and fires on a press. Finish and Discard are irreversible, so they fire
//! only on a completed hold, and their row fills as the rider holds Select. Back resumes.

use core::fmt::Write;

use obc_render::Surface;

use super::vocab::fmt::{distance_figure, duration_hms};
use crate::activity::Mode;
use crate::input::Gesture;
use crate::Msg;
use crate::RecorderIntent;

use super::vocab::chrome::title_frame;
use super::vocab::list;
use super::vocab::rows::{draw_guarded_rows, ledger_row, GuardedRowsGeometry, MenuItem};
use super::{palette, Ctx, Render, Transition};

/// The ride-so-far ledger: three caption/value rows under the title bar.
const ROWS_TOP: i32 = 50;
const ROW_PITCH: i32 = 42;

/// The option rows: sized so three rows end just above the bottom frame margin.
const OPTIONS_TOP: i32 = 178;
const OPTION_ROW_H: i32 = 38;
const OPTION_GAP: i32 = 8;

/// Per-row guard flags. Finish and Discard are irreversible.
const GUARDS: [bool; 3] = [false, true, true];

const FINISH: usize = 1;
const DISCARD: usize = 2;

#[derive(Debug, Default)]
pub struct RideControl {
    selected: usize,
}

impl RideControl {
    pub fn new() -> Self {
        RideControl { selected: 0 }
    }

    /// True when the highlighted row fills for a hold, which makes the app repaint that fill.
    pub fn selection_is_guarded(&self) -> bool {
        GUARDS[self.selected.min(GUARDS.len() - 1)]
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => list::on_step(&mut self.selected, n, GUARDS.len()),
            Gesture::Press => {
                if GUARDS[self.selected.min(GUARDS.len() - 1)] {
                    Transition::None
                } else {
                    cx.activity.mode = Mode::Riding;
                    Transition::Pop
                }
            }
            Gesture::Hold => {
                // The recognizer emits `Hold` only for a completed hold, so this is the
                // confirmation of a guarded row.
                match self.selected {
                    FINISH => self.end_ride(cx, RecorderIntent::Save),
                    DISCARD => self.end_ride(cx, RecorderIntent::Discard),
                    _ => Transition::None,
                }
            }
            Gesture::Back => {
                cx.activity.mode = Mode::Riding; // Back resumes the ride
                Transition::Pop
            }
            // Back-hold is the global escape, resolved above screen dispatch.
            Gesture::BackHold => Transition::None,
        }
    }

    /// Close the ride: name the disposition to Recorder, go idle, clear the route, and return
    /// home. The session does not end here. Recorder closes it when the store confirms the close,
    /// so a finalize that fails leaves a ride the rider can still finish.
    fn end_ride(&self, cx: &mut Ctx, intent: RecorderIntent) -> Transition {
        cx.recorder.request(intent);
        cx.activity.mode = Mode::Idle;
        cx.navigator.set_active_route(None);
        Transition::Home
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::RideControlTitle), "");

        let units = rx.settings.units;
        let ride = rx.recorder;
        let time = duration_hms(ride.moving_s());
        let dist = distance_figure(units.dist(ride.ridden_m() / 1000.0));
        let dist_unit = if units.is_imperial() { "mi" } else { "km" };
        let mut climb: heapless::String<8> = heapless::String::new();
        let _ = write!(climb, "{}", units.elev(ride.climb_m()) as u32);

        let rows: [(&str, &str, &str, Option<bool>); 3] = [
            (rx.t(Msg::RideControlRideTime), &time, "", None),
            (rx.t(Msg::RideControlDistance), &dist, dist_unit, None),
            (rx.t(Msg::RideControlClimb), &climb, units.elev_label(), Some(true)),
        ];
        for (i, (caption, value, unit, arrow)) in rows.iter().enumerate() {
            let y = ROWS_TOP + i as i32 * ROW_PITCH;
            ledger_row(cv, w, y, caption, value, unit, *arrow);
            if i + 1 < rows.len() {
                cv.hline(16, y + ROW_PITCH - 4, w - 32, RULE);
            }
        }

        let geo = GuardedRowsGeometry::panel(w, OPTIONS_TOP, OPTION_ROW_H, OPTION_GAP);
        let items = [
            MenuItem { label: rx.t(Msg::RideControlResume), guard: GUARDS[0] },
            MenuItem { label: rx.t(Msg::RideControlFinish), guard: GUARDS[1] },
            MenuItem { label: rx.t(Msg::RideControlDiscard), guard: GUARDS[2] },
        ];
        draw_guarded_rows(cv, &items, self.selected, rx.hold_progress, WARNING, geo);
    }
}
