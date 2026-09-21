use obc_formats::io::{ByteSource, Error};

use super::device::BlockDevice;
use super::error::StoreError;
use super::seam::{ObjectId, Revision, Store};
use super::store::{FlatStore, Handle, SealedAllocation};

/// A borrowed view of an owned sealed reservation. It cannot outlive its cleanup capability.
pub struct SealedSource<'a, D: BlockDevice> {
    store: &'a FlatStore<D>,
    sealed: &'a SealedAllocation<'a>,
}

impl<D: BlockDevice> ByteSource for SealedSource<'_, D> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        if offset.checked_add(buf.len() as u64).is_none_or(|end| end > self.len()) {
            return Err(Error::BadOffset);
        }
        match self.store.read_sealed(self.sealed, offset, buf) {
            Ok(n) if n == buf.len() => Ok(()),
            _ => Err(Error::Io),
        }
    }

    fn len(&self) -> u64 {
        self.sealed.len()
    }
}

impl<D: BlockDevice> FlatStore<D> {
    pub fn sealed_source<'a>(&'a self, sealed: &'a SealedAllocation<'a>) -> SealedSource<'a, D> {
        SealedSource { store: self, sealed }
    }
}

/// An open object, as a reader sees it. Construct with [`FlatStore::source`] and finish with
/// [`release`](Self::release), or use [`FlatStore::with_source`] and let the scope do both.
pub struct StoreSource<'a, D: BlockDevice> {
    store: &'a FlatStore<D>,
    /// `None` once [`release`](Self::release) has taken it. The `Option` is what lets `release`
    /// consume the handle out of a type that also has a `Drop` impl.
    handle: Option<Handle>,
    /// The payload's length, captured once: the handle serves one revision, whose length does not
    /// move under it.
    len: u64,
}

impl<'a, D: BlockDevice> StoreSource<'a, D> {
    /// Wrap an already-open `handle` of `store`'s. Prefer [`FlatStore::source`], which opens and
    /// wraps in one step.
    ///
    /// `Err` gives the handle back when it does not resolve against `store`, because whoever owns it
    /// still owes it a `close` against whichever store it really came from. Treating it as a
    /// zero-length object would turn a mount-time index slip into a shard that reads as empty forever.
    pub fn over(store: &'a FlatStore<D>, handle: Handle) -> Result<Self, Handle> {
        match store.handle_len(&handle) {
            Some(payload_len) => Ok(StoreSource::with_len(store, handle, payload_len)),
            None => Err(handle),
        }
    }

    fn with_len(store: &'a FlatStore<D>, handle: Handle, payload_len: u64) -> Self {
        StoreSource { store, handle: Some(handle), len: payload_len }
    }

    /// Retained bytes remain readable after replacement, but no longer authorize planning.
    pub fn is_current(&self) -> bool {
        self.store.current_revision(self.id()).is_ok_and(|head| head == Some(self.revision()))
    }

    /// Surrender the handle so the store can close it. This is the only way out.
    pub fn release(mut self) -> Handle {
        self.handle.take().expect("a StoreSource holds its handle until exactly one `release`")
    }

    pub fn id(&self) -> ObjectId {
        self.handle().id()
    }

    pub fn revision(&self) -> Revision {
        self.handle().revision()
    }

    fn handle(&self) -> &Handle {
        self.handle.as_ref().expect("a released StoreSource is consumed and cannot be read")
    }
}

impl<D: BlockDevice> Drop for StoreSource<'_, D> {
    fn drop(&mut self) {
        // A panic is already unwinding through here. Asserting now would be a second panic during
        // unwind, which aborts the process and takes the original failure's message with it. The leak
        // is the lesser problem.
        #[cfg(any(test, feature = "std"))]
        if std::thread::panicking() {
            return;
        }
        // Not a `panic!`: on the device this compiles out, and a hard fault is never the right answer
        // to a leaked row. On the host it fails the test that leaked.
        debug_assert!(
            self.handle.is_none(),
            "a StoreSource was dropped without `release`; its row and extents leak until the next mount",
        );
    }
}

impl<D: BlockDevice> ByteSource for StoreSource<'_, D> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        // Range first, medium second: a caller asking past the end is a bad offset, and only a
        // genuine media failure is `Io`. Catalog loaders distinguish them so a transient read
        // preserves the last validated snapshot while a definitively short object can be omitted.
        let end = offset.checked_add(buf.len() as u64).ok_or(Error::BadOffset)?;
        if end > self.len {
            return Err(Error::BadOffset);
        }
        let handle = self.handle();
        let mut done = 0usize;
        while done < buf.len() {
            // The store returns short only at end of payload, and the range check above proved we are
            // not near it, so a zero-length return is an I/O-class fault. Loop anyway: the seam's
            // contract is "bytes read".
            match self.store.read(handle, offset + done as u64, &mut buf[done..]) {
                Ok(0) => return Err(Error::Io),
                Ok(n) => done += n,
                Err(_) => return Err(Error::Io),
            }
        }
        Ok(())
    }

    fn len(&self) -> u64 {
        self.len
    }
}

impl<D: BlockDevice> FlatStore<D> {
    /// Open `id` and wrap it as a [`ByteSource`]. The caller owns the pairing: finish with
    /// [`StoreSource::release`] and [`close`](FlatStore::close). `revision` of `None` takes the head.
    pub fn source(&self, id: ObjectId, revision: Option<Revision>) -> Result<StoreSource<'_, D>, StoreError> {
        let handle = Store::open(self, id, revision)?;
        // `open` just wrote the row, so it resolves. This tail keeps the function total — a typed
        // error rather than an `expect`, which on the device would be a hard fault for something that
        // cannot happen. The `debug_assert` keeps the dropped handle honest.
        StoreSource::over(self, handle).map_err(|_returned| {
            debug_assert!(false, "the row `open` just wrote did not resolve; its handle is being dropped");
            StoreError::Invalid
        })
    }

    /// Open `id`, run `body` against it, and close it. This is the shape for everything that is not a
    /// session-long mount.
    ///
    /// It does not close on a panic: `body` runs between the open and the close with no unwind guard,
    /// so a panic inside it leaks the row until the next mount. The device does not unwind, and a host
    /// that panics is a test that has already failed.
    pub fn with_source<R>(
        &self,
        id: ObjectId,
        revision: Option<Revision>,
        body: impl FnOnce(&StoreSource<'_, D>) -> R,
    ) -> Result<R, StoreError> {
        let handle = Store::open(self, id, revision)?;
        // Unreachable for the same reason as in `source`, and dropping the returned handle is the
        // same acknowledged exception.
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

    use obc_crc::Crc32;

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
                added_at_utc: 0,
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
        assert_eq!(source.len(), LEN as u64);
        assert_eq!(source.id(), ids[0]);
        assert_eq!(source.revision(), Revision(1));

        for (offset, len) in [(0u64, LEN), (0, 1), (511, 2), (1_000, 512), (LEN as u64 - 1, 1)] {
            let mut through_seam = vec::from_elem(0u8, len);
            source.read_at(offset, &mut through_seam).expect("inside the object");
            let mut direct = vec::from_elem(0u8, len);
            let got = store.read(source.handle(), offset, &mut direct).expect("the store reads");
            assert_eq!(got, len, "the store filled the window");
            assert_eq!(through_seam, direct, "the adapter changed bytes at ({offset}, {len})");
            assert_eq!(through_seam, payload()[offset as usize..offset as usize + len], "and both differ from truth");
        }

        let handle = source.release();
        store.close(handle);
    }

    #[test]
    fn an_unpublished_stream_can_patch_its_header_and_hash_the_final_bytes() {
        let disk = SparseDisk::blank(EXTENT_AREA + Geometry::DEFAULT.extent_blocks() * 4, 3);
        let store = FlatStore::initialize(&disk, STORE).expect("an expressible card");
        let mut expected = payload();
        let mut allocation = store.allocate((LEN + 700) as u64).expect("the oversized reservation fits");
        store.write(&mut allocation, &expected).expect("the streamed payload fits");

        let header = [0xA5; 128];
        expected[..header.len()].copy_from_slice(&header);
        store.patch_allocation(&allocation, 0, &header).expect("an appended header remains patchable");
        assert_eq!(
            store.allocation_crc(&allocation).expect("the live allocation hashes"),
            Crc32::checksum(&expected),
            "the checksum covers the patched bytes and the unflushed tail",
        );

        let id = store.next_object_id();
        let meta = EntryMeta {
            added_at_utc: 0,
            id,
            revision: Revision(1),
            kind: ObjectKind::Route,
            flags: EntryFlags::NONE,
            payload_len: LEN as u64,
            payload_crc: Crc32::checksum(&expected),
            name: DisplayName::new("computed").expect("a short name"),
        };
        store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]).expect("the route publishes");
        store
            .with_source(id, None, |source| {
                let mut actual = vec![0; LEN];
                source.read_at(0, &mut actual).expect("the published payload reopens");
                assert_eq!(actual, expected);
            })
            .expect("the route source opens");
    }

    /// Past the end is a caller error, not a media one — including a window that starts inside and
    /// straddles the end, which a length check on `offset` alone would let through.
    #[test]
    fn reads_past_the_end_are_refused_as_bad_offsets() {
        let (disk, ids) = fixture(1);
        let store = FlatStore::mount(&disk);
        store
            .with_source(ids[0], None, |source| {
                let mut buf = [0u8; 16];
                assert_eq!(source.read_at(LEN as u64, &mut buf).unwrap_err(), Error::BadOffset, "starting at the end");
                assert_eq!(
                    source.read_at(LEN as u64 - 8, &mut buf).unwrap_err(),
                    Error::BadOffset,
                    "straddling the end"
                );
                assert_eq!(
                    source.read_at(u64::MAX, &mut buf).unwrap_err(),
                    Error::BadOffset,
                    "an offset that wraps — now at the top of the *seam's* width, not a u32's"
                );
                source.read_at(LEN as u64 - 16, &mut buf).expect("the last full window is fine");
            })
            .expect("the object opens");
    }

    /// `with_source` must hand its row back. Proved by exhaustion: walk more distinct objects than
    /// the table has rows, one scope at a time. Distinct matters, because repeating one object would
    /// share a row by refcount and pass whether or not the close happened.
    #[test]
    fn with_source_returns_its_row_to_the_table() {
        let objects = MAX_OPEN_OBJECTS + 4;
        let (disk, ids) = fixture(objects);
        let store = FlatStore::mount(&disk);

        for (index, id) in ids.iter().enumerate() {
            let len = store.with_source(*id, None, |source| source.len()).unwrap_or_else(|error| {
                panic!("object {index} of {objects} could not open ({error:?}) — a scope leaked its row")
            });
            assert_eq!(len, LEN as u64);
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
        store.with_source(ids[0], None, |source| assert_eq!(source.len(), LEN as u64)).expect("the local object opens");
    }

    /// A real 4 GiB object is not constructible in a test, so this pins the arithmetic where it
    /// lives: the reported length is the whole payload length, not an addressable prefix of it.
    #[test]
    fn a_payload_past_the_old_u32_ceiling_reports_its_whole_length() {
        let (disk, ids) = fixture(1);
        let store = FlatStore::mount(&disk);
        let handle = Store::open(&store, ids[0], None).expect("the object opens");

        let big = u64::from(u32::MAX) + 4_096;
        let huge = StoreSource::with_len(&store, handle, big);
        assert_eq!(huge.len(), big, "the length is the payload's, with nothing clamping it");
        // Inside the reported length but far past the bytes that exist: the range check passes and
        // the store's short read is what refuses it.
        let mut buf = [0u8; 4];
        assert_eq!(huge.read_at(big - 4, &mut buf).unwrap_err(), Error::Io, "past the payload is not silent");
        assert_eq!(
            huge.read_at(big, &mut buf).unwrap_err(),
            Error::BadOffset,
            "and past the reported length is still a caller error"
        );

        let handle = huge.release();
        store.close(handle);
    }

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
        assert_eq!(live.len(), LEN as u64);
        assert_eq!(store.free_extents(), free_before, "a spent refcount frees no extent");

        // And the last close is the one that does the work.
        let handle = live.release();
        store.close(handle);
        assert_eq!(store.free_extents(), free_before, "the entry still names them, so nothing moved");
        // The row really did come back. That a row still counted as held would also reopen, so this
        // is checked by exhaustion in `with_source_returns_its_row_to_the_table` instead.
        store.with_source(ids[0], None, |source| assert_eq!(source.len(), LEN as u64)).expect("it opens again");
    }

    /// A later joiner may not shorten what an earlier reader is already serving: `source`, then an
    /// amend that trims the entry, then a second `open` on the same key. The read past the new end is
    /// the point — it is inside the first reader's revision, and a handle keeps reading the revision
    /// it resolved.
    #[test]
    fn a_second_open_cannot_shorten_a_reader_already_serving_the_row() {
        let (disk, ids) = fixture(1);
        let store = FlatStore::mount(&disk);

        let live = store.source(ids[0], None).expect("the object opens");
        assert_eq!(live.len(), LEN as u64);

        // An amend that trims the entry to a third of its length, beside the live source. `Amend`
        // keeps the extents the entry already holds and rewrites only the metadata.
        const SHORT: u64 = 1_000;
        let trimmed = EntryMeta {
            added_at_utc: 0,
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
        assert_eq!(live.len(), LEN as u64, "the source's own length is captured at open and cannot move");
        let mut tail = [0u8; 64];
        live.read_at(LEN as u64 - 64, &mut tail).expect("past the amended end is still inside the resolved revision");
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

    /// A source and a writer coexisting: the board's shape, a mounted shard read while an upload
    /// commits.
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
            added_at_utc: 0,
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

    /// A listing that outlives its catalog stops, and says so. Two commits later the copy it is
    /// walking has been rewritten underneath its cursor, and serving those bytes with `entries_ok()`
    /// still `true` would splice two catalogs together and call the result complete.
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

    /// A full hold table is `Busy`, not `Invalid`: `invalidRequest` means this request is wrong and
    /// will be wrong next time, while every row being taken is a fact about who else is reading right
    /// now. `MAX_OPEN_OBJECTS + 1` distinct objects, because repeating one would share a row.
    #[test]
    fn a_full_hold_table_is_refused_as_busy_rather_than_invalid() {
        let (disk, ids) = fixture(MAX_OPEN_OBJECTS + 1);
        let store = FlatStore::mount(&disk);

        let mut open: vec::Vec<crate::flat::store::Handle> = vec::Vec::new();
        for (index, id) in ids.iter().take(MAX_OPEN_OBJECTS).enumerate() {
            open.push(Store::open(&store, *id, None).unwrap_or_else(|error| {
                panic!("row {index} of {MAX_OPEN_OBJECTS} should still be free, got {error:?}")
            }));
        }
        assert_eq!(
            Store::open(&store, ids[MAX_OPEN_OBJECTS], None).unwrap_err(),
            StoreError::Busy,
            "the row after the last one is a transient refusal, not a malformed request"
        );

        // And it really was transient: give one row back and the same open succeeds.
        store.close(open.pop().expect("a row to return"));
        let handle = Store::open(&store, ids[MAX_OPEN_OBJECTS], None).expect("the freed row serves the next caller");
        store.close(handle);
        for handle in open {
            store.close(handle);
        }
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
