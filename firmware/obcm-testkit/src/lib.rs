//! Hand-written OBCM v5 byte builder shared by the `obc-reader` and `obc-render`
//! integration tests.
//!
//! Both crates need to synthesise `.obcm` byte buffers by hand (rather than checking
//! in a binary fixture) so the Rust reader stays pinned to `OBCM_Spec.md` /
//! `packer/obcm/serialize.py`: if either drifts, the format tests break. Before this
//! kit the 32-byte header + style-record pack and the `pack_*` feature encoders were
//! copy-pasted into both crates' `tests/format.rs` / `tests/priority.rs`, so a v6
//! format bump would have meant editing the same layout in two places. This crate is
//! the single source: a v6 bump edits it once.
//!
//! Two map shapes are needed and kept as distinct, clearly-named builders so each call
//! site's bytes stay identical:
//! - [`build_file`] — the general multi-LOD builder ([`LodSpec`] per layer), used by
//!   the reader's format-contract tests.
//! - [`build_priority_tree`] — a fixed single-LOD NW-branch / NE-leaf quadtree, used by
//!   the renderer's priority-saturation test.
//!
//! Style records are `(id, z_index, color_rgb565, weight, priority)`; feature encoders
//! ([`pack_line`], [`pack_line16`], [`pack_poly`], [`pack_poly_hole`]) return one
//! packed feature, and [`pad`] right-pads a chunk to its `chunk_size` with `0xFF`.

/// A style record: `(id, z_index, color_rgb565, weight, priority)`.
pub type Style = (u8, i8, u16, u8, u8);

/// Quadtree node flag: this slot is a branch; its low bits are the child base index.
pub const BRANCH_BIT: u32 = 0x8000_0000;
/// Quadtree node sentinel: an empty leaf (no chunk).
pub const EMPTY_LEAF: u32 = 0x7FFF_FFFF;
/// Distinctive (non-default) marker color baked into [`build_file`]'s header, so the
/// reader's round-trip test is meaningful.
pub const MARKER: u16 = 0xABCD;

/// One LOD layer: its quadtree index (flat u32 nodes) and padded data chunks.
pub struct LodSpec {
    pub max_mpp: f32,
    pub index: Vec<u32>,
    pub chunks: Vec<Vec<u8>>,
    pub chunk_size: usize,
}

/// Pack the style table: a count byte followed by one 6-byte record per style
/// (`id, z, color_le, weight, (priority-1)&0x03`). Shared by both file builders.
fn style_table(styles: &[Style]) -> Vec<u8> {
    let mut style_bytes = vec![styles.len() as u8];
    for &(id, z, color, weight, priority) in styles {
        style_bytes.push(id);
        style_bytes.push(z as u8);
        style_bytes.extend_from_slice(&color.to_le_bytes());
        style_bytes.push(weight);
        style_bytes.push((priority - 1) & 0x03);
    }
    style_bytes
}

/// Build a general multi-LOD `.obcm` (mirrors `serialize.py`). `bbox` is
/// `(min_lon, min_lat, max_lon, max_lat)`; `styles` are
/// `(id, z_index, color_rgb565, weight, priority)`; each [`LodSpec`] is one layer with
/// its own quadtree index and padded chunks. The header carries [`MARKER`] as the
/// marker color.
pub fn build_file(bbox: (i32, i32, i32, i32), styles: &[Style], lods: &[LodSpec]) -> Vec<u8> {
    let style_off = 32usize;

    let style_bytes = style_table(styles);

    let lod_tab_off = style_off + style_bytes.len();
    let mut cursor = lod_tab_off + lods.len() * 18;
    let mut table = Vec::new();
    let mut payload = Vec::new();
    for lod in lods {
        let idx_off = cursor;
        let mut idx_bytes = Vec::new();
        for &node in &lod.index {
            idx_bytes.extend_from_slice(&node.to_le_bytes());
        }
        let mut chunk_bytes = Vec::new();
        for c in &lod.chunks {
            assert_eq!(c.len(), lod.chunk_size, "chunk must be padded to chunk_size");
            chunk_bytes.extend_from_slice(c);
        }
        table.extend_from_slice(&lod.max_mpp.to_le_bytes());
        table.extend_from_slice(&(idx_off as u32).to_le_bytes());
        table.extend_from_slice(&(lod.index.len() as u32).to_le_bytes());
        table.extend_from_slice(&(lod.chunk_size as u16).to_le_bytes());
        table.extend_from_slice(&(lod.chunks.len() as u32).to_le_bytes());
        cursor += idx_bytes.len() + chunk_bytes.len();
        payload.extend_from_slice(&idx_bytes);
        payload.extend_from_slice(&chunk_bytes);
    }

    // Header: <4sBiiiiIBIH  magic, ver, min_lat, min_lon, max_lat, max_lon,
    // style_off, lod_count, lod_table_off, marker_color.
    let mut f = Vec::new();
    f.extend_from_slice(b"OBCM");
    f.push(5);
    f.extend_from_slice(&bbox.1.to_le_bytes()); // min_lat
    f.extend_from_slice(&bbox.0.to_le_bytes()); // min_lon
    f.extend_from_slice(&bbox.3.to_le_bytes()); // max_lat
    f.extend_from_slice(&bbox.2.to_le_bytes()); // max_lon
    f.extend_from_slice(&(style_off as u32).to_le_bytes());
    f.push(lods.len() as u8);
    f.extend_from_slice(&(lod_tab_off as u32).to_le_bytes());
    f.extend_from_slice(&MARKER.to_le_bytes());
    assert_eq!(f.len(), 32, "header must be 32 bytes");

    f.extend_from_slice(&style_bytes);
    f.extend_from_slice(&table);
    f.extend_from_slice(&payload);
    f
}

/// Build a single-LOD file whose root quadtree node is a branch. NW is itself a branch whose
/// four leaves are chunks 0–3 (the "early" chunks, all visited before NE); NE is chunk 4 (the
/// "late" chunk). Splitting the early load across four leaves keeps every chunk under the
/// reader's `MAX_CHUNK_BYTES` cap while still saturating the frame buffer before NE is reached.
/// `styles` are `(id, z, color, weight, priority)`. The marker color is unused here, so it is 0.
pub fn build_priority_tree(
    bbox: (i32, i32, i32, i32),
    styles: &[Style],
    chunk_size: usize,
    nw_chunks: [Vec<u8>; 4],
    ne_chunk: Vec<u8>,
) -> Vec<u8> {
    let style_off = 32usize;
    let style_bytes = style_table(styles);

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

/// Right-pad a chunk to `size` bytes with `0xFF` (the empty-byte filler the reader skips).
pub fn pad(mut chunk: Vec<u8>, size: usize) -> Vec<u8> {
    assert!(chunk.len() <= size, "chunk {} exceeds chunk_size {}", chunk.len(), size);
    chunk.resize(size, 0xFF);
    chunk
}

/// A line feature with 8-bit deltas. Exterior point count = `1 + deltas.len()`.
pub fn pack_line(style_id: u8, ax: i32, ay: i32, deltas: &[(i8, i8)]) -> Vec<u8> {
    let mut v = Vec::new();
    v.push(style_id);
    v.extend_from_slice(&((1 + deltas.len()) as u16).to_le_bytes());
    v.extend_from_slice(&ax.to_le_bytes());
    v.extend_from_slice(&ay.to_le_bytes());
    v.push(0x00); // flags: line, 8-bit deltas
    for &(dx, dy) in deltas {
        v.push(dx as u8);
        v.push(dy as u8);
    }
    v
}

/// A line feature with 16-bit deltas (flag bit 0).
pub fn pack_line16(style_id: u8, ax: i32, ay: i32, deltas: &[(i16, i16)]) -> Vec<u8> {
    let mut v = Vec::new();
    v.push(style_id);
    v.extend_from_slice(&((1 + deltas.len()) as u16).to_le_bytes());
    v.extend_from_slice(&ax.to_le_bytes());
    v.extend_from_slice(&ay.to_le_bytes());
    v.push(0x01); // flags: line, 16-bit deltas
    for &(dx, dy) in deltas {
        v.extend_from_slice(&dx.to_le_bytes());
        v.extend_from_slice(&dy.to_le_bytes());
    }
    v
}

/// A hole-free polygon with 8-bit deltas. `deltas` are the points after the anchor, so
/// the stored exterior point count is `1 + deltas.len()`.
pub fn pack_poly(style_id: u8, ax: i32, ay: i32, deltas: &[(i8, i8)]) -> Vec<u8> {
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

/// A polygon with one hole, 8-bit deltas. Hole vertices are all deltas (first relative
/// to the anchor), so its stored point count == `hole_deltas.len()`.
pub fn pack_poly_hole(style_id: u8, ax: i32, ay: i32, ext_deltas: &[(i8, i8)], hole_deltas: &[(i8, i8)]) -> Vec<u8> {
    let mut v = Vec::new();
    v.push(style_id);
    v.extend_from_slice(&((1 + ext_deltas.len()) as u16).to_le_bytes());
    v.extend_from_slice(&ax.to_le_bytes());
    v.extend_from_slice(&ay.to_le_bytes());
    v.push(0x06); // flags: polygon | has-holes, 8-bit deltas
    for &(dx, dy) in ext_deltas {
        v.push(dx as u8);
        v.push(dy as u8);
    }
    v.push(1u8); // hole count
    v.extend_from_slice(&(hole_deltas.len() as u16).to_le_bytes());
    for &(dx, dy) in hole_deltas {
        v.push(dx as u8);
        v.push(dy as u8);
    }
    v
}
