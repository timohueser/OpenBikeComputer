//! The bounded handoff of committed route and trip uploads, from the cable to the ride loop.
//!
//! A protocol-v4 commit lands on the board's USB plane; the app learns of it only after the
//! catalog rescan the same commit caused. The facts wait in between, and what waits is ordered:
//! the ride loop replays them in commit order so that a same-id active route replacement
//! invalidates geometry-derived state. The interrupt-safe container is device-only, in the board
//! crate's `flat_store.rs`. The bookkeeping that decides what the queue keeps lives here, because
//! the board crate has no test harness in CI.
//!
//! Three rules make the queue bounded without lying to the rider: a repeated commit for the same
//! object coalesces onto its newest value, a full queue drops its oldest distinct fact, and a drop
//! raises [`loss`](UploadFacts::take_loss), which the ride loop answers with a conservative
//! active-route refresh. The capacity is a complete UI catalog's worth of identities, so a drop
//! needs more distinct objects than the menus can hold at once.

use crate::route::MAX_ROUTES;
use crate::trip::MAX_TRIPS;
use heapless::Deque;

/// One successful protocol-v4 route/trip upload waiting for the app's post-rescan event seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogUpload {
    id: [u8; 8],
    flags: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogUploadKind {
    Route,
    Trip,
}

impl CatalogUpload {
    pub fn new(kind: CatalogUploadKind, id: u64, replaced: bool) -> Self {
        let flags = (kind == CatalogUploadKind::Trip) as u8 | ((replaced as u8) << 1);
        Self { id: id.to_le_bytes(), flags }
    }

    pub const fn kind(self) -> CatalogUploadKind {
        if self.flags & 1 == 0 {
            CatalogUploadKind::Route
        } else {
            CatalogUploadKind::Trip
        }
    }

    pub const fn id(self) -> u64 {
        u64::from_le_bytes(self.id)
    }

    pub const fn replaced(self) -> bool {
        self.flags & 2 != 0
    }

    /// Same kind and same id. `replaced` is the value, not the identity, so it is excluded.
    fn same_object(self, other: Self) -> bool {
        (self.flags & 1) == (other.flags & 1) && self.id == other.id
    }
}

/// The unaligned `id` keeps the fact at 9 bytes: the queue holds a whole catalog of them.
const _: () = assert!(core::mem::size_of::<CatalogUpload>() == 9);

/// A complete UI catalog's worth of upload facts. Bounding this to the menus' combined identity
/// capacity keeps the resident cost explicit.
const UPLOAD_EVENTS_CAP: usize = MAX_ROUTES + MAX_TRIPS;

/// The queued facts and the one bit that says the queue could not hold them all.
#[derive(Debug)]
pub struct UploadFacts {
    queue: Deque<CatalogUpload, UPLOAD_EVENTS_CAP>,
    loss: bool,
}

impl Default for UploadFacts {
    fn default() -> Self {
        Self::new()
    }
}

impl UploadFacts {
    pub const fn new() -> UploadFacts {
        UploadFacts { queue: Deque::new(), loss: false }
    }

    /// Insert the latest fact for one object at the back of the queue. Repeated replaces must not
    /// spend another slot: remove the older fact, preserve every other fact's order, then append
    /// the final `replaced` value at its true commit position. Returns whether a distinct oldest
    /// fact had to be evicted because catalog churn left more queued identities than the UI can
    /// simultaneously hold; an eviction also raises [`take_loss`](Self::take_loss).
    pub fn note(&mut self, upload: CatalogUpload) -> bool {
        let queued = self.queue.len();
        let mut coalesced = false;
        for _ in 0..queued {
            if let Some(prior) = self.queue.pop_front() {
                if prior.same_object(upload) {
                    coalesced = true;
                } else {
                    let _ = self.queue.push_back(prior);
                }
            }
        }
        if coalesced {
            let _ = self.queue.push_back(upload);
            return false;
        }
        if let Err(upload) = self.queue.push_back(upload) {
            let _ = self.queue.pop_front();
            let _ = self.queue.push_back(upload);
            self.loss = true;
            return true;
        }
        false
    }

    /// The oldest fact still waiting, in commit order.
    pub fn take(&mut self) -> Option<CatalogUpload> {
        self.queue.pop_front()
    }

    /// Read and clear the loss bit. Clearing on read is what keeps one saturation from forcing a
    /// conservative refresh on every later drain.
    pub fn take_loss(&mut self) -> bool {
        core::mem::take(&mut self.loss)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(id: u64, replaced: bool) -> CatalogUpload {
        CatalogUpload::new(CatalogUploadKind::Route, id, replaced)
    }

    fn drained(facts: &mut UploadFacts) -> heapless::Vec<(u64, bool), UPLOAD_EVENTS_CAP> {
        let mut out = heapless::Vec::new();
        while let Some(upload) = facts.take() {
            assert_eq!(upload.kind(), CatalogUploadKind::Route, "these cases only queue routes");
            let _ = out.push((upload.id(), upload.replaced()));
        }
        out
    }

    /// A rider who re-sends the same route while two others land must not spend three slots on it,
    /// and must not see it out of order: the app applies the newest value at its true commit
    /// position, after both neighbours.
    #[test]
    fn a_repeat_coalesces_onto_its_newest_commit_position() {
        let mut facts = UploadFacts::new();
        assert!(!facts.note(route(7, false)));
        assert!(!facts.note(route(8, false)));
        assert!(!facts.note(route(9, false)));
        assert!(!facts.note(route(7, true)));

        assert_eq!(drained(&mut facts)[..], [(8, false), (9, false), (7, true)]);
    }

    /// The identity is kind plus id. Two objects that share an id are two facts.
    #[test]
    fn a_route_and_a_trip_with_the_same_id_are_separate_facts() {
        let mut facts = UploadFacts::new();
        facts.note(route(4, false));
        facts.note(CatalogUpload::new(CatalogUploadKind::Trip, 4, true));

        assert_eq!(facts.take(), Some(route(4, false)));
        assert_eq!(facts.take(), Some(CatalogUpload::new(CatalogUploadKind::Trip, 4, true)));
        assert_eq!(facts.take(), None);
    }

    /// Churn past one full catalog of distinct identities is the only way to lose a fact. The
    /// oldest goes, the rest keep their order, and the loss bit is the caller's cue to refresh the
    /// active route conservatively.
    #[test]
    fn a_full_queue_evicts_the_oldest_fact_and_reports_the_loss() {
        let mut facts = UploadFacts::new();
        for id in 0..UPLOAD_EVENTS_CAP as u64 {
            assert!(!facts.note(route(id, false)), "a queue that is not full loses nothing");
        }
        assert!(!facts.take_loss(), "and it reports no loss");

        assert!(facts.note(route(UPLOAD_EVENTS_CAP as u64, false)));
        assert!(facts.take_loss());

        let kept = drained(&mut facts);
        assert_eq!(kept.len(), UPLOAD_EVENTS_CAP, "exactly one fact went");
        assert_eq!(kept[0].0, 1, "and it was the oldest");
        assert_eq!(kept[kept.len() - 1].0, UPLOAD_EVENTS_CAP as u64);
    }

    /// A coalescing repeat frees the slot it reuses, so a saturated queue that only sees repeats
    /// stops losing facts.
    #[test]
    fn a_repeat_on_a_full_queue_costs_no_slot() {
        let mut facts = UploadFacts::new();
        for id in 0..UPLOAD_EVENTS_CAP as u64 {
            facts.note(route(id, false));
        }
        assert!(!facts.note(route(0, true)));
        assert!(!facts.take_loss());
        assert_eq!(drained(&mut facts).len(), UPLOAD_EVENTS_CAP);
    }

    /// The bit is a level for one drain only. Leaving it set would force the conservative
    /// active-route refresh on every later catalog read.
    #[test]
    fn the_loss_bit_clears_on_the_drain_that_reads_it() {
        let mut facts = UploadFacts::new();
        for id in 0..=UPLOAD_EVENTS_CAP as u64 {
            facts.note(route(id, false));
        }
        assert!(facts.take_loss());
        assert!(!facts.take_loss());
        while facts.take().is_some() {}
        assert!(!facts.take_loss());
    }
}
