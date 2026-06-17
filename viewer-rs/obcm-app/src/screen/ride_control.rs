//! The Ride control overlay — the pause menu: Resume / Finish / Discard.
//!
//! This screen carries the **guarded-action** pattern the brief wants to be
//! reusable: each option has a `guard` flag. Non-guarded options (Resume) fire on
//! `press`; guarded, irreversible ones (Finish, Discard) fire only on a completed
//! `hold`, and their row fills with a warning bar as the encoder is held (release
//! early — no `Hold` gesture — and nothing happens). `back` resumes (cancels the
//! pause). Drawn as an overlay on top of the still-visible map.

use embedded_graphics::{
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
};
use obcm_render::{
    text::{draw_text, Font, TextAlign},
    RenderStats,
};

use crate::activity::Mode;
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
                let len = ITEMS.len() as i32;
                self.selected = (self.selected as i32 + n).rem_euclid(len) as usize;
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
                if ITEMS[self.selected].guard {
                    cx.activity.mode = Mode::Idle; // Finish saves / Discard deletes (stub) → clear → Home
                    Transition::Home
                } else {
                    Transition::None
                }
            }
            Gesture::Back => {
                cx.activity.mode = Mode::Riding; // back = Resume (cancel the pause)
                Transition::Pop
            }
            Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw<D, F>(&self, target: &mut D, rx: &mut Render, color_fn: &F) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let (w, h) = (rx.w as i32, rx.h as i32);
        let (pw, ph) = (180, 132);
        let origin = Point::new(w / 2 - pw / 2, h / 2 - ph / 2);

        // Parchment panel + dark HUD title strip over the map.
        let _ = Rectangle::new(origin, Size::new(pw as u32, ph as u32))
            .into_styled(PrimitiveStyle::with_fill(color_fn(palette::PARCHMENT)))
            .draw(target);
        let _ = Rectangle::new(origin, Size::new(pw as u32, 22))
            .into_styled(PrimitiveStyle::with_fill(color_fn(palette::HUD)))
            .draw(target);
        draw_text(
            target,
            "PAUSED",
            Point::new(w / 2, origin.y + 6),
            Font::Label,
            TextAlign::Center,
            color_fn(palette::PARCHMENT),
        );

        // The options, each a highlighted row when selected.
        let mut y = origin.y + 34;
        for (i, item) in ITEMS.iter().enumerate() {
            if i == self.selected {
                let row = Rectangle::new(Point::new(origin.x + 6, y - 2), Size::new(pw as u32 - 12, 18));
                if item.guard {
                    // Dim base, filled by hold-progress in warning red.
                    let _ = row
                        .into_styled(PrimitiveStyle::with_fill(color_fn(palette::PARCHMENT_SHADE)))
                        .draw(target);
                    let fill_w = ((pw as u32 - 12) as f32 * rx.hold_progress.clamp(0.0, 1.0)) as u32;
                    if fill_w > 0 {
                        let _ = Rectangle::new(row.top_left, Size::new(fill_w, 18))
                            .into_styled(PrimitiveStyle::with_fill(color_fn(palette::WARNING)))
                            .draw(target);
                    }
                } else {
                    let _ = row
                        .into_styled(PrimitiveStyle::with_fill(color_fn(palette::AMBER)))
                        .draw(target);
                }
            }
            draw_text(
                target,
                item.label,
                Point::new(origin.x + 14, y),
                Font::Body,
                TextAlign::Left,
                color_fn(palette::INK),
            );
            y += 22;
        }
        RenderStats::default()
    }
}
