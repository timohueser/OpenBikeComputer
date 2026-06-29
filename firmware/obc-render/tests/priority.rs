//! Render-level test for priority rendering under buffer saturation.
//!
//! The reader-level invariants (`filtered_decode_skips_without_drifting`,
//! `for_each_chunk_has_no_cap` in `obc-reader/tests/format.rs`) cover decoding,
//! but nothing asserts the actual payoff of the priority passes in
//! [`MapRenderer::render`]: **when the frame buffers saturate, the highest-priority
//! features survive and the lowest-priority ones are dropped — across chunks**.
//!
//! The setup is the worst case for any chunk-order collector: a *late* chunk holds
//! the single priority-1 polygon while an *early* chunk is packed with enough
//! priority-4 polygons to overflow `MAX_SPANS` on its own. A renderer that dropped
//! in chunk order would fill the buffer from the early chunk and drop the late
//! priority-1 polygon entirely (no red pixels). The priority passes collect level 1
//! first, across all chunks, so the priority-1 polygon survives. This test fails if
//! collection ever reverts to chunk-order (non-priority) dropping.

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader, SliceSource};
use obc_render::{MapRenderer, Viewport, MAX_SPANS};
use obcm_testkit::{build_priority_tree, pack_poly, Style};

mod common;
use common::Buf;

// Distinct colors per priority so the recording target can tell them apart.
const LOW_565: u16 = 0x001F; // priority 4, blue
const HIGH_565: u16 = 0xF800; // priority 1, red
const RED: Rgb888 = Rgb888::new(255, 0, 0);
const BLUE: Rgb888 = Rgb888::new(0, 0, 255);

/// Priority-4 polygons in the early chunk: enough to overflow `MAX_SPANS` on their
/// own, so the buffer is already full before the late chunk is even reached.
const NUM_LOW: usize = MAX_SPANS + 64;

// The byte builders (`build_priority_tree`, `pack_poly`) now live in `obcm-testkit`,
// imported above — the same single source the reader's format tests use, so a format
// bump edits one place.

#[test]
fn priority_one_survives_saturation_across_chunks() {
    // Global bbox (0,0,1000,1000); root branch midpoints (500,500). The early
    // chunk lands in the NW quadrant, the late chunk in NE — they project to the
    // left and right halves of the screen respectively, so their colors never
    // overlap and can be counted independently.
    let styles: &[Style] = &[
        (1, 0, LOW_565, 1, 4),  // priority 4 (lowest) — the bulk, in the early chunk
        (2, 1, HIGH_565, 1, 1), // priority 1 (highest) — one polygon, in the late chunk
    ];

    // Early chunks (the four NW leaves, all in the left/upper quadrant): NUM_LOW small
    // priority-4 triangles split evenly across them (remainder into the last), each near its
    // leaf-local (50,50). Together they overflow MAX_SPANS before NE is reached, and splitting
    // keeps every chunk well under the reader's MAX_CHUNK_BYTES cap (a single chunk of all
    // NUM_LOW features would exceed it).
    let one_low = pack_poly(1, 50, 50, &[(50, 0), (0, 50)]);
    let make = |n: usize| -> Vec<u8> { (0..n).flat_map(|_| one_low.clone()).collect() };
    let base = NUM_LOW / 4;
    let nw_chunks = [make(base), make(base), make(base), make(NUM_LOW - 3 * base)];
    let chunk_size = nw_chunks.iter().map(Vec::len).max().unwrap() + 64;

    // Late chunk (NE, node min corner (500,500)): one large priority-1 triangle near
    // node-local (50,50), big enough that its red fill is unmistakable.
    let ne = pack_poly(2, 50, 50, &[(120, 0), (0, 120), (-120, 0)]);

    let bytes = build_priority_tree((0, 0, 1000, 1000), styles, chunk_size, nw_chunks, ne);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).expect("valid v5 file");
    let reader = Reader::new(&src, &tables, &cache);

    // North-up view centered on the bbox; the whole 1000×1000 map fits on screen.
    let vp = Viewport::new(200.0, 200.0, 500, 500, 0.15);
    let mut buf = Buf::new(200, 200);
    let mut renderer = MapRenderer::new();
    let stats = renderer.render(&mut buf, &reader, &vp, Rgb888::BLACK, |c| {
        let (r, g, b) = rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });

    // The setup must actually saturate, or the test proves nothing.
    assert_eq!(stats.features_tried, NUM_LOW + 1, "all features visited");
    assert!(stats.features_dropped > 0, "buffers must saturate for this test to mean anything");
    assert_eq!(
        stats.features_drawn + stats.features_dropped,
        stats.features_tried,
        "every feature is either drawn or dropped"
    );
    assert!(stats.features_drawn <= MAX_SPANS, "never draws past the span buffer");

    // The payoff: the lone priority-1 polygon — in the *late* chunk, behind enough
    // priority-4 features to fill the buffer — survives and is painted. Chunk-order
    // dropping would have discarded it, leaving zero red pixels.
    assert!(buf.count(RED) > 100, "priority-1 polygon must survive saturation (got {} red px)", buf.count(RED));

    // Sanity: priority-4 features are drawn too (just not all of them) — saturation
    // dropped the overflow, not the whole low-priority layer.
    assert!(buf.count(BLUE) > 0, "some priority-4 features are still drawn");
}
