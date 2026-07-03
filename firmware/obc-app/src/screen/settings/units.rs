//! The Units screen — metric ↔ imperial. [`Units`](crate::settings::Units) re-captions and re-scales
//! the Statistics readouts and the off-route distance. A binary choice, so it's a single value row
//! that press (or a turn) flips in place — no field sub-mode.

use embedded_graphics::prelude::{DrawTarget, Point};
use obc_render::{
    text::{Font, TextAlign},
    Canvas, RenderStats, Surface,
};

use crate::input::Gesture;
use crate::screen::{palette, title_frame, Ctx, Render, Transition, LIST_TOP};

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
            // A binary choice: press or a turn flips it (no separate edit mode).
            Gesture::Press | Gesture::Turn(_) => {
                cx.settings.units = cx.settings.units.toggled();
                Transition::None
            }
            Gesture::Back => Transition::Pop,
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw<D, F>(&self, target: &mut D, rx: &mut Render, color_fn: &F) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        use palette::*;
        let (w, h) = (rx.w as i32, rx.h as i32);
        let units = rx.settings.units;
        let mut cv = Canvas::new(target, color_fn);
        title_frame(&mut cv, w, h, "UNITS", "");

        // The single value row — the current system centred, flanked by left/right arrows to read as
        // "rotate to switch".
        let area = super::row_rect(0, LIST_TOP + 8, w, 50);
        super::row_cursor(&mut cv, area, true, false);
        let midy = area.top_left.y + area.size.height as i32 / 2;
        cv.text(units.name(), Point::new(w / 2, area.top_left.y + (50 - 22) / 2), Font::Body, TextAlign::Center, INK);
        // ◄ and ► as filled triangles, inset from the row edges.
        let ax = area.top_left.x + 18;
        cv.triangle(Point::new(ax, midy - 9), Point::new(ax, midy + 9), Point::new(ax - 11, midy), INK);
        let bx = area.top_left.x + area.size.width as i32 - 18;
        cv.triangle(Point::new(bx, midy - 9), Point::new(bx, midy + 9), Point::new(bx + 11, midy), INK);

        // What the system means for each readout — label left, unit right.
        let rows: [(&str, &str); 3] =
            [("Distance", units.dist_label()), ("Speed", units.speed_label()), ("Elevation", units.elev_label())];
        let mut ry = LIST_TOP + 96;
        for (label, value) in rows {
            cv.text(label, Point::new(area.top_left.x + 12, ry), Font::Body, TextAlign::Left, SUBTEXT);
            cv.text(
                value,
                Point::new(area.top_left.x + area.size.width as i32 - 12, ry),
                Font::Body,
                TextAlign::Right,
                INK,
            );
            ry += 44;
        }
        RenderStats::default()
    }
}
