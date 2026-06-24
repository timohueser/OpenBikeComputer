//! OBCM v5 serializer — lay out the `.obcm` bytes.
//!
//! Deterministic integer/byte work: given the same feature list and quadtree it
//! produces the same output every run. The geometry that reaches here is already
//! clipped + simplified; this module only rounds lon/lat to microdegrees (banker's
//! rounding — round-half-to-even), densifies long segments, delta-encodes rings,
//! and lays out the chunk / index / LOD-table / header bytes per `OBCM_Spec.md`.

use std::io::{self, Seek, SeekFrom, Write};

/// Max delta (microdegrees) before a segment is densified to keep deltas in
/// 16-bit range.
const MAX_SEGMENT: i64 = 30_000;

/// Fixed header length (bytes).
pub const HEADER_LEN: usize = 32;
/// One LOD-table entry, `<fIIHI>`.
pub const LOD_ENTRY_LEN: usize = 18;

/// Largest map `chunk_size` (bytes) that guarantees no single feature exceeds the device
/// reader's [`obc_reader::MAX_FEAT_PTS`] vertex cap. A chunk holds at most `chunk_size` bytes of
/// one feature, and the densest encoding is 8-bit deltas — a 12-byte feature header then 2 bytes
/// per vertex (`dx`, `dy`) — so a feature carries at most `(chunk_size - 12) / 2 + 1` vertices.
/// Above this the packer can emit a feature the reader **silently truncates** (`read_ring`'s
/// `heapless` push fails past capacity), corrupting that feature's fill/stroke with no error on
/// either side (issue #2). The packer default (4096) sits just under it.
pub const MAX_SAFE_CHUNK_SIZE: usize = (obc_reader::MAX_FEAT_PTS - 1) * 2 + 12;

// The safe ceiling must itself fit the on-wire `u16` chunk_size field, or the bound is moot.
const _: () = assert!(MAX_SAFE_CHUNK_SIZE <= u16::MAX as usize, "chunk_size is a u16 in the format");

/// Reject a `chunk_size` the device reader can't decode without truncating a feature — see
/// [`MAX_SAFE_CHUNK_SIZE`]. Returns a descriptive build error so a misconfigured pack fails
/// loudly instead of producing a silently-corrupt map (issue #2).
pub fn validate_chunk_size(chunk_size: usize) -> Result<(), String> {
    if chunk_size > MAX_SAFE_CHUNK_SIZE {
        return Err(format!(
            "chunk_size {chunk_size} exceeds the safe maximum {MAX_SAFE_CHUNK_SIZE}: a single feature \
             could then pack more than {} vertices, which the device reader silently truncates \
             (issue #2). Lower chunk_size, or raise the LOD's simplify tolerance.",
            obc_reader::MAX_FEAT_PTS
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Line,
    Polygon,
}

/// A style record as packed into the Style Table (`pack_style_dict`).
#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub id: u8,
    pub z_index: i8,
    pub color: u16,
    pub weight: u8,
    /// Priority 1..=4; clamped to that range on pack.
    pub priority: u8,
}

/// A feature as it reaches the serializer. Geometry is f64 lon/lat — the clipped +
/// simplified coords — rounded to microdegrees and densified here. `rings[0]` is
/// the exterior; `rings[1..]` are interior rings (polygons only). Lines carry a
/// single ring.
#[derive(Debug, Clone)]
pub struct Feature {
    pub style_id: u8,
    pub kind: Kind,
    pub rings: Vec<Vec<(f64, f64)>>,
}

/// One node of a serialized quadtree. `serialize_tree` only needs leaf bboxes
/// (for anchors) and the leaf/branch shape; child bboxes are re-derived by the
/// reader, so branches store only their four children (order NW, NE, SW, SE).
#[derive(Debug, Clone)]
pub enum Node {
    Leaf {
        /// (min_lon, min_lat, max_lon, max_lat) in microdegrees.
        bbox: (i64, i64, i64, i64),
        features: Vec<Feature>,
    },
    Branch(Box<[Node; 4]>),
}

/// One LOD layer to serialize: its quadtree root + per-layer chunk size and the
/// m/px upper bound (`None` ⇒ `+inf`, the coarsest layer).
#[derive(Debug, Clone)]
pub struct LodLayer {
    pub max_mpp: Option<f64>,
    pub chunk_size: usize,
    pub root: Node,
}

/// `round(v * 1e6)` to microdegrees with **round-half-to-even** (banker's
/// rounding). Rust's `f64::round` is half-*away*-from-zero, so we use
/// `round_ties_even`; the value is integer-valued before the `as i64` truncation,
/// so the cast is exact. (The rounding mode matters — getting it wrong shifts
/// vertices by a microdegree.)
#[inline]
fn to_udeg(v: f64) -> i64 {
    (v * 1e6).round_ties_even() as i64
}

/// Append intermediate points between `p1` and `p2` (then `p2`) so no single
/// (dx, dy) step exceeds the 16-bit delta range, using an integer step count and
/// banker's-rounded midpoints.
fn densify(p1: (i64, i64), p2: (i64, i64), out: &mut Vec<(i64, i64)>) {
    let dx = p2.0 - p1.0;
    let dy = p2.1 - p1.1;
    let max_dist = dx.abs().max(dy.abs());
    if max_dist > MAX_SEGMENT {
        let steps = max_dist / MAX_SEGMENT + 1; // integer step count
        for step in 1..steps {
            let t = step as f64 / steps as f64;
            out.push((
                (p1.0 as f64 + dx as f64 * t).round_ties_even() as i64,
                (p1.1 as f64 + dy as f64 * t).round_ties_even() as i64,
            ));
        }
    }
    out.push(p2);
}

#[inline]
fn push_deltas(data: &mut Vec<u8>, deltas: &[i64], is16: bool) {
    if is16 {
        for &d in deltas {
            data.extend_from_slice(&(d as i16).to_le_bytes());
        }
    } else {
        for &d in deltas {
            data.push(d as i8 as u8);
        }
    }
}

/// Pack the style table: `Count(u8)` then, sorted by id, `<BbHBB>` per style with
/// `flags = (priority-1) & 0x03`.
pub fn pack_style_dict(styles: &[Style]) -> Vec<u8> {
    let mut styles = styles.to_vec();
    styles.sort_by_key(|s| s.id);
    let mut data = Vec::with_capacity(1 + styles.len() * 6);
    data.push(styles.len() as u8);
    for s in &styles {
        let priority = (s.priority as i32).clamp(1, 4);
        let flags = ((priority - 1) & 0x03) as u8;
        data.push(s.id);
        data.push(s.z_index as u8);
        data.extend_from_slice(&s.color.to_le_bytes());
        data.push(s.weight);
        data.push(flags);
    }
    data
}

/// Pack one feature: 12-byte header `<BHiiB>` + delta-encoded rings. `node_bbox`
/// is the containing leaf's bbox; the exterior's first point becomes the anchor,
/// stored relative to the leaf min corner.
pub fn pack_feature(f: &Feature, node_bbox: (i64, i64, i64, i64)) -> Vec<u8> {
    let is_polygon = f.kind == Kind::Polygon;
    let mut flags: u8 = 0;
    if is_polygon {
        flags |= 0x02; // polygon
        if f.rings.len() > 1 {
            flags |= 0x04; // has holes
        }
    }

    let mut anchor_lon = 0i64;
    let mut anchor_lat = 0i64;
    let mut max_delta = 0i64;
    let mut packed_rings: Vec<(usize, Vec<i64>)> = Vec::with_capacity(f.rings.len());

    for (i, ring) in f.rings.iter().enumerate() {
        let raw_pts: Vec<(i64, i64)> = ring.iter().map(|&(lon, lat)| (to_udeg(lon), to_udeg(lat))).collect();

        let start_ref = if i == 0 {
            anchor_lon = raw_pts[0].0 - node_bbox.0;
            anchor_lat = raw_pts[0].1 - node_bbox.1;
            raw_pts[0]
        } else {
            (node_bbox.0 + anchor_lon, node_bbox.1 + anchor_lat)
        };

        // Jump from the reference point to the first vertex, then walk the ring.
        let mut pts: Vec<(i64, i64)> = Vec::new();
        densify(start_ref, raw_pts[0], &mut pts);
        for &p2 in &raw_pts[1..] {
            let last = *pts.last().unwrap();
            densify(last, p2, &mut pts);
        }

        // Exterior: first point is the anchor; deltas start at the 2nd vertex.
        // Hole: every vertex is a delta, the first relative to the anchor.
        let (mut prev, delta_pts): ((i64, i64), &[(i64, i64)]) =
            if i == 0 { (pts[0], &pts[1..]) } else { (start_ref, &pts[..]) };

        let mut deltas: Vec<i64> = Vec::with_capacity(delta_pts.len() * 2);
        for &(x, y) in delta_pts {
            let dx = x - prev.0;
            let dy = y - prev.1;
            deltas.push(dx);
            deltas.push(dy);
            max_delta = max_delta.max(dx.abs()).max(dy.abs());
            prev = (x, y);
        }
        packed_rings.push((pts.len(), deltas));
    }

    let is16 = max_delta > 127;
    if is16 {
        flags |= 0x01; // 16-bit deltas
    }

    // The reader decodes a whole feature (exterior + every hole) into one buffer of capacity
    // `MAX_FEAT_PTS`; past that its `heapless` push silently drops vertices. `validate_chunk_size`
    // keeps a packed feature under the cap, but assert the real invariant in debug (issue #2).
    debug_assert!(
        packed_rings.iter().map(|(n, _)| *n).sum::<usize>() <= obc_reader::MAX_FEAT_PTS,
        "feature vertex count exceeds the reader's MAX_FEAT_PTS — chunk_size too large?"
    );

    debug_assert!(packed_rings[0].0 <= u16::MAX as usize, "exterior pt_count overflows the u16 field");
    let ext_pt_count = packed_rings[0].0 as u16;
    let mut data = Vec::new();
    data.push(f.style_id);
    data.extend_from_slice(&ext_pt_count.to_le_bytes());
    data.extend_from_slice(&(anchor_lon as i32).to_le_bytes());
    data.extend_from_slice(&(anchor_lat as i32).to_le_bytes());
    data.push(flags);

    push_deltas(&mut data, &packed_rings[0].1, is16);

    if flags & 0x04 != 0 {
        data.push((packed_rings.len() - 1) as u8);
        for (pt_count, deltas) in &packed_rings[1..] {
            debug_assert!(*pt_count <= u16::MAX as usize, "hole pt_count overflows the u16 field");
            data.extend_from_slice(&(*pt_count as u16).to_le_bytes());
            push_deltas(&mut data, deltas, is16);
        }
    }
    data
}

/// Pack features into a fixed-size chunk, padded with `0xFF`. A feature that
/// would overflow the chunk (and every feature after it) is dropped.
pub fn pack_chunk(features: &[Feature], node_bbox: (i64, i64, i64, i64), chunk_size: usize) -> Vec<u8> {
    let mut data = Vec::new();
    for f in features {
        let packed = pack_feature(f, node_bbox);
        if data.len() + packed.len() > chunk_size {
            break;
        }
        data.extend_from_slice(&packed);
    }
    data.resize(chunk_size, 0xFF);
    data
}

/// Flatten one quadtree into `(index_bytes, node_count, chunk_bytes,
/// chunk_count)` via BFS. Child order and chunk-id assignment order are BFS, which
/// fixes the byte layout.
pub fn serialize_tree(root: &Node, chunk_size: usize) -> (Vec<u8>, u32, Vec<u8>, u32) {
    // BFS in enqueue order. Children are appended contiguously, so a branch's
    // first-child index is the length of `nodes` at the moment we expand it.
    let mut nodes: Vec<&Node> = vec![root];
    let mut first_child: Vec<usize> = vec![0];
    let mut i = 0;
    while i < nodes.len() {
        if let Node::Branch(children) = nodes[i] {
            first_child[i] = nodes.len();
            for c in children.iter() {
                nodes.push(c);
                first_child.push(0);
            }
        }
        i += 1;
    }

    let mut index: Vec<u32> = Vec::with_capacity(nodes.len());
    let mut chunks: Vec<u8> = Vec::new();
    let mut chunk_count: u32 = 0;
    for (idx, node) in nodes.iter().enumerate() {
        match node {
            Node::Leaf { bbox, features } => {
                if features.is_empty() {
                    index.push(0x7FFF_FFFF);
                } else {
                    let chunk_id = chunk_count;
                    chunks.extend_from_slice(&pack_chunk(features, *bbox, chunk_size));
                    chunk_count += 1;
                    index.push(chunk_id & 0x7FFF_FFFF);
                }
            }
            Node::Branch(_) => {
                index.push((first_child[idx] as u32) | 0x8000_0000);
            }
        }
    }

    let mut index_bytes = Vec::with_capacity(index.len() * 4);
    for v in &index {
        index_bytes.extend_from_slice(&v.to_le_bytes());
    }
    (index_bytes, index.len() as u32, chunks, chunk_count)
}

/// The 32-byte OBCM header `<4sBiiiiIBIH>`: magic, version, bbox stored as
/// lat,lon,lat,lon, header length, lod count, lod-table offset, marker color.
/// Shared by [`serialize_lods`] and [`serialize_lods_streaming`] so the layout
/// stays identical.
fn header_bytes(
    lod_count: usize,
    marker_color: u16,
    global_bbox: (i64, i64, i64, i64),
    lod_table_offset: usize,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN);
    out.extend_from_slice(b"OBCM");
    out.push(0x05);
    out.extend_from_slice(&(global_bbox.1 as i32).to_le_bytes()); // min_lat
    out.extend_from_slice(&(global_bbox.0 as i32).to_le_bytes()); // min_lon
    out.extend_from_slice(&(global_bbox.3 as i32).to_le_bytes()); // max_lat
    out.extend_from_slice(&(global_bbox.2 as i32).to_le_bytes()); // max_lon
    out.extend_from_slice(&(HEADER_LEN as u32).to_le_bytes());
    out.push(lod_count as u8);
    out.extend_from_slice(&(lod_table_offset as u32).to_le_bytes());
    out.extend_from_slice(&marker_color.to_le_bytes());
    debug_assert_eq!(out.len(), HEADER_LEN);
    out
}

/// Append one LOD-table entry `<fIIHI>`: max_mpp (`None` ⇒ `+inf`), index offset,
/// node count, chunk size, chunk count. Same field order in both serializers.
fn push_lod_entry(table: &mut Vec<u8>, max_mpp: Option<f64>, index_offset: u32, nc: u32, cs: usize, cc: u32) {
    let mpp_f: f32 = max_mpp.map_or(f32::INFINITY, |v| v as f32);
    table.extend_from_slice(&mpp_f.to_le_bytes());
    table.extend_from_slice(&index_offset.to_le_bytes());
    table.extend_from_slice(&nc.to_le_bytes());
    table.extend_from_slice(&(cs as u16).to_le_bytes());
    table.extend_from_slice(&cc.to_le_bytes());
}

/// Serialize a pyramid of LOD layers into the full v5 `.obcm` byte stream (header
/// field order, LOD table layout, and the bbox stored as lat,lon,lat,lon).
pub fn serialize_lods(
    lods: &[LodLayer],
    styles: &[Style],
    marker_color: u16,
    global_bbox: (i64, i64, i64, i64),
) -> Vec<u8> {
    let style_data = pack_style_dict(styles);
    let lod_count = lods.len();
    let lod_table_offset = HEADER_LEN + style_data.len();

    struct Block {
        ib: Vec<u8>,
        nc: u32,
        cb: Vec<u8>,
        cc: u32,
        cs: usize,
        mpp: Option<f64>,
    }
    let mut blocks = Vec::with_capacity(lod_count);
    for lod in lods {
        let (ib, nc, cb, cc) = serialize_tree(&lod.root, lod.chunk_size);
        blocks.push(Block { ib, nc, cb, cc, cs: lod.chunk_size, mpp: lod.max_mpp });
    }

    let mut cursor = lod_table_offset + lod_count * LOD_ENTRY_LEN;
    let mut table = Vec::with_capacity(lod_count * LOD_ENTRY_LEN);
    let mut payload = Vec::new();
    for b in &blocks {
        push_lod_entry(&mut table, b.mpp, cursor as u32, b.nc, b.cs, b.cc);
        payload.extend_from_slice(&b.ib);
        payload.extend_from_slice(&b.cb);
        cursor += b.ib.len() + b.cb.len();
    }

    let mut out = Vec::with_capacity(lod_table_offset + table.len() + payload.len());
    out.extend_from_slice(&header_bytes(lod_count, marker_color, global_bbox, lod_table_offset));
    out.extend_from_slice(&style_data);
    out.extend_from_slice(&table);
    out.extend_from_slice(&payload);
    out
}

/// Streaming counterpart to [`serialize_lods`]: writes the **same** v5 byte
/// stream, but builds, serializes, and *drops* one LOD tree at a time, streaming
/// each LOD's payload straight to `w`. Peak memory is ~one tree + one LOD's chunk
/// bytes, instead of all trees plus the whole output buffer in RAM — the Stage-6
/// memory win (freiburg's 5-LOD build peaked at ~2.7 GB here).
///
/// The header, style table, and LOD-table *offset* are all known up front; only
/// the per-LOD table entries need the built trees. So we write the header + style
/// table + a **zeroed** LOD table, stream each LOD's `index ++ chunks` (recording
/// its table entry), then `seek` back and patch the LOD table. The bytes are
/// identical to `serialize_lods` for the same trees (asserted by
/// `streaming_matches_in_memory`). Returns the total bytes written.
///
/// `build(i)` produces LOD `i`'s `(root, chunk_size, max_mpp)`; it is called once
/// per level, in order, and each tree is dropped before the next call.
pub fn serialize_lods_streaming<W, F>(
    w: &mut W,
    lod_count: usize,
    styles: &[Style],
    marker_color: u16,
    global_bbox: (i64, i64, i64, i64),
    mut build: F,
) -> io::Result<u64>
where
    W: Write + Seek,
    F: FnMut(usize) -> (Node, usize, Option<f64>),
{
    let style_data = pack_style_dict(styles);
    let lod_table_offset = HEADER_LEN + style_data.len();
    let payload_start = lod_table_offset + lod_count * LOD_ENTRY_LEN;

    // 1. Header (bbox stored lat,lon,lat,lon) — needs no tree.
    w.write_all(&header_bytes(lod_count, marker_color, global_bbox, lod_table_offset))?;

    // 2. Style table, then a zeroed LOD table we patch in step 4.
    w.write_all(&style_data)?;
    w.write_all(&vec![0u8; lod_count * LOD_ENTRY_LEN])?;

    // 3. Per-LOD: build → serialize → stream payload → drop the tree.
    let mut table = Vec::with_capacity(lod_count * LOD_ENTRY_LEN);
    let mut cursor = payload_start;
    for i in 0..lod_count {
        let (root, chunk_size, max_mpp) = build(i);
        let (ib, nc, cb, cc) = serialize_tree(&root, chunk_size);
        drop(root); // free the tree before writing this LOD / building the next
        push_lod_entry(&mut table, max_mpp, cursor as u32, nc, chunk_size, cc);
        w.write_all(&ib)?;
        w.write_all(&cb)?;
        cursor += ib.len() + cb.len();
    }

    // 4. Back-patch the LOD table in place, then leave the cursor at EOF.
    w.seek(SeekFrom::Start(lod_table_offset as u64))?;
    w.write_all(&table)?;
    w.seek(SeekFrom::Start(cursor as u64))?;
    Ok(cursor as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_is_ties_even_not_away() {
        // `round_ties_even` is round-half-to-even; `f64::round` is round-half-away-
        // from-zero. `to_udeg` must use the former. Exact halves are representable
        // in f64, so these pin the *mode* `to_udeg` relies on.
        assert_eq!(0.5_f64.round_ties_even(), 0.0);
        assert_eq!(1.5_f64.round_ties_even(), 2.0);
        assert_eq!(2.5_f64.round_ties_even(), 2.0);
        assert_eq!(3.5_f64.round_ties_even(), 4.0);
        assert_eq!((-1.5_f64).round_ties_even(), -2.0);
        // The wrong (away) mode would give 1.0 and 3.0 here — guard against it.
        assert_eq!(0.5_f64.round(), 1.0);
        assert_eq!(2.5_f64.round(), 3.0);
        // to_udeg scales by 1e6 then rounds ties-even.
        assert_eq!(to_udeg(1.0), 1_000_000);
        assert_eq!(to_udeg(1.0001), 1_000_100);
    }

    #[test]
    fn densify_steps_long_segments() {
        // A 55000-µdeg jump → steps = 55000//30000 + 1 = 2, so exactly one
        // banker's-rounded midpoint, then the endpoint.
        let mut out = Vec::new();
        densify((0, 0), (55000, 0), &mut out);
        assert_eq!(out, vec![(27500, 0), (55000, 0)]);

        // Just under the threshold: no midpoint, just the endpoint.
        let mut out2 = Vec::new();
        densify((0, 0), (100, 200), &mut out2);
        assert_eq!(out2, vec![(100, 200)]);

        // Exactly at the threshold (30000) is NOT densified (`> MAX_SEGMENT`).
        let mut out3 = Vec::new();
        densify((0, 0), (30000, -30000), &mut out3);
        assert_eq!(out3, vec![(30000, -30000)]);
    }

    #[test]
    fn max_safe_chunk_size_keeps_features_within_reader_cap() {
        // A feature packed to exactly fill a `MAX_SAFE_CHUNK_SIZE` chunk must carry no more than
        // the reader's `MAX_FEAT_PTS` vertices — otherwise the device silently truncates it (#2).
        let n = obc_reader::MAX_FEAT_PTS;
        // `n` points 1 µdeg apart: tiny deltas ⇒ 8-bit encoding (2 bytes/vertex — the densest
        // case), no densification (steps « MAX_SEGMENT).
        let coords: Vec<(f64, f64)> = (0..n).map(|i| (i as f64 * 1e-6, 0.0)).collect();
        let f = Feature { style_id: 1, kind: Kind::Line, rings: vec![coords] };
        let packed = pack_feature(&f, (0, 0, n as i64, 1));

        let ext_pt_count = u16::from_le_bytes([packed[1], packed[2]]) as usize;
        assert_eq!(ext_pt_count, n, "a cap-sized feature keeps every vertex");
        assert!(ext_pt_count <= obc_reader::MAX_FEAT_PTS, "must not exceed the reader cap");
        assert!(packed.len() <= MAX_SAFE_CHUNK_SIZE, "and fits the safe-max chunk: {} bytes", packed.len());
    }

    #[test]
    fn validate_chunk_size_accepts_safe_rejects_oversize() {
        assert!(validate_chunk_size(4096).is_ok(), "the packer default must pass");
        assert!(validate_chunk_size(MAX_SAFE_CHUNK_SIZE).is_ok(), "the boundary is inclusive");
        assert!(validate_chunk_size(MAX_SAFE_CHUNK_SIZE + 1).is_err(), "one over the cap is rejected");
        assert!(validate_chunk_size(8192).is_err(), "a chunk_size that could truncate is rejected (#2)");
    }

    #[test]
    fn streaming_matches_in_memory() {
        // The streaming serializer must be byte-identical to `serialize_lods` for
        // the same trees. Build a small 2-LOD pyramid via the real quadtree,
        // serialize both ways, and compare.
        use crate::geom::Geom;
        use std::io::Cursor;

        let bbox = (0, 0, 1_000_000, 1_000_000);
        let styles = vec![Style { id: 1, z_index: 0, color: 0x1234, weight: 2, priority: 1 }];
        let lods = vec![
            LodLayer {
                max_mpp: Some(100.0),
                chunk_size: 256,
                root: crate::quadtree::build_lod([(1u8, Geom::Line(vec![(0.1, 0.1), (0.9, 0.9)]))], bbox, 256),
            },
            LodLayer {
                max_mpp: None,
                chunk_size: 256,
                root: crate::quadtree::build_lod(
                    [(1u8, Geom::Line(vec![(0.2, 0.2), (0.8, 0.8), (0.5, 0.1)]))],
                    bbox,
                    256,
                ),
            },
        ];

        let reference = serialize_lods(&lods, &styles, 0xABCD, bbox);

        let mut cur = Cursor::new(Vec::new());
        let total = serialize_lods_streaming(&mut cur, lods.len(), &styles, 0xABCD, bbox, |i| {
            (lods[i].root.clone(), lods[i].chunk_size, lods[i].max_mpp)
        })
        .unwrap();

        assert_eq!(cur.into_inner(), reference, "streaming output must be byte-identical");
        assert_eq!(total as usize, reference.len());
    }
}
