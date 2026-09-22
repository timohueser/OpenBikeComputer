use super::*;
use obc_app::{find_place::State, AppState};
use obc_formats::io::ByteSource;
use obc_formats::obcm::{PoiApproach, PoiMetadata, SourceId};
use obc_pack::nav::{Edge, NavGraph, Node};
use obc_ports::{Fix, InputClock, LocationSource, RideClock};

/// No Assistant review is in progress. A selected ordinary route's own checkpoint still stands.
fn settled(app: &App) -> bool {
    matches!(
        app.assistant_review_status(),
        obc_app::navigator::ReviewStatus::Idle | obc_app::navigator::ReviewStatus::Accepted
    ) && app.assistant_preview().is_none()
}

struct Position;
impl LocationSource for Position {
    fn poll(&mut self) -> Option<Fix> {
        Some(Fix::at(500_000, 500_000))
    }
}

#[test]
fn find_reuses_ranked_routes_and_releases_every_unaccepted_candidate() {
    for scenario in [
        Scenario::Browse,
        Scenario::FreeRide,
        Scenario::FreeAccept,
        Scenario::ModeCycle,
        Scenario::CancelPlanning,
        Scenario::CancelPreview,
        Scenario::Accept,
        Scenario::AcceptEscaped,
        Scenario::Deleted,
        Scenario::RestartReady,
        Scenario::RestartPartial,
        Scenario::RestartAccepted,
    ] {
        run_find(scenario);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    Browse,
    FreeRide,
    FreeAccept,
    ModeCycle,
    CancelPlanning,
    CancelPreview,
    Accept,
    AcceptEscaped,
    Deleted,
    RestartReady,
    RestartPartial,
    RestartAccepted,
}

fn run_find(scenario: Scenario) {
    let free_ride = matches!(scenario, Scenario::FreeRide | Scenario::FreeAccept);
    let expected_plans = if free_ride { 4 } else { 8 };
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
                population: None,
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
    if !free_ride {
        app.activate_route(0);
    }
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
    let mut restores = 0;
    let mut releases = 0;
    let mut selected_preview: Option<obc_app::navigator::ReviewedRoute> = None;
    let mut selected_shape = Vec::new();
    let mut choices_frame = Vec::new();
    let mut reentered = false;
    let mut recording_before_mode = false;
    let mut pending_without_render = 0;
    let mut calculated = Vec::new();
    let mut ordinary = None;
    let cancel_at = match scenario {
        Scenario::CancelPlanning => Some(obc_app::navigator::ReviewStatus::Planning),
        Scenario::CancelPreview => Some(obc_app::navigator::ReviewStatus::Preview),
        _ => None,
    };
    let mut phase = 0;
    for tick in 0..4000 {
        if tick == 4 {
            app.apply_gesture(Gesture::BackHold);
            app.open_find_place();
            app.apply_gesture(Gesture::Press);
        }
        session.sync(&app, &mut routes);
        let accepted_source =
            ((16..=18).contains(&phase)).then(|| routes.source(selected_preview.unwrap().source.object).unwrap());
        let accepted_index = accepted_source.as_ref().map(|source| obc_route::RouteIndex::read(source).unwrap());
        let accepted_route = accepted_source
            .as_ref()
            .zip(accepted_index.as_ref())
            .map(|(source, index)| obc_route::RouteReader::new(index, source));
        let pass_route = accepted_route.as_ref().or((!free_ride).then_some(&route));
        let mut position = Position;
        let mut plan = host.pass(
            &mut app,
            PassClock { ride: RideClock(tick * 100), ui: InputClock(tick * 100) },
            &[],
            Sensors::new(&mut position),
            pass_route,
            tests::SUPPORT,
        );
        if let Some(effect) = plan.effects.navigator.take() {
            if let NavigatorEffect::Acquire { work, .. } = effect {
                assert!(host.plan_token().is_none());
                if matches!(work, PlannerWork::RestoreReview(_)) {
                    restores += 1;
                } else {
                    acquisitions += 1;
                }
            }
            if phase > 0 && phase != 16 && !(19..=21).contains(&phase) {
                assert!(
                    !matches!(effect, NavigatorEffect::Step { .. } | NavigatorEffect::CommitRoute { .. }),
                    "selection and reopening must not run A* or publish another route"
                );
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
            if phase == 0 {
                let preview = app.assistant_preview().unwrap();
                if !calculated.contains(&preview.source) {
                    calculated.push(preview.source);
                }
            }
        }
        let mode_cycle = (19..=21).contains(&phase);
        if mode_cycle && app.assistant_route_pending() && !host.owns_navigation() {
            assert_eq!(app.ms_until_next_wake(tick * 100), Some(1));
            app.prepare_find(Some(&map.reader()), pass_route);
            if !app.assistant_route_pending() {
                plan.render.map = true;
            }
        }
        let defer_preview = mode_cycle && app.assistant_route_pending();
        pending_without_render += usize::from(defer_preview);
        if !defer_preview && ((!free_ride && !mode_cycle) || (plan.render.map && !app.reroute_freeze_active())) {
            app.render_frame(Some(&mut scratch), &mut frame, &map.reader(), pass_route, 240.0, 320.0, |c| {
                embedded_graphics::pixelcolor::Rgb888::from(embedded_graphics::pixelcolor::Rgb565::from(
                    embedded_graphics::pixelcolor::raw::RawU16::new(c),
                ))
            });
        }
        if free_ride && phase < 12 {
            assert!(app.active_route_index().is_none());
        } else if !matches!(
            scenario,
            Scenario::FreeAccept
                | Scenario::ModeCycle
                | Scenario::Accept
                | Scenario::AcceptEscaped
                | Scenario::RestartAccepted
        ) || phase < 12
        {
            assert_eq!(app.route_ids()[app.active_route_index().unwrap()], original);
        }
        if phase == 2 && cancel_at == Some(app.assistant_review_status()) {
            app.apply_gesture(Gesture::BackHold);
            assert!(matches!(app.top_screen(), obc_app::screen::Screen::Menu(_)));
            phase = 8;
        }
        let restart = phase == 0
            && ((scenario == Scenario::RestartReady
                && app.find_place_state() == State::Ready
                && routes.ids().len() == 5)
                || (scenario == Scenario::RestartPartial && calculated.len() == 4))
            || phase == 12
                && scenario == Scenario::RestartAccepted
                && app.assistant_review_status() == obc_app::navigator::ReviewStatus::Accepted;
        if restart {
            if scenario == Scenario::RestartReady {
                let fingerprint = *calculated
                    .iter()
                    .find(|fingerprint| routes.fingerprint(fingerprint.object) == Some(**fingerprint))
                    .unwrap();
                let source = obc_formats::obcr::RouteSourceKey {
                    store: routes.store_scope().unwrap().store.bytes(),
                    object: fingerprint.object,
                    revision: fingerprint.revision,
                };
                let bytes = routes.pin_review(source).unwrap();
                let mut payload = vec![0; bytes.len() as usize];
                bytes.read_at(0, &mut payload).unwrap();
                for _ in 0..20 {
                    routes.publish_review_route(&payload).unwrap();
                }
                ordinary = Some(routes.publish_nav_route(output.bytes()).unwrap().id);
            }
            app = App::new_idle(AppState::new(500_000, 500_000, 0.1));
            feed_routes(&mut app, &routes, &mut NoTrace);
            if scenario != Scenario::RestartAccepted {
                app.activate_route(routes.ids().iter().position(|id| *id == original).unwrap());
            }
            host = HostLoop::new();
            session = ActiveRouteSession::new();
            phase = 14;
            continue;
        }
        match phase {
            14 if routes.unaccepted_routes() == 0 => {
                assert_eq!(
                    routes.ids().len(),
                    if scenario == Scenario::RestartAccepted || ordinary.is_some() { 2 } else { 1 }
                );
                assert!(routes.ids().contains(&original));
                if let Some(ordinary) = ordinary {
                    assert!(routes.ids().contains(&ordinary), "ordinary routes are not orphaned reviews");
                }
                if scenario == Scenario::RestartAccepted {
                    let checkpoint = routes.read_checkpoint().unwrap().unwrap();
                    assert_eq!(checkpoint.route, selected_preview.unwrap().source);
                    assert_eq!(checkpoint.original.unwrap().object, original);
                    assert!(routes.ids().contains(&checkpoint.route.object));
                    assert_eq!(app.assistant_review_status(), obc_app::navigator::ReviewStatus::ResumeAvailable);
                } else {
                    assert!(
                        routes.read_checkpoint().unwrap().is_none_or(|saved| saved.route.object == original),
                        "only the selected ordinary route is checkpointed, never a candidate"
                    );
                }
                phase = 15;
                break;
            }
            8 if app.assistant_planner_released() && settled(&app) && routes.ids() == [original] => {
                assert!(
                    routes.read_checkpoint().unwrap().is_none_or(|saved| saved.route.object == original),
                    "only the selected ordinary route is checkpointed, never a candidate"
                );
                assert!(app.assistant_preview_shape().is_empty());
                assert!(releases >= acquisitions + restores, "all planner owners receive release ACKs");
                if free_ride && !reentered {
                    app.open_find_place();
                    app.apply_gesture(Gesture::Press);
                    acquisitions = 0;
                    restores = 0;
                    releases = 0;
                    calculated.clear();
                    reentered = true;
                    phase = 0;
                    continue;
                }
                phase = 9;
                break;
            }
            0 if app.find_place_state() == State::Ready && routes.ids().len() == 5 => {
                assert_eq!(acquisitions, expected_plans, "all eligible nearby and forward places are measured");
                assert_eq!(calculated.len(), expected_plans);
                assert_eq!(restores, 0);
                assert_eq!(routes.unaccepted_routes().count_ones(), 4, "only the ranked choices remain after pruning");
                assert!(releases >= acquisitions);
                assert!(app.assistant_planner_released());
                assert_eq!(app.find_place_result_count(), 4, "previews={previews}, failures={failures:?}");
                assert!(
                    routes.read_checkpoint().unwrap().is_none_or(|saved| saved.route.object == original),
                    "only the selected ordinary route is checkpointed, never a candidate"
                );
                if let Ok(path) = std::env::var("OBC_FIND_FRAME") {
                    std::fs::write(path, frame.as_rgba()).unwrap();
                }
                choices_frame = frame.as_rgba().to_vec();
                app.apply_gesture(Gesture::Press);
                phase = 1;
            }
            1 if matches!(app.top_screen(), obc_app::screen::Screen::VisitReview(_)) => {
                phase = 2;
            }
            2 if app.assistant_review_status() == obc_app::navigator::ReviewStatus::Preview
                && app.assistant_planner_released() =>
            {
                assert_eq!(acquisitions, expected_plans);
                assert_eq!(restores, 1);
                let preview = app.assistant_preview().unwrap();
                assert!(calculated.contains(&preview.source), "selection reuses a measured candidate");
                assert!(!app.assistant_preview_shape().is_empty(), "published shape is token-bound and readable");
                assert!(
                    routes.read_checkpoint().unwrap().is_none_or(|saved| saved.route.object == original),
                    "only the selected ordinary route is checkpointed, never a candidate"
                );
                selected_preview = Some(preview);
                selected_shape = app.assistant_preview_shape().to_vec();
                if scenario == Scenario::ModeCycle {
                    recording_before_mode = app.recorder.recording();
                    app.apply_gesture(Gesture::Step(1));
                    phase = 19;
                    continue;
                }
                if matches!(
                    scenario,
                    Scenario::FreeAccept
                        | Scenario::ModeCycle
                        | Scenario::Accept
                        | Scenario::AcceptEscaped
                        | Scenario::RestartAccepted
                ) {
                    app.apply_gesture(Gesture::Press);
                    if scenario == Scenario::AcceptEscaped {
                        app.apply_gesture(Gesture::BackHold);
                    }
                    phase = 12;
                } else {
                    app.apply_gesture(Gesture::Back);
                    phase = 10;
                }
            }
            19..=21
                if app.assistant_review_status() == obc_app::navigator::ReviewStatus::Preview
                    && app.assistant_planner_released() =>
            {
                let destination = phase != 20;
                let preview = app.assistant_preview().unwrap();
                let context = app.assistant_review_context().unwrap();
                assert_eq!(
                    context.purpose,
                    if destination {
                        obc_app::navigator::ReviewPurpose::Destination
                    } else {
                        obc_app::navigator::ReviewPurpose::Visit
                    }
                );
                let source = routes.source(preview.source.object).unwrap();
                let info = obc_route::RouteObjectInfo::read(&source).unwrap();
                assert_eq!(info.visit.is_none(), destination);
                assert_eq!(acquisitions, expected_plans + phase as usize - 18);
                assert_eq!(app.route_ids()[app.active_route_index().unwrap()], original);
                assert_eq!(app.recorder.recording(), recording_before_mode);
                if phase == 21 {
                    selected_preview = Some(preview);
                    app.apply_gesture(Gesture::Press);
                    phase = 12;
                } else {
                    app.apply_gesture(Gesture::Step(1));
                    phase += 1;
                }
            }
            10 if app.assistant_planner_released() => {
                assert!(settled(&app), "no review is in progress: {:?}", app.assistant_review_status());
                assert!(app.assistant_preview_shape().is_empty());
                if free_ride {
                    assert_eq!(
                        frame.as_rgba(),
                        choices_frame,
                        "Back must repaint the choices without preview geometry"
                    );
                }
                let preview = selected_preview.unwrap();
                assert_eq!(routes.fingerprint(preview.source.object), Some(preview.source), "Back retains exact bytes");
                if scenario == Scenario::Deleted {
                    routes
                        .retract_nav_route(crate::RoutePublication {
                            store: routes.store_scope().map(|scope| scope.store),
                            id: preview.source.object,
                            revision: preview.source.revision,
                        })
                        .unwrap();
                    feed_routes(&mut app, &routes, &mut NoTrace);
                }
                app.apply_gesture(Gesture::Press);
                phase = 11;
            }
            11 if scenario == Scenario::Deleted
                && matches!(app.assistant_review_status(), obc_app::navigator::ReviewStatus::Failed(_)) =>
            {
                assert_eq!(acquisitions, expected_plans);
                assert!(app.assistant_preview().is_none(), "a removed source cannot be restored");
                app.apply_gesture(Gesture::BackHold);
                phase = 8;
            }
            11 if app.assistant_review_status() == obc_app::navigator::ReviewStatus::Preview => {
                assert_eq!(acquisitions, expected_plans);
                assert_eq!(restores, 2);
                assert_eq!(app.assistant_preview(), selected_preview);
                assert_eq!(app.assistant_preview_shape(), selected_shape);
                if free_ride {
                    assert!(app.assistant_review_context().unwrap().original.is_none());
                    app.apply_gesture(Gesture::BackHold);
                    phase = 8;
                    continue;
                }
                app.stamp_clock_ble(1_727_007_200, 0);
                phase = 7;
            }
            12 if app.assistant_review_status() == obc_app::navigator::ReviewStatus::Accepted
                && routes.ids().len() == 2 =>
            {
                let preview = selected_preview.unwrap();
                assert_eq!(routes.fingerprint(preview.source.object), Some(preview.source));
                assert_eq!(routes.read_checkpoint().unwrap().unwrap().route, preview.source);
                assert_eq!(app.route_ids()[app.active_route_index().unwrap()], preview.source.object);
                assert_eq!(routes.unaccepted_routes(), 0);
                if scenario == Scenario::ModeCycle {
                    assert!(
                        pending_without_render > 0,
                        "deferred mode changes advance while preview redraws are withheld"
                    );
                    assert!(routes.read_checkpoint().unwrap().unwrap().original.is_none());
                    assert!(app.recorder.recording());
                }
                if scenario == Scenario::FreeAccept {
                    app.open_find_place();
                    app.apply_gesture(Gesture::Press);
                    phase = 16;
                } else {
                    phase = 13;
                    break;
                }
            }
            16 if app.find_place_state() == State::Ready && app.assistant_planner_released() => {
                assert!(app.find_place_result_count() > 0, "a new search completes with an accepted journey");
                assert_eq!(routes.read_checkpoint().unwrap().unwrap().route, selected_preview.unwrap().source);
                app.apply_gesture(Gesture::BackHold);
                phase = 17;
            }
            17 if routes.ids().len() == 2 && app.assistant_planner_released() => {
                assert_eq!(app.assistant_review_status(), obc_app::navigator::ReviewStatus::Accepted);
                assert!(app.assistant_preview().is_none());
                assert!(app.assistant_preview_shape().is_empty());
                assert_eq!(routes.unaccepted_routes(), 0);
                assert_eq!(routes.read_checkpoint().unwrap().unwrap().route, selected_preview.unwrap().source);
                phase = 18;
                break;
            }
            7 => {
                assert_eq!(app.assistant_review_status(), obc_app::navigator::ReviewStatus::Preview);
                assert_eq!(app.assistant_preview(), selected_preview, "closing hours preserve the reviewed route");
                assert_eq!(app.assistant_preview_shape(), selected_shape);
                assert!(
                    routes.read_checkpoint().unwrap().is_none_or(|saved| saved.route.object == original),
                    "only the selected ordinary route is checkpointed, never a candidate"
                );
                app.apply_gesture(Gesture::Back);
                phase = 3;
            }
            3 if app.assistant_planner_released() => {
                app.stamp_clock_ble(1_727_000_000, 0);
                assert!(settled(&app), "no review is in progress: {:?}", app.assistant_review_status());
                assert!(app.assistant_preview_shape().is_empty());
                assert!(matches!(app.top_screen(), obc_app::screen::Screen::FindPlace(_)));
                app.apply_gesture(Gesture::Step(4));
                app.apply_gesture(Gesture::Press);
                assert!(matches!(app.top_screen(), obc_app::screen::Screen::PoiList(_)));
                assert_eq!(app.find_place_state(), State::Ready);
                phase = 4;
            }
            4 => {
                app.apply_gesture(Gesture::Step(8));
                phase = 5;
            }
            5 => {
                app.apply_gesture(Gesture::Press);
                assert!(matches!(app.top_screen(), obc_app::screen::Screen::PoiDetail(_)));
                assert_eq!(acquisitions, expected_plans, "paging does not plan more candidates");
                app.apply_gesture(Gesture::Back);
                assert!(matches!(app.top_screen(), obc_app::screen::Screen::PoiList(_)));
                app.apply_gesture(Gesture::Back);
                assert!(matches!(app.top_screen(), obc_app::screen::Screen::FindPlace(_)));
                assert_eq!(app.find_place_state(), State::Ready);
                app.apply_gesture(Gesture::BackHold);
                phase = 6;
            }
            6 if routes.ids() == [original] => break,
            _ => {}
        }
    }
    let expected = match scenario {
        Scenario::FreeAccept => 18,
        Scenario::ModeCycle | Scenario::Accept | Scenario::AcceptEscaped => 13,
        Scenario::Browse => 6,
        Scenario::RestartReady | Scenario::RestartPartial | Scenario::RestartAccepted => 15,
        _ => 9,
    };
    assert_eq!(
        phase,
        expected,
        "{scenario:?}: all requested phases complete; failures={failures:?}; review={:?}; routes={:?}",
        app.assistant_review_status(),
        routes.ids()
    );
    if !matches!(
        scenario,
        Scenario::FreeAccept
            | Scenario::ModeCycle
            | Scenario::Accept
            | Scenario::AcceptEscaped
            | Scenario::RestartReady
            | Scenario::RestartPartial
            | Scenario::RestartAccepted
    ) {
        assert_eq!(routes.ids(), &[original], "{scenario:?}: no retained route leaks after leaving Find");
    }
}
