//! The Statistics screen — the riding view's sibling of the [`Map`](super::MapScreen),
//! in the same "explorer's field map" style as the menus: the route's elevation
//! profile as a filled band under an amber top line, with a movable "you are here" /
//! inspection cursor (carrying a current-elevation readout), an amber progress bar, and
//! a 2×3 grid of ride stats. Same wood-framed chrome ([`title_frame`]) as the menus.
//!
//! Bindings (`bikepacking-computer-ui-spec.md` §5):
//! - **Cursor mode (default):** `turn` scrubs the cursor along the *full* profile to read
//!   the elevation/grade at any point; it **springs back to the live position** after a
//!   few seconds idle (a transient inspection, not a mode). `hold` enters Zoom mode.
//! - **Zoom mode:** `turn` zooms the profile **centred on the frozen cursor** (a small
//!   magnifying-glass icon marks the mode — no numbers, no labels). It does *not* spring
//!   back while zooming. `hold` **or** `back` exits, springing back to the full route +
//!   live position.
//! - Shared: `press` = pause → Ride control, `back` (in cursor mode) = the sibling Map,
//!   `back-hold` = Menu.
//!
//! Zoom is cheap: the profile is a load-time [`Profile`] pyramid, so a zoom step is just
//! [`Profile::window`] picking a level + sub-range to draw — no route re-read.
//!
//! **Phase B (live):** the live position comes from [`Activity::progress_m`] (map-matching);
//! the stat grid reads the actually-ridden accumulators (Speed / Avg. Speed / done /
//! climbed) and the route-relative remainders (to go / to climb). Going off-route freezes
//! the live position, tints it + the bar warning-red, and swaps the header's grade readout
//! for the cross-track distance.

use core::fmt::Write;

use embedded_graphics::{
    prelude::{DrawTarget, Point},
    primitives::Rectangle,
};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, RenderStats,
};
use obc_route::Profile;

use crate::activity::Activity;
use crate::input::Gesture;

use super::{palette, title_frame, Ctx, MapScreen, Render, Screen, Transition};

/// Cursor scrub per encoder detent, as a fraction of the whole route — ~42 detents end to
/// end, matching the Phase-B scrub feel (independent of the base column count).
const CURSOR_STEP_FRAC: f32 = 1.0 / 42.0;
/// Zoom multiplier per encoder detent (matches the Map's zoom feel).
const ZOOM_STEP: f32 = 1.2;
/// Zoom clamps: `1.0` = whole route; the max is a touch under where the 2048-col base
/// stops adding detail for a 240-px panel (≈ base / chart width).
const MIN_ZOOM: f32 = 1.0;
const MAX_ZOOM: f32 = 8.0;
/// After this many millis with no input the cursor springs back to the live position —
/// scrubbing is a transient inspection. (Zoom mode is exempt: it never springs back.)
const IDLE_MS: u32 = 4000;

// Chart geometry (px), tuned for the 240×320 panel; the band fills the top, the stat
// grid the rest. `x`/widths derive from `w` so a resized simulator window still frames.
const CHART_TOP: i32 = 42;
const CHART_BOT: i32 = 110;
/// The peak elevation maps here (a few px below `CHART_TOP`) so the apex clears the bar.
const BAND_TOP: i32 = CHART_TOP + 4;
const SIDE_MARGIN: i32 = 12;

/// What `turn` does: scrub the cursor, or zoom the view about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Cursor,
    Zoom,
}

/// The Statistics / elevation-profile view. The cursor defaults to (and springs back to)
/// the live matched position; Zoom is an explicit long-press sub-mode.
#[derive(Debug)]
pub struct StatisticsScreen {
    mode: Mode,
    /// Inspection cursor as a route fraction; `None` = track the live position.
    cursor: Option<f32>,
    /// Zoom factor (`1.0` = full route); only ever `> 1` while in [`Mode::Zoom`].
    zoom: f32,
    /// Millis at the last cursor scrub; the cursor springs back to live once `IDLE_MS`
    /// have elapsed since (Cursor mode only). Stored as the scrub *instant* — not a
    /// deadline — so the `wrapping_sub` elapsed check stays correct across the `u32`
    /// millis wrap, matching the gesture/hold-hint timers.
    last_scrub_ms: u32,
}

impl Default for StatisticsScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl StatisticsScreen {
    pub fn new() -> Self {
        StatisticsScreen { mode: Mode::Cursor, cursor: None, zoom: 1.0, last_scrub_ms: 0 }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let live = live_frac(cx.activity);
        match g {
            Gesture::Turn(n) => {
                self.on_turn(n, live, cx.now_ms);
                Transition::None
            }
            // hold = enter Zoom mode / exit it (springing back to the full route + live).
            Gesture::Hold => {
                match self.mode {
                    Mode::Cursor => {
                        // Freeze the cursor at its current spot; zoom starts at full and
                        // the user turns to zoom in.
                        self.cursor = Some(self.effective_cursor(cx.now_ms, live));
                        self.zoom = 1.0;
                        self.mode = Mode::Zoom;
                    }
                    Mode::Zoom => self.reset(),
                }
                Transition::None
            }
            Gesture::Back => match self.mode {
                // In zoom mode `back` is the quick exit (springs back); otherwise it's the
                // sibling toggle to the Map (the stack stays one deep).
                Mode::Zoom => {
                    self.reset();
                    Transition::None
                }
                Mode::Cursor => Transition::Replace(Screen::Map(MapScreen::new())),
            },
            // press = pause → Ride control, back-hold = Menu (shared by both riding views).
            Gesture::Press | Gesture::BackHold => super::riding_common(g, cx),
        }
    }

    /// Spring back to the default view: cursor tracking the live position, full route.
    fn reset(&mut self) {
        self.mode = Mode::Cursor;
        self.cursor = None;
        self.zoom = 1.0;
        self.last_scrub_ms = 0;
    }

    /// The cursor fraction in effect now: the scrub position while it's still live,
    /// otherwise the live position it has sprung back to.
    fn effective_cursor(&self, now_ms: u32, live: f32) -> f32 {
        match self.cursor {
            Some(c)
                if self.mode == Mode::Zoom || now_ms.wrapping_sub(self.last_scrub_ms) < IDLE_MS =>
            {
                c
            }
            _ => live,
        }
    }

    fn on_turn(&mut self, n: i32, live: f32, now_ms: u32) {
        match self.mode {
            Mode::Cursor => {
                let c = self.effective_cursor(now_ms, live);
                self.cursor = Some((c + n as f32 * CURSOR_STEP_FRAC).clamp(0.0, 1.0));
                self.zoom = 1.0;
                self.last_scrub_ms = now_ms;
            }
            Mode::Zoom => {
                // Multiply per detent (no_std: no powf) — `n` is a small count.
                let step = if n >= 0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
                let mut z = self.zoom;
                for _ in 0..n.unsigned_abs() {
                    z *= step;
                }
                self.zoom = z.clamp(MIN_ZOOM, MAX_ZOOM);
            }
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

        let total = route.total_distance_m;
        let off = rx.activity.off_route;

        // Live position (matched progress) drives the traveled shading + progress bar; the
        // cursor may be a scrub ahead of / behind it, and in zoom mode it's the zoom centre.
        let live_frac = if total > 0 {
            (rx.activity.progress_m as f32 / total as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let cursor_frac = self.effective_cursor(rx.now_ms, live_frac);
        let in_zoom = self.mode == Mode::Zoom;
        let zoom = if in_zoom { self.zoom } else { 1.0 };
        let scrubbing = (cursor_frac - live_frac).abs() > 1e-4;

        // The visible window: zoom mode centres on the (frozen) cursor; cursor mode is the
        // whole route (`zoom == 1`, so the centre is moot).
        let chart_x = SIDE_MARGIN;
        let chart_w = w - 2 * SIDE_MARGIN;
        let win = profile.window(cursor_frac, zoom, chart_w.max(1) as u32);
        let span = (win.hi_frac - win.lo_frac).max(1e-6);
        let frac_to_x = |f: f32| chart_x + ((f - win.lo_frac) / span * chart_w as f32) as i32;

        // Live indicators go warning-red off-route; the cursor stays amber while scrubbing
        // (it's an inspection point, not "you").
        let live_color = if off { WARNING } else { AMBER };
        let cursor_color = if off && !scrubbing { WARNING } else { AMBER };

        // Title bar: grade at the cursor, or the off-route cross-track readout
        let mut readout: heapless::String<16> = heapless::String::new();
        if off {
            let d = rx.activity.dist_to_route_m;
            if d >= 1000 {
                let _ = write!(readout, "off {}km", (d + 500) / 1000);
            } else {
                let _ = write!(readout, "off {}m", d);
            }
        } else {
            let _ = write!(readout, "grade {}%", grade_at(profile, total, cursor_frac));
        }
        title_frame(&mut cv, w, h, "STATS", &readout);

        // Elevation band + amber top line
        let band_bot = CHART_BOT;
        let span_ele = (profile.max_ele_m - profile.min_ele_m).max(1) as f32;
        let ele_to_y = |e: i16| -> i32 {
            let t = ((e - profile.min_ele_m) as f32 / span_ele).clamp(0.0, 1.0);
            band_bot - (t * (band_bot - BAND_TOP) as f32) as i32
        };

        let mut prev_top: Option<i32> = None;
        for px in 0..chart_w {
            // The route fraction this pixel shows, read from the window's pyramid level.
            let f = win.lo_frac + span * (px as f32 / chart_w as f32);
            let top_y = ele_to_y(profile.sample(win.level, f).1);
            let x = chart_x + px;
            // Filled band: the traveled part (left of the live position) reads darker
            // (olive), the part still ahead lighter (tan) — the traveled-portion shading.
            let band = if f <= live_frac { SUBTEXT } else { PARCHMENT_SHADE };
            cv.vline(x, top_y, band_bot - top_y + 1, 1, band);
            // Amber top line, connected to the previous column so it stays continuous on
            // steep sections rather than stair-stepping into gaps.
            let (y0, y1) = prev_top.map_or((top_y, top_y), |p| (p.min(top_y), p.max(top_y)));
            cv.vline(x, y0 - 1, (y1 - y0) + 2, 1, AMBER);
            prev_top = Some(top_y);
        }
        cv.hline(chart_x, band_bot + 1, chart_w, RULE); // baseline under the band

        // The cursor (always in-window: scrub point, or the zoom centre)
        let cursor_x = frac_to_x(cursor_frac).clamp(chart_x, chart_x + chart_w - 1);
        let cur_ele = profile.at(cursor_frac).1;
        let cur_y = ele_to_y(cur_ele);
        cv.vline(cursor_x, CHART_TOP, band_bot - CHART_TOP + 1, 2, cursor_color);
        cv.disc(Point::new(cursor_x, cur_y), 4, INK); // dark ring …
        cv.disc(Point::new(cursor_x, cur_y), 3, cursor_color); // … around the cursor dot
                                                               // Current-elevation readout at the cursor (updates as you scrub). Placed below the
                                                               // dot near the peak so the labels never overlap; else just above it, clamped inside
                                                               // the band and clear of the baseline/bar.
        let mut ele_s: heapless::String<8> = heapless::String::new();
        let _ = write!(ele_s, "{} m", cur_ele);
        let near_peak = (cursor_frac - profile.peak_frac()).abs() < 0.07;
        let label_y =
            (if near_peak { cur_y + 9 } else { cur_y - 5 }).clamp(CHART_TOP + 2, band_bot - 24);
        if cursor_x < w - 44 {
            cv.text(&ele_s, Point::new(cursor_x + 8, label_y), Font::Label, TextAlign::Left, INK);
        } else {
            cv.text(&ele_s, Point::new(cursor_x - 8, label_y), Font::Label, TextAlign::Right, INK);
        }

        // Zoom-mode marker: a small magnifying-glass icon (no numbers, no label)
        if in_zoom {
            draw_zoom_icon(&mut cv, chart_x + 2, CHART_TOP + 2);
        }

        // Progress bar at the live fraction
        let prog_y = CHART_BOT + 10;
        cv.round(rect(chart_x, prog_y, chart_w, 8), 4, PARCHMENT_SHADE);
        let fill_w = (chart_w as f32 * live_frac) as i32;
        if fill_w > 0 {
            cv.round(rect(chart_x, prog_y, fill_w, 8), 4, live_color);
        }

        // 2×3 stat grid
        // done/climbed are *actually-ridden* (the rider's effort, keep counting off-route);
        // to-go/to-climb are necessarily *route-relative* (remaining along the route).
        let a: &Activity = rx.activity;
        let to_go_m = total.saturating_sub(a.progress_m);
        let climbed = a.climb_m() as u32;
        // Remaining climb is route-relative: the route's total ascent minus what's been
        // climbed by the live position, read from the profile at column resolution.
        let to_climb = route.total_ascent_m.saturating_sub(profile.ascent_to(live_frac));

        // Values are **number only** — the unit lives in the tile's caption (Wahoo style),
        // so the big Display digits fit the half-width tiles instead of overrunning them.
        // Speeds keep one decimal; distances drop it past 100 km so they stay ≤ 3 digits.
        let mut speed: heapless::String<8> = heapless::String::new();
        match rx.state.user_fix.and_then(|f| f.speed_mps) {
            Some(mps) => {
                let _ = write!(speed, "{:.1}", mps * 3.6);
            }
            None => {
                let _ = speed.push_str("--");
            }
        }
        let mut avg: heapless::String<8> = heapless::String::new();
        match a.avg_kmh() {
            Some(kmh) => {
                let _ = write!(avg, "{:.1}", kmh);
            }
            None => {
                let _ = avg.push_str("--");
            }
        }
        let km_done = a.ridden_m / 1000.0;
        let km_to_go = to_go_m as f32 / 1000.0;
        let mut done: heapless::String<8> = heapless::String::new();
        let _ = if km_done >= 100.0 {
            write!(done, "{:.0}", km_done)
        } else {
            write!(done, "{:.1}", km_done)
        };
        let mut to_go: heapless::String<8> = heapless::String::new();
        let _ = if km_to_go >= 100.0 {
            write!(to_go, "{:.0}", km_to_go)
        } else {
            write!(to_go, "{:.1}", km_to_go)
        };
        let mut climbed_s: heapless::String<8> = heapless::String::new();
        let _ = write!(climbed_s, "{}", climbed);
        let mut to_climb_s: heapless::String<8> = heapless::String::new();
        let _ = write!(to_climb_s, "{}", to_climb);

        // (caption [unit-bearing], value [number only], climb-arrow?). The up-arrow on the
        // climb tiles reads as "elevation, metres".
        let cells: [(&str, &str, bool); 6] = [
            ("KPH", &speed, false),
            ("AVG KPH", &avg, false),
            ("KM DONE", &done, false),
            ("KM TO GO", &to_go, false),
            ("CLIMBED", &climbed_s, true),
            ("TO CLIMB", &to_climb_s, true),
        ];
        let gap = 6;
        let col_w = (chart_w - gap) / 2;
        // Tuck the grid up a little under the progress bar so the three rows get more
        // height (≈54 px), giving the big value room to sit off the tile's bottom edge.
        let grid_top = prog_y + 16;
        let row_h = ((h - 10 - grid_top - 2 * gap) / 3).max(20);
        for (i, &(label, value, arrow)) in cells.iter().enumerate() {
            let x = chart_x + (i % 2) as i32 * (col_w + gap);
            let y = grid_top + (i / 2) as i32 * (row_h + gap);
            tile(&mut cv, rect(x, y, col_w, row_h), label, value, arrow);
        }

        RenderStats::default()
    }
}

/// The fractional position (`0.0`–`1.0`) of the live matched position along the route.
/// `0.0` when no route length is known yet.
fn live_frac(a: &Activity) -> f32 {
    if a.route_total_m == 0 {
        return 0.0;
    }
    (a.progress_m as f32 / a.route_total_m as f32).clamp(0.0, 1.0)
}

/// Draw a small magnifying-glass icon on a parchment chip — the wordless "Zoom mode is on"
/// marker (top-left of the chart). A lens (ink ring) with a short diagonal handle; no
/// numbers or label, since the zoom *level* isn't useful information.
fn draw_zoom_icon<D, F>(cv: &mut Canvas<D, F>, x: i32, y: i32)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    use palette::*;
    let s = 22;
    cv.round(rect(x, y, s, s), 5, PARCHMENT);
    cv.round_outline(rect(x, y, s, s), 5, WOOD_LIGHT);
    // Lens: an ink ring (filled disc with a parchment disc punched out).
    let (lx, ly) = (x + 8, y + 8);
    cv.disc(Point::new(lx, ly), 5, INK);
    cv.disc(Point::new(lx, ly), 3, PARCHMENT);
    // Handle: a few ink discs stepping out from the lower-right of the lens.
    for k in 0..3 {
        cv.disc(Point::new(lx + 4 + k, ly + 4 + k), 2, INK);
    }
}

/// Draw one stat tile: a tan rounded pane with a small olive caption (unit-bearing) at
/// the top and a big ink Display value below, optionally prefixed by an up-triangle for
/// climb figures (the panel font has no ↑ glyph — same trick the Route menu uses). The
/// value is number-only, so the big digits fit the half-width tile.
fn tile<D, F>(cv: &mut Canvas<D, F>, area: Rectangle, label: &str, value: &str, arrow: bool)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    use palette::*;
    let (x, y) = (area.top_left.x, area.top_left.y);
    cv.round(area, 5, PARCHMENT_SHADE);
    // Caption inset slightly less than the value so the wide unit captions (KM TO GO,
    // TO CLIMB) sit nearer the tile's centre; the value keeps a touch more left margin.
    cv.text(label, Point::new(x + 5, y + 4), Font::Label, TextAlign::Left, SUBTEXT);
    let vy = y + 22;
    let vx = if arrow {
        // Up-triangle sized to sit alongside the Display digits (ink spans ≈ vy+6..vy+26).
        let ax = x + 8;
        cv.triangle(
            Point::new(ax, vy + 26),
            Point::new(ax + 13, vy + 26),
            Point::new(ax + 6, vy + 6),
            INK,
        );
        x + 26
    } else {
        x + 8
    };
    cv.text(value, Point::new(vx, vy), Font::Display, TextAlign::Left, INK);
}

/// The grade (%) at fractional position `frac`: rise over run across a small fixed window
/// of the route around it, using each end's mid-band elevation (base level). Zero when the
/// run is degenerate.
fn grade_at(profile: &Profile, total_distance_m: u32, frac: f32) -> i32 {
    // ±1.5 % of the route — a touch of smoothing, matching the old ±4-of-256-columns feel.
    const HALF: f32 = 0.015;
    let lo = (frac - HALF).max(0.0);
    let hi = (frac + HALF).min(1.0);
    let mid = |t: f32| {
        let (a, b) = profile.at(t);
        (a as i32 + b as i32) / 2
    };
    let run_m = (hi - lo) * total_distance_m as f32;
    if run_m < 1.0 {
        return 0;
    }
    ((mid(hi) - mid(lo)) as f32 / run_m * 100.0) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A live position to scrub away from; one detent right lands at `scrubbed`.
    const LIVE: f32 = 0.5;
    fn scrubbed() -> f32 {
        (LIVE + CURSOR_STEP_FRAC).clamp(0.0, 1.0)
    }

    /// Spring-back: after a scrub the cursor holds for `IDLE_MS`, then tracks live again.
    #[test]
    fn cursor_springs_back_after_idle() {
        let mut s = StatisticsScreen::new();
        s.on_turn(1, LIVE, 1_000);
        // Held while fresh and right up to (but not at) the threshold…
        assert_eq!(s.effective_cursor(1_000, LIVE), scrubbed());
        assert_eq!(s.effective_cursor(1_000 + IDLE_MS - 1, LIVE), scrubbed());
        // …then springs back to live once IDLE_MS have elapsed.
        assert_eq!(s.effective_cursor(1_000 + IDLE_MS, LIVE), LIVE);
    }

    /// The fix (issue #6): near the `u32` millis wrap, the old `now + IDLE_MS` deadline
    /// overflowed — a debug panic, a corrupted timer in release. The `wrapping_sub` elapsed
    /// check must behave identically straddling the wrap.
    #[test]
    fn idle_timer_is_wrap_safe() {
        let mut s = StatisticsScreen::new();
        let t0 = u32::MAX - 1_000; // 1 s before the wrap; t0 + IDLE_MS would overflow
        s.on_turn(1, LIVE, t0); // panicked here in debug before the fix
                                // Held across the wrap while still inside the window…
        assert_eq!(s.effective_cursor(t0, LIVE), scrubbed());
        assert_eq!(s.effective_cursor(t0.wrapping_add(IDLE_MS - 1), LIVE), scrubbed());
        // …and springs back to live once IDLE_MS have elapsed past the wrap.
        assert_eq!(s.effective_cursor(t0.wrapping_add(IDLE_MS), LIVE), LIVE);
    }
}
