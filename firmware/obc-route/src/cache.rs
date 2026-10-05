//! A small resident cache of route-geometry chunks, the route analogue of
//! `obc_reader::MapCache`.

use core::cell::{Ref, RefCell};

use obc_formats::cache::lru_victim;

use crate::walk::BODY_CAP;

/// Resident chunk slots. Only the chunks crossing the view are read, so a small LRU
/// holds a frame's working set: the matcher's chunk, the riding-zoom view, and one spare for a
/// zoomed-out pan. A very wide view of a winding route still re-reads.
const ROUTE_CHUNK_SLOTS: usize = 3;

/// A chunk's validated 7-byte records as stored, keyed by chunk index, with LRU recency. Raw
/// records are 7 bytes a point against a decoded point's 12; the anchor comes from the index.
/// The owning route identity lives once on [`RouteCacheInner`]. The key is stored as
/// `index + 1`, so zero is the empty tag and the cache is safe to create from all-zero memory.
struct RouteSlot {
    tag: u16,
    // LRU order, not a diagnostic counter. It is rebased before it can overflow, which keeps the
    // slot header at four bytes.
    used: u16,
    len: u16,
    body: [u8; BODY_CAP],
}

/// Without it, a redraw and the matcher's per-fix walk re-pull the same visible chunks from the
/// card every time.
///
/// Caller-owned and reused across frames, paired with the per-frame
/// [`RouteReader`](crate::RouteReader) via [`new_cached`](crate::RouteReader::new_cached). Slots
/// are keyed by chunk index and the cache as a whole adopts the parsed index's identity, so a
/// different route invalidates every same-key slot.
///
/// The state is in a `RefCell` so a `&RouteCache` can fill it. A miss reads the source before it
/// borrows, and a hit stays borrowed only while its chunk is walked.
pub struct RouteCache {
    inner: RefCell<RouteCacheInner>,
}

struct RouteCacheInner {
    /// The [`RouteIndex`](crate::RouteIndex) parse whose chunks occupy the slots. Zero is the
    /// unowned initial state and is never a parsed index.
    identity: u32,
    tick: u16,
    slots: [RouteSlot; ROUTE_CHUNK_SLOTS],
    hits: u32,
    misses: u32,
}

impl Default for RouteCache {
    fn default() -> Self {
        Self::new()
    }
}

impl RouteCache {
    /// A fresh, empty cache. On the device, place it once in the reserved region so it stays off
    /// the main stack.
    pub fn new() -> Self {
        RouteCache { inner: RefCell::new(RouteCacheInner::new()) }
    }

    /// Drop every resident slot and zero the counters. A route switch already invalidates through
    /// [`RouteReader::new_cached`](crate::RouteReader::new_cached). Only the slot tags and counters
    /// are touched.
    ///
    /// # Panics
    /// Panics while a [`RouteReader::with_chunk`](crate::RouteReader::with_chunk) callback walks a
    /// chunk this cache holds.
    pub fn clear(&self) {
        self.inner.borrow_mut().clear();
    }

    /// `(hits, misses)` since the last [`clear`](Self::clear), for the tests.
    pub fn stats(&self) -> (u32, u32) {
        let inner = self.inner.borrow();
        (inner.hits, inner.misses)
    }

    /// Bind the cache to one parsed route. A different identity clears every same-index slot
    /// before the reader can decode; a move of the same index preserves its hits. Identity zero is
    /// accepted for the empty index, which owns no decodable chunks. A walk can pin the old
    /// identity; lookups and fills then defer adoption until the borrow ends.
    pub(crate) fn adopt(&self, identity: u32) {
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            inner.adopt(identity);
        }
    }

    /// Chunk `key`'s resident records. Identity adoption and lookup share one borrow, so an
    /// interleaved reader cannot cross-serve a slot.
    pub(crate) fn borrow_chunk(&self, identity: u32, key: usize) -> Option<Ref<'_, [u8]>> {
        let i = {
            let mut inner = self.inner.try_borrow_mut().ok()?;
            inner.adopt(identity);
            let tag = u16::try_from(key).ok()?.checked_add(1)?;
            let i = inner.slots.iter().position(|s| s.tag == tag)?;
            inner.hits = inner.hits.saturating_add(1);
            inner.slots[i].used = inner.touch();
            i
        };
        Some(Ref::map(self.inner.borrow(), |inner| &inner.slots[i].body[..usize::from(inner.slots[i].len)]))
    }

    /// Store chunk `key`'s validated records, evicting the least recently used slot. The identity
    /// is re-adopted here, after the source read, because a reentrant source can fill the shared
    /// cache for another reader while this miss is in flight.
    pub(crate) fn put(&self, identity: u32, key: usize, body: &[u8]) {
        let Ok(mut inner) = self.inner.try_borrow_mut() else {
            return;
        };
        inner.adopt(identity);
        inner.misses = inner.misses.saturating_add(1);
        let i = lru_victim(inner.slots.iter().map(|s| (s.tag == 0, s.used)));
        let t = inner.touch();
        let s = &mut inner.slots[i];
        // Bounded by `RouteIndex::index`; zero stays reserved for an empty slot.
        s.tag = key as u16 + 1;
        s.used = t;
        s.len = body.len() as u16;
        s.body[..body.len()].copy_from_slice(body);
    }
}

impl RouteCacheInner {
    /// A `const` struct literal, not a zeroed `assume_init`: the whole value is a constant, so the
    /// buffers are never put in `.rodata` to be copied from. The `.rodata` plus `memcpy` lowering
    /// bricks the boot.
    const fn new() -> Self {
        RouteCacheInner {
            identity: 0,
            tick: 0,
            slots: [const { RouteSlot { tag: 0, used: 0, len: 0, body: [0; BODY_CAP] } }; ROUTE_CHUNK_SLOTS],
            hits: 0,
            misses: 0,
        }
    }

    fn adopt(&mut self, identity: u32) {
        if self.identity != identity {
            self.clear();
            self.identity = identity;
        }
    }

    /// Invalidate the slots and reset the counters without changing the adopted identity, so a
    /// reader over the same index simply starts cold.
    fn clear(&mut self) {
        for s in &mut self.slots {
            s.tag = 0;
        }
        self.tick = 0;
        self.hits = 0;
        self.misses = 0;
    }

    #[inline]
    fn touch(&mut self) -> u16 {
        if self.tick == u16::MAX {
            // Once per 65 535 touches, compress the live timestamps to their ranks. This keeps
            // the exact LRU order and stops an old slot becoming recent across a wraparound.
            let old = core::array::from_fn::<_, ROUTE_CHUNK_SLOTS, _>(|i| self.slots[i].used);
            let mut live = 0;
            for i in 0..ROUTE_CHUNK_SLOTS {
                if self.slots[i].tag == 0 {
                    continue;
                }
                let rank =
                    1 + old.iter().enumerate().filter(|(j, used)| self.slots[*j].tag != 0 && **used < old[i]).count()
                        as u16;
                self.slots[i].used = rank;
                live += 1;
            }
            self.tick = live;
        }
        self.tick += 1;
        self.tick
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_clock_rebases_without_changing_eviction_order() {
        let mut inner = RouteCacheInner::new();
        inner.slots[0].tag = 1;
        inner.slots[0].used = 1;
        inner.slots[1].tag = 2;
        inner.slots[1].used = u16::MAX - 1;
        inner.tick = u16::MAX;

        assert_eq!(inner.touch(), 3);
        assert_eq!(inner.slots[0].used, 1);
        assert_eq!(inner.slots[1].used, 2);
        assert_eq!(lru_victim(inner.slots[..2].iter().map(|s| (s.tag == 0, s.used))), 0);
    }
}
