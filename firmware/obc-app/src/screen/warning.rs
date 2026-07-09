//! The dismissable **warning** notice: the device booted and runs, but something a rider should
//! know is off — a sensor the I²C probe never answered (GPS / altimeter / compass), or a map that
//! loaded but reads slowly because it's fragmented (issue #504). Unlike a [`BootFault`](crate::fault),
//! the device is fully usable; this is advisory. Any press/Back dismisses it (like the
//! [`NavFailScreen`](super::NavFailScreen) card).
//!
//! **Host-pushed**, coalesced: the host calls [`App::notify_warning`](crate::App::notify_warning)
//! as each fault is discovered (sensor presence lands a moment after boot, the map-slow flag at
//! open). Each distinct flag is shown **once per boot** — a dismissed notice doesn't nag, but a
//! *new* flag arriving later re-opens the card (see `App::notify_warning`). The absent sensors are
//! listed by name so the rider knows which module to check.

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;

use super::{palette, title_frame, Ctx, Render, Transition};

/// The set of active device warnings, a small bitmask so several coalesce onto one card. Absent
/// sensors are distinct bits (the rider is told *which* to check); the map-slow advisory is its own.
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

        let line = Font::Body.line_height() as i32;
        let mut y = h * 26 / 100;

        // Absent sensors: one "Not detected:" headline, then a line per missing module so the rider
        // knows exactly which to check.
        if self.flags.any_sensor() {
            cv.text("Not detected:", Point::new(w / 2, y), Font::Body, TextAlign::Center, INK);
            y += line + 4;
            for (bit, name) in [
                (WarningFlags::NO_GPS, "GPS"),
                (WarningFlags::NO_ALTIMETER, "Altimeter"),
                (WarningFlags::NO_COMPASS, "Compass"),
            ] {
                if self.flags.contains(bit) {
                    cv.text(name, Point::new(w / 2, y), Font::Body, TextAlign::Center, WARNING);
                    y += line + 2;
                }
            }
            y += line / 2; // gap before the next block
        }

        // Map-slow advisory (issue #504): loaded fine, just fragmented → slower reads. Lines kept
        // short so they don't clip the 240 px panel (measured: 18 chars at Font::Body overruns).
        if self.flags.contains(WarningFlags::MAP_SLOW) {
            cv.text("Slow map reads", Point::new(w / 2, y), Font::Body, TextAlign::Center, INK);
            y += line + 2;
            cv.text("Re-copy the map.", Point::new(w / 2, y), Font::Label, TextAlign::Center, SUBTEXT);
        }
    }
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
    fn and_not_masks_seen_flags() {
        let raised = WarningFlags::NO_GPS | WarningFlags::NO_COMPASS;
        let seen = WarningFlags::NO_GPS;
        let fresh = raised & !seen;
        assert!(fresh.contains(WarningFlags::NO_COMPASS));
        assert!(!fresh.contains(WarningFlags::NO_GPS));
    }
}
