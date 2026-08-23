//! The Units screen — metric ↔ imperial. [`Units`](crate::settings::Units) re-captions and re-scales
//! the Statistics readouts and the off-route distance. A binary choice, so it's a single value row
//! that press (or a step) flips in place — no field sub-mode.

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::{palette, title_frame, Ctx, Render, Transition, LIST_TOP};
use crate::Msg;

/// The Units screen. Stateless — the value lives in [`Settings`](crate::Settings); the one row
/// is always the cursor.
#[derive(Debug, Default)]
pub struct UnitsScreen;

impl UnitsScreen {
    pub fn new() -> Self {
        UnitsScreen
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // A binary choice: press or a step flips it (no separate edit mode).
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

        // The single value row — the current system centred, flanked by left/right arrows to read as
        // "rotate to switch". Shared with the Language picker (`value_row_with_arrows`).
        let area = super::value_row_with_arrows(cv, LIST_TOP + 8, w, units.name(rx.settings.language));

        // What the system means for each readout — caption left, value right. The value is dimmed
        // one step (INK → the olive SUBTEXT the captions use) so the block reads as a **read-only
        // consequence preview** of the choice above, not three more editable rows (T8 item 1).
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
