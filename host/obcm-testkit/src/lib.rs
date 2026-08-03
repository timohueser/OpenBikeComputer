//! Hand-written OBCM v13 byte builder shared by the `obc-reader` and `obc-render`
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
//! junction records plus a chunked edge pool. **v9** reworked §8 (28-byte nav
//! directory + profile table, 17-byte neighbor entries, pinned 512-byte nav chunks).
//! **v10** grows the style record 6 → 8 bytes: a `dashed` flag bit + an optional
//! `color2` u16 (spec §2, epic #556). **v11** packs geometry chunks tight behind a
//! per-LOD offset table ([`chunk_region`], [`seal`]) and reorders the feature header
//! — `flags` to byte 1, then either the 7-byte compact or 12-byte wide layout
//! (issue #1009). **v12** is a §8-only bump (directional ascent + profile climb weight,
//! issue #1073) that the geometry builders here never see. **v13** appends an optional
//! `int16` **level** behind the feature header under flag bit 4 ([`pack_line_level`]) and
//! defines style-flag bit 6 (issue #1105); [`pack_poly_level`] and [`pack_line_flags`]
//! author the two shapes §5.2 requires a reader to *refuse*, which is the half of a wire
//! rule an oracle is uniquely placed to state. [`build_file`]/[`build_priority_tree`]
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
//! Style records are `(id, z_index, color_rgb565, weight, priority, dashed, color2)`; feature
//! encoders ([`pack_line`], [`pack_line16`], [`pack_poly`], [`pack_poly_hole`]) return one
//! packed feature, [`seal`] closes a chunk with its single `0xFF` sentinel, and [`chunk_region`]
//! lays sealed chunks out behind their offset table.

/// Hand-split OBCA volume-set fixtures (`OBCA_Spec.md` §5) — a monolithic file and the byte-level
/// split of the same data into a manifest plus shards, so a differential render can assert the two
/// are pixel-identical.
pub mod set;

/// A style record (OBCM §2, 8 bytes on the wire): `(id, z_index, color_rgb565, weight, priority,
/// dashed, color2)`. `dashed` sets flag bit 2; `color2 = Some(_)` sets flag bit 3 and writes the
/// secondary color, `None` writes `0x0000` with the bit clear.
pub type Style = (u8, i8, u16, u8, u8, bool, Option<u16>);

// Normative constants only: byte assembly below remains hand-written and never calls a production
// serializer/parser, preserving the testkit as an independent oracle.
pub use obc_formats::obcm::{
    BRANCH_BIT, EMPTY_LEAF, HEADER_LEN, LOD_ENTRY_LEN, NAV_CHUNK_SIZE, NAV_DIR_LEN, NAV_EDGE_FIXED_LEN,
    NAV_NEIGHBOR_LEN, NAV_NODE_FIXED_LEN, NAV_PROFILE_LEN, NAV_PROFILE_NAME_LEN, POI_CATEGORY_COUNT, POI_CAT_ENTRY_LEN,
    POI_CHUNK_SIZE, POI_DIR_POOL_FIELDS_LEN, POI_HOURS_BLOB_LEN, POI_NAME_LEN, POI_RECORD_LEN,
};
use obc_formats::obcm::{
    CHUNK_END, FEATURE_FLAG_16BIT, FEATURE_FLAG_HOLES, FEATURE_FLAG_POLYGON, FEATURE_FLAG_WIDE, FEATURE_HAS_LEVEL_BIT,
    MAGIC, STYLE_CONTOUR_INDEX_BIT, STYLE_DASHED_BIT, STYLE_FIXED_WIDTH_BIT, STYLE_HAS_COLOR2_BIT, STYLE_PRIORITY_MASK,
    STYLE_TERRAIN_LAYER_BIT, VERSION,
};
/// Distinctive (non-default) marker color baked into [`build_file`]'s header, so the
/// reader's round-trip test is meaningful.
pub const MARKER: u16 = 0xABCD;

/// One LOD layer: its quadtree index (flat u32 nodes) and its data chunks. Each chunk is the tight
/// v11 byte string [`seal`] produces — `chunk_size` bounds it, it no longer pads to it.
pub struct LodSpec {
    pub max_mpp: f32,
    pub index: Vec<u32>,
    pub chunks: Vec<Vec<u8>>,
    pub chunk_size: usize,
}

/// Pack the style table (OBCM §2): a count byte followed by one 8-byte record per style
/// (`id, z, color_le, weight, flags, color2_le`). `flags` = `(priority-1) & STYLE_PRIORITY_MASK`,
/// plus bit 2 when `dashed` and bit 3 when `color2` is `Some`. `color2` writes its RGB565 value when
/// present, else `0x0000` (ignored by the reader when bit 3 is clear). Shared by both file builders.
fn style_table(styles: &[Style]) -> Vec<u8> {
    style_table_flagged(&styles.iter().map(|&s| (s, 0u8)).collect::<Vec<_>>())
}

/// [`style_table`] with each record's `extra` flag bits OR-ed into its flags byte — the upper-bit
/// style properties the tuple has no field for (fixed width, terrain layer, contour index; §2 bits
/// 4-6). Kept as a raw byte rather than three more tuple slots: only the bench fixture authors one,
/// and the whole point of the testkit is that it writes the bytes the spec names.
fn style_table_flagged(styles: &[(Style, u8)]) -> Vec<u8> {
    let mut style_bytes = vec![styles.len() as u8];
    for &((id, z, color, weight, priority, dashed, color2), extra) in styles {
        let mut flags = ((priority - 1) & STYLE_PRIORITY_MASK) | extra;
        if dashed {
            flags |= STYLE_DASHED_BIT;
        }
        if color2.is_some() {
            flags |= STYLE_HAS_COLOR2_BIT;
        }
        style_bytes.push(id);
        style_bytes.push(z as u8);
        style_bytes.extend_from_slice(&color.to_le_bytes());
        style_bytes.push(weight);
        style_bytes.push(flags);
        style_bytes.extend_from_slice(&color2.unwrap_or(0).to_le_bytes());
    }
    style_bytes
}

/// The 40-byte OBCM header (§1), shared by both file builders. The version byte is `VERSION`,
/// so this builds whatever the reader currently reads — the length is asserted against
/// `HEADER_LEN` below rather than trusted to this comment.
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
    f.extend_from_slice(&MAGIC);
    f.push(VERSION);
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
    assert_eq!(f.len(), HEADER_LEN, "header length follows the normative constant");
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

/// Pack one §8.6 profile record (56 bytes, v12): a `0xFF`-padded 12-byte name + 32 highway + 8
/// surface multipliers (`u8` 1/16 fixed-point) + the `climb_weight` byte + 3 reserved zero bytes.
/// `name` is truncated to 12 bytes.
pub fn nav_profile_record(name: &str, highway: [u8; 32], surface: [u8; 8], climb_weight: u8) -> Vec<u8> {
    let mut rec = Vec::with_capacity(NAV_PROFILE_LEN);
    let nb = name.as_bytes();
    let n = nb.len().min(NAV_PROFILE_NAME_LEN);
    rec.extend_from_slice(&nb[..n]);
    rec.resize(NAV_PROFILE_NAME_LEN, 0xFF);
    rec.extend_from_slice(&highway);
    rec.extend_from_slice(&surface);
    rec.push(climb_weight);
    rec.resize(NAV_PROFILE_LEN, 0); // the reserved tail is zero, not 0xFF — it is not a padded name
    assert_eq!(rec.len(), NAV_PROFILE_LEN);
    rec
}

/// A minimal §8.6 profile table: one profile ("Default", every multiplier 16 = 1.0×, climb-blind),
/// 56 bytes — enough to satisfy the reader's "1..=8 profiles, always present" rule.
pub fn default_nav_profile_table() -> Vec<u8> {
    nav_profile_record("Default", [16; 32], [16; 8], 0)
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

/// One v12 §8.3 neighbor entry for [`pack_nav_record`]: `(neighbor_id, lat, lon, edge_id, cost_m,
/// way_kind, ascent_m)`. `lat`/`lon` are the neighbor's **absolute** µdeg coords
/// ([`pack_nav_record`] stores the `i16` delta from the owning record's own coord); `cost_m` must
/// fit `u16`; `ascent_m` is the climb of riding **toward** this neighbor, so the two entries of one
/// edge legitimately differ in it.
pub type NavNeighborSpec = (u32, i32, i32, u32, u32, u8, u16);

/// Pack one variable-length v12 §8.3 junction record: `lat i32, lon i32, node_id u32, degree u8`,
/// then one 17-byte entry per neighbor (`id u32, dlat i16, dlon i16, edge_id u32, cost_m u16,
/// way_kind u8, ascent_m u16`). The record head coords are absolute µdeg (lat first); each
/// neighbor's coord is stored as an `i16` delta from this record's own `lat`/`lon`.
pub fn pack_nav_record(lat: i32, lon: i32, node_id: u32, neighbors: &[NavNeighborSpec]) -> Vec<u8> {
    let mut rec = Vec::with_capacity(NAV_NODE_FIXED_LEN + neighbors.len() * NAV_NEIGHBOR_LEN);
    rec.extend_from_slice(&lat.to_le_bytes());
    rec.extend_from_slice(&lon.to_le_bytes());
    rec.extend_from_slice(&node_id.to_le_bytes());
    rec.push(neighbors.len() as u8);
    for &(id, nlat, nlon, edge_id, cost_m, way_kind, ascent_m) in neighbors {
        rec.extend_from_slice(&id.to_le_bytes());
        rec.extend_from_slice(&((nlat - lat) as i16).to_le_bytes());
        rec.extend_from_slice(&((nlon - lon) as i16).to_le_bytes());
        rec.extend_from_slice(&edge_id.to_le_bytes());
        rec.extend_from_slice(&(cost_m as u16).to_le_bytes());
        rec.push(way_kind);
        rec.extend_from_slice(&ascent_m.to_le_bytes());
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
    let styles: &[Style] = &[(1, 0, 0xFFFF, 1, 1, false, None)];
    let base = build_file(
        bbox,
        styles,
        &[LodSpec {
            max_mpp: f32::INFINITY,
            index: vec![0],
            chunks: vec![seal(pack_line(1, bbox.0, bbox.1, &[(0, 0)]), 64)],
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

/// Start a feature record (OBCM v11 §5): `style_id`, `flags`, then either the **compact** fields
/// (`point_count u8`, anchor `u16` ×2) or the **wide** ones (`point_count u16`, anchor `i32` ×2) with
/// [`FEATURE_FLAG_WIDE`] set — the common prefix of every `pack_*` encoder.
///
/// The form is picked by the same rule the packer uses, so a caller's ordinary small-anchor feature
/// exercises the compact path and one anchored on a real µdeg coordinate (or holding more than 255
/// vertices) exercises the wide escape. Note `flags` moved to byte 1 in v11: a reader must know the
/// `WIDE` bit before it can know the header's width.
fn feature_header(style_id: u8, point_count: u16, ax: i32, ay: i32, flags: u8) -> Vec<u8> {
    let compact = |v: i32| (0..=u16::MAX as i32).contains(&v);
    let wide = point_count > u8::MAX as u16 || !compact(ax) || !compact(ay);
    let mut v = Vec::new();
    v.push(style_id);
    v.push(if wide { flags | FEATURE_FLAG_WIDE } else { flags });
    if wide {
        v.extend_from_slice(&point_count.to_le_bytes());
        v.extend_from_slice(&ax.to_le_bytes());
        v.extend_from_slice(&ay.to_le_bytes());
    } else {
        v.push(point_count as u8);
        v.extend_from_slice(&(ax as u16).to_le_bytes());
        v.extend_from_slice(&(ay as u16).to_le_bytes());
    }
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

/// Build a general multi-LOD `.obcm` (mirrors `serialize.rs`). `bbox` is
/// `(min_lon, min_lat, max_lon, max_lat)`; `styles` are
/// `(id, z_index, color_rgb565, weight, priority, dashed, color2)`; each [`LodSpec`] is one layer
/// with its own quadtree index and padded chunks. The header carries [`MARKER`] as the
/// marker color.
pub fn build_file(bbox: (i32, i32, i32, i32), styles: &[Style], lods: &[LodSpec]) -> Vec<u8> {
    build_file_flagged(bbox, &styles.iter().map(|&s| (s, 0u8)).collect::<Vec<_>>(), lods)
}

/// [`build_file`] with per-style **extra flag bits** (§2 bits 4-6: fixed width, terrain layer,
/// contour index) — the upper style properties the [`Style`] tuple has no field for. Every other
/// byte is identical, and `extra = 0` reproduces [`build_file`] exactly.
pub fn build_file_flagged(bbox: (i32, i32, i32, i32), styles: &[(Style, u8)], lods: &[LodSpec]) -> Vec<u8> {
    let style_off = HEADER_LEN;

    let style_bytes = style_table_flagged(styles);

    let lod_tab_off = style_off + style_bytes.len();
    let mut cursor = lod_tab_off + lods.len() * LOD_ENTRY_LEN;
    let mut table = Vec::new();
    let mut payload = Vec::new();
    for lod in lods {
        let idx_off = cursor;
        let mut idx_bytes = Vec::new();
        for &node in &lod.index {
            idx_bytes.extend_from_slice(&node.to_le_bytes());
        }
        for c in &lod.chunks {
            assert!(c.len() <= lod.chunk_size, "chunk {} exceeds chunk_size {}", c.len(), lod.chunk_size);
        }
        let chunk_bytes = chunk_region(&lod.chunks);
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
/// `styles` are `(id, z, color, weight, priority, dashed, color2)`. The marker color is unused here,
/// so it is 0.
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
    let index_off = lod_tab_off + LOD_ENTRY_LEN; // one LOD entry

    // Quadtree (9 nodes). Root branch -> [NW=branch@5, NE=chunk 4, SW/SE empty]; NW's four
    // children (idx 5..8) -> chunks 0,1,2,3. Walk order NW(→0,1,2,3) then NE(→4): the four
    // early chunks are all visited before the late one.
    let index: [u32; 9] = [BRANCH_BIT | 1, BRANCH_BIT | 5, 4, EMPTY_LEAF, EMPTY_LEAF, 0, 1, 2, 3];
    let mut idx_bytes = Vec::new();
    for node in index {
        idx_bytes.extend_from_slice(&node.to_le_bytes());
    }
    // Chunk data in chunk-id order: 0..3 = NW leaves, 4 = NE. Sealed + laid out with their offset
    // table, the v11 §5 region.
    let [nw0, nw1, nw2, nw3] = nw_chunks;
    let chunks: Vec<Vec<u8>> = [nw0, nw1, nw2, nw3, ne_chunk].into_iter().map(|c| seal(c, chunk_size)).collect();
    let chunk_bytes = chunk_region(&chunks);

    // LOD entry: max_mpp=+inf, index_off, node_count, chunk_size, chunk_count.
    let mut table = Vec::new();
    table.extend_from_slice(&f32::INFINITY.to_le_bytes());
    table.extend_from_slice(&(index_off as u32).to_le_bytes());
    table.extend_from_slice(&(index.len() as u32).to_le_bytes());
    table.extend_from_slice(&(chunk_size as u16).to_le_bytes());
    table.extend_from_slice(&(chunks.len() as u32).to_le_bytes());

    // The POI section begins right after the index + the chunk region; the empty nav section
    // follows it.
    let poi_section_off = index_off + idx_bytes.len() + chunk_bytes.len();
    let poi_dir = empty_poi_directory(poi_section_off);
    let nav_section_off = poi_section_off + poi_dir.len();
    // marker unused here → 0
    let mut f = obcm_header(bbox, style_off, 1, lod_tab_off, 0, poi_section_off, nav_section_off);
    f.extend_from_slice(&style_bytes);
    f.extend_from_slice(&table);
    f.extend_from_slice(&idx_bytes);
    f.extend_from_slice(&chunk_bytes);
    f.extend_from_slice(&poi_dir);
    f.extend_from_slice(&empty_nav_directory(nav_section_off));
    f
}

/// Close a v11 geometry chunk: append the **one** trailing `0xFF` [`CHUNK_END`] sentinel that ends
/// its feature stream (spec §5), asserting the sealed chunk still fits `capacity` — the LOD's
/// declared `Chunk Size`, which v11 uses as a bound rather than a stride. v10's `pad` filled the rest
/// of the chunk with `0xFF`; tight chunks make that padding the thing the format got rid of.
///
/// A test that wants an *unsealed* chunk (no sentinel — malformed in v11) simply skips this.
///
/// The still-fixed-stride sections (POI §7.3, nav §8.3/§8.4) keep [`pad`].
pub fn seal(mut chunk: Vec<u8>, capacity: usize) -> Vec<u8> {
    chunk.push(CHUNK_END);
    assert!(chunk.len() <= capacity, "sealed chunk {} exceeds chunk_size {}", chunk.len(), capacity);
    chunk
}

/// Right-pad a **fixed-stride** chunk to `size` bytes with `0xFF` (the filler the reader skips):
/// the POI §7.3 and nav §8.3/§8.4 chunks, which v11 left alone. Geometry chunks are tight — they
/// want [`seal`].
pub fn pad(mut chunk: Vec<u8>, size: usize) -> Vec<u8> {
    assert!(chunk.len() <= size, "chunk {} exceeds chunk_size {}", chunk.len(), size);
    chunk.resize(size, CHUNK_END);
    chunk
}

/// The v11 §5 chunk-data region for one LOD: the `chunks.len() + 1` entry `uint32` offset table
/// (relative to the first chunk's byte, so `[0] == 0` and the last entry is the total chunk bytes)
/// followed by the chunks back to back. Hand-assembled here exactly as the spec reads it, so the
/// testkit stays an oracle independent of `serialize.rs`.
pub fn chunk_region(chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut region = Vec::new();
    let mut offset = 0u32;
    region.extend_from_slice(&offset.to_le_bytes());
    for c in chunks {
        offset += c.len() as u32;
        region.extend_from_slice(&offset.to_le_bytes());
    }
    for c in chunks {
        region.extend_from_slice(c);
    }
    region
}

/// A line feature with 8-bit deltas. Exterior point count = `1 + deltas.len()`.
pub fn pack_line(style_id: u8, ax: i32, ay: i32, deltas: &[(i8, i8)]) -> Vec<u8> {
    // line, 8-bit deltas — no flags set
    let mut v = feature_header(style_id, (1 + deltas.len()) as u16, ax, ay, 0);
    push_deltas8(&mut v, deltas);
    v
}

/// A line feature carrying the v13 §5.2 **level** (flag bit 4): the `int16` metres sit between the
/// header and the deltas, which is the only place the field ever appears.
pub fn pack_line_level(style_id: u8, ax: i32, ay: i32, level: i16, deltas: &[(i8, i8)]) -> Vec<u8> {
    let mut v = feature_header(style_id, (1 + deltas.len()) as u16, ax, ay, FEATURE_HAS_LEVEL_BIT);
    v.extend_from_slice(&level.to_le_bytes());
    push_deltas8(&mut v, deltas);
    v
}

/// A **polygon** carrying a level — a deliberately malformed feature (§5.2: levels are legal on
/// lines only), for the reject path. Byte-shaped exactly as a reader that ignored the rule would
/// expect, so a test proves the refusal and not a framing accident.
pub fn pack_poly_level(style_id: u8, ax: i32, ay: i32, level: i16, deltas: &[(i8, i8)]) -> Vec<u8> {
    let flags = FEATURE_FLAG_POLYGON | FEATURE_HAS_LEVEL_BIT;
    let mut v = feature_header(style_id, (1 + deltas.len()) as u16, ax, ay, flags);
    v.extend_from_slice(&level.to_le_bytes());
    push_deltas8(&mut v, deltas);
    v
}

/// A line with arbitrary `extra_flags` OR-ed into its flags byte and no level field — for authoring
/// the reserved-bit cases (§5.2 bits 5-7) a reader must reject.
pub fn pack_line_flags(style_id: u8, ax: i32, ay: i32, extra_flags: u8, deltas: &[(i8, i8)]) -> Vec<u8> {
    let mut v = feature_header(style_id, (1 + deltas.len()) as u16, ax, ay, extra_flags);
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

/// A 16-bit-delta line carrying the v13 §5.2 **level** — [`pack_line16`] and [`pack_line_level`]
/// combined, for a contour long enough on the ground to need wide deltas. The `int16` metres sit
/// between the header and the deltas in both delta widths.
pub fn pack_line16_level(style_id: u8, ax: i32, ay: i32, level: i16, deltas: &[(i16, i16)]) -> Vec<u8> {
    let flags = FEATURE_FLAG_16BIT | FEATURE_HAS_LEVEL_BIT;
    let mut v = feature_header(style_id, (1 + deltas.len()) as u16, ax, ay, flags);
    v.extend_from_slice(&level.to_le_bytes());
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

/// The style id [`build_bench_map`] gives its **index contours** — the one style in the fixture
/// carrying §2's fixed-width + terrain-layer + contour-index bits, and the only one whose features
/// carry a §5.2 level. Public so a test can name the labelled style instead of a magic number.
pub const BENCH_CONTOUR_STYLE: u8 = 7;

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

/// **Index contours** across a leaf `w × h` µdeg, one per entry of `ats` (a latitude inside the leaf,
/// in µdeg from its bottom edge): 8-vertex polylines spanning the leaf horizontally, each carrying
/// its level in metres (v13 §5.2) under the index-contour style [`BENCH_CONTOUR_STYLE`].
///
/// Present so the fixture exercises the renderer's contour-label pass (#1106) — the bench is the
/// instrument that pass's frame-time budget is measured with, and a fixture with no labelled contour
/// would measure an unlabelled frame forever. Levels are four digits (2 000 m up), the width the
/// label pill is sized for. The gentle vertical wander keeps the lines off the degenerate
/// axis-aligned case.
///
/// The callers deliberately hug the **leaf edges**: the bench camera sits at the map centre, which is
/// the shared corner of the four centre leaves, so a contour placed mid-leaf would be off-screen at
/// every zoom but the overview. Same reasoning as the fine chunk's corner "villages".
fn bench_contours(rng: &mut BenchRng, w: i32, h: i32, ats: &[i32]) -> Vec<u8> {
    let mut c = Vec::new();
    let step = (w / 7) as i16;
    for (k, &ay) in ats.iter().enumerate() {
        let level = (2_000 + 200 * k) as i16;
        let deltas: Vec<(i16, i16)> = (0..7).map(|_| (step, rng.range(-h / 24, h / 24) as i16)).collect();
        c.extend(pack_line16_level(BENCH_CONTOUR_STYLE, 0, ay, level, &deltas));
    }
    c
}

/// One coarse-LOD chunk: a leaf-covering land backdrop, a lake on roughly half the leaves, four
/// leaf-spanning index contours and 225 short 3-point road stubs cycling the three line styles. 16
/// leaves × ~230 features ≈ 3 680 — deliberately **over `obc_render::MAX_SPANS`** so a full-map
/// overview scene saturates the span buffer and exercises the priority-drop path. ~3.9 KB, under the
/// 4 KB chunk size.
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
    // Five index contours per leaf: at the mid and overview zooms (both on this LOD) these are the
    // labelled terrain the #1106 label pass is benched against. The near-edge pair puts contours
    // through the leaf corners the bench camera sits on; the rest spread up the leaf.
    c.extend(bench_contours(rng, w, h, &[h / 16, h / 4, h / 2, 3 * h / 4, 15 * h / 16]));
    for k in 0..225 {
        let style = [5u8, 4, 3][k % 3];
        let (ax, ay) = (rng.range(130, w - 130), rng.range(0, h - 260));
        let d0 = (rng.range(-120, 121) as i8, rng.range(1, 121) as i8);
        let d1 = (rng.range(-120, 121) as i8, rng.range(1, 121) as i8);
        c.extend(pack_line(style, ax, ay, &[d0, d1]));
    }
    c
}

/// One fine-LOD chunk: a leaf-covering backdrop, an occasional lake, two leaf-spanning index
/// contours, small 8-bit-delta buildings (every fourth with a hole), a few long 16-bit-delta roads,
/// and a batch of short 8-bit paths of varying vertex counts — the riding-zoom feature mix. ≤ ~2 KB,
/// well under the 4 KB chunk size.
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
    // Two index contours per leaf, one just inside each horizontal edge — the riding camera sits on
    // a leaf corner, so these are the ones it sees (#1106).
    c.extend(bench_contours(rng, w, h, &[h / 16, 15 * h / 16]));
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
                               // The index-contour style's upper flag bits (§2): fixed width + terrain layer + contour index —
                               // the shipped `contour.index` style's exact set, so the bench renders the real thing.
    let contour_flags = STYLE_FIXED_WIDTH_BIT | STYLE_TERRAIN_LAYER_BIT | STYLE_CONTOUR_INDEX_BIT;
    let styles: [(Style, u8); 7] = [
        ((1, -10, 0xD6DA, 0, 1, false, None), 0), // land backdrop — lowest z, fills under everything
        ((2, -5, 0x64DD, 0, 2, false, None), 0),  // water
        // Index contour: the shipped preset's own z / colour / weight / priority, so the benched
        // frame drops and paints contours exactly as the device does — priority 4 means a saturated
        // overview sheds them first, which is itself worth measuring.
        ((BENCH_CONTOUR_STYLE, 9, 0xAD55, 1, 4, false, None), contour_flags),
        ((6, 1, 0x9CD3, 0, 3, false, None), 0), // buildings
        ((5, 4, 0xFC00, 3, 2, false, None), 0), // major road, weight 3
        ((4, 3, 0xFEA0, 2, 3, false, None), 0), // secondary road, weight 2
        ((3, 2, 0xFFFF, 1, 4, false, None), 0), // minor path, weight 1 (Polyline path)
    ];

    let mut rng = BenchRng(0x0BC0_0327_D00D_F00D); // hard-coded seed — never change casually
    let (coarse_index, coarse_leaves) = uniform_quadtree(BENCH_BBOX, 2);
    let coarse_chunks: Vec<Vec<u8>> =
        coarse_leaves.iter().map(|&leaf| seal(bench_coarse_chunk(&mut rng, leaf), CHUNK)).collect();
    let (fine_index, fine_leaves) = uniform_quadtree(BENCH_BBOX, 3);
    let fine_chunks: Vec<Vec<u8>> =
        fine_leaves.iter().map(|&leaf| seal(bench_fine_chunk(&mut rng, leaf), CHUNK)).collect();

    build_file_flagged(
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
