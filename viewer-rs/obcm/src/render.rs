//! Shared map renderer (feature `render`).
//!
//! This is the rendering path that runs **both** in the desktop simulator and
//! on the nRF5340 firmware. It is written generically over `embedded-graphics`'
//! [`DrawTarget`], so the host (an SDL `SimulatorDisplay`) and the device (an
//! LS021B7DD02 driver) share the exact same projection, LOD selection, painter
//! ordering, polygon fill and line drawing. The host shell only owns the window,
//! event loop and color policy.
//!
//! Geometry math uses `libm` so it works unchanged in `no_std`.

use alloc::vec::Vec;

use embedded_graphics::{
    prelude::*,
    primitives::{Polyline, PrimitiveStyle, Rectangle},
};

use crate::{BBox, Kind, Reader};

/// Meters of ground per microdegree of latitude (≈ Earth circumference / 360e6).
const METERS_PER_MICRODEG_LAT: f64 = 0.111_320;

/// Screen projection: microdegrees → pixels, with longitude aspect correction so
/// the map keeps shape away from the equator. `zoom` is pixels per microdegree of
/// latitude; longitude is additionally scaled by `aspect = cos(lat)`.
#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub w: f64,
    pub h: f64,
    pub cam_lon: f64,
    pub cam_lat: f64,
    pub zoom: f64,
    pub aspect: f64,
}

impl Viewport {
    /// Build a viewport centered on `(cam_lon, cam_lat)` (microdegrees) with the
    /// aspect correction computed for that latitude.
    pub fn new(w: f64, h: f64, cam_lon: f64, cam_lat: f64, zoom: f64) -> Self {
        Viewport { w, h, cam_lon, cam_lat, zoom, aspect: aspect_for_lat(cam_lat) }
    }

    /// Recompute the longitude aspect correction for the current camera latitude.
    /// Call after panning north/south so far-apart latitudes stay shaped right.
    pub fn refresh_aspect(&mut self) {
        self.aspect = aspect_for_lat(self.cam_lat);
    }

    #[inline]
    pub fn to_screen(&self, lon: i32, lat: i32) -> (i32, i32) {
        let x = (lon as f64 - self.cam_lon) * self.zoom * self.aspect + self.w / 2.0;
        let y = (self.cam_lat - lat as f64) * self.zoom + self.h / 2.0;
        (x as i32, y as i32)
    }

    #[inline]
    pub fn to_map(&self, x: f64, y: f64) -> (f64, f64) {
        let lon = (x - self.w / 2.0) / (self.zoom * self.aspect) + self.cam_lon;
        let lat = self.cam_lat - (y - self.h / 2.0) / self.zoom;
        (lon, lat)
    }

    /// Bounding box (microdegrees) of the on-screen area, for quadtree culling.
    pub fn visible_bbox(&self) -> BBox {
        let (min_lon, max_lat) = self.to_map(0.0, 0.0);
        let (max_lon, min_lat) = self.to_map(self.w, self.h);
        BBox {
            min_lon: min_lon as i32,
            min_lat: min_lat as i32,
            max_lon: max_lon as i32,
            max_lat: max_lat as i32,
        }
    }

    /// Ground meters per pixel at the current zoom (latitude-based), used to pick
    /// the LOD layer. Independent of display size — a 1024px host and a 240px
    /// panel showing the same ground span pick the same level.
    #[inline]
    pub fn meters_per_pixel(&self) -> f32 {
        (METERS_PER_MICRODEG_LAT / self.zoom) as f32
    }
}

#[inline]
fn aspect_for_lat(cam_lat: f64) -> f64 {
    libm::cos((cam_lat / 1e6).to_radians())
}

/// What a single [`render_map`] call drew.
#[derive(Debug, Clone, Copy)]
pub struct RenderStats {
    pub features: usize,
    pub lod: usize,
}

/// Render the visible map into `target`.
///
/// Selects the LOD for the viewport's meters-per-pixel, clears to `bg`, queries
/// and decodes the visible chunks, orders them by style z-index (painter's
/// algorithm) and draws polygons (even-odd scanline fill) and lines. `color_fn`
/// maps a style's RGB565 to the target's pixel color, letting the host choose
/// true-color vs. device quantization while the device passes its native map.
pub fn render_map<D, F>(
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

    let mut feats: Vec<crate::Feature> = Vec::new();
    for (cid, node) in reader.query(lod, &view) {
        feats.extend(reader.decode_chunk(lod, cid, &node));
    }
    // Painter's order by z-index.
    feats.sort_by_key(|f| reader.style(f.style_id).map(|s| s.z_index).unwrap_or(0));

    let (w, h) = (vp.w as i32, vp.h as i32);
    let mut scratch_xs: Vec<f32> = Vec::new();
    let mut rings: Vec<Vec<(i32, i32)>> = Vec::new();

    for f in &feats {
        let style = match reader.style(f.style_id) {
            Some(s) => s,
            None => continue,
        };
        let color = color_fn(style.color);
        match f.kind {
            Kind::Polygon => {
                rings.clear();
                rings.push(f.exterior.iter().map(|&(lon, lat)| vp.to_screen(lon, lat)).collect());
                for inner in &f.interiors {
                    rings.push(inner.iter().map(|&(lon, lat)| vp.to_screen(lon, lat)).collect());
                }
                fill_polygon(target, &rings, color, w, h, &mut scratch_xs);
            }
            Kind::Line => {
                let pts: Vec<Point> = f
                    .exterior
                    .iter()
                    .map(|&(lon, lat)| {
                        let (x, y) = vp.to_screen(lon, lat);
                        Point::new(x.clamp(-4 * w, 4 * w), y.clamp(-4 * h, 4 * h))
                    })
                    .collect();
                if pts.len() >= 2 {
                    let weight = style.weight.max(1) as u32;
                    let _ = Polyline::new(&pts)
                        .into_styled(PrimitiveStyle::with_stroke(color, weight))
                        .draw(target);
                }
            }
        }
    }

    RenderStats { features: feats.len(), lod }
}

/// Scanline even-odd polygon fill over exterior + interior rings (holes handled
/// naturally by the even-odd rule). `rings` are already-projected screen points.
/// `scratch_xs` is a caller-owned reusable buffer for edge crossings.
fn fill_polygon<D>(
    target: &mut D,
    rings: &[Vec<(i32, i32)>],
    color: D::Color,
    w: i32,
    h: i32,
    scratch_xs: &mut Vec<f32>,
) where
    D: DrawTarget,
{
    let mut ymin = i32::MAX;
    let mut ymax = i32::MIN;
    for ring in rings {
        for &(_, y) in ring {
            ymin = ymin.min(y);
            ymax = ymax.max(y);
        }
    }
    ymin = ymin.max(0);
    ymax = ymax.min(h - 1);
    if ymin > ymax {
        return;
    }
    for y in ymin..=ymax {
        let yc = y as f32 + 0.5;
        scratch_xs.clear();
        for ring in rings {
            let n = ring.len();
            if n < 2 {
                continue;
            }
            let mut j = n - 1;
            for i in 0..n {
                let (xi, yi) = (ring[i].0 as f32, ring[i].1 as f32);
                let (xj, yj) = (ring[j].0 as f32, ring[j].1 as f32);
                if (yi <= yc && yc < yj) || (yj <= yc && yc < yi) {
                    scratch_xs.push(xi + (yc - yi) / (yj - yi) * (xj - xi));
                }
                j = i;
            }
        }
        if scratch_xs.len() < 2 {
            continue;
        }
        scratch_xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut k = 0;
        while k + 1 < scratch_xs.len() {
            let x0 = (libm::ceilf(scratch_xs[k]) as i32).max(0);
            let x1 = (libm::floorf(scratch_xs[k + 1]) as i32).min(w - 1);
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
