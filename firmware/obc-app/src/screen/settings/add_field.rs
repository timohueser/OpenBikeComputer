//! The Add Field picker — a wrapping list of every predefined field not yet on the grid. `press`
//! adds the highlighted field to the end of the selection and returns; `back` returns without adding.
//! When every field is already shown it's a quiet empty state.

use embedded_graphics::prelude::DrawTarget;
use obc_render::{Canvas, RenderStats};

use crate::input::Gesture;
use crate::screen::{scrollbar, title_frame, window_start, Ctx, Render, Transition, LIST_TOP};
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
            Gesture::Turn(n) => {
                self.selected = crate::screen::step_selection(self.selected, n, avail.len());
                Transition::None
            }
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

    pub fn draw<D, F>(&self, target: &mut D, rx: &mut Render, color_fn: &F) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        use crate::screen::palette::*;
        let (w, h) = (rx.w as i32, rx.h as i32);
        let avail = hidden(&rx.settings.stat_fields);
        let total = avail.len();
        let mut cv = Canvas::new(target, color_fn);

        title_frame(&mut cv, w, h, "ADD FIELD", "");

        if total == 0 {
            super::super::empty_state(&mut cv, w, h, "All fields added", "Remove one to swap");
            return RenderStats::default();
        }

        let list_h = h - LIST_TOP - 6;
        let visible = (list_h / ROW_H).max(1) as usize;
        let sel = self.selected.min(total - 1);
        let first = window_start(sel, visible, total);

        for slot in 0..visible {
            let i = first + slot;
            if i >= total {
                break;
            }
            let f = avail[i];
            let y = LIST_TOP + slot as i32 * ROW_H;
            let area = super::row_rect(0, y, w, ROW_H - 6);
            let selected = i == sel;
            super::row_cursor(&mut cv, area, selected, false);
            super::row_label(&mut cv, area, f.name(), None);
            let badge_color = if selected { INK } else { SUBTEXT };
            super::span_badge(&mut cv, area, f.span(), badge_color);
        }

        scrollbar(&mut cv, w - 8, LIST_TOP, visible as i32 * ROW_H, total, first, visible);
        RenderStats::default()
    }
}
