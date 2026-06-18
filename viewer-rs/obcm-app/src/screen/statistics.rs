//! The Statistics screen — the riding view's sibling of the [`Map`](super::MapScreen),
//! in the same "explorer's field map" style as the menus: the route's elevation
//! profile as a filled band under an amber top line, with a live "you are here" cursor
//! (the matched position), a peak label, an amber progress bar, and a 2×3 grid of ride
//! stats. Same wood-framed chrome ([`title_frame`]) as the menus, so the family reads as
//! one device.
//!
//! Bindings (`bikepacking-computer-ui-spec.md` §5): `turn` = scrub a transient inspection
//! cursor along the profile (snaps back to the live position after a few seconds idle),
//! `press` = pause → Ride control, `back` = the sibling Map view, `back-hold` = Menu.
//! `hold` is unbound (reserved for the profile-zoom phase).
//!
//! **Phase B (live):** the cursor follows [`Activity::progress_m`] from map-matching; the
//! stat grid reads the actually-ridden accumulators (Speed / Avg. Speed / done / climbed)
//! and the route-relative remainders (to go / to climb). Going off-route freezes the
//! cursor at the last on-route point, tints it + the bar warning-red, and swaps the
//! header's grade readout for the cross-track distance. The cursor carries a small
//! current-elevation readout that updates as you scrub.

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

use crate::activity::Activity;
use crate::input::Gesture;

use super::{palette, title_frame, Ctx, MapScreen, Render, Screen, Transition};

/// Columns the cursor moves per encoder detent — a full scrub of the route in ~40 turns.
const SCRUB_STEP: i32 = 6;
/// After this many millis with no scrub input the cursor snaps back to the live position —
/// scrubbing is a transient inspection, not a mode.
const SCRUB_HOLD_MS: u32 = 4000;

// Chart geometry (px), tuned for the 240×320 panel; the band fills the top, the stat
// grid the rest. `x`/widths derive from `w` so a resized simulator window still frames.
const CHART_TOP: i32 = 44;
const CHART_BOT: i32 = 148;
/// The peak elevation maps here (not to `CHART_TOP`), leaving headroom for the peak label.
const BAND_TOP: i32 = CHART_TOP + 16;
const SIDE_MARGIN: i32 = 12;

/// A transient scrub: the inspection cursor's column + the millis after which it snaps
/// back to the live position.
#[derive(Debug, Clone, Copy)]
struct Scrub {
    col: usize,
    until_ms: u32,
}

/// The Elevation profile view. The only state is an optional transient scrub cursor;
/// the live "you are here" position comes from the shared [`Activity`].
#[derive(Debug, Default)]
pub struct StatisticsScreen {
    scrub: Option<Scrub>,
}

impl StatisticsScreen {
    pub fn new() -> Self {
        StatisticsScreen { scrub: None }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => {
                // Seed from the current effective cursor — the active scrub if one is
                // still live, otherwise the matched position — then nudge and re-arm the
                // snap-back timer.
                let last = (PROFILE_COLS - 1) as i32;
                let base = match self.scrub {
                    Some(s) if cx.now_ms < s.until_ms => s.col as i32,
                    _ => live_col(cx.activity) as i32,
                };
                let col = (base + n * SCRUB_STEP).clamp(0, last) as usize;
                self.scrub = Some(Scrub { col, until_ms: cx.now_ms + SCRUB_HOLD_MS });
                Transition::None
            }
            // Sibling toggle: swap back to the Map without growing the stack.
            Gesture::Back => Transition::Replace(Screen::Map(MapScreen::new())),
            Gesture::Hold => Transition::None, // unbound (reserved for profile zoom)
            // press = pause → Ride control, back-hold = Menu (shared by both riding views).
            Gesture::Press | Gesture::BackHold => super::riding_common(g, cx),
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
            title_frame(&mut cv, w, h, "STATS", "");
            super::empty_state(&mut cv, w, h, "No route loaded", "Load one from Routes");
            return RenderStats::default();
        };

        let last_col = PROFILE_COLS - 1;
        let total = route.total_distance_m;
        let off = rx.activity.off_route;

        // Live position (matched progress) drives the traveled shading + progress bar; the
        // cursor may be a transient scrub ahead of / behind it.
        let live_frac = if total > 0 {
            (rx.activity.progress_m as f32 / total as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let live = (live_frac * last_col as f32) as usize;
        let cursor_col = match self.scrub {
            Some(s) if rx.now_ms < s.until_ms => s.col.min(last_col),
            _ => live,
        };
        let cursor_frac = cursor_col as f32 / last_col as f32;
        let scrubbing = cursor_col != live;

        // Live indicators go warning-red off-route; the scrub cursor stays amber (it's an
        // inspection point, not "you").
        let live_color = if off { WARNING } else { AMBER };
        let cursor_color = if off && !scrubbing { WARNING } else { AMBER };

        // --- Title bar: grade at the cursor, or the off-route cross-track readout ------
        if off {
            title_frame(&mut cv, w, h, "STATS", "");
            let mut s: heapless::String<16> = heapless::String::new();
            let _ = write!(s, "off {}m", rx.activity.dist_to_route_m);
            cv.text(&s, Point::new(w - 16, 13), Font::Label, TextAlign::Right, PARCHMENT);
        } else {
            let mut grade: heapless::String<12> = heapless::String::new();
            let _ = write!(grade, "grade {}%", cursor_grade(profile, total, cursor_col));
            title_frame(&mut cv, w, h, "STATS", &grade);
        }

        // --- Elevation band + amber top line ------------------------------------------
        let chart_x = SIDE_MARGIN;
        let chart_w = w - 2 * SIDE_MARGIN;
        let band_bot = CHART_BOT;
        let span = (profile.max_ele_m - profile.min_ele_m).max(1) as f32;
        let ele_to_y = |e: i16| -> i32 {
            let t = ((e - profile.min_ele_m) as f32 / span).clamp(0.0, 1.0);
            band_bot - (t * (band_bot - BAND_TOP) as f32) as i32
        };

        let cols = profile.cols();
        let live_px = (live_frac * chart_w as f32) as i32;
        let mut prev_top: Option<i32> = None;
        for px in 0..chart_w {
            let col = (px as usize * PROFILE_COLS / chart_w as usize).min(last_col);
            let top_y = ele_to_y(cols[col].1);
            let x = chart_x + px;
            // Filled band: the traveled (left of the live position) part reads darker
            // (olive), the part still ahead lighter (tan) — the traveled-portion shading.
            let band = if px <= live_px { SUBTEXT } else { PARCHMENT_SHADE };
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

        // --- The cursor: live "you are here" or a transient scrub ----------------------
        let cursor_px = (cursor_frac * chart_w as f32) as i32;
        let cursor_x = chart_x + cursor_px;
        let cur_ele = profile.at(cursor_frac).1;
        let cur_y = ele_to_y(cur_ele);
        cv.vline(cursor_x, CHART_TOP + 12, band_bot - (CHART_TOP + 12) + 1, 2, cursor_color);
        cv.disc(Point::new(cursor_x, cur_y), 4, INK); // dark ring …
        cv.disc(Point::new(cursor_x, cur_y), 3, cursor_color); // … around the cursor dot
        // Current-elevation readout at the cursor (updates as you scrub). Placed below the
        // dot when the cursor is near the peak column, so the two height labels never
        // overlap; otherwise just above it.
        let mut ele_s: heapless::String<8> = heapless::String::new();
        let _ = write!(ele_s, "{} m", cur_ele);
        let near_peak = (cursor_col as i32 - profile.peak_col as i32).abs() < 18;
        let label_y = if near_peak { cur_y + 9 } else { cur_y - 5 };
        if cursor_x < w - 44 {
            cv.text(&ele_s, Point::new(cursor_x + 8, label_y), Font::Label, TextAlign::Left, INK);
        } else {
            cv.text(&ele_s, Point::new(cursor_x - 8, label_y), Font::Label, TextAlign::Right, INK);
        }

        // --- Progress bar at the live fraction ----------------------------------------
        let prog_y = CHART_BOT + 10;
        cv.round(rect(chart_x, prog_y, chart_w, 8), 4, PARCHMENT_SHADE);
        let fill_w = (chart_w as f32 * live_frac) as i32;
        if fill_w > 0 {
            cv.round(rect(chart_x, prog_y, fill_w, 8), 4, live_color);
        }

        // --- 2×3 stat grid ------------------------------------------------------------
        // done/climbed are *actually-ridden* (the rider's effort, keep counting off-route);
        // to-go/to-climb are necessarily *route-relative* (remaining along the route).
        let a: &Activity = rx.activity;
        let to_go_m = total.saturating_sub(a.progress_m);
        let climbed = a.climb_m as u32;
        // Remaining climb is route-relative: the route's total ascent minus what's been
        // climbed by the live position, read from the profile at column resolution.
        let to_climb = route.total_ascent_m.saturating_sub(profile.ascent_to(live_frac));

        let mut speed: heapless::String<10> = heapless::String::new();
        match rx.state.user_fix.and_then(|f| f.speed_mps) {
            Some(mps) => {
                let _ = write!(speed, "{} km/h", (mps * 3.6) as i32);
            }
            None => {
                let _ = speed.push_str("--");
            }
        }
        let mut avg: heapless::String<10> = heapless::String::new();
        match a.avg_kmh() {
            Some(kmh) => {
                let _ = write!(avg, "{} km/h", kmh as i32);
            }
            None => {
                let _ = avg.push_str("--");
            }
        }
        let mut done: heapless::String<10> = heapless::String::new();
        let _ = write!(done, "{:.1} km", a.ridden_m / 1000.0);
        let mut to_go: heapless::String<10> = heapless::String::new();
        let _ = write!(to_go, "{:.1} km", to_go_m as f32 / 1000.0);
        let mut climbed_s: heapless::String<10> = heapless::String::new();
        let _ = write!(climbed_s, "{} m", climbed);
        let mut to_climb_s: heapless::String<10> = heapless::String::new();
        let _ = write!(to_climb_s, "{} m", to_climb);

        // (label, value, climb-arrow?).
        let cells: [(&str, &str, bool); 6] = [
            ("Speed", &speed, false),
            ("Avg. Speed", &avg, false),
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

/// The profile column of the live matched position, for seeding a scrub from "you are
/// here". `0` when no route length is known yet.
fn live_col(a: &Activity) -> usize {
    if a.route_total_m == 0 {
        return 0;
    }
    let last = PROFILE_COLS - 1;
    let frac = (a.progress_m as f32 / a.route_total_m as f32).clamp(0.0, 1.0);
    (frac * last as f32) as usize
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
