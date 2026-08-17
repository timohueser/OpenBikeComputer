//! What the two paths a rider waits on cost, pinned: card **commands**, card **blocks**, and the
//! **M33's** own share.
//!
//! §5.5 and §5.6 state block counts, and the FS4 bench (#1409, round 2) found them exactly right on
//! glass: 79 blocks written for a commit at 300 entries, 260 at 1,024. What was 11–35× over the plan
//! was partly the number of card *commands* those blocks were issued in — the media charges about a
//! program cycle per write command, and a `sync` costs nothing of its own because the sEMMC path polls
//! CMD13 per write — and partly something no I/O model can see at all.
//!
//! **A third of a commit and half a mount is CPU.** The bench times its three terms separately, inside
//! the adapter, and at 300 entries a commit is 116 ms writing / 53 ms reading / **42 ms on the M33**;
//! at 1,024 it is 390 / 178 / **141**; a mount at 1,025 entries is 88.5 reading / **95.6 on the M33**.
//! That last one is the important one: a mount is *CPU-bound*, not I/O-bound. The M33's work is entry
//! decode, `Structure::accept`'s §5.3 pass, the free-bitmap claim, entry encode on the way out, and the
//! body CRC fold — all of it per entry, and **none of it touched by batching**, which moves commands and
//! not bytes. So it is an additive term this change cannot improve, and after the change it is the
//! largest single term in both paths.
//!
//! Three counts, pinned for three reasons. **Commands** are scheduling: what this slice changed and
//! what a regression would silently give back. **Blocks** are the format's, restated from §5.5 and
//! §5.6, so a scheduling change that moved one fails as the format change it would be. **Entries** is
//! what the M33 term scales with, so a projection cannot quietly drop it.
//!
//! **The projections are arithmetic, not measurements** — the census times the constants below. What
//! makes them worth printing is that the same arithmetic reproduces every published round-2 figure to
//! within 5%: 210.5 ms against a measured 220.8 at 300 entries (the bottom of its own 210.5–241.2
//! spread), 701 against 701.8 at 1,024, and 184.0 against 184.1 for the mount. Re-measuring the *new*
//! schedule on glass is #1409's follow-up, not this file's claim.

use std::vec::Vec;

use super::crash::{entry, install_catalog, payload};
use super::layout::{EXTENT_AREA, EXTENT_BLOCKS, SUPERBLOCK};
use super::model::Model;
use super::seam::{EntryFlags, Mutation, ObjectKind, PutSource, Store, StoreId};
use super::sim::{MediaOp, SparseDisk};
use super::store::FlatStore;
use super::superblock::Superblock;

const STORE: StoreId = StoreId([0x71; 16]);

/// The card #1409's round 2 measured, in microseconds: a command's fixed cost, and each further block
/// inside it. A `sync` is free — durability is folded into every write by the CMD13 poll — which is the
/// measurement that contradicted §5.5's "dominated by the synchronizations".
///
/// The fixed costs are the bench's single-block figures (79 writes = 116 ms, 156 reads = 53 ms); the
/// marginal ones are its sequential throughputs (7,128 kB/s writing, 12,170 kB/s reading). The read
/// pair over-predicts the one published *wide* read — a 64-block call measured 2,692 µs against this
/// model's 2,986 — so read projections here are the conservative side of a two-point fit by about 10%.
const WRITE_COMMAND_US: u64 = 1_470;
const WRITE_BLOCK_US: u64 = 72;
const READ_COMMAND_US: u64 = 340;
const READ_BLOCK_US: u64 = 42;

/// The M33's own microseconds per entry in the body, which batching does not move.
///
/// A commit pays for two decode passes over the live prefix, one encode of every entry it writes,
/// `Structure::accept`'s §5.3 pass and the body CRC fold: 42.0 ms over 300 entries and 141.1 ms over
/// 1,024 — 140 and 138 µs an entry, which is as linear as it looks. A mount pays for one decode pass,
/// the same §5.3 pass, the free-bitmap claim and the same CRC fold, and no encode: 95.6 ms over 1,025
/// entries, 93 µs an entry.
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

    /// Both, which is what a rider waits for. See the module note: arithmetic, not a measurement.
    fn micros(&self, per_entry: u64) -> u64 {
        self.io_micros() + self.m33_micros(per_entry)
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
    Census::of(&disk.ledger()[before..], u64::from(entries) + 1)
}

fn mount_census(entries: u16) -> Census {
    let disk = populated(entries);
    let store = FlatStore::mount(&disk);
    assert_eq!(store.entry_count(), entries, "the mount under test did not serve the catalog");
    Census::of(&disk.ledger(), u64::from(entries))
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

/// §5.6's mount, at the 1,025 entries the bench measured 184.1 ms on: one superblock, two gates, and the
/// live prefix — header block included — in windows.
///
/// This is the case that makes the M33 term worth having. Batching takes the reading from 88.5 ms to
/// **32**, and the mount still projects ~127 ms, because 95 of those milliseconds are the M33 decoding
/// 1,025 entries, checking §5.3, claiming their extents and folding the body CRC — work the schedule
/// cannot touch. §5.6 plans for "about 100 ms" and this **misses it**, by about a quarter. Saying so is
/// the point: the ceiling below is where the path actually is, not where the plan wished it were.
///
/// The window here is [`MOUNT_STREAM_WINDOW`](super::layout::MOUNT_STREAM_WINDOW), half a commit's, so
/// 69 read commands rather than the 37 a 4 KiB window would give. That is ~9 ms bought back for 2 KiB
/// of the boot frame; see that constant for why this frame in particular could not spare it.
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

/// The model, held to the measurements it came from. Applied to the census the store *used* to produce
/// — every block its own command — the three constants have to reproduce #1409's round-2 wall times,
/// or the projections above are arithmetic about nothing.
///
/// This is the test that makes the constants evidence rather than taste, and it is why the M33 term is
/// separate: an I/O-only model reproduces the commit figures to about 20% and the *mount* to 48%, which
/// is how the first version of this file came to claim a mount would take 28 ms.
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
