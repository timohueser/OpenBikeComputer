use super::*;
use obc_app::device_core::*;
use obc_app::navigator::{ReviewContext, ReviewPurpose, ReviewStatus, REVIEW_FACTS_POLICY};
use obc_formats::obcm::{PoiApproach, PoiMetadata, SourceId};
struct NoFix;
impl obc_ports::LocationSource for NoFix {
    fn poll(&mut self) -> Option<obc_ports::Fix> {
        None
    }
}
struct VisitHarness {
    h: Harness,
    visit: visit::Executor,
    outcomes: OutcomeSlots,
    facts: ExternalFacts,
    now: u32,
    catalog: Option<obc_app::catalog_state::CatalogEffect>,
}
impl VisitHarness {
    fn new() -> Self {
        Self::with_route(&route())
    }
    fn with_route(route: &[u8]) -> Self {
        Self::with_access(route, true)
    }
    fn with_access(route: &[u8], mapped: bool) -> Self {
        Self::with_harness(Harness::with_route(route), mapped)
    }
    fn with_harness(mut h: Harness, mapped: bool) -> Self {
        h.app.activate_route(0);
        h.app.set_map_nav_graph(true);
        let mut this = Self {
            h,
            visit: visit::Executor::new(),
            outcomes: OutcomeSlots::default(),
            facts: ExternalFacts::NONE,
            now: 0,
            catalog: None,
        };
        this.pass();
        let map = flat_store::planner_map_key(this.h.store);
        let context = ReviewContext {
            purpose: ReviewPurpose::Visit,
            map,
            store: StoreIdentity::from_bytes(map.store),
            original: flat_store::route_fingerprint(this.h.store, 1),
            origin: (500_000, 500_000),
            progress_m: 0,
            occurrence: 0,
            required_anchors_m: [0; 3],
            profile: obc_route::BikeType::Road,
            facts_policy: REVIEW_FACTS_POLICY,
            unresolved_avoidance: false,
        };
        let target = obc_route::visit::VisitTarget {
            map,
            display: (if mapped { 500_000 } else { 499_500 }, 510_000),
            metadata: PoiMetadata {
                source: SourceId::osm(1, 3),
                approach: mapped.then_some(PoiApproach {
                    source: SourceId::osm(1, 3),
                    lon: 500_000,
                    lat: 510_000,
                    profile_mask: 1,
                }),
            },
        };
        assert!(this.h.app.plan_visit(target, context));
        this
    }
    fn pass(&mut self) {
        self.pass_with(&mut NoFix, false);
    }
    fn pass_with(&mut self, position: &mut dyn obc_ports::LocationSource, catalogs: bool) {
        self.now += 1;
        self.visit.accepted(&self.h.app, self.h.store);
        if catalogs {
            use obc_app::catalog_state::{CatalogEffect, CatalogOutcome};
            if let Some(effect) = self.catalog.take() {
                let outcome = match effect {
                    CatalogEffect::ReadCatalog { token } => {
                        flat_store::load_routes(self.h.store, &mut self.h.app);
                        CatalogOutcome::CatalogRead {
                            token,
                            scope: Some(StoreRevision {
                                store: StoreIdentity::from_bytes(self.h.store.store_id().0),
                                revision: obc_app::device_core::Revision::new(self.h.store.sequence()),
                            }),
                        }
                    }
                    CatalogEffect::RemoveReview { token, source } => {
                        self.h
                            .store
                            .commit(&[Mutation::Remove {
                                id: ObjectId(source.object),
                                revision: Revision(source.revision),
                            }])
                            .unwrap();
                        CatalogOutcome::ReviewRemoved { token, source }
                    }
                    _ => panic!("unexpected Find catalog effect"),
                };
                self.outcomes.catalog.try_put(outcome).unwrap();
            }
        }
        self.facts.note_store_revision(StoreRevision {
            store: StoreIdentity::from_bytes(self.h.store.store_id().0),
            revision: obc_app::device_core::Revision::new(self.h.store.sequence()),
        });
        let index = catalogs.then(|| obc_route::RouteIndex::read(self.h.original.as_ref().unwrap()).unwrap());
        let route = index.as_ref().map(|index| obc_route::RouteReader::new(index, self.h.original.as_ref().unwrap()));
        let mut plan = self.h.app.run_pass(PassInputs {
            now: PassClock { ride: obc_ports::RideClock(self.now), ui: obc_ports::InputClock(self.now) },
            gestures: &[],
            sensors: obc_ports::Sensors::new(position),
            route: route.as_ref(),
            support: PlatformSupport {
                detour: true,
                settings_persistence: false,
                dfu: false,
                bonding: false,
                storage_space_report: false,
            },
            outcomes: &mut self.outcomes,
            facts: &mut self.facts,
            derived: DerivedInputs::NONE,
            targets: DerivedTargets::NONE,
        });
        if let Some(effect) = plan.effects.metadata.take() {
            assert!(self.h.app.assistant_checkpoint_submission(effect.token()));
            self.outcomes
                .metadata
                .try_put(obc_app::metadata::MetadataOutcome::CheckpointWritten { token: effect.token() })
                .unwrap();
        }
        if let Some(effect) = plan.effects.catalog.take() {
            assert!(self.catalog.replace(effect).is_none());
        }
        if let Some(effect) = plan.effects.navigator.take() {
            if matches!(effect, Effect::Acquire { work: PlannerWork::AssistantRoute(_), .. }) {
                let id = self.h.app.active_route_index().map(|i| self.h.app.route_ids()[i]);
                assert!(self.h.app.bind_visit_sources(
                    StoreRevision {
                        store: StoreIdentity::from_bytes(self.h.store.store_id().0),
                        revision: obc_app::device_core::Revision::new(self.h.store.sequence()),
                    },
                    id.and_then(|id| flat_store::route_fingerprint(self.h.store, id)),
                    false,
                ));
            }
            assert!(self.visit.accepts(&effect, &self.h.app), "{effect:?}");
            if let Some(answer) = self.visit.accept(
                effect,
                &mut self.h.app,
                self.h.store,
                &mut self.h.guard,
                &mut obc_elevation::NullElevation,
            ) {
                self.outcomes.navigator.try_put(answer).unwrap();
            }
        }
        if let Some(answer) = self.visit.poll(
            &mut self.h.app,
            self.h.store,
            self.h.writer,
            &mut self.h.guard,
            self.h.map.as_ref().unwrap(),
            &self.h.tables,
            &self.h.cache,
            &mut obc_elevation::NullElevation,
            self.h.reply,
        ) {
            self.outcomes.navigator.try_put(answer).unwrap();
        }
    }
    fn until_ticket(&mut self, kind: Kind) {
        for _ in 0..10000 {
            if self.h.writer.pending() == Some(kind) {
                return;
            }
            if self.h.writer.pending().is_some() {
                self.h.writer.complete();
            }
            self.pass();
        }
        panic!("no {kind:?}: {:?}", self.h.app.assistant_review_status());
    }
    fn settle(&mut self, wanted: ReviewStatus) {
        for _ in 0..20000 {
            if self.h.writer.pending().is_some() {
                self.h.writer.complete();
            }
            self.pass_with(&mut NoFix, wanted == ReviewStatus::Accepted);
            if self.h.app.assistant_review_status() == wanted
                && self.h.guard.is_none()
                && self.h.writer.pending().is_none()
                && self.outcomes.navigator.is_empty()
                && (matches!(wanted, ReviewStatus::Preview | ReviewStatus::Unresolved) || !self.visit.active())
            {
                return;
            }
        }
        panic!("did not settle: {:?}", self.h.app.assistant_review_status());
    }
}
#[test]
fn visit_uses_real_legs_and_cancel_retracts_only_candidate() {
    for mapped in [true, false] {
        let mut h = VisitHarness::with_access(&route(), mapped);
        h.settle(ReviewStatus::Preview);
        let preview = h.h.app.assistant_preview().unwrap();
        assert!(!h.h.app.assistant_preview_shape().is_empty());
        assert_eq!(h.h.app.active_route_index(), Some(0));
        assert_ne!(preview.source.object, 1);
        let descriptor =
            h.h.store
                .with_source(ObjectId(preview.source.object), None, |s| {
                    obc_route::RouteObjectInfo::read(s).unwrap().visit.unwrap()
                })
                .unwrap();
        assert_eq!(descriptor.original.object, 1);
        assert_eq!((descriptor.target_lon, descriptor.target_lat), (500_000, 510_000));
        assert!(descriptor.accepted_anchors_m[1] > 1000);
        assert!(h.h.writer.transport().completed.borrow().iter().filter(|&&k| k == Kind::Seal).count() <= 6);
        h.h.app.cancel_assistant();
        h.settle(ReviewStatus::Idle);
        h.h.assert_clean();
        assert_eq!(h.h.store.entries().count(), 2);
    }
}

#[test]
fn stored_visit_restores_without_planning_and_rejects_changed_identity() {
    struct Fixed;
    impl obc_ports::LocationSource for Fixed {
        fn poll(&mut self) -> Option<obc_ports::Fix> {
            Some(obc_ports::Fix::at(500_000, 500_000))
        }
    }
    for wrong_revision in [false, true] {
        let mut h = VisitHarness::new();
        h.settle(ReviewStatus::Preview);
        let preview = h.h.app.assistant_preview().unwrap();
        let context = h.h.app.assistant_review_context().unwrap();
        let target = h.h.app.assistant_visit_target().unwrap();
        let bytes =
            h.h.store
                .with_source(ObjectId(preview.source.object), None, |source| {
                    let mut bytes = vec![0; source.len() as usize];
                    source.read_at(0, &mut bytes).unwrap();
                    bytes
                })
                .unwrap();
        h.h.app.cancel_assistant();
        h.settle(ReviewStatus::Idle);
        let id = put(h.h.store, ObjectKind::Route, &bytes, None);
        flat_store::load_routes(h.h.store, &mut h.h.app);
        let original = h.h.original.as_ref().unwrap();
        let index = obc_route::RouteIndex::read(original).unwrap();
        let route = obc_route::RouteReader::new(&index, original);
        let plan = h.h.app.run_pass(PassInputs {
            now: PassClock { ride: obc_ports::RideClock(h.now), ui: obc_ports::InputClock(h.now) },
            gestures: &[],
            sensors: obc_ports::Sensors::new(&mut Fixed),
            route: Some(&route),
            support: PlatformSupport::default(),
            outcomes: &mut h.outcomes,
            facts: &mut h.facts,
            derived: DerivedInputs::NONE,
            targets: DerivedTargets::NONE,
        });
        assert!(plan.effects.navigator.is_empty());
        let source = obc_formats::obcr::RouteSourceKey {
            store: h.h.store.store_id().0,
            object: id.0,
            revision: if wrong_revision { 2 } else { 1 },
        };
        assert!(h.h.app.restore_visit(target, context, source));
        let writes = h.h.writer.transport().completed.borrow().len();
        h.settle(if wrong_revision {
            ReviewStatus::Failed(NavigatorError::SourceChanged)
        } else {
            ReviewStatus::Preview
        });
        assert_eq!(
            h.h.writer.transport().completed.borrow().len(),
            writes,
            "restore must not plan, allocate, or publish"
        );
        if wrong_revision {
            assert!(h.h.store.entries().any(|entry| entry.id == id), "a different revision is not ours to remove");
        } else {
            assert_eq!(h.h.app.assistant_preview().unwrap().source.object, id.0);
            h.h.app.cancel_assistant();
            h.settle(ReviewStatus::Idle);
            assert!(!h.h.store.entries().any(|entry| entry.id == id));
        }
    }
}
#[test]
fn visit_cancellation_drains_owned_tickets_before_releasing_arena() {
    for kind in [Kind::Allocate, Kind::Write, Kind::Seal, Kind::ReleaseSealed, Kind::Publish] {
        let mut h = VisitHarness::new();
        h.until_ticket(kind);
        assert!(!h.visit.immediate(h.h.reply, true));
        h.h.app.cancel_assistant();
        for _ in 0..3 {
            h.pass();
            assert!(h.h.guard.is_some());
            assert_eq!(h.h.writer.pending(), Some(kind));
            assert!(!h.visit.immediate(h.h.reply, true));
        }
        h.h.writer.complete();
        assert!(h.visit.immediate(h.h.reply, false));
        h.settle(ReviewStatus::Idle);
        h.h.assert_clean();
        assert_eq!(h.h.store.entries().count(), 2);
    }
}

#[test]
fn find_prepares_ranked_candidates_without_render_or_early_catalog_shape_binding() {
    struct Position;
    impl obc_ports::LocationSource for Position {
        fn poll(&mut self) -> Option<obc_ports::Fix> {
            Some(obc_ports::Fix::at(500_000, 500_000))
        }
    }
    for accepted in [false, true] {
        let mut h = VisitHarness::new();
        let mut context = h.h.app.assistant_review_context().unwrap();
        let target = h.h.app.assistant_visit_target().unwrap();
        h.h.app.cancel_assistant();
        h.settle(ReviewStatus::Idle);
        if accepted {
            context.purpose = ReviewPurpose::Easier(obc_route::nav::Objective::Profile);
            h.h.app.plan_assistant(
                obc_app::NavRequest::new(context.origin, (520_000, 500_000), "Accepted route"),
                context,
            );
            h.settle(ReviewStatus::Preview);
            let id = h.h.app.assistant_preview().unwrap().source.object;
            h.h.app.accept_assistant(obc_app::navigator::ReviewOrigin {
                fix: context.origin,
                progress_m: 0,
                occurrence: 0,
                lateral_m: 0,
                trustworthy: true,
            });
            h.settle(ReviewStatus::Accepted);
            h.h.store.close(h.h.original.take().unwrap().release());
            h.h.original = Some(h.h.store.source(ObjectId(id), Some(Revision(1))).unwrap());
            flat_store::mount_sources(h.h.store, h.h.original.as_ref().unwrap(), h.h.map.as_ref().unwrap());
            h.h.free = h.h.store.free_extents();
        }
        let checkpoint = h.h.app.assistant_checkpoint();
        h.h.app.bind_place_map(Some(flat_store::planner_map_key(h.h.store)));
        h.h.app.open_find_place();
        h.h.app.apply_gesture(obc_app::Gesture::Press);
        let mut unbound_preview = false;
        for _ in 0..2000 {
            if h.h.writer.pending().is_some() {
                h.h.writer.complete();
            }
            h.pass_with(&mut Position, true);
            assert!(
                !matches!(h.h.app.assistant_review_status(), ReviewStatus::Failed(_)),
                "review={:?}, context={:?}, origin={:?}",
                h.h.app.assistant_review_status(),
                h.h.app.assistant_review_context(),
                h.h.app.current_review_origin()
            );
            if let Some(preview) = h.h.app.assistant_preview() {
                if !h.h.app.route_ids().contains(&preview.source.object) {
                    unbound_preview = true;
                    assert!(h.h.app.assistant_preview_shape().is_empty());
                }
            }
            if h.h.guard.is_none() {
                let reader = obc_reader::Reader::new(h.h.map.as_ref().unwrap(), &h.h.tables, &h.h.cache);
                let source = h.h.original.as_ref().unwrap();
                let index = obc_route::RouteIndex::read(source).unwrap();
                let route = obc_route::RouteReader::new(&index, source);
                h.h.app.prepare_find(Some(&reader), Some(&route));
            }
            if h.h.app.find_place_state() == obc_app::find_place::State::Ready {
                break;
            }
        }
        assert!(
            unbound_preview,
            "ranking must finish before its catalog refresh: state={:?}, review={:?}, results={}, routes={:?}",
            h.h.app.find_place_state(),
            h.h.app.assistant_review_status(),
            h.h.app.find_place_result_count(),
            h.h.app.route_ids()
        );
        assert_eq!(h.h.app.find_place_state(), obc_app::find_place::State::Ready);
        assert_eq!(h.h.app.find_place_result_count(), 1);
        h.h.app.apply_gesture(obc_app::Gesture::Back);
        assert_eq!(h.h.app.find_place_state(), obc_app::find_place::State::Ready);
        for _ in 0..2 {
            if !matches!(h.h.app.top_screen(), obc_app::screen::Screen::FindPlace(_)) {
                break;
            }
            h.h.app.apply_gesture(obc_app::Gesture::Back);
        }
        assert_eq!(h.h.app.find_place_state(), obc_app::find_place::State::Idle);
        for _ in 0..20 {
            h.pass_with(&mut Position, true);
            h.h.app.prepare_find(None, None);
        }
        if accepted {
            h.h.app.request_visit(target, "Another preview").unwrap();
            h.settle(ReviewStatus::Preview);
            let candidate = h.h.app.assistant_preview().unwrap().source.object;
            h.h.app.cancel_assistant();
            h.settle(ReviewStatus::Accepted);
            assert!(!h.h.store.entries().any(|entry| entry.id.0 == candidate));
        }
        assert_eq!(h.h.app.assistant_checkpoint(), checkpoint);
        assert_eq!(
            h.h.app.assistant_review_status(),
            if accepted { ReviewStatus::Accepted } else { ReviewStatus::Idle }
        );
        assert_eq!(h.h.store.entries().count(), if accepted { 3 } else { 2 });
        h.h.assert_clean();
    }
}

#[test]
fn visit_reuses_validation_until_catalog_changes_without_spinning_on_a_full_writer() {
    let mut h = VisitHarness::new();
    h.h.writer.transport().full.set(true);
    h.pass();
    assert!(h.h.guard.is_some());
    let reads = flat_store::fingerprint_reads();
    for _ in 0..3 {
        h.pass();
        assert!(!h.visit.immediate(h.h.reply, true));
        assert_eq!(flat_store::fingerprint_reads(), reads);
    }
    let unrelated = put(h.h.store, ObjectKind::Route, &route(), None);
    h.h.free = h.h.store.free_extents();
    h.pass();
    assert_eq!(flat_store::fingerprint_reads(), reads + 1);
    h.pass();
    assert_eq!(flat_store::fingerprint_reads(), reads + 1);
    h.h.writer.transport().full.set(false);
    h.h.app.cancel_assistant();
    h.settle(ReviewStatus::Idle);
    assert_eq!(h.h.store.current_revision(unrelated), Ok(Some(Revision(1))));
    h.h.assert_clean();
}

#[test]
fn visit_rejects_changed_sources_and_keeps_uncertain_publication_fenced() {
    let mut h = VisitHarness::new();
    h.until_ticket(Kind::Write);
    let original = h.h.original.as_ref().unwrap();
    put(h.h.store, ObjectKind::Route, &route(), Some((original.id(), original.revision())));
    h.settle(ReviewStatus::Failed(NavigatorError::SourceChanged));
    assert_eq!(h.h.store.entries().count(), 2);
    assert_eq!(h.h.app.active_route_index(), Some(0));
    let mut h = VisitHarness::new();
    h.until_ticket(Kind::Publish);
    h.h.writer.transport().fail.set(Some(Kind::Publish));
    h.settle(ReviewStatus::Unresolved);
    assert!(h.visit.original_current());
    assert!(h.h.guard.is_none());
    assert!(!h.h.writer.transport().completed.borrow().contains(&Kind::Remove));
    assert_eq!(h.h.app.active_route_index(), Some(0));
    // The unresolved executor keeps its hold until remount; model the end of that mount.
    std::mem::forget(h.visit);
}

#[test]
fn visit_return_uses_one_real_connector_and_keeps_original_tail() {
    let mut h = VisitHarness::new();
    h.settle(ReviewStatus::Preview);
    let preview = h.h.app.assistant_preview().unwrap();
    assert!(!h.h.app.assistant_preview_shape().is_empty());
    let (bytes, rejoin) =
        h.h.store
            .with_source(ObjectId(preview.source.object), None, |source| {
                let mut bytes = vec![0; source.len() as usize];
                source.read_at(0, &mut bytes).unwrap();
                (bytes, obc_route::RouteObjectInfo::read(source).unwrap().visit.unwrap().accepted_anchors_m[2])
            })
            .unwrap();
    h.h.app.cancel_assistant();
    h.settle(ReviewStatus::Idle);
    let original = h.h.original.take().unwrap();
    let id = original.id();
    let revision = original.revision();
    h.h.store.close(original.release());
    put(h.h.store, ObjectKind::Route, &bytes, Some((id, revision)));
    h.h.original = Some(h.h.store.source(id, None).unwrap());
    flat_store::mount_sources(h.h.store, h.h.original.as_ref().unwrap(), h.h.map.as_ref().unwrap());
    flat_store::load_routes(h.h.store, &mut h.h.app);
    h.h.app.activate_route(0);
    h.pass();
    let map = flat_store::planner_map_key(h.h.store);
    let context = ReviewContext {
        purpose: ReviewPurpose::ReturnToRoute,
        map,
        store: StoreIdentity::from_bytes(map.store),
        original: flat_store::route_fingerprint(h.h.store, id.0),
        origin: (500_000, 510_000),
        progress_m: 500,
        occurrence: 0,
        required_anchors_m: [rejoin; 3],
        profile: obc_route::BikeType::Road,
        facts_policy: REVIEW_FACTS_POLICY,
        unresolved_avoidance: false,
    };
    h.h.app.plan_assistant(obc_app::NavRequest::new(context.origin, (500_000, 500_000), "Return to route"), context);
    let seals = h.h.writer.transport().completed.borrow().iter().filter(|&&k| k == Kind::Seal).count();
    h.settle(ReviewStatus::Preview);
    let result = h.h.app.assistant_preview().unwrap();
    assert!(h
        .h
        .store
        .with_source(ObjectId(result.source.object), None, |s| obc_route::RouteObjectInfo::read(s)
            .unwrap()
            .visit
            .is_none())
        .unwrap());
    assert_eq!(h.h.writer.transport().completed.borrow().iter().filter(|&&k| k == Kind::Seal).count() - seals, 1);
    assert_eq!(h.h.app.active_route_index(), Some(0));
    h.h.app.cancel_assistant();
    h.settle(ReviewStatus::Idle);
}

#[test]
fn near_place_anchor_retains_prefix_and_uses_only_two_legs() {
    let gpx=b"<gpx><trk><trkseg><trkpt lon=\"0.500\" lat=\"0.500\"/><trkpt lon=\"0.5001\" lat=\"0.510\"/><trkpt lon=\"0.530\" lat=\"0.510\"/></trkseg></trk></gpx>";
    let mut sink = obc_host_core::VecSink::default();
    obc_route::gpx_to_obcr(&SliceSource(gpx), "Original", &mut sink).unwrap();
    let mut h = VisitHarness::with_route(sink.bytes());
    h.settle(ReviewStatus::Preview);
    let preview = h.h.app.assistant_preview().unwrap();
    let info =
        h.h.store
            .with_source(ObjectId(preview.source.object), None, |s| obc_route::RouteObjectInfo::read(s).unwrap())
            .unwrap();
    let anchors = info.visit.unwrap().original_anchors_m;
    assert_eq!(anchors[0], 0);
    assert!((1110..=1115).contains(&anchors[1]));
    assert_eq!(anchors[1], anchors[2]);
    assert_eq!(h.h.writer.transport().completed.borrow().iter().filter(|&&k| k == Kind::Seal).count(), 2);
    h.h.app.cancel_assistant();
    h.settle(ReviewStatus::Idle);
    h.h.assert_clean();
}

#[test]
fn easier_uses_shared_board_owner_and_releases_each_leg_without_adding_avoidance() {
    for objective in obc_route::nav::Objective::TRIALS {
        let mut h = VisitHarness::new();
        let mut context = h.h.app.assistant_review_context().unwrap();
        h.h.app.cancel_assistant();
        context.purpose = ReviewPurpose::Easier(objective);
        h.h.app.plan_assistant(obc_app::NavRequest::new(context.origin, (520_000, 500_000), "Easier"), context);
        h.settle(ReviewStatus::Preview);
        let preview = h.h.app.assistant_preview().unwrap();
        let info =
            h.h.store
                .with_source(
                    ObjectId(preview.source.object),
                    Some(obc_storage::flat::Revision(preview.source.revision)),
                    |source| obc_route::RouteObjectInfo::read(source).unwrap(),
                )
                .unwrap();
        assert!(info.assistant_candidate && !info.unresolved_avoidance && info.visit.is_none());
        assert!(!preview.visit_costs.unwrap().complete_elevation);
        assert!(!h.h.app.assistant_preview_shape().is_empty());
        assert_eq!(h.h.app.active_route_index(), Some(0));
        h.h.app.cancel_assistant();
        h.settle(ReviewStatus::Idle);
        h.h.assert_clean();
    }
}

#[test]
fn easier_terminal_anchor_at_the_last_required_coordinate_finishes_without_another_search() {
    let gpx = br#"<gpx><wpt lon="0.530" lat="0.500"><name>Required access</name></wpt><trk><trkseg><trkpt lon="0.500" lat="0.500"/><trkpt lon="0.530" lat="0.500"/><trkpt lon="0.520" lat="0.500"/><trkpt lon="0.530" lat="0.500"/></trkseg></trk></gpx>"#;
    let mut sink = obc_host_core::VecSink::default();
    obc_route::gpx_to_obcr(&SliceSource(gpx), "Terminal return", &mut sink).unwrap();
    let mut h = VisitHarness::with_route(sink.bytes());
    let mut context = h.h.app.assistant_review_context().unwrap();
    h.h.app.cancel_assistant();
    context.purpose = ReviewPurpose::Easier(obc_route::nav::Objective::Profile);
    h.h.app.plan_assistant(obc_app::NavRequest::new(context.origin, (530_000, 500_000), "Easier"), context);
    h.settle(ReviewStatus::Preview);
    let preview = h.h.app.assistant_preview().unwrap();
    h.h.store
        .with_source(ObjectId(preview.source.object), None, |source| {
            let count = obc_route::reader::for_each_waypoint(source, |waypoint| {
                assert_eq!(waypoint.name.as_str(), "Required access");
                assert_eq!(waypoint.dist_along_m, preview.distance_m);
            })
            .unwrap();
            assert_eq!(count, 1);
        })
        .unwrap();
    assert_eq!(h.h.writer.transport().completed.borrow().iter().filter(|&&k| k == Kind::Seal).count(), 1);
    assert!(h.h.writer.transport().completed.borrow().contains(&Kind::Cancel));
    h.h.app.cancel_assistant();
    h.settle(ReviewStatus::Idle);
    h.h.assert_clean();
}

#[test]
fn closed_place_can_be_previewed_and_accepted_even_if_it_closes_during_review() {
    struct Position;
    impl obc_ports::LocationSource for Position {
        fn poll(&mut self) -> Option<obc_ports::Fix> {
            Some(obc_ports::Fix::at(500_000, 500_000))
        }
    }
    let mut schedule = obc_pack::hours::Schedule::default();
    for day in &mut schedule.days {
        day[0].close_q = 48;
    }
    let bytes = map_with_hours(Some(schedule));
    for closed_at_search in [false, true] {
        let mut h = VisitHarness::with_harness(Harness::with_map(&route(), &bytes), true);
        h.h.app.cancel_assistant();
        h.settle(ReviewStatus::Idle);
        h.h.app.set_settings(obc_app::Settings { find_hide_closed: !closed_at_search, ..*h.h.app.settings() });
        let clock = obc_ports::DateTime {
            year: 2026,
            month: 9,
            day: 16,
            hour: if closed_at_search { 13 } else { 11 },
            minute: 0,
        };
        h.h.app.stamp_clock_ble(clock.to_unix(), 0);
        h.h.app.bind_place_map(Some(flat_store::planner_map_key(h.h.store)));
        h.h.app.open_find_place();
        h.h.app.apply_gesture(obc_app::Gesture::Press);
        let prepare = |h: &mut VisitHarness| {
            if h.h.guard.is_none() {
                let reader = obc_reader::Reader::new(h.h.map.as_ref().unwrap(), &h.h.tables, &h.h.cache);
                let source = h.h.original.as_ref().unwrap();
                let index = obc_route::RouteIndex::read(source).unwrap();
                let route = obc_route::RouteReader::new(&index, source);
                h.h.app.prepare_find(Some(&reader), Some(&route));
            }
        };
        for _ in 0..2000 {
            if h.h.writer.pending().is_some() {
                h.h.writer.complete();
            }
            h.pass_with(&mut Position, true);
            prepare(&mut h);
            if h.h.app.find_place_state() == obc_app::find_place::State::Ready {
                break;
            }
        }
        assert_eq!(h.h.app.find_place_state(), obc_app::find_place::State::Ready);
        assert_eq!(h.h.app.find_place_result_count(), 1);
        h.h.app.apply_gesture(obc_app::Gesture::Press);
        prepare(&mut h);
        assert!(matches!(h.h.app.top_screen(), obc_app::screen::Screen::VisitReview(_)));
        h.settle(ReviewStatus::Preview);
        h.h.app.stamp_clock_ble(obc_ports::DateTime { hour: 13, ..clock }.to_unix(), 0);
        prepare(&mut h);
        assert_eq!(h.h.app.assistant_review_status(), ReviewStatus::Preview);
        h.h.app.apply_gesture(obc_app::Gesture::Press);
        h.settle(ReviewStatus::Accepted);
        assert!(h.h.app.assistant_checkpoint().is_some());
    }
}
