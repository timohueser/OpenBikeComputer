//! Investigation harness that set the eg↔span split in [`flush_run`]. Kept so the choice can be
//! re-checked when new line weights or styles land.
//!
//! Reports, per stroke weight: the per-frame cost of stroking a real route (writes + µs), and
//! correctness probes on synthetic lines — horizontal body width, whether a 45° diagonal body is
//! gap-free, and a coverage signature for a screen-long line and a sharp zigzag. To compare the two
//! rasterisers head-to-head, force the `flush_run` branch (set its `weight <= 1` test to `<= 999`
//! for all-eg or `<= 0` for all-span).
//!
//! Findings: eg is cheap only at **1 px** (~35 µs); at 2 px it enters eg's thick-line path and
//! jumps ~8×. The span path is a flat ~27 µs at every width ≥ 2 and ~10× faster there — but can't
//! draw 1 px (a zero-width rectangle has no scanline crossings). So the split sits at 1 px.
//!
//! Lives in `obc-app` (not `obc-render`) because it decodes a real `.obcr` for its route polyline
//! — the renderer itself no longer knows the route format (issue #332).
//!
//!   cargo run -p obc-app --example stroke_study --release -- firmware/routes/kandel.obcr

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
use heapless::Vec as HVec;
use obc_render::{zoom_for_mpp, MapRenderer, Viewport};
use obc_route::{RouteIndex, RoutePoint, RouteReader, SliceSource, MAX_POINTS_PER_CHUNK};
use std::time::Instant;

const W: i32 = 240;
const H: i32 = 320;
const LINE: Rgb888 = Rgb888::new(220, 0, 220);

/// Counts in-bounds pixel writes (perf) — `draw_iter` per pixel, `fill_solid` by area.
struct Counter {
    writes: u64,
}
impl OriginDimensions for Counter {
    fn size(&self) -> Size {
        Size::new(W as u32, H as u32)
    }
}
impl DrawTarget for Counter {
    type Color = Rgb888;
    type Error = core::convert::Infallible;
    fn draw_iter<I: IntoIterator<Item = Pixel<Self::Color>>>(&mut self, px: I) -> Result<(), Self::Error> {
        for Pixel(p, _) in px {
            if (0..W).contains(&p.x) && (0..H).contains(&p.y) {
                self.writes += 1;
            }
        }
        Ok(())
    }
    fn fill_solid(&mut self, area: &Rectangle, _c: Self::Color) -> Result<(), Self::Error> {
        let c = area.intersection(&self.bounding_box());
        if let Some(br) = c.bottom_right() {
            self.writes += (br.y - c.top_left.y + 1) as u64 * (br.x - c.top_left.x + 1) as u64;
        }
        Ok(())
    }
}

/// Records which pixels were painted (correctness).
struct Cov {
    px: Vec<bool>,
}
impl Cov {
    fn new() -> Self {
        Cov { px: vec![false; (W * H) as usize] }
    }
    fn clear(&mut self) {
        self.px.iter_mut().for_each(|b| *b = false);
    }
    fn set(&mut self, x: i32, y: i32) {
        if (0..W).contains(&x) && (0..H).contains(&y) {
            self.px[(y * W + x) as usize] = true;
        }
    }
    fn on(&self, x: i32, y: i32) -> bool {
        (0..W).contains(&x) && (0..H).contains(&y) && self.px[(y * W + x) as usize]
    }
    fn count(&self) -> usize {
        self.px.iter().filter(|b| **b).count()
    }
}
impl OriginDimensions for Cov {
    fn size(&self) -> Size {
        Size::new(W as u32, H as u32)
    }
}
impl DrawTarget for Cov {
    type Color = Rgb888;
    type Error = core::convert::Infallible;
    fn draw_iter<I: IntoIterator<Item = Pixel<Self::Color>>>(&mut self, px: I) -> Result<(), Self::Error> {
        for Pixel(p, _) in px {
            self.set(p.x, p.y);
        }
        Ok(())
    }
    fn fill_solid(&mut self, area: &Rectangle, _c: Self::Color) -> Result<(), Self::Error> {
        let c = area.intersection(&self.bounding_box());
        if let Some(br) = c.bottom_right() {
            for y in c.top_left.y..=br.y {
                for x in c.top_left.x..=br.x {
                    self.set(x, y);
                }
            }
        }
        Ok(())
    }
}

// (lon, lat) µ° → screen (W/2 + lon, H/2 − lat) at zoom 1 on the equator.
fn vp() -> Viewport {
    Viewport::new(W as f32, H as f32, 0, 0, 1.0)
}

/// Painted rows in the column through screen-x = W/2 (mid-span of a horizontal stroke, clear of the
/// end discs) — the body's rendered width.
fn horizontal_body_width(r: &mut MapRenderer, weight: u32, cov: &mut Cov) -> usize {
    cov.clear();
    r.stroke_path(&mut *cov, &vp(), [(-60, 0), (60, 0)], LINE, weight);
    (0..H).filter(|&y| cov.on(W / 2, y)).count()
}

/// Whether a 45° diagonal body (screen y = x through the centre) is painted with no gaps, plus its
/// perpendicular width at the centre (set pixels along the (k, −k) normal).
fn diagonal(r: &mut MapRenderer, weight: u32, cov: &mut Cov) -> (bool, usize) {
    cov.clear();
    // (-60,60)→(60,-60) µ° maps to screen (60,100)→(180,220): the centreline is y = x + 40.
    r.stroke_path(&mut *cov, &vp(), [(-60, 60), (60, -60)], LINE, weight);
    let gapfree = (70..=170).all(|x| cov.on(x, x + 40)); // centreline screen (x, x+40)
    let (cx, cy) = (120, 160); // a point on the centreline
    let width = (-20..=20).filter(|&k| cov.on(cx + k, cy - k)).count(); // along the ⟂ (1,−1)
    (gapfree, width)
}

/// Coverage signature (painted-pixel count) of a screen-long horizontal line and a tight zigzag.
fn stress_signature(r: &mut MapRenderer, weight: u32, cov: &mut Cov) -> (usize, usize) {
    cov.clear();
    r.stroke_path(&mut *cov, &vp(), [(-400, 30), (400, 30)], LINE, weight);
    let long = cov.count();
    cov.clear();
    let zig = [(-60, 0), (-40, 40), (-20, 0), (0, 40), (20, 0), (40, 40), (60, 0)];
    r.stroke_path(&mut *cov, &vp(), zig, LINE, weight);
    (long, cov.count())
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: stroke_study <route.obcr>");
    let bytes = std::fs::read(&path).expect("read route");
    let src = SliceSource(&bytes);
    let idx = RouteIndex::read(&src).expect("parse route");
    let reader = RouteReader::new(&idx, &src);
    let mut pts: Vec<(i32, i32)> = Vec::new();
    let mut scratch = HVec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
    for k in 0..reader.chunks().len() {
        if reader.decode_chunk(k, &mut scratch).is_ok() {
            let skip = if pts.is_empty() { 0 } else { 1 };
            pts.extend(scratch.iter().skip(skip).map(|p| (p.lon, p.lat)));
        }
    }
    let (cam_lon, cam_lat) = pts[pts.len() / 2];
    let route_vp = Viewport::new(W as f32, H as f32, cam_lon, cam_lat, zoom_for_mpp(6.0)); // route fills screen

    let mut r = MapRenderer::new();
    let mut cov = Cov::new();
    println!("route: {} ({} pts), probes at zoom 1 (W={W} H={H})\n", reader.name(), pts.len());
    println!(
        "{:>3}  {:>10} {:>9}   {:>7} {:>9} {:>8}  {:>9} {:>8}",
        "w", "rt_writes", "rt_us", "h_width", "diag_gap", "diag_w", "long_px", "zig_px"
    );
    for weight in 1..=12u32 {
        let mut c = Counter { writes: 0 };
        r.stroke_path(&mut c, &route_vp, pts.iter().copied(), LINE, weight);
        const N: u32 = 2_000;
        let t = Instant::now();
        for _ in 0..N {
            let mut sink = Counter { writes: 0 };
            r.stroke_path(&mut sink, &route_vp, pts.iter().copied(), LINE, weight);
            std::hint::black_box(sink.writes);
        }
        let us = t.elapsed().as_micros() as f64 / N as f64;

        let hw = horizontal_body_width(&mut r, weight, &mut cov);
        let (gapfree, dw) = diagonal(&mut r, weight, &mut cov);
        let (long, zig) = stress_signature(&mut r, weight, &mut cov);
        println!(
            "{weight:>3}  {:>10} {us:>9.2}   {hw:>7} {:>9} {dw:>8}  {long:>9} {zig:>8}",
            c.writes,
            if gapfree { "ok" } else { "GAPS" },
        );
    }
}
