//! The Add Field picker: a wrapping list of available fields that are not yet on the grid. A press adds
//! the highlighted field to the end of the selection and returns.

use embedded_graphics::primitives::Rectangle;
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::vocab::chrome::empty_state;
use crate::screen::vocab::list::{self, ListGeometry, Separators};
use crate::screen::vocab::rows::ROW_X;
use crate::screen::{Ctx, Render, Transition};
use crate::stat_fields::StatField;
use crate::Msg;

const ROW_H: i32 = 46;
const BADGE_W: i32 = 26;

/// General-purpose fields that are not on the grid, in catalogue order.
fn hidden(list: &crate::stat_fields::StatFieldList) -> heapless::Vec<StatField, { StatField::ALL.len() }> {
    StatField::ALL.into_iter().filter(|f| f.category().is_none() && !list.contains(*f)).collect()
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
            let badge_color = if row.selected { ON_ACCENT } else { SUBTEXT };
            let a = row.area;
            let room = a.size.width as i32 - 10 - 8 - BADGE_W - 10;
            let font = if text_width(f.name(lang), Font::Body) as i32 <= room { Font::Body } else { Font::Label };
            let name = rx.marquee.fit(f.name(lang), room, font, row.scroll());
            cv.text_vcentered(
                &name,
                a.top_left.x + 10,
                (a.top_left.y, a.size.height as i32),
                font,
                TextAlign::Left,
                if row.selected { ON_ACCENT } else { INK },
            );
            size_badge(cv, a, f, badge_color);
        });
    }
}

/// A miniature page shows the field's footprint within the two-column, three-row grid.
fn size_badge(cv: &mut impl Surface, area: Rectangle, field: StatField, color: u16) {
    use crate::screen::palette::RULE;
    let x = area.top_left.x + area.size.width as i32 - 10 - BADGE_W;
    let y = area.top_left.y + (area.size.height as i32 - 34) / 2;
    cv.round_outline(rect(x, y, BADGE_W, 34), 3, color);
    for row in 0..crate::stat_fields::ROWS_PER_PAGE as i32 {
        for col in 0..crate::stat_fields::COLS as i32 {
            cv.round(rect(x + 3 + col * 11, y + 3 + row * 10, 9, 8), 1, RULE);
        }
    }
    cv.round(rect(x + 3, y + 3, field.span() as i32 * 11 - 2, field.rows() as i32 * 10 - 2), 1, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{activity::Activity, screen::test_ctx, AppState, Mode, Settings};

    #[test]
    fn picker_omits_categories_and_adds_the_selected_general_field() {
        let mut settings = Settings::default();
        let available = hidden(&settings.stat_fields);
        assert!(available.iter().all(|f| f.category().is_none() && !settings.stat_fields.contains(*f)));
        for field in [StatField::NextWaypoint, StatField::WaypointList, StatField::HeartRate, StatField::Power] {
            assert!(available.contains(&field));
        }

        let mut state = AppState::new(0, 0, 1.0);
        let mut activity = Activity::new(Mode::Idle);
        let mut cx = test_ctx(&mut state, &mut activity, &mut settings);
        let mut screen = AddFieldScreen::new();
        let index = available.iter().position(|f| *f == StatField::WaypointList).unwrap();
        screen.handle(Gesture::Step(index as i32), &mut cx);
        assert!(matches!(screen.handle(Gesture::Press, &mut cx), Transition::Pop));
        assert_eq!(settings.stat_fields.as_slice().last(), Some(&StatField::WaypointList));
        assert!(!hidden(&settings.stat_fields).contains(&StatField::WaypointList));
    }
}
