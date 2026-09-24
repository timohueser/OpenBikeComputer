//! The dismissable warning notice: the device runs, but something a rider must know is wrong — a
//! sensor that did not answer the I²C probe, a failed ride-log write, or a failed settings write.
//! The device stays usable, so this card is advisory and any press dismisses it.
//!
//! Alerts coalesce onto one card, and each alert shows once per boot: a dismissed notice does
//! not nag, but a new alert re-opens the card.

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::alert::{Alert, Alerts};
use crate::input::Gesture;

use super::vocab::chrome::{card_triangle, title_frame, TITLE_BAR_H};
use super::{palette, Ctx, Render, Transition};

#[derive(Debug)]
pub struct WarningScreen {
    alerts: Alerts,
}

impl WarningScreen {
    pub fn new(alerts: Alerts) -> Self {
        WarningScreen { alerts }
    }

    /// The alerts shown now, so the host can add a new alert to the live card instead of pushing
    /// a second one.
    pub fn alerts(&self) -> Alerts {
        self.alerts
    }

    pub fn add(&mut self, alerts: Alerts) {
        self.alerts.raise(alerts);
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Press | Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        // The copy is English and not in the message catalog on purpose: a hardware diagnostic
        // must read the same in every language build, and the sensor names must match the sheets.
        title_frame(cv, w, h, "WARNING", "");

        card_triangle(cv, Point::new(w / 2, TITLE_BAR_H + 46), 22);

        let line = Font::Body.line_height() as i32;
        let mut y = h * 36 / 100;

        let sensors = [(Alert::NoGps, "GPS"), (Alert::NoAltimeter, "Altimeter"), (Alert::NoCompass, "Compass")];
        if sensors.iter().any(|&(alert, _)| self.alerts.contains(alert)) {
            cv.text("Not detected:", Point::new(w / 2, y), Font::Body, TextAlign::Center, INK);
            y += line + 4;
            for (i, (alert, name)) in sensors.into_iter().enumerate() {
                if self.alerts.contains(alert) {
                    cv.text(name, Point::new(w / 2, y), Font::Body, TextAlign::Center, WARNING);
                    let gc = glyph_anchor(w, y, name, Font::Body);
                    match i {
                        0 => glyph_gps_fan(cv, gc, WARNING),
                        1 => glyph_altimeter(cv, gc, WARNING),
                        _ => super::menu::draw_needle(cv, gc, 45.0, 5.0, 2.0),
                    }
                    y += line + 2;
                }
            }
            y += line / 2;
        }

        // Keep these lines short: the copy-fit gate holds a centred Body line to 16 characters.
        // The most severe line first: with storage latched off, every other advisory is downstream
        // of it.
        if self.alerts.contains(Alert::StorageLost) {
            cv.text("Storage stopped", Point::new(w / 2, y), Font::Body, TextAlign::Center, WARNING);
            y += line + 2;
            cv.text("Check the card", Point::new(w / 2, y), Font::Label, TextAlign::Center, SUBTEXT);
            y += line + line / 2;
        }

        // The headline is in the warning colour: this is data loss, not only a slowdown. A failed
        // write now and an incomplete log found at boot read the same to the rider.
        if self.alerts.contains(Alert::RecordingFailed) || self.alerts.contains(Alert::RideRecoveredIncomplete) {
            cv.text("Recording error", Point::new(w / 2, y), Font::Body, TextAlign::Center, WARNING);
            y += line + 2;
            cv.text("Log incomplete", Point::new(w / 2, y), Font::Label, TextAlign::Center, SUBTEXT);
            y += line + line / 2;
        }

        if self.alerts.contains(Alert::SettingsNotSaved) {
            cv.text("Settings unsaved", Point::new(w / 2, y), Font::Body, TextAlign::Center, WARNING);
            y += line + 2;
            cv.text("Retrying write", Point::new(w / 2, y), Font::Label, TextAlign::Center, SUBTEXT);
        }
    }
}

/// Half-width of a leading glyph's 12 px cell, and the gap to the first character of its line.
const GLYPH_HALF: i32 = 6;
const GLYPH_GAP: i32 = 4;

/// The centre of a line's leading glyph: left of the centred `name`, on the row's cap height.
fn glyph_anchor(w: i32, y: i32, name: &str, font: Font) -> Point {
    Point::new(w / 2 - text_width(name, font) as i32 / 2 - GLYPH_GAP - GLYPH_HALF, y + font.cap_mid() as i32)
}

/// The GPS signal fan: an emitter dot with two quarter arcs. The arcs are stepped points,
/// because the canvas has no arc primitive.
fn glyph_gps_fan(cv: &mut impl Surface, c: Point, color: u16) {
    let o = Point::new(c.x - 5, c.y + 5);
    cv.disc(o, 1, color);
    for r in [5.0f32, 9.0] {
        let steps = (r * 2.0) as i32; // two points per px of radius keep the arc contiguous
        for k in 0..=steps {
            let a = core::f32::consts::FRAC_PI_2 * k as f32 / steps as f32;
            let p = Point::new(o.x + (libm::cosf(a) * r + 0.5) as i32, o.y - (libm::sinf(a) * r + 0.5) as i32);
            cv.disc(p, 0, color);
        }
    }
}

/// The altimeter's filled climb triangle, shrunk to the glyph cell.
fn glyph_altimeter(cv: &mut impl Surface, c: Point, color: u16) {
    cv.triangle(Point::new(c.x - 5, c.y + 5), Point::new(c.x + 5, c.y + 5), Point::new(c.x, c.y - 5), color);
}
