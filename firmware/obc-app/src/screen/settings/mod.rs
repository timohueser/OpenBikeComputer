//! The Settings tree. The hub and its pages are one screen type over row tables
//! ([`page`]); the editors of their values are the drawer's own editor, opened as a sheet over
//! the page. The screens with a shape of their own — the Fields grid, the Language pick list, the
//! Sensors lists, the About credits and the Reset confirm — keep their own files.

use embedded_graphics::primitives::Rectangle;
use obc_render::{rect, Surface};

use super::vocab::rows::{action_row, row_rect, ROW_ONE};

mod about;
mod add_field;
/// The bike sprites for the ride-start card that has no route.
pub(crate) mod bike_icons;
mod fields;
mod language;
pub(crate) mod page;
mod reset;
mod sensors;

pub use about::AboutScreen;
pub use add_field::AddFieldScreen;
pub use fields::StatFieldsScreen;
pub use language::LanguageScreen;
pub use page::SettingsPage;
pub use reset::ResetScreen;
pub use sensors::{SensorScanScreen, SensorsScreen};

/// The bottom-anchored guarded Forget row of the Sensors list: the destructive action row of the
/// shared grammar, filling warning-red with the hold. The caller draws it only when there is
/// something to forget.
pub(super) fn forget_footer(cv: &mut impl Surface, w: i32, h: i32, label: &str, hold: f32) {
    let row = row_rect(h - 10 - ROW_ONE, w, ROW_ONE);
    action_row(cv, row, label, None, true, true, true, hold);
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
