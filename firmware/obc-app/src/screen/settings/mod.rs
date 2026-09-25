//! The Settings tree. The hub and its pages are one screen type over row tables
//! ([`page`]); the editors of their values are the drawer's own editor, opened as a sheet over
//! the page. The screens with a shape of their own — the Fields grid, the Language pick list, the
//! Sensors lists, the About credits and the Reset confirm — keep their own files.

use obc_render::Surface;

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
pub(crate) use sensors::{kind_msg, status_line, wake_msg};
pub use sensors::{SensorScanScreen, SensorsScreen};

/// The bottom-anchored guarded Forget row of the Sensors list: the destructive action row of the
/// shared grammar, filling warning-red with the hold. The caller draws it only when there is
/// something to forget.
pub(super) fn forget_footer(cv: &mut impl Surface, w: i32, h: i32, label: &str, hold: f32) {
    let row = row_rect(h - 10 - ROW_ONE, w, ROW_ONE);
    action_row(cv, row, label, None, true, true, true, hold);
}
