//! Headless micro-benchmark for the route **overlay stroke** cost. Decodes a real route once, then
//! strokes it through [`MapRenderer::stroke_path`] into a target that counts on-screen pixel
//! **writes** and times the call, swept across zoom. Decode happens up front, so the timed loop is
//! the pure stroke path (project → simplify → clip → eg thick `Polyline` + the round-joint discs).
//!
//!   cargo run -p obc-render --example overlay_bench --release -- firmware/routes/kandel.obcr

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
use heapless::Vec as HVec;
use obc_render::{zoom_for_mpp, MapRenderer, Viewport};
use obc_route::{RouteIndex, RoutePoint, RouteReader, SliceSource, MAX_POINTS_PER_CHUNK};
use std::time::Instant;

const W: f32 = 240.0; // device LCD (LS021B7DD02): 240×320
const H: f32 = 320.0;
const ROUTE: Rgb888 = Rgb888::new(220, 0, 220);
const WEIGHT: u32 = 11; // ROUTE_WEIGHT from obc-app

/// A `DrawTarget` that records nothing but how many in-bounds pixel writes it was asked for —
/// `draw_iter` (eg's thick `Polyline` + the joint `Circle`s) one per pixel, `fill_solid` by area.
struct Counter {
    w: i32,
    h: i32,
    writes: u64,
}
impl Counter {
    fn fresh() -> Self {
        Counter { w: W as i32, h: H as i32, writes: 0 }
    }
}
impl OriginDimensions for Counter {
    fn size(&self) -> Size {
        Size::new(self.w as u32, self.h as u32)
    }
}
impl DrawTarget for Counter {
    type Color = Rgb888;
    type Error = core::convert::Infallible;
    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, _) in pixels {
            if p.x >= 0 && p.y >= 0 && p.x < self.w && p.y < self.h {
                self.writes += 1;
            }
        }
        Ok(())
    }
    fn fill_solid(&mut self, area: &Rectangle, _c: Self::Color) -> Result<(), Self::Error> {
        let clip = area.intersection(&self.bounding_box());
        if let Some(br) = clip.bottom_right() {
            let rows = (br.y - clip.top_left.y + 1) as u64;
            let cols = (br.x - clip.top_left.x + 1) as u64;
            self.writes += rows * cols;
        }
        Ok(())
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: overlay_bench <route.obcr>");
    let bytes = std::fs::read(&path).expect("read route file");
    let src = SliceSource(&bytes);
    let idx = RouteIndex::read(&src).expect("parse route");
    let reader = RouteReader::new(&idx, &src);

    // Decode every chunk once into one flat (lon, lat) polyline (dropping the shared seam vertex
    // each chunk repeats), so the timed loop strokes geometry without any per-frame decode.
    let mut pts: Vec<(i32, i32)> = Vec::new();
    let mut scratch = HVec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
    for k in 0..reader.chunks().len() {
        if reader.decode_chunk(k, &mut scratch).is_err() {
            continue;
        }
        let skip = if pts.is_empty() { 0 } else { 1 }; // adjacent chunks share a seam vertex
        pts.extend(scratch.iter().skip(skip).map(|p| (p.lon, p.lat)));
    }
    // Center the camera on the route's midpoint vertex so the riding zooms actually show the line.
    let (cam_lon, cam_lat) = pts[pts.len() / 2];

    let mut r = MapRenderer::new();
    println!("route: {}  ({} chunks, {} pts decoded)\n", reader.name(), reader.chunks().len(), pts.len());
    println!("{:>7}  {:>12}  {:>11}", "m/px", "px writes", "us/frame");
    for &mpp in &[1.0f32, 2.0, 4.0, 8.0, 16.0, 32.0] {
        let vp = Viewport::new(W, H, cam_lon, cam_lat, zoom_for_mpp(mpp));
        // One counted frame for the exact write tally…
        let mut c = Counter::fresh();
        r.stroke_path(&mut c, &vp, pts.iter().copied(), ROUTE, WEIGHT);
        // …then a timed sweep over the pure stroke.
        const N: u32 = 2_000;
        let t = Instant::now();
        for _ in 0..N {
            let mut sink = Counter::fresh();
            r.stroke_path(&mut sink, &vp, pts.iter().copied(), ROUTE, WEIGHT);
            std::hint::black_box(sink.writes);
        }
        let us = t.elapsed().as_micros() as f64 / N as f64;
        println!("{:>7.0}  {:>12}  {:>11.2}", mpp, c.writes, us);
    }
}
