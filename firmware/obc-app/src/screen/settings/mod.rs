//! The Settings tree, in the field-map style of the rest of the UI. This module owns the list
//! screen ([`SettingsScreen`]) and the reusable drawing kit every settings screen shares (the slider
//! toggle, the value/stepper field, the row cursor); the individual screens live one file each.
//!
//! The two-level encoder model:
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

use super::list;
use super::{palette, Ctx, Render, Screen, Transition};

/// Re-exported for the settings screens (the kit is their one-stop `super::`), so a sub-screen
/// like Add field never reaches for `super::super::`.
pub(super) use super::empty_state;

mod add_field;
/// Shared with the route-less ride-start card (T6, #684), which draws the selected profile's hero
/// bike from the same sprites + colours the Bike-type screen uses.
pub(crate) mod bike_icons;
mod bike_type;
mod bluetooth;
mod datetime;
mod display;
mod fields;
mod language;
mod power;
mod reset;
mod sensors;
mod stats;
mod system;
mod units;

pub use add_field::AddFieldScreen;
pub use bike_type::BikeTypeScreen;
pub use bluetooth::BluetoothScreen;
pub use datetime::DateTimeScreen;
pub use display::DisplayScreen;
pub use fields::StatFieldsScreen;
pub use language::LanguageScreen;
pub use power::PowerScreen;
pub use reset::ResetScreen;
pub use sensors::{SensorScanScreen, SensorsScreen};
pub use stats::StatsScreen;
pub use system::SystemScreen;
pub use units::UnitsScreen;

/// The number of Settings list entries. The row *labels* are looked up per-language at draw time
/// (see [`SettingsScreen::draw`]); Sensors (BLE sensors, epic #707) sits just under Bluetooth, and
/// System (the firmware-update door, epic #615 S5) just above the terminal (destructive) Reset row.
const N_ITEMS: usize = 11;

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

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => list::on_turn(&mut self.selected, n, N_ITEMS),
            Gesture::Press => match self.selected {
                0 => Transition::Push(Screen::DateTime(DateTimeScreen::new())),
                1 => Transition::Push(Screen::Units(UnitsScreen::new())),
                2 => Transition::Push(Screen::BikeType(BikeTypeScreen::new())),
                3 => Transition::Push(Screen::Stats(StatsScreen::new())),
                4 => Transition::Push(Screen::Display(DisplayScreen::new())),
                5 => Transition::Push(Screen::Power(PowerScreen::new())),
                6 => Transition::Push(Screen::Bluetooth(BluetoothScreen::new())),
                7 => Transition::Push(Screen::Sensors(SensorsScreen::new())),
                8 => Transition::Push(Screen::Language(LanguageScreen::new())),
                9 => {
                    // Opening System triggers the one-shot card-free scan (T8 item 6): the host runs
                    // the FAT free-cluster scan once on entry and answers via `App::set_card_free`.
                    cx.activity.request_card_scan();
                    Transition::Push(Screen::System(SystemScreen::new()))
                }
                _ => Transition::Push(Screen::Reset(ResetScreen::new())),
            },
            Gesture::Back => Transition::Pop, // climb back to the main Menu
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        // Built per-frame from the catalog (the old `const ITEMS` couldn't stay const once the labels
        // are language-dependent); the order matches the `handle` press arms above.
        let items: [&str; N_ITEMS] = [
            rx.t(Msg::SettingsDatetime),
            rx.t(Msg::SettingsUnits),
            rx.t(Msg::SettingsBikeType),
            rx.t(Msg::SettingsStats),
            rx.t(Msg::SettingsDisplay),
            rx.t(Msg::SettingsPower),
            rx.t(Msg::SettingsBluetooth),
            rx.t(Msg::SettingsSensors),
            rx.t(Msg::SettingsLanguage),
            rx.t(Msg::SettingsSystem),
            rx.t(Msg::SettingsReset),
        ];
        list::nav_list(cv, rx.w, rx.h, rx.t(Msg::SettingsTitle), &items, self.selected);
    }
}

// The shared kit — the reusable parts behind every settings screen.

/// Left inset of every settings row (clears the framed outline).
pub(super) const ROW_X: i32 = 14;

/// The full-width settings-row rectangle at `y` of height `h`.
pub(super) fn row_rect(y: i32, w: i32, h: i32) -> Rectangle {
    rect(ROW_X, y, w - 2 * ROW_X, h)
}

/// Paint a row's amber row-focus cursor. A no-op while editing (the field's `▲▼` box is the cursor
/// then) or when unselected, so the two focus levels never both light up.
pub(super) fn row_cursor(cv: &mut impl Surface, area: Rectangle, selected: bool, editing: bool) {
    if selected && !editing {
        cv.round(area, 6, palette::AMBER);
    }
}

/// One centred value row flanked by ◄ ► triangles — the "rotate to switch" picker row shared by the
/// single-row settings screens ([`Units`](units::UnitsScreen), [`Language`](language::LanguageScreen)).
/// Always drawn as the cursor (these screens have one row, always focused). Returns the row rectangle
/// so a caller can lay out further content beneath it (Units' consequence-preview rows). `y` is the
/// row's top; the row is a fixed 50 px tall.
pub(super) fn value_row_with_arrows(cv: &mut impl Surface, y: i32, w: i32, text: &str) -> Rectangle {
    let area = row_rect(y, w, 50);
    row_cursor(cv, area, true, false);
    let midy = area.top_left.y + area.size.height as i32 / 2;
    cv.text_vcentered(text, w / 2, (area.top_left.y, 50), Font::Body, TextAlign::Center, palette::INK);
    // ◄ and ► as filled triangles, inset from the row edges.
    let ax = area.top_left.x + 18;
    cv.triangle(Point::new(ax, midy - 9), Point::new(ax, midy + 9), Point::new(ax - 11, midy), palette::INK);
    let bx = area.top_left.x + area.size.width as i32 - 18;
    cv.triangle(Point::new(bx, midy - 9), Point::new(bx, midy + 9), Point::new(bx + 11, midy), palette::INK);
    area
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

/// Draw a slider toggle at the right of `area` — a white knob sliding left (off) / right (on), the
/// track dark for off and green for on. The knob position and track colour carry the state.
pub(super) fn toggle_slider(cv: &mut impl Surface, area: Rectangle, on: bool) {
    let (tw, th) = (50, 28);
    let tx = area.top_left.x + area.size.width as i32 - tw - 4;
    let ty = area.top_left.y + (area.size.height as i32 - th) / 2;
    cv.round(rect(tx, ty, tw, th), 6, if on { palette::ON } else { palette::INK });
    // Knob at the on/off end, with an even margin.
    let m = 4;
    let k = th - 2 * m;
    let kx = if on { tx + tw - m - k } else { tx + m };
    cv.round(rect(kx, ty + m, k, k), 4, palette::PARCHMENT);
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
