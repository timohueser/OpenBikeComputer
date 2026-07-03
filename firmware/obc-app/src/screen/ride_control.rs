//! The Ride control overlay — the pause menu: Resume / Finish / Discard, drawn over the still-
//! visible map.
//!
//! Each option has a `guard` flag: non-guarded (Resume) fire on `press`; guarded, irreversible ones
//! (Finish, Discard) fire only on a completed `hold`, their row filling with a warning bar as the
//! encoder is held (release early → no `Hold` gesture → nothing happens). `back` resumes.

use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::activity::{Mode, TrackAction};
use crate::input::Gesture;

use super::{list, palette, Ctx, MenuItem, Render, Transition};

const ITEMS: [MenuItem; 3] = [
    MenuItem { label: "Resume", guard: false },
    MenuItem { label: "Finish", guard: true },
    MenuItem { label: "Discard", guard: true },
];

const FINISH: usize = 1;
const DISCARD: usize = 2;

/// The pause overlay. State is just the highlighted option.
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
        ITEMS[self.selected].guard
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => list::on_turn(&mut self.selected, n, ITEMS.len()),
            Gesture::Press => {
                // Instant (non-guarded) options only — i.e. Resume.
                if ITEMS[self.selected].guard {
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
            Gesture::BackHold => Transition::None,
        }
    }

    /// End the tracking session: record the log's disposition (Save → GPX / Discard → drop,
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
        let (pw, ph) = (210, 176);
        let (px, py) = (w / 2 - pw / 2, h / 2 - ph / 2);

        // Parchment panel + dark HUD title strip over the map. The strip follows the panel's
        // 8 px top rounding (a square fill would clip the corners); its lower half is squared
        // off so the bottom edge stays flat against the rows.
        cv.round(rect(px, py, pw, ph), 8, PARCHMENT);
        cv.round(rect(px, py, pw, 32), 8, HUD);
        cv.fill(rect(px, py + 16, pw, 16), HUD);
        cv.text_vcentered("PAUSED", w / 2, py, 32, Font::Label, TextAlign::Center, PARCHMENT);

        // Guarded rows fill warning-red — Finish/Discard are irreversible.
        let geo = super::GuardedRowsGeometry {
            x: px + 10,
            w: pw - 20,
            top: py + 40,
            row_h: 38,
            gap: 6,
            label_dx: 12,
            label_dy: 5,
        };
        super::draw_guarded_rows(cv, &ITEMS, self.selected, rx.hold_progress, WARNING, geo);
    }
}
