#[path = "../../../firmware/obc-fw-nrf54l/src/demo_rides.rs"]
mod demo_rides;

use obc_formats::io::SliceSource;
use obc_formats::ride::{encode_footer, Footer};
use obc_formats::track::{decode_record, RECORD_LEN};
use obc_storage::flat::{
    sim::SparseDisk, DisplayName, EntryFlags, EntryMeta, FlatStore, Mutation, ObjectId, ObjectKind, PutSource,
    Revision, Store, StoreError, StoreId,
};

fn finished_file(name: &str, start: u32) -> Vec<u8> {
    let sample = obc_formats::track::encode_record(&obc_ports::TrackPoint {
        lon: 7_800_000,
        lat: 48_000_000,
        ele: 200,
        t_ms: 0,
        segment_start: true,
        hr: None,
        cadence: None,
        power: None,
    });
    let footer = encode_footer(&Footer::new(name, start, 0, 0, 0, 0, 40, None, None, None, None, None));
    let mut bytes = sample.repeat(40);
    bytes.extend_from_slice(&footer);
    bytes
}

#[test]
fn finished_rides_survive_remount_with_distinct_sensor_data_and_no_duplicates() {
    let disk = SparseDisk::blank(131_072, 7);
    let store = FlatStore::initialize(&disk, StoreId([7; 16])).unwrap();
    let mut allocation = store.allocate(4).unwrap();
    store.write(&mut allocation, b"keep").unwrap();
    let existing = EntryMeta {
        id: store.next_object_id(),
        revision: Revision(1),
        kind: ObjectKind::Route,
        flags: EntryFlags::NONE,
        payload_len: 4,
        payload_crc: obc_crc::crc32(b"keep"),
        name: DisplayName::new("Existing route").unwrap(),
        added_at_utc: 123,
    };
    store.commit(&[Mutation::Put { meta: existing, source: PutSource::Fresh(allocation) }]).unwrap();
    let ids = demo_rides::seed(&store, 1_700_000_000).unwrap();
    let sequence = store.sequence();
    let store = FlatStore::mount(&disk);
    assert_eq!(demo_rides::seed(&store, 1_800_000_000).unwrap(), ids);
    assert_eq!(store.sequence(), sequence);
    assert_eq!(store.entries().find(|entry| entry.id == existing.id), Some(existing));
    let mut preserved = [0; 4];
    store.read(&store.open(existing.id, None).unwrap(), 0, &mut preserved).unwrap();
    assert_eq!(&preserved, b"keep");
    for (index, id) in ids.into_iter().enumerate() {
        let entry = store.entries().find(|entry| entry.id == id).unwrap();
        assert_eq!(entry.kind, ObjectKind::Ride);
        let mut bytes = vec![0; entry.payload_len as usize];
        let handle = store.open(id, None).unwrap();
        assert_eq!(store.read(&handle, 0, &mut bytes).unwrap(), bytes.len());
        assert_eq!(obc_crc::crc32(&bytes), entry.payload_crc);
        let info = obc_route::RideInfo::read(&SliceSource(&bytes)).unwrap();
        assert_eq!(info.name.as_str(), demo_rides::NAMES[index]);
        assert_eq!(info.moving_time_s, 1800);
        assert!((8_000..12_000).contains(&info.distance_m), "{} m", info.distance_m);
        assert_eq!(info.start_time, 1_700_000_000 + index as u32 * 86_400);
        assert_eq!(info.avg_hr.is_some(), index >= 1);
        assert_eq!(info.avg_power.is_some(), index == 2);
        assert_eq!(info.energy_kj.is_some(), index == 2);
        let points: Vec<_> = bytes[..info.point_count as usize * RECORD_LEN]
            .as_chunks::<RECORD_LEN>()
            .0
            .iter()
            .map(decode_record)
            .collect();
        assert_eq!(points.last().unwrap().t_ms, 1_800_000);
        for (i, point) in points.iter().enumerate() {
            assert_eq!(point.t_ms, i as u32 * 5000);
            assert_eq!(point.hr.is_some(), index >= 1);
            assert_eq!(point.power.is_some(), index == 2);
            assert_eq!(point.cadence, None);
            assert_eq!(point.segment_start, i == 0);
        }
    }
}

#[test]
fn unformatted_card_is_refused_without_writes() {
    let disk = SparseDisk::blank(131_072, 8);
    let store = FlatStore::mount(&disk);
    assert_eq!(demo_rides::seed(&store, 1_700_000_000), Err(StoreError::ReadOnly));
    assert_eq!(demo_rides::seed_file(&store, &finished_file("Kandel", 1_700_000_000)), Err(StoreError::ReadOnly));
    assert!(disk.write_log().is_empty());
}

#[test]
fn active_recording_is_refused_without_writes() {
    let disk = SparseDisk::blank(131_072, 9);
    let store = FlatStore::initialize(&disk, StoreId([9; 16])).unwrap();
    let allocation = store.allocate(1024 * 1024).unwrap();
    store
        .commit(&[Mutation::Put {
            meta: EntryMeta {
                id: ObjectId(1),
                revision: Revision(1),
                kind: ObjectKind::Ride,
                flags: EntryFlags::RECORDING,
                payload_len: 0,
                payload_crc: 0,
                name: DisplayName::new("Recording").unwrap(),
                added_at_utc: 0,
            },
            source: PutSource::Fresh(allocation),
        }])
        .unwrap();
    let writes = disk.write_log().len();
    assert_eq!(demo_rides::seed(&store, 1_700_000_000), Err(StoreError::Busy));
    assert_eq!(demo_rides::seed_file(&store, &finished_file("Kandel", 1_700_000_000)), Err(StoreError::Busy));
    assert_eq!(disk.write_log().len(), writes);
}

#[test]
fn file_seed_survives_remount_and_preserves_existing_objects() {
    let disk = SparseDisk::blank(131_072, 10);
    let store = FlatStore::initialize(&disk, StoreId([10; 16])).unwrap();
    let mut allocation = store.allocate(4).unwrap();
    store.write(&mut allocation, b"keep").unwrap();
    let existing = EntryMeta {
        id: store.next_object_id(),
        revision: Revision(1),
        kind: ObjectKind::Route,
        flags: EntryFlags::NONE,
        payload_len: 4,
        payload_crc: obc_crc::crc32(b"keep"),
        name: DisplayName::new("Existing route").unwrap(),
        added_at_utc: 123,
    };
    store.commit(&[Mutation::Put { meta: existing, source: PutSource::Fresh(allocation) }]).unwrap();

    let bytes = finished_file("Kandel", 1_700_000_000);
    let id = demo_rides::seed_file(&store, &bytes).unwrap();
    let sequence = store.sequence();
    let store = FlatStore::mount(&disk);
    assert_eq!(demo_rides::seed_file(&store, &bytes), Ok(id));
    assert_eq!(store.sequence(), sequence);
    assert_eq!(store.entries().find(|entry| entry.id == existing.id), Some(existing));
    let mut preserved = [0; 4];
    store.read(&store.open(existing.id, None).unwrap(), 0, &mut preserved).unwrap();
    assert_eq!(&preserved, b"keep");
    let entry = store.entries().find(|entry| entry.id == id).unwrap();
    assert_eq!(entry.payload_crc, obc_crc::crc32(&bytes));
    let mut stored = vec![0; bytes.len()];
    store.read(&store.open(id, None).unwrap(), 0, &mut stored).unwrap();
    assert_eq!(stored, bytes);
    assert_eq!(obc_route::RideInfo::read(&SliceSource(&stored)).unwrap().name.as_str(), "Kandel");
}

#[test]
fn file_seed_refuses_invalid_or_conflicting_payload_without_writes() {
    let disk = SparseDisk::blank(131_072, 11);
    let store = FlatStore::initialize(&disk, StoreId([11; 16])).unwrap();
    let bytes = finished_file("Kandel", 1_700_000_000);
    let mut invalid = bytes.clone();
    invalid.pop();
    let mut reserved_flags = bytes.clone();
    reserved_flags[10] = 2;
    let mut bad_latitude = bytes.clone();
    bad_latitude[4..8].copy_from_slice(&91_000_000i32.to_le_bytes());
    let mut backward_time = bytes.clone();
    backward_time[RECORD_LEN + 12..RECORD_LEN + 16].copy_from_slice(&1u32.to_le_bytes());
    let writes = disk.write_log().len();
    for invalid in [&invalid, &reserved_flags, &bad_latitude, &backward_time] {
        assert_eq!(demo_rides::seed_file(&store, invalid), Err(StoreError::Invalid));
        assert_eq!(disk.write_log().len(), writes);
    }

    demo_rides::seed_file(&store, &bytes).unwrap();
    let conflicting = finished_file("Kandel", 1_800_000_000);
    let writes = disk.write_log().len();
    assert_eq!(demo_rides::seed_file(&store, &conflicting), Err(StoreError::Invalid));
    assert_eq!(disk.write_log().len(), writes);
}
