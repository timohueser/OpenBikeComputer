//! The Add Field picker — a wrapping list of every predefined field not yet on the grid. `press`
//! adds the highlighted field to the end of the selection and returns; `back` returns without adding.
//! When every field is already shown it's a quiet empty state.

use obc_render::Surface;

use crate::input::Gesture;
use crate::screen::list::{self, ListGeometry, Separators};
use crate::screen::{Ctx, Render, Transition};
use crate::stat_fields::StatField;

/// Per-row height — matches the Stat Fields list so the two read identically.
const ROW_H: i32 = 46;

/// The fields not currently on the grid, in catalogue order — the picker's contents.
fn hidden(list: &crate::stat_fields::StatFieldList) -> heapless::Vec<StatField, { StatField::ALL.len() }> {
    StatField::ALL.into_iter().filter(|f| !list.contains(*f)).collect()
}

/// The Add Field picker. State is just the highlighted row (a wrapping selection, like the menus).
#[derive(Debug, Default)]
pub struct AddFieldScreen {
    selected: usize,
}

impl AddFieldScreen {
    pub fn new() -> Self {
        AddFieldScreen::default()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let avail = hidden(&cx.settings.stat_fields);
        match g {
            Gesture::Turn(n) => list::on_turn(&mut self.selected, n, avail.len()),
            // Add the highlighted field to the end of the grid and return to the manage screen.
            Gesture::Press if !avail.is_empty() => {
                let f = avail[self.selected.min(avail.len() - 1)];
                cx.settings.stat_fields.push(f);
                Transition::Pop
            }
            Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use crate::screen::palette::*;
        let (w, h) = (rx.w, rx.h);
        let avail = hidden(&rx.settings.stat_fields);
        let total = avail.len();
        let geo = ListGeometry::below_title(w, h, ROW_H, 6, super::ROW_X, Separators::None);

        let sel = if total == 0 { 0 } else { self.selected.min(total - 1) };
        list::list_frame(cv, w, h, "ADD FIELD", sel + 1, total, geo.visible);

        if total == 0 {
            super::empty_state(cv, w, h, "All fields added", "Remove one to swap");
            return;
        }

        let first = list::window_start(sel, geo.visible, total) as i32;
        list::draw_rows(cv, geo, total, sel, first, |cv, row| {
            let f = avail[row.index];
            super::row_label(cv, row.area, f.name(), None);
            let badge_color = if row.selected { INK } else { SUBTEXT };
            super::span_badge(cv, row.area, f.span(), badge_color);
        });
    }
}
