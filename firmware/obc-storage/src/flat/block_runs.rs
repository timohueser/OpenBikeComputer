//! The one place above the seam that learns a block number: the firmware boot handoff resolves a
//! committed object to absolute block runs, because the bootloader that reads those bytes has no
//! catalog and no store.
//!
//! The oracle is the payload itself. Reading the runs straight off the device has to reproduce the
//! bytes the seam serves, which is what makes the address arithmetic testable without restating it.

use std::vec::Vec;

use super::layout::{Geometry, BLOCK, EXTENT_AREA, MAX_RANGES};
use super::seam::{DisplayName, EntryFlags, EntryMeta, Mutation, ObjectId, ObjectKind, PutSource, Revision, StoreId};
use super::sim::SparseDisk;
use super::store::{BlockRun, FlatStore};
use super::Store as _;

const STORE: StoreId = StoreId([0x7B; 16]);

fn publish(store: &FlatStore<&SparseDisk>, payload: &[u8]) -> (ObjectId, Revision) {
    let id = store.next_object_id();
    let mut allocation = store.allocate(payload.len() as u64).expect("the extents are free");
    store.write(&mut allocation, payload).expect("the payload fits");
    let meta = EntryMeta {
        added_at_utc: 0,
        id,
        revision: Revision(1),
        kind: ObjectKind::UpdatePackage,
        flags: EntryFlags::NONE,
        payload_len: payload.len() as u64,
        payload_crc: super::raw::crc32(payload),
        name: DisplayName::default(),
    };
    store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]).expect("the commit lands");
    (id, Revision(1))
}

/// A package laid over a hole, so its bytes are on two non-adjacent ranges: the runs come back in
/// payload order, they are whole extents, and reading them off the device reproduces the payload.
#[test]
fn the_runs_of_a_split_object_read_back_as_its_payload() {
    let geometry = Geometry::DEFAULT;
    let extent = geometry.extent_size() as usize;
    let disk = SparseDisk::blank(EXTENT_AREA + geometry.extent_blocks() * 8, 3);
    let store = FlatStore::initialize(&disk, STORE).expect("an expressible card");

    // Two one-extent spacers, then remove the first: the package that follows takes the hole and
    // then continues past the survivor.
    let (hole, revision) = publish(&store, &[0xA5u8; 8]);
    publish(&store, &[0x5Au8; 8]);
    store.commit(&[Mutation::Remove { id: hole, revision }]).expect("the spacer is removed");

    let payload: Vec<u8> = (0..extent + 4_096).map(|i| (i % 251) as u8).collect();
    let (id, revision) = publish(&store, &payload);

    let mut runs = [BlockRun::default(); MAX_RANGES];
    let count = store.block_runs(id, revision, &mut runs).expect("the entry is committed");
    assert_eq!(count, 2, "the hole and the run past the survivor are two ranges");
    assert_eq!(runs[0].blocks, geometry.extent_blocks(), "a run is whole extents, not the payload's tail");
    assert_eq!(runs[0].start_block, EXTENT_AREA, "the hole is the first extent of the area");
    assert!(runs[1].start_block > runs[0].start_block + runs[0].blocks, "the survivor sits between them");

    // The handoff contract: the bytes those blocks hold are the payload, in run order.
    let mut read = Vec::new();
    let mut block = [0u8; BLOCK];
    for run in &runs[..count] {
        for index in 0..run.blocks {
            super::device::BlockDevice::read(&&disk, run.start_block + index, &mut block).expect("the card reads");
            read.extend_from_slice(&block);
        }
    }
    read.truncate(payload.len());
    assert_eq!(read, payload, "the runs locate the payload the seam serves");
}

/// A revision the catalog does not hold has no runs, and a reserve's runs are readable although its
/// payload is not: the boot handoff needs the extents of exactly the entry that owns them.
#[test]
fn a_reserve_resolves_and_an_absent_revision_does_not() {
    let geometry = Geometry::DEFAULT;
    let disk = SparseDisk::blank(EXTENT_AREA + geometry.extent_blocks() * 4, 3);
    let store = FlatStore::initialize(&disk, STORE).expect("an expressible card");

    let id = store.next_object_id();
    let allocation = store.allocate(geometry.extent_size()).expect("the extents are free");
    let meta = EntryMeta {
        added_at_utc: 0,
        id,
        revision: Revision(1),
        kind: ObjectKind::RollbackReserve,
        flags: EntryFlags::RESERVED,
        payload_len: 0,
        payload_crc: 0,
        name: DisplayName::default(),
    };
    store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]).expect("the commit lands");

    let mut runs = [BlockRun::default(); MAX_RANGES];
    assert_eq!(store.block_runs(id, Revision(1), &mut runs).expect("the reserve is committed"), 1);
    assert_eq!(runs[0].blocks, geometry.extent_blocks(), "a reserve keeps every extent it was given");
    assert!(store.open(id, Some(Revision(1))).is_err(), "and the store still refuses to serve its bytes");
    assert!(store.block_runs(id, Revision(2), &mut runs).is_err(), "no such revision, no runs");
}
