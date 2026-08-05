//! The About page (issue #1149) — reached from the System menu's *About* row. The device's one
//! credits surface: the map data's OpenStreetMap attribution + ODbL notice, the terrain layer's
//! Copernicus credit, and the firmware's own licence + source pointer.
//!
//! Why this page exists at all: the rendered map is a *Produced Work* under the ODbL, and the
//! OSMF's attribution guidelines require a notice that the data came from OpenStreetMap **and**
//! that it is available under the ODbL. For a device that is offline by design the licence half
//! cannot be discharged with a link — the statement has to ship on the glass. Their guidelines
//! class GPS units as mobile devices, where attribution behind one deliberate interaction (an
//! about/info page) is acceptable; System → About is exactly that, and it matches where every
//! other bike computer keeps its legal page.
//!
//! The copy is **hand-wrapped constant lines**, not runtime-wrapped text: the legal formulas are
//! fixed strings, so pre-wrapping makes the exact on-glass layout reviewable in the source, and
//! the tests below enforce the two properties that matter — every line fits the panel width
//! (the never-ellipsize rule), and the Copernicus lines re-join to `obc_dem`'s canonical
//! [`COPERNICUS_ATTRIBUTION`] word for word (a dev-dependency, so the device build never sees the
//! host crate).
//!
//! The page is taller than the panel, so it **scrolls by line**: Rotate moves the window, Back
//! climbs out, and a right-edge scrollbar shows where you are. Press does nothing — there is
//! nothing to select on a credits page.

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::{palette, title_frame, Ctx, Render, Transition, LIST_TOP};
use crate::Msg;

use super::ROW_X;

/// Per-line vertical advance — [`Font::Label`] stacked the way the Firmware ledger stacks it.
const PITCH: i32 = 22;
/// Gap between the title bar and the first line.
const START_PAD: i32 = 16;
/// Room under the last visible line's full glyph cell (a Label cell is 24 px, 2 px more than
/// [`PITCH`]), so the bottom row never kisses the panel frame.
const BOTTOM_PAD: i32 = 14;
/// Wrapped-line budget in [`Font::Label`] characters: the panel is 240 px wide, text starts at
/// [`ROW_X`] with the same right margin, and a Label cell is 12 px — `(240 - 2·14) / 12 = 17`.
/// The width test below holds every content line and every caption translation to it.
#[cfg(test)]
const LINE_CHARS: usize = 17;

/// `© OpenStreetMap contributors` + the ODbL notice + where the licence lives, pre-wrapped.
/// The wording is the OSMF's requested credit; not translated — legal formulas stay canonical.
const OSM_LINES: &[&str] =
    &["\u{00a9} OpenStreetMap", "contributors", "Open Database", "License (ODbL)", "openstreetmap", ".org/copyright"];

/// `obc_dem::COPERNICUS_ATTRIBUTION`, pre-wrapped. The parity test re-joins these with single
/// spaces and compares against the host crate's const, so the wording cannot drift and the wraps
/// can only fall on word boundaries.
const COPERNICUS_LINES: &[&str] = &[
    "produced using",
    "Copernicus",
    "WorldDEM-30 \u{00a9} DLR",
    "e.V. 2010-2014",
    "and \u{00a9} Airbus",
    "Defence and Space",
    "GmbH 2014-2018",
    "provided under",
    "COPERNICUS by the",
    "European Union",
    "and ESA; all",
    "rights reserved",
];

/// The firmware's own licence and where the source lives (the GPL-3.0 §6 source pointer).
const FIRMWARE_LINES: &[&str] = &["GPL-3.0", "github.com/", "timohueser/", "OpenBikeComputer"];

/// The three credit sections: a translated caption over untranslated pre-wrapped lines.
const SECTIONS: [(Msg, &[&str]); 3] =
    [(Msg::AboutMapData, OSM_LINES), (Msg::AboutElevation, COPERNICUS_LINES), (Msg::AboutFirmware, FIRMWARE_LINES)];

/// Total virtual lines: each section is a caption + its lines, with one blank line between
/// sections (not after the last).
const TOTAL_LINES: usize = 3 + OSM_LINES.len() + COPERNICUS_LINES.len() + FIRMWARE_LINES.len() + (SECTIONS.len() - 1);

/// Lines that fit the 320 px panel below the title bar. The panel height reaches `draw` as
/// `rx.h`, but `handle` has no canvas — so the clamp uses this constant, and the
/// [`visible_matches_draw_geometry`](tests::visible_matches_draw_geometry) test pins it to the
/// same formula `draw` windows with.
const VISIBLE_LINES: usize = ((320 - LIST_TOP - START_PAD - BOTTOM_PAD) / PITCH) as usize;

/// The furthest the window may scroll — the last page exactly fills the panel.
const MAX_OFFSET: usize = TOTAL_LINES.saturating_sub(VISIBLE_LINES);

/// The About page. State is the first visible virtual line.
#[derive(Debug, Default)]
pub struct AboutScreen {
    offset: usize,
}

impl AboutScreen {
    pub fn new() -> Self {
        AboutScreen { offset: 0 }
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => {
                let next = self.offset as i32 + n;
                self.offset = next.clamp(0, MAX_OFFSET as i32) as usize;
                Transition::None
            }
            Gesture::Back => Transition::Pop, // climb back to the System menu
            Gesture::Press | Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::AboutTitle), "");

        let visible = ((h - LIST_TOP - START_PAD - BOTTOM_PAD) / PITCH).max(1) as usize;
        let start = self.offset.min(TOTAL_LINES.saturating_sub(visible));

        // Walk the virtual lines (caption, texts, gap, …), drawing the `[start, start+visible)`
        // window. A gap draws nothing; captions are the ledger's olive, content is ink.
        let mut virt = 0usize;
        let mut shown = 0usize;
        let mut y = LIST_TOP + START_PAD;
        for (i, (caption, lines)) in SECTIONS.iter().enumerate() {
            // The caption line.
            if virt >= start && shown < visible {
                cv.text(rx.t(*caption), Point::new(ROW_X, y), Font::Label, TextAlign::Left, SUBTEXT);
                y += PITCH;
                shown += 1;
            }
            virt += 1;
            // The section's content lines.
            for line in *lines {
                if virt >= start && shown < visible {
                    cv.text(line, Point::new(ROW_X, y), Font::Label, TextAlign::Left, INK);
                    y += PITCH;
                    shown += 1;
                }
                virt += 1;
            }
            // One blank line between sections.
            if i + 1 < SECTIONS.len() {
                if virt >= start && shown < visible {
                    y += PITCH;
                    shown += 1;
                }
                virt += 1;
            }
        }

        // Right-edge scrollbar: a hairline track with a proportional thumb, only when there is
        // more than one page — a static page needs no position cue.
        if TOTAL_LINES > visible {
            let track_top = LIST_TOP + 4;
            let track_h = h - track_top - BOTTOM_PAD;
            cv.fill(rect(w - 9, track_top, 3, track_h), RULE);
            let thumb_h = (track_h * visible as i32 / TOTAL_LINES as i32).max(16);
            let travel = track_h - thumb_h;
            let thumb_y = track_top + travel * start as i32 / MAX_OFFSET.max(1) as i32;
            cv.fill(rect(w - 9, thumb_y, 3, thumb_h), INK);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Language;
    use crate::AppState;

    fn run(scr: &mut AboutScreen, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut s = crate::Settings::default();
        let mut act = crate::activity::Activity::new(crate::Mode::Idle);
        let scratch = crate::screen::PoiScratch::new();
        let mut cx = Ctx {
            state: &mut st,
            activity: &mut act,
            settings: &mut s,
            routes: &[],
            rides: &[],
            trips: &[],
            nav_profiles: &crate::NavProfiles::EMPTY,
            poi_scratch: &scratch,
            waypoints: &[],
            corridor: &[],
            sensor_scan_hits: &[],
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// Scrolling clamps at both ends; Back pops; Press does nothing.
    #[test]
    fn scroll_clamps_and_back_pops() {
        let mut scr = AboutScreen::new();
        assert!(matches!(run(&mut scr, Gesture::Step(-3)), Transition::None));
        assert_eq!(scr.offset, 0, "cannot scroll above the top");
        for _ in 0..100 {
            run(&mut scr, Gesture::Step(1));
        }
        assert_eq!(scr.offset, MAX_OFFSET, "scrolling stops at the last page");
        assert!(matches!(run(&mut scr, Gesture::Press), Transition::None));
        assert!(matches!(run(&mut scr, Gesture::Back), Transition::Pop));
    }

    /// The Copernicus lines re-join to `obc_dem`'s canonical attribution word for word — the
    /// single-copy-of-the-wording rule, held across the firmware/host boundary by a dev-dep the
    /// device build never sees. If the wording ever changes in `obc-dem`, this fails here.
    #[test]
    fn copernicus_wording_matches_obc_dem() {
        let mut joined = std::string::String::new();
        for (i, line) in COPERNICUS_LINES.iter().enumerate() {
            if i > 0 {
                joined.push(' ');
            }
            joined.push_str(line);
        }
        assert_eq!(joined, obc_dem::COPERNICUS_ATTRIBUTION);
    }

    /// Every pre-wrapped content line fits the Label-width budget, and every caption does so in
    /// all four languages — the never-ellipsize rule made testable.
    #[test]
    fn every_line_fits_the_panel() {
        for (caption, lines) in SECTIONS {
            for lang in [Language::En, Language::De, Language::Fr, Language::Es] {
                let text = crate::i18n::t(caption, lang);
                assert!(text.chars().count() <= LINE_CHARS, "caption {text:?} ({lang:?}) exceeds {LINE_CHARS} chars");
            }
            for line in lines {
                assert!(line.chars().count() <= LINE_CHARS, "line {line:?} exceeds {LINE_CHARS} chars");
            }
        }
    }

    /// `handle`'s clamp constant and `draw`'s window formula describe the same 320 px panel. If
    /// the panel geometry ever changes, this is the seam that notices.
    #[test]
    fn visible_matches_draw_geometry() {
        let h = 320;
        assert_eq!(VISIBLE_LINES, ((h - LIST_TOP - START_PAD - BOTTOM_PAD) / PITCH) as usize);
        // The last visible line's full 24 px glyph cell stays inside the panel.
        let last_line_bottom = LIST_TOP + START_PAD + (VISIBLE_LINES as i32 - 1) * PITCH + 24;
        assert!(last_line_bottom <= h, "bottom line would clip: ends at {last_line_bottom} in a {h} px panel");
        assert!(TOTAL_LINES > VISIBLE_LINES, "the page scrolls; if it stopped scrolling, drop the scrollbar");
    }
}
