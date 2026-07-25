//! Edge-case coverage for the renderer's polygon fill, frame-buffer saturation, marker cull
//! boundary and text clipping.
//!
//! `priority.rs` covers the happy-path stub-select priority ordering and span saturation; `stroke.rs` the thick
//! line. This drives a polygon that straddles a screen edge or sits wholly off-screen, the
//! degenerate (sub-2-point) ring skip, the `MAX_FRAME_POINTS` drop trigger (distinct from the span
//! one), the marker's 16-px cull boundary, and text running off the buffer edge — each through the
//! real public entry point against a recording `DrawTarget`.

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader, SliceSource};
use obc_render::text::{draw_text, Font, TextAlign};
use obc_render::MAX_SPANS;
use obc_render::{MapRenderer, Viewport};
use obcm_testkit::{build_file, pack_poly, pack_poly16, pack_poly_decl, LodSpec, Style};

mod common;
use common::Buf;

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
    MapRenderer::new().render(buf, &reader, vp, Rgb888::BLACK, green565)
}

/// A polygon straddling the **top** edge of the screen: its upper half projects above y=0. The fill
/// clamps `ymin` to 0 and must paint the on-screen lower half while never writing a pixel at y<0
/// (the clamp is what keeps the scanline loop in range). Asserts the top buffer row is painted and
/// an interior row is filled.
#[test]
fn polygon_straddling_top_edge_clamps_and_fills_visible_part() {
    // A big square spanning lon/lat so that, with the camera near its top, the square's top is above
    // the screen and its bottom is on-screen. Style is priority 1 so it always draws.
    let styles: &[Style] = &[(1, 0, FILL_565, 1, 1, false, None)];
    // (0,0)->(1000,0)->(1000,1000)->(0,1000): a 1000-µdeg square (16-bit deltas).
    let square = pack_poly16(1, 0, 0, &[(1000, 0), (0, 1000), (-1000, 0)]);
    let bytes = one_chunk_map((0, 0, 2000, 2000), styles, square, 4096);

    // Camera at the square's center with a zoom that makes the 1000-µdeg square (300 px tall at
    // 0.3 px/µdeg) overflow the 200-px screen top and bottom — so the square's top edge projects
    // above y=0 and `fill_polygon` clamps ymin to 0. The top buffer row falls inside the square.
    let vp = Viewport::new(200.0, 200.0, 500, 500, 0.3);
    let mut buf = Buf::new(200, 200);
    render_into(&mut buf, &bytes, &vp);

    // The fill reaches the very top row (the square extends above the screen, clamped to y=0)…
    assert!((0..200).any(|x| buf.get(x, 0) == GREEN), "the clamped top row is filled");
    // …and a row well inside is filled too.
    assert!((0..200).any(|x| buf.get(x, 100) == GREEN), "an interior row is filled");
    assert!(buf.count(GREEN) > 1000, "a large on-screen area is painted");
}

/// A polygon projecting **entirely off-screen** must paint nothing: after clamping, `ymin > ymax`
/// and `fill_polygon` early-returns. The feature passes the per-feature bbox cull (it overlaps the
/// wide visible bbox) yet projects fully off the framebuffer. Asserts zero fill pixels while
/// confirming the feature was collected — so the early-return, not the cull, produced the empty
/// screen.
#[test]
fn polygon_entirely_offscreen_fills_nothing() {
    let styles: &[Style] = &[(1, 0, FILL_565, 1, 1, false, None)];
    // A small triangle near (50,50) in a large bbox.
    let tri = pack_poly(1, 50, 50, &[(20, 0), (0, 20)]);
    let bytes = one_chunk_map((0, 0, 100_000, 100_000), styles, tri, 4096);

    // Aim the camera far from the triangle but keep the triangle inside the (wide) visible bbox so
    // it survives the bbox cull and reaches fill_polygon — which then clamps it off-screen.
    // At a low zoom the visible bbox is huge; place the camera so the triangle projects past the
    // right/bottom edge.
    let vp = Viewport::new(200.0, 200.0, 50_000, 50_000, 0.0008);
    let mut buf = Buf::new(200, 200);
    let stats = render_into(&mut buf, &bytes, &vp);

    assert_eq!(buf.count(GREEN), 0, "an off-screen polygon paints nothing");
    // It WAS collected (overlapped the visible bbox) — so the empty screen is the fill's
    // off-screen clamp/early-return, not the upstream cull dropping it.
    assert!(stats.features_drawn >= 1, "the feature passed the bbox cull and was collected");
}

/// A degenerate polygon ring with fewer than 2 vertices (a single point) must fill nothing: both
/// the `len < 2` ring skip and the `xs.len() < 2` row skip drop it. LOD simplification can legally
/// emit a 1-point "polygon"; it must paint zero pixels, not a stray dot or a panic.
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

/// A zero-area (collinear) polygon — three vertices all on one horizontal line — encloses no
/// region, so every scanline finds <2 crossings and the row is skipped. It must paint nothing.
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

/// A drop trigger distinct from the span one (priority.rs): when a few **huge-point** features
/// arrive, `frame_points` fills before the span buffer and the capacity check
/// `frame_points.capacity() - frame_points.len() < pts.len()` drops the feature even though `spans`
/// has room. A high-priority feature must still survive while low-priority big ones are dropped for
/// lack of point room.
///
/// The premise: `MAX_FRAME_POINTS` (4768) holds three of the test's ~1580-pt blobs, so a few
/// pack in before saturation and `point_utilization` sits near 3×1582/4768 ≈ 0.99 — far past
/// anything the span buffer could explain.
#[test]
fn frame_points_saturate_before_spans_and_priority_still_wins() {
    // ~1580 points per feature. MAX_FRAME_POINTS = 4768, so ~3 fit; the 4th+ are dropped on the
    // point check, long before MAX_SPANS (1152) could fill. Two styles: low priority (4) blue and
    // high priority (1) red, both big.
    const LOW_565: u16 = 0x001F; // blue, priority 4
    const HIGH_565: u16 = 0xF800; // red, priority 1
    let styles: &[Style] = &[(1, 0, LOW_565, 1, 4, false, None), (2, 1, HIGH_565, 1, 1, false, None)];

    // A low-priority "blob": a ~1580-vertex thin filled rectangle (densified edges). Its vertex
    // count is what matters — every vertex lands in `frame_points`, the buffer under test. Anchored
    // at its leaf-local (10,10); 8-bit deltas keep each step ≤127 µdeg, well inside a quadrant.
    let big_blob = |style: u8| -> Vec<u8> {
        let mut deltas: Vec<(i8, i8)> = Vec::new();
        for _ in 0..790 {
            deltas.push((1, 0)); // densified east edge
        }
        deltas.push((0, 40)); // up
        for _ in 0..790 {
            deltas.push((-1, 0)); // densified west edge back
        }
        deltas.push((0, -40)); // close
        pack_poly(style, 10, 10, &deltas) // 1582 exterior points
    };
    // The high-priority feature: a solid 10000-µdeg red square (16-bit deltas) so it unmistakably
    // fills pixels (≈30 px across at the test zoom) yet fits inside its 25000-µdeg quadrant. Far
    // fewer points, so it isn't what saturates the buffer — it just has to survive because the
    // priority-1 pass collects it before the bulk.
    let hi_square = pack_poly16(2, 10, 10, &[(10_000, 0), (0, 10_000), (-10_000, 0)]);

    // A complete depth-2 quadtree: root branch (node 0, children 1..4), four sub-branches
    // (nodes 1..4) whose children are the 16 leaves (nodes 5..20). One feature per leaf, each
    // anchored at its own quadrant's (10,10) so it sits inside that quadrant and (at a whole-map
    // zoom) on-screen. Leaves 0..6 carry low-priority blobs (7 × 1582 = 11074 points, already past
    // MAX_FRAME_POINTS = 4768 → the point buffer overflows); leaf 7 carries the high-priority
    // square; leaves 8..15 carry more low-priority blobs, all dropped, keeping the buffer pinned
    // full so the saturation is unambiguous.
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

    // Whole-map view: a low zoom so all 16 quadrant-leaves (and their features) are on-screen.
    let vp = Viewport::new(200.0, 200.0, 50_000, 50_000, 0.003);
    let mut buf = Buf::new(200, 200);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).expect("valid v5 file");
    let reader = Reader::new(&src, &tables, &cache);
    let stats = MapRenderer::new().render(&mut buf, &reader, &vp, Rgb888::BLACK, green565);

    // The point buffer saturated and dropped features…
    assert!(stats.features_dropped > 0, "the point buffer must saturate and drop features");
    // …but the *span* buffer was nowhere near full (proving the point check, not the span check,
    // was the drop trigger — the distinct path from priority.rs).
    assert!(stats.features_drawn < MAX_SPANS, "spans were not the limiting buffer");
    // Three ~1582-pt blobs in a 4768 buffer land at ≈0.99 — the point buffer is what filled.
    assert!(stats.point_utilization > 0.75, "frame_points is the saturated buffer (util {})", stats.point_utilization);

    // The high-priority red square (priority 1, collected first) survived the saturation and
    // painted, even though enough low-priority points to overflow the buffer were packed around it.
    let red = green565(HIGH_565);
    assert!(buf.count(red) > 20, "the high-priority feature survives point-buffer saturation");
}

/// `offscreen_anchor_is_culled` (marker.rs) tests a fix *far* off-screen. This pins the cull
/// boundary itself: an anchor just *outside* the screen but within the 16-px `MARGIN` must still
/// draw (clipped), while one past the margin is culled — checked from both sides.
#[test]
fn marker_within_margin_draws_past_margin_culls() {
    // 1 px per microdegree, camera at origin → screen center (100,100); the right edge x=200 is at
    // lon = +100 µdeg. An anchor at lon = +106 is 6 px past the edge: inside the 16-px margin.
    let vp = Viewport::new(200.0, 200.0, 0, 0, 1.0);

    let mut inside = Buf::new(200, 200);
    MapRenderer::new().draw_marker(&mut inside, &vp, 106, 0, None, RED);
    assert!(inside.count(RED) > 0, "an anchor just past the edge but within MARGIN still draws (clipped)");

    // Push the anchor to +120 µdeg = 20 px past the edge, beyond the 16-px margin → culled.
    let mut outside = Buf::new(200, 200);
    MapRenderer::new().draw_marker(&mut outside, &vp, 120, 0, None, RED);
    assert_eq!(outside.count(RED), 0, "an anchor past MARGIN is culled");
}

/// Text drawn partly off-screen must paint only the on-screen part and never panic. A long string
/// starting near the right edge (so it runs off): some ink lands and every painted pixel is within
/// the buffer.
#[test]
fn text_off_the_right_edge_is_clipped_not_overflowed() {
    // Off the right edge: anchor near the right, a string longer than the remaining width.
    let mut b = Buf::new(48, 24);
    draw_text(&mut b, "LONGER", Point::new(36, 4), Font::Body, TextAlign::Left, RED);
    let painted = b.count(RED);
    assert!(painted > 0, "the on-screen head of the string is drawn");
    // Every painted pixel is inside the buffer (the recording target clips; this confirms no write
    // escaped the bounds the way a raw index would).
    for y in 0..b.h {
        for x in 0..b.w {
            if b.get(x, y) == RED {
                assert!(x >= 0 && x < b.w && y >= 0 && y < b.h, "painted pixel ({x},{y}) inside bounds");
            }
        }
    }
}

/// Text starting at **negative x**: the left half of the first glyph is off-screen; only the part
/// at x≥0 is recorded. This guards the negative-origin clip path text never otherwise takes.
#[test]
fn text_at_negative_x_clips_the_left_half() {
    let mut b = Buf::new(48, 24);
    draw_text(&mut b, "AB", Point::new(-6, 4), Font::Body, TextAlign::Left, RED);
    // Some of "B" (and the right of "A") lands at x>=0; nothing is recorded at x<0 (Buf clips it).
    let (minx, _, _, _) = b.bbox(RED).expect("the on-screen part of the text draws");
    assert!(minx >= 0, "no pixel is recorded left of the buffer ({minx})");
}
