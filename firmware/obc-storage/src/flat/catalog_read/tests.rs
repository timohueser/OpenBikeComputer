use crate::flat::{sim::*, *};
use obc_formats::io::{ByteSink, Error};
use obc_route::{RouteSummary, TripMeta};
use std::vec::Vec;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Routes {
    summaries: Vec<RouteSummary>,
    ids: Vec<u64>,
    candidates: u64,
    internal: u64,
    temporary: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Trips {
    metas: Vec<TripMeta>,
    ids: Vec<u64>,
}

fn current_routes<D: BlockDevice>(store: &FlatStore<D>) -> Result<Routes, StoreError> {
    let mut routes = Routes::default();
    super::scan(store, ObjectKind::Route, &mut [super::Head::EMPTY; 64], &mut |head, accepted, source| {
        let (summary, flags) = RouteSummary::read_with_flags(source)?;
        let index = routes.summaries.len();
        if obc_formats::obcr::disposable_navigation(flags) {
            routes.temporary |= 1 << index;
        }
        let candidate = flags & obc_formats::obcr::FLAG_ASSISTANT_CANDIDATE != 0;
        if candidate || flags & (obc_formats::obcr::FLAG_BUILT_DAY | obc_formats::obcr::FLAG_TEMPORARY) != 0 {
            routes.internal |= 1 << index;
        }
        if candidate && !accepted {
            routes.candidates |= 1 << index;
        }
        routes.summaries.push(summary);
        routes.ids.push(head.id.0);
        Ok(())
    })?;
    Ok(routes)
}

fn current_trips<D: BlockDevice>(store: &FlatStore<D>) -> Result<Trips, StoreError> {
    let mut trips = Trips::default();
    super::scan(store, ObjectKind::Trip, &mut [super::Head::EMPTY; 16], &mut |head, _, source| {
        trips.metas.push(TripMeta::read(source)?);
        trips.ids.push(head.id.0);
        Ok(())
    })?;
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

fn fingerprint(hash: &mut u32, value: &impl core::fmt::Debug) {
    for byte in std::format!("{value:?}").bytes() {
        *hash = (*hash ^ u32::from(byte)).wrapping_mul(16_777_619);
    }
}

#[test]
fn captured_loaders_choose_newest_decode_and_compact_masks() {
    let disk = SparseDisk::blank(250_000, 9);
    let store = FlatStore::initialize(&disk, StoreId([0x47; 16])).unwrap();
    populate(&store, 76, 20);
    let routes = current_routes(&store).unwrap();
    assert_eq!(routes.ids.len(), 63);
    assert_eq!(routes.ids[0], 76);
    assert_eq!(routes.ids[1], 74);
    assert_eq!(routes.ids[62], 13);
    assert_eq!(routes.candidates & 7, 2);
    assert_eq!(routes.temporary & 7, 0);
    assert_eq!(routes.internal & 7, 7);
    let trips = current_trips(&store).unwrap();
    assert_eq!(trips.ids.len(), 15);
    assert_eq!(trips.metas[0].key, 20);
    assert_eq!(trips.metas[14].key, 5);
    assert!(trips.metas.iter().all(|meta| meta.truncated && meta.day_routes.len() == 32));
    assert_eq!(trips.metas[0].day_routes[0], 0x1_0000_0000);
    let mut hash = 2_166_136_261;
    fingerprint(&mut hash, &routes);
    fingerprint(&mut hash, &trips);
    assert_eq!(hash, 0xf859_b084);
}

/// A refused card read anywhere in either scan fails that scan with a media error or leaves its
/// result whole, and every handle the scan opened is closed again.
#[test]
fn loaders_survive_every_media_read_refusal_without_leaking_a_handle() {
    let disk = SparseDisk::blank(250_000, 9);
    let faults = FaultOnce::new(&disk);
    let store = FlatStore::initialize(&faults, StoreId([0x47; 16])).unwrap();
    populate(&store, 8, 4);
    let start = disk.ops();
    let routes = current_routes(&store).unwrap();
    let trips = current_trips(&store).unwrap();
    let reads = disk.ops() - start;
    for skip in 0..reads {
        faults.fault_after(MediaOp::Read, skip);
        let faulted_routes = current_routes(&store);
        let faulted_trips = current_trips(&store);
        assert!(faults.fired(), "read refusal {skip}/{reads} never landed");
        assert!(
            matches!(faulted_routes, Err(StoreError::Media)) || faulted_routes.as_ref() == Ok(&routes),
            "read refusal {skip}: routes {faulted_routes:?}",
        );
        assert!(
            matches!(faulted_trips, Err(StoreError::Media)) || faulted_trips.as_ref() == Ok(&trips),
            "read refusal {skip}: trips {faulted_trips:?}",
        );
        let handles: Vec<_> = (1..=crate::flat::store::MAX_OPEN_OBJECTS as u64)
            .map(|id| store.open(ObjectId(id), None).unwrap_or_else(|error| panic!("refusal {skip} leaked: {error:?}")))
            .collect();
        for handle in handles {
            store.close(handle);
        }
    }
}
