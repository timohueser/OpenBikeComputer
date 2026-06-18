//! Tests for the route direction-chevron overlay ([`MapRenderer::draw_route`]'s
//! `draw_arrows` path). Builds a real `.obcr` route (GPX → converter → reader), strokes
//! it into a tiny in-memory `DrawTarget`, and counts the distinctly-coloured arrow pixels
//! — so the chevrons only appear when asked, and ride the line when they do. The arc-length
//! cadence itself is unit-tested against `walk_arrows` inside the crate; this pins the
//! end-to-end `draw_route` gate.

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
use obcm_render::{MapRenderer, Viewport};
use obcm_route::{gpx_to_obcr, ByteSink, Error, RouteReader, SliceSource};

const ROUTE: Rgb888 = Rgb888::new(255, 0, 255); // magenta stroke
const ARROW: Rgb888 = Rgb888::new(255, 255, 255); // white chevrons

/// A `w`×`h` Rgb888 buffer implementing `DrawTarget`, with clipped writes (mirrors the
/// `marker.rs` smoke-test target).
struct Buf {
    w: i32,
    h: i32,
    px: Vec<Rgb888>,
}

impl Buf {
    fn new(w: i32, h: i32) -> Self {
        Buf { w, h, px: vec![Rgb888::BLACK; (w * h) as usize] }
    }
    fn count(&self, c: Rgb888) -> usize {
        self.px.iter().filter(|&&p| p == c).count()
    }
    fn get(&self, x: i32, y: i32) -> Rgb888 {
        self.px[(y * self.w + x) as usize]
    }
    fn put(&mut self, x: i32, y: i32, c: Rgb888) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize] = c;
        }
    }
}

impl OriginDimensions for Buf {
    fn size(&self) -> Size {
        Size::new(self.w as u32, self.h as u32)
    }
}

impl DrawTarget for Buf {
    type Color = Rgb888;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            self.put(p.x, p.y, c);
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let clip = area.intersection(&self.bounding_box());
        if let Some(br) = clip.bottom_right() {
            for y in clip.top_left.y..=br.y {
                for x in clip.top_left.x..=br.x {
                    self.put(x, y, color);
                }
            }
        }
        Ok(())
    }
}

/// A `ByteSink` over a growable `Vec` — the host's "whole file to RAM" backing.
#[derive(Default)]
struct VecSink {
    buf: Vec<u8>,
}

impl ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), Error> {
        self.buf.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), Error> {
        let o = off as usize;
        self.buf[o..o + b.len()].copy_from_slice(b);
        Ok(())
    }
}

/// A due-north route at a fixed longitude: collinear points decimate to the two endpoints,
/// and a constant longitude maps straight down the screen by zoom alone (no aspect skew),
/// so the on-screen length — and thus the chevron count — is easy to reason about.
const NORTHWARD: &str = r#"<?xml version="1.0"?>
<gpx><trk><trkseg>
  <trkpt lat="48.000000" lon="7.800000"><ele>100.0</ele></trkpt>
  <trkpt lat="48.001500" lon="7.800000"><ele>100.0</ele></trkpt>
  <trkpt lat="48.003000" lon="7.800000"><ele>100.0</ele></trkpt>
</trkseg></trk></gpx>"#;

fn route_bytes(gpx: &str) -> Vec<u8> {
    let src = SliceSource(gpx.as_bytes());
    let mut sink = VecSink::default();
    gpx_to_obcr(&src, "test", &mut sink).unwrap();
    sink.buf
}

/// North-up viewport centred on the route's midpoint, zoomed so the ~3000 µ° span maps to
/// ~300 px on a 400×400 buffer (well clear of the edges).
fn vp() -> Viewport {
    Viewport::new(400.0, 400.0, 7_800_000, 48_001_500, 0.1)
}

#[test]
fn no_arrows_when_disabled() {
    let bytes = route_bytes(NORTHWARD);
    let src = SliceSource(&bytes);
    let route = RouteReader::open(&src).unwrap();

    let mut buf = Buf::new(400, 400);
    MapRenderer::new().draw_route(&mut buf, &vp(), &route, ROUTE, 6, ARROW, None);

    assert!(buf.count(ROUTE) > 0, "the route line itself should be drawn");
    assert_eq!(buf.count(ARROW), 0, "no chevrons when arrows_at = None");
}

#[test]
fn arrows_drawn_near_the_rider_when_enabled() {
    let bytes = route_bytes(NORTHWARD);
    let src = SliceSource(&bytes);
    let route = RouteReader::open(&src).unwrap();

    // Rider ~150 m along the ~333 m route → chevrons cluster around the route midpoint.
    let mut buf = Buf::new(400, 400);
    MapRenderer::new().draw_route(&mut buf, &vp(), &route, ROUTE, 6, ARROW, Some(150));

    assert!(buf.count(ROUTE) > 0, "the route line should still be drawn under the chevrons");
    assert!(buf.count(ARROW) > 0, "chevrons should be stencilled along the route near the rider");
}

#[test]
fn arrows_are_windowed_to_the_rider() {
    // Chevrons lead *ahead* of the rider but never trail behind — the travelled part of the
    // route carries none. This behind-cutoff is what stops an out-and-back's two passes from
    // colliding (only the leg you're on, the right way round).
    let bytes = route_bytes(NORTHWARD);
    let src = SliceSource(&bytes);
    let route = RouteReader::open(&src).unwrap();

    // Route start (progress 0) is at the screen bottom, the ~333 m end at the top. Rider at
    // 250 m → up near the top; chevrons lead the remaining ~83 m (upper screen), and the
    // travelled lower screen is clear.
    let mut buf = Buf::new(400, 400);
    MapRenderer::new().draw_route(&mut buf, &vp(), &route, ROUTE, 6, ARROW, Some(250));

    let arrows_in = |y0: i32, y1: i32| (y0..y1).any(|y| (0..400).any(|x| buf.get(x, y) == ARROW));
    assert!(arrows_in(40, 130), "chevrons expected ahead of the rider (toward route end, upper screen)");
    assert!(!arrows_in(170, 360), "no chevrons behind the rider (travelled part, lower screen)");
}

#[test]
fn route_stroke_has_the_requested_width() {
    // The fast quad stroke must render the line at ~`weight` px wide (the route is a vertical
    // line at screen x≈200; measure the run of route pixels across a clean row).
    let bytes = route_bytes(NORTHWARD);
    let src = SliceSource(&bytes);
    let route = RouteReader::open(&src).unwrap();

    let mut buf = Buf::new(400, 400);
    MapRenderer::new().draw_route(&mut buf, &vp(), &route, ROUTE, 6, ARROW, None);

    let row = 330; // below the route's on-screen extent? no — within it, clear of any chevron
    let width = (0..400).filter(|&x| buf.get(x, row) == ROUTE).count();
    assert!((5..=8).contains(&width), "weight-6 stroke should be ~6 px wide, got {width}");
}
