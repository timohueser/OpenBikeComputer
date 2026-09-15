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
}
impl VisitHarness {
    fn new() -> Self {
        let mut h = Harness::new();
        h.app.activate_route(0);
        h.app.set_nav_profiles(obc_reader::Reader::new(h.map.as_ref().unwrap(), &h.tables, &h.cache).nav_profiles());
        h.app.set_map_nav_graph(true);
        let mut this = Self {
            h,
            visit: visit::Executor::new(),
            outcomes: OutcomeSlots::default(),
            facts: ExternalFacts::NONE,
            now: 0,
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
            profile: 0,
            facts_policy: REVIEW_FACTS_POLICY,
            unresolved_avoidance: false,
        };
        let target = obc_route::visit::VisitTarget {
            map,
            display: (500_000, 510_000),
            metadata: PoiMetadata {
                source: SourceId::osm(1, 3),
                approach: Some(PoiApproach {
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
        self.now += 1;
        self.visit.accepted(&self.h.app, self.h.store);
        self.facts.note_store_revision(StoreRevision {
            store: StoreIdentity::from_bytes(self.h.store.store_id().0),
            revision: obc_app::device_core::Revision::new(self.h.store.sequence()),
        });
        let mut plan = self.h.app.run_pass(PassInputs {
            now: PassClock { ride: obc_ports::RideClock(self.now), ui: obc_ports::InputClock(self.now) },
            gestures: &[],
            sensors: obc_ports::Sensors::new(&mut NoFix),
            route: None,
            weather: None,
            support: PlatformSupport {
                detour: true,
                settings_persistence: false,
                dfu: false,
                weather: false,
                bonding: false,
                storage_space_report: false,
                retention_metadata: true,
            },
            outcomes: &mut self.outcomes,
            facts: &mut self.facts,
            derived: DerivedInputs::NONE,
            targets: DerivedTargets::NONE,
        });
        if let Some(effect) = plan.effects.navigator.take() {
            assert!(self.visit.accepts(&effect, &self.h.app), "{effect:?}");
            if let Some(answer) = self.visit.accept(effect, &mut self.h.app, self.h.store, &mut self.h.guard) {
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
            self.pass();
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
    let mut h = VisitHarness::new();
    h.settle(ReviewStatus::Preview);
    let preview = h.h.app.assistant_preview().unwrap();
    assert_eq!(h.h.app.active_route_index(), Some(0));
    assert_ne!(preview.source.object, 1);
    let descriptor =
        h.h.store
            .with_source(ObjectId(preview.source.object), None, |s| {
                obc_route::RouteObjectInfo::read(s).unwrap().visit.unwrap()
            })
            .unwrap();
    assert_eq!(descriptor.original.object, 1);
    assert!(descriptor.accepted_anchors_m[1] > 1000);
    assert!(h.h.writer.transport().completed.borrow().iter().filter(|&&k| k == Kind::Seal).count() <= 6);
    h.h.app.cancel_assistant();
    h.settle(ReviewStatus::Idle);
    h.h.assert_clean();
    assert_eq!(h.h.store.entries().count(), 2);
}
#[test]
fn visit_cancellation_drains_owned_tickets_before_releasing_arena() {
    for kind in [Kind::Allocate, Kind::Write, Kind::Seal, Kind::ReleaseSealed, Kind::Publish] {
        let mut h = VisitHarness::new();
        h.until_ticket(kind);
        h.h.app.cancel_assistant();
        for _ in 0..3 {
            h.pass();
            assert!(h.h.guard.is_some());
            assert_eq!(h.h.writer.pending(), Some(kind));
        }
        h.settle(ReviewStatus::Idle);
        h.h.assert_clean();
        assert_eq!(h.h.store.entries().count(), 2);
    }
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
        profile: 0,
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
