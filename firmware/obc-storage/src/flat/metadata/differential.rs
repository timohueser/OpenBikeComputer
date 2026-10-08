use super::*;
use crate::flat::sim::{FaultPlan, MediaOp, SparseDisk, When, EVERY_WHEN};
use std::{collections::BTreeMap, format, string::String, vec::Vec};

const CARD: StoreId = StoreId([0x42; 16]);

fn publish(store: &FlatStore<&SparseDisk>, kind: ObjectKind, bytes: &[u8]) -> EntryMeta {
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

#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    results: Vec<String>,
    image: BTreeMap<u64, [u8; 512]>,
    writes: Vec<(MediaOp, u64)>,
}

// Cuts name writes and barriers, not reads: streaming changes the read command count.
fn script(cut: Option<(usize, When)>) -> Outcome {
    let disk = SparseDisk::blank(200_000, 9);
    let store = FlatStore::initialize(&disk, CARD).unwrap();
    let route = publish(&store, ObjectKind::Route, b"route");
    let ride = publish(&store, ObjectKind::Ride, b"ride");
    let checkpoint = super::tests::checkpoint(route, None);
    let progress = TripProgress {
        day_route: obc_formats::trip_progress::RouteVersion { id: route.id.0, revision: 0 },
        ..super::tests::progress(1)
    };
    let start = disk.ops();
    let mut results = Vec::new();
    macro_rules! call {
        ($name:ident($($arg:expr),*)) => {
            format!("{:?}", super::$name($($arg),*))
        };
    }
    if let Some((ordinal, when)) = cut {
        let mut op = start;
        // Recover operation indices from the successful trace for this implementation.
        let trace = trace_script();
        for (index, kind, _) in trace {
            if kind != MediaOp::Read {
                if op == start + ordinal as u32 {
                    disk.plan(FaultPlan { op: index, when });
                    break;
                }
                op += 1;
            }
        }
    }
    for step in 0..7 {
        let result = match step {
            0 | 1 => call!(archive_ride(&store, CARD, ride.id, ride.revision, ride.payload_len, ride.payload_crc)),
            2 => call!(write_checkpoint(&store, CARD, store.sequence(), None, Some(checkpoint))),
            3 => call!(write_progress(&store, progress.clone(), |_| true)),
            4 => call!(write_checkpoint(&store, CARD, store.sequence(), Some(checkpoint), None)),
            5 => format!("{:?}", store.commit(&[Mutation::Remove { id: ride.id, revision: ride.revision }])),
            _ => call!(reconcile(&store)),
        };
        let failed = result.starts_with("Err");
        results.push(result);
        if failed {
            break;
        }
    }
    let writes = disk
        .ledger()
        .into_iter()
        .filter(|(op, kind, _)| *op > start && *kind != MediaOp::Read)
        .map(|(_, kind, width)| (kind, width))
        .collect();
    let mut image = BTreeMap::new();
    for (_, lba, blocks) in disk.write_log() {
        for block in lba..lba + blocks {
            let bytes = disk.block(block);
            if bytes != [0; 512] {
                image.insert(block, bytes);
            }
        }
    }
    disk.reboot();
    let reopened = FlatStore::mount(&disk);
    results.push(call!(read_checkpoint(&reopened)));
    let mut rows = Vec::new();
    let result = census(&reopened, |row| rows.push(format!("{row:?}"))).map_err(|e| format!("{e:?}"));
    results.push(format!("{result:?}:{rows:?}"));
    let mut progress = Vec::new();
    let result = read_progress(&reopened, |record| progress.push(record)).map_err(|e| format!("{e:?}"));
    results.push(format!("{result:?}:{progress:?}"));
    let mut retained = Vec::new();
    let result = read_rows(&reopened, |row| retained.push(format!("{row:?}"))).map_err(|e| format!("{e:?}"));
    results.push(format!("{result:?}:{retained:?}"));
    Outcome { results, image, writes }
}

fn trace_script() -> Vec<(u32, MediaOp, u64)> {
    let disk = SparseDisk::blank(200_000, 9);
    let store = FlatStore::initialize(&disk, CARD).unwrap();
    let route = publish(&store, ObjectKind::Route, b"route");
    let ride = publish(&store, ObjectKind::Ride, b"ride");
    let start = disk.ops();
    let checkpoint = super::tests::checkpoint(route, None);
    let progress = TripProgress {
        day_route: obc_formats::trip_progress::RouteVersion { id: route.id.0, revision: 0 },
        ..super::tests::progress(1)
    };
    macro_rules! run {
        ($name:ident($($arg:expr),*)) => {
            super::$name($($arg),*).unwrap();
        };
    }
    run!(archive_ride(&store, CARD, ride.id, ride.revision, ride.payload_len, ride.payload_crc));
    run!(archive_ride(&store, CARD, ride.id, ride.revision, ride.payload_len, ride.payload_crc));
    run!(write_checkpoint(&store, CARD, store.sequence(), None, Some(checkpoint)));
    run!(write_progress(&store, progress, |_| true));
    run!(write_checkpoint(&store, CARD, store.sequence(), Some(checkpoint), None));
    store.commit(&[Mutation::Remove { id: ride.id, revision: ride.revision }]).unwrap();
    run!(reconcile(&store));
    disk.ledger().into_iter().filter(|(op, _, _)| *op > start).collect()
}

fn fingerprint(crc: &mut obc_crc::Crc32, outcome: &Outcome) {
    for result in &outcome.results {
        crc.update(&(result.len() as u64).to_le_bytes());
        crc.update(result.as_bytes());
    }
    crc.update(&(outcome.image.len() as u64).to_le_bytes());
    for (lba, block) in &outcome.image {
        crc.update(&lba.to_le_bytes());
        crc.update(block);
    }
    for (kind, width) in &outcome.writes {
        crc.update(&[match kind {
            MediaOp::Write => 1,
            MediaOp::Sync => 2,
            MediaOp::Read => 0,
        }]);
        crc.update(&width.to_le_bytes());
    }
}

#[test]
fn operation_scripts_and_every_write_cut_match_the_card_oracle() {
    let mut crc = obc_crc::Crc32::new();
    let mut cases = 0;
    let mut compare = |cut| {
        let new = script(cut);
        fingerprint(&mut crc, &new);
        cases += 1;
    };
    compare(None);
    let trace = trace_script();
    for (ordinal, (_, kind, width)) in trace.into_iter().filter(|(_, kind, _)| *kind != MediaOp::Read).enumerate() {
        for when in EVERY_WHEN {
            compare(Some((ordinal, when)));
        }
        if kind == MediaOp::Write {
            for blocks in 0..width as u32 {
                for (tear, durable) in [(true, true), (true, false), (false, true)] {
                    compare(Some((ordinal, When::Inside { blocks, tear, durable })));
                }
            }
        }
    }
    assert_eq!((cases, crc.finalize()), (229, 0xe1fc040d));
}

#[test]
fn malformed_trailing_records_never_expose_an_earlier_row() {
    let mut bytes = [0; MAX_LEN];
    let mut image = Image::empty(CARD, &mut bytes).unwrap();
    for id in 1..=MAX_RIDES as u64 {
        image
            .set(Row {
                id: ObjectId(id),
                revision: Revision(1),
                payload_len: 4,
                payload_crc: 0,
                timestamp: 1234,
                kind: ObjectKind::Ride,
            })
            .unwrap();
    }
    let records: obc_formats::trip_progress::Records = (1..=MAX_RECORDS as u64).map(super::tests::progress).collect();
    image.set_progress(&records).unwrap();
    let valid = image.bytes().to_vec();
    let last_row = HEADER_LEN + (MAX_RIDES - 1) * ROW_LEN;
    for offset in [0, 4, 6, 8, 10, 12, 14, last_row + 32, last_row + 34, valid.len() - RECORD_LEN] {
        let mut malformed = valid.clone();
        malformed[offset] = 0;
        if matches!(offset, 12 | 14) || offset == last_row + 34 {
            malformed[offset] = 1;
        }
        let disk = SparseDisk::blank(200_000, 11);
        let store = FlatStore::initialize(&disk, CARD).unwrap();
        publish(&store, ObjectKind::Metadata, &malformed);
        let mut rows = Vec::new();
        assert_eq!(census(&store, |row| rows.push(row)), Err(Error::Invalid), "offset {offset}");
        assert!(rows.is_empty(), "offset {offset}");
    }
}
