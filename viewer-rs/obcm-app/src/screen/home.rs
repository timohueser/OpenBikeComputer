//! The Home screen — the Idle screensaver and the permanent root of the stack
//! (so Finish / Discard always have somewhere to land via [`Transition::Home`]).
//! A stub for this slice: it draws a title and opens the Menu on `back-hold`;
//! `press` → Route menu and the time/battery content land in later slices.

use embedded_graphics::prelude::{DrawTarget, Point};
use obcm_render::{
    text::{Font, TextAlign},
    Canvas, RenderStats,
};

use crate::input::Gesture;

use super::{palette, Ctx, MenuScreen, Render, RouteMenuScreen, Screen, Transition};

/// The idle home screen. No state yet.
#[derive(Debug, Default)]
pub struct HomeScreen;

impl HomeScreen {
    pub fn new() -> Self {
        HomeScreen
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Press => Transition::Push(Screen::RouteMenu(RouteMenuScreen::new())),
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
        let mut cv = Canvas::new(target, color_fn);
        cv.clear(palette::HUD);
        cv.text("HOME", Point::new(w / 2, h / 2 - 40), Font::Display, TextAlign::Center, palette::AMBER);
        cv.text("press to start", Point::new(w / 2, h / 2 + 8), Font::Label, TextAlign::Center, palette::PARCHMENT);
        RenderStats::default()
    }
}
