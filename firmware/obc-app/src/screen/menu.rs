//! The Menu overlay — Routes / Settings, in the "explorer's field map" style: a wood frame, a title
//! strip with an `n / total` counter, big rows with a pointer bullet and an amber highlight, and
//! hairline separators. Routes opens the Route menu, Settings the
//! [`SettingsScreen`](super::SettingsScreen) tree; `back` returns to the caller. (The Shutdown
//! prompt on `back-hold` is a later slice.)

use embedded_graphics::prelude::{DrawTarget, Point};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, RenderStats,
};

use crate::input::Gesture;

use super::{list_frame, palette, Ctx, Render, RouteMenuScreen, Screen, SettingsScreen, Transition, LIST_TOP};

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
            Gesture::Turn(n) => {
                self.selected = super::step_selection(self.selected, n, ITEMS.len());
                Transition::None
            }
            Gesture::Press => match self.selected {
                0 => Transition::Push(Screen::RouteMenu(RouteMenuScreen::new())), // Routes
                _ => Transition::Push(Screen::Settings(SettingsScreen::new())),   // Settings
            },
            Gesture::Back => Transition::Pop, // return to caller (Home or Map)
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

        list_frame(&mut cv, w, h, "MENU", self.selected + 1, ITEMS.len());

        for (i, &name) in ITEMS.iter().enumerate() {
            let y = LIST_TOP + i as i32 * ROW_H;
            let mid = y + (ROW_H - 8) / 2;
            let selected = i == self.selected;

            if selected {
                cv.round(rect(16, y, w - 32, ROW_H - 8), 6, AMBER);
            }
            let bullet = if selected { INK } else { SUBTEXT };
            cv.triangle(Point::new(30, mid - 9), Point::new(30, mid + 9), Point::new(43, mid), bullet);
            cv.text(name, Point::new(54, mid - 14), Font::Body, TextAlign::Left, INK);

            if i + 1 < ITEMS.len() {
                cv.hline(20, y + ROW_H - 4, w - 40, RULE);
            }
        }
        RenderStats::default()
    }
}
