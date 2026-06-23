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

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
use obc_reader::{rgb565_to_rgb888, MapCache, Reader, SliceSource};
use obc_render::{MapRenderer, Viewport, MAX_SPANS};

// Distinct colors per priority so the recording target can tell them apart.
const LOW_565: u16 = 0x001F; // priority 4, blue
const HIGH_565: u16 = 0xF800; // priority 1, red
const RED: Rgb888 = Rgb888::new(255, 0, 0);
const BLUE: Rgb888 = Rgb888::new(0, 0, 255);

const BRANCH_BIT: u32 = 0x8000_0000;
const EMPTY_LEAF: u32 = 0x7FFF_FFFF;

/// Priority-4 polygons in the early chunk: enough to overflow `MAX_SPANS` on their
/// own, so the buffer is already full before the late chunk is even reached.
const NUM_LOW: usize = MAX_SPANS + 64;

// ---------------------------------------------------------------------------
// Byte builders (mirror obc-reader/tests/format.rs / serialize.py).
// ---------------------------------------------------------------------------

/// A hole-free polygon with 8-bit deltas. `deltas` are the points after the
/// anchor, so the stored exterior point count is `1 + deltas.len()`.
fn pack_poly(style_id: u8, ax: i32, ay: i32, deltas: &[(i8, i8)]) -> Vec<u8> {
    let mut v = Vec::new();
    v.push(style_id);
    v.extend_from_slice(&((1 + deltas.len()) as u16).to_le_bytes());
    v.extend_from_slice(&ax.to_le_bytes());
    v.extend_from_slice(&ay.to_le_bytes());
    v.push(0x02); // flags: polygon, no holes, 8-bit deltas
    for &(dx, dy) in deltas {
        v.push(dx as u8);
        v.push(dy as u8);
    }
    v
}

fn pad(mut chunk: Vec<u8>, size: usize) -> Vec<u8> {
    assert!(chunk.len() <= size, "chunk {} exceeds chunk_size {}", chunk.len(), size);
    chunk.resize(size, 0xFF);
    chunk
}

/// Build a single-LOD file whose root quadtree node is a branch. NW is itself a branch whose
/// four leaves are chunks 0–3 (the "early" chunks, all visited before NE); NE is chunk 4 (the
/// "late" chunk). Splitting the early load across four leaves keeps every chunk under the
/// reader's `MAX_CHUNK_BYTES` cap while still saturating the frame buffer before NE is reached.
/// `styles` are `(id, z, color, weight, priority)`.
fn build_file(
    bbox: (i32, i32, i32, i32),
    styles: &[(u8, i8, u16, u8, u8)],
    chunk_size: usize,
    nw_chunks: [Vec<u8>; 4],
    ne_chunk: Vec<u8>,
) -> Vec<u8> {
    let style_off = 32usize;
    let mut style_bytes = vec![styles.len() as u8];
    for &(id, z, color, weight, priority) in styles {
        style_bytes.push(id);
        style_bytes.push(z as u8);
        style_bytes.extend_from_slice(&color.to_le_bytes());
        style_bytes.push(weight);
        style_bytes.push((priority - 1) & 0x03);
    }

    let lod_tab_off = style_off + style_bytes.len();
    let index_off = lod_tab_off + 18; // one 18-byte LOD entry

    // Quadtree (9 nodes). Root branch -> [NW=branch@5, NE=chunk 4, SW/SE empty]; NW's four
    // children (idx 5..8) -> chunks 0,1,2,3. Walk order NW(→0,1,2,3) then NE(→4): the four
    // early chunks are all visited before the late one.
    let index: [u32; 9] = [BRANCH_BIT | 1, BRANCH_BIT | 5, 4, EMPTY_LEAF, EMPTY_LEAF, 0, 1, 2, 3];
    let mut idx_bytes = Vec::new();
    for node in index {
        idx_bytes.extend_from_slice(&node.to_le_bytes());
    }
    // Chunk data in chunk-id order: 0..3 = NW leaves, 4 = NE.
    let [nw0, nw1, nw2, nw3] = nw_chunks;
    let chunks = [nw0, nw1, nw2, nw3, ne_chunk];

    // LOD entry: max_mpp=+inf, index_off, node_count, chunk_size, chunk_count.
    let mut table = Vec::new();
    table.extend_from_slice(&f32::INFINITY.to_le_bytes());
    table.extend_from_slice(&(index_off as u32).to_le_bytes());
    table.extend_from_slice(&(index.len() as u32).to_le_bytes());
    table.extend_from_slice(&(chunk_size as u16).to_le_bytes());
    table.extend_from_slice(&(chunks.len() as u32).to_le_bytes());

    let mut f = Vec::new();
    f.extend_from_slice(b"OBCM");
    f.push(5);
    f.extend_from_slice(&bbox.1.to_le_bytes()); // min_lat
    f.extend_from_slice(&bbox.0.to_le_bytes()); // min_lon
    f.extend_from_slice(&bbox.3.to_le_bytes()); // max_lat
    f.extend_from_slice(&bbox.2.to_le_bytes()); // max_lon
    f.extend_from_slice(&(style_off as u32).to_le_bytes());
    f.push(1); // lod count
    f.extend_from_slice(&(lod_tab_off as u32).to_le_bytes());
    f.extend_from_slice(&0u16.to_le_bytes()); // marker color (unused here)
    assert_eq!(f.len(), 32, "header must be 32 bytes");
    f.extend_from_slice(&style_bytes);
    f.extend_from_slice(&table);
    f.extend_from_slice(&idx_bytes);
    for c in chunks {
        f.extend_from_slice(&pad(c, chunk_size));
    }
    f
}

// ---------------------------------------------------------------------------
// Recording DrawTarget.
// ---------------------------------------------------------------------------

/// A `w`×`h` Rgb888 buffer implementing `DrawTarget`, with clipped writes.
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

#[test]
fn priority_one_survives_saturation_across_chunks() {
    // Global bbox (0,0,1000,1000); root branch midpoints (500,500). The early
    // chunk lands in the NW quadrant, the late chunk in NE — they project to the
    // left and right halves of the screen respectively, so their colors never
    // overlap and can be counted independently.
    let styles: &[(u8, i8, u16, u8, u8)] = &[
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

    let bytes = build_file((0, 0, 1000, 1000), styles, chunk_size, nw_chunks, ne);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let reader = Reader::new(&src, &cache).expect("valid v5 file");

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
