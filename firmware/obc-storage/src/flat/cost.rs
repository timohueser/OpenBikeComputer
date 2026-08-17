//! What the two paths a rider waits on cost the card, in **commands**, pinned.
//!
//! §5.5 and §5.6 state block counts, and the FS4 bench (#1409) found them exactly right on glass: 79
//! blocks written for a commit at 300 entries, 260 at 1,024. What was 10–35× over the plan was the
//! number of card *commands* those blocks were issued in, because the media charges roughly a program
//! cycle per write command — 1.34 ms measured on the sEMMC path, which polls CMD13 per write, so a
//! `sync` costs nothing of its own — and only ~74 µs a block inside one. A read command is ~0.5 ms plus
//! ~41 µs a block (12,301 kB/s sequential).
//!
//! So the two counts answer different questions and both are pinned here, for opposite reasons.
//! **Commands** are scheduling: they are what this slice changed and what a regression would silently
//! give back, so each case pins its exact census and prints the time it projects, and a change that
//! reintroduces block-at-a-time I/O fails with a number rather than with a shrug. **Blocks** are the
//! format's, restated from §5.5 and §5.6 — pinned so that a scheduling change which moved one fails as
//! the format change it would be.
//!
//! **The projections are arithmetic, not measurements.** They are the census times the bench's
//! per-command figures. Applied to the *old* census the same arithmetic under-predicts the bench's own
//! wall times by 10–25% (184 ms against a measured 203.6 at 300 entries, 609 against 697.5 at 1,024), so
//! the real numbers should be expected above these by about that much. Re-measuring on glass is
//! #1409's follow-up, not this file's claim.

use std::vec::Vec;

use super::crash::{entry, install_catalog, payload};
use super::layout::{EXTENT_AREA, EXTENT_BLOCKS, SUPERBLOCK};
use super::model::Model;
use super::seam::{EntryFlags, Mutation, ObjectKind, PutSource, Store, StoreId};
use super::sim::{MediaOp, SparseDisk};
use super::store::FlatStore;
use super::superblock::Superblock;

const STORE: StoreId = StoreId([0x71; 16]);

/// The card #1409 measured, in microseconds: a write command's program cycle and its marginal block, a
/// read command's turnaround and its marginal block. A `sync` is free — durability is folded into every
/// write by the CMD13 poll — which is the measurement that contradicted §5.5's "dominated by the
/// synchronizations".
const WRITE_COMMAND_US: u64 = 1_340;
const WRITE_BLOCK_US: u64 = 74;
const READ_COMMAND_US: u64 = 500;
const READ_BLOCK_US: u64 = 41;

/// The card commands one path issued, and the blocks they carried.
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

    /// The time this census projects on that card. See the module note: arithmetic, not a measurement.
    fn micros(&self) -> u64 {
        self.writes * WRITE_COMMAND_US
            + (self.write_blocks - self.writes) * WRITE_BLOCK_US
            + self.reads * READ_COMMAND_US
            + (self.read_blocks - self.reads) * READ_BLOCK_US
    }
}

/// A card whose catalog holds `entries` objects, each owning one extent of its own, with room for a few
/// more. Each entry gets a real extent because the mount that builds the free bitmap rejects an overlap,
/// so a fake catalog would not mount and the census would be of nothing.
fn populated(entries: u16) -> SparseDisk {
    let extents = entries as u32 + 8;
    let blocks = EXTENT_AREA + EXTENT_BLOCKS * extents as u64;
    let mut model = Model::empty(STORE, extents);
    for id in 1..=entries as u64 {
        model.entries.push(entry(id, 1, ObjectKind::Trip, EntryFlags::NONE, 600, "", &[(id as u16 - 1, 1)]));
    }
    model.next_object = entries as u64 + 1;
    model.sequence = 4;
    model.high_water = 4;

    let disk = SparseDisk::blank(blocks, 7);
    let superblock = Superblock { store: STORE, total_blocks: blocks }.encode();
    disk.install(SUPERBLOCK[0], &superblock);
    disk.install(SUPERBLOCK[1], &superblock);
    install_catalog(&disk, &model, 0);
    disk
}

/// One commit publishing one more object on a card that holds `entries`, counted from the commit alone:
/// the allocation and the payload writes are the transfer's cost, not §5.5's, and the bench timed them
/// apart too.
fn commit_census(entries: u16) -> Census {
    let disk = populated(entries);
    let mut store = FlatStore::mount(&disk);
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

/// §5.5's commit, at an empty catalog, at the few hundred entries the budget is quoted for, and at the
/// 1,024 the bench's worst case used. Each case publishes the *next* object, so the body it writes holds
/// `entries + 1` — the bench's 79 write blocks at 300 entries and this case's 80 are the same figure one
/// entry apart.
///
/// Writes are `1 + ceil(body_blocks / 8) + 2`: the payload's staged partial block, the body in windows,
/// and step 1's and step 3's gate blocks. Reads are two windowed passes over the live prefix plus
/// `find`'s binary search — 6 probes at 300 entries, 8 at 1,024 — which stays block-at-a-time on
/// purpose, because its probes are scattered and a window would read 4 KiB to look at 128 bytes of it.
/// The fourth `sync` is the payload's; §5.5 owns three, and none of them costs anything on this card.
#[test]
fn a_commit_costs_the_pinned_card_commands() {
    // (entries, census, the ceiling the projection has to stay under)
    let cases = [
        (0u16, Census { reads: 0, read_blocks: 0, writes: 4, write_blocks: 5, syncs: 4 }, 6_000u64),
        (300, Census { reads: 26, read_blocks: 156, writes: 13, write_blocks: 80, syncs: 4 }, 45_000),
        (1_024, Census { reads: 72, read_blocks: 520, writes: 36, write_blocks: 261, syncs: 4 }, 125_000),
    ];
    let measured: Vec<Census> = cases.iter().map(|(entries, _, _)| commit_census(*entries)).collect();
    for ((entries, _, _), census) in cases.iter().zip(&measured) {
        std::println!("commit at {entries} entries: {census:?} = {} µs projected", census.micros());
    }
    for ((entries, expected, ceiling), census) in cases.into_iter().zip(measured) {
        assert_eq!(census, expected, "the commit at {entries} entries no longer costs the pinned commands");
        assert!(
            census.micros() <= ceiling,
            "the commit at {entries} entries projects {} µs, above the {ceiling} µs this case allows",
            census.micros(),
        );
    }
}

/// §5.6's mount, at the 1,025 entries the bench measured 185.8 ms on: one superblock, two gates, and the
/// live prefix — header block included — in windows.
#[test]
fn a_mount_costs_the_pinned_card_commands() {
    let census = mount_census(1_025);
    std::println!("mount at 1,025 entries: {census:?} = {} µs projected", census.micros());
    assert_eq!(
        census,
        Census { reads: 37, read_blocks: 261, writes: 0, write_blocks: 0, syncs: 0 },
        "the mount at 1,025 entries no longer costs the pinned commands",
    );
    // §5.6's plan figure is "about 100 ms"; this is the margin under it that made the slice worth doing.
    assert!(census.micros() <= 30_000, "a mount projects {} µs, above the 30 ms this case allows", census.micros());
}

/// The blocks are still the format's, unchanged: §5.5's `ceil(n / 4) + 3` writes and §5.6's
/// `3 + 1 + ceil(n / 4)` reads. A batching change that moved a *block* would be a format change, and
/// this is what says it did not.
#[test]
fn the_block_counts_are_still_the_specs_own() {
    for entries in [0u16, 300, 1_024] {
        let census = commit_census(entries);
        // §5.5's `ceil(n / 4) + 3` for the `n + 1` entries this commit publishes, and the payload's
        // staged partial block, which is the transfer's rather than the commit's.
        let body = 1 + (u64::from(entries) + 1).div_ceil(4);
        assert_eq!(census.write_blocks, body + 2 + 1, "§5.5's block count moved at {entries} entries");
    }
    let census = mount_census(1_025);
    assert_eq!(census.read_blocks, 3 + 1 + 1_025u64.div_ceil(4), "§5.6's block count moved");
}
