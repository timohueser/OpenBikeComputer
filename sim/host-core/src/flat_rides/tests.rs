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

/// The v4 receipt an archiving client sends, answered by the same `Store` the link engine calls.
pub(crate) fn receipt(owner: &HostStore, head: EntryMeta) -> obc_link::flat::ArchiveResult {
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
}

/// The observation seam: the identity and rows the card metadata holds right now.
fn census(owner: &HostStore) -> (obc_storage::flat::StoreId, Vec<metadata::Row>) {
    let owner = owner.0.lock().unwrap();
    let store = owner.ready().unwrap();
    let mut rows = Vec::new();
    let identity = metadata::census(store, |row| rows.push(row)).unwrap();
    (identity, rows)
}

#[test]
fn visible_catalog_accepts_larger_stores_and_refuses_unreadable_sources() {
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
    assert_eq!(rides.catalog[0].id, heads.last().unwrap().id.0);
    assert!(rides.catalog[0].summary.synced);
    let overflow = {
        let mounted = owner.0.lock().unwrap();
        let store = mounted.ready().unwrap();
        let bytes = obc_formats::ride::encode_footer(&obc_formats::ride::Footer::new(
            "overflow", 0, 0, 0, 0, 0, 0, None, None, None, None, None,
        ));
        let mut allocation = store.allocate(bytes.len() as u64).unwrap();
        store.write(&mut allocation, &bytes).unwrap();
        let meta = EntryMeta {
            added_at_utc: 0,
            id: store.next_object_id(),
            revision: Revision(1),
            kind: ObjectKind::Ride,
            flags: EntryFlags::NONE,
            payload_len: bytes.len() as u64,
            payload_crc: obc_crc::crc32(&bytes),
            name: DisplayName::default(),
        };
        store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]).unwrap();
        meta
    };
    rides.refresh_metadata().unwrap();
    assert_eq!(rides.catalog.len(), obc_app::UI_RIDES_CAP);
    owner.remove(ObjectKind::Ride, overflow.id, overflow.revision).unwrap();
    // Replace an old, non-menu ride with malformed bytes: the full scan must still refuse it.
    owner
        .import(ObjectKind::Ride, Some((heads[1].id, heads[1].revision)), &mut &b"bad"[..], 3, DisplayName::default())
        .unwrap();
    assert_eq!(rides.refresh_metadata(), Err(MetadataError::WriteFailed));
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
    assert_eq!(rides.delete_by_id(replaced.id.0), Ok(true));
    assert_eq!(rides.delete_by_id(replaced.id.0), Ok(false), "confirmed absence before a catalog reload");
    // Unreadable card metadata must fail refresh, not publish unsynced defaults.
    {
        let owner = owner.0.lock().unwrap();
        let store = owner.ready().unwrap();
        let metadata = store.entries().map(Result::unwrap).find(|entry| entry.kind == ObjectKind::Metadata).unwrap();
        let handle = store.open(metadata.id, Some(metadata.revision)).unwrap();
        store.close(handle);
        let HostMedia::Memory(pages) = store.device() else { unreachable!() };
        pages.borrow_mut().clear();
        assert!(store.open(replaced.id, None).is_err());
    }
    assert!(rides.refresh_metadata().is_err());
}

/// Seed a nonzero archive proof timestamp in the stored metadata payload.
fn stamp_proof(owner: &HostStore, utc: u32) {
    let owner = owner.0.lock().unwrap();
    let store = owner.ready().unwrap();
    let head = store
        .entries()
        .map(Result::unwrap)
        .find(|entry| entry.kind == obc_storage::flat::ObjectKind::Metadata)
        .unwrap();
    let mut bytes = vec![0; head.payload_len as usize];
    let handle = store.open(head.id, Some(head.revision)).unwrap();
    assert_eq!(store.read(&handle, 0, &mut bytes).unwrap(), bytes.len());
    store.close(handle);
    bytes[metadata::HEADER_LEN + 28..metadata::HEADER_LEN + 32].copy_from_slice(&utc.to_le_bytes());
    let mut allocation = store.allocate(bytes.len() as u64).unwrap();
    store.write(&mut allocation, &bytes).unwrap();
    let meta = obc_storage::flat::EntryMeta {
        revision: obc_storage::flat::Revision(head.revision.0 + 1),
        payload_crc: obc_crc::crc32(&bytes),
        ..head
    };
    store
        .commit(&[
            Mutation::Remove { id: head.id, revision: head.revision },
            Mutation::Put { meta, source: PutSource::Fresh(allocation) },
        ])
        .unwrap();
}

#[test]
fn a_receipt_commits_one_observable_proof_whose_clock_starts_once() {
    const STAMP: u32 = 1_760_000_000;
    let owner = HostStore::memory().unwrap();
    let head = seed(&owner);
    let before = owner.0.lock().unwrap().ready().unwrap().sequence();
    let first = receipt(&owner, head);
    assert_eq!(first.timestamp, 0, "a fresh proof does not start the retention clock");
    assert!(first.sequence > before, "the proof is a catalog commit, not a resident note");

    let (identity, rows) = census(&owner);
    assert_eq!(
        rows,
        [metadata::Row {
            id: head.id,
            revision: head.revision,
            payload_len: head.payload_len,
            payload_crc: head.payload_crc,
            timestamp: 0,
            kind: ObjectKind::Ride,
        }]
    );

    stamp_proof(&owner, STAMP);
    let reopened = owner.remount_memory_snapshot();
    let (again, stamped) = census(&reopened);
    assert_eq!((again, stamped[0]), (identity, metadata::Row { timestamp: STAMP, ..rows[0] }));
    assert_eq!(stamped[0].id, head.id, "the stamp lands on the archived ride, not another row");

    let rides = FlatRideStore::new(reopened.clone()).unwrap();
    assert_eq!(
        (rides.catalog[0].summary.synced, rides.catalog[0].summary.synced_at_utc),
        (true, STAMP),
        "the catalog refresh republishes the durable stamp, not a resident guess"
    );

    let sequence = reopened.0.lock().unwrap().ready().unwrap().sequence();
    let duplicate = receipt(&reopened, head);
    assert_eq!(duplicate, obc_link::flat::ArchiveResult { sequence, timestamp: STAMP });
    assert_eq!(census(&reopened).1, stamped, "a duplicate receipt restarts no clock and writes nothing");
}
