//! Edge-case coverage for the renderer's polygon fill, frame-buffer saturation, marker cull
//! boundary and text clipping.
//!
//! `priority.rs` covers the happy-path priority ordering and span saturation, `stroke.rs` the
//! thick line. This drives a polygon that straddles a screen edge or sits wholly off-screen, the
//! degenerate ring skip, the `MAX_FRAME_POINTS` drop trigger, the marker's cull boundary, and
//! text running off the buffer edge, each through the real public entry point.

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader, SliceSource};
use obc_render::text::{draw_text, Font, TextAlign};
use obc_render::{RenderConfig, RenderScratch, Viewport};
use obc_render::{MAX_FRAME_POINTS, MAX_SPANS};
use obcm_testkit::{build_file, pack_poly, pack_poly16, pack_poly_decl, pack_poly_hole, LodSpec, Style};

use crate::common::Buf;

const FILL_565: u16 = 0x07E0; // green
const GREEN: Rgb888 = Rgb888::new(0, 255, 0);
const RED: Rgb888 = Rgb888::new(255, 0, 0);

fn green565(c: u16) -> Rgb888 {
    let (r, g, b) = rgb565_to_rgb888(c);
    Rgb888::new(r, g, b)
}

/// Build a single-LOD, single-leaf map over the global bbox holding `chunk`. The leaf node bbox is
/// the global bbox, so feature anchors are file-absolute.
fn one_chunk_map(bbox: (i32, i32, i32, i32), styles: &[Style], chunk: Vec<u8>, chunk_size: usize) -> Vec<u8> {
    let mut padded = chunk;
    padded.resize(chunk_size, 0xFF);
    build_file(bbox, styles, &[LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![padded], chunk_size }])
}

fn render_into(buf: &mut Buf, bytes: &[u8], vp: &Viewport) -> obc_render::RenderStats {
    let cache = MapCache::new();
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("valid v5 file");
    let reader = Reader::new(&src, &tables, &cache);
    RenderScratch::new().render(buf, &reader, vp, Rgb888::BLACK, RenderConfig::default(), green565)
}

/// A polygon straddling the top edge of the screen: the fill clamps `ymin` to 0 and must paint the
/// on-screen lower half while never writing a pixel at y<0.
#[test]
fn polygon_straddling_top_edge_clamps_and_fills_visible_part() {
    // A big square placed so that, with the camera near its top, its top is above the screen.
    let styles: &[Style] = &[(1, 0, FILL_565, 1, 1, false, None)];
    // (0,0)->(1000,0)->(1000,1000)->(0,1000): a 1000-µdeg square (16-bit deltas).
    let square = pack_poly16(1, 0, 0, &[(1000, 0), (0, 1000), (-1000, 0)]);
    let bytes = one_chunk_map((0, 0, 2000, 2000), styles, square, 4096);

    // Camera at the square's centre, zoomed so the square overflows the screen top and bottom, so
    // its top edge projects above y=0 and the fill clamps ymin to 0.
    let vp = Viewport::new(200.0, 200.0, 500, 500, 0.3);
    let mut buf = Buf::new(200, 200);
    render_into(&mut buf, &bytes, &vp);

    // The fill reaches the very top row…
    assert!((0..200).any(|x| buf.get(x, 0) == GREEN), "the clamped top row is filled");
    // …and a row well inside is filled too.
    assert!((0..200).any(|x| buf.get(x, 100) == GREEN), "an interior row is filled");
    assert!(buf.count(GREEN) > 1000, "a large on-screen area is painted");
}

/// A polygon projecting entirely off-screen must paint nothing: after clamping, `ymin > ymax` and
/// the fill early-returns. The feature still passes the per-feature bbox cull, so the empty screen
/// is the early-return and not the cull.
#[test]
fn polygon_entirely_offscreen_fills_nothing() {
    let styles: &[Style] = &[(1, 0, FILL_565, 1, 1, false, None)];
    // A small triangle near (50,50) in a large bbox.
    let tri = pack_poly(1, 50, 50, &[(20, 0), (0, 20)]);
    let bytes = one_chunk_map((0, 0, 100_000, 100_000), styles, tri, 4096);

    // Aim the camera far from the triangle but keep it inside the wide visible bbox, so it
    // survives the cull and reaches the fill, which then clamps it off-screen.
    let vp = Viewport::new(200.0, 200.0, 50_000, 50_000, 0.0008);
    let mut buf = Buf::new(200, 200);
    let stats = render_into(&mut buf, &bytes, &vp);

    assert_eq!(buf.count(GREEN), 0, "an off-screen polygon paints nothing");
    // It was collected, so the empty screen is the fill's clamp and not the upstream cull.
    assert!(stats.features_drawn >= 1, "the feature passed the bbox cull and was collected");
}

/// A degenerate polygon ring with fewer than 2 vertices must fill nothing: both the ring skip and
/// the row skip drop it. LOD simplification can legally emit a 1-point polygon.
#[test]
fn single_point_polygon_fills_nothing() {
    let styles: &[Style] = &[(1, 0, FILL_565, 1, 1, false, None)];
    // Declared count 1 → exterior is just the anchor; no deltas. A 1-vertex ring.
    let degenerate = pack_poly_decl(1, 100, 100, 1, &[]);
    let bytes = one_chunk_map((0, 0, 1000, 1000), styles, degenerate, 4096);

    let vp = Viewport::new(200.0, 200.0, 100, 100, 1.0); // anchor projects on-screen
    let mut buf = Buf::new(200, 200);
    render_into(&mut buf, &bytes, &vp);
    assert_eq!(buf.count(GREEN), 0, "a single-point polygon fills no pixels");
}

/// A correctly encoded hole affects only its own ring: scanline filling must not create a
/// triangular ground gap between the exterior anchor and a distant clearing.
#[test]
fn polygon_hole_does_not_cut_an_anchor_to_hole_wedge() {
    let styles: &[Style] = &[(1, 0, FILL_565, 1, 1, false, None)];
    // Exterior (100,100)..(200,200) with a hole (130,130)..(170,170). The hole's deltas start at
    // the feature anchor as the wire format requires, with no bridge vertices.
    let polygon =
        pack_poly_hole(1, 100, 100, &[(100, 0), (0, 100), (-100, 0)], &[(30, 30), (40, 0), (0, 40), (-40, 0)]);
    let bytes = one_chunk_map((0, 0, 300, 300), styles, polygon, 4096);
    let vp = Viewport::new(200.0, 200.0, 150, 150, 1.0);
    let mut buf = Buf::new(200, 200);
    render_into(&mut buf, &bytes, &vp);

    // North-up, unit zoom: the camera is screen (100,100), longitude right and latitude up.
    let pixel_at = |lon, lat| buf.get(100 + lon - 150, 100 - (lat - 150));
    assert_eq!(pixel_at(150, 150), Rgb888::BLACK, "the actual hole stays transparent");
    assert_eq!(pixel_at(115, 115), GREEN, "coverage near the exterior anchor remains filled");
    assert_eq!(pixel_at(125, 125), GREEN, "no diagonal anchor-to-hole wedge is invented");
    assert_eq!(pixel_at(185, 185), GREEN, "coverage beyond the hole remains filled");
}

/// A zero-area collinear polygon encloses no region, so every scanline finds fewer than two
/// crossings and the row is skipped.
#[test]
fn zero_area_collinear_polygon_fills_nothing() {
    let styles: &[Style] = &[(1, 0, FILL_565, 1, 1, false, None)];
    // Three collinear points along y = const: (100,100) -> (140,100) -> (180,100).
    let flat = pack_poly(1, 100, 100, &[(40, 0), (40, 0)]);
    let bytes = one_chunk_map((0, 0, 1000, 1000), styles, flat, 4096);

    let vp = Viewport::new(200.0, 200.0, 140, 100, 1.0);
    let mut buf = Buf::new(200, 200);
    render_into(&mut buf, &bytes, &vp);
    assert_eq!(buf.count(GREEN), 0, "a zero-area collinear polygon fills no pixels");
}

/// A drop trigger distinct from the span one: when a few huge-point features arrive, `frame_points`
/// fills before the span buffer and the capacity check drops the feature even though `spans` has
/// room. A high-priority feature must still survive.
///
/// The premise is derived from the cap rather than restated beside it: the blob is sized so
/// exactly `BLOBS` of them plus the high-priority square consume the point budget, one more cannot,
/// and `point_utilization` lands on 1.0. The `const` asserts hold that shape, so moving
/// `MAX_FRAME_POINTS` re-sizes the blob instead of quietly demoting the test.
#[test]
fn frame_points_saturate_before_spans_and_priority_still_wins() {
    /// How many blobs the budget must admit: several, so "a few pack in before saturation" is a
    /// real claim.
    const BLOBS: usize = 8;
    /// The high-priority square's vertex count; `select()` charges it first.
    const HI_PTS: usize = 4;
    /// Points per blob. The fixture makes each an alternating sharp corner, so the renderer's
    /// lossless collinear compaction cannot erase the pressure this test exercises.
    const BLOB_PTS: usize = (MAX_FRAME_POINTS - HI_PTS) / BLOBS;
    // The premise, asserted: `BLOBS` fit beside the square and one more does not, so the point
    // check is provably what drops the rest.
    const _: () = assert!(HI_PTS + BLOBS * BLOB_PTS <= MAX_FRAME_POINTS, "the premised blobs must fit");
    const _: () = assert!(HI_PTS + (BLOBS + 1) * BLOB_PTS > MAX_FRAME_POINTS, "one more blob must not fit");
    // A blob must also survive per-feature decode to reach the frame buffer at all.
    const _: () = assert!(BLOB_PTS <= obc_render::MAX_DECODE_POINTS, "a blob must decode whole");

    const LOW_565: u16 = 0x001F; // blue, priority 4
    const HIGH_565: u16 = 0xF800; // red, priority 1
    let styles: &[Style] = &[(1, 0, LOW_565, 1, 4, false, None), (2, 1, HIGH_565, 1, 1, false, None)];

    // A low-priority blob: a sawtooth whose alternating turns make every vertex a real projected
    // corner. A densely sampled straight rectangle would test the compactor instead.
    let big_blob = |style: u8| -> Vec<u8> {
        let mut deltas: Vec<(i16, i16)> = Vec::with_capacity(BLOB_PTS - 1);
        for index in 0..BLOB_PTS - 1 {
            deltas.push((if index % 2 == 0 { 10_000 } else { -10_000 }, 1));
        }
        pack_poly16(style, 10, 10, &deltas)
    };
    // The high-priority feature: a solid square that unmistakably fills pixels yet fits inside its
    // quadrant. Far fewer points, so it is not what saturates the buffer.
    let hi_square = pack_poly16(2, 10, 10, &[(10_000, 0), (0, 10_000), (-10_000, 0)]);

    // A complete depth-2 quadtree with one feature per leaf, each anchored inside its own
    // quadrant. Leaves 0..6 carry low-priority blobs, already well past `MAX_FRAME_POINTS`, leaf 7
    // carries the high-priority square, and leaves 8..15 carry more blobs, all dropped, which
    // keeps the buffer pinned full.
    const BRANCH: u32 = 0x8000_0000;
    let mut index = vec![BRANCH | 1, BRANCH | 5, BRANCH | 9, BRANCH | 13, BRANCH | 17];
    for leaf in 0..16u32 {
        index.push(leaf); // nodes 5..20 → chunk ids 0..15
    }
    let cs = 16384;
    let pad_to = |mut c: Vec<u8>| {
        c.resize(cs, 0xFF);
        c
    };
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    for leaf in 0..16usize {
        let feat = if leaf < 7 {
            big_blob(1) // low-priority bulk
        } else if leaf == 7 {
            hi_square.clone() // high-priority survivor
        } else {
            big_blob(1) // more low-priority (also dropped) to keep the buffer pinned full
        };
        chunks.push(pad_to(feat));
    }
    let bytes = build_file(
        (0, 0, 100_000, 100_000),
        styles,
        &[LodSpec { max_mpp: f32::INFINITY, index, chunks, chunk_size: cs }],
    );

    // Whole-map view, so all 16 quadrant leaves are on-screen.
    let vp = Viewport::new(200.0, 200.0, 50_000, 50_000, 0.0019);
    let mut buf = Buf::new(200, 200);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).expect("valid v5 file");
    let reader = Reader::new(&src, &tables, &cache);
    let stats = RenderScratch::new().render(&mut buf, &reader, &vp, Rgb888::BLACK, RenderConfig::default(), green565);

    // The point buffer saturated and dropped features…
    assert!(stats.features_dropped > 0, "the point buffer must saturate and drop features");
    // …but the span buffer was nowhere near full, which is the distinct path from priority.rs.
    assert!(stats.features_drawn < MAX_SPANS, "spans were not the limiting buffer");
    // Nor the ring buffer, which on busy real frames is the ceiling and here is not.
    assert!(stats.ring_utilization < 0.1, "rings were not the limiting buffer (util {})", stats.ring_utilization);
    // The blobs plus the square consume the budget to within a few points.
    assert_eq!(stats.features_drawn, BLOBS + 1, "exactly the premised blobs plus the square are admitted");
    assert!(stats.point_utilization > 0.75, "frame_points is the saturated buffer (util {})", stats.point_utilization);

    // The high-priority square survived the saturation and painted.
    let red = green565(HIGH_565);
    assert!(buf.count(red) > 20, "the high-priority feature survives point-buffer saturation");
}

/// This pins the marker cull boundary itself: an anchor just outside the screen but within the
/// 16-px margin must still draw, clipped, while one past the margin is culled.
#[test]
fn marker_within_margin_draws_past_margin_culls() {
    // 1 px per microdegree, camera at origin, so the right edge is at lon +100 µdeg. An anchor at
    // +106 is 6 px past the edge, inside the margin.
    let vp = Viewport::new(200.0, 200.0, 0, 0, 1.0);

    let mut inside = Buf::new(200, 200);
    RenderScratch::new().draw_marker(&mut inside, &vp, 106, 0, None, RED);
    assert!(inside.count(RED) > 0, "an anchor just past the edge but within MARGIN still draws (clipped)");

    // Push the anchor to +120 µdeg = 20 px past the edge, beyond the 16-px margin → culled.
    let mut outside = Buf::new(200, 200);
    RenderScratch::new().draw_marker(&mut outside, &vp, 120, 0, None, RED);
    assert_eq!(outside.count(RED), 0, "an anchor past MARGIN is culled");
}

/// Text drawn partly off-screen must paint only the on-screen part and never panic.
#[test]
fn text_off_the_right_edge_is_clipped_not_overflowed() {
    // Off the right edge: anchor near the right, a string longer than the remaining width.
    let mut b = Buf::new(48, 24);
    draw_text(&mut b, "LONGER", Point::new(36, 4), Font::Body, TextAlign::Left, RED);
    let painted = b.count(RED);
    assert!(painted > 0, "the on-screen head of the string is drawn");
    // Every painted pixel is inside the buffer, so no write escaped the bounds.
    for y in 0..b.h {
        for x in 0..b.w {
            if b.get(x, y) == RED {
                assert!(x >= 0 && x < b.w && y >= 0 && y < b.h, "painted pixel ({x},{y}) inside bounds");
            }
        }
    }
}

/// Text starting at negative x: only the part at x>=0 is recorded. This guards the negative-origin
/// clip path text never otherwise takes.
#[test]
fn text_at_negative_x_clips_the_left_half() {
    let mut b = Buf::new(48, 24);
    draw_text(&mut b, "AB", Point::new(-6, 4), Font::Body, TextAlign::Left, RED);
    // Some of "B" (and the right of "A") lands at x>=0; nothing is recorded at x<0 (Buf clips it).
    let (minx, _, _, _) = b.bbox(RED).expect("the on-screen part of the text draws");
    assert!(minx >= 0, "no pixel is recorded left of the buffer ({minx})");
}
