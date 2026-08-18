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
//! (`FLAT_Store_Format.md` §6.2). The obvious fix, closing in `Drop`, is **impossible**:
//! [`FlatStore::close`] needs `&mut` on the store (it returns extents to the free map) and a source
//! that is serving reads is holding `&`. A `Drop` impl has no way to get the one from the other.
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
//! ## What a live source costs: the store is immutable while one exists
//!
//! A `StoreSource` holds `&'a FlatStore`, and **every** mutator is `&mut self` — `allocate`, `write`,
//! `commit`, `journal`, `cancel` and `close`. So while any source is alive, none of them is callable.
//! The pleasant half of that is real: no object can be closed while a source is alive, so tearing
//! down a mounted set is all-or-nothing at the type level. The unpleasant half is the same fact.
//!
//! **The session-long board shape is not yet expressible in safe Rust.** A mounted set holds eleven
//! `&'static` shard sources plus terrain for the life of the image (`obc_reader::MountedSet` borrows
//! `&'a dyn ByteSource`), and a `&'static` borrow of the store pins it immutable *forever* — no
//! upload could commit, no ride could journal, for as long as a map is mounted. `obc-link`'s mirror
//! trait compounds it: its `open` takes `&mut self`, so even opening the next shard conflicts with
//! holding the last one.
//!
//! The two ways out are an interior-mutability write half (the store already keeps `holds` in a
//! `RefCell`; the free map and the reservations would have to join it) or passing the store to every
//! read instead of borrowing it once (`read(&self, store, offset, buf)`, which costs the
//! `ByteSource` seam its shape). **That is an architecture decision, not an implementation one**, and
//! it is recorded on #1256 as an FS7 blocker rather than settled here. Until it is settled, this
//! adapter is usable for scoped reads and for a host or a test; it is not yet usable for the board's
//! mount.
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
        // `open` just wrote the row, so it resolves. The `map_err` is the total-function tail rather
        // than a reachable path — hence a typed error and not an `expect`, which on the device would
        // be a hard fault for something that cannot happen.
        StoreSource::over(self, handle).map_err(|_| StoreError::Invalid)
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
        &mut self,
        id: ObjectId,
        revision: Option<Revision>,
        body: impl FnOnce(&StoreSource<'_, D>) -> R,
    ) -> Result<R, StoreError> {
        let handle = Store::open(&*self, id, revision)?;
        // The source borrows `*self` immutably; `release` consumes it, which ends that borrow and
        // lets the `&mut` close through. Sequencing, not cleverness — but it is why `body` cannot
        // stash the source somewhere it would outlive the close.
        let source = StoreSource::over(&*self, handle).map_err(|_| StoreError::Invalid)?;
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
        let mut store = FlatStore::initialize(&disk, STORE).expect("an expressible card");
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
        let mut store = FlatStore::mount(&disk);
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
        let mut store = FlatStore::mount(&disk);
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
    /// the close happened. Sequential scopes matter too, and not only for tidiness: holding sources
    /// while calling a `&mut` method does not compile, which is the module docs' point made by the
    /// borrow checker.
    #[test]
    fn with_source_returns_its_row_to_the_table() {
        let objects = MAX_OPEN_OBJECTS + 4;
        let (disk, ids) = fixture(objects);
        let mut store = FlatStore::mount(&disk);

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
        let mut store = FlatStore::mount(&disk);
        let mut other = FlatStore::mount(&other_disk);

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
        let mut store = FlatStore::mount(&disk);
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
