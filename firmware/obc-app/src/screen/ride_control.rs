//! The Paused page — a full screen (no longer the small overlay): the ride-so-far as a stat
//! ledger (ride time / distance / climb, the Route overview's pane-free look) over the pause
//! menu's option rows, Resume / Finish / Discard.
//!
//! Each option has a `guard` flag: non-guarded (Resume) fire on `press`; guarded, irreversible ones
//! (Finish, Discard) fire only on a completed `hold`, their row filling with a warning bar as the
//! Select is held (release early → no `Hold` gesture → nothing happens). `back` resumes.

use core::fmt::Write;

use obc_render::Surface;

use crate::activity::{Mode, TrackAction};
use crate::input::Gesture;
use crate::stat_fields::{fmt_hms, fmt_km};
use crate::Msg;

use super::{ledger_row, list, palette, title_frame, Ctx, MenuItem, Render, RideMenuScreen, Screen, Transition};

/// The ride-so-far ledger: three caption/value rows under the title bar.
const ROWS_TOP: i32 = 50;
const ROW_PITCH: i32 = 42;

/// The option rows: sized so three rows end just above the bottom frame margin.
const OPTIONS_TOP: i32 = 178;
const OPTION_ROW_H: i32 = 38;
const OPTION_GAP: i32 = 8;

/// Per-row guard flags (Finish / Discard are irreversible). Labels are looked up per language at
/// draw time (see [`RideControl::draw`]) — the old `const ITEMS` couldn't stay const.
const GUARDS: [bool; 3] = [false, true, true];

const FINISH: usize = 1;
const DISCARD: usize = 2;

/// The Paused page. State is just the highlighted option.
#[derive(Debug, Default)]
pub struct RideControl {
    selected: usize,
}

impl RideControl {
    pub fn new() -> Self {
        RideControl { selected: 0 }
    }

    /// True if the highlighted option is guarded (needs a hold): its row fills with the live hold
    /// progress in `draw`, so [`App::top_wants_hold_fill`](crate::App::top_wants_hold_fill) reports
    /// a charging hold as worth repainting here.
    pub fn selection_is_guarded(&self) -> bool {
        GUARDS[self.selected.min(GUARDS.len() - 1)]
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => list::on_step(&mut self.selected, n, GUARDS.len()),
            Gesture::Press => {
                // Instant (non-guarded) options only — i.e. Resume.
                if GUARDS[self.selected.min(GUARDS.len() - 1)] {
                    Transition::None
                } else {
                    cx.activity.mode = Mode::Riding;
                    Transition::Pop
                }
            }
            Gesture::Hold => {
                // Confirm guarded options. The recognizer emits `Hold` only when the hold completes,
                // so reaching here *is* the confirmation; releasing early never produces it.
                match self.selected {
                    FINISH => self.end_ride(cx, TrackAction::Save),
                    DISCARD => self.end_ride(cx, TrackAction::Discard),
                    _ => Transition::None,
                }
            }
            Gesture::Back => {
                cx.activity.mode = Mode::Riding; // back = Resume (cancel the pause)
                Transition::Pop
            }
            Gesture::BackHold => Transition::Push(Screen::RideMenu(RideMenuScreen::new())),
        }
    }

    /// End the tracking session: record the log's disposition (Save → the saved ride / Discard → drop,
    /// performed by the host), end the session, go Idle, clear the route, and return Home.
    fn end_ride(&self, cx: &mut Ctx, action: TrackAction) -> Transition {
        cx.activity.request_track(action);
        cx.activity.end_session();
        cx.activity.mode = Mode::Idle;
        cx.activity.active_route = None;
        Transition::Home
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::RideControlTitle), "");

        // The ride so far, in the shared pane-free ledger: what you're about to Finish (or throw
        // away with Discard) is on screen while the option rows are armed below.
        let units = rx.settings.units;
        let act = rx.activity;
        let time = fmt_hms(act.moving_s);
        let dist = fmt_km(units.dist(act.ridden_m / 1000.0));
        let dist_unit = if units.is_imperial() { "mi" } else { "km" };
        let mut climb: heapless::String<8> = heapless::String::new();
        let _ = write!(climb, "{}", units.elev(act.climb_m()) as u32);

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

        // Guarded rows fill warning-red — Finish/Discard are irreversible.
        let geo = super::GuardedRowsGeometry::panel(w, OPTIONS_TOP, OPTION_ROW_H, OPTION_GAP);
        let items = [
            MenuItem { label: rx.t(Msg::RideControlResume), guard: GUARDS[0] },
            MenuItem { label: rx.t(Msg::RideControlFinish), guard: GUARDS[1] },
            MenuItem { label: rx.t(Msg::RideControlDiscard), guard: GUARDS[2] },
        ];
        super::draw_guarded_rows(cv, &items, self.selected, rx.hold_progress, WARNING, geo);
    }
}
