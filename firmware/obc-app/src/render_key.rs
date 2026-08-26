//! **Render keys** — the exact facts the visible screens draw, compared once at the pass boundary.
//!
//! The repaint rule does not change (`dirty.rs`: over-redraw is safe, under-redraw is a bug). What
//! changes is who states it. A screen row declares its [`RenderKeyKind`](crate::screen::RenderKeyKind)
//! in the `screens!` table beside its other capabilities; [`App::render_key`] reads the visible
//! stack and answers with the exact values those kinds name. [`App::run_pass`](crate::App::run_pass)
//! builds that answer **before** its stages and again **after** them, both on its own stack, and
//! dirties the map when the two differ.
//!
//! ## Exact, never hashed
//!
//! Every field is a value, not a digest: floats are stored as [`f32::to_bits`], so `-0.0` and `NaN`
//! compare by their exact representation rather than by IEEE equality. A hash would trade a missed
//! redraw — the one failure mode the dirty contract calls a bug — for a few bytes of stack.
//!
//! ## What a key can and cannot see
//!
//! The comparison is stack-local and spans one pass, so it detects exactly the mutations that
//! happen **inside** that pass: the fix and the sensors (stage 3), the card sweep and the screen
//! ticks (stage 4), and every domain's own advance (stages 5–13). A host seam that mutates
//! `App` *between* two passes — `set_sensor_status`, `set_sensor_scan_hits`, `weather_feed_changed`,
//! `set_map_transfer`, the catalog feeds — has already moved the fact by the time the next pass
//! builds its *before* key, so both keys agree and the difference is invisible. Those seams keep an
//! explicit dirty request, and each says so at its call site. They become key-covered when the seam
//! moves into [`ExternalFacts`](crate::device_core::ExternalFacts) and is consumed at stage 2.
//!
//! The alternative — keeping the previous pass's key resident in `App` — is what this design
//! refuses: it would put a second copy of the visible state next to the state itself, which is the
//! multiplicity the manual mirrors already were.

use crate::screen::{RenderKeyKind, Screen, ScreenRow, MAX_DEPTH};
use crate::App;

/// The shape of the visible screen stack: every row from the lowest opaque screen to the top, by
/// variant. A navigation, a card landing and a card dismissal all move this, so a screen transition
/// dirties the map without any screen having to remember to say so.
pub(crate) type ShapeKey = heapless::Vec<ScreenRow, MAX_DEPTH>;

/// One GPS fix, exactly as the riding views draw it: position, course and speed. The camera derives
/// from it, so a fix that moved nothing compares equal and costs no repaint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FixKey {
    lon: i32,
    lat: i32,
    /// `course` / `speed_mps` as exact bit patterns — `None` reads as `u32::MAX`, which no finite
    /// float shares (it is a quiet NaN payload the sensors never produce).
    course: u32,
    speed: u32,
}

impl FixKey {
    fn of(fix: Option<obc_ports::Fix>) -> Option<FixKey> {
        fix.map(|f| FixKey {
            lon: f.lon,
            lat: f.lat,
            course: f.course.map_or(u32::MAX, f32::to_bits),
            speed: f.speed_mps.map_or(u32::MAX, f32::to_bits),
        })
    }
}

/// Home: the battery gauge, the connected indicator, and the backdrop's per-open jitter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HomeKey {
    battery_pct: u8,
    ble_link: crate::ble::BleLink,
    ble_paired: bool,
    /// The screensaver backdrop's seed — Home's whole animation state besides its minute ticker,
    /// which reports its own change through [`ScreenTick`](crate::screen::ScreenTick).
    backdrop_seed: u32,
}

/// A map base: the camera, the fix that drives it, the pan HUD, and the route-relative chrome
/// (the warning chip, the waypoint chip, the drawn route line).
///
/// Catalog and table indices are narrowed to `u32`: a route slot and a waypoint row are bounded by
/// their catalogs' own low caps, so the narrowing is lossless on every target this runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MapKey {
    cam_lon: i32,
    cam_lat: i32,
    /// `zoom` and the projection's rotation as exact bit patterns. `course_rad` already folds
    /// heading-up, the pan freeze, the GPS course and the compass into one angle, so the map's
    /// orientation is one field rather than four that must be kept in step.
    zoom: u32,
    course_rad: u32,
    pan: Option<(crate::app::PanBasis, crate::app::PanTool, u32)>,
    fix: Option<FixKey>,
    active_route: Option<u32>,
    progress_m: u32,
    off_route: bool,
    dist_to_route_m: u32,
    next_waypoint: Option<u32>,
    no_fix: bool,
    tracking: bool,
}

/// The Statistics grid: the ride readouts, the route-relative fields, and the live sensor tiles —
/// the last gated per quantity on the field actually being pinned to the grid, so an unconfigured
/// sensor never forces a full map render at its notification rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StatsKey {
    fix: Option<FixKey>,
    progress_m: u32,
    off_route: bool,
    no_fix: bool,
    active_climb: Option<u32>,
    next_waypoint: Option<u32>,
    /// The displayed heart-rate / power / cadence values, each `None` unless its field is on the
    /// grid — the same economy the per-quantity guards spelled out by hand.
    live: (Option<u16>, Option<u16>, Option<u8>),
}

/// The Climb view: which climb, how far along it, and the grade the cursor sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClimbKey {
    active_climb: Option<u32>,
    progress_m: u32,
    fix: Option<FixKey>,
}

/// The Sensors settings pages: the saved sensors' per-slot status.
///
/// The **scan list is deliberately absent**. Both it and the status above are fed by host seams
/// that run between two passes, so neither edge is visible to a stack-local comparison; each seam
/// asks for its own repaint, and copying a few hundred bytes of names and addresses into a key that
/// can never differ would be cost without cover. The status is here because it is free — a `Copy`
/// array the screens already hold — and because it is what the row declares it draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SensorsKey {
    status: [crate::sensors::SensorStatus; crate::settings::SENSOR_SLOTS],
}

/// The weather pages: which data is installed and which rain step is selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WeatherKey {
    installed: Option<crate::device_core::WeatherData>,
    rain_step: u8,
    rain_steps_ahead: u8,
}

/// One pass's answer: the visible stack's shape, plus the exact facts each declared kind names.
///
/// A kind's slot is `Some` exactly when some visible row declares it. Kinds are facts about the
/// *device*, not about a screen instance, so two visible rows declaring the same kind fill the same
/// slot with the same value — there is nothing to reconcile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderKey {
    shape: ShapeKey,
    home: Option<HomeKey>,
    map: Option<MapKey>,
    stats: Option<StatsKey>,
    climb: Option<ClimbKey>,
    sensors: Option<SensorsKey>,
    weather: Option<WeatherKey>,
}

// The pass keeps two of these on its own (non-`async`) frame, so growth here is residual stack, not
// resident RAM and not a poll frame. It is still the ride loop's deepest frame, so it is pinned like
// any arena arm: 248 B on a 64-bit host, less on the board, where a `usize` is four bytes.
const _: () =
    assert!(core::mem::size_of::<RenderKey>() <= 256, "a render key is the visible facts, not a copy of the app state");

impl App {
    /// The exact facts the currently visible screens draw.
    ///
    /// Reads DeviceCore state and mutates nothing — [`run_pass`](App::run_pass) calls it twice per
    /// pass, so a side effect here would be applied twice and once out of order.
    ///
    /// `#[inline(never)]`: the two calls are the same walk over the same stack, and letting the
    /// optimiser paste it into the ride loop's own frame twice buys nothing and costs both flash and
    /// the deepest stack frame on the board.
    #[inline(never)]
    pub(crate) fn render_key(&self) -> RenderKey {
        let mut key = RenderKey {
            shape: ShapeKey::new(),
            home: None,
            map: None,
            stats: None,
            climb: None,
            sensors: None,
            weather: None,
        };
        // Drawing starts at the lowest opaque screen: anything below it is covered and draws
        // nothing, so it is not part of the frame and not part of the key.
        let base = self.ui.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        let no_fix = !self.has_live_fix(self.ui.now_ms);
        for screen in self.ui.stack.iter().skip(base) {
            // Cannot overflow: the stack itself is `MAX_DEPTH` long.
            let _ = key.shape.push(screen.row());
            match screen.caps().render_key {
                RenderKeyKind::Static => {}
                RenderKeyKind::Home => key.home = Some(self.home_key(screen)),
                RenderKeyKind::Map => key.map = Some(self.map_key(no_fix)),
                RenderKeyKind::Statistics => key.stats = Some(self.stats_key(no_fix)),
                RenderKeyKind::Climb => key.climb = Some(self.climb_key()),
                RenderKeyKind::SensorSettings => key.sensors = Some(self.sensors_key()),
                RenderKeyKind::Weather => key.weather = Some(self.weather_key()),
            }
        }
        key
    }

    fn home_key(&self, screen: &Screen) -> HomeKey {
        HomeKey {
            battery_pct: self.state.device.battery_pct,
            ble_link: self.state.device.ble_link,
            ble_paired: self.state.device.ble_paired,
            backdrop_seed: match screen {
                Screen::Home(home) => home.backdrop_seed(),
                _ => 0,
            },
        }
    }

    fn map_key(&self, no_fix: bool) -> MapKey {
        MapKey {
            cam_lon: self.state.cam_lon,
            cam_lat: self.state.cam_lat,
            zoom: self.state.zoom.to_bits(),
            course_rad: self.state.course_rad().to_bits(),
            pan: self.state.pan.map(|p| (p.basis, p.tool, p.route_progress_m)),
            fix: FixKey::of(self.state.user_fix),
            active_route: self.activity.active_route.map(|i| i as u32),
            progress_m: self.activity.progress_m,
            off_route: self.activity.off_route,
            dist_to_route_m: self.activity.dist_to_route_m,
            next_waypoint: self.activity.next_waypoint.map(|i| i as u32),
            no_fix,
            tracking: self.activity.is_tracking(),
        }
    }

    fn stats_key(&self, no_fix: bool) -> StatsKey {
        use crate::stat_fields::StatField;
        let fields = &self.settings().stat_fields;
        StatsKey {
            fix: FixKey::of(self.state.user_fix),
            progress_m: self.activity.progress_m,
            off_route: self.activity.off_route,
            no_fix,
            active_climb: self.activity.active_climb.map(|i| i as u32),
            next_waypoint: self.activity.next_waypoint.map(|i| i as u32),
            live: (
                fields.contains(StatField::HeartRate).then(|| self.activity.live_hr_display()).flatten(),
                fields.contains(StatField::Power).then(|| self.activity.live_power_display()).flatten(),
                fields.contains(StatField::Cadence).then(|| self.activity.live_cadence_display()).flatten(),
            ),
        }
    }

    fn climb_key(&self) -> ClimbKey {
        ClimbKey {
            active_climb: self.activity.active_climb.map(|i| i as u32),
            progress_m: self.activity.progress_m,
            fix: FixKey::of(self.state.user_fix),
        }
    }

    fn sensors_key(&self) -> SensorsKey {
        SensorsKey { status: self.ui.sensor_status }
    }

    fn weather_key(&self) -> WeatherKey {
        WeatherKey {
            installed: self.weather.installed(),
            rain_step: self.state.rain_step,
            rain_steps_ahead: self.state.rain_steps_ahead,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppState;
    use crate::screen::{MenuScreen, RenderKeyKind};

    /// Every row states what it draws. The four the archetypes cannot answer for — the Climb view
    /// among the riding screens, the weather pages and the Sensors pages among the chrome — say so
    /// on the row, and this is where a new dynamic screen that forgot to is caught.
    #[test]
    fn every_screen_row_declares_a_render_key_kind() {
        let declared: std::vec::Vec<(&str, RenderKeyKind)> = Screen::NAMES
            .iter()
            .zip(Screen::CAPS)
            .filter(|(_, caps)| caps.render_key != RenderKeyKind::Static)
            .map(|(name, caps)| (*name, caps.render_key))
            .collect();
        assert_eq!(
            declared,
            [
                ("Home", RenderKeyKind::Home),
                ("Map", RenderKeyKind::Map),
                ("Statistics", RenderKeyKind::Statistics),
                ("Climb", RenderKeyKind::Climb),
                ("Detour", RenderKeyKind::Map),
                ("DetourPreview", RenderKeyKind::Map),
                ("Weather", RenderKeyKind::Weather),
                ("WeatherHourly", RenderKeyKind::Weather),
                ("WeatherRainMap", RenderKeyKind::Map),
                ("Sensors", RenderKeyKind::SensorSettings),
                ("SensorScan", RenderKeyKind::SensorSettings),
            ],
            "the dynamic rows, in table order — add a row here when a screen starts drawing a live fact"
        );
    }

    /// The quiet case, and the whole economy: a device nothing happened to answers the same key
    /// twice, so the pass asks for no repaint.
    #[test]
    fn an_unchanged_device_answers_the_same_key() {
        let app = App::new(AppState::new(0, 0, 1.0));
        assert_eq!(app.render_key(), app.render_key(), "reading the key must not change it");
    }

    /// A navigation moves the shape, so no screen has to remember to dirty the map on the way in or
    /// out — including a move between two screens that declare the *same* kind.
    #[test]
    fn a_screen_transition_moves_the_key() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let before = app.render_key();
        let _ = app.ui.stack.push(Screen::Menu(MenuScreen::new()));
        assert_ne!(app.render_key(), before, "the visible stack's shape is part of the key");
    }

    /// **Exact, never hashed, and never IEEE.** A float goes into the key as its bit pattern, so a
    /// zoom that changed to a value comparing `==` under IEEE rules still repaints.
    #[test]
    fn floats_are_compared_by_their_exact_bits() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let before = app.render_key();
        app.state.zoom = -0.0;
        let negative_zero = app.render_key();
        app.state.zoom = 0.0;
        assert_eq!(app.state.zoom, -0.0, "IEEE says these two zooms are the same number");
        assert_ne!(app.render_key(), negative_zero, "…and the key says they are two different frames");
        assert_ne!(negative_zero, before);
    }

    /// Home draws the gauge; the Map does not. The economy the per-screen declaration buys over the
    /// old base-screen class gate, in one comparison.
    #[test]
    fn the_battery_is_in_homes_key_and_in_no_other() {
        let mut home = App::new_idle(AppState::new(0, 0, 1.0));
        let before = home.render_key();
        home.state.device.battery_pct = 42;
        assert_ne!(home.render_key(), before, "Home draws the gauge");

        let mut map = App::new(AppState::new(0, 0, 1.0));
        let before = map.render_key();
        map.state.device.battery_pct = 42;
        assert_eq!(map.render_key(), before, "the map view does not, so a 30 s gauge tick costs it nothing");
    }
}
