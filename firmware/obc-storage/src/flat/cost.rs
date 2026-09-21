//! What the two paths a rider waits on cost, pinned: card commands, card blocks, and the M33's own
//! share.
//!
//! The format states block counts. What the bench found on glass is that the cost is mostly the
//! number of card commands those blocks are issued in — the media charges about a program cycle per
//! write command, and a `sync` costs nothing of its own because the sEMMC path polls CMD13 per
//! write — and partly CPU that no I/O model can see.
//!
//! A third of a commit and half a mount is CPU. A mount is CPU-bound, not I/O-bound: entry decode,
//! the structural pass, the free-bitmap claim, entry encode on the way out and the body CRC fold are
//! all per entry, and none of it is touched by batching, which moves commands and not bytes.
//!
//! Three counts, pinned for three reasons. Commands are scheduling. Blocks are the format's, so a
//! scheduling change that moved one fails as the format change it would be. Entries is what the M33
//! term scales with, so a projection cannot quietly drop it.
//!
//! The projections are arithmetic, not measurements: the census times the constants below.

use std::vec::Vec;

use super::crash::{entry, install_catalog, install_slot, payload};
use super::layout::{Geometry, EXTENT_AREA, SUPERBLOCK};
use super::model::Model;
use super::seam::{EntryFlags, Mutation, ObjectKind, PutSource, Store, StoreId};
use super::sim::{MediaOp, SparseDisk};
use super::store::FlatStore;
use super::superblock::Superblock;

const STORE: StoreId = StoreId([0x71; 16]);

/// The card the bench measured, in microseconds: a command's fixed cost, and each further block
/// inside it. A `sync` is free, because durability is folded into every write by the CMD13 poll.
///
/// The fixed costs are the bench's single-block figures; the marginal ones are its sequential
/// throughputs. Read projections are the conservative side of a two-point fit by about 10%.
const WRITE_COMMAND_US: u64 = 1_470;
const WRITE_BLOCK_US: u64 = 72;
const READ_COMMAND_US: u64 = 340;
const READ_BLOCK_US: u64 = 42;

/// The M33's own microseconds per entry in the body, which batching does not move.
///
/// A commit pays for two decode passes over the live prefix, one encode of every entry it writes, the
/// structural pass and the body CRC fold: about 140 µs an entry. A mount pays for one decode pass,
/// the same structural pass, the free-bitmap claim and the same fold, and no encode: about 93 µs.
const COMMIT_M33_PER_ENTRY_US: u64 = 138;
const MOUNT_M33_PER_ENTRY_US: u64 = 93;

/// The card commands one path issued, the blocks they carried, and the entries the M33 had to decode,
/// check, encode and fold to produce them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Census {
    reads: u64,
    read_blocks: u64,
    writes: u64,
    write_blocks: u64,
    syncs: u64,
    /// Entries in the body this path produced or read. The M33 term scales with it.
    entries: u64,
}

impl Census {
    fn of(ledger: &[(u32, MediaOp, u64)], entries: u64) -> Self {
        let count = |want: MediaOp| ledger.iter().filter(|(_, kind, _)| *kind == want).count() as u64;
        let blocks =
            |want: MediaOp| ledger.iter().filter(|(_, kind, _)| *kind == want).map(|(_, _, blocks)| blocks).sum();
        Census {
            reads: count(MediaOp::Read),
            read_blocks: blocks(MediaOp::Read),
            writes: count(MediaOp::Write),
            write_blocks: blocks(MediaOp::Write),
            syncs: count(MediaOp::Sync),
            entries,
        }
    }

    /// What the card charges: commands plus their marginal blocks.
    fn io_micros(&self) -> u64 {
        self.writes * WRITE_COMMAND_US
            + (self.write_blocks - self.writes) * WRITE_BLOCK_US
            + self.reads * READ_COMMAND_US
            + (self.read_blocks - self.reads) * READ_BLOCK_US
    }

    /// What the M33 charges, which no amount of batching reduces.
    fn m33_micros(&self, per_entry: u64) -> u64 {
        self.entries * per_entry
    }

    /// Both, which is what a rider waits for. Arithmetic, not a measurement.
    fn micros(&self, per_entry: u64) -> u64 {
        self.io_micros() + self.m33_micros(per_entry)
    }
}

/// A card whose catalog holds `entries` objects, each owning one extent of its own, with room for a
/// few more. Each entry gets a real extent because the mount that builds the free bitmap rejects an
/// overlap, so a fake catalog would not mount and the census would be of nothing.
fn populated(entries: u16) -> SparseDisk {
    let extents = entries as u32 + 8;
    // The census is the default geometry's, deliberately: a card this size gets 1 MiB extents, and a
    // census taken at another size would not be comparable with the bench's.
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
    Census::of(&disk.ledger()[before..], u64::from(entries) + 1)
}

fn mount_census(entries: u16) -> Census {
    let disk = populated(entries);
    let store = FlatStore::mount(&disk);
    assert_eq!(store.entry_count(), entries, "the mount under test did not serve the catalog");
    Census::of(&disk.ledger(), u64::from(entries))
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
    Census::of(&disk.ledger()[before..], 1)
}

/// The commit at an empty catalog, at the few hundred entries the budget is quoted for, and at the
/// 1,024 the bench's worst case used. Each case publishes the next object, so the body it writes
/// holds `entries + 1`.
///
/// Writes are `1 + ceil(body_blocks / 8) + 2`: the payload's staged partial block, the body in
/// windows, and the two gate blocks. Reads are two windowed passes over the live prefix plus `find`'s
/// binary search, which stays block-at-a-time on purpose because its probes are scattered. The fourth
/// `sync` is the payload's.
///
/// The ceilings are the projections plus about a tenth. They are deliberately loose: what catches a
/// regression is the census equality above them, and a ceiling that tracked the projection exactly
/// would fail on an improvement.
#[test]
fn a_commit_costs_the_pinned_card_commands() {
    // (entries, census, the ceiling the projection has to stay under)
    let cases = [
        (0u16, Census { reads: 0, read_blocks: 0, writes: 4, write_blocks: 5, syncs: 4, entries: 1 }, 7_000u64),
        (300, Census { reads: 26, read_blocks: 156, writes: 13, write_blocks: 80, syncs: 4, entries: 301 }, 88_000),
        (
            1_024,
            Census { reads: 72, read_blocks: 520, writes: 36, write_blocks: 261, syncs: 4, entries: 1_025 },
            280_000,
        ),
    ];
    let measured: Vec<Census> = cases.iter().map(|(entries, _, _)| commit_census(*entries)).collect();
    for ((entries, _, _), census) in cases.iter().zip(&measured) {
        std::println!(
            "commit at {entries} entries: {census:?} = {} µs I/O + {} µs M33 = {} µs projected",
            census.io_micros(),
            census.m33_micros(COMMIT_M33_PER_ENTRY_US),
            census.micros(COMMIT_M33_PER_ENTRY_US),
        );
    }
    for ((entries, expected, ceiling), census) in cases.into_iter().zip(measured) {
        assert_eq!(census, expected, "the commit at {entries} entries no longer costs the pinned commands");
        let projected = census.micros(COMMIT_M33_PER_ENTRY_US);
        assert!(
            projected <= ceiling,
            "the commit at {entries} entries projects {projected} µs, above the {ceiling} µs this case allows",
        );
    }
}

/// The mount, at the 1,025 entries the bench measured: one superblock, two gates, and the live
/// prefix — header block included — in windows.
///
/// This is the case that makes the M33 term worth having. The mount projects about 127 ms because 95
/// of those milliseconds are the M33 decoding 1,025 entries, checking the structural rules, claiming
/// their extents and folding the body CRC. The format plans for about 100 ms and this misses it by
/// about a quarter; the ceiling below is where the path actually is.
///
/// The window here is [`MOUNT_STREAM_WINDOW`](super::layout::MOUNT_STREAM_WINDOW), half a commit's,
/// so 69 read commands rather than the 37 a 4 KiB window would give.
#[test]
fn a_mount_costs_the_pinned_card_commands() {
    let census = mount_census(1_025);
    std::println!(
        "mount at 1,025 entries: {census:?} = {} µs I/O + {} µs M33 = {} µs projected",
        census.io_micros(),
        census.m33_micros(MOUNT_M33_PER_ENTRY_US),
        census.micros(MOUNT_M33_PER_ENTRY_US),
    );
    assert_eq!(
        census,
        Census { reads: 69, read_blocks: 261, writes: 0, write_blocks: 0, syncs: 0, entries: 1_025 },
        "the mount at 1,025 entries no longer costs the pinned commands",
    );
    let projected = census.micros(MOUNT_M33_PER_ENTRY_US);
    assert!(projected <= 140_000, "a mount projects {projected} µs, above the 140 ms this case allows");
    // And the part of it this change could reach: the reading, which was 261 commands and is now 69.
    assert!(census.io_micros() <= 35_000, "a mount reads {} µs, above the 35 ms this case allows", census.io_micros());
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
        Census { reads: 27, read_blocks: 55, writes: 29, write_blocks: 30, syncs: 5, entries: 1 },
        "the fixed finish census changed",
    );
}

/// The model, held to the measurements it came from. Applied to a census with one block per
/// command, the three constants have to reproduce the bench's wall times, or the projections above are arithmetic about nothing. This is also why the M33 term is
/// separate: an I/O-only model reproduces the commit figures to about 20% and the mount to 48%.
#[test]
fn the_model_reproduces_the_benchs_own_measurements() {
    // (name, census as the block-at-a-time store issued it, per-entry M33 term, µs measured on glass)
    let cases = [
        (
            "commit @ 300",
            Census { reads: 156, read_blocks: 156, writes: 79, write_blocks: 79, syncs: 4, entries: 300 },
            COMMIT_M33_PER_ENTRY_US,
            220_800u64,
        ),
        (
            "commit @ 1,024",
            Census { reads: 522, read_blocks: 522, writes: 260, write_blocks: 260, syncs: 4, entries: 1_024 },
            COMMIT_M33_PER_ENTRY_US,
            701_800,
        ),
        (
            "mount @ 1,025",
            Census { reads: 261, read_blocks: 261, writes: 0, write_blocks: 0, syncs: 0, entries: 1_025 },
            MOUNT_M33_PER_ENTRY_US,
            184_100,
        ),
    ];
    for (name, census, per_entry, measured) in cases {
        let projected = census.micros(per_entry);
        let error = projected.abs_diff(measured) * 100 / measured;
        std::println!("{name}: model {projected} µs vs measured {measured} µs ({error}%)");
        assert!(error <= 5, "{name}: the model is {error}% off its own measurement ({projected} vs {measured})");
    }
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
