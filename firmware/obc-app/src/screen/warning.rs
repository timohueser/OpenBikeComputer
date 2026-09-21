//! The dismissable warning notice: the device runs, but something a rider must know is wrong — a
//! sensor that did not answer the I²C probe, a map that reads slowly, a failed ride-log write, or
//! a failed settings write. The device stays usable, so this card is advisory and any press
//! dismisses it.
//!
//! Warnings coalesce onto one card, and each flag shows once per boot: a dismissed notice does
//! not nag, but a new flag re-opens the card.

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::input::Gesture;

use super::vocab::chrome::{card_triangle, title_frame, TITLE_BAR_H};
use super::{palette, Ctx, Render, Transition};

/// The active device warnings. Each condition is its own bit, so several coalesce onto one card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WarningFlags(u8);

impl WarningFlags {
    pub const NONE: WarningFlags = WarningFlags(0);
    /// The GPS module didn't answer the boot I²C probe.
    pub const NO_GPS: WarningFlags = WarningFlags(1 << 0);
    /// The barometric altimeter didn't answer the boot I²C probe.
    pub const NO_ALTIMETER: WarningFlags = WarningFlags(1 << 1);
    /// The compass / IMU didn't answer the boot I²C probe.
    pub const NO_COMPASS: WarningFlags = WarningFlags(1 << 2);
    /// The map loaded, but its extent table was refused, so reads use the slow FAT-seek path.
    pub const MAP_SLOW: WarningFlags = WarningFlags(1 << 3);
    /// A ride-log write did not happen mid-ride, so the log is incomplete or at risk of it.
    pub const REC_ERROR: WarningFlags = WarningFlags(1 << 4);
    /// A settings write did not reach the persistent store. The value stays live in RAM and the
    /// app retries, so this only tells the rider the edit is not durable yet.
    pub const SETTINGS_ERROR: WarningFlags = WarningFlags(1 << 5);
    /// The card transport latched off mid-ride: enough operations failed in a row that the device
    /// stopped attempting the card. Nothing reads or writes storage until a power cycle.
    pub const STORAGE_ERROR: WarningFlags = WarningFlags(1 << 6);

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// True when every bit of `other` is set here, and `other` is not empty.
    pub const fn contains(self, other: WarningFlags) -> bool {
        other.0 != 0 && self.0 & other.0 == other.0
    }

    /// True when a sensor-absence bit is set, not only an advisory bit.
    const fn any_sensor(self) -> bool {
        self.0 & (Self::NO_GPS.0 | Self::NO_ALTIMETER.0 | Self::NO_COMPASS.0) != 0
    }
}

impl core::ops::BitOr for WarningFlags {
    type Output = WarningFlags;
    fn bitor(self, rhs: WarningFlags) -> WarningFlags {
        WarningFlags(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for WarningFlags {
    fn bitor_assign(&mut self, rhs: WarningFlags) {
        self.0 |= rhs.0;
    }
}

impl core::ops::BitAnd for WarningFlags {
    type Output = WarningFlags;
    fn bitand(self, rhs: WarningFlags) -> WarningFlags {
        WarningFlags(self.0 & rhs.0)
    }
}

impl core::ops::Not for WarningFlags {
    type Output = WarningFlags;
    fn not(self) -> WarningFlags {
        WarningFlags(!self.0)
    }
}

#[derive(Debug)]
pub struct WarningScreen {
    flags: WarningFlags,
}

impl WarningScreen {
    pub fn new(flags: WarningFlags) -> Self {
        WarningScreen { flags }
    }

    /// The flags shown now, so the host can add a new fault to the live card instead of pushing
    /// a second one.
    pub fn flags(&self) -> WarningFlags {
        self.flags
    }

    pub fn add(&mut self, flags: WarningFlags) {
        self.flags |= flags;
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

        if self.flags.any_sensor() {
            cv.text("Not detected:", Point::new(w / 2, y), Font::Body, TextAlign::Center, INK);
            y += line + 4;
            for (i, (bit, name)) in [
                (WarningFlags::NO_GPS, "GPS"),
                (WarningFlags::NO_ALTIMETER, "Altimeter"),
                (WarningFlags::NO_COMPASS, "Compass"),
            ]
            .into_iter()
            .enumerate()
            {
                if self.flags.contains(bit) {
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

        // Keep these lines short: 18 characters at Font::Body clip the panel.
        // The most severe line first: with storage latched off, every other advisory is downstream
        // of it.
        if self.flags.contains(WarningFlags::STORAGE_ERROR) {
            cv.text("Storage stopped", Point::new(w / 2, y), Font::Body, TextAlign::Center, WARNING);
            y += line + 2;
            cv.text("Restart the device", Point::new(w / 2, y), Font::Label, TextAlign::Center, SUBTEXT);
            y += line + line / 2;
        }

        if self.flags.contains(WarningFlags::MAP_SLOW) {
            cv.text("Slow map reads", Point::new(w / 2, y), Font::Body, TextAlign::Center, INK);
            y += line + 2;
            cv.text("Re-copy the map.", Point::new(w / 2, y), Font::Label, TextAlign::Center, SUBTEXT);
            y += line + line / 2;
        }

        // The headline is in the warning colour: this is data loss, not only a slowdown.
        if self.flags.contains(WarningFlags::REC_ERROR) {
            cv.text("Recording error", Point::new(w / 2, y), Font::Body, TextAlign::Center, WARNING);
            y += line + 2;
            cv.text("Log incomplete", Point::new(w / 2, y), Font::Label, TextAlign::Center, SUBTEXT);
            y += line + line / 2;
        }

        if self.flags.contains(WarningFlags::SETTINGS_ERROR) {
            cv.text("Settings not saved", Point::new(w / 2, y), Font::Body, TextAlign::Center, WARNING);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_set_ops() {
        let mut f = WarningFlags::NONE;
        assert!(f.is_empty());
        f |= WarningFlags::NO_GPS;
        f |= WarningFlags::MAP_SLOW;
        assert!(!f.is_empty());
        assert!(f.contains(WarningFlags::NO_GPS));
        assert!(f.contains(WarningFlags::MAP_SLOW));
        assert!(!f.contains(WarningFlags::NO_COMPASS));
        assert!(f.any_sensor());
        // An empty flag never reports as present.
        assert!(!f.contains(WarningFlags::NONE));
    }

    #[test]
    fn map_slow_alone_is_not_a_sensor_warning() {
        let f = WarningFlags::MAP_SLOW;
        assert!(!f.any_sensor());
        assert!(f.contains(WarningFlags::MAP_SLOW));
    }

    #[test]
    fn rec_error_is_its_own_non_sensor_advisory() {
        let f = WarningFlags::REC_ERROR;
        assert!(!f.any_sensor());
        assert!(f.contains(WarningFlags::REC_ERROR));
        assert!(!f.contains(WarningFlags::MAP_SLOW));
        let both = WarningFlags::REC_ERROR | WarningFlags::MAP_SLOW;
        assert!(both.contains(WarningFlags::REC_ERROR));
        assert!(both.contains(WarningFlags::MAP_SLOW));
    }

    #[test]
    fn and_not_masks_seen_flags() {
        let raised = WarningFlags::NO_GPS | WarningFlags::NO_COMPASS;
        let seen = WarningFlags::NO_GPS;
        let fresh = raised & !seen;
        assert!(fresh.contains(WarningFlags::NO_COMPASS));
        assert!(!fresh.contains(WarningFlags::NO_GPS));
    }
}
