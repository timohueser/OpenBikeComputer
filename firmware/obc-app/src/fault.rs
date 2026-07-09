//! Full-screen **boot faults** — the unrecoverable bring-up failures that leave nothing else to
//! draw: no SD card, no map file on the card, or a map the reader can't parse. Unlike the
//! dismissable [warnings](crate::screen::WarningScreen), these are drawn *without* an [`App`] (there
//! is no map to build one around) and never dismiss — the device sits on the message until the
//! rider fixes the card and reboots.
//!
//! A host with a live display calls [`draw_boot_fault`] once, then idles (a heartbeat LED, say):
//! the frame persists on glass with no further work. The copy is plain — a rider reads *what's
//! wrong* and *what to do*, no jargon (the "TooFragmented" / parse-error detail stays in the log).
//!
//! Kept in `obc-app` (not the board crate) so the simulator draws the identical screen and the copy
//! is unit-tested here, next to the [warnings](crate::screen::warning) it mirrors.

use embedded_graphics::{draw_target::DrawTarget, prelude::Point};
use obc_render::{
    text::{Font, TextAlign},
    Canvas, Surface,
};

use crate::screen::{palette, title_frame};

/// An unrecoverable storage fault at boot, before the app exists. Each maps to one of the fatal
/// `idle` sites in the board's `main` — a card that won't mount, no `.obcm` in the root, or a map
/// that fails [`obc_reader`]'s header parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootFault {
    /// No card, or the card wouldn't mount — the map streams from it, so this is fatal.
    NoCard,
    /// The card mounted but holds no `.obcm` map file.
    NoMap,
    /// A map file is present but isn't valid OBCM (truncated / corrupt / wrong format).
    BadMap,
}

impl BootFault {
    /// The card's `(title, line 1, line 2)` — the wood-bar title and the two body lines. Plain
    /// language: the fault, then the fix. Pinned by [`tests`].
    ///
    /// **Intentionally English, not catalogued (epic #602).** A boot fault is drawn by
    /// [`draw_boot_fault`] before `App`/`Settings` exist — there is no `settings.language` to read at
    /// this point in the boot path — so this diagnostic copy stays English in every language build.
    pub fn copy(self) -> (&'static str, &'static str, &'static str) {
        match self {
            BootFault::NoCard => ("NO SD CARD", "Insert a card", "with a map file"),
            BootFault::NoMap => ("NO MAP", "Card has no map", "Add a map file"),
            BootFault::BadMap => ("MAP UNREADABLE", "Bad map file", "Re-copy it"),
        }
    }
}

/// Draw a full-screen boot fault into `target`. Standalone (no [`App`](crate::App)): the shared wood
/// frame + centred two-line message, so it reads as part of the same UI while needing nothing but a
/// display. Push it once and hold it; it draws no animation and expects no input.
pub fn draw_boot_fault<D, F>(target: &mut D, w: i32, h: i32, color_fn: F, fault: BootFault)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    use palette::*;
    let mut cv = Canvas::new(target, &color_fn);
    let (title, l1, l2) = fault.copy();
    title_frame(&mut cv, w, h, title, "");
    let line = Font::Body.line_height() as i32;
    // Two body lines, centred, sitting a little above the vertical middle so the pair reads centred.
    let y = h * 42 / 100;
    cv.text(l1, Point::new(w / 2, y), Font::Body, TextAlign::Center, INK);
    cv.text(l2, Point::new(w / 2, y + line + 6), Font::Body, TextAlign::Center, SUBTEXT);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every fault has a short, non-empty, distinct title + two body lines — the plain-copy
    /// contract (a rider must read what's wrong and what to do without the log).
    #[test]
    fn copy_is_present_and_distinct() {
        let all = [BootFault::NoCard, BootFault::NoMap, BootFault::BadMap];
        for f in all {
            let (t, l1, l2) = f.copy();
            assert!(!t.is_empty() && !l1.is_empty() && !l2.is_empty(), "{f:?} has empty copy");
            // Titles fit the wood bar (short, all-caps house style).
            assert!(t.len() <= 16, "{f:?} title too long for the bar: {t:?}");
            // Body lines fit the 240 px panel at Font::Body centred (≈18 chars = full width; keep a
            // margin). Measured on glass: "Insert a card with" (18) touched the border, "…damaged —"
            // (25) overran badly — this pins the fix so a future copy edit can't silently clip.
            assert!(l1.len() <= 16 && l2.len() <= 16, "{f:?} body line too wide: {l1:?} / {l2:?}");
        }
        // Distinct titles so the three fatal sites are told apart on glass.
        assert_ne!(BootFault::NoCard.copy().0, BootFault::NoMap.copy().0);
        assert_ne!(BootFault::NoMap.copy().0, BootFault::BadMap.copy().0);
    }
}
