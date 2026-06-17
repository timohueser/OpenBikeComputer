//! The Home screen — the Idle screensaver and the permanent root of the stack
//! (so Finish / Discard always have somewhere to land via [`Transition::Home`]).
//! A stub for this slice: it draws a title and opens the Menu on `back-hold`;
//! `press` → Route menu and the time/battery content land in later slices.

use embedded_graphics::prelude::*;
use obcm_render::{
    text::{draw_text, Font, TextAlign},
    RenderStats,
};

use crate::input::Gesture;

use super::{palette, Ctx, MenuScreen, Render, Screen, Transition};

/// The idle home screen. No state yet.
#[derive(Debug, Default)]
pub struct HomeScreen;

impl HomeScreen {
    pub fn new() -> Self {
        HomeScreen
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Press => Transition::None, // → Route menu, later slice
            Gesture::BackHold => Transition::Push(Screen::Menu(MenuScreen::new())),
            _ => Transition::None,
        }
    }

    pub fn draw<D, F>(&self, target: &mut D, rx: &mut Render, color_fn: &F) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let (w, h) = (rx.w as i32, rx.h as i32);
        let _ = target.clear(color_fn(palette::HUD));
        draw_text(target, "HOME", Point::new(w / 2, h / 2 - 14), Font::Display, TextAlign::Center, color_fn(palette::AMBER));
        draw_text(
            target,
            "press to start",
            Point::new(w / 2, h / 2 + 12),
            Font::Label,
            TextAlign::Center,
            color_fn(palette::PARCHMENT),
        );
        RenderStats::default()
    }
}
