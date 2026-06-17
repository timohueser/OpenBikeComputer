//! The Menu overlay — Routes / Settings. A stub for this slice: it scrolls and
//! draws, and `back` returns to its caller (proving overlay return-to-caller from
//! either Home or Map), but opening an item and the Shutdown prompt (`back-hold`)
//! land in later slices.

use embedded_graphics::{
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
};
use obcm_render::{
    text::{draw_text, Font, TextAlign},
    RenderStats,
};

use crate::input::Gesture;

use super::{palette, Ctx, Render, Transition};

const ITEMS: [&str; 2] = ["Routes", "Settings"];

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
        let w = rx.w as i32;
        // Wood backdrop + parchment body + dark HUD title strip (the menu chrome).
        let _ = target.clear(color_fn(palette::WOOD));
        let _ = Rectangle::new(Point::new(6, 6), Size::new(rx.w as u32 - 12, rx.h as u32 - 12))
            .into_styled(PrimitiveStyle::with_fill(color_fn(palette::PARCHMENT)))
            .draw(target);
        let _ = Rectangle::new(Point::new(6, 6), Size::new(rx.w as u32 - 12, 24))
            .into_styled(PrimitiveStyle::with_fill(color_fn(palette::HUD)))
            .draw(target);
        draw_text(target, "MENU", Point::new(w / 2, 12), Font::Label, TextAlign::Center, color_fn(palette::PARCHMENT));

        let mut y = 44;
        for (i, label) in ITEMS.iter().enumerate() {
            if i == self.selected {
                let _ = Rectangle::new(Point::new(12, y - 3), Size::new(rx.w as u32 - 24, 20))
                    .into_styled(PrimitiveStyle::with_fill(color_fn(palette::AMBER)))
                    .draw(target);
            }
            draw_text(target, label, Point::new(20, y), Font::Body, TextAlign::Left, color_fn(palette::INK));
            y += 26;
        }
        RenderStats::default()
    }
}
