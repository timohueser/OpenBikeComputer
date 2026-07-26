//! The routed-detour flow (#882), end to end through the real gesture path: ride a real route
//! (matcher locked by ticks over genuine OBCR geometry), open the ride menu's Detour station,
//! step the chooser, Press into the planning spinner (draining the `PlanDetour` request), land
//! the host's `DetourPlanned` answer on the preview, commit (draining `CommitDetour`), and land
//! `DetourCommitted` — asserting the re-adoption choreography: active route swapped by durable
//! id, the recording session untouched, the stack back on the riding view, and the seam
//! re-anchor installing matcher progress + the forward-only floor on the next route-aware tick.
//! The failure tiers and the planning-screen cancel run through the same seams.
//!
//! The host itself is simulated at the protocol boundary (drained commands answered with typed
//! events), exactly like `nav.rs`; the *real* corridor/A*/splice pipeline is pinned end to end
//! in `obc-route/tests/detour.rs` and by the sim.

use obc_app::screen::Screen;
use obc_app::{
    App, AppState, DetourPreview, DetourRequest, Fix, Gesture, HostCommand, HostEvent, HostMailbox, LocationSource,
    RideClock, RouteSummary, Sensors,
};
use obc_formats::io::SliceSource;
use obc_route::{gpx_to_obcr, NavError, RouteIndex, RouteReader};

/// The straight test road: lat 43.5°, lon 7.50° → 7.54° (~3 230 m ground). One `<trkpt>` per
/// 0.004° so the converter keeps real vertices along the way.
const LAT: f64 = 43.5;
const LON0: f64 = 7.50;

fn road_obcr() -> Vec<u8> {
    let mut g = String::from("<gpx><trk><trkseg>\n");
    for i in 0..=10 {
        let lon = LON0 + i as f64 * 0.004;
        g.push_str(&format!("  <trkpt lat=\"{LAT:.7}\" lon=\"{lon:.7}\"><ele>100.0</ele></trkpt>\n"));
    }
    g.push_str("</trkseg></trk></gpx>");
    let src = SliceSource(g.as_bytes());
    let mut sink = VecSink::default();
    gpx_to_obcr(&src, "Road", &mut sink).unwrap();
    sink.0
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
    app.tick(
        RideClock(now_ms),
        Sensors {
            loc: &mut loc,
            altimeter: None,
            temperature: None,
            clock: None,
            compass: None,
            track: None,
            fuel: None,
            hr: None,
            power: None,
            cadence: None,
        },
        route,
    );
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
        bbox: obc_route::BBox {
            min_lon: (LON0 * 1e6) as i32,
            min_lat: (LAT * 1e6) as i32,
            max_lon: ((LON0 + 0.04) * 1e6) as i32,
            max_lat: (LAT * 1e6) as i32 + 1,
        },
        start_lon: (LON0 * 1e6) as i32,
        start_lat: (LAT * 1e6) as i32,
    }
}

/// Drain the typed protocol and return every command (the per-test host boundary).
fn drained(app: &mut App) -> Vec<HostCommand> {
    let mut mb: HostMailbox = HostMailbox::new();
    let _ = app.drain_host_commands(&mut mb);
    core::iter::from_fn(|| mb.pop()).collect()
}

/// A riding app with the matcher locked ~1 km along the real road, the Detour chooser reachable.
/// Returns `(app, obcr_bytes)` — build the `RouteReader` per call from the bytes.
fn riding_app() -> (App, Vec<u8>) {
    let obcr = road_obcr();
    let mut app = App::new_idle(AppState::new((LON0 * 1e6) as i32, (LAT * 1e6) as i32, 0.05));
    app.set_map_nav_graph(true);
    app.set_routes_with_ids(&[summary("Road")], &[7]);
    app.state.user_fix = Some(road_at(0.0));
    // Home → Menu (Routes) → Route menu → overview → START RIDE.
    app.apply_gesture(Gesture::BackHold);
    app.apply_gesture(Gesture::Press);
    app.apply_gesture(Gesture::Press);
    app.apply_gesture(Gesture::Press);
    assert!(app.activity.is_tracking(), "the ride started");
    assert!(matches!(app.top_screen(), Screen::Map(_)));

    // Lock the matcher ~1 km along with a real route-aware tick.
    let src = SliceSource(&obcr[..]);
    let idx = RouteIndex::read(&src).unwrap();
    let route = RouteReader::new(&idx, &src);
    tick(&mut app, 0, Some(road_at(0.31)), Some(&route));
    assert!(app.activity.progress_m() > 900 && app.activity.progress_m() < 1_100, "matcher locked ~1 km along");
    (app, obcr)
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
    let (mut app, obcr) = riding_app();
    let session = app.activity.session();
    let progress = app.activity.progress_m();
    open_chooser(&mut app);

    // Chooser: two steps past the 600 m minimum, then Press → the planning spinner + request.
    app.apply_gesture(Gesture::Step(2));
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::NavPlanning(_)), "Press starts the plan flow");
    let cmds = drained(&mut app);
    let req = cmds
        .iter()
        .find_map(|c| match c {
            HostCommand::PlanDetour(r) => Some(*r),
            _ => None,
        })
        .expect("Press drains a PlanDetour request");
    assert_eq!(req.route, 0);
    assert_eq!(req.progress_m, progress, "the corridor anchor freezes at Press");
    assert_eq!(req.target_m, progress + 800, "600 m minimum + two steps");

    // Host answers: preview polyline + figures → the preview screen with the cost line.
    app.set_detour_preview(&[(7_512_000, 43_501_000), (7_516_000, 43_501_000)]);
    app.apply_event(HostEvent::DetourPlanned(Ok(DetourPreview { cost_delta_m: 420, total_distance_m: 1_220 })));
    assert!(matches!(app.top_screen(), Screen::DetourPreview(_)), "success swaps the spinner for the preview");

    // Commit: Press drains the one-shot; the host splices, rescans (both files exist: the
    // original and the reserved spliced route), and answers with the spliced durable id.
    app.apply_gesture(Gesture::Press);
    assert!(drained(&mut app).iter().any(|c| matches!(c, HostCommand::CommitDetour)), "Press drains CommitDetour");
    app.set_routes_with_ids(&[summary("Road"), summary("Detour · Road")], &[7, 9]);
    app.apply_event(HostEvent::DetourCommitted(Ok(9)));

    assert_eq!(app.active_route_index(), Some(1), "the spliced route re-adopts by durable id");
    assert_eq!(app.activity.session(), session, "the recording session is untouched");
    assert!(matches!(app.top_screen(), Screen::Map(_)), "the detour flow truncates back to the riding view");

    // The next route-aware tick installs the seam re-anchor: progress lands exactly at the
    // frozen anchor, and the forward-only floor holds against a fix back in the (now gone) span.
    let src = SliceSource(&obcr[..]);
    let idx = RouteIndex::read(&src).unwrap();
    let route = RouteReader::new(&idx, &src);
    tick(&mut app, 1_000, None, Some(&route));
    assert_eq!(app.activity.progress_m(), progress, "the seam re-anchor lands at the frozen anchor");
    tick(&mut app, 2_000, Some(road_at(0.06)), Some(&route));
    assert!(app.activity.progress_m() >= progress, "the floor is forward-only — no re-lock behind the seam");
}

#[test]
fn planning_back_cancels_and_failures_show_the_detour_tiers() {
    let (mut app, _obcr) = riding_app();
    open_chooser(&mut app);

    // Back on the spinner: pops to the chooser and drains the cancel (annihilating the plan).
    app.apply_gesture(Gesture::Press);
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::Detour(_)), "cancel returns to the chooser, steps intact");
    let cmds = drained(&mut app);
    assert!(cmds.iter().any(|c| matches!(c, HostCommand::CancelDetour)), "Back drains CancelDetour");
    assert!(!cmds.iter().any(|c| matches!(c, HostCommand::PlanDetour(_))), "the cancel annihilated the request");

    // Replan; the host fails with the range tier → the detour fail card, dismiss → chooser.
    app.apply_gesture(Gesture::Press);
    let _ = drained(&mut app);
    app.apply_event(HostEvent::DetourPlanned(Err(NavError::Exhausted)));
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
    let (mut app, _obcr) = riding_app();
    let session = app.activity.session();
    open_chooser(&mut app);
    app.apply_gesture(Gesture::Press);
    let _ = drained(&mut app);
    app.apply_event(HostEvent::DetourPlanned(Ok(DetourPreview { cost_delta_m: 420, total_distance_m: 1_020 })));
    app.apply_gesture(Gesture::Press); // commit
    let _ = drained(&mut app);
    app.apply_event(HostEvent::DetourCommitted(Err(NavError::NoPath)));

    assert!(matches!(app.top_screen(), Screen::DetourPreview(_)), "a failed commit stays on the preview");
    assert_eq!(app.active_route_index(), Some(0), "the old route is untouched");
    assert_eq!(app.activity.session(), session);
    // The commit is retryable.
    app.apply_gesture(Gesture::Press);
    assert!(drained(&mut app).iter().any(|c| matches!(c, HostCommand::CommitDetour)));
}

/// A `DetourRequest` is `Copy` and its fields are what the host needs — pin the shape so a field
/// rename shows up here, not in a host at runtime.
#[test]
fn request_shape_is_stable() {
    let req = DetourRequest { route: 1, from: (2, 3), progress_m: 4, target_m: 5 };
    let copy = req;
    assert_eq!((copy.route, copy.from, copy.progress_m, copy.target_m), (1, (2, 3), 4, 5));
}
