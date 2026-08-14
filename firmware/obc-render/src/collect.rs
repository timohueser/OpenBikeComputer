//! Visible-feature collection: fills the frame buffers with every visible feature's geometry plus
//! its [`Span`], in strict global priority order.
//!
//! **Two-phase "stub-select" collect (issue #564).** The device streams map chunks off SPI SD
//! through a small cache, so the collect phase must touch each visible
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
//!   collector's exactly. The only geometry change is the lossless off-panel polygon compaction
//!   below, which is raster-identical inside the panel.
//! - **Pass B** ([`FrameScratch::decode_winners`]) — a second chunk-major walk that re-decodes only
//!   the winners, appending their geometry and rewriting each stub slot in place with its final
//!   [`Span`]. Only chunks that own a winner are refetched.
//!
//! SD traffic drops from `4 × N` chunk fetches per frame (the old level-major collector's four
//! priority passes) to `≤ 2 × N`. The stubs share the `slots` buffer the spans end up in (a [`Stub`]
//! fits a [`Span`] slot, asserted below), so the split costs no extra frame RAM.

use heapless::Vec;

use embedded_graphics::prelude::Point;

use obc_map_scene::{Candidate, Feature, FeatureError, FeatureToken, Kind, MapScene, ReadFailures, SelectedFeatures};

use crate::{
    viewport::{point_in_rect, segment_intersects_rect},
    RenderStats, Viewport, MAX_DECODE_POINTS, MAX_DECODE_RINGS, MAX_FRAME_POINTS, MAX_FRAME_RINGS, MAX_LINE_PX,
    MAX_SPANS,
};

/// One retained frame vertex, already projected into integer screen space.
///
/// The panel is only a few hundred pixels across, and the collector has already rejected geometry
/// that cannot affect it. Keeping the projection result instead of the source microdegrees avoids
/// doing the same projection again in the draw pass and halves the resident frame-point buffer.
/// Conversion is checked: a hostile/custom scene with a vertex outside the signed-16-bit screen
/// envelope is rejected as malformed rather than wrapping into a visible but unrelated pixel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub(crate) struct ScreenPoint {
    x: i16,
    y: i16,
}

impl ScreenPoint {
    #[inline]
    fn checked((x, y): (i32, i32)) -> Result<Self, ()> {
        Ok(Self { x: i16::try_from(x).map_err(|_| ())?, y: i16::try_from(y).map_err(|_| ())? })
    }

    #[inline]
    pub(crate) fn point(self) -> Point {
        Point::new(i32::from(self.x), i32::from(self.y))
    }

    #[inline]
    fn tuple(self) -> (i32, i32) {
        (i32::from(self.x), i32::from(self.y))
    }
}

const _: () = assert!(core::mem::size_of::<ScreenPoint>() == 4, "frame screen points must stay packed");

#[inline]
fn ray_toggle(a: (i32, i32), b: (i32, i32), point: (i32, i32)) -> bool {
    let (px, py) = (point.0 as f64, point.1 as f64);
    let (ax, ay, bx, by) = (a.0 as f64, a.1 as f64, b.0 as f64, b.1 as f64);
    (ay > py) != (by > py) && px < (bx - ax) * (py - ay) / (by - ay) + ax
}

/// Whether `b` lies on the closed screen-space segment `a..c`. Removing such a vertex is exactly
/// raster-lossless: the two projected edges cover the same segment as their replacement chord.
/// Widen before subtracting so hostile off-panel coordinates cannot overflow the predicate.
#[inline]
fn point_on_screen_segment(a: (i32, i32), b: (i32, i32), c: (i32, i32)) -> bool {
    let (ax, ay) = (i64::from(a.0), i64::from(a.1));
    let (bx, by) = (i64::from(b.0), i64::from(b.1));
    let (cx, cy) = (i64::from(c.0), i64::from(c.1));
    (bx - ax) * (cy - ay) == (by - ay) * (cx - ax) && (bx - ax) * (bx - cx) + (by - ay) * (by - cy) <= 0
}

fn ring_affects_rect(viewport: &Viewport, ring: &[(i32, i32)], rect: (i32, i32, i32, i32), probe: (i32, i32)) -> bool {
    let Some(&last) = ring.last() else { return false };
    let mut previous = viewport.to_screen(last.0, last.1);
    let mut contains_probe = false;
    for &(lon, lat) in ring {
        let current = viewport.to_screen(lon, lat);
        if point_in_rect(current, rect) || segment_intersects_rect(previous, current, rect) {
            return true;
        }
        contains_probe ^= ray_toggle(previous, current, probe);
        previous = current;
    }
    // With no boundary entering the connected rectangle, containment is uniform across it.
    contains_probe
}

/// Append a polygon after removing only vertex corners whose old two edges and replacement chord
/// all stay wholly outside the panel. The ray-parity equality makes the local replacement
/// fill-equivalent for the entire panel: with no boundary segment entering the rectangle, winding
/// parity is constant over it, so one interior probe is sufficient. This is viewport clipping's
/// useful cheap subset — no intersection vertices, topology reconstruction, or extra scratch.
fn append_visible_polygon(
    viewport: &Viewport,
    points: &[(i32, i32)],
    ring_lens: &[usize],
    margin_px: i32,
    out_points: &mut Vec<ScreenPoint, MAX_FRAME_POINTS>,
    out_ring_lens: &mut Vec<u16, MAX_FRAME_RINGS>,
) -> Result<(), ()> {
    let rect = (-margin_px, -margin_px, viewport.w as i32 + margin_px, viewport.h as i32 + margin_px);
    let probe = (viewport.w as i32 / 2, viewport.h as i32 / 2);
    let mut offset = 0usize;
    for (ring_index, &len) in ring_lens.iter().enumerate() {
        let ring = points.get(offset..offset + len).ok_or(())?;
        offset += len;
        if ring_index > 0 && !ring_affects_rect(viewport, ring, rect, probe) {
            continue;
        }
        if ring.len() <= 4 {
            for &(lon, lat) in ring {
                out_points.push(ScreenPoint::checked(viewport.to_screen(lon, lat))?).map_err(|_| ())?;
            }
            out_ring_lens.push(ring.len() as u16).map_err(|_| ())?;
            continue;
        }

        let start = out_points.len();
        out_points.push(ScreenPoint::checked(viewport.to_screen(ring[0].0, ring[0].1))?).map_err(|_| ())?;
        for index in 1..ring.len() {
            let a = out_points.last().ok_or(())?.tuple();
            let b = viewport.to_screen(ring[index].0, ring[index].1);
            let c = viewport.to_screen(ring[(index + 1) % ring.len()].0, ring[(index + 1) % ring.len()].1);
            let remaining = ring.len() - index - 1;
            let kept = out_points.len() - start;
            let screen_redundant = point_on_screen_segment(a, b, c);
            let outside_equivalent = !point_in_rect(b, rect)
                && !segment_intersects_rect(a, b, rect)
                && !segment_intersects_rect(b, c, rect)
                && !segment_intersects_rect(a, c, rect)
                && (ray_toggle(a, b, probe) ^ ray_toggle(b, c, probe)) == ray_toggle(a, c, probe);
            if (!screen_redundant && !outside_equivalent) || kept + remaining < 4 {
                out_points.push(ScreenPoint::checked(b)?).map_err(|_| ())?;
            }
        }
        out_ring_lens.push((out_points.len() - start) as u16).map_err(|_| ())?;
    }
    (offset == points.len()).then_some(()).ok_or(())
}

/// Append line rings after removing vertices that project exactly onto the segment between their
/// neighbours. Endpoints and every real projected corner remain, so solid strokes and dash length
/// are unchanged while sub-pixel source detail consumes no frame points.
fn append_visible_line(
    viewport: &Viewport,
    points: &[(i32, i32)],
    ring_lens: &[usize],
    out_points: &mut Vec<ScreenPoint, MAX_FRAME_POINTS>,
    out_ring_lens: &mut Vec<u16, MAX_FRAME_RINGS>,
) -> Result<(), ()> {
    let mut offset = 0usize;
    for &len in ring_lens {
        let ring = points.get(offset..offset + len).ok_or(())?;
        offset += len;
        let start = out_points.len();
        let Some((&first, rest)) = ring.split_first() else { return Err(()) };
        out_points.push(ScreenPoint::checked(viewport.to_screen(first.0, first.1))?).map_err(|_| ())?;
        for index in 0..rest.len().saturating_sub(1) {
            let previous = out_points.last().ok_or(())?.tuple();
            let current = viewport.to_screen(rest[index].0, rest[index].1);
            let next = viewport.to_screen(rest[index + 1].0, rest[index + 1].1);
            if !point_on_screen_segment(previous, current, next) {
                out_points.push(ScreenPoint::checked(current)?).map_err(|_| ())?;
            }
        }
        if let Some(&last) = rest.last() {
            out_points.push(ScreenPoint::checked(viewport.to_screen(last.0, last.1))?).map_err(|_| ())?;
        }
        out_ring_lens.push((out_points.len() - start) as u16).map_err(|_| ())?;
    }
    (offset == points.len()).then_some(()).ok_or(())
}

/// The renderer's collection scratch: per-feature decode buffers plus the frame buffers that
/// accumulate every visible feature's geometry (and its [`Span`]). Cleared (not freed) each frame.
#[derive(Default)]
pub(crate) struct FrameScratch {
    // Per-feature ring decode scratch handed to the scene source's two streamed passes. Point
    // decode storage is phase-shared with the projected draw buffer by `DrawScratch`.
    dec_ring_lens: Vec<usize, MAX_DECODE_RINGS>,
    // All drawn features' geometry, concatenated (filled in pass B).
    pub(crate) frame_points: Vec<ScreenPoint, MAX_FRAME_POINTS>,
    // `u16` is lossless because a decoded feature has at most `MAX_DECODE_POINTS` vertices, and
    // halves this buffer's MCU footprint versus `usize`.
    pub(crate) frame_ring_lens: Vec<u16, MAX_FRAME_RINGS>,
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
    ///
    /// `suppress_terrain` drops the whole **terrain layer** — every style carrying
    /// [`StyleFlags::terrain_layer`](obc_map_scene::StyleFlags::terrain_layer) — out of the visible
    /// mask, so pass A never even asks the source to decode those features (see below).
    pub(crate) fn collect<S: MapScene>(
        &mut self,
        scene: &S,
        lod: usize,
        viewport: &Viewport,
        dec_points: &mut Vec<(i32, i32), MAX_DECODE_POINTS>,
        suppress_terrain: bool,
        stats: &mut RenderStats,
    ) {
        self.frame_points.clear();
        self.frame_ring_lens.clear();
        self.slots.clear();
        self.spans_len = 0;

        // A single "is this style drawn at all?" mask (bit set ⇔ the id has a style), built once —
        // the old per-priority-level masks are gone: pass A decodes every drawn feature in one walk.
        //
        // The terrain-layer suppression (#1096) is applied **here**, by clearing those styles' mask
        // bits, and nowhere else: the mask is what pass A hands the source as its `should_decode`
        // filter, so a suppressed contour is skipped before its geometry is decoded, never drawn and
        // painted over. It costs no span, no point, no ring — a hidden terrain layer therefore also
        // frees frame budget for everything else, which drawing-then-overpainting would not.
        let mut vis_mask = [0u32; 8];
        for id in 0..=255u8 {
            match scene.style(id) {
                Some(style) if !(suppress_terrain && style.flags.terrain_layer()) => {
                    vis_mask[(id >> 5) as usize] |= 1 << (id & 31)
                }
                _ => {}
            }
        }

        let candidates = self.collect_stubs(scene, lod, viewport, dec_points, &vis_mask, stats);
        let winners = self.select();
        let drawn = self.decode_winners(scene, lod, viewport, dec_points, winners, stats);

        self.spans_len = drawn;
        stats.features_drawn = drawn;
        // Every candidate that passed the cull is either drawn or dropped (evicted in pass A or cut
        // by the point/ring budget in select). Culled features count in `features_tried`, not here —
        // matching the old collector, so `drawn + dropped == tried` holds when nothing is culled.
        stats.features_dropped = candidates - winners;
        stats.span_utilization = drawn as f32 / self.slots.capacity() as f32;
        stats.point_utilization = self.frame_points.len() as f32 / self.frame_points.capacity() as f32;
        stats.ring_utilization = self.frame_ring_lens.len() as f32 / self.frame_ring_lens.capacity() as f32;
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
        viewport: &Viewport,
        dec_points: &mut Vec<(i32, i32), MAX_DECODE_POINTS>,
        vis_mask: &[u32; 8],
        stats: &mut RenderStats,
    ) -> usize {
        // Split the borrow so the decode callback can push stubs while `for_each_feature_filtered`
        // borrows the decode scratch.
        let view = viewport.visible_bbox();
        let FrameScratch { dec_ring_lens, frame_points, frame_ring_lens, slots, .. } = self;
        let mut candidates = 0usize;
        let mut arrival = 0u16;
        let report = scene.visit_candidates(
            lod,
            &view,
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

                // Per-feature bbox cull (tighter than the leaf); bounds come free from decode.
                if !f.bbox().intersects(&view) {
                    return;
                }
                let margin = if f.kind == Kind::Line || style.color2.is_some() { MAX_LINE_PX as i32 } else { 0 };
                if !viewport.bbox_might_be_visible(&f.bbox(), margin) {
                    return;
                }
                if !viewport.geometry_might_be_visible(pts, f.ring_lens(), f.kind == Kind::Polygon, margin) {
                    return;
                }
                frame_points.clear();
                frame_ring_lens.clear();
                let compacted = match f.kind {
                    Kind::Line => append_visible_line(viewport, pts, f.ring_lens(), frame_points, frame_ring_lens),
                    Kind::Polygon => {
                        append_visible_polygon(viewport, pts, f.ring_lens(), margin, frame_points, frame_ring_lens)
                    }
                };
                if compacted.is_err() {
                    stats.malformed_features = stats.malformed_features.saturating_add(1);
                    return;
                }
                let (total_pts, ring_count) = (frame_points.len(), frame_ring_lens.len());
                candidates += 1;

                let level = style.priority; // 1..=4
                let stub = Stub::new(token, total_pts, ring_count, f.style_id, f.kind, level, arrival);
                arrival = arrival.saturating_add(1);

                // Streaming "keep the K lowest-keyed" selection, key = `(priority_level, arrival)`
                // (`arrival` lives in `Stub::seq` until select overwrites it). `slots` is held as an
                // in-place max-heap on that key, so the **root is the worst retained candidate**
                // (largest priority number, then latest arrival). While the buffer has room every
                // stub is admitted; once it is full a new stub replaces the root exactly when it
                // out-ranks that worst resident — the classic bounded max-heap that ends holding the
                // K smallest keys. Because a new candidate's `arrival` is the largest seen so far,
                // `key < root_key` reduces to `level < root.priority` (equal priority ⇒ equal-or-later
                // arrival ⇒ never <), i.e. it can only displace a strictly-higher-level stub, and the
                // root is precisely the highest-arrival stub at the worst level present. That is the
                // same accept/reject decision and the same victim the old level-major linear scan
                // made, so the survivor set — hence the render — is identical (see the tie note on
                // `Stub::seq` for the >65,536-candidate saturation edge, the one case outside this
                // exactness claim).
                if heap_admit(slots, stub) {
                    stats.stub_evictions += 1;
                }
            },
        );
        stats.feature_decode_capacity_drops =
            stats.feature_decode_capacity_drops.saturating_add(report.capacity_dropped);
        stats.malformed_features = stats.malformed_features.saturating_add(report.malformed_features);
        record_read_failures(stats, report.read_failures);
        stats.chunks_visited = report.chunks_visited;
        frame_points.clear();
        frame_ring_lens.clear();
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
            let pts = s.total_pts() as usize;
            let rings = s.ring_count() as usize;
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
        viewport: &Viewport,
        dec_points: &mut Vec<(i32, i32), MAX_DECODE_POINTS>,
        winners: usize,
        stats: &mut RenderStats,
    ) -> usize {
        if winners == 0 {
            return 0;
        }
        let FrameScratch { dec_ring_lens, frame_points, frame_ring_lens, slots, .. } = self;
        // A winner slot, once rewritten to its `Span`, must not be re-read as a stub by a later
        // chunk's scan. `placed` marks the done slots so the scan skips them.
        let mut placed = [0u32; MAX_SPANS.div_ceil(32)];
        let mut selected = DecodeSink {
            scene,
            viewport,
            winners,
            frame_points,
            frame_ring_lens,
            slots,
            placed: &mut placed,
            stats,
            drawn: 0,
        };
        let report = scene.decode_selected(lod, &viewport.visible_bbox(), dec_points, dec_ring_lens, &mut selected);
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
    viewport: &'a Viewport,
    winners: usize,
    frame_points: &'a mut Vec<ScreenPoint, MAX_FRAME_POINTS>,
    frame_ring_lens: &'a mut Vec<u16, MAX_FRAME_RINGS>,
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
        let Some(style) = self.scene.style(stub.style_id()) else {
            self.finish_error(index, FeatureError::Malformed);
            return false;
        };
        if !feature.has_valid_rings() || feature.style_id != stub.style_id() || feature.kind != stub.kind() {
            self.finish_error(index, FeatureError::Malformed);
            return false;
        }
        if stub.total_pts() as usize > self.frame_points.capacity() - self.frame_points.len() {
            self.finish_error(index, FeatureError::Capacity(obc_map_scene::CapacityError::Points));
            return false;
        }
        if stub.ring_count() as usize > self.frame_ring_lens.capacity() - self.frame_ring_lens.len() {
            self.finish_error(index, FeatureError::Capacity(obc_map_scene::CapacityError::Rings));
            return false;
        }

        let pt_start = self.frame_points.len() as u16;
        let ring_start = self.frame_ring_lens.len() as u16;
        let published = match feature.kind {
            Kind::Line => append_visible_line(
                self.viewport,
                feature.points(),
                feature.ring_lens(),
                self.frame_points,
                self.frame_ring_lens,
            )
            .is_ok(),
            Kind::Polygon => {
                let margin = if style.color2.is_some() { MAX_LINE_PX as i32 } else { 0 };
                append_visible_polygon(
                    self.viewport,
                    feature.points(),
                    feature.ring_lens(),
                    margin,
                    self.frame_points,
                    self.frame_ring_lens,
                )
                .is_ok()
            }
        };
        if !published
            || self.frame_points.len() - pt_start as usize != stub.total_pts() as usize
            || self.frame_ring_lens.len() - ring_start as usize != stub.ring_count() as usize
        {
            // Pass A and B run the same deterministic compaction. A changed/corrupt refetch must
            // not publish a partial feature into the shared frame buffers.
            self.frame_points.truncate(pt_start as usize);
            self.frame_ring_lens.truncate(ring_start as usize);
            self.finish_error(index, FeatureError::Malformed);
            return false;
        }
        self.drawn += 1;
        self.stats.points_drawn += stub.total_pts() as usize;
        // The by-kind scratch split, counted where the geometry is published rather than by a second
        // walk over the finished spans: which render path — lines or polygons — eats the
        // span/point/ring budget is what the sim's scratch panel shows, and every span that survives
        // to `spans()` is published exactly here (a failed refetch lands in `finish_error` with
        // `ring_count == 0` and is compacted away), so counting here is free and identical.
        // `has_valid_rings` above makes a feature's point count the sum of its ring lengths.
        match feature.kind {
            Kind::Line => {
                self.stats.line_spans += 1;
                self.stats.line_rings += stub.ring_count() as usize;
                self.stats.line_points += stub.total_pts() as usize;
            }
            Kind::Polygon => {
                self.stats.poly_spans += 1;
                self.stats.poly_rings += stub.ring_count() as usize;
                self.stats.poly_points += stub.total_pts() as usize;
            }
        }
        let span = Span {
            kind: feature.kind,
            z: style.z_index,
            weight: style.weight,
            style_id: stub.style_id(),
            pt_start,
            ring_start,
            ring_count: stub.ring_count(),
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
        let (z, weight) = self.scene.style(stub.style_id()).map_or((0, 0), |style| (style.z_index, style.weight));
        let span = empty_span(stub, z, weight, self.frame_points.len() as u16, self.frame_ring_lens.len() as u16);
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
fn empty_span(stub: Stub, z: i8, weight: u8, pt_start: u16, ring_start: u16) -> Span {
    Span { kind: Kind::Line, z, weight, style_id: stub.style_id(), pt_start, ring_start, ring_count: 0, seq: stub.seq }
}

/// The max-heap ordering key of a stub slot: `(priority_level, arrival)`, read from the live `stub`
/// variant. Larger = worse = closer to the root. Valid only during pass A (before select rewrites
/// `seq`), where every live slot still holds a [`Stub`] and `seq` still carries the pass-A arrival.
#[inline]
fn stub_key(stub: &Stub) -> (u8, u16) {
    (stub.priority(), stub.seq)
}

/// Streaming bounded-max-heap admit — the whole pass-A insertion rule in one allocation-free step.
/// While the buffer has room, `stub` is pushed and sifted up. Once full, it replaces the root (the
/// worst retained candidate) exactly when its key is strictly smaller, then sifts the new root down;
/// otherwise it is rejected. Returns `true` iff it evicted a resident. Const-generic over the buffer
/// capacity so the tests can drive the identical logic at tiny K.
fn heap_admit<const N: usize>(slots: &mut Vec<Slot, N>, stub: Stub) -> bool {
    if !slots.is_full() {
        // `push` cannot fail here (checked not-full); index the fresh tail for sift-up.
        let _ = slots.push(Slot::of_stub(stub));
        sift_up(slots, slots.len() - 1);
        false
    } else if stub_key(&stub) < stub_key(&slots[0].stub()) {
        slots[0] = Slot::of_stub(stub);
        sift_down(slots, 0);
        true
    } else {
        false
    }
}

/// Restore the max-heap property after `slots[i]` was inserted at a leaf: bubble it toward the root
/// while it out-keys its parent. Allocation-free; compares stubs through [`Slot::stub`].
fn sift_up<const N: usize>(slots: &mut Vec<Slot, N>, mut i: usize) {
    while i > 0 {
        let parent = (i - 1) / 2;
        if stub_key(&slots[i].stub()) > stub_key(&slots[parent].stub()) {
            slots.swap(i, parent);
            i = parent;
        } else {
            break;
        }
    }
}

/// Restore the max-heap property after `slots[i]` (usually the root) was replaced with a smaller-keyed
/// stub: sink it toward the leaves, swapping with its larger-keyed child until it dominates both.
/// Allocation-free; compares stubs through [`Slot::stub`].
fn sift_down<const N: usize>(slots: &mut Vec<Slot, N>, mut i: usize) {
    let len = slots.len();
    loop {
        let left = 2 * i + 1;
        let right = 2 * i + 2;
        let mut largest = i;
        if left < len && stub_key(&slots[left].stub()) > stub_key(&slots[largest].stub()) {
            largest = left;
        }
        if right < len && stub_key(&slots[right].stub()) > stub_key(&slots[largest].stub()) {
            largest = right;
        }
        if largest == i {
            break;
        }
        slots.swap(i, largest);
        i = largest;
    }
}

/// A pass-A candidate: exactly what selection and the pass-B re-decode need, and nothing else —
/// never geometry. Sized to fit a [`Span`] slot (asserted below) so the whole candidate set lives in
/// the same `slots` buffer pass B rewrites in place: the split into stubs + spans costs no extra
/// frame RAM (issue #564). The six-byte source token stays opaque to the renderer.
#[derive(Clone, Copy)]
#[repr(C)]
struct Stub {
    /// All-rings point count (12 bits), ring count (6), priority (3), kind (1), and style id (8).
    /// Their production maxima fit in 30 bits; packing them is what lets the candidate reservoir
    /// grow without making the render arm larger.
    meta: u32,
    token: FeatureToken,
    /// Pass-A encounter index (level-major seq replication), overwritten with the admission `seq`
    /// during select — pass B reads it for the painter's-order tie-break. It is the low half of the
    /// pass-A max-heap key and is assigned with `saturating_add`, so the first 65,536 candidates
    /// (arrivals `0..=u16::MAX`) get **distinct** keys and the heap's survivor set — identities and
    /// all — is provably the exact set the old linear victim scan kept. Only a viewport with **more
    /// than 65,536** pass-A candidates pins later arrivals at `u16::MAX`, and only then can two
    /// worst-level residents share a key; the heap then still makes the identical accept/reject
    /// decision and keeps the identical multiset of keys, but which of the tied-key features it
    /// evicts is fixed by heap order rather than by the old buffer's slot order. That regime is far
    /// beyond `MAX_SPANS` (3,072) and never reached by the OBCM source at its coarsest LOD; the
    /// exactness claim and the reference-selector tests are scoped to it. See `collect_stubs`.
    seq: u16,
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
        debug_assert!(total_pts <= 0x0fff && ring_count <= 0x3f && priority <= 0x7);
        let meta = total_pts as u32
            | (ring_count as u32) << 12
            | (priority as u32) << 18
            | kind_bit << 21
            | (style_id as u32) << 22;
        Stub { token, meta, seq: arrival }
    }

    #[inline]
    fn total_pts(self) -> u16 {
        (self.meta & 0x0fff) as u16
    }

    #[inline]
    fn ring_count(self) -> u16 {
        ((self.meta >> 12) & 0x3f) as u16
    }

    #[inline]
    fn priority(self) -> u8 {
        ((self.meta >> 18) & 0x7) as u8
    }

    #[inline]
    fn kind(self) -> Kind {
        if self.meta & (1 << 21) == 0 {
            Kind::Line
        } else {
            Kind::Polygon
        }
    }

    #[inline]
    fn style_id(self) -> u8 {
        (self.meta >> 22) as u8
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
// a size (or the `slots`-as-`[Span]` reinterpret in `spans()` / the `MCU_SCRATCH_BYTES` accounting
// would be wrong).
const _: () = assert!(core::mem::size_of::<Stub>() <= core::mem::size_of::<Span>(), "Stub must fit a Span slot");
const _: () = assert!(core::mem::size_of::<Slot>() == core::mem::size_of::<Span>(), "Slot must be Span-sized");

/// One visible feature's draw metadata plus the ranges locating its geometry in the frame buffers.
/// Cheap to sort for the painter's algorithm.
///
/// Offsets are `u16` (not `usize`) and the draw path resolves the primary color from `style_id`,
/// keeping the struct to 12 bytes — thousands are buffered at
/// coarse zoom. The frame buffers they index are asserted `<= u16::MAX` at the buffer constants.
/// The draw loop re-resolves the full scene style — primary color, `dashed`, and `color2` — via the
/// source's hot `O(1)` style table.
#[derive(Clone, Copy)]
pub(crate) struct Span {
    pub(crate) kind: Kind,
    pub(crate) z: i8,
    pub(crate) weight: u8,
    pub(crate) style_id: u8,
    pub(crate) pt_start: u16,
    pub(crate) ring_start: u16,
    pub(crate) ring_count: u16,
    pub(crate) seq: u16,
}

// `style_id` must land in the spare byte, not grow the struct — thousands are buffered per frame and
// `MCU_SCRATCH_BYTES` budgets `MAX_SPANS * size_of::<Span>()`.
const _: () = assert!(core::mem::size_of::<Span>() == 12, "Span must stay 12 bytes");

#[cfg(test)]
mod viewport_compaction_tests {
    use super::*;
    use crate::fill::{fill_polygon, fill_polygon_proj};
    use crate::{MAX_CROSSINGS, MAX_SCREEN_POINTS};
    use embedded_graphics::mock_display::MockDisplay;
    use embedded_graphics::pixelcolor::BinaryColor;
    use embedded_graphics::prelude::Point;

    fn map_ring(viewport: &Viewport, screen: &[(i32, i32)]) -> std::vec::Vec<(i32, i32)> {
        screen.iter().map(|&(x, y)| viewport.to_map(x as f32, y as f32)).collect()
    }

    fn draw_compacted(
        target: &mut MockDisplay<BinaryColor>,
        viewport: &Viewport,
        points: &[ScreenPoint],
        ring_lens: &[u16],
        screen: &mut Vec<Point, MAX_SCREEN_POINTS>,
        xs: &mut Vec<f32, MAX_CROSSINGS>,
    ) {
        screen.clear();
        for p in points {
            screen.push(p.point()).unwrap();
        }
        fill_polygon(target, screen, ring_lens, BinaryColor::On, viewport.w as i32, viewport.h as i32, xs);
    }

    #[test]
    fn offscreen_chains_and_holes_compact_without_changing_pixels() {
        let viewport = Viewport::new(64.0, 64.0, 0, 0, 1.0);
        let exterior = map_ring(
            &viewport,
            &[
                (-80, -70),
                (-50, -95),
                (-10, -85),
                (70, -90),
                (130, -60),
                (130, 40),
                (70, 40),
                (64, 80),
                (0, 80),
                (-40, 40),
            ],
        );
        let visible_hole = map_ring(&viewport, &[(22, 22), (42, 22), (42, 42), (22, 42)]);
        let irrelevant_hole = map_ring(&viewport, &[(100, 100), (120, 100), (120, 120), (100, 120)]);
        let mut points = exterior.clone();
        points.extend_from_slice(&visible_hole);
        points.extend_from_slice(&irrelevant_hole);
        let ring_lens = [exterior.len(), visible_hole.len(), irrelevant_hole.len()];

        let mut compact_points: Vec<ScreenPoint, MAX_FRAME_POINTS> = Vec::new();
        let mut compact_rings: Vec<u16, MAX_FRAME_RINGS> = Vec::new();
        append_visible_polygon(&viewport, &points, &ring_lens, 0, &mut compact_points, &mut compact_rings).unwrap();
        assert!(compact_points.len() < points.len());
        assert_eq!(compact_rings.len(), 2, "the wholly offscreen hole consumes no frame ring");

        let mut before = MockDisplay::<BinaryColor>::new();
        let mut after = MockDisplay::<BinaryColor>::new();
        let mut screen: Vec<Point, MAX_SCREEN_POINTS> = Vec::new();
        let mut xs: Vec<f32, MAX_CROSSINGS> = Vec::new();
        fill_polygon_proj(&mut before, &viewport, &points, &ring_lens, BinaryColor::On, &mut screen, &mut xs);
        draw_compacted(&mut after, &viewport, &compact_points, &compact_rings, &mut screen, &mut xs);
        assert_eq!(before, after, "viewport compaction must be framebuffer-identical");
    }

    #[test]
    fn projected_collinear_vertices_compact_without_changing_pixels() {
        let viewport = Viewport::new(64.0, 64.0, 0, 0, 1.0);
        let points = map_ring(&viewport, &[(5, 5), (15, 5), (25, 5), (40, 5), (40, 40), (5, 40)]);
        let ring_lens = [points.len()];
        let mut compact_points: Vec<ScreenPoint, MAX_FRAME_POINTS> = Vec::new();
        let mut compact_rings: Vec<u16, MAX_FRAME_RINGS> = Vec::new();
        append_visible_polygon(&viewport, &points, &ring_lens, 0, &mut compact_points, &mut compact_rings).unwrap();
        assert_eq!(compact_points.len(), 4, "the straight projected edge needs only its endpoints");

        let mut before = MockDisplay::<BinaryColor>::new();
        let mut after = MockDisplay::<BinaryColor>::new();
        let mut screen: Vec<Point, MAX_SCREEN_POINTS> = Vec::new();
        let mut xs: Vec<f32, MAX_CROSSINGS> = Vec::new();
        fill_polygon_proj(&mut before, &viewport, &points, &ring_lens, BinaryColor::On, &mut screen, &mut xs);
        draw_compacted(&mut after, &viewport, &compact_points, &compact_rings, &mut screen, &mut xs);
        assert_eq!(before, after, "screen-collinear compaction must be framebuffer-identical");
    }

    #[test]
    fn projected_collinear_line_vertices_keep_endpoints_and_corners() {
        let viewport = Viewport::new(64.0, 64.0, 0, 0, 1.0);
        let points = map_ring(&viewport, &[(5, 5), (15, 5), (25, 5), (40, 5), (40, 25), (40, 40)]);
        let mut compact_points: Vec<ScreenPoint, MAX_FRAME_POINTS> = Vec::new();
        let mut compact_rings: Vec<u16, MAX_FRAME_RINGS> = Vec::new();
        append_visible_line(&viewport, &points, &[points.len()], &mut compact_points, &mut compact_rings).unwrap();
        let projected: std::vec::Vec<_> = compact_points.iter().map(|p| p.tuple()).collect();
        assert_eq!(projected, [(5, 5), (40, 5), (40, 40)]);
        assert_eq!(compact_rings.as_slice(), &[3]);
    }

    #[test]
    fn a_projection_outside_i16_is_rejected_instead_of_wrapping() {
        let viewport = Viewport::new(64.0, 64.0, 0, 0, 1.0);
        let points = [(40_000, 0), (0, 0)];
        let mut compact_points: Vec<ScreenPoint, MAX_FRAME_POINTS> = Vec::new();
        let mut compact_rings: Vec<u16, MAX_FRAME_RINGS> = Vec::new();
        assert!(
            append_visible_line(&viewport, &points, &[points.len()], &mut compact_points, &mut compact_rings,).is_err()
        );
    }
}

#[cfg(test)]
mod heap_tests {
    //! Unit tests for the pass-A bounded max-heap victim selection ([`heap_admit`] / [`sift_up`] /
    //! [`sift_down`]). They drive the exact production insertion at tiny capacities and check its
    //! survivor set — keys *and* identity tokens — plus its eviction count against two independent
    //! references:
    //!
    //! * a **spec reference selector** (collect every candidate, stable-sort by `(priority, arrival)`,
    //!   `truncate(K)`) — proves the heap keeps the K lowest keys, and
    //! * a **replica of the old linear victim scan** the heap replaces — proves the heap is
    //!   identical to the previous collector down to which stub each eviction drops (the byte-for-byte
    //!   render-equivalence guarantee) and to `stats.stub_evictions`.
    //!
    //! Every test stream stays well under 65,536 candidates, so all arrivals — hence all keys — are
    //! distinct and the survivor identity is uniquely determined (see the `Stub::seq` saturation note).

    extern crate std;
    use std::vec::Vec;

    use obc_map_scene::{FeatureToken, Kind};

    use super::{heap_admit, stub_key, Slot, Stub};

    /// A synthetic pass-A stub: `priority` is the heap key's high half, `arrival` its low half, and
    /// `token` an independent bijection of `arrival` stashed in the opaque source token so the tests
    /// can confirm the heap moves *identities* around, not just keys. Geometry fields are irrelevant
    /// to selection and left zero.
    fn test_stub(priority: u8, arrival: u16, token: u16) -> Stub {
        Stub::new(FeatureToken::from_source_words([token, 0, 0]), 0, 0, 0, Kind::Line, priority, arrival)
    }

    /// Token bijection: distinct from `arrival` so a test that checks the token is really checking the
    /// carried identity, not accidentally re-deriving the key.
    fn token_of(arrival: u16) -> u16 {
        arrival.wrapping_mul(2654).wrapping_add(3)
    }

    /// A survivor as `(priority, arrival, token)` — the full identity the render depends on.
    type Survivor = (u8, u16, u16);

    /// Drive the production heap over a stream of priorities (arrival = stream index) at capacity `N`.
    /// Returns the survivors sorted by `(priority, arrival)` and the eviction count.
    fn run_heap<const N: usize>(priorities: &[u8]) -> (Vec<Survivor>, u32) {
        let mut slots: super::Vec<Slot, N> = super::Vec::new();
        let mut evictions = 0u32;
        for (arrival, &priority) in priorities.iter().enumerate() {
            let arrival = arrival as u16;
            let admitted_full = heap_admit(&mut slots, test_stub(priority, arrival, token_of(arrival)));
            if admitted_full {
                evictions += 1;
            }
            // The max-heap invariant must hold after every single admit.
            assert!(is_max_heap(&slots), "heap property violated after arrival {arrival}");
        }
        let mut out: Vec<Survivor> = slots
            .iter()
            .map(|s| {
                let st = s.stub();
                (st.priority(), st.seq, st.token.source_words()[0])
            })
            .collect();
        out.sort_by_key(|&(p, a, _)| (p, a));
        (out, evictions)
    }

    /// True iff `slots` satisfies the max-heap property under `stub_key`.
    fn is_max_heap<const N: usize>(slots: &super::Vec<Slot, N>) -> bool {
        (1..slots.len()).all(|i| stub_key(&slots[i].stub()) <= stub_key(&slots[(i - 1) / 2].stub()))
    }

    /// Spec reference: every candidate, stable-sorted by `(priority, arrival)`, truncated to K. The
    /// definition of "the K lowest keys". Uses a *stable* sort so same-priority order stays arrival
    /// order — matching the heap's earlier-arrival-wins tie rule.
    fn reference_select(priorities: &[u8], k: usize) -> Vec<Survivor> {
        let mut all: Vec<Survivor> =
            priorities.iter().enumerate().map(|(i, &p)| (p, i as u16, token_of(i as u16))).collect();
        all.sort_by_key(|&(p, a, _)| (p, a)); // stable
        all.truncate(k);
        all
    }

    /// Replica of the removed linear victim scan (the `level_count` + `worst_level` + highest-arrival
    /// first-index eviction). Returns survivors sorted by `(priority, arrival)` and the eviction count
    /// so the heap can be checked against the exact prior behaviour it must reproduce.
    fn reference_linear(priorities: &[u8], k: usize) -> (Vec<Survivor>, u32) {
        let mut buf: Vec<Survivor> = Vec::new();
        let mut level_count = [0u32; 4];
        let mut evictions = 0u32;
        for (arrival, &p) in priorities.iter().enumerate() {
            let arrival = arrival as u16;
            let cand = (p, arrival, token_of(arrival));
            if buf.len() < k {
                buf.push(cand);
                level_count[(p - 1) as usize] += 1;
                continue;
            }
            let worst = (1..=4u8).rev().find(|&l| level_count[(l - 1) as usize] > 0).unwrap_or(4);
            if p < worst {
                // Highest-arrival stub at the worst level, ties broken by lowest slot index.
                let mut victim = 0usize;
                let mut va = 0u16;
                let mut found = false;
                for (i, &(hp, ha, _)) in buf.iter().enumerate() {
                    if hp == worst && (!found || ha > va) {
                        victim = i;
                        va = ha;
                        found = true;
                    }
                }
                buf[victim] = cand;
                level_count[(worst - 1) as usize] -= 1;
                level_count[(p - 1) as usize] += 1;
                evictions += 1;
            }
        }
        buf.sort_by_key(|&(p, a, _)| (p, a));
        (buf, evictions)
    }

    /// Assert the heap matches *both* references at capacity `N`.
    fn check<const N: usize>(priorities: &[u8]) {
        let (heap, heap_evictions) = run_heap::<N>(priorities);
        let (linear, linear_evictions) = reference_linear(priorities, N);
        let spec = reference_select(priorities, N);
        assert_eq!(heap, spec, "heap survivors != K lowest keys (N={N}, len={})", priorities.len());
        assert_eq!(heap, linear, "heap survivors != old linear scan (N={N}, len={})", priorities.len());
        assert_eq!(heap_evictions, linear_evictions, "eviction count != old linear scan (N={N})");
        assert_eq!(heap.len(), priorities.len().min(N), "survivor count is min(stream, K)");
    }

    #[test]
    fn empty_stream() {
        check::<8>(&[]);
    }

    #[test]
    fn under_capacity_keeps_everything() {
        check::<16>(&[1, 4, 2, 3, 1, 4]);
    }

    #[test]
    fn exactly_full_keeps_everything() {
        check::<6>(&[3, 1, 4, 2, 4, 1]);
    }

    #[test]
    fn heavily_over_capacity_all_priorities_and_ties() {
        // 40 candidates cycling all four priorities with repeats (ties within a level) into K=8.
        let stream: Vec<u8> = (0..40u32).map(|i| (i % 4) as u8 + 1).collect();
        check::<8>(&stream);
        // A different tie pattern: blocks of equal priority.
        let blocks: Vec<u8> = [1u8, 1, 1, 4, 4, 4, 4, 2, 2, 3, 3, 3, 1, 1, 4, 4, 2, 3, 3, 3].to_vec();
        check::<5>(&blocks);
    }

    #[test]
    fn late_priority_one_displaces_priority_four() {
        // Fill K=4 with priority-4 stubs, then stream priority-1 stubs late: each must evict a p4,
        // and after four of them the buffer is all priority-1 — the priority guarantee under
        // saturation.
        let mut stream = std::vec![4u8, 4, 4, 4];
        stream.extend_from_slice(&[1, 1, 1, 1]);
        let (heap, evictions) = run_heap::<4>(&stream);
        assert_eq!(evictions, 4, "each late priority-1 evicts a priority-4");
        assert!(heap.iter().all(|&(p, ..)| p == 1), "buffer is all priority-1 after four displacements");
        // Survivors are exactly the four priority-1 arrivals (indices 4..=7).
        let arrivals: Vec<u16> = heap.iter().map(|&(_, a, _)| a).collect();
        assert_eq!(arrivals, std::vec![4, 5, 6, 7]);
        check::<4>(&stream);
    }

    #[test]
    fn same_priority_later_never_evicts_earlier() {
        // All one priority: once full, no later same-priority arrival may displace an earlier one.
        let stream: Vec<u8> = std::vec![2u8; 20];
        let (heap, evictions) = run_heap::<5>(&stream);
        assert_eq!(evictions, 0, "equal-key candidates are all rejected once full");
        let arrivals: Vec<u16> = heap.iter().map(|&(_, a, _)| a).collect();
        assert_eq!(arrivals, std::vec![0, 1, 2, 3, 4], "the first-arriving five survive");
        check::<5>(&stream);
    }

    #[test]
    fn small_capacities_const_generic() {
        // The same over-capacity stream through a spread of tiny K, exercising the const-generic
        // helper the spec calls for.
        let stream: Vec<u8> = (0..64u32).map(|i| ((i * 7 + 2) % 4) as u8 + 1).collect();
        check::<1>(&stream);
        check::<2>(&stream);
        check::<3>(&stream);
        check::<4>(&stream);
        check::<7>(&stream);
        check::<16>(&stream);
        check::<64>(&stream);
    }

    #[test]
    fn deterministic_pseudo_random_streams() {
        // A cheap LCG over several seeds and lengths — the heap must track the references exactly on
        // every one. Deterministic, so a failure reproduces.
        for seed in [1u64, 2, 7, 42, 1234, 987_654_321] {
            let mut state = seed;
            let mut next = || {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (state >> 33) as u32
            };
            let len = 200 + (seed as usize % 300);
            let stream: Vec<u8> = (0..len).map(|_| (next() % 4) as u8 + 1).collect();
            check::<8>(&stream);
            check::<32>(&stream);
            check::<3>(&stream);
        }
    }
}
