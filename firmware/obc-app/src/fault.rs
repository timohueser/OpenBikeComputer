//! Full-screen storage and map faults at boot. These render without an [`App`], because no map
//! has mounted. They cannot be dismissed, but USB recovery can replace them with transfer status.
//!
//! A host draws [`draw_boot_fault`] for a static fault or uses [`BootRecovery`] while USB can
//! replace the map. The copy is a parallel two-line family: the ink line says what is
//! wrong, the olive line says the fix, and no jargon appears on either.
//!
//! It is kept in `obc-app` rather than the board crate, so the simulator draws the identical screen
//! and the copy is unit-tested here.

use embedded_graphics::{draw_target::DrawTarget, prelude::Point};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, Surface,
};

use crate::screen::palette;
use crate::screen::vocab::chrome::{title_frame, wrapped, TITLE_BAR_H};
use crate::settings::Language;
use crate::{t, Msg};

/// A storage fault before the app exists: a card that will not mount, no map object, or a map
/// that fails [`obc_reader`]'s header parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootFault {
    /// Card identification never completed: no card, an unpowered socket, or a broken bus. It is
    /// the narrow class — the host came up and the card did not answer. A storage subsystem that
    /// never got as far as asking is [`StorageFault`](Self::StorageFault), because telling a rider
    /// their card is missing when the reader never started sends them to the wrong place.
    NoCard,
    /// The card answered but is not an SDHC or SDXC card. SDSC is byte-addressed and caps at 2 GB,
    /// which no map this device stores fits, so it is rejected outright. It is distinct from
    /// [`NoCard`](Self::NoCard) because the card is present and working, just too small.
    CardUnsupported,
    /// The storage subsystem itself failed: the sEMMC soft peripheral would not boot, a barrier
    /// never echoed, or the flat store would not mount.
    StorageFault,
    /// The card mounted but holds no map object.
    NoMap,
    /// A map object is present but is not valid OBCM.
    BadMap,
}

impl BootFault {
    /// The card's `(title, what, fix)`: the wood-bar title, the ink what-is-wrong line, and the
    /// olive fix line, which is word-wrapped at draw time.
    ///
    /// The what and fix pair lives in the Msg catalog, but a boot fault is drawn before `App` and
    /// `Settings` exist, so there is no `settings.language` to read this early. It renders the
    /// English column in every language build.
    pub fn copy(self) -> (&'static str, &'static str, &'static str) {
        const EN: Language = Language::En;
        match self {
            BootFault::NoCard => ("NO SD CARD", t(Msg::FaultNocardWhat, EN), t(Msg::FaultNocardFix, EN)),
            BootFault::CardUnsupported => {
                ("CARD UNSUPPORTED", t(Msg::FaultCardtypeWhat, EN), t(Msg::FaultCardtypeFix, EN))
            }
            BootFault::StorageFault => {
                ("STORAGE FAULT", t(Msg::FaultStoragefaultWhat, EN), t(Msg::FaultStoragefaultFix, EN))
            }
            BootFault::NoMap => ("NO MAP", t(Msg::FaultNomapWhat, EN), t(Msg::FaultNomapFix, EN)),
            BootFault::BadMap => ("MAP UNREADABLE", t(Msg::FaultBadmapWhat, EN), t(Msg::FaultBadmapFix, EN)),
        }
    }
}

/// The boot fault gives way to USB progress and the result, without a mounted map.
#[derive(Debug)]
pub struct BootRecovery {
    fault: BootFault,
    transfer: Option<crate::screen::MapTransfer>,
}

impl BootRecovery {
    pub fn new(fault: BootFault) -> Self {
        Self { fault, transfer: None }
    }

    /// Return whether the visible state changed. Cancellation restores the boot fault.
    pub fn update(&mut self, transfer: Option<crate::screen::MapTransfer>) -> bool {
        let changed = self.transfer != transfer;
        self.transfer = transfer;
        changed
    }

    pub fn restart_requested(&self, gesture: crate::Gesture) -> bool {
        self.transfer == Some(crate::screen::MapTransfer::Installed) && gesture == crate::Gesture::Press
    }

    pub fn draw<D, F>(&self, target: &mut D, w: i32, h: i32, color_fn: F)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let Some(transfer) = self.transfer else {
            draw_boot_fault(target, w, h, color_fn, self.fault);
            return;
        };
        let mut cv = Canvas::new(target, &color_fn);
        transfer.draw(&mut cv, w, h, Language::En);
        if transfer == crate::screen::MapTransfer::Installed {
            cv.text("Press to restart", Point::new(w / 2, h - 32), Font::Label, TextAlign::Center, palette::INK);
        }
    }
}

/// Draw a full-screen boot fault into `target`, with no [`App`](crate::App): the shared wood frame,
/// the SD-card glyph, and the centred what and fix pair. Push it once and hold it; it draws no
/// animation and expects no input.
pub fn draw_boot_fault<D, F>(target: &mut D, w: i32, h: i32, color_fn: F, fault: BootFault)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    use palette::*;
    let mut cv = Canvas::new(target, &color_fn);
    let (title, what, fix) = fault.copy();
    title_frame(&mut cv, w, h, title, "");
    sd_card_glyph(&mut cv, Point::new(w / 2, TITLE_BAR_H + 56));
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

    #[test]
    fn recovery_shows_transfer_results_and_only_restarts_after_success() {
        use crate::screen::{MapTransfer, MapTransferError};
        use crate::Gesture;

        let mut panel = crate::harness::support::Buf::new(240, 320);
        let mut text = |recovery: &BootRecovery| {
            obc_render::text_tap::record(|| {
                recovery.draw(&mut panel, 240, 320, |color| {
                    let (r, g, b) = obc_reader::rgb565_to_rgb888(color);
                    embedded_graphics::pixelcolor::Rgb888::new(r, g, b)
                });
            })
            .into_iter()
            .map(|draw| draw.text)
            .collect::<Vec<_>>()
            .join(" ")
        };
        let mut recovery = BootRecovery::new(BootFault::BadMap);
        assert!(text(&recovery).contains("MAP UNREADABLE"));
        assert!(!recovery.restart_requested(Gesture::Press));

        let receiving = Some(MapTransfer::Receiving { received_kib: 50, total_kib: 100 });
        assert!(recovery.update(receiving));
        let progress = text(&recovery);
        assert!(progress.contains("Receiving map") && progress.contains("50 %"));
        assert!(!progress.contains("MAP UNREADABLE"));
        assert!(!recovery.restart_requested(Gesture::Press));
        assert!(!recovery.update(receiving), "unchanged progress needs no redraw");

        assert!(recovery.update(None));
        assert!(text(&recovery).contains("MAP UNREADABLE"), "cancellation restores the fault");
        for error in
            [MapTransferError::Storage, MapTransferError::Damaged, MapTransferError::NotAMap, MapTransferError::Refused]
        {
            recovery.update(Some(MapTransfer::Failed(error)));
            assert!(text(&recovery).contains("Map not stored"));
            assert!(!recovery.restart_requested(Gesture::Press));
        }

        recovery.update(receiving);
        recovery.update(Some(MapTransfer::Installed));
        let success = text(&recovery);
        assert!(success.contains("Map installed") && success.contains("Press to restart"));
        assert!(!success.contains("MAP UNREADABLE"));
        assert!(recovery.restart_requested(Gesture::Press));
        assert!(!recovery.restart_requested(Gesture::Back));
        recovery.update(receiving);
        assert!(!recovery.restart_requested(Gesture::Press), "a retry disables restart");
    }

    /// The four shipped languages. Every catalogued fault line must fit in each, although the card
    /// renders English before the app exists.
    const LANGS: [Language; 4] = [Language::En, Language::De, Language::Fr, Language::Es];

    #[test]
    fn copy_is_present_and_distinct() {
        let all = [
            BootFault::NoCard,
            BootFault::CardUnsupported,
            BootFault::StorageFault,
            BootFault::NoMap,
            BootFault::BadMap,
        ];
        let keys = [
            (Msg::FaultNocardWhat, Msg::FaultNocardFix),
            (Msg::FaultCardtypeWhat, Msg::FaultCardtypeFix),
            (Msg::FaultStoragefaultWhat, Msg::FaultStoragefaultFix),
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
                // The what line draws unwrapped at Font::Body on the 240 px panel, where 16 cells
                // is the safe budget: 18 chars touched the border on glass.
                assert!(what.chars().count() <= 16, "{f:?}/{lang:?} what-line too wide: {what:?}");
                // The fix line word-wraps at Font::Label within `w - 40`, so no single word may
                // overflow a line.
                for word in fix.split(' ') {
                    assert!(word.chars().count() <= 16, "{f:?}/{lang:?} fix word too wide: {word:?}");
                }
                assert!(fix.chars().count() <= 56, "{f:?}/{lang:?} fix too long for the card: {fix:?}");
            }
            // The English column is what the pre-app card renders.
            assert_eq!((what, fix), (t(what_key, Language::En), t(fix_key, Language::En)));
        }
        // Distinct titles, so every fatal site is told apart on glass: a reader that never started
        // and a 2 GB card must not both read as "NO SD CARD".
        let titles: heapless::Vec<&str, 8> = all.into_iter().map(|f| f.copy().0).collect();
        for (i, a) in titles.iter().enumerate() {
            for b in titles.iter().skip(i + 1) {
                assert_ne!(a, b, "two boot faults share a title: {a:?}");
            }
        }
    }
}
