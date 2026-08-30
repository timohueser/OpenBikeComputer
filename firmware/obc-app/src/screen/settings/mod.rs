//! The Settings tree, in the field-map style of the rest of the UI. This module owns the list
//! screen ([`SettingsScreen`]) and the settings-only drawing kit (the slider toggle, the stepper
//! field, the row label, the guarded Forget footer); the individual screens live one file each.
//! The row rectangle, the row cursor and the value picker are shared vocabulary
//! ([`vocab::rows`](crate::screen::vocab::rows)).
//!
//! The two-level Select model:
//! - **Rotate** moves the amber row cursor; while a field is open it changes that field's value.
//! - **Press** flips a toggle, or enters a value row's stepper (a `▲▼` box marks the live field);
//!   pressing again steps field→field and off the end steps back out.
//! - **Back** steps out of an open field, else climbs one screen up.
//! - **Long-press** is reserved for the one guarded action, the factory [`reset`].
//!
//! Editing is live: a stepper writes straight into the shared [`Settings`](crate::Settings) — no
//! save button, so `back` just exits. [`App::apply_gesture`](crate::App::apply_gesture) notices the
//! change and flags the host to persist it.

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
/// Shared with the route-less ride-start card (T6, #684), which draws the selected profile's hero
/// bike from the same sprites + colours the Bike-type screen uses.
pub(crate) mod bike_icons;
mod bike_type;
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
pub use bike_type::BikeTypeScreen;
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

/// The number of Settings list entries — five themed groups. The row *labels* are looked up
/// per-language at draw time (see [`SettingsScreen::draw`]). Each row opens a group screen: Ride
/// (routing + the riding grid + retention), Display, Connections (Phone + Sensors), Power, and
/// System (Units / Date & Time / Language / Firmware update / About / Reset).
///
/// Weather is **not** among them (#1515 D4b): its one control, the scheduled refresh interval, is a
/// row of the weather screens' own contextual sheet, which is the only home it has.
const N_ITEMS: usize = 5;

/// The Settings list — a nav menu whose rows open the individual settings screens. State is the
/// highlighted row.
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
            Gesture::Back => Transition::Pop, // climb back to the main Menu
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        // Built per-frame from the catalog (the old `const ITEMS` couldn't stay const once the labels
        // are language-dependent); the order matches the `handle` press arms above.
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

// The shared kit — the reusable parts behind every settings screen.

/// The Forget row's height + bottom anchor — the Route overview Delete row's geometry family
/// (38 px tall, the standard 10 px above the card bottom), so the button faces all match.
pub(super) const FORGET_H: i32 = 38;

/// **Back** on a settings page that has an editable field: an open field takes the press — `close`
/// runs and the page stays put — otherwise Back climbs to the Settings list. That's the two levels
/// of "inside" (row cursor, open field) unwinding one press at a time, identically on Display,
/// Power, Ride and Date & time.
pub(super) fn back_out_of_field(open: bool, close: impl FnOnce()) -> Transition {
    if open {
        close();
        Transition::None
    } else {
        Transition::Pop
    }
}

/// The bottom-anchored guarded **Forget** row shared by Bluetooth and Sensors — the Pause-menu
/// guarded-row treatment (owner review round 3: the round-2 focus outline is retired everywhere):
/// a plain left-aligned Body label while unselected, the shaded base + warning-red hold fill only
/// while the cursor is on it, exactly the `ride_control` family's selected-guarded face at the
/// delete rows' bottom anchor. Both pages draw it only while there *is* something to forget (the
/// round-1 only-when-possible grammar), so the caller owns that condition.
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

/// Draw a stepper field cell holding `text`. Inactive: just the text, no background. Active (the
/// live field): an amber fill plus up/down triangles. `cell` must leave ~10 px clearance for the arrows.
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

/// Draw a span badge at the right of a row: one small square for a one-column field, two for a
/// full-width one — the "how big is this tile" cue shared by the Stat Fields list and Add Field picker.
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
