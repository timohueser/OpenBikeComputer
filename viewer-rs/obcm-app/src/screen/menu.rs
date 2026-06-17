//! The Menu overlay — Routes / Settings, in the "explorer's field map" style of
//! the route-list mock (`docs/bikepacking_portrait_screens.html`): a dark wood
//! frame around a parchment panel, a wood title strip with a `n / total` counter,
//! two-line rows with a pointer bullet and an amber selection highlight, hairline
//! separators, and a control hint. A stub for behavior — `back` returns to the
//! caller; opening an item and the Shutdown prompt land in later slices — but it
//! doubles as the worked example of what the framework's drawing primitives can do.

use core::fmt::Write;

use embedded_graphics::{
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle, RoundedRectangle, Triangle},
};
use obcm_render::{
    text::{draw_text, Font, TextAlign},
    RenderStats,
};

use crate::input::Gesture;

use super::{palette, Ctx, Render, Transition};

/// A menu entry: a name and a one-line descriptor (the second row line).
struct Item {
    name: &'static str,
    desc: &'static str,
}

const ITEMS: [Item; 2] = [
    Item { name: "Routes", desc: "load a route" },
    Item { name: "Settings", desc: "device options" },
];

/// First row's top y, and the per-row height.
const LIST_TOP: i32 = 50;
const ROW_H: i32 = 52;

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
        let (w, h) = (rx.w as i32, rx.h as i32);

        // --- Wood frame → parchment panel → inset border. ---
        let _ = target.clear(color_fn(palette::HUD));
        let _ = RoundedRectangle::with_equal_corners(
            Rectangle::new(Point::new(8, 8), Size::new((w - 16) as u32, (h - 16) as u32)),
            Size::new(6, 6),
        )
        .into_styled(PrimitiveStyle::with_fill(color_fn(palette::PARCHMENT)))
        .draw(target);
        let _ = RoundedRectangle::with_equal_corners(
            Rectangle::new(Point::new(12, 12), Size::new((w - 24) as u32, (h - 24) as u32)),
            Size::new(4, 4),
        )
        .into_styled(PrimitiveStyle::with_stroke(color_fn(palette::WOOD_LIGHT), 1))
        .draw(target);

        // --- Title strip: "MENU" + an "n / total" counter. ---
        let _ = RoundedRectangle::with_equal_corners(
            Rectangle::new(Point::new(12, 12), Size::new((w - 24) as u32, 26)),
            Size::new(4, 4),
        )
        .into_styled(PrimitiveStyle::with_fill(color_fn(palette::WOOD)))
        .draw(target);
        draw_text(target, "MENU", Point::new(w / 2, 18), Font::Label, TextAlign::Center, color_fn(palette::PARCHMENT));
        let mut counter: heapless::String<8> = heapless::String::new();
        let _ = write!(counter, "{} / {}", self.selected + 1, ITEMS.len());
        draw_text(target, &counter, Point::new(w - 18, 18), Font::Label, TextAlign::Right, color_fn(palette::PARCHMENT));

        // --- Rows. ---
        for (i, item) in ITEMS.iter().enumerate() {
            let y = LIST_TOP + i as i32 * ROW_H;
            let selected = i == self.selected;
            let mid = y + (ROW_H - 8) / 2;

            if selected {
                let _ = RoundedRectangle::with_equal_corners(
                    Rectangle::new(Point::new(16, y), Size::new((w - 32) as u32, (ROW_H - 8) as u32)),
                    Size::new(5, 5),
                )
                .into_styled(PrimitiveStyle::with_fill(color_fn(palette::AMBER)))
                .draw(target);
            }

            // Pointer bullet (filled triangle) — ink when selected, muted otherwise.
            let bullet = if selected { palette::INK } else { palette::SUBTEXT };
            let _ = Triangle::new(
                Point::new(26, mid - 6),
                Point::new(26, mid + 6),
                Point::new(35, mid),
            )
            .into_styled(PrimitiveStyle::with_fill(color_fn(bullet)))
            .draw(target);

            draw_text(target, item.name, Point::new(48, y + 7), Font::Body, TextAlign::Left, color_fn(palette::INK));
            let desc = if selected { palette::INK } else { palette::SUBTEXT };
            draw_text(target, item.desc, Point::new(48, y + 26), Font::Label, TextAlign::Left, color_fn(desc));

            // Hairline separator between rows.
            if i + 1 < ITEMS.len() {
                let _ = Rectangle::new(Point::new(20, y + ROW_H - 4), Size::new((w - 40) as u32, 1))
                    .into_styled(PrimitiveStyle::with_fill(color_fn(palette::RULE)))
                    .draw(target);
            }
        }

        // --- Control hint. ---
        draw_text(
            target,
            "turn move   press open   back",
            Point::new(w / 2, h - 24),
            Font::Label,
            TextAlign::Center,
            color_fn(palette::SUBTEXT),
        );
        RenderStats::default()
    }
}
