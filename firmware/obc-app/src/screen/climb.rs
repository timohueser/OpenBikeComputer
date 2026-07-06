//! The Climb screen — the third riding view (epic #506, C4): the current climb's elevation profile
//! drawn ClimbPro-style, with per-column **grade-coloured stripes**, a "you are here" cursor, an
//! amber progress bar, and a 2×2 grid of four climb-scoped tiles.
//!
//! It's the sibling of the [`Statistics`](super::StatisticsScreen) view and mirrors its structure
//! (a chart band under the [`title_frame`] header, an `ele_to_y` / `frac_to_x` mapping, the cursor +
//! progress bar, the empty-state guard), but scoped to **one climb** rather than the whole route:
//! - The chart is **taller** — four fixed tiles buy the vertical room for a climb worth reading.
//! - Instead of a single amber band, each chart column is filled baseline→profile in its **local
//!   grade's colour** ([`grade_color`]), the Garmin-ClimbPro look.
//! - The grid is **four fixed tiles** — To climb / To top / Grade / Avg → top — not the
//!   customizable [`stat_fields`](crate::stat_fields) grid.
//!
//! All the drawn data comes from the resident [`ActiveClimb`](super::ActiveClimb) (`seg` + detail
//! `profile`) C3 threads into [`Render`], present exactly when a climb is being tracked; the live
//! cursor is [`ClimbProfile::cursor_frac`](obc_route::ClimbProfile::cursor_frac) of the route
//! progress. It owns no state — there's nothing to scrub or auto-cycle, so it needs no
//! [`tick_timers`](super::Screen::tick_timers) arm.
//!
//! Bindings reuse [`riding_common`](super::riding_common): `press` = pause → Ride control,
//! `back-hold` = Menu. `back` is the **last hop** of the conditional Back-cycle
//! (Map → Statistics → Climb → Map, C5) — a sibling move back to the Map. The Statistics screen
//! only routes here when a climb is active and [`ClimbMode`](crate::settings::ClimbMode) is on, so
//! this hop always closes the ring at the Map.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{rect, Surface};

use crate::input::Gesture;
use crate::screen::ActiveClimb;
use crate::settings::Units;

use super::{palette, tile, title_frame, Ctx, MapScreen, Render, Screen, Transition};

// Chart geometry (px), tuned for the 240×320 panel. Taller than Statistics' band (its chart is
// ~42→110) — the four tiles below leave the room the epic wants for a climb worth reading.
const CHART_TOP: i32 = 42;
const CHART_BOT: i32 = 168;
/// The summit maps here (a few px below `CHART_TOP`) so the apex clears the cursor's top.
const BAND_TOP: i32 = CHART_TOP + 4;
const SIDE_MARGIN: i32 = 12;

/// Grade-band → stripe colour (the ClimbPro ramp). Maps a **local** grade % to one of five
/// device-64 bands, hotter with steepness: green `< 3 %`, yellow `3–6 %`, amber `6–9 %`, orange
/// `9–12 %`, red `> 12 %`. Negative grades (an internal dip) fall in the green `< 3 %` band — a
/// give-back column is never "steep". Every returned colour is a pinned palette const, so the
/// stripes quantize exactly on glass.
fn grade_color(grade_pct: i32) -> u16 {
    use palette::*;
    match grade_pct {
        i32::MIN..3 => ON, // < 3 %  — green (also any downhill dip)
        3..6 => YELLOW,    // 3–6 %  — yellow
        6..9 => AMBER,     // 6–9 %  — amber
        9..12 => WARNING,  // 9–12 % — orange
        _ => RED,          // > 12 % — red
    }
}

/// The Climb / ClimbPro view. A unit struct: everything it draws comes from the frame's
/// [`ActiveClimb`], and there is no scrub / zoom / page state to hold.
#[derive(Debug, Default)]
pub struct ClimbScreen;

impl ClimbScreen {
    pub fn new() -> Self {
        ClimbScreen
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // The last hop of the Back-cycle: Map → Statistics → Climb → **Map**. Statistics only
            // routes here when a climb is active and ClimbMode is on, so closing back to the Map is
            // always correct (a crest that ends the climb auto-returns to the Map anyway, C5).
            Gesture::Back => Transition::Replace(Screen::Map(MapScreen::new())),
            // The two riding views' shared bindings (press → Ride control, back-hold → Menu).
            Gesture::Press | Gesture::BackHold => super::riding_common(g, cx),
            // No turn/hold behaviour — the climb view is a fixed readout, nothing to scrub.
            Gesture::Turn(_) | Gesture::Hold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        // The screen is only meaningful with an active climb (C5 gates entry to that). If it's ever
        // reached without one — a stray push, a climb that ended a frame before the repaint — draw a
        // safe placeholder instead of panicking, exactly like Statistics' no-route guard.
        let Some(climb) = rx.climb else {
            title_frame(cv, w, h, "CLIMB", "");
            super::empty_state(cv, w, h, "No climb", "Not on a climb");
            return;
        };
        let ActiveClimb { seg, profile } = climb;
        let units = rx.settings.units;

        // The live within-climb cursor from the route progress — column 0 is the base, the last the
        // summit; below the base clamps to 0.0, past the summit to 1.0.
        let cursor_frac = profile.cursor_frac(rx.activity.progress_m);

        // Header: "CLIMB" with the summit elevation in the right slot — just the number + unit
        // (e.g. "1762 m"). On a screen already titled CLIMB the figure reads as the summit without a
        // "top" label. (The climb's 1-based index `n / N` isn't cleanly reachable from `Render` — it
        // carries the active `ActiveClimb`, not the `Climbs` list length — and the summit height is a
        // meaningful, always-available climb figure regardless.)
        let mut readout: heapless::String<16> = heapless::String::new();
        let _ = write!(readout, "{} {}", units.elev(seg.top_ele_m as f32) as i32, units.elev_label());
        title_frame(cv, w, h, "CLIMB", &readout);

        // Elevation → y over the climb's own base..summit span (not the whole route's), so a small
        // climb still fills the chart. `.max(1)` guards a degenerate flat seg.
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

        // Grade-striped profile: one vertical stripe per chart pixel column, filled baseline→profile
        // height in the local grade's band colour. Each pixel column maps to its within-climb
        // fraction, reads that column's elevation for the stripe top, and the local grade there for
        // the colour. The done part (left of the cursor) is dimmed a shade toward ink so "where I am"
        // reads at a glance without a second pass.
        for px in 0..chart_w {
            let f = px as f32 / chart_w as f32;
            let col = ((f * (n_cols - 1) as f32) as usize).min(cols.len() - 1);
            let top_y = ele_to_y(cols[col]);
            let x = chart_x + px;
            let color = grade_color(profile.grade_at(f));
            cv.vline(x, top_y, CHART_BOT - top_y + 1, 1, color);
            // Subtle traveled shade: over-paint the done columns' top pixel with ink so the crest
            // line reads travelled vs. ahead. Kept to a hairline so the grade band still shows.
            if f <= cursor_frac {
                cv.vline(x, top_y, 1, 1, INK);
            }
        }
        cv.hline(chart_x, CHART_BOT + 1, chart_w, RULE); // baseline under the stripes

        // The "you are here" cursor: an amber rule from the header down to the baseline, with an
        // ink-ringed dot at the current elevation.
        let cursor_x = frac_to_x(cursor_frac).clamp(chart_x, chart_x + chart_w - 1);
        let cur_y = ele_to_y(profile.at(cursor_frac));
        cv.vline(cursor_x, CHART_TOP, CHART_BOT - CHART_TOP + 1, 2, AMBER);
        cv.disc(Point::new(cursor_x, cur_y), 4, INK);
        cv.disc(Point::new(cursor_x, cur_y), 3, AMBER);

        // Progress bar at the live within-climb fraction.
        let prog_y = CHART_BOT + 10;
        cv.round(rect(chart_x, prog_y, chart_w, 8), 4, PARCHMENT_SHADE);
        let fill_w = (chart_w as f32 * cursor_frac) as i32;
        if fill_w > 0 {
            cv.round(rect(chart_x, prog_y, fill_w, 8), 4, AMBER);
        }

        // The four fixed climb tiles in a 2×2 grid on the apricot background. Values are the
        // testable helpers below, formatted with the same `stat_fields` formatters + unit handling
        // the Statistics grid uses.
        let gap = 6;
        let col_w = (chart_w - gap) / 2;
        let grid_top = prog_y + 16;
        let row_h = ((h - 10 - grid_top - gap) / 2).max(20);
        let cells = climb_tiles(&climb, rx.activity.progress_m, units);
        for (i, cell) in cells.iter().enumerate() {
            let (r, c) = (i / 2, i % 2);
            let x = chart_x + c as i32 * (col_w + gap);
            let y = grid_top + r as i32 * (row_h + gap);
            tile(cv, rect(x, y, col_w, row_h), &cell.caption, &cell.value, cell.arrow, CLIMB_TILE);
        }
    }
}

/// One climb tile's rendered content — caption, number-only value, and the up-arrow flag (the two
/// ascent tiles carry the climb triangle, like Statistics' climb figures).
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

/// Build the four climb tiles for the current live position — the pure value logic, split out so it
/// unit-tests against a synthetic climb without a draw context. The order is the 2×2 grid,
/// row-major: To climb, To top / Grade, Avg → top.
fn climb_tiles(climb: &ActiveClimb, progress_m: u32, units: Units) -> [ClimbCell; 4] {
    let ActiveClimb { seg, profile } = climb;
    let cursor = profile.cursor_frac(progress_m);
    [
        ClimbCell::new("TO CLIMB", fmt_int(units.elev(to_climb_m(climb, progress_m) as f32) as u32), true),
        ClimbCell::new(
            &cap_dist(units, " TO GO"),
            crate::stat_fields::fmt_km(units.dist(to_top_m(seg, progress_m) as f32 / 1000.0)),
            false,
        ),
        ClimbCell::new("GRADE", fmt_pct(profile.grade_at(cursor)), false),
        // The epic's "Avg → top": average grade over the climb's remainder. Captioned "AVG GRAD"
        // (average gradient) — a monospace 8-char caption that fits the half-width tile, where the
        // 9-char "AVG AHEAD" (and "AVG→TOP", whose `→` renders `?` in the ASCII panel font) overshot.
        // Reads as the average gradient still to come, paired with the instantaneous "GRADE" beside it.
        ClimbCell::new("AVG GRAD", fmt_pct(avg_to_top_pct(climb, progress_m)), false),
    ]
}

/// **To climb** — remaining ascent on this climb: the summit minus the elevation at the live cursor,
/// clamped ≥ 0 (a cursor past the summit reads 0). The up-arrow ascent figure.
fn to_climb_m(climb: &ActiveClimb, progress_m: u32) -> u32 {
    let cursor = climb.profile.cursor_frac(progress_m);
    (climb.profile.top_ele_m() as i32 - climb.profile.at(cursor) as i32).max(0) as u32
}

/// **To top** — remaining distance to the summit: `end_m − progress_m`, clamped ≥ 0.
fn to_top_m(seg: &obc_route::ClimbSeg, progress_m: u32) -> u32 {
    seg.end_m.saturating_sub(progress_m)
}

/// **Avg → top** — the average grade (whole %) over the *remainder* of the climb: remaining ascent
/// over remaining distance ×100. Guarded near the summit — within [`AVG_MIN_RUN_M`] of the top the
/// run is too short for a meaningful slope, so it reads 0 rather than a divide-by-tiny spike.
fn avg_to_top_pct(climb: &ActiveClimb, progress_m: u32) -> i32 {
    let run_m = to_top_m(climb.seg, progress_m);
    if run_m < AVG_MIN_RUN_M {
        return 0;
    }
    to_climb_m(climb, progress_m) as i32 * 100 / run_m as i32
}

/// Below this remaining distance (m) to the summit, [`avg_to_top_pct`] reads 0 — the run is too
/// short to divide the give-back gain into a sane average (and the rider is essentially there).
const AVG_MIN_RUN_M: u32 = 20;

/// A grade figure for a tile: signed whole percent with a `%` suffix.
fn fmt_pct(pct: i32) -> heapless::String<8> {
    let mut s = heapless::String::new();
    let _ = write!(s, "{pct}%");
    s
}

/// An integer figure (remaining ascent) as plain digits — mirrors `stat_fields`' climb formatting.
fn fmt_int(m: u32) -> heapless::String<8> {
    let mut s = heapless::String::new();
    let _ = write!(s, "{m}");
    s
}

/// The "TO GO" caption, unit-prefixed (`KM TO GO` / `MI TO GO`) so the distance unit lives in the
/// caption and the big digits fit the half-width tile — the same idiom as `stat_fields`' distance
/// tiles. (`stat_fields::cap` is private, so the glue is duplicated here rather than exported.)
fn cap_dist(units: Units, tail: &str) -> heapless::String<12> {
    let mut s = heapless::String::new();
    let _ = s.push_str(units.dist_label());
    let _ = s.push_str(tail);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_route::{ClimbProfile, ClimbSeg};

    /// Back closes the ring: the Climb screen's Back always returns to the Map (the last hop of the
    /// Map → Statistics → Climb → Map 3-cycle).
    #[test]
    fn back_returns_to_map() {
        use crate::activity::Activity;
        use crate::{AppState, Mode, Settings};
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Riding);
        let mut s = Settings::default();
        let scratch = crate::screen::PoiScratch::new();
        let mut cx = Ctx {
            state: &mut st,
            activity: &mut act,
            settings: &mut s,
            routes: &[],
            rides: &[],
            poi_scratch: &scratch,
            now_ms: 0,
        };
        assert!(matches!(ClimbScreen::new().handle(Gesture::Back, &mut cx), Transition::Replace(Screen::Map(_))));
    }

    /// A synthetic climb: base 1000 m at 5 000 m along the route, summit 1400 m at 9 000 m — a
    /// 400 m gain over 4 000 m (a clean 10 % average). The detail profile is built from a
    /// hand-filled linear ramp so `at`/`grade_at` are predictable.
    fn synthetic() -> (ClimbSeg, ClimbProfile) {
        let seg = ClimbSeg {
            start_m: 5_000,
            end_m: 9_000,
            base_ele_m: 1_000,
            top_ele_m: 1_400,
            gain_m: 400,
            avg_grade_pct: 10,
            category: 0,
        };
        // A linear ramp base→summit across the columns, matching the seg. `at(f)` then reads
        // `1000 + 400·f` (to a column), `grade_at` a steady ~10 %.
        let profile = ClimbProfile::from_linear_ramp(&seg);
        (seg, profile)
    }

    /// **To climb** counts the remaining ascent from the live cursor to the summit, and clamps to 0
    /// past the summit — never negative.
    #[test]
    fn to_climb_is_remaining_ascent() {
        let (seg, profile) = synthetic();
        let climb = ActiveClimb { seg: &seg, profile: &profile };
        // At the base (progress = start): the whole 400 m is still to climb (±1 column of ramp
        // quantization).
        assert!((to_climb_m(&climb, 5_000) as i32 - 400).abs() <= 4, "at the base ~400 m remain");
        // Halfway along the climb (progress = 7 000, cursor 0.5, ele ~1200): ~200 m remain.
        assert!((to_climb_m(&climb, 7_000) as i32 - 200).abs() <= 6, "halfway ~200 m remain");
        // At/over the summit: nothing left, clamped to 0.
        assert_eq!(to_climb_m(&climb, 9_000), 0, "at the summit nothing remains");
        assert_eq!(to_climb_m(&climb, 12_000), 0, "past the summit clamps to 0, never negative");
    }

    /// **To top** is the remaining distance `end_m − progress_m`, clamped ≥ 0.
    #[test]
    fn to_top_is_remaining_distance() {
        let (seg, _) = synthetic();
        assert_eq!(to_top_m(&seg, 5_000), 4_000, "at the base the whole 4 km remains");
        assert_eq!(to_top_m(&seg, 7_500), 1_500, "1.5 km left at 7 500 m");
        assert_eq!(to_top_m(&seg, 9_000), 0, "at the summit 0");
        assert_eq!(to_top_m(&seg, 10_000), 0, "past the summit clamps to 0, never underflows");
    }

    /// **Grade** at the cursor reads the profile's local grade — ~10 % on this steady ramp.
    #[test]
    fn grade_tile_reads_local_grade() {
        let (_seg, profile) = synthetic();
        let g = profile.grade_at(profile.cursor_frac(7_000));
        assert!((g - 10).abs() <= 2, "a 400 m / 4 000 m ramp reads ~10 %, got {g}");
    }

    /// **Avg → top** is remaining ascent over remaining distance ×100 — ~10 % anywhere on a steady
    /// ramp — and is guarded to 0 in the last few metres so it can't spike near the summit.
    #[test]
    fn avg_to_top_is_remaining_average_grade() {
        let (seg, profile) = synthetic();
        let climb = ActiveClimb { seg: &seg, profile: &profile };
        // At the base: 400 m over 4 000 m = 10 %.
        assert!((avg_to_top_pct(&climb, 5_000) - 10).abs() <= 2, "from the base ~10 %");
        // Halfway: ~200 m over 2 000 m, still ~10 %.
        assert!((avg_to_top_pct(&climb, 7_000) - 10).abs() <= 3, "halfway still ~10 %");
        // Within the summit guard: reads 0 rather than dividing a give-back into a tiny run.
        assert_eq!(avg_to_top_pct(&climb, 8_990), 0, "inside the summit guard reads 0, no spike");
    }

    /// The four tiles assemble in grid order with the two ascent tiles carrying the up-arrow, and
    /// the distance tile's unit-prefixed caption.
    #[test]
    fn climb_tiles_assemble_in_grid_order() {
        let (seg, profile) = synthetic();
        let climb = ActiveClimb { seg: &seg, profile: &profile };
        let cells = climb_tiles(&climb, 7_000, Units::Metric);
        assert_eq!(cells[0].caption.as_str(), "TO CLIMB");
        assert!(cells[0].arrow, "To climb is an ascent figure → up-arrow");
        assert_eq!(cells[1].caption.as_str(), "KM TO GO", "the distance tile prefixes the unit");
        assert!(!cells[1].arrow);
        assert_eq!(cells[2].caption.as_str(), "GRADE");
        assert!(cells[3].caption.as_str().starts_with("AVG"));
        // 2 km to top at 7 000 m → "2.0" km.
        assert_eq!(cells[1].value.as_str(), "2.0", "remaining distance formats km like Statistics");
    }

    /// Imperial rescales the distance + ascent tiles and swaps the unit caption.
    #[test]
    fn climb_tiles_respect_imperial_units() {
        let (seg, profile) = synthetic();
        let climb = ActiveClimb { seg: &seg, profile: &profile };
        let cells = climb_tiles(&climb, 5_000, Units::Imperial);
        assert_eq!(cells[1].caption.as_str(), "MI TO GO");
        // 4 km × 0.621371 ≈ 2.5 mi.
        assert_eq!(cells[1].value.as_str(), "2.5", "4 km reads 2.5 mi");
        // ~400 m × 3.28084 ≈ 1312 ft of remaining ascent (±ramp quantization).
        let ft: i32 = cells[0].value.as_str().parse().unwrap();
        assert!((ft - 1312).abs() <= 20, "remaining ascent converts to feet, got {ft}");
    }

    /// The grade → colour ramp lands each band on its pinned palette const, with the boundaries on
    /// the higher band and downhill dips in the green band.
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
