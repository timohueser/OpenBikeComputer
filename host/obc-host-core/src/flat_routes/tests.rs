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
    assert!(ROUTE.len() < PAGE);
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
    let mut routes = FlatRouteStore::from_bytes(&[ROUTE; obc_storage::flat::store::MAX_OPEN_OBJECTS - 1]).unwrap();
    let id = routes.write_nav_route(ROUTE).unwrap();
    let index = routes.ids().len() - 1;
    routes.sync_active(Some(index));
    let old = routes.active.as_ref().unwrap().clone();
    let mut held: Vec<_> = routes.ids[..obc_storage::flat::store::MAX_OPEN_OBJECTS - 1]
        .iter()
        .map(|&id| routes.owner.open(ObjectId(id), Revision(1)).unwrap())
        .collect();
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
                added_at_utc: 0,
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
    assert!(routes.refresh_metadata().is_ok());
    assert_eq!(routes.ids(), ids, "the menu stays bounded while the store can hold more routes");
    assert_eq!(FlatRouteStore::new(owner, &[]).unwrap().ids().len(), obc_app::MAX_ROUTES);
}

/// A built trip day is internal, and each new build replaces the one before it in place: two
/// presses of the day row leave one built day on the card.
#[test]
fn a_built_day_replaces_the_one_before_it_and_stays_out_of_the_list() {
    let mut built = ROUTE.to_vec();
    built[5] |= obc_formats::obcr::FLAG_BUILT_DAY;
    let mut routes = FlatRouteStore::from_bytes(&[ROUTE]).unwrap();
    let first = routes.publish_nav_route(&built).unwrap();
    let second = routes.publish_nav_route(&built).unwrap();
    assert_eq!((second.id, second.revision), (first.id, first.revision + 1), "the same object, replaced");
    assert_eq!(routes.ids().len(), 2, "the day and one built day");
    let index = routes.ids().iter().position(|&id| id == first.id).unwrap();
    assert_eq!(routes.internal_routes(), 1 << index, "the built day is internal");
    routes.refresh_metadata().unwrap();
    assert_eq!(routes.internal_routes(), 1 << index, "and stays so after a catalog read");
    assert_ne!(routes.publish_nav_route(ROUTE).unwrap().id, first.id, "a plain route is its own object");
}

#[test]
fn temporary_navigation_stays_readable_but_hidden_after_catalog_reload() {
    let owner = HostStore::memory().unwrap();
    let mut routes = FlatRouteStore::new(owner.clone(), &[ROUTE]).unwrap();
    let original = routes.ids()[0];
    let mut temporary = ROUTE.to_vec();
    temporary[5] |= obc_formats::obcr::FLAG_TEMPORARY;
    let publication = routes.publish_nav_route(&temporary).unwrap();
    for pass in 0..3 {
        let index = routes.ids().iter().position(|&id| id == publication.id).unwrap();
        assert_eq!(routes.internal_routes(), 1 << index);
        assert_eq!(routes.unaccepted_routes(), 0, "a temporary ride is not an abandoned review");
        assert_eq!(bytes(&routes.source(publication.id).unwrap()), temporary);
        assert_eq!(bytes(&routes.source(original).unwrap()), ROUTE);
        if pass == 0 {
            routes.refresh_metadata().unwrap();
        } else {
            routes = FlatRouteStore::new(owner.clone(), &[]).unwrap();
        }
    }
}
