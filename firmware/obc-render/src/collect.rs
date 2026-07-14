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

use obc_map_scene::{
    BBox, Candidate, Feature, FeatureError, FeatureToken, Kind, MapScene, ReadFailures, SelectedFeatures,
};

use crate::{
    RenderStats, Viewport, BASE_MAP_INK_MARGIN_PX, MAX_DECODE_POINTS, MAX_DECODE_RINGS, MAX_FRAME_POINTS,
    MAX_FRAME_RINGS, MAX_SPANS,
};

/// The renderer's collection scratch: per-feature decode buffers plus the frame buffers that
/// accumulate every visible feature's geometry (and its [`Span`]). Cleared (not freed) each frame.
#[derive(Default)]
pub(crate) struct FrameScratch {
    // Per-feature decode scratch handed to the scene source's two streamed passes.
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
    pub(crate) fn collect<S: MapScene>(
        &mut self,
        scene: &S,
        lod: usize,
        vp: &Viewport,
        view: &BBox,
        stats: &mut RenderStats,
    ) {
        self.frame_points.clear();
        self.frame_ring_lens.clear();
        self.slots.clear();
        self.spans_len = 0;

        // A single "is this style drawn at all?" mask (bit set ⇔ the id has a style), built once —
        // the old per-priority-level masks are gone: pass A decodes every drawn feature in one walk.
        let mut vis_mask = [0u32; 8];
        for id in 0..=255u8 {
            if scene.style(id).is_some() {
                vis_mask[(id >> 5) as usize] |= 1 << (id & 31);
            }
        }

        let candidates = self.collect_stubs(scene, lod, vp, view, &vis_mask, stats);
        let winners = self.select();
        let drawn = self.decode_winners(scene, lod, view, winners, stats);

        self.spans_len = drawn;
        stats.features_drawn = drawn;
        // Every candidate that passed the cull is either drawn or dropped (evicted in pass A or cut
        // by the point/ring budget in select). Culled features count in `features_tried`, not here —
        // matching the old collector, so `drawn + dropped == tried` holds when nothing is culled.
        stats.features_dropped = candidates - winners;
        stats.span_utilization = drawn as f32 / self.slots.capacity() as f32;
        stats.point_utilization = self.frame_points.len() as f32 / self.frame_points.capacity() as f32;
        stats.ring_utilization = self.frame_ring_lens.len() as f32 / self.frame_ring_lens.capacity() as f32;

        // TEMP debug (scratch-budget investigation): split the drawn geometry by kind so the sim can
        // show which render path — lines or polygons — eats the span/point/ring scratch at the zoom
        // levels that saturate it. A span's point count is the sum of its ring lengths.
        for span in self.spans() {
            let start = span.ring_start as usize;
            let rings = span.ring_count as usize;
            let points: usize = self.frame_ring_lens[start..start + rings].iter().sum();
            match span.kind {
                Kind::Line => {
                    stats.line_spans += 1;
                    stats.line_rings += rings;
                    stats.line_points += points;
                }
                Kind::Polygon => {
                    stats.poly_spans += 1;
                    stats.poly_rings += rings;
                    stats.poly_points += points;
                }
            }
        }
    }

    /// **Pass A.** One source-native walk over the viewport, decoding every visible feature once
    /// (its bbox comes free from the decode) and recording a
    /// [`Stub`] — no geometry kept. On stub-buffer overflow the lowest-priority stub is evicted, so
    /// the buffer always holds the best-by-priority candidates (the triage that keeps the priority
    /// guarantee under span saturation). Returns the number of candidates that passed the per-feature
    /// cull; leaves the surviving stubs in `self.slots`.
    fn collect_stubs<S: MapScene>(
        &mut self,
        scene: &S,
        lod: usize,
        vp: &Viewport,
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
        let report = scene.visit_candidates(
            lod,
            view,
            dec_points,
            dec_ring_lens,
            |sid| vis_mask[(sid >> 5) as usize] & (1 << (sid & 31)) != 0,
            |Candidate { token, feature: f }| {
                let pts = f.points();
                stats.features_tried += 1;
                stats.points_tried += pts.len();
                let Some(style) = scene.style(f.style_id) else {
                    stats.malformed_features = stats.malformed_features.saturating_add(1);
                    return;
                };

                // Scene sources are outside the renderer's trust boundary. Reject malformed ring
                // partitions and invalid priority levels before they can reserve a stub or index
                // the four-level triage table.
                if !f.has_valid_rings() || !(1..=4).contains(&style.priority) {
                    stats.malformed_features = stats.malformed_features.saturating_add(1);
                    return;
                }

                // Per-feature map-space bbox cull (tighter than the leaf); bounds come free from
                // decode. Keep it first — it is the cheaper of the two culls.
                let bbox = f.bbox();
                if !bbox.intersects(view) {
                    return;
                }
                // Conservative screen-space broad phase (issue #847). A heading-up view's enclosing
                // map-space AABB has large empty corners the map-space test above still admits;
                // reject a candidate whose projected-corner screen AABB can't touch the
                // ink-margin-expanded display. Affine projection ⇒ an AABB miss is a safe reject.
                // Only after both tests pass does the feature count as an admitted candidate.
                if !vp.bbox_may_touch_screen(&bbox, BASE_MAP_INK_MARGIN_PX) {
                    return;
                }
                candidates += 1;

                let level = style.priority; // 1..=4
                let stub = Stub::new(token, pts.len(), f.ring_lens().len(), f.style_id, f.kind, level, arrival);
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
                        if held.priority() == worst && (!found || held.seq > victim_arrival) {
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
        stats.feature_decode_capacity_drops =
            stats.feature_decode_capacity_drops.saturating_add(report.capacity_dropped);
        stats.malformed_features = stats.malformed_features.saturating_add(report.malformed_features);
        record_read_failures(stats, report.read_failures);
        stats.chunks_visited = report.chunks_visited;
        candidates
    }

    /// **Select.** RAM only. Sort the surviving stubs into the old level-major, chunk-walk order
    /// (`(priority_level, arrival)`), then admit greedily while the exact point / ring budgets hold —
    /// so drops are strictly lowest-priority-first and, unsaturated, every candidate is admitted in
    /// the old collector's exact order. Admitted stubs are compacted to the front of `self.slots`
    /// with their `seq` set to the admission index; returns the admitted count.
    fn select(&mut self) -> usize {
        let slots = &mut self.slots;
        // `(priority_level, arrival)`: level-major, and within a level the pass-A encounter order —
        // which is the quadtree-walk order, identical to the old level-major collector's. Sorting
        // by it and assigning `seq` from the result reproduces the old paint order exactly.
        slots.sort_unstable_by_key(|slot| {
            let s = slot.stub();
            (s.priority(), s.seq)
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

    /// **Pass B.** Ask the scene to stream the same view again and re-decode only selected tokens.
    /// The source retains its natural cache/grouping order; this renderer sees no file offsets or
    /// quadtree records and appends complete geometry directly into its existing frame buffers.
    fn decode_winners<S: MapScene>(
        &mut self,
        scene: &S,
        lod: usize,
        view: &BBox,
        winners: usize,
        stats: &mut RenderStats,
    ) -> usize {
        if winners == 0 {
            return 0;
        }
        let FrameScratch { dec_points, dec_ring_lens, frame_points, frame_ring_lens, slots, .. } = self;
        // A winner slot, once rewritten to its `Span`, must not be re-read as a stub by a later
        // chunk's scan. `placed` marks the done slots so the scan skips them.
        let mut placed = [0u32; MAX_SPANS.div_ceil(32)];
        let mut selected =
            DecodeSink { scene, winners, frame_points, frame_ring_lens, slots, placed: &mut placed, stats, drawn: 0 };
        let report = scene.decode_selected(lod, view, dec_points, dec_ring_lens, &mut selected);
        selected.stats.chunks_refetched = selected.stats.chunks_refetched.saturating_add(report.chunks_refetched);
        record_read_failures(selected.stats, report.read_failures);
        let drawn = selected.drawn;

        // The second index walk or a winner refetch may fail after only some slots were rewritten.
        // Compact only successfully decoded spans. `placed` is the variant tag here: an unset bit
        // means the slot is still a Stub and must not be read through the union's Span arm; a set
        // bit means pass B wrote a Span, with `ring_count == 0` reserved for a failed refetch.
        let mut compacted = 0usize;
        for i in 0..winners {
            if placed[i >> 5] & (1 << (i & 31)) == 0 {
                continue;
            }
            let span = slots[i].span();
            if span.ring_count == 0 {
                continue;
            }
            slots[compacted] = Slot::of_span(span);
            compacted += 1;
        }
        slots.truncate(compacted);
        debug_assert_eq!(compacted, drawn);
        compacted
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

struct DecodeSink<'a, S: MapScene> {
    scene: &'a S,
    winners: usize,
    frame_points: &'a mut Vec<(i32, i32), MAX_FRAME_POINTS>,
    frame_ring_lens: &'a mut Vec<usize, MAX_FRAME_RINGS>,
    slots: &'a mut Vec<Slot, MAX_SPANS>,
    placed: &'a mut [u32; MAX_SPANS.div_ceil(32)],
    stats: &'a mut RenderStats,
    drawn: usize,
}

impl<S: MapScene> SelectedFeatures for DecodeSink<'_, S> {
    #[inline]
    fn len(&self) -> usize {
        self.winners
    }

    #[inline]
    fn is_pending(&self, index: usize) -> bool {
        self.pending(index)
    }

    #[inline]
    fn token(&self, index: usize) -> Option<FeatureToken> {
        self.pending(index).then(|| self.slots[index].stub().token)
    }

    fn decoded(&mut self, index: usize, feature: Feature<'_>) -> bool {
        if !self.pending(index) {
            return false;
        }
        let stub = self.slots[index].stub();
        let Some(style) = self.scene.style(stub.style_id) else {
            self.finish_error(index, FeatureError::Malformed);
            return false;
        };
        if !feature.has_valid_rings()
            || feature.style_id != stub.style_id
            || feature.kind != stub.kind()
            || feature.points().len() != stub.total_pts as usize
            || feature.ring_lens().len() != stub.ring_count as usize
        {
            self.finish_error(index, FeatureError::Malformed);
            return false;
        }
        if feature.points().len() > self.frame_points.capacity() - self.frame_points.len() {
            self.finish_error(index, FeatureError::Capacity(obc_map_scene::CapacityError::Points));
            return false;
        }
        if feature.ring_lens().len() > self.frame_ring_lens.capacity() - self.frame_ring_lens.len() {
            self.finish_error(index, FeatureError::Capacity(obc_map_scene::CapacityError::Rings));
            return false;
        }

        let pt_start = self.frame_points.len() as u16;
        let ring_start = self.frame_ring_lens.len() as u16;
        // The exact pass-A reservation plus the remaining-capacity checks above make these
        // infallible and keep publication transactional: no partial geometry is ever visible.
        if self.frame_points.extend_from_slice(feature.points()).is_err()
            || self.frame_ring_lens.extend_from_slice(feature.ring_lens()).is_err()
        {
            unreachable!("prechecked frame capacity");
        };
        self.drawn += 1;
        self.stats.points_drawn += feature.points().len();
        let span = Span {
            kind: feature.kind,
            z: style.z_index,
            weight: style.weight,
            style_id: stub.style_id,
            color: style.color,
            pt_start,
            ring_start,
            ring_count: feature.ring_lens().len() as u16,
            seq: stub.seq,
        };
        self.slots[index] = Slot::of_span(span);
        self.placed[index >> 5] |= 1 << (index & 31);
        true
    }

    fn failed(&mut self, index: usize, error: FeatureError) -> bool {
        if !self.pending(index) {
            return false;
        }
        self.finish_error(index, error);
        true
    }
}

impl<S: MapScene> DecodeSink<'_, S> {
    #[inline]
    fn pending(&self, index: usize) -> bool {
        index < self.winners && self.placed[index >> 5] & (1 << (index & 31)) == 0
    }

    fn finish_error(&mut self, index: usize, error: FeatureError) {
        debug_assert!(self.pending(index));
        record_feature_error(self.stats, error);
        let stub = self.slots[index].stub();
        let (z, weight, color) =
            self.scene.style(stub.style_id).map_or((0, 0, 0), |style| (style.z_index, style.weight, style.color));
        let span =
            empty_span(stub, z, weight, color, self.frame_points.len() as u16, self.frame_ring_lens.len() as u16);
        self.slots[index] = Slot::of_span(span);
        self.placed[index >> 5] |= 1 << (index & 31);
    }
}

#[inline]
fn record_read_failures(stats: &mut RenderStats, failures: ReadFailures) {
    stats.map_read_failures = stats.map_read_failures.saturating_add(failures.source);
    stats.map_cache_contentions = stats.map_cache_contentions.saturating_add(failures.cache_busy);
    stats.map_structure_failures = stats.map_structure_failures.saturating_add(failures.malformed);
}

#[inline]
fn record_read_error(stats: &mut RenderStats, error: obc_map_scene::ReadError) {
    match error {
        obc_map_scene::ReadError::Source => stats.map_read_failures = stats.map_read_failures.saturating_add(1),
        obc_map_scene::ReadError::CacheBusy => {
            stats.map_cache_contentions = stats.map_cache_contentions.saturating_add(1)
        }
        obc_map_scene::ReadError::Malformed => {
            stats.map_structure_failures = stats.map_structure_failures.saturating_add(1)
        }
    }
}

#[inline]
fn record_feature_error(stats: &mut RenderStats, error: FeatureError) {
    match error {
        FeatureError::Capacity(_) => {
            stats.feature_decode_capacity_drops = stats.feature_decode_capacity_drops.saturating_add(1)
        }
        FeatureError::Malformed => stats.malformed_features = stats.malformed_features.saturating_add(1),
        FeatureError::Read(error) => record_read_error(stats, error),
    }
}

#[inline]
fn empty_span(stub: Stub, z: i8, weight: u8, color: u16, pt_start: u16, ring_start: u16) -> Span {
    Span {
        kind: Kind::Line,
        z,
        weight,
        style_id: stub.style_id,
        color,
        pt_start,
        ring_start,
        ring_count: 0,
        seq: stub.seq,
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
/// frame RAM (issue #564). The six-byte source token stays opaque to the renderer.
#[derive(Clone, Copy)]
#[repr(C)]
struct Stub {
    token: FeatureToken,
    /// All-rings vertex count, for the exact point-budget admission.
    total_pts: u16,
    /// Ring count, for the ring-budget admission.
    ring_count: u16,
    /// Pass-A encounter index (level-major seq replication), overwritten with the admission `seq`
    /// during select — pass B reads it for the painter's-order tie-break.
    seq: u16,
    /// Style id. z / weight / color re-derive `O(1)` from the style table.
    style_id: u8,
    /// Pass-A priority (bits 1..=3) and geometry kind (bit 0). Keeping both identity fields in the
    /// old padding byte makes selection independent of later style-table answers while the stub and
    /// span remain exactly 14 bytes.
    priority_kind: u8,
}

impl Stub {
    #[inline]
    fn new(
        token: FeatureToken,
        total_pts: usize,
        ring_count: usize,
        style_id: u8,
        kind: Kind,
        priority: u8,
        arrival: u16,
    ) -> Stub {
        let kind_bit = match kind {
            Kind::Line => 0,
            Kind::Polygon => 1,
        };
        Stub {
            token,
            total_pts: total_pts as u16,
            ring_count: ring_count as u16,
            seq: arrival,
            style_id,
            priority_kind: priority << 1 | kind_bit,
        }
    }

    #[inline]
    fn priority(self) -> u8 {
        self.priority_kind >> 1
    }

    #[inline]
    fn kind(self) -> Kind {
        if self.priority_kind & 1 == 0 {
            Kind::Line
        } else {
            Kind::Polygon
        }
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
    /// Read the `span` variant after pass B marked this slot as placed.
    #[inline]
    fn span(&self) -> Span {
        // SAFETY: the caller checks pass B's placed bit, which is set only after `of_span` was
        // written. Both variants are `Copy` plain-old-data.
        unsafe { self.span }
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
/// `style_id` fills what was a spare padding byte (the `u8` fields pack against the `u16`s), so the
/// draw loop can re-resolve the full scene style — `dashed`/`color2` — via the source's hot `O(1)`
/// style table without widening `Span`.
#[derive(Clone, Copy)]
pub(crate) struct Span {
    pub(crate) kind: Kind,
    pub(crate) z: i8,
    pub(crate) weight: u8,
    pub(crate) style_id: u8,
    pub(crate) color: u16,
    pub(crate) pt_start: u16,
    pub(crate) ring_start: u16,
    pub(crate) ring_count: u16,
    pub(crate) seq: u16,
}

// `style_id` must land in the spare byte, not grow the struct — thousands are buffered per frame and
// `MCU_RENDERER_BYTES` budgets `MAX_SPANS * size_of::<Span>()`.
const _: () = assert!(core::mem::size_of::<Span>() == 14, "Span must stay 14 bytes");
