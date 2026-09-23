//! The Statistics screen: the route elevation profile, an inspection cursor, a progress bar, and a
//! grid of ride stats. Follow tracks the live position; Inspect freezes the cursor and keeps its
//! zoom, so a change between the Pan and Zoom tools does not discard the chosen window.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::app::step_zoom;
use crate::input::Gesture;
use crate::settings::Settings;
use crate::stat_fields;
use crate::Msg;

use super::vocab::band::ElevationBand;
use super::vocab::chrome::title_frame;
use super::vocab::fmt::write_distance_coarse;
use super::vocab::tiles::{category_tile, tile, waypoint_panel};
use super::{palette, ClimbScreen, Ctx, MapScreen, Render, Screen, ScreenTick, Transition};

/// Cursor scrub per Up/Down step, as a fraction of the whole route.
const CURSOR_STEP_FRAC: f32 = 1.0 / 42.0;
/// Zoom clamps; `1.0` = the whole route.
const MIN_ZOOM: f32 = 1.0;
const MAX_ZOOM: f32 = 8.0;
// Chart geometry (px) for the 240×320 panel.
const CHART_TOP: i32 = 42;
const CHART_BOT: i32 = 110;
/// The peak elevation maps here (a few px below `CHART_TOP`) so the apex clears the bar.
const BAND_TOP: i32 = CHART_TOP + 4;
const SIDE_MARGIN: i32 = 12;
/// "Near the peak" for the cursor elevation label, in screen px. Inside this distance the label
/// draws below the dot, so it cannot overlap the apex.
const PEAK_NEAR_PX: i32 = 36;

// Waypoint ticks on the progress bar, inset inside the 8 px bar so its rounded ends stay clean.
const WP_TICK_W: i32 = 2;
const WP_TICK_H: i32 = 6;
const WP_TICK_INSET_Y: i32 = 1;

/// Statistics interaction state: live Follow, or one of the two persistent Inspect tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Follow,
    Pan,
    Zoom,
}

impl Mode {
    fn inspecting(self) -> bool {
        self != Mode::Follow
    }
}

#[derive(Debug)]
pub struct StatisticsScreen {
    mode: Mode,
    /// Inspection cursor as a route fraction; `None` = track the live position.
    cursor: Option<f32>,
    /// Zoom factor; `1.0` = the full route.
    zoom: f32,
    /// Which page of the stat grid is showing.
    page: usize,
    /// Instant of the last page flip; `None` until the first frame anchors it.
    last_flip_ms: Option<u32>,
}

impl Default for StatisticsScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl StatisticsScreen {
    pub fn new() -> Self {
        StatisticsScreen { mode: Mode::Follow, cursor: None, zoom: MIN_ZOOM, page: 0, last_flip_ms: None }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let live = stat_fields::live_frac(cx.navigator.route_state());
        let has_route = cx.navigator.route_state().active_route.is_some();
        match g {
            Gesture::Step(n) => {
                if has_route {
                    if self.mode == Mode::Follow {
                        self.begin_inspect(live);
                    }
                    self.on_step(n, live);
                }
                Transition::None
            }
            // Once Inspect is active, Hold is inert so it cannot discard the chosen cursor and window.
            Gesture::Hold => {
                if has_route && self.mode == Mode::Follow {
                    self.begin_inspect(live);
                }
                Transition::None
            }
            Gesture::Back => match self.mode {
                Mode::Pan | Mode::Zoom => {
                    self.reset();
                    Transition::None
                }
                // The middle hop of the Back-cycle: on to Climb when a climb is active and the
                // Climb screen is on, else back to the Map.
                Mode::Follow => {
                    if cx.settings.climb_mode.is_on() && cx.navigator.route_state().active_climb.is_some() {
                        Transition::Replace(Screen::Climb(ClimbScreen::new()))
                    } else {
                        Transition::Replace(Screen::Map(MapScreen::new()))
                    }
                }
            },
            Gesture::Press => match self.mode {
                Mode::Follow => super::riding_common(g, cx),
                Mode::Pan => {
                    self.mode = Mode::Zoom;
                    Transition::None
                }
                Mode::Zoom => {
                    self.mode = Mode::Pan;
                    Transition::None
                }
            },
            // Back-hold is the global escape, resolved above screen dispatch.
            Gesture::BackHold => Transition::None,
        }
    }

    fn begin_inspect(&mut self, live: f32) {
        self.mode = Mode::Pan;
        self.cursor = Some(live);
        self.zoom = MIN_ZOOM;
    }

    /// Return to Follow: the cursor tracks the live position, at the full route scale.
    fn reset(&mut self) {
        self.mode = Mode::Follow;
        self.cursor = None;
        self.zoom = MIN_ZOOM;
    }

    /// Poll the stat grid page auto-cycle. With more than one page the view dwells
    /// [`stat_cycle_s`](Settings::stat_cycle_s) on each; the first poll anchors the timer, so page 0
    /// gets a full dwell. The elapsed check wraps, so it stays correct across the `u32` millis wrap.
    pub fn tick_timers(&mut self, now_ms: u32, settings: &Settings) -> ScreenTick {
        let mut changed = false;

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

        ScreenTick { changed, next_wake_ms: page, region: None }
    }

    fn effective_cursor(&self, live: f32) -> f32 {
        if self.mode.inspecting() {
            self.cursor.unwrap_or(live)
        } else {
            live
        }
    }

    fn on_step(&mut self, n: i32, live: f32) {
        match self.mode {
            Mode::Pan => {
                let c = self.effective_cursor(live);
                // Scale the step by the zoom, to keep the same on-glass travel at every zoom.
                let step = CURSOR_STEP_FRAC / self.zoom.max(MIN_ZOOM);
                self.cursor = Some((c + n as f32 * step).clamp(0.0, 1.0));
            }
            Mode::Zoom => {
                self.zoom = step_zoom(self.zoom, n, MIN_ZOOM, MAX_ZOOM);
            }
            Mode::Follow => {}
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        // With no route loaded, the title bar and the stat grid still draw. The chart region shows
        // a "No route loaded" note, and the progress bar draws empty.
        let (Some(profile), Some(route)) = (rx.profile, rx.route) else {
            title_frame(cv, w, h, rx.t(Msg::StatsTitle), if rx.no_fix { rx.t(Msg::StatsNoGps) } else { "" });
            cv.text(
                rx.t(Msg::StatsNoRoute),
                Point::new(w / 2, (CHART_TOP + CHART_BOT) / 2 - 9),
                Font::Label,
                TextAlign::Center,
                palette::SUBTEXT,
            );
            cv.hline(SIDE_MARGIN, CHART_BOT + 1, w - 2 * SIDE_MARGIN, palette::RULE);
            let prog_y = CHART_BOT + 10;
            cv.round(rect(SIDE_MARGIN, prog_y, w - 2 * SIDE_MARGIN, 8), 4, palette::PARCHMENT_SHADE);
            self.draw_stat_grid(cv, rx, prog_y + 16);
            self.draw_profile_tool(cv);
            return;
        };

        let total = route.total_distance_m;
        let off = rx.navigation.off_route;
        let units = rx.settings.units;

        // The live position drives the traveled shading and the progress bar. The cursor can be
        // scrubbed away from it, and is the zoom centre.
        let live_frac = if total > 0 { (rx.navigation.progress_m as f32 / total as f32).clamp(0.0, 1.0) } else { 0.0 };
        let cursor_frac = self.effective_cursor(live_frac);
        let zoom = if self.mode.inspecting() { self.zoom } else { MIN_ZOOM };
        let scrubbing = (cursor_frac - live_frac).abs() > 1e-4;

        let chart_x = SIDE_MARGIN;
        let chart_w = w - 2 * SIDE_MARGIN;
        let win = profile.window(cursor_frac, zoom, chart_w.max(1) as u32);
        let band = ElevationBand::new(profile, win, rect(chart_x, BAND_TOP, chart_w, CHART_BOT - BAND_TOP + 1));

        // The cursor stays amber while scrubbing: it marks an inspection point, not the rider.
        let live_color = if off { WARNING } else { AMBER };
        let cursor_color = if off && !scrubbing { WARNING } else { AMBER };

        let mut readout: heapless::String<16> = heapless::String::new();
        if rx.no_fix {
            let _ = readout.push_str(rx.t(Msg::StatsNoGps));
        } else if off && rx.navigation.dist_to_route_m == u32::MAX {
            let _ = readout.push_str(rx.t(Msg::StatsOff).trim_end());
        } else if off {
            write_distance_coarse(&mut readout, rx.t(Msg::StatsOff), rx.navigation.dist_to_route_m, units);
        } else {
            if let Some(grade) = stat_fields::grade_at(profile, total, cursor_frac) {
                let _ = write!(readout, "{}{}%", rx.t(Msg::StatsGrade), grade);
            } else {
                let _ = readout.push_str("—");
            }
        }
        title_frame(cv, w, h, rx.t(Msg::StatsTitle), &readout);

        band.fill(cv, PARCHMENT_SHADE);
        for px in 0..chart_w {
            if band.frac(px) <= live_frac {
                band.fill_column(cv, px, SUBTEXT);
            }
        }
        band.stroke(cv, AMBER);
        cv.hline(chart_x, CHART_BOT + 1, chart_w, RULE);

        let cursor_x = band.frac_to_x(cursor_frac).clamp(chart_x, chart_x + chart_w - 1);
        let cursor_band = profile.at(cursor_frac);
        let cur_ele = if cursor_band.0 <= cursor_band.1 { cursor_band.1 } else { profile.min_ele_m };
        let cur_y = band.ele_to_y(cur_ele);
        cv.vline(cursor_x, CHART_TOP, CHART_BOT - CHART_TOP + 1, 2, cursor_color);
        if cursor_band.0 <= cursor_band.1 {
            cv.disc(Point::new(cursor_x, cur_y), 4, INK);
            cv.disc(Point::new(cursor_x, cur_y), 3, cursor_color);
        }
        let mut ele_s: heapless::String<8> = heapless::String::new();
        if cursor_band.0 <= cursor_band.1 {
            let _ = write!(ele_s, "{} {}", units.elev(cur_ele as f32) as i32, units.elev_label());
        } else {
            let _ = ele_s.push_str("—");
        }
        let peak_x = band.frac_to_x(profile.peak_frac());
        let near_peak = (chart_x..chart_x + chart_w).contains(&peak_x) && (cursor_x - peak_x).abs() < PEAK_NEAR_PX;
        let label_y = (if near_peak { cur_y + 9 } else { cur_y - 5 }).clamp(CHART_TOP + 2, CHART_BOT - 24);
        if cursor_x < w - 44 {
            cv.text(&ele_s, Point::new(cursor_x + 8, label_y), Font::Label, TextAlign::Left, INK);
        } else {
            cv.text(&ele_s, Point::new(cursor_x - 8, label_y), Font::Label, TextAlign::Right, INK);
        }

        let prog_y = CHART_BOT + 10;
        cv.round(rect(chart_x, prog_y, chart_w, 8), 4, PARCHMENT_SHADE);
        let fill_w = (chart_w as f32 * live_frac) as i32;
        if fill_w > 0 {
            cv.round(rect(chart_x, prog_y, fill_w, 8), 4, live_color);
        }

        // Draw the ticks after the fill and always in INK: the bar tints WARNING-red off-route, and
        // red ticks would not be visible against it.
        for wp in rx.waypoints.as_slice() {
            if let Some(x) = waypoint_tick_x(wp.dist_along_m, total, chart_x, chart_w) {
                cv.vline(x, prog_y + WP_TICK_INSET_Y, WP_TICK_H, WP_TICK_W, INK);
            }
        }

        self.draw_stat_grid(cv, rx, prog_y + 16);
        self.draw_profile_tool(cv);
    }

    /// Draw the rider's stat fields, paginated 3×2, with the top of the grid at `grid_top`.
    fn draw_stat_grid(&self, cv: &mut impl Surface, rx: &mut Render, grid_top: i32) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        let chart_x = SIDE_MARGIN;
        let chart_w = w - 2 * SIDE_MARGIN;
        let fields = &rx.settings.stat_fields;
        let page = self.page.min(stat_fields::page_count(fields) - 1);

        let gap = 6;
        let col_w = (chart_w - gap) / 2;
        let row_h = ((h - 10 - grid_top - 2 * gap) / stat_fields::ROWS_PER_PAGE as i32).max(20);
        let cx = rx.readout();
        for placed in stat_fields::page_fields(fields, page) {
            let x = chart_x + placed.col as i32 * (col_w + gap);
            let y = grid_top + placed.row as i32 * (row_h + gap);
            // The waypoint panel bypasses `cell()`: its list does not fit the caption+value shape
            // of a tile. It always starts a page at col 0 / row 0 and fills the whole grid.
            if placed.field.rows() > 1 {
                let panel_h = row_h * stat_fields::ROWS_PER_PAGE as i32 + gap * (stat_fields::ROWS_PER_PAGE as i32 - 1);
                waypoint_panel(cv, rect(chart_x, y, chart_w, panel_h), &cx, PARCHMENT_SHADE);
                continue;
            }
            let cell = placed.field.cell(&cx);
            let tile_w = if placed.field.span() == 2 { chart_w } else { col_w };
            let area = rect(x, y, tile_w, row_h);
            // A `Next: <category>` tile puts the category icon in front of its caption, which moves
            // the caption, so it has its own drawer.
            match placed.field.category() {
                Some(cat) => {
                    category_tile(cv, area, cat, &cell.caption, &cell.value, PARCHMENT_SHADE, INK);
                }
                None => tile(
                    cv,
                    area,
                    &rx.marquee,
                    &cell.caption,
                    cell.caption_climb.as_deref(),
                    &cell.value,
                    cell.arrow,
                    cell.value_align,
                    PARCHMENT_SHADE,
                    INK,
                ),
            }
        }
    }

    /// Zoom's only extra chrome: an amber −/+ pair at the left of the chart. Pan needs no marker.
    fn draw_profile_tool(&self, cv: &mut impl Surface) {
        if self.mode != Mode::Zoom {
            return;
        }

        let cx = (SIDE_MARGIN + 12) as f32;
        super::map::draw_zoom_cue(cv, (cx, (CHART_TOP + 10) as f32), false);
        super::map::draw_zoom_cue(cv, (cx, (CHART_TOP + 36) as f32), true);
    }
}

/// Map a waypoint's along-route distance to the x of its tick in the progress bar. The clamp keeps
/// the full [`WP_TICK_W`] px tick inside the bar. Returns `None` for a zero-length route.
fn waypoint_tick_x(dist_along_m: u32, total: u32, chart_x: i32, chart_w: i32) -> Option<i32> {
    if total == 0 {
        return None;
    }
    let frac = dist_along_m as f32 / total as f32;
    let x = chart_x + (chart_w as f32 * frac) as i32;
    Some(x.clamp(chart_x, chart_x + chart_w - WP_TICK_W))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::app::ZOOM_STEP;
    use crate::screen::test_ctx;
    use crate::settings::ClimbMode;
    use crate::AppState;

    /// Drive `handle` with a set climb mode and active-climb state.
    fn back_with(mode: ClimbMode, active_climb: Option<usize>) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        // Fully-qualified, so the local `Mode` does not shadow the ride `Mode`.
        let mut act = Activity::new(crate::activity::Mode::Riding);
        let mut navigator = crate::navigator::NavigatorMachine::new();
        navigator.route_state_mut().active_climb = active_climb;
        let mut s = Settings { climb_mode: mode, ..Settings::default() };
        let mut cx = Ctx { navigator: &mut navigator, ..test_ctx(&mut st, &mut act, &mut s) };
        StatisticsScreen::new().handle(Gesture::Back, &mut cx)
    }

    #[test]
    fn back_off_climb_is_the_two_cycle() {
        for mode in [ClimbMode::Off, ClimbMode::Manual, ClimbMode::Auto] {
            assert!(matches!(back_with(mode, None), Transition::Replace(Screen::Map(_))), "{mode:?} off-climb → Map");
        }
    }

    #[test]
    fn back_on_climb_inserts_the_climb_hop() {
        for mode in [ClimbMode::Manual, ClimbMode::Auto] {
            assert!(
                matches!(back_with(mode, Some(0)), Transition::Replace(Screen::Climb(_))),
                "{mode:?} on-climb → Climb"
            );
        }
    }

    #[test]
    fn back_off_mode_never_routes_to_climb() {
        assert!(
            matches!(back_with(ClimbMode::Off, Some(0)), Transition::Replace(Screen::Map(_))),
            "Off keeps Climb out of the Back-cycle even mid-climb"
        );
    }

    const LIVE: f32 = 0.5;
    fn scrubbed() -> f32 {
        (LIVE + CURSOR_STEP_FRAC).clamp(0.0, 1.0)
    }

    /// Drive one gesture with a loaded route and a 50%-along live point.
    fn profile_gesture(s: &mut StatisticsScreen, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(crate::activity::Mode::Riding);
        let mut navigator = crate::navigator::NavigatorMachine::new();
        navigator.route_state_mut().active_route = Some(0);
        navigator.route_state_mut().route_total_m = 1_000;
        navigator.route_state_mut().progress_m = 500;
        let mut settings = Settings::default();
        let mut cx = Ctx { navigator: &mut navigator, ..test_ctx(&mut st, &mut act, &mut settings) };
        s.handle(g, &mut cx)
    }

    #[test]
    fn step_enters_persistent_pan_from_live() {
        let mut s = StatisticsScreen::new();
        assert!(matches!(profile_gesture(&mut s, Gesture::Step(1)), Transition::None));
        assert_eq!(s.mode, Mode::Pan);
        assert_eq!(s.effective_cursor(LIVE), scrubbed());
        assert_eq!(s.zoom, MIN_ZOOM);

        let cfg = Settings::default();
        assert!(!s.tick_timers(1_000_000, &cfg).changed);
        assert_eq!(s.effective_cursor(LIVE), scrubbed(), "Inspect never auto-snaps away from the chosen point");
    }

    #[test]
    fn inspect_toggles_pan_zoom_without_losing_the_window() {
        let mut s = StatisticsScreen::new();
        profile_gesture(&mut s, Gesture::Hold);
        assert_eq!(s.mode, Mode::Pan);
        assert_eq!(s.cursor, Some(LIVE));

        profile_gesture(&mut s, Gesture::Press);
        assert_eq!(s.mode, Mode::Zoom);
        profile_gesture(&mut s, Gesture::Step(3));
        let (cursor, zoom) = (s.cursor, s.zoom);
        assert!(zoom > MIN_ZOOM);

        profile_gesture(&mut s, Gesture::Press);
        assert_eq!(s.mode, Mode::Pan);
        assert_eq!((s.cursor, s.zoom), (cursor, zoom), "Zoom → Pan retains the chosen window");
        profile_gesture(&mut s, Gesture::Hold);
        assert_eq!((s.mode, s.cursor, s.zoom), (Mode::Pan, cursor, zoom), "hold is inert inside Inspect");

        profile_gesture(&mut s, Gesture::Press);
        assert_eq!((s.mode, s.cursor, s.zoom), (Mode::Zoom, cursor, zoom), "Pan → Zoom retains it too");
    }

    #[test]
    fn back_exits_inspect_and_recentres_before_navigating() {
        let mut s = StatisticsScreen::new();
        profile_gesture(&mut s, Gesture::Step(2));
        profile_gesture(&mut s, Gesture::Press); // Zoom
        profile_gesture(&mut s, Gesture::Step(2));
        assert!(matches!(profile_gesture(&mut s, Gesture::Back), Transition::None));
        assert_eq!((s.mode, s.cursor, s.zoom), (Mode::Follow, None, MIN_ZOOM));
        assert!(matches!(profile_gesture(&mut s, Gesture::Back), Transition::Replace(Screen::Map(_))));
    }

    #[test]
    fn page_auto_cycles_on_the_timer() {
        let mut cfg = Settings::default();
        assert!(cfg.stat_fields.push(crate::stat_fields::StatField::Grade), "7 fields → two pages");
        cfg.stat_cycle_s = 5;
        let period = cfg.stat_cycle_s as u32 * 1000;
        let mut s = StatisticsScreen::new();
        assert!(!s.tick_timers(10_000, &cfg).changed, "the anchoring frame doesn't flip");
        assert_eq!(s.page, 0);
        assert!(!s.tick_timers(10_000 + period - 1, &cfg).changed, "still dwelling just before the deadline");
        assert_eq!(s.page, 0);
        assert!(s.tick_timers(10_000 + period, &cfg).changed, "flips to page 1 at the deadline → dirty");
        assert_eq!(s.page, 1);
        assert!(s.tick_timers(10_000 + 2 * period, &cfg).changed, "and wraps back to page 0 a period later");
        assert_eq!(s.page, 0);
    }

    #[test]
    fn single_page_grid_never_flips() {
        let cfg = Settings::default();
        let mut s = StatisticsScreen::new();
        assert!(!s.tick_timers(1_000, &cfg).changed);
        assert!(!s.tick_timers(1_000_000, &cfg).changed, "one page never flips");
        assert_eq!(s.page, 0);
    }

    #[test]
    fn next_wake_tracks_only_the_page_dwell() {
        let mut cfg = Settings::default();
        assert!(cfg.stat_fields.push(crate::stat_fields::StatField::Grade), "7 fields → two pages");
        cfg.stat_cycle_s = 5;
        let period = cfg.stat_cycle_s as u32 * 1000;
        let mut s = StatisticsScreen::new();
        assert_eq!(s.tick_timers(10_000, &cfg).next_wake_ms, Some(period), "the anchoring poll = the full period");
        assert_eq!(s.tick_timers(10_000 + 2_000, &cfg).next_wake_ms, Some(period - 2_000), "2 s into the dwell");
        s.begin_inspect(LIVE);
        s.on_step(1, LIVE);
        assert_eq!(
            s.tick_timers(10_000 + 2_000, &cfg).next_wake_ms,
            Some(period - 2_000),
            "Inspect does not add an auto-return wake"
        );
    }

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

    fn zoom_screen() -> StatisticsScreen {
        let mut s = StatisticsScreen::new();
        s.cursor = Some(0.5);
        s.zoom = MIN_ZOOM;
        s.mode = Mode::Zoom;
        s
    }

    #[test]
    fn zoom_in_multiplies_per_step() {
        let mut s = zoom_screen();
        s.on_step(1, LIVE);
        assert!((s.zoom - ZOOM_STEP).abs() < 1e-5, "one step is ×ZOOM_STEP, got {}", s.zoom);
        s.on_step(1, LIVE);
        assert!((s.zoom - ZOOM_STEP * ZOOM_STEP).abs() < 1e-4, "two steps compound, got {}", s.zoom);
    }

    #[test]
    fn zoom_multi_step_turn_compounds_in_one_call() {
        let mut s = zoom_screen();
        s.on_step(3, LIVE);
        let expect = ZOOM_STEP * ZOOM_STEP * ZOOM_STEP;
        assert!((s.zoom - expect).abs() < 1e-4, "Step(3) compounds three steps, got {}", s.zoom);
    }

    #[test]
    fn zoom_out_at_full_is_clamped_at_min() {
        let mut s = zoom_screen();
        s.on_step(-1, LIVE);
        assert_eq!(s.zoom, MIN_ZOOM, "can't zoom out past the whole route");
        s.on_step(-5, LIVE);
        assert_eq!(s.zoom, MIN_ZOOM, "a long backward flick saturates at full, not below");
    }

    #[test]
    fn zoom_in_saturates_at_max() {
        let mut s = zoom_screen();
        s.on_step(100, LIVE);
        assert_eq!(s.zoom, MAX_ZOOM, "an enormous forward flick saturates at MAX_ZOOM, not beyond");
    }

    #[test]
    fn pan_keeps_zoom_and_scales_its_step() {
        let mut s = StatisticsScreen::new();
        s.mode = Mode::Pan;
        s.cursor = Some(LIVE);
        s.zoom = 4.0;
        s.on_step(1, LIVE);
        assert_eq!(s.zoom, 4.0, "panning retains the zoomed window");
        assert!((s.cursor.unwrap() - (LIVE + CURSOR_STEP_FRAC / 4.0)).abs() < 1e-6);
    }

    #[test]
    fn waypoint_tick_x_guards_zero_total_and_clamps_to_the_bar() {
        let (cx, cw) = (SIDE_MARGIN, 240 - 2 * SIDE_MARGIN);
        assert_eq!(waypoint_tick_x(100, 0, cx, cw), None, "a zero-length route places no tick");
        assert_eq!(waypoint_tick_x(0, 1000, cx, cw), Some(cx), "frac 0 → left edge");
        assert_eq!(waypoint_tick_x(500, 1000, cx, cw), Some(cx + cw / 2), "frac 0.5 → centre");
        assert_eq!(
            waypoint_tick_x(1000, 1000, cx, cw),
            Some(cx + cw - WP_TICK_W),
            "frac 1 → clamped flush against the right edge, not one column past it"
        );
        assert_eq!(
            waypoint_tick_x(5000, 1000, cx, cw),
            Some(cx + cw - WP_TICK_W),
            "past the route end clamps, never overflows"
        );
    }
}
