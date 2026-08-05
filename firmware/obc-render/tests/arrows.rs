//! Tests for the route direction-chevron overlay ([`RenderScratch::draw_route`]). Feeds a static
//! route through the [`RouteOverlaySource`] seam (the renderer's whole view of a route — no OBCR
//! bytes involved), strokes it into an in-memory `DrawTarget`, and counts the distinctly-coloured
//! arrow pixels. The arc-length cadence itself is unit-tested inside the crate; this pins the
//! end-to-end `draw_route` gate.

use embedded_graphics::pixelcolor::Rgb888;
use obc_map_scene::{ground_dist_m, BBox};
use obc_render::{OverlayChunk, RenderScratch, RouteOverlaySource, Viewport};

mod common;
use common::Buf;

const ROUTE: Rgb888 = Rgb888::new(255, 0, 255); // magenta stroke
const ARROW: Rgb888 = Rgb888::new(255, 255, 255); // white chevrons

/// A due-north route at a fixed longitude: a constant longitude maps straight down the screen by
/// zoom alone (no aspect skew), so the on-screen length — and thus the chevron count — is easy to
/// reason about. `(lon, lat)` microdegrees, ~334 m end to end.
const NORTHWARD: &[(i32, i32)] = &[(7_800_000, 48_000_000), (7_800_000, 48_001_500), (7_800_000, 48_003_000)];

/// A longer due-north route (~1.8 km) so a full screenful of chevrons fits at more than one
/// zoom — used to check the on-screen spacing is held constant as you zoom (see
/// `chevron_spacing_is_held_in_screen_space`).
const LONG_NORTH: &[(i32, i32)] = &[(7_800_000, 48_000_000), (7_800_000, 48_008_000), (7_800_000, 48_016_000)];

/// The simplest possible [`RouteOverlaySource`]: one chunk holding a static polyline, with the
/// bbox and total ground distance derived from the points. What every host adapter reduces to.
struct StaticRoute(&'static [(i32, i32)]);

impl RouteOverlaySource for StaticRoute {
    fn chunk_count(&self) -> usize {
        1
    }
    fn chunk(&self, _k: usize) -> OverlayChunk {
        let mut bbox = BBox { min_lon: i32::MAX, min_lat: i32::MAX, max_lon: i32::MIN, max_lat: i32::MIN };
        for &(lon, lat) in self.0 {
            bbox.min_lon = bbox.min_lon.min(lon);
            bbox.min_lat = bbox.min_lat.min(lat);
            bbox.max_lon = bbox.max_lon.max(lon);
            bbox.max_lat = bbox.max_lat.max(lat);
        }
        OverlayChunk { bbox, cum_distance_m: 0 }
    }
    fn total_distance_m(&self) -> u32 {
        self.0.windows(2).map(|s| ground_dist_m(s[0], s[1])).sum::<f32>() as u32
    }
    fn visit_points(&self, _k: usize, visit: &mut dyn FnMut(&[(i32, i32)])) {
        visit(self.0);
    }
}

/// North-up viewport centred on the route's midpoint, zoomed so the ~3000 µ° span maps to
/// ~300 px on a 400×400 buffer (well clear of the edges).
fn vp() -> Viewport {
    Viewport::new(400.0, 400.0, 7_800_000, 48_001_500, 0.1)
}

#[test]
fn no_arrows_when_disabled() {
    let mut buf = Buf::new(400, 400);
    RenderScratch::new().draw_route(&mut buf, &vp(), &StaticRoute(NORTHWARD), ROUTE, 6, ARROW, None);

    assert!(buf.count(ROUTE) > 0, "the route line itself should be drawn");
    assert_eq!(buf.count(ARROW), 0, "no chevrons when arrows_at = None");
}

#[test]
fn arrows_drawn_near_the_rider_when_enabled() {
    // Rider ~150 m along the ~334 m route → chevrons cluster around the route midpoint.
    let mut buf = Buf::new(400, 400);
    RenderScratch::new().draw_route(&mut buf, &vp(), &StaticRoute(NORTHWARD), ROUTE, 6, ARROW, Some(150));

    assert!(buf.count(ROUTE) > 0, "the route line should still be drawn under the chevrons");
    assert!(buf.count(ARROW) > 0, "chevrons should be stencilled along the route near the rider");
}

#[test]
fn arrows_are_windowed_to_the_rider() {
    // Chevrons lead *ahead* of the rider but never trail behind — the travelled part of the
    // route carries none. This behind-cutoff is what stops an out-and-back's two passes from
    // colliding (only the leg you're on, the right way round).
    //
    // Route start (progress 0) is at the screen bottom, the ~334 m end at the top. Rider at
    // 250 m → up near the top; chevrons lead the remaining ~84 m (upper screen), and the
    // travelled lower screen is clear.
    let mut buf = Buf::new(400, 400);
    RenderScratch::new().draw_route(&mut buf, &vp(), &StaticRoute(NORTHWARD), ROUTE, 6, ARROW, Some(250));

    let arrows_in = |y0: i32, y1: i32| (y0..y1).any(|y| (0..400).any(|x| buf.get(x, y) == ARROW));
    assert!(arrows_in(40, 130), "chevrons expected ahead of the rider (toward route end, upper screen)");
    assert!(!arrows_in(170, 360), "no chevrons behind the rider (travelled part, lower screen)");
}

#[test]
fn route_stroke_has_the_requested_width() {
    // The fast quad stroke must render the line at ~`weight` px wide (the route is a vertical
    // line at screen x≈200; measure the run of route pixels across a clean row).
    let mut buf = Buf::new(400, 400);
    RenderScratch::new().draw_route(&mut buf, &vp(), &StaticRoute(NORTHWARD), ROUTE, 6, ARROW, None);

    let row = 330; // within the route's on-screen extent, clear of any chevron
    let width = (0..400).filter(|&x| buf.get(x, row) == ROUTE).count();
    assert!((5..=8).contains(&width), "weight-6 stroke should be ~6 px wide, got {width}");
}

/// Centre-to-centre screen gaps (px) between consecutive chevrons down the route column. The
/// route is the vertical `LONG_NORTH` line at screen x≈200; each chevron shows as a short run
/// of white over the magenta stroke, so distinct white runs in the central columns are the
/// chevrons and the gaps between their mid-rows are the on-screen spacing.
fn chevron_gaps(buf: &Buf) -> Vec<i32> {
    let has_white: Vec<bool> = (0..buf.h).map(|y| (197..=203).any(|x| buf.get(x, y) == ARROW)).collect();
    let mut centres = Vec::new();
    let mut y = 0;
    while y < buf.h {
        if has_white[y as usize] {
            let start = y;
            while y < buf.h && has_white[y as usize] {
                y += 1;
            }
            centres.push((start + y - 1) / 2);
        } else {
            y += 1;
        }
    }
    centres.windows(2).map(|w| w[1] - w[0]).collect()
}

#[test]
fn chevron_spacing_is_held_in_screen_space() {
    // Chevron spacing is a fixed *screen* cadence (ARROW_SPACING_PX) converted to ground metres at
    // the current zoom, so the on-screen gap between chevrons stays the same as you zoom. Render the
    // same route at two zooms (2× apart), rider at the start so chevrons fill upward; the measured
    // pixel gaps must match (fixed-metre spacing would scale ~2× with zoom).
    let gaps_at = |zoom: f32| {
        let vp = Viewport::new(400.0, 400.0, 7_800_000, 48_001_500, zoom);
        let mut buf = Buf::new(400, 400);
        RenderScratch::new().draw_route(&mut buf, &vp, &StaticRoute(LONG_NORTH), ROUTE, 11, ARROW, Some(0));
        chevron_gaps(&buf)
    };
    let median = |mut v: Vec<i32>| {
        v.sort_unstable();
        v[v.len() / 2]
    };

    let far = gaps_at(0.05); // zoomed out — more m/px
    let near = gaps_at(0.10); // zoomed in 2×
    assert!(far.len() >= 2 && near.len() >= 2, "need several chevrons to measure spacing (far {far:?}, near {near:?})");

    let (g_far, g_near) = (median(far.clone()), median(near.clone()));
    assert!(
        (g_far - g_near).abs() <= 10,
        "on-screen chevron spacing should be ~constant across zoom, got {g_far} px (far {far:?}) vs {g_near} px (near {near:?})"
    );
}
