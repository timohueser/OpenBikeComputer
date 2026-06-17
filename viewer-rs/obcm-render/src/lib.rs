//! Shared map renderer (feature `render`).
//!
//! This is the rendering path that runs **both** in the desktop simulator and
//! on the nRF5340 firmware. It is written generically over `embedded-graphics`'
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

use obcm_reader::{BBox, Kind, Reader};

/// Meters of ground per microdegree of latitude (≈ Earth circumference / 360e6).
const METERS_PER_MICRODEG_LAT: f32 = 0.111_320;

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
        (x as i32, y as i32)
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
        METERS_PER_MICRODEG_LAT / self.zoom
    }
}

#[inline]
fn aspect_for_lat(cam_lat: i32) -> f32 {
    libm::cosf((cam_lat as f32 / 1e6).to_radians())
}

/// What a single render call drew.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderStats {
    pub lod: usize,
    pub features_tried: usize,
    pub features_drawn: usize,
    pub points_tried: usize,
    pub points_drawn: usize,
}

/// One visible feature's draw metadata plus the ranges locating its geometry in
/// the renderer's frame buffers. Cheap to sort for the painter's algorithm.
struct Span {
    kind: Kind,
    z: i8,
    weight: u8,
    color: u16,
    pt_start: usize,
    ring_start: usize,
    ring_count: usize,
    seq: u16,
}

/// Reusable renderer holding every scratch buffer. Construct once (the firmware
/// keeps a single instance) and call [`MapRenderer::render`] per frame; the
/// buffers are cleared and reused, so no per-frame allocation.
#[derive(Default)]
pub struct MapRenderer {
    // Per-feature decode scratch handed to `Reader::for_each_feature`.
    dec_points: Vec<(i32, i32), 2048>,
    dec_ring_lens: Vec<usize, 32>,
    // Visible chunks for this frame.
    chunks: Vec<(u32, BBox), 64>,
    // All visible features' geometry, concatenated, plus per-feature spans.
    frame_points: Vec<(i32, i32), 8192>,
    frame_ring_lens: Vec<usize, 2048>,
    spans: Vec<Span, 512>,
    // Drawing scratch.
    screen: Vec<Point, 2048>,
    xs: Vec<f32, 128>,
}

impl MapRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Render the visible map into `target`.
    ///
    /// Selects the LOD for the viewport's meters-per-pixel, clears to `bg`,
    /// streams the visible chunks' features into reused buffers, orders them by
    /// style z-index (painter's algorithm) and draws polygons (even-odd scanline
    /// fill) and lines. `color_fn` maps a style's RGB565 to the target's pixel
    /// color, letting the host choose true-color vs. device quantization while
    /// the device passes its native map.
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

        // --- Collect phase: stream visible features into the frame buffers. ---
        // Split borrows so the decode callback can fill `frame_*`/`spans` while
        // `for_each_feature` borrows the decode scratch.
        let Self {
            dec_points,
            dec_ring_lens,
            chunks,
            frame_points,
            frame_ring_lens,
            spans,
            screen,
            xs,
        } = self;

        frame_points.clear();
        frame_ring_lens.clear();
        spans.clear();

        reader.query_into(lod, &view, chunks);
        for &(cid, node) in chunks.iter() {
            reader.for_each_feature(lod, cid, &node, dec_points, dec_ring_lens, |f| {
                let style = match reader.style(f.style_id) {
                    Some(s) => s,
                    None => return,
                };
                let pts = f.points();
                let lens = f.ring_lens();

                stats.features_tried += 1;
                stats.points_tried += pts.len();

                if pts.is_empty() {
                    return;
                }
                let mut min_lon = pts[0].0;
                let mut max_lon = pts[0].0;
                let mut min_lat = pts[0].1;
                let mut max_lat = pts[0].1;
                for &(lon, lat) in pts.iter().skip(1) {
                    min_lon = min_lon.min(lon);
                    max_lon = max_lon.max(lon);
                    min_lat = min_lat.min(lat);
                    max_lat = max_lat.max(lat);
                }
                let feat_bbox = BBox { min_lon, min_lat, max_lon, max_lat };
                if !feat_bbox.intersects(&view) {
                    return;
                }

                if spans.is_full()
                    || frame_points.capacity() - frame_points.len() < pts.len()
                    || frame_ring_lens.capacity() - frame_ring_lens.len() < lens.len()
                {
                    return;
                }

                stats.features_drawn += 1;
                stats.points_drawn += pts.len();

                let _ = spans.push(Span {
                    kind: f.kind,
                    z: style.z_index,
                    weight: style.weight,
                    color: style.color,
                    pt_start: frame_points.len(),
                    ring_start: frame_ring_lens.len(),
                    ring_count: lens.len(),
                    seq: spans.len() as u16,
                });
                let _ = frame_points.extend_from_slice(pts);
                let _ = frame_ring_lens.extend_from_slice(lens);
            });
        }

        // Painter's order by z-index. Using sort_unstable with a sequence number for stable tie-breaking without alloc.
        spans.sort_unstable_by_key(|s| (s.z, s.seq));

        // --- Draw phase. ---
        let (w, h) = (vp.w as i32, vp.h as i32);
        for span in spans.iter() {
            let ring_lens = &frame_ring_lens[span.ring_start..span.ring_start + span.ring_count];
            let total: usize = ring_lens.iter().sum();
            let pts = &frame_points[span.pt_start..span.pt_start + total];
            let color = color_fn(span.color);

            match span.kind {
                Kind::Polygon => {
                    screen.clear();
                    for &(lon, lat) in pts {
                        let (x, y) = vp.to_screen(lon, lat);
                        let _ = screen.push(Point::new(x, y));
                    }
                    fill_polygon(target, screen, ring_lens, color, w, h, xs);
                }
                Kind::Line => {
                    // Lines use only the exterior ring.
                    let n = ring_lens.first().copied().unwrap_or(0);
                    screen.clear();
                    let mut prev_pt: Option<Point> = None;
                    for &(lon, lat) in &pts[..n] {
                        let (x, y) = vp.to_screen(lon, lat);
                        let pt = Point::new(x.clamp(-4 * w, 4 * w), y.clamp(-4 * h, 4 * h));
                        if let Some(p1) = prev_pt {
                            let dx = pt.x - p1.x;
                            let dy = pt.y - p1.y;
                            let dist = dx.abs().max(dy.abs());
                            // Subdivide segments with deltas > 150 pixels.
                            // This prevents `embedded-graphics`'s line intersection logic from
                            // overflowing and panicking on `denominator.pow(2)` in debug builds,
                            // and avoids rendering glitches (miter spikes) on the MCU in release builds.
                            if dist > 150 {
                                let steps = (dist + 149) / 150;
                                for i in 1..steps {
                                    let sx = p1.x + dx * i / steps;
                                    let sy = p1.y + dy * i / steps;
                                    let _ = screen.push(Point::new(sx, sy));
                                }
                            }
                        }
                        let _ = screen.push(pt);
                        prev_pt = Some(pt);
                    }
                    if screen.len() >= 2 {
                        let weight = span.weight.max(1) as u32;
                        let _ = Polyline::new(screen)
                            .into_styled(PrimitiveStyle::with_stroke(color, weight))
                            .draw(target);
                    }
                }
            }
        }

        stats
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
        let pt = |x: f32, y: f32| Point::new(libm::roundf(x) as i32, libm::roundf(y) as i32);

        self.screen.clear();
        let ring_len = match forward {
            // Chevron: a tip a bit ahead and two base corners swept back and out.
            Some((fx, fy)) => {
                let (rx, ry) = (-fy, fx); // right perpendicular
                const TIP: f32 = 9.0;
                const BACK: f32 = 5.0;
                const HALF: f32 = 6.0;
                let _ = self.screen.push(pt(cx + fx * TIP, cy + fy * TIP));
                let _ = self.screen.push(pt(cx - fx * BACK + rx * HALF, cy - fy * BACK + ry * HALF));
                let _ = self.screen.push(pt(cx - fx * BACK - rx * HALF, cy - fy * BACK - ry * HALF));
                3
            }
            // Stationary glyph: a small orientation-free diamond.
            None => {
                const R: f32 = 5.0;
                let _ = self.screen.push(pt(cx, cy - R));
                let _ = self.screen.push(pt(cx + R, cy));
                let _ = self.screen.push(pt(cx, cy + R));
                let _ = self.screen.push(pt(cx - R, cy));
                4
            }
        };
        fill_polygon(target, &self.screen, &[ring_len], color, w, h, &mut self.xs);
    }
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
    xs: &mut Vec<f32, 128>,
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
            let x0 = (libm::ceilf(xs[k]) as i32).max(0);
            let x1 = (libm::floorf(xs[k + 1]) as i32).min(w - 1);
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
