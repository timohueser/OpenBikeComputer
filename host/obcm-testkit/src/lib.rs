//! Hand-written current-OBCM byte builder shared by the `obc-reader` and `obc-render` integration
//! tests.
//!
//! Both crates synthesise `.obcm` byte buffers by hand rather than checking in a binary fixture, so
//! the Rust reader stays pinned to `OBCM_Spec.md`: if either drifts, the format tests break. This
//! crate is the single source, so a format bump edits the layout once.
//!
//! [`build_file`] and [`build_priority_tree`] write empty POI and nav sections so the reader
//! accepts them; the directory, record and pool builders ([`poi_directory`], [`pack_poi_record`],
//! [`hours_pool`], [`nav_directory`], [`pack_nav_record`], [`pack_nav_edge_record`]) let the
//! contract tests pin each section's bytes explicitly.
//!
//! Every offset field is a count of `U = 1 << Offset Scale` byte units: every structure an offset
//! reaches begins on a unit boundary and the `0..U-1` bytes before it are `0xFF` filler. See
//! [`align_up`], [`filler_len`], [`scaled`] and [`splice_terrain`].
//!
//! Three map shapes are needed and kept as distinct, clearly-named builders so each call site's
//! bytes stay identical:
//! - [`build_file`] — the general multi-LOD builder ([`LodSpec`] per layer), used by the reader's
//!   format-contract tests.
//! - [`build_priority_tree`] — a fixed single-LOD NW-branch, NE-leaf quadtree, used by the
//!   renderer's priority-saturation test.
//! - [`build_bench_map`] — the deterministic two-LOD bench fixture `obc-bench` renders and hashes;
//!   its bytes must stay identical on every machine, forever.
//!
//! Style records are `(id, z_index, color_rgb565, weight, priority, dashed, color2)`; feature
//! encoders ([`pack_line`], [`pack_line16`], [`pack_poly`], [`pack_poly_hole`]) return one packed
//! feature, [`seal`] closes a chunk with its single `0xFF` sentinel, and the file builders'
//! `chunk_region` lays sealed chunks out behind their offset table.

/// Throwaway temp paths for tests — not an OBCM concern, but this crate is the one dev-dep every
/// host tool and shell already shares, which makes it the cheapest common home (the alternative,
/// `obc-host-core`, drags the whole device render path into a CLI tool's test build).
pub mod articles;
pub mod scratch;
pub mod terrain;

/// A style record (OBCM §2, 8 bytes on the wire): `(id, z_index, color_rgb565, weight, priority,
/// dashed, color2)`. `dashed` sets flag bit 2; `color2 = Some(_)` sets flag bit 3 and writes the
/// secondary color, `None` writes `0x0000` with the bit clear.
pub type Style = (u8, i8, u16, u8, u8, bool, Option<u16>);

// Normative constants only: byte assembly below remains hand-written and never calls a production
// serializer/parser, preserving the testkit as an independent oracle.
pub use obc_formats::obcm::{
    nav_edge_id, BRANCH_BIT, EMPTY_LEAF, FILLER, HEADER_LEN, LOD_ENTRY_LEN, NAV_CHUNK_SIZE, NAV_DIR_LEN,
    NAV_EDGE_FIXED_LEN, NAV_NEIGHBOR_LEN, NAV_NODE_FIXED_LEN, NAV_PROFILE_LEN, NAV_PROFILE_NAME_LEN,
    POI_CATEGORY_COUNT, POI_CAT_ENTRY_LEN, POI_CHUNK_SIZE, POI_DIR_POOL_FIELDS_LEN, POI_HOURS_BLOB_LEN, POI_NAME_LEN,
    POI_RECORD_LEN,
};
use obc_formats::obcm::{
    CHUNK_END, FEATURE_FLAG_16BIT, FEATURE_FLAG_HOLES, FEATURE_FLAG_POLYGON, FEATURE_FLAG_WIDE,
    HEADER_DARK_MARKER_COLOR_OFF, HEADER_DARK_STYLE_OFFSET_OFF, HEADER_TERRAIN_LENGTH_OFF, HEADER_TERRAIN_OFFSET_OFF,
    MAGIC, OFFSET_SCALE_DEFAULT, STYLE_DASHED_BIT, STYLE_HAS_COLOR2_BIT, STYLE_PRIORITY_MASK, VERSION,
};
/// Distinctive (non-default) marker color baked into [`build_file`]'s header, so the
/// reader's round-trip test is meaningful.
pub const MARKER: u16 = 0xABCD;
/// Distinctive dark marker used by the default paired test maps.
pub const DARK_MARKER: u16 = 0x1234;

// The three helpers below are the whole of the scaled-offset addressing, transcribed from the spec
// rather than imported from `serialize.rs`: the packer's `align_up`, `filler_len` and `scaled` are
// the bytes this kit is the independent oracle for.

/// The `Offset Scale` byte every file this kit builds carries: the base-2 logarithm of the offset
/// unit, so `4` gives `U = 16`, which is what every producer in this tree writes.
pub const OFFSET_SCALE: u8 = OFFSET_SCALE_DEFAULT;

/// `U`, the offset unit in bytes — `1 << OFFSET_SCALE`. A scaled offset counts these, not bytes.
pub const UNIT: usize = 1usize << OFFSET_SCALE;

/// The next unit boundary at or after `at`. Every structure a header or directory offset reaches
/// begins on one.
pub const fn align_up(at: usize) -> usize {
    (at + UNIT - 1) & !(UNIT - 1)
}

/// The `0..U-1` bytes of [`FILLER`] that [`align_up`] implies at `at`.
pub const fn filler_len(at: usize) -> usize {
    align_up(at) - at
}

/// The `uint32` a scaled offset field stores for byte offset `at`.
///
/// A scaled offset cannot name a byte that is not a multiple of `U`, so a non-boundary argument is
/// a bug in the layout above it rather than a rounding request — hence the panic. A test handing
/// this a bad offset fails loudly instead of silently writing a file the reader then rejects for
/// the wrong reason.
pub fn scaled(at: usize) -> u32 {
    assert_eq!(at % UNIT, 0, "byte {at} is not on a {UNIT}-byte unit boundary (§1.1)");
    (at / UNIT) as u32
}

/// Byte offset of the style table in every file this kit builds: the first unit boundary at or
/// after the 65-byte header, which at the default `U = 16` is `80`.
pub const STYLE_OFFSET: usize = align_up(HEADER_LEN);

/// One LOD layer: its quadtree index (flat u32 nodes) and its data chunks. Each chunk is the tight
/// byte string [`seal`] produces, bounded by `chunk_size` rather than padded to it.
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
    let mut style_bytes = vec![styles.len() as u8];
    for &(id, z, color, weight, priority, dashed, color2) in styles {
        let mut flags = (priority - 1) & STYLE_PRIORITY_MASK;
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

/// The fixed OBCM header, shared by both file builders. The version byte is `VERSION`, so this
/// builds whatever the reader currently reads.
///
/// `<4sBiiiiIBIHIIBII`: magic, ver, min_lat, min_lon, max_lat, max_lon, style_off, lod_count,
/// lod_table_off, marker_color, poi_section_off, nav_section_off, offset_scale, terrain_off,
/// terrain_len. `bbox` is `(min_lon, min_lat, max_lon, max_lat)`.
///
/// Every offset argument is a byte offset and is [`scaled`] here, so a caller passing one that is
/// not on a unit boundary panics at the point of the mistake. `terrain` is the terrain region as
/// `(byte offset, byte length)`, both unit-aligned; `None` writes the `(0, 0)` pair that means this
/// map carries no elevation.
#[allow(clippy::too_many_arguments)]
fn obcm_header(
    bbox: (i32, i32, i32, i32),
    style_off: usize,
    lod_count: u8,
    lod_tab_off: usize,
    marker: u16,
    poi_section_off: usize,
    nav_section_off: usize,
    terrain: Option<(usize, usize)>,
) -> Vec<u8> {
    let mut f = Vec::new();
    f.extend_from_slice(&MAGIC);
    f.push(VERSION);
    f.extend_from_slice(&bbox.1.to_le_bytes()); // min_lat
    f.extend_from_slice(&bbox.0.to_le_bytes()); // min_lon
    f.extend_from_slice(&bbox.3.to_le_bytes()); // max_lat
    f.extend_from_slice(&bbox.2.to_le_bytes()); // max_lon
    f.extend_from_slice(&scaled(style_off).to_le_bytes());
    f.push(lod_count);
    f.extend_from_slice(&scaled(lod_tab_off).to_le_bytes());
    f.extend_from_slice(&marker.to_le_bytes());
    f.extend_from_slice(&scaled(poi_section_off).to_le_bytes());
    f.extend_from_slice(&scaled(nav_section_off).to_le_bytes());
    f.push(OFFSET_SCALE);
    let (terrain_off, terrain_len) = terrain.unwrap_or((0, 0));
    f.extend_from_slice(&scaled(terrain_off).to_le_bytes());
    f.extend_from_slice(&scaled(terrain_len).to_le_bytes());
    f.extend_from_slice(&[0; 16]);
    // The dark presentation is appended after the rest of the map and patched there.
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(&DARK_MARKER.to_le_bytes());
    assert_eq!(f.len(), HEADER_LEN, "header length follows the normative constant");
    f
}

/// Append a dark style table and patch its header pointer and marker. The default builders call
/// this with the light styles, while format tests can replace either argument to forge a distinct
/// but structurally valid presentation.
pub fn append_dark_styles(map: &mut Vec<u8>, styles: &[Style], marker: u16) -> usize {
    map.resize(align_up(map.len()), FILLER);
    let offset = map.len();
    map.extend_from_slice(&style_table(styles));
    map[HEADER_DARK_STYLE_OFFSET_OFF..HEADER_DARK_STYLE_OFFSET_OFF + 4].copy_from_slice(&scaled(offset).to_le_bytes());
    map[HEADER_DARK_MARKER_COLOR_OFF..HEADER_DARK_MARKER_COLOR_OFF + 2].copy_from_slice(&marker.to_le_bytes());
    offset
}

/// The header plus the filler that carries it to the style table's unit boundary — the first
/// [`STYLE_OFFSET`] bytes of every file this kit builds.
#[allow(clippy::too_many_arguments)]
fn header_block(
    bbox: (i32, i32, i32, i32),
    lod_count: u8,
    lod_tab_off: usize,
    marker: u16,
    poi_section_off: usize,
    nav_section_off: usize,
) -> Vec<u8> {
    let mut f = obcm_header(bbox, STYLE_OFFSET, lod_count, lod_tab_off, marker, poi_section_off, nav_section_off, None);
    f.resize(STYLE_OFFSET, FILLER);
    f
}

/// A recognisable, deterministic stand-in for a baked OBCT container: `len` bytes of
/// position-derived noise that is not a parseable terrain file.
///
/// A reader hands the region over without parsing it, so the fixture's job is to be
/// distinguishable at the byte level and nothing else.
pub fn terrain_stub(len: usize) -> Vec<u8> {
    (0..len).map(|k| (k as u8).wrapping_mul(37).wrapping_add(0x5A)).collect()
}

/// Splice a terrain region into a finished map: `region`'s bytes land at the first unit boundary at
/// or after the file's current tail, `0xFF`-filled up to the next boundary, and the header's
/// `Terrain Offset` and `Terrain Length` pair is patched to name them.
///
/// Terrain sits last precisely so that splicing it moves no other offset, which is why this is a
/// post-pass over an already-built file rather than a parameter of every builder. `Terrain Length`
/// counts units, so the window this hands a reader is up to `U - 1` bytes longer than `region`.
///
/// It stays here although `obcm-assemble` splices for real, for the reason the testkit's alignment
/// arithmetic stays: an oracle must not import the code it is an oracle for.
pub fn splice_terrain(map: &[u8], region: &[u8]) -> Vec<u8> {
    assert!(!region.is_empty(), "an absent terrain region is the header's `0` pair, not a zero-length one");
    let mut f = map.to_vec();
    f.resize(align_up(f.len()), FILLER);
    let offset = f.len();
    f.extend_from_slice(region);
    f.resize(align_up(f.len()), FILLER);
    let len = f.len() - offset;
    f[HEADER_TERRAIN_OFFSET_OFF..HEADER_TERRAIN_OFFSET_OFF + 4].copy_from_slice(&scaled(offset).to_le_bytes());
    f[HEADER_TERRAIN_LENGTH_OFF..HEADER_TERRAIN_LENGTH_OFF + 4].copy_from_slice(&scaled(len).to_le_bytes());
    f
}

/// One POI-directory category entry (spec §7.1): `category_id, index_offset, index_node_count,
/// chunk_count`. Used by [`poi_directory`] and the reader's POI contract tests.
/// `index_offset` is the index's byte offset; [`poi_directory`] scales it, so a category pointed at
/// a byte that is no unit boundary fails at the point of the mistake.
pub struct PoiCat {
    pub category_id: u8,
    pub index_offset: usize,
    pub node_count: u32,
    pub chunk_count: u32,
}

/// Build a POI directory (spec §7.1): the count byte, the shared `chunk_size`, one 13-byte entry
/// per category, then the `hours_pool_offset u32` and `hours_pool_count u16`. The caller supplies
/// the already-computed per-category byte offsets and counts and the pool's byte offset and count;
/// this only lays out the directory bytes. Both offset fields are scaled here.
pub fn poi_directory(chunk_size: u16, cats: &[PoiCat], hours_pool_offset: usize, hours_pool_count: u16) -> Vec<u8> {
    let mut d = Vec::with_capacity(3 + cats.len() * POI_CAT_ENTRY_LEN + POI_DIR_POOL_FIELDS_LEN);
    d.push(cats.len() as u8);
    d.extend_from_slice(&chunk_size.to_le_bytes());
    for c in cats {
        d.push(c.category_id);
        d.extend_from_slice(&scaled(c.index_offset).to_le_bytes());
        d.extend_from_slice(&c.node_count.to_le_bytes());
        d.extend_from_slice(&c.chunk_count.to_le_bytes());
    }
    d.extend_from_slice(&scaled(hours_pool_offset).to_le_bytes());
    d.extend_from_slice(&hours_pool_count.to_le_bytes());
    d
}

/// The full POI-directory length in bytes: count + chunk_size + six entries + the two pool fields.
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

/// An empty POI section: the directory's six categories all carry `node_count 0` and
/// `chunk_count 0`, and the hours pool is a bare `count 0`. `section_off` is the directory's
/// absolute byte offset, which must itself be a unit boundary. This is what a map with no POIs
/// carries.
///
/// A zero-length region still has to be nameable, so every one of those offsets points at the first
/// unit boundary past the 87-byte directory rather than at the byte behind it, with that gap
/// written as filler. The returned section ends on a unit boundary too.
pub fn empty_poi_directory(section_off: usize) -> Vec<u8> {
    let dir_gap = filler_len(section_off + poi_dir_len());
    let after_dir = section_off + poi_dir_len() + dir_gap;
    let cats: Vec<PoiCat> = obc_formats::obcm::PoiCategory::ALL
        .into_iter()
        .map(|c| c.id())
        .map(|id| PoiCat { category_id: id, index_offset: after_dir, node_count: 0, chunk_count: 0 })
        .collect();
    // No categories ⇒ no chunks: the (empty) hours pool sits at that same aligned offset.
    let mut d = poi_directory(POI_CHUNK_SIZE as u16, &cats, after_dir, 0);
    d.resize(d.len() + dir_gap, FILLER);
    d.extend_from_slice(&hours_pool(&[]));
    d.resize(align_up(section_off + d.len()) - section_off, FILLER);
    d
}

/// Build a current nav directory (spec §8.1). The caller supplies the already-computed absolute
/// byte offsets and counts; this only lays out the 40 directory bytes. Each offset is [`scaled`]
/// here, so one that is not on a unit boundary panics rather than becoming a file the reader
/// rejects for a different reason. `chunk_size` must be 512.
#[allow(clippy::too_many_arguments)]
pub fn nav_directory(
    index_offset: usize,
    index_node_count: u32,
    node_chunk_count: u32,
    edge_pool_offset: usize,
    edge_chunk_count: u32,
    chunk_size: u16,
    profile_table_offset: usize,
    profile_count: u8,
) -> Vec<u8> {
    let mut d = Vec::with_capacity(NAV_DIR_LEN);
    d.extend_from_slice(&scaled(index_offset).to_le_bytes());
    d.extend_from_slice(&index_node_count.to_le_bytes());
    d.extend_from_slice(&node_chunk_count.to_le_bytes());
    d.extend_from_slice(&scaled(edge_pool_offset).to_le_bytes());
    d.extend_from_slice(&edge_chunk_count.to_le_bytes());
    d.extend_from_slice(&chunk_size.to_le_bytes());
    d.extend_from_slice(&scaled(profile_table_offset).to_le_bytes());
    d.push(profile_count);
    d.push(0); // reserved — a field, so `0`, unlike a gap

    // Testkit graphs carry no long-edge lookup anchors. Point the empty snap region just past the
    // edge pool, matching the production writer's empty-index convention. 512 is a multiple of `U`
    // at every legal scale, so a whole number of chunks past an aligned pool is aligned too.
    let snap_offset = edge_pool_offset + edge_chunk_count as usize * usize::from(chunk_size);
    d.extend_from_slice(&scaled(snap_offset).to_le_bytes());
    d.extend_from_slice(&0u32.to_le_bytes());
    d.extend_from_slice(&0u32.to_le_bytes());
    assert_eq!(d.len(), NAV_DIR_LEN);
    d
}

/// Pack one profile record (56 bytes): a `0xFF`-padded 12-byte name, 32 highway and 8 surface
/// multipliers (`u8` 1/16 fixed-point), the `climb_weight` byte and 3 reserved zero bytes. `name`
/// is truncated to 12 bytes.
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

/// A minimal profile table: one profile ("Default", every multiplier 16 = 1.0×, climb-blind), 56
/// bytes — enough to satisfy the reader's "1..=8 profiles, always present" rule.
pub fn default_nav_profile_table() -> Vec<u8> {
    nav_profile_record("Default", [16; 32], [16; 8], 0)
}

/// An empty nav section: the directory, its filler, and the always-present profile table — no
/// quadtree, no chunks, no edges. This is what a map with no routable ways carries. `section_off`
/// must be a unit boundary.
///
/// The directory is 40 bytes, which is no multiple of `U`, so the profile table sits at
/// `align_up(section_off + 40, U)`, with eight bytes of filler behind the directory at the default
/// `U = 16`. The zero-length index and edge pool then start at the first unit boundary past the
/// table, because a zero-length region still has to be nameable.
pub fn empty_nav_directory(section_off: usize) -> Vec<u8> {
    let table = default_nav_profile_table();
    let dir_gap = filler_len(section_off + NAV_DIR_LEN);
    let profile_table_offset = section_off + NAV_DIR_LEN + dir_gap;
    // Zero-length index + edge pool start here, on the first boundary past the table.
    let after = align_up(profile_table_offset + table.len());
    let mut out = nav_directory(after, 0, 0, after, 0, NAV_CHUNK_SIZE as u16, profile_table_offset, 1);
    out.resize(out.len() + dir_gap, FILLER);
    out.extend_from_slice(&table);
    out.resize(after - section_off, FILLER);
    out
}

/// One neighbor entry for [`pack_nav_record`]: `(neighbor_id, lat, lon, edge_id, cost_m, way_kind,
/// ascent_m)`. `lat` and `lon` are the neighbor's absolute µdeg coords, and [`pack_nav_record`]
/// stores the `i16` delta from the owning record's own coord; `cost_m` must fit `u16`; `ascent_m`
/// is the climb of riding toward this neighbor, so the two entries of one edge legitimately differ
/// in it.
pub type NavNeighborSpec = (u32, i32, i32, u32, u32, u8, u16);

/// Pack one variable-length junction record: `lat i32, lon i32, node_id u32, degree u8`, then one
/// 17-byte entry per neighbor (`id u32, dlat i16, dlon i16, edge_id u32, cost_m u16, way_kind u8,
/// ascent_m u16`). The record head coords are absolute µdeg, latitude first; each neighbor's coord
/// is stored as an `i16` delta from this record's own.
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

/// Pack one edge record: `length_m u32, pt_count u16, way_kind u8, anchor_lat i32, anchor_lon i32`,
/// then `pt_count - 1` × `(dlat i16, dlon i16)`. The polyline is absolute µdeg `(lat, lon)` pairs,
/// latitude first, and the caller keeps deltas within `i16`.
///
/// An `Edge Id` is the packed `(chunk, ordinal)` pair, so a caller writing one into an adjacency
/// entry or a snap anchor builds it with [`nav_edge_id`] rather than from a byte offset.
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

/// Pack one 36-byte POI record: absolute `int32 lat, int32 lon`, `u8 subtype`, `u8 name_len`, a
/// 24-byte `0xFF`-padded name, and the `u16 payload` at offset 34 — an hours-pool index for a
/// service place (`0xFFFF` means none), a signed elevation for a summit, a population in hundreds
/// for a settlement. `name` is stored as-is, pre-folded by the caller to at most 24 bytes.
pub fn pack_poi_record(lat: i32, lon: i32, subtype: u8, name: &str, payload: u16) -> [u8; POI_RECORD_LEN] {
    let mut rec = [0xFFu8; POI_RECORD_LEN];
    rec[0..4].copy_from_slice(&lat.to_le_bytes());
    rec[4..8].copy_from_slice(&lon.to_le_bytes());
    rec[8] = subtype;
    let bytes = name.as_bytes();
    let len = bytes.len().min(POI_NAME_LEN);
    rec[9] = len as u8;
    rec[10..10 + len].copy_from_slice(&bytes[..len]);
    // rec[10 + len .. 34] stays 0xFF (name pad); the payload goes at [34..36].
    rec[34..36].copy_from_slice(&payload.to_le_bytes());
    let identity = ((lat as u32 as u64) << 24 ^ lon as u32 as u64 ^ subtype as u64) & ((1 << 62) - 1);
    rec[36..64].copy_from_slice(
        &obc_formats::obcm::PoiMetadata {
            source: obc_formats::obcm::SourceId::osm(1, identity.max(1)),
            approach: None,
        }
        .encode(),
    );
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
/// (already-folded, ≤ 24-byte) name, and its [`pack_poi_record`] `payload`. Mirrors a serializer
/// `PoiPoint`.
#[derive(Clone)]
pub struct PoiSpec {
    pub lat: i32,
    pub lon: i32,
    pub subtype: u8,
    pub name: String,
    pub payload: u16,
}

/// Serialize one category's POIs into a per-category quadtree over `bbox`: the flat `u32` index and
/// its data chunks, built to walk identically to the reader and packer. A leaf holds at most
/// `chunk_size/36` records; an over-full leaf subdivides on floor-division midpoints in NW/NE/SW/SE
/// order, where east and north of the midline is `>= mid`, stopping at the 10-µdeg recursion floor.
/// Returns `(index_bytes, node_count, chunk_bytes, chunk_count)`. The test-only mirror of
/// `obc-pack`'s tree builder, so the reader tests need no GEOS-linked packer.
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
        // West is lon < mid, South is lat < mid — a point on a midline lands East or North, matching
        // the packer's assignment so it stays inside its leaf's bbox for the query.
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

    // The shared resident walk fixes child numbering; this oracle keeps its independent tree
    // construction, leaf policy, record encoding and chunk framing.
    let (nodes, first_child) = obc_tree_walk::breadth_first(&root, |node| match node {
        PoiNode::Leaf(_) => None,
        PoiNode::Branch(children) => Some(children),
    });
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
                    pts.iter().map(|p| pack_poi_record(p.lat, p.lon, p.subtype, &p.name, p.payload)).collect();
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

/// Build a full `.obcm` with a populated POI section — the query-test analogue of [`build_file`].
/// `bbox` is `(min_lon, min_lat, max_lon, max_lat)`; a minimal one-line geometry LOD keeps the map
/// valid; `pois_by_cat` maps a category id to the POIs to place there, each a full per-category
/// quadtree over `bbox`. Categories absent from the map are written empty, and an empty hours pool
/// follows at the tail. Use [`build_poi_map_with_hours`] to bake a real pool. The section is
/// assembled at its file-absolute offset so the reader's walk resolves.
pub fn build_poi_map(bbox: (i32, i32, i32, i32), chunk_size: usize, pois_by_cat: &[(u8, Vec<PoiSpec>)]) -> Vec<u8> {
    build_poi_map_with_hours(bbox, chunk_size, pois_by_cat, &[])
}

/// Like [`build_poi_map`] but bakes a real hours pool of `hours_blobs` at the file tail, with the
/// directory's `hours_pool_offset` and `hours_pool_count` pointing at it. Each [`PoiSpec`]'s
/// `payload` indexes into `hours_blobs`, where `0xFFFF` means no hours.
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
    let poi_off = resolve_offset(&base, 32);

    // Lay out `[directory][filler][cat index][filler][chunks]*[hours pool]`, categories in id
    // order. Every `Index Offset` is scaled, so each index starts on a unit boundary, and a
    // category's chunks begin at `align_up(Index Offset * U + Index Node Count * 4, U)`. 512 is a
    // multiple of `U`, so whole chunks leave the cursor aligned.
    let mut ids: Vec<_> = obc_formats::obcm::PoiCategory::ALL.into_iter().map(|c| c.id()).collect();
    ids.extend(pois_by_cat.iter().map(|(id, _)| *id));
    ids.sort_unstable();
    ids.dedup();
    let category_count = ids.len();
    let directory_len = 3 + category_count * POI_CAT_ENTRY_LEN + POI_DIR_POOL_FIELDS_LEN;
    let mut payload = Vec::new(); // everything after the directory
    let mut cats: Vec<PoiCat> = Vec::new();
    let dir_gap = filler_len(poi_off + directory_len);
    payload.resize(dir_gap, FILLER);
    let mut cursor = poi_off + directory_len + dir_gap; // absolute offset of the next category's index
    for id in ids {
        let pois = pois_by_cat.iter().find(|(c, _)| *c == id).map(|(_, v)| v.as_slice()).unwrap_or(&[]);
        if pois.is_empty() {
            // Empty category: its (zero-length) index "starts" at the cursor, no chunks.
            cats.push(PoiCat { category_id: id, index_offset: cursor, node_count: 0, chunk_count: 0 });
            continue;
        }
        let (index_bytes, node_count, chunk_bytes, chunk_count) = serialize_poi_category(pois, bbox, chunk_size);
        cats.push(PoiCat { category_id: id, index_offset: cursor, node_count, chunk_count });
        payload.extend_from_slice(&index_bytes);
        cursor += index_bytes.len();
        let gap = filler_len(cursor);
        payload.resize(payload.len() + gap, FILLER);
        cursor += gap;
        payload.extend_from_slice(&chunk_bytes);
        cursor += chunk_bytes.len();
    }

    // The hours pool begins at the first unit boundary at or after the last category's chunks;
    // those are whole `chunk_size` strides, so in practice `cursor` is already one.
    let gap = filler_len(cursor);
    payload.resize(payload.len() + gap, FILLER);
    let hours_pool_offset = cursor + gap;
    payload.extend_from_slice(&hours_pool(hours_blobs));

    let mut f = base[..poi_off].to_vec();
    f.extend_from_slice(&poi_directory(chunk_size as u16, &cats, hours_pool_offset, hours_blobs.len() as u16));
    f.extend_from_slice(&payload);
    // The populated POI section displaced `base`'s tail sections, so re-append the empty nav section
    // at the new aligned tail and patch the header's nav offset to match.
    f.resize(align_up(f.len()), FILLER);
    let nav_section_off = f.len();
    f[36..40].copy_from_slice(&scaled(nav_section_off).to_le_bytes());
    f.extend_from_slice(&empty_nav_directory(nav_section_off));
    append_dark_styles(&mut f, styles, DARK_MARKER);
    f
}

/// Resolve the scaled `uint32` at byte `at` back to a byte offset: `u32(field) * U`, widened before
/// the multiply. The read-back twin of [`scaled`].
pub fn resolve_offset(bytes: &[u8], at: usize) -> usize {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize * UNIT
}

/// Start a feature record (OBCM v11 §5): `style_id`, `flags`, then either the **compact** fields
/// (`point_count u8`, anchor `u16` ×2) or the **wide** ones (`point_count u16`, anchor `i32` ×2) with
/// [`FEATURE_FLAG_WIDE`] set — the common prefix of every `pack_*` encoder.
///
/// The form is picked by the same rule the packer uses, so a caller's ordinary small-anchor feature
/// exercises the compact path and one anchored on a real µdeg coordinate, or holding more than 255
/// vertices, exercises the wide escape. `flags` sits at byte 1 because a reader must know the
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

/// Build a general multi-LOD `.obcm`. `bbox` is `(min_lon, min_lat, max_lon, max_lat)`; `styles`
/// are `(id, z_index, color_rgb565, weight, priority, dashed, color2)`; each [`LodSpec`] is one
/// layer with its own quadtree index and padded chunks. The header carries [`MARKER`] as the marker
/// color.
pub fn build_file(bbox: (i32, i32, i32, i32), styles: &[Style], lods: &[LodSpec]) -> Vec<u8> {
    let style_bytes = style_table(styles);

    // Every offset field names a unit boundary, so the style table sits at `STYLE_OFFSET` and each
    // following structure starts at the first boundary past the one before it.
    let lod_tab_off = align_up(STYLE_OFFSET + style_bytes.len());
    let style_gap = lod_tab_off - (STYLE_OFFSET + style_bytes.len());
    let payload_start = align_up(lod_tab_off + lods.len() * LOD_ENTRY_LEN);
    let table_gap = payload_start - (lod_tab_off + lods.len() * LOD_ENTRY_LEN);

    let mut cursor = payload_start;
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
        let chunk_bytes = chunk_region(&lod.chunks, idx_bytes.len());
        table.extend_from_slice(&lod.max_mpp.to_le_bytes());
        table.extend_from_slice(&scaled(idx_off).to_le_bytes());
        table.extend_from_slice(&(lod.index.len() as u32).to_le_bytes());
        table.extend_from_slice(&(lod.chunk_size as u16).to_le_bytes());
        table.extend_from_slice(&(lod.chunks.len() as u32).to_le_bytes());
        cursor += idx_bytes.len() + chunk_bytes.len();
        payload.extend_from_slice(&idx_bytes);
        payload.extend_from_slice(&chunk_bytes);
    }

    // The POI section begins right after the LOD payload, which ends aligned; the empty nav section
    // follows it at the file tail.
    let poi_section_off = cursor;
    let poi_dir = empty_poi_directory(poi_section_off);
    let nav_section_off = poi_section_off + poi_dir.len();

    let mut f = header_block(bbox, lods.len() as u8, lod_tab_off, MARKER, poi_section_off, nav_section_off);
    f.extend_from_slice(&style_bytes);
    f.resize(f.len() + style_gap, FILLER);
    f.extend_from_slice(&table);
    f.resize(f.len() + table_gap, FILLER);
    f.extend_from_slice(&payload);
    f.extend_from_slice(&poi_dir);
    f.extend_from_slice(&empty_nav_directory(nav_section_off));
    append_dark_styles(&mut f, styles, DARK_MARKER);
    f
}

/// Build a single-LOD file whose root quadtree node is a branch. NW is itself a branch whose four
/// leaves are chunks 0–3, all visited before NE, which is chunk 4. Splitting the early load across
/// four leaves keeps every chunk under the reader's `MAX_CHUNK_BYTES` cap while still saturating
/// the frame buffer before NE is reached. `styles` are
/// `(id, z, color, weight, priority, dashed, color2)`. The marker color is unused here, so it is
/// 0.
pub fn build_priority_tree(
    bbox: (i32, i32, i32, i32),
    styles: &[Style],
    chunk_size: usize,
    nw_chunks: [Vec<u8>; 4],
    ne_chunk: Vec<u8>,
) -> Vec<u8> {
    let style_bytes = style_table(styles);

    let lod_tab_off = align_up(STYLE_OFFSET + style_bytes.len());
    let style_gap = lod_tab_off - (STYLE_OFFSET + style_bytes.len());
    let index_off = align_up(lod_tab_off + LOD_ENTRY_LEN); // one LOD entry
    let table_gap = index_off - (lod_tab_off + LOD_ENTRY_LEN);

    // Quadtree (9 nodes). Root branch -> [NW=branch@5, NE=chunk 4, SW/SE empty]; NW's four
    // children (idx 5..8) -> chunks 0,1,2,3. Walk order NW(→0,1,2,3) then NE(→4): the four
    // early chunks are all visited before the late one.
    let index: [u32; 9] = [BRANCH_BIT | 1, BRANCH_BIT | 5, 4, EMPTY_LEAF, EMPTY_LEAF, 0, 1, 2, 3];
    let mut idx_bytes = Vec::new();
    for node in index {
        idx_bytes.extend_from_slice(&node.to_le_bytes());
    }
    // Chunk data in chunk-id order: 0..3 are the NW leaves, 4 is NE. Sealed and laid out with their
    // offset table.
    let [nw0, nw1, nw2, nw3] = nw_chunks;
    let chunks: Vec<Vec<u8>> = [nw0, nw1, nw2, nw3, ne_chunk].into_iter().map(|c| seal(c, chunk_size)).collect();
    let chunk_bytes = chunk_region(&chunks, idx_bytes.len());

    // LOD entry: max_mpp=+inf, index_off, node_count, chunk_size, chunk_count.
    let mut table = Vec::new();
    table.extend_from_slice(&f32::INFINITY.to_le_bytes());
    table.extend_from_slice(&scaled(index_off).to_le_bytes());
    table.extend_from_slice(&(index.len() as u32).to_le_bytes());
    table.extend_from_slice(&(chunk_size as u16).to_le_bytes());
    table.extend_from_slice(&(chunks.len() as u32).to_le_bytes());

    // The POI section begins right after the index + the chunk region; the empty nav section
    // follows it.
    let poi_section_off = index_off + idx_bytes.len() + chunk_bytes.len();
    let poi_dir = empty_poi_directory(poi_section_off);
    let nav_section_off = poi_section_off + poi_dir.len();
    // marker unused here → 0
    let mut f = header_block(bbox, 1, lod_tab_off, 0, poi_section_off, nav_section_off);
    f.extend_from_slice(&style_bytes);
    f.resize(f.len() + style_gap, FILLER);
    f.extend_from_slice(&table);
    f.resize(f.len() + table_gap, FILLER);
    f.extend_from_slice(&idx_bytes);
    f.extend_from_slice(&chunk_bytes);
    f.extend_from_slice(&poi_dir);
    f.extend_from_slice(&empty_nav_directory(nav_section_off));
    append_dark_styles(&mut f, styles, DARK_MARKER);
    f
}

/// Close a geometry chunk: append the one trailing `0xFF` [`CHUNK_END`] sentinel that ends its
/// feature stream, asserting the sealed chunk still fits `capacity` — the LOD's declared
/// `Chunk Size`, which is a bound rather than a stride.
///
/// A test that wants an unsealed chunk, which is malformed, simply skips this. The fixed-stride
/// POI and nav chunks keep [`pad`].
pub fn seal(mut chunk: Vec<u8>, capacity: usize) -> Vec<u8> {
    chunk.push(CHUNK_END);
    assert!(chunk.len() <= capacity, "sealed chunk {} exceeds chunk_size {}", chunk.len(), capacity);
    chunk
}

/// Right-pad a fixed-stride chunk to `size` bytes with `0xFF`, the filler the reader skips: the POI
/// and nav chunks. Geometry chunks are tight and want [`seal`].
pub fn pad(mut chunk: Vec<u8>, size: usize) -> Vec<u8> {
    assert!(chunk.len() <= size, "chunk {} exceeds chunk_size {}", chunk.len(), size);
    chunk.resize(size, CHUNK_END);
    chunk
}

/// The chunk-data region for one LOD: the `chunks.len() + 1` entry `uint32` offset table, the
/// filler that carries it to a unit boundary, then the chunks, each `0xFF`-padded to the next
/// boundary. Hand-assembled here exactly as the spec reads it, so the testkit stays an oracle
/// independent of `serialize.rs`.
///
/// The offsets are scaled: entry `e` names byte `data_start + e * U`, where
/// `data_start = align_up(index_start + node_count * 4 + (chunk_count + 1) * 4, U)`. That rounding
/// step is the only thing between the table and the chunks, and it is computable here because
/// `index_start` is itself a unit boundary — which is why `index_len`, the preceding index's byte
/// length, is an argument.
///
/// The table is written even for a chunkless LOD, where it is the single `0` entry, and the region
/// ends on a unit boundary.
fn chunk_region(chunks: &[Vec<u8>], index_len: usize) -> Vec<u8> {
    let table_len = (chunks.len() + 1) * 4;
    let gap = filler_len(index_len + table_len);
    let spans: Vec<usize> = chunks.iter().map(|c| align_up(c.len())).collect();
    let mut region = Vec::new();
    let mut offset = 0usize;
    region.extend_from_slice(&scaled(offset).to_le_bytes());
    for span in &spans {
        offset += span;
        region.extend_from_slice(&scaled(offset).to_le_bytes());
    }
    region.resize(region.len() + gap, FILLER);
    for (c, span) in chunks.iter().zip(&spans) {
        let end = region.len() + span;
        region.extend_from_slice(c);
        region.resize(end, FILLER);
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

/// A hole-free polygon with 16-bit deltas — the polygon analogue of [`pack_line16`]. Lets a test
/// build a polygon whose vertices span more than ±127 µdeg per delta, such as a screen-sized square
/// for the renderer's edge-fill tests. The stored exterior point count is `1 + deltas.len()`.
pub fn pack_poly16(style_id: u8, ax: i32, ay: i32, deltas: &[(i16, i16)]) -> Vec<u8> {
    // polygon | 16-bit deltas
    let mut v = feature_header(style_id, (1 + deltas.len()) as u16, ax, ay, FEATURE_FLAG_POLYGON | FEATURE_FLAG_16BIT);
    push_deltas16(&mut v, deltas);
    v
}

/// A polygon with `holes.len()` 8-bit-delta holes, each its own delta list. Generalises
/// [`pack_poly_hole`] so a test can pack more rings than the reader's `MAX_FEAT_RINGS` scratch
/// holds and assert the past-capacity rings are dropped. The exterior's stored point count is
/// `1 + ext_deltas.len()`; each hole's stored count is its own `hole.len()`, because every hole
/// vertex is a delta and the first is relative to the anchor.
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

// The deterministic bench fixture.

/// Bounding box of [`build_bench_map`] (µdeg, `(min_lon, min_lat, max_lon, max_lat)`): a 54 000 µdeg
/// square near 47° N (≈ 6 km of latitude, ≈ 4.1 km of ground longitude at that latitude's aspect).
/// Divisible by 8 on both axes so the depth-3 quadtree subdivides into uniform leaves. Public so the
/// bench aims its camera at the fixture's center without re-deriving it.
pub const BENCH_BBOX: (i32, i32, i32, i32) = (8_500_000, 47_000_000, 8_554_000, 47_054_000);

/// A quadtree-node bbox in the builders' `(min_lon, min_lat, max_lon, max_lat)` µdeg spelling.
type LeafBox = (i32, i32, i32, i32);

/// A tiny inline xorshift64* PRNG — deterministic and dependency-free, so [`build_bench_map`]
/// produces the same bytes on every machine, forever. The bench's committed frame hashes depend on
/// that.
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

/// Build a complete, uniform-depth breadth-first quadtree over `bbox` — the generalization of
/// [`build_priority_tree`]'s hand-laid 9-node index to depth `k`. Nodes are laid out exactly like
/// the packer flattens them: level by level, each branch pointing at its four children's block in
/// the next level, children in NW/NE/SW/SE order with floor-division midpoints. Every level-`depth`
/// node is a leaf holding chunk id = its breadth-first position, so the caller packs one chunk per
/// leaf in returned-bbox order.
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
            // Floor-division midpoints and NW/NE/SW/SE order must match the reader's `walk_leaves`
            // subdivision, or the leaf bboxes and every anchor base disagree.
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
/// short 3-point road stubs cycling the three line styles. 16 leaves × about 226 features is
/// deliberately over the frame's feature ceiling, `obc_render::MAX_SPANS`, so a full-map overview
/// scene saturates and exercises the priority-drop path. About 3.7 KB, under the 4 KB chunk
/// size.
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
/// varying vertex counts — the riding-zoom feature mix, well under the 4 KB chunk size.
fn bench_fine_chunk(rng: &mut BenchRng, leaf: LeafBox) -> Vec<u8> {
    let (min_lon, min_lat, max_lon, max_lat) = leaf;
    let (w, h) = (max_lon - min_lon, max_lat - min_lat);
    let mut c = Vec::new();
    // Land backdrop covering the leaf — a polygon big enough to force 16-bit deltas.
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
    // A "village" cluster within about 700 µdeg of every leaf corner. The bench's riding camera sits
    // at the map center, the shared corner of the four center leaves, so corner clusters guarantee
    // the 0.5 m/px scenes draw a realistic feature load whichever leaves the view straddles.
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

/// The deterministic bench fixture: a two-LOD map whose bytes are identical on every machine,
/// forever — the `obc-bench` frame hashes are computed over renders of it, so any byte drift here
/// invalidates the committed golden file.
///
/// - Coarse LOD (`max_mpp = ∞`): a uniform depth-2 quadtree, 16 leaves with one 4 KB chunk each,
///   holding about 3 620 features — over the frame's feature ceiling `obc_render::MAX_SPANS` in a
///   full-map view, so the overview scenes saturate the span buffer and take the priority-drop
///   path.
/// - Fine LOD (`max_mpp = 2.0`): a real depth-3 multi-chunk quadtree, 64 leaves with one chunk
///   each, built breadth-first exactly like the packer, holding the riding-zoom mix: per-leaf
///   backdrop polygons, buildings with and without holes, long 16-bit roads and short 8-bit paths
///   of varying vertex counts.
/// - Six styles spanning priorities 1–4 and z-indices −10…4, including an obvious backdrop at the
///   lowest z and line weights 1, 2 and 3, where weight 1 exercises the `Polyline` path and 2 or
///   more the span-stroke path.
///
/// Geometry is generated by the inline seeded [`BenchRng`] and packed through the same `pack_*`
/// encoders the format tests use, so a format layout bump lands here automatically.
pub fn build_bench_map() -> Vec<u8> {
    const CHUNK: usize = 4096; // the packer's default — every chunk stays cacheable
    let styles: [Style; 6] = [
        (1, -10, 0xD6DA, 0, 1, false, None), // land backdrop — lowest z, fills under everything
        (2, -5, 0x64DD, 0, 2, false, None),  // water
        (6, 1, 0x9CD3, 0, 3, false, None),   // buildings
        (5, 4, 0xFC00, 3, 2, false, None),   // major road, weight 3
        (4, 3, 0xFEA0, 2, 3, false, None),   // secondary road, weight 2
        (3, 2, 0xFFFF, 1, 4, false, None),   // minor path, weight 1 (Polyline path)
    ];

    let mut rng = BenchRng(0x0BC0_0327_D00D_F00D); // hard-coded seed — never change casually
    let (coarse_index, coarse_leaves) = uniform_quadtree(BENCH_BBOX, 2);
    let coarse_chunks: Vec<Vec<u8>> =
        coarse_leaves.iter().map(|&leaf| seal(bench_coarse_chunk(&mut rng, leaf), CHUNK)).collect();
    let (fine_index, fine_leaves) = uniform_quadtree(BENCH_BBOX, 3);
    let fine_chunks: Vec<Vec<u8>> =
        fine_leaves.iter().map(|&leaf| seal(bench_fine_chunk(&mut rng, leaf), CHUNK)).collect();

    build_file(
        BENCH_BBOX,
        &styles,
        &[
            // Strictly decreasing max_mpp, coarse first — the LOD-table ordering the reader expects.
            LodSpec { max_mpp: f32::INFINITY, index: coarse_index, chunks: coarse_chunks, chunk_size: CHUNK },
            LodSpec { max_mpp: 2.0, index: fine_index, chunks: fine_chunks, chunk_size: CHUNK },
        ],
    )
}

/// A line whose declared exterior point count — the `uint16` in the feature header — is set
/// independently of the `deltas` actually written. The reader trusts that count and loops
/// `decl_count - 1` deltas, so a count larger than `1 + deltas.len()` forges a header that runs
/// past the bytes present, and, sized right, past the reader's `MAX_FEAT_PTS` scratch. That drives
/// the scratch-overflow and truncated-ring guards the count-correct [`pack_line`] never reaches.
pub fn pack_line_decl(style_id: u8, ax: i32, ay: i32, decl_count: u16, deltas: &[(i8, i8)]) -> Vec<u8> {
    // line, 8-bit deltas — no flags. Count is forged, not derived from `deltas`.
    let mut v = feature_header(style_id, decl_count, ax, ay, 0);
    push_deltas8(&mut v, deltas);
    v
}

/// A hole-free polygon whose declared exterior point count is forged independently of the `deltas`
/// written — the polygon analogue of [`pack_line_decl`], used to overrun the reader's
/// `MAX_FEAT_PTS` exterior scratch with one big feature.
pub fn pack_poly_decl(style_id: u8, ax: i32, ay: i32, decl_count: u16, deltas: &[(i8, i8)]) -> Vec<u8> {
    // polygon, no holes, 8-bit deltas. Count is forged, not derived from `deltas`.
    let mut v = feature_header(style_id, decl_count, ax, ay, FEATURE_FLAG_POLYGON);
    push_deltas8(&mut v, deltas);
    v
}
