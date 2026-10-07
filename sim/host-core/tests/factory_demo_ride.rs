#[path = "../../../firmware/obc-fw-nrf54l/src/factory_demo_ride.rs"]
mod factory_demo_ride;

use obc_formats::io::SliceSource;
use obc_formats::ride::{encode_footer, Footer};
use obc_formats::track::RECORD_LEN;
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
    let mut footer = Footer::new(name, start, 0, 0, 0, 0, 40, None, None, None, None, None);
    footer.is_demo = true;
    let footer = encode_footer(&footer);
    let mut bytes = sample.repeat(40);
    bytes.extend_from_slice(&footer);
    bytes
}

#[test]
fn unformatted_card_is_refused_without_writes() {
    let disk = SparseDisk::blank(131_072, 8);
    let store = FlatStore::mount(&disk);
    assert_eq!(factory_demo_ride::seed(&store, &finished_file("Kandel", 1_700_000_000)), Err(StoreError::ReadOnly));
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
    assert_eq!(factory_demo_ride::seed(&store, &finished_file("Kandel", 1_700_000_000)), Err(StoreError::Busy));
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
    let id = factory_demo_ride::seed(&store, &bytes).unwrap();
    let sequence = store.sequence();
    let store = FlatStore::mount(&disk);
    assert_eq!(factory_demo_ride::seed(&store, &bytes), Ok(id));
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
    let mut not_demo = bytes.clone();
    let flags = not_demo.len() - obc_formats::ride::FOOTER_LEN + 33;
    not_demo[flags] = 0;
    let writes = disk.write_log().len();
    for invalid in [&invalid, &reserved_flags, &bad_latitude, &backward_time, &not_demo] {
        assert_eq!(factory_demo_ride::seed(&store, invalid), Err(StoreError::Invalid));
        assert_eq!(disk.write_log().len(), writes);
    }

    factory_demo_ride::seed(&store, &bytes).unwrap();
    let conflicting = finished_file("Kandel", 1_800_000_000);
    let writes = disk.write_log().len();
    assert_eq!(factory_demo_ride::seed(&store, &conflicting), Err(StoreError::Invalid));
    assert_eq!(disk.write_log().len(), writes);
}
