//! Shared map renderer, generic over `embedded-graphics`' [`DrawTarget`], so the host simulator
//! and the device share one projection, LOD selection, painter ordering and rasterization.
//!
//! [`RenderScratch`] clears its per-frame buffers instead of freeing them, so steady-state
//! rendering allocates nothing. What a frame looks like is decided by the per-call
//! [`RenderConfig`]; no sticky setting lives in the scratch.

#![no_std]

use core::mem::ManuallyDrop;

use heapless::Vec;

use embedded_graphics::prelude::*;

use obc_map_scene::{Diagnostics, Kind, LineStyle, MapScene, ReadError};

pub mod canvas;
mod collect;
mod fill;
mod font_data;
mod overlay;

mod stroke;
pub mod surface;
pub mod text;
#[cfg(feature = "text-tap")]
pub mod text_tap;
mod viewport;
pub use canvas::{rect, Canvas};
pub use overlay::{OverlayChunk, RouteOverlaySource};

pub use surface::Surface;
pub use text::{draw_text, glyph_supported, text_width, Font, TextAlign};
pub use viewport::{mpp_for_zoom, round_coord, zoom_for_mpp, Viewport};

use collect::{FrameScratch, ScreenPoint, Span};
use fill::{fill_polygon_edges, PackedEdge};
use stroke::{draw_line, Stroker};

// Per-frame buffer capacities, statically allocated. Growing one costs boot RAM. One profile
// serves device, simulator and tests, so the simulator drops features at the same zooms the
// device does. The board crate's resident-set assert is the binding fit check.

/// Capacity of pass-A's candidate reservoir. Its surplus over a selected frame buys backfill: a
/// lower-priority candidate can still take a slot after a large feature is skipped on the point
/// or ring budget.
pub const MAX_SPANS: usize = 3072;

/// Maximum retained vertices per frame, as projected signed-16-bit screen coordinates.
pub const MAX_FRAME_POINTS: usize = 16323;

/// Maximum ring entries per frame. Every admitted feature costs at least one ring, so no frame
/// draws more than `min(MAX_SPANS, MAX_FRAME_RINGS)` features. `ring_count == 0` is pass-B's
/// failure sentinel.
pub const MAX_FRAME_RINGS: usize = 3328;

/// Maximum vertices for one feature during decode. Equals the largest feature an OBCM file holds.
pub const MAX_DECODE_POINTS: usize = 2048;

pub const MAX_DECODE_RINGS: usize = 32;

/// Maximum screen points buffered while drawing one feature. It must hold a whole decode buffer,
/// asserted below.
pub const MAX_SCREEN_POINTS: usize = 2048;

/// Maximum scanline crossings for one polygon-fill row. A row with more is skipped, not mis-filled.
pub const MAX_CROSSINGS: usize = 384;

const _: () = assert!(MAX_FRAME_POINTS <= u16::MAX as usize, "Span::pt_start is u16");
const _: () = assert!(MAX_FRAME_RINGS <= u16::MAX as usize, "Span::ring_start is u16");
const _: () = assert!(MAX_SPANS <= u16::MAX as usize, "Span::seq is u16");
const _: () = assert!(
    MAX_FRAME_RINGS >= MAX_SPANS,
    "every feature costs one span and >=1 ring; a ring cap below the span cap makes MAX_SPANS unreachable dead weight"
);

const _: () = assert!(MAX_SCREEN_POINTS >= MAX_DECODE_POINTS, "`screen` must hold a whole decoded feature");

/// Static RAM the scratch buffers take on the 32-bit MCU target. `pub` so a board crate's budget
/// assert can use it without re-deriving the formula.
pub const MCU_SCRATCH_BYTES: usize = MAX_DECODE_POINTS * 8
    + MAX_DECODE_RINGS * 4
    + MAX_FRAME_POINTS * core::mem::size_of::<ScreenPoint>()
    + MAX_FRAME_RINGS * 2
    + MAX_SPANS * core::mem::size_of::<Span>()
    + MAX_CROSSINGS * 4;
// Loose per-crate ceiling; the board crate's resident-set assert is the binding check.
const _: () = assert!(MCU_SCRATCH_BYTES <= 200 * 1024, "RenderScratch exceeds the 200 KB MCU budget");

/// Ground scale at which a style's `weight` is its nominal pixel width. Set at mid-riding zoom.
const REF_MPP: f32 = 10.0;

/// Exponent of the zoom→width ramp. `1.0` fails at both ends: every road sub-pixel zoomed out, a
/// motorway across the panel zoomed in.
const WIDTH_GAMMA: f32 = 0.6;

/// Upper clamp on a ramped stroke, in px. The lower clamp is 1 px.
const MAX_LINE_PX: u32 = 12;

/// Per-frame width multiplier: `(REF_MPP / mpp) ^ WIDTH_GAMMA`, computed once per frame.
#[inline]
pub(crate) fn width_scale(mpp: f32) -> f32 {
    libm::powf(REF_MPP / mpp.max(f32::MIN_POSITIVE), WIDTH_GAMMA)
}

/// `weight` in on-screen px, rounded and clamped. Integer rounding stops shimmer while zooming.
#[inline]
pub(crate) fn scale_weight(weight: u8, scale: f32) -> u32 {
    (libm::roundf(weight as f32 * scale) as i32).clamp(1, MAX_LINE_PX as i32) as u32
}

/// A line span's stroke width in px: [`scale_weight`] for an ordinary style, the authored
/// `weight` verbatim for a fixed-width one. A mark on the map has no ground width, so for it the
/// ramp is backwards. Both are clamped to `1..=MAX_LINE_PX`.
#[inline]
pub(crate) fn line_px(weight: u8, scale: f32, fixed_width: bool) -> u32 {
    if fixed_width {
        (weight as u32).clamp(1, MAX_LINE_PX)
    } else {
        scale_weight(weight, scale)
    }
}

/// Decode and projected points are phase-exclusive, and both are two `i32`s, so one union-backed
/// `Vec` serves both phases.
#[repr(C)]
union SharedPoints {
    decode: ManuallyDrop<Vec<(i32, i32), MAX_DECODE_POINTS>>,
    screen: ManuallyDrop<Vec<Point, MAX_SCREEN_POINTS>>,
    edges: ManuallyDrop<Vec<PackedEdge, MAX_SCREEN_POINTS>>,
}

impl Default for SharedPoints {
    fn default() -> Self {
        Self { decode: ManuallyDrop::new(Vec::new()) }
    }
}

impl SharedPoints {
    fn decode(&mut self) -> &mut Vec<(i32, i32), MAX_DECODE_POINTS> {
        // Writing a union member makes it the active one; `Vec::new()` writes only the metadata.
        self.decode = ManuallyDrop::new(Vec::new());
        // SAFETY: `decode` was initialized as the active member immediately above.
        unsafe { &mut self.decode }
    }

    fn screen(&mut self) -> &mut Vec<Point, MAX_SCREEN_POINTS> {
        self.screen = ManuallyDrop::new(Vec::new());
        // SAFETY: `screen` was initialized as the active member immediately above.
        unsafe { &mut self.screen }
    }

    fn edges(&mut self) -> &mut Vec<PackedEdge, MAX_SCREEN_POINTS> {
        self.edges = ManuallyDrop::new(Vec::new());
        // SAFETY: `edges` was initialized as the active member immediately above.
        unsafe { &mut self.edges }
    }
}

const _: () = assert!(
    core::mem::size_of::<Vec<(i32, i32), MAX_DECODE_POINTS>>() == core::mem::size_of::<Vec<Point, MAX_SCREEN_POINTS>>()
);
const _: () = assert!(
    core::mem::align_of::<Vec<(i32, i32), MAX_DECODE_POINTS>>()
        == core::mem::align_of::<Vec<Point, MAX_SCREEN_POINTS>>()
);
const _: () = assert!(
    core::mem::size_of::<Vec<PackedEdge, MAX_SCREEN_POINTS>>() == core::mem::size_of::<Vec<Point, MAX_SCREEN_POINTS>>()
);

#[derive(Default)]
pub(crate) struct DrawScratch {
    points: SharedPoints,
    pub(crate) xs: Vec<f32, MAX_CROSSINGS>,
}

/// A monotonic microsecond clock for the stage timings of [`RenderScratch::render_timed`]. This
/// crate is `no_std` and carries no clock, so the caller supplies one.
pub trait Clock {
    /// Microseconds since a fixed epoch. Only differences are taken.
    fn now_us(&self) -> u64;
}

/// The zero-cost [`Clock`]: always `0`, so the optimizer folds the timing away.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopClock;

impl Clock for NoopClock {
    #[inline(always)]
    fn now_us(&self) -> u64 {
        0
    }
}

#[inline]
fn diagnostics<S: MapScene>(scene: &S, stats: &mut RenderStats, fallback: Diagnostics) -> Diagnostics {
    match scene.diagnostics() {
        Ok(Some(diagnostics)) => diagnostics,
        Ok(None) => fallback,
        Err(ReadError::Source) => {
            stats.map_read_failures = stats.map_read_failures.saturating_add(1);
            fallback
        }
        Err(ReadError::CacheBusy) => {
            stats.map_cache_contentions = stats.map_cache_contentions.saturating_add(1);
            fallback
        }
        Err(ReadError::Malformed) => {
            stats.map_structure_failures = stats.map_structure_failures.saturating_add(1);
            fallback
        }
    }
}

/// What a single render call drew.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderStats {
    pub lod: usize,
    /// Quadtree leaves overlapping the viewport, summed over every walk the frame did: a
    /// saturated frame re-walks as pass A and counts both.
    pub chunks_visited: usize,
    pub features_tried: usize,
    pub features_drawn: usize,
    /// Complete features rejected by the fixed span/point/ring frame budgets.
    pub features_dropped: usize,
    /// Features consumed whole but rejected because decode scratch was too small.
    pub feature_decode_capacity_drops: u32,
    /// Structurally invalid feature records consumed without publishing partial geometry.
    pub malformed_features: u32,
    /// Structural corruption outside an individual feature record.
    pub map_structure_failures: u32,
    /// Backing-medium failures while reading indexes or chunks.
    pub map_read_failures: u32,
    /// Legal cache contention; these never panic through the safe API.
    pub map_cache_contentions: u32,
    pub points_tried: usize,
    pub points_drawn: usize,
    /// `stub_evictions`: pass-A overflows where a higher-priority candidate displaced the worst
    /// resident stub. `chunks_refetched`: distinct chunks owning an admitted pass-B winner.
    pub stub_evictions: u32,
    pub chunks_refetched: u32,
    /// Active-route overlay: chunks decoded, points across them, and points actually stroked
    /// after the clip and simplify. The route carries no LOD.
    pub route_chunks: usize,
    pub route_points: usize,
    pub route_points_drawn: usize,
    // Buffer utilization (0.0–1.0).
    pub span_utilization: f32,
    pub point_utilization: f32,
    pub ring_utilization: f32,
    // How the frame's scratch splits between the line and polygon paths.
    pub line_spans: usize,
    pub line_points: usize,
    pub line_rings: usize,
    pub poly_spans: usize,
    pub poly_points: usize,
    pub poly_rings: usize,
    /// Streamed-map cache accounting: hits are served from RAM, misses read from SD, and
    /// `map_sd_reads` with `map_bytes_read` are the raw source overhead.
    pub map_chunk_hits: u32,
    pub map_chunk_misses: u32,
    pub map_sd_reads: u32,
    pub map_bytes_read: u32,
    /// Host-measured wall time for the whole frame draw (render + overlays), µs; `0` = not measured.
    pub render_us: u32,
    /// Per-stage wall time of the map render, µs; `0` on the untimed path. Overlays run after
    /// `render` returns, so overlay time is `total − (collect_us + sort_us + draw_us)`.
    pub collect_us: u32,
    pub sort_us: u32,
    pub draw_us: u32,
}

/// What a render call should draw, stated per frame by the caller. A caller that wants a switch
/// to stick owns that state itself. [`Default`] draws everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderConfig {
    /// Draw the terrain layer: every style carrying
    /// [`StyleFlags::terrain_layer`](obc_map_scene::StyleFlags::terrain_layer). A hidden layer is
    /// dropped in the collect pass's style mask, so it costs no frame budget, but it saves no
    /// I/O: the map's cells interleave terrain with everything else.
    pub terrain_layer: bool,
}

impl Default for RenderConfig {
    fn default() -> Self {
        RenderConfig { terrain_layer: true }
    }
}

/// The reusable per-frame scratch: every decode, collect and draw buffer. Construct once and hand
/// `&mut` to [`render`](RenderScratch::render) per frame.
///
/// Never construct one by value on a device stack. It holds large `heapless::Vec`s
/// ([`MCU_SCRATCH_BYTES`]) that only return-value optimization keeps off the stack, and a debug
/// build can decline that. The device places it with [`init_zeroed`](RenderScratch::init_zeroed).
#[derive(Default)]
pub struct RenderScratch {
    frame: FrameScratch,
    /// Draw scratch, shared by the map draw phase and the overlays.
    pub(crate) draw: DrawScratch,
}

impl RenderScratch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Initialize a scratch in place at `slot`, the MCU placement path that never materializes
    /// the buffers on the stack.
    ///
    /// # Safety
    /// `slot` must be valid for writes, aligned, and exclusively owned for the call.
    pub unsafe fn init_zeroed(slot: *mut Self) {
        // SAFETY: the scratch holds only `heapless::Vec`s, whose empty state is the all-zero bit
        // pattern, so this lowers to a `memset`. The caller guarantees a valid, owned slot.
        unsafe { slot.write_bytes(0u8, 1) }
    }

    /// Render the visible map into `target`, as `cfg` asks: pick the LOD for the viewport's
    /// metres per pixel, clear to `bg`, collect visible features in priority order, sort by style
    /// z-index and draw. `color_fn` maps a style's RGB565 to the target's pixel color.
    pub fn render<D, F, S>(
        &mut self,
        target: &mut D,
        scene: &S,
        vp: &Viewport,
        bg: D::Color,
        cfg: RenderConfig,
        color_fn: F,
    ) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: MapScene,
    {
        self.render_timed(target, scene, vp, bg, cfg, color_fn, &NoopClock)
    }

    /// Like [`render`](RenderScratch::render), but fills the per-stage timings from `clock`.
    #[allow(clippy::too_many_arguments)]
    pub fn render_timed<D, F, S>(
        &mut self,
        target: &mut D,
        scene: &S,
        vp: &Viewport,
        bg: D::Color,
        cfg: RenderConfig,
        color_fn: F,
        clock: &dyn Clock,
    ) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: MapScene,
    {
        let t0 = clock.now_us();
        let _ = target.clear(bg);
        let t_cleared = clock.now_us();

        let lod_count = scene.lod_count();
        if lod_count == 0 {
            // An empty scene is a valid background-only render, and LOD selection would underflow.
            return RenderStats { draw_us: clock.now_us().saturating_sub(t0) as u32, ..Default::default() };
        }
        let requested_lod = scene.select_lod_for_mpp(vp.meters_per_pixel());
        let lod = requested_lod.min(lod_count - 1);
        let is_finest = lod == lod_count - 1;
        let mut stats = RenderStats { lod, ..Default::default() };
        if requested_lod >= lod_count {
            stats.map_structure_failures = 1;
        }

        // Snapshot the cache counters across `collect` and record the per-frame delta.
        let before = diagnostics(scene, &mut stats, Diagnostics::default());
        {
            // Collection and drawing are disjoint phases; `draw_map` reinterprets the same empty
            // backing as screen points later.
            let Self { frame, draw, .. } = self;
            frame.collect(scene, lod, vp, draw.points.decode(), !cfg.terrain_layer, &mut stats);
        }
        let after = diagnostics(scene, &mut stats, before);
        stats.map_chunk_hits = after.chunk_hits.wrapping_sub(before.chunk_hits);
        stats.map_chunk_misses = after.chunk_misses.wrapping_sub(before.chunk_misses);
        stats.map_sd_reads = after.source_reads.wrapping_sub(before.source_reads);
        stats.map_bytes_read = after.bytes_read.wrapping_sub(before.bytes_read);
        let t_collected = clock.now_us();

        self.frame.spans_mut().sort_unstable_by_key(|s| (s.z, s.seq));
        let t_sorted = clock.now_us();

        self.draw_map(target, scene, is_finest, vp, &color_fn);
        let t_drawn = clock.now_us();

        // The clear is a framebuffer write, so it counts toward `draw` although it ran first.
        // `saturating_sub` guards a non-monotonic clock.
        stats.collect_us = t_collected.saturating_sub(t_cleared) as u32;
        stats.sort_us = t_sorted.saturating_sub(t_collected) as u32;
        stats.draw_us = (t_cleared.saturating_sub(t0) + t_drawn.saturating_sub(t_sorted)) as u32;

        stats
    }

    /// Casing width added on each side of a cased road's fill, in px.
    const CASING_PX: u32 = 1;

    /// Draw the collected, painter-ordered spans: polygons even-odd fill, lines the view-clipped
    /// stroke.
    ///
    /// A cased line (a solid style with a `color2`) also gets a wider `color2` base under the road
    /// fills, at the finest LOD only. Spans are `(z, seq)`-sorted, so cased roads form one z-band
    /// and the casing pass runs where that band begins: above the fills that would paint over it,
    /// under every road fill, so crossing roads keep continuous fills.
    #[allow(clippy::too_many_arguments)]
    fn draw_map<D, F, S>(&mut self, target: &mut D, scene: &S, is_finest: bool, vp: &Viewport, color_fn: &F)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: MapScene,
    {
        let Self { frame, draw } = self;
        let spans = frame.spans();

        // One zoom→width multiplier for the whole frame; the casing derives its width from it too.
        let wscale = width_scale(vp.meters_per_pixel());

        // Two 256-bit style masks, built once per frame. A style is *cased* when it is a solid
        // line with a `color2`; dashed plus `color2` is the railway stripe, which never cases. A
        // style is *outlined* when it carries a `color2` at all.
        let (mut cased_mask, mut outlined_mask) = ([0u32; 8], [0u32; 8]);
        for id in 0..=255u8 {
            if let Some(s) = scene.style(id) {
                if s.color2.is_some() {
                    let (word, bit) = ((id >> 5) as usize, 1u32 << (id & 31));
                    outlined_mask[word] |= bit;
                    if s.flags.line_style() == LineStyle::Solid {
                        cased_mask[word] |= bit;
                    }
                }
            }
        }
        let any_cased = cased_mask.iter().any(|&w| w != 0);
        let is_cased = |sid: u8| cased_mask[(sid >> 5) as usize] & (1 << (sid & 31)) != 0;

        // The z boundary: the first cased road line span. With no cased style the two ranges below
        // collapse into one pass.
        let split = if any_cased {
            spans.iter().position(|s| s.kind == Kind::Line && is_cased(s.style_id)).unwrap_or(spans.len())
        } else {
            spans.len()
        };

        // (1) Everything below the road band, exactly as the base pass.
        Self::draw_spans(frame, draw, target, scene, is_finest, vp, color_fn, wscale, &outlined_mask, &spans[..split]);

        // (2) Casing pass, finest LOD only: a `color2` base at the fill width plus `2*CASING_PX`.
        if is_finest {
            for span in &spans[split..] {
                if span.kind != Kind::Line || !is_cased(span.style_id) {
                    continue;
                }
                // A line uses only its exterior (first) ring — the leading `n` frame points.
                let n = frame.frame_ring_lens[span.ring_start as usize] as usize;
                let pt_start = span.pt_start as usize;
                let pts = &frame.frame_points[pt_start..pt_start + n];
                // `is_cased` guarantees `color2.is_some()`; the `unwrap_or` is a defensive no-op.
                let style = scene.style(span.style_id);
                let casing_color =
                    color_fn(style.and_then(|s| s.color2).unwrap_or_else(|| style.map_or(0, |s| s.color)));
                // The casing follows the fill, so a fixed-width cased style cases its own weight.
                let fixed_width = style.is_some_and(|s| s.flags.fixed_width());
                draw_line(
                    target,
                    vp,
                    pts,
                    casing_color,
                    line_px(span.weight, wscale, fixed_width) + 2 * Self::CASING_PX,
                    LineStyle::Solid,
                    None,
                    draw.points.screen(),
                );
            }
        }

        // (3) The road band and above, exactly as the base pass, on top of the casings.
        Self::draw_spans(frame, draw, target, scene, is_finest, vp, color_fn, wscale, &outlined_mask, &spans[split..]);
    }

    /// Draw a contiguous, painter-ordered `spans` slice, for the two ranges either side of the
    /// casing pass.
    ///
    /// At the finest LOD, a polygon whose style carries a `color2` has every ring stroked closed
    /// in that colour. Touching row-house buildings share walls, so an outline drawn right after
    /// its own fill would be erased by the neighbour's fill: the loop walks equal-`z` groups and
    /// strokes a group's outlines only after every fill in it.
    #[allow(clippy::too_many_arguments)]
    fn draw_spans<D, F, S>(
        frame: &FrameScratch,
        draw: &mut DrawScratch,
        target: &mut D,
        scene: &S,
        is_finest: bool,
        vp: &Viewport,
        color_fn: &F,
        wscale: f32,
        outlined_mask: &[u32; 8],
        spans: &[Span],
    ) where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: MapScene,
    {
        let is_outlined = |sid: u8| outlined_mask[(sid >> 5) as usize] & (1 << (sid & 31)) != 0;
        // Outlines need the finest LOD and some `color2` style; without both, take the single pass.
        let any_outlined = is_finest && outlined_mask.iter().any(|&w| w != 0);
        if !any_outlined {
            for span in spans {
                Self::draw_span(frame, draw, target, scene, vp, color_fn, wscale, span);
            }
            return;
        }

        // Fills-then-outlines per contiguous equal-`z` group.
        let mut i = 0;
        while i < spans.len() {
            let z = spans[i].z;
            let mut j = i + 1;
            while j < spans.len() && spans[j].z == z {
                j += 1;
            }
            let group = &spans[i..j];
            i = j;

            // pass 1 — every span as the base pass; note whether the group needs pass 2.
            let mut group_has_outline = false;
            for span in group {
                Self::draw_span(frame, draw, target, scene, vp, color_fn, wscale, span);
                group_has_outline |= span.kind == Kind::Polygon && is_outlined(span.style_id);
            }
            if !group_has_outline {
                continue;
            }

            // pass 2 — re-stroke each outlined polygon's rings closed in `color2`, over both fills.
            for span in group {
                if span.kind != Kind::Polygon || !is_outlined(span.style_id) {
                    continue;
                }
                Self::outline_polygon(frame, draw, target, scene, vp, color_fn, span);
            }
        }
    }

    /// Draw one span: a polygon even-odd fill, or a line's view-clipped stroke at the frame's
    /// ramped width.
    #[allow(clippy::too_many_arguments)]
    fn draw_span<D, F, S>(
        frame: &FrameScratch,
        draw: &mut DrawScratch,
        target: &mut D,
        scene: &S,
        vp: &Viewport,
        color_fn: &F,
        wscale: f32,
        span: &Span,
    ) where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: MapScene,
    {
        let ring_start = span.ring_start as usize;
        let pt_start = span.pt_start as usize;
        let ring_lens = &frame.frame_ring_lens[ring_start..ring_start + span.ring_count as usize];
        let total: usize = ring_lens.iter().map(|&len| len as usize).sum();
        let pts = &frame.frame_points[pt_start..pt_start + total];
        let color = color_fn(scene.style(span.style_id).map_or(0, |style| style.color));

        let DrawScratch { points, xs } = draw;
        match span.kind {
            Kind::Polygon => {
                fill_polygon_edges(target, pts, ring_lens, color, (vp.w as i32, vp.h as i32), points.edges(), xs);
            }
            Kind::Line => {
                // Lines use only the exterior ring. A missing style falls back to a solid stroke.
                let n = ring_lens.first().copied().unwrap_or(0) as usize;
                let style = scene.style(span.style_id);
                let line = style.map_or(LineStyle::Solid, |s| s.flags.line_style());
                let color2 = style.and_then(|s| s.color2).map(color_fn);
                let fixed_width = style.is_some_and(|s| s.flags.fixed_width());
                draw_line(
                    target,
                    vp,
                    &pts[..n],
                    color,
                    line_px(span.weight, wscale, fixed_width),
                    line,
                    color2,
                    points.screen(),
                );
            }
        }
    }

    /// Stroke a polygon span's rings, exterior and holes, closed in the style's `color2` at a
    /// fixed hairline `weight.max(1)`. Ramped it would reach 3 to 4 px at the finest LOD, and a
    /// ring stroked that thick floods a small building footprint until the fill drowns.
    #[allow(clippy::too_many_arguments)]
    fn outline_polygon<D, F, S>(
        frame: &FrameScratch,
        draw: &mut DrawScratch,
        target: &mut D,
        scene: &S,
        vp: &Viewport,
        color_fn: &F,
        span: &Span,
    ) where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: MapScene,
    {
        // `outlined_mask` guarantees `color2.is_some()`; the `unwrap_or` is a defensive no-op.
        let style = scene.style(span.style_id);
        let color2 = color_fn(style.and_then(|s| s.color2).unwrap_or_else(|| style.map_or(0, |s| s.color)));
        let weight = span.weight.max(1) as u32;
        let (w, h) = (vp.w as i32, vp.h as i32);

        let ring_start = span.ring_start as usize;
        let ring_lens = &frame.frame_ring_lens[ring_start..ring_start + span.ring_count as usize];
        let mut off = span.pt_start as usize;
        for &rl in ring_lens {
            let rl = rl as usize;
            let ring = &frame.frame_points[off..off + rl];
            off += rl;
            if rl < 2 {
                continue;
            }
            // Stroke the ring closed: chain the first vertex again so the last wall is drawn.
            let closed = ring.iter().chain(ring.first()).map(|p| p.point());
            Stroker::new(target, draw.points.screen(), color2, weight, w, h).stroke(closed);
        }
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod shared_points_tests {
    use super::SharedPoints;
    use embedded_graphics::prelude::Point;

    #[test]
    fn switching_phases_reinitializes_the_active_vector() {
        let mut points = SharedPoints::default();
        points.decode().push((1, 2)).unwrap();
        assert!(points.screen().is_empty());
        points.screen().push(Point::new(3, 4)).unwrap();
        assert!(points.decode().is_empty());
    }
}

#[cfg(test)]
mod width_ramp_tests {
    use super::{line_px, scale_weight, width_scale, MAX_LINE_PX, REF_MPP};

    #[test]
    fn identity_at_reference_scale() {
        let s = width_scale(REF_MPP);
        assert!((s - 1.0).abs() < 1e-4, "scale at REF_MPP is 1.0, got {s}");
        for w in 1..=5u8 {
            assert_eq!(scale_weight(w, s), w as u32, "weight {w} unchanged at REF_MPP");
        }
    }

    #[test]
    fn thickens_zoomed_in_thins_zoomed_out() {
        let (near, far) = (width_scale(1.0), width_scale(120.0));
        assert!(near > 1.0, "zoomed in past REF_MPP grows width, got {near}");
        assert!(far < 1.0, "zoomed out past REF_MPP shrinks width, got {far}");
        assert_eq!(scale_weight(3, far), 1, "motorway is a hairline at 120 mpp");
        assert!(scale_weight(3, near) >= 10, "motorway is fat at 1 mpp");
    }

    #[test]
    fn clamps_to_one_and_cap() {
        assert_eq!(scale_weight(1, width_scale(1000.0)), 1);
        assert_eq!(scale_weight(6, width_scale(0.05)), MAX_LINE_PX);
        // Degenerate mpp (0) must not divide-by-zero into NaN and defeat the clamp.
        assert_eq!(scale_weight(3, width_scale(0.0)), MAX_LINE_PX);
    }

    /// A fixed-width style keeps its authored `weight` at every zoom; an ordinary one rides the ramp.
    #[test]
    fn fixed_width_ignores_the_zoom_ramp() {
        for mpp in [1000.0, 9.0, 4.0, 1.0, 0.05] {
            let s = width_scale(mpp);
            assert_eq!(line_px(1, s, true), 1, "a weight-1 contour is a hairline at {mpp} mpp");
            assert_eq!(line_px(3, s, true), 3, "a weight-3 fixed style is 3 px at {mpp} mpp");
        }
        // The same authored weight on an ordinary style still rides the ramp.
        assert_eq!(line_px(1, width_scale(4.0), false), 2);
        assert_eq!(line_px(1, width_scale(1.0), false), 4);
    }

    /// A fixed width is still clamped to `1..=MAX_LINE_PX`.
    #[test]
    fn fixed_width_is_clamped_like_the_ramp() {
        let s = width_scale(REF_MPP);
        assert_eq!(line_px(0, s, true), 1, "weight 0 never vanishes");
        assert_eq!(line_px(255, s, true), MAX_LINE_PX, "a fixed width cannot eat the panel");
        // At the reference scale the ramp is the identity, so both paths agree there by definition.
        for w in 1..=5u8 {
            assert_eq!(line_px(w, s, true), line_px(w, s, false), "identical at REF_MPP, weight {w}");
        }
    }
}
