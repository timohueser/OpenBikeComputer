//! The upload **staging buffer** — the seam between a transport's chunk size and the card's.
//!
//! Both data planes used to hand every arriving chunk straight to
//! [`ObjectStore::upload_append`], so the card saw a write per chunk: 512 B on USB, 244 B on BLE.
//! `VolumeManager::write` turns each of those into one single-block device write, which on SD over
//! SPI is a CMD24, the card's whole internal program cycle and a CMD13 status query — measured at
//! 2458 µs per 512 B on the shipping card (`sd_bench`, `wr-fat b1`). Handing the same card 32
//! blocks at a time instead costs 517 µs per block, because our embedded-sdmmc fork batches a
//! contiguous run into one ACMD23 + CMD25 (see the `[patch.crates-io]` note in `Cargo.toml`).
//!
//! So: pile chunks up in RAM and append a batch at a time. The fork does the rest.
//!
//! **Alignment is the whole trick.** A flush that starts mid-block cannot batch its first or last
//! block — `VolumeManager` falls back to a read-modify-write for the partial ends, and on a
//! 244-byte transport *every* write was partial. [`Stage`] therefore flushes on the store's write
//! offset reaching a multiple of the buffer length, not on the buffer merely being full: the first
//! flush of a map is [`MAGIC_LEN`](obc_ble::MAGIC_LEN) bytes short precisely so that every flush
//! after it is block- and cluster-aligned.
//!
//! Lives here rather than in either plane because `usb::data_plane` and `ble::data_plane` are
//! deliberate line-for-line twins (see the module doc on either), and a staging buffer copied into
//! both is exactly the kind of duplication this module exists to prevent.

use core::cell::RefCell;

use crate::object_store::ObjectStore;
use crate::SharedStoreMutex;

/// One upload's staged bytes: fill from the transport, drain to the store in aligned batches.
///
/// Holds no lock between flushes — the store mutex is taken for the append and released, so the
/// ride loop's map render still interleaves. The flush is longer than the old per-chunk append
/// (~17 ms for 16 KB against ~2.5 ms for 512 B), which is still frame-scale.
pub(crate) struct Stage<'a> {
    buf: &'a mut [u8],
    /// Bytes staged and not yet appended.
    used: usize,
    /// Bytes this stage has appended — the store's write offset *relative to where staging began*,
    /// seeded by [`Stage::new`] so the alignment arithmetic knows about a map's magic placeholder.
    offset: usize,
}

impl<'a> Stage<'a> {
    /// A stage over `buf`, for a file whose write offset is already `store_offset` bytes in.
    ///
    /// `store_offset` is 0 for every object that streams into an empty `UPLOAD.TMP`, and
    /// [`MAGIC_LEN`](obc_ble::MAGIC_LEN) for a **map**: `map_upload_begin` has already written the
    /// zero magic placeholder into `MP{id}.OBM`, so the payload's first staged byte lands at file
    /// offset 4 and an unseeded stage would misalign every flush of the transfer.
    pub(crate) fn new(buf: &'a mut [u8], store_offset: usize) -> Self {
        Stage { buf, used: 0, offset: store_offset }
    }

    /// How many bytes to stage before the next flush: whatever lands the store's write offset on a
    /// multiple of the buffer length. Constant at `buf.len()` after the first flush.
    fn target(&self) -> usize {
        let n = self.buf.len();
        n - (self.offset % n)
    }

    /// Stage `bytes`, appending whenever the staged run reaches its target. `false` = the card
    /// refused an append; the caller discards the partial upload and answers `error`.
    ///
    /// An empty slice is not a special case — it stages nothing and flushes nothing.
    pub(crate) async fn push(
        &mut self,
        mut bytes: &[u8],
        store: &RefCell<ObjectStore>,
        shared: &SharedStoreMutex,
    ) -> bool {
        while !bytes.is_empty() {
            let take = (self.target() - self.used).min(bytes.len());
            self.buf[self.used..self.used + take].copy_from_slice(&bytes[..take]);
            self.used += take;
            bytes = &bytes[take..];
            if self.used == self.target() && !self.flush(store, shared).await {
                return false;
            }
        }
        true
    }

    /// Append whatever is staged, whether or not it reached a target — the transfer's last flush,
    /// where the tail is short by definition. Idempotent on an empty stage.
    ///
    /// Ends on an explicit `yield_now`, and this is **liveness, not politeness**: a saturating
    /// host keeps a completed packet waiting at every `ep.read`, so the upload task's own awaits
    /// are always instantly ready and it never returns `Pending` on its own — first observed as a
    /// map upload that froze the whole UI (an 18.7 s frame push on glass, 2026-07-30). The yield
    /// is the one point where the ride loop is guaranteed a turn, once per flush (~20 ms of work),
    /// which is what makes the data planes' "the map render interleaves between chunks" doc claim
    /// actually true under load.
    pub(crate) async fn flush(&mut self, store: &RefCell<ObjectStore>, shared: &SharedStoreMutex) -> bool {
        if self.used == 0 {
            return true;
        }
        let appended = {
            let mut guard = shared.lock().await;
            store.borrow_mut().upload_append(&mut guard, &self.buf[..self.used])
        };
        if appended {
            self.offset += self.used;
            self.used = 0;
        }
        embassy_futures::yield_now().await;
        appended
    }
}
