//! The shared quadtree-index block cache.
//!
//! A quadtree walk reads 4-byte nodes whose siblings are adjacent in the file, so a few resident
//! block-aligned windows coalesce a whole descent into a handful of source reads. One driver here
//! serves every walker; callers own only the slot count and the insertion decision.

use crate::io::{ByteSource, Error as IoError};

/// Size of one cached index window. One 512-byte block holds 128 quadtree nodes, so a descent that
/// stays inside one subtree is served from a single fill.
pub const INDEX_BLOCK: usize = 512;

const INDEX_META_RRPV_SHIFT: u8 = 5;
const INDEX_META_VALID: u8 = 0x80;

/// One resident, block-aligned window of an index region. The validity bit and the two-bit RRIP
/// prediction share `meta`, and `len` is bounded by the window — tags packed this tight because the
/// render cache spends every byte they save on the leaf bbox in its chunk slots.
#[derive(Clone, Copy)]
#[repr(C)]
struct IndexBlock {
    /// Which window this holds, as a **block number** (`byte_offset / INDEX_BLOCK`) rather than the
    /// offset itself — exact, never a rounding, since every fill is block-aligned by construction.
    /// A `u32` keeps the cache 4-aligned, which the `align_of::<MapCache>()` assert in `obc-reader`
    /// explains is a boot requirement; the const assert below is the proof that it gives up no
    /// reach in exchange.
    block: u32,
    len: u16,
    meta: u8,
    /// Keep `buf` word-aligned so a full-sector extent read bypasses the board's alignment bounce.
    _align: u8,
    buf: [u8; INDEX_BLOCK],
}

// On-device each compact tagged window is 520 bytes including alignment — unmoved by the u64 read
// seam, which is the point of the block-number tag above.
#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<IndexBlock>() == INDEX_BLOCK + 8);
// The last byte of the largest interior §1.1 permits — `2^32` units at the largest legal
// `Offset Scale` — must still have a block number a `u32` can hold, or [`IndexBlock::block`] would
// be the wall instead of the format. It fits, and at that extreme it fits exactly.
const _: () = assert!(
    ((1u64 << 32) * (1u64 << crate::obcm::OFFSET_SCALE_MAX) - 1) / INDEX_BLOCK as u64 <= u32::MAX as u64,
    "a block number must reach every byte an `Offset Scale` can cover"
);

impl IndexBlock {
    const EMPTY: Self = Self { block: 0, len: 0, meta: 0, _align: 0, buf: [0; INDEX_BLOCK] };

    #[inline]
    fn valid(&self) -> bool {
        self.meta & INDEX_META_VALID != 0
    }

    /// Re-reference prediction (0 = near, 3 = distant). A hit promotes to 0; most one-pass fills
    /// enter at 3 so an ordered tree scan churns one probation slot instead of flushing them all.
    #[inline]
    fn rrpv(&self) -> u8 {
        (self.meta >> INDEX_META_RRPV_SHIFT) & 0x03
    }

    #[inline]
    fn set_rrpv(&mut self, rrpv: u8) {
        self.meta = (self.meta & !(0x03 << INDEX_META_RRPV_SHIFT)) | ((rrpv & 0x03) << INDEX_META_RRPV_SHIFT);
    }

    #[inline]
    fn commit(&mut self, rrpv: u8) {
        self.meta = INDEX_META_VALID | ((rrpv & 0x03) << INDEX_META_RRPV_SHIFT);
    }
}

/// `SLOTS` block-aligned index windows with scan-resistant RRIP replacement.
///
/// RRIP rather than LRU because the access pattern is an *ordered walk repeated*: when the working
/// set is a little larger than the cache, LRU evicts every block just before the next pass asks for
/// it and scores zero hits forever. RRIP keeps a protected subset and churns one probation slot.
///
/// The **insertion** half of that policy is the caller's: `on_fill` decides whether each fill
/// enters protected. Today's two callers answer from two different counters with two different
/// off-by-ones, so a driver that grew a counter of its own would silently change which blocks they
/// evict. [`hits`](Self::hits) and [`misses`](Self::misses) are a report, never an input.
///
/// All-zero is a valid empty cache, so an embedding cache may zero-init it. Tags mean nothing
/// across files: reset before binding to a different source.
pub struct IndexBlockCache<const SLOTS: usize> {
    blocks: [IndexBlock; SLOTS],
    hits: u32,
    misses: u32,
}

impl<const SLOTS: usize> Default for IndexBlockCache<SLOTS> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const SLOTS: usize> IndexBlockCache<SLOTS> {
    /// A zero-slot cache would hang: nothing is ever resident, no slot is empty, and
    /// [`rrip_victim`] ages an empty slice forever looking for a victim. The two hand-written
    /// caches this replaced had fixed arrays and could not express it; making the count a caller's
    /// knob is what opens the hole, so close it where the knob is. Const-generic asserts are lazy,
    /// so [`read`](Self::read) forces this one.
    const NON_EMPTY: () = assert!(SLOTS > 0, "an index cache needs at least one window");

    pub const fn new() -> Self {
        Self { blocks: [IndexBlock::EMPTY; SLOTS], hits: 0, misses: 0 }
    }

    /// Drop every resident window and zero the counters. The counters are part of the reset because
    /// for both callers the counter *is* a policy phase: preserving them across a map switch would
    /// carry the old file's insertion phase into the new one.
    pub fn reset(&mut self) {
        for block in &mut self.blocks {
            block.meta = 0;
        }
        self.hits = 0;
        self.misses = 0;
    }

    /// Node reads served from a resident window.
    #[inline]
    pub const fn hits(&self) -> u32 {
        self.hits
    }

    /// Windows filled from the source.
    #[inline]
    pub const fn misses(&self) -> u32 {
        self.misses
    }

    /// Fill `out` from index-region offset `off`, assembling it from resident windows and reading
    /// any window that is missing. A node read is 4 bytes and may straddle a block edge, so this
    /// loops.
    ///
    /// `on_fill(bytes, fill)` is called once per source fill, after that fill has succeeded and been
    /// counted: `bytes` is what it moved and `fill` is the post-increment [`misses`](Self::misses).
    /// It returns whether the window enters **protected** (RRIP 2) rather than on probation (3) —
    /// the caller's bimodal insertion decision — and doubles as its hook for byte accounting. A fill
    /// into a previously empty slot is protected regardless, so a cold cache seeds every slot.
    pub fn read(
        &mut self,
        src: &dyn ByteSource,
        off: u64,
        out: &mut [u8],
        on_fill: &mut dyn FnMut(usize, u32) -> bool,
    ) -> Result<(), IoError> {
        let () = Self::NON_EMPTY;
        let mut filled = 0usize;
        while filled < out.len() {
            let cur = off.checked_add(filled as u64).ok_or(IoError::BadOffset)?;
            let block_off = cur - cur % INDEX_BLOCK as u64;
            let slot = self.block(src, block_off, on_fill)?;
            let within = (cur - block_off) as usize;
            let blen = self.blocks[slot].len as usize;
            if within >= blen {
                return Err(IoError::BadOffset);
            }
            let take = (blen - within).min(out.len() - filled);
            out[filled..filled + take].copy_from_slice(&self.blocks[slot].buf[within..within + take]);
            filled += take;
        }
        Ok(())
    }

    /// Ensure the [`INDEX_BLOCK`]-aligned window at `block_off` is resident, returning its slot.
    fn block(
        &mut self,
        src: &dyn ByteSource,
        block_off: u64,
        on_fill: &mut dyn FnMut(usize, u32) -> bool,
    ) -> Result<usize, IoError> {
        // Checked, not cast: the const assert on [`IndexBlock::block`] proves no *legal* file
        // reaches a block number past `u32`, but `block_off` is derived from directory bytes and a
        // corrupt one is not legal. A wrap here would alias two different windows of the file and
        // serve one for the other, which is the one failure a cache must never have.
        let tag = u32::try_from(block_off / INDEX_BLOCK as u64).map_err(|_| IoError::BadOffset)?;
        if let Some(i) = self.blocks.iter().position(|b| b.valid() && b.block == tag) {
            self.blocks[i].set_rrpv(0);
            self.hits = self.hits.saturating_add(1);
            return Ok(i);
        }
        // Checked rather than `-`: `block_off` is derived from file data, so a corrupt directory can
        // name a block past the source's end. A `u64` subtraction would panic in debug and produce
        // an absurd length in release; this refuses instead.
        let remaining = src.len().checked_sub(block_off).ok_or(IoError::BadOffset)?;
        let want = remaining.min(INDEX_BLOCK as u64) as usize;
        if want == 0 {
            return Err(IoError::BadOffset);
        }
        let empty = self.blocks.iter().position(|b| !b.valid());
        let i = empty.unwrap_or_else(|| rrip_victim(&mut self.blocks));
        // Invalidate before the read: a flaky source can fail partway, half-overwriting the buffer.
        // Committing the tag only after the read succeeds means a failed read leaves an empty slot,
        // not a poisoned one still keyed to the old block (which would serve as a corrupt hit).
        self.blocks[i].meta = 0;
        src.read_at(block_off, &mut self.blocks[i].buf[..want])?;
        self.blocks[i].block = tag;
        self.blocks[i].len = want as u16;
        self.misses = self.misses.saturating_add(1);
        // Called unconditionally, and *before* the empty-slot arm can decide the answer: it is the
        // caller's per-fill hook, not just a predicate, so short-circuiting it would silently drop
        // the byte accounting of every seeding fill.
        let protect = on_fill(want, self.misses);
        let rrpv = if empty.is_some() || protect { 2 } else { 3 };
        self.blocks[i].commit(rrpv);
        Ok(i)
    }
}

/// Pick the next RRIP victim. If no entry currently predicts a distant re-reference, age every
/// entry one step and try again. Bounded: predictions saturate at 3, so at most three passes.
fn rrip_victim(slots: &mut [IndexBlock]) -> usize {
    loop {
        if let Some(i) = slots.iter().position(|slot| slot.rrpv() >= 3) {
            return i;
        }
        for slot in slots.iter_mut() {
            slot.set_rrpv((slot.rrpv() + 1).min(3));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::SliceSource;

    /// The render cache's seven windows and the router's sixteen.
    const MAP_SLOTS: usize = 7;

    /// `working` distinct 512-byte windows; every byte of block `b` is `b`, so a served window is
    /// checkable against the block that was asked for.
    fn source(working: usize) -> std::vec::Vec<u8> {
        (0..working * INDEX_BLOCK).map(|k| (k / INDEX_BLOCK) as u8).collect()
    }

    /// A source that fills `partial` bytes and then fails, for the read at `fail_at` — the
    /// flaky-SD partial-overwrite a torn fill must survive.
    struct FlakySource<'a> {
        data: &'a [u8],
        fail_at: u64,
    }

    impl ByteSource for FlakySource<'_> {
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), IoError> {
            let start = offset as usize;
            let end = start.checked_add(buf.len()).ok_or(IoError::BadOffset)?;
            let bytes = self.data.get(start..end).ok_or(IoError::BadOffset)?;
            if offset == self.fail_at {
                let n = 8.min(buf.len());
                buf[..n].copy_from_slice(&bytes[..n]); // partial write, then fail
                return Err(IoError::Io);
            }
            buf.copy_from_slice(bytes);
            Ok(())
        }

        fn len(&self) -> u64 {
            self.data.len() as u64
        }
    }

    /// Run `passes` ordered scans over `working` blocks and report `(hits, misses)`.
    ///
    /// Also pins `on_fill`'s **`bytes`** argument, which is what carries a caller's byte accounting
    /// and so has no other cover at this tier: every window of this source is whole, so the reported
    /// bytes must total exactly one [`INDEX_BLOCK`] per fill and nothing per hit.
    fn scan(on_fill: &mut dyn FnMut(usize, u32) -> bool, working: usize, passes: usize) -> (u32, u32) {
        let data = source(working);
        let src = SliceSource(&data);
        let mut cache = IndexBlockCache::<MAP_SLOTS>::new();
        let mut word = [0u8; 4];
        let mut reported = 0usize;
        let (hits, misses) = {
            let mut counted = |bytes: usize, fill: u32| {
                reported += bytes;
                on_fill(bytes, fill)
            };
            for _ in 0..passes {
                for block in 0..working {
                    cache.read(&src, (block * INDEX_BLOCK) as u64, &mut word, &mut counted).unwrap();
                    assert_eq!(word[0], block as u8, "a served window must carry its own block's bytes");
                }
            }
            (cache.hits(), cache.misses())
        };
        assert_eq!(reported, misses as usize * INDEX_BLOCK, "`on_fill` must report one whole window per fill");
        (hits, misses)
    }

    /// The insertion decision is the caller's, and the driver has none of its own: one access
    /// pattern under three `on_fill` sequences must reach three distinct outcomes, with the real
    /// bimodal policy between the two extremes — which is what "bimodal" means. A driver that
    /// quietly grew its own predicate and ignored `on_fill` would collapse all three to one row.
    ///
    /// The extremes rather than the two shipping predicates, deliberately: those are the *same*
    /// every-eighth-fill rule read off two different counters, so they differ only in phase — and
    /// over a repeated scan a phase shift changes nothing. (Verified by exhaustive simulation over
    /// 7 and 16 slots, working sets up to slots+15 and 2–4 passes: not one configuration separates
    /// them.) A test claiming those two sequences diverge would be asserting something false; the
    /// numbers that actually pin each caller's phase are its own exact-count scan-resistance test.
    #[test]
    fn the_insertion_decision_comes_from_the_caller() {
        const WORKING: usize = 18; // one repeated ordered scan, well over MAP_SLOTS
        let always = scan(&mut |_, _| true, WORKING, 2);
        let never = scan(&mut |_, _| false, WORKING, 2);
        // Every eighth fill protected — the shape both shipping callers use.
        let bimodal = scan(&mut |_bytes, fill| fill.is_multiple_of(8), WORKING, 2);
        assert_eq!((always, never, bimodal), ((0, 36), (6, 30), (5, 31)));
    }

    /// A read that fails partway must leave the evicted window *empty*, not poisoned with the old
    /// tag over half-overwritten bytes — otherwise the old block is later served as a corrupt hit.
    #[test]
    fn a_torn_fill_leaves_an_empty_slot_not_a_poisoned_one() {
        const WORKING: usize = MAP_SLOTS + 1; // one more block than slots, so a fill must evict
        let data = source(WORKING);
        // The failing read is the last block's; priming 0..MAP_SLOTS fills every slot first.
        let src = FlakySource { data: &data, fail_at: (MAP_SLOTS * INDEX_BLOCK) as u64 };
        let mut cache = IndexBlockCache::<MAP_SLOTS>::new();
        let mut word = [0u8; 4];
        let mut fill = |_: usize, _: u32| false;

        for block in 0..MAP_SLOTS {
            cache.read(&src, (block * INDEX_BLOCK) as u64, &mut word, &mut fill).unwrap();
        }
        let victim = 0u64; // RRIP's next victim is slot 0, holding block 0
        assert_eq!(cache.misses(), MAP_SLOTS as u32);

        assert_eq!(cache.read(&src, src.fail_at, &mut word, &mut fill), Err(IoError::Io));
        assert_eq!(cache.misses(), MAP_SLOTS as u32, "a failed fill is not a miss");

        // Block 0 must re-read (a miss) and come back with its own bytes, never the torn window's.
        let misses = cache.misses();
        cache.read(&src, victim, &mut word, &mut fill).unwrap();
        assert_eq!(cache.misses(), misses + 1, "the torn slot must be empty, so block 0 re-reads");
        assert_eq!(word[0], 0, "the re-read must return block 0's bytes");
    }

    /// A block number past `u32` is refused rather than wrapped: a wrap would alias two windows of
    /// the file and serve one for the other. No legal file can reach here — the const assert on
    /// `IndexBlock::block` proves that — but a corrupt directory can name it.
    #[test]
    fn a_block_number_past_u32_is_refused_not_wrapped() {
        let data = [0u8; INDEX_BLOCK];
        let src = SliceSource(&data);
        let mut cache = IndexBlockCache::<MAP_SLOTS>::new();
        let mut word = [0u8; 4];
        let past = (u32::MAX as u64 + 1) * INDEX_BLOCK as u64;
        assert_eq!(
            cache.read(&src, past, &mut word, &mut |_, _| false),
            Err(IoError::BadOffset),
            "the block number must not wrap into a resident window's tag"
        );
        assert_eq!(cache.misses(), 0);
    }
}
