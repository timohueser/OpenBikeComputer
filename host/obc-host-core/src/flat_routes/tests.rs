use super::*;
use crate::{
    flat_map::FlatMap,
    flat_store::{HostMedia, PAGE},
    ActiveRouteSession,
};
use obc_storage::flat::Revision;

const ROUTE: &[u8] = include_bytes!("../../../../fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr");
const MAP: &[u8] = include_bytes!("../../../../apps/obc-sim/assets/grimsel-demo.obcm");

fn bytes(source: &dyn ByteSource) -> Vec<u8> {
    let mut bytes = vec![0; source.len() as usize];
    source.read_at(0, &mut bytes).unwrap();
    bytes
}
fn pages(owner: &HostStore) -> usize {
    let store = owner.0.lock().unwrap();
    let store = &store.card;
    let HostMedia::Memory(pages) = store.device() else { unreachable!() };
    let count = pages.borrow().len();
    count
}

#[test]
fn shared_map_and_routes_pin_revisions_and_reuse_sparse_pages() {
    let owner = HostStore::memory().unwrap();
    let map = FlatMap::from_bytes_in(&owner, MAP).unwrap();
    let before = pages(&owner);
    let mut routes = FlatRouteStore::new(HostStore(owner.0.clone()), &[ROUTE]).unwrap();
    assert_eq!(pages(&owner) - before, 1);
    assert_eq!(PAGE - ROUTE.len(), 12_632);
    let id = routes.write_nav_route(ROUTE).unwrap();
    let index = routes.ids().iter().position(|&candidate| candidate == id).unwrap();
    assert!(routes.sync_active(Some(index)));
    let old = routes.active.as_ref().unwrap().clone();
    assert_eq!(old.store_id(), map.source().store_id());
    assert_ne!(old.id(), map.source().id());
    assert_eq!(routes.delete_by_id(map.source().id().0), Err(CatalogError::Unsupported));
    assert_eq!(&bytes(&map.source())[..4], b"OBCM");

    let mut session = ActiveRouteSession::new();
    session.reparse(true, &routes);
    assert!(session.index().is_some());
    assert_eq!(routes.write_nav_route(ROUTE), Some(id));
    // Same catalog position, but an exact new revision, without external invalidation.
    assert!(routes.sync_active(Some(index)));
    session.reparse(true, &routes);
    assert_eq!(routes.active.as_ref().unwrap().revision(), Revision(2));
    assert!(!routes.sync_active(Some(index)));
    assert_eq!(old.revision(), Revision(1));
    assert_eq!(bytes(&old), ROUTE);
    drop(old);
    let high_water = pages(&owner);
    for _ in 0..12 {
        assert_eq!(routes.write_nav_route(ROUTE), Some(id));
        assert!(routes.sync_active(Some(index)));
        assert!(!routes.sync_active(Some(index)));
        assert_eq!(pages(&owner), high_water, "released extents reuse already allocated pages");
    }
    let last = routes.active.as_ref().unwrap().clone();
    assert_eq!(routes.delete_by_id(id), Ok(true));
    assert_eq!(routes.delete_by_id(id), Ok(false));
    assert_eq!(bytes(&last), ROUTE);
    assert!(routes.active_source().is_none());
    drop(last);
    assert_eq!(pages(&owner), high_water, "deletion does not shrink sparse memory high-water");
}

#[test]
fn committed_revision_survives_busy_open_and_retries_without_stale_source() {
    let mut routes = FlatRouteStore::from_bytes(&[ROUTE; 5]).unwrap();
    let id = routes.write_nav_route(ROUTE).unwrap();
    let index = routes.ids().len() - 1;
    routes.sync_active(Some(index));
    let old = routes.active.as_ref().unwrap().clone();
    let mut held: Vec<_> =
        routes.ids[..5].iter().map(|&id| routes.owner.open(ObjectId(id), Revision(1)).unwrap()).collect();
    assert_eq!(routes.write_nav_route(ROUTE), Some(id), "commit does not need another reader slot");
    assert!(routes.sync_active(Some(index)), "failed open clears the old active binding");
    assert!(routes.active_source().is_none());
    assert_eq!(bytes(&old), ROUTE);
    let mut session = ActiveRouteSession::new();
    session.reparse(true, &routes);
    assert!(session.index().is_none());
    assert!(!routes.sync_active(Some(index)), "a still-missing source causes no repeated reparse");
    held.pop();
    assert!(routes.sync_active(Some(index)), "the exact committed revision remains owed");
    session.reparse(true, &routes);
    assert!(session.index().is_some());
    assert_eq!(routes.active.as_ref().unwrap().revision(), Revision(2));
    assert!(!routes.sync_active(Some(index)));
}

#[test]
fn failed_route_write_and_delete_keep_committed_projection() {
    let mut routes = FlatRouteStore::from_bytes(&[ROUTE]).unwrap();
    assert!(routes.write_nav_route(b"invalid").is_none());
    assert!(routes.nav_id.is_none());
    assert_eq!(routes.ids().len(), 1);
    let id = routes.ids()[0];
    // A stale expected revision must fail rather than publish successful absence.
    let old = (ObjectId(id), routes.revisions[0]);
    routes
        .owner
        .import(ObjectKind::Route, Some(old), &mut &ROUTE[..], ROUTE.len() as u64, DisplayName::default())
        .unwrap();
    assert_eq!(routes.delete_by_id(id), Err(CatalogError::Stale));
    assert_eq!(routes.ids(), &[id]);
    // A different current revision is a failed removal, not confirmed absence.
    assert_eq!(bytes(&routes.owner.open(ObjectId(id), Revision(2)).unwrap()), ROUTE);
    {
        let store = routes.owner.0.lock().unwrap();
        let store = &store.card;
        let HostMedia::Memory(pages) = store.device() else { unreachable!() };
        pages.borrow_mut().clear();
    }
    assert_eq!(routes.delete_by_id(id), Err(CatalogError::Unreadable));
    assert_eq!(routes.ids(), &[id], "media errors cannot remove a catalog row");
}

#[test]
fn metadata_executor_commits_then_refreshes_and_refuses_stale_card_and_object_scopes() {
    use obc_app::{
        device_core::{RetentionTag, TokenSource},
        retention::{RetentionEffect, RetentionError},
        Retention, RouteRetentionMeta,
    };
    let owner = HostStore::memory().unwrap();
    let mut routes = FlatRouteStore::new(HostStore(owner.0.clone()), &[ROUTE]).unwrap();
    let original = routes.refresh_metadata().unwrap().unwrap();
    let id = routes.ids()[0];
    let mut tokens = TokenSource::<RetentionTag>::new();
    let effect = RetentionEffect::WriteRouteMetadata {
        token: tokens.issue(),
        scope: Some(original),
        id,
        meta: RouteRetentionMeta::new(Retention::Day1, 1234),
    };
    routes.write_metadata(effect).unwrap();
    assert_eq!(
        routes.retention_metas()[0],
        RouteRetentionMeta::default(),
        "commit does not mutate the served projection"
    );
    let committed = routes.refresh_metadata().unwrap().unwrap();
    assert_eq!(routes.retention_metas()[0].last_used_utc, 1234);
    assert_eq!(routes.write_metadata(effect), Err(RetentionError::Stale));
    let mut other_card = FlatRouteStore::from_bytes(&[ROUTE]).unwrap();
    assert_eq!(other_card.ids()[0], id);
    assert_eq!(other_card.write_metadata(effect), Err(RetentionError::Stale));
    routes.write(ROUTE, Some((ObjectId(id), routes.revisions[0]))).unwrap();
    assert_eq!(routes.expire_route(id, committed), Err(CatalogError::Stale));
    routes.refresh_metadata().unwrap();
    assert_eq!(routes.retention_metas()[0], RouteRetentionMeta::default(), "replacement has no old stamp authority");
    let store = owner.0.lock().unwrap();
    let mut count = 0;
    obc_storage::flat::metadata::read_routes(&store.card, |_| count += 1).unwrap();
    assert_eq!(count, 0, "reconciliation published the empty metadata image");
}

#[test]
fn full_route_catalog_refuses_growth_but_keeps_replacement_and_complete_projection() {
    use obc_storage::flat::{EntryFlags, Mutation, PutSource, Store};
    let owner = HostStore::memory().unwrap();
    let mut routes = FlatRouteStore::new(owner.clone(), &vec![ROUTE; obc_app::MAX_ROUTES]).unwrap();
    let ids = routes.ids().to_vec();
    let before = routes.store_scope();
    assert!(matches!(routes.import(ROUTE), Err(ImportError::Storage(StoreError::Busy))));
    assert!(routes.publish_nav_route(ROUTE).is_none());
    assert_eq!(routes.store_scope(), before, "refusal does not publish or allocate an object");
    routes.replace(ids[0], ROUTE).unwrap();
    assert_eq!(routes.source(ids[0]).unwrap().revision(), Revision(2));
    {
        // Media from another producer can exceed the application's bounded catalog.
        let owner = owner.0.lock().unwrap();
        let card = owner.ready().unwrap();
        let mut allocation = card.allocate(ROUTE.len() as u64).unwrap();
        card.write(&mut allocation, ROUTE).unwrap();
        card.commit(&[Mutation::Put {
            meta: EntryMeta {
                id: card.next_object_id(),
                revision: Revision(1),
                kind: ObjectKind::Route,
                flags: EntryFlags::NONE,
                payload_len: ROUTE.len() as u64,
                payload_crc: obc_crc::crc32(ROUTE),
                name: DisplayName::default(),
            },
            source: PutSource::Fresh(allocation),
        }])
        .unwrap();
    }
    assert!(routes.refresh_metadata().is_err());
    assert_eq!(routes.ids(), ids, "an incomplete refresh keeps the previous complete projection");
    assert!(FlatRouteStore::new(owner, &[]).is_err(), "reopen cannot hide excess routes");
}
