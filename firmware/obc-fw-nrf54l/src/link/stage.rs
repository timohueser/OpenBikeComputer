//! The upload **staging buffer** — the seam between a transport's chunk size and the card's.
//!
//! Both data planes used to hand every arriving chunk straight to
//! [`ObjectStore::upload_append`], so the card saw a write per chunk: 512 B on USB, 244 B on BLE.
//! `VolumeManager::write` turns each of those into one single-block device write — a CMD24, the
//! card's **whole internal program cycle**, and a CMD13 status query. That cost is card-side, so it
//! survived the transport pivot untouched: it was 2458 µs per 512 B over the retired SPI path
//! (measured 2026-07-30) and it is the same program cycle over sEMMC, where a single block costs
//! 430 µs on the wire and the program dominates by an order of magnitude. Handing the card an
//! aligned multi-block run instead amortises one program cycle over the whole run, because our
//! embedded-sdmmc fork batches a contiguous run into one CMD25 (see the `[patch.crates-io]` note in
//! `Cargo.toml`) — which is exactly what turns the sEMMC host's 8.2 MB/s of raw write bandwidth
//! into upload throughput rather than one stall per block.
//!
//! So: pile chunks up in RAM and append a batch at a time. The fork does the rest.
//!
//! **Alignment is the whole trick.** A flush that starts mid-block cannot batch its first or last
//! block — `VolumeManager` falls back to a read-modify-write for the partial ends, and on a
//! 244-byte transport *every* write was partial. [`Stage`] therefore flushes on the store's write
//! offset reaching a multiple of the half length, not on the buffer merely being full: the first
//! flush of a map is [`MAGIC_LEN`](obc_ble::MAGIC_LEN) bytes short precisely so that every flush
//! after it is block- and cluster-aligned.
//!
//! # Two halves (#1158 follow-up), and what they honestly buy
//!
//! The arm is [`STAGE_LEN`](crate::usb::STAGE_LEN) = two [`STAGE_HALF_LEN`](crate::usb::STAGE_HALF_LEN)
//! halves. The transport always fills the half the card is *not* holding, so a full half is handed
//! over and the very next packet lands in the other one rather than waiting behind an append.
//!
//! The halves now provide real hardware overlap. A full half is passed through FAT, adjacent
//! cluster writes are coalesced, and `Semmc::start_write_blocks` starts the FLPR DMA without
//! waiting for the card's data/program phase. USB then fills the other half. Before a half can be
//! reused, the next append joins the previous DMA and starts the newly-filled half, preserving
//! byte order and the DMA buffer lifetime. The M33 never keeps a borrowed arena slice across an
//! await; only the FLPR's address remains live under that explicit handoff rule.
//!
//! USB uses the controller's buffer-DMA mode too, so neither side spends the upload copying 512 B
//! packets in an interrupt. The remaining ceiling is the card itself: one aligned 128-block CMD25
//! per normal half, with ACMD23 offered as a best-effort pre-erase hint.
//!
//! # Who uses this
//!
//! **Only [`usb::data_plane`](crate::usb::data_plane) today.** The BLE plane still hands each
//! arriving SDU straight to `ObjectStore::upload_append`, because the arena arm it would stage into
//! is granted against a *transfer screen* that only a cable upload raises, and a 244-byte CoC moving
//! a route is not what the batching was built for.
//!
//! It lives in `link/` rather than in the USB plane all the same, and the reason is unchanged: the
//! two data planes are deliberate line-for-line twins (see the module doc on either), so the moment
//! BLE wants batching it takes this type rather than growing a second copy of it. What both planes
//! *do* share today is the reservation one level down — `ObjectStore::upload_reserve`, which needs
//! no staging buffer.

use core::cell::RefCell;

use crate::object_store::ObjectStore;
use crate::SharedStoreMutex;

/// One upload's staged bytes: fill from the transport, drain to the store in aligned batches.
///
/// Holds no lock between flushes — the store mutex is taken for the append and released, so the
/// ride loop's map render still interleaves. The flush is longer than the old per-chunk append
/// (two clusters against 512 B), which is still frame-scale over sEMMC.
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
    /// Which half the transport is filling — `0` or `1`. The other half is the card's.
    fill: usize,
    /// Bytes in the filling half.
    used: usize,
    /// Bytes parked in the **other** half, waiting for the card. `0` = that half is free.
    ///
    /// At most one half is ever parked: a half is handed over only when the fill half is full, and
    /// the handover drains the previous parked half first. That is what keeps the store's write
    /// order the byte order.
    parked: usize,
    /// Bytes this stage has **appended** — the store's write offset *relative to where staging
    /// began*, seeded by [`Stage::new`] so the alignment arithmetic knows about a map's magic
    /// placeholder. Deliberately excludes [`parked`](Stage::parked), which is why
    /// [`target`](Stage::target) adds it back.
    offset: usize,
    /// Store offset at the last explicit executor yield. USB reads naturally yield whenever the
    /// controller has no burst ready; this is the bounded backstop for a permanently-ready host.
    yielded_at: usize,
}

/// One half's capacity — what a single flush hands the card, and what the alignment arithmetic is
/// modulo. See [`STAGE_HALF_LEN`](crate::usb::STAGE_HALF_LEN) for the block/cluster sizing.
const CAP: usize = crate::usb::STAGE_HALF_LEN;

/// Guaranteed cooperative cadence under a saturating host. At the measured card rate this is
/// about 30 ms: responsive enough for the transfer card and far inside the watchdog budget, while
/// no longer scheduling a whole ride-loop pass after every FAT cluster.
const YIELD_BYTES: usize = 4 * CAP;

impl Stage {
    /// A stage for a file whose write offset is already `store_offset` bytes in. `staged` is the
    /// ride loop's answer to [`request_stage`](crate::usb::request_stage).
    ///
    /// `store_offset` is 0 for every object that streams into an empty `UPLOAD.TMP`, and
    /// [`MAGIC_LEN`](obc_ble::MAGIC_LEN) for a **map**: `map_upload_begin` has already written the
    /// zero magic placeholder into `MP{id}.OBM`, so the payload's first staged byte lands at file
    /// offset 4 and an unseeded stage would misalign every flush of the transfer.
    pub(crate) fn new(staged: bool, store_offset: usize) -> Self {
        Stage { staged, fill: 0, used: 0, parked: 0, offset: store_offset, yielded_at: store_offset }
    }

    /// Where in the arm half `which` starts.
    fn base(which: usize) -> usize {
        which * CAP
    }

    /// How many bytes to stage into the filling half before handing it over: whatever lands the
    /// store's write offset on a multiple of [`CAP`]. Constant at [`CAP`] once the first handover
    /// has happened; `0` when unstaged, which [`push`](Stage::push) reads as "append everything
    /// immediately".
    ///
    /// The offset the filling half will *land* at is [`offset`](Stage::offset) plus whatever is
    /// still [`parked`](Stage::parked) ahead of it — missing that term is how a double buffer
    /// silently un-aligns every flush after the first.
    fn target(&self) -> usize {
        if !self.staged {
            return 0;
        }
        CAP - ((self.offset + self.parked) % CAP)
    }

    /// Stage `bytes`, handing a half to the card whenever the staged run reaches its target.
    /// `false` = the card refused an append; the caller discards the partial upload and answers
    /// `error`.
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
            let target = self.target();
            let take = (target - self.used).min(bytes.len());
            let at = Self::base(self.fill) + self.used;
            let copied = crate::arena::with_usb_stage(|buf| {
                buf[at..at + take].copy_from_slice(&bytes[..take]);
            });
            if copied.is_none() {
                defmt::warn!("stage: the arena's staging arm was revoked mid-transfer — discarding the upload");
                return false;
            }
            self.used += take;
            bytes = &bytes[take..];
            if self.used == target {
                // This half is full. Send the older half first, then hand this half to the store
                // immediately. The fast map writer starts its FLPR DMA and returns; its next
                // append joins that DMA before starting this half's successor. In particular, do
                // not leave the just-filled half merely parked while swapping back to the older
                // half: the older half may still be the DMA source and must not be overwritten.
                if !self.drain(store, shared).await {
                    return false;
                }
                self.parked = self.used;
                self.used = 0;
                self.fill ^= 1;
                if !self.drain(store, shared).await {
                    return false;
                }
            }
        }
        true
    }

    /// Append whatever is staged, in order, whether or not it reached a target — the transfer's last
    /// flush, where the tail is short by definition. Idempotent on an empty stage.
    pub(crate) async fn flush(&mut self, store: &RefCell<ObjectStore>, shared: &SharedStoreMutex) -> bool {
        if !self.staged {
            // Every chunk already went straight to the card; there is nothing held back.
            return true;
        }
        // The parked half is older than the filling one, so it goes first — and it must go even when
        // the fill half is empty, which is exactly what a transfer ending on a half boundary looks
        // like.
        if !self.drain(store, shared).await {
            return false;
        }
        if self.used != 0 {
            let (at, len) = (Self::base(self.fill), self.used);
            if !self.append_arm(at, len, store, shared).await {
                return false;
            }
            self.used = 0;
        }
        self.sync(store, shared).await
    }

    /// Hand the parked half to the card, if there is one. The whole of the double buffer's ordering
    /// rule lives in the two call sites of this: drain **before** parking a new half, and drain
    /// **before** the tail.
    async fn drain(&mut self, store: &RefCell<ObjectStore>, shared: &SharedStoreMutex) -> bool {
        if self.parked == 0 {
            return true;
        }
        // The parked half is the one the transport is *not* filling.
        let (at, len) = (Self::base(self.fill ^ 1), self.parked);
        if !self.append_arm(at, len, store, shared).await {
            return false;
        }
        self.parked = 0;
        true
    }

    /// One card append out of the arena arm: `len` bytes at arm offset `at`.
    ///
    /// Periodically ends on an explicit `yield_now`, and this is **liveness, not politeness**: a saturating
    /// host keeps a completed packet waiting at every `ep.read`, so the upload task's own awaits
    /// are always instantly ready and it never returns `Pending` on its own — first observed as a
    /// map upload that froze the whole UI (an 18.7 s frame push on glass, 2026-07-30). The yield
    /// is the one point where the ride loop is guaranteed a turn, once per [`YIELD_BYTES`], which is what
    /// makes the data planes' "the map render interleaves between chunks" doc claim actually true
    /// under load. It is also what keeps #1014's WDT fix honest: the feed rides the ride loop's
    /// pass, so a flush that never yielded would starve it.
    async fn append_arm(
        &mut self,
        at: usize,
        len: usize,
        store: &RefCell<ObjectStore>,
        shared: &SharedStoreMutex,
    ) -> bool {
        let appended = {
            // The store lock is taken **before** the arena access, so nothing borrowed from the
            // arena is live across the `.await`.
            let mut guard = shared.lock().await;
            crate::arena::with_usb_stage(|buf| store.borrow_mut().upload_append(&mut guard, &buf[at..at + len]))
                .unwrap_or(false)
        };
        let should_yield = if appended {
            self.offset += len;
            self.offset.saturating_sub(self.yielded_at) >= YIELD_BYTES
        } else {
            true
        };
        if should_yield {
            self.yielded_at = self.offset;
            embassy_futures::yield_now().await;
        }
        appended
    }

    /// Join the final FLPR write before the caller validates or exposes the file's magic.
    async fn sync(&mut self, store: &RefCell<ObjectStore>, shared: &SharedStoreMutex) -> bool {
        let synced = {
            let mut guard = shared.lock().await;
            store.borrow_mut().upload_sync(&mut guard)
        };
        embassy_futures::yield_now().await;
        synced
    }

    /// Append `bytes` straight to the card — the unstaged path's whole implementation, and the
    /// bookkeeping half of [`append_arm`](Stage::append_arm) without the buffer. Keeps the same
    /// trailing `yield_now` for the same liveness reason.
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
