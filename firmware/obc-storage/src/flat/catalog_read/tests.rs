use crate::flat::{sim::*, *};
use obc_formats::io::{ByteSink, Error};
use obc_route::{RouteSummary, TripMeta};
use std::vec::Vec;

#[derive(Clone, Copy)]
struct Head {
    id: ObjectId,
    revision: Revision,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Routes {
    summaries: Vec<RouteSummary>,
    ids: Vec<u64>,
    candidates: u64,
    internal: u64,
    temporary: u64,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Trips {
    metas: Vec<TripMeta>,
    ids: Vec<u64>,
}

fn retain_newest<const N: usize>(entries: &mut heapless::Vec<Head, N>, entry: Head) {
    let at = entries.iter().position(|old| entry.id > old.id).unwrap_or(entries.len());
    if at >= N {
        return;
    }
    if entries.is_full() {
        let _ = entries.pop();
    }
    let _ = entries.insert(at, entry);
}

// The captured board loader stages are the oracle; App publishes them only after success.
fn original_routes<D: BlockDevice>(store: &FlatStore<D>) -> Result<Routes, ()> {
    let mut heads: heapless::Vec<Head, 64> = heapless::Vec::new();
    for entry in store.entries().filter(|entry| entry.kind == ObjectKind::Route && entry.flags.is_route_head()) {
        retain_newest(&mut heads, Head { id: entry.id, revision: entry.revision });
    }
    if !store.entries_ok() {
        return Err(());
    }
    let mut accepted = 0u64;
    for meta in store.entries().filter(|meta| meta.flags.has(EntryFlags::ASSISTANT_ACCEPTED)) {
        if let Some(index) = heads.iter().position(|head| head.id == meta.id && head.revision == meta.revision) {
            accepted |= 1 << index;
        }
    }
    if !store.entries_ok() {
        return Err(());
    }
    let mut routes = Routes::default();
    for (index, entry) in heads.into_iter().enumerate() {
        match store.with_source(entry.id, Some(entry.revision), |source| RouteSummary::read_with_flags(source)) {
            Ok(Ok((summary, flags))) => {
                if obc_formats::obcr::disposable_navigation(flags) {
                    routes.temporary |= 1 << routes.summaries.len();
                }
                let candidate = flags & obc_formats::obcr::FLAG_ASSISTANT_CANDIDATE != 0;
                if candidate || flags & (obc_formats::obcr::FLAG_BUILT_DAY | obc_formats::obcr::FLAG_TEMPORARY) != 0 {
                    routes.internal |= 1 << routes.summaries.len();
                }
                if candidate && accepted & (1 << index) == 0 {
                    routes.candidates |= 1 << routes.summaries.len();
                }
                routes.summaries.push(summary);
                routes.ids.push(entry.id.0);
            }
            Ok(Err(Error::Io)) | Err(_) => return Err(()),
            Ok(Err(_)) => {}
        }
    }
    Ok(routes)
}

fn original_trips<D: BlockDevice>(store: &FlatStore<D>) -> Result<Trips, ()> {
    let mut heads: heapless::Vec<Head, 16> = heapless::Vec::new();
    for entry in store.entries().filter(|entry| entry.kind == ObjectKind::Trip) {
        retain_newest(&mut heads, Head { id: entry.id, revision: entry.revision });
    }
    if !store.entries_ok() {
        return Err(());
    }
    let mut trips = Trips::default();
    for entry in heads {
        match store.with_source(entry.id, Some(entry.revision), |source| TripMeta::read(source)) {
            Ok(Ok(meta)) => {
                trips.metas.push(meta);
                trips.ids.push(entry.id.0);
            }
            Ok(Err(Error::Io)) | Err(_) => return Err(()),
            Ok(Err(_)) => {}
        }
    }
    Ok(trips)
}

#[derive(Default)]
struct Sink(Vec<u8>);
impl ByteSink for Sink {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.0.extend_from_slice(bytes);
        Ok(())
    }
    fn patch_at(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Error> {
        let offset = offset as usize;
        self.0[offset..offset + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
}

fn put<D: BlockDevice>(store: &FlatStore<D>, kind: ObjectKind, flags: EntryFlags, bytes: &[u8]) {
    let mut allocation = store.allocate(bytes.len() as u64).unwrap();
    store.write(&mut allocation, bytes).unwrap();
    let meta = EntryMeta {
        id: store.next_object_id(),
        revision: Revision(1),
        kind,
        flags: EntryFlags::NONE,
        payload_len: bytes.len() as u64,
        payload_crc: obc_crc::crc32(bytes),
        added_at_utc: 0,
        name: Default::default(),
    };
    store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]).unwrap();
    if flags != EntryFlags::NONE {
        store.commit(&[Mutation::Put { meta: EntryMeta { flags, ..meta }, source: PutSource::Amend }]).unwrap();
    }
}

fn populate<D: BlockDevice>(store: &FlatStore<D>, routes: usize, trips: usize) {
    for i in 0..routes {
        let mut bytes = include_bytes!("../../../../../specs/vectors/route-plain.obcr").to_vec();
        bytes[5] |= match i % 4 {
            0 | 1 => obc_formats::obcr::FLAG_ASSISTANT_CANDIDATE,
            2 => obc_formats::obcr::FLAG_TEMPORARY,
            _ => obc_formats::obcr::FLAG_BUILT_DAY,
        };
        if i + 2 == routes {
            bytes[4] = 0;
        }
        let flags = if i % 4 == 0 { EntryFlags::ASSISTANT_ACCEPTED } else { EntryFlags::NONE };
        put(store, ObjectKind::Route, flags, &bytes);
    }
    for i in 0..trips {
        let mut sink = Sink::default();
        let days: Vec<_> = (0..35).map(|day| obc_route::TripDay::whole(0x1_0000_0000 + day)).collect();
        obc_route::write_trip(i as u64 + 1, "Trip", i as u16, &days, &mut sink).unwrap();
        if i + 2 == trips {
            sink.0[0] = 0;
        }
        put(store, ObjectKind::Trip, EntryFlags::NONE, &sink.0);
    }
}

#[test]
fn captured_loaders_choose_newest_decode_and_compact_masks() {
    let disk = SparseDisk::blank(250_000, 9);
    let store = FlatStore::initialize(&disk, StoreId([0x47; 16])).unwrap();
    populate(&store, 76, 20);
    let routes = original_routes(&store).unwrap();
    assert_eq!(routes.ids.len(), 63);
    assert_eq!(routes.ids[0], 76);
    assert_eq!(routes.ids[1], 74);
    assert_eq!(routes.ids[62], 13);
    assert_eq!(routes.candidates & 7, 2);
    assert_eq!(routes.temporary & 7, 0);
    assert_eq!(routes.internal & 7, 7);
    let trips = original_trips(&store).unwrap();
    assert_eq!(trips.ids.len(), 15);
    assert_eq!(trips.metas[0].key, 20);
    assert_eq!(trips.metas[14].key, 5);
    assert!(trips.metas.iter().all(|meta| meta.truncated && meta.day_routes.len() == 32));
    assert_eq!(trips.metas[0].day_routes[0], 0x1_0000_0000);
}

#[test]
fn captured_loaders_release_handles_after_every_media_read_refusal() {
    let disk = SparseDisk::blank(250_000, 9);
    let faults = FaultOnce::new(&disk);
    let store = FlatStore::initialize(&faults, StoreId([0x47; 16])).unwrap();
    populate(&store, 8, 4);
    let start = disk.ops();
    original_routes(&store).unwrap();
    original_trips(&store).unwrap();
    let reads = disk.ops() - start;
    for skip in 0..reads {
        faults.fault_after(MediaOp::Read, skip);
        let routes = original_routes(&store);
        let trips = original_trips(&store);
        assert!(faults.fired(), "read refusal {skip}/{reads}");
        let mut published_routes = Routes { ids: std::vec![999], ..Default::default() };
        let mut published_trips = Trips { ids: std::vec![999], ..Default::default() };
        if let Ok(routes) = routes {
            published_routes = routes;
        }
        if let Ok(trips) = trips {
            published_trips = trips;
        }
        assert!(!published_routes.ids.is_empty() && !published_trips.ids.is_empty());
        let handles: Vec<_> = (0..5).map(|_| store.open(ObjectId(1), Some(Revision(1))).unwrap()).collect();
        for handle in handles {
            store.close(handle);
        }
    }
}
