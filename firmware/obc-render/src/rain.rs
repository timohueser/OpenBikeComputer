//! The precipitation raster overlay (WX10, epic #1185): nearest-neighbour provider cells,
//! RGB222-targeted colors, ordered-Bayer *transparency* dithering.
//!
//! Drawn inside the base-map paint order — after the low-z ground fills (land / water / landuse /
//! buildings / terrain) and **before the road band**, so roads, route, rider and compass always
//! render above rain ([`RAIN_BELOW_Z`]). The renderer stays ignorant of the OBCW byte format: rain
//! arrives through the [`RainOverlaySource`] seam as a regular lat/lon grid of 4-bit intensity
//! cells in 16 × 16 tiles, exactly the shape `obc-weather` serves off SD.
//!
//! **Semantics are locked (epic #1185, "Locked UX"):**
//! - Sampling is direct nearest-neighbour on provider cells, mapped with per-pixel fixed-point
//!   increments. No bilinear, no supersampling, no contouring — no fabricated precision.
//! - The Bayer matrix is applied **only as transparency**: a selected pixel shows the rain color,
//!   every other pixel keeps the already-rendered basemap. It never mixes or smooths values.
//! - The intensity → color/coverage table is **firmware-owned** ([`rain_style`]): semantic
//!   intensity, not cartography. It lives here, next to the code that draws it, and a map skin
//!   change can never alter it.
//! - `dry`, the reserved codes and `no-data` all draw **nothing**. Whether missing data may be
//!   *claimed* dry is decision logic and lives with the weather screens, never here.

use embedded_graphics::prelude::*;

use crate::viewport::Viewport;
use crate::RenderStats;

/// Tile edge/cell counts of the seam's raster tiles. These mirror the OBCW §5 tile shape
/// (`obc_formats::obcw::TILE_EDGE`); the `obc-app` adapter const-asserts the two never drift.
pub const RAIN_TILE_EDGE: usize = 16;
pub const RAIN_TILE_CELLS: usize = RAIN_TILE_EDGE * RAIN_TILE_EDGE;

/// The z-index boundary the rain overlay draws **below**: spans with `z >= RAIN_BELOW_Z` (the road
/// band and everything above it) paint over rain; spans below it (the ground fills) paint under.
///
/// Skins **do** carry `z_index` — the boundary holds because the z ladder's band gap `(16, 24)`
/// is a *contract*, enforced where skins are stamped and pinned over the shipped presets; see the
/// authority's docs at [`obc_map_scene::RAIN_BELOW_Z`] (re-exported here so render callers keep
/// one path).
pub use obc_map_scene::RAIN_BELOW_Z;

/// Decoded-tile slots the per-frame [`RainScratch`] cache holds. Within the overlay's zoom regime
/// ([`RAIN_MAX_CELL_STEP`], grid-axis row norms ≤ 1/3 cell/px) a scanline's combined step is at
/// most `√2/3` cells per pixel, so a 240-px row crosses at most `240·√2/(3·16) + 2 ≈ 10` tiles;
/// twelve slots hold that worst-case staircase **plus** the neighbouring rows' overlap, so a
/// tile's whole contiguous span of use stays resident and every visible tile decodes exactly once
/// per frame at any heading anywhere in the regime (pinned across the full reachable m/px range —
/// including the 45°-worst-case boundary zooms — by `decode_bound_across_the_reachable_zoom_range`;
/// eight slots measurably re-fetched near the regime edge). The bound that keeps rotation from
/// causing per-pixel SD reads.
pub const RAIN_TILE_SLOTS: usize = 12;

/// The overlay's **zoom regime** cap: the rain raster draws only while each **grid axis** advances
/// at or below this many cells per screen pixel — the Euclidean row norms of the screen→grid
/// Jacobian, `√(dcol_dx² + dcol_dy²)` and `√(drow_dx² + drow_dy²)`. Rotation acts on the *screen*
/// side of that map, so the norms — and with them the regime verdict — are **heading-invariant**:
/// at a fixed zoom the overlay is in or out for every course alike, never popping in and out as a
/// heading-up rider turns (the first per-screen-axis criterion measurably did exactly that
/// through the 250–330 m/px band — delta review of PR #1213). Algebraically the norms reduce to
/// `kx/(zoom·aspect)` and `ky/zoom`: cells per pixel of ground scale along each grid axis.
///
/// `1/3` means every provider cell spans **≥ 3 px along each grid axis** in-regime, which buys
/// two locked properties at once:
///
/// - **no strong cell can vanish** — nearest-neighbour sampling cannot skip a cell wider than a
///   pixel, so a storm core is always hit while the overlay draws at all (the "bounded
///   conservative cell selection" the issue permits is unnecessary inside the regime, and outside
///   it nothing draws rather than something wrong);
/// - **no cache thrash** — a scanline's combined step is at most `√2 × RAIN_MAX_CELL_STEP` cells
///   per pixel (both norms at cap, 45° heading), so a 240-px row crosses at most
///   `240·√2/(3·16) + 2 ≈ 10` tiles, inside the [`RAIN_TILE_SLOTS`]-slot cache — each visible
///   tile decodes once per frame at any heading (test-pinned across the whole reachable m/px
///   range).
///
/// For a 1 km product the regime covers up to ~333 m/px — at **every** heading — comfortably past
/// the locked 50 km view (~208 m/px); coarser products reach proportionally further out. **Out of
/// regime the overlay draws nothing and says so** ([`RenderStats::rain_out_of_regime`],
/// [`rain_in_regime`]): the weather screens (WX11/WX12) must read that signal and show their
/// explicit out-of-regime state — a silent rain-free map must never be read as dry.
pub const RAIN_MAX_CELL_STEP: f64 = 1.0 / 3.0;

/// A rain product's placement: a regular lat/lon grid in integer microdegrees, `width × height`
/// cells spanning the half-open box `[west, east) × [south, north)`, row 0 at the **south** edge —
/// exactly the OBCW §4 frame geometry, so the fixed-point sampler and `obc-weather`'s
/// `cell_index` name the same provider cell for the same coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RainGrid {
    pub west_udeg: i32,
    pub south_udeg: i32,
    pub east_udeg: i32,
    pub north_udeg: i32,
    pub width_cells: u16,
    pub height_cells: u16,
}

/// The rain overlay's view of the active precipitation frame — implemented by the host over the
/// OBCW reader (`obc-app`'s adapter on device and simulator alike). Keeps `obc-render` ignorant
/// of the OBCW format, exactly as [`RouteOverlaySource`](crate::RouteOverlaySource) does for OBCR.
///
/// The implementor owns **freshness**: it must refuse to exist (or return `None` from [`grid`])
/// rather than serve an expired or not-yet-valid frame — stale rain never renders as current.
pub trait RainOverlaySource {
    /// The active frame's grid, or `None` when nothing renderable is available.
    fn grid(&self) -> Option<RainGrid>;

    /// Decode 16 × 16 tile `tile_index` (row-major over the grid's tile columns) into `out`,
    /// intensity codes per OBCW §5 (0 = dry, 1..=12 bands, 15 = no-data). Returns `false` when
    /// the tile is unavailable (SD fault, malformed payload); the renderer then draws those cells
    /// as transparent — a read failure never fabricates weather.
    fn tile(&mut self, tile_index: u32, out: &mut [u8; RAIN_TILE_CELLS]) -> bool;
}

// ---------------------------------------------------------------------------------------------
// THE RAIN TUNING SURFACE — every visual knob of the overlay lives between these two rules.
//
// A look-tuning round edits only this block (colors, coverages, or the matrix) and re-renders;
// nothing else in firmware, app, skins or sim plumbing participates. Constraints that must hold
// (all pinned by `style_table_is_rgb222_exact_and_transparent_codes_are_pinned`):
//   - every color sits exactly on the panel's RGB222 grid — each 8-bit channel in
//     {0, 85, 170, 255}, i.e. RGB565 R/B in {0, 10, 21, 31} and G in {0, 21, 42, 63} — so device
//     quantization is the identity and host / simulator / device frames agree byte-for-byte;
//   - indices 0 (dry), 13/14 (reserved) and 15 (no-data) stay coverage 0: the renderer never
//     invents a "dry look" for missing data;
//   - coverage rises (or the color changes) between adjacent bands, so no two bands look equal.
// ---------------------------------------------------------------------------------------------

/// RGB222-exact palette entries used by [`RAIN_STYLE`] — `(R, G, B)` in 8-bit terms.
const LIGHT_BLUE: u16 = 0x555F; // ( 85, 170, 255) — drizzle
const MID_BLUE: u16 = 0x02BF; // (  0,  85, 255) — rain
const DEEP_BLUE: u16 = 0x0015; // (  0,   0, 170) — heavy rain
const VIOLET: u16 = 0xA81F; // (170,   0, 255) — torrential
const TRANSPARENT: (u16, u8) = (0, 0);

/// The firmware-owned intensity → `(RGB565 color, coverage)` table, indexed by the 4-bit OBCW
/// intensity code (`obc_formats::precip4`: 0 = dry, 1..=12 = the mm/h bands, 15 = no-data).
///
/// `coverage` is in Bayer-16ths and is **transparency, not blending**: `n` paints the cell's rain
/// color on the `n` matrix positions whose [`BAYER`] value is `< n` and leaves every other pixel's
/// basemap untouched; `0` is fully transparent, `16` opaque.
pub const RAIN_STYLE: [(u16, u8); 16] = [
    TRANSPARENT,     //  0  dry — never painted
    (LIGHT_BLUE, 4), //  1  < 0.10 mm/h
    (LIGHT_BLUE, 6), //  2  < 0.25 mm/h
    (LIGHT_BLUE, 8), //  3  < 0.50 mm/h
    (MID_BLUE, 8),   //  4  < 1.0 mm/h
    (MID_BLUE, 10),  //  5  < 2.0 mm/h
    (MID_BLUE, 12),  //  6  < 4.0 mm/h
    (DEEP_BLUE, 10), //  7  < 6.0 mm/h
    (DEEP_BLUE, 12), //  8  < 10 mm/h
    (DEEP_BLUE, 14), //  9  < 16 mm/h
    (VIOLET, 12),    // 10  < 25 mm/h
    (VIOLET, 14),    // 11  < 50 mm/h
    (VIOLET, 16),    // 12  ≥ 50 mm/h
    TRANSPARENT,     // 13  reserved — rejected by the codec, never painted
    TRANSPARENT,     // 14  reserved — rejected by the codec, never painted
    TRANSPARENT,     // 15  no-data — never painted, and never a dry claim
];

/// [`RAIN_STYLE`] as a lookup, for tests and hosts.
pub const fn rain_style(intensity: u8) -> (u16, u8) {
    RAIN_STYLE[(intensity & 0x0F) as usize]
}

/// The fixed 4 × 4 ordered-dither (Bayer) matrix, indexed `[y & 3][x & 3]` in **panel**
/// coordinates. Screen-anchored, so the pattern is deterministic per pixel and never swims with
/// pan/zoom/rotation; a cell's *coverage* is what changes, never the matrix. Part of the tuning
/// surface above — a different pattern (or an 8 × 8 matrix, adjusting the index masks in
/// [`draw_rain`]) is a legal tuning-round edit as long as it stays a permutation.
const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

// ------------------------------- end of the rain tuning surface -------------------------------

/// Fixed-point format of the per-pixel grid walk: Q31.32 in an `i64`. A 240-px row accumulates at
/// most `240 × 2⁻³²` cells of increment rounding — far below any cell boundary's width.
const FP_SHIFT: u32 = 32;
const FP_ONE: f64 = 4_294_967_296.0; // 2^32

/// The per-frame decoded-tile cache — pure scratch (written before read every frame, reset at
/// overlay start), living inside [`RenderScratch`](crate::RenderScratch) so the device pays for it
/// through the scratch arena's `max(arms)` rather than as new resident RAM. Round-robin
/// replacement over [`RAIN_TILE_SLOTS`] slots; failed tiles are cached as failed so a persistent
/// SD fault costs one fetch attempt per tile per frame, never one per pixel.
pub(crate) struct RainScratch {
    /// Slot keys: `tile_index + 1`, `0` = empty — the all-zero state is the empty cache, which is
    /// what keeps [`RenderScratch::init_zeroed`](crate::RenderScratch::init_zeroed) valid.
    keys: [u32; RAIN_TILE_SLOTS],
    /// Whether the keyed slot holds a good decode (`false` = the fetch failed; cells read no-data).
    ok: [bool; RAIN_TILE_SLOTS],
    /// Round-robin replacement cursor.
    next: u8,
    tiles: [[u8; RAIN_TILE_CELLS]; RAIN_TILE_SLOTS],
}

impl Default for RainScratch {
    fn default() -> Self {
        Self {
            keys: [0; RAIN_TILE_SLOTS],
            ok: [false; RAIN_TILE_SLOTS],
            next: 0,
            tiles: [[0; RAIN_TILE_CELLS]; RAIN_TILE_SLOTS],
        }
    }
}

impl RainScratch {
    /// Forget every cached tile — run at overlay start so a frame never reads another frame's
    /// tiles (scratch is per-frame working memory, nothing may persist through it).
    fn reset(&mut self) {
        self.keys = [0; RAIN_TILE_SLOTS];
        self.ok = [false; RAIN_TILE_SLOTS];
        self.next = 0;
    }

    /// The intensity of `cell` inside `tile_index`, fetching the tile through the cache. `None`
    /// when the tile is unavailable (drawn transparent).
    fn cell(
        &mut self,
        source: &mut dyn RainOverlaySource,
        tile_index: u32,
        cell: usize,
        stats: &mut RenderStats,
    ) -> Option<u8> {
        let key = tile_index.wrapping_add(1);
        if let Some(slot) = self.keys.iter().position(|&k| k == key) {
            return self.ok[slot].then(|| self.tiles[slot][cell]);
        }
        let slot = self.next as usize % RAIN_TILE_SLOTS;
        self.next = (self.next + 1) % RAIN_TILE_SLOTS as u8;
        self.keys[slot] = key;
        self.ok[slot] = source.tile(tile_index, &mut self.tiles[slot]);
        stats.rain_tiles = stats.rain_tiles.saturating_add(1);
        self.ok[slot].then(|| self.tiles[slot][cell])
    }
}

/// Convert a setup-time f64 to Q31.32, or `None` for values a viewport that plausibly overlaps the
/// grid can never produce (non-finite, or beyond ±2³⁰ cells) — then the overlay draws nothing
/// rather than wrapping into bogus cells.
fn to_fp(value: f64) -> Option<i64> {
    if value.is_finite() && value.abs() < (1u64 << 30) as f64 {
        Some(libm::round(value * FP_ONE) as i64)
    } else {
        None
    }
}

/// The setup-time affine screen→cell map, in f64: `(col00, row00, dcol_dx, dcol_dy, drow_dx,
/// drow_dy)` at pixel centers, or `None` for a degenerate grid/camera. One derivation shared by
/// [`draw_rain`] and [`rain_in_regime`], so the regime answer a screen shows and the walk the
/// renderer runs can never disagree.
#[allow(clippy::type_complexity)]
fn grid_affine(vp: &Viewport, grid: &RainGrid) -> Option<(f64, f64, f64, f64, f64, f64)> {
    let (width, height) = (grid.width_cells as i64, grid.height_cells as i64);
    let lon_span = grid.east_udeg as i64 - grid.west_udeg as i64;
    let lat_span = grid.north_udeg as i64 - grid.south_udeg as i64;
    if width <= 0 || height <= 0 || lon_span <= 0 || lat_span <= 0 {
        return None;
    }
    let zoom = vp.zoom as f64;
    let aspect = vp.aspect as f64;
    // NaN-safe positivity guard: a degenerate camera draws nothing.
    if zoom.partial_cmp(&0.0) != Some(core::cmp::Ordering::Greater)
        || aspect.partial_cmp(&0.0) != Some(core::cmp::Ordering::Greater)
    {
        return None;
    }
    let (sin_c, cos_c) = (libm::sin(vp.course_rad as f64), libm::cos(vp.course_rad as f64));
    let kx = width as f64 / lon_span as f64; // cells per microdegree of longitude
    let ky = height as f64 / lat_span as f64; // cells per microdegree of latitude
    let (half_w, half_h) = (vp.w as f64 / 2.0, vp.h as f64 / 2.0);
    let col_center = (vp.cam_lon as i64 - grid.west_udeg as i64) as f64 * kx;
    let row_center = (vp.cam_lat as i64 - grid.south_udeg as i64) as f64 * ky;
    let col00 = col_center + (cos_c * (0.5 - half_w) - sin_c * (0.5 - half_h)) / (zoom * aspect) * kx;
    let row00 = row_center + (-sin_c * (0.5 - half_w) - cos_c * (0.5 - half_h)) / zoom * ky;
    let dcol_dx = cos_c / (zoom * aspect) * kx;
    let dcol_dy = -sin_c / (zoom * aspect) * kx;
    let drow_dx = -sin_c / zoom * ky;
    let drow_dy = -cos_c / zoom * ky;
    Some((col00, row00, dcol_dx, dcol_dy, drow_dx, drow_dy))
}

/// Whether the overlay is inside its zoom regime for this viewport and grid — the shared authority
/// behind [`draw_rain`]'s gate and [`RenderStats::rain_out_of_regime`], public so the weather
/// screens (WX11) and decision logic (WX12) ask the *same* predicate before deciding what an
/// absent overlay means. `false` when the per-pixel grid step exceeds [`RAIN_MAX_CELL_STEP`] on
/// either screen axis, or the grid/camera is degenerate. Out of regime the overlay draws nothing —
/// and the screen owning the frame must present that as its explicit out-of-regime state, never as
/// a dry map.
pub fn rain_in_regime(vp: &Viewport, grid: &RainGrid) -> bool {
    let Some((_, _, dcol_dx, dcol_dy, drow_dx, drow_dy)) = grid_affine(vp, grid) else {
        return false;
    };
    regime_steps_ok(dcol_dx, dcol_dy, drow_dx, drow_dy)
}

/// The smallest [`Viewport::zoom`] (pixels per microdegree of latitude) at which the overlay is
/// still inside its zoom regime for `grid`, at `aspect` (the viewport's `cos(lat)` longitude
/// compression) — the inversion of [`rain_in_regime`]'s criterion, and therefore **the rain
/// map's zoom-out clamp** (WX11, owner tuning round 2): the screen clamps `zoom` to this floor
/// so a rider never reaches the out-of-regime state at all; the coarser the product's cells,
/// the further out the clamp allows, exactly as the regime does. Heading-invariant like the
/// criterion itself. `None` for a degenerate grid (the clamp then does not engage and the
/// defensive out-of-regime banner remains the backstop).
///
/// Derivation: the grid-axis row norms reduce to `kx/(zoom·aspect)` and `ky/zoom` (cells per
/// pixel; see [`RAIN_MAX_CELL_STEP`]), so `zoom ≥ max(kx/aspect, ky) / RAIN_MAX_CELL_STEP`.
pub fn rain_min_zoom(grid: &RainGrid, aspect: f32) -> Option<f32> {
    let lon_span = grid.east_udeg as i64 - grid.west_udeg as i64;
    let lat_span = grid.north_udeg as i64 - grid.south_udeg as i64;
    // The aspect gate must refuse NaN (partial_cmp None ≠ Greater), not just non-positives.
    if grid.width_cells == 0
        || grid.height_cells == 0
        || lon_span <= 0
        || lat_span <= 0
        || aspect.partial_cmp(&0.0) != Some(core::cmp::Ordering::Greater)
    {
        return None;
    }
    let kx = grid.width_cells as f64 / lon_span as f64;
    let ky = grid.height_cells as f64 / lat_span as f64;
    let min_zoom = (kx / aspect as f64).max(ky) / RAIN_MAX_CELL_STEP;
    if !(min_zoom.is_finite() && min_zoom > 0.0) {
        return None;
    }
    // The caller clamps `Viewport::zoom` (an f32) to this floor and expects the clamped camera to
    // be IN regime — but [`rain_in_regime`] re-derives the criterion in f64 from that f32, and a
    // nearest-rounding `as` cast lands *below* the true f64 edge about half the time, making the
    // clamp's own zoom evaluate out of regime (adversarial review of #1224, F1: 192/360 swept
    // grid/latitude/heading cases). Return the next f32 whose f64 promotion clears the edge with
    // a hair of margin for the criterion's own f64 rounding (sqrt of squares ~1 ULP of f64 —
    // ten orders below one f32 ULP, so the loop steps at most twice).
    let edge = min_zoom * (1.0 + 1e-9);
    let mut z = min_zoom as f32;
    while (z as f64) < edge {
        z = f32::from_bits(z.to_bits() + 1);
    }
    Some(z)
}

/// The regime criterion on the screen→grid Jacobian: both **grid-axis** row norms at or below
/// [`RAIN_MAX_CELL_STEP`]. Rotation-invariant by construction (see the constant's docs); the one
/// implementation behind [`rain_in_regime`] and [`draw_rain`]'s gate.
fn regime_steps_ok(dcol_dx: f64, dcol_dy: f64, drow_dx: f64, drow_dy: f64) -> bool {
    let col_norm = libm::sqrt(dcol_dx * dcol_dx + dcol_dy * dcol_dy);
    let row_norm = libm::sqrt(drow_dx * drow_dx + drow_dy * drow_dy);
    col_norm <= RAIN_MAX_CELL_STEP && row_norm <= RAIN_MAX_CELL_STEP
}

/// Draw the rain overlay over the already-painted low-z map band.
///
/// One pass over the panel in scan order. The screen→grid transform is affine (the viewport's
/// rotate/scale/translate composed with the grid's linear lat/lon → cell map), so cell coordinates
/// are walked with per-pixel Q31.32 **fixed-point increments** — floor of the accumulator is the
/// nearest-neighbour provider cell, bit-exactly reproducible. Per pixel the steady-state work is
/// two adds and a cell-change compare; inside the zoom regime ([`RAIN_MAX_CELL_STEP`]) tiles are
/// decoded through the per-frame [`RainScratch`] cache at most once per tile per frame (failures
/// included), and outside it the overlay draws nothing and sets
/// [`RenderStats::rain_out_of_regime`].
pub(crate) fn draw_rain<D, F>(
    target: &mut D,
    vp: &Viewport,
    scratch: &mut RainScratch,
    source: &mut dyn RainOverlaySource,
    color_fn: &F,
    stats: &mut RenderStats,
) where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    let Some(grid) = source.grid() else { return };
    // The affine screen→cell map, derived once per frame in f64 (exactly `Viewport::to_map`
    // composed with the OBCW cell formula), then frozen into Q31.32. Sampling is at pixel
    // centers (x + 0.5, y + 0.5).
    let Some((col00, row00, dcol_dx, dcol_dy, drow_dx, drow_dy)) = grid_affine(vp, &grid) else {
        stats.rain_out_of_regime = true;
        return;
    };
    let (width, height) = (grid.width_cells as i64, grid.height_cells as i64);

    // The zoom-regime gate ([`RAIN_MAX_CELL_STEP`], heading-invariant grid-axis norms): out of
    // regime the overlay draws nothing and reports it — the owning screen presents that state
    // explicitly, never as a dry map.
    if !regime_steps_ok(dcol_dx, dcol_dy, drow_dx, drow_dy) {
        stats.rain_out_of_regime = true;
        return;
    }

    // Overflow bound for the whole Q31.32 walk, checked in f64 before freezing: the accumulator's
    // magnitude anywhere on the panel is at most |seed| + h·|row step| + w·|column step|, and
    // keeping that under 2³⁰ **cells** keeps every fixed-point sum under 2⁶² — no i64 wrap even
    // with validation-legal extreme metadata (a u16::MAX-cell grid over a 1 µdeg span). Violation
    // draws nothing rather than fabricating cells from wrapped arithmetic.
    let bound = (1u64 << 30) as f64;
    let (w_f, h_f) = (vp.w as f64, vp.h as f64);
    if !(col00.abs() + h_f * dcol_dy.abs() + w_f * dcol_dx.abs() < bound
        && row00.abs() + h_f * drow_dy.abs() + w_f * drow_dx.abs() < bound)
    {
        stats.rain_out_of_regime = true;
        return;
    }
    let (Some(col00), Some(row00), Some(dcol_dx), Some(dcol_dy), Some(drow_dx), Some(drow_dy)) =
        (to_fp(col00), to_fp(row00), to_fp(dcol_dx), to_fp(dcol_dy), to_fp(drow_dx), to_fp(drow_dy))
    else {
        stats.rain_out_of_regime = true;
        return;
    };
    scratch.reset();

    // Resolve all 16 intensity styles through the caller's color quantizer once per frame.
    let lut: [(D::Color, u8); 16] = core::array::from_fn(|i| {
        let (rgb565, coverage) = rain_style(i as u8);
        (color_fn(rgb565), coverage)
    });
    let tile_cols = (grid.width_cells as u32).div_ceil(RAIN_TILE_EDGE as u32);

    let (w_px, h_px) = (vp.w as i32, vp.h as i32);
    for y in 0..h_px {
        // Row starts are exact multiples of the row increments, so a row's cells are independent
        // of any other row's walk (deterministic under partial redraws).
        let mut col = col00 + y as i64 * dcol_dy;
        let mut row = row00 + y as i64 * drow_dy;
        let bayer_row = &BAYER[(y & 3) as usize];
        // Nearest-neighbour cell state, reused while consecutive pixels stay inside one cell —
        // the steady-state fast path (a provider cell spans many pixels at riding zooms).
        let mut last_cell: Option<(i64, i64)> = None;
        let mut last_style: (D::Color, u8) = lut[0];
        let pixels = (0..w_px).filter_map(|x| {
            let (c_fp, r_fp) = (col, row);
            col += dcol_dx;
            row += drow_dx;
            let (cf, rf) = (c_fp >> FP_SHIFT, r_fp >> FP_SHIFT);
            if cf < 0 || rf < 0 || cf >= width || rf >= height {
                last_cell = None;
                return None;
            }
            if last_cell != Some((cf, rf)) {
                let tile_index = (rf as u32 / RAIN_TILE_EDGE as u32) * tile_cols + cf as u32 / RAIN_TILE_EDGE as u32;
                let cell = (rf as usize % RAIN_TILE_EDGE) * RAIN_TILE_EDGE + cf as usize % RAIN_TILE_EDGE;
                let intensity = scratch.cell(source, tile_index, cell, stats);
                last_style = intensity.map_or(lut[15], |i| lut[(i & 0x0F) as usize]);
                last_cell = Some((cf, rf));
            }
            let (color, coverage) = last_style;
            if bayer_row[(x & 3) as usize] < coverage {
                stats.rain_px = stats.rain_px.saturating_add(1);
                Some(Pixel(Point::new(x, y), color))
            } else {
                None
            }
        });
        let _ = target.draw_iter(pixels);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RenderStats;
    use embedded_graphics::pixelcolor::Rgb565;

    /// A deterministic in-memory grid source: intensity = `pattern(row, col)`, with fetch counting.
    struct TestSource<F: Fn(u32, u32) -> u8> {
        grid: RainGrid,
        pattern: F,
        fetches: u32,
    }

    impl<F: Fn(u32, u32) -> u8> RainOverlaySource for TestSource<F> {
        fn grid(&self) -> Option<RainGrid> {
            Some(self.grid)
        }

        fn tile(&mut self, tile_index: u32, out: &mut [u8; RAIN_TILE_CELLS]) -> bool {
            self.fetches += 1;
            let tile_cols = (self.grid.width_cells as u32).div_ceil(RAIN_TILE_EDGE as u32);
            let (tr, tc) = (tile_index / tile_cols, tile_index % tile_cols);
            for cy in 0..RAIN_TILE_EDGE as u32 {
                for cx in 0..RAIN_TILE_EDGE as u32 {
                    let (row, col) = (tr * 16 + cy, tc * 16 + cx);
                    let v = if row < self.grid.height_cells as u32 && col < self.grid.width_cells as u32 {
                        (self.pattern)(row, col)
                    } else {
                        15 // out-of-frame padding is no-data, as OBCW mandates
                    };
                    out[(cy * 16 + cx) as usize] = v;
                }
            }
            true
        }
    }

    /// A 96×96-cell grid roughly 1 km/cell near 48°N — the DWD shape.
    fn dwd_grid() -> RainGrid {
        RainGrid {
            west_udeg: 7_000_000,
            south_udeg: 47_000_000,
            east_udeg: 8_290_000,
            north_udeg: 47_864_000,
            width_cells: 96,
            height_cells: 96,
        }
    }

    /// Render into a `FrameBuf`-like byte grid via a tiny DrawTarget over a Vec.
    struct Frame {
        w: i32,
        h: i32,
        px: std::vec::Vec<u16>,
    }

    impl Frame {
        fn new(w: i32, h: i32) -> Self {
            Self { w, h, px: std::vec![0xFFFF_u16 - 1; (w * h) as usize] }
        }
    }

    impl embedded_graphics::geometry::OriginDimensions for Frame {
        fn size(&self) -> Size {
            Size::new(self.w as u32, self.h as u32)
        }
    }

    impl DrawTarget for Frame {
        type Color = Rgb565;
        type Error = core::convert::Infallible;

        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Self::Color>>,
        {
            for Pixel(p, c) in pixels {
                if p.x >= 0 && p.x < self.w && p.y >= 0 && p.y < self.h {
                    self.px[(p.y * self.w + p.x) as usize] = c.into_storage();
                }
            }
            Ok(())
        }
    }

    fn draw(vp: &Viewport, source: &mut dyn RainOverlaySource, w: i32, h: i32) -> (Frame, RenderStats) {
        let mut frame = Frame::new(w, h);
        let mut scratch = RainScratch::default();
        let mut stats = RenderStats::default();
        draw_rain(
            &mut frame,
            vp,
            &mut scratch,
            source,
            &|c| {
                use embedded_graphics::pixelcolor::raw::RawU16;
                Rgb565::from(RawU16::new(c))
            },
            &mut stats,
        );
        (frame, stats)
    }

    /// The independent per-pixel reference of the fixed-point walk: evaluate the same affine map
    /// per pixel in f64 and floor it, mirroring `obc-weather`'s integer `cell_index` orientation
    /// (row 0 south, half-open bounds).
    fn reference_cell(vp: &Viewport, grid: &RainGrid, x: i32, y: i32) -> Option<(i64, i64)> {
        let (zoom, aspect) = (vp.zoom as f64, vp.aspect as f64);
        let (sin_c, cos_c) = (libm::sin(vp.course_rad as f64), libm::cos(vp.course_rad as f64));
        let (xc, yc) = (x as f64 + 0.5 - vp.w as f64 / 2.0, y as f64 + 0.5 - vp.h as f64 / 2.0);
        let (rx, ry) = (xc / zoom, yc / zoom);
        let ex = cos_c * rx - sin_c * ry;
        let ny = -sin_c * rx - cos_c * ry;
        let lon = vp.cam_lon as f64 + ex / aspect;
        let lat = vp.cam_lat as f64 + ny;
        let col =
            (lon - grid.west_udeg as f64) * grid.width_cells as f64 / (grid.east_udeg as f64 - grid.west_udeg as f64);
        let row = (lat - grid.south_udeg as f64) * grid.height_cells as f64
            / (grid.north_udeg as f64 - grid.south_udeg as f64);
        let (cf, rf) = (libm::floor(col) as i64, libm::floor(row) as i64);
        (cf >= 0 && rf >= 0 && cf < grid.width_cells as i64 && rf < grid.height_cells as i64).then_some((cf, rf))
    }

    /// A viewport whose sampled cells are compared pixel-by-pixel against the f64 reference. Uses a
    /// pattern where every cell has a distinct paintable intensity parity so a one-cell shift at a
    /// boundary flips the drawn color and fails.
    #[test]
    fn fixed_point_walk_matches_the_per_pixel_reference() {
        let grid = dwd_grid();
        for (zoom, course_deg) in [(0.026_f32, 0.0_f32), (0.026, 37.0), (0.0012, 218.5), (0.26, 90.0)] {
            let vp = Viewport::new_rotated(240.0, 320.0, 7_600_000, 47_400_000, zoom, course_deg.to_radians());
            let mut source = TestSource { grid, pattern: |r, c| if (r + c) % 2 == 0 { 12 } else { 6 }, fetches: 0 };
            let (frame, _) = draw(&vp, &mut source, 240, 320);
            let mut checked = 0usize;
            for y in 0..320 {
                for x in 0..240 {
                    let expect = reference_cell(&vp, &grid, x, y).map(|(cf, rf)| {
                        let intensity = if (rf + cf) % 2 == 0 { 12u8 } else { 6 };
                        let (rgb565, coverage) = rain_style(intensity);
                        if BAYER[(y & 3) as usize][(x & 3) as usize] < coverage {
                            Some(rgb565)
                        } else {
                            None
                        }
                    });
                    let expect = expect.flatten().unwrap_or(0xFFFF - 1);
                    let got = frame.px[(y * 240 + x) as usize];
                    // Cell-boundary pixels may legitimately land either side of the f64 vs Q31.32
                    // rounding; everywhere else the two must agree exactly. Boundary pixels are the
                    // ones whose fractional cell coordinate is within 1e-6 of an integer.
                    let boundary = {
                        let near = |v: f64| (v - libm::round(v)).abs() < 1e-6;
                        let (zoomf, aspect) = (vp.zoom as f64, vp.aspect as f64);
                        let (sin_c, cos_c) = (libm::sin(vp.course_rad as f64), libm::cos(vp.course_rad as f64));
                        let (xc, yc) = (x as f64 + 0.5 - 120.0, y as f64 + 0.5 - 160.0);
                        let ex = cos_c * (xc / zoomf) - sin_c * (yc / zoomf);
                        let ny = -sin_c * (xc / zoomf) - cos_c * (yc / zoomf);
                        let lon = vp.cam_lon as f64 + ex / aspect;
                        let lat = vp.cam_lat as f64 + ny;
                        near((lon - grid.west_udeg as f64) * 96.0 / (grid.east_udeg - grid.west_udeg) as f64)
                            || near((lat - grid.south_udeg as f64) * 96.0 / (grid.north_udeg - grid.south_udeg) as f64)
                    };
                    if !boundary {
                        assert_eq!(got, expect, "pixel ({x},{y}) at zoom {zoom} course {course_deg}");
                        checked += 1;
                    }
                }
            }
            assert!(checked > 60_000, "the comparison actually covered the panel ({checked})");
        }
    }

    /// Same viewport, same source → byte-identical output, twice over (dither and sampling carry
    /// no hidden state between frames).
    #[test]
    fn rendering_is_deterministic() {
        let grid = dwd_grid();
        let vp = Viewport::new_rotated(240.0, 320.0, 7_600_000, 47_400_000, 0.026, 1.1);
        let mut source = TestSource { grid, pattern: |r, c| ((r / 7 + c / 5) % 13) as u8, fetches: 0 };
        let (a, _) = draw(&vp, &mut source, 240, 320);
        let (b, _) = draw(&vp, &mut source, 240, 320);
        assert_eq!(a.px, b.px);
    }

    /// Dry, reserved and no-data cells paint nothing at all.
    #[test]
    fn dry_reserved_and_nodata_paint_nothing() {
        let grid = dwd_grid();
        let vp = Viewport::new(240.0, 320.0, 7_600_000, 47_400_000, 0.026);
        for code in [0u8, 13, 14, 15] {
            let mut source = TestSource { grid, pattern: move |_, _| code, fetches: 0 };
            let (frame, stats) = draw(&vp, &mut source, 240, 320);
            assert!(frame.px.iter().all(|&p| p == 0xFFFF - 1), "code {code} painted pixels");
            assert_eq!(stats.rain_px, 0);
        }
    }

    /// A viewport fully outside the grid touches no tiles and paints nothing.
    #[test]
    fn outside_the_grid_paints_and_fetches_nothing() {
        let grid = dwd_grid();
        let vp = Viewport::new(240.0, 320.0, 2_000_000, 40_000_000, 0.026);
        let mut source = TestSource { grid, pattern: |_, _| 12, fetches: 0 };
        let (frame, stats) = draw(&vp, &mut source, 240, 320);
        assert!(frame.px.iter().all(|&p| p == 0xFFFF - 1));
        assert_eq!((source.fetches, stats.rain_tiles), (0, 0));
    }

    /// The rotation/SD-thrash bound, re-pinned across the **whole reachable m/px range** (the
    /// app's zoom clamp reaches ~111 km/px — adversarial review of this PR) at every heading:
    /// in regime, fetches never exceed the 36-tile product (no per-scanline re-reading at any
    /// rotation — the regime cap keeps a row's tile staircase inside the 8-slot cache); out of
    /// regime the overlay decodes nothing, paints nothing, and reports itself
    /// ([`RenderStats::rain_out_of_regime`]); the public [`rain_in_regime`] predicate agrees with
    /// the drawn outcome at every point; and the locked 50 km view (~208 m/px) stays **in**
    /// regime for a 1 km product at every heading.
    #[test]
    fn decode_bound_across_the_reachable_zoom_range() {
        let grid = dwd_grid();
        for mpp in [
            1.0_f32, 5.0, 10.0, 50.0, 100.0, 208.0, 250.0, 300.0, 320.0, 330.0, 340.0, 350.0, 400.0, 800.0, 3_000.0,
            111_000.0,
        ] {
            let zoom = crate::viewport::zoom_for_mpp(mpp);
            for course_deg in (0..360).step_by(15) {
                let vp =
                    Viewport::new_rotated(240.0, 320.0, 7_600_000, 47_400_000, zoom, (course_deg as f32).to_radians());
                let mut source = TestSource { grid, pattern: |_, _| 12, fetches: 0 };
                let (frame, stats) = draw(&vp, &mut source, 240, 320);
                assert_eq!(
                    super::rain_in_regime(&vp, &grid),
                    !stats.rain_out_of_regime,
                    "regime predicate disagrees with the drawn outcome at {mpp} m/px, {course_deg}deg"
                );
                if stats.rain_out_of_regime {
                    assert_eq!(
                        (source.fetches, stats.rain_px),
                        (0, 0),
                        "out of regime must decode and paint nothing ({mpp} m/px, {course_deg}deg)"
                    );
                } else {
                    assert!(
                        source.fetches <= 36,
                        "{mpp} m/px, {course_deg}deg: {} fetches for a 36-tile product",
                        source.fetches
                    );
                    assert_eq!(source.fetches, stats.rain_tiles, "stats mirror the source's own count");
                    assert!(
                        frame.px.iter().any(|&p| p != 0xFFFF - 1),
                        "in regime over the grid, full-coverage rain paints ({mpp} m/px, {course_deg}deg)"
                    );
                }
                if mpp <= 208.0 {
                    assert!(
                        !stats.rain_out_of_regime,
                        "the locked 50 km view must stay in regime ({mpp} m/px, {course_deg}deg)"
                    );
                }
            }
        }
    }

    /// Delta review of PR #1213: at a fixed zoom the regime verdict must be **heading-invariant**
    /// — the criterion is the grid-axis Jacobian row norms, which rotation (acting on the screen
    /// side) cannot change. The first per-screen-axis criterion flipped in/out across headings at
    /// 250–330 m/px; this pins the fix over a fine zoom scan spanning both regime sides.
    #[test]
    fn regime_verdict_is_heading_invariant_at_fixed_zoom() {
        let grid = dwd_grid();
        for mpp in [10.0_f32, 100.0, 208.0, 230.0, 250.0, 280.0, 300.0, 320.0, 330.0, 340.0, 350.0, 400.0, 1_000.0] {
            let zoom = crate::viewport::zoom_for_mpp(mpp);
            let mut verdicts = std::vec::Vec::new();
            for course_deg in (0..360).step_by(5) {
                let vp =
                    Viewport::new_rotated(240.0, 320.0, 7_600_000, 47_400_000, zoom, (course_deg as f32).to_radians());
                let predicate = super::rain_in_regime(&vp, &grid);
                let mut source = TestSource { grid, pattern: |_, _| 12, fetches: 0 };
                let (_, stats) = draw(&vp, &mut source, 240, 320);
                assert_eq!(predicate, !stats.rain_out_of_regime, "{mpp} m/px, {course_deg}deg");
                verdicts.push(predicate);
            }
            assert!(
                verdicts.windows(2).all(|w| w[0] == w[1]),
                "{mpp} m/px: the regime verdict varies with heading ({verdicts:?})"
            );
        }
    }

    /// Adversarial review (PR #1213), adopted: a grid that passes every OBCW validation gate —
    /// u16::MAX cells over a 1 udeg lon span — used to overflow the per-row `y * dcol_dy` seed in
    /// release and panic in debug. The setup-time magnitude bound now refuses it: no panic,
    /// nothing painted, and the refusal is reported rather than silent.
    #[test]
    fn extreme_anisotropic_grid_must_not_overflow() {
        let grid = RainGrid {
            west_udeg: 7_000_000,
            south_udeg: 47_000_000,
            east_udeg: 7_000_001, // 1 udeg lon span
            north_udeg: 47_864_000,
            width_cells: u16::MAX,
            height_cells: 96,
        };
        let zoom = crate::viewport::zoom_for_mpp(208.0);
        for course_deg in [0.0_f32, 45.0, 218.5] {
            let vp = Viewport::new_rotated(240.0, 320.0, 7_000_000, 47_400_000, zoom, course_deg.to_radians());
            let mut source = TestSource { grid, pattern: |_, _| 12, fetches: 0 };
            let (frame, stats) = draw(&vp, &mut source, 240, 320);
            assert!(frame.px.iter().all(|&p| p == 0xFFFF - 1), "wrapped cells must never paint");
            assert!(stats.rain_out_of_regime, "the refusal must be reported, not silent");
            assert_eq!(source.fetches, 0);
        }
    }

    /// Adversarial review (PR #1213), adopted: a viewport straddling the grid's west/south corner
    /// under rotation — every pixel's painted/not-painted outcome must agree with the f64
    /// reference (the original sweep never crossed a grid boundary).
    #[test]
    fn straddling_the_grid_edge_matches_the_reference() {
        let grid = dwd_grid();
        for course_deg in [0.0_f32, 37.0, 218.5] {
            // Camera on the grid's south-west corner: much of the panel is off-grid.
            let vp = Viewport::new_rotated(240.0, 320.0, 7_000_000, 47_000_000, 0.026, course_deg.to_radians());
            let mut source = TestSource { grid, pattern: |_, _| 12, fetches: 0 };
            let (frame, stats) = draw(&vp, &mut source, 240, 320);
            assert!(!stats.rain_out_of_regime);
            for y in 0..320i32 {
                for x in 0..240i32 {
                    let reference = reference_cell(&vp, &grid, x, y);
                    // Boundary pixels may land either side of f64 vs Q31.32 rounding; skip only
                    // those (the same epsilon as the main sweep).
                    let boundary = {
                        let near = |v: f64| (v - libm::round(v)).abs() < 1e-6;
                        let (zoomf, aspect) = (vp.zoom as f64, vp.aspect as f64);
                        let (sin_c, cos_c) = (libm::sin(vp.course_rad as f64), libm::cos(vp.course_rad as f64));
                        let (xc, yc) = (x as f64 + 0.5 - 120.0, y as f64 + 0.5 - 160.0);
                        let ex = cos_c * (xc / zoomf) - sin_c * (yc / zoomf);
                        let ny = -sin_c * (xc / zoomf) - cos_c * (yc / zoomf);
                        let lon = vp.cam_lon as f64 + ex / aspect;
                        let lat = vp.cam_lat as f64 + ny;
                        near((lon - grid.west_udeg as f64) * 96.0 / (grid.east_udeg - grid.west_udeg) as f64)
                            || near((lat - grid.south_udeg as f64) * 96.0 / (grid.north_udeg - grid.south_udeg) as f64)
                    };
                    if boundary {
                        continue;
                    }
                    let expect_painted = reference.is_some() && BAYER[(y & 3) as usize][(x & 3) as usize] < 16;
                    let painted = frame.px[(y * 240 + x) as usize] != 0xFFFF - 1;
                    assert_eq!(painted, expect_painted, "pixel ({x},{y}) at course {course_deg}");
                }
            }
        }
    }

    /// A failing tile renders transparent and is fetched once, not per pixel.
    #[test]
    fn failed_tiles_are_transparent_and_fetched_once() {
        struct Failing {
            grid: RainGrid,
            fetches: u32,
        }
        impl RainOverlaySource for Failing {
            fn grid(&self) -> Option<RainGrid> {
                Some(self.grid)
            }
            fn tile(&mut self, _t: u32, _out: &mut [u8; RAIN_TILE_CELLS]) -> bool {
                self.fetches += 1;
                false
            }
        }
        let mut source = Failing { grid: dwd_grid(), fetches: 0 };
        let vp = Viewport::new(240.0, 320.0, 7_600_000, 47_400_000, 0.026);
        let mut frame = Frame::new(240, 320);
        let mut scratch = RainScratch::default();
        let mut stats = RenderStats::default();
        draw_rain(
            &mut frame,
            &vp,
            &mut scratch,
            &mut source,
            &|c| {
                use embedded_graphics::pixelcolor::raw::RawU16;
                Rgb565::from(RawU16::new(c))
            },
            &mut stats,
        );
        assert!(frame.px.iter().all(|&p| p == 0xFFFF - 1), "failures never fabricate rain");
        assert!(source.fetches <= 36, "failures are cached per tile, got {}", source.fetches);
    }

    /// Every table color is RGB222-exact (device quantization is the identity), the transparent
    /// codes are pinned, and coverage is monotonic within each color band.
    #[test]
    fn style_table_is_rgb222_exact_and_transparent_codes_are_pinned() {
        for code in [0u8, 13, 14, 15] {
            assert_eq!(rain_style(code).1, 0, "code {code} must be transparent");
        }
        let mut previous: Option<(u16, u8)> = None;
        for intensity in 1..=12u8 {
            let (color, coverage) = rain_style(intensity);
            assert!(coverage > 0, "band {intensity} must be visible");
            let (r, g, b) = ((color >> 11) & 0x1F, (color >> 5) & 0x3F, color & 0x1F);
            assert!([0, 10, 21, 31].contains(&r), "band {intensity} red not RGB222-exact");
            assert!([0, 21, 42, 63].contains(&g), "band {intensity} green not RGB222-exact");
            assert!([0, 10, 21, 31].contains(&b), "band {intensity} blue not RGB222-exact");
            if let Some((prev_color, prev_cov)) = previous {
                assert_ne!((color, coverage), (prev_color, prev_cov), "adjacent bands must differ");
                if color == prev_color {
                    assert!(coverage > prev_cov, "coverage rises within a color band ({intensity})");
                }
            }
            previous = Some((color, coverage));
        }
    }

    /// The Bayer matrix is the canonical index-4 ordered matrix: a permutation of 0..16 in which
    /// every adjacent 2×2 block holds one value from each quartile — the property that makes low
    /// coverages spread evenly instead of clumping.
    #[test]
    fn bayer_matrix_is_the_canonical_permutation() {
        let mut seen = [false; 16];
        for row in &BAYER {
            for &v in row {
                assert!(!seen[v as usize]);
                seen[v as usize] = true;
            }
        }
        assert!(seen.iter().all(|&s| s));
        for by in 0..2 {
            for bx in 0..2 {
                let mut quartiles = [false; 4];
                for dy in 0..2 {
                    for dx in 0..2 {
                        quartiles[(BAYER[by * 2 + dy][bx * 2 + dx] / 4) as usize] = true;
                    }
                }
                assert!(quartiles.iter().all(|&q| q), "2×2 block ({bx},{by}) misses a quartile");
            }
        }
    }
    /// `rain_min_zoom` is the exact inversion of the regime criterion: at the returned floor the
    /// overlay is in regime at every heading, a hair below it is out at every heading, and a
    /// coarser product's floor sits proportionally further out (the WX11 rain-map zoom clamp).
    #[test]
    fn min_zoom_is_the_regime_edge_at_every_heading() {
        let cam = (7_600_000, 47_400_000);
        let fine = RainGrid {
            west_udeg: 7_000_000,
            south_udeg: 47_000_000,
            east_udeg: 8_000_000,
            north_udeg: 48_000_000,
            width_cells: 96,
            height_cells: 96,
        };
        let coarse = RainGrid { width_cells: 24, height_cells: 24, ..fine };
        let aspect = obc_map_scene::cos_lat(cam.1);
        let fine_floor = super::rain_min_zoom(&fine, aspect).unwrap();
        let coarse_floor = super::rain_min_zoom(&coarse, aspect).unwrap();
        assert!(
            (fine_floor / coarse_floor - 4.0).abs() < 0.05,
            "4x coarser cells allow ~4x wider zoom-out ({fine_floor} vs {coarse_floor})"
        );
        for (grid, floor) in [(&fine, fine_floor), (&coarse, coarse_floor)] {
            for course_deg in [0.0f32, 35.0, 90.0, 215.0] {
                // The EXACT returned floor must be in regime — the clamp sets `zoom` to
                // precisely this f32, so testing a nudged value would dodge the boundary the
                // rider actually lands on (review F1).
                let at = Viewport::new_rotated(240.0, 320.0, cam.0, cam.1, floor, course_deg.to_radians());
                assert!(super::rain_in_regime(&at, grid), "at the exact floor: in regime ({course_deg} deg)");
                let below = Viewport::new_rotated(240.0, 320.0, cam.0, cam.1, floor * 0.98, course_deg.to_radians());
                assert!(!super::rain_in_regime(&below, grid), "below the floor: out ({course_deg} deg)");
            }
        }
        // Degenerate grids disengage the clamp rather than inventing a floor.
        let degenerate = RainGrid { width_cells: 0, ..fine };
        assert_eq!(super::rain_min_zoom(&degenerate, aspect), None);
    }
    /// Adopted from the #1224 adversarial review's `hostile_floor` probe (F1): the EXACT
    /// `rain_min_zoom` result is in regime across hostile grid shapes, spans, latitudes (equator
    /// to 69°N) and headings — before the fix, 192/360 of these evaluated out of regime because
    /// the f64→f32 cast rounded below the true edge.
    #[test]
    fn exact_floor_is_in_regime_everywhere() {
        let mut total = 0u32;
        for lat_udeg in [0i32, 15_000_000, 47_400_000, 60_000_000, 69_000_000] {
            let aspect = obc_map_scene::cos_lat(lat_udeg);
            for (w_cells, h_cells) in [(96u16, 96u16), (24, 24), (300, 200), (1100, 900), (7, 5), (640, 480)] {
                for (lon_span, lat_span) in [(1_000_000i32, 1_000_000i32), (3_337_000, 2_221_000), (500_001, 777_777)] {
                    let grid = RainGrid {
                        west_udeg: 7_000_000,
                        south_udeg: lat_udeg - lat_span / 2,
                        east_udeg: 7_000_000 + lon_span,
                        north_udeg: lat_udeg - lat_span / 2 + lat_span,
                        width_cells: w_cells,
                        height_cells: h_cells,
                    };
                    let Some(floor) = super::rain_min_zoom(&grid, aspect) else { continue };
                    for course_deg in [0.0f32, 35.0, 90.0, 215.0] {
                        total += 1;
                        let vp =
                            Viewport::new_rotated(240.0, 320.0, 7_500_000, lat_udeg, floor, course_deg.to_radians());
                        assert!(
                            super::rain_in_regime(&vp, &grid),
                            "exact floor OUT of regime: lat={lat_udeg} cells={w_cells}x{h_cells} \
                             span={lon_span}x{lat_span} course={course_deg} floor={floor}"
                        );
                    }
                }
            }
        }
        assert_eq!(total, 360, "the sweep's breadth is part of the pin");
    }
}
