//! The read seam, joined to the byte seam: an open object as an [`ByteSource`].
//!
//! Everything that reads a format — `obc-reader`'s chunk caches, `obc-route`'s A\*, `RouteReader`,
//! the OBCW reader — consumes [`ByteSource`] and nothing else. The store speaks
//! [`open`](Store::open) and [`read`](Store::read). This module is the one adapter between them, and
//! it is deliberately the *only* thing in the crate that knows both vocabularies: no reader learns
//! what an `ObjectId` is, and the store learns nothing about chunks.
//!
//! ## The lifecycle, and why it is shaped like this
//!
//! `close` is mandatory — a dropped [`Handle`] leaks its row, and its extents, until the next mount
//! (`FLAT_Store_Format.md` §6.2). The obvious fix, closing in `Drop`, is still **impossible**, though
//! no longer for the reason it used to be: a `Drop` impl could reach `close` now that the whole seam
//! is `&self`, but it has no handle to pass it — [`release`](StoreSource::release) is what takes the
//! handle out, and a `Drop` that could take it too would be a second way to spend the same row.
//!
//! So the seam makes the pairing structural instead, in two shapes, and a consumer picks by how long
//! it holds the object:
//!
//! - **Short-lived** — [`FlatStore::with_source`]. Opens, runs the body, closes. Nothing to forget,
//!   and the borrow checker never sees a source outlive its close. It does **not** close on a panic —
//!   see its own docs.
//! - **Session-long** — [`StoreSource`] + [`release`](StoreSource::release), for the eleven shards
//!   and the terrain sidecar a mounted set holds from boot to unmount, where a scope is not
//!   available (they live in `.bss`, across `await`s). Here the pairing is enforced twice over: the
//!   source *owns* its handle and will only give it back through `release`, and dropping one that
//!   still holds a handle trips a `debug_assert` — so a leak fails loudly in every test and every
//!   host build rather than showing up as a card that will not mount a set after the third try.
//!
//! ## What a live source costs: a refcount, and no longer the store's mutability
//!
//! A `StoreSource` holds `&'a FlatStore`, and since #1256's owner ruling of 2026-08-18 **every seam
//! operation takes `&self`** — `allocate`, `write`, `commit`, `journal`, `cancel` and `close`
//! included. So sources and writers coexist: a mounted set can hold eleven `&'static` shard sources
//! plus terrain for the life of the image while an upload commits and a ride journals. That is the
//! board's actual shape, and it is the whole point of the ruling. (`obc_reader::MountedSet` borrows
//! `&'a dyn ByteSource`, so a `&'static` borrow of the store is what a mount *is*; under the old
//! `&mut` write half it pinned the store immutable forever, and `obc-link`'s mirror trait compounded
//! it by taking `&mut self` even to `open` the next shard.) The two alternatives were rejected by
//! name — per-call store passing, because it cannot fit under `ByteSource::read_at(&self)` without
//! threading context through the very consumers FS6 promised not to touch; and unsafe board-side
//! aliasing, because it discards the guarantee exactly where it matters most.
//!
//! **What was given up is one compile-time guarantee, and it is the one the type system was making
//! for free: an object could not be closed while a source read it, because a `close` needed `&mut`
//! and the source held `&`.** That property is now enforced at runtime instead, by the reader
//! refcount the hold table already keeps, and the downgrade is from *impossible* to *refused* — never
//! to *silent*:
//!
//! - A `StoreSource` **owns** its handle and surrenders it only through `release`, so the only close
//!   that can name a live source's row is a close of some *other* handle on the same object.
//! - [`FlatStore::close`] on a row with more than one reader spends a refcount and returns. The row,
//!   its ranges and its length are untouched, and the source keeps reading exactly the revision it
//!   resolved. `a_close_beside_a_live_source_is_refused_by_the_refcount` is where that is a fact
//!   rather than a claim.
//! - Extents a commit takes away from a held revision stay out of the allocator until the **last**
//!   reader closes — `FlatStore::release` asks the hold table before it frees anything, which is
//!   §6.2's rule and is what makes the refusal safe rather than merely polite.
//!
//! The other half of the ruling is granularity, and it is the store's to keep rather than this
//! adapter's: **the state borrow is per card command, never per commit.** [`store`](super::store)'s
//! module docs carry the three rules and the re-entrancy argument; what matters here is the
//! consequence — a source's `read_at` can be served in the gaps of a running commit, because that
//! commit's ~36 card commands hold no borrow this path needs.
//!
//! What is still *not* here is the board's cross-task layer: one storage task owning the writes, with
//! callers sending it messages, so that a commit's card commands and a render's reads interleave on a
//! real scheduler rather than merely being able to. That is FS7 slice 3's. Nothing in this module
//! precludes it — a `Cell`/`RefCell` store is the single-context shape of exactly that design — but
//! nothing in this module provides it either, and a `FlatStore` must not be shared between execution
//! contexts until it does.
//!
//! ## Addressing
//!
//! [`ByteSource`] is `u32`-addressed and every format that rides it — OBCM, OBCR, OBCT, OBCW — has
//! `u32` offsets, so 4 GiB − 1 is the whole addressable space on this side of the seam. An object
//! longer than that serves its addressable prefix and refuses everything past it with
//! [`Error::BadOffset`]; see [`StoreSource::len`]. It cannot serve wrong bytes, which is the only
//! property that matters here — a map that big is not a map this firmware can read anyway.

use obc_formats::io::{ByteSource, Error};

use super::device::BlockDevice;
use super::error::StoreError;
use super::seam::{ObjectId, Revision, Store};
use super::store::{FlatStore, Handle};

/// An open object, as a reader sees it.
///
/// Construct with [`FlatStore::source`], finish with [`release`](Self::release) — or use
/// [`FlatStore::with_source`] and let the scope do both. See the module docs for why `Drop` cannot.
pub struct StoreSource<'a, D: BlockDevice> {
    store: &'a FlatStore<D>,
    /// `None` once [`release`](Self::release) has taken it. The `Option` is what lets `release`
    /// consume the handle out of a type that also has a `Drop` impl; the branch it costs per read is
    /// a predictable one against a multi-millisecond card read.
    handle: Option<Handle>,
    /// The addressable length: the payload's, saturated at [`u32::MAX`]. Captured once — the handle
    /// serves one revision, whose length does not move under it.
    len: u32,
}

impl<'a, D: BlockDevice> StoreSource<'a, D> {
    /// Wrap an already-open `handle` of `store`'s.
    ///
    /// Prefer [`FlatStore::source`], which opens and wraps in one step. This exists for the caller
    /// that already holds a handle — the board's mount path, which opens every shard before it has
    /// anywhere to put the sources, and is exactly where an index slip would put the wrong handle
    /// against the wrong store.
    ///
    /// `Err` gives the handle **back** when it does not resolve against `store` — because it belongs
    /// to a different store, or because it has already been closed and its row reused. The handle is
    /// returned rather than swallowed for the obvious reason: whoever owns it still owes it a
    /// `close`, against whichever store it really came from. Silently treating it as a zero-length
    /// object was the alternative, and it would have turned a mount-time index slip into a shard that
    /// reads as empty forever.
    pub fn over(store: &'a FlatStore<D>, handle: Handle) -> Result<Self, Handle> {
        match store.handle_len(&handle) {
            Some(payload_len) => Ok(StoreSource::with_len(store, handle, payload_len)),
            None => Err(handle),
        }
    }

    /// The common tail of [`over`](Self::over) and [`FlatStore::source`], where the one saturation in
    /// this module lives.
    fn with_len(store: &'a FlatStore<D>, handle: Handle, payload_len: u64) -> Self {
        StoreSource { store, handle: Some(handle), len: payload_len.min(u32::MAX as u64) as u32 }
    }

    /// Surrender the handle so the store can close it. **This is the only way out** — see the module
    /// docs.
    ///
    /// ```ignore
    /// let handle = source.release();
    /// store.close(handle);
    /// ```
    pub fn release(mut self) -> Handle {
        self.handle.take().expect("a StoreSource holds its handle until exactly one `release`")
    }

    /// The object this source reads.
    pub fn id(&self) -> ObjectId {
        self.handle().id()
    }

    /// The revision this source is pinned to.
    pub fn revision(&self) -> Revision {
        self.handle().revision()
    }

    fn handle(&self) -> &Handle {
        self.handle.as_ref().expect("a released StoreSource is consumed and cannot be read")
    }
}

impl<D: BlockDevice> Drop for StoreSource<'_, D> {
    fn drop(&mut self) {
        // A panic is already unwinding through here — from a `body` passed to `with_source`, or from
        // any failed assertion in a test holding a source. Asserting now would be a *second* panic
        // during unwind, which aborts the process: the run dies with `SIGABRT` and takes the original
        // failure's message with it. The leak is the lesser problem, and the panic that caused it is
        // the one worth reading.
        #[cfg(any(test, feature = "std"))]
        if std::thread::panicking() {
            return;
        }
        // Not a `panic!`: on the device this compiles out, and a hard fault is never the right answer
        // to a leaked row (the cost is one row until the next mount). On the host it fails the test
        // that leaked, which is where the mistake is cheap to fix.
        debug_assert!(
            self.handle.is_none(),
            "a StoreSource was dropped without `release`; its row and extents leak until the next mount",
        );
    }
}

impl<D: BlockDevice> ByteSource for StoreSource<'_, D> {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), Error> {
        // Range first, medium second — the same order, and for the same reason, as every other
        // `ByteSource` in the tree: a caller asking past the end is a bad offset, and only a genuine
        // media failure is `Io`. Callers distinguish them (the weather A/B publisher refuses to
        // truncate a slot it could not read, rather than one it read as short).
        let count = u32::try_from(buf.len()).map_err(|_| Error::BadOffset)?;
        let end = offset.checked_add(count).ok_or(Error::BadOffset)?;
        if end > self.len {
            return Err(Error::BadOffset);
        }
        let handle = self.handle();
        let mut done = 0usize;
        while done < buf.len() {
            // The store returns short only at end of payload, and the range check above proved we are
            // not near it — so a zero-length return is the store disagreeing with its own hold about
            // the length, which is an I/O-class fault rather than a caller error. Loop anyway: the
            // seam's contract is "bytes read", and depending on it returning everything in one turn
            // would be depending on an implementation detail.
            match self.store.read(handle, u64::from(offset) + done as u64, &mut buf[done..]) {
                Ok(0) => return Err(Error::Io),
                Ok(n) => done += n,
                Err(_) => return Err(Error::Io),
            }
        }
        Ok(())
    }

    /// The addressable length — the payload's, saturated at [`u32::MAX`]. See the module docs on
    /// addressing: past 4 GiB − 1 there is no `u32` offset to name the bytes with, so the source
    /// reports what it can serve and [`read_at`](Self::read_at) refuses the rest.
    fn len(&self) -> u32 {
        self.len
    }
}

impl<D: BlockDevice> FlatStore<D> {
    /// Open `id` and wrap it as a [`ByteSource`]. The caller owns the pairing: finish with
    /// [`StoreSource::release`] and [`close`](FlatStore::close).
    ///
    /// `revision` of `None` takes the head, exactly as [`Store::open`] does.
    pub fn source(&self, id: ObjectId, revision: Option<Revision>) -> Result<StoreSource<'_, D>, StoreError> {
        let handle = Store::open(self, id, revision)?;
        // `open` just wrote the row, so it resolves. This tail exists to keep the function total —
        // hence a typed error rather than an `expect`, which on the device would be a hard fault for
        // something that cannot happen. It *does* drop the handle `over` hands back, which is the
        // swallow this module refuses everywhere else; the `debug_assert` is what keeps that
        // exception honest, by failing loudly on the host if the unreachable ever becomes reachable.
        StoreSource::over(self, handle).map_err(|_returned| {
            debug_assert!(false, "the row `open` just wrote did not resolve; its handle is being dropped");
            StoreError::Invalid
        })
    }

    /// Open `id`, run `body` against it, and close it.
    ///
    /// This is the shape for everything that is not a session-long mount: a menu reading a header, a
    /// `STATUS` resolving an object, a test. See the module docs for the other shape.
    ///
    /// **It does not close on a panic.** `body` runs between the open and the close with no unwind
    /// guard, so a panic inside it leaks the row until the next mount — and `StoreSource`'s own leak
    /// detector deliberately stays quiet during unwind rather than aborting the process on top of the
    /// original failure. That is the accepted trade: this firmware does not unwind (`panic = "abort"`
    /// on the device), and a host that panics is a test that has already failed. There is no early
    /// return between the open and the close either; the close is simply the next statement.
    pub fn with_source<R>(
        &self,
        id: ObjectId,
        revision: Option<Revision>,
        body: impl FnOnce(&StoreSource<'_, D>) -> R,
    ) -> Result<R, StoreError> {
        let handle = Store::open(self, id, revision)?;
        // Unreachable for the same reason as in `source`, and dropping the returned handle is the
        // same acknowledged exception — see there.
        let source = StoreSource::over(self, handle).map_err(|_returned| {
            debug_assert!(false, "the row `open` just wrote did not resolve; its handle is being dropped");
            StoreError::Invalid
        })?;
        let out = body(&source);
        let handle = source.release();
        self.close(handle);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use std::vec;

    use super::*;
    use crate::flat::layout::{Geometry, EXTENT_AREA};
    use crate::flat::seam::{DisplayName, EntryFlags, EntryMeta, Mutation, ObjectKind, PutSource, StoreId};
    use crate::flat::sim::SparseDisk;
    use crate::flat::store::MAX_OPEN_OBJECTS;

    const STORE: StoreId = StoreId([0x5e; 16]);
    const LEN: usize = 3_000;

    fn payload() -> vec::Vec<u8> {
        (0..LEN).map(|i| (i * 31 + 7) as u8).collect()
    }

    /// A card holding `count` committed objects, and their ids.
    fn fixture(count: usize) -> (SparseDisk, vec::Vec<ObjectId>) {
        let disk = SparseDisk::blank(EXTENT_AREA + Geometry::DEFAULT.extent_blocks() * (count as u64 + 4), 3);
        let store = FlatStore::initialize(&disk, STORE).expect("an expressible card");
        let mut ids = vec::Vec::new();
        for index in 0..count {
            let id = store.next_object_id();
            let mut allocation = store.allocate(LEN as u64).expect("an extent is free");
            store.write(&mut allocation, &payload()).expect("the payload fits");
            let meta = EntryMeta {
                id,
                revision: Revision(1),
                kind: ObjectKind::MapShard,
                flags: EntryFlags::NONE,
                payload_len: LEN as u64,
                payload_crc: 0,
                name: DisplayName::new(&std::format!("shard {index}")).expect("a short name"),
            };
            store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]).expect("the commit lands");
            ids.push(id);
        }
        (disk, ids)
    }

    /// The adapter must serve exactly what the store serves — the seam may not become a place where
    /// bytes change.
    #[test]
    fn a_source_reads_byte_identically_to_the_store_underneath_it() {
        let (disk, ids) = fixture(1);
        let store = FlatStore::mount(&disk);
        let source = store.source(ids[0], None).expect("the object opens");
        assert_eq!(source.len(), LEN as u32);
        assert_eq!(source.id(), ids[0]);
        assert_eq!(source.revision(), Revision(1));

        for (offset, len) in [(0u32, LEN), (0, 1), (511, 2), (1_000, 512), (LEN as u32 - 1, 1)] {
            let mut through_seam = vec::from_elem(0u8, len);
            source.read_at(offset, &mut through_seam).expect("inside the object");
            let mut direct = vec::from_elem(0u8, len);
            let got = store.read(source.handle(), u64::from(offset), &mut direct).expect("the store reads");
            assert_eq!(got, len, "the store filled the window");
            assert_eq!(through_seam, direct, "the adapter changed bytes at ({offset}, {len})");
            assert_eq!(through_seam, payload()[offset as usize..offset as usize + len], "and both differ from truth");
        }

        let handle = source.release();
        store.close(handle);
    }

    /// Past the end is a caller error, not a media one — including a window that *starts* inside and
    /// straddles the end, which is the case a length check on `offset` alone would let through.
    #[test]
    fn reads_past_the_end_are_refused_as_bad_offsets() {
        let (disk, ids) = fixture(1);
        let store = FlatStore::mount(&disk);
        store
            .with_source(ids[0], None, |source| {
                let mut buf = [0u8; 16];
                assert_eq!(source.read_at(LEN as u32, &mut buf).unwrap_err(), Error::BadOffset, "starting at the end");
                assert_eq!(
                    source.read_at(LEN as u32 - 8, &mut buf).unwrap_err(),
                    Error::BadOffset,
                    "straddling the end"
                );
                assert_eq!(source.read_at(u32::MAX, &mut buf).unwrap_err(), Error::BadOffset, "an offset that wraps");
                source.read_at(LEN as u32 - 16, &mut buf).expect("the last full window is fine");
            })
            .expect("the object opens");
    }

    /// `with_source` must hand its row back. Proved by exhaustion rather than by inspection: walk more
    /// *distinct* objects than the table has rows, one scope at a time. A scope that leaked its row
    /// would run the table dry and fail partway.
    ///
    /// Distinct objects matter — repeating one would share a row by refcount and pass whether or not
    /// the close happened. Sequential scopes matter for the same reason they always did, and it is
    /// now the *only* reason: since the seam went `&self`, nesting them would compile.
    #[test]
    fn with_source_returns_its_row_to_the_table() {
        let objects = MAX_OPEN_OBJECTS + 4;
        let (disk, ids) = fixture(objects);
        let store = FlatStore::mount(&disk);

        for (index, id) in ids.iter().enumerate() {
            let len = store.with_source(*id, None, |source| source.len()).unwrap_or_else(|error| {
                panic!("object {index} of {objects} could not open ({error:?}) — a scope leaked its row")
            });
            assert_eq!(len, LEN as u32);
        }
    }

    /// A foreign or stale handle must not become a zero-length source that reads as empty forever —
    /// it comes back, so its real owner can still close it.
    #[test]
    fn a_handle_that_does_not_resolve_is_handed_back() {
        let (disk, ids) = fixture(1);
        let (other_disk, other_ids) = fixture(1);
        let store = FlatStore::mount(&disk);
        let other = FlatStore::mount(&other_disk);

        let foreign = other.source(other_ids[0], None).expect("the other card's object opens").release();
        let returned = StoreSource::over(&store, foreign).err().expect("a foreign handle must be refused");

        // And it is the same handle, still closable against the store it came from.
        assert_eq!(returned.id(), other_ids[0]);
        other.close(returned);

        // The local one still works, so the refusal was about the handle and not the store.
        store.with_source(ids[0], None, |source| assert_eq!(source.len(), LEN as u32)).expect("the local object opens");
    }

    /// The saturation at the top of the address space. A real 4 GiB object is not constructible in a
    /// test, so this pins the arithmetic where it lives.
    #[test]
    fn a_payload_past_the_u32_ceiling_reports_the_addressable_prefix() {
        let (disk, ids) = fixture(1);
        let store = FlatStore::mount(&disk);
        let handle = Store::open(&store, ids[0], None).expect("the object opens");

        let huge = StoreSource::with_len(&store, handle, u64::from(u32::MAX) + 4_096);
        assert_eq!(huge.len(), u32::MAX, "the reported length saturates rather than wrapping");
        // Inside the *reported* length but far past the bytes that exist: the range check passes and
        // the store's short read is what refuses it. `u32::MAX - 4` rather than `- 3` because the
        // window has to end exactly at the ceiling, not one past it — a wrapping window is the other
        // test's case.
        let mut buf = [0u8; 4];
        assert_eq!(huge.read_at(u32::MAX - 4, &mut buf).unwrap_err(), Error::Io, "past the payload is not silent");

        let handle = huge.release();
        store.close(handle);
    }

    /// **The runtime refusal the ruling traded the compile-time one for.** Under the old `&mut` write
    /// half this test could not be written: a live source held `&` and `close` wanted `&mut`, so the
    /// borrow checker refused it. Now it compiles, so the hold table's refcount has to be the thing
    /// that holds — and it is what §6.2's "extents come back when the last reader lets go" was always
    /// resting on.
    ///
    /// Two readers on one object, the second closed while the first is mid-session: the survivor must
    /// keep reading the same bytes, and the store must not have taken the row apart underneath it.
    #[test]
    fn a_close_beside_a_live_source_is_refused_by_the_refcount() {
        let (disk, ids) = fixture(1);
        let store = FlatStore::mount(&disk);
        let free_before = store.free_extents();

        let live = store.source(ids[0], None).expect("the object opens");
        // A second handle on the same `(id, revision)`: the same row, refcount 2.
        let second = Store::open(&store, ids[0], None).expect("a second reader joins the row");
        store.close(second);

        // The row survived the close, which is the whole claim — a torn-down row would make this
        // read `Err(Io)` rather than the payload.
        let mut after = vec::from_elem(0u8, LEN);
        live.read_at(0, &mut after).expect("the live source still resolves its row");
        assert_eq!(after, payload(), "the surviving reader reads the revision it opened");
        assert_eq!(live.len(), LEN as u32);
        assert_eq!(store.free_extents(), free_before, "a spent refcount frees no extent");

        // And the last close is the one that does the work.
        let handle = live.release();
        store.close(handle);
        assert_eq!(store.free_extents(), free_before, "the entry still names them, so nothing moved");
        // The row really did come back: reopening resolves, which a row still counted as held by a
        // reader that no longer exists would also do — so this is checked by exhaustion instead, in
        // `with_source_returns_its_row_to_the_table`. Here it is only that the object is still whole.
        store.with_source(ids[0], None, |source| assert_eq!(source.len(), LEN as u32)).expect("it opens again");
    }

    /// **A later joiner may not shorten what an earlier reader is already serving.** The one real bug
    /// the aliasing rework introduced, caught in review, and the sequence is only expressible *because*
    /// of the rework: `source` → an amend that trims the entry → a second `open` on the same key. The
    /// second open joins the row by refcount and used to overwrite its length with the trimmed one,
    /// which handed the original source `Err(Io)` at offsets below the `len()` it had just reported —
    /// a silent truncation, which is the one outcome `source`'s docs promise the runtime refusal never
    /// degrades to.
    ///
    /// The read past the *new* end is the whole point: it is inside the first reader's revision, and
    /// §2.1 says a handle keeps reading the revision it resolved.
    #[test]
    fn a_second_open_cannot_shorten_a_reader_already_serving_the_row() {
        let (disk, ids) = fixture(1);
        let store = FlatStore::mount(&disk);

        let live = store.source(ids[0], None).expect("the object opens");
        assert_eq!(live.len(), LEN as u32);

        // An amend that trims the entry to a third of its length, beside the live source. `Amend`
        // keeps the extents the entry already holds and rewrites only the metadata.
        const SHORT: u64 = 1_000;
        let trimmed = EntryMeta {
            id: ids[0],
            revision: Revision(1),
            kind: ObjectKind::MapShard,
            flags: EntryFlags::NONE,
            payload_len: SHORT,
            payload_crc: 0,
            name: DisplayName::new("trimmed").expect("a short name"),
        };
        store
            .commit(&[Mutation::Put { meta: trimmed, source: PutSource::Amend }])
            .expect("the amend lands beside the open source");

        // The second reader joins the *same* row — same `(id, revision)`, so same hold.
        let joiner = Store::open(&store, ids[0], None).expect("a second reader joins the row");

        // The original source still reports, and still serves, the whole revision it resolved.
        assert_eq!(live.len(), LEN as u32, "the source's own length is captured at open and cannot move");
        let mut tail = [0u8; 64];
        live.read_at(LEN as u32 - 64, &mut tail).expect("past the amended end is still inside the resolved revision");
        assert_eq!(tail[..], payload()[LEN - 64..], "and the bytes are the object's own");

        // A fresh read through the joiner's handle is bounded by the row, which now holds the wider
        // of the two lengths — the join adopts a longer amend and refuses a shorter one.
        let mut whole = vec::from_elem(0u8, LEN);
        assert_eq!(
            store.read(&joiner, 0, &mut whole).expect("the joined handle reads"),
            LEN,
            "the row kept the longer length"
        );

        store.close(joiner);
        let handle = live.release();
        store.close(handle);
    }

    /// The other half of the same trade: a source and a writer coexisting at all. It is the board's
    /// shape — a mounted shard read while an upload commits — and before the ruling it did not
    /// compile, which is why it is worth a test of its own rather than a comment.
    #[test]
    fn a_commit_runs_while_a_source_is_open_and_the_source_is_unmoved() {
        let (disk, ids) = fixture(1);
        let store = FlatStore::mount(&disk);

        let live = store.source(ids[0], None).expect("the shard opens");
        let sequence = store.sequence();

        // A whole unrelated object published while the source is alive: allocate, write, commit.
        let id = store.next_object_id();
        let mut allocation = store.allocate(LEN as u64).expect("an extent is free");
        store.write(&mut allocation, &payload()).expect("the payload fits");
        let meta = EntryMeta {
            id,
            revision: Revision(1),
            kind: ObjectKind::MapShard,
            flags: EntryFlags::NONE,
            payload_len: LEN as u64,
            payload_crc: 0,
            name: DisplayName::new("uploaded mid-mount").expect("a short name"),
        };
        let after = store
            .commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }])
            .expect("the commit lands beside the open source");
        assert_eq!(after, sequence + 1, "the commit really happened");

        // The source is pinned to the revision it resolved and reads it after the catalog moved.
        let mut bytes = vec::from_elem(0u8, LEN);
        live.read_at(0, &mut bytes).expect("the source outlived the commit");
        assert_eq!(bytes, payload());

        let handle = live.release();
        store.close(handle);
    }

    /// **A listing that outlives its catalog stops, and says so.** The other case the `&self` seam made
    /// reachable: an `entries()` iterator can now be held across a commit, and two commits later the
    /// copy it is walking has been rewritten underneath its cursor. Serving those bytes as if they
    /// were the listing's own — with `entries_ok()` still `true` — would splice two catalogs together
    /// and call the result complete.
    #[test]
    fn a_listing_that_outlives_its_commit_stops_short_and_reports_it() {
        let (disk, ids) = fixture(3);
        let store = FlatStore::mount(&disk);

        // Drained inside its own moment: the whole catalog, and the flag agrees.
        assert_eq!(Store::entries(&store).count(), 3);
        assert!(store.entries_ok());

        // Now hold one open across a commit. The first entry is served — it was read before anything
        // moved — and the walk stops at the commit rather than crossing it.
        let mut listing = Store::entries(&store);
        assert!(listing.next().is_some(), "the first entry comes from the catalog the listing was made against");
        store
            .commit(&[Mutation::Remove { id: ids[2], revision: Revision(1) }])
            .expect("a commit lands while the listing is alive");
        assert!(listing.next().is_none(), "the listing does not cross the commit");
        drop(listing);
        assert!(!store.entries_ok(), "and a short listing is never silent");

        // The store itself is unharmed: a fresh listing is complete again, and one entry shorter.
        assert_eq!(Store::entries(&store).count(), 2);
        assert!(store.entries_ok());
    }

    /// The leak detector itself. Dropping a source that still holds its handle is the mistake it
    /// exists to catch, and a test that stops catching it is worse than no test.
    #[test]
    #[should_panic(expected = "dropped without `release`")]
    fn dropping_a_source_that_still_holds_its_handle_is_caught() {
        let (disk, ids) = fixture(1);
        let store = FlatStore::mount(&disk);
        let source = store.source(ids[0], None).expect("the object opens");
        drop(source);
    }
}
