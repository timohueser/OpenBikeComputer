//! The About page: the credits for the map data, the terrain layer and the firmware.
//!
//! The rendered map is a Produced Work under the ODbL, and the device is offline, so the notice
//! must be on the glass. The copy is hand-wrapped constant lines, so the on-glass layout is
//! reviewable in the source. The page is taller than the panel, so it scrolls by line.

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::vocab::chrome::{title_frame, LIST_TOP};
use crate::screen::{palette, Ctx, Render, Transition};
use crate::Msg;

use crate::screen::vocab::rows::ROW_X;

/// Per-line vertical advance for [`Font::Label`].
const PITCH: i32 = 22;
const START_PAD: i32 = 16;
/// Room under the last line. A Label cell is 24 px, which is 2 px more than [`PITCH`].
const BOTTOM_PAD: i32 = 14;
/// Line budget in [`Font::Label`] characters: `(240 - 2·14) / 12 = 17` for a 240 px panel.
#[cfg(test)]
const LINE_CHARS: usize = 17;

/// The OSMF requested credit, pre-wrapped. Legal formulas are not translated.
const OSM_LINES: &[&str] =
    &["\u{00a9} OpenStreetMap", "contributors", "Open Database", "License (ODbL)", "openstreetmap", ".org/copyright"];

/// `obc_elevation::COPERNICUS_ATTRIBUTION`, pre-wrapped. A test re-joins the lines with single
/// spaces and compares them with that constant, so the wording cannot drift.
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

/// The firmware licence and the source pointer it requires.
const FIRMWARE_LINES: &[&str] = &["GPL-3.0", "github.com/", "timohueser/", "OpenBikeComputer"];

/// The three credit sections: a translated caption over untranslated pre-wrapped lines.
const SECTIONS: [(Msg, &[&str]); 3] =
    [(Msg::AboutMapData, OSM_LINES), (Msg::AboutElevation, COPERNICUS_LINES), (Msg::AboutFirmware, FIRMWARE_LINES)];

/// Total virtual lines: a caption and its lines per section, with a blank line between sections.
const TOTAL_LINES: usize = 3 + OSM_LINES.len() + COPERNICUS_LINES.len() + FIRMWARE_LINES.len() + (SECTIONS.len() - 1);

/// Lines that fit the 320 px panel below the title bar. `handle` has no canvas, so its clamp uses
/// this constant. A test pins it to the formula `draw` uses.
const VISIBLE_LINES: usize = ((320 - LIST_TOP - START_PAD - BOTTOM_PAD) / PITCH) as usize;

/// The furthest the window may scroll. The last page fills the panel exactly.
const MAX_OFFSET: usize = TOTAL_LINES.saturating_sub(VISIBLE_LINES);

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
            Gesture::Back => Transition::Pop,
            Gesture::Press | Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::AboutTitle), "");

        let visible = ((h - LIST_TOP - START_PAD - BOTTOM_PAD) / PITCH).max(1) as usize;
        let start = self.offset.min(TOTAL_LINES.saturating_sub(visible));

        // Walk the virtual lines and draw the `[start, start + visible)` window.
        let mut virt = 0usize;
        let mut shown = 0usize;
        let mut y = LIST_TOP + START_PAD;
        for (i, (caption, lines)) in SECTIONS.iter().enumerate() {
            if virt >= start && shown < visible {
                cv.text(rx.t(*caption), Point::new(ROW_X, y), Font::Label, TextAlign::Left, SUBTEXT);
                y += PITCH;
                shown += 1;
            }
            virt += 1;
            for line in *lines {
                if virt >= start && shown < visible {
                    cv.text(line, Point::new(ROW_X, y), Font::Label, TextAlign::Left, INK);
                    y += PITCH;
                    shown += 1;
                }
                virt += 1;
            }
            if i + 1 < SECTIONS.len() {
                if virt >= start && shown < visible {
                    y += PITCH;
                    shown += 1;
                }
                virt += 1;
            }
        }

        // The scrollbar shows only when the page has more than one screen of lines.
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
        let mut cx = crate::screen::test_ctx(&mut st, &mut act, &mut s);
        scr.handle(g, &mut cx)
    }

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

    #[test]
    fn copernicus_wording_matches_obc_elevation() {
        let mut joined = std::string::String::new();
        for (i, line) in COPERNICUS_LINES.iter().enumerate() {
            if i > 0 {
                joined.push(' ');
            }
            joined.push_str(line);
        }
        assert_eq!(joined, obc_elevation::COPERNICUS_ATTRIBUTION);
    }

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

    #[test]
    fn visible_matches_draw_geometry() {
        let h = 320;
        assert_eq!(VISIBLE_LINES, ((h - LIST_TOP - START_PAD - BOTTOM_PAD) / PITCH) as usize);
        let last_line_bottom = LIST_TOP + START_PAD + (VISIBLE_LINES as i32 - 1) * PITCH + 24;
        assert!(last_line_bottom <= h, "bottom line would clip: ends at {last_line_bottom} in a {h} px panel");
        let walked = SECTIONS.iter().map(|(_, lines)| 1 + lines.len()).sum::<usize>() + SECTIONS.len() - 1;
        assert_eq!(walked, TOTAL_LINES);
        assert!(walked > VISIBLE_LINES, "the page scrolls; if it stopped scrolling, drop the scrollbar");
    }
}
