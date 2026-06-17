//! The Elevation screen — the riding view's sibling of the [`Map`](super::MapScreen),
//! in the same "explorer's field map" style as the menus: the route's elevation
//! profile as a filled band under an amber top line, with a "you are here" / scrub
//! cursor, a peak label, an amber progress bar, and a 2×3 grid of ride stats. Same
//! wood-framed chrome ([`title_frame`]) as the menus, so the family reads as one device.
//!
//! Bindings (`bikepacking-computer-ui-spec.md` §5): `turn` = scrub the cursor along the
//! profile (manual inspection — needs no GPS), `press` = pause → Ride control, `back` =
//! the sibling Map view, `back-hold` = Menu. `hold` is unbound.
//!
//! This is the Phase-A slice: the profile, peak, cursor and the route-relative stats it
//! can read now (distance done/to-go and climb done/to-go at the cursor, live Speed).
//! The cursor stands in for GPS progress until map-matching lands (Phase B), when it
//! becomes the matched position and Avg. Speed / a live "climbed" fill in.

use core::fmt::Write;

use embedded_graphics::{
    prelude::{DrawTarget, Point},
    primitives::Rectangle,
};
use obcm_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, RenderStats,
};
use obcm_route::{Profile, PROFILE_COLS};

use crate::activity::Mode;
use crate::input::Gesture;

use super::{palette, title_frame, Ctx, MapScreen, MenuScreen, Render, RideControl, Screen, Transition};

/// Columns the cursor moves per encoder detent — a full scrub of the route in ~40 turns.
const SCRUB_STEP: i32 = 6;

// Chart geometry (px), tuned for the 240×320 panel; the band fills the top, the stat
// grid the rest. `x`/widths derive from `w` so a resized simulator window still frames.
const CHART_TOP: i32 = 44;
const CHART_BOT: i32 = 148;
/// The peak elevation maps here (not to `CHART_TOP`), leaving headroom for the peak label.
const BAND_TOP: i32 = CHART_TOP + 16;
const SIDE_MARGIN: i32 = 12;

/// The Elevation profile view. State is the scrub cursor as a profile-column index.
#[derive(Debug, Default)]
pub struct ElevationScreen {
    cursor_col: usize,
}

impl ElevationScreen {
    pub fn new() -> Self {
        ElevationScreen { cursor_col: 0 }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => {
                // Scrub the inspection cursor along the profile (clamped to its ends).
                let last = (PROFILE_COLS - 1) as i32;
                self.cursor_col = (self.cursor_col as i32 + n * SCRUB_STEP).clamp(0, last) as usize;
                Transition::None
            }
            Gesture::Press => {
                // Pause → Ride control, exactly like the Map (the mode outlives the view).
                cx.activity.mode = Mode::Paused;
                Transition::Push(Screen::RideControl(RideControl::new()))
            }
            // Sibling toggle: swap back to the Map without growing the stack.
            Gesture::Back => Transition::Replace(Screen::Map(MapScreen::new())),
            Gesture::BackHold => Transition::Push(Screen::Menu(MenuScreen::new())),
            Gesture::Hold => Transition::None, // unbound
        }
    }

    pub fn draw<D, F>(&self, target: &mut D, rx: &mut Render, color_fn: &F) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        use palette::*;
        let (w, h) = (rx.w as i32, rx.h as i32);
        let mut cv = Canvas::new(target, color_fn);

        // The screen needs both the resident profile (the band) and the route (totals +
        // cumulative climb). Either missing → the empty state, same as the Route menu's.
        let (Some(profile), Some(route)) = (rx.profile, rx.route) else {
            title_frame(&mut cv, w, h, "ELEVATION", "");
            cv.text("No route loaded", Point::new(w / 2, h / 2 - 10), Font::Body, TextAlign::Center, INK);
            cv.text("Load one from Routes", Point::new(w / 2, h / 2 + 12), Font::Label, TextAlign::Center, SUBTEXT);
            return RenderStats::default();
        };

        let last_col = PROFILE_COLS - 1;
        let cursor_col = self.cursor_col.min(last_col);
        let frac = cursor_col as f32 / last_col as f32;

        // --- Title bar: "ELEVATION" + the grade at the cursor ---------------------
        let mut grade: heapless::String<10> = heapless::String::new();
        let _ = write!(grade, "grade {}%", cursor_grade(profile, route.total_distance_m, cursor_col));
        title_frame(&mut cv, w, h, "ELEVATION", &grade);

        // --- Elevation band + amber top line --------------------------------------
        let chart_x = SIDE_MARGIN;
        let chart_w = w - 2 * SIDE_MARGIN;
        let band_bot = CHART_BOT;
        let span = (profile.max_ele_m - profile.min_ele_m).max(1) as f32;
        let ele_to_y = |e: i16| -> i32 {
            let t = ((e - profile.min_ele_m) as f32 / span).clamp(0.0, 1.0);
            band_bot - (t * (band_bot - BAND_TOP) as f32) as i32
        };

        let cols = profile.cols();
        let cursor_px = (frac * chart_w as f32) as i32;
        let mut prev_top: Option<i32> = None;
        for px in 0..chart_w {
            let col = (px as usize * PROFILE_COLS / chart_w as usize).min(last_col);
            let top_y = ele_to_y(cols[col].1);
            let x = chart_x + px;
            // Filled band: the traveled (left of cursor) part reads darker (olive), the
            // part still ahead lighter (tan) — the reference's traveled-portion shading.
            let band = if px <= cursor_px { SUBTEXT } else { PARCHMENT_SHADE };
            cv.vline(x, top_y, band_bot - top_y + 1, 1, band);
            // Amber top line, connected to the previous column so it stays continuous on
            // steep sections rather than stair-stepping into gaps.
            let (y0, y1) = prev_top.map_or((top_y, top_y), |p| (p.min(top_y), p.max(top_y)));
            cv.vline(x, y0 - 1, (y1 - y0) + 2, 1, AMBER);
            prev_top = Some(top_y);
        }
        cv.hline(chart_x, band_bot + 1, chart_w, RULE); // baseline under the band

        // Peak label, centered over the peak column (kept on-panel).
        let peak_x = (chart_x + (profile.peak_col * chart_w as usize / PROFILE_COLS) as i32)
            .clamp(chart_x + 20, w - chart_x - 20);
        let mut peak: heapless::String<10> = heapless::String::new();
        let _ = write!(peak, "{} m", profile.peak_ele_m());
        cv.text(&peak, Point::new(peak_x, CHART_TOP + 1), Font::Label, TextAlign::Center, SUBTEXT);

        // --- "You are here" / scrub cursor ----------------------------------------
        let cursor_x = chart_x + cursor_px;
        let cur_y = ele_to_y(profile.at(frac).1);
        cv.vline(cursor_x, CHART_TOP + 12, band_bot - (CHART_TOP + 12) + 1, 2, AMBER);
        cv.disc(Point::new(cursor_x, cur_y), 4, INK); // dark ring …
        cv.disc(Point::new(cursor_x, cur_y), 3, AMBER); // … around the amber dot
        // "you" beside the dot, on whichever side has room.
        if cursor_x < w - 40 {
            cv.text("you", Point::new(cursor_x + 8, cur_y - 5), Font::Label, TextAlign::Left, INK);
        } else {
            cv.text("you", Point::new(cursor_x - 8, cur_y - 5), Font::Label, TextAlign::Right, INK);
        }

        // --- Amber progress bar at the cursor fraction ----------------------------
        let prog_y = CHART_BOT + 10;
        cv.round(rect(chart_x, prog_y, chart_w, 8), 4, PARCHMENT_SHADE);
        let fill_w = (chart_w as f32 * frac) as i32;
        if fill_w > 0 {
            cv.round(rect(chart_x, prog_y, fill_w, 8), 4, AMBER);
        }

        // --- 2×3 stat grid --------------------------------------------------------
        let total_m = route.total_distance_m as f32;
        let done_m = total_m * frac;
        let climbed = route.ascent_to(done_m as u32);
        let to_climb = route.total_ascent_m.saturating_sub(climbed);

        let mut speed: heapless::String<10> = heapless::String::new();
        match rx.state.user_fix.and_then(|f| f.speed_mps) {
            Some(mps) => {
                let _ = write!(speed, "{} km/h", (mps * 3.6) as i32);
            }
            None => {
                let _ = speed.push_str("--");
            }
        }
        let mut done: heapless::String<10> = heapless::String::new();
        let _ = write!(done, "{:.1} km", done_m / 1000.0);
        let mut to_go: heapless::String<10> = heapless::String::new();
        let _ = write!(to_go, "{:.1} km", (total_m - done_m) / 1000.0);
        let mut climbed_s: heapless::String<10> = heapless::String::new();
        let _ = write!(climbed_s, "{} m", climbed);
        let mut to_climb_s: heapless::String<10> = heapless::String::new();
        let _ = write!(to_climb_s, "{} m", to_climb);

        // (label, value, climb-arrow?). Avg. Speed waits for the ride accumulators (Phase B).
        let cells: [(&str, &str, bool); 6] = [
            ("Speed", &speed, false),
            ("Avg. Speed", "--", false),
            ("done", &done, false),
            ("to go", &to_go, false),
            ("climbed", &climbed_s, true),
            ("to climb", &to_climb_s, true),
        ];
        let gap = 6;
        let col_w = (chart_w - gap) / 2;
        let grid_top = prog_y + 8 + 12;
        let row_h = ((h - 10 - grid_top - 2 * gap) / 3).max(20);
        for (i, &(label, value, arrow)) in cells.iter().enumerate() {
            let x = chart_x + (i % 2) as i32 * (col_w + gap);
            let y = grid_top + (i / 2) as i32 * (row_h + gap);
            tile(&mut cv, rect(x, y, col_w, row_h), label, value, arrow);
        }

        RenderStats::default()
    }
}

/// Draw one stat tile: a tan rounded pane with a small olive caption and a big ink
/// value, optionally prefixed by an up-triangle for climb figures (the panel font has
/// no ↑ glyph — same trick the Route menu uses).
fn tile<D, F>(cv: &mut Canvas<D, F>, area: Rectangle, label: &str, value: &str, arrow: bool)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    use palette::*;
    let (x, y) = (area.top_left.x, area.top_left.y);
    cv.round(area, 4, PARCHMENT_SHADE);
    cv.text(label, Point::new(x + 8, y + 5), Font::Label, TextAlign::Left, SUBTEXT);
    let vy = y + 17;
    let vx = if arrow {
        let ax = x + 9;
        cv.triangle(Point::new(ax, vy + 13), Point::new(ax + 9, vy + 13), Point::new(ax + 4, vy + 2), INK);
        x + 24
    } else {
        x + 8
    };
    cv.text(value, Point::new(vx, vy), Font::Display, TextAlign::Left, INK);
}

/// The grade (%) at the cursor: rise over run across a small window of columns around
/// it, using each column's mid-band elevation. Zero when the run is degenerate.
fn cursor_grade(profile: &Profile, total_distance_m: u32, cursor_col: usize) -> i32 {
    let cols = profile.cols();
    let last = PROFILE_COLS - 1;
    let lo = cursor_col.saturating_sub(4);
    let hi = (cursor_col + 4).min(last);
    let mid = |c: usize| (cols[c].0 as i32 + cols[c].1 as i32) / 2;
    let run_m = (hi - lo) as f32 / last as f32 * total_distance_m as f32;
    if run_m < 1.0 {
        return 0;
    }
    ((mid(hi) - mid(lo)) as f32 / run_m * 100.0) as i32
}
