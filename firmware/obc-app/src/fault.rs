//! Full-screen **boot faults** — the unrecoverable bring-up failures that leave nothing else to
//! draw: no SD card, no map file on the card, or a map the reader can't parse. Unlike the
//! dismissable [warnings](crate::screen::WarningScreen), these are drawn *without* an [`App`] (there
//! is no map to build one around) and never dismiss — the device sits on the message until the
//! rider fixes the card and reboots.
//!
//! A host with a live display calls [`draw_boot_fault`] once, then idles (a heartbeat LED, say):
//! the frame persists on glass with no further work. The copy is a parallel two-line family —
//! line 1 ink = *what's wrong*, line 2 olive = *the fix*, no jargon (the "TooFragmented" /
//! parse-error detail stays in the log) — under the shared SD-card pictogram in the glyph slot
//! (dialog anatomy, epic #678 T1).
//!
//! Kept in `obc-app` (not the board crate) so the simulator draws the identical screen and the copy
//! is unit-tested here, next to the [warnings](crate::screen::warning) it mirrors.

use embedded_graphics::{draw_target::DrawTarget, prelude::Point};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, Surface,
};

use crate::screen::{palette, title_frame, wrapped, TITLE_BAR_H};
use crate::settings::Language;
use crate::{t, Msg};

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
    /// The card's `(title, what, fix)` — the wood-bar title, the ink *what's wrong* line, and the
    /// olive *fix* line (word-wrapped at draw time). Pinned by [`tests`].
    ///
    /// The what/fix pair lives in the Msg catalog (so the repertoire test pins it and the
    /// translations exist), but a boot fault is drawn by [`draw_boot_fault`] **before**
    /// `App`/`Settings` exist — there is no `settings.language` to read this early in the boot
    /// path — so it renders the English column in every language build, the same intentionally-
    /// English diagnostics decision as epic #602's.
    pub fn copy(self) -> (&'static str, &'static str, &'static str) {
        const EN: Language = Language::En;
        match self {
            BootFault::NoCard => ("NO SD CARD", t(Msg::FaultNocardWhat, EN), t(Msg::FaultNocardFix, EN)),
            BootFault::NoMap => ("NO MAP", t(Msg::FaultNomapWhat, EN), t(Msg::FaultNomapFix, EN)),
            BootFault::BadMap => ("MAP UNREADABLE", t(Msg::FaultBadmapWhat, EN), t(Msg::FaultBadmapFix, EN)),
        }
    }
}

/// Draw a full-screen boot fault into `target`. Standalone (no [`App`](crate::App)): the shared wood
/// frame, the SD-card glyph, and the centred what/fix pair, so it reads as part of the same UI while
/// needing nothing but a display. Push it once and hold it; it draws no animation and expects no
/// input.
pub fn draw_boot_fault<D, F>(target: &mut D, w: i32, h: i32, color_fn: F, fault: BootFault)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    use palette::*;
    let mut cv = Canvas::new(target, &color_fn);
    let (title, what, fix) = fault.copy();
    title_frame(&mut cv, w, h, title, "");
    // The SD-card pictogram in the glyph slot — the one pictogram all three storage faults share.
    sd_card_glyph(&mut cv, Point::new(w / 2, TITLE_BAR_H + 56));
    // What's wrong (ink, Body), then the fix (olive, Label, wrapped) — the parallel two-line family.
    let y = h * 42 / 100;
    cv.text(what, Point::new(w / 2, y), Font::Body, TextAlign::Center, INK);
    wrapped(&mut cv, fix, w / 2, y + Font::Body.line_height() as i32 + 6, w - 40, Font::Label, SUBTEXT);
}

/// The SD-card pictogram: a vertical rounded rectangle ≈28×36 px in a 2 px INK outline, the
/// top-right corner cut at 45° (the SD notch, re-stroked 2 px), and four short vertical contact
/// stripes hanging inside the top edge. Centred at `c`.
fn sd_card_glyph(cv: &mut impl Surface, c: Point) {
    use palette::*;
    const W: i32 = 28;
    const H: i32 = 36;
    const NOTCH: i32 = 10;
    let (x0, y0) = (c.x - W / 2, c.y - H / 2);
    // Body: a filled INK round-rect with the interior punched back out, leaving a 2 px outline.
    cv.round(rect(x0, y0, W, H), 4, INK);
    cv.round(rect(x0 + 2, y0 + 2, W - 4, H - 4), 3, PARCHMENT);
    // The notch: erase the top-right corner past the 45° cut, then re-stroke the cut edge 2 px.
    cv.triangle(
        Point::new(x0 + W - NOTCH, y0 - 1),
        Point::new(x0 + W + 1, y0 - 1),
        Point::new(x0 + W + 1, y0 + NOTCH),
        PARCHMENT,
    );
    for off in 0..2 {
        cv.line(Point::new(x0 + W - NOTCH - 1 + off, y0), Point::new(x0 + W - 1, y0 + NOTCH - off), INK);
    }
    // The contact stripes, clear of the notch.
    for i in 0..4 {
        cv.vline(x0 + 4 + i * 4, y0 + 2, 6, 2, INK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four shipped languages — every catalogued fault line must fit in each (the card renders
    /// English pre-app today, but the catalog columns are real translations and must not clip if a
    /// future boot path learns the language).
    const LANGS: [Language; 4] = [Language::En, Language::De, Language::Fr, Language::Es];

    /// Every fault has a short, non-empty, distinct title plus the what/fix pair — the plain-copy
    /// contract (a rider must read what's wrong and what to do without the log), across all four
    /// catalog columns.
    #[test]
    fn copy_is_present_and_distinct() {
        let all = [BootFault::NoCard, BootFault::NoMap, BootFault::BadMap];
        let keys = [
            (Msg::FaultNocardWhat, Msg::FaultNocardFix),
            (Msg::FaultNomapWhat, Msg::FaultNomapFix),
            (Msg::FaultBadmapWhat, Msg::FaultBadmapFix),
        ];
        for (f, (what_key, fix_key)) in all.into_iter().zip(keys) {
            let (title, what, fix) = f.copy();
            assert!(!title.is_empty() && !what.is_empty() && !fix.is_empty(), "{f:?} has empty copy");
            // Titles fit the wood bar (short, all-caps house style).
            assert!(title.len() <= 16, "{f:?} title too long for the bar: {title:?}");
            for lang in LANGS {
                let (what, fix) = (t(what_key, lang), t(fix_key, lang));
                // The *what* line draws unwrapped at Font::Body, centred on the 240 px panel:
                // 16 cells = 224 px is the safe budget (measured on glass: 18 chars touched the
                // border) — this pins the fix so a future copy edit can't silently clip.
                assert!(what.chars().count() <= 16, "{f:?}/{lang:?} what-line too wide: {what:?}");
                // The *fix* line word-wraps at Font::Label within `w - 40` (16 cells): no single
                // word may overflow a line, and the whole line stays short enough for the card.
                for word in fix.split(' ') {
                    assert!(word.chars().count() <= 16, "{f:?}/{lang:?} fix word too wide: {word:?}");
                }
                assert!(fix.chars().count() <= 56, "{f:?}/{lang:?} fix too long for the card: {fix:?}");
            }
            // The English column is what the pre-app card actually renders.
            assert_eq!((what, fix), (t(what_key, Language::En), t(fix_key, Language::En)));
        }
        // Distinct titles so the three fatal sites are told apart on glass.
        assert_ne!(BootFault::NoCard.copy().0, BootFault::NoMap.copy().0);
        assert_ne!(BootFault::NoMap.copy().0, BootFault::BadMap.copy().0);
    }
}
