//! The Ride control overlay — the pause menu: Resume / Finish / Discard, drawn over the still-
//! visible map.
//!
//! Each option has a `guard` flag: non-guarded (Resume) fire on `press`; guarded, irreversible ones
//! (Finish, Discard) fire only on a completed `hold`, their row filling with a warning bar as the
//! encoder is held (release early → no `Hold` gesture → nothing happens). `back` resumes.

use embedded_graphics::prelude::{DrawTarget, Point};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, RenderStats, Surface,
};

use crate::activity::{Mode, TrackAction};
use crate::input::Gesture;

use super::{palette, Ctx, MenuItem, Render, Transition};

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

    /// True if the highlighted option is guarded (needs a hold) — the host fills the confirm ring.
    pub fn selection_is_guarded(&self) -> bool {
        ITEMS[self.selected].guard
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => {
                self.selected = super::step_selection(self.selected, n, ITEMS.len());
                Transition::None
            }
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

    pub fn draw<D, F>(&self, target: &mut D, rx: &mut Render, color_fn: &F) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        use palette::*;
        let (w, h) = (rx.w as i32, rx.h as i32);
        let (pw, ph) = (210, 176);
        let (px, py) = (w / 2 - pw / 2, h / 2 - ph / 2);
        let mut cv = Canvas::new(target, color_fn);

        // Parchment panel + dark HUD title strip over the map.
        cv.round(rect(px, py, pw, ph), 8, PARCHMENT);
        cv.fill(rect(px, py, pw, 32), HUD);
        cv.text("PAUSED", Point::new(w / 2, py + 7), Font::Label, TextAlign::Center, PARCHMENT);

        let (row_h, gap, first) = (38, 6, py + 40);
        for (i, item) in ITEMS.iter().enumerate() {
            let y = first + i as i32 * (row_h + gap);
            let row = rect(px + 10, y, pw - 20, row_h);
            super::confirm_row(&mut cv, row, i == self.selected, item.guard, rx.hold_progress, WARNING, 6);
            cv.text(item.label, Point::new(px + 22, y + 5), Font::Body, TextAlign::Left, INK);
        }
        RenderStats::default()
    }
}
