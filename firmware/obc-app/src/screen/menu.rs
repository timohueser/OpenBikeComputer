//! The Menu overlay — **layout prototype pass**: two candidate designs behind the [`COMPASS`]
//! switch. Both show the five entries (Routes / Rides / POIs / Map / Settings) and keep the list
//! semantics: `turn` moves the selection (wrapping), `press` enters, `back` returns to the caller.
//! Four stations open a menu; the **Map** station opens the Map screen directly (see [`open_map`]) —
//! the live riding map while tracking, else a route-less browse map.
//!
//! * Compass dial: a wood bezel ring with the five entries as stations evenly spaced around the
//!   ring (72° apart, starting from N — see [`station_dir`]), an amber needle that *sweeps* to the
//!   selection (an ease-out driven through [`tick_timers`](MenuScreen::tick_timers)), and the
//!   selected name in Display type below.
//! * Card grid: the conventional icon-card layout under the standard title bar.

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;

use super::{
    list, palette, title_frame_ble, Ctx, MapScreen, PoiMenuScreen, Render, RidesScreen, RouteMenuScreen, Screen,
    ScreenTick, SettingsScreen, Transition,
};

const ITEMS: [&str; 5] = ["Routes", "Rides", "POIs", "Map", "Settings"];

/// Prototype switch: `true` draws the compass dial, `false` the 2×2 card grid.
const COMPASS: bool = true;

/// The station's unit direction for item `i` of `n`, starting at N (0°) and stepping clockwise, so a
/// clockwise encoder turn walks the ring clockwise. Replaces the old fixed N/E/S/W table now that the
/// menu holds five entries (`Rides` joined Routes/POIs/Map/Settings).
fn station_dir(i: usize, n: usize) -> (f32, f32) {
    let a = (i as f32 / n as f32) * core::f32::consts::TAU;
    (libm::sinf(a), -libm::cosf(a)) // screen coords: 0° = up, clockwise positive
}

/// Degrees the needle sweeps per encoder detent — one station step around the ring of [`ITEMS`]
/// (360° ÷ five entries = 72°).
const DETENT_DEG: f32 = 360.0 / ITEMS.len() as f32;

/// Needle sweep tuning: an ease-out — the needle moves at `SWEEP_RATE` of the remaining arc per
/// second, floored at `SWEEP_MIN_DEG_S` so the tail doesn't crawl. A one-detent step lands in ≈200 ms.
const SWEEP_RATE: f32 = 8.0;
const SWEEP_MIN_DEG_S: f32 = 180.0;
/// The frame cadence the sweep asks the host for while in flight.
const SWEEP_FRAME_MS: u32 = 16;

/// The main menu. State is the highlighted entry plus the needle sweep: `target_deg` accumulates
/// ±[`DETENT_DEG`] per detent (so the needle always follows the *turn direction*, including across
/// the wrap) and `needle_deg` chases it in [`tick_timers`](Self::tick_timers); both are normalized
/// back into one revolution when the sweep lands.
#[derive(Debug, Default)]
pub struct MenuScreen {
    selected: usize,
    needle_deg: f32,
    target_deg: f32,
    /// Clock of the previous sweep tick, for the per-frame `dt`; `None` while the needle rests.
    last_anim_ms: Option<u32>,
}

impl MenuScreen {
    pub fn new() -> Self {
        MenuScreen::default()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => {
                self.target_deg += n as f32 * DETENT_DEG;
                list::on_turn(&mut self.selected, n, ITEMS.len())
            }
            Gesture::Press => match self.selected {
                0 => Transition::Push(Screen::RouteMenu(RouteMenuScreen::new())), // Routes
                1 => Transition::Push(Screen::Rides(RidesScreen::new())),         // Rides
                2 => Transition::Push(Screen::PoiMenu(PoiMenuScreen::new())),     // POIs
                3 => open_map(cx),                                                // Map
                _ => Transition::Push(Screen::Settings(SettingsScreen::new())),   // Settings
            },
            Gesture::Back => Transition::Pop, // return to caller (Home or Map)
            Gesture::Hold => Transition::None,
            Gesture::BackHold => Transition::None, // Shutdown prompt — later slice
        }
    }

    /// [`Screen::tick_timers`] arm: advance the needle toward the target by the eased step for the
    /// elapsed `dt`, requesting a [`SWEEP_FRAME_MS`] wake while in flight. Settled (the common
    /// case) is [`ScreenTick::idle`] — a resting menu costs no timed repaints.
    pub fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        let diff = self.target_deg - self.needle_deg;
        if diff == 0.0 {
            self.last_anim_ms = None;
            return ScreenTick::idle();
        }
        let last = self.last_anim_ms.replace(now_ms).unwrap_or(now_ms);
        // Cap dt so a stalled host (or the first tick after a long pause) steps, not teleports.
        let dt = (now_ms.wrapping_sub(last) as f32 / 1000.0).min(0.1);
        let step = (diff.abs() * SWEEP_RATE).max(SWEEP_MIN_DEG_S) * dt;
        if step >= diff.abs() {
            // Landed: snap to the target and fold both angles back into one revolution
            // (`%` is core; `rem_euclid` on floats is std-only, hence the manual sign fix).
            let mut landed = self.target_deg % 360.0;
            if landed < 0.0 {
                landed += 360.0;
            }
            self.needle_deg = landed;
            self.target_deg = landed;
            self.last_anim_ms = None;
            return ScreenTick { changed: true, next_wake_ms: None, region: None };
        }
        self.needle_deg += if diff > 0.0 { step } else { -step };
        ScreenTick { changed: step > 0.0, next_wake_ms: Some(SWEEP_FRAME_MS), region: None }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let ble = rx.state.ble_connected();
        if COMPASS {
            draw_compass(cv, rx.w, rx.h, self.selected, self.needle_deg, ble);
        } else {
            draw_grid(cv, rx.w, rx.h, self.selected, ble);
        }
    }
}

/// Open the Map station. **While tracking**, land the rider on the live riding map — the ride base
/// — by rooting the stack to a clean `[Home, Map]`, exactly the normalization
/// [`App::apply_idle_return`](crate::App::apply_idle_return) does when the idle timeout returns a
/// tracking rider to the Map (so a second Map is never stacked and stale overlays are cleared). The
/// camera is already following in the riding view. **Not tracking**, it's a route-less *browse* map:
/// enter the riding view (GPS-follow, zoomed in) and push the Map over the Menu, so `back` returns
/// here and `press` opens the start card.
fn open_map(cx: &mut Ctx) -> Transition {
    if cx.activity.is_tracking() {
        return Transition::Root(Screen::Map(MapScreen::new()));
    }
    // Seed the browse camera on the rider (last fix) if there is one; Follow recenters on each fix.
    if let Some(fix) = cx.state.user_fix {
        cx.state.enter_riding_view(fix.lon, fix.lat);
    } else {
        cx.state.enter_riding_view(cx.state.cam_lon, cx.state.cam_lat);
    }
    Transition::Push(Screen::Map(MapScreen::new()))
}

/// The compass-dial layout under the standard title bar: bezel ring, intercardinal ticks, needle,
/// four icon stations, and the selected entry's name in Display type at the bottom. The ring sits
/// centred between the bar and the name strip — which works out to exactly `h / 2`. The needle
/// points at `needle_deg` (0° = N, clockwise) — mid-sweep that's between stations; the station
/// highlight and the name snap to the selection immediately.
fn draw_compass(cv: &mut impl Surface, w: i32, h: i32, selected: usize, needle_deg: f32, ble_connected: bool) {
    use palette::*;
    title_frame_ble(cv, w, h, "MENU", "", ble_connected);

    let c = Point::new(w / 2, h / 2);

    // Bezel ring: a wood disc with the parchment punched back out of the middle.
    cv.disc(c, 106, WOOD);
    cv.disc(c, 98, PARCHMENT);
    // Intercardinal ticks just inside the bezel (the cardinal slots hold the stations) —
    // doubled 1px lines for a visible 2px stroke; 62/68 are the 45° components of r 88→96.
    for (sx, sy) in [(1, -1), (1, 1), (-1, 1), (-1, -1)] {
        for off in 0..2 {
            cv.line(
                Point::new(c.x + sx * 62 + off, c.y + sy * 62),
                Point::new(c.x + sx * 68 + off, c.y + sy * 68),
                WOOD,
            );
        }
    }

    draw_needle(cv, c, needle_deg, 42.0, 10.0);

    // Stations: amber-filled when selected, a thin tan ring otherwise. Evenly spaced around the ring
    // by `station_dir`, so the five entries sit at 72° detents starting from N.
    let n = ITEMS.len();
    for i in 0..n {
        let (dx, dy) = station_dir(i, n);
        let sc = Point::new(c.x + si(1.0, dx * 72.0), c.y + si(1.0, dy * 72.0));
        let is_sel = i == selected;
        if is_sel {
            cv.disc(sc, 24, AMBER);
        } else {
            cv.disc(sc, 24, RULE);
            cv.disc(sc, 21, PARCHMENT);
        }
        let ink = if is_sel { INK } else { SUBTEXT };
        let bg = if is_sel { AMBER } else { PARCHMENT };
        draw_icon(cv, i, sc, 1.2, ink, bg);
    }

    cv.text(ITEMS[selected], Point::new(w / 2, h - 38), Font::Display, TextAlign::Center, INK);
}

/// Draw the compass **needle** centred at `c`, pointing `deg` (0° = N, clockwise): amber head of
/// length `r`, grey counterweight, ink hub with a parchment cap. Screen coords: the direction is
/// `(sin, -cos)` and its perpendicular `(cos, sin)`; `half_w` is the base half-width. Shared by
/// the Menu compass dial and the nav **planning** screen's spinner (#499), so the two needles can
/// never drift apart.
pub(super) fn draw_needle(cv: &mut impl Surface, c: Point, deg: f32, r: f32, half_w: f32) {
    use palette::*;
    let rad = deg.to_radians();
    let (dx, dy) = (libm::sinf(rad), -libm::cosf(rad));
    let (px, py) = (-dy, dx);
    let at = |ux: f32, uy: f32, d: f32| Point::new(c.x + si(1.0, ux * d), c.y + si(1.0, uy * d));
    let b1 = at(px, py, half_w);
    let b2 = at(-px, -py, half_w);
    cv.triangle(at(dx, dy, r), b1, b2, AMBER);
    cv.triangle(at(-dx, -dy, r), b1, b2, CONTOUR);
    cv.disc(c, 6, INK);
    cv.disc(c, 2, PARCHMENT);
}

/// The 2×2 card-grid layout under the standard title bar: amber fill on the selected card, a tan
/// outline on the rest, each with its icon over a centred label.
fn draw_grid(cv: &mut impl Surface, w: i32, h: i32, selected: usize, ble_connected: bool) {
    use palette::*;
    title_frame_ble(cv, w, h, "MENU", "", ble_connected);
    for (i, label) in ITEMS.iter().enumerate() {
        let col = (i % 2) as i32;
        let row = (i / 2) as i32;
        let (x, y) = (14 + col * 110, 51 + row * 132);
        let area = rect(x, y, 102, 124);
        let is_sel = i == selected;
        if is_sel {
            cv.round(area, 8, AMBER);
        } else {
            // Doubled 1px outlines for a 2px card edge.
            cv.round_outline(area, 8, RULE);
            cv.round_outline(rect(x + 1, y + 1, 100, 122), 7, RULE);
        }
        let ink = if is_sel { INK } else { SUBTEXT };
        let bg = if is_sel { AMBER } else { PARCHMENT };
        draw_icon(cv, i, Point::new(x + 51, y + 48), 1.5, ink, bg);
        cv.text(label, Point::new(x + 51, y + 92), Font::Label, TextAlign::Center, INK);
    }
}

/// Dispatch an entry's icon, centred at `c` and scaled by `k` (`1.0` fits a station disc, the
/// grid uses `1.5`). `bg` is the surface behind the icon, for punched-out details.
fn draw_icon(cv: &mut impl Surface, i: usize, c: Point, k: f32, color: u16, bg: u16) {
    match i {
        0 => icon_route(cv, c, k, color),
        1 => icon_rides(cv, c, k, color, bg),
        2 => icon_poi(cv, c, k, color, bg),
        3 => icon_map(cv, c, k, color),
        _ => icon_sliders(cv, c, k, color),
    }
}

/// The Rides glyph: a stopwatch — a round face with a top stem/button and a single hand, reading as
/// "recorded ride" (time + distance) distinct from the route icon's road line.
fn icon_rides(cv: &mut impl Surface, c: Point, k: f32, color: u16, bg: u16) {
    let r = si(k, 9.0) as u32;
    cv.disc(c, r, color);
    cv.disc(c, si(k, 6.5) as u32, bg); // punch the face out to a ring
                                       // Top button/stem.
    cv.fill(rect(c.x - si(k, 2.0), c.y - si(k, 13.0), si(k, 4.0), si(k, 4.0)), color);
    // A single hand from the centre up-right.
    cv.line(c, Point::new(c.x + si(k, 4.0), c.y - si(k, 4.0)), color);
    cv.disc(c, si(k, 1.5).max(1) as u32, color); // hub
}

/// Scale an icon-space offset by `k`, rounding away from zero so mirrored offsets stay symmetric.
fn si(k: f32, v: f32) -> i32 {
    let x = k * v;
    if x >= 0.0 {
        (x + 0.5) as i32
    } else {
        (x - 0.5) as i32
    }
}

/// A winding route: a cubic Bézier stroked by stamping discs, with fatter end caps.
fn icon_route(cv: &mut impl Surface, c: Point, k: f32, color: u16) {
    const B: [(f32, f32); 4] = [(-11.0, 7.0), (-5.0, -9.0), (4.0, 9.0), (12.0, -6.0)];
    let r = si(k, 2.0).max(1) as u32;
    for i in 0..=16 {
        let t = i as f32 / 16.0;
        let u = 1.0 - t;
        let (w0, w1, w2, w3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        let x = w0 * B[0].0 + w1 * B[1].0 + w2 * B[2].0 + w3 * B[3].0;
        let y = w0 * B[0].1 + w1 * B[1].1 + w2 * B[2].1 + w3 * B[3].1;
        cv.disc(Point::new(c.x + si(1.0, k * x), c.y + si(1.0, k * y)), r, color);
    }
    cv.disc(Point::new(c.x + si(k, -11.0), c.y + si(k, 7.0)), si(k, 3.0) as u32, color);
    cv.disc(Point::new(c.x + si(k, 12.0), c.y + si(k, -6.0)), si(k, 3.0) as u32, color);
}

/// A map pin: head disc, tapering tip, and a punched-out centre dot.
fn icon_poi(cv: &mut impl Surface, c: Point, k: f32, color: u16, bg: u16) {
    let head = Point::new(c.x, c.y + si(k, -5.0));
    cv.disc(head, si(k, 8.0) as u32, color);
    cv.triangle(
        Point::new(c.x + si(k, -6.0), c.y),
        Point::new(c.x + si(k, 6.0), c.y),
        Point::new(c.x, c.y + si(k, 12.0)),
        color,
    );
    cv.disc(head, si(k, 3.0) as u32, bg);
}

/// A folded map with a "you are here" dot: an outlined sheet, two *hairline* fold creases inset
/// from the edges (heavier bars read as a grill at this size), and a marker dot in the middle
/// panel — Map-without-a-route is exactly "just you on the map".
fn icon_map(cv: &mut impl Surface, c: Point, k: f32, color: u16) {
    let (hw, hh) = (si(k, 14.0), si(k, 10.0));
    cv.round_outline(rect(c.x - hw, c.y - hh, 2 * hw, 2 * hh), 2, color);
    cv.round_outline(rect(c.x - hw + 1, c.y - hh + 1, 2 * hw - 2, 2 * hh - 2), 2, color);
    cv.vline(c.x - si(k, 4.5), c.y - hh + 3, 2 * hh - 6, 1, color);
    cv.vline(c.x + si(k, 4.5), c.y - hh + 3, 2 * hh - 6, 1, color);
    cv.disc(c, si(k, 2.5) as u32, color);
}

/// Three slider tracks with offset knobs — the settings glyph.
fn icon_sliders(cv: &mut impl Surface, c: Point, k: f32, color: u16) {
    let hw = si(k, 12.0);
    let track_h = si(k, 3.0).max(2) as u32;
    let knob_r = si(k, 3.5) as u32;
    for (row, knob) in [(-7.0, 6.0), (0.0, -8.0), (7.0, 0.0)] {
        let y = c.y + si(k, row);
        cv.fill(rect(c.x - hw, y - track_h as i32 / 2, 2 * hw, track_h as i32), color);
        cv.disc(Point::new(c.x + si(k, knob), y), knob_r, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::{AppState, Settings};

    fn run(scr: &mut MenuScreen, act: &mut Activity, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let scratch = crate::screen::PoiScratch::new();
        let mut cx = Ctx {
            state: &mut st,
            activity: act,
            settings: &mut settings,
            routes: &[],
            rides: &[],
            poi_scratch: &scratch,
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// Pressing the Map station while idle (not tracking) pushes the Map over the Menu — the
    /// route-less browse map, reached without a route or session.
    #[test]
    fn map_station_idle_pushes_the_browse_map() {
        let mut act = Activity::new(Mode::Idle);
        let mut scr = MenuScreen::new();
        scr.selected = 3; // the Map station
        let t = run(&mut scr, &mut act, Gesture::Press);
        assert!(matches!(t, Transition::Push(Screen::Map(_))), "idle → push the browse Map over the Menu");
    }

    /// Pressing the Map station **while tracking** lands the rider on the live riding map by rooting
    /// the stack to a clean `[Home, Map]` — never a second stacked Map.
    #[test]
    fn map_station_while_tracking_roots_to_the_ride_base() {
        let mut act = Activity::new(Mode::Riding);
        act.start_session(); // now tracking
        let mut scr = MenuScreen::new();
        scr.selected = 3;
        let t = run(&mut scr, &mut act, Gesture::Press);
        assert!(
            matches!(t, Transition::Root(Screen::Map(_))),
            "tracking → root to [Home, Map] (the idle-return ride-base normalization), not a stacked Map"
        );
    }
}
