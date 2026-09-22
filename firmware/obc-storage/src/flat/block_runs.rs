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

/// A reserve the way an arm makes one: the image is written into the allocation before the commit,
/// the entry publishes with `payload_len: 0`, and the extents land on both sides of a hole. The
/// runs have to read back the whole image although the store serves none of it, because that is the
/// only route the bootloader has to those bytes.
#[test]
fn a_reserve_split_by_a_hole_reads_back_the_image_the_store_will_not_serve() {
    let geometry = Geometry::DEFAULT;
    let extent = geometry.extent_size() as usize;
    let disk = SparseDisk::blank(EXTENT_AREA + geometry.extent_blocks() * 8, 3);
    let store = FlatStore::initialize(&disk, STORE).expect("an expressible card");

    // A hole of one extent, then a survivor: a three-extent reserve takes the hole and continues
    // past it, which is two ranges over three extents.
    let (hole, revision) = publish(&store, &[0xA5u8; 8]);
    publish(&store, &[0x5Au8; 8]);
    store.commit(&[Mutation::Remove { id: hole, revision }]).expect("the spacer is removed");

    let image: Vec<u8> = (0..2 * extent + 1_024).map(|i| (i % 253) as u8).collect();
    let id = store.next_object_id();
    let mut allocation = store.allocate(image.len() as u64).expect("three extents are free");
    store.write(&mut allocation, &image).expect("the image fits the reservation");
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
    let count = store.block_runs(id, Revision(1), &mut runs).expect("the reserve is committed");
    assert_eq!(count, 2, "the hole and the run past the survivor are two ranges");
    assert_eq!(
        runs.iter().take(count).map(|run| run.blocks).sum::<u64>(),
        geometry.extent_blocks() * 3,
        "a `payload_len: 0` reserve keeps every extent it was given, untrimmed"
    );
    assert_eq!(runs[0].start_block, EXTENT_AREA, "the hole is the first extent of the area");
    assert!(runs[1].start_block > runs[0].start_block + runs[0].blocks, "the survivor sits between them");

    let mut read = Vec::new();
    let mut block = [0u8; BLOCK];
    for run in &runs[..count] {
        for index in 0..run.blocks {
            super::device::BlockDevice::read(&&disk, run.start_block + index, &mut block).expect("the card reads");
            read.extend_from_slice(&block);
        }
    }
    read.truncate(image.len());
    assert_eq!(read, image, "the runs locate the image the bootloader restores");

    assert!(store.open(id, Some(Revision(1))).is_err(), "and the store still refuses to serve those bytes");
    assert!(store.block_runs(id, Revision(2), &mut runs).is_err(), "no such revision, no runs");
}
