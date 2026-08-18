//! What a render pass and a route plan cost the card in **block reads**, pinned — and measured
//! against the path they replace, in the same test session.
//!
//! This is issue #500's guard rail. That bug was the map's scattered reads going through
//! `embedded-sdmmc`'s O(offset) seek: FAT is a singly-linked list, every backward seek restarts at
//! the file's first cluster, and the one-block FAT cache thrashed against the data reads — ~109 FAT
//! sector reads per 2 KB nav chunk, 41k block reads for a 2 km plan, ~100% of a 56 s route
//! computation. [`fat_extents`](crate::fat_extents) fixed it by resolving the chain once at open, so
//! every later read is arithmetic plus the data blocks themselves.
//!
//! The flat store is that same arithmetic, one layer down and without the filesystem: `Ranges::locate`
//! over at most 8 extent ranges, then one card command per contiguous run. So the claim this file has
//! to make good is not "the flat path is fast" in the abstract — it is **"the flat path costs no more
//! than `fat_extents` did, on the same access pattern, for the same object"**. Both columns are
//! measured here, side by side, so the comparison cannot drift out of date the way a number copied
//! into a commit message does.
//!
//! ## The two patterns
//!
//! Neither is invented. Both take their chunk size from the reader that issues them and their
//! *offsets* from the fact that a chunk section starts at a header-length boundary, not a block one —
//! so the reads straddle 512-byte blocks exactly as they do on a real map. An aligned pattern would
//! flatter both paths equally and prove less.
//!
//! - **Route plan** — `obc-reader`'s nav chunks are `NAV_CHUNK_SIZE` = 512 B (OBCM v9 pins the wire
//!   value), read at scattered chunk ids as A\* expands its frontier. This is the pattern #500 was
//!   measured on.
//! - **Render pass** — `obc-reader`'s geometry chunks run to `MAX_CHUNK_BYTES` = 16 KiB, read as the
//!   viewport walks a LOD's chunk grid: a handful of large, mostly-sequential reads.
//!
//! ## What the numbers mean
//!
//! **What these pins do not cover: the cost of *getting* to a read.** Both columns start counting
//! after the object is open — after `FlatStore::open`'s catalog binary search on one side, and after
//! `ExtentTable::build`'s FAT chain walk on the other. That is the right boundary for comparing read
//! paths, and it means an open-cost regression is **invisible here**: a store whose `open` grew a
//! chain walk would pass every assertion in this file. `cost.rs` pins the mount and the commit; the
//! open is pinned by neither, and should be, when the board mount lands.
//!
//! `commands` is what the card charges a per-command handshake for and `blocks` is what it charges
//! transfer time for; `cost.rs` fits both to glass measurements. The two paths do not have to be
//! *equal* — the flat store issues fewer, wider commands, because
//! [`READ_BATCH`](crate::fat_extents::READ_BATCH) caps a `fat_extents` command at 8 blocks and the
//! flat read path is capped only by the extent run. Equal **blocks** with fewer **commands** is the
//! expected shape, and strictly-not-worse on both is what the assertions enforce.

use std::vec;
use std::vec::Vec;

use obc_formats::io::ByteSource;

use crate::fat_extents::tests::{mkfs_fat32, pattern, setup, RecordingDevice};
use crate::fat_extents::{ExtentSource, ExtentTable};

use super::layout::{Geometry, EXTENT_AREA};
use super::seam::StoreId;
use super::seam::{DisplayName, EntryFlags, EntryMeta, Mutation, ObjectKind, PutSource, Revision, Store};
use super::sim::{MediaOp, SparseDisk};
use super::store::FlatStore;

const STORE: StoreId = StoreId([0x6d; 16]);

/// The object both paths serve: 1 MiB, which is one default-geometry extent on the flat side and one
/// FAT run on the other. Contiguous on both, deliberately — a fragmented comparison would measure the
/// two allocators against each other rather than the two read paths.
const OBJECT_BLOCKS: usize = 2 * 1024;
const OBJECT_LEN: usize = OBJECT_BLOCKS * 512;

/// `obc-reader`'s nav chunk (`NAV_CHUNK_SIZE`, pinned by OBCM v9) and geometry chunk ceiling
/// (`MAX_CHUNK_BYTES`). Named here rather than imported: `obc-storage` does not depend on
/// `obc-reader`, and inverting that for a test would be the wrong direction entirely.
const NAV_CHUNK: usize = 512;
const GEO_CHUNK: usize = 16 * 1024;

/// Where a chunk section begins. Not block-aligned, because a real one is not: it follows a header
/// and a directory, so every chunk read straddles a block boundary.
const NAV_DATA_START: u32 = 8_197;
const GEO_DATA_START: u32 = 3_449;

/// Card reads one pass issued: the commands, and the blocks they carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReadCensus {
    commands: u64,
    blocks: u64,
}

/// The windows a pass reads, as `(offset, length)`.
type Pass = Vec<(u64, usize)>;

/// A\* expanding a frontier: 64 nav chunks at scattered ids. The ids are a fixed low-discrepancy walk
/// (an odd stride over a power-of-two chunk count, so it visits each once without repeating) rather
/// than a random sequence — a pinned count needs a pinned pattern.
fn route_plan_pass() -> Pass {
    let chunks = (OBJECT_LEN - NAV_DATA_START as usize) / NAV_CHUNK;
    (0..64)
        .map(|step: u32| {
            let id = (step.wrapping_mul(181) % chunks as u32) as usize;
            (u64::from(NAV_DATA_START) + (id * NAV_CHUNK) as u64, NAV_CHUNK)
        })
        .collect()
}

/// A viewport walking a LOD's chunk grid: 8 geometry chunks, mostly sequential, with one backward
/// step — panning is not monotonic, and a backward seek is exactly what #500 punished.
fn render_pass() -> Pass {
    let order = [0usize, 1, 2, 3, 2, 4, 5, 6];
    order.iter().map(|&id| (u64::from(GEO_DATA_START) + (id * GEO_CHUNK) as u64, GEO_CHUNK)).collect()
}

/// Run `pass` against `source` and check every byte against the ground-truth pattern, so a census
/// can never be of a path that read the wrong bytes.
fn read_pass(source: &dyn ByteSource, pass: &Pass, tag: u8) {
    for &(offset, len) in pass {
        let mut got = vec![0u8; len];
        source.read_at(offset, &mut got).expect("the pass stays inside the object");
        assert_eq!(got, pattern(tag, offset as usize, len), "read at ({offset}, {len}) served the wrong bytes");
    }
}

/// The `fat_extents` column: the same file, the same pass, counted at the block device.
fn fat_census(pass: &Pass) -> ReadCensus {
    let fs = setup(mkfs_fat32(), &["MAP.BIN"], OBJECT_BLOCKS);
    let (entry_block, entry_offset, len) = fs.entry_facts("MAP.BIN");
    assert_eq!(len as usize, OBJECT_LEN);
    let table = ExtentTable::build(fs.disk, entry_block, entry_offset, len).expect("a contiguous file maps");
    assert_eq!(table.extent_count(), 1, "the comparison object must be one run on the FAT side");

    let dev = RecordingDevice { disk: fs.disk, reads: core::cell::RefCell::new(Vec::new()) };
    read_pass(&ExtentSource::new(&dev, &table), pass, 0);
    let reads = dev.reads.borrow();
    ReadCensus { commands: reads.len() as u64, blocks: reads.iter().map(|(_, n)| *n as u64).sum() }
}

/// The flat-store column: the same bytes as one committed object, the same pass, counted at the same
/// place — `SparseDisk`'s ledger is the block device, so both columns count the identical thing.
fn flat_census(pass: &Pass) -> ReadCensus {
    let blocks = EXTENT_AREA + Geometry::DEFAULT.extent_blocks() * 8;
    let disk = SparseDisk::blank(blocks, 11);
    let store = FlatStore::initialize(&disk, STORE).expect("an expressible card initializes");

    let id = store.next_object_id();
    let mut allocation = store.allocate(OBJECT_LEN as u64).expect("one extent is available");
    store.write(&mut allocation, &pattern(0, 0, OBJECT_LEN)).expect("the payload fits its reservation");
    let meta = EntryMeta {
        id,
        revision: Revision(1),
        kind: ObjectKind::MapShard,
        flags: EntryFlags::NONE,
        payload_len: OBJECT_LEN as u64,
        payload_crc: 0,
        name: DisplayName::new("core").expect("a short name"),
    };
    store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]).expect("the commit lands");

    let source = store.source(id, None).expect("the committed object opens");
    let before = disk.ledger().len();
    read_pass(&source, pass, 0);
    let census = {
        let ledger = disk.ledger();
        let reads = ledger[before..].iter().filter(|(_, kind, _)| *kind == MediaOp::Read);
        ReadCensus { commands: reads.clone().count() as u64, blocks: reads.map(|(_, _, n)| *n).sum() }
    };

    // The seam's own rule, exercised rather than described: the source surrenders its handle and the
    // store closes it. Dropping it instead would trip `StoreSource`'s debug assertion and fail here.
    let handle = source.release();
    store.close(handle);
    census
}

/// The pin, and the comparison that gives it meaning. `fat_extents` is the bar because it is what
/// #500 left behind; the flat path has to clear it on both counts.
#[test]
fn a_render_pass_and_a_route_plan_cost_no_more_than_the_path_they_replace() {
    // (name, pass, the flat census this pins)
    let cases = [
        ("route plan (64 × 512 B nav chunks)", route_plan_pass(), ReadCensus { commands: 128, blocks: 128 }),
        ("render pass (8 × 16 KiB geometry chunks)", render_pass(), ReadCensus { commands: 24, blocks: 264 }),
    ];

    for (name, pass, expected) in cases {
        let fat = fat_census(&pass);
        let flat = flat_census(&pass);
        std::println!("{name}: fat_extents {fat:?} vs flat {flat:?}");

        assert_eq!(flat, expected, "{name}: the flat read path no longer costs the pinned card reads");
        assert!(
            flat.blocks <= fat.blocks,
            "{name}: the flat path reads {} blocks against fat_extents' {} — #500's regression, in the new path",
            flat.blocks,
            fat.blocks,
        );
        assert!(
            flat.commands <= fat.commands,
            "{name}: the flat path issues {} commands against fat_extents' {}",
            flat.commands,
            fat.commands,
        );
    }
}

/// The property behind the pin: a read costs the blocks it spans and nothing else — no chain walk, no
/// directory, no per-read fixed cost that scales with *offset*. That last one is #500 restated, and it
/// is what a pinned total alone would not catch: a path that read a constant 40 extra blocks would
/// still be "no worse than `fat_extents`" on a 64-chunk pass while being catastrophic on a 4,000-chunk
/// one.
///
/// So this reads one chunk at the front of the object and one at the back and requires them to cost
/// the same. The two offsets are congruent mod 512 on purpose: a read's cost legitimately depends on
/// how it straddles blocks, so holding alignment fixed is what isolates the thing under test —
/// *distance into the object* — from the thing that is allowed to matter.
#[test]
fn a_read_costs_the_same_at_the_back_of_an_object_as_at_the_front() {
    let back_offset = OBJECT_LEN as u32 - 1_024 + NAV_DATA_START % 512;
    assert_eq!(back_offset % 512, NAV_DATA_START % 512, "the two probes must share an alignment");

    let front = flat_census(&vec![(u64::from(NAV_DATA_START), NAV_CHUNK)]);
    let back = flat_census(&vec![(u64::from(back_offset), NAV_CHUNK)]);
    assert_eq!(front, back, "a read's cost must not depend on how far into the object it is (#500)");
    assert_eq!(front, ReadCensus { commands: 2, blocks: 2 }, "an unaligned 512 B chunk spans two blocks");
}
