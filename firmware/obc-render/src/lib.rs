//! Shared map renderer (feature `render`).
//!
//! Generic over `embedded-graphics`' [`DrawTarget`], so the host (SDL
//! `SimulatorDisplay`) and the device (LS021B7DD02) share the same projection,
//! LOD selection, painter ordering, polygon fill and line drawing.
//!
//! [`MapRenderer`] owns every scratch buffer and clears (not frees) them each
//! frame, so steady-state rendering does no heap allocation. Geometry math uses
//! `libm` for `no_std`.

#![no_std]

use heapless::Vec;

use embedded_graphics::prelude::*;

use obc_reader::{Kind, Reader};

pub mod canvas;
mod collect;
mod fill;
mod font_data;
mod overlay;
mod stroke;
pub mod surface;
pub mod text;
mod viewport;
pub use canvas::{rect, Canvas};
pub use overlay::{OverlayChunk, RouteOverlaySource};
pub use surface::Surface;
pub use text::{draw_text, text_width, Font, TextAlign};
pub use viewport::{mpp_for_zoom, round_coord, zoom_for_mpp, Viewport};

use collect::{FrameScratch, Span};
use fill::fill_polygon_proj;
use stroke::{draw_line, Stroker};

// Per-frame buffer capacities. Statically allocated (heapless::Vec); growing one costs boot
// RAM, not per-frame. Two memory profiles select the caps:
//   - default (512 KB nRF54LM20 / sim / tests): the shipping-part profile. The renderer scratch
//     (`MCU_RENDERER_BYTES` below, ~90 KB) is sized to the LM20's 512 KB budget alongside the
//     75 KB RGB222 framebuffer, the map/route caches, the on-device router, the BLE stack (issue
//     #270 — map + BLE share one image), and a larger stack reserve than the 256 KB DK can spare
//     (the DK's ~36 KB residual stack has overflowed the deep render path more than once). The
//     **simulator builds this profile**, so it renders exactly what the LM20 will — features start
//     dropping at the same busy coarse zooms (deliberate: an over-dense frame is slow on-glass, so
//     the sim shows that limit rather than an unattainable host-fidelity map).
//   - `nrf-mem`: constrained 256 KB nRF54L15-DK profile — culled ~3× harder (`~30 KB` scratch) so
//     map + BLE still fit the 256 KB part; the board crate's budget assert is the binding check.
//     The cost: features drop at busier coarse zooms than on the LM20 (see [`render`]).
// On `nrf-mem` even the single-feature decode buffers (`MAX_DECODE_*`) are trimmed below the
// format's per-feature bound — see the truncation note at [`MAX_DECODE_POINTS`].

/// Maximum visible features per frame (each is a [`Span`] — 14 bytes). Saturates first at coarse
/// zoom (many small features).
#[cfg(not(feature = "nrf-mem"))]
pub const MAX_SPANS: usize = 1536;
// Trimmed hard on nrf-mem: the ride loop's deep per-frame render path (per-frame `Reader::new` +
// streamed-chunk decode over embedded-sdmmc) needs a large MSP stack that must coexist with the
// resident `RouteCache`/`RouteIndex` — and, on the combined image, the BLE stack — on the 256 KB
// part; freeing scratch buys that headroom.
#[cfg(feature = "nrf-mem")]
pub const MAX_SPANS: usize = 384;

/// Maximum total vertices across all visible features per frame (8 bytes each).
///
/// Known `nrf-mem` oddity, kept deliberately: there the frame cap (768) sits *below* the
/// single-feature decode cap [`MAX_DECODE_POINTS`] (1024), so a decoded max-size feature can
/// never be admitted to the frame buffers — the capacity check drops it every frame, and it
/// counts into `features_dropped`. It's undroppable-by-design: a 256 KB-DK artifact (the shipping
/// LM20 relaxes the trim), and real map features rarely approach these sizes. Do not "fix" it by
/// raising this cap.
#[cfg(not(feature = "nrf-mem"))]
pub const MAX_FRAME_POINTS: usize = 4096;
#[cfg(feature = "nrf-mem")]
pub const MAX_FRAME_POINTS: usize = 768;

/// Maximum total ring entries across all visible features per frame.
#[cfg(not(feature = "nrf-mem"))]
pub const MAX_FRAME_RINGS: usize = 1024;
#[cfg(feature = "nrf-mem")]
pub const MAX_FRAME_RINGS: usize = 192;

/// Maximum vertices for a single feature during decode (reused per feature). On the host this
/// equals `obc_reader::MAX_FEAT_PTS` (asserted below) — full format fidelity. On `nrf-mem` it is
/// trimmed **below the format's per-feature bound**: the reader's `read_ring` saturates its
/// output `Vec`, so a feature past the cap draws with silently truncated geometry (a visibly
/// degraded large polygon) instead of failing. A deliberate 256 KB-DK compromise (issue #270 —
/// the map path must coexist with the BLE stack); the 512 KB LM20 restores the format bound.
#[cfg(not(feature = "nrf-mem"))]
pub const MAX_DECODE_POINTS: usize = 2048;
#[cfg(feature = "nrf-mem")]
pub const MAX_DECODE_POINTS: usize = 1024;

/// Maximum rings for a single feature during decode. Must equal `obc_reader::MAX_FEAT_RINGS`
/// (asserted below).
pub const MAX_DECODE_RINGS: usize = 32;

/// Maximum projected screen points for drawing one feature. The fill/polyline path projects
/// **every** vertex of a decoded feature into this buffer before walking it, so it must hold a
/// whole decode buffer (invariant asserted below; dropping under it makes `fill_polygon` index
/// past the projected points).
#[cfg(not(feature = "nrf-mem"))]
pub const MAX_SCREEN_POINTS: usize = 2048;
#[cfg(feature = "nrf-mem")]
pub const MAX_SCREEN_POINTS: usize = 1024;

/// Maximum scanline crossings buffered for one polygon-fill row. A row whose
/// outline crossings exceed this is skipped rather than mis-filled (see
/// [`fill_polygon`]) — sized to fit the MCU RAM budget asserted below, not the
/// worst-case comb (which could approach [`MAX_SCREEN_POINTS`]).
pub const MAX_CROSSINGS: usize = 256;

// `Span` packs its buffer offsets into `u16` to stay small, so the frame buffers
// it indexes must fit in a `u16`. These guard that invariant at compile time.
const _: () = assert!(MAX_FRAME_POINTS <= u16::MAX as usize, "Span::pt_start is u16");
const _: () = assert!(MAX_FRAME_RINGS <= u16::MAX as usize, "Span::ring_start is u16");
const _: () = assert!(MAX_SPANS <= u16::MAX as usize, "Span::seq is u16");
// The draw path projects a whole decoded feature into `screen` before walking its rings
// (`screen[base..base + len]`), so it must hold at least a full decode buffer or it indexes
// past the points and panics.
const _: () = assert!(MAX_SCREEN_POINTS >= MAX_DECODE_POINTS, "`screen` must hold a whole decoded feature");
// The decode scratch pairs with the reader's format bounds across the crate seam: the reader's
// `read_ring` pushes with `let _ = out.push(..)`, so a smaller render-side buffer *silently
// truncates geometry* rather than failing. On the host that must never happen — equality is
// pinned so neither crate's constant can drift alone. On `nrf-mem` the trim below the format
// bound is deliberate (see [`MAX_DECODE_POINTS`]); only the direction is pinned so the caps
// can't accidentally *exceed* what the reader hands out.
#[cfg(not(feature = "nrf-mem"))]
const _: () =
    assert!(MAX_DECODE_POINTS == obc_reader::MAX_FEAT_PTS, "decode scratch must hold the format's max feature");
const _: () =
    assert!(MAX_DECODE_POINTS <= obc_reader::MAX_FEAT_PTS, "decode scratch cannot exceed the format's max feature");
const _: () = assert!(MAX_DECODE_RINGS == obc_reader::MAX_FEAT_RINGS, "ring scratch must hold the format's max rings");

/// Static RAM the [`MapRenderer`]'s scratch buffers occupy on the 32-bit MCU target (`usize` = 4
/// bytes there). `pub` so a board crate's RAM-budget assert can add it to the framebuffer + caches
/// without re-deriving the formula. (`(i32, i32)` and `Point` are 8 bytes; `usize`/`f32` are 4 on
/// the MCU.) ~90 KB on the default (512 KB LM20 / sim) profile, ~30 KB on `nrf-mem`.
pub const MCU_RENDERER_BYTES: usize = MAX_DECODE_POINTS * 8
    + MAX_DECODE_RINGS * 4
    + MAX_FRAME_POINTS * 8
    + MAX_FRAME_RINGS * 4
    + MAX_SPANS * core::mem::size_of::<Span>()
    + MAX_SCREEN_POINTS * 8
    + MAX_CROSSINGS * 4;
// Loose per-crate ceiling catching an accidental cap blow-up; the binding fit check is the board
// crate's whole-resident-set budget assert.
const _: () = assert!(MCU_RENDERER_BYTES <= 200 * 1024, "MapRenderer exceeds the 200 KB MCU budget");

/// Ground scale (metres per pixel) at which a style's configured `weight` renders at its **nominal**
/// pixel width — i.e. the width ramp is the identity here. Chosen at mid-riding zoom so the presets
/// look exactly as authored right where you actually ride; zooming in thickens roads, out thins them.
const REF_MPP: f32 = 10.0;

/// Exponent of the zoom→width ramp. `1.0` would scale strokes with true ground size — which fails at
/// both ends (every road sub-pixel zoomed out; a motorway engulfs the panel zoomed in). A sub-linear
/// exponent grows width perceptibly without blowing up: the standard cartographic road ramp.
const WIDTH_GAMMA: f32 = 0.6;

/// Upper clamp on a ramped stroke, in px — keeps a fat road class zoomed all the way in from eating
/// the 240-px panel. The lower clamp is 1 px (a hairline never vanishes). See [`scale_weight`].
const MAX_LINE_PX: u32 = 12;

/// Per-frame width multiplier from the current ground scale: `(REF_MPP / mpp) ^ WIDTH_GAMMA`. A
/// style's nominal `weight` times this is its on-screen px width, so a road thickens as you zoom in
/// and thins as you zoom out. Computed **once per frame** (not per span) and fed to [`scale_weight`].
#[inline]
pub(crate) fn width_scale(mpp: f32) -> f32 {
    libm::powf(REF_MPP / mpp.max(f32::MIN_POSITIVE), WIDTH_GAMMA)
}

/// A style's nominal `weight` scaled to on-screen px at the frame's [`width_scale`], rounded to a
/// whole pixel and clamped to `1..=MAX_LINE_PX`. Rounding to an integer px + the map's ×1.2 zoom
/// detents keep the width stepping cleanly frame to frame (no sub-pixel shimmer while zooming).
#[inline]
pub(crate) fn scale_weight(weight: u8, scale: f32) -> u32 {
    (libm::roundf(weight as f32 * scale) as i32).clamp(1, MAX_LINE_PX as i32) as u32
}

/// The renderer's draw scratch: projected screen points (also the polyline run
/// buffer) and the scanline-fill crossing buffer. Cleared per use.
#[derive(Default)]
pub(crate) struct DrawScratch {
    pub(crate) screen: Vec<Point, MAX_SCREEN_POINTS>,
    pub(crate) xs: Vec<f32, MAX_CROSSINGS>,
}

/// A monotonic microsecond clock for **stage timing** inside [`MapRenderer::render_timed`].
///
/// `obc-render` is `no_std` and carries no clock, so a caller wanting the per-stage breakdown
/// (collect / sort / draw) passes one in (the device an embassy-`Instant` clock, a host
/// `std::time::Instant`). The plain [`MapRenderer::render`] path passes the zero-cost
/// [`NoopClock`], leaving the stage fields at `0`.
pub trait Clock {
    /// Microseconds since some fixed, monotonic epoch. Only differences are taken.
    fn now_us(&self) -> u64;
}

/// The zero-cost [`Clock`] for the untimed [`MapRenderer::render`] path: always `0`, so every stage
/// delta is `0` and the optimizer folds the timing away.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopClock;

impl Clock for NoopClock {
    #[inline(always)]
    fn now_us(&self) -> u64 {
        0
    }
}

/// What a single render call drew.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderStats {
    pub lod: usize,
    /// Quadtree leaves overlapping the viewport this frame (uncapped). Counted once — the
    /// stub-select collect's pass A walks the leaves a single time (see [`collect`](crate::collect)).
    pub chunks_visited: usize,
    pub features_tried: usize,
    pub features_drawn: usize,
    pub features_dropped: usize,
    pub points_tried: usize,
    pub points_drawn: usize,
    /// Stub-select accounting (issue #564). `stub_evictions`: pass-A stub-buffer overflows where a
    /// higher-priority candidate displaced the lowest-priority resident stub (only under span
    /// saturation). `chunks_refetched`: chunks pass B re-read because they owned an admitted winner
    /// — so `map_chunk_misses ≈ chunks_visited + chunks_refetched`, versus `4 × chunks_visited` for
    /// the old level-major collector.
    pub stub_evictions: u32,
    pub chunks_refetched: u32,
    /// Active-route overlay this frame: chunks decoded (bbox met the viewport), total points across
    /// them, and how many were *actually* stroked after the view clip + subpixel simplify. The route
    /// carries **no LOD**, so `route_points` climbs as you zoom out of a long route while
    /// `route_points_drawn` stays near what's on-screen.
    pub route_chunks: usize,
    pub route_points: usize,
    pub route_points_drawn: usize,
    // Buffer utilization (0.0–1.0).
    pub span_utilization: f32,
    pub point_utilization: f32,
    pub ring_utilization: f32,
    /// Streamed-map cache accounting for this frame. Stub-select touches each visible chunk once in
    /// pass A and once more in pass B only if it owns a winner (`chunks_refetched`), so
    /// `map_chunk_misses ≈ chunks_visited + chunks_refetched`. `map_chunk_hits` are fetches served
    /// from a resident cache slot (e.g. a chunk's later winners in pass B), `map_chunk_misses` the
    /// ones that read from SD. `map_sd_reads` / `map_bytes_read` are the raw source overhead (index
    /// blocks + chunk fills). Hit rate is `hits / (hits + misses)`.
    pub map_chunk_hits: u32,
    pub map_chunk_misses: u32,
    pub map_sd_reads: u32,
    pub map_bytes_read: u32,
    /// Host-measured wall time for the whole frame draw (render + overlays), µs; `0` = not measured.
    /// Filled by the host after timing the draw (sim uses `Instant`, device the DWT cycle counter).
    pub render_us: u32,
    /// Per-stage wall time of the **map** render, µs — filled by
    /// [`render_timed`](MapRenderer::render_timed) from the caller's [`Clock`]; `0` on the untimed
    /// path. `collect_us` = visible-feature collection (walk + read + decode + cull + span build),
    /// `sort_us` = painter's-order span sort, `draw_us` = full-screen clear + rasterization. Base
    /// map only; overlays run after `render` returns, so overlay time is
    /// `total − (collect_us + sort_us + draw_us)`.
    pub collect_us: u32,
    pub sort_us: u32,
    pub draw_us: u32,
}

/// Reusable renderer holding every scratch buffer. Construct once, call
/// [`MapRenderer::render`] per frame; buffers are cleared and reused, so no per-frame allocation.
#[derive(Default)]
pub struct MapRenderer {
    /// Collection scratch + the frame buffers (decode → cull → spans).
    frame: FrameScratch,
    /// Draw scratch (projected points / polyline runs + scanline crossings), shared by the map
    /// draw phase and the marker/route/breadcrumb overlays.
    pub(crate) draw: DrawScratch,
}

impl MapRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Initialize a renderer **in place** at `slot` as the empty, ready-to-render state — the MCU
    /// placement path, building the resident renderer straight into a fixed RAM region without ever
    /// materializing the ~200 KB of scratch on the stack.
    ///
    /// Every scratch buffer is a [`heapless::Vec`], whose empty state (`len = 0` over an
    /// uninitialized backing array) is exactly the all-zero bit pattern, so `write_bytes(0, 1)`
    /// lowers to a `memset` with no temporary and no reliance on return-value optimization.
    ///
    /// # Safety
    /// `slot` must be valid for writes, aligned, and exclusively owned for the call.
    /// On return the slot holds a fully initialized, empty [`MapRenderer`].
    pub unsafe fn init_zeroed(slot: *mut Self) {
        // SAFETY: a renderer is only `heapless::Vec`s — no references, no non-zero-discriminant
        // enum, no `bool` — so the all-zero bit pattern is the empty renderer (`len = 0`,
        // write-before-read buffers). The caller guarantees a valid, owned, aligned slot.
        unsafe { slot.write_bytes(0u8, 1) }
    }

    /// Render the visible map into `target`.
    ///
    /// Selects the LOD for the viewport's meters-per-pixel, clears to `bg`, collects visible
    /// features in global priority order ([`FrameScratch::collect`]), orders them by style z-index
    /// (painter's algorithm) and draws polygons (even-odd scanline fill) and lines. `color_fn` maps
    /// a style's RGB565 to the target's pixel color (host chooses true-color vs. device
    /// quantization).
    pub fn render<D, F>(
        &mut self,
        target: &mut D,
        reader: &Reader,
        vp: &Viewport,
        bg: D::Color,
        color_fn: F,
    ) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        self.render_timed(target, reader, vp, bg, color_fn, &NoopClock)
    }

    /// Like [`render`](MapRenderer::render) but fills the per-stage timings on the returned
    /// [`RenderStats`] from `clock`. Base map only; see the [`RenderStats`] stage-field docs.
    pub fn render_timed<D, F>(
        &mut self,
        target: &mut D,
        reader: &Reader,
        vp: &Viewport,
        bg: D::Color,
        color_fn: F,
        clock: &dyn Clock,
    ) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let t0 = clock.now_us();
        let _ = target.clear(bg);
        let t_cleared = clock.now_us();

        let lod = reader.select_lod_for_mpp(vp.meters_per_pixel());
        let view = vp.visible_bbox();
        let mut stats = RenderStats { lod, ..Default::default() };

        // Collect → painter's order → draw. `seq` is the stable, alloc-free tie-break within a
        // z-index. Snapshot the streamed-map cache counters across `collect` (the only phase that
        // reads the map source) and record the per-frame delta — robust whether the caller hands us
        // a fresh `Reader` each frame or a reused one.
        let before = reader.chunk_cache_stats();
        self.frame.collect(reader, lod, &view, &mut stats);
        let after = reader.chunk_cache_stats();
        stats.map_chunk_hits = after.chunk_hits.wrapping_sub(before.chunk_hits);
        stats.map_chunk_misses = after.chunk_misses.wrapping_sub(before.chunk_misses);
        stats.map_sd_reads = after.sd_reads.wrapping_sub(before.sd_reads);
        stats.map_bytes_read = after.bytes_read.wrapping_sub(before.bytes_read);
        let t_collected = clock.now_us();

        self.frame.spans_mut().sort_unstable_by_key(|s| (s.z, s.seq));
        let t_sorted = clock.now_us();

        self.draw_map(target, reader, lod, vp, &color_fn);
        let t_drawn = clock.now_us();

        // The clear is a framebuffer write, so it counts toward `draw` even though it ran first.
        // `saturating_sub` guards a momentarily non-monotonic clock; a frame is well under a
        // second, so the `u32` µs casts never truncate.
        stats.collect_us = t_collected.saturating_sub(t_cleared) as u32;
        stats.sort_us = t_sorted.saturating_sub(t_collected) as u32;
        stats.draw_us = (t_cleared.saturating_sub(t0) + t_drawn.saturating_sub(t_sorted)) as u32;

        stats
    }

    /// Casing width added on **each** side of a cased road's fill, in px: a cased road strokes a solid
    /// base in `color2` at the fill's ramped on-screen width (`scale_weight`, #579) `+ 2*CASING_PX`,
    /// under its fill. Fixed const — a per-style casing width is out of scope for #559.
    const CASING_PX: u32 = 1;

    /// Draw the collected, painter-ordered spans into `target`. Polygons fill via even-odd scanline;
    /// lines stroke via the view-clipped overlay path — resolving each line's full
    /// [`Style`](obc_reader::Style) (`dashed`/`color2`) from `reader` via [`Span::style_id`].
    ///
    /// **Road casing (#559).** Solid lines whose style carries a `color2` (the *cased* styles) get a
    /// `weight + 2*CASING_PX` stroke in `color2` painted **under** their normal fill — but only at the
    /// finest LOD. Spans are `(z, seq)`-sorted, so the cased road lines form a contiguous z-band; the
    /// casing pass is inserted at the **z boundary where that band begins** (`split`), *not* before the
    /// whole frame. That keeps casings above the low-z land/water/landuse/forest fills — which would
    /// otherwise paint over them — yet under **all** road fills, so crossing roads keep continuous
    /// fills through a junction (no casing slicing across another road's fill) and a road over a forest
    /// polygon keeps its casing. Three steps:
    ///
    /// 1. `spans[0..split)` — everything below the road band, drawn exactly as the base pass.
    /// 2. casing pass over `spans[split..]` (finest LOD only) — the cased lines, wide `color2` base.
    /// 3. `spans[split..]` — the road band and above, drawn exactly as the base pass, over the casings.
    ///
    /// When no style is cased `split == spans.len()`: step 2 is empty and steps 1 + 3 collapse to
    /// today's single pass → **byte-identical** output at zero extra per-span cost. Coarser LODs skip
    /// step 2 outright (`lod` gate). Polygons are never cased (that's #560).
    fn draw_map<D, F>(&mut self, target: &mut D, reader: &Reader, lod: usize, vp: &Viewport, color_fn: &F)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        // Disjoint borrows: spans/geometry read from `frame`, draw scratch written to `draw`.
        let Self { frame, draw } = self;
        let spans = frame.spans();

        // One zoom→width multiplier for the whole frame (#579, `width_scale`): a style's nominal
        // `weight` ramps to its on-screen px width via `scale_weight`, so roads thicken zoomed in and
        // thin zoomed out. The casing pass derives its width from this same ramped value.
        let wscale = width_scale(vp.meters_per_pixel());

        // The 256-bit "cased" style mask, built once per frame (mirrors `collect`'s `vis_mask`): a
        // style is cased ⇔ it's a **solid** line (`!dashed`) carrying a `color2`. Dashed + color2 is
        // the railway stripe (#558), which never cases.
        let mut cased_mask = [0u32; 8];
        for id in 0..=255u8 {
            if let Some(s) = reader.style(id) {
                if !s.dashed && s.color2.is_some() {
                    cased_mask[(id >> 5) as usize] |= 1 << (id & 31);
                }
            }
        }
        let any_cased = cased_mask.iter().any(|&w| w != 0);
        let is_cased = |sid: u8| cased_mask[(sid >> 5) as usize] & (1 << (sid & 31)) != 0;

        // The 256-bit "outlined" style mask (#560): a style is polygon-outline-eligible ⇔ it carries a
        // `color2` (`line_style`/`dashed` are irrelevant for polygons — `draw_spans`'s outline pass
        // filters to `Kind::Polygon`, so a cased *line* sharing this bit never triggers an outline).
        // Built **once per frame** and threaded into both `draw_spans` calls so neither rebuilds it; an
        // empty mask makes each call take today's exact single-loop path (byte-identical, zero cost).
        let mut outlined_mask = [0u32; 8];
        for id in 0..=255u8 {
            if reader.style(id).is_some_and(|s| s.color2.is_some()) {
                outlined_mask[(id >> 5) as usize] |= 1 << (id & 31);
            }
        }

        // The z boundary: the first cased road **line** span. Everything before it is the low-z band
        // (land / water / landuse / buildings / low-z lines). No cased style ⇒ `split == spans.len()`,
        // so the scan is skipped and the two ranges below collapse to one full pass (today's path).
        let split = if any_cased {
            spans.iter().position(|s| s.kind == Kind::Line && is_cased(s.style_id)).unwrap_or(spans.len())
        } else {
            spans.len()
        };

        // (1) Everything below the road band, exactly as the base pass.
        Self::draw_spans(frame, draw, target, reader, lod, vp, color_fn, wscale, &outlined_mask, &spans[..split]);

        // (2) Casing pass — finest LOD only. Each cased road strokes a solid `color2` base at the
        // **ramped** fill width + `2*CASING_PX` (tracks the #579 zoom ramp, not a fixed px), under the
        // fills step 3 paints on top. Re-projects each cased line (accepted; reuses `DrawScratch`).
        if lod == reader.lods().len() - 1 {
            for span in &spans[split..] {
                if span.kind != Kind::Line || !is_cased(span.style_id) {
                    continue;
                }
                // A line uses only its exterior (first) ring — the leading `n` frame points.
                let n = frame.frame_ring_lens[span.ring_start as usize];
                let pt_start = span.pt_start as usize;
                let pts = &frame.frame_points[pt_start..pt_start + n];
                // `is_cased` guarantees `color2.is_some()`; quantize it like the fill color. The
                // `unwrap_or` is a defensive no-op (falls back to an invisible same-color casing).
                let casing_color = color_fn(reader.style(span.style_id).and_then(|s| s.color2).unwrap_or(span.color));
                draw_line(
                    target,
                    vp,
                    pts,
                    casing_color,
                    scale_weight(span.weight, wscale) + 2 * Self::CASING_PX,
                    false,
                    None,
                    &mut draw.screen,
                    &mut draw.xs,
                );
            }
        }

        // (3) The road band and above, exactly as the base pass, on top of the casings.
        Self::draw_spans(frame, draw, target, reader, lod, vp, color_fn, wscale, &outlined_mask, &spans[split..]);
    }

    /// Draw a contiguous, painter-ordered `spans` slice: polygons even-odd fill, lines the view-clipped
    /// stroke with their resolved `dashed`/`color2` style at the frame's ramped width (`wscale`, #579).
    /// Factored out of [`draw_map`](Self::draw_map) so it can be called for the two ranges either side
    /// of the casing pass — the z-group iteration here applies to both automatically.
    ///
    /// **Polygon outlines (#560).** At the finest LOD, a polygon whose style carries a `color2` gets
    /// **every ring — exterior and holes — stroked closed** in `color2` (the fill in `color` is
    /// unchanged; `line_style` is ignored for polygons). Touching row-house buildings share walls, so
    /// outlining each polygon right after its own fill would let a neighbour's fill erase the shared
    /// wall. Instead the loop walks **contiguous equal-`z` groups** (spans are `(z, seq)`-sorted, so a
    /// group is a maximal equal-`z` run) and, per group:
    ///
    /// 1. **pass 1** — draw every span exactly as the base pass (fills + line strokes, in seq order).
    /// 2. **pass 2** (finest LOD only, and only if the group holds an outlined polygon) — re-stroke each
    ///    outlined polygon's rings in `color2`, **after both** shared-wall neighbours' fills, so the
    ///    wall survives. The group finishes before the next `z` begins, so outlines never paint over a
    ///    higher-`z` feature (roads at z 24+ still cover z-20 building outlines where they cross).
    ///
    /// **Zero cost when unused.** `outlined_mask` (built once per frame in `draw_map`) is empty for a
    /// config with no polygon `color2`, and pass 2 is gated on the finest LOD — either case takes the
    /// early single-loop path, byte-identical to today. A group with no outlined polygon skips pass 2.
    #[allow(clippy::too_many_arguments)]
    fn draw_spans<D, F>(
        frame: &FrameScratch,
        draw: &mut DrawScratch,
        target: &mut D,
        reader: &Reader,
        lod: usize,
        vp: &Viewport,
        color_fn: &F,
        wscale: f32,
        outlined_mask: &[u32; 8],
        spans: &[Span],
    ) where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let is_outlined = |sid: u8| outlined_mask[(sid >> 5) as usize] & (1 << (sid & 31)) != 0;
        // Outlines exist only at the finest LOD and only when some style carries a `color2`. Neither
        // holds ⇒ today's exact single pass (byte-identical, and no group-boundary scan).
        let any_outlined = lod == reader.lods().len() - 1 && outlined_mask.iter().any(|&w| w != 0);
        if !any_outlined {
            for span in spans {
                Self::draw_span(frame, draw, target, reader, vp, color_fn, wscale, span);
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

            // pass 1 — every span exactly as the base pass, tracking whether this group needs pass 2.
            let mut group_has_outline = false;
            for span in group {
                Self::draw_span(frame, draw, target, reader, vp, color_fn, wscale, span);
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
                Self::outline_polygon(frame, draw, target, reader, vp, color_fn, span);
            }
        }
    }

    /// Draw one span exactly as the base pass: a polygon even-odd fill, or a line's view-clipped stroke
    /// with its resolved `dashed`/`color2` style at the frame's ramped width (`wscale`, #579). The
    /// unit of pass 1 in [`draw_spans`](Self::draw_spans), unchanged from the pre-#560 single loop.
    #[allow(clippy::too_many_arguments)]
    fn draw_span<D, F>(
        frame: &FrameScratch,
        draw: &mut DrawScratch,
        target: &mut D,
        reader: &Reader,
        vp: &Viewport,
        color_fn: &F,
        wscale: f32,
        span: &Span,
    ) where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let ring_start = span.ring_start as usize;
        let pt_start = span.pt_start as usize;
        let ring_lens = &frame.frame_ring_lens[ring_start..ring_start + span.ring_count as usize];
        let total: usize = ring_lens.iter().sum();
        let pts = &frame.frame_points[pt_start..pt_start + total];
        let color = color_fn(span.color);

        match span.kind {
            Kind::Polygon => fill_polygon_proj(target, vp, pts, ring_lens, color, &mut draw.screen, &mut draw.xs),
            Kind::Line => {
                // Lines use only the exterior ring. Re-resolve the style for `dashed`/`color2`;
                // `color2` quantizes through `color_fn` exactly like the primary. A missing
                // style (never collected) falls back to today's solid stroke.
                let n = ring_lens.first().copied().unwrap_or(0);
                let style = reader.style(span.style_id);
                let dashed = style.is_some_and(|s| s.dashed);
                let color2 = style.and_then(|s| s.color2).map(color_fn);
                draw_line(
                    target,
                    vp,
                    &pts[..n],
                    color,
                    scale_weight(span.weight, wscale),
                    dashed,
                    color2,
                    &mut draw.screen,
                    &mut draw.xs,
                );
            }
        }
    }

    /// Stroke a polygon span's rings — **exterior and every hole (courtyards)** — **closed** (first
    /// point repeated) in its style's `color2`, at a **fixed hairline** width `weight.max(1)`. The #560
    /// finest-LOD outline: called from [`draw_spans`](Self::draw_spans)'s pass 2 for a span the
    /// `outlined_mask` already vetted (`color2.is_some()`). Reuses `DrawScratch` — no new buffers; each
    /// ring projects exactly like a line's exterior. At the preset `weight 1` this is the thin Bresenham
    /// polyline path.
    ///
    /// **Fixed, not ramped.** A line's *stroke* ramps with zoom ([`scale_weight`], #579), but a
    /// building outline is a **1-px edge accent**, not a road: ramped, it hits 3–4 px at the finest LOD
    /// where the ground scale is sub-metre, and a closed ring stroked that thick (round joins + a disc
    /// per sharp corner) floods a small footprint — the fill drowns and the building reads as a dark
    /// slab (measured: outline `color2` pixels ≫ fill pixels). A fixed `weight.max(1)` keeps the wall a
    /// crisp hairline at every finest-LOD zoom, which is the whole point of the feature.
    #[allow(clippy::too_many_arguments)]
    fn outline_polygon<D, F>(
        frame: &FrameScratch,
        draw: &mut DrawScratch,
        target: &mut D,
        reader: &Reader,
        vp: &Viewport,
        color_fn: &F,
        span: &Span,
    ) where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        // `outlined_mask` guarantees `color2.is_some()`; the `unwrap_or` is a defensive no-op that
        // falls back to an invisible same-color outline rather than panicking.
        let color2 = color_fn(reader.style(span.style_id).and_then(|s| s.color2).unwrap_or(span.color));
        let weight = span.weight.max(1) as u32;
        let (w, h) = (vp.w as i32, vp.h as i32);

        let ring_start = span.ring_start as usize;
        let ring_lens = &frame.frame_ring_lens[ring_start..ring_start + span.ring_count as usize];
        let mut off = span.pt_start as usize;
        for &rl in ring_lens {
            let ring = &frame.frame_points[off..off + rl];
            off += rl;
            if rl < 2 {
                continue;
            }
            // Stroke the ring **closed**: chain the first vertex again so the wall between the last and
            // first point is drawn. Projects lazily, exactly like a line's exterior ring.
            let closed = ring.iter().chain(ring.first()).map(|&(lon, lat)| vp.project(lon, lat));
            Stroker::new(target, &mut draw.screen, &mut draw.xs, color2, weight, w, h).stroke(closed);
        }
    }
}

#[cfg(test)]
mod width_ramp_tests {
    use super::{scale_weight, width_scale, MAX_LINE_PX, REF_MPP};

    #[test]
    fn identity_at_reference_scale() {
        // At REF_MPP the ramp is the identity: presets render at exactly their authored weight.
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
        // A motorway (weight 3): ~1 px in overview, ~12 px zoomed all the way in — the numbers
        // quoted when this ramp was proposed. Exact px are by-eye, so assert the shape, not values.
        assert_eq!(scale_weight(3, far), 1, "motorway is a hairline at 120 mpp");
        assert!(scale_weight(3, near) >= 10, "motorway is fat at 1 mpp");
    }

    #[test]
    fn clamps_to_one_and_cap() {
        // Never vanishes: a thin road zoomed far out floors at 1 px.
        assert_eq!(scale_weight(1, width_scale(1000.0)), 1);
        // Never engulfs the panel: a heavy weight zoomed far in caps at MAX_LINE_PX.
        assert_eq!(scale_weight(6, width_scale(0.05)), MAX_LINE_PX);
        // Degenerate mpp (0) must not divide-by-zero into NaN and defeat the clamp.
        assert_eq!(scale_weight(3, width_scale(0.0)), MAX_LINE_PX);
    }
}
