//! The Ride control overlay — the pause menu: Resume / Finish / Discard.
//!
//! This screen carries the **guarded-action** pattern the brief wants to be
//! reusable: each option has a `guard` flag. Non-guarded options (Resume) fire on
//! `press`; guarded, irreversible ones (Finish, Discard) fire only on a completed
//! `hold`, and their row fills with a warning bar as the encoder is held (release
//! early — no `Hold` gesture — and nothing happens). `back` resumes (cancels the
//! pause). Drawn as an overlay on top of the still-visible map.

use embedded_graphics::prelude::{DrawTarget, Point};
use obcm_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, RenderStats,
};

use crate::activity::{Mode, TrackAction};
use crate::input::Gesture;

use super::{palette, Ctx, Render, Transition};

/// One Ride-control option. `guard` = irreversible → hold-to-confirm.
struct Item {
    label: &'static str,
    guard: bool,
}

const ITEMS: [Item; 3] = [
    Item { label: "Resume", guard: false },
    Item { label: "Finish", guard: true },
    Item { label: "Discard", guard: true },
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

    /// True if the highlighted option is guarded (needs a hold) — the host reads
    /// this to know whether to fill the confirm ring while the encoder is held.
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
                // Activate instant (non-guarded) options only — i.e. Resume.
                if ITEMS[self.selected].guard {
                    Transition::None
                } else {
                    cx.activity.mode = Mode::Riding;
                    Transition::Pop
                }
            }
            Gesture::Hold => {
                // Confirm guarded options only — Finish / Discard. The recognizer
                // emits `Hold` exactly when the hold completes, so reaching here
                // *is* the confirmation; releasing early never produces it.
                match self.selected {
                    FINISH => self.end_ride(cx, TrackAction::Save),
                    DISCARD => self.end_ride(cx, TrackAction::Discard),
                    _ => Transition::None, // Resume isn't guarded — hold does nothing
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

        // The options, each a highlighted row when selected. Guarded rows fill with
        // a warning bar tracking the hold-progress; instant ones get a solid amber.
        let (row_h, gap, first) = (38, 6, py + 40);
        for (i, item) in ITEMS.iter().enumerate() {
            let y = first + i as i32 * (row_h + gap);
            let row = rect(px + 10, y, pw - 20, row_h);
            if i == self.selected {
                if item.guard {
                    cv.round(row, 6, PARCHMENT_SHADE);
                    let fill_w = ((pw - 20) as f32 * rx.hold_progress.clamp(0.0, 1.0)) as i32;
                    if fill_w > 0 {
                        cv.round(rect(px + 10, y, fill_w, row_h), 6, WARNING);
                    }
                } else {
                    cv.round(row, 6, AMBER);
                }
            }
            cv.text(item.label, Point::new(px + 22, y + 5), Font::Body, TextAlign::Left, INK);
        }
        RenderStats::default()
    }
}
