//! The dismissable **warning** notice: the device runs, but something a rider should know is off —
//! a sensor the I²C probe never answered (GPS / altimeter / compass), a map that loaded but reads
//! slowly because it's fragmented (issue #504), or the ride log dropping points because an SD write
//! failed mid-ride (issue #11). Unlike a [`BootFault`](crate::fault), the device is fully usable;
//! this is advisory. Any press/Back dismisses it (like the [`NavFailScreen`](super::NavFailScreen)
//! card).
//!
//! **Raised as each fault is discovered**, coalesced onto one card: the host calls
//! [`App::notify_warning`](crate::App::notify_warning) for the boot-time faults (sensor presence
//! lands a moment after boot, the map-slow flag at open), and the app raises the recording-error
//! flag itself the first time [`TrackSink::record`](crate::TrackSink::record) fails. Each distinct
//! flag is shown **once per boot** — a dismissed notice doesn't nag, but a *new* flag arriving
//! later re-opens the card (see `App::notify_warning`). The absent sensors are listed by name so
//! the rider knows which module to check.

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::input::Gesture;

use super::{palette, title_frame, Ctx, Render, Transition};

/// The set of active device warnings, a small bitmask so several coalesce onto one card. Absent
/// sensors are distinct bits (the rider is told *which* to check); the map-slow and recording-error
/// advisories are each their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WarningFlags(u8);

impl WarningFlags {
    /// No warnings.
    pub const NONE: WarningFlags = WarningFlags(0);
    /// The GPS module didn't answer the boot I²C probe.
    pub const NO_GPS: WarningFlags = WarningFlags(1 << 0);
    /// The barometric altimeter didn't answer the boot I²C probe.
    pub const NO_ALTIMETER: WarningFlags = WarningFlags(1 << 1);
    /// The compass / IMU didn't answer the boot I²C probe.
    pub const NO_COMPASS: WarningFlags = WarningFlags(1 << 2);
    /// The map loaded but reads slowly — its extent table was refused (fragmented past the cap or
    /// failed verification), so reads fall back to the FAT-seek path (issue #504).
    pub const MAP_SLOW: WarningFlags = WarningFlags(1 << 3);
    /// A ride-log append failed mid-ride, so at least one track point was dropped and the log is
    /// now incomplete. Raised by the app the first time [`TrackSink::record`](crate::TrackSink::record)
    /// returns an error (a card pull, a write error, a full medium) — issue #11.
    pub const REC_ERROR: WarningFlags = WarningFlags(1 << 4);
    /// A settings write to the persistent store failed, so an edit did not reach RRAM/the file. The
    /// value stays live in RAM and the app keeps retrying (bounded backoff); this is the advisory that
    /// the persist is not yet durable (#810). Raised by the app on a
    /// [`HostEvent::SettingsPersistFailed`](crate::HostEvent::SettingsPersistFailed).
    pub const SETTINGS_ERROR: WarningFlags = WarningFlags(1 << 5);

    /// No bits set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether **every** bit of `other` is set here (and `other` is non-empty).
    pub const fn contains(self, other: WarningFlags) -> bool {
        other.0 != 0 && self.0 & other.0 == other.0
    }

    /// Whether any sensor-absence bit is set (as opposed to the map-slow advisory).
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

/// The advisory warning card. Carries the coalesced [`WarningFlags`] it lists; opened / updated by
/// [`App::notify_warning`](crate::App::notify_warning), dismissed on any press.
#[derive(Debug)]
pub struct WarningScreen {
    flags: WarningFlags,
}

impl WarningScreen {
    /// A card showing `flags`. The host only pushes one when `flags` is non-empty.
    pub fn new(flags: WarningFlags) -> Self {
        WarningScreen { flags }
    }

    /// The flags currently shown (so the host can OR a newly-discovered fault into a live card
    /// rather than stacking a second one).
    pub fn flags(&self) -> WarningFlags {
        self.flags
    }

    /// Add newly-discovered warnings to the live card.
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
        // Intentionally English, not catalogued (epic #602): this is a boot-time hardware-diagnostic
        // card (missing sensors / a slow map), grouped with the pre-`App` boot-fault copy in
        // `fault.rs` — kept English in every language build so a diagnostic reads the same to anyone
        // debugging a device, and so the sensor names line up with the datasheet. `rx.settings`
        // exists here, but the epic deliberately leaves this copy out of the catalog.
        title_frame(cv, w, h, "WARNING", "");

        // The shared warning triangle in the glyph slot (dialog anatomy, #678 T1); the text block
        // starts below it (was 26 % pre-glyph — nudged down only to make room).
        super::card_triangle(cv, Point::new(w / 2, super::TITLE_BAR_H + 46), 22);

        let line = Font::Body.line_height() as i32;
        let mut y = h * 36 / 100;

        // Absent sensors: one "Not detected:" headline, then a line per missing module so the rider
        // knows exactly which to check — each led by a tiny per-sensor glyph (#679).
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
                        // The mini compass needle: the Menu dial's shared needle, pointing NE.
                        _ => super::menu::draw_needle(cv, gc, 45.0, 5.0, 2.0),
                    }
                    y += line + 2;
                }
            }
            y += line / 2; // gap before the next block
        }

        // Map-slow advisory (issue #504): loaded fine, just fragmented → slower reads. Lines kept
        // short so they don't clip the 240 px panel (measured: 18 chars at Font::Body overruns).
        // No leading glyph: the mini map icon crowded the headline (owner review round 2) — the
        // per-sensor glyphs stay because their lines are short names with room to spare.
        if self.flags.contains(WarningFlags::MAP_SLOW) {
            cv.text("Slow map reads", Point::new(w / 2, y), Font::Body, TextAlign::Center, INK);
            y += line + 2;
            cv.text("Re-copy the map.", Point::new(w / 2, y), Font::Label, TextAlign::Center, SUBTEXT);
            y += line + line / 2; // advance past this block (+ gap) in case the recording error follows
        }

        // Recording-error advisory (issue #11): an SD append failed while riding, so the ride log
        // dropped at least one point and is now incomplete. The headline is in the WARNING colour —
        // it's a data loss the rider should act on (check the card), not a mere slowdown.
        if self.flags.contains(WarningFlags::REC_ERROR) {
            cv.text("Recording error", Point::new(w / 2, y), Font::Body, TextAlign::Center, WARNING);
            y += line + 2;
            cv.text("Log incomplete", Point::new(w / 2, y), Font::Label, TextAlign::Center, SUBTEXT);
            y += line + line / 2; // advance past this block (+ gap) in case the settings error follows
        }

        // Settings-write advisory (#810): a persist to the store failed, so a settings edit is not yet
        // durable. The value is still live and the app keeps retrying — this only tells the rider the
        // write hasn't landed. WARNING colour: it's a (recoverable) data-persistence issue.
        if self.flags.contains(WarningFlags::SETTINGS_ERROR) {
            cv.text("Settings not saved", Point::new(w / 2, y), Font::Body, TextAlign::Center, WARNING);
            y += line + 2;
            cv.text("Retrying write", Point::new(w / 2, y), Font::Label, TextAlign::Center, SUBTEXT);
        }
    }
}

// ── The tiny leading glyphs (≤ 12×12 px, one per warning line) ──

/// Half-width of a leading glyph's 12 px cell, and the gap to its line's first character. Only the
/// short sensor names carry a glyph now (the map-slow headline lost its icon in owner review round
/// 2 — 14 Body cells left it crammed against the card border), so the gap has room to spare.
const GLYPH_HALF: i32 = 6;
const GLYPH_GAP: i32 = 4;

/// The centre a line's leading glyph draws at: just left of the centred `name`'s first character,
/// vertically centred on the row's cap height.
fn glyph_anchor(w: i32, y: i32, name: &str, font: Font) -> Point {
    Point::new(w / 2 - text_width(name, font) as i32 / 2 - GLYPH_GAP - GLYPH_HALF, y + font.cap_height() as i32 / 2)
}

/// The GPS **signal fan**: a dot at bottom-left plus two concentric quarter-arc strokes opening
/// up-right — the classic "signal" mark. Arcs are plotted as stepped 1 px points (the canvas has
/// no arc primitive); the whole glyph stays inside the 12 px cell around `c`.
fn glyph_gps_fan(cv: &mut impl Surface, c: Point, color: u16) {
    let o = Point::new(c.x - 5, c.y + 5); // the emitter dot, bottom-left of the cell
    cv.disc(o, 1, color);
    for r in [5.0f32, 9.0] {
        let steps = (r * 2.0) as i32; // ~2 points per px of radius keeps the arc contiguous
        for k in 0..=steps {
            let a = core::f32::consts::FRAC_PI_2 * k as f32 / steps as f32;
            let p = Point::new(o.x + (libm::cosf(a) * r + 0.5) as i32, o.y - (libm::sinf(a) * r + 0.5) as i32);
            cv.disc(p, 0, color);
        }
    }
}

/// The altimeter's filled **climb triangle** — the same up-triangle idiom the stat tiles' climb
/// figures use, shrunk to the glyph cell.
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
        // NONE is never "contained" (a no-op flag shouldn't report present).
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
        // The recording-error flag is a distinct bit, not a sensor-absence one, and coalesces with
        // the map-slow advisory (both are SD/storage conditions that can be shown on one card).
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
