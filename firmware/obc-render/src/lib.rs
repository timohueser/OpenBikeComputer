//! Shared map renderer (feature `render`).
//!
//! Generic over `embedded-graphics`' [`DrawTarget`], so the host (SDL
//! `SimulatorDisplay`) and the device (LS021B7DD02) share the same projection,
//! LOD selection, painter ordering, polygon fill and line drawing.
//!
//! [`RenderScratch`] owns every scratch buffer and clears (not frees) them each
//! frame, so steady-state rendering does no heap allocation. Geometry math uses
//! `libm` for `no_std`.
//!
//! **Config and scratch are two things (#1146).** [`RenderScratch`] is *only* per-frame working
//! memory — buffers written before they are read, meaningless between frames. Everything that
//! decides what a frame looks like travels as a per-call [`RenderConfig`] argument, and **no sticky
//! setting is ever a field of the scratch**. That rule is structural, not stylistic: the scratch is
//! headed for an arena the caller lends the render path one frame at a time, so a setting parked in
//! it would quietly reset when the arena is re-lent (or, worse, arrive carrying another owner's
//! choice). A caller that wants a switch to persist keeps it in its own state and re-states it each
//! frame — which is exactly what the Map screen does with the rider's contour switch.

#![no_std]

use core::mem::ManuallyDrop;

use heapless::Vec;

use embedded_graphics::prelude::*;

use obc_map_scene::{Diagnostics, Kind, MapScene, ReadError};

pub mod canvas;
mod collect;
mod fill;
mod font_data;
mod overlay;
mod rain;
mod stroke;
pub mod surface;
pub mod text;
mod viewport;
pub use canvas::{rect, Canvas};
pub use overlay::{OverlayChunk, RouteOverlaySource};
pub use rain::{
    rain_in_regime, rain_min_zoom, rain_style, RainGrid, RainOverlaySource, RainSampling, RAIN_BELOW_Z,
    RAIN_MAX_CELL_STEP, RAIN_SAMPLING, RAIN_STYLE, RAIN_TILE_CELLS, RAIN_TILE_EDGE, RAIN_TILE_SLOTS,
};
pub use surface::Surface;
pub use text::{draw_text, glyph_supported, text_width, Font, TextAlign};
pub use viewport::{mpp_for_zoom, round_coord, zoom_for_mpp, Viewport};

use collect::{FrameScratch, ScreenPoint, Span};
use fill::{fill_polygon_edges, PackedEdge};
use stroke::{draw_line, Stroker};

// Per-frame buffer capacities. Statically allocated (heapless::Vec); growing one costs boot
// RAM, not per-frame. One profile for everything: the shipping 512 KB nRF54LM20 budget, which the
// simulator and tests build too, so they render exactly what the device will — features start
// dropping at the same busy coarse zooms (deliberate: an over-dense frame is slow on-glass, so the
// sim shows that limit rather than an unattainable host-fidelity map). The renderer scratch
// (`MCU_SCRATCH_BYTES` below, under 128 KiB) is sized alongside the 75 KB RGB222 framebuffer, the
// map/route caches, the on-device router, the BLE stack (issue #270 — map + BLE share one image),
// and a ~75 KB stack reserve. The board crate's budget assert is the binding fit check.
//
// **Where the current numbers came from (#1146 P3).** The board's scratch arena made these caps one
// arm of a three-way union (render ⊥ nav ⊥ usb), and max-of-arms accounting freed ~76 KB of
// resident RAM; P3 spent ~25 KB of that back here, where it buys visible map. The USB arm later grew
// to 128 KiB and became the maximum. This coarse-LOD work phase-shares the two per-feature point
// buffers and spends the resulting headroom on frame points without moving that 128 KiB ceiling
// (`firmware/obc-fw-nrf54l/src/arena.rs` explains the cliff).
// (The old `nrf-mem` feature — the culled 256 KB nRF54L15-DK profile — was deleted when the LM20
// hardware arrived; its history lives in git and the #677/#270 discussions.)

/// Capacity of pass-A's candidate reservoir — every stub the collector may hold before `select()`
/// picks the winners (a slot is `Span`-sized, 12 bytes). It is the frame's candidate/feature
/// ceiling, while its surplus over a typical selected frame buys *backfill* — lower-priority
/// candidates remain available to take a slot after a large feature is skipped on the point or ring
/// budget. A frame draws at most `min(MAX_SPANS, MAX_FRAME_RINGS)` features.
/// This PR packs the candidate metadata and removes the span's redundant resolved color. The
/// packed screen-space frame vertices below free enough bytes to hold 3,072 candidates while
/// staying inside the same arena arm.
pub const MAX_SPANS: usize = 3072;

/// Maximum total retained vertices across all visible features per frame — one of the two budgets
/// `select()` actually enforces. These are already-projected signed-16-bit screen coordinates
/// ([`ScreenPoint`]), four bytes per vertex instead of the former eight-byte map coordinates. The
/// recovered bytes are reinvested here and in the span/ring reservoirs; 16,323 is deliberately the
/// exact capacity that fills the board's 128 KiB arena arm after alignment rather than leaving a
/// second, smaller render limit inside it.
pub const MAX_FRAME_POINTS: usize = 16323;

/// Maximum total ring entries across all visible features per frame. Every admitted feature costs
/// at least one ring, `Kind::Line` included:
/// `select()` charges `ring_count`, a candidate with empty `ring_lens` is rejected outright
/// (`Feature::has_valid_rings`), and `ring_count == 0` is reserved as pass-B's failure sentinel.
/// So no frame draws more features than `min(MAX_SPANS, MAX_FRAME_RINGS)`, however much point room
/// is left over. Ring lengths are `u16`, which halves their MCU storage; the crossing-buffer
/// packed-coordinate rebalance raises this cap to 3,328, above the candidate reservoir because
/// polygons may contribute more than one ring.
pub const MAX_FRAME_RINGS: usize = 3328;

/// Maximum vertices for a single feature during decode (reused per feature). Equals the OBCM
/// production source's maximum feature size — full format fidelity.
pub const MAX_DECODE_POINTS: usize = 2048;

/// Maximum rings for a single feature during decode. Matches the production source bound.
pub const MAX_DECODE_RINGS: usize = 32;

/// Maximum screen points buffered while drawing one feature. Polygon fills unpack every retained
/// [`ScreenPoint`] into this buffer; the stroker reuses it for its current clipped run, so it must
/// hold a whole decode buffer (invariant asserted below).
pub const MAX_SCREEN_POINTS: usize = 2048;

/// Maximum scanline crossings buffered for one polygon-fill row. A row whose
/// outline crossings exceed this is skipped rather than mis-filled (see
/// [`fill_polygon`]) — sized to fit the MCU RAM budget asserted below, not the worst-case comb
/// (which could approach [`MAX_SCREEN_POINTS`]). No scene in the pinned corpus or A/B fixtures has
/// exceeded 256 crossings on one row. Keeping 384 retains 50% measured headroom; the 1,024 bytes
/// recovered from the former 640-entry insurance cap fund 96 frame points and 128 ring entries,
/// where coarse-volume stress frames demonstrably use them.
pub const MAX_CROSSINGS: usize = 384;

// `Span` packs its buffer offsets into `u16` to stay small, so the frame buffers
// it indexes must fit in a `u16`. These guard that invariant at compile time.
const _: () = assert!(MAX_FRAME_POINTS <= u16::MAX as usize, "Span::pt_start is u16");
const _: () = assert!(MAX_FRAME_RINGS <= u16::MAX as usize, "Span::ring_start is u16");
const _: () = assert!(MAX_SPANS <= u16::MAX as usize, "Span::seq is u16");
// The two caps must not invert: `select()` never counts spans, so the ring cap is the real
// feature ceiling and a reservoir larger than it is the only shape that pays.
const _: () = assert!(
    MAX_FRAME_RINGS >= MAX_SPANS,
    "every feature costs one span and >=1 ring; a ring cap below the span cap makes MAX_SPANS unreachable dead weight"
);

// The draw path unpacks a whole decoded feature into `screen` before walking its rings, so it must
// hold at least a full decode buffer or it indexes past the points and panics.
const _: () = assert!(MAX_SCREEN_POINTS >= MAX_DECODE_POINTS, "`screen` must hold a whole decoded feature");
// These values are pinned by the production-source integration tests.

/// Static RAM a [`RenderScratch`]'s buffers occupy on the 32-bit MCU target (`usize` = 4
/// bytes there). `pub` so a board crate's RAM-budget assert can add it to the framebuffer + caches
/// without re-deriving the formula. (`ScreenPoint` is 4 bytes, `(i32, i32)` / `Point` are 8,
/// frame ring lengths are `u16`, and `usize`/`f32` are 4 on the MCU.) Exactly fills the 128 KiB
/// board arena arm after `RenderScratch`'s alignment, including [`rain::RainScratch`].
pub const MCU_SCRATCH_BYTES: usize = MAX_DECODE_POINTS * 8
    + MAX_DECODE_RINGS * 4
    + MAX_FRAME_POINTS * core::mem::size_of::<ScreenPoint>()
    + MAX_FRAME_RINGS * 2
    + MAX_SPANS * core::mem::size_of::<Span>()
    + MAX_CROSSINGS * 4
    // WX10: the rain overlay's per-frame decoded-tile cache (16 slots, ~4.1 KB; the slots over the
    // original twelve keep a smoothing kernel's wider reach inside one decode per visible tile —
    // see `RAIN_TILE_SLOTS`). It shares the render arm with the buffers above; the board assertion
    // is the byte-accurate authority that the complete arm remains under USB's 128 KiB ceiling.
    + core::mem::size_of::<rain::RainScratch>();
// Loose per-crate ceiling catching an accidental cap blow-up; the binding fit check is the board
// crate's whole-resident-set budget assert.
const _: () = assert!(MCU_SCRATCH_BYTES <= 200 * 1024, "RenderScratch exceeds the 200 KB MCU budget");

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
/// steps keep the width stepping cleanly frame to frame (no sub-pixel shimmer while zooming).
#[inline]
pub(crate) fn scale_weight(weight: u8, scale: f32) -> u32 {
    (libm::roundf(weight as f32 * scale) as i32).clamp(1, MAX_LINE_PX as i32) as u32
}

/// A line span's stroke width in device px: [`scale_weight`] for an ordinary style, the authored
/// `weight` **verbatim** for a *fixed-width* one (OBCM §2 style-record flag bit 4, #1095).
///
/// The ramp models a thing that is genuinely wider on the ground — a motorway is wider than a
/// footpath, and both are wider seen from 1 m/px than from 100. A **mark on the map** has no ground
/// width at all, so for it the ramp is not merely wrong but backwards: authored `weight 1` contours
/// draw 4 px at street zoom (where they do the most damage) and 1 px at planning zoom (where the
/// landform read wants them). Bit 4 is that opt-out, and it is a general style property, not a
/// contour special case — any future mark-like style (a hairline boundary hatch, a grid) takes it.
/// Contours are simply the first shipped style that is one.
///
/// Still clamped to `1..=MAX_LINE_PX`: `weight` is a `u8`, and neither a vanishing 0-px stroke nor a
/// 200-px one that eats the panel is something a style table should be able to ask for.
#[inline]
pub(crate) fn line_px(weight: u8, scale: f32, fixed_width: bool) -> u32 {
    if fixed_width {
        (weight as u32).clamp(1, MAX_LINE_PX)
    } else {
        scale_weight(weight, scale)
    }
}

/// Decode and projected-point storage are phase-exclusive: collection finishes before drawing.
/// Both element types are two `i32`s, so one union-backed `heapless::Vec` serves both phases and
/// lets the frame buffer grow without keeping two redundant 16 KiB per-feature buffers resident.
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
        // Writing a union member is safe and makes this the active member. `Vec::new()` initializes
        // only its empty metadata; the inline MaybeUninit backing is deliberately left untouched.
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

/// The renderer's draw scratch: phase-shared decoded/projected points and the scanline crossings.
#[derive(Default)]
pub(crate) struct DrawScratch {
    points: SharedPoints,
    pub(crate) xs: Vec<f32, MAX_CROSSINGS>,
}

/// A monotonic microsecond clock for **stage timing** inside [`RenderScratch::render_timed`].
///
/// `obc-render` is `no_std` and carries no clock, so a caller wanting the per-stage breakdown
/// (collect / sort / draw) passes one in (the device an embassy-`Instant` clock, a host
/// `std::time::Instant`). The plain [`RenderScratch::render`] path passes the zero-cost
/// [`NoopClock`], leaving the stage fields at `0`.
pub trait Clock {
    /// Microseconds since some fixed, monotonic epoch. Only differences are taken.
    fn now_us(&self) -> u64;
}

/// The zero-cost [`Clock`] for the untimed [`RenderScratch::render`] path: always `0`, so every stage
/// delta is `0` and the optimizer folds the timing away.
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
    /// Quadtree leaves overlapping the viewport this frame (uncapped), summed over **every**
    /// candidate walk the frame performed. A frame that fits walks once, directly. A saturated one
    /// abandons that walk and re-walks it as the stub-select fallback's pass A, so its leaves count
    /// for both — the honest read cost (see [`collect`](crate::collect)). Pass B adds none: it
    /// refetches winners, not leaves, and reports `chunks_refetched`.
    pub chunks_visited: usize,
    pub features_tried: usize,
    pub features_drawn: usize,
    /// Complete features rejected by the fixed span/point/ring frame budgets.
    pub features_dropped: usize,
    /// Features consumed whole but rejected because decode scratch could not hold every point/ring.
    pub feature_decode_capacity_drops: u32,
    /// Structurally invalid feature records consumed without publishing partial geometry.
    pub malformed_features: u32,
    /// Structural map/index/chunk-reference corruption outside an individual feature record.
    pub map_structure_failures: u32,
    /// Backing-medium failures while walking indexes or loading geometry chunks.
    pub map_read_failures: u32,
    /// Legal cache re-entry/contention outcomes; these never panic through the safe API.
    pub map_cache_contentions: u32,
    pub points_tried: usize,
    pub points_drawn: usize,
    /// Stub-select accounting (issue #564). `stub_evictions`: pass-A stub-buffer overflows where a
    /// higher-priority candidate displaced the lowest-priority resident stub (only under span
    /// saturation). `chunks_refetched`: distinct chunks that owned an admitted pass-B winner,
    /// whether decoded from a resident cache slot or re-read after eviction.
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
    // How the drawn frame's span / point / ring scratch splits between the line and polygon render
    // paths — which render path is eating the budget at the zoom levels that saturate it. Counted
    // for free as [`collect`](crate::collect) publishes each drawn feature's geometry, so
    // `line_* + poly_*` equals the totals behind `*_utilization`.
    pub line_spans: usize,
    pub line_points: usize,
    pub line_rings: usize,
    pub poly_spans: usize,
    pub poly_points: usize,
    pub poly_rings: usize,
    /// Streamed-map cache accounting for this frame. Direct collection reads each visible chunk
    /// once; the saturated stub-select fallback may refetch admitted winners in pass B.
    /// `map_chunk_hits` are requests served from RAM and `map_chunk_misses` the ones
    /// that read from SD. `map_sd_reads` / `map_bytes_read` are the raw source overhead (index
    /// blocks + chunk fills). Hit rate is `hits / (hits + misses)`.
    pub map_chunk_hits: u32,
    pub map_chunk_misses: u32,
    pub map_sd_reads: u32,
    pub map_bytes_read: u32,
    /// Host-measured wall time for the whole frame draw (render + overlays), µs; `0` = not measured.
    /// Filled by the host after timing the draw (sim uses `Instant`, device the DWT cycle counter).
    pub render_us: u32,
    /// Per-stage wall time of the **map** render, µs — filled by
    /// [`render_timed`](RenderScratch::render_timed) from the caller's [`Clock`]; `0` on the untimed
    /// path. `collect_us` = visible-feature collection (walk + read + decode + cull + span build),
    /// `sort_us` = painter's-order span sort, `draw_us` = full-screen clear + rasterization. Base
    /// map only; overlays run after `render` returns, so overlay time is
    /// `total − (collect_us + sort_us + draw_us)`.
    pub collect_us: u32,
    pub sort_us: u32,
    pub draw_us: u32,
    /// Rain overlay accounting (WX10): tiles decoded through the per-frame cache (== the source's
    /// own fetch count; each visible tile at most once per frame), pixels actually painted, and the
    /// overlay's wall time in µs (inside `draw_us`, timed only on the rain-lending path).
    pub rain_tiles: u32,
    pub rain_px: u32,
    pub rain_us: u32,
    /// The rain overlay was lent but declined to draw: outside its zoom regime
    /// ([`RAIN_MAX_CELL_STEP`]) or degenerate/overflowing grid geometry. The owning screen must
    /// surface this as its explicit out-of-regime state — a frame with this flag set must never
    /// be presented as a dry map ([`rain_in_regime`] is the same predicate, queryable up front).
    pub rain_out_of_regime: bool,
}

/// What a render call should draw — the presentation switches, stated **per frame** by the caller.
///
/// The Config half of #1146's Config/Scratch split (see the crate docs): every knob that changes
/// what a frame looks like lives here and travels as an argument, so [`RenderScratch`] stays pure
/// working memory and no setting can be smuggled between frames inside it. A caller that wants a
/// switch to stick owns that state itself and re-states it each frame.
///
/// [`Default`] is "draw everything" — the config a caller with no opinion passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderConfig {
    /// Draw the **terrain layer** — every style carrying
    /// [`StyleFlags::terrain_layer`](obc_map_scene::StyleFlags::terrain_layer) (today: the E3
    /// contour styles, #1095). `true` (the [`Default`]) draws it.
    ///
    /// A hidden layer's features are dropped in the collect pass's visible-style mask, so their
    /// geometry is never decoded — they cost no frame budget rather than being drawn and painted
    /// over. What this does *not* do is skip any I/O: the map's cells interleave terrain with
    /// everything else, so the same chunks are read either way (#1096).
    ///
    /// **Provisional (#1096).** This exists so the #1097 ride review can A/B contours on the same
    /// ride; it is expected to be removed either way that review lands — this field and the mask
    /// branch in `collect` are the whole of it.
    pub terrain_layer: bool,
}

impl Default for RenderConfig {
    fn default() -> Self {
        // Shown, not hidden: a caller that has never heard of the toggle gets the whole map.
        RenderConfig { terrain_layer: true }
    }
}

/// The reusable **per-frame scratch** of the render path: every decode / collect / draw buffer.
/// Construct once, hand `&mut` to [`render`](RenderScratch::render) per frame; buffers are cleared
/// and reused, so steady-state rendering allocates nothing.
///
/// Pure working memory, and only that — see the crate docs' Config/Scratch rule. Nothing here
/// decides what a frame looks like ([`RenderConfig`] does), and nothing here means anything between
/// frames: every buffer is written before it is read.
///
/// **Never construct one by value on a device stack.** It is 128 KiB of `heapless::Vec`s
/// ([`MCU_SCRATCH_BYTES`]); a by-value constructor only stays off the stack via return-value
/// optimization, a guarantee a debug build or a different toolchain can decline — the way
/// `RouteIndex::read_into` earned its own in-place constructor after a STKOF HardFault. The device
/// places it with [`init_zeroed`](RenderScratch::init_zeroed); [`new`](RenderScratch::new) /
/// [`Default`] are for hosts and tests, whose stacks are not 36 KB.
#[derive(Default)]
pub struct RenderScratch {
    /// Collection scratch + the frame buffers (decode → cull → spans).
    frame: FrameScratch,
    /// Draw scratch (projected points / polyline runs + scanline crossings), shared by the map
    /// draw phase and the marker/route/breadcrumb overlays.
    pub(crate) draw: DrawScratch,
    /// The rain overlay's per-frame decoded-tile cache (WX10) — reset at overlay start, so like
    /// every other buffer here it is written before it is read and carries nothing between frames.
    rain: rain::RainScratch,
}

impl RenderScratch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Initialize a scratch **in place** at `slot` as the empty, ready-to-render state — the MCU
    /// placement path, building the resident scratch straight into a fixed RAM region without ever
    /// materializing its 128 KiB of buffers on the stack.
    ///
    /// Every buffer is a [`heapless::Vec`], whose empty state (`len = 0` over an uninitialized
    /// backing array) is exactly the all-zero bit pattern, so `write_bytes(0, 1)` lowers to a
    /// `memset` with no temporary and no reliance on return-value optimization.
    ///
    /// # Safety
    /// `slot` must be valid for writes, aligned, and exclusively owned for the call.
    /// On return the slot holds a fully initialized, empty [`RenderScratch`].
    pub unsafe fn init_zeroed(slot: *mut Self) {
        // SAFETY: a scratch is only `heapless::Vec`s plus the rain tile cache's plain arrays — no
        // references, no non-zero-discriminant enum, and (since #1146 moved the terrain switch into
        // `RenderConfig`) no settings at all — so the all-zero bit pattern is the empty scratch:
        // `len = 0` over write-before-read buffers, and an all-empty (`key = 0`) rain cache. The
        // caller guarantees a valid, owned, aligned slot.
        unsafe { slot.write_bytes(0u8, 1) }
    }

    /// Render the visible map into `target`, as `cfg` asks.
    ///
    /// Selects the LOD for the viewport's meters-per-pixel, clears to `bg`, collects visible
    /// features in global priority order ([`FrameScratch::collect`]), orders them by style z-index
    /// (painter's algorithm) and draws polygons (even-odd scanline fill) and lines. `color_fn` maps
    /// a style's RGB565 to the target's pixel color (host chooses true-color vs. device
    /// quantization). A caller with no presentation opinion passes [`RenderConfig::default`].
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

    /// Like [`render`](RenderScratch::render) but fills the per-stage timings on the returned
    /// [`RenderStats`] from `clock`. Base map only; see the [`RenderStats`] stage-field docs.
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
        self.render_rain_timed(target, scene, vp, bg, cfg, None, color_fn, clock)
    }

    /// Like [`render_timed`](RenderScratch::render_timed), with the optional **rain overlay**
    /// (WX10): when `rain` is `Some`, the precipitation raster is drawn inside the base-map paint
    /// order — after every span below [`RAIN_BELOW_Z`] (the ground fills) and before the road band
    /// and everything above it — through the format-agnostic [`RainOverlaySource`] seam. `None` is
    /// **byte-identical** to [`render_timed`](RenderScratch::render_timed): the rain path is not
    /// entered at all.
    #[allow(clippy::too_many_arguments)]
    pub fn render_rain_timed<D, F, S>(
        &mut self,
        target: &mut D,
        scene: &S,
        vp: &Viewport,
        bg: D::Color,
        cfg: RenderConfig,
        rain: Option<&mut dyn RainOverlaySource>,
        color_fn: F,
        clock: &dyn Clock,
    ) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: MapScene,
    {
        self.render_rain_sampled_timed(target, scene, vp, bg, cfg, rain, RAIN_SAMPLING, color_fn, clock)
    }

    /// [`render_rain_timed`](RenderScratch::render_rain_timed) with the overlay's spatial sampling
    /// mode passed in rather than taken from [`RAIN_SAMPLING`].
    ///
    /// Every shipped caller goes through `render_rain_timed`, so [`RAIN_SAMPLING`] stays the one
    /// switch that decides what a rider sees. This exists for the two callers that must span the
    /// modes rather than obey the const: the host binary that renders one frame in all four for a
    /// side-by-side look round, and `obc-app`'s
    /// `the_decision_path_is_identical_in_every_sampling_mode`, which proves no display mode can
    /// move a claim (OBCW §5, OBCG §6).
    #[allow(clippy::too_many_arguments)]
    pub fn render_rain_sampled_timed<D, F, S>(
        &mut self,
        target: &mut D,
        scene: &S,
        vp: &Viewport,
        bg: D::Color,
        cfg: RenderConfig,
        rain: Option<&mut dyn RainOverlaySource>,
        rain_sampling: RainSampling,
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
            // An empty semantic scene is a valid background-only render. In particular, do not
            // call LOD selection or enter draw paths that use `lod_count - 1`.
            return RenderStats { draw_us: clock.now_us().saturating_sub(t0) as u32, ..Default::default() };
        }
        let requested_lod = scene.select_lod_for_mpp(vp.meters_per_pixel());
        let lod = requested_lod.min(lod_count - 1);
        let is_finest = lod == lod_count - 1;
        let mut stats = RenderStats { lod, ..Default::default() };
        if requested_lod >= lod_count {
            stats.map_structure_failures = 1;
        }

        // Collect → painter's order → draw. `seq` is the stable, alloc-free tie-break within a
        // z-index. Snapshot the streamed-map cache counters across `collect` (the only phase that
        // reads the map source) and record the per-frame delta — robust whether the caller hands us
        // a fresh source adapter each frame or a reused one.
        let before = diagnostics(scene, &mut stats, Diagnostics::default());
        {
            // Collection and drawing are disjoint phases. Decode through the shared point backing
            // now; `draw_map` reinterprets the same empty backing as projected screen points later.
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

        self.draw_map(target, scene, is_finest, vp, &color_fn, rain, rain_sampling, clock, &mut stats);
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
    /// complete style metadata (`dashed`/`color2`) from `scene` via [`Span::style_id`].
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
    /// step 2 outright (`is_finest` gate). Polygons are never cased (that's #560).
    #[allow(clippy::too_many_arguments)]
    fn draw_map<D, F, S>(
        &mut self,
        target: &mut D,
        scene: &S,
        is_finest: bool,
        vp: &Viewport,
        color_fn: &F,
        rain: Option<&mut dyn RainOverlaySource>,
        rain_sampling: RainSampling,
        clock: &dyn Clock,
        stats: &mut RenderStats,
    ) where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: MapScene,
    {
        // Disjoint borrows: spans/geometry read from `frame`, draw scratch written to `draw`.
        let Self { frame, draw, rain: rain_scratch } = self;
        let spans = frame.spans();

        // One zoom→width multiplier for the whole frame (#579, `width_scale`): a style's nominal
        // `weight` ramps to its on-screen px width via `scale_weight`, so roads thicken zoomed in and
        // thin zoomed out. The casing pass derives its width from this same ramped value.
        let wscale = width_scale(vp.meters_per_pixel());

        // Two 256-bit style masks, built once per frame in **one** walk of the id space (both mirror
        // `collect`'s `vis_mask` shape) and threaded into both `draw_spans` calls so neither
        // rebuilds them. They nest: a `color2` is what makes a style interesting at all.
        //
        // - **cased** (#557) — a style is cased ⇔ it's a **solid** line (`!dashed`) carrying a
        //   `color2`. Dashed + color2 is the railway stripe (#558), which never cases.
        // - **outlined** (#560) — a style is polygon-outline-eligible ⇔ it carries a `color2` at
        //   all (`line_style`/`dashed` are irrelevant for polygons: `draw_spans`'s outline pass
        //   filters to `Kind::Polygon`, so a cased *line* sharing this bit never triggers an
        //   outline). An empty mask makes each `draw_spans` take today's exact single-loop path
        //   (byte-identical, zero cost).
        let (mut cased_mask, mut outlined_mask) = ([0u32; 8], [0u32; 8]);
        for id in 0..=255u8 {
            if let Some(s) = scene.style(id) {
                if s.color2.is_some() {
                    let (word, bit) = ((id >> 5) as usize, 1u32 << (id & 31));
                    outlined_mask[word] |= bit;
                    if !s.flags.dashed() {
                        cased_mask[word] |= bit;
                    }
                }
            }
        }
        let any_cased = cased_mask.iter().any(|&w| w != 0);
        let is_cased = |sid: u8| cased_mask[(sid >> 5) as usize] & (1 << (sid & 31)) != 0;

        // **Rain overlay insertion (WX10).** With a rain source lent, everything strictly below
        // [`RAIN_BELOW_Z`] — the ground fills, water, buildings, terrain — paints first, then the
        // dithered precipitation raster, then the rest of the frame (the road band upward) on top
        // of it. `None` keeps `rain_at == 0`: the scan is skipped, the slice below is empty, and
        // the frame draws exactly today's path, byte for byte. (A skin that cased a line *below*
        // the rain boundary would lose that casing's under-stroke on rain frames only — no shipped
        // or plausible skin does; the road band starts at z 24 and every cased style sits in it.)
        let rain_at = match rain {
            Some(_) => spans.iter().position(|s| s.z >= rain::RAIN_BELOW_Z).unwrap_or(spans.len()),
            None => 0,
        };
        if let Some(source) = rain {
            Self::draw_spans(
                frame,
                draw,
                target,
                scene,
                is_finest,
                vp,
                color_fn,
                wscale,
                &outlined_mask,
                &spans[..rain_at],
            );
            let t_rain = clock.now_us();
            rain::draw_rain(target, vp, rain_scratch, source, color_fn, stats, rain_sampling);
            stats.rain_us = clock.now_us().saturating_sub(t_rain) as u32;
        }
        let spans = &spans[rain_at..];

        // The z boundary: the first cased road **line** span. Everything before it is the low-z band
        // (land / water / landuse / buildings / low-z lines). No cased style ⇒ `split == spans.len()`,
        // so the scan is skipped and the two ranges below collapse to one full pass (today's path).
        let split = if any_cased {
            spans.iter().position(|s| s.kind == Kind::Line && is_cased(s.style_id)).unwrap_or(spans.len())
        } else {
            spans.len()
        };

        // (1) Everything below the road band, exactly as the base pass.
        Self::draw_spans(frame, draw, target, scene, is_finest, vp, color_fn, wscale, &outlined_mask, &spans[..split]);

        // (2) Casing pass — finest LOD only. Each cased road strokes a solid `color2` base at the
        // **ramped** fill width + `2*CASING_PX` (tracks the #579 zoom ramp, not a fixed px), under the
        // fills step 3 paints on top. Reuses the collected screen points and `DrawScratch`.
        if is_finest {
            for span in &spans[split..] {
                if span.kind != Kind::Line || !is_cased(span.style_id) {
                    continue;
                }
                // A line uses only its exterior (first) ring — the leading `n` frame points.
                let n = frame.frame_ring_lens[span.ring_start as usize] as usize;
                let pt_start = span.pt_start as usize;
                let pts = &frame.frame_points[pt_start..pt_start + n];
                // `is_cased` guarantees `color2.is_some()`; quantize it like the fill color. The
                // `unwrap_or` is a defensive no-op (falls back to an invisible same-color casing).
                let style = scene.style(span.style_id);
                let casing_color =
                    color_fn(style.and_then(|s| s.color2).unwrap_or_else(|| style.map_or(0, |s| s.color)));
                // Casing is defined *relative to the fill* — "the fill's width plus one px a side" —
                // so it composes with #1095's fixed width rather than being special-cased against
                // it: a fixed-width cased style would case its verbatim `weight`. No shipped style
                // is both (a contour carries no `color2`, and `is_cased` requires one), so this is
                // dormant today; leaving `scale_weight` here would make it silently incoherent.
                let fixed_width = style.is_some_and(|s| s.flags.fixed_width());
                draw_line(
                    target,
                    vp,
                    pts,
                    casing_color,
                    line_px(span.weight, wscale, fixed_width) + 2 * Self::CASING_PX,
                    false,
                    None,
                    draw.points.screen(),
                );
            }
        }

        // (3) The road band and above, exactly as the base pass, on top of the casings.
        Self::draw_spans(frame, draw, target, scene, is_finest, vp, color_fn, wscale, &outlined_mask, &spans[split..]);
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
        // Outlines exist only at the finest LOD and only when some style carries a `color2`. Neither
        // holds ⇒ today's exact single pass (byte-identical, and no group-boundary scan).
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

            // pass 1 — every span exactly as the base pass, tracking whether this group needs pass 2.
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

    /// Draw one span exactly as the base pass: a polygon even-odd fill, or a line's view-clipped stroke
    /// with its resolved `dashed`/`color2` style at the frame's ramped width (`wscale`, #579). The
    /// unit of pass 1 in [`draw_spans`](Self::draw_spans), unchanged from the pre-#560 single loop.
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
                // Lines use only the exterior ring. Re-resolve the style for `dashed`/`color2`;
                // `color2` quantizes through `color_fn` exactly like the primary. A missing
                // style (never collected) falls back to today's solid stroke.
                let n = ring_lens.first().copied().unwrap_or(0) as usize;
                let style = scene.style(span.style_id);
                let dashed = style.is_some_and(|s| s.flags.dashed());
                let color2 = style.and_then(|s| s.color2).map(color_fn);
                // #1095: a fixed-width style strokes its authored `weight` verbatim (`line_px`); a
                // missing style falls back to the ramp, exactly as it falls back to a solid stroke.
                let fixed_width = style.is_some_and(|s| s.flags.fixed_width());
                draw_line(
                    target,
                    vp,
                    &pts[..n],
                    color,
                    line_px(span.weight, wscale, fixed_width),
                    dashed,
                    color2,
                    points.screen(),
                );
            }
        }
    }

    /// Stroke a polygon span's rings — **exterior and every hole (courtyards)** — **closed** (first
    /// point repeated) in its style's `color2`, at a **fixed hairline** width `weight.max(1)`. The #560
    /// finest-LOD outline: called from [`draw_spans`](Self::draw_spans)'s pass 2 for a span the
    /// `outlined_mask` already vetted (`color2.is_some()`). Reuses `DrawScratch` — no new buffers; each
    /// ring reuses its collected screen coordinates. At the preset `weight 1` this is the thin Bresenham
    /// polyline path.
    ///
    /// **Fixed, not ramped.** A line's *stroke* ramps with zoom ([`scale_weight`], #579), but a
    /// building outline is a **1-px edge accent**, not a road: ramped, it hits 3–4 px at the finest LOD
    /// where the ground scale is sub-metre, and a closed ring stroked that thick (round joins + a disc
    /// per sharp corner) floods a small footprint — the fill drowns and the building reads as a dark
    /// slab (measured: outline `color2` pixels ≫ fill pixels). A fixed `weight.max(1)` keeps the wall a
    /// crisp hairline at every finest-LOD zoom, which is the whole point of the feature.
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
        // `outlined_mask` guarantees `color2.is_some()`; the `unwrap_or` is a defensive no-op that
        // falls back to an invisible same-color outline rather than panicking.
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
            // Stroke the ring **closed**: chain the first vertex again so the wall between the last
            // and first point is drawn. The collector already projected each retained vertex.
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

    /// #1095, flag bit 4: a fixed-width style renders its authored `weight` verbatim at **every**
    /// zoom, while the same weight on an ordinary style rides the ramp. These are the three zooms
    /// the E3 contour frames are made at (planning / riding / street, ~9 / 4 / 1 m/px).
    #[test]
    fn fixed_width_ignores_the_zoom_ramp() {
        for mpp in [1000.0, 9.0, 4.0, 1.0, 0.05] {
            let s = width_scale(mpp);
            assert_eq!(line_px(1, s, true), 1, "a weight-1 contour is a hairline at {mpp} mpp");
            assert_eq!(line_px(3, s, true), 3, "a weight-3 fixed style is 3 px at {mpp} mpp");
        }
        // ...and the ramp is still the ramp for everything that does not opt out: the same authored
        // weight 1 is 2 px at riding zoom and 4 px at street zoom, which is what bit 4 opts out of.
        assert_eq!(line_px(1, width_scale(4.0), false), 2);
        assert_eq!(line_px(1, width_scale(1.0), false), 4);
    }

    /// A fixed width is still a *width*: `weight` is a `u8` and the framebuffer is 240 px, so the
    /// same `1..=MAX_LINE_PX` clamp the ramp carries applies verbatim — bit 4 opts out of the zoom
    /// ramp, not out of the panel.
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

// ci-speed probe: measuring the gate on a representative Rust change.
// second probe touch.
