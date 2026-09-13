use super::*;
use crate::flat_store::HostMedia;
use obc_storage::flat::{DisplayName, EntryMeta, Mutation, PutSource};

pub(crate) fn seed(owner: &HostStore) -> EntryMeta {
    let bytes = obc_formats::ride::encode_footer(&obc_formats::ride::Footer::new(
        "ride",
        1_700_000_000,
        1000,
        100,
        1000,
        10,
        0,
        None,
        None,
        None,
        None,
        None,
    ));
    owner.import(ObjectKind::Ride, None, &mut &bytes[..], bytes.len() as u64, DisplayName::default()).unwrap()
}

pub(crate) fn receipt(owner: &HostStore, head: EntryMeta) -> u32 {
    let owner = owner.0.lock().unwrap();
    let store = owner.ready().unwrap();
    obc_link::flat::Store::archive_ride(
        store,
        obc_link::flat::ArchiveSource {
            store: obc_link::flat::StoreId(store.store_id().0),
            id: obc_link::flat::ObjectId(head.id.0),
            revision: obc_link::flat::Revision(head.revision.0),
            payload_len: head.payload_len,
            payload_crc: head.payload_crc,
        },
    )
    .unwrap()
    .timestamp
}

#[test]
fn complete_inventory_covers_128_and_refuses_overflow_or_unreadable_sources() {
    let owner = HostStore::memory().unwrap();
    let mut heads = Vec::new();
    for _ in 0..obc_app::MAX_RIDES {
        heads.push(seed(&owner));
    }
    // The oldest is outside the menu; the newest is inside it.
    receipt(&owner, heads[0]);
    receipt(&owner, *heads.last().unwrap());
    let mut rides = FlatRideStore::new(HostStore(owner.0.clone())).unwrap();
    assert_eq!(rides.catalog.len(), obc_app::UI_RIDES_CAP);
    assert_eq!(rides.inventory.len(), obc_app::MAX_RIDES);
    assert_eq!(rides.catalog[0].id, heads.last().unwrap().id.0);
    assert!(rides.catalog[0].summary.synced);
    assert!(rides.inventory[0].synced);
    assert_eq!(rides.inventory[0].synced_at_utc, 0);
    let overflow = seed(&owner);
    assert_eq!(rides.refresh_metadata(), Err(RetentionError::WriteFailed));
    assert_eq!(rides.inventory.len(), obc_app::MAX_RIDES, "failure keeps the prior complete snapshot");
    owner.remove(ObjectKind::Ride, overflow.id, overflow.revision).unwrap();
    // Replace an old, non-menu ride with malformed bytes: the full scan must still refuse it.
    owner
        .import(ObjectKind::Ride, Some((heads[1].id, heads[1].revision)), &mut &b"bad"[..], 3, DisplayName::default())
        .unwrap();
    assert_eq!(rides.refresh_metadata(), Err(RetentionError::WriteFailed));
}

#[test]
fn retained_recording_and_replaced_heads_cannot_inherit_proof_or_be_recorded_into() {
    let owner = HostStore::memory().unwrap();
    let retained = seed(&owner);
    let replaced = seed(&owner);
    for head in [retained, replaced] {
        receipt(&owner, head);
    }
    {
        let owner = owner.0.lock().unwrap();
        let store = owner.ready().unwrap();
        let allocation = store.allocate(1024).unwrap();
        store
            .commit(&[
                Mutation::Put { meta: EntryMeta { flags: EntryFlags::RETAINED, ..retained }, source: PutSource::Amend },
                Mutation::Put {
                    meta: EntryMeta {
                        revision: Revision(2),
                        flags: EntryFlags::RECORDING,
                        payload_len: 0,
                        payload_crc: 0,
                        ..retained
                    },
                    source: PutSource::Fresh(allocation),
                },
            ])
            .unwrap();
    }
    let source = owner.open(replaced.id, replaced.revision).unwrap();
    let mut bytes = vec![0; replaced.payload_len as usize];
    obc_formats::io::ByteSource::read_at(&source, 0, &mut bytes).unwrap();
    drop(source);
    owner
        .import(
            ObjectKind::Ride,
            Some((replaced.id, replaced.revision)),
            &mut &bytes[..],
            bytes.len() as u64,
            DisplayName::default(),
        )
        .unwrap();
    let mut rides = FlatRideStore::new(HostStore(owner.0.clone())).unwrap();
    assert_eq!(rides.inventory.len(), 1);
    assert_eq!(rides.inventory[0].id, replaced.id.0);
    assert!(!rides.inventory[0].synced);
    let expected = rides.store_scope().unwrap();
    assert_eq!(rides.expire_ride(replaced.id.0, expected), Err(CatalogError::Stale));
    assert!(!rides.open(1, None));
    assert!(!rides.checkpoint());
    assert!(!rides.discard());
    let stats = RideStats {
        distance_m: 0,
        moving_time_s: 0,
        avg_speed_cms: 0,
        climb_m: 0,
        unix_at_anchor: 0,
        anchor_ms: 0,
        clock_trusted: false,
        avg_hr: None,
        max_hr: None,
        avg_cadence: None,
        avg_power: None,
        max_power: None,
    };
    assert!(matches!(rides.finalize(stats), RideClose::Failed));
    // Unreadable card metadata must fail refresh, not publish unsynced defaults.
    {
        let owner = owner.0.lock().unwrap();
        let store = owner.ready().unwrap();
        let metadata = store.entries().find(|entry| entry.kind == ObjectKind::Metadata).unwrap();
        let handle = store.open(metadata.id, Some(metadata.revision)).unwrap();
        store.close(handle);
        let HostMedia::Memory(pages) = store.device() else { unreachable!() };
        pages.borrow_mut().clear();
        assert!(store.open(replaced.id, None).is_err());
    }
    assert!(rides.refresh_metadata().is_err());
    assert_eq!(rides.inventory.len(), 1);
}
