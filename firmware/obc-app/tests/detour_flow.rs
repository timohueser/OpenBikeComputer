//! The routed-detour flow (#882), end to end through the real gesture path: ride a real route
//! (matcher locked by ticks over genuine OBCR geometry), open the ride menu's Detour station,
//! step the chooser, Press into the planning spinner (the detour search the pass hands out), land
//! the executor's `DetourFinished` answer on the preview, commit (the `CommitDetour` effect), and
//! land `DetourCommitted` — asserting the re-adoption choreography: active route swapped by durable
//! id, the recording session untouched, the stack back on the riding view, and the seam
//! re-anchor installing matcher progress + the forward-only floor on the next route-aware tick.
//! The failure tiers and the planning-screen cancel run through the same seams.
//!
//! The executor is [`Planner`], the suites' one-shot navigation host, exactly like `nav.rs`; the
//! *real* corridor/A*/splice pipeline is pinned end to end in `obc-route/tests/detour.rs` and by
//! the sim.
//!
//! The **Recalculating freeze** (issue #1146, P2) is pinned here too, at the tail: the detour flow
//! is the one path that runs a search with a *map* base underneath it, so it is where the freeze
//! engages, pauses the matcher, raises its banner, and — on every exit the flow has — releases.

use embedded_graphics::pixelcolor::Rgb888;
use obc_app::device_core::ModeState;
use obc_app::navigator::{NavigatorError, NavigatorOutcome, PlannerWork};
use obc_app::screen::{palette, Screen};
use obc_app::{App, AppState, DetourPreview, DetourRequest, Gesture, RouteSummary};
use obc_formats::io::SliceSource;
use obc_ports::{Fix, LocationSource, RideClock, Sensors};
use obc_reader::rgb565_to_rgb888;
use obc_route::{gpx_to_obcr, NavError, RouteIndex, RouteReader};

mod common;
use common::Planner;

/// The straight test road: lat 43.5°, lon 7.50° → 7.54° (~3 230 m ground). One `<trkpt>` per
/// 0.004° so the converter keeps real vertices along the way.
const LAT: f64 = 43.5;
const LON0: f64 = 7.50;

/// The road as `segs` even steps over the same ~3 230 m span, with every other vertex nudged
/// `wobble` degrees north.
///
/// The wobble is not decoration: the converter decimates anything within 1 m of the chord, so a
/// *straight* dense road comes back out of `gpx_to_obcr` as a handful of long segments. A test that
/// needs the matcher's **segment**-counted forward window to be narrower than the ground the rider
/// covers needs real vertices, and 2.2 m of alternating offset buys them for ~3.6 % of extra route
/// length and about a metre of cross-track (well inside the 15 m on-route band).
fn road_obcr_segs(segs: usize, wobble: f64) -> Vec<u8> {
    let mut g = String::from("<gpx><trk><trkseg>\n");
    for i in 0..=segs {
        let lon = LON0 + 0.04 * i as f64 / segs as f64;
        let lat = LAT + if i % 2 == 1 { wobble } else { 0.0 };
        g.push_str(&format!("  <trkpt lat=\"{lat:.7}\" lon=\"{lon:.7}\"><ele>100.0</ele></trkpt>\n"));
    }
    g.push_str("</trkseg></trk></gpx>");
    let src = SliceSource(g.as_bytes());
    let mut sink = VecSink::default();
    gpx_to_obcr(&src, "Road", &mut sink).unwrap();
    sink.0
}

fn road_obcr() -> Vec<u8> {
    road_obcr_segs(10, 0.0)
}

/// Minimal in-test `ByteSink` (the shared test helper lives per-crate; this suite needs one write).
#[derive(Default)]
struct VecSink(Vec<u8>);
impl obc_formats::io::ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), obc_formats::io::Error> {
        self.0.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), obc_formats::io::Error> {
        self.0[off as usize..off as usize + b.len()].copy_from_slice(b);
        Ok(())
    }
}

struct OneFix(Option<Fix>);
impl LocationSource for OneFix {
    fn poll(&mut self) -> Option<Fix> {
        self.0.take()
    }
}

/// One route-aware tick with an optional fresh fix.
fn tick(app: &mut App, now_ms: u32, fix: Option<Fix>, route: Option<&RouteReader>) {
    let mut loc = OneFix(fix);
    app.tick(RideClock(now_ms), Sensors::new(&mut loc), route);
}

/// The coordinate `frac` of the way along the road (good enough for fix placement).
fn road_at(frac: f64) -> Fix {
    Fix::at((LAT * 1e6) as i32, ((LON0 + 0.04 * frac) * 1e6) as i32)
}

fn summary(name: &str) -> RouteSummary {
    let mut n = heapless::String::<48>::new();
    let _ = n.push_str(name);
    RouteSummary {
        name: n,
        distance_km: 3,
        climb_m: 0,
        bbox: obc_map_scene::BBox {
            min_lon: (LON0 * 1e6) as i32,
            min_lat: (LAT * 1e6) as i32,
            max_lon: ((LON0 + 0.04) * 1e6) as i32,
            max_lat: (LAT * 1e6) as i32 + 1,
        },
        start_lon: (LON0 * 1e6) as i32,
        start_lat: (LAT * 1e6) as i32,
    }
}

/// The detour search the pass handed the executor, if it handed one out.
fn detour_req(app: &mut App, host: &mut Planner<'_>) -> Option<DetourRequest> {
    match host.take_work(app) {
        Some(PlannerWork::Detour(req)) => Some(req),
        Some(other) => panic!("this flow plans detours — {other:?}"),
        None => None,
    }
}

/// The executor's terminal answer to the running detour search.
fn answer_plan(app: &mut App, host: &mut Planner<'_>, result: Result<DetourPreview, NavError>) {
    host.answer(app, |token| match result {
        Ok(preview) => NavigatorOutcome::DetourFinished { token, preview },
        Err(error) => NavigatorOutcome::Failed { token, error: NavigatorError::Plan(error) },
    });
}

/// The executor's answer to the splice.
fn answer_commit(app: &mut App, host: &mut Planner<'_>, result: Result<obc_app::CatalogObjectId, NavError>) {
    host.answer(app, |token| match result {
        Ok(route) => NavigatorOutcome::DetourCommitted { token, route },
        Err(error) => NavigatorOutcome::Failed { token, error: NavigatorError::Plan(error) },
    });
}

/// A riding app with the matcher locked ~1 km along the real road, the Detour chooser reachable.
/// Returns `(app, obcr_bytes)`; [`riding!`] wraps it with the reader and the executor.
fn riding_app_on(obcr: Vec<u8>) -> (App, Vec<u8>) {
    let mut app = App::new_idle(AppState::new((LON0 * 1e6) as i32, (LAT * 1e6) as i32, 0.05));
    common::mount_store(&mut app); // a device with a card — a ride cannot start without one
    app.set_map_nav_graph(true);
    app.set_routes_with_ids(&[summary("Road")], &[7]);
    app.state.user_fix = Some(road_at(0.0));
    // Home → Menu (Routes) → Route menu → overview → START RIDE.
    app.apply_gesture(Gesture::BackHold);
    app.apply_gesture(Gesture::Press);
    app.apply_gesture(Gesture::Press);
    app.apply_gesture(Gesture::Press);
    assert!(app.recording(), "the ride started");
    assert!(matches!(app.top_screen(), Screen::Map(_)));

    // Lock the matcher ~1 km along with a real route-aware tick.
    let src = SliceSource(&obcr[..]);
    let idx = RouteIndex::read(&src).unwrap();
    let route = RouteReader::new(&idx, &src);
    tick(&mut app, 0, Some(road_at(0.31)), Some(&route));
    assert!(app.activity.progress_m() > 900 && app.activity.progress_m() < 1_100, "matcher locked ~1 km along");
    (app, obcr)
}

/// A riding app, the road's reader, and the executor riding it.
///
/// The reader is not optional furniture: a pass carrying `None` is the active route line
/// *vanishing*, which resets the matcher — so the executor that runs the passes carries it, exactly
/// as a host does.
macro_rules! riding {
    ($app:ident, $route:ident, $host:ident) => {
        riding!($app, $route, $host, road_obcr());
    };
    ($app:ident, $route:ident, $host:ident, $bytes:expr) => {
        let (mut $app, obcr) = riding_app_on($bytes);
        let src = SliceSource(&obcr[..]);
        let idx = RouteIndex::read(&src).unwrap();
        let $route = RouteReader::new(&idx, &src);
        #[allow(unused_mut, unused_variables)]
        let mut $host = Planner::on(&$route);
    };
}

/// Open the Detour chooser from the riding view.
fn open_chooser(app: &mut App) {
    app.apply_gesture(Gesture::BackHold); // → ride menu
    app.apply_gesture(Gesture::Step(1)); // Waypoints → Detour
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::Detour(_)), "the Detour station opens the chooser");
}

#[test]
fn full_flow_plans_previews_commits_and_reanchors_at_the_seam() {
    riding!(app, route, host);
    let session = app.ride_session();
    let progress = app.activity.progress_m();
    open_chooser(&mut app);

    // Chooser: two steps past the 600 m minimum, then Press → the planning spinner + request.
    app.apply_gesture(Gesture::Step(2));
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::NavPlanning(_)), "Press starts the plan flow");
    let req = detour_req(&mut app, &mut host).expect("Press hands the executor a detour search");
    assert_eq!(req.route, 0);
    assert_eq!(req.progress_m, progress, "the corridor anchor freezes at Press");
    assert_eq!(req.target_m, progress + 800, "600 m minimum + two steps");

    // The executor answers: preview polyline + figures → the preview screen with the cost line.
    app.set_detour_preview(&[(7_512_000, 43_501_000), (7_516_000, 43_501_000)]);
    answer_plan(
        &mut app,
        &mut host,
        Ok(DetourPreview { cost_delta_m: 420, total_distance_m: 1_220, rejoin_m: 2_000, ascent_m: None }),
    );
    assert!(matches!(app.top_screen(), Screen::DetourPreview(_)), "success swaps the spinner for the preview");

    // Commit: Press asks for the splice; the executor splices, rescans (both files exist: the
    // original and the reserved spliced route), and answers with the spliced durable id.
    app.apply_gesture(Gesture::Press);
    assert!(host.took_commit(&mut app), "Press asks the executor to splice");
    app.set_routes_with_ids(&[summary("Road"), summary("Detour · Road")], &[7, 9]);
    answer_commit(&mut app, &mut host, Ok(9));

    assert_eq!(app.active_route_index(), Some(1), "the spliced route re-adopts by durable id");
    assert_eq!(app.ride_session(), session, "the recording session is untouched");
    assert!(matches!(app.top_screen(), Screen::Map(_)), "the detour flow truncates back to the riding view");

    // The next route-aware tick installs the seam re-anchor: progress lands exactly at the
    // frozen anchor, and the forward-only floor holds against a fix back in the (now gone) span.
    tick(&mut app, 1_000, None, Some(&route));
    assert_eq!(app.activity.progress_m(), progress, "the seam re-anchor lands at the frozen anchor");
    tick(&mut app, 2_000, Some(road_at(0.06)), Some(&route));
    assert!(app.activity.progress_m() >= progress, "the floor is forward-only — no re-lock behind the seam");
}

#[test]
fn planning_back_cancels_and_failures_show_the_detour_tiers() {
    riding!(app, _route, host);
    open_chooser(&mut app);

    // Back on the spinner: pops to the chooser and releases the workspace (annihilating the plan).
    app.apply_gesture(Gesture::Press);
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::Detour(_)), "cancel returns to the chooser, steps intact");
    assert!(host.took_release(&mut app), "Back asks for the workspace back");
    assert!(detour_req(&mut app, &mut host).is_none(), "the cancel annihilated the request");

    // Replan; the executor fails with the range tier → the detour fail card, dismiss → chooser.
    app.apply_gesture(Gesture::Press);
    answer_plan(&mut app, &mut host, Err(NavError::Exhausted));
    match app.top_screen() {
        Screen::NavFail(card) => assert!(card.shows_too_far(), "Exhausted shows the range tier"),
        _ => panic!("failure swaps in the fail card"),
    }
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::Detour(_)), "dismiss lands back on the chooser");
    assert_eq!(app.active_route_index(), Some(0), "nothing re-adopts on failure");
}

#[test]
fn commit_failure_keeps_the_old_route_and_the_preview_retries() {
    riding!(app, _route, host);
    let session = app.ride_session();
    open_chooser(&mut app);
    app.apply_gesture(Gesture::Press);
    answer_plan(
        &mut app,
        &mut host,
        Ok(DetourPreview { cost_delta_m: 420, total_distance_m: 1_020, rejoin_m: 2_000, ascent_m: None }),
    );
    app.apply_gesture(Gesture::Press); // commit
    answer_commit(&mut app, &mut host, Err(NavError::NoPath));

    assert!(matches!(app.top_screen(), Screen::DetourPreview(_)), "a failed commit stays on the preview");
    assert_eq!(app.active_route_index(), Some(0), "the old route is untouched");
    assert_eq!(app.ride_session(), session);
    // The commit is retryable.
    app.apply_gesture(Gesture::Press);
    assert!(host.took_commit(&mut app));
}

// --- The Recalculating freeze (issue #1146, P2) ---

/// Drive a riding app to the **exposure window** the freeze exists for: a planner run the host has
/// started, with the map-base Detour chooser showing again because Back popped the spinner. Returns
/// with the freeze engaged and the cancel still undrained.
fn frozen_over_the_chooser(app: &mut App, host: &mut Planner<'_>) {
    open_chooser(app);
    app.apply_gesture(Gesture::Press); // → the planning spinner, and the search
    assert!(detour_req(app, host).is_some(), "the executor starts planning");
    app.apply_gesture(Gesture::Back); // pops the spinner; the executor's search is still running
    assert!(matches!(app.top_screen(), Screen::Detour(_)));
}

/// Whether a planner run holds the nav arm — `CoreMode`'s search level, read through the one public
/// mode. No transfer streams in this file, so `Searching` is exactly "a search is live".
fn searching(app: &App) -> bool {
    app.core_mode() == ModeState::Searching
}

fn rgb(c: u16) -> Rgb888 {
    let (r, g, b) = rgb565_to_rgb888(c);
    Rgb888::new(r, g, b)
}

/// **The regression** the freeze exists for: `NavPlanning` is *pushed* over the map-base chooser,
/// so Back lands a map base back under a search that is still running — and the next frame would
/// have rendered straight into the arena the planner owns. Draining the plan is the engaging edge
/// (not the gesture: a request the rider cancels first is annihilated and never reaches the host),
/// and the base screen is what decides whether there is anything to freeze.
#[test]
fn the_freeze_covers_a_live_search_exactly_while_a_map_base_would_draw() {
    riding!(app, _route, host);
    assert!(app.base_draws_map(), "riding on the Map");
    assert!(!searching(&app));
    assert!(!app.reroute_freeze_active(), "no plan, no freeze");
    assert!(app.nav_arena_precondition().is_none(), "…so a search may not take the arena yet");

    open_chooser(&mut app);
    app.apply_gesture(Gesture::Press);
    assert!(detour_req(&mut app, &mut host).is_some());
    assert!(searching(&app), "the handed-out search is what says the executor began planning");
    assert!(!app.base_draws_map(), "the spinner is an opaque chrome base — no map underneath to freeze");
    assert!(!app.reroute_freeze_active(), "menu-shaped planning needs no freeze");
    assert!(app.nav_arena_precondition().is_some(), "…and the arena is claimable: nothing will draw a map");

    app.apply_gesture(Gesture::Back);
    assert!(app.base_draws_map(), "the chooser is a map base again");
    assert!(searching(&app), "…while the host has not been told to stop yet");
    assert!(app.reroute_freeze_active(), "THE window — the freeze covers it");

    assert!(host.took_release(&mut app));
    assert!(!searching(&app), "the release reaching the executor ends the run");
    assert!(!app.reroute_freeze_active());
    assert!(app.nav_arena_precondition().is_none(), "a map base with no freeze refuses the nav claim again");
}

/// A silent freeze reads as a crash — the map stops answering and nothing says why. Pin that the
/// banner is on the **overlay** plane (the map plane is exactly what is not being redrawn), that
/// the host is told to paint it (`overlay_active`, the `Dirty::overlay` edge, the row band), and
/// that all of it vanishes with the freeze.
///
/// The window is the one a freeze can actually be *observed* in: a map base returning under a
/// search that is already running. The chooser's own Back pops the spinner and asks for the
/// workspace back in the same gesture, so the next pass ends the run — that path is
/// `the_board_loop_renders_the_map_again_the_pass_a_cancel_lands`.
#[test]
fn the_frozen_map_wears_a_recalculating_banner_on_the_overlay_plane() {
    riding!(app, _route, host);
    let _ = app.take_dirty();
    let _ = host.one_pass(&mut app); // settle the start-of-ride dirt

    app.apply_gesture(Gesture::BackHold); // the ride menu: an opaque chrome base
    app.debug_set_plan_live(true); // …under which the executor begins a planner run
    let _ = host.one_pass(&mut app);
    app.apply_gesture(Gesture::Back); // and a map base is back under the live search
    assert!(app.reroute_freeze_active());

    assert!(app.overlay_active(), "the host must paint the overlay layer while frozen");
    let _ = host.take_render();
    let dirty = host.one_pass(&mut app).render;
    assert!(dirty.overlay, "the freeze edge asks for one overlay repaint");
    let (y0, rows) = app.reroute_banner_rows(320.0).expect("the banner has a row band to re-present");

    let mut buf = common::Buf::new(240, 320);
    app.render_overlay(&mut buf, 240.0, 320.0, rgb);
    let ink = buf.count(rgb(palette::INK));
    let parchment = buf.count(rgb(palette::PARCHMENT));
    assert!(parchment > 500, "the pill fills its band ({parchment} px)");
    assert!(ink > 50, "…and carries outlined copy ({ink} px)");
    for y in 0..320 {
        for x in 0..240 {
            let drawn = buf.get(x, y) != Rgb888::new(0, 0, 0);
            if drawn {
                assert!(
                    y >= y0 as i32 && y < (y0 + rows) as i32,
                    "the banner drew at ({x},{y}), outside the band it reported"
                );
            }
        }
    }

    // Release: the banner comes off, and the map — which held still for the whole search — is asked
    // to repaint itself.
    app.debug_set_plan_live(false);
    let _ = host.take_render();
    let dirty = host.one_pass(&mut app).render;
    assert!(dirty.overlay, "…and one more to clear it");
    assert!(dirty.map, "the frozen map catches up");
    assert!(!app.overlay_active());
    assert!(app.reroute_banner_rows(320.0).is_none());
    let mut buf = common::Buf::new(240, 320);
    app.render_overlay(&mut buf, 240.0, 320.0, rgb);
    assert_eq!(buf.count(rgb(palette::PARCHMENT)), 0, "nothing is drawn once the run ends");
}

/// What one ride-loop pass puts on glass.
#[derive(Debug, PartialEq, Eq)]
enum Painted {
    /// The banner band only — the frozen branch (`ride.rs`: `dirty.overlay` while frozen).
    Banner,
    /// A whole frame — the ordinary render (`dirty.map` with no freeze).
    Frame,
    /// Nothing was pushed this pass.
    Nothing,
}

/// The board's ride loop, reduced to the part that decides what gets painted — in the board's order
/// and, crucially, with **one** pass per frame. That single pass is the whole point: the repaint
/// flags are one-shots the pass drains, so a loop that ran two silently hands itself an edge the
/// real one would have spent on the previous frame.
struct BoardLoop<'r> {
    /// The executor behind the loop.
    host: Planner<'r>,
    /// `ride.rs`'s latch: a map redraw the freeze swallowed, replayed the pass it lifts.
    pending_map_redraw: bool,
}

impl<'r> BoardLoop<'r> {
    fn new(route: &'r RouteReader<'r>) -> Self {
        BoardLoop { host: Planner::on(route), pending_map_redraw: false }
    }

    fn pass(&mut self, app: &mut App) -> Painted {
        let mut dirty = self.host.one_pass(app).render;
        if core::mem::take(&mut self.pending_map_redraw) {
            dirty.map = true;
        }
        let frozen = app.reroute_freeze_active();
        if frozen && dirty.map {
            self.pending_map_redraw = true;
            dirty.map = false;
        }
        if frozen {
            match app.reroute_banner_rows(320.0).filter(|_| dirty.overlay) {
                Some(_) => Painted::Banner,
                None => Painted::Nothing,
            }
        } else if dirty.map {
            Painted::Frame
        } else {
            Painted::Nothing
        }
    }
}

/// **The regression**, driven the way the board drives it: the freeze is a *level*, and the two
/// facts it is made of move independently. A plan that starts under the opaque planning spinner
/// freezes nothing — and that chrome frame is where a plan-start overlay edge goes to die. When a
/// map base comes back under the still-running search there is no plan edge left to raise the
/// banner, so a host keyed on it renders **nothing at all** for the rest of the search: stale pixels
/// on glass, no explanation, and input going to the screen underneath.
///
/// The chrome interlude is the ride menu and the plan is the simulator's `--freeze` seam, because
/// the detour flow's own way back to a map base (Back on the spinner) drains a cancel in the same
/// pass and ends the run — see `the_freeze_covers_a_live_search_exactly_while_a_map_base_would_draw`
/// for that path.
#[test]
fn the_banner_lands_when_a_map_base_returns_under_a_search_that_already_started() {
    riding!(app, route, _host);
    let mut board = BoardLoop::new(&route);
    let _ = board.pass(&mut app); // settle the start-of-ride dirt

    app.apply_gesture(Gesture::BackHold); // the ride menu: an opaque chrome base
    assert!(!app.base_draws_map(), "no map underneath the menu");
    app.debug_set_plan_live(true); // the host begins a planner run
    assert!(searching(&app));
    assert!(!app.reroute_freeze_active(), "a chrome base freezes nothing — and needs no banner");
    assert_eq!(board.pass(&mut app), Painted::Frame, "the menu frame renders normally");

    app.apply_gesture(Gesture::Back); // …and a map base is back under the live search
    assert!(app.reroute_freeze_active(), "THE window");
    assert_eq!(board.pass(&mut app), Painted::Banner, "the banner must land on this pass");
    assert_eq!(board.pass(&mut app), Painted::Nothing, "…once: a level, not a repaint per pass");
    assert_eq!(board.pass(&mut app), Painted::Nothing);

    app.debug_set_plan_live(false);
    assert_eq!(board.pass(&mut app), Painted::Frame, "the run ends and the frozen map catches up");
    assert!(app.reroute_banner_rows(320.0).is_none(), "with no banner over it");
}

/// **The regression** the plan families exist for, through the App: an answer that belongs to a
/// detour operation the app already abandoned must not release a freeze a *route* search is still
/// holding the nav arm behind — the next frame would claim the render arm, the arena would answer
/// `Busy(Nav)`, and the map would be dead for the rest of the ride.
#[test]
fn a_detour_terminal_edge_leaves_a_live_route_search_frozen() {
    riding!(app, _route, host);

    // A detour search the rider cancels: the executor is left holding its operation.
    frozen_over_the_chooser(&mut app, &mut host);
    assert!(host.took_release(&mut app), "the cancel reached the executor");
    let stale = host.abandoned().expect("the abandoned detour operation");

    app.debug_set_plan_live(true); // a route search, running over the map base
    assert!(app.reroute_freeze_active(), "the route search froze the map");
    assert!(app.nav_arena_precondition().is_some(), "…and holds the arm behind that freeze");

    // The abandoned detour answers anyway — the slow executor finishing behind the rider.
    host.deliver(&mut app, NavigatorOutcome::Failed { token: stale, error: NavigatorError::Plan(NavError::NoPath) });
    assert!(searching(&app), "the route search is untouched by another operation's answer");
    assert!(app.reroute_freeze_active(), "so the map stays frozen");
    host.deliver(&mut app, NavigatorOutcome::Failed { token: stale, error: NavigatorError::Store });
    assert!(app.reroute_freeze_active(), "…and by its commit failure too");

    // Only the route family's own terminal edge releases it.
    app.debug_set_plan_live(false);
    assert!(!app.reroute_freeze_active());
    assert!(!searching(&app));
}

/// And the mirror: a live **detour** plan is ended by nothing but its own terminal edge — not by an
/// answer that belongs to an operation the rider already walked away from. Same arm, same freeze,
/// different edges.
#[test]
fn a_route_cancel_leaves_a_live_detour_plan_frozen() {
    riding!(app, _route, host);

    // A route plan the rider cancels away, leaving the executor holding its operation…
    app.debug_start_nav((0, 0), (1, 1), "Bench");
    assert!(host.take_work(&mut app).is_some(), "the route search went out");
    app.apply_gesture(Gesture::Back);
    assert!(host.took_release(&mut app), "the rider cancelled it");
    let stale = host.abandoned().expect("the abandoned route operation");

    // …then a detour search.
    open_chooser(&mut app);
    app.apply_gesture(Gesture::Press);
    assert!(detour_req(&mut app, &mut host).is_some());
    assert!(searching(&app), "the detour search holds the arm");

    // The abandoned route operation answers late: it is not this run, so it changes nothing.
    host.deliver(&mut app, NavigatorOutcome::Failed { token: stale, error: NavigatorError::Plan(NavError::NoPath) });
    assert!(searching(&app), "the detour search is still the arm's holder");

    // Back puts a map base under it — frozen until its own release reaches the executor.
    app.apply_gesture(Gesture::Back);
    assert!(app.reroute_freeze_active(), "THE window, for the detour family");
    assert!(host.took_release(&mut app));
    assert!(!app.reroute_freeze_active(), "its own cancel is what releases it");
}

/// The same single-drain loop over the flow's *own* exit: the spinner is popped by Back, whose
/// cancel drains in the very next pass and ends the run — so the pass renders the map rather than a
/// banner, and nothing is left frozen behind it.
#[test]
fn the_board_loop_renders_the_map_again_the_pass_a_cancel_lands() {
    riding!(app, route, host);
    let mut board = BoardLoop::new(&route);
    let _ = board.pass(&mut app);

    open_chooser(&mut app);
    assert_eq!(board.pass(&mut app), Painted::Frame, "the chooser is a map base");
    app.apply_gesture(Gesture::Press); // → the spinner, and the request
    assert_eq!(board.pass(&mut app), Painted::Frame, "the spinner renders; the plan drains with it");
    assert!(searching(&app));

    app.apply_gesture(Gesture::Back); // pops the spinner *and* pends the cancel
    assert!(app.reroute_freeze_active(), "frozen until the cancel actually reaches the host");
    assert_eq!(board.pass(&mut app), Painted::Frame, "which it does on this pass — so the map redraws");
    assert!(!searching(&app));
    assert_eq!(board.pass(&mut app), Painted::Nothing, "and nothing is left demanding a repaint");
}

/// The freeze pauses **the matcher and nothing else**: progress holds still under the frozen frame
/// (a search can replace the very geometry it is measured along), while the fix itself keeps being
/// recorded — the camera, the breadcrumb, the ride totals and the altimeter all ride the same tick.
#[test]
fn a_frozen_tick_holds_route_progress_but_still_records_the_fix() {
    riding!(app, route, host);
    frozen_over_the_chooser(&mut app, &mut host);
    let held = app.activity.progress_m();

    let ahead = road_at(0.62);
    tick(&mut app, 1_000, Some(ahead), Some(&route));
    assert_eq!(app.activity.progress_m(), held, "the matcher did not advance under the freeze");
    assert_eq!(
        app.state.user_fix.map(|f| (f.lon, f.lat)),
        Some((ahead.lon, ahead.lat)),
        "…but the fix landed: a freeze pauses the map, not the ride"
    );

    // The cancel lifts it, and the very next fix re-locks from wherever the rider actually is.
    assert!(host.took_release(&mut app));
    assert!(!app.reroute_freeze_active());
    tick(&mut app, 2_000, Some(road_at(0.62)), Some(&route));
    assert!(app.activity.progress_m() > held, "the matcher resumes cleanly ({} m)", app.activity.progress_m());
}

/// …and it re-locks over the ground the rider covered *during* the search, not just the next fix's
/// worth. The on-route window is 64 **segments** ahead — sized for one fix's travel — while a plan
/// takes seconds on the SD-bound device, so on a route with real vertex density the rider rides
/// clean out of it. Without the one-shot wide re-lock the first match after the freeze finds
/// nothing in range: off-route chip up, progress still frozen, on a rider who never left the line.
/// The exit under test is a **cancel** on purpose — that is the shape the wide window exists for. A
/// search that comes back with new geometry resets the matcher instead and never spends the flag.
#[test]
fn the_matcher_relocks_over_the_ground_covered_during_the_freeze() {
    // 400 segments over the same road: ~8 m each, so the 64-segment on-route window reaches ~520 m
    // and the ride below covers ~1.3 km of it.
    riding!(app, route, host, road_obcr_segs(400, 0.00002));

    frozen_over_the_chooser(&mut app, &mut host);
    let held = app.activity.progress_m();
    for (i, frac) in [0.40, 0.50, 0.60, 0.70].into_iter().enumerate() {
        tick(&mut app, 1_000 + 1_000 * i as u32, Some(road_at(frac)), Some(&route));
    }
    assert_eq!(app.activity.progress_m(), held, "held still for the whole search, as designed");

    assert!(host.took_release(&mut app));
    tick(&mut app, 9_000, Some(road_at(0.72)), Some(&route));
    // Progress advancing *is* the on-route assertion: an off-route match freezes it.
    assert!(
        app.activity.progress_m() > 2_000,
        "the first fix after the freeze must re-lock where the rider is, not {} m back",
        app.activity.progress_m()
    );
}

/// The other two exits from a planner run — the answer and the failure — release the freeze too. A
/// stuck freeze is a map that never redraws again, so every edge that ends a run must clear it,
/// including a late answer whose planning screen the rider already cancelled away.
#[test]
fn every_way_a_plan_ends_releases_the_freeze() {
    // The answer: it lands on the map-base preview, which must render immediately.
    riding!(app, _route, host);
    open_chooser(&mut app);
    app.apply_gesture(Gesture::Press);
    assert!(detour_req(&mut app, &mut host).is_some());
    assert!(searching(&app));
    answer_plan(
        &mut app,
        &mut host,
        Ok(DetourPreview { cost_delta_m: 420, total_distance_m: 1_020, rejoin_m: 2_000, ascent_m: None }),
    );
    assert!(matches!(app.top_screen(), Screen::DetourPreview(_)));
    assert!(app.base_draws_map(), "the preview is a map base");
    assert!(!searching(&app), "the answer ended the run");
    assert!(!app.reroute_freeze_active(), "so the preview's first frame renders");

    // The failure tier: same release, on a card that isn't a map base at all.
    riding!(app, _route, host);
    open_chooser(&mut app);
    app.apply_gesture(Gesture::Press);
    answer_plan(&mut app, &mut host, Err(NavError::Exhausted));
    assert!(!searching(&app), "a failed run is still a finished run");

    // A late answer behind a cancel: the run already ended, and the abandoned operation's answer
    // must not re-engage (or leave) anything.
    riding!(app, _route, host);
    frozen_over_the_chooser(&mut app, &mut host);
    assert!(host.took_release(&mut app));
    assert!(!searching(&app));
    host.answer_late(&mut app, |token| NavigatorOutcome::Failed {
        token,
        error: NavigatorError::Plan(NavError::NoPath),
    });
    assert!(!searching(&app));
    assert!(!app.reroute_freeze_active(), "the map keeps rendering through a late answer");
}

/// A `DetourRequest` is `Copy` and its fields are what the host needs — pin the shape so a field
/// rename shows up here, not in a host at runtime.
#[test]
fn request_shape_is_stable() {
    let req = DetourRequest { route: 1, from: (2, 3), progress_m: 4, target_m: 5 };
    let copy = req;
    assert_eq!((copy.route, copy.from, copy.progress_m, copy.target_m), (1, (2, 3), 4, 5));
}
