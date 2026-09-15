use super::*;
use obc_app::{find_place::State, AppState};
use obc_formats::obcm::{PoiApproach, PoiMetadata, SourceId};
use obc_pack::nav::{Edge, NavGraph, Node};
use obc_ports::{Fix, InputClock, LocationSource, RideClock};

struct Position;
impl LocationSource for Position {
    fn poll(&mut self) -> Option<Fix> {
        Some(Fix::at(500_000, 500_000))
    }
}

#[test]
fn find_runs_sixteen_sequential_real_visits_then_keeps_paging_independent() {
    use obc_app::navigator::ReviewStatus;
    for cancel_at in [None, Some(ReviewStatus::Planning), Some(ReviewStatus::Preview)] {
        run_find(cancel_at);
    }
}

fn run_find(cancel_at: Option<obc_app::navigator::ReviewStatus>) {
    let mut points: Vec<_> = (1..=8).rev().map(|n| (500_000, 503_400 + n * 100)).collect();
    points.push((500_000, 500_000));
    points.extend((1..=8).map(|n| (500_000 + n * 10_000, 500_000)));
    let graph = NavGraph {
        nodes: points.iter().enumerate().map(|(id, &coord)| Node { id: id as u32, coord }).collect(),
        edges: points
            .windows(2)
            .enumerate()
            .map(|(id, p)| Edge {
                a: id as u32,
                b: id as u32 + 1,
                polyline: p.to_vec(),
                length_m: obc_map_scene::ground_dist_m(p[0], p[1]) as u32,
                kind: 0,
            })
            .collect(),
    };
    let mut pois: Vec<_> = points
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 8)
        .map(|(i, &(lon, lat))| {
            let source = SourceId::osm(1, i as u64 + 1);
            obc_pack::poi::Poi {
                metadata: PoiMetadata { source, approach: Some(PoiApproach { source, lon, lat, profile_mask: 1 }) },
                access_nodes: vec![],
                wikidata: None,
                wikipedia: None,
                subtype: 1,
                lon_udeg: lon,
                lat_udeg: lat,
                name: Some(format!("Water {i}")),
                from_node: true,
                hours: obc_pack::hours::parse("Mo-Su 09:00-11:00"),
                elevation_m: None,
            }
        })
        .collect();
    let mut closed = pois[0].clone();
    closed.name = Some("Closed nearest".into());
    closed.lon_udeg = 499_990;
    closed.metadata.source = SourceId::osm(1, 99);
    closed.hours = obc_pack::hours::parse("off");
    pois.push(closed);
    let bbox = (490_000, 490_000, 590_000, 510_000);
    let lods =
        [obc_pack::LodLayer { max_mpp: None, chunk_size: 2048, root: obc_pack::Node::Leaf { bbox, features: vec![] } }];
    let profiles =
        [obc_pack::NavProfile { name: "Neutral".into(), highway: [16; 32], surface: [16; 8], climb_weight: 0 }];
    let bytes =
        obc_pack::serialize_lods(&lods, &[], 0, bbox, &pois, &graph, &profiles, &mut obc_elevation::NullElevation).0;
    let owner = crate::flat_store::HostStore::memory().unwrap();
    let map = crate::flat_map::FlatMap::from_bytes_in(&owner, &bytes).unwrap();
    let mut output = crate::VecSink::default();
    let gpx =
        br#"<gpx><trk><trkseg><trkpt lon="0.500" lat="0.500"/><trkpt lon="0.580" lat="0.500"/></trkseg></trk></gpx>"#;
    obc_route::gpx_to_obcr(&obc_formats::io::SliceSource(gpx), "Original", &mut output).unwrap();
    let source = obc_formats::io::SliceSource(output.bytes());
    let index = obc_route::RouteIndex::read(&source).unwrap();
    let route = obc_route::RouteReader::new(&index, &source);
    let mut routes = crate::FlatRouteStore::new(owner, &[output.bytes()]).unwrap();
    let original = routes.ids()[0];
    let mut app = App::new_idle(AppState::new(500_000, 500_000, 0.1));
    feed_routes(&mut app, &routes, &mut NoTrace);
    app.activate_route(0);
    app.stamp_clock_ble(1_727_000_000, 0);
    let mut host = HostLoop::new();
    let mut session = ActiveRouteSession::new();
    let mut rides = crate::MemRideStore::new(vec![]);
    let mut tracks = tests::RecordingTrackStore::default();
    let mut frame = crate::RgbaFrame::new(240, 320);
    let mut scratch = Box::new(obc_render::RenderScratch::new());
    let mut failures = Vec::new();
    let mut previews = 0;
    let mut acquisitions = 0;
    let mut releases = 0;
    let mut phase = 0;
    for tick in 0..4000 {
        if tick == 4 {
            app.apply_gesture(Gesture::BackHold);
            app.open_find_place();
            app.apply_gesture(Gesture::Press);
        }
        session.sync(&app, &mut routes);
        let mut position = Position;
        let mut plan = host.pass(
            &mut app,
            PassClock { ride: RideClock(tick * 100), ui: InputClock(tick * 100) },
            &[],
            Sensors::new(&mut position),
            Some(&route),
            tests::SUPPORT,
        );
        if let Some(effect) = plan.effects.navigator.take() {
            if matches!(effect, NavigatorEffect::Acquire { .. }) {
                assert!(host.plan_token().is_none());
                acquisitions += 1;
            }
            if matches!(effect, NavigatorEffect::Release { .. }) {
                releases += 1;
            }
            plan.effects.navigator.try_put(effect).unwrap();
        }
        host.execute(
            &mut app,
            &mut plan,
            &mut session,
            &mut routes,
            &mut rides,
            &mut tracks,
            &mut (),
            &map,
            &mut obc_route::NullElevation,
            &mut (),
        );
        if let obc_app::navigator::ReviewStatus::Failed(error) = app.assistant_review_status() {
            failures.push(error);
        }
        if app.assistant_review_status() == obc_app::navigator::ReviewStatus::Preview {
            previews += 1;
        }
        app.render_frame(Some(&mut scratch), &mut frame, &map.reader(), Some(&route), 240.0, 320.0, |c| {
            embedded_graphics::pixelcolor::Rgb888::from(embedded_graphics::pixelcolor::Rgb565::from(
                embedded_graphics::pixelcolor::raw::RawU16::new(c),
            ))
        });
        assert_eq!(app.route_ids()[app.active_route_index().unwrap()], original);
        if phase == 2 && cancel_at == Some(app.assistant_review_status()) {
            app.apply_gesture(Gesture::BackHold);
            assert!(matches!(app.top_screen(), obc_app::screen::Screen::Menu(_)));
            phase = 8;
        }
        match phase {
            8 if app.assistant_planner_released()
                && app.assistant_review_status() == obc_app::navigator::ReviewStatus::Idle =>
            {
                assert!(routes.read_checkpoint().unwrap().is_none());
                assert!(app.assistant_preview_shape().is_empty());
                assert!(releases >= acquisitions, "all planner owners receive release ACKs");
                phase = 9;
                break;
            }
            0 if app.find_place_state() == State::Ready => {
                assert_eq!(acquisitions, 16, "eight eligible nearby plus eight distinct forward places");
                assert!(releases >= acquisitions);
                assert!(app.assistant_planner_released());
                assert_eq!(app.find_place_result_count(), 4, "previews={previews}, failures={failures:?}");
                assert!(routes.read_checkpoint().unwrap().is_none());
                if let Ok(path) = std::env::var("OBC_FIND_FRAME") {
                    std::fs::write(path, frame.as_rgba()).unwrap();
                }
                app.apply_gesture(Gesture::Press);
                phase = 1;
            }
            1 if matches!(app.top_screen(), obc_app::screen::Screen::VisitReview(_)) => {
                phase = 2;
            }
            2 if app.assistant_review_status() == obc_app::navigator::ReviewStatus::Preview => {
                assert_eq!(acquisitions, 17, "selection recomputes one exact review");
                assert!(!app.assistant_preview_shape().is_empty(), "published shape is token-bound and readable");
                assert!(routes.read_checkpoint().unwrap().is_none());
                app.stamp_clock_ble(1_727_007_200, 0);
                app.apply_gesture(Gesture::Press);
                assert!(routes.read_checkpoint().unwrap().is_none(), "a place that closed cannot be accepted");
                assert_ne!(app.assistant_review_status(), obc_app::navigator::ReviewStatus::Saving);
                phase = 7;
            }
            7 => {
                assert_ne!(app.assistant_review_status(), obc_app::navigator::ReviewStatus::Preview);
                app.apply_gesture(Gesture::Back);
                phase = 3;
            }
            3 if app.assistant_planner_released() => {
                app.stamp_clock_ble(1_727_000_000, 0);
                assert_eq!(app.assistant_review_status(), obc_app::navigator::ReviewStatus::Idle);
                assert!(app.assistant_preview_shape().is_empty());
                assert!(matches!(app.top_screen(), obc_app::screen::Screen::FindPlace(_)));
                app.apply_gesture(Gesture::Step(4));
                app.apply_gesture(Gesture::Press);
                assert!(matches!(app.top_screen(), obc_app::screen::Screen::PoiList(_)));
                assert_eq!(app.find_place_state(), State::Idle);
                phase = 4;
            }
            4 => {
                app.apply_gesture(Gesture::Step(8));
                phase = 5;
            }
            5 => {
                app.apply_gesture(Gesture::Press);
                assert!(matches!(app.top_screen(), obc_app::screen::Screen::PoiDetail(_)));
                assert_eq!(acquisitions, 17, "paging does not plan more candidates");
                phase = 6;
                break;
            }
            _ => {}
        }
    }
    assert_eq!(phase, if cancel_at.is_some() { 9 } else { 6 }, "all requested phases complete");
}
