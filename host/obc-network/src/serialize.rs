//! Place, graph and snap section encoding.
use crate::nav::{polyline_len_m, NavGraph};
use obc_elevation::ElevationSource;
use obc_formats::obcm::{
    nav_edge_id, settlement_class_of, BRANCH_BIT, CHUNK_END, EMPTY_LEAF, FILLER, NAV_CHUNK_SIZE, NAV_EDGE_FIXED_LEN,
    NAV_EDGE_MAX_CHUNKS, NAV_EDGE_MAX_RECORDS_PER_CHUNK, NAV_MAX_DEGREE, NAV_MAX_PROFILES, NAV_NEIGHBOR_LEN,
    NAV_NODE_FIXED_LEN, NAV_SNAP_ANCHOR_GAP_M, NAV_SNAP_EDGE_MIN_M, NAV_SNAP_RECORD_LEN, POI_CHUNK_SIZE,
    POI_HOURS_BLOB_LEN, POI_HOURS_REF_NONE, POI_NAME_LEN, POI_RECORD_LEN, SETTLEMENT_CATEGORY_ID, SUMMIT_CATEGORY_ID,
    SUMMIT_ELEVATION_UNKNOWN, SUMMIT_SUBTYPE_ID,
};
use obc_map_core::serialize::{
    densify, emit_nav_section, emit_poi_section, pack_profile_table, NavBody, NavProfile, PoiBytes,
};
use obc_map_core::tree::{flatten_tree, FlattenTree, TreeWalk};
use obc_map_scene::ground_dist_m;
use obc_places::metadata::{table_row, Poi};
use std::io;

// A cap-degree record must fit one chunk, or `pack_nav_chunk` would drop real junctions.
const _: () = assert!(NAV_NODE_FIXED_LEN + NAV_MAX_DEGREE * NAV_NEIGHBOR_LEN <= NAV_CHUNK_SIZE);

/// Max endpoint-to-endpoint µdeg delta (lat or lon) a serialized adjacency piece may span, so every
/// neighbor entry's `dlat`/`dlon` fits `i16`. The serializer's long-edge split re-checks it, because
/// splitting a densified polyline into pieces can otherwise land a synthetic junction farther than
/// `i16` from its neighbor.
const NAV_MAX_NEIGHBOR_DELTA: i64 = 32_000;

// The reader treats `profile_count == 0` as malformed, so the packer must never write zero.
const _: () = assert!(NAV_MAX_PROFILES <= u8::MAX as usize);

/// Max polyline points of one serialized edge record: a record must never straddle a chunk
/// boundary, so it is bounded by the chunk itself. An edge whose densified polyline is longer, or
/// whose pieces would span more than [`NAV_MAX_NEIGHBOR_DELTA`], is split at a vertex into pieces
/// joined by synthetic degree-2 nodes — routing-neutral, and one chunk-sized read per edge.
pub const NAV_MAX_EDGE_PTS: usize = (NAV_CHUNK_SIZE - NAV_EDGE_FIXED_LEN) / 4 + 1;

/// A POI record's absolute microdegree coordinates plus the fields packed into its record. Owned so
/// the tree can move records into leaves. The trailer holds a service-hours reference, a summit
/// height or a population.
struct PoiPoint {
    metadata: obc_formats::obcm::PoiMetadata,
    lon_udeg: i32,
    lat_udeg: i32,
    subtype: u8,
    name: Option<String>,
    payload: u16,
}

/// One node of a category's POI quadtree, mirroring the geometry [`Node`] shape so the shared
/// [`flatten_tree`] serializes it to the identical index layout.
enum PoiNode {
    Leaf(Vec<PoiPoint>),
    Branch(Box<[PoiNode; 4]>),
}

impl TreeWalk for PoiNode {
    fn children(&self) -> Option<&[PoiNode; 4]> {
        match self {
            PoiNode::Leaf(_) => None,
            PoiNode::Branch(children) => Some(children),
        }
    }
}

impl FlattenTree for PoiNode {
    fn pack_leaf(&self, chunk_size: usize) -> Option<(Vec<u8>, usize)> {
        match self {
            PoiNode::Leaf(points) if !points.is_empty() => Some(pack_poi_chunk(points, chunk_size)),
            _ => None,
        }
    }
}

/// Pack one POI record. Names truncate only at UTF-8 character boundaries.
fn pack_poi_record(p: &PoiPoint) -> [u8; POI_RECORD_LEN] {
    let mut rec = [CHUNK_END; POI_RECORD_LEN];
    rec[0..4].copy_from_slice(&p.lat_udeg.to_le_bytes());
    rec[4..8].copy_from_slice(&p.lon_udeg.to_le_bytes());
    rec[8] = p.subtype;
    let name = p.name.as_deref().unwrap_or("");
    let bytes = name.as_bytes();
    let mut len = bytes.len().min(POI_NAME_LEN);
    while !name.is_char_boundary(len) {
        len -= 1;
    }
    rec[9] = len as u8;
    rec[10..10 + len].copy_from_slice(&bytes[..len]);
    // rec[10 + len .. 34] stays 0xFF (name pad).
    rec[34..36].copy_from_slice(&p.payload.to_le_bytes());
    rec[36..64].copy_from_slice(&p.metadata.encode());
    rec
}

/// Pack a leaf's POI records into one `chunk_size`-byte chunk: as many fixed records as fit, back to
/// back, then a `0xFF` subtype sentinel and `0xFF` padding. `build_poi_tree` splits a leaf before it
/// exceeds the capacity, so `dropped` is the safety net for the one case the tree cannot split away:
/// more POIs than a chunk holds inside the ~1 m recursion floor. Truncating loudly beats corrupting
/// the chunk.
fn pack_poi_chunk(points: &[PoiPoint], chunk_size: usize) -> (Vec<u8>, usize) {
    let capacity = chunk_size / POI_RECORD_LEN;
    let kept = points.len().min(capacity);
    let mut data = Vec::with_capacity(chunk_size);
    for p in &points[..kept] {
        data.extend_from_slice(&pack_poi_record(p));
    }
    // The 0xFF subtype byte ends the records (mirroring the geometry chunk's style-id sentinel);
    // `resize` writes it and the rest of the padding in one go.
    data.resize(chunk_size, CHUNK_END);
    (data, points.len() - kept)
}

/// Build one category's POI quadtree over the global bbox, splitting a leaf once it holds more
/// points than one chunk can carry. Split geometry matches the reader's `walk_leaves` exactly:
/// floor-division midpoints, NW/NE/SW/SE order, and a 10-µdeg recursion guard so a dense cluster
/// cannot recurse forever.
fn build_poi_tree(points: Vec<PoiPoint>, bbox: (i64, i64, i64, i64), capacity: usize) -> PoiNode {
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    // Fits a chunk, or the box is too small to subdivide. The guard matches the geometry
    // quadtree's 10-µdeg floor so both agree on when to stop.
    if points.len() <= capacity || max_lon - min_lon < 10 || max_lat - min_lat < 10 {
        return PoiNode::Leaf(points);
    }
    let mid_lon = (min_lon + max_lon).div_euclid(2);
    let mid_lat = (min_lat + max_lat).div_euclid(2);
    // West is `lon < mid` and South is `lat < mid`, so a point exactly on a midline lands in the
    // East or North child — inside that child's bbox either way.
    let mut nw = Vec::new();
    let mut ne = Vec::new();
    let mut sw = Vec::new();
    let mut se = Vec::new();
    for p in points {
        let west = (p.lon_udeg as i64) < mid_lon;
        let south = (p.lat_udeg as i64) < mid_lat;
        match (west, south) {
            (true, false) => nw.push(p),
            (false, false) => ne.push(p),
            (true, true) => sw.push(p),
            (false, true) => se.push(p),
        }
    }
    PoiNode::Branch(Box::new([
        build_poi_tree(nw, (min_lon, mid_lat, mid_lon, max_lat), capacity), // NW
        build_poi_tree(ne, (mid_lon, mid_lat, max_lon, max_lat), capacity), // NE
        build_poi_tree(sw, (min_lon, min_lat, mid_lon, mid_lat), capacity), // SW
        build_poi_tree(se, (mid_lon, min_lat, max_lon, mid_lat), capacity), // SE
    ]))
}

/// Serialize the full POI section: the directory, then each category's quadtree index and data
/// chunks, then the shared hours-pool section at the tail. `pois` is the deduped classified list,
/// bucketed by its subtype's category. Every category gets a directory entry, empty or not; summits
/// and settlements add theirs only when the map holds one. `section_offset` is the section's
/// absolute byte offset, because the directory's offsets are file-absolute.
///
/// The hours pool is built once over the whole list: identical weekly-schedule blobs collapse to
/// one, and each POI's `hours_ref` is stamped onto its record before tree-building, so it travels
/// into the right leaf.
pub fn serialize_poi_section(
    pois: &[Poi],
    global_bbox: (i64, i64, i64, i64),
    section_offset: usize,
) -> io::Result<Vec<u8>> {
    // `refs[k]` is POI k's 0-based pool index (or `None` for no hours), aligned to `pois`.
    let (pool, refs) =
        obc_places::hours::build_hours_pool(pois, |p| has_hours(p.subtype).then_some(p.hours.as_ref()).flatten());
    serialize_poi_pool(pois, global_bbox, section_offset, &pool, &refs)
}

/// Only a service place carries an hours reference in its payload. A summit holds an elevation there
/// and a settlement a population, so neither joins the pool.
pub fn has_hours(subtype: u8) -> bool {
    subtype != SUMMIT_SUBTYPE_ID && settlement_class_of(subtype).is_none()
}

pub fn serialize_poi_pool(
    pois: &[Poi],
    global_bbox: (i64, i64, i64, i64),
    section_offset: usize,
    pool: &[[u8; POI_HOURS_BLOB_LEN]],
    refs: &[Option<u16>],
) -> io::Result<Vec<u8>> {
    if pool.len() >= POI_HOURS_REF_NONE as usize {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "hours pool exceeds its record limit"));
    }
    let mut category_ids: Vec<_> = obc_formats::obcm::PoiCategory::ALL.iter().map(|c| c.id()).collect();
    if pois.iter().any(|p| p.subtype == SUMMIT_SUBTYPE_ID) {
        category_ids.push(SUMMIT_CATEGORY_ID);
    }
    if pois.iter().any(|p| settlement_class_of(p.subtype).is_some()) {
        category_ids.push(SETTLEMENT_CATEGORY_ID);
    }
    category_ids.sort_unstable();
    let category_count = category_ids.len();
    let mut by_cat: Vec<Vec<PoiPoint>> = (0..=category_ids.last().copied().unwrap_or(0)).map(|_| Vec::new()).collect();
    for (p, hours_ref) in pois.iter().zip(refs.iter()) {
        let cat = table_row(p.subtype).category() as usize;
        by_cat[cat].push(PoiPoint {
            metadata: p.metadata,
            lon_udeg: p.lon_udeg,
            lat_udeg: p.lat_udeg,
            subtype: p.subtype,
            name: p.name.clone(),
            payload: match p.subtype {
                SUMMIT_SUBTYPE_ID => p.elevation_m.unwrap_or(SUMMIT_ELEVATION_UNKNOWN) as u16,
                s if settlement_class_of(s).is_some() => obc_places::metadata::settlement_payload(p.population),
                _ => hours_ref.unwrap_or(POI_HOURS_REF_NONE),
            },
        });
    }

    // Records per chunk = chunk_size / record_len, so a leaf holds at most that many before the
    // tree splits.
    let capacity = POI_CHUNK_SIZE / POI_RECORD_LEN;

    // Flatten every category's tree first; the directory's index offsets are then laid out
    // sequentially after the fixed-size directory itself.
    let mut blocks = Vec::with_capacity(category_count);
    for cat_id in category_ids {
        let pts = std::mem::take(&mut by_cat[cat_id as usize]);
        if pts.is_empty() {
            blocks.push(PoiBytes { cat_id, index: Vec::new(), node_count: 0, chunks: Vec::new(), chunk_count: 0 });
            continue;
        }
        let root = build_poi_tree(pts, global_bbox, capacity);
        let (index, node_count, chunks, dropped) = flatten_tree(&root, POI_CHUNK_SIZE);
        if dropped != 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "POI density exceeds the chunk capacity"));
        }
        // POI chunks keep the fixed stride and have no offset table, so the reader's chunk range
        // stays the plain `k * chunk_size`.
        let chunk_count = chunks.len() as u32;
        blocks.push(PoiBytes { cat_id, index, node_count, chunks: chunks.concat(), chunk_count });
    }

    Ok(emit_poi_section(section_offset, &blocks, pool))
}

/// One adjacency entry of a junction record, holding the neighbor's absolute µdeg coords (the
/// serializer turns them into `i16` deltas from the owning record at pack time), the resolved wire
/// `edge_id`, the edge's ground `cost_m`, its `way_kind` class byte, and `ascent_m` — the climb of
/// riding the edge toward this neighbor, the one field the two sides of an edge legitimately
/// disagree on.
struct WireNeighbor {
    id: u32,
    lat: i32,
    lon: i32,
    edge_id: u32,
    cost_m: u32,
    way_kind: u8,
    ascent_m: u16,
}

/// A junction node ready to serialize: absolute µdeg coords, its dense id, and its capped neighbor
/// list.
struct NavPoint {
    lat: i32,
    lon: i32,
    id: u32,
    neighbors: Vec<WireNeighbor>,
}

impl NavPoint {
    fn record_len(&self) -> usize {
        NAV_NODE_FIXED_LEN + self.neighbors.len() * NAV_NEIGHBOR_LEN
    }
}

/// One node of the nav quadtree, mirroring [`PoiNode`] so the shared [`flatten_tree`] gives the
/// identical index layout. Leaves split on packed bytes, not record count.
enum NavTreeNode {
    Leaf(Vec<NavPoint>),
    Branch(Box<[NavTreeNode; 4]>),
}

/// Pack one junction record. Coordinates are absolute µdeg in the head, lat first; each neighbor's
/// coord is an `i16` delta from this record's own lat/lon (the edge splits guarantee the delta
/// fits), so relaxation reconstructs `neighbor = node + delta` exactly.
fn pack_nav_record(p: &NavPoint, out: &mut Vec<u8>) {
    out.extend_from_slice(&p.lat.to_le_bytes());
    out.extend_from_slice(&p.lon.to_le_bytes());
    out.extend_from_slice(&p.id.to_le_bytes());
    debug_assert!(p.neighbors.len() <= NAV_MAX_DEGREE, "degree capped before packing");
    out.push(p.neighbors.len() as u8);
    for n in &p.neighbors {
        let dlat = n.lat as i64 - p.lat as i64;
        let dlon = n.lon as i64 - p.lon as i64;
        debug_assert!(
            (-NAV_MAX_NEIGHBOR_DELTA..=NAV_MAX_NEIGHBOR_DELTA).contains(&dlat)
                && (-NAV_MAX_NEIGHBOR_DELTA..=NAV_MAX_NEIGHBOR_DELTA).contains(&dlon),
            "N1 + the serializer's split guarantee neighbor deltas fit i16 ({dlat},{dlon})"
        );
        debug_assert!(n.cost_m <= u16::MAX as u32, "N1 guarantees cost_m ≤ 60 000, fits u16 ({})", n.cost_m);
        out.extend_from_slice(&n.id.to_le_bytes());
        out.extend_from_slice(&(dlat as i16).to_le_bytes());
        out.extend_from_slice(&(dlon as i16).to_le_bytes());
        out.extend_from_slice(&n.edge_id.to_le_bytes());
        out.extend_from_slice(&(n.cost_m.min(u16::MAX as u32) as u16).to_le_bytes());
        out.push(n.way_kind);
        out.extend_from_slice(&n.ascent_m.to_le_bytes());
    }
}

/// Bin-pack the tree's leaves into 512-byte node chunks. A leaf's record block goes into the first
/// already-open chunk with room, so a small leaf back-fills the slack a larger one left.
///
/// Distinct leaves may therefore share a chunk id, and because first-fit reaches back they can be
/// spatially distant: a walk that visits several leaves of one chunk decodes that chunk once per
/// leaf and hands a consumer the same junction more than once. The reference consumers are
/// idempotent, so this is the section's contract, not a bug. A single leaf's records never straddle
/// chunks; a leaf larger than one chunk keeps what fits and counts the rest in `dropped`.
///
/// Returns the same shape as [`flatten_tree`], so the directory-writing code is shared.
fn flatten_nav_tree(root: &NavTreeNode) -> (Vec<u8>, u32, Vec<u8>, u32, usize) {
    flatten_binned_tree(root, NAV_CHUNK_SIZE)
}

/// A resident tree whose leaf records are first-fit packed without splitting a leaf across chunks.
/// Graph nodes and snap anchors share the traversal and binning, and keep their own record sizing
/// and wire encoding.
trait FlattenBinnedTree: TreeWalk {
    type Record;

    fn records(&self) -> Option<&[Self::Record]>;
    fn record_len(record: &Self::Record) -> usize;
    fn pack_record(record: &Self::Record, out: &mut Vec<u8>);
}

fn flatten_binned_tree<N: FlattenBinnedTree>(root: &N, chunk_size: usize) -> (Vec<u8>, u32, Vec<u8>, u32, usize) {
    let (nodes, first_child) = obc_tree_walk::breadth_first(root, TreeWalk::children);
    let mut index: Vec<u32> = Vec::with_capacity(nodes.len());
    let mut bins: Vec<Vec<u8>> = Vec::new();
    let mut dropped: usize = 0;
    for (idx, node) in nodes.iter().enumerate() {
        let records = match node.records() {
            None => {
                index.push(first_child[idx] as u32 | BRANCH_BIT);
                continue;
            }
            Some(records) if !records.is_empty() => records,
            Some(_) => {
                index.push(EMPTY_LEAF);
                continue;
            }
        };
        let leaf_len: usize = records.iter().map(N::record_len).sum();
        // First-fit: the first open chunk whose remaining space holds the whole leaf, else a new
        // one. A leaf larger than a whole chunk opens a fresh chunk and drops its overflow.
        let bin = match bins.iter().position(|b| b.len() + leaf_len <= chunk_size) {
            Some(c) => c,
            None => {
                bins.push(Vec::with_capacity(chunk_size));
                bins.len() - 1
            }
        };
        index.push((bin as u32) & !BRANCH_BIT);
        for record in records {
            if bins[bin].len() + N::record_len(record) > chunk_size {
                dropped += 1;
                continue; // co-located overflow inside one leaf — effectively impossible in real OSM
            }
            N::pack_record(record, &mut bins[bin]);
        }
    }

    // Concatenate the bins, each 0xFF-padded to a full chunk. The padding's first byte lands on a
    // `degree` slot, which gives the reader its end-of-chunk sentinel.
    let chunk_count = bins.len() as u32;
    let mut chunks: Vec<u8> = Vec::with_capacity(bins.len() * chunk_size);
    for mut b in bins {
        b.resize(chunk_size, CHUNK_END);
        chunks.extend_from_slice(&b);
    }

    let mut index_bytes = Vec::with_capacity(index.len() * 4);
    for v in &index {
        index_bytes.extend_from_slice(&v.to_le_bytes());
    }
    (index_bytes, index.len() as u32, chunks, chunk_count, dropped)
}

impl TreeWalk for NavTreeNode {
    fn children(&self) -> Option<&[NavTreeNode; 4]> {
        match self {
            NavTreeNode::Leaf(_) => None,
            NavTreeNode::Branch(children) => Some(children),
        }
    }
}

impl FlattenBinnedTree for NavTreeNode {
    type Record = NavPoint;

    fn records(&self) -> Option<&[NavPoint]> {
        match self {
            NavTreeNode::Leaf(points) => Some(points),
            NavTreeNode::Branch(_) => None,
        }
    }

    fn record_len(record: &NavPoint) -> usize {
        record.record_len()
    }

    fn pack_record(record: &NavPoint, out: &mut Vec<u8>) {
        pack_nav_record(record, out);
    }
}

/// Build the node quadtree over the global bbox, splitting a leaf once its packed records exceed one
/// chunk. Split geometry is identical to [`build_poi_tree`], so the reader's `walk_leaves` resolves
/// it verbatim.
fn build_nav_tree(points: Vec<NavPoint>, bbox: (i64, i64, i64, i64), chunk_size: usize) -> NavTreeNode {
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    let packed: usize = points.iter().map(NavPoint::record_len).sum();
    if packed <= chunk_size || max_lon - min_lon < 10 || max_lat - min_lat < 10 {
        return NavTreeNode::Leaf(points);
    }
    let mid_lon = (min_lon + max_lon).div_euclid(2);
    let mid_lat = (min_lat + max_lat).div_euclid(2);
    // Same midline rule as POIs: a point exactly on a midline lands East / North.
    let mut nw = Vec::new();
    let mut ne = Vec::new();
    let mut sw = Vec::new();
    let mut se = Vec::new();
    for p in points {
        let west = (p.lon as i64) < mid_lon;
        let south = (p.lat as i64) < mid_lat;
        match (west, south) {
            (true, false) => nw.push(p),
            (false, false) => ne.push(p),
            (true, true) => sw.push(p),
            (false, true) => se.push(p),
        }
    }
    NavTreeNode::Branch(Box::new([
        build_nav_tree(nw, (min_lon, mid_lat, mid_lon, max_lat), chunk_size), // NW
        build_nav_tree(ne, (mid_lon, mid_lat, max_lon, max_lat), chunk_size), // NE
        build_nav_tree(sw, (min_lon, min_lat, mid_lon, mid_lat), chunk_size), // SW
        build_nav_tree(se, (mid_lon, min_lat, max_lon, mid_lat), chunk_size), // SE
    ]))
}

/// A working edge on its way into the pool: endpoints (dense node ids), the densified polyline, the
/// cost carried into both endpoints' records, and the parent way's `kind` class byte, which every
/// split piece inherits.
struct WorkEdge {
    a: u32,
    b: u32,
    polyline: Vec<(i32, i32)>,
    cost_m: u32,
    kind: u8,
}

/// One interior lookup anchor. It is deliberately not a graph node: the coordinate only gets the
/// router to a small candidate set, after which the full edge geometry is projected exactly and
/// connected virtually to the edge's real endpoints.
struct SnapPoint {
    lat: i32,
    lon: i32,
    edge_id: u32,
}

enum SnapTreeNode {
    Leaf(Vec<SnapPoint>),
    Branch(Box<[SnapTreeNode; 4]>),
}

/// Place evenly-spaced interior anchors on one final serialized edge piece. Endpoints need no
/// records, because they already live in the node quadtree. `ceil(length / 300)` intervals keep
/// every gap at most 300 m even when the edge's rounded wire cost differs from the sum of its
/// floating segment lengths.
fn append_snap_points(edge: &WorkEdge, edge_id: u32, out: &mut Vec<SnapPoint>) {
    let seg_lens: Vec<f32> = edge.polyline.windows(2).map(|w| ground_dist_m(w[0], w[1])).collect();
    let length: f32 = seg_lens.iter().sum();
    if length <= NAV_SNAP_EDGE_MIN_M as f32 {
        return;
    }
    let intervals = (length / NAV_SNAP_ANCHOR_GAP_M as f32).ceil() as usize;
    let mut segment = 0usize;
    let mut before = 0.0f32;
    for i in 1..intervals {
        let target = length * i as f32 / intervals as f32;
        while segment + 1 < seg_lens.len() && before + seg_lens[segment] < target {
            before += seg_lens[segment];
            segment += 1;
        }
        let a = edge.polyline[segment];
        let b = edge.polyline[segment + 1];
        let t = ((target - before) / seg_lens[segment].max(f32::EPSILON)).clamp(0.0, 1.0);
        // Interpolate the small delta rather than the absolute microdegree coordinate: at real
        // latitudes an f32 cannot represent every i32 microdegree, while the per-segment delta can.
        let lon = a.0.saturating_add(((b.0 - a.0) as f32 * t).round() as i32);
        let lat = a.1.saturating_add(((b.1 - a.1) as f32 * t).round() as i32);
        out.push(SnapPoint { lat, lon, edge_id });
    }
}

fn build_snap_tree(points: Vec<SnapPoint>, bbox: (i64, i64, i64, i64)) -> SnapTreeNode {
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    if points.len() * NAV_SNAP_RECORD_LEN <= NAV_CHUNK_SIZE || max_lon - min_lon < 10 || max_lat - min_lat < 10 {
        return SnapTreeNode::Leaf(points);
    }
    let mid_lon = (min_lon + max_lon).div_euclid(2);
    let mid_lat = (min_lat + max_lat).div_euclid(2);
    let (mut nw, mut ne, mut sw, mut se) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for p in points {
        match ((p.lon as i64) < mid_lon, (p.lat as i64) < mid_lat) {
            (true, false) => nw.push(p),
            (false, false) => ne.push(p),
            (true, true) => sw.push(p),
            (false, true) => se.push(p),
        }
    }
    SnapTreeNode::Branch(Box::new([
        build_snap_tree(nw, (min_lon, mid_lat, mid_lon, max_lat)),
        build_snap_tree(ne, (mid_lon, mid_lat, max_lon, max_lat)),
        build_snap_tree(sw, (min_lon, min_lat, mid_lon, mid_lat)),
        build_snap_tree(se, (mid_lon, min_lat, max_lon, mid_lat)),
    ]))
}

/// Flatten the anchor quadtree with the node tree's first-fit leaf binning. Records keep absolute
/// coordinates, because distinct spatial leaves may share one chunk.
fn flatten_snap_tree(root: &SnapTreeNode) -> (Vec<u8>, u32, Vec<u8>, u32, usize) {
    flatten_binned_tree(root, NAV_CHUNK_SIZE)
}

impl TreeWalk for SnapTreeNode {
    fn children(&self) -> Option<&[SnapTreeNode; 4]> {
        match self {
            SnapTreeNode::Leaf(_) => None,
            SnapTreeNode::Branch(children) => Some(children),
        }
    }
}

impl FlattenBinnedTree for SnapTreeNode {
    type Record = SnapPoint;

    fn records(&self) -> Option<&[SnapPoint]> {
        match self {
            SnapTreeNode::Leaf(points) => Some(points),
            SnapTreeNode::Branch(_) => None,
        }
    }

    fn record_len(_: &SnapPoint) -> usize {
        NAV_SNAP_RECORD_LEN
    }

    fn pack_record(record: &SnapPoint, out: &mut Vec<u8>) {
        out.extend_from_slice(&record.lat.to_le_bytes());
        out.extend_from_slice(&record.lon.to_le_bytes());
        out.extend_from_slice(&record.edge_id.to_le_bytes());
    }
}

/// Densify one nav polyline so every `(dlat, dlon)` fits an `i16` — the same [`densify`] the
/// geometry rings use, over the same 30 000-µdeg threshold.
fn densify_polyline(pts: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut out64: Vec<(i64, i64)> = Vec::with_capacity(pts.len());
    out64.push((pts[0].0 as i64, pts[0].1 as i64));
    for w in pts.windows(2) {
        let last = *out64.last().unwrap();
        densify(last, (w[1].0 as i64, w[1].1 as i64), &mut out64);
    }
    // Midpoints interpolate between in-range i32 endpoints, so the cast is exact.
    out64.into_iter().map(|(x, y)| (x as i32, y as i32)).collect()
}

/// Pack one edge record. The polyline is already densified, so every delta fits `i16`.
fn pack_edge_record(e: &WorkEdge, elevation_complete: bool, out: &mut Vec<u8>) {
    out.extend_from_slice(&e.cost_m.to_le_bytes());
    out.extend_from_slice(&((e.polyline.len() as u16) | if elevation_complete { 0x8000 } else { 0 }).to_le_bytes());
    out.push(e.kind);
    out.extend_from_slice(&e.polyline[0].1.to_le_bytes()); // anchor lat
    out.extend_from_slice(&e.polyline[0].0.to_le_bytes()); // anchor lon
    for w in e.polyline.windows(2) {
        let dlat = w[1].1 - w[0].1;
        let dlon = w[1].0 - w[0].0;
        debug_assert!(i16::try_from(dlat).is_ok() && i16::try_from(dlon).is_ok(), "densified deltas must fit i16");
        out.extend_from_slice(&(dlat as i16).to_le_bytes());
        out.extend_from_slice(&(dlon as i16).to_le_bytes());
    }
}

/// Mints `Edge Id`s as records are placed in the edge pool: the id is a per-chunk record counter.
///
/// The ordinal restarts whenever the chunk does, which is not the same as whenever a record was
/// pushed past a boundary. A record whose length divides the space left in its chunk exactly ends
/// flush with it, and the next record then opens the next chunk with no filler and no push. Deriving
/// both halves from the byte the record lands on makes that case fall out.
#[derive(Default)]
struct EdgeIds {
    chunk: u32,
    ordinal: u32,
}

impl EdgeIds {
    /// The id of a record starting at pool byte `at`, which must already have been advanced past
    /// any no-straddle filler.
    fn mint(&mut self, at: usize) -> u32 {
        let chunk = (at / NAV_CHUNK_SIZE) as u32;
        if chunk != self.chunk {
            self.chunk = chunk;
            self.ordinal = 0;
        }
        // The producer cap, and the reason `0xFFFFFFFF` is an impossible id. The 19-byte minimum
        // record puts the real maximum at 26, so this never binds today; it is asserted so that it
        // stays true if a future record shrinks.
        assert!(
            (self.ordinal as usize) < NAV_EDGE_MAX_RECORDS_PER_CHUNK,
            "an edge chunk may hold at most {NAV_EDGE_MAX_RECORDS_PER_CHUNK} records"
        );
        let id = nav_edge_id(chunk, self.ordinal).expect("chunk index and ordinal are both in field");
        self.ordinal += 1;
        id
    }
}

/// Serialize the full nav-graph section at absolute byte `section_offset`:
/// `[directory][profile table][node quadtree index][node chunks][edge pool][snap index][snap
/// chunks]`. The profile table is written immediately after the directory, before the node index, so
/// even an empty graph carries its profiles; the section is always present.
///
/// Graph normalizations happen here, on working copies, so the caller's [`NavGraph`] is untouched.
/// Polylines are densified, and an edge whose record would overflow one chunk, or whose piece would
/// span more than [`NAV_MAX_NEIGHBOR_DELTA`], is split at a vertex into pieces joined by synthetic
/// degree-2 nodes, so no record straddles a chunk and every neighbor delta fits `i16`. A node keeps
/// its first [`NAV_MAX_DEGREE`] adjacency entries in edge-pool order and the packer warns about the
/// rest. A self-loop edge contributes one adjacency entry, not two.
///
/// `terrain` is where `Ascent M` comes from. It is sampled after every split, over each final
/// piece's own polyline, because an edge's total climb cannot be divided among its pieces after the
/// fact: the dead-band is a fold over samples, not a length. Hand it
/// [`NullElevation`](obc_elevation::NullElevation) and every entry gets `0`.
pub fn serialize_nav_section(
    graph: &NavGraph,
    profiles: &[NavProfile],
    global_bbox: (i64, i64, i64, i64),
    section_offset: usize,
    terrain: &mut dyn ElevationSource,
) -> Vec<u8> {
    let profile_table = pack_profile_table(profiles);
    if graph.nodes.is_empty() {
        return emit_nav_section(section_offset, &profile_table, None, profiles.len());
    }

    // R1 assigns dense ids in push order — the serializer indexes `coords` by id.
    debug_assert!(graph.nodes.iter().enumerate().all(|(i, n)| n.id as usize == i), "node ids are dense");
    let mut coords: Vec<(i32, i32)> = graph.nodes.iter().map(|n| n.coord).collect();

    // Densify, then split anything over one chunk's worth of points or whose piece would span more
    // than the i16-delta bound. Splitting appends synthetic nodes, so it runs before adjacency.
    let mut edges: Vec<WorkEdge> = Vec::with_capacity(graph.edges.len());
    for e in &graph.edges {
        let poly = densify_polyline(&e.polyline);
        // Fast path: a short polyline whose endpoints already fit the i16 neighbor delta. A
        // hand-built graph may not, so the span is re-checked rather than trusted.
        let (a, z) = (poly[0], *poly.last().unwrap());
        let span_ok = (a.0 as i64 - z.0 as i64).abs() <= NAV_MAX_NEIGHBOR_DELTA
            && (a.1 as i64 - z.1 as i64).abs() <= NAV_MAX_NEIGHBOR_DELTA;
        if poly.len() <= NAV_MAX_EDGE_PTS && span_ok {
            edges.push(WorkEdge { a: e.a, b: e.b, polyline: poly, cost_m: e.length_m, kind: e.kind });
            continue;
        }
        // Walk the long polyline in pieces bounded by both the chunk point cap and the i16 endpoint
        // span; each interior cut vertex becomes a synthetic junction. Densify caps each segment
        // below the span bound, so a piece always advances at least one vertex and the loop
        // terminates. Pieces are re-measured, so their costs sum to the original within rounding.
        let mut start = 0usize;
        let mut from = e.a;
        while start < poly.len() - 1 {
            let max_end = (start + NAV_MAX_EDGE_PTS - 1).min(poly.len() - 1);
            let mut end = start + 1;
            while end < max_end {
                let dlon = (poly[end + 1].0 as i64 - poly[start].0 as i64).abs();
                let dlat = (poly[end + 1].1 as i64 - poly[start].1 as i64).abs();
                if dlon > NAV_MAX_NEIGHBOR_DELTA || dlat > NAV_MAX_NEIGHBOR_DELTA {
                    break;
                }
                end += 1;
            }
            let piece = poly[start..=end].to_vec();
            let to = if end == poly.len() - 1 {
                e.b
            } else {
                let id = coords.len() as u32;
                coords.push(poly[end]);
                id
            };
            let cost_m = polyline_len_m(&piece);
            edges.push(WorkEdge { a: from, b: to, polyline: piece, cost_m, kind: e.kind });
            from = to;
            start = end;
        }
    }

    // The pool follows `obcm-assemble`'s emission order: lower endpoint by `(lat, lon)`, then the
    // upper one, then cost and kind. An assembly then reads each cell's pool front to back. The
    // sort is stable, so an exact tie keeps its input order.
    let lat_lon = |id: u32| {
        let (lon, lat) = coords[id as usize];
        (lat, lon)
    };
    edges.sort_by_key(|e| {
        let (a, b) = (lat_lon(e.a), lat_lon(e.b));
        (a.min(b), a.max(b), e.cost_m, e.kind)
    });

    // Edge pool: records back to back in `edges` order, each pushed to the next chunk start if it
    // would straddle a boundary.
    let edge_facts: Vec<_> = edges.iter().map(|e| crate::nav::integrate_edge_facts(&e.polyline, terrain)).collect();
    let mut pool: Vec<u8> = Vec::new();
    let mut edge_ids: Vec<u32> = Vec::with_capacity(edges.len());
    let mut ids = EdgeIds::default();
    for (e, facts) in edges.iter().zip(&edge_facts) {
        let rec_len = NAV_EDGE_FIXED_LEN + (e.polyline.len() - 1) * 4;
        debug_assert!(rec_len <= NAV_CHUNK_SIZE, "split bounded every record to one chunk");
        let within = pool.len() % NAV_CHUNK_SIZE;
        if within + rec_len > NAV_CHUNK_SIZE {
            pool.resize(pool.len() + (NAV_CHUNK_SIZE - within), FILLER);
        }
        edge_ids.push(ids.mint(pool.len()));
        pack_edge_record(e, facts.2, &mut pool);
    }
    pool.resize(pool.len().div_ceil(NAV_CHUNK_SIZE) * NAV_CHUNK_SIZE, FILLER);
    let edge_chunk_count = (pool.len() / NAV_CHUNK_SIZE) as u32;
    assert!(
        edge_chunk_count as u64 <= NAV_EDGE_MAX_CHUNKS,
        "the edge pool's {edge_chunk_count} chunks exceed the {NAV_EDGE_MAX_CHUNKS} an Edge Id can name (§8.4)"
    );

    // Adjacency with inline neighbor coords, capped at NAV_MAX_DEGREE.
    let mut adj: Vec<Vec<WireNeighbor>> = (0..coords.len()).map(|_| Vec::new()).collect();
    let mut truncated = 0usize;
    for ((e, &edge_id), &(ascent_ab, ascent_ba, _)) in edges.iter().zip(&edge_ids).zip(&edge_facts) {
        // The two entries of an edge differ in exactly one field: `a->b` books the climb of riding
        // the polyline forwards, `b->a` the climb of riding it backwards. A self-loop writes the
        // forward one only, matching its single entry.
        let mut push = |from: u32, to: u32, ascent_m: u16| {
            let list = &mut adj[from as usize];
            if list.len() >= NAV_MAX_DEGREE {
                truncated += 1;
                return;
            }
            let (lon, lat) = coords[to as usize];
            list.push(WireNeighbor { id: to, lat, lon, edge_id, cost_m: e.cost_m, way_kind: e.kind, ascent_m });
        };
        push(e.a, e.b, ascent_ab);
        if e.a != e.b {
            push(e.b, e.a, ascent_ba);
        }
    }
    if truncated > 0 {
        eprintln!("warning: {truncated} adjacency entrie(s) dropped at the degree cap ({NAV_MAX_DEGREE})");
    }

    let points: Vec<NavPoint> = coords
        .iter()
        .zip(adj)
        .enumerate()
        .map(|(id, (&(lon, lat), neighbors))| NavPoint { lat, lon, id: id as u32, neighbors })
        .collect();
    let root = build_nav_tree(points, global_bbox, NAV_CHUNK_SIZE);
    let (index, node_count, chunks, chunk_count, dropped) = flatten_nav_tree(&root);
    if dropped > 0 {
        // Co-located junctions inside the 10-µdeg split floor: effectively impossible in real
        // OSM, but never silent.
        eprintln!("warning: {dropped} nav node record(s) dropped (leaf overflow at the split floor)");
    }

    // The lookup-only interior anchors name the final pool ids, so they are generated after every
    // geometry split and after pool placement. A short edge contributes none: its endpoints in the
    // node quadtree already provide the same 300 m spacing contract.
    let mut snap_points = Vec::new();
    for (edge, &edge_id) in edges.iter().zip(&edge_ids) {
        append_snap_points(edge, edge_id, &mut snap_points);
    }
    let (snap_index, snap_node_count, snap_chunks, snap_chunk_count, snap_dropped) = if snap_points.is_empty() {
        (Vec::new(), 0, Vec::new(), 0, 0)
    } else {
        flatten_snap_tree(&build_snap_tree(snap_points, global_bbox))
    };
    if snap_dropped > 0 {
        eprintln!("warning: {snap_dropped} nav snap anchor(s) dropped (leaf overflow at the split floor)");
    }

    let body = NavBody {
        index: &index,
        node_count,
        chunks: &chunks,
        chunk_count,
        pool: &pool,
        edge_chunk_count,
        snap_index: &snap_index,
        snap_node_count,
        snap_chunks: &snap_chunks,
        snap_chunk_count,
    };
    emit_nav_section(section_offset, &profile_table, Some(&body), profiles.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::obcm::nav_edge_id_ordinal;
    #[test]
    fn an_edge_ordinal_restarts_with_the_chunk_not_with_the_push() {
        // 19 + 19 + 19 + 455 = 512 exactly: the fourth record ends on the boundary.
        let flush = [19usize, 19, 19, 455, 19, 19];
        let mut ids = EdgeIds::default();
        let mut at = 0usize;
        let mut minted = Vec::new();
        for len in flush {
            let within = at % NAV_CHUNK_SIZE;
            if within + len > NAV_CHUNK_SIZE {
                at += NAV_CHUNK_SIZE - within;
            }
            minted.push(ids.mint(at));
            at += len;
        }
        let decoded: Vec<(u32, u32)> =
            minted.iter().map(|&id| (obc_formats::obcm::nav_edge_id_chunk(id), nav_edge_id_ordinal(id))).collect();
        assert_eq!(decoded, [(0, 0), (0, 1), (0, 2), (0, 3), (1, 0), (1, 1)]);
        // Distinctness is the property that matters, and what a push-driven counter would break.
        let mut sorted = minted.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), minted.len(), "every record gets its own id");
    }

    #[test]
    fn an_edge_pushed_past_a_boundary_opens_the_next_chunk_at_ordinal_zero() {
        let mut ids = EdgeIds::default();
        let mut at = 0usize;
        let mut minted = Vec::new();
        for len in [500usize, 19, 19] {
            let within = at % NAV_CHUNK_SIZE;
            if within + len > NAV_CHUNK_SIZE {
                at += NAV_CHUNK_SIZE - within;
            }
            minted.push(ids.mint(at));
            at += len;
        }
        let decoded: Vec<(u32, u32)> =
            minted.iter().map(|&id| (obc_formats::obcm::nav_edge_id_chunk(id), nav_edge_id_ordinal(id))).collect();
        assert_eq!(decoded, [(0, 0), (1, 0), (1, 1)]);
    }
}
