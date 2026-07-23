//! The mid-ride compass menu (epic #789): five fixed stations with the same bezel, needle sweep and
//! detents as the main Menu. Waypoints opens the route-ordered whole-plan list (#787), Detour
//! opens the rejoin chooser (#788 → routed detour #882), and POIs, Routes and Main menu open
//! their existing screens.

use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};
use obc_route::{Profile, Waypoints, WptEntry};

use crate::input::Gesture;
use crate::settings::Units;
use crate::Msg;

use super::list::{self, ListGeometry, Separators};
use super::menu::{CompassDial, CompassIcons, N_ITEMS};
use super::{
    empty_state, fit_caption, palette, Ctx, DetourScreen, MenuScreen, PoiMenuScreen, Render, RouteMenuScreen, Screen,
    ScreenTick, Transition,
};

/// The waypoint list's two-line row pitch: name above, distance + remaining ascent below. Four rows
/// fit the 240×320 panel with the same side margins and scrollbar as the other list screens.
const WAYPOINT_ROW_H: i32 = 66;
const WAYPOINT_SIDE_INSET: i32 = 12;
const WAYPOINT_NAME_INSET: i32 = 12;
const WAYPOINT_CLIMB_COL_PCT: i32 = 55;

/// The fixed ride-menu ring. The selected station always starts at Waypoints (north); keeping all
/// five entries present on route-less rides preserves the dial geometry and muscle memory.
#[derive(Debug, Default)]
pub struct RideMenuScreen {
    dial: CompassDial,
}

impl RideMenuScreen {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => self.dial.turn(n),
            Gesture::Press => match self.dial.selected() {
                // Open on the next waypoint when one is resolved, while keeping passed entries in
                // the list above it. A route-less/no-waypoint ride simply starts at row 0.
                0 => Transition::Push(Screen::RideWaypoints(RideWaypointsScreen::at(
                    cx.activity.next_waypoint.unwrap_or(0),
                ))),
                // Replace the transient compass, so chooser Press/Back can Pop once to the exact
                // caller (Map, Statistics/Climb, or paused Ride control). A dimmed station (no
                // nav graph / no route / off-route) never transitions.
                1 if detour_available(cx.activity, cx.state.has_nav_graph) => {
                    Transition::Replace(Screen::Detour(DetourScreen::new(cx.activity)))
                }
                1 => Transition::None,
                2 => Transition::Push(Screen::PoiMenu(PoiMenuScreen::new())),
                3 => Transition::Push(Screen::RouteMenu(RouteMenuScreen::new())),
                _ => Transition::Push(Screen::Menu(MenuScreen::new())),
            },
            Gesture::Back => Transition::Pop,
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        self.dial.tick_timers(now_ms)
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let items: [&str; N_ITEMS] = [
            rx.t(Msg::RideMenuWaypoints),
            rx.t(Msg::RideMenuDetour),
            rx.t(Msg::MenuPois),
            rx.t(Msg::MenuRoutes),
            rx.t(Msg::RideMenuMainMenu),
        ];
        let mut batt: heapless::String<8> = heapless::String::new();
        let _ = write!(batt, "{}%", rx.state.battery_pct);
        self.dial.draw(
            cv,
            rx.w,
            rx.h,
            rx.state.ble_connected(),
            &batt,
            rx.t(Msg::RideMenuTitle),
            &items,
            CompassIcons::Ride {
                route_loaded: rx.activity.active_route.is_some(),
                detour_available: detour_available(rx.activity, rx.state.has_nav_graph),
            },
        );
    }
}

/// Whether the Detour station is actionable (#882): a route is loaded, the map has a nav graph,
/// and the rider is on the route (the corridor anchors on live progress, which off-route
/// freezes). Shared by the station's Press gate and its dimming.
fn detour_available(activity: &crate::activity::Activity, has_nav_graph: bool) -> bool {
    activity.active_route.is_some() && has_nav_graph && !activity.off_route
}

/// The active route's named waypoints in route order. The list deliberately keeps passed entries
/// as muted whole-plan context; their forward-looking distance and climb clamp to zero. On a normal
/// route the resident table is the whole plan (up to [`obc_route::MAX_WAYPOINTS`] entries). For an
/// oversized route, the existing cache re-window policy means this screen shows the current
/// 32-entry resident plan window; already-evicted passed rows cannot be reconstructed without
/// re-reading route storage or growing the fixed device cache.
#[derive(Debug, Default)]
pub struct RideWaypointsScreen {
    selected: usize,
}

impl RideWaypointsScreen {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open with `selected` highlighted (the ride-menu station passes the resolved next-waypoint
    /// index). Draw/handle clamp it against a table that can re-window between fixes.
    fn at(selected: usize) -> Self {
        RideWaypointsScreen { selected }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let len = cx.activity.waypoint_count;
        self.selected = self.selected.min(len.saturating_sub(1));
        match g {
            Gesture::Turn(n) => list::on_turn(&mut self.selected, n, len),
            Gesture::Back => Transition::Pop,
            // MVP is the list: a row has no detail child yet. Holding/back-holding inside ride
            // chrome must likewise leave session, mode and navigation stack untouched.
            Gesture::Press | Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        draw_waypoint_list(
            cv,
            rx.w,
            rx.h,
            rx.t(Msg::RideMenuWaypoints),
            rx.t(Msg::WaypointsNone),
            if rx.activity.active_route.is_none() { rx.t(Msg::RideMenuNoRoute) } else { "" },
            rx.activity.active_route.is_some(),
            rx.waypoints,
            self.selected,
            rx.activity.progress_m,
            rx.activity.route_total_m,
            rx.profile,
            rx.settings.units,
        );
    }
}

/// Pure route-relative figures for one waypoint row. The distance axis comes from the resident
/// waypoint table and matched activity progress. Remaining ascent uses the cached profile's
/// cumulative-ascent curve at those same two fractions — not waypoint elevation (which may be
/// absent or off the line), and not coarse chunk metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WaypointFigures {
    passed: bool,
    distance_m: u32,
    climb_m: Option<u32>,
}

fn waypoint_figures(wp: &WptEntry, progress_m: u32, route_total_m: u32, profile: Option<&Profile>) -> WaypointFigures {
    let passed = progress_m > wp.dist_along_m;
    let distance_m = wp.dist_along_m.saturating_sub(progress_m);
    let climb_m = profile.filter(|_| route_total_m > 0).map(|p| {
        let frac = |m: u32| (m.min(route_total_m) as f32 / route_total_m as f32).clamp(0.0, 1.0);
        p.ascent_to(frac(wp.dist_along_m)).saturating_sub(p.ascent_to(frac(progress_m)))
    });
    WaypointFigures { passed, distance_m, climb_m }
}

fn write_climb(value_m: Option<u32>, units: Units) -> heapless::String<12> {
    let mut s = heapless::String::new();
    match value_m {
        Some(m) => {
            let shown = (units.elev(m as f32) + 0.5) as u32;
            let _ = write!(s, "{shown} {}", units.elev_label());
        }
        None => {
            let _ = s.push_str("--");
        }
    }
    s
}

#[allow(clippy::too_many_arguments)]
fn draw_waypoint_list(
    cv: &mut impl Surface,
    w: i32,
    h: i32,
    title: &str,
    empty_title: &str,
    empty_hint: &str,
    route_loaded: bool,
    waypoints: &Waypoints,
    selected: usize,
    progress_m: u32,
    route_total_m: u32,
    profile: Option<&Profile>,
    units: Units,
) {
    use palette::*;

    let total = if route_loaded { waypoints.len() } else { 0 };
    let geo = ListGeometry::below_title(w, h, WAYPOINT_ROW_H, 8, WAYPOINT_SIDE_INSET, Separators::Unselected);
    let selected = selected.min(total.saturating_sub(1));
    let pos = if total == 0 { 0 } else { selected + 1 };
    list::list_frame(cv, w, h, title, pos, total, geo.visible);
    if total == 0 {
        empty_state(cv, w, h, empty_title, empty_hint);
        return;
    }

    let first = list::window_start(selected, geo.visible, total) as i32;
    list::draw_rows(cv, geo, total, selected, first, |cv, row| {
        let wp = &waypoints.as_slice()[row.index];
        let values = waypoint_figures(wp, progress_m, route_total_m, profile);
        // Passed stays muted even when highlighted: the amber cursor still locates the row, while
        // both lines remain visually behind the next/upcoming plan.
        let name_color = if values.passed { SUBTEXT } else { INK };
        let stat_color = if values.passed {
            SUBTEXT
        } else if row.selected {
            INK
        } else {
            SUBTEXT
        };
        let x = row.area.top_left.x + WAYPOINT_NAME_INSET;
        let y = row.area.top_left.y;
        let mut name_buf: heapless::String<24> = heapless::String::new();
        let name = fit_caption(wp.name.as_str(), w - x - WAYPOINT_SIDE_INSET - 10, &mut name_buf, Font::Body);
        cv.text(name, Point::new(x, y + 9), Font::Body, TextAlign::Left, name_color);

        let sy = y + 36;
        let dist = crate::stat_fields::fmt_dist_short(values.distance_m, units);
        cv.text(&dist, Point::new(x, sy), Font::Label, TextAlign::Left, stat_color);

        let climb_x = row.area.top_left.x + (w - 2 * WAYPOINT_SIDE_INSET) * WAYPOINT_CLIMB_COL_PCT / 100;
        cv.triangle(
            Point::new(climb_x, sy + 14),
            Point::new(climb_x + 9, sy + 14),
            Point::new(climb_x + 4, sy + 5),
            stat_color,
        );
        let climb = write_climb(values.climb_m, units);
        cv.text(&climb, Point::new(climb_x + 16, sy), Font::Label, TextAlign::Left, stat_color);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::screen::{apply, Stack};
    use crate::{AppState, Settings};
    use embedded_graphics::primitives::Rectangle;
    use obc_formats::io::{ByteSink, Error, SliceSource};
    use obc_route::{gpx_to_obcr, RouteIndex, RouteReader};

    #[derive(Debug)]
    struct TextCall {
        text: std::string::String,
        at: Point,
        color: u16,
    }

    /// Records the list's text calls so render tests can pin order, figures, empty copy and muted
    /// passed-row colour without coupling to raster-font pixels.
    #[derive(Default)]
    struct TextRec {
        calls: std::vec::Vec<TextCall>,
    }

    impl Surface for TextRec {
        fn clear(&mut self, _: u16) {}
        fn fill(&mut self, _: Rectangle, _: u16) {}
        fn round(&mut self, _: Rectangle, _: u32, _: u16) {}
        fn round_outline(&mut self, _: Rectangle, _: u32, _: u16) {}
        fn line(&mut self, _: Point, _: Point, _: u16) {}
        fn triangle(&mut self, _: Point, _: Point, _: Point, _: u16) {}
        fn disc(&mut self, _: Point, _: u32, _: u16) {}
        fn text(&mut self, s: &str, at: Point, _: Font, _: TextAlign, color: u16) -> Point {
            self.calls.push(TextCall { text: s.into(), at, color });
            at
        }
    }

    #[derive(Default)]
    struct VecSink(std::vec::Vec<u8>);

    impl ByteSink for VecSink {
        fn write(&mut self, b: &[u8]) -> Result<(), Error> {
            self.0.extend_from_slice(b);
            Ok(())
        }
        fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), Error> {
            let off = off as usize;
            self.0[off..off + b.len()].copy_from_slice(b);
            Ok(())
        }
    }

    /// Three named waypoints deliberately listed out of order in GPX. The track climbs 100 m to
    /// First climb, descends to Valley, then climbs 100 m to Finish; all corners survive geometry
    /// decimation, making the expected ascent between Valley and Finish ~100 m.
    const WAYPOINT_GPX: &str = r#"<gpx>
  <wpt lat="48.0200" lon="7.8100"><name>Finish</name></wpt>
  <wpt lat="48.0100" lon="7.8000"><name>First climb</name></wpt>
  <wpt lat="48.0100" lon="7.8100"><name>Valley</name></wpt>
  <trk><trkseg>
    <trkpt lat="48.0000" lon="7.8000"><ele>100</ele></trkpt>
    <trkpt lat="48.0100" lon="7.8000"><ele>200</ele></trkpt>
    <trkpt lat="48.0100" lon="7.8100"><ele>150</ele></trkpt>
    <trkpt lat="48.0200" lon="7.8100"><ele>250</ele></trkpt>
  </trkseg></trk>
</gpx>"#;

    fn fixture_bytes() -> std::vec::Vec<u8> {
        let mut sink = VecSink::default();
        gpx_to_obcr(&SliceSource(WAYPOINT_GPX.as_bytes()), "Fixture", &mut sink).unwrap();
        sink.0
    }

    fn waypoint_ctx<'a>(activity: &'a mut Activity, waypoint_count: usize) -> Ctx<'a> {
        activity.waypoint_count = waypoint_count;
        let state = Box::leak(Box::new(AppState::new(0, 0, 1.0)));
        let settings = Box::leak(Box::new(Settings::default()));
        let scratch = Box::leak(Box::new(super::super::PoiScratch::new()));
        let nav_profiles = Box::leak(Box::new(crate::NavProfiles::new()));
        Ctx {
            state,
            activity,
            settings,
            routes: &[],
            rides: &[],
            trips: &[],
            nav_profiles,
            poi_scratch: scratch,
            sensor_scan_hits: &[],
            now_ms: 0,
        }
    }

    fn run(scr: &mut RideMenuScreen, g: Gesture) -> Transition {
        let mut state = AppState::new(0, 0, 1.0);
        // A routed, nav-graph ride so every station (incl. the gated Detour, #882) is actionable.
        state.has_nav_graph = true;
        let mut activity = Activity::new(Mode::Riding);
        activity.active_route = Some(0);
        let mut settings = Settings::default();
        let scratch = super::super::PoiScratch::new();
        let mut cx = Ctx {
            state: &mut state,
            activity: &mut activity,
            settings: &mut settings,
            routes: &[],
            rides: &[],
            trips: &[],
            nav_profiles: &crate::NavProfiles::EMPTY,
            poi_scratch: &scratch,
            sensor_scan_hits: &[],
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    #[test]
    fn fixed_ring_dispatches_all_five_stations_in_order() {
        fn press_at(turn: i32) -> Transition {
            let mut scr = RideMenuScreen::new();
            run(&mut scr, Gesture::Turn(turn));
            run(&mut scr, Gesture::Press)
        }
        assert!(matches!(press_at(0), Transition::Push(Screen::RideWaypoints(_))));
        assert!(matches!(press_at(1), Transition::Replace(Screen::Detour(_))));
        assert!(matches!(press_at(2), Transition::Push(Screen::PoiMenu(_))));
        assert!(matches!(press_at(3), Transition::Push(Screen::RouteMenu(_))));
        assert!(matches!(press_at(-1), Transition::Push(Screen::Menu(_))));
    }

    #[test]
    fn ring_wrap_and_back_are_stable() {
        let mut scr = RideMenuScreen::new();
        run(&mut scr, Gesture::Turn(5));
        assert!(matches!(run(&mut scr, Gesture::Press), Transition::Push(Screen::RideWaypoints(_))));
        assert!(matches!(run(&mut RideMenuScreen::new(), Gesture::Back), Transition::Pop));
    }

    #[test]
    fn route_less_ring_dims_only_the_route_dependent_stations() {
        let route_less = CompassIcons::Ride { route_loaded: false, detour_available: false };
        assert!(!route_less.enabled(0));
        assert!(!route_less.enabled(1));
        assert!((2..N_ITEMS).all(|i| route_less.enabled(i)));
        // A loaded route on a nav-graph map lights everything; a graph-less (or off-route) map
        // dims only the Detour station (#882).
        let all = CompassIcons::Ride { route_loaded: true, detour_available: true };
        assert!((0..N_ITEMS).all(|i| all.enabled(i)));
        let no_nav = CompassIcons::Ride { route_loaded: true, detour_available: false };
        assert!(no_nav.enabled(0) && !no_nav.enabled(1));
        assert!((2..N_ITEMS).all(|i| no_nav.enabled(i)));
    }

    #[test]
    fn ride_menu_uses_the_shared_needle_timer() {
        let mut scr = RideMenuScreen::new();
        run(&mut scr, Gesture::Turn(1));
        assert_eq!(scr.tick_timers(0).next_wake_ms, Some(16));
        let tick = scr.tick_timers(16);
        assert!(tick.changed);
        assert_eq!(tick.next_wake_ms, Some(16));
    }

    #[test]
    fn waypoints_station_opens_on_next_waypoint_and_preserves_activity() {
        let mut activity = Activity::new(Mode::Paused);
        activity.start_session();
        activity.mode = Mode::Paused;
        activity.next_waypoint = Some(2);
        let session = activity.session;
        let mut menu = RideMenuScreen::new();
        let t = menu.handle(Gesture::Press, &mut waypoint_ctx(&mut activity, 4));
        match t {
            Transition::Push(Screen::RideWaypoints(screen)) => assert_eq!(screen.selected, 2),
            _ => panic!("Waypoints station did not push its list"),
        }
        assert_eq!(activity.mode, Mode::Paused, "opening ride chrome never resumes/pauses the session");
        assert_eq!(activity.session, session, "opening the list never starts a new session");
    }

    #[test]
    fn waypoint_list_wraps_rows_and_only_back_navigates() {
        let mut activity = Activity::new(Mode::Riding);
        activity.start_session();
        let session = activity.session;
        let mut screen = RideWaypointsScreen::new();
        assert!(matches!(screen.handle(Gesture::Turn(-1), &mut waypoint_ctx(&mut activity, 3)), Transition::None));
        assert_eq!(screen.selected, 2, "turning above the first row wraps to the last resident waypoint");
        for g in [Gesture::Press, Gesture::Hold, Gesture::BackHold] {
            assert!(matches!(screen.handle(g, &mut waypoint_ctx(&mut activity, 3)), Transition::None));
        }
        assert!(matches!(screen.handle(Gesture::Back, &mut waypoint_ctx(&mut activity, 3)), Transition::Pop));
        assert_eq!(activity.mode, Mode::Riding);
        assert_eq!(activity.session, session);
    }

    #[test]
    fn figures_use_exact_route_distance_and_profile_ascent_deltas() {
        let bytes = fixture_bytes();
        let src = SliceSource(&bytes);
        let idx = RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&idx, &src);
        let profile = route.elevation_profile();
        let waypoints = route.load_waypoints(0);
        let w = waypoints.as_slice();
        assert_eq!(
            w.iter().map(|wp| wp.name.as_str()).collect::<std::vec::Vec<_>>(),
            ["First climb", "Valley", "Finish"],
            "the screen's resident source is route order, not GPX declaration order"
        );

        let progress = w[1].dist_along_m; // Valley
        let finish = waypoint_figures(&w[2], progress, route.total_distance_m, Some(&profile));
        assert_eq!(finish.distance_m, w[2].dist_along_m - progress, "distance is the exact along-route delta");
        let frac = |m: u32| m as f32 / route.total_distance_m as f32;
        let exact_climb = profile.ascent_to(frac(w[2].dist_along_m)) - profile.ascent_to(frac(progress));
        assert_eq!(finish.climb_m, Some(exact_climb), "climb is the exact cached cumulative-ascent delta");
        assert!((95..=105).contains(&exact_climb), "the fixture's final leg climbs ~100 m, got {exact_climb}");
        assert!((1_100..=1_120).contains(&finish.distance_m), "Valley → Finish is ~1.11 km");
        assert!(!finish.passed);

        let passed = waypoint_figures(&w[0], progress, route.total_distance_m, Some(&profile));
        assert_eq!(
            passed,
            WaypointFigures { passed: true, distance_m: 0, climb_m: Some(0) },
            "past rows stay in the plan but both forward-looking figures clamp to zero"
        );
        let exactly_here = waypoint_figures(&w[1], progress, route.total_distance_m, Some(&profile));
        assert_eq!(
            exactly_here,
            WaypointFigures { passed: false, distance_m: 0, climb_m: Some(0) },
            "a waypoint is only muted once matched progress has moved beyond it"
        );
    }

    #[test]
    fn render_keeps_route_order_and_mutes_passed_rows() {
        let bytes = fixture_bytes();
        let src = SliceSource(&bytes);
        let idx = RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&idx, &src);
        let profile = route.elevation_profile();
        let waypoints = route.load_waypoints(0);
        let progress = waypoints.as_slice()[1].dist_along_m; // First climb is passed; Valley is current.
        let mut cv = TextRec::default();
        draw_waypoint_list(
            &mut cv,
            240,
            320,
            "Waypoints",
            "No waypoints",
            "",
            true,
            &waypoints,
            1,
            progress,
            route.total_distance_m,
            Some(&profile),
            Units::Metric,
        );

        let body = &cv.calls[2..]; // title + empty right-hand title slot
        let texts = body.iter().map(|c| c.text.as_str()).collect::<std::vec::Vec<_>>();
        assert_eq!(
            &texts[..9],
            ["First climb", "0m", "0 m", "Valley", "0m", "0 m", "Finish", "1.1km", "100 m"],
            "each route-ordered row renders name, distance-to-go, then climb-to-go"
        );
        assert_eq!(body[0].color, palette::SUBTEXT, "the passed waypoint name is muted");
        assert_eq!(body[1].color, palette::SUBTEXT, "its zero distance is muted too");
        assert_eq!(body[2].color, palette::SUBTEXT, "its zero climb is muted too");
        assert_eq!(body[3].color, palette::INK, "the current/upcoming plan stays full ink");
        assert!(body[0].at.y < body[3].at.y && body[3].at.y < body[6].at.y, "route order is also visual top-to-bottom");
    }

    #[test]
    fn route_less_and_waypoint_less_rides_have_distinct_correct_empty_states() {
        let mut stale = Waypoints::new();
        let mut name = heapless::String::new();
        name.push_str("Stale").unwrap();
        stale.entries.push(WptEntry { dist_along_m: 10, lon: 0, lat: 0, name }).unwrap();

        let mut route_less = TextRec::default();
        draw_waypoint_list(
            &mut route_less,
            240,
            320,
            "Waypoints",
            "No waypoints",
            "No route loaded",
            false,
            &stale,
            0,
            0,
            0,
            None,
            Units::Metric,
        );
        let route_less_text = route_less.calls.iter().map(|c| c.text.as_str()).collect::<std::vec::Vec<_>>();
        assert!(route_less_text.contains(&"No waypoints"));
        assert!(route_less_text.contains(&"No route loaded"));
        assert!(!route_less_text.contains(&"Stale"), "a route-less frame never leaks a stale resident cache");

        let mut no_waypoints = TextRec::default();
        draw_waypoint_list(
            &mut no_waypoints,
            240,
            320,
            "Waypoints",
            "No waypoints",
            "",
            true,
            &Waypoints::new(),
            0,
            0,
            1,
            None,
            Units::Metric,
        );
        let no_waypoint_text = no_waypoints.calls.iter().map(|c| c.text.as_str()).collect::<std::vec::Vec<_>>();
        assert!(no_waypoint_text.contains(&"No waypoints"));
        assert!(!no_waypoint_text.contains(&"No route loaded"));
    }

    #[test]
    fn skip_replace_then_pop_preserves_each_caller_and_activity() {
        fn round_trip(caller: Screen, paused: bool, caller_matches: impl Fn(&Screen) -> bool) {
            let mut activity = Activity::new(if paused { Mode::Paused } else { Mode::Riding });
            activity.active_route = Some(0);
            activity.route_total_m = 2_000;
            activity.start_session();
            let mode = activity.mode;
            let session = activity.session();
            let mut stack = Stack::new();
            assert!(stack.push(caller).is_ok());
            assert!(stack.push(Screen::RideMenu(RideMenuScreen::new())).is_ok());
            apply(&mut stack, Transition::Replace(Screen::Detour(DetourScreen::new(&activity))));
            assert!(matches!(stack.last(), Some(Screen::Detour(_))));
            apply(&mut stack, Transition::Pop);
            assert!(caller_matches(stack.last().unwrap()));
            assert_eq!(activity.mode, mode);
            assert_eq!(activity.session(), session);
        }

        round_trip(Screen::Map(super::super::MapScreen::new()), false, |s| matches!(s, Screen::Map(_)));
        round_trip(Screen::Statistics(super::super::StatisticsScreen::new()), false, |s| {
            matches!(s, Screen::Statistics(_))
        });
        round_trip(Screen::RideControl(super::super::RideControl::new()), true, |s| {
            matches!(s, Screen::RideControl(_))
        });
    }
}
