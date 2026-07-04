//! Visible-feature collection: fills the frame buffers with every visible feature's
//! geometry plus its [`Span`], in strict global priority order.

use heapless::Vec;

use obc_reader::{BBox, Kind, Reader};

use crate::{RenderStats, MAX_DECODE_POINTS, MAX_DECODE_RINGS, MAX_FRAME_POINTS, MAX_FRAME_RINGS, MAX_SPANS};

/// The renderer's collection scratch: per-feature decode buffers plus the frame buffers that
/// accumulate every visible feature's geometry (and its [`Span`]). Cleared (not freed) each frame.
#[derive(Default)]
pub(crate) struct FrameScratch {
    // Per-feature decode scratch handed to `Reader::for_each_feature_filtered`.
    dec_points: Vec<(i32, i32), MAX_DECODE_POINTS>,
    dec_ring_lens: Vec<usize, MAX_DECODE_RINGS>,
    // All visible features' geometry, concatenated, plus per-feature spans.
    pub(crate) frame_points: Vec<(i32, i32), MAX_FRAME_POINTS>,
    pub(crate) frame_ring_lens: Vec<usize, MAX_FRAME_RINGS>,
    pub(crate) spans: Vec<Span, MAX_SPANS>,
}

impl FrameScratch {
    /// Fill the frame buffers with every visible feature, in strict global priority order. One pass
    /// per priority level (the format stores a 2-bit level, 1..=4), lowest first: each pass fills
    /// every visible feature at that level across *all* chunks before the next runs, so on buffer
    /// saturation the dropped features are always the lowest priority regardless of chunk. Each
    /// feature matches one level, so its coordinates decode at most once per frame.
    pub(crate) fn collect(&mut self, reader: &Reader, lod: usize, view: &BBox, stats: &mut RenderStats) {
        self.frame_points.clear();
        self.frame_ring_lens.clear();
        self.spans.clear();

        for level in 1..=4u8 {
            self.collect_level(reader, lod, level, view, stats);
        }

        stats.span_utilization = self.spans.len() as f32 / self.spans.capacity() as f32;
        stats.point_utilization = self.frame_points.len() as f32 / self.frame_points.capacity() as f32;
        stats.ring_utilization = self.frame_ring_lens.len() as f32 / self.frame_ring_lens.capacity() as f32;
    }

    /// Append every visible feature whose style is at priority `level` to the frame buffers.
    /// Streams the viewport's leaves via [`Reader::for_each_chunk`] (no chunk cap) and decodes only
    /// this level's features. The leaf walk reads only the index, so the per-level re-walk is cheap.
    fn collect_level(&mut self, reader: &Reader, lod: usize, level: u8, view: &BBox, stats: &mut RenderStats) {
        // Split the borrow so the decode callback can fill `frame_*`/`spans` while
        // `for_each_feature_filtered` borrows the decode scratch.
        let FrameScratch { dec_points, dec_ring_lens, frame_points, frame_ring_lens, spans } = self;
        let mut chunks = 0usize;
        reader.for_each_chunk(lod, view, |cid, node| {
            chunks += 1;
            reader.for_each_feature_filtered(
                lod,
                cid,
                &node,
                dec_points,
                dec_ring_lens,
                |sid| reader.style(sid).is_some_and(|s| s.priority == level),
                |f| {
                    let style = match reader.style(f.style_id) {
                        Some(s) => s,
                        None => return,
                    };

                    let pts = f.points();
                    let lens = f.ring_lens();

                    stats.features_tried += 1;
                    stats.points_tried += pts.len();

                    // Per-feature bbox cull (tighter than the leaf); bounds come free from decode.
                    if pts.is_empty() || !f.bbox().intersects(view) {
                        return;
                    }

                    if spans.is_full()
                        || frame_points.capacity() - frame_points.len() < pts.len()
                        || frame_ring_lens.capacity() - frame_ring_lens.len() < lens.len()
                    {
                        stats.features_dropped += 1;
                        return;
                    }

                    stats.features_drawn += 1;
                    stats.points_drawn += pts.len();

                    // Casts safe: the capacity check guarantees room, buffer sizes asserted
                    // `<= u16::MAX` at the constants.
                    let _ = spans.push(Span {
                        kind: f.kind,
                        z: style.z_index,
                        weight: style.weight,
                        color: style.color,
                        pt_start: frame_points.len() as u16,
                        ring_start: frame_ring_lens.len() as u16,
                        ring_count: lens.len() as u16,
                        seq: spans.len() as u16,
                    });
                    let _ = frame_points.extend_from_slice(pts);
                    let _ = frame_ring_lens.extend_from_slice(lens);
                },
            );
        });
        // Visible-chunk count; identical across levels, so record it once.
        if level == 1 {
            stats.chunks_visited = chunks;
        }
    }
}

/// One visible feature's draw metadata plus the ranges locating its geometry in the frame buffers.
/// Cheap to sort for the painter's algorithm.
///
/// Offsets are `u16` (not `usize`) to keep the struct to 14 bytes — thousands are buffered at
/// coarse zoom. The frame buffers they index are asserted `<= u16::MAX` at the buffer constants.
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
