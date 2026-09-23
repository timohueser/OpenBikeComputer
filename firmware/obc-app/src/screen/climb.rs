//! The Climb screen: the current climb's elevation profile, drawn with one grade-coloured stripe
//! per chart column, a "you are here" cursor, a progress bar, and four climb-scoped tiles. All the
//! drawn data comes from the frame's [`ActiveClimb`], so the screen holds no state of its own.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use super::vocab::chrome::{empty_state, title_frame};
use super::vocab::fmt::{distance_figure, integer, percent};
use super::vocab::tiles::tile;
use crate::input::Gesture;
use crate::screen::ActiveClimb;
use crate::settings::{Language, Units};
use crate::{t, Msg};

use super::{palette, Ctx, MapScreen, Render, Screen, Transition};

// Chart geometry (px) for the 240×320 panel. Taller than the Statistics band: only four tiles sit
// below it.
const CHART_TOP: i32 = 42;
const CHART_BOT: i32 = 168;
/// The summit maps here (a few px below `CHART_TOP`) so the apex clears the cursor's top.
const BAND_TOP: i32 = CHART_TOP + 4;
const SIDE_MARGIN: i32 = 12;

/// Right inset (px) of the title bar readout slot. It mirrors `title_frame_ble`, so the summit
/// glyph lands directly left of the right-aligned elevation.
const TITLE_RIGHT_INSET: i32 = 14;
/// Vertical centre (px) of the title bar text line, where the summit glyph centres. The Label face
/// inks its digits in rows 14..28, and the glyph spans `cy-7..cy+6`, so `22` aligns the two.
const TITLE_TEXT_CY: i32 = 22;
/// Gap (px) between the summit glyph and the elevation number it prefixes.
const SUMMIT_FLAG_GAP: i32 = 4;

/// Map a local grade % to its stripe colour: green `< 3 %`, yellow `3–6 %`, amber `6–9 %`, orange
/// `9–12 %`, red `> 12 %`. A negative grade falls in the green band.
pub(super) fn grade_color(grade_pct: i32) -> u16 {
    use palette::*;
    match grade_pct {
        i32::MIN..3 => ON,
        3..6 => YELLOW,
        6..9 => AMBER,
        9..12 => WARNING,
        _ => RED,
    }
}

#[derive(Debug, Default)]
pub struct ClimbScreen;

impl ClimbScreen {
    pub fn new() -> Self {
        ClimbScreen
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // The last hop of the Back-cycle: Map → Statistics → Climb → Map.
            Gesture::Back => Transition::Replace(Screen::Map(MapScreen::new())),
            Gesture::Press => super::riding_common(g, cx),
            Gesture::BackHold => Transition::None,
            Gesture::Step(_) | Gesture::Hold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        // The screen is only meaningful with an active climb. Without one, draw a placeholder.
        let Some(climb) = rx.climb else {
            title_frame(cv, w, h, rx.t(Msg::ClimbTitle), "");
            empty_state(cv, w, h, rx.t(Msg::ClimbNoClimb), rx.t(Msg::ClimbNotOnClimb));
            return;
        };
        let ActiveClimb { seg, profile } = climb;
        let units = rx.settings.units;

        let cursor_frac = profile.cursor_frac(rx.navigation.progress_m);

        let mut readout: heapless::String<16> = heapless::String::new();
        let _ = write!(readout, "{} {}", units.elev(seg.top_ele_m as f32) as i32, units.elev_label());
        title_frame(cv, w, h, rx.t(Msg::ClimbTitle), &readout);
        // The summit-flag glyph left of the elevation, so the figure reads as the summit height
        // without a "top" label.
        let readout_left = (w - TITLE_RIGHT_INSET) - text_width(&readout, Font::Label) as i32;
        summit_glyph(cv, readout_left - SUMMIT_FLAG_GAP, TITLE_TEXT_CY, BAR_TEXT);

        // Elevation maps to y over the climb's own base..summit span, so a small climb still fills
        // the chart. `.max(1)` guards a flat seg.
        let base = profile.base_ele_m();
        let top = profile.top_ele_m();
        let span_ele = (top - base).max(1) as f32;
        let ele_to_y = |e: i16| -> i32 {
            let t = ((e - base) as f32 / span_ele).clamp(0.0, 1.0);
            CHART_BOT - (t * (CHART_BOT - BAND_TOP) as f32) as i32
        };

        let chart_x = SIDE_MARGIN;
        let chart_w = w - 2 * SIDE_MARGIN;
        let cols = profile.cols();
        let n_cols = cols.len().max(1) as i32;
        let frac_to_x = |f: f32| chart_x + (f.clamp(0.0, 1.0) * chart_w as f32) as i32;

        // One vertical stripe per chart pixel column, filled baseline to profile height in the
        // colour of the local grade.
        for px in 0..chart_w {
            let f = px as f32 / chart_w as f32;
            let col = ((f * (n_cols - 1) as f32) as usize).min(cols.len() - 1);
            let top_y = ele_to_y(cols[col]);
            let x = chart_x + px;
            let color = grade_color(profile.grade_at(f));
            cv.vline(x, top_y, CHART_BOT - top_y + 1, 1, color);
            // Ink the top pixel of the done columns, so the crest line shows travelled vs. ahead.
            // A hairline only, so the grade band is still visible.
            if f <= cursor_frac {
                cv.vline(x, top_y, 1, 1, INK);
            }
        }
        cv.hline(chart_x, CHART_BOT + 1, chart_w, RULE);

        let cursor_x = frac_to_x(cursor_frac).clamp(chart_x, chart_x + chart_w - 1);
        let cur_y = ele_to_y(profile.at(cursor_frac));
        cv.vline(cursor_x, CHART_TOP, CHART_BOT - CHART_TOP + 1, 2, AMBER);
        cv.disc(Point::new(cursor_x, cur_y), 4, INK);
        cv.disc(Point::new(cursor_x, cur_y), 3, AMBER);

        let prog_y = CHART_BOT + 10;
        cv.round(rect(chart_x, prog_y, chart_w, 8), 4, PARCHMENT_SHADE);
        let fill_w = (chart_w as f32 * cursor_frac) as i32;
        if fill_w > 0 {
            cv.round(rect(chart_x, prog_y, fill_w, 8), 4, AMBER);
        }

        let gap = 6;
        let col_w = (chart_w - gap) / 2;
        let grid_top = prog_y + 16;
        let row_h = ((h - 10 - grid_top - gap) / 2).max(20);
        let cells = climb_tiles(&climb, rx.navigation.progress_m, units, rx.settings.language);
        for (i, cell) in cells.iter().enumerate() {
            let (r, c) = (i / 2, i % 2);
            let x = chart_x + c as i32 * (col_w + gap);
            let y = grid_top + r as i32 * (row_h + gap);
            tile(
                cv,
                rect(x, y, col_w, row_h),
                &rx.marquee,
                &cell.caption,
                &cell.value,
                cell.arrow,
                TextAlign::Left,
                CLIMB_TILE,
                SUBTEXT,
                INK,
            );
        }
    }
}

/// One climb tile: caption, number-only value, and the up-arrow flag the ascent tiles carry.
struct ClimbCell {
    caption: heapless::String<12>,
    value: heapless::String<8>,
    arrow: bool,
}

impl ClimbCell {
    fn new(caption: &str, value: heapless::String<8>, arrow: bool) -> Self {
        let mut cap = heapless::String::new();
        let _ = cap.push_str(caption);
        ClimbCell { caption: cap, value, arrow }
    }
}

/// Build the four climb tiles in 2×2 grid order, row-major: To climb, To top / Grade, Avg → top.
fn climb_tiles(climb: &ActiveClimb, progress_m: u32, units: Units, lang: Language) -> [ClimbCell; 4] {
    let ActiveClimb { seg, profile } = climb;
    let cursor = profile.cursor_frac(progress_m);
    [
        ClimbCell::new(
            t(Msg::ClimbToClimb, lang),
            integer(units.elev(to_climb_m(climb, progress_m) as f32) as u32),
            true,
        ),
        ClimbCell::new(
            &cap_dist(units, t(Msg::ClimbToGo, lang)),
            distance_figure(units.dist(to_top_m(seg, progress_m) as f32 / 1000.0)),
            false,
        ),
        ClimbCell::new(t(Msg::ClimbGrade, lang), percent(profile.grade_at(cursor)), false),
        // Average grade over the remainder of the climb. The caption is 8 characters, the most that
        // fits the half-width tile.
        ClimbCell::new(t(Msg::ClimbAvgGrad, lang), percent(avg_to_top_pct(climb, progress_m)), false),
    ]
}

/// Remaining ascent: the summit minus the elevation at the live cursor, clamped to 0 past the top.
fn to_climb_m(climb: &ActiveClimb, progress_m: u32) -> u32 {
    let cursor = climb.profile.cursor_frac(progress_m);
    (climb.profile.top_ele_m() as i32 - climb.profile.at(cursor) as i32).max(0) as u32
}

/// Remaining distance to the summit.
fn to_top_m(seg: &obc_route::ClimbSeg, progress_m: u32) -> u32 {
    seg.end_m.saturating_sub(progress_m)
}

/// The average grade (whole %) over the remainder of the climb. Within [`AVG_MIN_RUN_M`] of the
/// top it reads 0: the run is too short for a meaningful slope.
fn avg_to_top_pct(climb: &ActiveClimb, progress_m: u32) -> i32 {
    let run_m = to_top_m(climb.seg, progress_m);
    if run_m < AVG_MIN_RUN_M {
        return 0;
    }
    to_climb_m(climb, progress_m) as i32 * 100 / run_m as i32
}

/// Below this remaining distance (m) to the summit, [`avg_to_top_pct`] reads 0.
const AVG_MIN_RUN_M: u32 = 20;

/// The unit-prefixed "TO GO" caption (`KM TO GO` / `MI TO GO`): the unit lives in the caption, so
/// the big digits fit the half-width tile.
fn cap_dist(units: Units, tail: &str) -> heapless::String<12> {
    let mut s = heapless::String::new();
    let _ = s.push_str(units.dist_label());
    let _ = s.push_str(tail);
    s
}

/// Draw the summit-flag glyph in `color`, with its right edge at `right_x` and centred on `cy`.
fn summit_glyph(cv: &mut impl Surface, right_x: i32, cy: i32, color: u16) {
    let base_y = cy + 6;
    let apex_y = base_y - 7;
    let apex_x = right_x - 5;
    cv.triangle(Point::new(apex_x, apex_y), Point::new(apex_x - 5, base_y), Point::new(apex_x + 5, base_y), color);
    cv.vline(apex_x, apex_y - 6, 6, 1, color);
    cv.fill(rect(apex_x + 1, apex_y - 6, 4, 3), color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::test_ctx;
    use obc_route::{ClimbProfile, ClimbSeg};

    #[test]
    fn back_returns_to_map() {
        use crate::activity::Activity;
        use crate::{AppState, Mode, Settings};
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Riding);
        let mut s = Settings::default();
        let mut cx = test_ctx(&mut st, &mut act, &mut s);
        assert!(matches!(ClimbScreen::new().handle(Gesture::Back, &mut cx), Transition::Replace(Screen::Map(_))));
    }

    /// A synthetic climb: 1000 m at 5 000 m along the route to 1400 m at 9 000 m, a 10 % average.
    fn synthetic() -> (ClimbSeg, ClimbProfile) {
        let seg = ClimbSeg {
            start_m: 5_000,
            end_m: 9_000,
            base_ele_m: 1_000,
            top_ele_m: 1_400,
            gain_m: 400,
            avg_grade_pct: 10,
        };
        let profile = ClimbProfile::from_linear_ramp(&seg);
        (seg, profile)
    }

    #[test]
    fn to_climb_is_remaining_ascent() {
        let (seg, profile) = synthetic();
        let climb = ActiveClimb { seg: &seg, profile: &profile };
        assert!((to_climb_m(&climb, 5_000) as i32 - 400).abs() <= 4, "at the base ~400 m remain");
        assert!((to_climb_m(&climb, 7_000) as i32 - 200).abs() <= 6, "halfway ~200 m remain");
        assert_eq!(to_climb_m(&climb, 9_000), 0, "at the summit nothing remains");
        assert_eq!(to_climb_m(&climb, 12_000), 0, "past the summit clamps to 0, never negative");
    }

    #[test]
    fn to_top_is_remaining_distance() {
        let (seg, _) = synthetic();
        assert_eq!(to_top_m(&seg, 5_000), 4_000, "at the base the whole 4 km remains");
        assert_eq!(to_top_m(&seg, 7_500), 1_500, "1.5 km left at 7 500 m");
        assert_eq!(to_top_m(&seg, 9_000), 0, "at the summit 0");
        assert_eq!(to_top_m(&seg, 10_000), 0, "past the summit clamps to 0, never underflows");
    }

    #[test]
    fn grade_tile_reads_local_grade() {
        let (_seg, profile) = synthetic();
        let g = profile.grade_at(profile.cursor_frac(7_000));
        assert!((g - 10).abs() <= 2, "a 400 m / 4 000 m ramp reads ~10 %, got {g}");
    }

    #[test]
    fn avg_to_top_is_remaining_average_grade() {
        let (seg, profile) = synthetic();
        let climb = ActiveClimb { seg: &seg, profile: &profile };
        assert!((avg_to_top_pct(&climb, 5_000) - 10).abs() <= 2, "from the base ~10 %");
        assert!((avg_to_top_pct(&climb, 7_000) - 10).abs() <= 3, "halfway still ~10 %");
        assert_eq!(avg_to_top_pct(&climb, 8_990), 0, "inside the summit guard reads 0, no spike");
    }

    #[test]
    fn climb_tiles_assemble_in_grid_order() {
        let (seg, profile) = synthetic();
        let climb = ActiveClimb { seg: &seg, profile: &profile };
        let cells = climb_tiles(&climb, 7_000, Units::Metric, Language::En);
        assert_eq!(cells[0].caption.as_str(), "TO CLIMB");
        assert!(cells[0].arrow, "To climb is an ascent figure → up-arrow");
        assert_eq!(cells[1].caption.as_str(), "KM TO GO", "the distance tile prefixes the unit");
        assert!(!cells[1].arrow);
        assert_eq!(cells[2].caption.as_str(), "GRADE");
        assert!(cells[3].caption.as_str().starts_with("AVG"));
        assert_eq!(cells[1].value.as_str(), "2.0", "remaining distance formats km like Statistics");
    }

    #[test]
    fn climb_tiles_respect_imperial_units() {
        let (seg, profile) = synthetic();
        let climb = ActiveClimb { seg: &seg, profile: &profile };
        let cells = climb_tiles(&climb, 5_000, Units::Imperial, Language::En);
        assert_eq!(cells[1].caption.as_str(), "MI TO GO");
        assert_eq!(cells[1].value.as_str(), "2.5", "4 km reads 2.5 mi");
        let ft: i32 = cells[0].value.as_str().parse().unwrap();
        assert!((ft - 1312).abs() <= 20, "remaining ascent converts to feet, got {ft}");
    }

    #[test]
    fn grade_color_bands() {
        use palette::*;
        assert_eq!(grade_color(-5), ON, "a downhill dip is never steep → green");
        assert_eq!(grade_color(0), ON);
        assert_eq!(grade_color(2), ON, "just under 3 % is green");
        assert_eq!(grade_color(3), YELLOW, "3 % is the yellow band");
        assert_eq!(grade_color(5), YELLOW);
        assert_eq!(grade_color(6), AMBER, "6 % is the amber band");
        assert_eq!(grade_color(8), AMBER);
        assert_eq!(grade_color(9), WARNING, "9 % is the orange band");
        assert_eq!(grade_color(11), WARNING);
        assert_eq!(grade_color(12), RED, "12 % and up is red");
        assert_eq!(grade_color(25), RED);
    }
}
