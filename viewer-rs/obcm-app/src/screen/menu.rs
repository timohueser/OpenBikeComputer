//! The Menu overlay — Routes / Settings, in the "explorer's field map" style of
//! the route-list mock (`docs/bikepacking_portrait_screens.html`): a dark wood
//! frame around a panel, a wood title strip with a `n / total` counter, big rows
//! with a pointer bullet and an amber selection highlight, hairline separators,
//! and a control hint. A stub for behavior — `back` returns to the caller; opening
//! an item and the Shutdown prompt land in later slices — but it doubles as the
//! worked example of the framework's drawing surface ([`Canvas`]).

use core::fmt::Write;

use embedded_graphics::prelude::{DrawTarget, Point};
use obcm_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, RenderStats,
};

use crate::input::Gesture;

use super::{palette, Ctx, Render, Transition};

const ITEMS: [&str; 2] = ["Routes", "Settings"];

/// First row's top y, and the per-row height.
const LIST_TOP: i32 = 52;
const ROW_H: i32 = 48;

/// The main menu. State is the highlighted row.
#[derive(Debug, Default)]
pub struct MenuScreen {
    selected: usize,
}

impl MenuScreen {
    pub fn new() -> Self {
        MenuScreen { selected: 0 }
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => {
                let len = ITEMS.len() as i32;
                self.selected = (self.selected as i32 + n).rem_euclid(len) as usize;
                Transition::None
            }
            Gesture::Press => Transition::None,    // open Routes / Settings — later slice
            Gesture::Back => Transition::Pop,      // return to caller (Home or Map)
            Gesture::Hold => Transition::None,
            Gesture::BackHold => Transition::None, // Shutdown prompt — later slice
        }
    }

    pub fn draw<D, F>(&self, target: &mut D, rx: &mut Render, color_fn: &F) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        use palette::*;
        let (w, h) = (rx.w as i32, rx.h as i32);
        let mut cv = Canvas::new(target, color_fn);

        // Wood frame → parchment panel → inset border line.
        cv.clear(HUD);
        cv.round(rect(8, 8, w - 16, h - 16), 6, PARCHMENT);
        cv.round_outline(rect(12, 12, w - 24, h - 24), 4, WOOD_LIGHT);

        // Title strip: "MENU" + an "n / total" counter.
        cv.round(rect(12, 12, w - 24, 30), 4, WOOD);
        cv.text("MENU", Point::new(w / 2, 20), Font::Body, TextAlign::Center, PARCHMENT);
        let mut counter: heapless::String<8> = heapless::String::new();
        let _ = write!(counter, "{} / {}", self.selected + 1, ITEMS.len());
        cv.text(&counter, Point::new(w - 18, 21), Font::Label, TextAlign::Right, PARCHMENT);

        // Rows: one big label each, with a pointer bullet + amber selection.
        for (i, &name) in ITEMS.iter().enumerate() {
            let y = LIST_TOP + i as i32 * ROW_H;
            let mid = y + (ROW_H - 10) / 2;
            let selected = i == self.selected;

            if selected {
                cv.round(rect(16, y, w - 32, ROW_H - 10), 5, AMBER);
            }
            let bullet = if selected { INK } else { SUBTEXT };
            cv.triangle(Point::new(28, mid - 7), Point::new(28, mid + 7), Point::new(39, mid), bullet);
            cv.text(name, Point::new(50, mid - 10), Font::Display, TextAlign::Left, INK);

            if i + 1 < ITEMS.len() {
                cv.hline(20, y + ROW_H - 5, w - 40, RULE);
            }
        }

        // Control hint.
        cv.text("turn move    press open    back", Point::new(w / 2, h - 26), Font::Label, TextAlign::Center, SUBTEXT);
        RenderStats::default()
    }
}
