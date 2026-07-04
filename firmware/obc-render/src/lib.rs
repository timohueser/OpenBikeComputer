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
use stroke::draw_line;

// Per-frame buffer capacities. Statically allocated (heapless::Vec); growing one costs boot
// RAM, not per-frame. Two memory profiles select the caps:
//   - default (host / sim / tests): generous, full preview fidelity.
//   - `nrf-mem`: constrained nRF54L15 profile — culled hard so the renderer scratch
//     (`MCU_RENDERER_BYTES` below, ~30 KB vs ~200 KB) fits the 256 KB DK part alongside the 75 KB
//     RGB222 framebuffer + map/route caches **and** the BLE stack (issue #270 — map + BLE share
//     one image); the board crate's budget assert is the binding check. The cost: a frame whose
//     visible-feature / vertex count exceeds a cap drops the overflow (see [`render`]), starting
//     at busier coarse zooms than on the host. These are stopgap sizes — the shipping 512 KB
//     nRF54LM20 re-decides them (generously) when it arrives.
// On `nrf-mem` even the single-feature decode buffers (`MAX_DECODE_*`) are trimmed below the
// format's per-feature bound — see the truncation note at [`MAX_DECODE_POINTS`].

/// Maximum visible features per frame (each is a [`Span`] — 14 bytes). Saturates first at coarse
/// zoom (many small features).
#[cfg(not(feature = "nrf-mem"))]
pub const MAX_SPANS: usize = 3072;
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
pub const MAX_FRAME_POINTS: usize = 12_288;
#[cfg(feature = "nrf-mem")]
pub const MAX_FRAME_POINTS: usize = 768;

/// Maximum total ring entries across all visible features per frame.
#[cfg(not(feature = "nrf-mem"))]
pub const MAX_FRAME_RINGS: usize = 3072;
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
pub const MAX_SCREEN_POINTS: usize = 4096;
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
/// the MCU.) ~200 KB on the full profile, ~30 KB on `nrf-mem`.
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
    /// Quadtree leaves overlapping the viewport this frame (uncapped).
    pub chunks_visited: usize,
    pub features_tried: usize,
    pub features_drawn: usize,
    pub features_dropped: usize,
    pub points_tried: usize,
    pub points_drawn: usize,
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
    /// Streamed-map cache accounting for this frame. The `Reader` re-walks the visible chunks once
    /// per priority level; `map_chunk_hits` are passes served from a resident cache slot,
    /// `map_chunk_misses` the ones that read from SD. `map_sd_reads` / `map_bytes_read` are the raw
    /// source overhead (index blocks + chunk fills). Hit rate is `hits / (hits + misses)`.
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

        self.frame.spans.sort_unstable_by_key(|s| (s.z, s.seq));
        let t_sorted = clock.now_us();

        self.draw_map(target, vp, &color_fn);
        let t_drawn = clock.now_us();

        // The clear is a framebuffer write, so it counts toward `draw` even though it ran first.
        // `saturating_sub` guards a momentarily non-monotonic clock; a frame is well under a
        // second, so the `u32` µs casts never truncate.
        stats.collect_us = t_collected.saturating_sub(t_cleared) as u32;
        stats.sort_us = t_sorted.saturating_sub(t_collected) as u32;
        stats.draw_us = (t_cleared.saturating_sub(t0) + t_drawn.saturating_sub(t_sorted)) as u32;

        stats
    }

    /// Draw the collected, painter-ordered spans into `target`. Polygons fill via even-odd
    /// scanline; lines stroke via the view-clipped overlay path.
    fn draw_map<D, F>(&mut self, target: &mut D, vp: &Viewport, color_fn: &F)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        // Disjoint borrows: spans/geometry read from `frame`, draw scratch written.
        let Self { frame, draw } = self;
        for span in frame.spans.iter() {
            let ring_start = span.ring_start as usize;
            let pt_start = span.pt_start as usize;
            let ring_lens = &frame.frame_ring_lens[ring_start..ring_start + span.ring_count as usize];
            let total: usize = ring_lens.iter().sum();
            let pts = &frame.frame_points[pt_start..pt_start + total];
            let color = color_fn(span.color);

            match span.kind {
                Kind::Polygon => fill_polygon_proj(target, vp, pts, ring_lens, color, &mut draw.screen, &mut draw.xs),
                Kind::Line => {
                    // Lines use only the exterior ring.
                    let n = ring_lens.first().copied().unwrap_or(0);
                    draw_line(target, vp, &pts[..n], color, span.weight.max(1) as u32, &mut draw.screen, &mut draw.xs);
                }
            }
        }
    }
}
