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
    primitives::{Circle, Polyline, PrimitiveStyle, Rectangle},
};

use obc_reader::{BBox, Kind, Reader};

pub mod canvas;
mod font_data;
pub mod text;
pub use canvas::{rect, Canvas};
pub use text::{draw_text, text_width, Font, TextAlign};

// Per-frame buffer capacities. Statically allocated (heapless::Vec), tuned for an
// MCU with 512 KB RAM — growing one costs boot RAM, not per-frame. The
// MCU_RENDERER_BYTES assertion below guards the ~200 KB budget.

/// Maximum visible features per frame (each is a [`Span`] — 14 bytes). At coarse
/// zoom this is the buffer that saturates first (many small features), so it is
/// sized generously; see the RAM-budget assertion below.
pub const MAX_SPANS: usize = 3072;

/// Maximum total vertices across all visible features per frame (8 bytes each).
pub const MAX_FRAME_POINTS: usize = 12_288;

/// Maximum total ring entries across all visible features per frame.
pub const MAX_FRAME_RINGS: usize = 3072;

/// Maximum vertices for a single feature during decode (reused per feature).
pub const MAX_DECODE_POINTS: usize = 2048;

/// Maximum rings for a single feature during decode.
pub const MAX_DECODE_RINGS: usize = 32;

/// Maximum projected screen points for drawing one feature.
pub const MAX_SCREEN_POINTS: usize = 4096;

/// Maximum scanline crossings for polygon fill.
pub const MAX_CROSSINGS: usize = 256;

// `Span` packs its buffer offsets into `u16` to stay small, so the frame buffers
// it indexes must fit in a `u16`. These guard that invariant at compile time.
const _: () = assert!(MAX_FRAME_POINTS <= u16::MAX as usize, "Span::pt_start is u16");
const _: () = assert!(MAX_FRAME_RINGS <= u16::MAX as usize, "Span::ring_start is u16");
const _: () = assert!(MAX_SPANS <= u16::MAX as usize, "Span::seq is u16");

/// Static RAM the [`MapRenderer`]'s scratch buffers occupy on the 32-bit MCU
/// target (`usize` = 4 bytes there). Computed from the constants above so the
/// assertion below fails the build if a buffer is grown past the ~200 KB budget.
/// (`(i32, i32)` and `Point` are 8 bytes; `usize`/`f32` are 4 on the MCU.)
const MCU_RENDERER_BYTES: usize = MAX_DECODE_POINTS * 8
    + MAX_DECODE_RINGS * 4
    + MAX_FRAME_POINTS * 8
    + MAX_FRAME_RINGS * 4
    + MAX_SPANS * core::mem::size_of::<Span>()
    + MAX_SCREEN_POINTS * 8
    + MAX_CROSSINGS * 4;
const _: () = assert!(MCU_RENDERER_BYTES <= 200 * 1024, "MapRenderer exceeds the 200 KB MCU budget");

/// Meters of ground per microdegree of latitude — the renderer's zoom is pixels per
/// microdegree-lat, so this turns zoom into meters-per-pixel. Derived from the shared
/// [`obc_reader::M_PER_DEG`] (a microdegree is 1e-6°) so the on-screen scale tracks the
/// one Earth model the route/packer measure against.
const METERS_PER_MICRODEG_LAT: f32 = (obc_reader::M_PER_DEG / 1_000_000.0) as f32;

// Route direction chevrons (tunable). Arrowheads along the active route at riding zoom,
// anchored to route distance (not screen) so each stays pinned to a ground spot, drawn only
// in a window around the rider — so an out-and-back marks just the leg you're on, the right
// way round. Spacing + window are screen-relative — a fixed pixel cadence and a chevron *count*,
// not ground metres — so the chevrons keep an even spread across the finest LOD's zoom range
// (no bunching when zoomed out); the ground spacing is derived per-frame from the camera's
// m/px. Glyph sizes are screen pixels. Sweep with the app's `ROUTE_WEIGHT`.
//
/// On-screen gap between consecutive chevrons (px). Held in *screen* space, not ground metres:
/// each frame the route-distance spacing is `ARROW_SPACING_PX × m/px`, so the chevrons stay
/// evenly spread however far you zoom. At the ~0.5 m/px riding zoom this works out to ≈ 33 m
/// apart on the ground — the original feel.
const ARROW_SPACING_PX: f32 = 66.0;
/// How many chevrons lead *ahead* of the rider. A count (not a ground distance) so the
/// look-ahead tracks the screen cadence — the chevrons reach a fixed way up the screen at every
/// zoom. Off-screen ones are free, so this is generous (≈ the old 300 m at riding zoom).
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
/// on every side), the Garmin look — independent of whatever map colour the line crosses.
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

    /// Recompute the longitude aspect correction for the current camera latitude.
    /// Call after panning north/south so far-apart latitudes stay shaped right.
    pub fn refresh_aspect(&mut self) {
        self.aspect = aspect_for_lat(self.cam_lat);
    }

    #[inline]
    pub fn to_screen(&self, lon: i32, lat: i32) -> (i32, i32) {
        // Integer difference preserves absolute microdegree precision up to max.
        let delta_lon = lon.wrapping_sub(self.cam_lon);
        let delta_lat = lat.wrapping_sub(self.cam_lat);
        // Cast the small relative delta to f32.
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
        (libm::roundf(x) as i32, libm::roundf(y) as i32)
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
        let corners = [
            self.to_map(0.0, 0.0),
            self.to_map(self.w, 0.0),
            self.to_map(0.0, self.h),
            self.to_map(self.w, self.h),
        ];
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
        BBox {
            min_lon,
            min_lat,
            max_lon,
            max_lat,
        }
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

                    // Capacity check.
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
    // Buffer utilization (0.0–1.0) for saturation display.
    pub span_utilization: f32,
    pub point_utilization: f32,
    pub ring_utilization: f32,
    /// Host-measured wall time for the whole frame draw (render + route/overlays), in
    /// microseconds; `0` = not measured. `obc-render` is `no_std` and carries no clock,
    /// so the **host** fills this after timing the draw (the sim uses `Instant`; the
    /// device the Cortex-M DWT cycle counter) — kept on the stats so the control panel and
    /// the headless line surface it without a side channel.
    pub render_us: u32,
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
        let _ = target.clear(bg);

        let lod = reader.select_lod_for_mpp(vp.meters_per_pixel());
        let view = vp.visible_bbox();
        let mut stats = RenderStats { lod, ..Default::default() };

        // Collect → painter's order → draw. `seq` is the stable, alloc-free
        // tie-break within a z-index.
        self.frame.collect(reader, lod, &view, &mut stats);
        self.frame.spans.sort_unstable_by_key(|s| (s.z, s.seq));
        self.draw_map(target, vp, &color_fn);

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
                    draw_line(target, vp, &pts[..n], color, span.weight.max(1) as u32, &mut draw.screen);
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
                let pt = |x: f32, y: f32| Point::new(libm::roundf(x) as i32, libm::roundf(y) as i32);
                let diamond = [pt(cx, cy - R), pt(cx + R, cy), pt(cx, cy + R), pt(cx - R, cy)];
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
    ) where
        D: DrawTarget,
    {
        let (w, h) = (vp.w as i32, vp.h as i32);
        let view = vp.visible_bbox();
        let chunks = route.chunks();
        let mut pts = Vec::<obc_route::RoutePoint, { obc_route::MAX_POINTS_PER_CHUNK }>::new();
        // Split the borrow so the fills can take `xs` while we build the polyline in `screen`.
        let DrawScratch { screen, xs } = &mut self.draw;

        // Pass 1 — stroke every visible chunk, in full, before any chevron is drawn.
        for (k, cm) in chunks.iter().enumerate() {
            if !cm.bbox.intersects(&view) {
                continue;
            }
            if route.decode_chunk(k, &mut pts).is_err() {
                continue;
            }
            let projected = pts.iter().map(|p| {
                let (x, y) = vp.to_screen(p.lon, p.lat);
                Point::new(x, y)
            });
            stroke_overlay(target, screen, projected, color, weight, w, h);
        }

        // Pass 2 — chevrons, anchored to route distance and windowed around the rider.
        let Some(progress_m) = arrows_at else {
            return;
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
    }

    /// Stroke a single polyline of `(lon, lat)` microdegree points as an overlay, clipped to the
    /// view and stroked with embedded-graphics (see [`stroke_overlay`]) — this is the recorded
    /// **breadcrumb**, whose two tiers (spine, recent) are each one call. Call after
    /// [`render`](MapRenderer::render) so the path sits on the map.
    pub fn stroke_path<D, I>(&mut self, target: &mut D, vp: &Viewport, pts: I, color: D::Color, weight: u32)
    where
        D: DrawTarget,
        I: IntoIterator<Item = (i32, i32)>,
    {
        let (w, h) = (vp.w as i32, vp.h as i32);
        let projected = pts.into_iter().map(|(lon, lat)| {
            let (x, y) = vp.to_screen(lon, lat);
            Point::new(x, y)
        });
        stroke_overlay(target, &mut self.draw.screen, projected, color, weight, w, h);
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
            let r = |x: f32, y: f32| Point::new(libm::roundf(x) as i32, libm::roundf(y) as i32);
            return Some((r(x0, y0), r(x1, y1)));
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

/// Append `c1` to the current run, first subdividing a step longer than 150 px into ≤150 px hops so
/// embedded-graphics' thick-line intersection math stays well-behaved (no overflow in debug,
/// no miter spikes on the MCU in release).
fn push_run(run: &mut Vec<Point, MAX_SCREEN_POINTS>, c1: Point) {
    if let Some(&p1) = run.last() {
        let (dx, dy) = (c1.x - p1.x, c1.y - p1.y);
        let dist = dx.abs().max(dy.abs());
        if dist > 150 {
            let steps = (dist + 149) / 150;
            for s in 1..steps {
                let _ = run.push(Point::new(p1.x + dx * s / steps, p1.y + dy * s / steps));
            }
        }
    }
    let _ = run.push(c1);
}

/// Stroke the accumulated run with embedded-graphics' (properly jointed) thick `Polyline`,
/// then clear it for the next run.
fn flush_run<D>(target: &mut D, run: &mut Vec<Point, MAX_SCREEN_POINTS>, color: D::Color, weight: u32)
where
    D: DrawTarget,
{
    if run.len() >= 2 {
        let _ = Polyline::new(run)
            .into_styled(PrimitiveStyle::with_stroke(color, weight))
            .draw(target);
        // Round joints + caps. eg joins thick segments with a flat **bevel**, so a densely
        // sampled curve renders as a fan of facets — the scalloped "beading" on thick lines.
        // Filling a disc (⌀ = stroke width) at each vertex turns every joint into a smooth arc,
        // keeping full shape detail (no decimation needed). Only thick lines need it (≤2 px don't
        // visibly facet), and the disc at a shared chunk-seam vertex also closes the butt-cap gap
        // between adjacent features.
        if weight > 2 {
            let r = (weight / 2) as i32;
            for p in run.iter() {
                let _ = Circle::new(Point::new(p.x - r, p.y - r), weight)
                    .into_styled(PrimitiveStyle::with_fill(color))
                    .draw(target);
            }
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
#[allow(clippy::too_many_arguments)]
fn stroke_seg<D>(target: &mut D, run: &mut Vec<Point, MAX_SCREEN_POINTS>, a: Point, b: Point, color: D::Color, weight: u32, xmin: f32, ymin: f32, xmax: f32, ymax: f32)
where
    D: DrawTarget,
{
    match clip_segment(a, b, xmin, ymin, xmax, ymax) {
        None => flush_run(target, run, color, weight), // segment wholly off-screen
        Some((c0, c1)) => {
            // (Re)start a run if this segment didn't continue the previous one.
            if run.last().copied() != Some(c0) {
                flush_run(target, run, color, weight);
                let _ = run.push(c0);
            }
            push_run(run, c1);
            // Clipped at its far end → the line left the view here; close this run.
            if c1 != b {
                flush_run(target, run, color, weight);
            }
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

/// Clip a projected overlay polyline to the view and stroke the on-screen runs with
/// embedded-graphics. eg's `Polyline` gives thick lines but rasterises width pixel-by-pixel —
/// ruinous when the route/breadcrumb is ~96% off-screen. Clipping first (Cohen–Sutherland, into
/// the screen grown by the stroke width so an edge-hugging line keeps its full thickness) means
/// eg only ever pays for the visible part: the line where it crosses the view splits into
/// separate runs, each stroked on its own (and round-jointed in [`flush_run`]).
///
/// The points are first **simplified in screen space** ([`simplify`] at [`SIMPLIFY_EPS_PX`]) — a
/// *subpixel* dedup that folds away the integer-projection staircase and same-pixel vertex
/// pile-ups a dense route/road carries when zoomed out. It never moves the line a visible pixel,
/// so no shape is lost (the joint smoothing that kills thick-line beading is [`flush_run`]'s
/// round discs, not this); it just hands eg far fewer segments and discs. The `run` scratch is
/// reused; long runs are subdivided.
fn stroke_overlay<D, I>(target: &mut D, run: &mut Vec<Point, MAX_SCREEN_POINTS>, points: I, color: D::Color, weight: u32, w: i32, h: i32)
where
    D: DrawTarget,
    I: IntoIterator<Item = Point>,
{
    let weight = weight.max(1);
    let m = weight as f32 + 2.0; // clip margin ≥ half-width, so edge strokes still paint in
    let (xmin, ymin, xmax, ymax) = (-m, -m, w as f32 + m, h as f32 + m);
    run.clear();

    // Simplify in screen space, then stroke consecutive kept vertices as clipped segments — their
    // runs join because each segment starts where the previous ended.
    let mut prev: Option<Point> = None;
    simplify(points, SIMPLIFY_EPS_PX, |v| {
        if let Some(a) = prev {
            stroke_seg(target, run, a, v, color, weight, xmin, ymin, xmax, ymax);
        }
        prev = Some(v);
    });
    flush_run(target, run, color, weight);
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
        let (x, y) = vp.to_screen(lon, lat);
        let _ = screen.push(Point::new(x, y));
    }
    fill_polygon(target, screen, ring_lens, color, vp.w as i32, vp.h as i32, xs);
}

/// Project and stroke one map line (its exterior ring). The draw phase's
/// `Kind::Line` arm — factored out as the single point where per-feature line
/// styling (dashes, casing) will branch later (see `docs/rendering_pipeline.md`
/// §9d). Uses the same view-clipped eg stroke as the route/breadcrumb overlays:
/// clipping spares eg the off-screen part of a line whose chunk straddles the
/// view edge (most visible at coarse zoom) while keeping its properly-jointed
/// thick rendering for thicker classes.
fn draw_line<D>(
    target: &mut D,
    vp: &Viewport,
    pts: &[(i32, i32)],
    color: D::Color,
    weight: u32,
    screen: &mut Vec<Point, MAX_SCREEN_POINTS>,
) where
    D: DrawTarget,
{
    let projected = pts.iter().map(|&(lon, lat)| {
        let (x, y) = vp.to_screen(lon, lat);
        Point::new(x, y)
    });
    stroke_overlay(target, screen, projected, color, weight, vp.w as i32, vp.h as i32);
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
    let r = |x: f32, y: f32| Point::new(libm::roundf(x) as i32, libm::roundf(y) as i32);
    let tri = [
        r(c.0 + fx * tip, c.1 + fy * tip),
        r(c.0 - fx * back + rx * half, c.1 - fy * back + ry * half),
        r(c.0 - fx * back - rx * half, c.1 - fy * back - ry * half),
    ];
    fill_polygon(target, &tri, &[3], color, w, h, xs);
}

/// Scanline even-odd polygon fill. `screen` holds every ring's projected points
/// concatenated; `ring_lens` partitions them (exterior first, then holes — holes
/// fall out of the even-odd rule for free). `xs` is a reused crossing buffer.
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
        for &len in ring_lens {
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
                    let _ = xs.push(xi + (yc - yi) / (yj - yi) * (xj - xi));
                }
                j = i;
            }
        }
        if xs.len() < 2 {
            continue;
        }
        xs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
        let mut k = 0;
        while k + 1 < xs.len() {
            // Round spans *outward* (floor left, ceil right) to close hairline
            // background gaps between adjacent fills. A feature clipped across a
            // chunk boundary (`packer/obcm/quadtree.py`) becomes two polygons whose
            // shared edge is clipped independently, so their pixel staircases can
            // disagree by ≤1px (most visible along a rotated diagonal seam).
            // `to_screen`'s round-to-nearest collapses nearly all of it; this ≤1px
            // outward overlap is the cheap insurance (invisible for same-colored
            // fills). See `firmware/docs/render_followups.md` item 2.
            let x0 = (libm::floorf(xs[k]) as i32).max(0);
            let x1 = (libm::ceilf(xs[k + 1]) as i32).min(w - 1);
            if x1 >= x0 {
                let _ = target.fill_solid(
                    &Rectangle::new(Point::new(x0, y), Size::new((x1 - x0 + 1) as u32, 1)),
                    color,
                );
            }
            k += 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{aspect_for_lat, simplify, walk_route_arrows, within_eps};
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
}
