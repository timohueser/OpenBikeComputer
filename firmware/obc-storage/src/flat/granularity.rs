//! The aliasing invariant, measured rather than asserted: **what a caller can still do while the
//! store is in the middle of a card command.**
//!
//! [`store`](super::store)'s rules 2 and 3 are the whole point of FS7 slice 1 and, until this file,
//! they lived only in comments — a borrow of the free map held across an entire commit conflicts with
//! nothing the rest of the suite does, so all 499 tests pass with the invariant broken. That is not a
//! gap worth leaving open, because slice 3 (the board's storage task) is *built on* the invariant: it
//! interleaves a render read into a commit's gaps, and if there are no gaps the design does not work.
//!
//! So this measures it. A [`ReentrantCard`] wraps the sim card and, on **every** card command the
//! store issues, re-enters the store from inside the driver and tries four things a real caller
//! does:
//!
//! | Probe | Seam call | Cells it needs |
//! |---|---|---|
//! | `reader` | [`FlatStore::with_source`] — open, read, close | `holds` (exclusive), then `free` (exclusive) |
//! | `listing` | [`Store::entries`] drained + `entries_ok` | none — the claim is that it needs none |
//! | `free_space` | [`FlatStore::free_extents`] | `free` (shared) |
//! | `writer` | [`FlatStore::cancel`] of a token naming no row | `reservations` (exclusive) |
//!
//! Each runs under [`catch_unwind`](std::panic::catch_unwind), so a `RefCell` that refuses is
//! recorded rather than fatal — on the device that same refusal is a hard fault, which is exactly why
//! the counts below have to be pinned instead of trusted.
//!
//! **What the numbers say**, for the one scenario below — a `write` that leaves a partial block, then
//! the `commit` that publishes it:
//!
//! | Phase | commands | reader | listing | free space | writer |
//! |---|---|---|---|---|---|
//! | `write` | 1 | 1 / 0 | 1 / 0 | 1 / 0 | 0 / **1** |
//! | `commit` | 10 | 10 / 0 | 10 / 0 | 10 / 0 | 9 / **1** |
//!
//! (served / refused.) The three reader-side probes succeed at *every* command of both — including the
//! staging flush, the body stream and the gate write. That is rule 2, and it is the property slice 3
//! needs. The `writer` probe is refused at exactly the commands rule 3 discloses — the ones a
//! reservation's staging block is written out of — and nowhere else: **one command per reservation,
//! not a phase**. A whole-commit lock would show `served: 0` on that row, and a free map held across
//! the gate write takes the `reader` and `free space` rows to `8 / 2` — both checked by hand against
//! this probe before it was pinned.
//!
//! **Mount is not measured, and cannot be**: the probe is armed with a pointer to the store, and
//! `mount` runs inside the constructor that produces it. `load` holds the free map across its scan,
//! which `load`'s own docs disclose as the constructor exemption; nothing else can reach a store that
//! does not exist yet.

use std::cell::Cell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::NonNull;
use std::vec::Vec;

use super::device::BlockDevice;
use super::layout::{Geometry, EXTENT_AREA};
use super::seam::{
    Allocation, DisplayName, EntryFlags, EntryMeta, Mutation, ObjectId, ObjectKind, PutSource, Revision, Store, StoreId,
};
use super::sim::{DiskError, SparseDisk};
use super::store::FlatStore;

const STORE: StoreId = StoreId([0x9c; 16]);
const LEN: usize = 6_000;

fn payload() -> Vec<u8> {
    (0..LEN).map(|i| (i * 17 + 3) as u8).collect()
}

/// What one probe found, over the commands of one phase.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Tally {
    served: u32,
    refused: u32,
}

impl Tally {
    fn commands(self) -> u32 {
        self.served + self.refused
    }
}

/// The four probes' tallies over one phase.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Phase {
    reader: Tally,
    listing: Tally,
    free_space: Tally,
    writer: Tally,
}

/// A sim card that re-enters the store on every command it is asked to perform.
///
/// The store owns its device by value, and here that value is a `&ReentrantCard` — so the card is a
/// separate local that outlives nothing and is outlived by nothing. That is what makes the back
/// pointer expressible at all: the card does not own the store and the store does not own the card.
struct ReentrantCard {
    inner: SparseDisk,
    /// The store to re-enter, erased to `*const ()` because the honest type is self-referential:
    /// `FlatStore<&ReentrantCard>` names the card that holds the pointer.
    store: Cell<Option<NonNull<()>>>,
    /// Re-entry guard. The probes issue card commands of their own, and without this the first one
    /// would recurse until the stack ran out.
    probing: Cell<bool>,
    tally: Cell<Phase>,
    /// The object the reader probe reads, once one exists.
    subject: Cell<Option<ObjectId>>,
    /// A token naming no live row. `cancel` takes the reservations borrow *before* it discovers that,
    /// which is precisely the borrow this probe is measuring.
    bogus: Allocation,
}

impl ReentrantCard {
    fn new(inner: SparseDisk, bogus: Allocation) -> Self {
        ReentrantCard {
            inner,
            store: Cell::new(None),
            probing: Cell::new(false),
            tally: Cell::new(Phase::default()),
            subject: Cell::new(None),
            bogus,
        }
    }

    /// Points the card at the store that owns it. Called once, after `mount` has returned and the
    /// store has come to rest in a local that is not moved again.
    fn arm(&self, store: &FlatStore<&ReentrantCard>) {
        self.store.set(Some(NonNull::from(store).cast::<()>()));
    }

    /// Stops probing — before the store drops, so no probe can outlive it.
    fn disarm(&self) {
        self.store.set(None);
    }

    fn take(&self) -> Phase {
        let phase = self.tally.get();
        self.tally.set(Phase::default());
        phase
    }

    /// The store, as a shared reference.
    ///
    /// SAFETY: three facts, and all three are checked by construction rather than assumed.
    ///
    /// 1. **The pointer is valid whenever it is `Some`.** [`arm`](Self::arm) sets it from a live
    ///    `&FlatStore` that is a local in the test body, already returned from `mount` and never moved
    ///    afterwards; [`disarm`](Self::disarm) clears it before that local goes out of scope. The only
    ///    code that dereferences it runs *inside* a card command, which can only be running because
    ///    the store is executing a method — so the store is alive, borrowed, and at a fixed address.
    /// 2. **It is only ever a shared reference.** The whole point of this slice is that the seam is
    ///    `&self`, so `&FlatStore` is all a caller needs, and no `&mut` to the store exists anywhere in
    ///    the program while it is armed. Several live `&` to one store is exactly the aliasing the
    ///    board will have.
    /// 3. **It is single-threaded and synchronous.** The re-entry happens on the same thread, inside
    ///    the store's own call stack. `FlatStore` is neither `Send` nor `Sync` (it holds `Cell`s), so
    ///    the compiler rules out the case this reasoning does not cover.
    ///
    /// This is `cfg(test)`-only — the module is declared `#[cfg(test)]` in
    /// [`flat`](super), alongside `crash` and `cost`, so no device build can reach it.
    fn store(&self) -> Option<&FlatStore<&Self>> {
        let raw = self.store.get()?;
        Some(unsafe { &*(raw.as_ptr() as *const FlatStore<&Self>) })
    }

    /// One re-entry, at one card command.
    fn probe(&self) {
        if self.probing.get() {
            return;
        }
        let Some(store) = self.store() else { return };
        self.probing.set(true);
        let mut phase = self.tally.get();

        // A read of a whole object, through the public read seam: open, read, close. It needs the hold
        // table exclusively and then the free map exclusively, so it is the strongest statement rule 2
        // makes — a render's chunk read is this call.
        if let Some(id) = self.subject.get() {
            let ok = catch_unwind(AssertUnwindSafe(|| {
                store.with_source(id, None, |source| {
                    let mut head = [0u8; 512];
                    obc_formats::io::ByteSource::read_at(source, 0, &mut head).is_ok()
                })
            }));
            tick(&mut phase.reader, matches!(ok, Ok(Ok(true))));
        }

        // The catalog view. It should need no borrow at all, which is what lets a `LIST` page be
        // drained while a commit runs.
        let ok = catch_unwind(AssertUnwindSafe(|| {
            let count = Store::entries(store).count();
            (count, store.entries_ok())
        }));
        tick(&mut phase.listing, matches!(ok, Ok((_, true))));

        // Free space: the shared half of the free map, which is what a `PUT` admission asks.
        let ok = catch_unwind(AssertUnwindSafe(|| store.free_extents()));
        tick(&mut phase.free_space, ok.is_ok());

        // And the writer's cell, which rule 3 says is *not* always available.
        let ok = catch_unwind(AssertUnwindSafe(|| store.cancel(self.bogus)));
        tick(&mut phase.writer, ok.is_ok());

        self.tally.set(phase);
        self.probing.set(false);
    }
}

fn tick(tally: &mut Tally, served: bool) {
    if served {
        tally.served += 1;
    } else {
        tally.refused += 1;
    }
}

impl BlockDevice for &ReentrantCard {
    type Error = DiskError;

    fn block_count(&self) -> Result<u64, DiskError> {
        (&self.inner).block_count()
    }

    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), DiskError> {
        self.probe();
        (&self.inner).read(lba, buf)
    }

    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), DiskError> {
        self.probe();
        (&self.inner).write(lba, buf)
    }

    fn sync(&self) -> Result<(), DiskError> {
        self.probe();
        (&self.inner).sync()
    }
}

fn meta(id: ObjectId, revision: Revision, len: u64, name: &str) -> EntryMeta {
    EntryMeta {
        id,
        revision,
        kind: ObjectKind::MapShard,
        flags: EntryFlags::NONE,
        payload_len: len,
        payload_crc: 0,
        name: DisplayName::new(name).expect("a short name"),
    }
}

/// The whole measurement, as one scenario: seed an object to read, arm the probe, then run a `write`
/// and a `commit` with every command re-entered.
fn measure() -> (Phase, Phase) {
    let blank = SparseDisk::blank(EXTENT_AREA + Geometry::DEFAULT.extent_blocks() * 12, 5);
    // A token that names no row, built before the card so the card can carry it. `cancel` matches an
    // `Allocation` on all four fields, so a nonce no `allocate` ever handed out cannot match.
    let bogus = {
        let store = FlatStore::initialize(&blank, STORE).expect("an expressible card");
        let allocation = store.allocate(64).expect("an extent is free");
        store.cancel(allocation);
        allocation
    };
    let card = ReentrantCard::new(blank, bogus);
    let store = FlatStore::mount(&card);

    // One committed object for the reader probe to read. Not measured — the probe is armed after it.
    let seeded = store.next_object_id();
    let mut allocation = store.allocate(LEN as u64).expect("an extent is free");
    store.write(&mut allocation, &payload()).expect("the payload fits");
    store
        .commit(&[Mutation::Put {
            meta: meta(seeded, Revision(1), LEN as u64, "read me"),
            source: PutSource::Fresh(allocation),
        }])
        .expect("the seed commits");

    card.subject.set(Some(seeded));
    card.arm(&store);

    // Phase 1: a `write` that leaves a partial block behind, so the commit has a staging flush to do.
    let published = store.next_object_id();
    let mut allocation = store.allocate(LEN as u64).expect("an extent is free");
    card.take();
    store.write(&mut allocation, &payload()).expect("the payload fits");
    let write = card.take();

    // Phase 2: the commit that publishes it.
    store
        .commit(&[Mutation::Put {
            meta: meta(published, Revision(1), LEN as u64, "published"),
            source: PutSource::Fresh(allocation),
        }])
        .expect("the commit lands");
    let commit = card.take();

    card.disarm();
    (write, commit)
}

/// **Rule 2, pinned: a reader is served at every card command a writer issues.**
///
/// The numbers are this scenario's and are meant to be re-derived if it changes; what must not change
/// is the shape. `refused` on the three reader-side probes is **zero**, in both phases, or the
/// interleaving slice 3 is designed around does not exist.
///
/// The `writer` row is rule 3, stated as a measurement rather than a promise: it is refused at
/// exactly the commands that write a reservation's staging block out — one in the `write` (the
/// partial block `fill` stages) and one in the `commit` (the staging flush) — and served at every
/// other command, including all of the body stream and the gate write. A whole-commit lock would show
/// `served: 0` here.
#[test]
fn a_reader_is_served_at_every_command_of_a_write_and_a_commit() {
    let (write, commit) = measure();

    // The command counts this scenario issues, pinned so "zero refusals" cannot become vacuous by a
    // scenario that quietly stopped issuing commands. These two are the *scenario's*, not the format's
    // — `flat::cost` is where a command count is a contract — so re-derive them if the scenario moves.
    assert_eq!(write.listing.commands(), 1, "the write issues one command: the partial staging block ({write:?})");
    assert_eq!(commit.listing.commands(), 10, "the commit's command count moved ({commit:?})");

    for (name, phase) in [("write", write), ("commit", commit)] {
        assert_eq!(phase.reader.refused, 0, "{name}: a read was refused mid-command — rule 2 is broken ({phase:?})");
        assert_eq!(phase.listing.refused, 0, "{name}: a listing was refused mid-command ({phase:?})");
        assert_eq!(phase.free_space.refused, 0, "{name}: free space was refused mid-command ({phase:?})");
        assert_eq!(phase.reader.served, phase.listing.commands(), "{name}: the reader probe skipped a command");
    }

    // Rule 3, both halves: the exception is real, and it is *one command per reservation* rather than
    // a phase. A regression that widened it — a reservations borrow taken for the whole commit — would
    // fail the second assertion, and one that removed the disclosure would fail the first.
    assert_eq!(write.writer.refused, 1, "the write's staging command is rule 3's, and there is one ({write:?})");
    assert_eq!(commit.writer.refused, 1, "the commit's staging flush is one command, not a phase ({commit:?})");
    assert_eq!(commit.writer.served, 9, "every other command of the commit is open to a writer ({commit:?})");
}

/// **The probe's positive control.** It is only worth its lines if it can *see* the failure it is
/// aiming at, and a probe that quietly stopped re-entering — an arming bug, a stuck `probing` flag —
/// would report zero refusals forever and read as a pass.
///
/// [`FlatStore::hold_free_across_a_command`] is rule 2 broken on purpose: one card command with the
/// free map held. Everything that needs that cell must be refused during it.
///
/// The listing is the interesting row: it comes through anyway, because it borrows no cell at all.
/// That is not incidental — it is why a `LIST` page can be drained while a commit runs.
#[test]
fn the_probe_detects_a_borrow_held_across_a_command() {
    let blank = SparseDisk::blank(EXTENT_AREA + Geometry::DEFAULT.extent_blocks() * 12, 11);
    let bogus = {
        let store = FlatStore::initialize(&blank, STORE).expect("an expressible card");
        let allocation = store.allocate(64).expect("an extent is free");
        store.cancel(allocation);
        allocation
    };
    let card = ReentrantCard::new(blank, bogus);
    let store = FlatStore::mount(&card);

    let seeded = store.next_object_id();
    let mut allocation = store.allocate(LEN as u64).expect("an extent is free");
    store.write(&mut allocation, &payload()).expect("the payload fits");
    store
        .commit(&[Mutation::Put {
            meta: meta(seeded, Revision(1), LEN as u64, "read me"),
            source: PutSource::Fresh(allocation),
        }])
        .expect("the seed commits");
    card.subject.set(Some(seeded));
    card.arm(&store);

    card.take();
    store.hold_free_across_a_command();
    let seen = card.take();
    card.disarm();

    assert_eq!(seen.free_space.commands(), 1, "the control issues exactly one command ({seen:?})");
    assert_eq!(seen.free_space.refused, 1, "the probe missed a free map held across a command ({seen:?})");
    assert_eq!(seen.reader.refused, 1, "a read closes through the free map, so it must be refused too ({seen:?})");
    assert_eq!(seen.listing.refused, 0, "a listing borrows no cell and comes through regardless ({seen:?})");
    assert_eq!(seen.writer.refused, 0, "the reservation table is a different cell and is untouched ({seen:?})");
}
