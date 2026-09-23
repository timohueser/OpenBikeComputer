use super::*;
use crate::flat::sim::{FaultOnce, FaultPlan, MediaOp, SparseDisk, When};
use std::vec::Vec;

const CARD: StoreId = StoreId([0x42; 16]);
const BLOCKS: u64 = 200_000;

fn publish<D: BlockDevice>(store: &FlatStore<D>, kind: ObjectKind, bytes: &[u8]) -> EntryMeta {
    let mut allocation = store.allocate(bytes.len() as u64).unwrap();
    store.write(&mut allocation, bytes).unwrap();
    let meta = EntryMeta {
        added_at_utc: 0,
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
    }
}

#[test]
fn normative_vector_is_exact_and_hostile_records_are_refused() {
    let vector = include_bytes!("../../../../../specs/vectors/ride-archive-metadata/two-rides.bin");
    let mut bytes = [0; MAX_LEN];
    let mut image = Image::empty(CARD, &mut bytes).unwrap();
    image
        .set(Row {
            id: ObjectId(0x100000001),
            revision: Revision(0x200000003),
            payload_len: 0x300000004,
            payload_crc: 0x12345678,
            timestamp: 0x65000000,
            kind: ObjectKind::Ride,
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
        })
        .unwrap();
    assert_eq!(image.bytes(), vector);
    for (offset, value) in [
        (4, 1),
        (8, 41),
        (12, 1),
        (10, 3),
        (HEADER_LEN + 34, 6),
        (HEADER_LEN + ROW_LEN + 34, 1),
        (HEADER_LEN + 35, 1),
        (HEADER_LEN + 32, 1),
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
    for id in 1..=(MAX_RIDES) as u64 {
        image
            .set(Row {
                id: ObjectId(id),
                revision: Revision(1),
                payload_len: 1,
                payload_crc: 0,
                timestamp: 0,
                kind: ObjectKind::Ride,
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
    let route = publish(&store, ObjectKind::Ride, b"route");
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
    let route = publish(&store, ObjectKind::Ride, b"route");
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
    let route = publish(&store, ObjectKind::Ride, b"route");
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
    let route = publish(&store, ObjectKind::Ride, b"route");
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
    let route = publish(&store, ObjectKind::Ride, b"route");
    let mut handles = Vec::new();
    for _ in 0..crate::flat::store::MAX_OPEN_OBJECTS {
        let object = publish(&store, ObjectKind::Ride, b"ride");
        handles.push(store.open(object.id, Some(object.revision)).unwrap());
    }
    let sequence = store.sequence();
    assert_eq!(write_proof(&store, route), Err(Error::Store(StoreError::Busy)));
    assert_eq!(store.sequence(), sequence);
    assert!(store.mode().writable());
    store.close(handles.pop().unwrap());
    write_proof(&store, route).unwrap();
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
        let target = publish(&store, ObjectKind::Ride, b"route");
        let sequence = store.sequence();
        let before = disk.ledger().len();
        if let Some(skip) = fail_read {
            device.fault_after(MediaOp::Read, skip);
        }
        let result = write_proof(&store, target);
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

fn write_proof<D: BlockDevice>(store: &FlatStore<D>, source: EntryMeta) -> Result<EntryMeta, Error> {
    let mut owner = Metadata::new(store);
    let mut bytes = [0; MAX_LEN];
    let mut image = owner.load(store, &mut bytes)?;
    image.set(row(source))?;
    owner.replace(store, &mut image, Some(source))
}

fn checkpoint(route: EntryMeta, original: Option<EntryMeta>) -> NavigatorCheckpoint {
    NavigatorCheckpoint {
        route: fingerprint(route),
        original: original.map(fingerprint),
        progress_m: 10,
        occurrence: 2,
        lon: 8_000_000,
        lat: 47_000_000,
        phase: obc_formats::assistant::JourneyPhase::Following,
        unresolved_avoidance: false,
        selection: false,
        lower_m: 0,
        upper_m: 100,
    }
}

#[test]
fn checkpoint_rows_reconcile_and_capacity_preserve_each_other() {
    let mut bytes = [0; MAX_LEN];
    let mut image = Image::empty(CARD, &mut bytes).unwrap();
    let cp = NavigatorCheckpoint {
        route: PayloadFingerprint { object: 1, revision: 1, length: 1, crc: 0 },
        original: None,
        progress_m: 0,
        occurrence: 0,
        lon: 0,
        lat: 0,
        phase: obc_formats::assistant::JourneyPhase::Following,
        unresolved_avoidance: false,
        selection: false,
        lower_m: 0,
        upper_m: 1,
    };
    image.set_checkpoint(Some(cp)).unwrap();
    for id in 1..=MAX_RIDES as u64 {
        image
            .set(Row {
                id: ObjectId(id),
                revision: Revision(1),
                payload_len: 1,
                payload_crc: 0,
                timestamp: 0,
                kind: ObjectKind::Ride,
            })
            .unwrap();
    }
    assert_eq!(image.bytes().len(), 5248);
    assert_eq!(image.checkpoint(), Some(cp));
    let len = image.bytes().len();
    assert_eq!(Image::decode(&mut bytes, len).unwrap().checkpoint(), Some(cp));
    let mut image = Image::decode(&mut bytes, len).unwrap();
    image.set_checkpoint(None).unwrap();
    assert_eq!(image.bytes().len(), 5152);
    assert_eq!(image.rows().filter(|r| r.kind == ObjectKind::Ride).count(), 128);
}

#[test]
fn archive_and_checkpoint_writes_share_current_image_and_exact_target_validation() {
    let disk = SparseDisk::blank(BLOCKS, 30);
    let store = FlatStore::initialize(&disk, CARD).unwrap();
    store.set_route_added_at(Some(10));
    let route = publish(&store, ObjectKind::Route, b"derived route");
    let original = publish(&store, ObjectKind::Route, b"original route");
    let ride = publish(&store, ObjectKind::Ride, b"archived ride");
    let cp = checkpoint(route, Some(original));
    let mut owner = Metadata::new(&store);
    let mut draft_bytes = [0; MAX_LEN];
    let mut stale = owner.load(&store, &mut draft_bytes).unwrap();
    stale.set_checkpoint(Some(cp)).unwrap();
    archive_ride(&store, CARD, ride.id, ride.revision, ride.payload_len, ride.payload_crc).unwrap();
    assert_eq!(owner.replace_checkpoint(&store, &mut stale), Err(Error::Stale));
    write_checkpoint(&store, CARD, store.sequence(), None, Some(cp)).unwrap();
    archive_ride(&store, CARD, ride.id, ride.revision, ride.payload_len, ride.payload_crc).unwrap();
    assert_eq!(read_checkpoint(&store), Ok(Some(cp)));
    assert_eq!(check_route_change(&store, original.id), Err(Error::Store(StoreError::Busy)));
    assert!(crate::flat::route_cleanup::next(&store, 20, None).unwrap().is_none());
    let accepted = store.entries().find(|entry| entry.id == route.id).unwrap();
    assert_eq!(
        store.commit(&[Mutation::Put {
            meta: EntryMeta { payload_crc: accepted.payload_crc ^ 1, ..accepted },
            source: PutSource::Amend
        }]),
        Err(StoreError::Invalid)
    );
    assert_eq!(check_route_change(&store, route.id), Err(Error::Store(StoreError::Busy)));
    let mut rows = Vec::new();
    read_rows(&store, |row| rows.push(row)).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(store.entries().find(|entry| entry.id == route.id).unwrap().flags.has(EntryFlags::ASSISTANT_ACCEPTED));
    assert_eq!(rows.iter().find(|r| r.id == ride.id).unwrap().timestamp, 0);
    let mut observed = Vec::new();
    assert_eq!(census(&store, |row| observed.push(row)), Ok(CARD), "the proof names the card it lives on");
    assert_eq!(observed, rows, "observation and policy read the same durable rows");
    let changed = NavigatorCheckpoint { route: PayloadFingerprint { crc: cp.route.crc ^ 1, ..cp.route }, ..cp };
    assert_eq!(write_checkpoint(&store, CARD, store.sequence(), Some(cp), Some(changed)), Err(Error::Stale));
    write_checkpoint(&store, CARD, store.sequence(), Some(cp), None).unwrap();
    assert_eq!(read_checkpoint(&store), Ok(None));
    assert_eq!(check_route_change(&store, original.id), Ok(()));
    assert!(crate::flat::route_cleanup::next(&store, 20, Some(original.id)).unwrap().is_some());
    let mut after = Vec::new();
    read_rows(&store, |row| after.push(row)).unwrap();
    assert_eq!(after, rows);
    assert!(store.entries().find(|entry| entry.id == route.id).unwrap().flags.has(EntryFlags::ASSISTANT_ACCEPTED));
    let mut allocation = store.allocate(3).unwrap();
    store.write(&mut allocation, b"new").unwrap();
    let replacement = EntryMeta {
        revision: Revision(route.revision.0 + 1),
        payload_len: 3,
        payload_crc: store.allocation_crc(&allocation).unwrap(),
        ..route
    };
    store
        .commit(&[
            Mutation::Remove { id: route.id, revision: route.revision },
            Mutation::Put { meta: replacement, source: PutSource::Fresh(allocation) },
        ])
        .unwrap();
    assert!(
        !store.entries().find(|entry| entry.id == route.id).unwrap().flags.has(EntryFlags::ASSISTANT_ACCEPTED),
        "a new fingerprint never inherits acceptance"
    );
}

#[test]
fn every_checkpoint_publication_cut_recovers_a_complete_prior_or_accepted_image() {
    fn run(cut: Option<(u32, When)>, clear: bool) -> Vec<(u32, MediaOp, u64)> {
        let disk = SparseDisk::blank(BLOCKS, 31);
        let store = FlatStore::initialize(&disk, CARD).unwrap();
        let route = publish(&store, ObjectKind::Route, b"accepted bytes");
        let ride = publish(&store, ObjectKind::Ride, b"archive proof");
        archive_ride(&store, CARD, ride.id, ride.revision, ride.payload_len, ride.payload_crc).unwrap();
        let cp = checkpoint(route, None);
        if clear {
            write_checkpoint(&store, CARD, store.sequence(), None, Some(cp)).unwrap();
        }
        let expected = clear.then_some(cp);
        let next = (!clear).then_some(cp);
        let before = disk.ledger().last().map_or(0, |row| row.0);
        if let Some((op, when)) = cut {
            disk.plan(FaultPlan { op, when });
        }
        let result = write_checkpoint(&store, CARD, store.sequence(), expected, next);
        let operations = disk.ledger().into_iter().filter(|row| row.0 > before).collect::<Vec<_>>();
        if matches!(result, Err(Error::RemountRequired)) {
            assert_eq!(write_checkpoint(&store, CARD, store.sequence(), None, Some(cp)), Err(Error::RemountRequired));
        }
        disk.reboot();
        let reopened = FlatStore::mount(&disk);
        let recovered = read_checkpoint(&reopened).unwrap();
        assert!(recovered.is_none() || recovered == Some(cp));
        if result.is_ok() {
            assert_eq!(recovered, next);
        }
        let mut rows = Vec::new();
        read_rows(&reopened, |row| rows.push(row)).unwrap();
        assert_eq!(rows.iter().find(|row| row.id == ride.id).unwrap().timestamp, 0);
        let accepted =
            reopened.entries().find(|entry| entry.id == route.id).unwrap().flags.has(EntryFlags::ASSISTANT_ACCEPTED);
        assert_eq!(
            accepted,
            clear || recovered.is_some(),
            "acceptance marker and checkpoint publish atomically; clear preserves acceptance"
        );
        operations
    }
    for clear in [false, true] {
        let operations = run(None, clear);
        for (op, _, _) in operations {
            for when in [When::Before, When::After] {
                run(Some((op, when)), clear);
            }
        }
    }
}

#[test]
fn corrupt_checkpoint_metadata_does_not_block_unrelated_ride_mutations() {
    let disk = SparseDisk::blank(BLOCKS, 32);
    let store = FlatStore::initialize(&disk, CARD).unwrap();
    let route = publish(&store, ObjectKind::Route, b"route");
    let ride = publish(&store, ObjectKind::Ride, b"ride");
    publish(&store, ObjectKind::Metadata, b"invalid image");
    assert_eq!(check_route_change(&store, ride.id), Ok(()));
    assert_eq!(check_route_change(&store, route.id), Err(Error::Invalid));
    assert!(crate::flat::route_cleanup::next(&store, 20, None).is_err());
}

use obc_formats::trip_progress::RouteVersion;

fn progress(key: u64) -> TripProgress {
    TripProgress {
        key,
        day: 1,
        day_route: RouteVersion { id: 7, revision: 0 },
        metres: 20_000,
        last_finished: Some(0),
        dates: [0; obc_formats::trip_progress::MAX_DAYS],
    }
}

#[test]
fn progress_records_survive_row_and_checkpoint_edits_and_a_remount() {
    let disk = SparseDisk::blank(BLOCKS, 1);
    let store = FlatStore::initialize(&disk, CARD).unwrap();
    let route = publish(&store, ObjectKind::Route, b"route");
    let ride = publish(&store, ObjectKind::Ride, b"ride");
    let at_route =
        |key, revision| TripProgress { day_route: RouteVersion { id: route.id.0, revision }, ..progress(key) };
    write_progress(&store, progress(3), |_| true).unwrap();
    write_progress(&store, at_route(1, 0), |_| true).unwrap();
    write_progress(&store, at_route(5, 9), |_| true).unwrap();
    write_proof(&store, ride).unwrap();
    write_checkpoint(&store, CARD, store.sequence(), None, Some(checkpoint(route, None))).unwrap();
    disk.reboot();
    let store = FlatStore::mount(&disk);
    let mut read = Vec::new();
    read_progress(&store, |p| read.push(p)).unwrap();
    let stamped = at_route(1, 1);
    let gone = TripProgress { metres: 0, day_route: RouteVersion { id: 7, revision: 0 }, ..progress(3) };
    let kept = TripProgress { metres: 0, ..at_route(5, 9) };
    assert_eq!(
        read,
        [gone.clone(), stamped.clone(), kept],
        "write order stays; a record without a Revision takes its route's, and one with a Revision keeps it"
    );
    let mut bytes = [0; MAX_LEN];
    let mut image = Metadata::new(&store).load(&store, &mut bytes).unwrap();
    assert_eq!(image.rows().count(), 1);
    image.set_checkpoint(None).unwrap();
    assert_eq!(image.progress().map(|p| p.key).collect::<Vec<_>>(), [3, 1, 5]);
    assert_eq!(image.set_progress(&[progress(3), progress(3)]), Err(Error::Invalid), "one record per key");

    let len = image.bytes().len();
    let mut torn = image.bytes()[..len - 1].to_vec();
    assert!(Image::decode(&mut torn, len - 1).is_err());
    let mut duplicate = image.bytes().to_vec();
    duplicate[len - RECORD_LEN..len - RECORD_LEN + 8].copy_from_slice(&3u64.to_le_bytes());
    assert!(Image::decode(&mut duplicate, len).is_err());
}
