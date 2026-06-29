//! The Units screen — metric ↔ imperial. The one setting that reaches beyond the settings
//! tree: [`Units`](crate::settings::Units) re-captions and re-scales the Statistics readouts and
//! the off-route distance. A binary choice, so it's a single value row that **press** (or a
//! turn) flips in place — no field sub-mode needed. Built to grow later (separate distance /
//! elevation / temperature rows) without changing the navigation.

use core::fmt::Write;

use embedded_graphics::prelude::{DrawTarget, Point};
use obc_render::{
    text::{Font, TextAlign},
    Canvas, RenderStats,
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

        // The single value row, always the cursor — the current system (Metric / Imperial)
        // centred and bold. The title already says UNITS, so no left label is needed (and it
        // would crowd the longer "Imperial" on the 240 px row).
        let area = super::row_rect(0, LIST_TOP + 8, w, 50);
        super::row_cursor(&mut cv, area, true, false);
        cv.text(units.name(), Point::new(w / 2, area.top_left.y + (50 - 22) / 2), Font::Body, TextAlign::Center, INK);

        // A compact reminder of what that system means for each readout (ASCII only — the panel
        // font has no middle-dot), so flipping it shows the effect at a glance.
        let mut summary: heapless::String<24> = heapless::String::new();
        let _ = write!(summary, "{} / {} / {}", units.dist_label(), units.speed_label(), units.elev_label());
        cv.text(&summary, Point::new(w / 2, LIST_TOP + 80), Font::Body, TextAlign::Center, SUBTEXT);
        cv.text("dist / speed / elev", Point::new(w / 2, LIST_TOP + 112), Font::Label, TextAlign::Center, SUBTEXT);

        super::back_hint(&mut cv, w, h, "press to switch");
        RenderStats::default()
    }
}
