//! Actual board detour executor over real card bytes, with deterministic ticket delivery.
extern crate self as defmt;
#[macro_export]
macro_rules! info {
    ($format:literal $(, $arg:expr)* $(,)?) => {{ let _ = ($($arg,)*); }};
}
#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {{
        $crate::unexpected_compensation_refusal()
    }};
}
fn unexpected_compensation_refusal() {
    panic!("unexpected terminal compensation refusal");
}
#[path = "board_detour_support/arena.rs"]
mod arena;
#[path = "../../../firmware/obc-fw-nrf54l/src/assistant.rs"]
mod assistant;
#[path = "../../../firmware/obc-fw-nrf54l/src/detour.rs"]
mod detour;
#[path = "board_detour_support/flat_store.rs"]
mod flat_store;
#[path = "board_detour_support/ride.rs"]
mod ride;
#[path = "../../../firmware/obc-fw-nrf54l/src/visit.rs"]
mod visit;

use flat_store::{FlatCard, Kind, Writer};
use obc_app::device_core::{NavigatorTag, TokenSource};
use obc_app::navigator::{
    NavigatorEffect as Effect, NavigatorError, NavigatorOutcome as Outcome, PlanFamily, PlannerWork,
};
use obc_formats::io::{ByteSource, SliceSource};
use obc_storage::flat::*;

struct Harness {
    executor: detour::Executor,
    app: obc_app::App,
    guard: Option<arena::NavGuard>,
    store: &'static FlatStore<FlatCard>,
    writer: Writer,
    reply: &'static flat_store::Reply,
    tokens: TokenSource<NavigatorTag>,
    original: Option<StoreSource<'static, FlatCard>>,
    map: Option<StoreSource<'static, FlatCard>>,
    tables: obc_reader::MapTables,
    cache: obc_reader::MapCache,
    free: u32,
}
fn put(
    store: &FlatStore<FlatCard>,
    kind: ObjectKind,
    bytes: &[u8],
    previous: Option<(ObjectId, Revision)>,
) -> ObjectId {
    let mut allocation = store.allocate(bytes.len() as u64).unwrap();
    store.write(&mut allocation, bytes).unwrap();
    let (id, revision) =
        previous.map(|(id, revision)| (id, Revision(revision.0 + 1))).unwrap_or((store.next_object_id(), Revision(1)));
    let meta = EntryMeta {
        added_at_utc: 0,
        id,
        revision,
        kind,
        flags: EntryFlags::NONE,
        payload_len: bytes.len() as u64,
        payload_crc: store.allocation_crc(&allocation).unwrap(),
        name: DisplayName::default(),
    };
    let mut batch = Vec::new();
    if let Some((id, revision)) = previous {
        batch.push(Mutation::Remove { id, revision });
    }
    batch.push(Mutation::Put { meta, source: PutSource::Fresh(allocation) });
    store.commit(&batch).unwrap();
    id
}
fn route() -> Vec<u8> {
    let gpx = b"<gpx><trk><trkseg><trkpt lon=\"0.500\" lat=\"0.500\"/><trkpt lon=\"0.510\" lat=\"0.500\"/><trkpt lon=\"0.520\" lat=\"0.500\"/><trkpt lon=\"0.530\" lat=\"0.500\"/></trkseg></trk></gpx>";
    let mut sink = obc_host_core::VecSink::default();
    obc_route::gpx_to_obcr(&SliceSource(gpx), "Original", &mut sink).unwrap();
    sink.into_bytes()
}
fn map_bytes() -> Vec<u8> {
    map_with_hours(None)
}
fn map_with_hours(hours: Option<obc_pack::hours::Schedule>) -> Vec<u8> {
    use obc_pack::nav::{Edge, NavGraph, Node};
    let coords = [
        (500_000, 500_000),
        (500_000, 510_000),
        (530_000, 510_000),
        (530_000, 500_000),
        (525_000, 500_000),
        (520_000, 500_000),
    ];
    let nodes = coords.iter().enumerate().map(|(id, &coord)| Node { id: id as u32, coord }).collect();
    let edges = coords
        .windows(2)
        .enumerate()
        .map(|(i, p)| Edge { a: i as u32, b: i as u32 + 1, polyline: p.to_vec(), length_m: 1600, kind: 0 })
        .collect();
    let bbox = (0, 0, 1_000_000, 1_000_000);
    let lods =
        [obc_pack::LodLayer { max_mpp: None, chunk_size: 2048, root: obc_pack::Node::Leaf { bbox, features: vec![] } }];
    let profiles =
        [obc_pack::NavProfile { name: "Neutral".into(), highway: [16; 32], surface: [16; 8], climb_weight: 0 }];
    obc_pack::serialize_lods(
        &lods,
        &[],
        0,
        bbox,
        &[obc_pack::poi::Poi {
            metadata: obc_formats::obcm::PoiMetadata { source: obc_formats::obcm::SourceId::osm(1, 3), approach: None },
            access_nodes: vec![],
            wikidata: None,
            wikipedia: None,
            subtype: 1,
            lon_udeg: 500_000,
            lat_udeg: 510_000,
            name: Some("Water".into()),
            from_node: true,
            hours,
            elevation_m: None,
        }],
        &NavGraph { nodes, edges },
        &profiles,
        &mut obc_elevation::NullElevation,
    )
    .0
}
impl Harness {
    fn new() -> Self {
        Self::with_route(&route())
    }
    fn with_route(route: &[u8]) -> Self {
        Self::with_map(route, &map_bytes())
    }
    fn with_map(route: &[u8], map_bytes: &[u8]) -> Self {
        let media = Box::leak(Box::new(obc_storage::flat::sim::SparseDisk::blank(2_000_000, 7)));
        let disk = Box::leak(Box::new(obc_storage::flat::sim::FaultOnce::new(&*media)));
        let store = Box::leak(Box::new(FlatStore::initialize(&*disk, StoreId([0x54; 16])).unwrap()));
        let id = put(store, ObjectKind::Route, route, None);
        let map_id = put(store, ObjectKind::MapShard, map_bytes, None);
        let original = store.source(id, None).unwrap();
        let map = store.source(map_id, None).unwrap();
        flat_store::mount_sources(store, &original, &map);
        let tables = obc_reader::MapTables::parse(&map).unwrap();
        let mut app = obc_app::App::new_idle(obc_app::AppState::new(500_000, 500_000, 10.0));
        flat_store::load_routes(store, &mut app);
        Self {
            executor: detour::Executor::new(),
            app,
            guard: None,
            store,
            writer: Writer::new(store),
            reply: Box::leak(Box::default()),
            tokens: TokenSource::new(),
            original: Some(original),
            map: Some(map),
            tables,
            cache: obc_reader::MapCache::new(),
            free: store.free_extents(),
        }
    }
    fn poll(&mut self) -> Option<Outcome> {
        self.executor.poll(
            &mut self.app,
            self.store,
            self.writer,
            &mut self.guard,
            self.map.as_ref().unwrap(),
            &self.tables,
            &self.cache,
            &mut obc_elevation::NullElevation,
            self.reply,
        )
    }
    fn accept(&mut self, effect: Effect) -> Option<Outcome> {
        assert!(self.executor.accepts(&effect));
        self.executor.accept(effect, &self.app, self.store, &mut self.guard, 0)
    }
    fn acquire(&mut self) {
        let token = self.tokens.issue();
        assert_eq!(
            self.accept(Effect::Acquire {
                token,
                work: PlannerWork::Detour(obc_app::DetourRequest {
                    route: 0,
                    from: (500_000, 500_000),
                    progress_m: 0,
                    target_m: 2226
                })
            }),
            None
        );
    }
    fn step(&mut self) {
        let token = self.tokens.issue();
        assert_eq!(self.accept(Effect::Step { token }), None);
    }
    fn commit(&mut self) {
        let token = self.tokens.issue();
        assert_eq!(self.accept(Effect::CommitDetour { token }), None);
    }
    fn release(&mut self, retain_result: bool) -> obc_app::device_core::OperationToken<NavigatorTag> {
        let token = self.tokens.issue();
        assert_eq!(self.accept(Effect::Release { token, family: PlanFamily::Detour, retain_result }), None);
        token
    }
    fn next_outcome(&mut self) -> Outcome {
        for _ in 0..2000 {
            if self.writer.pending().is_some() {
                self.writer.complete();
            }
            if let Some(outcome) = self.poll() {
                return outcome;
            }
        }
        panic!("executor failed to settle");
    }
    fn until_ticket(&mut self, wanted: Kind) {
        for _ in 0..2000 {
            if let Some(kind) = self.writer.pending() {
                if kind == wanted {
                    return;
                }
                self.writer.complete();
            }
            if let Some(outcome) = self.poll() {
                match outcome {
                    Outcome::Acquired { .. } | Outcome::Stepped { .. } => self.step(),
                    other => panic!("before {wanted:?}: {other:?}"),
                }
            }
        }
        panic!("no {wanted:?} ticket");
    }
    fn preview(&mut self) {
        self.acquire();
        loop {
            match self.next_outcome() {
                Outcome::Acquired { .. } | Outcome::Stepped { .. } => self.step(),
                Outcome::DetourFinished { .. } => break,
                other => panic!("preview: {other:?}"),
            }
        }
        self.release(true);
        assert!(matches!(self.next_outcome(), Outcome::Released { .. }));
        assert!(self.guard.is_none());
        assert!(!self.executor.active());
    }
    fn assert_clean(&self) {
        assert!(self.guard.is_none());
        assert!(self.writer.pending().is_none());
        assert_eq!(self.store.free_extents(), self.free);
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.release(false);
            assert!(matches!(self.next_outcome(), Outcome::Released { .. }));
        }
        if let Some(source) = self.original.take() {
            self.store.close(source.release());
        }
        if let Some(source) = self.map.take() {
            self.store.close(source.release());
        }
    }
}

#[test]
fn real_preview_releases_arena_and_commit_publishes_fresh_route() {
    let mut h = Harness::new();
    h.preview();
    assert_eq!(h.store.entries().count(), 2);
    assert!(h.store.free_extents() < h.free, "preview retains sealed leg");
    h.commit();
    let Outcome::DetourCommitted { route, .. } = h.next_outcome() else { panic!("commit failed") };
    assert_ne!(route, h.original.as_ref().unwrap().id().0);
    h.store.with_source(ObjectId(route), None, |source| obc_route::RouteIndex::read(source)).unwrap().unwrap();
    h.release(true);
    assert!(matches!(h.next_outcome(), Outcome::Released { .. }));
    assert!(h.guard.is_none());
    assert!(h.original.as_ref().unwrap().is_current());
    assert_eq!(h.store.entries().count(), 3);
}

#[test]
fn canceled_tickets_drain_before_arena_release_and_retract_publication() {
    for (kind, refuse_allocation) in [
        (Kind::Allocate, false),
        (Kind::Allocate, true),
        (Kind::Write, false),
        (Kind::Seal, false),
        (Kind::Publish, false),
    ] {
        let mut h = Harness::new();
        if kind == Kind::Publish {
            h.preview();
            h.commit();
        } else {
            h.acquire();
        }
        h.until_ticket(kind);
        let other_reservations =
            refuse_allocation.then(|| [h.store.allocate(1).unwrap(), h.store.allocate(1).unwrap()]);
        let ticket = h.writer.pending();
        let token = h.tokens.issue();
        assert!(matches!(
            h.accept(Effect::Step { token }),
            Some(Outcome::Failed { error: NavigatorError::Workspace, .. })
        ));
        assert_eq!(h.writer.pending(), ticket, "wrong phase cannot replace ticket");
        let release_token = h.release(false);
        for _ in 0..3 {
            assert_eq!(h.poll(), None);
            assert!(h.guard.is_some());
        }
        assert_eq!(h.next_outcome(), Outcome::Released { token: release_token });
        if let Some(reservations) = other_reservations {
            for allocation in reservations {
                h.store.cancel(allocation);
            }
        }
        h.assert_clean();
        assert_eq!(h.store.entries().count(), 2);
    }
}

#[test]
fn failed_commit_keeps_preview_for_retry_and_full_cleanup_queue_keeps_owners() {
    let mut h = Harness::new();
    h.preview();
    h.commit();
    h.writer.transport().fail.set(Some(Kind::Write));
    assert!(matches!(h.next_outcome(), Outcome::Failed { error: NavigatorError::Store, .. }));
    h.release(true);
    assert!(matches!(h.next_outcome(), Outcome::Released { .. }));
    assert!(h.guard.is_none());
    h.commit();
    assert!(matches!(h.next_outcome(), Outcome::DetourCommitted { .. }));
    h.release(false);
    h.writer.transport().full.set(true);
    let free = h.store.free_extents();
    for _ in 0..3 {
        assert_eq!(h.poll(), None);
        assert!(h.guard.is_some());
        assert_eq!(h.store.free_extents(), free);
    }
    h.writer.transport().full.set(false);
    assert!(matches!(h.next_outcome(), Outcome::Released { .. }));
    h.assert_clean();
}

#[test]
fn retained_source_replacement_cannot_authorize_preview_commit() {
    let mut h = Harness::new();
    h.preview();
    let source = h.original.as_ref().unwrap();
    put(h.store, ObjectKind::Route, &route(), Some((source.id(), source.revision())));
    let token = h.tokens.issue();
    assert!(matches!(
        h.accept(Effect::CommitDetour { token }),
        Some(Outcome::Failed { error: NavigatorError::SourceChanged, .. })
    ));
    h.release(false);
    assert!(matches!(h.next_outcome(), Outcome::Released { .. }));
    assert!(h.guard.is_none());
    assert_eq!(h.store.entries().filter(|e| e.flags == EntryFlags::NONE).count(), 2);
}

#[test]
fn canceled_trim_replacement_preserves_both_sealed_owners_until_their_tickets_drain() {
    let mut h = Harness::new();
    h.acquire();
    h.until_ticket(Kind::Seal);
    h.writer.complete(); // sealed untrimmed A
    h.until_ticket(Kind::Seal);
    h.writer.complete(); // sealed trimmed B now occupies arena result slot
    assert_eq!(h.poll(), None);
    let free = h.store.free_extents();
    h.writer.transport().full.set(true);
    for _ in 0..3 {
        assert_eq!(h.poll(), None);
        assert_eq!(h.store.free_extents(), free);
    }
    h.writer.transport().full.set(false);
    assert_eq!(h.poll(), None);
    assert_eq!(h.writer.pending(), Some(Kind::ReleaseSealed));
    h.release(false);
    assert_eq!(h.poll(), None);
    assert!(h.guard.is_some());
    h.writer.complete(); // releasing A cannot discard the sealed B arena slot
    assert_eq!(h.poll(), None);
    h.writer.transport().full.set(true);
    for _ in 0..3 {
        assert_eq!(h.poll(), None);
        assert!(h.guard.is_some());
    }
    h.writer.transport().full.set(false);
    assert!(matches!(h.next_outcome(), Outcome::Released { .. }));
    h.assert_clean();
    assert_eq!(h.writer.transport().completed.borrow().iter().filter(|&&k| k == Kind::ReleaseSealed).count(), 2);
}

#[test]
fn full_hold_table_shares_exact_original_reader_and_map_removal_cancels_publication() {
    let mut h = Harness::new();
    let mut holds = Vec::new();
    for _ in 0..obc_storage::flat::store::MAX_OPEN_OBJECTS - 2 {
        let id = put(h.store, ObjectKind::Route, &route(), None);
        holds.push(h.store.open(id, None).unwrap());
    }
    let unopened = put(h.store, ObjectKind::Route, &route(), None);
    assert!(matches!(h.store.open(unopened, None), Err(StoreError::Busy)));
    h.preview(); // active route reader shares a row, sealed preview spends no hold
    for handle in holds {
        h.store.close(handle);
    }
    h.commit();
    h.until_ticket(Kind::Publish);
    let map = h.map.as_ref().unwrap();
    h.store.commit(&[Mutation::Remove { id: map.id(), revision: map.revision() }]).unwrap();
    h.writer.complete();
    assert!(matches!(h.poll(), Some(Outcome::Failed { error: NavigatorError::SourceChanged, .. })));
    h.release(false);
    assert!(matches!(h.next_outcome(), Outcome::Released { .. }));
    assert_eq!(
        h.store.entries().filter(|e| e.kind == ObjectKind::Route && e.flags == EntryFlags::NONE).count(),
        obc_storage::flat::store::MAX_OPEN_OBJECTS
    );
}

#[test]
fn assistant_board_admission_and_terminal_release_preserve_original_ownership() {
    use obc_app::navigator::{ReviewContext, ReviewPurpose, REVIEW_FACTS_POLICY};
    let h = Harness::new();
    let id = h.original.as_ref().unwrap().id();
    let entry = h.store.entries().find(|entry| entry.id == id).unwrap();
    let context = ReviewContext {
        purpose: ReviewPurpose::Destination,
        map: obc_formats::obcr::RouteSourceKey { store: h.store.store_id().0, object: 2, revision: 1 },
        store: obc_app::device_core::StoreIdentity::from_bytes(h.store.store_id().0),
        original: Some(metadata::fingerprint(entry)),
        origin: (0, 0),
        progress_m: 0,
        occurrence: 0,
        required_anchors_m: [0; 3],
        profile: 0,
        facts_policy: REVIEW_FACTS_POLICY,
        unresolved_avoidance: false,
    };
    assert!(assistant::original_allowed(h.store, context, Some(id.0)));
    assert!(!assistant::original_allowed(h.store, ReviewContext { original: None, ..context }, Some(id.0)));
    for _ in 0..12 {
        let mut retained = Some(h.store.source(id, Some(entry.revision)).unwrap());
        assistant::release_original(h.store, &mut retained, true);
        assert!(retained.is_some());
        assistant::release_original(h.store, &mut retained, false);
        assert!(retained.is_none());
    }
    let mut bytes = vec![0; entry.payload_len as usize];
    h.original.as_ref().unwrap().read_at(0, &mut bytes).unwrap();
    bytes[5] |= obc_formats::obcr::FLAG_UNRESOLVED_AVOIDANCE;
    let replacement = put(h.store, ObjectKind::Route, &bytes, Some((id, entry.revision)));
    let entry = h.store.entries().find(|entry| entry.id == replacement).unwrap();
    assert!(!assistant::original_allowed(
        h.store,
        ReviewContext { original: Some(metadata::fingerprint(entry)), ..context },
        Some(id.0)
    ));
}

#[path = "board_detour_support/visit_tests.rs"]
mod visit_tests;
