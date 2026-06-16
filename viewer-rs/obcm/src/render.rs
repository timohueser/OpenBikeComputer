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

/// What a single render call drew.
#[derive(Debug, Clone, Copy)]
pub struct RenderStats {
    pub features: usize,
    pub lod: usize,
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
}

/// Reusable renderer holding every scratch buffer. Construct once (the firmware
/// keeps a single instance) and call [`MapRenderer::render`] per frame; the
/// buffers are cleared and reused, so no per-frame allocation.
#[derive(Default)]
pub struct MapRenderer {
    // Per-feature decode scratch handed to `Reader::for_each_feature`.
    dec_points: Vec<(i32, i32)>,
    dec_ring_lens: Vec<usize>,
    // Visible chunks for this frame.
    chunks: Vec<(u32, BBox)>,
    // All visible features' geometry, concatenated, plus per-feature spans.
    frame_points: Vec<(i32, i32)>,
    frame_ring_lens: Vec<usize>,
    spans: Vec<Span>,
    // Drawing scratch.
    screen: Vec<Point>,
    xs: Vec<f32>,
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
                spans.push(Span {
                    kind: f.kind,
                    z: style.z_index,
                    weight: style.weight,
                    color: style.color,
                    pt_start: frame_points.len(),
                    ring_start: frame_ring_lens.len(),
                    ring_count: lens.len(),
                });
                frame_points.extend_from_slice(pts);
                frame_ring_lens.extend_from_slice(lens);
            });
        }

        // Painter's order by z-index (stable: preserves chunk/decode order ties).
        spans.sort_by_key(|s| s.z);

        // --- Draw phase. ---
        let (w, h) = (vp.w as i32, vp.h as i32);
        let count = spans.len();
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
                        screen.push(Point::new(x, y));
                    }
                    fill_polygon(target, screen, ring_lens, color, w, h, xs);
                }
                Kind::Line => {
                    // Lines use only the exterior ring.
                    let n = ring_lens.first().copied().unwrap_or(0);
                    screen.clear();
                    for &(lon, lat) in &pts[..n] {
                        let (x, y) = vp.to_screen(lon, lat);
                        screen.push(Point::new(x.clamp(-4 * w, 4 * w), y.clamp(-4 * h, 4 * h)));
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

        RenderStats { features: count, lod }
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
    xs: &mut Vec<f32>,
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
                    xs.push(xi + (yc - yi) / (yj - yi) * (xj - xi));
                }
                j = i;
            }
        }
        if xs.len() < 2 {
            continue;
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
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
