//! The Menu overlay — Routes / Settings, in the "explorer's field map" style: a wood frame, a title
//! strip, big rows with a pointer bullet and an amber highlight, and hairline separators. Routes
//! opens the Route menu, Settings the [`SettingsScreen`](super::SettingsScreen) tree; `back`
//! returns to the caller. (The Shutdown prompt on `back-hold` is a later slice.)

use obc_render::Surface;

use crate::input::Gesture;

use super::list::{self, ListGeometry, Separators};
use super::{Ctx, Render, RouteMenuScreen, Screen, SettingsScreen, Transition, LIST_TOP};

const ITEMS: [&str; 2] = ["Routes", "Settings"];

/// Per-row height — fits a Body-tier row with an amber highlight + padding.
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
        let (w, h) = (rx.w, rx.h);
        let geo = ListGeometry {
            w,
            top: LIST_TOP,
            row_h: ROW_H,
            row_gap: 8,
            side_inset: 16,
            separators: Separators::All,
            visible: list::visible_rows(h, ROW_H),
        };
        list::list_frame(cv, w, h, "MENU", self.selected + 1, ITEMS.len(), geo.visible);
        let first = list::window_start(self.selected, geo.visible, ITEMS.len()) as i32;
        list::draw_rows(cv, geo, ITEMS.len(), self.selected, first, |cv, row| {
            list::nav_row(cv, row.area, ITEMS[row.index], row.selected);
        });
    }
}
