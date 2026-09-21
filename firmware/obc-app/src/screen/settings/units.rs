//! The Units screen: metric or imperial. The choice re-captions and re-scales the Statistics
//! readouts and the off-route distance. It is one value row that a press or a step flips.

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::vocab::chrome::{title_frame, LIST_TOP};
use crate::screen::vocab::rows::value_row_with_arrows;
use crate::screen::{palette, Ctx, Render, Transition};
use crate::Msg;

/// Stateless. The value lives in [`Settings`](crate::Settings), and the one row is always the cursor.
#[derive(Debug, Default)]
pub struct UnitsScreen;

impl UnitsScreen {
    pub fn new() -> Self {
        UnitsScreen
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Press | Gesture::Step(_) => {
                cx.settings.units = cx.settings.units.cycled();
                Transition::None
            }
            Gesture::Back => Transition::Pop,
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        let units = rx.settings.units;
        title_frame(cv, w, h, rx.t(Msg::UnitsTitle), "");

        let area = value_row_with_arrows(cv, LIST_TOP + 8, w, units.name(rx.settings.language));

        // The unit labels are dimmed, so the block reads as a preview of the choice above and not
        // as three more editable rows.
        let rows: [(&str, &str); 3] = [
            (rx.t(Msg::UnitsDistance), units.dist_label()),
            (rx.t(Msg::UnitsSpeed), units.speed_label()),
            (rx.t(Msg::UnitsElevation), units.elev_label()),
        ];
        let mut ry = LIST_TOP + 96;
        for (label, value) in rows {
            cv.text(label, Point::new(area.top_left.x + 12, ry), Font::Body, TextAlign::Left, SUBTEXT);
            cv.text(
                value,
                Point::new(area.top_left.x + area.size.width as i32 - 12, ry),
                Font::Body,
                TextAlign::Right,
                SUBTEXT,
            );
            ry += 44;
        }
    }
}
