//! The upload **staging buffer** — the seam between a transport's chunk size and the card's.
//!
//! Both data planes used to hand every arriving chunk straight to
//! [`ObjectStore::upload_append`], so the card saw a write per chunk: 512 B on USB, 244 B on BLE.
//! `VolumeManager::write` turns each of those into one single-block device write — a CMD24, the
//! card's **whole internal program cycle**, and a CMD13 status query. That cost is card-side, so it
//! survived the transport pivot untouched: it was 2458 µs per 512 B over the retired SPI path
//! (measured 2026-07-30) and it is the same program cycle over sEMMC, where a single block costs
//! 430 µs on the wire and the program dominates by an order of magnitude. Handing the card 32
//! blocks at a time instead amortises one program cycle over the whole run, because our
//! embedded-sdmmc fork batches a contiguous run into one CMD25 (see the `[patch.crates-io]` note in
//! `Cargo.toml`) — which is exactly what turns the sEMMC host's 8.2 MB/s of raw write bandwidth
//! into upload throughput rather than one stall per block.
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
///
/// **The buffer is not this type's** (issue #1146, P2). It is the scratch arena's USB arm, owned by
/// the ride loop for the duration of the transfer and reached — synchronously, one access at a time
/// — through [`arena::with_usb_stage`](crate::arena::with_usb_stage). Holding a `&'static mut` into
/// the arena across this type's `.await`s would have made the arm un-reclaimable on an unplug (the
/// transfer future is parked, not dropped, while the cable is out), so the reference is re-derived
/// per access instead and a revoked arm simply fails the append.
pub(crate) struct Stage {
    /// Whether the arena's staging arm was granted for this transfer. `false` = **unstaged**: every
    /// chunk goes straight to the card, the pre-#1039 path at ~0.20 MB/s. Slower, never wrong — and
    /// it is what a small object (a route, a trip, a firmware image) gets, since the arm's
    /// precondition is a transfer screen only a map upload raises.
    staged: bool,
    /// Bytes staged and not yet appended.
    used: usize,
    /// Bytes this stage has appended — the store's write offset *relative to where staging began*,
    /// seeded by [`Stage::new`] so the alignment arithmetic knows about a map's magic placeholder.
    offset: usize,
}

/// The staging arm's capacity — the whole arm, since the arm exists for exactly this.
const CAP: usize = crate::usb::STAGE_LEN;

impl Stage {
    /// A stage for a file whose write offset is already `store_offset` bytes in. `staged` is the
    /// ride loop's answer to [`request_stage`](crate::usb::request_stage).
    ///
    /// `store_offset` is 0 for every object that streams into an empty `UPLOAD.TMP`, and
    /// [`MAGIC_LEN`](obc_ble::MAGIC_LEN) for a **map**: `map_upload_begin` has already written the
    /// zero magic placeholder into `MP{id}.OBM`, so the payload's first staged byte lands at file
    /// offset 4 and an unseeded stage would misalign every flush of the transfer.
    pub(crate) fn new(staged: bool, store_offset: usize) -> Self {
        Stage { staged, used: 0, offset: store_offset }
    }

    /// How many bytes to stage before the next flush: whatever lands the store's write offset on a
    /// multiple of the buffer length. Constant at [`CAP`] after the first flush; `0` when unstaged,
    /// which [`push`](Stage::push) reads as "append everything immediately".
    fn target(&self) -> usize {
        if !self.staged {
            return 0;
        }
        CAP - (self.offset % CAP)
    }

    /// Stage `bytes`, appending whenever the staged run reaches its target. `false` = the card
    /// refused an append; the caller discards the partial upload and answers `error`.
    ///
    /// An empty slice is not a special case — it stages nothing and flushes nothing.
    ///
    /// A revoked arena arm (the ride loop took the staging arm back mid-transfer — an unplug) also
    /// returns `false`: to the caller it is indistinguishable from a card that refused the append,
    /// and the response is the same one, discarding the partial upload.
    pub(crate) async fn push(
        &mut self,
        mut bytes: &[u8],
        store: &RefCell<ObjectStore>,
        shared: &SharedStoreMutex,
    ) -> bool {
        // Unstaged: no arena arm, so there is nothing to batch into — hand each chunk to the card as
        // it lands, exactly as both data planes did before staging existed.
        if !self.staged {
            return self.append(bytes, store, shared).await;
        }
        while !bytes.is_empty() {
            let take = (self.target() - self.used).min(bytes.len());
            let copied = crate::arena::with_usb_stage(|buf| {
                buf[self.used..self.used + take].copy_from_slice(&bytes[..take]);
            });
            if copied.is_none() {
                defmt::warn!("stage: the arena's staging arm was revoked mid-transfer — discarding the upload");
                return false;
            }
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
        let used = self.used;
        let appended = {
            // The store lock is taken **before** the arena access, so nothing borrowed from the
            // arena is live across the `.await`.
            let mut guard = shared.lock().await;
            crate::arena::with_usb_stage(|buf| store.borrow_mut().upload_append(&mut guard, &buf[..used]))
                .unwrap_or(false)
        };
        if appended {
            self.offset += self.used;
            self.used = 0;
        }
        embassy_futures::yield_now().await;
        appended
    }

    /// Append `bytes` straight to the card — the unstaged path's whole implementation, and the
    /// bookkeeping half of [`flush`](Stage::flush) without the buffer. Keeps the same trailing
    /// `yield_now` for the same liveness reason: a saturating host never lets the transfer task
    /// return `Pending` on its own, so this is the one point where the ride loop is guaranteed a
    /// turn.
    async fn append(&mut self, bytes: &[u8], store: &RefCell<ObjectStore>, shared: &SharedStoreMutex) -> bool {
        if bytes.is_empty() {
            return true;
        }
        let appended = {
            let mut guard = shared.lock().await;
            store.borrow_mut().upload_append(&mut guard, bytes)
        };
        if appended {
            self.offset += bytes.len();
        }
        embassy_futures::yield_now().await;
        appended
    }
}
