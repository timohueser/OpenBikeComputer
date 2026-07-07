//! Hand-written OBCM v8 byte builder shared by the `obc-reader` and `obc-render`
//! integration tests.
//!
//! Both crates need to synthesise `.obcm` byte buffers by hand (rather than checking
//! in a binary fixture) so the Rust reader stays pinned to `OBCM_Spec.md`: if
//! either drifts, the format tests break. Before this kit the header + style-record
//! pack and the `pack_*` feature encoders were copy-pasted into both crates'
//! `tests/format.rs` / `tests/priority.rs`, so a format bump would have meant
//! editing the same layout in two places. This crate is the single source: a bump
//! edits it once.
//!
//! v6 grew the header to 36 bytes (the trailing `POI Section Offset`) and appended
//! the POI section (spec §7). **v7** widened the POI record 32 → 36 bytes (name
//! 20 → 24 plus a `hours_ref` u16), appended two directory fields
//! (`hours_pool_offset`, `hours_pool_count`), and added the tail hours-pool section
//! (§7.5). **v8** grew the header to 40 bytes (the trailing `Nav Graph Offset`) and
//! appended the nav-graph section (spec §8): a node quadtree over variable-length
//! junction records plus a chunked edge pool. [`build_file`]/[`build_priority_tree`]
//! write **empty** POI + nav sections so the reader accepts them; the directory,
//! record, and pool builders ([`poi_directory`], [`pack_poi_record`], [`hours_pool`],
//! [`nav_directory`], [`pack_nav_record`], [`pack_nav_edge_record`]) let the
//! contract tests pin each section's bytes explicitly.
//!
//! Three map shapes are needed and kept as distinct, clearly-named builders so each call
//! site's bytes stay identical:
//! - [`build_file`] — the general multi-LOD builder ([`LodSpec`] per layer), used by
//!   the reader's format-contract tests.
//! - [`build_priority_tree`] — a fixed single-LOD NW-branch / NE-leaf quadtree, used by
//!   the renderer's priority-saturation test.
//! - [`build_bench_map`] — the deterministic two-LOD bench fixture `obc-bench` renders and
//!   hashes (issue #327); its bytes must stay identical on every machine, forever.
//!
//! Style records are `(id, z_index, color_rgb565, weight, priority)`; feature encoders
//! ([`pack_line`], [`pack_line16`], [`pack_poly`], [`pack_poly_hole`]) return one
//! packed feature, and [`pad`] right-pads a chunk to its `chunk_size` with `0xFF`.

/// A style record: `(id, z_index, color_rgb565, weight, priority)`.
pub type Style = (u8, i8, u16, u8, u8);

// The quadtree branch / empty-leaf sentinels are the format's, defined once in obc-reader
// (issue #12) and re-exported here so the builders and their call sites keep the short names.
pub use obc_reader::format::{BRANCH_BIT, EMPTY_LEAF};
// The per-feature flag bits, composed by the `pack_*` encoders below (used, not re-exported).
use obc_reader::format::{FEATURE_FLAG_16BIT, FEATURE_FLAG_HOLES, FEATURE_FLAG_POLYGON};
/// Distinctive (non-default) marker color baked into [`build_file`]'s header, so the
/// reader's round-trip test is meaningful.
pub const MARKER: u16 = 0xABCD;

/// POI category count in a v7 directory (spec §7.1): ids 1..=6.
pub const POI_CATEGORY_COUNT: u8 = 6;
/// One 36-byte POI record (spec §7.3): v7 widened it from 32 (name 20 → 24 + a `hours_ref` u16).
pub const POI_RECORD_LEN: usize = 36;
/// The `Name` field width inside a v7 POI record (spec §7.3): 24 bytes, `0xFF`-padded.
pub const POI_NAME_LEN: usize = 24;
/// One hours-pool blob (spec §7.5): `flags u8` + `7 × 2 × (open_q, close_q)`.
pub const POI_HOURS_BLOB_LEN: usize = 29;
/// The fixed POI chunk capacity the packer writes (spec §7.1); the builders use it too.
pub const POI_CHUNK_SIZE: usize = 512;
/// One 13-byte POI-directory category entry (spec §7.1).
pub const POI_CAT_ENTRY_LEN: usize = 13;
/// The two v7 directory fields trailing the per-category entries (spec §7.1): `hours_pool_offset
/// u32` + `hours_pool_count u16`.
pub const POI_DIR_POOL_FIELDS_LEN: usize = 6;

/// The v9 nav directory length (spec §8.1): `index_offset u32, index_node_count u32,
/// node_chunk_count u32, edge_pool_offset u32, edge_chunk_count u32, chunk_size u16,
/// profile_table_offset u32, profile_count u8, reserved u8` (v8 was 22).
pub const NAV_DIR_LEN: usize = 28;
/// Fixed prefix of a §8.3 junction record (`lat i32, lon i32, node_id u32, degree u8`).
pub const NAV_NODE_FIXED_LEN: usize = 13;
/// One v9 §8.3 neighbor entry (`neighbor_id u32, dlat i16, dlon i16, edge_id u32, cost_m u16,
/// way_kind u8`), 15 bytes (v8 was 20 — absolute coords + u32 cost, no kind).
pub const NAV_NEIGHBOR_LEN: usize = 15;
/// Fixed prefix of a v9 §8.4 edge record (`length_m u32, pt_count u16, way_kind u8, anchor_lat i32,
/// anchor_lon i32`), 15 bytes (v8 was 14 — no way_kind).
pub const NAV_EDGE_FIXED_LEN: usize = 15;
/// The fixed nav chunk capacity the packer writes (spec §8.1) — pinned to 512 in v9.
pub const NAV_CHUNK_SIZE: usize = 512;
/// One §8.6 profile record (`name [u8;12]`, `highway_mult [u8;32]`, `surface_mult [u8;8]`).
pub const NAV_PROFILE_LEN: usize = 52;
/// The `Name` field width inside a §8.6 profile record: 12 bytes, `0xFF`-padded.
pub const NAV_PROFILE_NAME_LEN: usize = 12;

/// The v8 header length (bytes); the Style Table conventionally follows immediately, so it is the
/// builders' `style_off`. Kept in lock-step with [`obc_reader::HEADER_LEN`].
pub const HEADER_LEN: usize = 40;

/// One LOD layer: its quadtree index (flat u32 nodes) and padded data chunks.
pub struct LodSpec {
    pub max_mpp: f32,
    pub index: Vec<u32>,
    pub chunks: Vec<Vec<u8>>,
    pub chunk_size: usize,
}

/// Pack the style table: a count byte followed by one 6-byte record per style
/// (`id, z, color_le, weight, (priority-1) & STYLE_PRIORITY_MASK`). Shared by both file builders.
fn style_table(styles: &[Style]) -> Vec<u8> {
    let mut style_bytes = vec![styles.len() as u8];
    for &(id, z, color, weight, priority) in styles {
        style_bytes.push(id);
        style_bytes.push(z as u8);
        style_bytes.extend_from_slice(&color.to_le_bytes());
        style_bytes.push(weight);
        style_bytes.push((priority - 1) & obc_reader::format::STYLE_PRIORITY_MASK);
    }
    style_bytes
}

/// The 40-byte OBCM v8 header, shared by both file builders.
///
/// `<4sBiiiiIBIHII`: magic, ver, min_lat, min_lon, max_lat, max_lon, style_off, lod_count,
/// lod_table_off, marker_color, poi_section_off, nav_section_off. `bbox` is
/// `(min_lon, min_lat, max_lon, max_lat)`.
#[allow(clippy::too_many_arguments)]
fn obcm_header(
    bbox: (i32, i32, i32, i32),
    style_off: usize,
    lod_count: u8,
    lod_tab_off: usize,
    marker: u16,
    poi_section_off: usize,
    nav_section_off: usize,
) -> Vec<u8> {
    let mut f = Vec::new();
    f.extend_from_slice(b"OBCM");
    f.push(9);
    f.extend_from_slice(&bbox.1.to_le_bytes()); // min_lat
    f.extend_from_slice(&bbox.0.to_le_bytes()); // min_lon
    f.extend_from_slice(&bbox.3.to_le_bytes()); // max_lat
    f.extend_from_slice(&bbox.2.to_le_bytes()); // max_lon
    f.extend_from_slice(&(style_off as u32).to_le_bytes());
    f.push(lod_count);
    f.extend_from_slice(&(lod_tab_off as u32).to_le_bytes());
    f.extend_from_slice(&marker.to_le_bytes());
    f.extend_from_slice(&(poi_section_off as u32).to_le_bytes());
    f.extend_from_slice(&(nav_section_off as u32).to_le_bytes());
    assert_eq!(f.len(), 40, "header must be 40 bytes");
    f
}

/// One POI-directory category entry (spec §7.1): `category_id, index_offset, index_node_count,
/// chunk_count`. Used by [`poi_directory`] and the reader's POI contract tests.
pub struct PoiCat {
    pub category_id: u8,
    pub index_offset: u32,
    pub node_count: u32,
    pub chunk_count: u32,
}

/// Build a v7 POI directory (spec §7.1): the count byte, the shared `chunk_size`, one 13-byte entry
/// per category, then the `hours_pool_offset u32` + `hours_pool_count u16`. The caller supplies the
/// (already-computed) per-category offsets/counts and the pool offset/count — this only lays out the
/// directory bytes, not the indexes/chunks/pool that follow.
pub fn poi_directory(chunk_size: u16, cats: &[PoiCat], hours_pool_offset: u32, hours_pool_count: u16) -> Vec<u8> {
    let mut d = Vec::with_capacity(3 + cats.len() * POI_CAT_ENTRY_LEN + POI_DIR_POOL_FIELDS_LEN);
    d.push(cats.len() as u8);
    d.extend_from_slice(&chunk_size.to_le_bytes());
    for c in cats {
        d.push(c.category_id);
        d.extend_from_slice(&c.index_offset.to_le_bytes());
        d.extend_from_slice(&c.node_count.to_le_bytes());
        d.extend_from_slice(&c.chunk_count.to_le_bytes());
    }
    d.extend_from_slice(&hours_pool_offset.to_le_bytes());
    d.extend_from_slice(&hours_pool_count.to_le_bytes());
    d
}

/// The full v7 POI-directory length (bytes): count + chunk_size + six entries + the two pool fields.
pub const fn poi_dir_len() -> usize {
    3 + POI_CATEGORY_COUNT as usize * POI_CAT_ENTRY_LEN + POI_DIR_POOL_FIELDS_LEN
}

/// Pack the hours-pool section (spec §7.5): a `count u16` then `count × 29-byte` blobs, back-to-back.
/// Blob `i` (a record's `hours_ref`) lands at `hours_pool_offset + 2 + i*29`. An empty pool is just
/// the `0` count (2 bytes).
pub fn hours_pool(blobs: &[[u8; POI_HOURS_BLOB_LEN]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + blobs.len() * POI_HOURS_BLOB_LEN);
    out.extend_from_slice(&(blobs.len() as u16).to_le_bytes());
    for b in blobs {
        out.extend_from_slice(b);
    }
    out
}

/// An **empty** v7 POI directory + empty hours pool (spec §7.1/§7.5): six categories, all with
/// `node_count 0` and `chunk_count 0`, their `index_offset` pointing just past the directory (where a
/// zero-length index would begin); the hours pool (a bare `count 0`) sits right after, at the same
/// offset. `section_off` is the directory's absolute byte offset. This is what a map with no POIs
/// carries, and what [`build_file`]/[`build_priority_tree`] append so the reader accepts them.
pub fn empty_poi_directory(section_off: usize) -> Vec<u8> {
    let after_dir = (section_off + poi_dir_len()) as u32;
    let cats: Vec<PoiCat> = (1..=POI_CATEGORY_COUNT)
        .map(|id| PoiCat { category_id: id, index_offset: after_dir, node_count: 0, chunk_count: 0 })
        .collect();
    // No categories ⇒ no chunks: the (empty) hours pool follows the directory immediately.
    let mut d = poi_directory(POI_CHUNK_SIZE as u16, &cats, after_dir, 0);
    d.extend_from_slice(&hours_pool(&[]));
    d
}

/// Build a v9 nav directory (spec §8.1). The caller supplies the (already-computed) absolute
/// offsets/counts — this only lays out the 28 directory bytes, not the profile table / index /
/// chunks / pool that follow. `chunk_size` must be 512 (the reader rejects anything else).
#[allow(clippy::too_many_arguments)]
pub fn nav_directory(
    index_offset: u32,
    index_node_count: u32,
    node_chunk_count: u32,
    edge_pool_offset: u32,
    edge_chunk_count: u32,
    chunk_size: u16,
    profile_table_offset: u32,
    profile_count: u8,
) -> Vec<u8> {
    let mut d = Vec::with_capacity(NAV_DIR_LEN);
    d.extend_from_slice(&index_offset.to_le_bytes());
    d.extend_from_slice(&index_node_count.to_le_bytes());
    d.extend_from_slice(&node_chunk_count.to_le_bytes());
    d.extend_from_slice(&edge_pool_offset.to_le_bytes());
    d.extend_from_slice(&edge_chunk_count.to_le_bytes());
    d.extend_from_slice(&chunk_size.to_le_bytes());
    d.extend_from_slice(&profile_table_offset.to_le_bytes());
    d.push(profile_count);
    d.push(0); // reserved
    assert_eq!(d.len(), NAV_DIR_LEN);
    d
}

/// Pack one §8.6 profile record (52 bytes): a `0xFF`-padded 12-byte name + 32 highway + 8 surface
/// multipliers (`u8` 1/16 fixed-point). `name` is truncated to 12 bytes.
pub fn nav_profile_record(name: &str, highway: [u8; 32], surface: [u8; 8]) -> Vec<u8> {
    let mut rec = Vec::with_capacity(NAV_PROFILE_LEN);
    let nb = name.as_bytes();
    let n = nb.len().min(NAV_PROFILE_NAME_LEN);
    rec.extend_from_slice(&nb[..n]);
    rec.resize(NAV_PROFILE_NAME_LEN, 0xFF);
    rec.extend_from_slice(&highway);
    rec.extend_from_slice(&surface);
    assert_eq!(rec.len(), NAV_PROFILE_LEN);
    rec
}

/// A minimal §8.6 profile table: one profile ("Default", every multiplier 16 = 1.0×), 52 bytes —
/// enough to satisfy the v9 reader's "1..=8 profiles, always present" rule.
pub fn default_nav_profile_table() -> Vec<u8> {
    nav_profile_record("Default", [16; 32], [16; 8])
}

/// An **empty** v9 nav directory + its (always-present) profile table: no quadtree, no chunks, no
/// edges — what a map with no routable ways carries, and what [`build_file`]/[`build_priority_tree`]
/// append so the v9 reader accepts them. The profile table sits right after the 28-byte directory;
/// the zero-length index and edge pool "start" just past it. Returns dir + table (80 bytes).
pub fn empty_nav_directory(section_off: usize) -> Vec<u8> {
    let table = default_nav_profile_table();
    let profile_table_offset = (section_off + NAV_DIR_LEN) as u32;
    let after = profile_table_offset + table.len() as u32; // zero-length index + edge pool start here
    let mut out = nav_directory(after, 0, 0, after, 0, NAV_CHUNK_SIZE as u16, profile_table_offset, 1);
    out.extend_from_slice(&table);
    out
}

/// One v9 §8.3 neighbor entry for [`pack_nav_record`]: `(neighbor_id, lat, lon, edge_id, cost_m,
/// way_kind)`. `lat`/`lon` are the neighbor's **absolute** µdeg coords ([`pack_nav_record`] stores
/// the `i16` delta from the owning record's own coord); `cost_m` must fit `u16`.
pub type NavNeighborSpec = (u32, i32, i32, u32, u32, u8);

/// Pack one variable-length v9 §8.3 junction record: `lat i32, lon i32, node_id u32, degree u8`,
/// then one 15-byte entry per neighbor (`id u32, dlat i16, dlon i16, edge_id u32, cost_m u16,
/// way_kind u8`). The record head coords are absolute µdeg (lat first); each neighbor's coord is
/// stored as an `i16` delta from this record's own `lat`/`lon`.
pub fn pack_nav_record(lat: i32, lon: i32, node_id: u32, neighbors: &[NavNeighborSpec]) -> Vec<u8> {
    let mut rec = Vec::with_capacity(NAV_NODE_FIXED_LEN + neighbors.len() * NAV_NEIGHBOR_LEN);
    rec.extend_from_slice(&lat.to_le_bytes());
    rec.extend_from_slice(&lon.to_le_bytes());
    rec.extend_from_slice(&node_id.to_le_bytes());
    rec.push(neighbors.len() as u8);
    for &(id, nlat, nlon, edge_id, cost_m, way_kind) in neighbors {
        rec.extend_from_slice(&id.to_le_bytes());
        rec.extend_from_slice(&((nlat - lat) as i16).to_le_bytes());
        rec.extend_from_slice(&((nlon - lon) as i16).to_le_bytes());
        rec.extend_from_slice(&edge_id.to_le_bytes());
        rec.extend_from_slice(&(cost_m as u16).to_le_bytes());
        rec.push(way_kind);
    }
    rec
}

/// Pack junction records into one `chunk_size`-byte nav chunk (spec §8.3): back-to-back, then
/// `0xFF` padding — whose first byte lands on the next record's `degree` slot, the end sentinel.
pub fn pack_nav_chunk(records: &[Vec<u8>], chunk_size: usize) -> Vec<u8> {
    let mut c = Vec::with_capacity(chunk_size);
    for r in records {
        c.extend_from_slice(r);
    }
    assert!(c.len() <= chunk_size, "records exceed the nav chunk");
    c.resize(chunk_size, 0xFF);
    c
}

/// Pack one v9 §8.4 edge record: `length_m u32, pt_count u16, way_kind u8, anchor_lat i32,
/// anchor_lon i32`, then `pt_count - 1` × `(dlat i16, dlon i16)`. The polyline is absolute µdeg
/// `(lat, lon)` pairs (lat first, the §8 record convention); the caller keeps deltas within `i16`.
pub fn pack_nav_edge_record(length_m: u32, way_kind: u8, polyline: &[(i32, i32)]) -> Vec<u8> {
    let mut rec = Vec::with_capacity(NAV_EDGE_FIXED_LEN + (polyline.len() - 1) * 4);
    rec.extend_from_slice(&length_m.to_le_bytes());
    rec.extend_from_slice(&(polyline.len() as u16).to_le_bytes());
    rec.push(way_kind);
    rec.extend_from_slice(&polyline[0].0.to_le_bytes()); // anchor lat
    rec.extend_from_slice(&polyline[0].1.to_le_bytes()); // anchor lon
    for w in polyline.windows(2) {
        rec.extend_from_slice(&((w[1].0 - w[0].0) as i16).to_le_bytes()); // dlat
        rec.extend_from_slice(&((w[1].1 - w[0].1) as i16).to_le_bytes()); // dlon
    }
    rec
}

/// Pack one 36-byte v7 POI record (spec §7.3): absolute `int32 lat, int32 lon`, `u8 subtype`, `u8
/// name_len`, a 24-byte `0xFF`-padded name, and a `u16 hours_ref` (0-based hours-pool index, `0xFFFF`
/// = none). `name` is stored as-is (the caller pre-folds it to ≤ 24 ASCII bytes, as the packer does).
pub fn pack_poi_record(lat: i32, lon: i32, subtype: u8, name: &str, hours_ref: u16) -> [u8; POI_RECORD_LEN] {
    let mut rec = [0xFFu8; POI_RECORD_LEN];
    rec[0..4].copy_from_slice(&lat.to_le_bytes());
    rec[4..8].copy_from_slice(&lon.to_le_bytes());
    rec[8] = subtype;
    let bytes = name.as_bytes();
    let len = bytes.len().min(POI_NAME_LEN);
    rec[9] = len as u8;
    rec[10..10 + len].copy_from_slice(&bytes[..len]);
    // rec[10 + len .. 34] stays 0xFF (name pad); hours_ref goes at [34..36].
    rec[34..36].copy_from_slice(&hours_ref.to_le_bytes());
    rec
}

/// Pack POI records into one `chunk_size`-byte chunk (spec §7.3): the records back-to-back, a `0xFF`
/// subtype sentinel after the last, then `0xFF` padding — mirroring the packer's `pack_poi_chunk`.
pub fn pack_poi_chunk(records: &[[u8; POI_RECORD_LEN]], chunk_size: usize) -> Vec<u8> {
    let mut c = Vec::with_capacity(chunk_size);
    for r in records {
        c.extend_from_slice(r);
    }
    c.resize(chunk_size, 0xFF);
    c
}

/// One POI to place in a [`build_poi_map`] category: absolute `(lat, lon)` µdeg, its subtype id, its
/// (already-folded, ≤ 24-byte) name, and its `hours_ref` (0-based hours-pool index, `0xFFFF` = none).
/// Mirrors a serializer `PoiPoint`.
#[derive(Clone)]
pub struct PoiSpec {
    pub lat: i32,
    pub lon: i32,
    pub subtype: u8,
    pub name: String,
    pub hours_ref: u16,
}

/// Serialize one category's POIs into a per-category quadtree over `bbox` — the flat `u32` index +
/// its data chunks (spec §7.2/§7.3), built to walk **identically** to the reader/packer: a leaf
/// holds ≤ `chunk_size/36` records; an over-full leaf subdivides on floor-division midpoints in
/// NW/NE/SW/SE order (east/north of the midline is `>= mid`), stopping at the 10-µdeg recursion
/// floor. Returns `(index_bytes, node_count, chunk_bytes, chunk_count)`. The test-only mirror of
/// `obc-pack`'s `build_poi_tree` + `flatten_tree`, so the reader tests need no GEOS-linked packer.
fn serialize_poi_category(
    pois: &[PoiSpec],
    bbox: (i32, i32, i32, i32),
    chunk_size: usize,
) -> (Vec<u8>, u32, Vec<u8>, u32) {
    // A node of the recursively-built tree: a leaf (its records) or a branch (four children).
    enum PoiNode {
        Leaf(Vec<PoiSpec>),
        Branch(Box<[PoiNode; 4]>),
    }
    fn build(points: Vec<PoiSpec>, bbox: (i32, i32, i32, i32), capacity: usize) -> PoiNode {
        let (min_lon, min_lat, max_lon, max_lat) = bbox;
        if points.len() <= capacity || max_lon - min_lon < 10 || max_lat - min_lat < 10 {
            return PoiNode::Leaf(points);
        }
        let mid_lon = (min_lon + max_lon).div_euclid(2);
        let mid_lat = (min_lat + max_lat).div_euclid(2);
        // West is lon < mid, South is lat < mid — a point on a midline lands East/North (>= mid),
        // matching the packer's assignment so it stays inside its leaf's bbox for the query.
        let mut quads: [Vec<PoiSpec>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        for p in points {
            let east = p.lon >= mid_lon;
            let north = p.lat >= mid_lat;
            let q = match (north, east) {
                (true, false) => 0,  // NW
                (true, true) => 1,   // NE
                (false, false) => 2, // SW
                (false, true) => 3,  // SE
            };
            quads[q].push(p);
        }
        let boxes = [
            (min_lon, mid_lat, mid_lon, max_lat), // NW
            (mid_lon, mid_lat, max_lon, max_lat), // NE
            (min_lon, min_lat, mid_lon, mid_lat), // SW
            (mid_lon, min_lat, max_lon, mid_lat), // SE
        ];
        let [q0, q1, q2, q3] = quads;
        let [b0, b1, b2, b3] = boxes;
        PoiNode::Branch(Box::new([
            build(q0, b0, capacity),
            build(q1, b1, capacity),
            build(q2, b2, capacity),
            build(q3, b3, capacity),
        ]))
    }

    let capacity = chunk_size / POI_RECORD_LEN;
    let root = build(pois.to_vec(), bbox, capacity);

    // BFS-flatten exactly like the packer: children appended contiguously, chunk ids in BFS leaf
    // order, empty leaves → EMPTY_LEAF, branches → BRANCH_BIT | first-child index.
    let mut nodes: Vec<&PoiNode> = vec![&root];
    let mut first_child: Vec<usize> = vec![0];
    let mut i = 0;
    while i < nodes.len() {
        if let PoiNode::Branch(kids) = nodes[i] {
            first_child[i] = nodes.len();
            for k in kids.iter() {
                nodes.push(k);
                first_child.push(0);
            }
        }
        i += 1;
    }
    let mut index: Vec<u32> = Vec::with_capacity(nodes.len());
    let mut chunks: Vec<u8> = Vec::new();
    let mut chunk_count = 0u32;
    for (idx, node) in nodes.iter().enumerate() {
        match node {
            PoiNode::Branch(_) => index.push(BRANCH_BIT | first_child[idx] as u32),
            PoiNode::Leaf(pts) if pts.is_empty() => index.push(EMPTY_LEAF),
            PoiNode::Leaf(pts) => {
                index.push(chunk_count);
                let recs: Vec<[u8; POI_RECORD_LEN]> =
                    pts.iter().map(|p| pack_poi_record(p.lat, p.lon, p.subtype, &p.name, p.hours_ref)).collect();
                chunks.extend_from_slice(&pack_poi_chunk(&recs, chunk_size));
                chunk_count += 1;
            }
        }
    }
    let mut index_bytes = Vec::with_capacity(index.len() * 4);
    for n in &index {
        index_bytes.extend_from_slice(&n.to_le_bytes());
    }
    (index_bytes, index.len() as u32, chunks, chunk_count)
}

/// Build a full v8 `.obcm` with a **populated POI section** — the query-test analogue of
/// [`build_file`]. `bbox` is `(min_lon, min_lat, max_lon, max_lat)`; a minimal one-line geometry LOD
/// keeps the map valid; `pois_by_cat` maps a category id (1..=6) to the POIs to place there (each a
/// full per-category quadtree over `bbox`, `chunk_size`-byte chunks). Categories absent from the map
/// are written empty. An **empty hours pool** (`count 0`) follows at the tail — the query tests don't
/// exercise hours, and each `PoiSpec` carries its own `hours_ref` into its record. Use
/// [`build_poi_map_with_hours`] to bake a real pool (the detail-screen tests). The section is
/// assembled at its file-absolute offset so the reader's `walk_leaves`/`chunk_range` math resolves.
pub fn build_poi_map(bbox: (i32, i32, i32, i32), chunk_size: usize, pois_by_cat: &[(u8, Vec<PoiSpec>)]) -> Vec<u8> {
    build_poi_map_with_hours(bbox, chunk_size, pois_by_cat, &[])
}

/// Like [`build_poi_map`] but bakes a real **hours pool** of `hours_blobs` (spec §7.5) at the file
/// tail, with the directory's `hours_pool_offset`/`hours_pool_count` pointing at it. Each
/// [`PoiSpec`]'s `hours_ref` indexes into `hours_blobs` (`0xFFFF` = no hours). Used by the POI
/// detail-screen tests to exercise the reader's `poi_hours` lookup end to end through the app.
pub fn build_poi_map_with_hours(
    bbox: (i32, i32, i32, i32),
    chunk_size: usize,
    pois_by_cat: &[(u8, Vec<PoiSpec>)],
    hours_blobs: &[[u8; POI_HOURS_BLOB_LEN]],
) -> Vec<u8> {
    // A trivial single-leaf geometry LOD so the file is a valid map (the query never touches it).
    let styles: &[Style] = &[(1, 0, 0xFFFF, 1, 1)];
    let base = build_file(
        bbox,
        styles,
        &[LodSpec {
            max_mpp: f32::INFINITY,
            index: vec![0],
            chunks: vec![pad(pack_line(1, bbox.0, bbox.1, &[(0, 0)]), 64)],
            chunk_size: 64,
        }],
    );
    let poi_off = u32::from_le_bytes(base[32..36].try_into().unwrap()) as usize;

    // Lay out: [directory][cat index+chunks]*[hours pool] — categories in id order 1..=6, populated
    // ones first getting their index right after the directory. Compute each category's offsets.
    let mut payload = Vec::new(); // everything after the directory
    let mut cats: Vec<PoiCat> = Vec::new();
    let mut cursor = poi_off + poi_dir_len(); // absolute offset of the next category's index
    for id in 1..=POI_CATEGORY_COUNT {
        let pois = pois_by_cat.iter().find(|(c, _)| *c == id).map(|(_, v)| v.as_slice()).unwrap_or(&[]);
        if pois.is_empty() {
            // Empty category: its (zero-length) index "starts" at the cursor, no chunks.
            cats.push(PoiCat { category_id: id, index_offset: cursor as u32, node_count: 0, chunk_count: 0 });
            continue;
        }
        let (index_bytes, node_count, chunk_bytes, chunk_count) = serialize_poi_category(pois, bbox, chunk_size);
        cats.push(PoiCat { category_id: id, index_offset: cursor as u32, node_count, chunk_count });
        cursor += index_bytes.len() + chunk_bytes.len();
        payload.extend_from_slice(&index_bytes);
        payload.extend_from_slice(&chunk_bytes);
    }

    // The hours pool follows the last category's chunks (`cursor`); the directory points at it.
    let hours_pool_offset = cursor as u32;
    payload.extend_from_slice(&hours_pool(hours_blobs));

    let mut f = base[..poi_off].to_vec();
    f.extend_from_slice(&poi_directory(chunk_size as u16, &cats, hours_pool_offset, hours_blobs.len() as u16));
    f.extend_from_slice(&payload);
    // The populated POI section displaced `base`'s tail sections, so re-append the empty nav
    // section at the new tail and patch the header's nav offset (byte 36) to match.
    let nav_section_off = f.len();
    f[36..40].copy_from_slice(&(nav_section_off as u32).to_le_bytes());
    f.extend_from_slice(&empty_nav_directory(nav_section_off));
    f
}

/// Start a feature record: `style_id`, the `uint16` exterior point count, the i32 anchor
/// `(ax, ay)`, and the `flags` byte — the common prefix of every `pack_*` encoder.
fn feature_header(style_id: u8, point_count: u16, ax: i32, ay: i32, flags: u8) -> Vec<u8> {
    let mut v = Vec::new();
    v.push(style_id);
    v.extend_from_slice(&point_count.to_le_bytes());
    v.extend_from_slice(&ax.to_le_bytes());
    v.extend_from_slice(&ay.to_le_bytes());
    v.push(flags);
    v
}

/// Append 8-bit `(dx, dy)` deltas (one byte each).
fn push_deltas8(v: &mut Vec<u8>, deltas: &[(i8, i8)]) {
    for &(dx, dy) in deltas {
        v.push(dx as u8);
        v.push(dy as u8);
    }
}

/// Append 16-bit `(dx, dy)` deltas (little-endian, two bytes each).
fn push_deltas16(v: &mut Vec<u8>, deltas: &[(i16, i16)]) {
    for &(dx, dy) in deltas {
        v.extend_from_slice(&dx.to_le_bytes());
        v.extend_from_slice(&dy.to_le_bytes());
    }
}

/// Build a general multi-LOD `.obcm` (mirrors `serialize.py`). `bbox` is
/// `(min_lon, min_lat, max_lon, max_lat)`; `styles` are
/// `(id, z_index, color_rgb565, weight, priority)`; each [`LodSpec`] is one layer with
/// its own quadtree index and padded chunks. The header carries [`MARKER`] as the
/// marker color.
pub fn build_file(bbox: (i32, i32, i32, i32), styles: &[Style], lods: &[LodSpec]) -> Vec<u8> {
    let style_off = HEADER_LEN;

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

    // The POI section begins right after the LOD payload (`cursor` now points there); the empty
    // nav section follows it at the file tail.
    let poi_section_off = cursor;
    let poi_dir = empty_poi_directory(poi_section_off);
    let nav_section_off = poi_section_off + poi_dir.len();

    let mut f = obcm_header(bbox, style_off, lods.len() as u8, lod_tab_off, MARKER, poi_section_off, nav_section_off);
    f.extend_from_slice(&style_bytes);
    f.extend_from_slice(&table);
    f.extend_from_slice(&payload);
    f.extend_from_slice(&poi_dir);
    f.extend_from_slice(&empty_nav_directory(nav_section_off));
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
    let style_off = HEADER_LEN;
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

    // The POI section begins right after the index + all chunk bytes; the empty nav section
    // follows it.
    let poi_section_off = index_off + idx_bytes.len() + chunks.len() * chunk_size;
    let poi_dir = empty_poi_directory(poi_section_off);
    let nav_section_off = poi_section_off + poi_dir.len();
    // marker unused here → 0
    let mut f = obcm_header(bbox, style_off, 1, lod_tab_off, 0, poi_section_off, nav_section_off);
    f.extend_from_slice(&style_bytes);
    f.extend_from_slice(&table);
    f.extend_from_slice(&idx_bytes);
    for c in chunks {
        f.extend_from_slice(&pad(c, chunk_size));
    }
    f.extend_from_slice(&poi_dir);
    f.extend_from_slice(&empty_nav_directory(nav_section_off));
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
    // line, 8-bit deltas — no flags set
    let mut v = feature_header(style_id, (1 + deltas.len()) as u16, ax, ay, 0);
    push_deltas8(&mut v, deltas);
    v
}

/// A line feature with 16-bit deltas (flag bit 0).
pub fn pack_line16(style_id: u8, ax: i32, ay: i32, deltas: &[(i16, i16)]) -> Vec<u8> {
    // line, 16-bit deltas
    let mut v = feature_header(style_id, (1 + deltas.len()) as u16, ax, ay, FEATURE_FLAG_16BIT);
    push_deltas16(&mut v, deltas);
    v
}

/// A hole-free polygon with 8-bit deltas. `deltas` are the points after the anchor, so
/// the stored exterior point count is `1 + deltas.len()`.
pub fn pack_poly(style_id: u8, ax: i32, ay: i32, deltas: &[(i8, i8)]) -> Vec<u8> {
    // polygon, no holes, 8-bit deltas
    let mut v = feature_header(style_id, (1 + deltas.len()) as u16, ax, ay, FEATURE_FLAG_POLYGON);
    push_deltas8(&mut v, deltas);
    v
}

/// A polygon with one hole, 8-bit deltas. Hole vertices are all deltas (first relative
/// to the anchor), so its stored point count == `hole_deltas.len()`.
pub fn pack_poly_hole(style_id: u8, ax: i32, ay: i32, ext_deltas: &[(i8, i8)], hole_deltas: &[(i8, i8)]) -> Vec<u8> {
    // polygon | has-holes, 8-bit deltas
    let mut v =
        feature_header(style_id, (1 + ext_deltas.len()) as u16, ax, ay, FEATURE_FLAG_POLYGON | FEATURE_FLAG_HOLES);
    push_deltas8(&mut v, ext_deltas);
    v.push(1u8); // hole count
    v.extend_from_slice(&(hole_deltas.len() as u16).to_le_bytes());
    push_deltas8(&mut v, hole_deltas);
    v
}

/// A hole-free polygon with 16-bit deltas (flag bit 0) — the polygon analogue of [`pack_line16`].
/// Lets a test build a polygon whose vertices span more than ±127 µdeg per delta (e.g. a screen-
/// sized square for the renderer's edge-fill tests), which the 8-bit [`pack_poly`] can't express.
/// The stored exterior point count is `1 + deltas.len()`.
pub fn pack_poly16(style_id: u8, ax: i32, ay: i32, deltas: &[(i16, i16)]) -> Vec<u8> {
    // polygon | 16-bit deltas
    let mut v = feature_header(style_id, (1 + deltas.len()) as u16, ax, ay, FEATURE_FLAG_POLYGON | FEATURE_FLAG_16BIT);
    push_deltas16(&mut v, deltas);
    v
}

/// A polygon with `holes.len()` 8-bit-delta holes (each its own delta list). Generalises
/// [`pack_poly_hole`] so a test can pack *more rings than the reader's `MAX_FEAT_RINGS` scratch
/// holds* and assert the past-capacity rings are dropped (issue #96, reader item 1). The
/// exterior's stored point count is `1 + ext_deltas.len()`; each hole's stored count is its own
/// `hole.len()` (every hole vertex is a delta, the first relative to the anchor).
pub fn pack_poly_holes(style_id: u8, ax: i32, ay: i32, ext_deltas: &[(i8, i8)], holes: &[Vec<(i8, i8)>]) -> Vec<u8> {
    // polygon | has-holes, 8-bit deltas
    let mut v =
        feature_header(style_id, (1 + ext_deltas.len()) as u16, ax, ay, FEATURE_FLAG_POLYGON | FEATURE_FLAG_HOLES);
    push_deltas8(&mut v, ext_deltas);
    v.push(holes.len() as u8); // hole count
    for hole in holes {
        v.extend_from_slice(&(hole.len() as u16).to_le_bytes());
        push_deltas8(&mut v, hole);
    }
    v
}

// ---------------------------------------------------------------------------
// Deterministic bench fixture (issue #327)
// ---------------------------------------------------------------------------

/// Bounding box of [`build_bench_map`] (µdeg, `(min_lon, min_lat, max_lon, max_lat)`): a 54 000 µdeg
/// square near 47° N (≈ 6 km of latitude, ≈ 4.1 km of ground longitude at that latitude's aspect).
/// Divisible by 8 on both axes so the depth-3 quadtree subdivides into uniform leaves. Public so the
/// bench aims its camera at the fixture's center without re-deriving it.
pub const BENCH_BBOX: (i32, i32, i32, i32) = (8_500_000, 47_000_000, 8_554_000, 47_054_000);

/// A quadtree-node bbox in the builders' `(min_lon, min_lat, max_lon, max_lat)` µdeg spelling.
type LeafBox = (i32, i32, i32, i32);

/// A tiny inline xorshift64* PRNG — deterministic and dependency-free (no `rand`, no time/OS
/// input), so [`build_bench_map`] produces the *same bytes on every machine, forever*. The bench's
/// committed frame hashes depend on that.
struct BenchRng(u64);

impl BenchRng {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }

    /// Uniform-ish `i32` in `[lo, hi)` (the modulo bias is irrelevant for fixture generation).
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        debug_assert!(hi > lo);
        lo + (self.next_u32() % (hi - lo) as u32) as i32
    }
}

/// Build a **complete, uniform-depth** breadth-first quadtree over `bbox` — the generalization of
/// [`build_priority_tree`]'s hand-laid 9-node index to depth `k`. Nodes are laid out exactly like
/// the packer flattens them: level by level, each branch pointing at its four children's block in
/// the next level, children in NW/NE/SW/SE order with floor-division midpoints (matching the
/// reader's `walk_leaves` split). Every level-`depth` node is a leaf holding chunk id = its
/// breadth-first position, so the caller packs one chunk per leaf in returned-bbox order.
///
/// Returns `(index, leaf_bboxes)`: the flat `u32` node array and each leaf's bbox in chunk-id order.
fn uniform_quadtree(bbox: LeafBox, depth: u32) -> (Vec<u32>, Vec<LeafBox>) {
    let mut index = Vec::new();
    let mut boxes = vec![bbox]; // current level's node bboxes, breadth-first
    let mut level_start = 0usize; // flat index where the current level begins
    for _ in 0..depth {
        // Every node above the leaf level is a branch; node j's children start at
        // `child_start + 4*j` — the packer's breadth-first child-block layout.
        let child_start = level_start + boxes.len();
        for j in 0..boxes.len() {
            index.push(BRANCH_BIT | (child_start + 4 * j) as u32);
        }
        let mut next = Vec::with_capacity(boxes.len() * 4);
        for &(min_lon, min_lat, max_lon, max_lat) in &boxes {
            // Floor-division midpoints + NW/NE/SW/SE order — must match the reader's `walk_leaves`
            // subdivision or the leaf bboxes (and thus every anchor base) disagree.
            let mid_lon = (min_lon + max_lon).div_euclid(2);
            let mid_lat = (min_lat + max_lat).div_euclid(2);
            next.push((min_lon, mid_lat, mid_lon, max_lat)); // NW
            next.push((mid_lon, mid_lat, max_lon, max_lat)); // NE
            next.push((min_lon, min_lat, mid_lon, mid_lat)); // SW
            next.push((mid_lon, min_lat, max_lon, mid_lat)); // SE
        }
        level_start = child_start;
        boxes = next;
    }
    for cid in 0..boxes.len() {
        index.push(cid as u32);
    }
    (index, boxes)
}

/// One coarse-LOD chunk: a leaf-covering land backdrop, a lake on roughly half the leaves, and 225
/// short 3-point road stubs cycling the three line styles. 16 leaves × ~226 features ≈ 3 620 —
/// deliberately **over `obc_render::MAX_SPANS` (3072)** so a full-map overview scene saturates the
/// span buffer and exercises the priority-drop path. ~3.7 KB, under the 4 KB chunk size.
fn bench_coarse_chunk(rng: &mut BenchRng, leaf: LeafBox) -> Vec<u8> {
    let (min_lon, min_lat, max_lon, max_lat) = leaf;
    let (w, h) = (max_lon - min_lon, max_lat - min_lat);
    let mut c = Vec::new();
    // Land backdrop covering the leaf (16-bit deltas: the leaf spans 13 500 µdeg).
    c.extend(pack_poly16(1, 0, 0, &[(w as i16, 0), (0, h as i16), (-w as i16, 0)]));
    // A lake on roughly half the leaves.
    if rng.range(0, 2) == 0 {
        let (lw, lh) = (rng.range(1_500, 4_000), rng.range(1_500, 4_000));
        let (ax, ay) = (rng.range(0, w - lw), rng.range(0, h - lh));
        c.extend(pack_poly16(2, ax, ay, &[(lw as i16, 0), (0, lh as i16), (-lw as i16, 0)]));
    }
    for k in 0..225 {
        let style = [5u8, 4, 3][k % 3];
        let (ax, ay) = (rng.range(130, w - 130), rng.range(0, h - 260));
        let d0 = (rng.range(-120, 121) as i8, rng.range(1, 121) as i8);
        let d1 = (rng.range(-120, 121) as i8, rng.range(1, 121) as i8);
        c.extend(pack_line(style, ax, ay, &[d0, d1]));
    }
    c
}

/// One fine-LOD chunk: a leaf-covering backdrop, an occasional lake, small 8-bit-delta buildings
/// (every fourth with a hole), a few long 16-bit-delta roads, and a batch of short 8-bit paths of
/// varying vertex counts — the riding-zoom feature mix. ≤ ~2 KB, well under the 4 KB chunk size.
fn bench_fine_chunk(rng: &mut BenchRng, leaf: LeafBox) -> Vec<u8> {
    let (min_lon, min_lat, max_lon, max_lat) = leaf;
    let (w, h) = (max_lon - min_lon, max_lat - min_lat);
    let mut c = Vec::new();
    // Land backdrop covering the leaf — a polygon big enough to *force* 16-bit deltas (6 750 µdeg).
    c.extend(pack_poly16(1, 0, 0, &[(w as i16, 0), (0, h as i16), (-w as i16, 0)]));
    // A lake on roughly a third of the leaves.
    if rng.range(0, 3) == 0 {
        let (lw, lh) = (rng.range(500, 1_800), rng.range(500, 1_800));
        let (ax, ay) = (rng.range(0, w - lw), rng.range(0, h - lh));
        c.extend(pack_poly16(2, ax, ay, &[(lw as i16, 0), (0, lh as i16), (-lw as i16, 0)]));
    }
    // Buildings: small 8-bit-delta rectangles; every fourth big-enough one carries a hole.
    for k in 0..rng.range(8, 16) {
        let (bw, bh) = (rng.range(40, 120), rng.range(40, 120));
        let (ax, ay) = (rng.range(0, w - bw), rng.range(0, h - bh));
        let ext = [(bw as i8, 0), (0, bh as i8), (-bw as i8, 0)];
        if k % 4 == 3 && bw > 60 && bh > 60 {
            let (hw, hh) = (bw - 40, bh - 40);
            let hole = [(20i8, 20i8), (hw as i8, 0), (0, hh as i8), (-hw as i8, 0)];
            c.extend(pack_poly_hole(6, ax, ay, &ext, &hole));
        } else {
            c.extend(pack_poly(6, ax, ay, &ext));
        }
    }
    // Long roads: 16-bit random walks crossing the leaf, cycling major/secondary/minor.
    for k in 0..rng.range(3, 7) {
        let style = [5u8, 4, 3][k as usize % 3];
        let mut deltas = Vec::new();
        for _ in 0..rng.range(4, 9) {
            deltas.push((rng.range(-1_400, 1_401) as i16, rng.range(-1_400, 1_401) as i16));
        }
        let (ax, ay) = (rng.range(0, w), rng.range(0, h));
        c.extend(pack_line16(style, ax, ay, &deltas));
    }
    // Short paths: 8-bit walks of varying vertex counts (weight-1 minor exercises the Polyline path).
    for k in 0..rng.range(10, 20) {
        let style = if k % 3 == 0 { 4 } else { 3 };
        let mut deltas = Vec::new();
        for _ in 0..rng.range(3, 20) {
            deltas.push((rng.range(-100, 101) as i8, rng.range(-100, 101) as i8));
        }
        let (ax, ay) = (rng.range(0, w), rng.range(0, h));
        c.extend(pack_line(style, ax, ay, &deltas));
    }
    // A "village" cluster within ~700 µdeg of **every leaf corner**. The bench's riding camera sits
    // at the map center — the shared corner of the four center leaves — so corner clusters guarantee
    // the ~0.5 m/px scenes draw a realistic feature load instead of a near-empty frame, whichever
    // leaves the view straddles.
    for &(qx, qy) in &[(0, 0), (1, 0), (0, 1), (1, 1)] {
        for _ in 0..rng.range(5, 10) {
            let (bw, bh) = (rng.range(40, 110), rng.range(40, 110));
            let ax = if qx == 0 { rng.range(0, 700) } else { rng.range(w - 700 - bw, w - bw) };
            let ay = if qy == 0 { rng.range(0, 700) } else { rng.range(h - 700 - bh, h - bh) };
            c.extend(pack_poly(6, ax, ay, &[(bw as i8, 0), (0, bh as i8), (-bw as i8, 0)]));
        }
        for k in 0..rng.range(3, 6) {
            let style = if k == 0 { 4 } else { 3 };
            let mut deltas = Vec::new();
            for _ in 0..rng.range(5, 14) {
                deltas.push((rng.range(-100, 101) as i8, rng.range(-100, 101) as i8));
            }
            let ax = if qx == 0 { rng.range(0, 700) } else { rng.range(w - 700, w) };
            let ay = if qy == 0 { rng.range(0, 700) } else { rng.range(h - 700, h) };
            c.extend(pack_line(style, ax, ay, &deltas));
        }
    }
    c
}

/// The deterministic **bench fixture** (issue #327): a two-LOD OBCM v8 map whose bytes are
/// identical on every machine, forever — the `obc-bench` frame hashes are computed over renders of
/// it, so any byte drift here invalidates the committed golden file.
///
/// Shape:
/// - **Coarse LOD** (`max_mpp = ∞`): a uniform depth-2 quadtree (16 leaves, one 4 KB chunk each)
///   holding ≈ 3 620 features — over `obc_render::MAX_SPANS` (3072) in a full-map view, so the
///   overview scenes saturate the span buffer and take the priority-drop path.
/// - **Fine LOD** (`max_mpp = 2.0`): a real depth-3 multi-chunk quadtree (64 leaves, one chunk
///   each), built breadth-first exactly like the packer, holding the riding-zoom mix: per-leaf
///   backdrop polygons (16-bit deltas), buildings with and without holes, long 16-bit roads and
///   short 8-bit paths of varying vertex counts.
/// - **6 styles** spanning priorities 1–4 and z-indices −10…4, including an obvious backdrop
///   (lowest z) and line weights 1, 2 and 3 (weight 1 exercises the `Polyline` path, ≥ 2 the
///   span-stroke path).
///
/// Geometry is generated by the inline seeded [`BenchRng`] and packed through the same `pack_*`
/// encoders the format tests use, so a format layout bump lands here automatically.
pub fn build_bench_map() -> Vec<u8> {
    const CHUNK: usize = 4096; // the packer's default — every chunk stays cacheable
    let styles: [Style; 6] = [
        (1, -10, 0xD6DA, 0, 1), // land backdrop — lowest z, fills under everything
        (2, -5, 0x64DD, 0, 2),  // water
        (6, 1, 0x9CD3, 0, 3),   // buildings
        (5, 4, 0xFC00, 3, 2),   // major road, weight 3
        (4, 3, 0xFEA0, 2, 3),   // secondary road, weight 2
        (3, 2, 0xFFFF, 1, 4),   // minor path, weight 1 (Polyline path)
    ];

    let mut rng = BenchRng(0x0BC0_0327_D00D_F00D); // hard-coded seed — never change casually
    let (coarse_index, coarse_leaves) = uniform_quadtree(BENCH_BBOX, 2);
    let coarse_chunks: Vec<Vec<u8>> =
        coarse_leaves.iter().map(|&leaf| pad(bench_coarse_chunk(&mut rng, leaf), CHUNK)).collect();
    let (fine_index, fine_leaves) = uniform_quadtree(BENCH_BBOX, 3);
    let fine_chunks: Vec<Vec<u8>> =
        fine_leaves.iter().map(|&leaf| pad(bench_fine_chunk(&mut rng, leaf), CHUNK)).collect();

    build_file(
        BENCH_BBOX,
        &styles,
        &[
            // Strictly decreasing max_mpp, coarse (∞) first — the LOD-table ordering the reader expects.
            LodSpec { max_mpp: f32::INFINITY, index: coarse_index, chunks: coarse_chunks, chunk_size: CHUNK },
            LodSpec { max_mpp: 2.0, index: fine_index, chunks: fine_chunks, chunk_size: CHUNK },
        ],
    )
}

/// A line whose **declared** exterior point count (`decl_count`, the `uint16` in the feature
/// header) is set independently of the `deltas` actually written. The reader trusts that count and
/// loops `decl_count - 1` deltas; a `decl_count` *larger* than `1 + deltas.len()` forges a header
/// that runs past the bytes present — and, sized right, past the reader's `MAX_FEAT_PTS` scratch —
/// letting a test drive the scratch-overflow + truncated-ring guards of issue #96 (reader items 1
/// and 4) that the count-correct [`pack_line`] never reaches. 8-bit deltas.
pub fn pack_line_decl(style_id: u8, ax: i32, ay: i32, decl_count: u16, deltas: &[(i8, i8)]) -> Vec<u8> {
    // line, 8-bit deltas — no flags. Count is forged, not derived from `deltas`.
    let mut v = feature_header(style_id, decl_count, ax, ay, 0);
    push_deltas8(&mut v, deltas);
    v
}

/// A hole-free polygon whose **declared** exterior point count is forged independently of the
/// `deltas` written — the polygon analogue of [`pack_line_decl`], used to overrun the reader's
/// `MAX_FEAT_PTS` exterior scratch with one big feature (issue #96, reader item 1). 8-bit deltas.
pub fn pack_poly_decl(style_id: u8, ax: i32, ay: i32, decl_count: u16, deltas: &[(i8, i8)]) -> Vec<u8> {
    // polygon, no holes, 8-bit deltas. Count is forged, not derived from `deltas`.
    let mut v = feature_header(style_id, decl_count, ax, ay, FEATURE_FLAG_POLYGON);
    push_deltas8(&mut v, deltas);
    v
}
