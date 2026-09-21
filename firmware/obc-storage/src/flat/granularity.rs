//! The aliasing invariant, measured rather than asserted: what a caller can still do while the store
//! is in the middle of a card command.
//!
//! [`ReentrantCard`] wraps the sim card and, on every card command the store issues, re-enters the
//! store and tries four things a real caller does: a read through [`FlatStore::with_source`], which
//! needs `holds` and then `free` exclusively; a drained [`Store::entries`] listing, which should need
//! no cell at all; [`FlatStore::free_extents`], which shares `free`; and a [`FlatStore::cancel`] of a
//! token naming no row, which needs `reservations` exclusively. Each runs under
//! [`catch_unwind`](std::panic::catch_unwind), so a `RefCell` that refuses is recorded rather than
//! fatal — on the device that same refusal is a hard fault.
//!
//! The three reader-side probes are served at every command of a `write` and of a `commit`, the
//! staging flush, the body stream and the gate write included. That is [`store`](super::store)'s
//! rule 2, and it is what lets the board's storage task interleave a render read into a commit's
//! gaps. The writer probe is refused at exactly the commands a reservation's staging block is written
//! out of — one command per reservation, not a phase.
//!
//! Mount is not measured, and cannot be: the probe is armed with a pointer to the store, and `mount`
//! runs inside the constructor that produces it.

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
/// The store owns its device by value, and here that value is a `&ReentrantCard`, so the card does
/// not own the store and the store does not own the card. That is what makes the back pointer
/// expressible at all.
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
    /// A token naming no live row. `cancel` takes the reservations borrow before it discovers that,
    /// which is the borrow this probe measures.
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
    /// SAFETY: [`arm`](Self::arm) sets the pointer from a live `&FlatStore` local that is never moved
    /// afterwards, and [`disarm`](Self::disarm) clears it before that local goes out of scope. The only
    /// code that dereferences it runs inside a card command, which can only be running because the
    /// store is executing a method. It is only ever a shared reference, and no `&mut` to the store
    /// exists anywhere while it is armed. The re-entry is on the same thread inside the store's own
    /// call stack, and `FlatStore` is neither `Send` nor `Sync`, so the compiler rules out the rest.
    /// This is `cfg(test)`-only, so no device build can reach it.
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

        // A read of a whole object through the public read seam: open, read, close. It needs the hold
        // table exclusively and then the free map exclusively, so it is the strongest statement rule 2
        // makes.
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
        added_at_utc: 0,
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

/// Rule 2, pinned: a reader is served at every card command a writer issues. `refused` on the three
/// reader-side probes must be zero in both phases.
///
/// The `writer` row is rule 3 as a measurement: refused at exactly the two commands that write a
/// reservation's staging block out, and served at every other command. A whole-commit lock would show
/// `served: 0` here.
#[test]
fn a_reader_is_served_at_every_command_of_a_write_and_a_commit() {
    let (write, commit) = measure();

    // The command counts this scenario issues, pinned so "zero refusals" cannot become vacuous by a
    // scenario that quietly stopped issuing commands. These are the scenario's, not the format's.
    assert_eq!(write.listing.commands(), 1, "the write issues one command: the partial staging block ({write:?})");
    assert_eq!(commit.listing.commands(), 10, "the commit's command count moved ({commit:?})");

    for (name, phase) in [("write", write), ("commit", commit)] {
        assert_eq!(phase.reader.refused, 0, "{name}: a read was refused mid-command — rule 2 is broken ({phase:?})");
        assert_eq!(phase.listing.refused, 0, "{name}: a listing was refused mid-command ({phase:?})");
        assert_eq!(phase.free_space.refused, 0, "{name}: free space was refused mid-command ({phase:?})");
        assert_eq!(phase.reader.served, phase.listing.commands(), "{name}: the reader probe skipped a command");
    }

    // Rule 3, both halves: the exception is real, and it is one command per reservation rather than a
    // phase. A reservations borrow taken for the whole commit would fail the second assertion.
    assert_eq!(write.writer.refused, 1, "the write's staging command is rule 3's, and there is one ({write:?})");
    assert_eq!(commit.writer.refused, 1, "the commit's staging flush is one command, not a phase ({commit:?})");
    assert_eq!(commit.writer.served, 9, "every other command of the commit is open to a writer ({commit:?})");
}

/// The probe's positive control. A probe that quietly stopped re-entering would report zero refusals
/// forever and read as a pass.
///
/// [`FlatStore::hold_free_across_a_command`] holds the free map across one card command, so everything
/// that needs that cell must be refused during it. The listing comes through anyway, because it
/// borrows no cell at all.
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
