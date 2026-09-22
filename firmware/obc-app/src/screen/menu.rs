use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::Msg;

use super::vocab::chrome::title_frame_ble;
use super::vocab::list;
use super::{
    palette, Ctx, MapScreen, PeakViewScreen, Render, RidesScreen, RouteMenuScreen, Screen, ScreenTick, SettingsScreen,
    Transition,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuItem {
    Routes,
    Rides,
    Map,
    Peaks,

    Settings,
}

const ITEMS: [MenuItem; 5] = [MenuItem::Routes, MenuItem::Rides, MenuItem::Map, MenuItem::Peaks, MenuItem::Settings];

/// The menu copy, resolved each frame because the language is a runtime value.
struct MenuText {
    title: &'static str,
    items: [&'static str; ITEMS.len()],
    len: usize,
}

impl MenuText {
    fn resolve(rx: &Render, kinds: &[MenuItem]) -> Self {
        let mut items = [""; ITEMS.len()];
        for (slot, kind) in items.iter_mut().zip(kinds) {
            *slot = match kind {
                MenuItem::Routes => rx.t(Msg::MenuRoutes),
                MenuItem::Rides => rx.t(Msg::MenuRides),
                MenuItem::Map => rx.t(Msg::MenuMap),
                MenuItem::Peaks => rx.t(Msg::MenuPeaks),

                MenuItem::Settings => rx.t(Msg::MenuSettings),
            };
        }
        Self { title: rx.t(Msg::MenuTitle), items, len: kinds.len() }
    }

    fn labels(&self) -> &[&'static str] {
        &self.items[..self.len]
    }
}

/// The unit direction of station `i` of `n`, from N and clockwise, so a Down step walks the ring
/// clockwise.
fn station_dir(i: usize, n: usize) -> (f32, f32) {
    let a = (i as f32 / n as f32) * core::f32::consts::TAU;
    (libm::sinf(a), -libm::cosf(a)) // screen coords: 0° = up, clockwise positive
}

/// Degrees the needle sweeps for one step around a ring of `len` entries.
fn step_deg(len: usize) -> f32 {
    360.0 / len.max(1) as f32
}

/// The needle eases out: it moves this part of the remaining arc per second, with a floor below,
/// so the tail does not crawl.
const SWEEP_RATE: f32 = 8.0;
const SWEEP_MIN_DEG_S: f32 = 180.0;
/// The frame cadence the sweep asks the host for while in flight.
const SWEEP_FRAME_MS: u32 = 16;

#[derive(Debug, Default)]
pub(super) struct CompassDial {
    selected: usize,
    needle_deg: f32,
    target_deg: f32,
    /// The clock of the previous sweep tick. `None` while the needle rests.
    last_anim_ms: Option<u32>,
}

impl CompassDial {
    pub(super) fn selected(&self) -> usize {
        self.selected
    }

    pub(super) fn step(&mut self, n: i32, len: usize) -> Transition {
        self.target_deg += n as f32 * step_deg(len);
        list::on_step(&mut self.selected, n, len)
    }

    /// Advance the needle toward the target and ask for a wake while it is in flight. A resting
    /// menu is idle, so it costs no timed repaints.
    pub(super) fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
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
            // Fold both angles back into one revolution. `rem_euclid` on floats is std-only,
            // so the sign is fixed by hand.
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
}

#[derive(Debug, Default)]
pub struct MenuScreen {
    dial: CompassDial,
}

impl MenuScreen {
    pub fn new() -> Self {
        MenuScreen::default()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => self.dial.step(n, ITEMS.len()),
            Gesture::Press => match ITEMS.get(self.dial.selected()).copied().unwrap_or(MenuItem::Settings) {
                MenuItem::Routes => Transition::Push(Screen::RouteMenu(RouteMenuScreen::new())),
                MenuItem::Rides => Transition::Push(Screen::Rides(RidesScreen::new())),
                MenuItem::Map => open_map(cx),
                MenuItem::Peaks => Transition::Push(Screen::PeakView(PeakViewScreen::default())),

                MenuItem::Settings => Transition::Push(Screen::Settings(SettingsScreen::new())),
            },
            Gesture::Back => Transition::Pop,
            Gesture::Hold => Transition::None,
            Gesture::BackHold => Transition::None,
        }
    }

    pub fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        self.dial.tick_timers(now_ms)
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let device = rx.state.device;
        let ble = device.ble_connected();
        let kinds = &ITEMS;
        let txt = MenuText::resolve(rx, kinds);
        let mut batt: heapless::String<8> = heapless::String::new();
        let _ = write!(batt, "{}%", device.battery_pct);
        draw_compass(
            cv,
            rx.w,
            rx.h,
            self.dial.selected().min(kinds.len() - 1),
            self.dial.needle_deg,
            ble,
            &batt,
            txt.title,
            kinds,
            txt.labels(),
        );
    }
}

/// Open the Map station. While tracking, root the stack to `[Home, Map]`, so a second Map is
/// never stacked and stale overlays clear. Otherwise push a route-less browse map over the Menu.
fn open_map(cx: &mut Ctx) -> Transition {
    if cx.recorder.recording() {
        return Transition::Root(Screen::Map(MapScreen::new()));
    }
    // Seed the browse camera on the last fix if there is one.
    if let Some(fix) = cx.state.user_fix {
        cx.state.enter_riding_view(fix.lon, fix.lat);
    } else {
        cx.state.enter_riding_view(cx.state.cam_lon, cx.state.cam_lat);
    }
    Transition::Push(Screen::Map(MapScreen::new()))
}

/// The compass-dial layout: bezel ring, midpoint ticks, needle, icon stations, and the name of
/// the selected entry. The needle points at `needle_deg`, which is between stations mid-sweep,
/// but the station highlight and the name go to the selection immediately.
#[allow(clippy::too_many_arguments)] // one flat draw fn; bundling the geometry and state adds no clarity
fn draw_compass(
    cv: &mut impl Surface,
    w: i32,
    h: i32,
    selected: usize,
    needle_deg: f32,
    ble_connected: bool,
    battery: &str,
    title: &str,
    kinds: &[MenuItem],
    items: &[&str],
) {
    use palette::*;
    title_frame_ble(cv, w, h, title, battery, ble_connected);

    let c = Point::new(w / 2, h / 2);

    cv.disc(c, 106, WOOD);
    cv.disc(c, 98, PARCHMENT);
    // The ticks sit at the station midpoints, so no tick is below a station disc. Doubled 1 px
    // lines make a 2 px stroke.
    let n_ring = items.len();
    for k in 0..n_ring {
        let a = (k as f32 + 0.5) / n_ring as f32 * core::f32::consts::TAU;
        let (dx, dy) = (libm::sinf(a), -libm::cosf(a));
        let (ix, iy) = (si(1.0, dx * 88.0), si(1.0, dy * 88.0));
        let (ox, oy) = (si(1.0, dx * 96.0), si(1.0, dy * 96.0));
        for off in 0..2 {
            cv.line(Point::new(c.x + ix + off, c.y + iy), Point::new(c.x + ox + off, c.y + oy), WOOD);
        }
    }

    draw_needle(cv, c, needle_deg, 42.0, 10.0);

    let n = items.len();
    for (i, item) in kinds.iter().copied().enumerate() {
        let (dx, dy) = station_dir(i, n);
        let sc = Point::new(c.x + si(1.0, dx * 72.0), c.y + si(1.0, dy * 72.0));
        let is_sel = i == selected;
        if is_sel {
            cv.disc(sc, 24, AMBER);
        } else {
            cv.disc(sc, 24, RULE);
            cv.disc(sc, 21, PARCHMENT);
        }
        let (ink, bg) = if is_sel { (INK, AMBER) } else { (SUBTEXT, PARCHMENT) };
        draw_icon(cv, item, sc, 1.2, ink, bg);
    }

    cv.text(items[selected], Point::new(w / 2, h - 38), Font::Display, TextAlign::Center, INK);
}

/// Draw the compass needle at `c`, pointing `deg` (0° = N, clockwise): an amber head of length
/// `r` with a base half-width of `half_w`, a grey counterweight, and an ink hub. The menu dial,
/// the nav spinner, and the warning card glyph share it, so the needles cannot drift apart.
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
    // The hub scales with the radius, so it does not swallow the mini needle of the warning card.
    let hub = (r / 7.0) as i32;
    cv.disc(c, hub.max(1) as u32, INK);
    if hub >= 3 {
        cv.disc(c, (hub / 3) as u32, PARCHMENT);
    }
}

/// Draw a station icon at `c`, scaled by `k`. `bg` is the surface behind it, for punched details.
fn draw_icon(cv: &mut impl Surface, item: MenuItem, c: Point, k: f32, color: u16, bg: u16) {
    match item {
        MenuItem::Routes => icon_route(cv, c, k, color),
        MenuItem::Rides => icon_rides(cv, c, k, color, bg),
        MenuItem::Map => icon_map(cv, c, k, color),
        MenuItem::Peaks => icon_peaks(cv, c, k, color, bg),

        MenuItem::Settings => icon_sliders(cv, c, k, color),
    }
}

/// Two mountain ridges with small punched snow caps.
fn icon_peaks(cv: &mut impl Surface, c: Point, k: f32, color: u16, bg: u16) {
    cv.triangle(
        Point::new(c.x - si(k, 13.0), c.y + si(k, 10.0)),
        Point::new(c.x - si(k, 3.0), c.y - si(k, 11.0)),
        Point::new(c.x + si(k, 6.0), c.y + si(k, 10.0)),
        color,
    );
    cv.triangle(
        Point::new(c.x - si(k, 2.0), c.y + si(k, 10.0)),
        Point::new(c.x + si(k, 7.0), c.y - si(k, 5.0)),
        Point::new(c.x + si(k, 14.0), c.y + si(k, 10.0)),
        color,
    );
    cv.triangle(
        Point::new(c.x - si(k, 6.0), c.y - si(k, 5.0)),
        Point::new(c.x - si(k, 3.0), c.y - si(k, 11.0)),
        Point::new(c.x, c.y - si(k, 4.0)),
        bg,
    );
}

/// The Rides glyph: a stopwatch, which reads differently from the route icon's road line.
fn icon_rides(cv: &mut impl Surface, c: Point, k: f32, color: u16, bg: u16) {
    let r = si(k, 9.0) as u32;
    cv.disc(c, r, color);
    cv.disc(c, si(k, 6.5) as u32, bg); // punch the face out to a ring
    cv.fill(rect(c.x - si(k, 2.0), c.y - si(k, 13.0), si(k, 4.0), si(k, 4.0)), color);
    cv.line(c, Point::new(c.x + si(k, 4.0), c.y - si(k, 4.0)), color);
    cv.disc(c, si(k, 1.5).max(1) as u32, color);
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

/// A folded map with a "you are here" dot. The fold creases stay hairlines, because heavier bars
/// read as a grill at this size.
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
    use crate::screen::test_ctx;
    use crate::{AppState, Settings};

    static PEAK_PROFILE: crate::PeakViewProfile<'static> = crate::PeakViewProfile {
        observer_lat: 0,
        observer_lon: 0,
        observer_elevation_m: 0,
        default_heading_q4: 0,
        fov_q4: 15,
        vertical_centre_q4: 0,
        vertical_span_q4: 11,
        peaks: &[],
    };

    fn run(scr: &mut MenuScreen, act: &mut Activity, rec: &mut crate::RecorderMachine, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let mut cx = Ctx { recorder: rec, ..test_ctx(&mut st, act, &mut settings) };
        scr.handle(g, &mut cx)
    }

    #[test]
    fn map_station_idle_pushes_the_browse_map() {
        let mut rec = crate::RecorderMachine::new();
        let mut act = Activity::new(Mode::Idle);
        let mut scr = MenuScreen::new();
        scr.dial.selected = 2; // the Map station
        let t = run(&mut scr, &mut act, &mut rec, Gesture::Press);
        assert!(matches!(t, Transition::Push(Screen::Map(_))), "idle → push the browse Map over the Menu");
    }

    #[test]
    fn map_station_while_tracking_roots_to_the_ride_base() {
        let mut rec = crate::RecorderMachine::new();
        let mut act = Activity::new(Mode::Riding);
        rec.test_open();
        let mut scr = MenuScreen::new();
        scr.dial.selected = 2;
        let t = run(&mut scr, &mut act, &mut rec, Gesture::Press);
        assert!(
            matches!(t, Transition::Root(Screen::Map(_))),
            "tracking → root to [Home, Map] (the idle-return ride-base normalization), not a stacked Map"
        );
    }

    #[test]
    fn peak_station_stays_available_with_or_without_installed_terrain() {
        let mut state = AppState::new(0, 0, 1.0);

        let mut activity = Activity::new(Mode::Idle);
        let mut settings = Settings::default();
        let mut screen = MenuScreen::new();
        screen.dial.selected = 3;
        let mut cx = test_ctx(&mut state, &mut activity, &mut settings);
        for has_fix in [false, true] {
            cx.state.peak_view_profile = has_fix.then_some(PEAK_PROFILE);
            cx.state.user_fix = has_fix.then_some(obc_ports::Fix { lat: 0, lon: 0, course: None, speed_mps: None });
            let Transition::Push(Screen::PeakView(mut peak)) = screen.handle(Gesture::Press, &mut cx) else {
                panic!("Peak View")
            };
            assert!(peak.tick_timers(0, 240, 320).next_wake_ms.is_some(), "App supplies freshness on entry");
        }
    }
}
