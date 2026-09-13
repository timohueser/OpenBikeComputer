use super::*;
use crate::flat::sim::{FaultOnce, FaultPlan, MediaOp, SparseDisk, When};
use std::vec::Vec;

const CARD: StoreId = StoreId([0x42; 16]);
const BLOCKS: u64 = 200_000;

fn publish<D: BlockDevice>(store: &FlatStore<D>, kind: ObjectKind, bytes: &[u8]) -> EntryMeta {
    let mut allocation = store.allocate(bytes.len() as u64).unwrap();
    store.write(&mut allocation, bytes).unwrap();
    let meta = EntryMeta {
        id: store.next_object_id(),
        revision: Revision(1),
        kind,
        flags: EntryFlags::NONE,
        payload_len: bytes.len() as u64,
        payload_crc: obc_crc::crc32(bytes),
        name: Default::default(),
    };
    store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]).unwrap();
    meta
}

fn row(meta: EntryMeta) -> Row {
    Row {
        id: meta.id,
        revision: meta.revision,
        payload_len: meta.payload_len,
        payload_crc: meta.payload_crc,
        timestamp: 1234,
        kind: meta.kind,
        retention: 0,
    }
}

#[test]
fn normative_vector_is_exact_and_hostile_records_are_refused() {
    let vector = include_bytes!("../../../../../specs/vectors/retention-metadata/route-and-ride.bin");
    let mut bytes = [0; MAX_LEN];
    let mut image = Image::empty(CARD, &mut bytes).unwrap();
    image
        .set(Row {
            id: ObjectId(0x100000001),
            revision: Revision(0x200000003),
            payload_len: 0x300000004,
            payload_crc: 0x12345678,
            timestamp: 0x65000000,
            kind: ObjectKind::Route,
            retention: 5,
        })
        .unwrap();
    image
        .set(Row {
            id: ObjectId(0x100000002),
            revision: Revision(7),
            payload_len: 123456,
            payload_crc: 0xabcdef01,
            timestamp: 0x66000000,
            kind: ObjectKind::Ride,
            retention: 0,
        })
        .unwrap();
    assert_eq!(image.bytes(), vector);
    for (offset, value) in [
        (4, 2),
        (8, 41),
        (12, 1),
        (10, 3),
        (HEADER_LEN + 34, 6),
        (HEADER_LEN + ROW_LEN + 34, 1),
        (HEADER_LEN + 35, 1),
        (HEADER_LEN + 32, 2),
    ] {
        let mut bad = vector.to_vec();
        bad[offset] = value;
        let len = bad.len();
        assert!(Image::decode(&mut bad, len).is_err(), "accepted corruption at {offset}");
    }
    let mut duplicate = vector.to_vec();
    duplicate[HEADER_LEN + ROW_LEN..HEADER_LEN + ROW_LEN + 8].copy_from_slice(&vector[HEADER_LEN..HEADER_LEN + 8]);
    let len = duplicate.len();
    assert!(Image::decode(&mut duplicate, len).is_err());
    let mut trailing = vector.to_vec();
    trailing.push(0);
    let len = trailing.len();
    assert!(Image::decode(&mut trailing, len).is_err());
}

#[test]
fn capacity_refuses_without_losing_any_existing_record() {
    let mut bytes = [0; MAX_LEN];
    let mut image = Image::empty(CARD, &mut bytes).unwrap();
    for id in 1..=(MAX_ROUTES + MAX_RIDES) as u64 {
        image
            .set(Row {
                id: ObjectId(id),
                revision: Revision(1),
                payload_len: 1,
                payload_crc: 0,
                timestamp: 0,
                kind: if id <= MAX_ROUTES as u64 { ObjectKind::Route } else { ObjectKind::Ride },
                retention: 0,
            })
            .unwrap();
    }
    let previous = image.bytes().to_vec();
    let extra = Row { id: ObjectId(1000), ..image.rows().last().unwrap() };
    assert_eq!(image.set(extra), Err(Error::Capacity));
    assert_eq!(image.bytes(), previous);
    assert_eq!(image.rows().filter(|r| r.kind == ObjectKind::Ride).count(), 128);
}

#[test]
fn actual_store_replace_reopen_reconcile_and_identity_guards() {
    let disk = SparseDisk::blank(BLOCKS, 1);
    let store = FlatStore::initialize(&disk, CARD).unwrap();
    let route = publish(&store, ObjectKind::Route, b"route");
    let ride = publish(&store, ObjectKind::Ride, b"ride");
    let mut owner = Metadata::new(&store);
    let mut bytes = [0; MAX_LEN];
    let mut image = owner.load(&store, &mut bytes).unwrap();
    assert_eq!(image.rows().count(), 0);
    image.set(row(route)).unwrap();
    let first = owner.replace(&store, &mut image, Some(route)).unwrap();
    image.set(row(ride)).unwrap();
    let second = owner.replace(&store, &mut image, Some(ride)).unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(second.revision, Revision(2));
    assert_eq!(store.entries().filter(|e| e.kind == ObjectKind::Metadata).count(), 1);
    let mut rival = Metadata::new(&store);
    let mut other_bytes = [0; MAX_LEN];
    let mut rival_image = rival.load(&store, &mut other_bytes).unwrap();
    image.set(Row { timestamp: 8888, ..row(route) }).unwrap();
    owner.replace(&store, &mut image, Some(route)).unwrap();
    assert_eq!(rival.replace(&store, &mut rival_image, Some(route)), Err(Error::Stale));
    store.commit(&[Mutation::Remove { id: ride.id, revision: ride.revision }]).unwrap();
    assert!(image.reconcile(&store).unwrap());
    assert_eq!(image.rows().collect::<Vec<_>>(), [Row { timestamp: 8888, ..row(route) }]);
    owner.replace(&store, &mut image, Some(route)).unwrap();
    disk.reboot();
    let reopened = FlatStore::mount(&disk);
    let mut owner = Metadata::new(&reopened);
    let mut loaded = owner.load(&reopened, &mut bytes).unwrap();
    assert_eq!(loaded.rows().count(), 1);
    assert_eq!(loaded.rows().next().unwrap().timestamp, 8888);
    let other_disk = SparseDisk::blank(BLOCKS, 2);
    let other = FlatStore::initialize(&other_disk, StoreId([3; 16])).unwrap();
    assert_eq!(owner.replace(&other, &mut loaded, Some(route)), Err(Error::WrongStore));
    let new_route = EntryMeta { revision: Revision(2), ..route };
    let mut allocation = reopened.allocate(5).unwrap();
    reopened.write(&mut allocation, b"route").unwrap();
    reopened
        .commit(&[
            Mutation::Remove { id: route.id, revision: route.revision },
            Mutation::Put { meta: new_route, source: PutSource::Fresh(allocation) },
        ])
        .unwrap();
    assert_eq!(owner.replace(&reopened, &mut loaded, Some(route)), Err(Error::Stale));
}

#[test]
fn malformed_duplicate_and_failed_catalog_reads_never_become_empty_defaults() {
    let disk = SparseDisk::blank(BLOCKS, 3);
    let device = FaultOnce::new(&disk);
    let store = FlatStore::initialize(&device, CARD).unwrap();
    let mut owner = Metadata::new(&store);
    let mut bytes = [0; MAX_LEN];
    publish(&store, ObjectKind::Metadata, b"broken");
    assert!(matches!(owner.load(&store, &mut bytes), Err(Error::Invalid)));
    publish(&store, ObjectKind::Metadata, b"also broken");
    assert!(matches!(owner.load(&store, &mut bytes), Err(Error::DuplicateObject)));
    store.device().fault_next(MediaOp::Read);
    assert!(matches!(owner.load(&store, &mut bytes), Err(Error::Store(StoreError::Media))));
    assert!(store.device().fired());
    let mut image = Image::empty(CARD, &mut bytes).unwrap();
    let route = publish(&store, ObjectKind::Route, b"route");
    image.set(row(route)).unwrap();
    store.device().fault_next(MediaOp::Read);
    assert_eq!(image.reconcile(&store), Err(Error::Store(StoreError::Media)));
    assert_eq!(image.rows().count(), 1, "a failed scan must not prune anything");
}

#[test]
fn prepublication_commit_error_keeps_old_metadata_readable() {
    let disk = SparseDisk::blank(BLOCKS, 4);
    let device = FaultOnce::new(&disk);
    let store = FlatStore::initialize(&device, CARD).unwrap();
    let target = publish(&store, ObjectKind::Ride, b"ride");
    let mut owner = Metadata::new(&store);
    let mut bytes = [0; MAX_LEN];
    let mut image = owner.load(&store, &mut bytes).unwrap();
    image.set(row(target)).unwrap();
    owner.replace(&store, &mut image, Some(target)).unwrap();
    image.set(Row { timestamp: 9999, ..row(target) }).unwrap();
    store.device().fault_next(MediaOp::Sync);
    assert_eq!(owner.replace(&store, &mut image, Some(target)), Err(Error::Store(StoreError::Media)));
    assert!(store.device().fired());
    assert_eq!(owner.load(&store, &mut bytes).unwrap().rows().next().unwrap().timestamp, 1234);
    disk.reboot();
    let reopened = FlatStore::mount(&disk);
    let mut owner = Metadata::new(&reopened);
    assert_eq!(owner.load(&reopened, &mut bytes).unwrap().rows().next().unwrap().timestamp, 1234);
}

#[test]
fn power_cut_at_final_gate_sync_recovers_a_complete_generation() {
    // Locate this operation's final sync once; cut there after durability but before successful return.
    fn run(cut: Option<u32>) -> (u32, u32) {
        let disk = SparseDisk::blank(BLOCKS, 5);
        let store = FlatStore::initialize(&disk, CARD).unwrap();
        let target = publish(&store, ObjectKind::Ride, b"ride");
        let mut owner = Metadata::new(&store);
        let mut bytes = [0; MAX_LEN];
        let mut image = owner.load(&store, &mut bytes).unwrap();
        image.set(row(target)).unwrap();
        owner.replace(&store, &mut image, Some(target)).unwrap();
        image.set(Row { timestamp: 9999, ..row(target) }).unwrap();
        if let Some(op) = cut {
            disk.plan(FaultPlan { op, when: When::After });
        }
        let result = owner.replace(&store, &mut image, Some(target));
        let last_sync = disk.ledger().iter().rev().find(|(_, op, _)| *op == MediaOp::Sync).unwrap().0;
        if cut.is_some() {
            assert_eq!(result, Err(Error::RemountRequired));
        }
        disk.reboot();
        let reopened = FlatStore::mount(&disk);
        let mut owner = Metadata::new(&reopened);
        let loaded = owner.load(&reopened, &mut bytes).unwrap();
        let stamp = loaded.rows().next().unwrap().timestamp;
        (last_sync, stamp)
    }
    let (last_sync, stamp) = run(None);
    assert_eq!(stamp, 9999);
    assert_eq!(run(Some(last_sync)).1, 9999, "failed return can follow a durable whole generation");
}

#[test]
fn leases_preserve_old_bytes_without_admitting_old_source_or_metadata_heads() {
    let disk = SparseDisk::blank(BLOCKS, 6);
    let store = FlatStore::initialize(&disk, CARD).unwrap();
    let route = publish(&store, ObjectKind::Route, b"route");
    let route_lease = store.open(route.id, Some(route.revision)).unwrap();
    let mut owner = Metadata::new(&store);
    let mut bytes = [0; MAX_LEN];
    let mut image = owner.load(&store, &mut bytes).unwrap();
    image.set(row(route)).unwrap();
    let old = owner.replace(&store, &mut image, Some(route)).unwrap();
    let old_bytes = image.bytes().to_vec();
    let metadata_lease = store.open(old.id, Some(old.revision)).unwrap();
    image.set(Row { timestamp: 9999, ..row(route) }).unwrap();
    owner.replace(&store, &mut image, Some(route)).unwrap();
    let mut readback = [0; MAX_LEN];
    assert_eq!(store.read(&metadata_lease, 0, &mut readback).unwrap(), old_bytes.len());
    assert_eq!(&readback[..old_bytes.len()], old_bytes);
    // A retained catalog row and a pinned reader are independent storage mechanisms.
    let mut allocation = store.allocate(5).unwrap();
    store.write(&mut allocation, b"newer").unwrap();
    let next = EntryMeta { revision: Revision(2), payload_crc: obc_crc::crc32(b"newer"), ..route };
    store
        .commit(&[
            Mutation::Put { meta: EntryMeta { flags: EntryFlags::RETAINED, ..route }, source: PutSource::Amend },
            Mutation::Put { meta: next, source: PutSource::Fresh(allocation) },
        ])
        .unwrap();
    assert_eq!(owner.replace(&store, &mut image, Some(route)), Err(Error::Stale));
    assert!(image.reconcile(&store).unwrap());
    assert_eq!(image.rows().count(), 0);
    owner.replace(&store, &mut image, None).unwrap();
    let mut mounted_owner = Metadata::new(&store);
    assert_eq!(mounted_owner.load(&store, &mut readback).unwrap().rows().count(), 0);
    store.close(route_lease);
    store.close(metadata_lease);
}

#[test]
fn precommit_payload_failure_can_retry_without_leaking_a_reservation() {
    let disk = SparseDisk::blank(BLOCKS, 7);
    let device = FaultOnce::new(&disk);
    let store = FlatStore::initialize(&device, CARD).unwrap();
    let mut owner = Metadata::new(&store);
    let mut bytes = [0; MAX_LEN];
    let mut image = owner.load(&store, &mut bytes).unwrap();
    for _ in 0..13 {
        image.set(row(publish(&store, ObjectKind::Ride, b"ride"))).unwrap();
    }
    for _ in 0..3 {
        device.fault_next(MediaOp::Write);
        assert_eq!(owner.replace(&store, &mut image, None), Err(Error::Store(StoreError::Media)));
        assert!(device.fired());
    }
    owner.replace(&store, &mut image, None).unwrap();
}

#[test]
fn reloading_the_writer_does_not_refresh_an_older_images_publication_authority() {
    let disk = SparseDisk::blank(BLOCKS, 8);
    let store = FlatStore::initialize(&disk, CARD).unwrap();
    let route = publish(&store, ObjectKind::Route, b"route");
    let ride = publish(&store, ObjectKind::Ride, b"ride");
    let mut owner = Metadata::new(&store);
    let mut bytes = [0; MAX_LEN];
    let mut old = owner.load(&store, &mut bytes).unwrap();
    old.set(row(route)).unwrap();
    owner.replace(&store, &mut old, Some(route)).unwrap();
    let mut rival = Metadata::new(&store);
    let mut other = [0; MAX_LEN];
    let mut latest = rival.load(&store, &mut other).unwrap();
    latest.set(row(ride)).unwrap();
    rival.replace(&store, &mut latest, Some(ride)).unwrap();
    let mut refreshed_bytes = [0; MAX_LEN];
    let _refreshed = owner.load(&store, &mut refreshed_bytes).unwrap();
    assert_eq!(owner.replace(&store, &mut old, Some(route)), Err(Error::Stale));
    let mut synthetic = Image::empty(CARD, &mut other).unwrap();
    assert_eq!(owner.replace(&store, &mut synthetic, None), Err(Error::Stale));
}

#[test]
fn a_full_read_handle_table_refuses_before_publication_and_retries_after_close() {
    let disk = SparseDisk::blank(BLOCKS, 9);
    let store = FlatStore::initialize(&disk, CARD).unwrap();
    let route = publish(&store, ObjectKind::Route, b"route");
    let mut handles = Vec::new();
    for _ in 0..6 {
        let object = publish(&store, ObjectKind::Ride, b"ride");
        handles.push(store.open(object.id, Some(object.revision)).unwrap());
    }
    let sequence = store.sequence();
    assert_eq!(write_route(&store, CARD, sequence, route.id, 1, 1234), Err(Error::Store(StoreError::Busy)));
    assert_eq!(store.sequence(), sequence);
    assert!(store.mode().writable());
    store.close(handles.pop().unwrap());
    write_route(&store, CARD, sequence, route.id, 1, 1234).unwrap();
    for handle in handles {
        store.close(handle);
    }
}

#[test]
fn committed_readback_failure_fences_every_writer_until_remount() {
    fn run(fail_read: Option<u32>) -> u32 {
        let disk = SparseDisk::blank(BLOCKS, 10);
        let device = FaultOnce::new(&disk);
        let store = FlatStore::initialize(&device, CARD).unwrap();
        let target = publish(&store, ObjectKind::Route, b"route");
        let sequence = store.sequence();
        let before = disk.ledger().len();
        if let Some(skip) = fail_read {
            device.fault_after(MediaOp::Read, skip);
        }
        let result = write_route(&store, CARD, sequence, target.id, 1, 1234);
        let ledger = disk.ledger();
        let writes = &ledger[before..];
        let published = writes.iter().rposition(|(_, op, _)| *op == MediaOp::Sync).unwrap();
        let reads = writes[..=published].iter().filter(|(_, op, _)| *op == MediaOp::Read).count() as u32;
        if fail_read.is_some() {
            assert!(device.fired());
            assert_eq!(result, Err(Error::RemountRequired));
            assert!(store.sequence() > sequence, "publication preceded the failed readback");
            assert_eq!(store.mode(), super::super::Mode::RemountRequired);
            assert!(matches!(store.allocate(1), Err(StoreError::ReadOnly)));
        } else {
            result.unwrap();
        }
        reads
    }
    let reads = run(None);
    run(Some(reads));
}

#[test]
fn uncertain_expiry_reports_remount_before_policy_can_retry() {
    fn run(fail_sync: Option<u32>) -> u32 {
        let disk = SparseDisk::blank(BLOCKS, 11);
        let device = FaultOnce::new(&disk);
        let store = FlatStore::initialize(&device, CARD).unwrap();
        let target = publish(&store, ObjectKind::Route, b"route");
        let sequence = store.sequence();
        let before = disk.ledger().len();
        if let Some(skip) = fail_sync {
            device.fault_after(MediaOp::Sync, skip);
        }
        let result = remove_route(&store, CARD, sequence, target.id);
        let syncs = disk.ledger()[before..].iter().filter(|(_, op, _)| *op == MediaOp::Sync).count() as u32;
        if fail_sync.is_some() {
            assert!(device.fired());
            assert_eq!(result, Err(Error::RemountRequired));
            assert_eq!(store.mode(), super::super::Mode::RemountRequired);
            let mut bytes = [0; MAX_LEN];
            assert!(matches!(Metadata::new(&store).load(&store, &mut bytes), Err(Error::RemountRequired)));
        } else {
            result.unwrap();
        }
        syncs
    }
    let syncs = run(None);
    run(Some(syncs - 1));
}

#[test]
fn ride_clock_requires_exact_proof_and_never_restarts_or_creates_it() {
    let disk = SparseDisk::blank(BLOCKS, 12);
    let device = FaultOnce::new(&disk);
    let store = FlatStore::initialize(&device, CARD).unwrap();
    let ride = publish(&store, ObjectKind::Ride, b"ride");
    assert_eq!(write_ride(&store, CARD, store.sequence(), ride.id, 1234), Err(Error::Stale));
    archive_ride(&store, CARD, ride.id, ride.revision, ride.payload_len, ride.payload_crc).unwrap();
    let unstamped = store.sequence();
    assert_eq!(remove_ride(&store, CARD, unstamped, ride.id), Err(Error::Stale));
    assert_eq!(write_ride(&store, CARD, unstamped, ride.id, 0), Err(Error::Invalid));
    device.fault_next(MediaOp::Write);
    assert_eq!(write_ride(&store, CARD, unstamped, ride.id, 1234), Err(Error::Store(StoreError::Media)));
    assert!(store.mode().writable());
    write_ride(&store, CARD, unstamped, ride.id, 1234).unwrap();
    let stamped = store.sequence();
    assert_eq!(write_ride(&store, CARD, unstamped, ride.id, 9999), Err(Error::Stale));
    write_ride(&store, CARD, stamped, ride.id, 9999).unwrap();
    assert_eq!(archive_ride(&store, CARD, ride.id, ride.revision, ride.payload_len, ride.payload_crc), Ok(1234));
    assert_eq!(store.sequence(), stamped);
    assert_eq!(write_ride(&store, StoreId([1; 16]), stamped, ride.id, 9999), Err(Error::WrongStore));
    let mut rows = Vec::new();
    read_rows(&store, |row| rows.push(row)).unwrap();
    assert_eq!(rows[0].timestamp, 1234);
    // Retaining the old revision cannot transfer its proof to a replacement head.
    let mut allocation = store.allocate(4).unwrap();
    store.write(&mut allocation, b"next").unwrap();
    store
        .commit(&[
            Mutation::Put { meta: EntryMeta { flags: EntryFlags::RETAINED, ..ride }, source: PutSource::Amend },
            Mutation::Put {
                meta: EntryMeta { revision: Revision(2), payload_crc: obc_crc::crc32(b"next"), ..ride },
                source: PutSource::Fresh(allocation),
            },
        ])
        .unwrap();
    assert_eq!(write_ride(&store, CARD, store.sequence(), ride.id, 9999), Err(Error::Stale));
    assert_eq!(remove_ride(&store, CARD, store.sequence(), ride.id), Err(Error::Stale));
    reconcile(&store).unwrap();
    assert_eq!(write_ride(&store, CARD, store.sequence(), ride.id, 9999), Err(Error::Stale));
    rows.clear();
    read_rows(&store, |row| rows.push(row)).unwrap();
    assert!(rows.is_empty());
}

#[test]
fn policy_read_barrier_reconciles_live_remount_without_exposing_uncertain_stamps() {
    fn run(fail_sync: Option<u32>) -> u32 {
        let disk = SparseDisk::blank(BLOCKS, 13);
        let device = FaultOnce::new(&disk);
        let store = FlatStore::initialize(&device, CARD).unwrap();
        let ride = publish(&store, ObjectKind::Ride, b"ride");
        archive_ride(&store, CARD, ride.id, ride.revision, ride.payload_len, ride.payload_crc).unwrap();
        let before = disk.ledger().len();
        if let Some(skip) = fail_sync {
            device.fault_after(MediaOp::Sync, skip);
        }
        let result = write_ride(&store, CARD, store.sequence(), ride.id, 1234);
        let syncs = disk.ledger()[before..].iter().filter(|(_, op, _)| *op == MediaOp::Sync).count() as u32;
        if fail_sync.is_none() {
            result.unwrap();
            return syncs;
        }
        assert_eq!(result, Err(Error::RemountRequired));
        assert!(device.fired());
        // Do not reboot: the live device still exposes the gate whose sync was refused.
        drop(store);
        let mounted = FlatStore::mount(&device);
        let mut bytes = [0; MAX_LEN];
        let mut metadata = Metadata::new(&mounted);
        assert_eq!(metadata.load(&mounted, &mut bytes).unwrap().rows().next().unwrap().timestamp, 1234);
        device.fault_next(MediaOp::Sync);
        let mut seen = 0;
        assert_eq!(read_rows(&mounted, |_| seen += 1), Err(Error::RemountRequired));
        assert_eq!(seen, 0);
        assert!(matches!(mounted.allocate(1), Err(StoreError::ReadOnly)));
        drop(mounted);
        let mounted = FlatStore::mount(&device);
        read_rows(&mounted, |row| {
            seen += 1;
            assert_eq!(row.timestamp, 1234);
        })
        .unwrap();
        assert_eq!(seen, 1);
        drop(mounted);
        disk.reboot();
        let mounted = FlatStore::mount(&device);
        // Verify before another policy-read barrier can conceal loss.
        assert_eq!(Metadata::new(&mounted).load(&mounted, &mut bytes).unwrap().rows().next().unwrap().timestamp, 1234);
        remove_ride(&mounted, CARD, mounted.sequence(), ride.id).unwrap();
        assert!(matches!(mounted.open(ride.id, None), Err(StoreError::NotFound)));
        syncs
    }
    let syncs = run(None);
    run(Some(syncs - 1));
}

#[test]
fn invalid_metadata_or_nonfinal_heads_never_authorize_ride_policy() {
    let disk = SparseDisk::blank(BLOCKS, 14);
    let store = FlatStore::initialize(&disk, CARD).unwrap();
    let ride = publish(&store, ObjectKind::Ride, b"ride");
    archive_ride(&store, CARD, ride.id, ride.revision, ride.payload_len, ride.payload_crc).unwrap();
    store
        .commit(&[Mutation::Put { meta: EntryMeta { flags: EntryFlags::RETAINED, ..ride }, source: PutSource::Amend }])
        .unwrap();
    assert_eq!(write_ride(&store, CARD, store.sequence(), ride.id, 1234), Err(Error::Stale));
    assert_eq!(remove_ride(&store, CARD, store.sequence(), ride.id), Err(Error::Stale));
    let recording = publish(&store, ObjectKind::Ride, b"recording");
    store
        .commit(&[Mutation::Put {
            meta: EntryMeta { flags: EntryFlags::RECORDING, ..recording },
            source: PutSource::Amend,
        }])
        .unwrap();
    assert_eq!(write_ride(&store, CARD, store.sequence(), recording.id, 1234), Err(Error::Stale));
    assert_eq!(remove_ride(&store, CARD, store.sequence(), recording.id), Err(Error::Stale));
    let target = publish(&store, ObjectKind::Ride, b"finalized");
    let head = store.entries().find(|entry| entry.kind == ObjectKind::Metadata).unwrap();
    store.commit(&[Mutation::Remove { id: head.id, revision: head.revision }]).unwrap();
    publish(&store, ObjectKind::Metadata, b"corrupt");
    let mut seen = 0;
    assert_eq!(read_rows(&store, |_| seen += 1), Err(Error::Invalid));
    assert_eq!(seen, 0);
    assert_eq!(write_ride(&store, CARD, store.sequence(), target.id, 1234), Err(Error::Invalid));
    assert_eq!(remove_ride(&store, CARD, store.sequence(), target.id), Err(Error::Invalid));
}
