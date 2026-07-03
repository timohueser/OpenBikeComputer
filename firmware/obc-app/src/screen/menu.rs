//! The Menu overlay — Routes / Settings, in the "explorer's field map" style: a wood frame, a title
//! strip, big rows with a pointer bullet and an amber highlight, and hairline separators. Routes
//! opens the Route menu, Settings the [`SettingsScreen`](super::SettingsScreen) tree; `back`
//! returns to the caller. (The Shutdown prompt on `back-hold` is a later slice.)

use obc_render::Surface;

use crate::input::Gesture;

use super::{list, Ctx, Render, RouteMenuScreen, Screen, SettingsScreen, Transition};

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
            Gesture::Turn(n) => list::on_turn(&mut self.selected, n, ITEMS.len()),
            Gesture::Press => match self.selected {
                0 => Transition::Push(Screen::RouteMenu(RouteMenuScreen::new())), // Routes
                _ => Transition::Push(Screen::Settings(SettingsScreen::new())),   // Settings
            },
            Gesture::Back => Transition::Pop, // return to caller (Home or Map)
            Gesture::Hold => Transition::None,
            Gesture::BackHold => Transition::None, // Shutdown prompt — later slice
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        list::nav_list(cv, rx.w, rx.h, "MENU", &ITEMS, self.selected);
    }
}
