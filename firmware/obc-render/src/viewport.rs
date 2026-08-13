//! Screen projection: the [`Viewport`] camera and its zoom/scale helpers.

use embedded_graphics::prelude::*;

use obc_map_scene::BBox;

/// Meters of ground per microdegree of latitude — the renderer's zoom is pixels per
/// microdegree-lat, so this turns zoom into meters-per-pixel. Derived from the shared
/// [`obc_map_scene::M_PER_DEG`] so the on-screen scale tracks the route/packer Earth model.
const METERS_PER_MICRODEG_LAT: f32 = (obc_map_scene::M_PER_DEG / 1_000_000.0) as f32;

/// The zoom (pixels per microdegree of latitude) that yields a given ground **meters-per-pixel** —
/// the inverse of [`mpp_for_zoom`]. Lets callers aim the camera at a real-world scale.
#[inline]
pub fn zoom_for_mpp(mpp: f32) -> f32 {
    METERS_PER_MICRODEG_LAT / mpp
}

/// Ground **meters-per-pixel** at a given zoom — the viewport-free form of
/// [`Viewport::meters_per_pixel`] and the inverse of [`zoom_for_mpp`].
#[inline]
pub fn mpp_for_zoom(zoom: f32) -> f32 {
    METERS_PER_MICRODEG_LAT / zoom
}

/// Screen projection: microdegrees → pixels, with longitude aspect correction (`aspect = cos(lat)`)
/// so the map keeps shape away from the equator. `zoom` is pixels per microdegree of latitude.
///
/// Can rotate the map so a given course points to screen-top ("heading-up" navigation).
/// `course_rad` is that course in radians CW from north; `0` is north-up (plain translate+scale).
/// Rotation is applied about the camera center, after aspect correction.
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
    /// Build a north-up viewport centered on `(cam_lon, cam_lat)` (microdegrees).
    pub fn new(w: f32, h: f32, cam_lon: i32, cam_lat: i32, zoom: f32) -> Self {
        Self::new_rotated(w, h, cam_lon, cam_lat, zoom, 0.0)
    }

    /// Like [`new`](Viewport::new) but rotated so `course_rad` (radians CW from
    /// north) points to screen-top.
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
        // Round to nearest, not truncate: `as i32` truncation is asymmetric around the origin
        // (biases toward screen center) and feeds the chunk-seam staircase divergence (see
        // `fill_polygon`). Round-to-nearest is symmetric and sub-pixel correct.
        let p = round_pt(x, y);
        (p.x, p.y)
    }

    /// [`to_screen`](Viewport::to_screen) as an `embedded-graphics` [`Point`].
    #[inline]
    pub(crate) fn project(&self, lon: i32, lat: i32) -> Point {
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

    /// Whether a map-space bbox can touch the actual rotated panel rectangle.
    ///
    /// The quadtree must be queried with [`visible_bbox`](Self::visible_bbox), the axis-aligned
    /// envelope of the rotated screen. At diagonal headings that envelope includes four large
    /// corner wedges which are not visible. Projection turns a feature bbox into a convex
    /// parallelogram; this separating-axis test rejects only parallelograms disjoint from the
    /// panel, before their vertices consume frame scratch.
    #[inline]
    pub(crate) fn bbox_might_be_visible(&self, bbox: &BBox, margin_px: i32) -> bool {
        let quad = [
            self.to_screen(bbox.min_lon, bbox.min_lat),
            self.to_screen(bbox.min_lon, bbox.max_lat),
            self.to_screen(bbox.max_lon, bbox.max_lat),
            self.to_screen(bbox.max_lon, bbox.min_lat),
        ];
        let screen = [
            (-margin_px, -margin_px),
            (-margin_px, self.h as i32 + margin_px),
            (self.w as i32 + margin_px, self.h as i32 + margin_px),
            (self.w as i32 + margin_px, -margin_px),
        ];
        let e0 = (quad[1].0 as i64 - quad[0].0 as i64, quad[1].1 as i64 - quad[0].1 as i64);
        let e1 = (quad[3].0 as i64 - quad[0].0 as i64, quad[3].1 as i64 - quad[0].1 as i64);
        [(1_i64, 0_i64), (0, 1), (-e0.1, e0.0), (-e1.1, e1.0)]
            .into_iter()
            .all(|axis| projections_overlap(&quad, &screen, axis))
    }

    /// Whether decoded geometry itself can paint the panel. This is the exact second stage after
    /// [`bbox_might_be_visible`](Self::bbox_might_be_visible): clipped coverage faces often have a
    /// bbox that touches a rotated view although every edge and filled pixel lies in an envelope
    /// corner. Rejecting those faces before frame selection preserves every visible vertex while
    /// avoiding false point-budget pressure.
    pub(crate) fn geometry_might_be_visible(
        &self,
        points: &[(i32, i32)],
        ring_lens: &[usize],
        is_polygon: bool,
        margin_px: i32,
    ) -> bool {
        let rect = (-margin_px, -margin_px, self.w as i32 + margin_px, self.h as i32 + margin_px);
        let mut offset = 0usize;
        for &len in ring_lens {
            let ring = &points[offset..offset + len];
            offset += len;
            let Some((&first_map, rest)) = ring.split_first() else {
                continue;
            };
            let first = self.to_screen(first_map.0, first_map.1);
            if point_in_rect(first, rect) {
                return true;
            }
            let mut previous = first;
            for &(lon, lat) in rest {
                let current = self.to_screen(lon, lat);
                if point_in_rect(current, rect) || segment_intersects_rect(previous, current, rect) {
                    return true;
                }
                previous = current;
            }
            if is_polygon && segment_intersects_rect(previous, first, rect) {
                return true;
            }
        }

        // No vertex or edge reaches the panel. A filled polygon can still surround it wholesale;
        // test every panel corner against all rings with the same even-odd rule as rasterization.
        is_polygon
            && [(rect.0, rect.1), (rect.0, rect.3), (rect.2, rect.3), (rect.2, rect.1)]
                .into_iter()
                .any(|corner| point_in_polygon(self, corner, points, ring_lens))
    }

    /// Ground meters per pixel at the current zoom, used to pick the LOD layer. Independent of
    /// display size — a 1024px host and a 240px panel over the same ground span pick the same level.
    #[inline]
    pub fn meters_per_pixel(&self) -> f32 {
        mpp_for_zoom(self.zoom)
    }

    /// Unit screen-space vector pointing to map **north** (for a compass needle). At north-up this
    /// is `(0, -1)`; heading-up rotates it. A +lat step maps to `(-sin_c, -cos_c)` in
    /// [`to_screen`](Viewport::to_screen) before the (irrelevant) scale, already unit length.
    #[inline]
    pub fn north_screen_unit(&self) -> (f32, f32) {
        (-self.sin_c, -self.cos_c)
    }
}

#[inline]
fn projections_overlap(a: &[(i32, i32); 4], b: &[(i32, i32); 4], axis: (i64, i64)) -> bool {
    let project = |points: &[(i32, i32); 4]| {
        let mut low = i64::MAX;
        let mut high = i64::MIN;
        for &(x, y) in points {
            let value = (x as i64).checked_mul(axis.0)?.checked_add((y as i64).checked_mul(axis.1)?)?;
            low = low.min(value);
            high = high.max(value);
        }
        Some((low, high))
    };
    let (Some((a_low, a_high)), Some((b_low, b_high))) = (project(a), project(b)) else {
        return true;
    };
    a_high >= b_low && b_high >= a_low
}

#[inline]
fn point_in_rect(point: (i32, i32), rect: (i32, i32, i32, i32)) -> bool {
    point.0 >= rect.0 && point.0 <= rect.2 && point.1 >= rect.1 && point.1 <= rect.3
}

fn segment_intersects_rect(a: (i32, i32), b: (i32, i32), rect: (i32, i32, i32, i32)) -> bool {
    if a.0.max(b.0) < rect.0 || a.0.min(b.0) > rect.2 || a.1.max(b.1) < rect.1 || a.1.min(b.1) > rect.3 {
        return false;
    }
    if point_in_rect(a, rect) || point_in_rect(b, rect) {
        return true;
    }
    let corners = [(rect.0, rect.1), (rect.0, rect.3), (rect.2, rect.3), (rect.2, rect.1)];
    corners.into_iter().zip(corners.into_iter().cycle().skip(1)).take(4).any(|(c, d)| segments_intersect(a, b, c, d))
}

#[inline]
fn segments_intersect(a: (i32, i32), b: (i32, i32), c: (i32, i32), d: (i32, i32)) -> bool {
    let orient = |p: (i32, i32), q: (i32, i32), r: (i32, i32)| {
        (q.0 as i128 - p.0 as i128) * (r.1 as i128 - p.1 as i128)
            - (q.1 as i128 - p.1 as i128) * (r.0 as i128 - p.0 as i128)
    };
    let (o1, o2, o3, o4) = (orient(a, b, c), orient(a, b, d), orient(c, d, a), orient(c, d, b));
    (o1 == 0 || o2 == 0 || o1.signum() != o2.signum()) && (o3 == 0 || o4 == 0 || o3.signum() != o4.signum())
}

fn point_in_polygon(viewport: &Viewport, point: (i32, i32), points: &[(i32, i32)], ring_lens: &[usize]) -> bool {
    let (px, py) = (point.0 as f64, point.1 as f64);
    let mut inside = false;
    let mut offset = 0usize;
    for &len in ring_lens {
        let ring = &points[offset..offset + len];
        offset += len;
        let Some(&last_map) = ring.last() else {
            continue;
        };
        let mut previous = viewport.to_screen(last_map.0, last_map.1);
        for &(lon, lat) in ring {
            let current = viewport.to_screen(lon, lat);
            let (xi, yi, xj, yj) = (current.0 as f64, current.1 as f64, previous.0 as f64, previous.1 as f64);
            if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
                inside = !inside;
            }
            previous = current;
        }
    }
    inside
}

/// The longitude-compression factor at a latitude — `cos(lat)` in the shared local-equirectangular
/// Earth model. The projection and the packer's ground-distance math must agree to the last bit, so
/// there is one implementation: [`obc_map_scene::cos_lat`].
pub(crate) use obc_map_scene::cos_lat as aspect_for_lat;

/// Round to nearest, half away from zero — the shared rounding convention for every
/// screen-space vertex. Same result as `libm::roundf` for all in-screen magnitudes,
/// without the soft-float call on the hot per-vertex path.
#[inline]
pub fn round_coord(v: f32) -> i32 {
    (v + if v >= 0.0 { 0.5 } else { -0.5 }) as i32
}

/// Round sub-pixel `(x, y)` to the nearest integer-pixel [`Point`] — [`round_coord`] on both axes.
#[inline]
pub(crate) fn round_pt(x: f32, y: f32) -> Point {
    Point::new(round_coord(x), round_coord(y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotated_screen_rejects_only_the_envelope_corner() {
        let viewport = Viewport::new_rotated(200.0, 200.0, 0, 0, 1.0, core::f32::consts::FRAC_PI_4);
        let envelope = viewport.visible_bbox();
        let corner = BBox { min_lon: 125, min_lat: 125, max_lon: 135, max_lat: 135 };
        assert!(corner.intersects(&envelope));
        assert!(!viewport.bbox_might_be_visible(&corner, 0));
        assert!(viewport.bbox_might_be_visible(&BBox { min_lon: -5, min_lat: -5, max_lon: 5, max_lat: 5 }, 0));
        assert!(viewport.bbox_might_be_visible(&BBox { min_lon: -500, min_lat: -500, max_lon: 500, max_lat: 500 }, 0));
    }

    #[test]
    fn margin_keeps_a_stroke_reaching_the_panel() {
        let viewport = Viewport::new(200.0, 200.0, 0, 0, 1.0);
        let just_left = BBox { min_lon: -106, min_lat: -5, max_lon: -104, max_lat: 5 };
        assert!(!viewport.bbox_might_be_visible(&just_left, 0));
        assert!(viewport.bbox_might_be_visible(&just_left, 12));
    }

    #[test]
    fn geometry_rejects_a_bbox_false_positive_but_keeps_a_surrounding_fill() {
        let viewport = Viewport::new(200.0, 200.0, 0, 0, 1.0);
        let outside_triangle = [(-120, 120), (-90, 120), (-120, 90)];
        assert!(viewport.bbox_might_be_visible(&BBox { min_lon: -120, min_lat: 90, max_lon: -90, max_lat: 120 }, 0));
        assert!(!viewport.geometry_might_be_visible(&outside_triangle, &[3], true, 0));

        let surrounds_panel = [(-200, -200), (200, -200), (200, 200), (-200, 200)];
        assert!(viewport.geometry_might_be_visible(&surrounds_panel, &[4], true, 0));
    }
}
