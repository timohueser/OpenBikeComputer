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

/// What a remount serves from the metadata image, and whether the route it binds is accepted.
#[derive(Debug, PartialEq)]
struct Served {
    checkpoint: Result<Option<NavigatorCheckpoint>, Error>,
    census: Result<Vec<Row>, Error>,
    progress: Result<Vec<TripProgress>, Error>,
    policy: Result<Vec<Row>, Error>,
    accepted: bool,
}

const STEPS: usize = 7;

/// Runs the first `steps` steps of the script with an optional power cut on the `ordinal`th write
/// or sync of `trace`, then remounts. Returns the step that failed, if one did, and what the
/// remount serves.
fn run(steps: usize, cut: Option<(usize, When)>, trace: &[(u32, MediaOp, u64)]) -> (Option<usize>, Served) {
    let disk = SparseDisk::blank(200_000, 9);
    let store = FlatStore::initialize(&disk, CARD).unwrap();
    let route = publish(&store, ObjectKind::Route, b"route");
    let ride = publish(&store, ObjectKind::Ride, b"ride");
    let checkpoint = super::tests::checkpoint(route, None);
    let progress = TripProgress {
        day_route: obc_formats::trip_progress::RouteVersion { id: route.id.0, revision: 0 },
        ..super::tests::progress(1)
    };
    if let Some((ordinal, when)) = cut {
        let (op, _, _) = trace.iter().filter(|(_, kind, _)| *kind != MediaOp::Read).nth(ordinal).unwrap();
        disk.plan(FaultPlan { op: *op, when });
    }
    let failed = (0..steps).find(|step| match step {
        0 | 1 => archive_ride(&store, CARD, ride.id, ride.revision, ride.payload_len, ride.payload_crc).is_err(),
        2 => write_checkpoint(&store, CARD, store.sequence(), None, Some(checkpoint)).is_err(),
        3 => write_progress(&store, progress.clone(), |_| true).is_err(),
        4 => write_checkpoint(&store, CARD, store.sequence(), Some(checkpoint), None).is_err(),
        5 => store.commit(&[Mutation::Remove { id: ride.id, revision: ride.revision }]).is_err(),
        _ => reconcile(&store).is_err(),
    });

    disk.reboot();
    let store = FlatStore::mount(&disk);
    assert_eq!(store.mode(), crate::flat::Mode::ReadWrite, "cut {cut:?} left a read-only card");
    let mut rows = Vec::new();
    let mut records = Vec::new();
    let mut kept = Vec::new();
    let route = store.find_entry(|entry| entry.id == route.id && entry.flags.is_route_head()).unwrap().unwrap();
    let served = Served {
        checkpoint: read_checkpoint(&store),
        census: census(&store, |row| rows.push(row)).map(|_| rows),
        progress: read_progress(&store, |record| records.push(record)).map(|_| records),
        policy: read_rows(&store, |row| kept.push(row)).map(|_| kept),
        accepted: route.flags.has(EntryFlags::ASSISTANT_ACCEPTED),
    };
    (failed, served)
}

/// Every write and sync of a metadata script, cut before, during, after and inside: the remount is
/// writable and serves the state before the step the cut failed or the state after it, never a mix.
#[test]
fn every_cut_of_a_metadata_script_serves_the_state_before_or_after_its_step() {
    let trace = trace_script();
    let states: Vec<Served> = (0..=STEPS)
        .map(|steps| {
            let (failed, served) = run(steps, None, &trace);
            assert_eq!(failed, None, "the fault-free script failed within {steps} steps");
            served
        })
        .collect();
    let ids = |rows: &Result<Vec<Row>, Error>| rows.clone().map(|rows| rows.iter().map(|row| row.id.0).collect());
    let keys = |served: &Served| served.progress.clone().map(|records| records.iter().map(|r| r.key).collect());
    // The route is object 1 and the ride object 2.
    let nothing = Served {
        checkpoint: Ok(None),
        census: Ok(Vec::new()),
        progress: Ok(Vec::new()),
        policy: Ok(Vec::new()),
        accepted: false,
    };
    assert_eq!(states[0], nothing);
    assert_eq!(ids(&states[1].census), Ok(std::vec![2]), "the archive receipt wrote no row");
    assert_eq!(states[2], states[1], "a duplicate receipt changed the image");
    assert!(matches!(states[3].checkpoint, Ok(Some(c)) if c.route.object == 1) && states[3].accepted);
    assert_eq!(keys(&states[4]), Ok(std::vec![1]), "the progress record was not written");
    assert!(states[5].checkpoint == Ok(None) && states[5].accepted, "clearing the checkpoint un-accepted the route");
    assert_eq!((ids(&states[6].census), ids(&states[6].policy)), (Ok(std::vec![2]), Ok(Vec::new())));
    let last = &states[STEPS];
    assert_eq!((&last.checkpoint, ids(&last.census), ids(&last.policy)), (&Ok(None), Ok(Vec::new()), Ok(Vec::new())));
    assert!(keys(last) == Ok(std::vec![1]) && last.accepted, "reconcile lost the progress or the acceptance");

    let mut cut_steps = std::collections::BTreeSet::new();
    for (ordinal, (_, kind, width)) in trace.iter().filter(|(_, kind, _)| *kind != MediaOp::Read).enumerate() {
        let mut whens = EVERY_WHEN.to_vec();
        if *kind == MediaOp::Write {
            for blocks in 0..*width as u32 {
                for (tear, durable) in [(true, true), (true, false), (false, true)] {
                    whens.push(When::Inside { blocks, tear, durable });
                }
            }
        }
        for when in whens {
            let (failed, served) = run(STEPS, Some((ordinal, when)), &trace);
            let admissible = failed.map_or(&states[STEPS..], |step| &states[step..=step + 1]);
            assert!(
                admissible.contains(&served),
                "a cut {when:?} on write or sync {ordinal} failed step {failed:?} and served {served:?}",
            );
            cut_steps.extend(failed);
        }
    }
    assert_eq!(cut_steps.into_iter().collect::<Vec<_>>(), (0..STEPS).collect::<Vec<_>>(), "a step was never cut");
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
