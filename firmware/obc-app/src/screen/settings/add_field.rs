//! The Add Field picker: a wrapping list of every field that is not yet on the grid. A press adds
//! the highlighted field to the end of the selection and returns.

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

/// Per-row height. It matches the Fields list, so the two read the same.
const ROW_H: i32 = 46;

/// The fields that are not on the grid, in catalogue order.
fn hidden(list: &crate::stat_fields::StatFieldList) -> heapless::Vec<StatField, { StatField::ALL.len() }> {
    StatField::ALL.into_iter().filter(|f| !list.contains(*f)).collect()
}

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
                // A category field shows its own icon in a left gutter, and the row name stays the
                // plain category word. The icon replaces the span badge, because all these rows are
                // full width, and the free space lets the longest category name draw whole.
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
                    let a = row.area;
                    cv.text_vcentered(
                        f.name(lang),
                        a.top_left.x + 10,
                        (a.top_left.y, a.size.height as i32),
                        Font::Body,
                        TextAlign::Left,
                        INK,
                    );
                    super::span_badge(cv, a, f.span(), badge_color);
                }
            }
        });
    }
}
