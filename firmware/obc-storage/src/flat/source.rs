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
//! - **Short-lived** — [`FlatStore::with_source`]. Opens, runs the body, closes on every path out.
//!   Nothing to forget, and the borrow checker never sees a source outlive its close.
//! - **Session-long** — [`StoreSource`] + [`release`](StoreSource::release), for the eleven shards
//!   and the terrain sidecar a mounted set holds from boot to unmount, where a scope is not
//!   available (they live in `.bss`, across `await`s). Here the pairing is enforced twice over: the
//!   source *owns* its handle and will only give it back through `release`, and dropping one that
//!   still holds a handle trips a `debug_assert` — so a leak fails loudly in every test and every
//!   host build rather than showing up as a card that will not mount a set after the third try.
//!
//! There is a third guarantee that costs nothing and is worth naming: because a live `StoreSource`
//! holds `&FlatStore` and `close` wants `&mut`, **no object can be closed while any source is
//! alive**. Tearing down a mounted set is therefore all-or-nothing at the type level.
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
    /// Wrap an already-open `handle`. The handle must be `store`'s.
    ///
    /// Prefer [`FlatStore::source`], which opens and wraps in one step. This exists for the caller
    /// that already holds a handle — the board's mount path, which opens every shard before it has
    /// anywhere to put the sources.
    pub fn over(store: &'a FlatStore<D>, handle: Handle) -> Self {
        let len = store.handle_len(&handle).unwrap_or(0).min(u32::MAX as u64) as u32;
        StoreSource { store, handle: Some(handle), len }
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
        Ok(StoreSource::over(self, handle))
    }

    /// Open `id`, run `body` against it, and close it — on every path out, including an early return
    /// or a panic inside `body`, because the close is sequenced after the source is consumed.
    ///
    /// This is the shape for everything that is not a session-long mount: a menu reading a header, a
    /// `STATUS` resolving an object, a test. See the module docs for the other shape.
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
        let source = StoreSource::over(&*self, handle);
        let out = body(&source);
        let handle = source.release();
        self.close(handle);
        Ok(out)
    }
}
