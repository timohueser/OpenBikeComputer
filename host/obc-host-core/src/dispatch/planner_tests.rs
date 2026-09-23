use super::*;
use crate::flat_map::FlatMap;
use crate::flat_store::HostStore;
use obc_app::device_core::TokenSource;
use obc_storage::flat::{DisplayName, ObjectId, ObjectKind, Revision, Store};

pub(super) fn map_bytes() -> Vec<u8> {
    map_bytes_with(&mut obc_route::NullElevation)
}
fn map_bytes_with(elevation: &mut dyn obc_route::ElevationSource) -> Vec<u8> {
    use obc_pack::nav::{Edge, NavGraph, Node};
    let coords = [(500_000, 500_000), (520_000, 500_000), (510_000, 510_000)];
    let nodes = coords.iter().enumerate().map(|(id, &coord)| Node { id: id as u32, coord }).collect();
    let edges = [(0, 1, 2400), (0, 2, 1700), (2, 1, 1700)]
        .into_iter()
        .map(|(a, b, length_m)| Edge {
            a,
            b,
            polyline: vec![coords[a as usize], coords[b as usize]],
            length_m,
            kind: 0,
        })
        .collect();
    let bbox = (490_000, 490_000, 530_000, 520_000);
    let lods =
        [obc_pack::LodLayer { max_mpp: None, chunk_size: 2048, root: obc_pack::Node::Leaf { bbox, features: vec![] } }];
    let profiles =
        [obc_pack::NavProfile { name: "Neutral".into(), highway: [16; 32], surface: [16; 8], climb_weight: 0 }];
    obc_pack::serialize_lods(&lods, &[], 0, bbox, &[], &NavGraph { nodes, edges }, &profiles, elevation).0
}

struct Planner {
    host: HostLoop,
    app: App,
    routes: crate::FlatRouteStore,
    card: HostStore,
    map: FlatMap,
    tokens: TokenSource<NavigatorTag>,
}
impl Planner {
    fn new() -> Self {
        let card = HostStore::memory().unwrap();
        let map = FlatMap::from_bytes_in(&card, &map_bytes()).unwrap();
        let routes = crate::FlatRouteStore::new(HostStore(card.0.clone()), &[]).unwrap();
        Self {
            host: HostLoop::new(),
            app: App::new(obc_app::AppState::new(500_000, 500_000, 1.0)),
            routes,
            card,
            map,
            tokens: TokenSource::new(),
        }
    }
    fn call(&mut self, build: impl FnOnce(OperationToken<NavigatorTag>) -> NavigatorEffect) -> NavigatorOutcome {
        let token = self.tokens.issue();
        let answer = self
            .host
            .serve_navigator(&mut self.app, build(token), &mut self.routes, &self.map, &mut obc_route::NullElevation)
            .unwrap();
        assert_eq!(answer.token(), token);
        answer
    }
    fn acquire_route(&mut self) {
        assert!(matches!(
            self.call(|token| NavigatorEffect::Acquire {
                token,
                work: PlannerWork::Route(obc_app::NavRequest::new((500_000, 500_000), (520_000, 500_000), "Test"))
            }),
            NavigatorOutcome::Acquired { .. }
        ));
    }
    fn finish_steps(&mut self) -> NavigatorOutcome {
        for _ in 0..128 {
            let outcome = self.call(|token| NavigatorEffect::Step { token });
            if !matches!(outcome, NavigatorOutcome::Stepped { progress: PlannerProgress::Searching, .. }) {
                return outcome;
            }
        }
        panic!("tiny graph exceeded bounded step count")
    }
    fn release(&mut self, family: PlanFamily, retain_result: bool) {
        assert!(matches!(
            self.call(|token| NavigatorEffect::Release { token, family, retain_result }),
            NavigatorOutcome::Released { .. }
        ));
    }
    fn initial_route(&mut self) -> u64 {
        self.acquire_route();
        assert!(matches!(self.finish_steps(), NavigatorOutcome::Stepped { progress: PlannerProgress::Reached, .. }));
        let NavigatorOutcome::PlanFinished { route, .. } = self.call(|token| NavigatorEffect::CommitRoute { token })
        else {
            panic!("commit failed")
        };
        self.release(PlanFamily::Route, true);
        self.routes.sync_active(Some(0));
        route
    }
    fn acquire_detour(&mut self) {
        let source = self.routes.pin_active().unwrap();
        let index = obc_route::RouteIndex::read(&source).unwrap();
        assert!(matches!(
            self.call(|token| NavigatorEffect::Acquire {
                token,
                work: PlannerWork::Detour(obc_app::DetourRequest {
                    route: 0,
                    from: (500_000, 500_000),
                    progress_m: 0,
                    target_m: index.total_distance_m,
                    leg: obc_route::Leg::Detour,
                })
            }),
            NavigatorOutcome::Acquired { .. }
        ));
    }
}

#[test]
fn real_planner_waits_for_steps_and_explicit_commit_then_releases() {
    let mut p = Planner::new();
    p.acquire_route();
    for _ in 0..3 {
        let mut plan = PassPlan {
            render: obc_app::Dirty::CLEAN,
            next_wake_ms: None,
            derived_needs: obc_app::device_core::derived::DerivedNeeds::NONE,
            sources: obc_app::device_core::pass::SourceNeeds { map: false, route: false },
            effects: obc_app::device_core::EffectSlots::new(),
            immediate: false,
        };
        p.host.execute(
            &mut p.app,
            &mut plan,
            &mut ActiveRouteSession::new(),
            &mut p.routes,
            &mut crate::MemRideStore::new(vec![]),
            &mut crate::MemTrackStore::new(),
            &mut (),
            &p.map,
            &mut obc_route::NullElevation,
            &mut (),
        );
    }
    assert!(matches!(p.host.plan, Some(InflightPlan::Nav(_))));
    assert!(p.routes.ids().is_empty());
    assert!(matches!(p.finish_steps(), NavigatorOutcome::Stepped { progress: PlannerProgress::Reached, .. }));
    assert!(p.routes.ids().is_empty(), "reached is not committed");
    assert!(matches!(p.call(|token| NavigatorEffect::CommitRoute { token }), NavigatorOutcome::PlanFinished { .. }));
    assert!(p.host.plan.is_some(), "workspace retained until release");
    p.release(PlanFamily::Route, true);
    assert!(p.host.plan.is_none() && p.host.sources.is_none());
    assert_eq!(p.routes.ids().len(), 1);
}

#[test]
fn unrelated_commit_preserves_map_lease_but_exact_head_replacement_refuses_commit() {
    let mut p = Planner::new();
    p.acquire_route();
    p.card.import(ObjectKind::MapShard, None, &mut &b"unrelated"[..], 9, DisplayName::default()).unwrap();
    assert!(matches!(p.finish_steps(), NavigatorOutcome::Stepped { progress: PlannerProgress::Reached, .. }));
    let held = p.map.source();
    let bytes = map_bytes();
    p.card
        .import(
            ObjectKind::MapShard,
            Some((held.id(), held.revision())),
            &mut &bytes[..],
            bytes.len() as u64,
            DisplayName::default(),
        )
        .unwrap();
    assert!(!held.is_current());
    assert!(matches!(
        p.call(|token| NavigatorEffect::CommitRoute { token }),
        NavigatorOutcome::Failed { error: NavigatorError::SourceChanged, .. }
    ));
    assert!(p.routes.ids().is_empty());
    p.release(PlanFamily::Route, false);
    assert!(matches!(
        p.call(|token| NavigatorEffect::Acquire {
            token,
            work: PlannerWork::Route(obc_app::NavRequest::new((0, 0), (1, 1), "Stale"))
        }),
        NavigatorOutcome::Failed { error: NavigatorError::SourceChanged, .. }
    ));
}

#[test]
fn real_detour_keeps_preview_and_source_on_store_refusal_then_retries() {
    let mut p = Planner::new();
    p.initial_route();
    p.acquire_detour();
    assert!(matches!(p.finish_steps(), NavigatorOutcome::DetourFinished { .. }));
    p.release(PlanFamily::Detour, true);
    let reservations = {
        let owner = p.card.0.lock().unwrap();
        [owner.card.allocate(4096).unwrap(), owner.card.allocate(4096).unwrap()]
    };
    assert!(matches!(
        p.call(|token| NavigatorEffect::CommitDetour { token }),
        NavigatorOutcome::Failed { error: NavigatorError::Store, .. }
    ));
    p.release(PlanFamily::Detour, true);
    assert!(p.host.detour_ready.is_some());
    for allocation in reservations {
        p.card.0.lock().unwrap().card.cancel(allocation);
    }
    assert!(matches!(
        p.call(|token| NavigatorEffect::CommitDetour { token }),
        NavigatorOutcome::DetourCommitted { .. }
    ));
    p.release(PlanFamily::Detour, true);
    assert!(p.host.detour_ready.is_none());
}

#[test]
fn retained_original_route_cannot_authorize_a_detour_after_replacement() {
    let mut p = Planner::new();
    p.initial_route();
    p.acquire_detour();
    assert!(matches!(p.finish_steps(), NavigatorOutcome::DetourFinished { .. }));
    p.release(PlanFamily::Detour, true);
    let held = p.routes.pin_active().unwrap();
    let mut bytes = vec![0; obc_formats::io::ByteSource::len(&held) as usize];
    obc_formats::io::ByteSource::read_at(&held, 0, &mut bytes).unwrap();
    p.card
        .import(
            ObjectKind::Route,
            Some((ObjectId(held.id()), Revision(1))),
            &mut &bytes[..],
            bytes.len() as u64,
            DisplayName::default(),
        )
        .unwrap();
    assert!(matches!(
        p.call(|token| NavigatorEffect::CommitDetour { token }),
        NavigatorOutcome::Failed { error: NavigatorError::SourceChanged, .. }
    ));
    p.release(PlanFamily::Detour, false);
    assert!(p.host.detour_ready.is_none());
}

#[test]
fn cancellation_after_publication_removes_only_the_unadopted_revision() {
    for replace in [false, true] {
        let mut p = Planner::new();
        p.acquire_route();
        assert!(matches!(p.finish_steps(), NavigatorOutcome::Stepped { progress: PlannerProgress::Reached, .. }));
        let NavigatorOutcome::PlanFinished { route, .. } = p.call(|token| NavigatorEffect::CommitRoute { token })
        else {
            panic!("commit failed")
        };
        if replace {
            let Some(InflightPlan::Ready(plan, _)) = p.host.plan.as_ref() else { panic!("output held") };
            let bytes = plan.bytes();
            p.card
                .import(
                    ObjectKind::Route,
                    Some((ObjectId(route), Revision(1))),
                    &mut &bytes[..],
                    bytes.len() as u64,
                    DisplayName::default(),
                )
                .unwrap();
        }
        p.release(PlanFamily::Route, false);
        assert_eq!(p.routes.ids().contains(&route), replace, "a newer revision is outside the abandoned operation");
        assert!(p.host.publication.is_none() && p.host.plan.is_none());
    }
}

#[test]
fn cancelled_self_detour_preserves_the_original_computed_route() {
    let mut p = Planner::new();
    let original = p.initial_route();
    let held = p.routes.pin_active().unwrap();
    p.acquire_detour();
    assert!(matches!(p.finish_steps(), NavigatorOutcome::DetourFinished { .. }));
    p.release(PlanFamily::Detour, true);
    let NavigatorOutcome::DetourCommitted { route, .. } = p.call(|token| NavigatorEffect::CommitDetour { token })
    else {
        panic!("detour commit failed")
    };
    assert_ne!(route, original, "publication must not replace the route being spliced");
    p.release(PlanFamily::Detour, false);
    assert_eq!(p.routes.ids(), &[original]);
    p.routes.sync_active(Some(0));
    assert!(held.matches(&p.routes.pin_active().unwrap()));
    assert!(obc_route::RouteIndex::read(&held).is_ok());
}

#[test]
fn visits_measure_complete_graph_paths_and_real_cancellation_connectors() {
    use obc_app::navigator::{ReviewContext, ReviewPurpose, REVIEW_FACTS_POLICY};
    use obc_formats::io::SliceSource;
    use obc_formats::obcm::{PoiApproach, PoiMetadata, SourceId};
    use obc_formats::obcr::RouteSourceKey;
    use obc_route::visit::{VisitCosts, VisitTarget};
    let mut p = Planner::new();
    let original_id = p.initial_route();
    let held = p.routes.pin_active().unwrap();
    let index = obc_route::RouteIndex::read(&held).unwrap();
    let original = obc_route::RouteReader::new(&index, &held);
    let source = p.map.source();
    let key = RouteSourceKey { store: source.store_id().0, object: source.id().0, revision: source.revision().0 };
    let context = ReviewContext {
        purpose: ReviewPurpose::Visit,
        map: key,
        store: p.routes.store_scope().unwrap().store,
        original: p.routes.fingerprint(original_id),
        origin: (500_000, 500_000),
        progress_m: 0,
        occurrence: 0,
        required_anchors_m: [0; 3],
        profile: obc_route::BikeType::Road,
        facts_policy: REVIEW_FACTS_POLICY,
        unresolved_avoidance: false,
    };
    let target = VisitTarget {
        map: key,
        display: (510_100, 510_100),
        metadata: PoiMetadata {
            source: SourceId::osm(1, 99),
            approach: Some(PoiApproach { source: SourceId::osm(1, 100), lon: 510_000, lat: 510_000, profile_mask: 1 }),
        },
    };
    let run = |plan: &mut crate::nav_visit::VisitPlan, route: &obc_route::RouteReader| {
        for _ in 0..512 {
            match plan.step(&p.map.reader(), route, &mut obc_route::NullElevation) {
                Ok(Some(stats)) => return stats,
                Ok(None) => (),
                Err(error) => panic!("real graph visit failed: {error:?}"),
            }
        }
        panic!("bounded visit did not complete")
    };
    // A nearby imported anchor keeps its measured connection and the original continuation.
    let mut offset = crate::VecSink::default();
    obc_route::gpx_to_obcr(&SliceSource(b"<gpx><trk><trkseg><trkpt lon=\"0.500\" lat=\"0.500\"/><trkpt lon=\"0.520\" lat=\"0.5001\"/><trkpt lon=\"0.530\" lat=\"0.5001\"/></trkseg></trk></gpx>"),"Offset tail",&mut offset).unwrap();
    let offset_source = SliceSource(offset.bytes());
    let offset_index = obc_route::RouteIndex::read(&offset_source).unwrap();
    let offset_route = obc_route::RouteReader::new(&offset_index, &offset_source);
    let mut fallback = crate::nav_visit::VisitPlan::start(context, Some(target), &offset_route).unwrap();
    let offset_stats = run(&mut fallback, &offset_route);
    assert_eq!(fallback.searches(), 2);
    assert!(offset_stats.total_distance_m > offset_route.total_distance_m);
    let offset_visit = SliceSource(fallback.bytes());
    let offset_index = obc_route::RouteIndex::read(&offset_visit).unwrap();
    let offset_reader = obc_route::RouteReader::new(&offset_index, &offset_visit);
    assert_eq!(offset_reader.preview_polyline::<64>().last(), Some(&(530_000, 500_100)));
    let stop = offset_reader.visit_descriptor().unwrap().unwrap().accepted_anchors_m[1];
    assert!(!VisitCosts::read(&offset_visit, [0, stop]).unwrap().complete_elevation);
    let mut visit = crate::nav_visit::VisitPlan::start(context, Some(target), &original).unwrap();
    let stats = run(&mut visit, &original);
    assert_eq!(visit.searches(), 2);
    assert_eq!(visit.original_anchors()[1], visit.original_anchors()[2]);
    let source = SliceSource(visit.bytes());
    let index = obc_route::RouteIndex::read(&source).unwrap();
    let route = obc_route::RouteReader::new(&index, &source);
    let descriptor = route.visit_descriptor().unwrap().unwrap();
    let costs = VisitCosts::read(&source, [0, descriptor.accepted_anchors_m[1]]).unwrap();
    assert!(!costs.complete_elevation && !costs.arrival_elevation_complete);
    assert_eq!(route.total_distance_m, stats.total_distance_m);
    assert!(stats.total_distance_m > original.total_distance_m);
    let here = VisitTarget {
        display: context.origin,
        metadata: PoiMetadata {
            approach: Some(PoiApproach {
                source: SourceId::osm(1, 100),
                lon: context.origin.0,
                lat: context.origin.1,
                profile_mask: 1,
            }),
            ..target.metadata
        },
        ..target
    };
    let mut no_travel = crate::nav_visit::VisitPlan::start(context, Some(here), &original).unwrap();
    let here_stats = run(&mut no_travel, &original);
    assert_eq!(no_travel.searches(), 2);
    let here_info = obc_route::RouteObjectInfo::read(&SliceSource(no_travel.bytes())).unwrap();
    assert_eq!(here_info.visit.unwrap().accepted_anchors_m, [0; 3]);
    assert_eq!(here_stats.total_distance_m, original.total_distance_m);
    let rejoin = descriptor.accepted_anchors_m[2];
    let returning = ReviewContext {
        purpose: ReviewPurpose::ReturnToRoute,
        origin: (510_000, 510_000),
        progress_m: descriptor.accepted_anchors_m[1],
        required_anchors_m: [rejoin; 3],
        ..context
    };
    let mut connector = crate::nav_visit::VisitPlan::start(returning, None, &route).unwrap();
    let stats = run(&mut connector, &route);
    assert_eq!(connector.searches(), 1);
    let source = SliceSource(connector.bytes());
    let info = obc_route::RouteObjectInfo::read(&source).unwrap();
    assert!(info.visit.is_none());
    assert!(stats.total_distance_m < route.total_distance_m);
}

#[test]
fn easier_production_batch_is_bounded_deduplicates_and_accepts_only_on_explicit_review() {
    use obc_app::device_core::*;
    use obc_app::navigator::ReviewStatus;
    struct Flat;
    impl obc_route::ElevationSource for Flat {
        fn sample(&mut self, _: i32, _: i32) -> Option<i16> {
            Some(10)
        }
    }
    struct Fix;
    impl obc_ports::LocationSource for Fix {
        fn poll(&mut self) -> Option<obc_ports::Fix> {
            Some(obc_ports::Fix::at(500_000, 500_000))
        }
    }
    let mut p = Planner::new();
    p.map = FlatMap::from_bytes_in(&p.card, &map_bytes_with(&mut Flat)).unwrap();
    let mut original = crate::VecSink::default();
    obc_route::gpx_to_obcr(&obc_formats::io::SliceSource(b"<gpx><trk><trkseg><trkpt lon=\"0.5\" lat=\"0.5\"><ele>10</ele></trkpt><trkpt lon=\"0.51\" lat=\"0.51\"><ele>10</ele></trkpt><trkpt lon=\"0.52\" lat=\"0.5\"><ele>10</ele></trkpt></trkseg></trk></gpx>"),"Original",&mut original).unwrap();
    let original_id = p.routes.import(original.bytes()).unwrap();
    feed_routes(&mut p.app, &p.routes, &mut NoTrace);
    p.app.activate_route(0);
    p.routes.sync_active(Some(0));
    let held = p.routes.pin_active().unwrap();
    let index = obc_route::RouteIndex::read(&held).unwrap();
    let original = obc_route::RouteReader::new(&index, &held);
    let source = p.map.source();
    let key = obc_formats::obcr::RouteSourceKey {
        store: source.store_id().0,
        object: source.id().0,
        revision: source.revision().0,
    };
    let mut session = ActiveRouteSession::new();
    let mut rides = crate::MemRideStore::new(vec![]);
    let mut tracks = crate::MemTrackStore::new();
    let support = PlatformSupport {
        detour: true,
        settings_persistence: false,
        dfu: false,
        bonding: false,
        storage_space_report: false,
    };
    let mut acquisitions = 0;
    let mut opened = false;
    let mut reviewed = false;
    let mut pressed = false;
    for now in 1..10_000 {
        let mut plan = p.host.pass(
            &mut p.app,
            PassClock { ride: obc_ports::RideClock(now), ui: obc_ports::InputClock(now) },
            &[],
            obc_ports::Sensors::new(&mut Fix),
            Some(&original),
            support,
        );
        if let Some(effect) = plan.effects.navigator.take() {
            if matches!(effect, NavigatorEffect::Acquire { work: PlannerWork::AssistantRoute(_), .. }) {
                acquisitions += 1;
            }
            plan.effects.navigator.try_put(effect).unwrap();
        }
        p.host.execute(
            &mut p.app,
            &mut plan,
            &mut session,
            &mut p.routes,
            &mut rides,
            &mut tracks,
            &mut (),
            &p.map,
            &mut Flat,
            &mut (),
        );
        if !opened && now > 2 {
            p.app.open_easier_routes(key).unwrap();
            opened = true;
        }
        if acquisitions == 8 && p.app.assistant_review_status() == ReviewStatus::Preview {
            assert_eq!(p.app.route_ids()[p.app.active_route_index().unwrap()], original_id);
            assert!(!p.app.assistant_preview_shape().is_empty());
            if !reviewed {
                p.app.apply_gesture(obc_app::Gesture::Press);
                p.app.apply_gesture(obc_app::Gesture::Back); // Back retains the frozen selected candidate.
                p.app.apply_gesture(obc_app::Gesture::Press);
                reviewed = true;
            } else if !pressed {
                p.app.apply_gesture(obc_app::Gesture::Press);
                pressed = true;
            }
        }
        if pressed && p.app.assistant_review_status() == ReviewStatus::Accepted {
            let checkpoint = p.routes.read_checkpoint().unwrap().unwrap();
            assert!(!checkpoint.unresolved_avoidance);
            assert_ne!(checkpoint.route.object, original_id);
            assert!(checkpoint.route.length > 0);
            assert_eq!(acquisitions, 8); // Seven complete probes and one exact selected reconstruction.
            let source = p.routes.source(checkpoint.route.object).unwrap();
            let accepted = obc_route::RouteIndex::read(&source).unwrap();
            assert!(accepted.total_distance_m + 500 <= original.total_distance_m);
            let route = obc_route::RouteReader::new(&accepted, &source);
            for step in 1..32 {
                let mut plan = p.host.pass(
                    &mut p.app,
                    PassClock { ride: obc_ports::RideClock(now + step), ui: obc_ports::InputClock(now + step) },
                    &[],
                    obc_ports::Sensors::new(&mut Fix),
                    Some(&route),
                    support,
                );
                p.host.execute(
                    &mut p.app,
                    &mut plan,
                    &mut session,
                    &mut p.routes,
                    &mut rides,
                    &mut tracks,
                    &mut (),
                    &p.map,
                    &mut Flat,
                    &mut (),
                );
                if p.app.assistant_planner_released() {
                    break;
                }
            }
            assert_eq!(p.app.open_easier_routes(key), Ok(()), "accepted route permits a later refresh after release");
            p.app.apply_gesture(obc_app::Gesture::BackHold);
            assert!(p.app.assistant_review_status() != ReviewStatus::Planning);
            return;
        }
    }
    panic!("batch did not finish: acquisitions={acquisitions}, status={:?}", p.app.assistant_review_status());
}
