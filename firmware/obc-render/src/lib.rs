//! Shared map renderer (feature `render`).
//!
//! This is the rendering path that runs **both** in the desktop simulator and
//! on the nRF54L firmware. It is written generically over `embedded-graphics`'
//! [`DrawTarget`], so the host (an SDL `SimulatorDisplay`) and the device (an
//! LS021B7DD02 driver) share the exact same projection, LOD selection, painter
//! ordering, polygon fill and line drawing. The host shell only owns the window,
//! event loop and color policy.
//!
//! [`MapRenderer`] owns every scratch buffer it needs and clears (not frees)
//! them each frame, so steady-state rendering does no heap allocation — decode
//! streams through [`Reader::for_each_feature`] into reused buffers. Geometry
//! math uses `libm` so it works unchanged in `no_std`.

#![no_std]

use heapless::Vec;

use embedded_graphics::{
    prelude::*,
    primitives::{Polyline, PrimitiveStyle, Rectangle},
};

use obc_reader::{BBox, Kind, Reader};

pub mod canvas;
mod font_data;
pub mod text;
pub use canvas::{rect, Canvas};
pub use text::{draw_text, text_width, Font, TextAlign};

// Per-frame buffer capacities. Statically allocated (heapless::Vec); growing one costs boot
// RAM, not per-frame. Two memory profiles select the caps (issue #124):
//   - default (host / sim / tests): generous, full preview fidelity.
//   - `nrf-mem`: the constrained nRF54L15 profile — these are roughly halved so the renderer
//     scratch (`MCU_RENDERER_BYTES` below, ~74 KB vs ~200 KB) fits the 256 KB part alongside
//     the 75 KB RGB222 framebuffer + the map/route caches; the board crate's budget assert is
//     the binding check. The cost is a coarse-zoom feature-density compromise: a frame whose
//     visible-feature / vertex count exceeds a cap drops the overflow (see [`render`]), which on
//     the constrained profile starts at busier coarse zooms than on the host.
// The single-feature decode buffers (`MAX_DECODE_*`) are *not* trimmed — they must hold the
// worst single feature either way, and pair with `obc_reader::MAX_FEAT_PTS` / `MAX_FEAT_RINGS`.

/// Maximum visible features per frame (each is a [`Span`] — 14 bytes). At coarse zoom this is
/// the buffer that saturates first (many small features), so on the full profile it is sized
/// generously; see the RAM-budget assertion below.
#[cfg(not(feature = "nrf-mem"))]
pub const MAX_SPANS: usize = 3072;
// N6 (#127): 1280→768. This is the coarse-zoom feature-density knob (saturates first — the most
// visible L15 cost, #124); it's trimmed hard because the ride loop's **deep per-frame render path**
// (per-frame `Reader::new` + streamed-chunk decode over embedded-sdmmc) needs a large MSP stack that
// must coexist with the resident `RouteCache`/`RouteIndex` on the 256 KB part — freeing scratch is
// what buys that stack headroom. The 512 KB LM20 restores it.
#[cfg(feature = "nrf-mem")]
pub const MAX_SPANS: usize = 768;

/// Maximum total vertices across all visible features per frame (8 bytes each).
#[cfg(not(feature = "nrf-mem"))]
pub const MAX_FRAME_POINTS: usize = 12_288;
#[cfg(feature = "nrf-mem")]
pub const MAX_FRAME_POINTS: usize = 1536; // N6 (#127): 2560→1536, freeing ~8 KB toward the ride-loop residents + stack

/// Maximum total ring entries across all visible features per frame.
#[cfg(not(feature = "nrf-mem"))]
pub const MAX_FRAME_RINGS: usize = 3072;
#[cfg(feature = "nrf-mem")]
pub const MAX_FRAME_RINGS: usize = 384; // N6 (#127): 768→384, freeing RAM for the deep render path's stack

/// Maximum vertices for a single feature during decode (reused per feature).
pub const MAX_DECODE_POINTS: usize = 2048;

/// Maximum rings for a single feature during decode.
pub const MAX_DECODE_RINGS: usize = 32;

/// Maximum projected screen points for drawing one feature. The fill/polyline path projects
/// **every** vertex of a decoded feature into this buffer before walking it, so it must hold a
/// whole decode buffer — kept at [`MAX_DECODE_POINTS`] on the constrained profile (the invariant
/// is asserted below; dropping under it makes `fill_polygon` index past the projected points).
#[cfg(not(feature = "nrf-mem"))]
pub const MAX_SCREEN_POINTS: usize = 4096;
#[cfg(feature = "nrf-mem")]
pub const MAX_SCREEN_POINTS: usize = 2048;

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
// `fill_polygon` / the polyline path project a whole decoded feature into `screen` before walking
// its rings (`screen[base..base + len]`), so the projected buffer must hold at least a full decode
// buffer or it indexes past the points and panics. Held with room to spare on the full profile;
// the `nrf-mem` profile sets them equal. Guard it so a future cap edit can't reintroduce the panic.
const _: () = assert!(MAX_SCREEN_POINTS >= MAX_DECODE_POINTS, "`screen` must hold a whole decoded feature");

/// Static RAM the [`MapRenderer`]'s scratch buffers occupy on the 32-bit MCU target (`usize` = 4
/// bytes there). Computed from the caps above — `pub` so a board crate's RAM-budget assert can add
/// it to the framebuffer + caches without re-deriving the formula (issue #124). (`(i32, i32)` and
/// `Point` are 8 bytes; `usize`/`f32` are 4 on the MCU.) ~200 KB on the full profile, ~74 KB on
/// `nrf-mem`.
pub const MCU_RENDERER_BYTES: usize = MAX_DECODE_POINTS * 8
    + MAX_DECODE_RINGS * 4
    + MAX_FRAME_POINTS * 8
    + MAX_FRAME_RINGS * 4
    + MAX_SPANS * core::mem::size_of::<Span>()
    + MAX_SCREEN_POINTS * 8
    + MAX_CROSSINGS * 4;
// A loose per-crate ceiling that catches an accidental cap blow-up on either profile; the binding
// fit check is the board crate's whole-resident-set budget assert (the nRF profile lands well under
// this, ~74 KB). The full profile sits just under 200 KB.
const _: () = assert!(MCU_RENDERER_BYTES <= 200 * 1024, "MapRenderer exceeds the 200 KB MCU budget");

/// Meters of ground per microdegree of latitude — the renderer's zoom is pixels per
/// microdegree-lat, so this turns zoom into meters-per-pixel. Derived from the shared
/// [`obc_reader::M_PER_DEG`] (a microdegree is 1e-6°) so the on-screen scale tracks the
/// one Earth model the route/packer measure against.
const METERS_PER_MICRODEG_LAT: f32 = (obc_reader::M_PER_DEG / 1_000_000.0) as f32;

// Route direction chevrons (tunable). Arrowheads along the active route at riding zoom,
// anchored to route distance (not screen) so each stays pinned to a ground spot, drawn only in a
// window around the rider — so an out-and-back marks just the current leg. Spacing + window are
// screen-relative — a fixed pixel cadence and a chevron *count*, not ground metres — so the
// chevrons keep an even spread across the finest LOD's zoom range (no bunching when zoomed out);
// the ground spacing is derived per-frame from the camera's m/px. Glyph sizes are screen pixels.
// Sweep with the app's `ROUTE_WEIGHT`.
//
/// On-screen gap between consecutive chevrons (px). Held in *screen* space: each frame the
/// route-distance spacing is `ARROW_SPACING_PX × m/px`, so the chevrons stay evenly spread however
/// far you zoom. At the ~0.5 m/px riding zoom this is ≈ 33 m apart on the ground.
const ARROW_SPACING_PX: f32 = 66.0;
/// How many chevrons lead *ahead* of the rider. A count (not a ground distance) so the look-ahead
/// tracks the screen cadence — the chevrons reach a fixed way up the screen at every zoom.
/// Off-screen ones are free, so this is generous (≈ the old 300 m at riding zoom).
const ARROW_AHEAD_COUNT: u32 = 9;
/// How many chevrons trail *behind* the rider. Zero — the breadcrumb shows the travelled line,
/// so chevrons only lead ahead.
const ARROW_BEHIND_COUNT: u32 = 0;
/// Chevron tip reach ahead of its centre (px).
const ARROW_TIP: f32 = 8.0;
/// Chevron base reach behind its centre (px).
const ARROW_BACK: f32 = 2.5;
/// Chevron base half-width (px) — half the spread of the two trailing corners. Kept under
/// the route's half-stroke so the glyph sits *inside* the line (framed by the route colour
/// on every side), independent of whatever map colour the line crosses.
const ARROW_HALF: f32 = 4.5;

/// The [`Viewport`]/`AppState` zoom (pixels per microdegree of latitude) that yields
/// a given ground **meters-per-pixel** — the inverse of [`mpp_for_zoom`] /
/// [`Viewport::meters_per_pixel`]. Lets callers aim the camera at a real-world scale
/// (e.g. "zoom to 0.5 m/px for riding") instead of a raw zoom value.
#[inline]
pub fn zoom_for_mpp(mpp: f32) -> f32 {
    METERS_PER_MICRODEG_LAT / mpp
}

/// Ground **meters-per-pixel** at a given zoom (pixels per microdegree of latitude) —
/// the viewport-free form of [`Viewport::meters_per_pixel`] and the inverse of
/// [`zoom_for_mpp`]. Lets a caller (e.g. the simulator's zoom slider) read a real-world
/// scale from a raw zoom without constructing a [`Viewport`].
#[inline]
pub fn mpp_for_zoom(zoom: f32) -> f32 {
    METERS_PER_MICRODEG_LAT / zoom
}

/// Screen projection: microdegrees → pixels, with longitude aspect correction so
/// the map keeps shape away from the equator. `zoom` is pixels per microdegree of
/// latitude; longitude is additionally scaled by `aspect = cos(lat)`.
///
/// The projection can also rotate the map so a given course points to the top of
/// the screen ("heading-up" / track-up navigation). `course_rad` is that course
/// in radians clockwise from north; `0` is north-up and reduces the math to a
/// plain translate+scale. The rotation is applied about the camera center, after
/// aspect correction, so shapes stay correct.
#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub w: f32,
    pub h: f32,
    pub cam_lon: i32,
    pub cam_lat: i32,
    pub zoom: f32,
    pub aspect: f32,
    /// Course (radians CW from north) the projection rotates to screen-up. 0 = north-up.
    pub course_rad: f32,
    // Precomputed once per frame (rotation is hot — called per projected point).
    sin_c: f32,
    cos_c: f32,
}

impl Viewport {
    /// Build a north-up viewport centered on `(cam_lon, cam_lat)` (microdegrees)
    /// with the aspect correction computed for that latitude.
    pub fn new(w: f32, h: f32, cam_lon: i32, cam_lat: i32, zoom: f32) -> Self {
        Self::new_rotated(w, h, cam_lon, cam_lat, zoom, 0.0)
    }

    /// Like [`new`](Viewport::new) but rotated so `course_rad` (radians CW from
    /// north) points to the top of the screen.
    pub fn new_rotated(w: f32, h: f32, cam_lon: i32, cam_lat: i32, zoom: f32, course_rad: f32) -> Self {
        Viewport {
            w,
            h,
            cam_lon,
            cam_lat,
            zoom,
            aspect: aspect_for_lat(cam_lat),
            course_rad,
            sin_c: libm::sinf(course_rad),
            cos_c: libm::cosf(course_rad),
        }
    }

    #[inline]
    pub fn to_screen(&self, lon: i32, lat: i32) -> (i32, i32) {
        // Integer difference first, then cast the *small* relative delta to f32 — preserves
        // absolute microdegree precision that casting the raw coordinates would lose.
        let delta_lon = lon.wrapping_sub(self.cam_lon);
        let delta_lat = lat.wrapping_sub(self.cam_lat);
        let ex = (delta_lon as f32) * self.aspect;
        let ny = delta_lat as f32;
        // Rotate so `course_rad` points up; at course 0 this is (ex, -ny).
        let rx = self.cos_c * ex - self.sin_c * ny;
        let ry = -self.sin_c * ex - self.cos_c * ny;
        let x = rx * self.zoom + self.w / 2.0;
        let y = ry * self.zoom + self.h / 2.0;
        // Round to nearest rather than truncate toward zero: `as i32` truncation
        // is asymmetric around the origin (it biases toward the screen center) and
        // feeds the staircase divergence behind the chunk-seam overdraw (see the
        // `fill_polygon` comment). Round-to-nearest is symmetric and sub-pixel
        // correct. `roundf` matches the marker glyph, which already rounds.
        let p = round_pt(x, y);
        (p.x, p.y)
    }

    /// [`to_screen`](Viewport::to_screen) as an `embedded-graphics` [`Point`] — the form every
    /// overlay stroker wants when mapping a microdegree vertex into the projected run buffer.
    #[inline]
    fn project(&self, lon: i32, lat: i32) -> Point {
        let (x, y) = self.to_screen(lon, lat);
        Point::new(x, y)
    }

    #[inline]
    pub fn to_map(&self, x: f32, y: f32) -> (i32, i32) {
        let rx = (x - self.w / 2.0) / self.zoom;
        let ry = (y - self.h / 2.0) / self.zoom;
        // Inverse rotation reuses the same coefficients — the screen→ground matrix
        // is an involution (its own inverse), so no extra trig.
        let ex = self.cos_c * rx - self.sin_c * ry;
        let ny = -self.sin_c * rx - self.cos_c * ry;
        let delta_lon = (ex / self.aspect) as i32;
        let delta_lat = ny as i32;
        let lon = self.cam_lon.wrapping_add(delta_lon);
        let lat = self.cam_lat.wrapping_add(delta_lat);
        (lon, lat)
    }

    /// Bounding box (microdegrees) of the on-screen area, for quadtree culling.
    /// Uses all four screen corners so a *rotated* view still culls correctly —
    /// the axis-aligned box must cover the tilted rectangle's full extent.
    pub fn visible_bbox(&self) -> BBox {
        let corners =
            [self.to_map(0.0, 0.0), self.to_map(self.w, 0.0), self.to_map(0.0, self.h), self.to_map(self.w, self.h)];
        let mut min_lon = i32::MAX;
        let mut max_lon = i32::MIN;
        let mut min_lat = i32::MAX;
        let mut max_lat = i32::MIN;
        for (lon, lat) in corners {
            min_lon = min_lon.min(lon);
            max_lon = max_lon.max(lon);
            min_lat = min_lat.min(lat);
            max_lat = max_lat.max(lat);
        }
        BBox { min_lon, min_lat, max_lon, max_lat }
    }

    /// Ground meters per pixel at the current zoom (latitude-based), used to pick
    /// the LOD layer. Independent of display size — a 1024px host and a 240px
    /// panel showing the same ground span pick the same level.
    #[inline]
    pub fn meters_per_pixel(&self) -> f32 {
        mpp_for_zoom(self.zoom)
    }

    /// Unit screen-space vector pointing to map **north** — for a compass needle.
    /// At north-up this is `(0, -1)` (straight up); a heading-up rotation turns it.
    /// Pure rotation, independent of zoom, latitude, and position: a +lat step maps
    /// to `(-sin_c, -cos_c)` in [`to_screen`](Viewport::to_screen) before the (here
    /// irrelevant) scale, and that is already unit length.
    #[inline]
    pub fn north_screen_unit(&self) -> (f32, f32) {
        (-self.sin_c, -self.cos_c)
    }
}

#[inline]
fn aspect_for_lat(cam_lat: i32) -> f32 {
    libm::cosf((cam_lat as f32 / 1e6).to_radians())
}

/// Round sub-pixel `(x, y)` to the nearest integer-pixel [`Point`] — the one rounding convention
/// every screen-space vertex shares (see [`Viewport::to_screen`]).
#[inline]
fn round_pt(x: f32, y: f32) -> Point {
    Point::new(libm::roundf(x) as i32, libm::roundf(y) as i32)
}

/// The renderer's collection scratch: per-feature decode buffers plus the frame
/// buffers that accumulate every visible feature's geometry (and its [`Span`]) for
/// the current frame. Cleared (not freed) each frame — see [`MapRenderer`].
#[derive(Default)]
struct FrameScratch {
    // Per-feature decode scratch handed to `Reader::for_each_feature_filtered`.
    dec_points: Vec<(i32, i32), MAX_DECODE_POINTS>,
    dec_ring_lens: Vec<usize, MAX_DECODE_RINGS>,
    // All visible features' geometry, concatenated, plus per-feature spans.
    frame_points: Vec<(i32, i32), MAX_FRAME_POINTS>,
    frame_ring_lens: Vec<usize, MAX_FRAME_RINGS>,
    spans: Vec<Span, MAX_SPANS>,
}

impl FrameScratch {
    /// Fill the frame buffers with every visible feature, in strict global
    /// priority order. One pass per priority level (the format stores a 2-bit
    /// level, 1..=4), lowest number first: each pass fills the buffers with every
    /// visible feature at that level across *all* chunks before the next runs, so
    /// when the buffers saturate the dropped features are always the lowest
    /// priority — regardless of which chunk they sit in. Each feature matches one
    /// level, so its coordinates decode at most once per frame.
    fn collect(&mut self, reader: &Reader, lod: usize, view: &BBox, stats: &mut RenderStats) {
        self.frame_points.clear();
        self.frame_ring_lens.clear();
        self.spans.clear();

        for level in 1..=4u8 {
            self.collect_level(reader, lod, level, view, stats);
        }

        // Record utilization for the stats panel.
        stats.span_utilization = self.spans.len() as f32 / self.spans.capacity() as f32;
        stats.point_utilization = self.frame_points.len() as f32 / self.frame_points.capacity() as f32;
        stats.ring_utilization = self.frame_ring_lens.len() as f32 / self.frame_ring_lens.capacity() as f32;
    }

    /// Append every visible feature whose style is at priority `level` to the
    /// frame buffers. Streams the viewport's leaves via [`Reader::for_each_chunk`]
    /// (no chunk cap) and decodes only this level's features via
    /// [`Reader::for_each_feature_filtered`]. The leaf walk reads only the index,
    /// so the per-level re-walk is cheap; `stats.chunks_visited` is recorded once
    /// (the chunk set is identical every level).
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

                    // Per-feature bbox cull (tighter than the leaf): the feature's
                    // bounds come free from decode (`FeatureRef::bbox`).
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

                    // Casts are safe: the capacity check above guarantees room, and
                    // the buffer sizes are asserted `<= u16::MAX` at the constants.
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
        // Visible-chunk count for the stats panel (documents that the chunk set is
        // uncapped). Identical across levels, so record it once.
        if level == 1 {
            stats.chunks_visited = chunks;
        }
    }
}

/// The renderer's draw scratch: projected screen points (also the polyline run
/// buffer) and the scanline-fill crossing buffer. Cleared per use.
#[derive(Default)]
struct DrawScratch {
    screen: Vec<Point, MAX_SCREEN_POINTS>,
    xs: Vec<f32, MAX_CROSSINGS>,
}

/// A monotonic microsecond clock for **stage timing** inside [`MapRenderer::render_timed`].
///
/// `obc-render` is `no_std` and carries no clock of its own, so a caller that wants the
/// per-stage breakdown (collect / sort / draw) passes one in: the device an embassy-`Instant`
/// microsecond clock, a desktop host a `std::time::Instant` clock. The plain
/// [`MapRenderer::render`] path passes the zero-cost [`NoopClock`], so it pays nothing and leaves
/// the stage fields at `0`.
///
/// This whole timing seam — the trait, [`NoopClock`], [`MapRenderer::render_timed`] and the
/// `*_us` stage fields on [`RenderStats`] — is self-contained on purpose: it backs the
/// render-performance benchmark and can be lifted out in one revert once that work is done.
pub trait Clock {
    /// Microseconds since some fixed, monotonic epoch. Only differences are taken.
    fn now_us(&self) -> u64;
}

/// The zero-cost [`Clock`] used by the untimed [`MapRenderer::render`] path: always `0`, so
/// every stage delta is `0` and the optimizer can fold the timing away entirely.
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
    /// Quadtree leaves overlapping the viewport this frame. No longer capped, so
    /// this can exceed any fixed buffer size — watching it confirms wide views
    /// aren't silently dropping chunks.
    pub chunks_visited: usize,
    pub features_tried: usize,
    pub features_drawn: usize,
    pub features_dropped: usize,
    pub points_tried: usize,
    pub points_drawn: usize,
    /// Active-route overlay this frame: chunks whose bbox met the viewport (so they were
    /// decoded), the total points across them, and how many of those were *actually* stroked
    /// after the view clip + subpixel simplify. The route carries **no LOD**, so `route_points`
    /// climbs as you zoom out of a long route — but `route_points_drawn` stays near what's
    /// on-screen, confirming the per-segment clip is doing its job.
    pub route_chunks: usize,
    pub route_points: usize,
    pub route_points_drawn: usize,
    // Buffer utilization (0.0–1.0) for saturation display.
    pub span_utilization: f32,
    pub point_utilization: f32,
    pub ring_utilization: f32,
    /// Streamed-map cache accounting for this frame (issue #37). The map `Reader` re-walks the
    /// visible chunks once per priority level; `map_chunk_hits` are the passes served from a
    /// resident cache slot, `map_chunk_misses` the ones that read from SD. `map_sd_reads` /
    /// `map_bytes_read` are the raw source overhead (index blocks + chunk fills) — the
    /// "SD-read overhead per frame" the issue asks to measure. Hit rate is
    /// `hits / (hits + misses)`.
    pub map_chunk_hits: u32,
    pub map_chunk_misses: u32,
    pub map_sd_reads: u32,
    pub map_bytes_read: u32,
    /// Host-measured wall time for the whole frame draw (render + route/overlays), in
    /// microseconds; `0` = not measured. `obc-render` is `no_std` and carries no clock,
    /// so the **host** fills this after timing the draw (the sim uses `Instant`; the
    /// device the Cortex-M DWT cycle counter) — kept on the stats so the control panel and
    /// the headless line surface it without a side channel.
    pub render_us: u32,
    /// Per-stage wall time of the **map** render, microseconds — filled by
    /// [`render_timed`](MapRenderer::render_timed) from the caller's [`Clock`]; left `0` on the
    /// untimed [`render`](MapRenderer::render) path. `collect_us` is visible-feature collection
    /// (quadtree walk + SD/cache reads + decode + cull + span build), `sort_us` the painter's-
    /// order span sort, `draw_us` the full-screen clear + rasterization (`draw_map`). They cover
    /// only the base map; overlays (route/marker/breadcrumb) are drawn after `render` returns, so
    /// a caller derives overlay time as `total − (collect_us + sort_us + draw_us)`.
    pub collect_us: u32,
    pub sort_us: u32,
    pub draw_us: u32,
    /// Wall time (µs) to compute + stroke the **Home screensaver's contour backdrop** (the
    /// marching-squares pass), filled by `HomeScreen::draw` from the caller's [`Clock`]; `0` on
    /// every non-Home frame and on the untimed [`NoopClock`] path. A diagnostic for the one
    /// screen that does heavier per-draw arithmetic than a plain chrome redraw.
    pub contour_us: u32,
}

/// One visible feature's draw metadata plus the ranges locating its geometry in
/// the renderer's frame buffers. Cheap to sort for the painter's algorithm.
///
/// Offsets are `u16` (not `usize`) to keep the struct to 14 bytes — at coarse
/// zoom thousands of these are buffered, so the width matters. The frame buffers
/// they index are asserted `<= u16::MAX` near the buffer constants.
struct Span {
    kind: Kind,
    z: i8,
    weight: u8,
    color: u16,
    pt_start: u16,
    ring_start: u16,
    ring_count: u16,
    seq: u16,
}

/// Reusable renderer holding every scratch buffer. Construct once (the firmware
/// keeps a single instance) and call [`MapRenderer::render`] per frame; the
/// buffers are cleared and reused, so no per-frame allocation.
#[derive(Default)]
pub struct MapRenderer {
    /// Collection scratch + the frame buffers (decode → cull → spans).
    frame: FrameScratch,
    /// Draw scratch (projected points / polyline runs + scanline crossings),
    /// shared by the map draw phase and the marker/route/breadcrumb overlays.
    draw: DrawScratch,
}

impl MapRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Initialize a renderer **in place** at `slot` as the empty, ready-to-render
    /// state — the placement path an MCU uses to build the resident renderer (and
    /// the [`App`](../obc_app) that embeds it) straight into a fixed RAM region
    /// without ever materializing the ~200 KB of scratch on the stack.
    ///
    /// Every scratch buffer is a [`heapless::Vec`], whose empty state is `len = 0`
    /// over an *uninitialized* backing array — i.e. exactly the all-zero bit
    /// pattern. So zeroing the slot yields a valid, empty renderer, and
    /// `write_bytes(0, 1)` lowers to a `memset` of the slot with no temporary and
    /// no reliance on the optimizer's return-value optimization. This is the same
    /// robust trick `MapCache` uses (obc-reader); the desktop simulator keeps using
    /// [`new`](MapRenderer::new)/`Default`, where a stack temporary is fine.
    ///
    /// # Safety
    /// `slot` must be valid for writes, aligned, and exclusively owned for the call.
    /// On return the slot holds a fully initialized, empty [`MapRenderer`].
    pub unsafe fn init_zeroed(slot: *mut Self) {
        // SAFETY: a renderer is `{ FrameScratch, DrawScratch }`, each only
        // `heapless::Vec`s — no references, no enum with a non-zero discriminant, no
        // `bool` that must be non-zero — so the all-zero bit pattern is the empty
        // renderer (`len = 0`, write-before-read buffers). The caller guarantees the
        // slot is a valid, owned, aligned region; `write_bytes` initializes it whole.
        unsafe { slot.write_bytes(0u8, 1) }
    }

    /// Render the visible map into `target`.
    ///
    /// Selects the LOD for the viewport's meters-per-pixel, clears to `bg`,
    /// collects the visible features into reused buffers in global priority order
    /// ([`FrameScratch::collect`]), orders them by style z-index (painter's
    /// algorithm) and draws polygons (even-odd scanline fill) and lines.
    /// `color_fn` maps a style's RGB565 to the target's pixel color, letting the
    /// host choose true-color vs. device quantization while the device passes its
    /// native map.
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
        // Untimed path: the zero-cost `NoopClock` leaves the `*_us` stage fields at 0 and lets
        // the optimizer fold the timing away. Callers wanting the per-stage breakdown (the
        // device render benchmark) use `render_timed` with a real clock.
        self.render_timed(target, reader, vp, bg, color_fn, &NoopClock)
    }

    /// Like [`render`](MapRenderer::render) but fills the per-stage timings on the returned
    /// [`RenderStats`] (`collect_us` / `sort_us` / `draw_us`) from `clock` — a `no_std`-friendly
    /// microsecond source the caller supplies (the device's DWT cycle counter; a host an
    /// `Instant`). The stages cover the base map only; see the [`RenderStats`] stage-field docs
    /// and [`Clock`].
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

        // Collect → painter's order → draw. `seq` is the stable, alloc-free
        // tie-break within a z-index.
        //
        // Snapshot the streamed-map cache counters across `collect` (the only phase that reads
        // the map source) and record the per-frame delta — robust whether the caller hands us a
        // fresh `Reader` each frame (sim / device) or a reused one. (#37)
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

        // `collect` = walk + read + decode; `sort` = painter order; `draw` = the full-screen
        // clear plus rasterization (the clear is a framebuffer write, so it belongs with draw
        // even though it ran first). `saturating_sub` guards a momentarily non-monotonic clock;
        // a frame is well under a second, so the `u32` µs casts never truncate.
        stats.collect_us = t_collected.saturating_sub(t_cleared) as u32;
        stats.sort_us = t_sorted.saturating_sub(t_collected) as u32;
        stats.draw_us = (t_cleared.saturating_sub(t0) + t_drawn.saturating_sub(t_sorted)) as u32;

        stats
    }

    /// Draw the collected, painter-ordered spans into `target` (the map's "draw
    /// phase"). Polygons fill via even-odd scanline; lines stroke via the
    /// view-clipped overlay path. Kept separate from collection so each is read
    /// (and, later, extended — see `docs/rendering_pipeline.md` §9d) on its own.
    fn draw_map<D, F>(&mut self, target: &mut D, vp: &Viewport, color_fn: &F)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        // Disjoint borrows: the spans/geometry are read from `frame` while the
        // draw scratch (`screen`/`xs`) is written.
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

    /// Draw the user-position marker: a chevron at `(lon, lat)` pointing along
    /// `course` (degrees CW from north), or a non-directional diamond when
    /// `course` is `None` (stationary fix). Screen-space size is fixed
    /// (zoom-independent). Call **after** [`render`](MapRenderer::render) so it
    /// sits on top of the map. Skips drawing when the anchor projects outside the
    /// view (with a small margin), so an off-screen fix in Free mode draws nothing.
    ///
    /// `color` is the already-resolved device color (the app passes it through the
    /// host's `color_fn`, same as map styles), so the marker quantizes correctly
    /// on the device and stays true-color in the simulator.
    pub fn draw_marker<D>(
        &mut self,
        target: &mut D,
        vp: &Viewport,
        lon: i32,
        lat: i32,
        course: Option<f32>,
        color: D::Color,
    ) where
        D: DrawTarget,
    {
        let (w, h) = (vp.w as i32, vp.h as i32);
        let (sx, sy) = vp.to_screen(lon, lat);
        // Cull when the anchor is well off-screen (Free-mode fix outside the view).
        // The glyph is small, so a modest margin keeps a just-off-edge marker visible.
        const MARGIN: i32 = 16;
        if sx < -MARGIN || sx > w + MARGIN || sy < -MARGIN || sy > h + MARGIN {
            return;
        }

        // On-screen "forward" unit vector: project a point a ground step ahead
        // along the course and take the screen delta. Letting the projection do
        // the rotation makes this correct for both north-up and heading-up with no
        // special case. The step is sized so integer screen rounding barely skews
        // the direction; we normalize, so its exact length doesn't matter.
        let forward = course.and_then(|deg| {
            let theta = deg.to_radians();
            let step = (64.0 / vp.zoom).clamp(1.0, 100_000.0);
            let lon2 = lon as f32 + libm::sinf(theta) * step / vp.aspect;
            let lat2 = lat as f32 + libm::cosf(theta) * step;
            let (sx2, sy2) = vp.to_screen(lon2 as i32, lat2 as i32);
            let (dx, dy) = ((sx2 - sx) as f32, (sy2 - sy) as f32);
            let len = libm::sqrtf(dx * dx + dy * dy);
            (len > 1e-3).then(|| (dx / len, dy / len))
        });

        let (cx, cy) = (sx as f32, sy as f32);
        match forward {
            // Chevron: a tip a bit ahead and two base corners swept back and out.
            Some(fwd) => {
                const TIP: f32 = 12.0;
                const BACK: f32 = 6.0;
                const HALF: f32 = 8.0;
                fill_chevron(target, &mut self.draw.xs, (cx, cy), fwd, TIP, BACK, HALF, color, w, h);
            }
            // Stationary glyph: a small orientation-free diamond.
            None => {
                const R: f32 = 7.0;
                let diamond = [round_pt(cx, cy - R), round_pt(cx + R, cy), round_pt(cx, cy + R), round_pt(cx - R, cy)];
                fill_polygon(target, &diamond, &[4], color, w, h, &mut self.draw.xs);
            }
        }
    }

    /// Stroke an active route as a polyline overlay, with optional travel-direction
    /// chevrons. Call **after** [`render`](MapRenderer::render) so it sits on top of the map.
    ///
    /// Streams chunk-by-chunk — only chunks intersecting the view are decoded and stroked, via
    /// [`stroke_overlay`] (view-clipped, so embedded-graphics only pays for the visible part,
    /// not the ~96 % off-screen at riding zoom). Consecutive chunks share a seam vertex so the
    /// strokes join.
    ///
    /// `arrows_at` is the rider's matched route distance (m), or `None` to skip chevrons. When
    /// set, chevrons are drawn in a **second pass** (so they sit on top where the route doubles
    /// back) within a window of [`ARROW_AHEAD_COUNT`] chevrons around that distance, each pinned
    /// to a multiple of the screen-relative spacing — see the chevron constants above.
    ///
    /// Returns `(chunks, points, drawn)`: chunks decoded (bbox met the view), the points across
    /// them (the route has no LOD, so this grows as you zoom out of a long route), and how many
    /// of those vertices were *actually* stroked after the per-segment view clip + subpixel
    /// simplify — so `drawn` ≪ `points` when most of the route is off-screen. The caller folds
    /// all three into [`RenderStats`] for the stats panel.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_route<D>(
        &mut self,
        target: &mut D,
        vp: &Viewport,
        route: &obc_route::RouteReader,
        color: D::Color,
        weight: u32,
        arrow_color: D::Color,
        arrows_at: Option<u32>,
    ) -> (usize, usize, usize)
    where
        D: DrawTarget,
    {
        let (w, h) = (vp.w as i32, vp.h as i32);
        let view = vp.visible_bbox();
        let chunks = route.chunks();
        let mut pts = Vec::<obc_route::RoutePoint, { obc_route::MAX_POINTS_PER_CHUNK }>::new();
        // Split the borrow so the fills can take `xs` while we build the polyline in `screen`.
        let DrawScratch { screen, xs } = &mut self.draw;
        let (mut route_chunks, mut route_points, mut route_drawn) = (0usize, 0usize, 0usize);

        // Pass 1 — stroke every visible chunk, in full, before any chevron is drawn.
        for (k, cm) in chunks.iter().enumerate() {
            if !cm.bbox.intersects(&view) {
                continue;
            }
            if route.decode_chunk(k, &mut pts).is_err() {
                continue;
            }
            // Tally what we actually decode + stroke (the stat the panel shows). Adjacent chunks
            // share a seam vertex, so this counts it on both — matching the points eg strokes.
            route_chunks += 1;
            route_points += pts.len();
            let projected = pts.iter().map(|p| vp.project(p.lon, p.lat));
            route_drawn += stroke_overlay(target, screen, xs, projected, color, weight, w, h);
        }

        // Pass 2 — chevrons, anchored to route distance and windowed around the rider.
        let Some(progress_m) = arrows_at else {
            return (route_chunks, route_points, route_drawn);
        };
        let total = route.total_distance_m;
        // Ground spacing for *this* frame: a fixed screen cadence scaled by the current m/px, so
        // the chevrons keep an even spread as you zoom (the `.max` is a divide-by-zero guard for
        // absurd zoom-in). The window is then a chevron *count* either side of the rider.
        let spacing_m = (ARROW_SPACING_PX * vp.meters_per_pixel()).max(1e-3);
        let lo = (progress_m as f32 - ARROW_BEHIND_COUNT as f32 * spacing_m).max(0.0);
        let hi = (progress_m as f32 + ARROW_AHEAD_COUNT as f32 * spacing_m).min(total as f32);
        for (k, cm) in chunks.iter().enumerate() {
            // Skip chunks whose cumulative-distance span misses the window (then the view).
            let chunk_start = cm.cum_distance_m as f32;
            let chunk_end = chunks.get(k + 1).map_or(total, |c| c.cum_distance_m) as f32;
            if chunk_end < lo || chunk_start > hi || !cm.bbox.intersects(&view) {
                continue;
            }
            if route.decode_chunk(k, &mut pts).is_err() {
                continue;
            }
            walk_route_arrows(&pts, chunk_start, lo, hi, spacing_m, vp.aspect, |a, b, f| {
                let (ax, ay) = vp.to_screen(a.lon, a.lat);
                let (bx, by) = vp.to_screen(b.lon, b.lat);
                let (ax, ay, bx, by) = (ax as f32, ay as f32, bx as f32, by as f32);
                let (dx, dy) = (bx - ax, by - ay);
                let m = dx.abs().max(dy.abs()) + 0.41 * dx.abs().min(dy.abs());
                if m < 1e-3 {
                    return;
                }
                let fwd = (dx / m, dy / m); // screen travel dir (north-up & heading-up)
                let centre = (ax + dx * f, ay + dy * f); // chevron centre along the segment
                fill_chevron(target, xs, centre, fwd, ARROW_TIP, ARROW_BACK, ARROW_HALF, arrow_color, w, h);
            });
        }
        (route_chunks, route_points, route_drawn)
    }

    /// Stroke a single polyline of `(lon, lat)` microdegree points as an overlay, clipped to the
    /// view (see [`stroke_overlay`]) — this is the recorded **breadcrumb**, whose two tiers (spine,
    /// recent) are each one call. Call after [`render`](MapRenderer::render) so the path sits on
    /// the map.
    pub fn stroke_path<D, I>(&mut self, target: &mut D, vp: &Viewport, pts: I, color: D::Color, weight: u32)
    where
        D: DrawTarget,
        I: IntoIterator<Item = (i32, i32)>,
    {
        let (w, h) = (vp.w as i32, vp.h as i32);
        let projected = pts.into_iter().map(|(lon, lat)| vp.project(lon, lat));
        // Split the borrow so the span fills can take `xs` while the run builds in `screen`.
        let DrawScratch { screen, xs } = &mut self.draw;
        stroke_overlay(target, screen, xs, projected, color, weight, w, h);
    }
}

/// Cohen–Sutherland outcode: bit 1 = left, 2 = right, 4 = above the top, 8 = below the bottom.
#[inline]
fn outcode(x: f32, y: f32, xmin: f32, ymin: f32, xmax: f32, ymax: f32) -> u8 {
    let mut c = 0;
    if x < xmin {
        c |= 1;
    } else if x > xmax {
        c |= 2;
    }
    if y < ymin {
        c |= 4;
    } else if y > ymax {
        c |= 8;
    }
    c
}

/// Clip segment `a`→`b` to the rectangle (Cohen–Sutherland), returning the visible sub-segment
/// rounded back to integer pixels, or `None` if it misses the rectangle entirely.
fn clip_segment(a: Point, b: Point, xmin: f32, ymin: f32, xmax: f32, ymax: f32) -> Option<(Point, Point)> {
    let (mut x0, mut y0) = (a.x as f32, a.y as f32);
    let (mut x1, mut y1) = (b.x as f32, b.y as f32);
    let mut o0 = outcode(x0, y0, xmin, ymin, xmax, ymax);
    let mut o1 = outcode(x1, y1, xmin, ymin, xmax, ymax);
    loop {
        if o0 | o1 == 0 {
            return Some((round_pt(x0, y0), round_pt(x1, y1)));
        }
        if o0 & o1 != 0 {
            return None; // both ends past the same edge — wholly outside
        }
        let o = if o0 != 0 { o0 } else { o1 };
        let (x, y) = if o & 8 != 0 {
            (x0 + (x1 - x0) * (ymax - y0) / (y1 - y0), ymax)
        } else if o & 4 != 0 {
            (x0 + (x1 - x0) * (ymin - y0) / (y1 - y0), ymin)
        } else if o & 2 != 0 {
            (xmax, y0 + (y1 - y0) * (xmax - x0) / (x1 - x0))
        } else {
            (xmin, y0 + (y1 - y0) * (xmin - x0) / (x1 - x0))
        };
        if o == o0 {
            x0 = x;
            y0 = y;
            o0 = outcode(x0, y0, xmin, ymin, xmax, ymax);
        } else {
            x1 = x;
            y1 = y;
            o1 = outcode(x1, y1, xmin, ymin, xmax, ymax);
        }
    }
}

/// The `cos²θ` threshold below which a `weight`-px thick stroke's bare butt-join is already within
/// ½ px of a round joint, so the vertex needs no round-join disc in [`flush_run`]. The span stroke
/// lays each segment as a rectangle whose butt ends meet at the vertex; on the outer side of a turn
/// of `θ` (off straight) that leaves an uncovered notch about `r·sin(θ/2)` deep (`r = weight/2`).
/// Sub-pixel means `r·sin(θ/2) ≤ ½`, i.e. `sin(θ/2) ≤ 1/weight`, so the cut-off cosine is
/// `cosθ = 1 − 2·sin²(θ/2) = 1 − 2·(1/weight)²` — returned squared for the magnitude-folded test.
/// At the route's weight 11 that's a ~10° cut-off; a sharper bend gets its disc.
#[inline]
fn joint_disc_cos2(weight: u32) -> f32 {
    let sin_half = (1.0 / weight as f32).min(1.0); // ½px ÷ (weight/2)
    let cos = 1.0 - 2.0 * sin_half * sin_half;
    if cos <= 0.0 {
        0.0 // cos ≤ 0 ⇒ every turn discs — the `dot ≤ 0` guard in `turn_is_sharp` already covers it
    } else {
        cos * cos
    }
}

/// Whether the polyline turns sharply enough at `b` (across `a → b → c`) that its butt-join notch
/// would show — i.e. `cos²θ` falls below `cos2` ([`joint_disc_cos2`]). A reversed/obtuse turn
/// (non-positive dot) always qualifies; otherwise compare with the segment magnitudes folded in
/// (no `sqrt`, no `acos`).
#[inline]
fn turn_is_sharp(a: Point, b: Point, c: Point, cos2: f32) -> bool {
    let (ux, uy) = ((b.x - a.x) as f32, (b.y - a.y) as f32);
    let (vx, vy) = ((c.x - b.x) as f32, (c.y - b.y) as f32);
    let dot = ux * vx + uy * vy;
    if dot <= 0.0 {
        return true; // ≥ 90° turn (or a degenerate spur): always disc it
    }
    // sharp ⇔ cosθ < √cos2 ⇔ dot² < cos2 · |u|²|v|²  (dot ≥ 0, so squaring keeps the sense)
    dot * dot < cos2 * (ux * ux + uy * uy) * (vx * vx + vy * vy)
}

/// Fill a solid disc of radius `r` px centred at `(cx, cy)` as horizontal spans — one
/// [`fill_solid`](DrawTarget::fill_solid) per row (`hw = √(r² − dy²)`), the same coalesced row
/// fill the polygon scanline uses, rather than embedded-graphics' per-pixel `Circle`. Rounds the
/// thick stroke's joints and caps. Rows off the top/bottom are skipped; `fill_solid` clips x.
fn fill_disc<D>(target: &mut D, cx: i32, cy: i32, r: i32, color: D::Color, h: i32)
where
    D: DrawTarget,
{
    if r < 1 {
        return;
    }
    let r2 = (r * r) as f32;
    for dy in -r..=r {
        let y = cy + dy;
        if y < 0 || y >= h {
            continue;
        }
        let hw = libm::sqrtf((r2 - (dy * dy) as f32).max(0.0)) as i32;
        let _ = target.fill_solid(&Rectangle::new(Point::new(cx - hw, y), Size::new((2 * hw + 1) as u32, 1)), color);
    }
}

/// Lay down one segment of a thick stroke as a filled rectangle (the segment swept ±`hw` px along
/// its perpendicular) via the even-odd scanline [`fill_polygon`] — a convex quad, so every row has
/// exactly two crossings and fills as one span. A degenerate zero-length segment is left to the
/// joint/cap disc. Spans round **outward** (see `fill_polygon`), so adjacent quads and the joint
/// discs overlap by ≤1 px and leave no hairline crack.
#[allow(clippy::too_many_arguments)]
fn fill_thick_segment<D>(
    target: &mut D,
    a: Point,
    b: Point,
    hw: f32,
    color: D::Color,
    w: i32,
    h: i32,
    xs: &mut Vec<f32, MAX_CROSSINGS>,
) where
    D: DrawTarget,
{
    let (ax, ay, bx, by) = (a.x as f32, a.y as f32, b.x as f32, b.y as f32);
    let (dx, dy) = (bx - ax, by - ay);
    let len = libm::sqrtf(dx * dx + dy * dy);
    if len < 1e-3 {
        return;
    }
    let (nx, ny) = (-dy / len * hw, dx / len * hw); // perpendicular × half-width
    let quad = [
        round_pt(ax + nx, ay + ny),
        round_pt(bx + nx, by + ny),
        round_pt(bx - nx, by - ny),
        round_pt(ax - nx, ay - ny),
    ];
    fill_polygon(target, &quad, &[4], color, w, h, xs);
}

/// Rasterise the accumulated run, then clear it for the next.
///
/// A **1 px** stroke goes through embedded-graphics' `Polyline` — a thin Bresenham line it draws
/// well and cheaply, and the one width the span path below can't (a zero-width rectangle has no
/// scanline crossings). **Everything ≥ 2 px** — most of the map's roads, plus the route and
/// breadcrumb — is laid down as **spans**: a filled rectangle per segment ([`fill_thick_segment`])
/// plus a round-join/cap disc ([`fill_disc`]) at the two run ends (always — they round the cap and,
/// at a chunk seam, close the butt gap to the next feature) and at every interior vertex that bends
/// sharply enough to show a notch ([`turn_is_sharp`]). Both go through the framebuffer's coalesced
/// `fill_solid`, not the per-pixel `draw_iter` eg's thick `Polyline` + `Circle` ran through. eg's
/// thick path was the overlay's former dominant cost, and (measured) it's ~10× a span stroke even
/// at 2 px — the narrowest width that path enters — so the split sits at 1 px, not 2.
#[allow(clippy::too_many_arguments)]
fn flush_run<D>(
    target: &mut D,
    run: &mut Vec<Point, MAX_SCREEN_POINTS>,
    color: D::Color,
    weight: u32,
    w: i32,
    h: i32,
    xs: &mut Vec<f32, MAX_CROSSINGS>,
) where
    D: DrawTarget,
{
    if run.len() >= 2 {
        if weight <= 1 {
            let _ = Polyline::new(run).into_styled(PrimitiveStyle::with_stroke(color, weight)).draw(target);
        } else {
            // The body's half-width is the integer disc radius, not `weight/2` — so the rectangle
            // and the round-join/cap disc come out the same thickness (the disc never narrower than
            // the body it caps), and an odd `weight` lands on its nominal width instead of rounding
            // a px fatter.
            let r = (weight / 2) as i32;
            let hw = r as f32;
            for seg in run.windows(2) {
                fill_thick_segment(target, seg[0], seg[1], hw, color, w, h, xs);
            }
            let cos2 = joint_disc_cos2(weight);
            let n = run.len();
            fill_disc(target, run[0].x, run[0].y, r, color, h);
            for i in 1..n - 1 {
                if turn_is_sharp(run[i - 1], run[i], run[i + 1], cos2) {
                    fill_disc(target, run[i].x, run[i].y, r, color, h);
                }
            }
            fill_disc(target, run[n - 1].x, run[n - 1].y, r, color, h);
        }
    }
    run.clear();
}

/// Screen-space simplification tolerance (px) for [`stroke_overlay`]. **Subpixel** by design:
/// big enough to fold away the integer-projection staircase (≤ ½ px deviations) and the
/// same-pixel vertex pile-ups that make eg's thick `Polyline` bead, but under 1 px so the
/// stroked line never shifts a visible pixel — beading goes, road/route shape stays.
const SIMPLIFY_EPS_PX: f32 = 0.75;

/// True when `p` lies within `eps` px (perpendicular) of the infinite line through `a` and `b`
/// — the near-collinear test [`simplify`] uses to drop redundant vertices. Cross / length-squared
/// in `f32` (no `sqrt`); degenerate `a == b` falls back to `|p − a|`.
#[inline]
fn within_eps(p: Point, a: Point, b: Point, eps: f32) -> bool {
    let (abx, aby) = ((b.x - a.x) as f32, (b.y - a.y) as f32);
    let (apx, apy) = ((p.x - a.x) as f32, (p.y - a.y) as f32);
    let cross = apx * aby - apy * abx;
    let len_sq = abx * abx + aby * aby;
    let e2 = eps * eps;
    if len_sq < 1e-6 {
        return apx * apx + apy * apy <= e2; // a == b: distance to the point
    }
    cross * cross <= e2 * len_sq // (cross / len)² ≤ eps²  ⇔  perp-dist ≤ eps
}

/// Clip one committed segment `a`→`b` to the view and append it to the current run, flushing
/// where the line is discontinuous (segment off-screen, or it doesn't continue the last run).
/// Pulled out of [`stroke_overlay`] so the main loop can feed it the *simplified* segments.
///
/// Returns how many **real on-screen vertices** this segment contributed (0 if wholly clipped
/// out) — `c1` always, plus `c0` when it (re)starts a run — so the caller's running total is the
/// points actually rasterised, not decoded.
#[allow(clippy::too_many_arguments)]
fn stroke_seg<D>(
    target: &mut D,
    run: &mut Vec<Point, MAX_SCREEN_POINTS>,
    a: Point,
    b: Point,
    color: D::Color,
    weight: u32,
    clip: (f32, f32, f32, f32),
    w: i32,
    h: i32,
    xs: &mut Vec<f32, MAX_CROSSINGS>,
) -> usize
where
    D: DrawTarget,
{
    let (xmin, ymin, xmax, ymax) = clip;
    match clip_segment(a, b, xmin, ymin, xmax, ymax) {
        None => {
            flush_run(target, run, color, weight, w, h, xs); // segment wholly off-screen
            0
        }
        Some((c0, c1)) => {
            let mut drawn = 1; // c1
                               // (Re)start a run if this segment didn't continue the previous one.
            if run.last().copied() != Some(c0) {
                flush_run(target, run, color, weight, w, h, xs);
                let _ = run.push(c0);
                drawn += 1; // c0 enters the view here
            }
            let _ = run.push(c1);
            // Clipped at its far end → the line left the view here; close this run.
            if c1 != b {
                flush_run(target, run, color, weight, w, h, xs);
            }
            drawn
        }
    }
}

/// Streaming one-lookahead collinear simplification: calls `emit` for the first vertex, the
/// last, and every vertex of `points` that bends off the line through its kept neighbours by
/// more than `eps` px ([`within_eps`]) — dropping the rest. O(1) state; each dropped vertex lies
/// within `eps` of the kept path, so the simplified line never leaves that tolerance.
fn simplify<I, F>(points: I, eps: f32, mut emit: F)
where
    I: IntoIterator<Item = Point>,
    F: FnMut(Point),
{
    let mut anchor: Option<Point> = None; // last kept (emitted) vertex
    let mut held: Option<Point> = None; // candidate, kept only if it bends away by > eps
    for cur in points {
        match (anchor, held) {
            (None, _) => {
                anchor = Some(cur);
                emit(cur);
            }
            (Some(_), None) => held = Some(cur),
            (Some(a), Some(hp)) => {
                if within_eps(hp, a, cur, eps) {
                    held = Some(cur); // `hp` redundant — extend the straight run through it
                } else {
                    emit(hp);
                    anchor = Some(hp);
                    held = Some(cur);
                }
            }
        }
    }
    if let Some(hp) = held {
        emit(hp); // tail vertex
    }
}

/// Clip a projected overlay polyline to the view and stroke the on-screen runs ([`flush_run`]).
/// Clipping first (Cohen–Sutherland, into the screen grown by the stroke width so an edge-hugging
/// line keeps its full thickness) means the stroker only ever pays for the visible part — vital
/// when the route/breadcrumb is ~96% off-screen at riding zoom: the line splits into separate runs
/// where it crosses the view, each stroked on its own.
///
/// The points are first **simplified in screen space** ([`simplify`] at [`SIMPLIFY_EPS_PX`]) — a
/// *subpixel* dedup that folds away the integer-projection staircase and same-pixel vertex
/// pile-ups a dense route/road carries when zoomed out. It never moves the line a visible pixel,
/// so no shape is lost; it just hands the stroker far fewer segments and joints. The `run` scratch
/// is reused; `xs` is the shared crossing buffer the span fills need.
///
/// Returns the count of **on-screen vertices actually stroked** (after the subpixel simplify and
/// the view clip) — far fewer than the points fed in when the line is mostly off-screen or
/// folds to the same pixels. Callers that want the stat (the route overlay) sum it; the
/// breadcrumb ignores it.
#[allow(clippy::too_many_arguments)]
fn stroke_overlay<D, I>(
    target: &mut D,
    run: &mut Vec<Point, MAX_SCREEN_POINTS>,
    xs: &mut Vec<f32, MAX_CROSSINGS>,
    points: I,
    color: D::Color,
    weight: u32,
    w: i32,
    h: i32,
) -> usize
where
    D: DrawTarget,
    I: IntoIterator<Item = Point>,
{
    let weight = weight.max(1);
    let m = weight as f32 + 2.0; // clip margin ≥ half-width, so edge strokes still paint in
    let clip = (-m, -m, w as f32 + m, h as f32 + m);
    run.clear();

    // Simplify in screen space, then stroke consecutive kept vertices as clipped segments — their
    // runs join because each segment starts where the previous ended.
    let mut prev: Option<Point> = None;
    let mut drawn = 0usize;
    simplify(points, SIMPLIFY_EPS_PX, |v| {
        if let Some(a) = prev {
            drawn += stroke_seg(target, run, a, v, color, weight, clip, w, h, xs);
        }
        prev = Some(v);
    });
    flush_run(target, run, color, weight, w, h, xs);
    drawn
}

/// Walk a decoded route chunk (its absolute points plus `s0`, the cumulative route distance
/// in metres at its first point) and call `emit(&a, &b, f)` for every chevron whose route
/// distance is a multiple of `spacing_m` lying inside `[lo, hi]` — `f` is the fraction along
/// segment `a`→`b`. Anchoring to the route's own cumulative distance (rather than the screen)
/// pins each chevron to one ground spot as the camera pans; the `[lo, hi]` window keeps them
/// near the rider. `spacing_m` is the per-frame ground spacing the caller derives from the zoom
/// (see [`ARROW_SPACING_PX`]). Segment length is real ground metres
/// ([`obc_route::ground_dist_m_cl`]); `cl` is the band's hoisted `cos(lat)` (the caller
/// passes the viewport's, computed once per frame), so the walk costs no per-segment `cosf`.
fn walk_route_arrows<F>(pts: &[obc_route::RoutePoint], s0: f32, lo: f32, hi: f32, spacing_m: f32, cl: f32, mut emit: F)
where
    F: FnMut(&obc_route::RoutePoint, &obc_route::RoutePoint, f32),
{
    let mut s = s0;
    for seg in pts.windows(2) {
        let (a, b) = (&seg[0], &seg[1]);
        let dl = obc_route::ground_dist_m_cl((a.lon, a.lat), (b.lon, b.lat), cl);
        if dl > 1e-3 {
            // Grid multiples of spacing_m that fall on this segment and in the window.
            let lo_seg = s.max(lo);
            let hi_seg = (s + dl).min(hi);
            let mut n = libm::ceilf(lo_seg / spacing_m) * spacing_m;
            while n <= hi_seg {
                emit(a, b, ((n - s) / dl).clamp(0.0, 1.0));
                n += spacing_m;
            }
        }
        s += dl;
    }
}

/// Project a feature's microdegree rings into `screen` and scanline-fill them.
/// The draw phase's `Kind::Polygon` arm; also the marker diamond's path. `screen`
/// and `xs` are the reused draw scratch; the screen bounds come from `vp`.
fn fill_polygon_proj<D>(
    target: &mut D,
    vp: &Viewport,
    pts: &[(i32, i32)],
    ring_lens: &[usize],
    color: D::Color,
    screen: &mut Vec<Point, MAX_SCREEN_POINTS>,
    xs: &mut Vec<f32, MAX_CROSSINGS>,
) where
    D: DrawTarget,
{
    screen.clear();
    for &(lon, lat) in pts {
        let _ = screen.push(vp.project(lon, lat));
    }
    fill_polygon(target, screen, ring_lens, color, vp.w as i32, vp.h as i32, xs);
}

/// Project and stroke one map line (its exterior ring). The draw phase's
/// `Kind::Line` arm — factored out as the single point where per-feature line
/// styling (dashes, casing) will branch later (see `docs/rendering_pipeline.md`
/// §9d). Uses the same view-clipped stroke as the route/breadcrumb overlays:
/// clipping spares the off-screen part of a line whose chunk straddles the view
/// edge (most visible at coarse zoom), while thick classes get the span-filled
/// rectangles + round joints and thin ones eg's `Polyline` (see [`flush_run`]).
fn draw_line<D>(
    target: &mut D,
    vp: &Viewport,
    pts: &[(i32, i32)],
    color: D::Color,
    weight: u32,
    screen: &mut Vec<Point, MAX_SCREEN_POINTS>,
    xs: &mut Vec<f32, MAX_CROSSINGS>,
) where
    D: DrawTarget,
{
    let projected = pts.iter().map(|&(lon, lat)| vp.project(lon, lat));
    stroke_overlay(target, screen, xs, projected, color, weight, vp.w as i32, vp.h as i32);
}

/// Fill a 3-point direction chevron centred at `c`, pointing along the unit
/// vector `fwd`: a tip `tip` px ahead and two base corners swept `back` px behind
/// and `half` px out to each side (all screen pixels). Shared by the user-position
/// marker ([`MapRenderer::draw_marker`]) and the route arrows
/// ([`MapRenderer::draw_route`]); the caller supplies `fwd` already normalized
/// (exact for the marker, alpha-max-beta-min for the arrows). `xs` is the reused
/// scanline-crossing scratch — the 3-point glyph needs no `screen` buffer.
#[allow(clippy::too_many_arguments)]
fn fill_chevron<D>(
    target: &mut D,
    xs: &mut Vec<f32, MAX_CROSSINGS>,
    c: (f32, f32),
    fwd: (f32, f32),
    tip: f32,
    back: f32,
    half: f32,
    color: D::Color,
    w: i32,
    h: i32,
) where
    D: DrawTarget,
{
    let (fx, fy) = fwd;
    let (rx, ry) = (-fy, fx); // right perpendicular = base spread
    let tri = [
        round_pt(c.0 + fx * tip, c.1 + fy * tip),
        round_pt(c.0 - fx * back + rx * half, c.1 - fy * back + ry * half),
        round_pt(c.0 - fx * back - rx * half, c.1 - fy * back - ry * half),
    ];
    fill_polygon(target, &tri, &[3], color, w, h, xs);
}

/// Scanline even-odd polygon fill. `screen` holds every ring's projected points
/// concatenated; `ring_lens` partitions them (exterior first, then holes — holes
/// fall out of the even-odd rule for free). `xs` is a reused crossing buffer; a
/// row that overflows it is skipped to keep even-odd parity intact rather than
/// pairing spans from a truncated crossing list (issue #3).
fn fill_polygon<D>(
    target: &mut D,
    screen: &[Point],
    ring_lens: &[usize],
    color: D::Color,
    w: i32,
    h: i32,
    xs: &mut Vec<f32, MAX_CROSSINGS>,
) where
    D: DrawTarget,
{
    let mut ymin = i32::MAX;
    let mut ymax = i32::MIN;
    for p in screen {
        ymin = ymin.min(p.y);
        ymax = ymax.max(p.y);
    }
    ymin = ymin.max(0);
    ymax = ymax.min(h - 1);
    if ymin > ymax {
        return;
    }
    for y in ymin..=ymax {
        let yc = y as f32 + 0.5;
        xs.clear();
        let mut base = 0usize;
        let mut saturated = false;
        'rings: for &len in ring_lens {
            let ring = &screen[base..base + len];
            base += len;
            if len < 2 {
                continue;
            }
            let mut j = len - 1;
            for i in 0..len {
                let (xi, yi) = (ring[i].x as f32, ring[i].y as f32);
                let (xj, yj) = (ring[j].x as f32, ring[j].y as f32);
                if (yi <= yc && yc < yj) || (yj <= yc && yc < yi) {
                    // A row crossing the outline more than MAX_CROSSINGS times
                    // can't be captured whole; pairing a truncated list would
                    // break even-odd parity and paint background-colored gaps
                    // across the row (issue #3). Stop and skip the row instead —
                    // an unfilled 1px seam, only on the densest features, beats a
                    // mis-filled span, and the buffer can't grow without busting
                    // the MCU_RENDERER_BYTES budget asserted above.
                    if xs.push(xi + (yc - yi) / (yj - yi) * (xj - xi)).is_err() {
                        saturated = true;
                        break 'rings;
                    }
                }
                j = i;
            }
        }
        if saturated || xs.len() < 2 {
            continue;
        }
        xs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
        let mut k = 0;
        while k + 1 < xs.len() {
            // Round spans *outward* (floor left, ceil right) to close hairline
            // background gaps between adjacent fills. A feature clipped across a
            // chunk boundary (`obc-pack`'s `quadtree.rs`) becomes two polygons whose
            // shared edge is clipped independently, so their pixel staircases can
            // disagree by ≤1px (most visible along a rotated diagonal seam).
            // `to_screen`'s round-to-nearest collapses nearly all of it; this ≤1px
            // outward overlap is the cheap insurance (invisible for same-colored
            // fills). See `firmware/docs/render_followups.md` item 2.
            let x0 = (libm::floorf(xs[k]) as i32).max(0);
            let x1 = (libm::ceilf(xs[k + 1]) as i32).min(w - 1);
            if x1 >= x0 {
                let _ =
                    target.fill_solid(&Rectangle::new(Point::new(x0, y), Size::new((x1 - x0 + 1) as u32, 1)), color);
            }
            k += 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        aspect_for_lat, fill_polygon, joint_disc_cos2, simplify, turn_is_sharp, walk_route_arrows, within_eps,
        MAX_CROSSINGS,
    };
    use embedded_graphics::prelude::Point;
    use heapless::Vec;
    use obc_route::{ground_dist_m, RoutePoint};

    /// Collect the vertices [`simplify`] keeps from `pts` at tolerance `eps`.
    fn kept(pts: &[Point], eps: f32) -> Vec<Point, 64> {
        let mut out = Vec::new();
        simplify(pts.iter().copied(), eps, |p| {
            let _ = out.push(p);
        });
        out
    }

    #[test]
    fn within_eps_is_perpendicular_distance() {
        let (a, b) = (Point::new(0, 0), Point::new(10, 0)); // the x-axis
        assert!(within_eps(Point::new(5, 0), a, b, 0.5), "on the line");
        assert!(!within_eps(Point::new(5, 1), a, b, 0.5), "1 px off > 0.5 tol");
        assert!(within_eps(Point::new(5, 1), a, b, 1.5), "1 px off < 1.5 tol");
        // Degenerate a == b falls back to the point distance |p − a|.
        assert!(within_eps(Point::new(0, 1), a, a, 1.5));
        assert!(!within_eps(Point::new(0, 2), a, a, 1.5));
    }

    #[test]
    fn turn_is_sharp_discs_only_notch_corners() {
        let cos2 = joint_disc_cos2(11); // route weight ⇒ ~10° cut-off
        let b = Point::new(100, 0);
        // Collinear continuation: never a disc.
        assert!(!turn_is_sharp(Point::new(0, 0), b, Point::new(200, 0), cos2));
        // A ~6° bend stays under the cut-off — the butt-join notch is sub-pixel: no disc.
        assert!(!turn_is_sharp(Point::new(0, 0), b, Point::new(200, 10), cos2));
        // A ~27° bend clears it: disc.
        assert!(turn_is_sharp(Point::new(0, 0), b, Point::new(200, 50), cos2));
        // A right-angle and a hairpin (non-positive dot) always disc.
        assert!(turn_is_sharp(Point::new(0, 0), b, Point::new(100, 50), cos2));
        assert!(turn_is_sharp(Point::new(0, 0), b, Point::new(0, 10), cos2));
        // A thinner stroke tolerates a wider bend before the notch shows (looser cut-off).
        assert!(joint_disc_cos2(3) < joint_disc_cos2(11));
    }

    #[test]
    fn simplify_collapses_the_subpixel_staircase() {
        // y = round(0.4·x): a straight line the integer projection turned into a staircase. Every
        // point sits within ½ px of the true line, so a subpixel tolerance drops all but the ends.
        let mut pts = Vec::<Point, 64>::new();
        for x in 0..=30 {
            let _ = pts.push(Point::new(x, libm::roundf(x as f32 * 0.4) as i32));
        }
        let out = kept(&pts, 0.75);
        assert!(out.len() <= 3, "staircase should collapse to ~the endpoints, kept {}", out.len());
        assert_eq!(out.first(), pts.first(), "keeps the start");
        assert_eq!(out.last(), pts.last(), "keeps the end");
    }

    #[test]
    fn simplify_keeps_a_real_corner() {
        // A right-angle L: the straight arms collapse, but the corner bends far past any subpixel
        // tolerance, so it survives — shape is preserved, only redundant vertices go.
        let mut pts = Vec::<Point, 64>::new();
        for x in 0..=10 {
            let _ = pts.push(Point::new(x, 0));
        }
        for y in 1..=10 {
            let _ = pts.push(Point::new(10, y));
        }
        let out = kept(&pts, 0.75);
        assert_eq!(out.len(), 3, "start, corner, end");
        assert_eq!(out[1], Point::new(10, 0), "the corner is kept");
    }

    /// Ground spacing used to drive the grid walker directly. In the app this is derived
    /// per-frame from the zoom (`ARROW_SPACING_PX × m/px`); the walker itself just takes metres,
    /// so these tests pin the grid maths with a fixed, easy-to-reason-about spacing.
    const SPACING: f32 = 33.0;

    /// A due-north two-point segment ~300 m long (fixed longitude, so its length is pure
    /// latitude — the chevron grid is easy to reason about). Returned with its ground length.
    fn north_line() -> (Vec<RoutePoint, 4>, f32) {
        let mut v = Vec::new();
        v.push(RoutePoint { lon: 7_800_000, lat: 48_000_000, ele: 0 }).unwrap();
        v.push(RoutePoint { lon: 7_800_000, lat: 48_002_700, ele: 0 }).unwrap();
        let dl = ground_dist_m((v[0].lon, v[0].lat), (v[1].lon, v[1].lat));
        (v, dl)
    }

    /// Route distances (m from the segment start) at which chevrons land for a window `[lo,hi]`.
    fn distances(pts: &[RoutePoint], dl: f32, lo: f32, hi: f32) -> Vec<i32, 64> {
        let mut v = Vec::new();
        let cl = aspect_for_lat(pts[0].lat);
        walk_route_arrows(pts, 0.0, lo, hi, SPACING, cl, |_, _, f| {
            let _ = v.push(libm::roundf(f * dl) as i32);
        });
        v
    }

    #[test]
    fn chevrons_land_on_the_spacing_grid() {
        // Chevrons sit at 0, SPACING, 2·SPACING, … of route distance — they're anchored to the
        // route, not the screen, so each is a fixed multiple of the spacing.
        let (pts, dl) = north_line();
        let ds = distances(&pts, dl, 0.0, dl);
        assert!(ds.len() >= 5, "a {dl:.0} m segment should carry several chevrons");
        for (i, d) in ds.iter().enumerate() {
            let expect = libm::roundf(i as f32 * SPACING) as i32;
            assert!((d - expect).abs() <= 1, "chevron {i} at {d} m, expected {expect} m");
        }
    }

    #[test]
    fn chevrons_stay_within_the_window() {
        // Only chevrons inside [lo, hi] are emitted, and a wider window strictly adds more.
        let (pts, dl) = north_line();
        let (lo, hi) = (50.0, 140.0);
        let narrow = distances(&pts, dl, lo, hi);
        assert!(!narrow.is_empty());
        for d in &narrow {
            assert!(*d as f32 >= lo - 0.5 && *d as f32 <= hi + 0.5, "chevron at {d} m outside window");
        }
        assert!(distances(&pts, dl, 0.0, dl).len() > narrow.len());
    }

    #[test]
    fn chevrons_are_pinned_to_route_distance_not_the_rider() {
        // The exact property the redesign is about: slide the window forward (as the rider
        // advances) and the chevrons still visible keep the *same* route distances — they do
        // not crawl with the rider. Here the shared [80, 200] m band must match between a
        // window centred earlier and one centred later.
        let (pts, dl) = north_line();
        let band = |lo, hi| -> Vec<i32, 64> {
            distances(&pts, dl, lo, hi).iter().copied().filter(|&d| (80..=200).contains(&d)).collect()
        };
        let early = band(0.0, 210.0);
        let late = band(70.0, 280.0);
        assert!(!early.is_empty(), "the shared band should contain chevrons");
        assert_eq!(early, late, "a chevron moved when the window slid — it should be ground-pinned");
    }

    #[test]
    fn fill_polygon_skips_rows_that_overflow_the_crossing_buffer() {
        // Issue #3: a scanline crossing the outline more than MAX_CROSSINGS times
        // must NOT be filled from the truncated crossing list (which corrupts the
        // even-odd parity and paints background-colored gaps). It must be skipped
        // instead, while ordinary rows of the same polygon still fill correctly.
        use embedded_graphics::{pixelcolor::BinaryColor, prelude::*, primitives::Rectangle};

        const P: usize = 200; // prongs → 2·P scanline crossings in the prong band
        const W: i32 = 2 * P as i32; // one column per prong + its gap
        const H: i32 = 8;
        const HBASE: i32 = 4; // prongs span y ∈ [0, HBASE); a solid base sits below
        const HBOTTOM: i32 = 6;
        // The comb only proves anything if it actually overflows the buffer.
        const { assert!(2 * P > MAX_CROSSINGS, "comb must exceed MAX_CROSSINGS to exercise saturation") };

        // Records pixels painted per row via fill_solid — the only primitive
        // fill_polygon uses — so a skipped row (0) is distinguishable from a
        // correctly filled one (full width).
        struct RowFill {
            rows: [u32; H as usize],
        }
        impl OriginDimensions for RowFill {
            fn size(&self) -> Size {
                Size::new(W as u32, H as u32)
            }
        }
        impl DrawTarget for RowFill {
            type Color = BinaryColor;
            type Error = core::convert::Infallible;
            fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
            where
                I: IntoIterator<Item = Pixel<Self::Color>>,
            {
                for Pixel(p, _) in pixels {
                    if (0..H).contains(&p.y) && (0..W).contains(&p.x) {
                        self.rows[p.y as usize] += 1;
                    }
                }
                Ok(())
            }
            fn fill_solid(&mut self, area: &Rectangle, _: Self::Color) -> Result<(), Self::Error> {
                let y = area.top_left.y;
                if (0..H).contains(&y) {
                    self.rows[y as usize] += area.size.width;
                }
                Ok(())
            }
        }

        // A comb: P vertical 1px prongs (1px gaps) standing on a solid base. A
        // scanline through the prongs crosses both walls of every prong (2·P);
        // one through the base crosses only the two outer walls.
        let mut poly: Vec<Point, 1024> = Vec::new();
        poly.push(Point::new(0, 0)).unwrap();
        for i in 0..P as i32 {
            let x1 = 2 * i + 1;
            poly.push(Point::new(x1, 0)).unwrap(); // prong top-right
            poly.push(Point::new(x1, HBASE)).unwrap(); // right wall down to base
            if i + 1 < P as i32 {
                poly.push(Point::new(x1 + 1, HBASE)).unwrap(); // base across the gap
                poly.push(Point::new(x1 + 1, 0)).unwrap(); // next prong's left wall up
            }
        }
        poly.push(Point::new(W - 1, HBOTTOM)).unwrap(); // right wall down past the base
        poly.push(Point::new(0, HBOTTOM)).unwrap(); // base bottom edge (closing edge → (0,0))

        let mut target = RowFill { rows: [0; H as usize] };
        let mut xs: Vec<f32, MAX_CROSSINGS> = Vec::new();
        let len = poly.len();
        fill_polygon(&mut target, &poly, &[len], BinaryColor::On, W, H, &mut xs);

        // Prong-band rows overflow the buffer → skipped, not mis-filled.
        for y in 0..HBASE {
            assert_eq!(target.rows[y as usize], 0, "saturated prong row {y} must be left unfilled, not mis-filled");
        }
        // Base-band rows have just two crossings → filled edge to edge.
        for y in HBASE..HBOTTOM {
            assert_eq!(target.rows[y as usize], W as u32, "base row {y} should fill the full width");
        }
    }
}
