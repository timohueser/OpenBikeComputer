#![cfg(feature = "external-fixtures")]
#[allow(dead_code)]
#[path = "../src/present.rs"]
mod present;
#[path = "../src/gui/support.rs"]
mod support;

use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::pixelcolor::Rgb565;
use obc_display::ls021::{FRAME_H, FRAME_W};
use obc_display::FbDevice64;
use obc_host_core::{flat_store::HostStore, FlatRouteStore, RouteRepository};
use present::*;

/// Driven through the real [`App`] and renderer over the demo map: an idle Home re-render pushes
/// no rows, a Home minute tick pushes the clock's rows, and a map pan pushes nearly all. The oracle
/// inside [`Present::present_now`] backs every count.
#[test]
fn app_scenarios_idle_is_free_tick_is_small_pan_is_most() {
    use obc_app::{App, AppState};
    use obc_ports::InputClock;
    use obc_reader::{MapCache, MapTables, Reader, SliceSource};

    // The device resolution comes from the one display authority.
    const W: u32 = FRAME_W as u32;
    const H: u32 = FRAME_H as u32;
    let bytes = obc_fixtures::read("sim-grimsel", "grimsel.obcm");
    let tables = MapTables::parse(&SliceSource(&bytes)).expect("valid demo map");
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let reader = Reader::new(&src, &tables, &cache);
    // A fixed street view stays inside the canonical fixture crop. Complete OSM ways can expand
    // the file bbox far beyond this rendered region.
    let (cx, cy, zoom) = (8_184_271, 46_727_359, W as f32 / 12_000.0);

    // Render the whole frame into the resident device-64 plane, as the GUI loop does: the device
    // colour path, not an RGB888 side buffer.
    let color_fn = |c: u16| Rgb565::from(RawU16::new(c));
    // The host's render scratch, built once and lent to every frame below.
    let mut scratch = Box::new(obc_render::RenderScratch::new());
    let mut render = |app: &mut App, fb: &mut [u8]| {
        let mut fbdev = FbDevice64::new(fb, W, H);
        app.render_frame(Some(&mut scratch), &mut fbdev, &reader, None, W as f32, H as f32, color_fn);
    };

    // Home idle, then a minute tick.
    let mut app = App::new_idle(AppState::new(cx, cy, zoom));
    app.reseed_home(0); // pin the contour backdrop so only the clock moves
    let mut fb = vec![0u8; (W * H) as usize];
    let mut present = Present::new(W, H);

    app.advance_animations(InputClock(0));
    render(&mut app, &mut fb);
    present.present_now(&fb, None); // first present: the whole frame
    render(&mut app, &mut fb);
    present.present_now(&fb, None);
    let idle = present.stats.pushed_rows;

    app.advance_animations(InputClock(60_000)); // +1 min: 12:00 → 12:01
    render(&mut app, &mut fb);
    present.present_now(&fb, None);
    let tick = present.stats.pushed_rows;

    // A fresh map, then a pan.
    let mut app = App::new(AppState::new(cx, cy, zoom));
    let mut fb = vec![0u8; (W * H) as usize];
    let mut present = Present::new(W, H);
    app.advance_animations(InputClock(0));
    render(&mut app, &mut fb);
    present.present_now(&fb, None); // first present: the whole frame
    app.state.cam_lon += 3_000; // pan a quarter of the fixed view
    render(&mut app, &mut fb);
    present.present_now(&fb, None);
    let pan = present.stats.pushed_rows;

    assert_eq!(idle, 0, "an idle Home re-render pushes nothing");
    assert!(tick > 0 && tick < H as usize / 3, "a minute tick pushes only the clock rows, got {tick}");
    // A pan invalidates far more rows than a clock tick, and the exact count depends on the
    // content, so the assertion is the three-tier contrast and not an absolute fraction.
    assert!(pan > 3 * tick && pan > H as usize / 3, "a map pan pushes far more than a tick, got pan {pan} tick {tick}");
}

/// Real-data pack and parse of the POI section: the committed `monaco.obcm`, a POI-dense coastal
/// fixture, must parse at the current format version and expose a POI directory with several
/// non-empty categories, each carrying a real quadtree, plus a populated nav graph. It complements
/// the reader's hand-built byte pins by exercising the whole write and read path on real
/// geometry.
#[test]
fn monaco_fixture_parses_populated_poi_and_nav_sections() {
    use obc_reader::{MapCache, MapTables, Reader, SliceSource};

    let bytes = obc_fixtures::read("sim-monaco", "monaco.obcm");
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).expect("monaco.obcm parses as a valid current map");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);

    assert_eq!(r.version, obc_formats::obcm::VERSION, "the fixture is the OBCM version this build reads");
    let dir = r.poi_directory();
    let mut expected: Vec<_> = obc_formats::obcm::PoiCategory::ALL.iter().map(|c| c.id()).collect();
    expected.push(obc_formats::obcm::SUMMIT_CATEGORY_ID);
    expected.push(obc_formats::obcm::SETTLEMENT_CATEGORY_ID);
    expected.sort_unstable();
    assert_eq!(dir.entries.iter().map(|e| e.category_id).collect::<Vec<_>>(), expected);
    assert_eq!(dir.chunk_size, 512, "the packer's fixed 512-byte POI chunks");
    // A dense coastal city populates several categories, and each must carry a real quadtree:
    // non-zero node and chunk counts, and known category ids.
    let populated: Vec<u8> = dir
        .entries
        .iter()
        .filter(|e| !e.is_empty())
        .inspect(|e| {
            assert!(expected.contains(&e.category_id), "known category {}", e.category_id);
            assert!(e.node_count > 0 && e.chunk_count > 0, "a non-empty category has a real tree");
        })
        .map(|e| e.category_id)
        .collect();
    assert!(populated.len() >= 3, "Monaco packs ≥3 POI categories, got {populated:?}");

    // The nav graph: a dense city extract must bake a routable graph, with a populated node
    // quadtree and a non-empty edge pool.
    let nav = r.nav_directory();
    assert!(!nav.is_empty(), "Monaco has routable streets");
    assert!(nav.chunk_count > 0 && nav.edge_chunk_count > 0, "node chunks + edge pool present");
    assert_eq!(nav.chunk_size, 512, "the packer's fixed 512-byte nav chunks");
    let mut scratch = [0u8; 512];
    let mut nodes = 0usize;
    r.for_each_nav_node(&r.bbox, &mut scratch, |n| {
        nodes += 1;
        assert!(n.degree() >= 1, "a junction always carries at least one arc");
    })
    .expect("nav walk over the whole bbox");
    assert!(nodes > 100, "a city extract yields a real junction set, got {nodes}");
}

/// One full sim frame, on the `gui.rs` skeleton: drain nav requests, step an in-flight route plan,
/// open the active route, advance the GPX replay and tick, render into the resident device-64
/// plane, then present. It then asserts the byte-equality postcondition.
#[allow(clippy::too_many_arguments)]
fn tour_frame(
    app: &mut obc_app::App,
    scratch: &mut obc_render::RenderScratch,
    fb: &mut [u8],
    present: &mut Present,
    player: &mut obc_replay::GpxPlayer,
    baro: &mut obc_replay::BaroSensor,
    store: &mut FlatRouteStore,
    host: &mut obc_host_core::HostLoop,
    session: &mut obc_host_core::ActiveRouteSession,
    map: &obc_host_core::flat_map::FlatMap,
    tour_active: bool,
    frame_no: &mut usize,
    label: &str,
) {
    use obc_route::RouteReader;

    const W: u32 = FRAME_W as u32;
    const H: u32 = FRAME_H as u32;

    let reader = map.reader();
    // The tour rides, and asks only once the device has reported the card that makes a ride
    // possible. Asking earlier is refused, and the refusal card would sit in the tour's frames.
    if app.active_route_index().is_some() && !app.recording() && app.can_record() {
        app.recorder.request(obc_app::RecorderIntent::Start);
    }

    // This tour uses only routes, so the ride, track and trip repositories are empty stand-ins.
    let mut rides = obc_host_core::MemRideStore::new(Vec::new());
    let mut tracks = obc_host_core::MemTrackStore::new();
    let mut no_trips = ();
    // The tour drives geometry and not terrain, so it wires the null terrain source.
    let mut elev = obc_route::NullElevation;

    // Open the active route's geometry from the resident session and run one DeviceCore pass
    // over it.
    //
    // The UI clock stands still at zero. The tour drives gestures directly, and its ambient reset
    // seeks the replay backwards, so a UI clock taken from playback time would run backwards with
    // it. A still clock advances no needle and arms no idle return.
    session.sync(app, store);
    let mut plan = {
        let route_src = store.active_source();
        let route = match (session.index(), route_src) {
            (Some(idx), Some(s)) => Some(RouteReader::new(idx, s)),
            _ => None,
        };
        let (ride, sensors) = obc_host_core::replay_advance(player, baro, None, 1.0 / 60.0, Default::default());
        host.pass(
            app,
            obc_app::device_core::PassClock { ride, ui: obc_ports::InputClock(0) },
            &[],
            sensors,
            route.as_ref(),
            support::SIM_SUPPORT,
        )
    };
    // The typed executor: the same `obc-host-core::HostLoop` the GUI drives, one bounded step per
    // frame.
    host.execute(app, &mut plan, session, store, &mut rides, &mut tracks, &mut no_trips, map, &mut elev, &mut ());

    // The wasm demo's ambient auto-restart, suppressed while a tour runs.
    if !tour_active && !player.is_playing() {
        player.play();
        app.recorder.request(obc_app::RecorderIntent::Start);
    }

    // Re-open the route for the render: the executor may have committed new geometry under it.
    session.sync(app, store);
    let route_src = store.active_source();
    let route = match (session.index(), route_src) {
        (Some(idx), Some(s)) => Some(RouteReader::new(idx, s)),
        _ => None,
    };

    // Render the whole frame into the resident device-64 plane.
    let mut fbdev = FbDevice64::new(fb, W, H);
    app.render_frame(Some(scratch), &mut fbdev, &reader, route.as_ref(), W as f32, H as f32, |c| {
        Rgb565::from(RawU16::new(c))
    });

    // Present, where the oracle asserts no miss, then the postcondition: after a clean present
    // the reconstruction equals the frame byte for byte on every row.
    present.present_now(fb, None);
    for y in 0..present.rows {
        let r = y * present.width..(y + 1) * present.width;
        assert!(
            present.presented[r.clone()] == fb[r],
            "presented != fb at row {y} after present (frame {frame_no} [{label}])"
        );
    }
    *frame_no += 1;
}

/// Drive the real `App` and renderer through the guided tour's own command sequences: the ambient
/// ride, an app rebuild and mid-climb seek per `enter`, the climb demo's Back-cycle, the
/// reroute-to-POI demo with its frame-stepped planner, and the ambient reset's backward seek. Each
/// tour screen is dwelt on for hundreds of presents, all under the oracle and the byte-equality
/// postcondition in [`tour_frame`].
#[test]
fn tour_screens_dwell_with_no_present_miss() {
    use std::path::Path;

    use obc_app::screen::Screen;
    use obc_app::settings::{ClimbMode, Settings};
    use obc_app::{App, AppState, CameraMode, Gesture};
    use obc_reader::{MapTables, SliceSource};
    use obc_replay::{gpx::Track, BaroSensor, GpxPlayer};

    const W: u32 = FRAME_W as u32;
    const H: u32 = FRAME_H as u32;
    let bytes = obc_fixtures::read("sim-grimsel", "grimsel.obcm");
    let tables = MapTables::parse(&SliceSource(&bytes)).expect("valid demo map");
    let owner = HostStore::memory().unwrap();
    let map = obc_host_core::flat_map::FlatMap::from_bytes_in(&owner, &bytes).unwrap();

    // The route and planner publications share the map's card and source identities.
    let mut store = FlatRouteStore::new(
        owner,
        &[include_bytes!("../../../fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr")],
    )
    .expect("seed demo route on the map card");

    let track = Track::load(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/sources/sim-grimsel/tracks/grimsel-climb-demo.gpx"
    )))
    .expect("gpx");
    let mut player = GpxPlayer::new(track);
    player.set_speed(3.0); // the page's ambient pace (obc-web-demo's `DEMO_SPEED`)
    let mut baro = BaroSensor::new();
    let mut host = obc_host_core::HostLoop::new();
    let mut session = obc_host_core::ActiveRouteSession::new();
    let mut fb = vec![0u8; (W * H) as usize];
    let mut present = Present::new(W, H);
    let mut frame_no = 0usize;
    // One host-owned render scratch for the whole tour, lent to every frame.
    let mut scratch = Box::new(obc_render::RenderScratch::new());

    // A fixed street view stays inside the canonical fixture crop. Complete OSM ways can expand
    // the file bbox far beyond this rendered region.
    let (cx, cy, zoom) = (8_184_271, 46_727_359, W as f32 / 12_000.0);
    let build_app = |settings: Settings, store: &FlatRouteStore| {
        let mut state = AppState::new(cx, cy, zoom * 12.0);
        state.mode = CameraMode::Follow;
        state.heading_up = true;
        let mut app = App::new(state);
        app.set_nav_profiles(tables.nav_profiles());
        app.set_routes_with_ids(store.catalog(), store.ids());
        app.set_settings(settings);
        if !store.catalog().is_empty() {
            app.activate_route(0);
        }
        app
    };

    // Run frames until the top screen matches, as the page's closed-loop polling does, then park
    // there for the dwell, which is where a missed row would sit stale. It panics if the target
    // screen is never reached, because otherwise the dwell would test the wrong screen.
    macro_rules! until_then_dwell {
        ($app:expr, $label:expr, $pat:pat, $dwell:expr) => {
            until_then_dwell!($app, $label, $pat, $dwell, true)
        };
        ($app:expr, $label:expr, $pat:pat, $dwell:expr, $ready:expr) => {{
            let mut reached = false;
            // Find measures a bounded batch of candidates through the serial planner.
            for _ in 0..1200 * obc_app::find_place::PLAN_LIMIT {
                tour_frame(
                    $app,
                    &mut scratch,
                    &mut fb,
                    &mut present,
                    &mut player,
                    &mut baro,
                    &mut store,
                    &mut host,
                    &mut session,
                    &map,
                    true,
                    &mut frame_no,
                    $label,
                );
                if matches!($app.top_screen(), $pat) && $ready {
                    reached = true;
                    break;
                }
            }
            assert!(
                reached,
                "never reached {}: find={:?}, review={:?}, origin={:?}",
                $label,
                $app.find_place_state(),
                $app.assistant_review_status(),
                $app.current_review_origin()
            );
            for _ in 0..$dwell {
                tour_frame(
                    $app,
                    &mut scratch,
                    &mut fb,
                    &mut present,
                    &mut player,
                    &mut baro,
                    &mut store,
                    &mut host,
                    &mut session,
                    &map,
                    true,
                    &mut frame_no,
                    $label,
                );
            }
        }};
    }

    // Page load: an ambient ride from the start.
    let mut app = build_app(Settings::default(), &store);
    player.seek(0.0);
    player.play();
    for _ in 0..120 {
        tour_frame(
            &mut app,
            &mut scratch,
            &mut fb,
            &mut present,
            &mut player,
            &mut baro,
            &mut store,
            &mut host,
            &mut session,
            &map,
            false,
            &mut frame_no,
            "ambient",
        );
    }

    // The climb demo: `enter`, then a Back-cycle to the Climb park. It runs first, while the
    // active route is still the demo climb.
    app = build_app(Settings { climb_mode: ClimbMode::Manual, ..Settings::default() }, &store);
    player.seek(1500.0);
    player.play();
    until_then_dwell!(&mut app, "climb: Map", Screen::Map(_), 300);
    app.apply_gesture(Gesture::Back);
    until_then_dwell!(&mut app, "climb: Statistics", Screen::Statistics(_), 300);
    app.apply_gesture(Gesture::Back);
    until_then_dwell!(&mut app, "climb: Climb", Screen::Climb(_), 300);
    app.apply_gesture(Gesture::Back);
    until_then_dwell!(&mut app, "climb: Map again", Screen::Map(_), 60);

    // The add-a-stop demo. Its `enter` is a reset from deep in the previous demo's session: an
    // app rebuild plus a backward seek.
    app = build_app(Settings { climb_mode: ClimbMode::Manual, ..Settings::default() }, &store);
    player.seek(1500.0);
    player.play();
    until_then_dwell!(&mut app, "reroute: Map", Screen::Map(_), 60);
    player.pause();
    let original = app.route_ids()[app.active_route_index().unwrap()];
    app.apply_chord(obc_app::input::Chord::Context);
    app.apply_gesture(Gesture::Press);
    app.apply_gesture(Gesture::Press);
    until_then_dwell!(&mut app, "visit: category", Screen::FindPlace(_), 45);
    app.apply_gesture(Gesture::Step(6));
    app.apply_gesture(Gesture::Press);
    until_then_dwell!(
        &mut app,
        "visit: station choices",
        Screen::FindPlace(_),
        300,
        app.find_place_state() == obc_app::find_place::State::Ready
            && app.find_place_result_count() > 0
            && app.assistant_planner_released()
    );
    app.apply_gesture(Gesture::Press);
    until_then_dwell!(
        &mut app,
        "visit: immutable preview",
        Screen::VisitReview(_),
        300,
        app.assistant_review_status() == obc_app::navigator::ReviewStatus::Preview
    );
    let preview = app.assistant_preview().unwrap();
    assert!(preview.visit_costs.is_some() && preview.distance_m > 0);
    assert_eq!(app.route_ids()[app.active_route_index().unwrap()], original);
    app.apply_gesture(Gesture::Press);
    until_then_dwell!(
        &mut app,
        "visit: accepted Map",
        Screen::Map(_),
        300,
        app.assistant_review_status() == obc_app::navigator::ReviewStatus::Accepted
    );
    assert_eq!(store.read_checkpoint().unwrap().unwrap().route, preview.source);
    assert_eq!(app.route_ids()[app.active_route_index().unwrap()], preview.source.object);

    // Back to the interactive page: the ambient reset, which is a big backward seek.
    app = build_app(Settings::default(), &store);
    player.seek(0.0);
    player.play();
    for _ in 0..300 {
        tour_frame(
            &mut app,
            &mut scratch,
            &mut fb,
            &mut present,
            &mut player,
            &mut baro,
            &mut store,
            &mut host,
            &mut session,
            &map,
            false,
            &mut frame_no,
            "ambient reset",
        );
    }
}

/// A mid-session `App` rebuild plus the seek that the baseline resets perform, followed by repeated
/// presents, twice: forward to mid-climb, then backward to the start. Every present runs under the
/// oracle and the byte-equality postcondition in [`tour_frame`].
#[test]
fn demo_reset_rebuild_and_seek_present_clean() {
    use std::path::Path;

    use obc_app::settings::{ClimbMode, Settings};
    use obc_app::{App, AppState, CameraMode};
    use obc_reader::{MapTables, SliceSource};
    use obc_replay::{gpx::Track, BaroSensor, GpxPlayer};

    const W: u32 = FRAME_W as u32;
    const H: u32 = FRAME_H as u32;
    let bytes = obc_fixtures::read("sim-grimsel", "grimsel.obcm");
    let tables = MapTables::parse(&SliceSource(&bytes)).expect("valid demo map");
    let owner = HostStore::memory().unwrap();
    let map = obc_host_core::flat_map::FlatMap::from_bytes_in(&owner, &bytes).unwrap();

    let mut store = FlatRouteStore::new(
        owner,
        &[include_bytes!("../../../fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr")],
    )
    .expect("seed demo route on the map card");

    let track = Track::load(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/sources/sim-grimsel/tracks/grimsel-climb.gpx"
    )))
    .expect("gpx");
    let mut player = GpxPlayer::new(track);
    player.set_speed(3.0);
    let mut baro = BaroSensor::new();
    let mut host = obc_host_core::HostLoop::new();
    let mut session = obc_host_core::ActiveRouteSession::new();
    let mut fb = vec![0u8; (W * H) as usize];
    let mut present = Present::new(W, H);
    let mut frame_no = 0usize;
    // One host-owned render scratch for the whole tour, lent to every frame.
    let mut scratch = Box::new(obc_render::RenderScratch::new());

    // A fixed street view stays inside the canonical fixture crop. Complete OSM ways can expand
    // the file bbox far beyond this rendered region.
    let (cx, cy, zoom) = (8_184_271, 46_727_359, W as f32 / 12_000.0);
    let build_app = |settings: Settings, store: &FlatRouteStore| {
        let mut state = AppState::new(cx, cy, zoom * 12.0);
        state.mode = CameraMode::Follow;
        state.heading_up = true;
        let mut app = App::new(state);
        app.set_nav_profiles(tables.nav_profiles());
        app.set_routes_with_ids(store.catalog(), store.ids());
        app.set_settings(settings);
        if !store.catalog().is_empty() {
            app.activate_route(0);
        }
        app
    };
    let mut run = |app: &mut App,
                   fb: &mut Vec<u8>,
                   present: &mut Present,
                   player: &mut GpxPlayer,
                   baro: &mut BaroSensor,
                   store: &mut FlatRouteStore,
                   host: &mut obc_host_core::HostLoop,
                   session: &mut obc_host_core::ActiveRouteSession,
                   tour: bool,
                   n: usize,
                   label: &str| {
        for _ in 0..n {
            tour_frame(
                app,
                &mut scratch,
                fb,
                present,
                player,
                baro,
                store,
                host,
                session,
                &map,
                tour,
                &mut frame_no,
                label,
            );
        }
    };

    // A short ambient ride, then the `enter` reset: rebuild and seek forward to mid-climb.
    let mut app = build_app(Settings::default(), &store);
    player.seek(0.0);
    player.play();
    run(
        &mut app,
        &mut fb,
        &mut present,
        &mut player,
        &mut baro,
        &mut store,
        &mut host,
        &mut session,
        false,
        60,
        "ambient",
    );
    app = build_app(Settings { climb_mode: ClimbMode::Manual, ..Settings::default() }, &store);
    player.seek(1500.0);
    player.play();
    run(
        &mut app,
        &mut fb,
        &mut present,
        &mut player,
        &mut baro,
        &mut store,
        &mut host,
        &mut session,
        true,
        300,
        "after enter",
    );

    // The ambient reset: rebuild and seek backward to the start.
    app = build_app(Settings::default(), &store);
    player.seek(0.0);
    player.play();
    run(
        &mut app,
        &mut fb,
        &mut present,
        &mut player,
        &mut baro,
        &mut store,
        &mut host,
        &mut session,
        false,
        300,
        "after ambient",
    );
}
