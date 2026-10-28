//! What the two paths a rider waits on cost, held to a budget: card commands and card blocks.
//!
//! Two counts, for two reasons. Commands are scheduling, so a path may issue fewer than its budget,
//! never more. Blocks are the format's, so a scheduling change that moved one fails as the format
//! change it would be.

use super::crash::{entry, install_catalog, install_slot, payload};
use super::layout::{Geometry, EXTENT_AREA, SUPERBLOCK};
use super::model::Model;
use super::seam::{EntryFlags, Mutation, ObjectKind, PutSource, Store, StoreId};
use super::sim::{MediaOp, SparseDisk};
use super::store::FlatStore;
use super::superblock::Superblock;

const STORE: StoreId = StoreId([0x71; 16]);

/// The card commands one path issued and the blocks they carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Census {
    reads: u64,
    read_blocks: u64,
    writes: u64,
    write_blocks: u64,
    syncs: u64,
}

impl Census {
    fn of(ledger: &[(u32, MediaOp, u64)]) -> Self {
        let count = |want: MediaOp| ledger.iter().filter(|(_, kind, _)| *kind == want).count() as u64;
        let blocks =
            |want: MediaOp| ledger.iter().filter(|(_, kind, _)| *kind == want).map(|(_, _, blocks)| blocks).sum();
        Census {
            reads: count(MediaOp::Read),
            read_blocks: blocks(MediaOp::Read),
            writes: count(MediaOp::Write),
            write_blocks: blocks(MediaOp::Write),
            syncs: count(MediaOp::Sync),
        }
    }
}

/// A card whose catalog holds `entries` objects, each owning one extent of its own, with room for a
/// few more. Each entry gets a real extent because the mount that builds the free bitmap rejects an
/// overlap, so a fake catalog would not mount and the census would be of nothing.
fn populated(entries: u16) -> SparseDisk {
    let extents = entries as u32 + 8;
    // The census is the default geometry's, deliberately: the budgets below are quoted for it.
    let blocks = EXTENT_AREA + Geometry::DEFAULT.extent_blocks() * extents as u64;
    let mut model = Model::empty(STORE, extents);
    for id in 1..=entries as u64 {
        model.entries.push(entry(id, 1, ObjectKind::Trip, EntryFlags::NONE, 600, "", &[(id as u16 - 1, 1)]));
    }
    model.next_object = entries as u64 + 1;
    model.sequence = 4;
    model.high_water = 4;

    let disk = SparseDisk::blank(blocks, 7);
    let superblock = Superblock::for_card(STORE, blocks).expect("an expressible card").encode();
    disk.install(SUPERBLOCK[0], &superblock);
    disk.install(SUPERBLOCK[1], &superblock);
    install_catalog(&disk, &model, 0);
    disk
}

/// One commit publishing one more object on a card that holds `entries`, counted from the commit
/// alone: the allocation and the payload writes are the transfer's cost, not the commit's.
fn commit_census(entries: u16) -> Census {
    let disk = populated(entries);
    let store = FlatStore::mount(&disk);
    let mut allocation = store.allocate(3_000).unwrap();
    store.write(&mut allocation, &payload(3_000)).unwrap();
    let published =
        entry(entries as u64 + 1, 1, ObjectKind::Trip, EntryFlags::NONE, 3_000, "one more", &[(entries, 1)]);
    let before = disk.ledger().len();
    store.commit(&[Mutation::Put { meta: published.meta, source: PutSource::Fresh(allocation) }]).unwrap();
    assert_eq!(store.entry_count(), entries + 1, "the commit under test did not land");
    Census::of(&disk.ledger()[before..])
}

fn mount_census(entries: u16) -> Census {
    let disk = populated(entries);
    let store = FlatStore::mount(&disk);
    assert_eq!(store.entry_count(), entries, "the mount under test did not serve the catalog");
    Census::of(&disk.ledger())
}

/// Finalise a recording with `flushed` bytes already in write-once payload pages and a fixed tail.
/// Setup writes are installed directly and do not enter the census; only the footer/tail flush and
/// the one catalog commit the rider waits for are measured.
fn finish_census(flushed: u64) -> Census {
    const TAIL: usize = 5_000;
    let extents = 64u32;
    let blocks = EXTENT_AREA + Geometry::DEFAULT.extent_blocks() * extents as u64;
    let ride = entry(1, 1, ObjectKind::Ride, EntryFlags::RECORDING, 0, "", &[(0, 32)]);
    let mut model = Model::empty(STORE, extents);
    model.entries.push(ride);
    model.next_object = 2;
    model.sequence = 4;
    model.high_water = 4;

    let disk = SparseDisk::blank(blocks, 23);
    let superblock = Superblock::for_card(STORE, blocks).unwrap().encode();
    disk.install(SUPERBLOCK[0], &superblock);
    disk.install(SUPERBLOCK[1], &superblock);
    install_catalog(&disk, &model, 0);
    let tail = payload(TAIL);
    install_slot(&disk, STORE, &ride, 1, flushed, &tail);

    let store = FlatStore::mount(&disk);
    assert_eq!(store.recovered_ride().unwrap().flushed, flushed);
    let finalised = entry(1, 1, ObjectKind::Ride, EntryFlags::NONE, flushed + TAIL as u64, "finished", &[(0, 1)]);
    let before = disk.ledger().len();
    store.commit(&[Mutation::Put { meta: finalised.meta, source: PutSource::Amend }]).unwrap();
    Census::of(&disk.ledger()[before..])
}

/// The commit at an empty catalog, at the few hundred entries the budget is quoted for, and at
/// 1,024. Each case publishes the next object, so the body it writes
/// holds `entries + 1`.
///
/// Each command count is a ceiling: a commit may issue fewer, never more. Writes are at most
/// `1 + ceil(body_blocks / 8) + 2`: the payload's staged partial block, the body in windows, and the
/// two gate blocks. Reads are two windowed passes over the live prefix plus `find`'s block-at-a-time
/// binary search. The fourth `sync` is the payload's.
#[test]
fn a_commit_stays_within_its_command_budget() {
    // (entries, the most reads, writes and syncs)
    let cases = [(0u16, 0, 4, 4), (300, 26, 13, 4), (1_024, 72, 36, 4)];
    for (entries, reads, writes, syncs) in cases {
        let census = commit_census(entries);
        std::println!("commit at {entries} entries: {census:?}");
        assert!(census.reads <= reads, "the commit at {entries} entries issues {} reads", census.reads);
        assert!(census.writes <= writes, "the commit at {entries} entries issues {} writes", census.writes);
        assert!(census.syncs <= syncs, "the commit at {entries} entries issues {} syncs", census.syncs);
    }
}

/// The mount, at 1,025 entries: one superblock, two gates, and the live
/// prefix, header block included, in windows of
/// [`MOUNT_STREAM_WINDOW`](super::layout::MOUNT_STREAM_WINDOW). It may issue fewer than 69 reads,
/// never more, and it writes nothing.
#[test]
fn a_mount_stays_within_its_command_budget() {
    let census = mount_census(1_025);
    std::println!("mount at 1,025 entries: {census:?}");
    assert!(census.reads <= 69, "a mount issues {} reads, above the 69 it is allowed", census.reads);
    assert_eq!((census.writes, census.syncs), (0, 0), "a clean mount wrote to the card");
}

/// Finishing a ride is O(1) in ride length: it copies only the selected tail slot and commits one
/// catalog, so one page and one thousand pages issue the same card commands and blocks.
#[test]
fn finishing_io_is_constant_in_the_ride_length() {
    let short = finish_census(super::layout::PROGRAM_PAGE as u64);
    let long = finish_census(1_000 * super::layout::PROGRAM_PAGE as u64);
    std::println!("ride finish: {short:?}");
    assert_eq!(short, long, "finish I/O grew with the flushed ride prefix");
    assert_eq!(
        short,
        Census { reads: 27, read_blocks: 55, writes: 29, write_blocks: 30, syncs: 5 },
        "the fixed finish census changed",
    );
}

/// The blocks are still the format's, unchanged: `ceil(n / 4) + 3` writes and `3 + 1 + ceil(n / 4)`
/// reads. A batching change that moved a block would be a format change, and this says it did not.
#[test]
fn the_block_counts_are_still_the_specs_own() {
    for entries in [0u16, 300, 1_024] {
        let census = commit_census(entries);
        // `ceil(n / 4) + 3` for the `n + 1` entries this commit publishes, and the payload's staged
        // partial block, which is the transfer's rather than the commit's.
        let body = 1 + (u64::from(entries) + 1).div_ceil(4);
        assert_eq!(census.write_blocks, body + 2 + 1, "§5.5's block count moved at {entries} entries");
    }
    let census = mount_census(1_025);
    assert_eq!(census.read_blocks, 3 + 1 + 1_025u64.div_ceil(4), "§5.6's block count moved");
}
