//! The Add Field picker — a wrapping list of every predefined field not yet on the grid. `press`
//! adds the highlighted field to the end of the selection and returns; `back` returns without adding.
//! When every field is already shown it's a quiet empty state.

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::vocab::chrome::empty_state;
use crate::screen::vocab::list::{self, ListGeometry, Separators};
use crate::screen::vocab::rows::ROW_X;
use crate::screen::{Ctx, Render, Transition};
use crate::stat_fields::StatField;
use crate::Msg;

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
            Gesture::Step(n) => list::on_step(&mut self.selected, n, avail.len()),
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
        let geo = ListGeometry::below_title(w, h, ROW_H, 6, ROW_X, Separators::None);

        let sel = if total == 0 { 0 } else { self.selected.min(total - 1) };
        list::list_frame(cv, w, h, rx.t(Msg::AddFieldTitle), sel + 1, total, geo.visible);

        if total == 0 {
            empty_state(cv, w, h, rx.t(Msg::AddFieldAllAdded), rx.t(Msg::AddFieldAllAddedSub));
            return;
        }

        let lang = rx.settings.language;
        let first = list::window_start(sel, geo.visible, total) as i32;
        list::draw_rows(cv, geo, total, sel, first, |cv, row| {
            let f = avail[row.index];
            let badge_color = if row.selected { INK } else { SUBTEXT };
            match f.category() {
                // A `Next: <category>` field (epic #946, U5) wears the category's own row icon in a
                // left gutter — the same glyph the tile, the Up-ahead rows and the POI menu use. The
                // icon *is* the "Next:" of the name: six icon rows in a block, directly under
                // `Next waypoint`, are unmistakably one group, and the name stays the plain
                // (already-translated) category word instead of a composed label that no longer fits
                // a row in German or French. It replaces the span badge on these rows — all six are
                // full-width by construction, so the badge would carry no information the block
                // doesn't already state, and the freed pixels are what let every language's longest
                // category name draw whole.
                Some(cat) => {
                    let a = row.area;
                    let mid = a.top_left.y + a.size.height as i32 / 2;
                    let bg = if row.selected { AMBER } else { PARCHMENT };
                    crate::screen::poi_menu::draw_category_icon(
                        cv,
                        cat,
                        Point::new(a.top_left.x + 22, mid),
                        badge_color,
                        bg,
                    );
                    cv.text_vcentered(
                        f.name(lang),
                        a.top_left.x + 40,
                        (a.top_left.y, a.size.height as i32),
                        Font::Body,
                        TextAlign::Left,
                        INK,
                    );
                }
                None => {
                    super::row_label(cv, row.area, f.name(lang), None);
                    super::span_badge(cv, row.area, f.span(), badge_color);
                }
            }
        });
    }
}
