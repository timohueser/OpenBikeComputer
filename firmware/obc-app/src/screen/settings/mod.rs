//! The Settings tree. This module owns the list screen ([`SettingsScreen`]) and the drawing kit the
//! settings screens share. Each settings screen has its own file.
//!
//! The Select model has two levels: rotate moves the row cursor, or changes the value of an open
//! field. Press flips a toggle or opens a field. Back steps out of an open field, or climbs one
//! screen up. A long press is only for a guarded action, such as the factory [`reset`].
//!
//! Editing is live: a stepper writes into the shared [`Settings`](crate::Settings), and
//! [`App::apply_gesture`](crate::App::apply_gesture) flags the host to persist the change.

use embedded_graphics::{prelude::Point, primitives::Rectangle};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::Msg;

use super::vocab::list;
use super::vocab::rows::{confirm_row, row_rect};
use super::{palette, Ctx, Render, Screen, Transition};

mod about;
mod add_field;
/// The bike sprites for the ride-start card that has no route.
pub(crate) mod bike_icons;
mod bluetooth;
mod connections;
mod datetime;
mod display;
mod fields;
mod firmware;
mod language;
mod power;
mod reset;
mod ride;
mod sensors;
mod system;
mod units;

pub use about::AboutScreen;
pub use add_field::AddFieldScreen;
pub use bluetooth::BluetoothScreen;
pub use connections::ConnectionsScreen;
pub use datetime::DateTimeScreen;
pub use display::DisplayScreen;
pub use fields::StatFieldsScreen;
pub use firmware::FirmwareScreen;
pub use language::LanguageScreen;
pub use power::PowerScreen;
pub use reset::ResetScreen;
pub use ride::RideScreen;
pub use sensors::{SensorScanScreen, SensorsScreen};
pub use system::SystemScreen;
pub use units::UnitsScreen;

const N_ITEMS: usize = 5;

/// The Settings list. Its rows open the individual settings screens.
#[derive(Debug, Default)]
pub struct SettingsScreen {
    selected: usize,
}

impl SettingsScreen {
    pub fn new() -> Self {
        SettingsScreen { selected: 0 }
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => list::on_step(&mut self.selected, n, N_ITEMS),
            Gesture::Press => match self.selected {
                0 => Transition::Push(Screen::Ride(RideScreen::new())),
                1 => Transition::Push(Screen::Display(DisplayScreen::new())),
                2 => Transition::Push(Screen::Connections(ConnectionsScreen::new())),
                3 => Transition::Push(Screen::Power(PowerScreen::new())),
                _ => Transition::Push(Screen::System(SystemScreen::new())),
            },
            Gesture::Back => Transition::Pop,
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        // The order must match the press arms in `handle`.
        let items: [&str; N_ITEMS] = [
            rx.t(Msg::SettingsRide),
            rx.t(Msg::SettingsDisplay),
            rx.t(Msg::SettingsConnections),
            rx.t(Msg::SettingsPower),
            rx.t(Msg::SettingsSystem),
        ];
        list::nav_list(cv, rx.w, rx.h, rx.t(Msg::SettingsTitle), &items, self.selected);
    }
}

/// The Forget row height. It matches the Route overview Delete row, so the buttons look the same.
pub(super) const FORGET_H: i32 = 38;

/// Back on a page with an editable field. An open field takes the press and `close` runs.
/// If no field is open, Back climbs to the Settings list.
pub(super) fn back_out_of_field(open: bool, close: impl FnOnce()) -> Transition {
    if open {
        close();
        Transition::None
    } else {
        Transition::Pop
    }
}

/// The bottom-anchored guarded Forget row that Bluetooth and Sensors share. It fills warning-red
/// with the hold while the cursor is on it. The caller draws it only when there is something to
/// forget.
pub(super) fn forget_footer(cv: &mut impl Surface, w: i32, h: i32, label: &str, selected: bool, hold: f32) {
    let fy = h - 10 - FORGET_H;
    let row = row_rect(fy, w, FORGET_H);
    confirm_row(cv, row, selected, true, hold, palette::WARNING, 6);
    cv.text_vcentered(label, row.top_left.x + 12, (fy, FORGET_H), Font::Body, TextAlign::Left, palette::INK);
}

/// Draw a row's left-hand label (Body) with an optional muted sub-caption (Label) under it. The
/// caller draws the right-hand control.
pub(super) fn row_label(cv: &mut impl Surface, area: Rectangle, label: &str, sub: Option<&str>) {
    let x = area.top_left.x + 10;
    match sub {
        Some(sub) => {
            cv.text(label, Point::new(x, area.top_left.y + 5), Font::Body, TextAlign::Left, palette::INK);
            cv.text(sub, Point::new(x, area.top_left.y + 30), Font::Label, TextAlign::Left, palette::SUBTEXT);
        }
        None => {
            let (top, h) = (area.top_left.y, area.size.height as i32);
            cv.text_vcentered(label, x, (top, h), Font::Body, TextAlign::Left, palette::INK);
        }
    }
}

/// Draw a stepper field cell holding `text`. An active cell gets an amber fill and arrows, so
/// `cell` must leave about 10 px of clearance for them.
pub(super) fn stepper_field(cv: &mut impl Surface, cell: Rectangle, text: &str, active: bool, font: Font) {
    let cx = cell.top_left.x + cell.size.width as i32 / 2;
    if active {
        cv.round(cell, 4, palette::AMBER);
        let top = cell.top_left.y;
        let bot = cell.top_left.y + cell.size.height as i32;
        cv.triangle(Point::new(cx - 6, top - 3), Point::new(cx + 6, top - 3), Point::new(cx, top - 10), palette::INK);
        cv.triangle(Point::new(cx - 6, bot + 3), Point::new(cx + 6, bot + 3), Point::new(cx, bot + 10), palette::INK);
    }
    cv.text_vcentered(text, cx, (cell.top_left.y, cell.size.height as i32), font, TextAlign::Center, palette::INK);
}

/// Draw a span badge at the right of a row: one square for a one-column field, two for a full-width one.
pub(super) fn span_badge(cv: &mut impl Surface, area: Rectangle, span: u8, color: u16) {
    let cell = 11;
    let gap = 3;
    let cy = area.top_left.y + (area.size.height as i32 - cell) / 2;
    let right = area.top_left.x + area.size.width as i32 - 10;
    // Laid out right-to-left from the row edge.
    for i in 0..span as i32 {
        let x = right - (i + 1) * cell - i * gap;
        cv.round(rect(x, cy, cell, cell), 2, color);
    }
}
