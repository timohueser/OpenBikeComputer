//! The Statistics screen — the riding view's sibling of the [`Map`](super::MapScreen): the route's
//! elevation profile as a filled band under an amber top line, with a movable inspection cursor
//! (carrying a current-elevation readout), an amber progress bar, and a grid of ride stats.
//!
//! Bindings:
//! - **Cursor mode (default):** `turn` scrubs the cursor along the full profile; it springs back to
//!   the live position after a few seconds idle. `hold` enters Zoom mode.
//! - **Zoom mode:** `turn` zooms centred on the frozen cursor (a magnifying-glass icon marks the
//!   mode). It does not spring back while zooming. `hold` or `back` exits, springing back.
//! - Shared: `press` = pause → Ride control, `back` (cursor mode) = the sibling Map, `back-hold` = Menu.
//!
//! Zoom is cheap: the profile is a load-time [`Profile`] pyramid, so a step is just
//! [`Profile::window`] picking a level + sub-range — no route re-read. Going off-route freezes the
//! live position, tints it + the bar warning-red, and swaps the grade readout for cross-track distance.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::settings::Settings;
use crate::stat_fields;

use super::{palette, title_frame, Ctx, MapScreen, Render, Screen, ScreenTick, Transition};

/// Cursor scrub per encoder detent, as a fraction of the whole route — ~42 detents end to end.
const CURSOR_STEP_FRAC: f32 = 1.0 / 42.0;
/// Zoom multiplier per encoder detent (matches the Map's zoom feel).
const ZOOM_STEP: f32 = 1.2;
/// Zoom clamps: `1.0` = whole route; the max is a touch under where the base stops adding detail.
const MIN_ZOOM: f32 = 1.0;
const MAX_ZOOM: f32 = 8.0;
/// After this many millis with no input the cursor springs back to the live position. (Zoom mode
/// is exempt: it never springs back.)
const IDLE_MS: u32 = 4000;

// Chart geometry (px), tuned for the 240×320 panel; the band fills the top, the stat grid the rest.
const CHART_TOP: i32 = 42;
const CHART_BOT: i32 = 110;
/// The peak elevation maps here (a few px below `CHART_TOP`) so the apex clears the bar.
const BAND_TOP: i32 = CHART_TOP + 4;
const SIDE_MARGIN: i32 = 12;
/// "Near the peak" for the cursor's elevation label, in **screen px**: inside this of the peak the
/// label drops below the dot so it can't overlap the apex. Screen-space (not a route fraction) so
/// it stays a constant on-glass distance at every zoom; an off-window peak is never near.
const PEAK_NEAR_PX: i32 = 36;

/// What `turn` does: scrub the cursor, or zoom the view about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Cursor,
    Zoom,
}

/// The Statistics / elevation-profile view. The cursor defaults to (and springs back to) the live
/// matched position; Zoom is an explicit long-press sub-mode.
#[derive(Debug)]
pub struct StatisticsScreen {
    mode: Mode,
    /// Inspection cursor as a route fraction; `None` = track the live position.
    cursor: Option<f32>,
    /// Zoom factor (`1.0` = full route); only ever `> 1` while in [`Mode::Zoom`].
    zoom: f32,
    /// Instant of the last cursor scrub (not a deadline, so the `wrapping_sub` elapsed check stays
    /// correct across the `u32` millis wrap). The cursor springs back once `IDLE_MS` elapse.
    last_scrub_ms: u32,
    /// Which page of the stat grid is showing; [`tick_timers`](Self::tick_timers) auto-cycles it on
    /// a timer.
    page: usize,
    /// Instant of the last page flip (wrap-safe like `last_scrub_ms`). `None` until the first frame
    /// anchors it, so the first page gets a full dwell on entry.
    last_flip_ms: Option<u32>,
}

impl Default for StatisticsScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl StatisticsScreen {
    pub fn new() -> Self {
        StatisticsScreen { mode: Mode::Cursor, cursor: None, zoom: 1.0, last_scrub_ms: 0, page: 0, last_flip_ms: None }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let live = stat_fields::live_frac(cx.activity);
        match g {
            Gesture::Turn(n) => {
                self.on_turn(n, live, cx.now_ms);
                Transition::None
            }
            // hold = enter/exit Zoom mode.
            Gesture::Hold => {
                match self.mode {
                    Mode::Cursor => {
                        // Freeze the cursor at its current spot; zoom starts at full.
                        self.cursor = Some(self.effective_cursor(cx.now_ms, live));
                        self.zoom = 1.0;
                        self.mode = Mode::Zoom;
                    }
                    Mode::Zoom => self.reset(),
                }
                Transition::None
            }
            Gesture::Back => match self.mode {
                // Zoom: quick exit (springs back). Cursor: sibling toggle to the Map.
                Mode::Zoom => {
                    self.reset();
                    Transition::None
                }
                Mode::Cursor => Transition::Replace(Screen::Map(MapScreen::new())),
            },
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

    /// Poll the view's two timers in one body — the cursor's idle spring-back and the stat grid's
    /// page auto-cycle — firing whichever is due and reporting the soonest residual deadline from
    /// the same elapsed-time locals, so what fired and when to wake next can never disagree. Both
    /// elapsed checks are `wrapping_sub`, so they stay correct across the `u32` millis wrap.
    ///
    /// Spring-back: once [`IDLE_MS`] elapse since the last scrub (Cursor mode only — Zoom never
    /// springs back), the cursor drops back to tracking live — making observable the transition
    /// [`effective_cursor`] already does lazily, so the dirty-tracking host (issue #47) redraws at
    /// the right moment. Idempotent: once sprung back, nothing fires and nothing is pending.
    ///
    /// Page cycle: with more than one page, the view dwells [`stat_cycle_s`](Settings::stat_cycle_s)
    /// on each; with one page it pins page 0 and re-anchors the timer so a later expansion starts a
    /// fresh dwell. The anchor is lazily set on the first poll, so entering the screen gives page 0
    /// a full dwell.
    pub fn tick_timers(&mut self, now_ms: u32, settings: &Settings) -> ScreenTick {
        let mut changed = false;

        // Cursor spring-back: armed only while a scrub is live in Cursor mode. Due → fire (drop
        // back to live); not yet → the remainder is the deadline, strictly positive by the gate.
        let mut spring = None;
        if self.mode == Mode::Cursor && self.cursor.is_some() {
            let elapsed = now_ms.wrapping_sub(self.last_scrub_ms);
            if elapsed >= IDLE_MS {
                self.cursor = None;
                changed = true;
            } else {
                spring = Some(IDLE_MS - elapsed);
            }
        }

        // Page auto-cycle: always pending with more than one page (a flip re-arms a full dwell).
        let pages = stat_fields::page_count(&settings.stat_fields);
        let last = *self.last_flip_ms.get_or_insert(now_ms);
        let page = if pages <= 1 {
            self.page = 0;
            self.last_flip_ms = Some(now_ms);
            None
        } else {
            self.page = self.page.min(pages - 1);
            let period_ms = settings.stat_cycle_s.max(1) as u32 * 1000;
            let elapsed = now_ms.wrapping_sub(last);
            if elapsed >= period_ms {
                self.page = (self.page + 1) % pages;
                self.last_flip_ms = Some(now_ms);
                changed = true;
                Some(period_ms)
            } else {
                Some(period_ms - elapsed)
            }
        };

        let next_wake_ms = match (spring, page) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        ScreenTick { changed, next_wake_ms }
    }

    /// The cursor fraction in effect now: the scrub position while it's still live,
    /// otherwise the live position it has sprung back to.
    fn effective_cursor(&self, now_ms: u32, live: f32) -> f32 {
        match self.cursor {
            Some(c) if self.mode == Mode::Zoom || now_ms.wrapping_sub(self.last_scrub_ms) < IDLE_MS => c,
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
                // Multiply per detent (no_std: no powf).
                let step = if n >= 0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
                let mut z = self.zoom;
                for _ in 0..n.unsigned_abs() {
                    z *= step;
                }
                self.zoom = z.clamp(MIN_ZOOM, MAX_ZOOM);
            }
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        // The screen needs both the resident profile (the band) and the route (totals +
        // cumulative climb). Either missing → the empty state, same as the Route menu's.
        let (Some(profile), Some(route)) = (rx.profile, rx.route) else {
            title_frame(cv, w, h, "STATS", "");
            super::empty_state(cv, w, h, "No route loaded", "Load one from Routes");
            return;
        };

        let total = route.total_distance_m;
        let off = rx.activity.off_route;
        // Re-captions + re-scales every readout below; grade stays a bare percentage.
        let units = rx.settings.units;

        // Live position (matched progress) drives the traveled shading + progress bar; the cursor
        // may be a scrub ahead of / behind it, and in zoom mode it's the zoom centre.
        let live_frac = if total > 0 { (rx.activity.progress_m as f32 / total as f32).clamp(0.0, 1.0) } else { 0.0 };
        let cursor_frac = self.effective_cursor(rx.now_ms, live_frac);
        let in_zoom = self.mode == Mode::Zoom;
        let zoom = if in_zoom { self.zoom } else { 1.0 };
        let scrubbing = (cursor_frac - live_frac).abs() > 1e-4;

        // Zoom mode centres the window on the frozen cursor; cursor mode is the whole route.
        let chart_x = SIDE_MARGIN;
        let chart_w = w - 2 * SIDE_MARGIN;
        let win = profile.window(cursor_frac, zoom, chart_w.max(1) as u32);
        let span = (win.hi_frac - win.lo_frac).max(1e-6);
        let frac_to_x = |f: f32| chart_x + ((f - win.lo_frac) / span * chart_w as f32) as i32;

        // Live indicators go warning-red off-route; the cursor stays amber while scrubbing (it's an
        // inspection point, not "you").
        let live_color = if off { WARNING } else { AMBER };
        let cursor_color = if off && !scrubbing { WARNING } else { AMBER };

        // Title bar: "no GPS" while there's no current fix (readouts are stale), else the off-route
        // cross-track distance, else the grade at the cursor.
        let mut readout: heapless::String<16> = heapless::String::new();
        if rx.no_fix {
            let _ = readout.push_str("no GPS");
        } else if off {
            super::write_off_route(&mut readout, "off ", rx.activity.dist_to_route_m, units);
        } else {
            let _ = write!(readout, "grade {}%", stat_fields::grade_at(profile, total, cursor_frac));
        }
        title_frame(cv, w, h, "STATS", &readout);

        // Elevation band + amber top line
        let band_bot = CHART_BOT;
        let span_ele = (profile.max_ele_m - profile.min_ele_m).max(1) as f32;
        let ele_to_y = |e: i16| -> i32 {
            let t = ((e - profile.min_ele_m) as f32 / span_ele).clamp(0.0, 1.0);
            band_bot - (t * (band_bot - BAND_TOP) as f32) as i32
        };

        let mut prev_top: Option<i32> = None;
        for px in 0..chart_w {
            let f = win.lo_frac + span * (px as f32 / chart_w as f32);
            let top_y = ele_to_y(profile.sample(win.level, f).1);
            let x = chart_x + px;
            // Traveled part (left of live) reads darker olive, the part ahead lighter tan.
            let band = if f <= live_frac { SUBTEXT } else { PARCHMENT_SHADE };
            cv.vline(x, top_y, band_bot - top_y + 1, 1, band);
            // Amber top line, connected to the previous column so it stays continuous on steep
            // sections rather than stair-stepping into gaps.
            let (y0, y1) = prev_top.map_or((top_y, top_y), |p| (p.min(top_y), p.max(top_y)));
            cv.vline(x, y0 - 1, (y1 - y0) + 2, 1, AMBER);
            prev_top = Some(top_y);
        }
        cv.hline(chart_x, band_bot + 1, chart_w, RULE); // baseline under the band

        // The cursor (scrub point, or the zoom centre)
        let cursor_x = frac_to_x(cursor_frac).clamp(chart_x, chart_x + chart_w - 1);
        let cur_ele = profile.at(cursor_frac).1;
        let cur_y = ele_to_y(cur_ele);
        cv.vline(cursor_x, CHART_TOP, band_bot - CHART_TOP + 1, 2, cursor_color);
        cv.disc(Point::new(cursor_x, cur_y), 4, INK);
        cv.disc(Point::new(cursor_x, cur_y), 3, cursor_color);
        // Elevation readout at the cursor. Below the dot near the peak so labels don't overlap;
        // else just above, clamped inside the band and clear of the baseline/bar.
        let mut ele_s: heapless::String<8> = heapless::String::new();
        let _ = write!(ele_s, "{} {}", units.elev(cur_ele as f32) as i32, units.elev_label());
        let peak_x = frac_to_x(profile.peak_frac());
        let near_peak = (chart_x..chart_x + chart_w).contains(&peak_x) && (cursor_x - peak_x).abs() < PEAK_NEAR_PX;
        let label_y = (if near_peak { cur_y + 9 } else { cur_y - 5 }).clamp(CHART_TOP + 2, band_bot - 24);
        if cursor_x < w - 44 {
            cv.text(&ele_s, Point::new(cursor_x + 8, label_y), Font::Label, TextAlign::Left, INK);
        } else {
            cv.text(&ele_s, Point::new(cursor_x - 8, label_y), Font::Label, TextAlign::Right, INK);
        }

        if in_zoom {
            draw_zoom_icon(cv, chart_x + 2, CHART_TOP + 2);
        }

        // Progress bar at the live fraction
        let prog_y = CHART_BOT + 10;
        cv.round(rect(chart_x, prog_y, chart_w, 8), 4, PARCHMENT_SHADE);
        let fill_w = (chart_w as f32 * live_frac) as i32;
        if fill_w > 0 {
            cv.round(rect(chart_x, prog_y, fill_w, 8), 4, live_color);
        }

        // Customizable stat grid: the rider's fields, paginated (3x2) and auto-cycled by `tick_timers`.
        // Each placed field renders its own tile via the registry; a two-span field fills a row.
        let fields = &rx.settings.stat_fields;
        let page = self.page.min(stat_fields::page_count(fields) - 1);

        let gap = 6;
        let col_w = (chart_w - gap) / 2;
        let grid_top = prog_y + 16;
        let row_h = ((h - 10 - grid_top - 2 * gap) / stat_fields::ROWS_PER_PAGE as i32).max(20);
        let cx = rx.readout();
        for placed in stat_fields::page_fields(fields, page) {
            let cell = placed.field.cell(&cx);
            let x = chart_x + placed.col as i32 * (col_w + gap);
            let y = grid_top + placed.row as i32 * (row_h + gap);
            let tile_w = if placed.field.span() == 2 { chart_w } else { col_w };
            super::tile(cv, rect(x, y, tile_w, row_h), &cell.caption, &cell.value, cell.arrow, PARCHMENT_SHADE);
        }
    }
}

/// Draw a magnifying-glass icon on a parchment chip — the wordless "Zoom mode is on" marker. A
/// lens (ink ring) with a short diagonal handle.
fn draw_zoom_icon(cv: &mut impl Surface, x: i32, y: i32) {
    use palette::*;
    let s = 22;
    cv.round(rect(x, y, s, s), 5, PARCHMENT);
    cv.round_outline(rect(x, y, s, s), 5, WOOD_LIGHT);
    // Lens: an ink ring (filled disc with a parchment disc punched out).
    let (lx, ly) = (x + 8, y + 8);
    cv.disc(Point::new(lx, ly), 5, INK);
    cv.disc(Point::new(lx, ly), 3, PARCHMENT);
    // Handle: ink discs stepping out from the lower-right of the lens.
    for k in 0..3 {
        cv.disc(Point::new(lx + 4 + k, ly + 4 + k), 2, INK);
    }
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

    /// `tick_timers` fires `changed` exactly once, at the deadline, and agrees with the lazy
    /// spring-back `effective_cursor` does. Between the scrub and the deadline it's `false`.
    #[test]
    fn tick_timers_reports_the_spring_back_once_at_the_deadline() {
        // Default = six tiles = one page, so the page auto-cycle never fires — isolates the spring-back.
        let cfg = Settings::default();
        let mut s = StatisticsScreen::new();
        // Untouched: tracking the live position already, nothing to settle.
        assert!(!s.tick_timers(1_000, &cfg).changed, "an untouched view never self-dirties");

        s.on_turn(1, LIVE, 1_000); // scrub the cursor away from live
        assert!(!s.tick_timers(1_000, &cfg).changed, "the scrub frame itself isn't a spring-back");
        assert!(!s.tick_timers(1_000 + IDLE_MS - 1, &cfg).changed, "still frozen inside the idle window");
        assert!(s.tick_timers(1_000 + IDLE_MS, &cfg).changed, "springs back exactly at the deadline → dirty once");
        assert_eq!(s.effective_cursor(1_000 + IDLE_MS, LIVE), LIVE, "and it really is back at live");
        assert!(!s.tick_timers(1_000 + IDLE_MS + 5_000, &cfg).changed, "and only once — it stays put afterwards");
    }

    /// Zoom mode is exempt from the spring-back (the frozen cursor is the zoom centre), so
    /// `tick_timers` must never fire there.
    #[test]
    fn tick_timers_never_springs_back_in_zoom_mode() {
        let cfg = Settings::default();
        let mut s = StatisticsScreen::new();
        s.on_turn(1, LIVE, 0); // a scrub…
        s.mode = Mode::Zoom; // …then into zoom (as `Hold` would)
        assert!(!s.tick_timers(IDLE_MS * 3, &cfg).changed, "zoom mode holds the cursor — no spring-back");
    }

    /// A seven-field selection (two pages) auto-cycles: the first frame anchors a full dwell, the
    /// page flips at each period, and wraps back round.
    #[test]
    fn page_auto_cycles_on_the_timer() {
        let mut cfg = Settings::default();
        assert!(cfg.stat_fields.push(crate::stat_fields::StatField::Grade), "7 fields → two pages");
        cfg.stat_cycle_s = 5;
        let period = cfg.stat_cycle_s as u32 * 1000;
        let mut s = StatisticsScreen::new();
        // First frame anchors the timer — page 0 gets a full dwell, no flip.
        assert!(!s.tick_timers(10_000, &cfg).changed, "the anchoring frame doesn't flip");
        assert_eq!(s.page, 0);
        assert!(!s.tick_timers(10_000 + period - 1, &cfg).changed, "still dwelling just before the deadline");
        assert_eq!(s.page, 0);
        assert!(s.tick_timers(10_000 + period, &cfg).changed, "flips to page 1 at the deadline → dirty");
        assert_eq!(s.page, 1);
        assert!(s.tick_timers(10_000 + 2 * period, &cfg).changed, "and wraps back to page 0 a period later");
        assert_eq!(s.page, 0);
    }

    /// A single-page selection (the default six) never auto-cycles and pins page 0.
    #[test]
    fn single_page_grid_never_flips() {
        let cfg = Settings::default();
        let mut s = StatisticsScreen::new();
        assert!(!s.tick_timers(1_000, &cfg).changed);
        assert!(!s.tick_timers(1_000_000, &cfg).changed, "one page never flips");
        assert_eq!(s.page, 0);
    }

    /// `next_wake_ms` reports the time left until the same timer would fire `changed`. A live scrub
    /// counts down to its `IDLE_MS` spring-back; an untouched single-page view has no timed redraw.
    #[test]
    fn next_wake_counts_down_to_the_spring_back() {
        let cfg = Settings::default(); // single page → only the cursor spring-back can be pending
        let mut s = StatisticsScreen::new();
        assert_eq!(s.tick_timers(1_000, &cfg).next_wake_ms, None, "an untouched view needs no timed wake");
        s.on_turn(1, LIVE, 1_000); // scrub → the spring-back timer is now armed
        assert_eq!(s.tick_timers(1_000, &cfg).next_wake_ms, Some(IDLE_MS), "the full idle window remains at the scrub");
        assert_eq!(
            s.tick_timers(1_000 + 1_000, &cfg).next_wake_ms,
            Some(IDLE_MS - 1_000),
            "counts down as time passes"
        );
        // The poll that springs back reports the change and, in the same result, nothing left to wake for.
        let tick = s.tick_timers(1_000 + IDLE_MS, &cfg);
        assert!(tick.changed);
        assert_eq!(tick.next_wake_ms, None, "sprung back → no further timed wake");
    }

    /// With more than one page the auto-cycle is always pending, so `next_wake_ms` tracks the dwell
    /// remaining (and, when both timers are live, the spring-back wins if it's sooner).
    #[test]
    fn next_wake_tracks_the_page_dwell_and_takes_the_soonest() {
        let mut cfg = Settings::default();
        assert!(cfg.stat_fields.push(crate::stat_fields::StatField::Grade), "7 fields → two pages");
        cfg.stat_cycle_s = 5;
        let period = cfg.stat_cycle_s as u32 * 1000;
        let mut s = StatisticsScreen::new();
        // The first poll anchors the dwell at t = 10 s — the whole period remains.
        assert_eq!(s.tick_timers(10_000, &cfg).next_wake_ms, Some(period), "the anchoring poll = the full period");
        assert_eq!(s.tick_timers(10_000 + 2_000, &cfg).next_wake_ms, Some(period - 2_000), "2 s into the dwell");
        // A fresh scrub arms the spring-back too; IDLE_MS (4 s) < the 3 s left? no — 3 s page wins.
        s.on_turn(1, LIVE, 10_000 + 2_000);
        assert_eq!(
            s.tick_timers(10_000 + 2_000, &cfg).next_wake_ms,
            Some(period - 2_000),
            "the sooner of the two deadlines"
        );
    }

    /// The page timer is wrap-safe like the cursor spring-back: anchored just before the `u32` millis
    /// wrap, it still flips exactly one period later (the `wrapping_sub` elapsed check), not instantly.
    #[test]
    fn page_cycle_is_wrap_safe() {
        let mut cfg = Settings::default();
        assert!(cfg.stat_fields.push(crate::stat_fields::StatField::Grade));
        cfg.stat_cycle_s = 5;
        let period = cfg.stat_cycle_s as u32 * 1000;
        let t0 = u32::MAX - 1_000; // anchor 1 s before the wrap
        let mut s = StatisticsScreen::new();
        assert!(!s.tick_timers(t0, &cfg).changed, "the anchoring frame doesn't flip");
        assert!(!s.tick_timers(t0.wrapping_add(period - 1), &cfg).changed, "still dwelling across the wrap");
        assert!(s.tick_timers(t0.wrapping_add(period), &cfg).changed, "flips a full period later, across the wrap");
        assert_eq!(s.page, 1);
    }

    /// Near the `u32` millis wrap, an `now + IDLE_MS` deadline would overflow. The `wrapping_sub`
    /// elapsed check must behave identically straddling the wrap.
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

    // Zoom-mode math + clamps: `on_turn` in Zoom multiplies by `ZOOM_STEP` per detent and clamps to
    // [MIN_ZOOM, MAX_ZOOM]. A dropped clamp would zoom to a degenerate/inverted window; a `+`/`pow`
    // instead of the per-detent multiply would mis-scale.

    /// A helper screen frozen in Zoom mode at full zoom — the state `Hold` lands in.
    fn zoom_screen() -> StatisticsScreen {
        let mut s = StatisticsScreen::new();
        s.cursor = Some(0.5); // a frozen centre (Hold sets this)
        s.zoom = MIN_ZOOM; // zoom starts at full (1.0)
        s.mode = Mode::Zoom;
        s
    }

    /// Two detents in Zoom mode compound to `ZOOM_STEP²`, not `2·ZOOM_STEP` — the per-detent
    /// geometric step.
    #[test]
    fn zoom_in_multiplies_per_detent() {
        let mut s = zoom_screen();
        s.on_turn(1, LIVE, 0);
        assert!((s.zoom - ZOOM_STEP).abs() < 1e-5, "one detent is ×ZOOM_STEP, got {}", s.zoom);
        s.on_turn(1, LIVE, 0);
        assert!((s.zoom - ZOOM_STEP * ZOOM_STEP).abs() < 1e-4, "two detents compound, got {}", s.zoom);
    }

    /// A `Turn(3)` compounds to `ZOOM_STEP³` in one call, matching three separate detents.
    #[test]
    fn zoom_multi_detent_turn_compounds_in_one_call() {
        let mut s = zoom_screen();
        s.on_turn(3, LIVE, 0);
        let expect = ZOOM_STEP * ZOOM_STEP * ZOOM_STEP;
        assert!((s.zoom - expect).abs() < 1e-4, "Turn(3) compounds three steps, got {}", s.zoom);
    }

    /// A backward turn at full zoom can't drive the zoom under 1× and invert the span (lower clamp).
    #[test]
    fn zoom_out_at_full_is_clamped_at_min() {
        let mut s = zoom_screen(); // already at MIN_ZOOM
        s.on_turn(-1, LIVE, 0);
        assert_eq!(s.zoom, MIN_ZOOM, "can't zoom out past the whole route");
        s.on_turn(-5, LIVE, 0);
        assert_eq!(s.zoom, MIN_ZOOM, "a long backward flick saturates at full, not below");
    }

    /// A huge forward turn saturates at `MAX_ZOOM` instead of running away.
    #[test]
    fn zoom_in_saturates_at_max() {
        let mut s = zoom_screen();
        s.on_turn(100, LIVE, 0);
        assert_eq!(s.zoom, MAX_ZOOM, "an enormous forward flick saturates at MAX_ZOOM, not beyond");
    }

    /// A turn in Cursor mode scrubs and forces the zoom back to 1.0, so zoom state can't leak in
    /// from a scrub.
    #[test]
    fn cursor_mode_turn_keeps_zoom_at_full() {
        let mut s = StatisticsScreen::new(); // Cursor mode, zoom 1.0
        s.on_turn(3, LIVE, 0);
        assert_eq!(s.zoom, 1.0, "a scrub leaves the zoom at full");
    }
}
