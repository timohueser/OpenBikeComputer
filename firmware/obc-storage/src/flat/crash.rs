//! The crash matrix: every media operation of every durable path, cut before, during and after,
//! checked against the reference model.
//!
//! `FLAT_Store_Format.md` §5.5 is the obligation this discharges: "A cut anywhere before step 3
//! completes leaves `A` valid with the greater sequence and `B` invalid: the commit did not happen,
//! and every byte it would have made visible is anonymous again." So for each scenario the matrix
//! runs it once on a fresh deterministic card per `(operation, before | during | after)`, reboots,
//! mounts, and requires the result to be the model **before** the scenario or the model **after** it.
//! Nothing in between is admissible, and neither is a read-only mount — a silent rollback and a
//! spurious "catalog unreadable" are failures of the same test.
//!
//! The comparison is the whole state: the catalog's byte image, the commit sequence, the `ObjectId`
//! cursor, the entry listing and the free-extent count. §5.3 is what makes the byte image fair game —
//! "the byte image of the catalog is a function of the store's state" — so this is not "the same
//! entries somehow", it is the same 768 bytes.
//!
//! **What a cut point is.** [`EVERY_WHEN`] for every counted media operation, *plus* three
//! [`When::Inside`] shapes per block of every write that moved more than one — because a batched write
//! is one operation covering many blocks, and a matrix that only cut at its edges would test a body
//! written in ten commands less thoroughly than the same body written in seventy-six. Each scenario
//! pins its own total, so a change that makes cut points disappear fails rather than quietly passing:
//! the count is the argument, and [`cut_points`] is where it is made.
//!
//! The last section is the other failure media produces: one operation refused, the card still there,
//! and a caller that gets to retry. A power cut cannot reach those paths, because after a cut there is
//! no store left to ask — so they get [`FaultOnce`] and a probe each.

use std::vec;
use std::vec::Vec;

use super::catalog::{Entry, Gate};
use super::layout::{
    catalog_gate, slot_block, Geometry, Ranges, BLOCK, CATALOG, EXTENT_AREA, PROGRAM_PAGE, SLOTS, SUPERBLOCK,
};
use super::model::{self, Change, Model, Snapshot};
use super::raw::crc32;
use super::seam::{
    DisplayName, EntryFlags, EntryMeta, Mutation, ObjectId, ObjectKind, PutSource, Revision, RideCheckpoint, Store,
    StoreId,
};
use super::sim::{FaultOnce, FaultPlan, MediaOp, SparseDisk, When, EVERY_WHEN};
use super::store::{FlatStore, Mode, RideRecovery};
use super::superblock::Superblock;

/// A card with 64 extents: enough for a 32 MiB ride reserve and small enough that several hundred of
/// them cost nothing.
const EXTENTS: u32 = 64;
const TOTAL_BLOCKS: u64 = EXTENT_AREA + Geometry::DEFAULT.extent_blocks() * EXTENTS as u64;
const STORE: StoreId = StoreId([0x11; 16]);

type Card<'a> = FlatStore<&'a SparseDisk>;

pub(super) fn entry(
    id: u64,
    revision: u64,
    kind: ObjectKind,
    flags: EntryFlags,
    payload_len: u64,
    name: &str,
    ranges: &[(u16, u16)],
) -> Entry {
    let mut built = Ranges::default();
    for (first, count) in ranges {
        built.push(*first, *count).unwrap();
    }
    Entry {
        meta: EntryMeta {
            id: ObjectId(id),
            revision: Revision(revision),
            kind,
            flags,
            payload_len,
            payload_crc: crc32(&payload(payload_len as usize)),
            name: DisplayName::new(name).unwrap(),
        },
        ranges: built,
    }
}

/// The payload bytes every scenario writes, so a stored CRC means something.
pub(super) fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index * 7 + 11) as u8).collect()
}

/// Installs a card of `blocks` holding `model` in `copy`, without counting a media operation: the
/// state a scenario starts from, not part of what it is cut inside.
fn card_of(seed: u64, blocks: u64, model: &Model, copy: usize) -> SparseDisk {
    let disk = SparseDisk::blank(blocks, seed);
    let superblock = Superblock::for_card(model.store, blocks).expect("an expressible card");
    // The model is what places the payload bytes (`install_payload`) and what a `Snapshot` is compared
    // against, so a model whose geometry is not the one §8 gives *this* card would install every
    // payload at an address the store never reads — and the mismatch would show up as a payload CRC
    // difference three layers away. Asserted rather than derived, because `extents` is the scenario's
    // own number too: the two have to agree on purpose.
    assert_eq!(model.geometry, superblock.geometry, "the model's extent size is not this card's");
    assert_eq!(model.extents, superblock.extent_count(), "the model's extent count is not this card's");
    let superblock = superblock.encode();
    disk.install(SUPERBLOCK[0], &superblock);
    disk.install(SUPERBLOCK[1], &superblock);
    install_catalog(&disk, model, copy);
    disk
}

/// The same on the card size the rest of this file uses.
fn card(seed: u64, model: &Model, copy: usize) -> SparseDisk {
    card_of(seed, TOTAL_BLOCKS, model, copy)
}

/// The card builder every matrix takes, so the same scenario can start from a card whose catalog is in
/// copy A or in copy B — which is what decides whether the commit under test targets B or A.
fn builder(model: &Model, copy: usize) -> impl Fn(u64) -> SparseDisk + '_ {
    move |seed| card(seed, model, copy)
}

/// The same, on a card of a stated size — which is how a matrix runs on a card-scaled geometry.
fn builder_of(blocks: u64, model: &Model, copy: usize) -> impl Fn(u64) -> SparseDisk + '_ {
    move |seed| card_of(seed, blocks, model, copy)
}

/// The catalog, its gate, and every entry's payload bytes. `pub(super)` because the sibling cost tests
/// build their cards the same way, and a second copy of this would be a second definition of what a
/// consistent pre-state is. Installing the payloads is what makes it consistent: the entries claim a
/// length and a CRC, and `Snapshot` reads those bytes back through the seam, so a scenario would
/// otherwise start from a card that already lies.
pub(super) fn install_catalog(disk: &SparseDisk, model: &Model, copy: usize) {
    let body = model.body();
    disk.install(CATALOG[copy], &body);
    let gate = Gate {
        copy: copy as u8,
        store: model.store,
        sequence: model.sequence,
        entry_count: model.entries.len() as u16,
        body_crc: crc32(&body),
    };
    disk.install(catalog_gate(copy), &gate.encode());
    for entry in &model.entries {
        if entry.meta.flags.has(EntryFlags::RESERVED) {
            continue;
        }
        install_payload(disk, model.geometry, entry, &payload(entry.meta.payload_len as usize));
    }
}

/// Writes `bytes` into an entry's extents, following its ranges exactly as §6.1 does.
fn install_payload(disk: &SparseDisk, geometry: Geometry, entry: &Entry, bytes: &[u8]) {
    let mut done = 0usize;
    while done < bytes.len() {
        let located = entry.ranges.locate(geometry, done as u64).expect("the entry covers its payload");
        let run = (bytes.len() - done).min(located.contiguous as usize);
        disk.install(located.block, &bytes[done..done + run]);
        done += run;
    }
}

fn empty() -> Model {
    Model::empty(STORE, EXTENTS)
}

/// A model holding `entries`, as if some earlier commit had published them.
fn holding(entries: &[Entry], sequence: u64) -> Model {
    let mut model = empty();
    model.entries = entries.to_vec();
    model.next_object = entries.iter().map(|entry| entry.meta.id.0).max().unwrap_or(0) + 1;
    model.sequence = sequence;
    model.high_water = sequence;
    model
}

fn snapshot(store: &mut Card) -> Snapshot {
    model::snapshot(store).expect("a mounted store")
}

/// Every cut point a scenario admits: the three [`EVERY_WHEN`] positions of each of its `total` media
/// operations, and [`When::Inside`]'s three shapes for each block of every write that moved more than
/// one.
///
/// Those interior points are what makes a batched write reviewable, and the claim they carry is a
/// containment one rather than an arithmetic one. Take a region written as `n` single-block writes: its
/// cut points were `3n`, and the only ones with a durable effect were the `n` `During` cuts, each
/// tearing the page of one block. Written in windows instead, the same region gets `3n` interior points
/// — `(tear, !durable)` at each block *is* that old `During` cut, block for block — plus one
/// whole-command `During` per window, which tears every page the window touched at once, plus `2n`
/// partial-durable images no single-block write could produce. Strictly more, per block.
///
/// The totals nonetheless fall, and it is worth being exact about where: batching removes **read**
/// operations (a mount's scan and each of a commit's two passes), and a cut inside a read changes no
/// byte on the card — it powers the card off, and what it exercises is the abort path, which is the
/// same one whatever block the scan had reached. Writes are where the durable outcomes are, and no
/// write's coverage got smaller.
fn cut_points(total: u32, widths: &[(u32, u64)]) -> Vec<(u32, When)> {
    let mut cuts = Vec::new();
    for op in 1..=total {
        for when in EVERY_WHEN {
            cuts.push((op, when));
        }
    }
    for (op, width) in widths.iter().filter(|(_, width)| *width > 1) {
        for blocks in 0..*width as u32 {
            for (tear, durable) in [(true, true), (true, false), (false, true)] {
                cuts.push((*op, When::Inside { blocks, tear, durable }));
            }
        }
    }
    cuts
}

/// The card seed one cut point runs on. The three whole-operation positions keep the seed they always
/// had — so the matrix this change inherits runs on exactly the bytes it used to — and an interior cut
/// takes a stream of its own, because two cut points sharing a seed share a torn page's contents too.
fn seed(base: u64, when: When) -> u64 {
    match when {
        When::Inside { blocks, tear, durable } => {
            base + (1 + u64::from(blocks)) * 1_000_003 + u64::from(tear) * 31 + u64::from(durable) * 131
        }
        _ => base,
    }
}

/// The crash matrix for one scenario, over whatever card `build` hands it: the old state or the new
/// one, and nothing else. `cuts` is [`cut_points`]'s census, pinned.
fn matrix(
    name: &str,
    cuts: usize,
    before: &Model,
    after: &Model,
    build: impl Fn(u64) -> SparseDisk,
    scenario: impl Fn(&mut Card),
) {
    matrix_admitting(name, cuts, &[before.snapshot(), after.snapshot()], after, build, scenario);
}

/// The same, for the one scenario whose cut points admit a third state §5.5 names.
fn matrix_admitting(
    name: &str,
    cuts: usize,
    admissible: &[Snapshot],
    expected: &Model,
    build: impl Fn(u64) -> SparseDisk,
    scenario: impl Fn(&mut Card),
) {
    let (total, widths) = {
        let disk = build(1);
        let mut store = FlatStore::mount(&disk);
        let baseline = disk.ops();
        scenario(&mut store);
        let widths: Vec<(u32, u64)> = disk
            .write_widths()
            .into_iter()
            .filter(|(op, _)| *op > baseline)
            .map(|(op, blocks)| (op - baseline, blocks))
            .collect();
        (disk.ops() - baseline, widths)
    };
    assert!(total > 0, "{name}: the scenario performs no media operation");
    let points = cut_points(total, &widths);
    assert_eq!(
        points.len(),
        cuts,
        "{name}: {} cut points over {total} media operations ({} of them multi-block writes), not the \
         {cuts} this scenario is pinned to — a count that *fell* means a cut point stopped existing, \
         which is the one thing batching must not do",
        points.len(),
        widths.iter().filter(|(_, blocks)| *blocks > 1).count(),
    );

    for (op, when) in points {
        let disk = build(seed(u64::from(op) * 31 + 7, when));
        let mut store = FlatStore::mount(&disk);
        disk.plan(FaultPlan { op: disk.ops() + op, when });
        scenario(&mut store);
        disk.reboot();

        let mut store = FlatStore::mount(&disk);
        assert_eq!(store.mode(), Mode::ReadWrite, "{name}: cut at op {op} {when:?} did not mount read-write");
        let recovered = snapshot(&mut store);
        assert!(
            admissible.contains(&recovered),
            "{name}: cut at op {op} {when:?} recovered no admissible state \
             (sequence {}, high water {}, {} entries, ride {:?})",
            recovered.sequence,
            recovered.high_water,
            recovered.entries.len(),
            recovered.ride,
        );
    }
    let after = expected;

    // The fault-free run must land on the new state, or the matrix above proves nothing. The resident
    // state is checked before the remount, so a commit that wrote the right bytes and then lied about
    // them in RAM is caught too.
    let disk = build(99);
    let mut store = FlatStore::mount(&disk);
    scenario(&mut store);
    assert_eq!(snapshot(&mut store), after.snapshot(), "{name}: fault-free run, resident state");
    disk.reboot();
    assert_eq!(snapshot(&mut FlatStore::mount(&disk)), after.snapshot(), "{name}: fault-free run, remounted");
}

/// The same scenario cut on a card whose catalog starts in copy A and again on one where it starts in
/// copy B, so the commit under test targets B in one run and A in the other. §5.5's target is "the copy
/// the store is not serving", and a matrix that only ever wrote B would not have tested that sentence.
fn matrix_both_copies(name: &str, cuts: usize, before: &Model, after: &Model, scenario: impl Fn(&mut Card) + Copy) {
    for copy in [0, 1] {
        matrix(name, cuts, before, after, builder(before, copy), scenario);
    }
}

// -------------------------------------------------------------------------------------------
// §5.5 — the commit
// -------------------------------------------------------------------------------------------

#[test]
fn creating_an_object_recovers_the_old_or_the_new_catalog() {
    let route = entry(1, 1, ObjectKind::Route, EntryFlags::NONE, 3_000, "Grimsel Loop", &[(0, 1)]);
    let before = empty();
    let after = empty().apply(&[Change::Put(route)]).clone();
    matrix_both_copies("create", 48, &before, &after, |store: &mut Card| {
        let Ok(mut allocation) = store.allocate(3_000) else { return };
        if store.write(&mut allocation, &payload(3_000)).is_err() {
            return;
        }
        let _ = store.commit(&[Mutation::Put { meta: route.meta, source: PutSource::Fresh(allocation) }]);
    });
}

#[test]
fn replacing_an_object_is_atomic_and_frees_the_old_extents() {
    let first = entry(1, 1, ObjectKind::Route, EntryFlags::NONE, 3_000, "Grimsel Loop", &[(0, 1)]);
    let second = entry(1, 2, ObjectKind::Route, EntryFlags::NONE, 5_000, "Grimsel Loop", &[(1, 1)]);
    let before = holding(&[first], 4);
    let after = holding(&[first], 4).apply(&[Change::Put(second), Change::Remove(first.meta.key())]).clone();
    matrix_both_copies("replace", 69, &before, &after, |store: &mut Card| {
        let Ok(mut allocation) = store.allocate(5_000) else { return };
        if store.write(&mut allocation, &payload(5_000)).is_err() {
            return;
        }
        let _ = store.commit(&[
            Mutation::Put { meta: second.meta, source: PutSource::Fresh(allocation) },
            Mutation::Remove { id: first.meta.id, revision: first.meta.revision },
        ]);
    });
}

/// Weather's retention: one commit publishes the new head and leaves the displaced revision
/// `RETAINED`, so a reader mid-stream and a domain that wants continuity both still have bytes.
#[test]
fn retaining_the_previous_revision_is_one_commit() {
    let old = entry(1, 1, ObjectKind::WeatherBundle, EntryFlags::NONE, 3_000, "", &[(0, 1)]);
    let retained = Entry { meta: EntryMeta { flags: EntryFlags::RETAINED, ..old.meta }, ..old };
    let new = entry(1, 2, ObjectKind::WeatherBundle, EntryFlags::NONE, 3_000, "", &[(1, 1)]);
    let before = holding(&[old], 4);
    let after = holding(&[old], 4).apply(&[Change::Put(retained), Change::Put(new)]).clone();
    matrix_both_copies("retain", 57, &before, &after, |store: &mut Card| {
        let Ok(mut allocation) = store.allocate(3_000) else { return };
        if store.write(&mut allocation, &payload(3_000)).is_err() {
            return;
        }
        let _ = store.commit(&[
            Mutation::Put { meta: new.meta, source: PutSource::Fresh(allocation) },
            Mutation::Put { meta: retained.meta, source: PutSource::Amend },
        ]);
    });
}

#[test]
fn removing_an_object_recovers_the_old_or_the_new_catalog() {
    let route = entry(1, 1, ObjectKind::Route, EntryFlags::NONE, 3_000, "Grimsel Loop", &[(0, 1)]);
    let trip = entry(2, 1, ObjectKind::Trip, EntryFlags::NONE, 600, "Alps", &[(1, 1)]);
    let before = holding(&[route, trip], 9);
    let after = holding(&[route, trip], 9).apply(&[Change::Remove(route.meta.key())]).clone();
    matrix_both_copies("remove", 30, &before, &after, |store: &mut Card| {
        let _ = store.commit(&[Mutation::Remove { id: route.meta.id, revision: route.meta.revision }]);
    });
}

/// §5.5's target rule, from the case that motivates it: mount fell back to the older copy because the
/// newer one's gate is well-formed but its body fails, so the commit must target the copy it is *not*
/// serving. A commit that targeted the greater sequence instead would leave the card with no valid
/// catalog at all — and the sequence it writes must still clear the ill-formed copy's high-water mark.
#[test]
fn a_commit_after_a_fallback_targets_the_copy_it_is_not_serving() {
    let route = entry(1, 1, ObjectKind::Route, EntryFlags::NONE, 3_000, "Grimsel Loop", &[(0, 1)]);
    let trip = entry(2, 1, ObjectKind::Trip, EntryFlags::NONE, 600, "Alps", &[(1, 1)]);
    let mut served = holding(&[route], 7);
    // Copy B's gate is well-formed at 8 even though its body is not, so that is the mark the store
    // continues from — `Snapshot` carries it, and a commit that reissued 8 would be caught here.
    served.high_water = 8;
    // Copy B carries a well-formed gate at sequence 8 over a body that does not match it.
    let poisoned = {
        let mut model = holding(&[route, trip], 8);
        model.next_object = 3;
        model
    };
    let build = |seed: u64| {
        let disk = card(seed, &served, 0);
        install_catalog(&disk, &poisoned, 1);
        let mut torn = disk.block(CATALOG[1] + 1);
        torn[0] ^= 0xFF;
        disk.install(CATALOG[1] + 1, &torn);
        disk
    };

    let disk = build(3);
    let mut store = FlatStore::mount(&disk);
    assert_eq!(store.serving_copy(), 0, "the fallback did not select the copy that validates");
    assert_eq!(snapshot(&mut store), served.snapshot());

    let published = entry(2, 1, ObjectKind::Trip, EntryFlags::NONE, 600, "Alps", &[(1, 1)]);
    let mut allocation = store.allocate(600).unwrap();
    store.write(&mut allocation, &payload(600)).unwrap();
    store.commit(&[Mutation::Put { meta: published.meta, source: PutSource::Fresh(allocation) }]).unwrap();
    assert_eq!(store.serving_copy(), 1, "the commit did not target the copy it was not serving");
    assert_eq!(store.sequence(), 9, "the sequence did not clear every well-formed gate's high-water mark");

    // And the whole path, cut everywhere: the served copy is never the one destroyed, and the sequence
    // the commit lands on is one past the ill-formed copy's, not one past the served copy's.
    //
    // Three states are admissible here rather than two, and §5.5 says which third: "the mark lives only
    // in the gates and step 1 erases the one that carried it". A cut once step 1 is durable leaves the
    // served copy alone but the high-water mark back at 7 — the same catalog, a lower mark, and the
    // reason the spec calls the sequence a staleness hint rather than a version.
    let after = served.clone().apply(&[Change::Put(published)]).clone();
    let mark_erased = {
        let mut model = served.clone();
        model.high_water = 7;
        model.snapshot()
    };
    let admissible = [served.snapshot(), mark_erased, after.snapshot()];
    matrix_admitting("fallback commit", 39, &admissible, &after, build, |store: &mut Card| {
        let Ok(mut allocation) = store.allocate(600) else { return };
        if store.write(&mut allocation, &payload(600)).is_err() {
            return;
        }
        let _ = store.commit(&[Mutation::Put { meta: published.meta, source: PutSource::Fresh(allocation) }]);
    });
}

/// A catalog whose body spans program pages, cut everywhere inside it. §5.5 step 1 exists for exactly
/// this shape — "`B`'s old gate would still name an old entry count and an old body CRC, both of which a
/// partially written body can accidentally satisfy in the prefix the count selects" — and a body of one
/// block can never produce that partial prefix. This one is 151 blocks over five program pages, so a
/// cut lands inside it, tears pages of it, and leaves the rest of the old body underneath.
#[test]
fn a_multi_page_catalog_body_recovers_the_old_or_the_new_state() {
    const ENTRIES: u64 = 600;
    const BIG: u32 = ENTRIES as u32 + 8;
    let mut before = Model::empty(STORE, BIG);
    // One object with bytes to read back, and 599 rollback reserves behind it. What this fixture is for
    // is the *length* of the body, and a reserve is the cheapest entry that occupies its 128 bytes: the
    // store never wrote its payload, so no reader has to walk 600 of them to prove the card is intact.
    before.entries.push(entry(1, 1, ObjectKind::Trip, EntryFlags::NONE, 8, "witness", &[(0, 1)]));
    for id in 2..=ENTRIES {
        before.entries.push(entry(
            id,
            1,
            ObjectKind::RollbackReserve,
            EntryFlags::RESERVED,
            0,
            "",
            &[(id as u16 - 1, 1)],
        ));
    }
    before.next_object = ENTRIES + 1;
    before.sequence = 4;
    before.high_water = 4;
    // Five program pages of body, so the write is not one page and not one block.
    assert_eq!(super::layout::body_len(ENTRIES as u16).div_ceil(BLOCK), 151);

    let removed = before.entries[1];
    let after = before.clone().apply(&[Change::Remove(removed.meta.key())]).clone();
    let blocks = EXTENT_AREA + Geometry::DEFAULT.extent_blocks() * BIG as u64;
    let build = |seed: u64| {
        let disk = SparseDisk::blank(blocks, seed);
        let superblock = Superblock::for_card(STORE, blocks).expect("an expressible card").encode();
        disk.install(SUPERBLOCK[0], &superblock);
        disk.install(SUPERBLOCK[1], &superblock);
        install_catalog(&disk, &before, 0);
        disk
    };
    matrix("multi-page body", 663, &before, &after, build, |store: &mut Card| {
        let _ = store.commit(&[Mutation::Remove { id: removed.meta.id, revision: removed.meta.revision }]);
    });
}

/// Steady state: both copies valid, and a commit reuses the one that is not being served. The cut
/// matrices above start from a single valid copy, which is the *initial* state, not the steady one.
#[test]
fn a_steady_state_pair_alternates_and_never_serves_a_stale_copy() {
    let disk = card(5, &empty(), 0);
    let mut store = FlatStore::mount(&disk);
    let mut model = empty();
    for revision in 1..=5u64 {
        // The extents alternate as the copies do, and for the same reason: the reservation is taken
        // before the commit that frees the revision it replaces.
        let extent = ((revision - 1) % 2) as u16;
        let published = entry(1, revision, ObjectKind::Route, EntryFlags::NONE, 3_000, "Grimsel Loop", &[(extent, 1)]);
        let mut allocation = store.allocate(3_000).unwrap();
        store.write(&mut allocation, &payload(3_000)).unwrap();
        let mut batch = vec![Mutation::Put { meta: published.meta, source: PutSource::Fresh(allocation) }];
        if revision > 1 {
            batch.push(Mutation::Remove { id: ObjectId(1), revision: Revision(revision - 1) });
        }
        let sequence = store.commit(&batch).unwrap();
        assert_eq!(sequence, revision + 1, "the commit sequence did not increment by exactly one");
        assert_eq!(store.serving_copy(), (revision % 2) as usize, "the copies did not alternate");

        let mut changes = vec![Change::Put(published)];
        if revision > 1 {
            changes.push(Change::Remove((ObjectId(1), Revision(revision - 1))));
        }
        model.apply(&changes);
        assert_eq!(snapshot(&mut store), model.snapshot());
    }
    // Every revision reused extent 0, because the previous one was freed at its gate.
    assert_eq!(store.free_extents(), EXTENTS - 1);
}

// -------------------------------------------------------------------------------------------
// §8 — initialization
// -------------------------------------------------------------------------------------------

/// §8's bracket, cut at every point: the superblocks are destroyed first and written last, so a card
/// that mounts at all mounts as exactly one of two whole stores — the old one, if the cut landed
/// before step 1 became durable, or the new empty one. Anything in between classifies as *not a flat
/// store*, and a mount that produced a valid `StoreId` over no catalog would be the failure this
/// ordering exists to prevent.
#[test]
fn initialization_produces_no_store_or_a_complete_empty_one() {
    let expected = empty().snapshot();
    // The card being re-initialized carries a different identity, so "which store did I mount" is a
    // question with an answer. Re-initialization is the ordinary case — old cards are re-initialized
    // rather than migrated — and it is what §8 step 1 exists for.
    let old = {
        let mut model = Model::empty(StoreId([0x22; 16]), EXTENTS);
        model.entries = vec![entry(9, 4, ObjectKind::Route, EntryFlags::NONE, 3_000, "old", &[(7, 1)])];
        model.next_object = 10;
        model.sequence = 12;
        model.high_water = 12;
        model
    };
    let (total, widths) = {
        let disk = SparseDisk::blank(TOTAL_BLOCKS, 1);
        FlatStore::initialize(&disk, STORE).unwrap();
        (disk.ops(), disk.write_widths())
    };
    let points = cut_points(total, &widths);
    assert_eq!(points.len(), 99, "initialization: {} cut points, not the pinned count", points.len());
    for (op, when) in points {
        let disk = card(seed(u64::from(op) * 23 + 3, when), &old, 0);
        let baseline = disk.ops();
        disk.plan(FaultPlan { op: baseline + op, when });
        let _ = FlatStore::initialize(&disk, STORE);
        disk.reboot();

        let mut store = FlatStore::mount(&disk);
        let recovered = store.store_id();
        match store.mode() {
            Mode::Unformatted => {}
            Mode::ReadWrite if recovered == STORE => assert_eq!(
                snapshot(&mut store),
                expected,
                "initialization: cut at op {op} {when:?} mounted the new store over something else",
            ),
            Mode::ReadWrite => {
                assert_eq!(recovered, old.store, "cut at op {op} {when:?} mounted an identity from nowhere");
                assert_eq!(
                    snapshot(&mut store),
                    old.snapshot(),
                    "initialization: cut at op {op} {when:?} left the old store half-erased",
                );
            }
            other => panic!("initialization: cut at op {op} {when:?} mounted {other:?}"),
        }
    }

    let disk = SparseDisk::blank(TOTAL_BLOCKS, 7);
    let mut store = FlatStore::initialize(&disk, STORE).unwrap();
    assert_eq!(snapshot(&mut store), expected);
    assert_eq!(store.free_extents(), EXTENTS);
    assert_eq!(store.serving_copy(), 0, "§8: a mount after step 6 finds copy A valid and copy B ill-formed");
}

// -------------------------------------------------------------------------------------------
// §6 and §8 — the card-scaled extent size
// -------------------------------------------------------------------------------------------

/// A real "128 GB" card: 250,000,000 blocks, 128 billion bytes, 119.2 GiB. A fixed 1 MiB extent could
/// not have addressed it at all — 122,068 extents against an index that names 65,536.
///
/// **A decimal size on purpose.** `128e9 / 65,536` is 1,953,125 bytes, which is not a power of two, so
/// this card gets 2 MiB only *because* §8 rounds up; a rule that rounded down or truncated would give
/// it 1 MiB, and a card the superblock decoder then refuses. A card of exactly 128 GiB — the tidy
/// number — divides to 2 MiB on the nose and would let that mutation through, which is why the
/// scenarios below use this one and §4.1's second vector uses the tidy one.
///
/// The card is virtual and [`SparseDisk`] is a map of the blocks somebody wrote, so it costs exactly
/// what the 68 MiB one the rest of this file uses does.
const BIG_CARD_BLOCKS: u64 = 250_000_000;
/// The extents §6 recomputes from it at the 2 MiB §8 gives it.
const BIG_CARD_EXTENTS: u32 = 61_034;

/// Initialization on a card past the index's reach, and then the whole seam on the geometry it chose:
/// an allocation, a payload written in awkward pieces across a range boundary, a commit, a read back,
/// and a remount that agrees with all of it.
///
/// The payload deliberately spans two extents *at the recorded size*, so `locate` crosses a range
/// boundary at 2 MiB and every LBA it computes is 2,048 blocks further along than the same index would
/// have been on a 1 MiB card.
#[test]
fn a_card_the_index_cannot_reach_at_one_mib_gets_bigger_extents() {
    let disk = SparseDisk::blank(BIG_CARD_BLOCKS, 3);
    let mut store = FlatStore::initialize(&disk, STORE).expect("a 128 GB card formats");
    let extent = 2 << 20;
    assert_eq!(store.extent_size(), extent, "§8: 128e9 / 65,536 is 1,953,125, which rounds up to 2 MiB");
    assert_eq!(store.free_extents(), BIG_CARD_EXTENTS, "§6's count at that size");
    // The rounding is what makes this card representable at all: at the size a truncating rule would
    // have picked, its own extent count is past the index and §4 refuses the superblock.
    assert_eq!(
        Superblock { store: STORE, total_blocks: BIG_CARD_BLOCKS, geometry: Geometry::DEFAULT }.extent_count(),
        122_068,
    );

    // Extent 0 is taken first, so the object below starts at extent 1 and its second extent is 2.
    let bytes = payload(extent as usize + 4_242);
    let mut allocation = store.allocate(bytes.len() as u64).unwrap();
    for chunk in bytes.chunks(777) {
        store.write(&mut allocation, chunk).unwrap();
    }
    let published = entry(1, 1, ObjectKind::MapShard, EntryFlags::NONE, bytes.len() as u64, "shard", &[(0, 2)]);
    store.commit(&[Mutation::Put { meta: published.meta, source: PutSource::Fresh(allocation) }]).unwrap();
    assert_eq!(
        store.free_extents(),
        BIG_CARD_EXTENTS - 2,
        "two 2 MiB extents cover a payload two 1 MiB ones would not",
    );

    // The second extent's first payload byte sits 2,048 blocks past the first's, which is the whole
    // claim: the same extent index means twice as many blocks on this card.
    let handle = store.open(ObjectId(1), None).unwrap();
    let mut buf = [0u8; 1_000];
    for offset in [0u64, 1, 511, 512, extent - 3, extent, extent + 4_000] {
        let read = store.read(&handle, offset, &mut buf).unwrap();
        assert_eq!(&buf[..read], &bytes[offset as usize..offset as usize + read], "offset {offset}");
    }
    assert_eq!(
        disk.block(EXTENT_AREA + Geometry::from_log2(21).unwrap().extent_blocks())[..16],
        bytes[extent as usize..extent as usize + 16],
        "extent 1 does not begin where a 2 MiB stride puts it",
    );
    store.close(handle);

    let resident = snapshot(&mut store);
    disk.reboot();
    let mut remounted = FlatStore::mount(&disk);
    assert_eq!(remounted.extent_size(), extent, "the size came back off the card, not out of a constant");
    assert_eq!(snapshot(&mut remounted), resident, "the remount disagrees with the store that wrote it");
}

/// The same, one doubling further out, and the ride path with it: a 512 GiB card gets 8 MiB extents, a
/// 32 MiB reserve is four of them rather than thirty-two, and a checkpoint's 16 KiB payload page still
/// lands page-aligned inside the first one (§6.1).
#[test]
fn a_ride_records_and_recovers_on_a_card_scaled_geometry() {
    let disk = SparseDisk::blank((512 << 30) / BLOCK as u64, 11);
    let mut store = FlatStore::initialize(&disk, STORE).expect("a 512 GiB card formats");
    assert_eq!(store.extent_size(), 8 << 20, "§8: 512 GiB / 65,536 is 8 MiB");
    assert_eq!(store.free_extents(), 65_535);

    let reserve = 32 << 20;
    let allocation = store.allocate(reserve).unwrap();
    let ride = entry(1, 1, ObjectKind::Ride, EntryFlags::RECORDING, 0, "", &[(0, 4)]);
    store.commit(&[Mutation::Put { meta: ride.meta, source: PutSource::Fresh(allocation) }]).unwrap();
    assert_eq!(store.free_extents(), 65_531, "§7.2's 32 MiB reserve is four extents at this size");

    // Two checkpoints, the second past a whole payload page, so §7.2 step 2 flushes one into the
    // ride's own extents at an address derived from the 8 MiB stride.
    let mut rider = Rider::new();
    rider.grow(200);
    assert!(rider.checkpoint(&mut store));
    rider.grow(PROGRAM_PAGE + 300);
    assert!(rider.checkpoint(&mut store));
    assert_eq!(rider.flushed, PROGRAM_PAGE, "the checkpoint flushed no page");

    disk.reboot();
    let store = FlatStore::mount(&disk);
    let (expected, tail) = rider.expect(2);
    assert_eq!(store.recovered_ride(), Some(expected), "§7.3 recovered the wrong checkpoint");
    let mut recovered = vec![0u8; tail.len()];
    assert_eq!(store.recovered_tail(&mut recovered).unwrap(), tail.len());
    assert_eq!(recovered, tail);
    // And the flushed page is where §6.1 says it is: payload offset 0 of extent 0.
    assert_eq!(disk.block(EXTENT_AREA)[..8], rider.payload[..8], "the flushed page missed the extent area");
}

/// A model of a card-scaled card: the geometry §8 gives [`BIG_CARD_BLOCKS`] and the count §6
/// recomputes, which [`card_of`] then holds the card to.
fn big_card_model(entries: &[Entry], sequence: u64) -> Model {
    let mut model = Model::empty(STORE, BIG_CARD_EXTENTS);
    model.geometry = Geometry::from_log2(21).expect("2 MiB");
    model.entries = entries.to_vec();
    model.next_object = entries.iter().map(|entry| entry.meta.id.0).max().unwrap_or(0) + 1;
    model.sequence = sequence;
    model.high_water = sequence;
    model
}

/// §5.5's commit, cut everywhere, on a card whose extents are 2 MiB.
///
/// **This is where a cut is worth taking at a non-default geometry, and initialization is not.** §8
/// writes the same blocks at every extent size — two superblocks, a gate, sixteen slot headers, a
/// body — so an initialization matrix at 2 MiB re-runs the default one's cut points against
/// byte-identical media traffic and pins nothing the geometry can break. A commit does depend on it:
/// the payload's staged block goes to an address `locate` derived at the recorded stride, §5.3's
/// covering rule trims the published entry against that stride, and the mount that follows every cut
/// rebuilds the free map at this card's count and reads every payload back through it. A geometry the
/// store dropped or halved anywhere in that chain surfaces here as a payload CRC or a free-extent
/// count that no admissible state carries.
#[test]
fn a_commit_on_a_card_scaled_geometry_recovers_the_old_or_the_new_catalog() {
    // The pre-state payload sits at extent **3**, not extent 0: extent 0 is the one address every
    // stride agrees on, so a scenario whose only installed bytes live there would pass at a geometry
    // the store had dropped. At extent 3 the harness installs 2 MiB × 3 along and a store reading at
    // 1 MiB × 3 finds nothing. The commit's own object then takes extent 0, which first-fit hands it.
    let first = entry(1, 1, ObjectKind::Route, EntryFlags::NONE, 3_000, "Grimsel Loop", &[(3, 1)]);
    let second = entry(1, 2, ObjectKind::Route, EntryFlags::NONE, 5_000, "Grimsel Loop", &[(0, 1)]);
    let before = big_card_model(&[first], 4);
    let after = big_card_model(&[first], 4).apply(&[Change::Put(second), Change::Remove(first.meta.key())]).clone();
    for copy in [0, 1] {
        matrix(
            "replace @ 2 MiB",
            69,
            &before,
            &after,
            builder_of(BIG_CARD_BLOCKS, &before, copy),
            |store: &mut Card| {
                let Ok(mut allocation) = store.allocate(5_000) else { return };
                if store.write(&mut allocation, &payload(5_000)).is_err() {
                    return;
                }
                let _ = store.commit(&[
                    Mutation::Put { meta: second.meta, source: PutSource::Fresh(allocation) },
                    Mutation::Remove { id: first.meta.id, revision: first.meta.revision },
                ]);
            },
        );
    }
}

/// A fill pass over a run wider than a device `usize`: 32,768 extents of 64 MiB, coalesced into one
/// 2 TiB range, on a 4 TiB card.
///
/// What it holds is **progress** — that one `write` consumes every byte it was given, at a geometry
/// where the block count of the contiguous run is exactly `2^32`. The arithmetic that produces it is
/// [`Located::whole_blocks`], and this test cannot fail on a 64-bit host for the reason stated there;
/// what it does cover on any target is a pass that writes fewer blocks than it should, or none, from
/// any cause — an inverted bound, a byte/block mix-up, a `locate` that reports the wrong run.
#[test]
fn a_write_through_a_two_tebibyte_run_consumes_its_input() {
    use super::device::BlockDevice as _;

    let disk = SparseDisk::blank((4u64 << 40) / BLOCK as u64, 5);
    let mut store = FlatStore::initialize(&disk, STORE).expect("a 4 TiB card formats");
    assert_eq!(store.extent_size(), 64 << 20, "§8: 4 TiB / 65,536 is 64 MiB");
    let free = store.free_extents();

    let mut allocation = store.allocate(2 << 40).expect("half the card, in one first-fit run");
    let bytes = payload(4 * BLOCK + 300);
    store.write(&mut allocation, &bytes).expect("the write is refused, not short");
    assert_eq!(allocation.written, bytes.len() as u64, "a fill pass consumed none of its input");

    // The bytes are still in the card's volatile cache — a `write` promises nothing durable, and the
    // commit that would sync them is not this test's subject — so the sync is the harness's.
    (&disk).sync().unwrap();
    assert_eq!(disk.block(EXTENT_AREA)[..16], bytes[..16], "the first block missed the extent area");
    assert_eq!(disk.block(EXTENT_AREA + 3)[..16], bytes[3 * BLOCK..3 * BLOCK + 16], "a later block did not land");

    store.cancel(allocation);
    assert_eq!(store.free_extents(), free, "the cancel did not return the 32,768-extent run");
}

/// The two geometry refusals, on a whole card rather than on 512 bytes. Both are §5.6 step 1's "not a
/// flat store" — the card's addresses are unreadable, so there is no store here to serve read-only
/// either — and both are reachable only by forgery: §8 writes neither.
#[test]
fn a_superblock_whose_geometry_is_inadmissible_is_not_a_flat_store() {
    // A 128 GiB card recorded with 1 MiB extents: 131,070 extents, which the entry's `u16` cannot name.
    let understated = Superblock { store: STORE, total_blocks: BIG_CARD_BLOCKS, geometry: Geometry::DEFAULT }.encode();
    // And a size below the 1 MiB minimum, re-stamped so the CRC still checks.
    let mut too_small = Superblock::for_card(STORE, TOTAL_BLOCKS).expect("an expressible card").encode();
    too_small[32] = 19;
    let crc = crc32(&too_small[..504]);
    too_small[504..508].copy_from_slice(&crc.to_le_bytes());

    for (name, superblock, blocks) in [
        ("an unnameable extent count", understated, BIG_CARD_BLOCKS),
        ("a sub-minimum extent", too_small, TOTAL_BLOCKS),
    ] {
        let disk = SparseDisk::blank(blocks, 19);
        // A real catalog underneath, so what the mount refuses is the geometry and nothing else.
        FlatStore::initialize(&disk, STORE).expect("the card formats first");
        disk.install(SUPERBLOCK[0], &superblock);
        disk.install(SUPERBLOCK[1], &superblock);
        assert_eq!(FlatStore::mount(&disk).mode(), Mode::Unformatted, "{name} was mounted");
    }
}

// -------------------------------------------------------------------------------------------
// §7 — the ride
// -------------------------------------------------------------------------------------------

fn recording() -> Entry {
    entry(1, 1, ObjectKind::Ride, EntryFlags::RECORDING, 0, "", &[(0, 32)])
}

#[test]
fn starting_a_ride_recovers_the_old_or_the_new_catalog() {
    let ride = recording();
    let before = empty();
    let after = empty().apply(&[Change::Put(ride)]).clone();
    matrix_both_copies("ride start", 27, &before, &after, |store: &mut Card| {
        let Ok(allocation) = store.allocate(32 << 20) else { return };
        let _ = store.commit(&[Mutation::Put { meta: ride.meta, source: PutSource::Fresh(allocation) }]);
    });
}

/// Installs one journal slot, uncounted: the state a ride that has been recording leaves behind, and
/// what a finalising commit has to move into the ride's extents.
fn install_slot(disk: &SparseDisk, ride: &Entry, sequence: u64, flushed: u64, tail: &[u8]) {
    let slot = super::journal::Slot {
        slot: (sequence % SLOTS as u64) as u16,
        id: ride.meta.id,
        revision: ride.meta.revision,
        sequence,
        flushed,
        tail_len: tail.len() as u32,
        payload_crc: crc32(&payload((flushed + tail.len() as u64) as usize)),
        ranges: ride.ranges,
        slot_crc: 0,
    };
    let base = slot_block(slot.slot as usize);
    disk.install(base, &slot.seal(&STORE, tail));
    let mut padded = tail.to_vec();
    padded.resize(super::journal::TAIL_CAPACITY, 0);
    disk.install(base + 1, &padded);
}

/// A card carrying a ride that has recorded `flushed + tail` bytes: the catalog entry, and the slot
/// those trailing bytes live in.
fn recording_card<'m>(
    model: &'m Model,
    ride: &'m Entry,
    flushed: u64,
    tail_len: usize,
) -> impl Fn(u64) -> SparseDisk + 'm {
    let whole = payload((flushed as usize) + tail_len);
    move |seed| {
        let disk = card(seed, model, 0);
        // The pages §7.2 already flushed are in the ride's extents; the rest is in the newest slot.
        install_payload(&disk, model.geometry, ride, &whole[..flushed as usize]);
        install_slot(&disk, ride, 1, flushed, &whole[flushed as usize..]);
        disk
    }
}

/// Ride end: one commit clears `RECORDING`, sets the final length and CRC, trims the ranges to what the
/// payload needs, and — because a checkpoint only ever flushed whole 16 KiB pages — moves the last
/// partial page out of the journal slot and into the ride's extents. The read-back in `Snapshot` is
/// what holds it to that: the entry states a CRC over bytes that have to be on the card.
#[test]
fn finalising_a_ride_publishes_the_bytes_it_claims() {
    const FLUSHED: u64 = PROGRAM_PAGE as u64;
    const TAIL: usize = 5_000;
    let ride = recording();
    let length = FLUSHED + TAIL as u64;
    let finalised = entry(1, 1, ObjectKind::Ride, EntryFlags::NONE, length, "Tuesday", &[(0, 1)]);

    let mut before = holding(&[ride], 6);
    before.ride = Some((FLUSHED, length));
    let after = {
        let mut model = holding(&[ride], 6);
        model.apply(&[Change::Put(finalised)]);
        model
    };
    matrix("ride end", 147, &before, &after, recording_card(&before, &ride, FLUSHED, TAIL), |store: &mut Card| {
        let _ = store.commit(&[Mutation::Put { meta: finalised.meta, source: PutSource::Amend }]);
    });
}

/// The same commit, from the other end of §7.2's page boundary: a ride shorter than one program page has
/// flushed nothing at all, so every byte it publishes comes out of the slot.
#[test]
fn finalising_a_ride_shorter_than_one_page_publishes_all_of_it() {
    let ride = recording();
    let finalised = entry(1, 1, ObjectKind::Ride, EntryFlags::NONE, 900, "Short", &[(0, 1)]);
    let mut before = holding(&[ride], 6);
    before.ride = Some((0, 900));
    let after = {
        let mut model = holding(&[ride], 6);
        model.apply(&[Change::Put(finalised)]);
        model
    };
    matrix("short ride end", 99, &before, &after, recording_card(&before, &ride, 0, 900), |store: &mut Card| {
        let _ = store.commit(&[Mutation::Put { meta: finalised.meta, source: PutSource::Amend }]);
    });
}

/// A finalising commit that claims a length no slot can account for is refused, and the card is
/// untouched — the store will not publish a length it cannot produce the bytes for.
#[test]
fn a_finalisation_that_outruns_the_journal_is_refused() {
    let ride = recording();
    let mut before = holding(&[ride], 6);
    before.ride = Some((0, 900));
    let build = recording_card(&before, &ride, 0, 900);

    for claimed in [901u64, 4_000, 700_000] {
        let disk = build(5);
        let mut store = FlatStore::mount(&disk);
        let finalised = entry(1, 1, ObjectKind::Ride, EntryFlags::NONE, claimed, "Tuesday", &[(0, 1)]);
        assert_eq!(
            store.commit(&[Mutation::Put { meta: finalised.meta, source: PutSource::Amend }]).unwrap_err(),
            super::error::StoreError::Invalid,
            "a finalisation claiming {claimed} bytes was accepted",
        );
        assert_eq!(snapshot(&mut store), before.snapshot(), "a refused finalisation changed the card");
    }

    // And a ride with no checkpoint at all has nothing to publish.
    let disk = card(6, &holding(&[ride], 6), 0);
    let mut store = FlatStore::mount(&disk);
    let finalised = entry(1, 1, ObjectKind::Ride, EntryFlags::NONE, 900, "Tuesday", &[(0, 1)]);
    assert_eq!(
        store.commit(&[Mutation::Put { meta: finalised.meta, source: PutSource::Amend }]).unwrap_err(),
        super::error::StoreError::Invalid,
    );
}

/// The rider's side of §7.2: the whole payload recorded so far, and how much of it the store has
/// flushed into the ride's extents. A checkpoint hands over everything past that.
struct Rider {
    payload: Vec<u8>,
    flushed: usize,
}

impl Rider {
    fn new() -> Self {
        Rider { payload: Vec::new(), flushed: 0 }
    }

    fn grow(&mut self, bytes: usize) {
        let from = self.payload.len();
        self.payload.extend((from..from + bytes).map(|index| (index * 7 + 11) as u8));
    }

    fn tail(&self) -> &[u8] {
        &self.payload[self.flushed..]
    }

    /// One checkpoint. On success the store flushed every whole page out of the front of the tail, so
    /// the rider drops the same bytes from its own — the one thing the seam leaves to the caller.
    fn checkpoint(&mut self, store: &mut Card) -> bool {
        let checkpoint = RideCheckpoint {
            id: ObjectId(1),
            revision: Revision(1),
            tail: self.tail(),
            payload_crc: crc32(&self.payload),
        };
        if store.journal(checkpoint).is_err() {
            return false;
        }
        self.flushed += self.tail().len() / PROGRAM_PAGE * PROGRAM_PAGE;
        true
    }

    /// What a mount must recover after that checkpoint, and the tail bytes it must hand back.
    fn expect(&self, sequence: u64) -> (RideRecovery, Vec<u8>) {
        let recovery = RideRecovery {
            id: ObjectId(1),
            revision: Revision(1),
            checkpoint_sequence: sequence,
            flushed: self.flushed as u64,
            tail_len: self.tail().len() as u32,
            payload_crc: crc32(&self.payload),
            slot: (sequence % SLOTS as u64) as u16,
        };
        (recovery, self.tail().to_vec())
    }
}

/// The checkpoint, cut at every media operation: recovery yields the previous checkpoint or this one,
/// never a mixture, and never a ride that rolled back further than one interval — §7.4's loss cap.
fn checkpoint_matrix(name: &str, cuts: usize, growths: &[usize]) {
    let start = holding(&[recording()], 6);
    let mut admissible: Vec<Option<(RideRecovery, Vec<u8>)>> = vec![None];
    let mut rider = Rider::new();
    for (index, growth) in growths.iter().enumerate() {
        rider.grow(*growth);
        // The rider's own model of the flush, which is what makes this an expectation rather than an
        // echo of what the store did.
        rider.flushed += rider.tail().len() / PROGRAM_PAGE * PROGRAM_PAGE;
        admissible.push(Some(rider.expect(index as u64 + 1)));
    }
    let last = growths.len() - 1;

    let run = |disk: &SparseDisk, cut: Option<FaultPlan>| {
        let mut store = FlatStore::mount(disk);
        let mut rider = Rider::new();
        for growth in &growths[..last] {
            rider.grow(*growth);
            assert!(rider.checkpoint(&mut store), "{name}: the setup checkpoints must succeed");
        }
        let baseline = disk.ops();
        if let Some(plan) = cut {
            disk.plan(FaultPlan { op: baseline + plan.op, when: plan.when });
        }
        rider.grow(growths[last]);
        rider.checkpoint(&mut store);
        // The operations of the checkpoint under test alone: the setup ones are the card this scenario
        // starts from, not part of what it is cut inside.
        let widths = disk
            .write_widths()
            .into_iter()
            .filter(|(op, _)| *op > baseline)
            .map(|(op, blocks)| (op - baseline, blocks))
            .collect::<Vec<_>>();
        (disk.ops() - baseline, widths)
    };

    let (total, widths) = run(&card(1, &start, 0), None);
    let points = cut_points(total, &widths);
    assert_eq!(points.len(), cuts, "{name}: {} cut points, not the {cuts} pinned", points.len());
    for (op, when) in points {
        let disk = card(seed(u64::from(op) * 41 + 13, when), &start, 0);
        run(&disk, Some(FaultPlan { op, when }));
        disk.reboot();

        let mut store = FlatStore::mount(&disk);
        assert_eq!(store.mode(), Mode::ReadWrite, "{name}: cut at op {op} {when:?} did not mount");
        let recovered_state = Snapshot { ride: None, ..snapshot(&mut store) };
        assert_eq!(recovered_state, start.snapshot(), "{name}: a checkpoint changed the catalog");
        let recovered = store.recovered_ride();
        let expected = &admissible[last..];
        assert!(
            expected.iter().any(|state| state.as_ref().map(|(recovery, _)| *recovery) == recovered),
            "{name}: cut at op {op} {when:?} recovered {recovered:?}, neither admissible checkpoint",
        );
        if let Some(recovered) = recovered {
            let (_, tail) = expected
                .iter()
                .flatten()
                .find(|(recovery, _)| *recovery == recovered)
                .expect("the recovered state was just matched");
            let mut read = vec![0u8; recovered.tail_len as usize];
            store.recovered_tail(&mut read).unwrap();
            assert_eq!(&read, tail, "{name}: the recovered tail is not that checkpoint's bytes");
        }
    }
}

#[test]
fn a_checkpoint_recovers_the_previous_one_or_itself() {
    checkpoint_matrix("checkpoint", 219, &[200, 200]);
}

/// The page flush and the slot, in the order §7.2 fixes: a payload page is written only when every
/// byte in it is already in a slot on the card. A cut between the two leaves the previous slot
/// authoritative — its flushed length one page behind, its tail still holding those bytes — and
/// recovery simply rewrites the page.
#[test]
fn a_checkpoint_that_flushes_a_page_recovers_either_side_of_the_flush() {
    checkpoint_matrix("checkpoint with flush", 321, &[100, PROGRAM_PAGE + 200]);
}

/// The first checkpoint of a ride: the admissible states are "no slot at all" — a ride start commits
/// its entry before any checkpoint exists — and this one.
#[test]
fn the_first_checkpoint_recovers_nothing_or_itself() {
    checkpoint_matrix("first checkpoint", 219, &[300]);
}

/// §7.3's optional prefix check, done here because the spec says a host harness should: the flushed
/// prefix on the card plus the recovered tail hash to exactly the CRC the slot carries.
#[test]
fn the_recovered_payload_crc_covers_the_prefix_on_the_card() {
    let disk = card(11, &holding(&[recording()], 6), 0);
    let mut store = FlatStore::mount(&disk);
    let tail = payload(3 * PROGRAM_PAGE + 777);
    store
        .journal(RideCheckpoint { id: ObjectId(1), revision: Revision(1), tail: &tail, payload_crc: crc32(&tail) })
        .unwrap();
    disk.reboot();

    let store = FlatStore::mount(&disk);
    let recovered = store.recovered_ride().unwrap();
    assert_eq!(recovered.flushed, 3 * PROGRAM_PAGE as u64);
    assert_eq!(recovered.payload_len(), tail.len() as u64);

    // The prefix is read off the card by extent arithmetic — the reserve starts at extent 0.
    let mut whole = Vec::new();
    for block in 0..recovered.flushed / BLOCK as u64 {
        whole.extend_from_slice(&disk.block(EXTENT_AREA + block));
    }
    let mut recovered_tail = vec![0u8; recovered.tail_len as usize];
    store.recovered_tail(&mut recovered_tail).unwrap();
    whole.extend_from_slice(&recovered_tail);
    assert_eq!(whole, tail, "the flushed prefix on the card is not the ride's payload");
    assert_eq!(crc32(&whole), recovered.payload_crc);
}

/// §7.3: "Recording resumes at checkpoint sequence `recovered + 1`", never at `1` — restarting the
/// count would leave this ride's stale slots carrying greater sequences and the next recovery would
/// roll the ride back.
#[test]
fn recording_resumes_at_the_recovered_sequence_plus_one() {
    let disk = card(13, &holding(&[recording()], 6), 0);
    let mut store = FlatStore::mount(&disk);
    for step in 1..=20u64 {
        let tail = payload(100 + step as usize);
        store
            .journal(RideCheckpoint { id: ObjectId(1), revision: Revision(1), tail: &tail, payload_crc: crc32(&tail) })
            .unwrap();
    }
    disk.reboot();

    let mut store = FlatStore::mount(&disk);
    let recovered = store.recovered_ride().unwrap();
    assert_eq!(recovered.checkpoint_sequence, 20, "the greatest sequence did not win the wrapped ring");
    assert_eq!(recovered.slot, 4, "checkpoint 20 belongs in slot 20 mod 16");

    let tail = payload(500);
    store
        .journal(RideCheckpoint { id: ObjectId(1), revision: Revision(1), tail: &tail, payload_crc: crc32(&tail) })
        .unwrap();
    disk.reboot();
    assert_eq!(FlatStore::mount(&disk).recovered_ride().unwrap().checkpoint_sequence, 21);
}

/// §7.2's ride end zeroes the sixteen slot headers, and §5.6 never reads them afterwards — so a stale
/// slot cannot resurrect a finished ride, and a new ride over the same extents cannot inherit one.
#[test]
fn ending_a_ride_leaves_no_slot_behind() {
    let disk = card(17, &holding(&[recording()], 6), 0);
    let mut store = FlatStore::mount(&disk);
    let tail = payload(1_000);
    store
        .journal(RideCheckpoint { id: ObjectId(1), revision: Revision(1), tail: &tail, payload_crc: crc32(&tail) })
        .unwrap();
    let finalised = entry(1, 1, ObjectKind::Ride, EntryFlags::NONE, 1_000, "Tuesday", &[(0, 1)]);
    store.commit(&[Mutation::Put { meta: finalised.meta, source: PutSource::Amend }]).unwrap();
    assert_eq!(store.free_extents(), EXTENTS - 1, "the rest of the reserve was not freed");
    for slot in 0..SLOTS {
        assert_eq!(disk.block(slot_block(slot)), [0u8; BLOCK], "slot {slot} was not zeroed");
    }
    disk.reboot();
    assert_eq!(FlatStore::mount(&disk).recovered_ride(), None);
}

/// An amend changes an entry's metadata under its own key, so a reader that opens it afterwards gets the
/// metadata that is there now — a length of zero from before a ride was finalised would have it read an
/// empty object off a card that holds its bytes. §2.1's promise is about the *revision* a handle
/// resolved, and an amend does not make a new one.
#[test]
fn a_reader_that_opens_after_an_amend_sees_the_amended_length() {
    let ride = recording();
    let mut before = holding(&[ride], 6);
    before.ride = Some((0, 900));
    let disk = recording_card(&before, &ride, 0, 900)(7);
    let mut store = FlatStore::mount(&disk);

    // A reader while the ride is still recording: length zero, nothing to read.
    let during = store.open(ObjectId(1), Some(Revision(1))).unwrap();
    assert_eq!(store.read(&during, 0, &mut [0u8; 16]).unwrap(), 0);

    let finalised = entry(1, 1, ObjectKind::Ride, EntryFlags::NONE, 900, "Tuesday", &[(0, 1)]);
    store.commit(&[Mutation::Put { meta: finalised.meta, source: PutSource::Amend }]).unwrap();

    let after = store.open(ObjectId(1), Some(Revision(1))).unwrap();
    let mut bytes = vec![0u8; 900];
    assert_eq!(store.read(&after, 0, &mut bytes).unwrap(), 900, "the second reader inherited a stale length");
    assert_eq!(bytes, payload(900));
    store.close(after);
    store.close(during);
}

/// §6.2's hold rule covers an amend as well as a removal: finalising a ride frees 31 of its 32 reserve
/// extents, and a reader holding the entry keeps them out of the allocator until it closes.
#[test]
fn an_amend_that_trims_a_held_entry_defers_the_extents_it_frees() {
    let ride = recording();
    let mut before = holding(&[ride], 6);
    before.ride = Some((0, 900));
    let disk = recording_card(&before, &ride, 0, 900)(8);
    let mut store = FlatStore::mount(&disk);
    assert_eq!(store.free_extents(), EXTENTS - 32);

    let handle = store.open(ObjectId(1), Some(Revision(1))).unwrap();
    let finalised = entry(1, 1, ObjectKind::Ride, EntryFlags::NONE, 900, "Tuesday", &[(0, 1)]);
    store.commit(&[Mutation::Put { meta: finalised.meta, source: PutSource::Amend }]).unwrap();
    assert_eq!(store.free_extents(), EXTENTS - 32, "the trimmed reserve was freed under a live reader");

    store.close(handle);
    assert_eq!(store.free_extents(), EXTENTS - 1, "the trimmed tail did not come back at the last close");
    // And what the entry still names is untouched: the object reads back whole.
    let handle = store.open(ObjectId(1), None).unwrap();
    let mut bytes = vec![0u8; 900];
    assert_eq!(store.read(&handle, 0, &mut bytes).unwrap(), 900);
    assert_eq!(bytes, payload(900));
    store.close(handle);
}

/// The other half of §6.2's hold rule, and the one a second reader exposes: `release` defers a trimmed
/// tail *because* a hold names it, so the hold has to keep naming it. A reader that joins the row after
/// the trim takes the amended length — §2.1 does not promise it a stale one — and leaves the extents
/// alone, because a trim keeps a prefix and the wider ranges serve every byte of the shorter length.
/// Narrowing them there would leave the 31 trimmed extents named by nobody: not by the catalog, which
/// gave them up, and not by the hold that was deferring them.
#[test]
fn a_reader_joining_a_trimmed_hold_still_returns_the_whole_reserve() {
    let ride = recording();
    let mut before = holding(&[ride], 6);
    before.ride = Some((0, 900));
    let disk = recording_card(&before, &ride, 0, 900)(43);
    let mut store = FlatStore::mount(&disk);
    assert_eq!(store.free_extents(), EXTENTS - 32);

    let first = store.open(ObjectId(1), Some(Revision(1))).unwrap();
    let finalised = entry(1, 1, ObjectKind::Ride, EntryFlags::NONE, 900, "Tuesday", &[(0, 1)]);
    store.commit(&[Mutation::Put { meta: finalised.meta, source: PutSource::Amend }]).unwrap();
    assert_eq!(store.free_extents(), EXTENTS - 32, "the trimmed reserve was freed under a live reader");

    // The joining reader: the amended length, and the ride's bytes.
    let second = store.open(ObjectId(1), Some(Revision(1))).unwrap();
    let mut bytes = vec![0u8; 900];
    assert_eq!(store.read(&second, 0, &mut bytes).unwrap(), 900);
    assert_eq!(bytes, payload(900));

    store.close(second);
    assert_eq!(store.free_extents(), EXTENTS - 32, "the row went away while the first reader held it");
    store.close(first);
    assert_eq!(store.free_extents(), EXTENTS - 1, "the trimmed tail of a joined hold never came back");
    disk.reboot();
    assert_eq!(FlatStore::mount(&disk).free_extents(), EXTENTS - 1, "and the next mount says the same");
}

/// §7.1's cross-check: a slot left by an earlier ride over reused extents is not this ride's, and a
/// slot whose ranges differ from the recording entry's is rejected.
#[test]
fn a_slot_from_another_ride_is_not_this_ones() {
    let disk = card(19, &holding(&[recording()], 6), 0);
    let mut store = FlatStore::mount(&disk);
    let tail = payload(1_000);
    store
        .journal(RideCheckpoint { id: ObjectId(1), revision: Revision(1), tail: &tail, payload_crc: crc32(&tail) })
        .unwrap();
    disk.reboot();

    // The same slot bytes, under a catalog whose recording entry is a different ride.
    let other = entry(2, 1, ObjectKind::Ride, EntryFlags::RECORDING, 0, "", &[(0, 32)]);
    install_catalog(&disk, &holding(&[other], 7), 1);
    let store = FlatStore::mount(&disk);
    assert_eq!(store.serving_copy(), 1);
    assert_eq!(store.recovered_ride(), None, "a slot naming another ride was accepted");
}

// -------------------------------------------------------------------------------------------
// §5.6 — the read-only mounts
// -------------------------------------------------------------------------------------------

#[test]
fn an_unformatted_card_refuses_everything_at_the_seam() {
    let disk = SparseDisk::blank(TOTAL_BLOCKS, 1);
    let mut store = FlatStore::mount(&disk);
    assert_eq!(store.mode(), Mode::Unformatted);
    assert_eq!(store.allocate(512), Err(super::error::StoreError::ReadOnly));
    assert_eq!(
        store.commit(&[Mutation::Remove { id: ObjectId(1), revision: Revision(1) }]).unwrap_err(),
        super::error::StoreError::ReadOnly
    );
    assert!(matches!(store.open(ObjectId(1), None), Err(super::error::StoreError::ReadOnly)));
    assert_eq!(store.entries().count(), 0);
    assert!(model::snapshot(&mut store).is_none());
}

/// §5.6 step 2 and 3: no well-formed gate, two well-formed gates at equal sequences, and no candidate
/// body that validates are all media damage rather than a state the store can produce, and each
/// mounts read-only with the evidence preserved.
#[test]
fn a_card_with_no_usable_catalog_mounts_read_only() {
    let route = entry(1, 1, ObjectKind::Route, EntryFlags::NONE, 3_000, "Grimsel Loop", &[(0, 1)]);
    let model = holding(&[route], 4);

    let disk = card(2, &model, 0);
    disk.install(catalog_gate(0), &[0u8; BLOCK]);
    assert_eq!(FlatStore::mount(&disk).mode(), Mode::CatalogUnreadable);

    let disk = card(3, &model, 0);
    install_catalog(&disk, &model, 1);
    assert_eq!(FlatStore::mount(&disk).mode(), Mode::CatalogUnreadable, "two gates at one sequence is corruption");

    let disk = card(4, &model, 0);
    let mut torn = disk.block(CATALOG[0] + 1);
    torn[0] ^= 0xFF;
    disk.install(CATALOG[0] + 1, &torn);
    let store = FlatStore::mount(&disk);
    assert_eq!(store.mode(), Mode::CatalogUnreadable);
    assert!(matches!(store.open(ObjectId(1), None), Err(super::error::StoreError::ReadOnly)));
    // Evidence preserved: nothing was repaired, and the torn block is still on the card.
    assert_eq!(disk.block(CATALOG[0] + 1), torn);
}

/// §4: a card smaller than the superblock recorded is refused as damaged or swapped, never silently
/// truncated.
#[test]
fn a_shrunken_card_is_refused() {
    let disk = SparseDisk::blank(TOTAL_BLOCKS / 2, 1);
    let superblock = Superblock::for_card(STORE, TOTAL_BLOCKS).expect("an expressible card").encode();
    disk.install(SUPERBLOCK[0], &superblock);
    assert_eq!(FlatStore::mount(&disk).mode(), Mode::CardTooSmall);
}

/// §3: a `Revision` that reached `u64::MAX` mounts the store read-only, and only that case still
/// serves reads.
#[test]
fn an_exhausted_revision_space_still_serves_reads() {
    let last = entry(1, u64::MAX, ObjectKind::Route, EntryFlags::NONE, 3_000, "Grimsel Loop", &[(0, 1)]);
    let disk = card(6, &holding(&[last], 4), 0);
    let mut store = FlatStore::mount(&disk);
    assert_eq!(store.mode(), Mode::RevisionSpaceExhausted);
    assert!(store.open(ObjectId(1), None).is_ok());
    assert_eq!(store.allocate(512), Err(super::error::StoreError::ReadOnly));
}

/// The other counter, from the same rule: a card whose newest gate carries commit sequence `u64::MAX`
/// has no sequence for §5.5 step 2 to continue to. It mounts read-only rather than wrapping — and than
/// panicking, which is what an unchecked `high_water + 1` would do in a debug build.
#[test]
fn an_exhausted_sequence_space_still_serves_reads() {
    let route = entry(1, 1, ObjectKind::Route, EntryFlags::NONE, 3_000, "Grimsel Loop", &[(0, 1)]);
    let mut model = holding(&[route], u64::MAX);
    model.high_water = u64::MAX;
    let disk = card(11, &model, 0);
    let mut store = FlatStore::mount(&disk);
    assert_eq!(store.mode(), Mode::SequenceSpaceExhausted);

    let handle = store.open(ObjectId(1), None).unwrap();
    let mut bytes = vec![0u8; 3_000];
    assert_eq!(store.read(&handle, 0, &mut bytes).unwrap(), 3_000);
    assert_eq!(bytes, payload(3_000));
    store.close(handle);
    assert_eq!(store.allocate(512), Err(super::error::StoreError::ReadOnly));
    assert_eq!(
        store.commit(&[Mutation::Remove { id: ObjectId(1), revision: Revision(1) }]).unwrap_err(),
        super::error::StoreError::ReadOnly,
    );
}

/// The sequence space, from the other end of the mount that detects it: a card whose high-water mark is
/// one short of `u64::MAX` mounts read-write and has exactly one commit left in it. That commit lands,
/// and the store is read-only from then on — the next one is refused rather than continuing from a mark
/// there is nothing past, which unchecked would be an `attempt to add with overflow` in a debug build.
#[test]
fn the_commit_that_reaches_the_last_sequence_leaves_the_store_read_only() {
    let route = entry(1, 1, ObjectKind::Route, EntryFlags::NONE, 3_000, "Grimsel Loop", &[(0, 1)]);
    let trip = entry(2, 1, ObjectKind::Trip, EntryFlags::NONE, 600, "Alps", &[(1, 1)]);
    let model = holding(&[route, trip], u64::MAX - 1);
    let disk = card(44, &model, 0);
    let mut store = FlatStore::mount(&disk);
    assert_eq!(store.mode(), Mode::ReadWrite);

    let sequence = store.commit(&[Mutation::Remove { id: ObjectId(2), revision: Revision(1) }]).unwrap();
    assert_eq!(sequence, u64::MAX);
    assert_eq!(store.mode(), Mode::SequenceSpaceExhausted, "a store with no sequence left still claimed to write");
    assert_eq!(
        store.commit(&[Mutation::Remove { id: ObjectId(1), revision: Revision(1) }]).unwrap_err(),
        super::error::StoreError::ReadOnly,
    );
    assert_eq!(store.allocate(512), Err(super::error::StoreError::ReadOnly));

    // Reads are still served, here and at the next mount — which reaches the same verdict from the gate.
    let handle = store.open(ObjectId(1), None).unwrap();
    let mut bytes = vec![0u8; 3_000];
    assert_eq!(store.read(&handle, 0, &mut bytes).unwrap(), 3_000);
    assert_eq!(bytes, payload(3_000));
    store.close(handle);
    disk.reboot();
    assert_eq!(FlatStore::mount(&disk).mode(), Mode::SequenceSpaceExhausted);
}

/// The identity space stops one short of wrapping too: §5.2's cursor has to end up strictly greater than
/// every id in the array, and `u64::MAX` leaves nowhere for it to go.
#[test]
fn an_object_id_at_the_end_of_the_space_is_refused() {
    let disk = card(12, &empty(), 0);
    let mut store = FlatStore::mount(&disk);
    let mut allocation = store.allocate(600).unwrap();
    store.write(&mut allocation, &payload(600)).unwrap();
    let last = entry(u64::MAX, 1, ObjectKind::Trip, EntryFlags::NONE, 600, "", &[(0, 1)]);
    assert_eq!(
        store.commit(&[Mutation::Put { meta: last.meta, source: PutSource::Fresh(allocation) }]).unwrap_err(),
        super::error::StoreError::ReadOnly,
    );
    store.cancel(allocation);
    assert_eq!(snapshot(&mut store), empty().snapshot());
}

/// A store serving no catalog answers for no extents. `free_extents` is public, and a number left over
/// from the copy a mount refused would be a promise the store cannot keep.
#[test]
fn a_read_only_mount_reports_no_free_space() {
    let route = entry(1, 1, ObjectKind::Route, EntryFlags::NONE, 3_000, "Grimsel Loop", &[(0, 1)]);
    let model = holding(&[route], 4);

    let unformatted = SparseDisk::blank(TOTAL_BLOCKS, 1);
    assert_eq!(FlatStore::mount(&unformatted).free_extents(), 0);

    // A body that fails its gate: the load that rejected it had already claimed its ranges.
    let disk = card(13, &model, 0);
    let mut torn = disk.block(CATALOG[0] + 1);
    torn[0] ^= 0xFF;
    disk.install(CATALOG[0] + 1, &torn);
    let store = FlatStore::mount(&disk);
    assert_eq!(store.mode(), Mode::CatalogUnreadable);
    assert_eq!(store.free_extents(), 0, "a refused catalog left its free map resident");

    // Two well-formed gates at one sequence: no candidate is even tried.
    let disk = card(14, &model, 0);
    install_catalog(&disk, &model, 1);
    let store = FlatStore::mount(&disk);
    assert_eq!(store.mode(), Mode::CatalogUnreadable);
    assert_eq!(store.free_extents(), 0);
}

/// §4: two copies of identical bytes exist so that one bad block does not make the card unreadable.
#[test]
fn a_torn_superblock_falls_back_to_the_other_copy() {
    let route = entry(1, 1, ObjectKind::Route, EntryFlags::NONE, 3_000, "Grimsel Loop", &[(0, 1)]);
    let model = holding(&[route], 4);
    let disk = card(8, &model, 0);
    disk.install(SUPERBLOCK[0], &[0xFF; BLOCK]);
    let mut store = FlatStore::mount(&disk);
    assert_eq!(store.mode(), Mode::ReadWrite);
    assert_eq!(snapshot(&mut store), model.snapshot());
}

// -------------------------------------------------------------------------------------------
// §6.2 — reservations, reader holds, and the free map
// -------------------------------------------------------------------------------------------

/// §6.2: an allocation is RAM state until the commit that names its extents. A cut before that commit
/// leaves those bytes anonymous, and the next mount rebuilds the bitmap from the catalog and cannot
/// see them.
#[test]
fn an_uncommitted_allocation_is_free_again_at_the_next_mount() {
    let disk = card(21, &empty(), 0);
    let mut store = FlatStore::mount(&disk);
    let mut allocation = store.allocate(4 << 20).unwrap();
    store.write(&mut allocation, &payload(BLOCK)).unwrap();
    assert_eq!(store.free_extents(), EXTENTS - 4);
    disk.reboot();
    assert_eq!(FlatStore::mount(&disk).free_extents(), EXTENTS);
}

#[test]
fn cancelling_an_allocation_returns_its_extents_immediately() {
    let disk = card(22, &empty(), 0);
    let mut store = FlatStore::mount(&disk);
    let allocation = store.allocate(4 << 20).unwrap();
    assert_eq!(store.free_extents(), EXTENTS - 4);
    store.cancel(allocation);
    assert_eq!(store.free_extents(), EXTENTS);
}

/// §6.2's one qualification, and §2.1's promise: while an open handle names an entry, the store keeps
/// that entry's extents out of the allocator even after a commit has removed it, and returns them when
/// the last handle closes.
#[test]
fn a_reader_holds_its_extents_until_it_closes() {
    let route = entry(1, 1, ObjectKind::Route, EntryFlags::NONE, 3_000, "Grimsel Loop", &[(0, 1)]);
    let disk = card(23, &holding(&[route], 4), 0);
    disk.install(EXTENT_AREA, &payload(3_000));
    let mut store = FlatStore::mount(&disk);

    let handle = store.open(ObjectId(1), None).unwrap();
    let second = store.open(ObjectId(1), None).unwrap();
    store.commit(&[Mutation::Remove { id: ObjectId(1), revision: Revision(1) }]).unwrap();
    assert_eq!(store.entries().count(), 0, "the commit did not remove the entry");
    assert_eq!(store.free_extents(), EXTENTS - 1, "a held entry's extents went back to the allocator");

    // The handle keeps reading the revision it resolved, across the commit that removed it.
    let mut buf = [0u8; 3_000];
    assert_eq!(store.read(&handle, 0, &mut buf).unwrap(), 3_000);
    assert_eq!(buf[..], payload(3_000)[..]);

    store.close(handle);
    assert_eq!(store.free_extents(), EXTENTS - 1, "the extents were freed while a second reader held them");
    store.close(second);
    assert_eq!(store.free_extents(), EXTENTS, "the last close did not return the extents");

    // The hold was RAM-only: after a reboot there is no reader left to be surprised.
    disk.reboot();
    assert_eq!(FlatStore::mount(&disk).free_extents(), EXTENTS);
}

#[test]
fn a_reader_of_a_retained_revision_reaches_it_by_naming_it() {
    let old = entry(1, 1, ObjectKind::WeatherBundle, EntryFlags::RETAINED, 3_000, "", &[(0, 1)]);
    let new = entry(1, 2, ObjectKind::WeatherBundle, EntryFlags::NONE, 5_000, "", &[(1, 1)]);
    let disk = card(24, &holding(&[old, new], 9), 0);
    let store = FlatStore::mount(&disk);
    assert_eq!(store.open(ObjectId(1), None).unwrap().revision(), Revision(2), "None did not take the head");
    assert_eq!(store.open(ObjectId(1), Some(Revision(1))).unwrap().revision(), Revision(1));
    assert!(store.open(ObjectId(1), Some(Revision(3))).is_err());
}

#[test]
fn a_reserve_owns_extents_and_refuses_to_be_read() {
    let reserve = entry(1, 1, ObjectKind::RollbackReserve, EntryFlags::RESERVED, 0, "", &[(0, 8)]);
    let disk = card(25, &holding(&[reserve], 4), 0);
    let store = FlatStore::mount(&disk);
    assert_eq!(store.free_extents(), EXTENTS - 8);
    assert_eq!(store.open(ObjectId(1), None).unwrap_err(), super::error::StoreError::Invalid);
}

/// §6.2: fragmentation's worst case is a refused allocation, never a partial object and never a
/// rewritten card.
#[test]
fn allocation_refusals_are_the_two_the_format_admits() {
    let disk = card(26, &empty(), 0);
    let mut store = FlatStore::mount(&disk);
    assert_eq!(
        store.allocate((EXTENTS as u64 + 1) << 20),
        Err(super::error::StoreError::NoSpace { required: (EXTENTS as u64 + 1) << 20 })
    );

    // Nine one-extent holes cannot be expressed in eight ranges.
    let mut model = empty();
    for id in 1..=9u64 {
        model.entries.push(entry(id, 1, ObjectKind::Route, EntryFlags::NONE, 3_000, "", &[((id as u16 - 1) * 2, 1)]));
    }
    model.next_object = 10;
    let disk = card(27, &model, 0);
    let mut store = FlatStore::mount(&disk);
    assert_eq!(store.allocate(9 << 20), Err(super::error::StoreError::TooFragmented));
    assert_eq!(store.free_extents(), EXTENTS - 9, "a refusal changed the free map");
}

// -------------------------------------------------------------------------------------------
// The seam's own rules
// -------------------------------------------------------------------------------------------

/// A payload written through the seam reads back byte for byte, across a range boundary and at every
/// alignment — `read` is arithmetic on the entry's ranges and nothing else.
#[test]
fn a_payload_round_trips_across_a_range_boundary() {
    let mut model = empty();
    // Extent 1 is taken, so a three-extent object gets two ranges.
    model.entries.push(entry(9, 1, ObjectKind::Route, EntryFlags::NONE, 3_000, "", &[(1, 1)]));
    model.next_object = 10;
    let disk = card(28, &model, 0);
    let mut store = FlatStore::mount(&disk);

    // This card's extents are the 1 MiB minimum §8 gives it, and the payload spans two of them.
    let extent = Geometry::DEFAULT.extent_size();
    let bytes = payload(2 * extent as usize + 4_242);
    let published =
        entry(10, 1, ObjectKind::MapShard, EntryFlags::NONE, bytes.len() as u64, "shard", &[(0, 1), (2, 2)]);
    let mut allocation = store.allocate(bytes.len() as u64).unwrap();
    // Written in awkward pieces, so the staging block matters.
    for chunk in bytes.chunks(777) {
        store.write(&mut allocation, chunk).unwrap();
    }
    store.commit(&[Mutation::Put { meta: published.meta, source: PutSource::Fresh(allocation) }]).unwrap();

    let handle = store.open(ObjectId(10), None).unwrap();
    let mut whole = vec![0u8; bytes.len()];
    assert_eq!(store.read(&handle, 0, &mut whole).unwrap(), bytes.len());
    assert_eq!(whole, bytes);
    for offset in [0u64, 1, 511, 512, extent - 3, extent, 2 * extent] {
        let mut buf = [0u8; 1_000];
        let read = store.read(&handle, offset, &mut buf).unwrap();
        assert_eq!(&buf[..read], &bytes[offset as usize..offset as usize + read], "offset {offset}");
    }
    // Short only at end of payload.
    let mut buf = [0u8; 100];
    assert_eq!(store.read(&handle, bytes.len() as u64 - 10, &mut buf).unwrap(), 10);
    assert_eq!(store.read(&handle, bytes.len() as u64, &mut buf).unwrap(), 0);
}

/// The identity rule at the seam, and the reason §5.2's cursor never rewinds: an `ObjectId` the cursor
/// has passed named an object once and may never name another. Removing an object does not return its
/// id — and a `Fresh` put re-using one would find the array empty, take `Revision(1)` from the
/// compare-and-swap as if it were a create, and re-create a key a reader's hold still names over
/// different extents. The trace this refuses is #1406's: the second reader joins that row and reads a
/// removed object's bytes under the live one's identity.
#[test]
fn a_fresh_put_may_not_re_use_an_object_id_the_cursor_has_passed() {
    let route = entry(1, 1, ObjectKind::Route, EntryFlags::NONE, 3_000, "Grimsel Loop", &[(0, 1)]);
    let model = holding(&[route], 4);
    let disk = card(45, &model, 0);
    let mut store = FlatStore::mount(&disk);

    // A reader holds revision 1, and a commit removes it: the hold defers extent 0, so first-fit hands
    // the next object extent 1 and the impostor would name bytes this reader never resolved.
    let handle = store.open(ObjectId(1), None).unwrap();
    store.commit(&[Mutation::Remove { id: ObjectId(1), revision: Revision(1) }]).unwrap();
    let mut allocation = store.allocate(600).unwrap();
    store.write(&mut allocation, &payload(600)).unwrap();
    let impostor = entry(1, 1, ObjectKind::Route, EntryFlags::NONE, 600, "Impostor", &[(1, 1)]);
    assert_eq!(
        store.commit(&[Mutation::Put { meta: impostor.meta, source: PutSource::Fresh(allocation) }]).unwrap_err(),
        super::error::StoreError::Invalid,
        "a Fresh put re-used an ObjectId the cursor had passed",
    );

    // So there is nothing for a second reader to join, and the first one still reads its own object.
    assert_eq!(store.open(ObjectId(1), Some(Revision(1))).unwrap_err(), super::error::StoreError::NotFound);
    assert_eq!(store.open(ObjectId(1), None).unwrap_err(), super::error::StoreError::NotFound);
    let mut bytes = vec![0u8; 3_000];
    assert_eq!(store.read(&handle, 0, &mut bytes).unwrap(), 3_000);
    assert_eq!(bytes, payload(3_000), "the held revision stopped reading its own bytes");

    // The cursor's own id is what a create names, and the refusal changed nothing that stops it.
    assert_eq!(store.next_object_id(), ObjectId(2));
    let published = entry(2, 1, ObjectKind::Route, EntryFlags::NONE, 600, "Alps", &[(1, 1)]);
    store.commit(&[Mutation::Put { meta: published.meta, source: PutSource::Fresh(allocation) }]).unwrap();
    let second = store.open(ObjectId(2), None).unwrap();
    let mut six = vec![0u8; 600];
    assert_eq!(store.read(&second, 0, &mut six).unwrap(), 600);
    assert_eq!(six, payload(600));
    store.close(second);
    store.close(handle);
}

/// The revision rule at the seam: `Revision` is the compare-and-swap token every mutation carries, so
/// a fresh publication has to be exactly one past the head — and a `Revision` that already exists is
/// never overwritten, because an object never changes.
#[test]
fn a_put_that_does_not_continue_the_revision_chain_is_refused() {
    let route = entry(1, 1, ObjectKind::Route, EntryFlags::NONE, 3_000, "Grimsel Loop", &[(0, 1)]);
    let model = holding(&[route], 4);
    let disk = card(29, &model, 0);
    let mut store = FlatStore::mount(&disk);

    for revision in [1u64, 4] {
        let attempt = entry(1, revision, ObjectKind::Route, EntryFlags::NONE, 3_000, "Grimsel Loop", &[(1, 1)]);
        let allocation = store.allocate(3_000).unwrap();
        let batch = [Mutation::Put { meta: attempt.meta, source: PutSource::Fresh(allocation) }];
        let error = store.commit(&batch).unwrap_err();
        assert_eq!(
            error,
            super::error::StoreError::RevisionConflict { current: Revision(1) },
            "revision {revision} was not refused as a compare-and-swap failure",
        );
        store.cancel(allocation);
    }
    assert_eq!(snapshot(&mut store), model.snapshot(), "a refused commit changed the card");
}

/// A commit that returns `Err` changed nothing, and a batch the structural rules refuse never reaches
/// the card.
#[test]
fn a_refused_batch_leaves_the_catalog_untouched() {
    let route = entry(1, 1, ObjectKind::Route, EntryFlags::NONE, 3_000, "Grimsel Loop", &[(0, 1)]);
    let model = holding(&[route], 4);
    let disk = card(30, &model, 0);
    let mut store = FlatStore::mount(&disk);

    // A kind that disagrees with the object's other revision.
    let wrong_kind = entry(1, 2, ObjectKind::Trip, EntryFlags::NONE, 3_000, "", &[(1, 1)]);
    let allocation = store.allocate(3_000).unwrap();
    assert_eq!(
        store.commit(&[Mutation::Put { meta: wrong_kind.meta, source: PutSource::Fresh(allocation) }]).unwrap_err(),
        super::error::StoreError::Invalid,
    );
    store.cancel(allocation);

    // Removing something that is not there.
    assert_eq!(
        store.commit(&[Mutation::Remove { id: ObjectId(7), revision: Revision(1) }]).unwrap_err(),
        super::error::StoreError::NotFound,
    );
    // Two mutations naming one key.
    assert_eq!(
        store
            .commit(&[
                Mutation::Remove { id: ObjectId(1), revision: Revision(1) },
                Mutation::Remove { id: ObjectId(1), revision: Revision(1) },
            ])
            .unwrap_err(),
        super::error::StoreError::Invalid,
    );
    assert_eq!(store.commit(&[]).unwrap_err(), super::error::StoreError::Invalid);

    // One reservation published as two entries would name the same extents twice, which only a mount
    // would catch — by which point the card would be unreadable.
    let first = entry(2, 1, ObjectKind::Trip, EntryFlags::NONE, 600, "one", &[(1, 1)]);
    let second = entry(3, 1, ObjectKind::Trip, EntryFlags::NONE, 600, "two", &[(1, 1)]);
    let mut allocation = store.allocate(600).unwrap();
    store.write(&mut allocation, &payload(600)).unwrap();
    assert_eq!(
        store
            .commit(&[
                Mutation::Put { meta: first.meta, source: PutSource::Fresh(allocation) },
                Mutation::Put { meta: second.meta, source: PutSource::Fresh(allocation) },
            ])
            .unwrap_err(),
        super::error::StoreError::Invalid,
    );
    store.cancel(allocation);

    assert_eq!(snapshot(&mut store), model.snapshot());
    assert_eq!(disk.block(catalog_gate(1)), [0u8; BLOCK], "a refused commit touched the other copy");
}

/// The catalog's 1,916 entries are a refusal, not a corruption. The card here is big enough to give
/// each of them an extent of its own, because the mount that builds the free bitmap rejects an overlap
/// and a fake catalog would prove nothing.
#[test]
fn a_full_catalog_refuses_one_more_entry() {
    const BIG: u32 = super::layout::ENTRY_CAPACITY as u32 + 4;
    let mut model = Model::empty(STORE, BIG);
    for id in 1..=super::layout::ENTRY_CAPACITY as u64 {
        model.entries.push(entry(id, 1, ObjectKind::Trip, EntryFlags::NONE, 600, "", &[(id as u16 - 1, 1)]));
    }
    model.next_object = super::layout::ENTRY_CAPACITY as u64 + 1;
    model.sequence = 4;
    model.high_water = 4;

    let blocks = EXTENT_AREA + Geometry::DEFAULT.extent_blocks() * BIG as u64;
    let disk = SparseDisk::blank(blocks, 31);
    let superblock = Superblock::for_card(STORE, blocks).expect("an expressible card").encode();
    disk.install(SUPERBLOCK[0], &superblock);
    install_catalog(&disk, &model, 0);

    let mut store = FlatStore::mount(&disk);
    assert_eq!(store.mode(), Mode::ReadWrite);
    assert_eq!(store.entry_count() as usize, super::layout::ENTRY_CAPACITY);
    assert_eq!(snapshot(&mut store), model.snapshot());

    let one_more = entry(9_999, 1, ObjectKind::Trip, EntryFlags::NONE, 600, "", &[(1_916, 1)]);
    let mut allocation = store.allocate(600).unwrap();
    store.write(&mut allocation, &payload(600)).unwrap();
    let batch = [Mutation::Put { meta: one_more.meta, source: PutSource::Fresh(allocation) }];
    assert_eq!(store.commit(&batch).unwrap_err(), super::error::StoreError::CatalogFull);
    store.cancel(allocation);
    assert_eq!(snapshot(&mut store), model.snapshot(), "a refused commit changed the card");
}

/// The batched body write at the array's widest, which is the one place it could reach something that is
/// not body: §5.1 gives the entry array 480 blocks and puts the copy's **gate** in the block after them.
///
/// What this pins is the *boundary* — a window whose base drifted, or a body length miscounted at
/// capacity, would program that gate as part of the body it is about to certify and then write a real
/// gate over it. What it does **not** pin is the no-padding rule, and the distinction is worth stating
/// because it is easy to assume otherwise: 480 blocks is exactly 60 windows, so at capacity the last
/// window is already full and a writer that padded every window to eight blocks would pass this test
/// unchanged. Padding is caught instead by [`cost`](super::cost), whose commit at 300 entries pins a
/// 77-block body written in ten commands — nine full windows and a five-block one — so a writer that
/// rounded that last window up would report 83 write blocks instead of 80 and fail.
#[test]
fn a_commit_at_capacity_never_programs_the_gate_as_body() {
    const AT: usize = super::layout::ENTRY_CAPACITY - 1;
    const BIG: u32 = super::layout::ENTRY_CAPACITY as u32 + 4;
    let mut model = Model::empty(STORE, BIG);
    for id in 1..=AT as u64 {
        model.entries.push(entry(id, 1, ObjectKind::Trip, EntryFlags::NONE, 600, "", &[(id as u16 - 1, 1)]));
    }
    model.next_object = AT as u64 + 1;
    model.sequence = 4;
    model.high_water = 4;

    let blocks = EXTENT_AREA + Geometry::DEFAULT.extent_blocks() * BIG as u64;
    let disk = SparseDisk::blank(blocks, 61);
    let superblock = Superblock::for_card(STORE, blocks).expect("an expressible card").encode();
    disk.install(SUPERBLOCK[0], &superblock);
    disk.install(SUPERBLOCK[1], &superblock);
    install_catalog(&disk, &model, 0);

    let mut store = FlatStore::mount(&disk);
    let last = entry(AT as u64 + 1, 1, ObjectKind::Trip, EntryFlags::NONE, 600, "last", &[(AT as u16, 1)]);
    let mut allocation = store.allocate(600).unwrap();
    store.write(&mut allocation, &payload(600)).unwrap();
    store.commit(&[Mutation::Put { meta: last.meta, source: PutSource::Fresh(allocation) }]).unwrap();
    assert_eq!(store.entry_count() as usize, super::layout::ENTRY_CAPACITY);
    assert_eq!(store.serving_copy(), 1, "the commit did not target the copy it was not serving");

    // The body now fills blocks 0..480 of copy B, and 480 is its gate.
    assert_eq!(super::layout::body_len(super::layout::ENTRY_CAPACITY as u16).div_ceil(BLOCK) as u64, 480);
    let gate = Gate::decode(&disk.block(catalog_gate(1)), 1, &STORE).expect("the gate survived the body write");
    assert_eq!(gate.entry_count as usize, super::layout::ENTRY_CAPACITY);

    model.apply(&[Change::Put(last)]);
    assert_eq!(snapshot(&mut store), model.snapshot());
    disk.reboot();
    assert_eq!(snapshot(&mut FlatStore::mount(&disk)), model.snapshot(), "the card the commit left does not remount");
}

// -------------------------------------------------------------------------------------------
// Transient media failures — the error paths, and what a caller retries against
// -------------------------------------------------------------------------------------------

/// §7.2's flushed length is durable state, and a checkpoint that fails partway has not moved it. The
/// caller still holds every byte of its tail — the seam leaves dropping the flushed prefix to the
/// caller, and a failed checkpoint reported no prefix — so its retry hands over the same tail, and the
/// store has to put the same bytes at the same payload offsets. A resident flushed length one page
/// ahead of the card's would put the retry's page past the bytes it repeats and publish a ride nearly
/// twice the length it recorded.
#[test]
fn a_checkpoint_that_fails_after_its_page_flush_does_not_advance_the_flushed_length() {
    let ride = recording();
    let disk = card(41, &holding(&[ride], 6), 0);
    let faulty = FaultOnce::new(&disk);
    let mut store = FlatStore::mount(&faulty);

    let tail = payload(PROGRAM_PAGE + 300);
    let checkpoint =
        || RideCheckpoint { id: ObjectId(1), revision: Revision(1), tail: &tail, payload_crc: crc32(&tail) };

    // One whole page and a bit: the page flush is the first write, the slot header the second.
    faulty.fault_after(MediaOp::Write, 1);
    assert_eq!(store.journal(checkpoint()), Err(super::error::StoreError::Media));
    assert!(faulty.fired(), "the probe never reached the slot write");
    store.journal(checkpoint()).unwrap();
    disk.reboot();

    let store = FlatStore::mount(&disk);
    let recovered = store.recovered_ride().unwrap();
    assert_eq!(recovered.flushed, PROGRAM_PAGE as u64, "the refused checkpoint's page flush was counted twice");
    assert_eq!(recovered.payload_len(), tail.len() as u64, "the retry published a length the ride never recorded");

    // And the bytes are the ride's, in order: the flushed prefix off the card, then the recovered tail.
    let mut whole = Vec::new();
    for block in 0..recovered.flushed / BLOCK as u64 {
        whole.extend_from_slice(&disk.block(EXTENT_AREA + block));
    }
    let mut recovered_tail = vec![0u8; recovered.tail_len as usize];
    store.recovered_tail(&mut recovered_tail).unwrap();
    whole.extend_from_slice(&recovered_tail);
    assert_eq!(whole, tail, "the retry left the payload out of order on the card");
    assert_eq!(crc32(&whole), recovered.payload_crc);
}

/// §6.2's hold rule asks the catalog which of a closing hold's extents an entry still names. A read
/// that fails has not answered that question, and "the entry is gone" is not the answer to assume: an
/// extent a live entry names would go back to the allocator, the next object would be written over it,
/// and the commit publishing that would leave a catalog whose overlap only a mount can see — by which
/// point the card is unreadable. So the extents stay allocated until the next mount rebuilds the map
/// from the catalog.
#[test]
fn a_close_whose_catalog_read_fails_keeps_the_extents_allocated() {
    let route = entry(1, 1, ObjectKind::Route, EntryFlags::NONE, 3_000, "Grimsel Loop", &[(0, 1)]);
    let model = holding(&[route], 4);
    let disk = card(42, &model, 0);
    let faulty = FaultOnce::new(&disk);
    let mut store = FlatStore::mount(&faulty);

    let handle = store.open(ObjectId(1), None).unwrap();
    assert_eq!(store.free_extents(), EXTENTS - 1);
    faulty.fault_next(MediaOp::Read);
    store.close(handle);
    assert!(faulty.fired(), "the probe never reached the catalog read");
    assert_eq!(store.free_extents(), EXTENTS - 1, "a failed read freed a live entry's extents");

    // The consequence it would have had, run out: the next object goes somewhere else, and the card the
    // commit leaves still mounts.
    let published = entry(2, 1, ObjectKind::Trip, EntryFlags::NONE, 600, "Alps", &[(1, 1)]);
    let mut allocation = store.allocate(600).unwrap();
    store.write(&mut allocation, &payload(600)).unwrap();
    store.commit(&[Mutation::Put { meta: published.meta, source: PutSource::Fresh(allocation) }]).unwrap();
    disk.reboot();

    let mut expected = model.clone();
    expected.apply(&[Change::Put(published)]);
    let mut store = FlatStore::mount(&disk);
    assert_eq!(store.mode(), Mode::ReadWrite, "the commit published a catalog naming one extent twice");
    assert_eq!(snapshot(&mut store), expected.snapshot());
}

/// A fragmented allocation is several block writes, and one of them can fail with the others already on
/// the card. What must survive that is the reservation: `row` names an `Allocation` by its cursor as
/// well as its nonce, so a cursor left ahead of the caller's makes the row unnameable — the transfer
/// cannot be published, cannot be retried, and cannot be cancelled either, which wedges a row and its
/// extents until the next mount (#1407's trace). The cursor goes back where it was instead.
fn fragmented(seed: u64) -> (Model, SparseDisk) {
    // Extent 1 is taken, so a two-extent allocation gets two ranges and `write` iterates twice.
    let mut model = empty();
    model.entries.push(entry(9, 1, ObjectKind::Route, EntryFlags::NONE, 3_000, "", &[(1, 1)]));
    model.next_object = 10;
    let disk = card(seed, &model, 0);
    (model, disk)
}

#[test]
fn a_write_that_fails_leaves_an_allocation_its_caller_can_cancel() {
    let (_, disk) = fragmented(46);
    let faulty = FaultOnce::new(&disk);
    let mut store = FlatStore::mount(&faulty);
    assert_eq!(store.free_extents(), EXTENTS - 1);

    let bytes = payload(2 << 20);
    let mut allocation = store.allocate(bytes.len() as u64).unwrap();
    faulty.fault_after(MediaOp::Write, 1);
    assert_eq!(store.write(&mut allocation, &bytes), Err(super::error::StoreError::Media));
    assert!(faulty.fired(), "the probe never reached the second range's write");

    store.cancel(allocation);
    assert_eq!(store.free_extents(), EXTENTS - 1, "a failed write wedged the extents it had reserved");
    // And the row went with them: both reservation rows have to be free, which two allocations prove.
    let first = store.allocate(bytes.len() as u64).unwrap();
    let second = store.allocate(bytes.len() as u64).unwrap();
    store.cancel(first);
    store.cancel(second);
    assert_eq!(store.free_extents(), EXTENTS - 1);
    disk.reboot();
    assert_eq!(FlatStore::mount(&disk).free_extents(), EXTENTS - 1, "and the next mount says the same");
}

/// The other half of the same rule: the cursor did not move, so the retry writes the same bytes to the
/// same payload offsets, and what the commit publishes is what a reader gets back. `Snapshot` reads every
/// payload through the seam and hashes it against the entry's CRC, so this is the byte-correctness claim
/// and not a "the call returned Ok" claim.
#[test]
fn a_write_that_fails_may_be_retried_and_publishes_the_bytes_it_claims() {
    let (model, disk) = fragmented(47);
    let faulty = FaultOnce::new(&disk);
    let mut store = FlatStore::mount(&faulty);

    let bytes = payload(2 << 20);
    let mut allocation = store.allocate(bytes.len() as u64).unwrap();
    faulty.fault_after(MediaOp::Write, 1);
    assert_eq!(store.write(&mut allocation, &bytes), Err(super::error::StoreError::Media));
    store.write(&mut allocation, &bytes).unwrap();

    let published =
        entry(10, 1, ObjectKind::MapShard, EntryFlags::NONE, bytes.len() as u64, "shard", &[(0, 1), (2, 1)]);
    store.commit(&[Mutation::Put { meta: published.meta, source: PutSource::Fresh(allocation) }]).unwrap();
    let mut expected = model.clone();
    expected.apply(&[Change::Put(published)]);
    // Through the faulting wrapper, so the local `snapshot` helper's card type does not fit.
    assert_eq!(model::snapshot(&mut store).expect("a mounted store"), expected.snapshot());
    disk.reboot();
    assert_eq!(snapshot(&mut FlatStore::mount(&disk)), expected.snapshot(), "the retry left the payload torn");
}
