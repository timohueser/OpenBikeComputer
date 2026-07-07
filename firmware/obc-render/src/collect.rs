//! Visible-feature collection: fills the frame buffers with every visible feature's geometry plus
//! its [`Span`], in strict global priority order.
//!
//! **Two-phase "stub-select" collect (issue #564).** The device streams map chunks off SPI SD
//! through a tiny cache (one slot under `nrf-mem`), so the collect phase must touch each visible
//! chunk as few times as possible. A single chunk-major walk that filled the frame buffers directly
//! would break the priority-drop guarantee (an early chunk's low-priority features would take
//! capacity a late chunk's high-priority feature needs), so selection is split from geometry
//! retention:
//!
//! - **Pass A** ([`FrameScratch::collect_stubs`]) — one chunk-major walk. Every visible feature is
//!   decoded once (for its bbox cull) and recorded as a fixed-size [`Stub`]; geometry is *not* kept.
//!   When the stub buffer fills, the lowest-priority stub is evicted, so the buffer always holds the
//!   best-by-priority candidates.
//! - **Select** ([`FrameScratch::select`]) — RAM only, no I/O. Stubs are sorted into the old
//!   level-major order and admitted greedily against the exact point / ring / span budgets, so drops
//!   are strictly lowest-priority-first *globally* and the surviving order reproduces the old
//!   collector's exactly (byte-identical output when nothing saturates).
//! - **Pass B** ([`FrameScratch::decode_winners`]) — a second chunk-major walk that re-decodes only
//!   the winners, appending their geometry and rewriting each stub slot in place with its final
//!   [`Span`]. Only chunks that own a winner are refetched.
//!
//! SD traffic drops from `4 × N` chunk fetches per frame (the old level-major collector's four
//! priority passes) to `≤ 2 × N`. The stubs share the `slots` buffer the spans end up in (a [`Stub`]
//! fits a [`Span`] slot, asserted below), so the split costs no extra frame RAM.

use heapless::Vec;

use obc_reader::{BBox, Kind, Reader};

use crate::{RenderStats, MAX_DECODE_POINTS, MAX_DECODE_RINGS, MAX_FRAME_POINTS, MAX_FRAME_RINGS, MAX_SPANS};

/// The renderer's collection scratch: per-feature decode buffers plus the frame buffers that
/// accumulate every visible feature's geometry (and its [`Span`]). Cleared (not freed) each frame.
#[derive(Default)]
pub(crate) struct FrameScratch {
    // Per-feature decode scratch handed to `Reader::for_each_feature_filtered` / `decode_feature_at`.
    dec_points: Vec<(i32, i32), MAX_DECODE_POINTS>,
    dec_ring_lens: Vec<usize, MAX_DECODE_RINGS>,
    // All drawn features' geometry, concatenated (filled in pass B).
    pub(crate) frame_points: Vec<(i32, i32), MAX_FRAME_POINTS>,
    pub(crate) frame_ring_lens: Vec<usize, MAX_FRAME_RINGS>,
    // One record per candidate: a [`Stub`] during passes A / select, rewritten to the final [`Span`]
    // in pass B. After `collect`, its first `spans_len` entries are all the `span` variant.
    slots: Vec<Slot, MAX_SPANS>,
    /// How many leading `slots` are live final [`Span`]s after `collect` (the admitted-winner count).
    spans_len: usize,
}

impl FrameScratch {
    /// Fill the frame buffers with every visible feature, in strict global priority order, via the
    /// two-phase stub-select collect (see the module docs). On return, [`FrameScratch::spans`] /
    /// [`FrameScratch::spans_mut`] expose the drawn features' [`Span`]s (unordered — the caller
    /// sorts them into painter order).
    pub(crate) fn collect(&mut self, reader: &Reader, lod: usize, view: &BBox, stats: &mut RenderStats) {
        self.frame_points.clear();
        self.frame_ring_lens.clear();
        self.slots.clear();
        self.spans_len = 0;

        // A single "is this style drawn at all?" mask (bit set ⇔ the id has a style), built once —
        // the old per-priority-level masks are gone: pass A decodes every drawn feature in one walk.
        let mut vis_mask = [0u32; 8];
        for id in 0..=255u8 {
            if reader.style(id).is_some() {
                vis_mask[(id >> 5) as usize] |= 1 << (id & 31);
            }
        }

        let candidates = self.collect_stubs(reader, lod, view, &vis_mask, stats);
        let winners = self.select(reader);
        self.decode_winners(reader, lod, view, winners, stats);

        self.spans_len = winners;
        stats.features_drawn = winners;
        // Every candidate that passed the cull is either drawn or dropped (evicted in pass A or cut
        // by the point/ring budget in select). Culled features count in `features_tried`, not here —
        // matching the old collector, so `drawn + dropped == tried` holds when nothing is culled.
        stats.features_dropped = candidates - winners;
        stats.span_utilization = winners as f32 / self.slots.capacity() as f32;
        stats.point_utilization = self.frame_points.len() as f32 / self.frame_points.capacity() as f32;
        stats.ring_utilization = self.frame_ring_lens.len() as f32 / self.frame_ring_lens.capacity() as f32;
    }

    /// **Pass A.** One chunk-major walk over the viewport's leaves ([`Reader::for_each_chunk`]),
    /// decoding every visible feature once (its bbox comes free from the decode) and recording a
    /// [`Stub`] — no geometry kept. On stub-buffer overflow the lowest-priority stub is evicted, so
    /// the buffer always holds the best-by-priority candidates (the triage that keeps the priority
    /// guarantee under span saturation). Returns the number of candidates that passed the per-feature
    /// cull; leaves the surviving stubs in `self.slots`.
    fn collect_stubs(
        &mut self,
        reader: &Reader,
        lod: usize,
        view: &BBox,
        vis_mask: &[u32; 8],
        stats: &mut RenderStats,
    ) -> usize {
        // Split the borrow so the decode callback can push stubs while `for_each_feature_filtered`
        // borrows the decode scratch.
        let FrameScratch { dec_points, dec_ring_lens, slots, .. } = self;
        let mut candidates = 0usize;
        let mut arrival = 0u16;
        // Count of resident stubs at each priority level (1..=4 → index 0..=3), for O(1) worst-level
        // eviction triage.
        let mut level_count = [0u16; 4];
        let mut chunks = 0usize;

        reader.for_each_chunk(lod, view, |cid, node| {
            chunks += 1;
            reader.for_each_feature_filtered(
                lod,
                cid,
                &node,
                dec_points,
                dec_ring_lens,
                |sid| vis_mask[(sid >> 5) as usize] & (1 << (sid & 31)) != 0,
                |f| {
                    let style = match reader.style(f.style_id) {
                        Some(s) => s,
                        None => return,
                    };

                    let pts = f.points();
                    stats.features_tried += 1;
                    stats.points_tried += pts.len();

                    // Per-feature bbox cull (tighter than the leaf); bounds come free from decode.
                    if pts.is_empty() || !f.bbox().intersects(view) {
                        return;
                    }
                    candidates += 1;

                    let level = style.priority; // 1..=4
                    let stub = Stub::new(cid, f.offset(), pts.len(), f.ring_lens().len(), f.style_id, arrival);
                    arrival = arrival.saturating_add(1);

                    if !slots.is_full() {
                        let _ = slots.push(Slot::of_stub(stub));
                        level_count[(level - 1) as usize] += 1;
                        return;
                    }

                    // Buffer full — a streaming "keep the K lowest-keyed" selection, key =
                    // (priority_level, arrival). This candidate's arrival is the largest seen so far,
                    // so it can only displace a *higher-level* held stub; and to keep exactly the K
                    // lowest keys, the one it displaces is the **highest-arrival** stub at the worst
                    // (highest) level present. Evicting that specific stub — not just any at that
                    // level — makes the survivors identical to the old level-major collector's set
                    // under saturation too, so the render stays byte-identical there, not only when
                    // nothing saturates. Equal-or-worse candidates are dropped, so a higher-priority
                    // feature is never lost to a lower one.
                    let worst = worst_level(&level_count);
                    if level < worst {
                        let mut victim = 0usize;
                        let mut victim_arrival = 0u16;
                        let mut found = false;
                        for (i, slot) in slots.iter().enumerate() {
                            let held = slot.stub();
                            // `held.seq` still holds the pass-A arrival (select overwrites it later).
                            if reader.style(held.style_id).map_or(0, |s| s.priority) == worst
                                && (!found || held.seq > victim_arrival)
                            {
                                victim = i;
                                victim_arrival = held.seq;
                                found = true;
                            }
                        }
                        // `worst` has a positive count, so a victim always exists.
                        slots[victim] = Slot::of_stub(stub);
                        level_count[(worst - 1) as usize] -= 1;
                        level_count[(level - 1) as usize] += 1;
                        stats.stub_evictions += 1;
                    }
                },
            );
        });
        stats.chunks_visited = chunks;
        candidates
    }

    /// **Select.** RAM only. Sort the surviving stubs into the old level-major, chunk-walk order
    /// (`(priority_level, arrival)`), then admit greedily while the exact point / ring budgets hold —
    /// so drops are strictly lowest-priority-first and, unsaturated, every candidate is admitted in
    /// the old collector's exact order. Admitted stubs are compacted to the front of `self.slots`
    /// with their `seq` set to the admission index; returns the admitted count.
    fn select(&mut self, reader: &Reader) -> usize {
        let slots = &mut self.slots;
        // `(priority_level, arrival)`: level-major, and within a level the pass-A encounter order —
        // which is the quadtree-walk order, identical to the old level-major collector's. Sorting
        // by it and assigning `seq` from the result reproduces the old paint order exactly.
        slots.sort_unstable_by_key(|slot| {
            let s = slot.stub();
            let level = reader.style(s.style_id).map_or(u8::MAX, |st| st.priority);
            (level, s.seq)
        });

        let mut used_pts = 0usize;
        let mut used_rings = 0usize;
        let mut m = 0usize;
        for i in 0..slots.len() {
            let mut s = slots[i].stub();
            let pts = s.total_pts as usize;
            let rings = s.ring_count as usize;
            if used_pts + pts <= MAX_FRAME_POINTS && used_rings + rings <= MAX_FRAME_RINGS {
                used_pts += pts;
                used_rings += rings;
                // Reuse the arrival field as the final painter seq (admission index).
                s.seq = m as u16;
                // Compaction: `m <= i`, and `slots[i]` was already read into `s`, so writing
                // `slots[m]` never clobbers an unread stub.
                slots[m] = Slot::of_stub(s);
                m += 1;
            }
        }
        slots.truncate(m);
        m
    }

    /// **Pass B.** A second chunk-major walk ([`Reader::for_each_chunk`], same viewport as pass A, so
    /// every winner's leaf is revisited and its `node` anchor comes free). For each visited chunk,
    /// re-decode the winners it owns via [`Reader::decode_feature_at`] — consecutive winners in one
    /// chunk hit the resident cache slot, so each winner-owning chunk is fetched once — append their
    /// geometry to the frame buffers, and rewrite each stub slot in place with its final [`Span`].
    fn decode_winners(&mut self, reader: &Reader, lod: usize, view: &BBox, winners: usize, stats: &mut RenderStats) {
        if winners == 0 {
            return;
        }
        let FrameScratch { dec_points, dec_ring_lens, frame_points, frame_ring_lens, slots, .. } = self;
        // A winner slot, once rewritten to its `Span`, must not be re-read as a stub by a later
        // chunk's scan. `placed` marks the done slots so the scan skips them.
        let mut placed = [0u32; MAX_SPANS.div_ceil(32)];

        reader.for_each_chunk(lod, view, |cid, node| {
            let mut refetched = false;
            for i in 0..winners {
                if placed[i >> 5] & (1 << (i & 31)) != 0 {
                    continue;
                }
                let stub = slots[i].stub();
                if stub.cid() != cid {
                    continue;
                }
                let (z, weight, color) = match reader.style(stub.style_id) {
                    Some(s) => (s.z_index, s.weight, s.color),
                    None => (0, 0, 0),
                };
                let pt_start = frame_points.len() as u16;
                let ring_start = frame_ring_lens.len() as u16;
                // Re-decode this winner. The point/ring budget was reserved for it in `select`, so
                // the appends fit; the `is_ok` guards stay defensive against a corrupt refetch (then
                // the slot becomes a no-draw span, keeping the winner count consistent).
                let span =
                    match reader.decode_feature_at(lod, cid, stub.offset as usize, &node, dec_points, dec_ring_lens) {
                        Some(f)
                            if frame_points.extend_from_slice(f.points()).is_ok()
                                && frame_ring_lens.extend_from_slice(f.ring_lens()).is_ok() =>
                        {
                            refetched = true;
                            stats.points_drawn += f.points().len();
                            Span {
                                kind: f.kind,
                                z,
                                weight,
                                color,
                                pt_start,
                                ring_start,
                                ring_count: f.ring_lens().len() as u16,
                                seq: stub.seq,
                            }
                        }
                        _ => Span {
                            kind: Kind::Line,
                            z,
                            weight,
                            color,
                            pt_start,
                            ring_start,
                            ring_count: 0,
                            seq: stub.seq,
                        },
                    };
                slots[i] = Slot::of_span(span);
                placed[i >> 5] |= 1 << (i & 31);
            }
            if refetched {
                stats.chunks_refetched += 1;
            }
        });
    }

    /// The drawn features' spans (unordered; the caller sorts them into painter order). Valid only
    /// after [`FrameScratch::collect`], which leaves every live slot holding the `span` variant.
    #[inline]
    pub(crate) fn spans(&self) -> &[Span] {
        // SAFETY: after `collect`, `slots[..spans_len]` are all the `span` variant; `Slot` and `Span`
        // share layout (union; `size_of` asserted equal), so the reinterpret is sound and in bounds
        // (`spans_len <= slots.len()`).
        unsafe { core::slice::from_raw_parts(self.slots.as_ptr() as *const Span, self.spans_len) }
    }

    /// The drawn features' spans, mutable — for the painter's-order sort.
    #[inline]
    pub(crate) fn spans_mut(&mut self) -> &mut [Span] {
        // SAFETY: as [`FrameScratch::spans`].
        unsafe { core::slice::from_raw_parts_mut(self.slots.as_mut_ptr() as *mut Span, self.spans_len) }
    }
}

/// Highest priority level (1..=4, i.e. *lowest* priority) with a resident stub, for eviction triage;
/// `4` when the buffer is somehow empty (never, since the caller only asks when it is full).
#[inline]
fn worst_level(level_count: &[u16; 4]) -> u8 {
    for level in (1..=4u8).rev() {
        if level_count[(level - 1) as usize] > 0 {
            return level;
        }
    }
    4
}

/// A pass-A candidate: exactly what selection and the pass-B re-decode need, and nothing else —
/// never geometry. Sized to fit a [`Span`] slot (asserted below) so the whole candidate set lives in
/// the same `slots` buffer pass B rewrites in place: the split into stubs + spans costs no extra
/// frame RAM (issue #564). `#[repr(C)]`, and `cid` is split into two `u16` halves, to keep the struct
/// 2-aligned and `<= size_of::<Span>()`.
#[derive(Clone, Copy)]
#[repr(C)]
struct Stub {
    cid_lo: u16,
    cid_hi: u16,
    /// Feature byte offset within its chunk (chunks `<= MAX_CHUNK_BYTES`, so it fits a `u16`).
    offset: u16,
    /// All-rings vertex count, for the exact point-budget admission.
    total_pts: u16,
    /// Ring count, for the ring-budget admission.
    ring_count: u16,
    /// Pass-A encounter index (level-major seq replication), overwritten with the admission `seq`
    /// during select — pass B reads it for the painter's-order tie-break.
    seq: u16,
    /// Style id. Priority / z / weight / color / kind all re-derive `O(1)` from the style table, so
    /// none of them are copied here.
    style_id: u8,
}

impl Stub {
    #[inline]
    fn new(cid: u32, offset: usize, total_pts: usize, ring_count: usize, style_id: u8, arrival: u16) -> Stub {
        Stub {
            cid_lo: cid as u16,
            cid_hi: (cid >> 16) as u16,
            offset: offset as u16,
            total_pts: total_pts as u16,
            ring_count: ring_count as u16,
            seq: arrival,
            style_id,
        }
    }

    #[inline]
    fn cid(&self) -> u32 {
        (self.cid_hi as u32) << 16 | self.cid_lo as u32
    }
}

/// One `slots` entry: a [`Stub`] during passes A / select, rewritten in place to the final [`Span`]
/// in pass B. Both variants are `Copy` with no `Drop`, so the union is sound to overwrite; the phase
/// (never mixed within one entry at one time) determines which variant is live, and the accessors
/// below read the one the current phase wrote.
union Slot {
    stub: Stub,
    span: Span,
}

impl Slot {
    #[inline]
    fn of_stub(stub: Stub) -> Slot {
        Slot { stub }
    }
    #[inline]
    fn of_span(span: Span) -> Slot {
        Slot { span }
    }
    /// Read the `stub` variant. Valid only during passes A / select (before this slot is rewritten).
    #[inline]
    fn stub(&self) -> Stub {
        // SAFETY: the caller only reads `stub` in a phase that wrote a `Stub` into this slot; both
        // variants are `Copy` plain-old-data, so the read is well-defined.
        unsafe { self.stub }
    }
}

// The union reuses the span buffer for stubs, so a stub must fit a span slot, and the two must share
// a size (or the `slots`-as-`[Span]` reinterpret in `spans()` / the `MCU_RENDERER_BYTES` accounting
// would be wrong).
const _: () = assert!(core::mem::size_of::<Stub>() <= core::mem::size_of::<Span>(), "Stub must fit a Span slot");
const _: () = assert!(core::mem::size_of::<Slot>() == core::mem::size_of::<Span>(), "Slot must be Span-sized");

/// One visible feature's draw metadata plus the ranges locating its geometry in the frame buffers.
/// Cheap to sort for the painter's algorithm.
///
/// Offsets are `u16` (not `usize`) to keep the struct to 14 bytes — thousands are buffered at
/// coarse zoom. The frame buffers they index are asserted `<= u16::MAX` at the buffer constants.
#[derive(Clone, Copy)]
pub(crate) struct Span {
    pub(crate) kind: Kind,
    pub(crate) z: i8,
    pub(crate) weight: u8,
    pub(crate) color: u16,
    pub(crate) pt_start: u16,
    pub(crate) ring_start: u16,
    pub(crate) ring_count: u16,
    pub(crate) seq: u16,
}
