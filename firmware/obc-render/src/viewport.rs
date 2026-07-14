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

    /// Conservative screen-space broad-phase overlap test for a map-space bbox.
    ///
    /// Projects all four map-bbox corners through the **same** [`to_screen`](Viewport::to_screen)
    /// path drawing uses, takes the integer AABB of the four projected corners, and returns `false`
    /// only when that AABB lies wholly outside the display rectangle (inclusive pixels `0..=w-1` ×
    /// `0..=h-1`) expanded by `margin_px` on every side.
    ///
    /// For one frame the projection is affine (aspect scale → rotation → scale → translate), so the
    /// projected bbox is a parallelogram fully contained by the AABB of its projected corners. An
    /// AABB miss therefore guarantees the feature paints no pixel inside the margin-expanded screen
    /// — a **safe reject**. This is a broad phase, not polygon clipping: false positives (a
    /// corner-AABB that overlaps while the true parallelogram does not) are allowed and harmless;
    /// false negatives are not, so `margin_px` must cover the widest ink a feature can extend past
    /// its centerline bbox (see [`BASE_MAP_INK_MARGIN_PX`](crate::BASE_MAP_INK_MARGIN_PX)).
    ///
    /// This is the renderer-owned second cull, applied only after the source's coarse map-space
    /// quadtree walk (over [`visible_bbox`](Viewport::visible_bbox)) and the per-feature map-space
    /// AABB test have already passed — heading-up, that enclosing AABB has large empty corners this
    /// test rejects.
    pub fn bbox_may_touch_screen(&self, bbox: &BBox, margin_px: i32) -> bool {
        let corners = [
            self.to_screen(bbox.min_lon, bbox.min_lat),
            self.to_screen(bbox.min_lon, bbox.max_lat),
            self.to_screen(bbox.max_lon, bbox.min_lat),
            self.to_screen(bbox.max_lon, bbox.max_lat),
        ];
        let mut min_x = i32::MAX;
        let mut max_x = i32::MIN;
        let mut min_y = i32::MAX;
        let mut max_y = i32::MIN;
        for (x, y) in corners {
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
        // Inclusive display rect [0, w-1] × [0, h-1] expanded by `margin_px` on each side, with
        // saturating arithmetic so an absurd margin or a far-off-screen projected corner can't wrap.
        let left = margin_px.saturating_neg();
        let top = margin_px.saturating_neg();
        let right = (self.w as i32).saturating_sub(1).saturating_add(margin_px);
        let bottom = (self.h as i32).saturating_sub(1).saturating_add(margin_px);
        // Overlap iff the projected-corner AABB meets the expanded rect on both axes.
        !(max_x < left || min_x > right || max_y < top || min_y > bottom)
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
pub(crate) fn aspect_for_lat(cam_lat: i32) -> f32 {
    libm::cosf((cam_lat as f32 / 1e6).to_radians())
}

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
