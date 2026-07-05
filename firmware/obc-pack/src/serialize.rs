//! OBCM v6 serializer — lay out the `.obcm` bytes per `OBCM_Spec.md`.
//!
//! Deterministic: same feature list + quadtree → same output. Geometry arrives
//! already clipped + simplified; this module rounds lon/lat to microdegrees
//! (round-half-to-even), densifies long segments, delta-encodes rings, and lays out
//! the chunk / index / LOD-table / header bytes. The trailing **POI section** (v6,
//! §7 of the spec) is a per-category quadtree over fixed 32-byte point records,
//! reusing the same BFS-flatten + u32 node encoding as the geometry tree.

use std::io::{self, Seek, SeekFrom, Write};

use obc_reader::format::{
    BRANCH_BIT, EMPTY_LEAF, FEATURE_FLAG_16BIT, FEATURE_FLAG_HOLES, FEATURE_FLAG_POLYGON, STYLE_PRIORITY_MASK,
};

use crate::poi::{table_row, Poi};

/// Max delta (microdegrees) before a segment is densified to keep deltas in
/// 16-bit range. Crate-visible so `geom::packed_size_budget` can count the
/// midpoints `densify` will insert.
pub(crate) const MAX_SEGMENT: i64 = 30_000;

/// Fixed header length (bytes) — v6 appended the 4-byte POI Section Offset (§1).
pub const HEADER_LEN: usize = 36;
/// One LOD-table entry, `<fIIHI>`.
pub const LOD_ENTRY_LEN: usize = 18;

/// The OBCM format version byte written into the header (`OBCM_Spec.md` §1).
pub const OBCM_VERSION: u8 = 6;

/// POI category count baked into every v6 map's directory (§7.1). Fixed by the
/// canonical [`crate::poi::POI_TABLE`]: category ids 1..=6.
pub const POI_CATEGORY_COUNT: u8 = 6;

/// One 32-byte POI record (§7.3): `int32 lat, int32 lon, u8 subtype, u8 name_len,
/// [u8; 20] name, [0xFF; 2] reserved`.
pub const POI_RECORD_LEN: usize = 32;

/// The `Name` field width inside a POI record (§7.3): 20 bytes, `0xFF`-padded.
pub const POI_NAME_LEN: usize = 20;

/// Fixed POI chunk capacity (bytes) the packer writes (§7.1). 512 ⇒ 16 records per
/// chunk. Shared by every category, stored once in the directory's `Chunk Size`.
pub const POI_CHUNK_SIZE: usize = 512;

/// One POI-directory category entry (§7.1): `u8 category_id, u32 index_offset,
/// u32 index_node_count, u32 chunk_count`.
pub const POI_CAT_ENTRY_LEN: usize = 13;

/// Largest `chunk_size` (bytes) that keeps every feature within the reader's
/// [`obc_reader::MAX_FEAT_PTS`] vertex cap. Densest encoding is 8-bit deltas
/// (12-byte header + 2 bytes/vertex), so a chunk carries at most
/// `(chunk_size - 12) / 2 + 1` vertices. Above this the reader **silently
/// truncates** past-cap vertices (`heapless` push fails, no error either side),
/// corrupting the feature's fill/stroke.
pub const MAX_SAFE_CHUNK_SIZE: usize = (obc_reader::MAX_FEAT_PTS - 1) * 2 + 12;

// The safe ceiling must itself fit the on-wire `u16` chunk_size field, or the bound is moot.
const _: () = assert!(MAX_SAFE_CHUNK_SIZE <= u16::MAX as usize, "chunk_size is a u16 in the format");

/// Smallest accepted `chunk_size` (bytes). The format decodes any positive size,
/// but below this even modest features exceed the chunk and [`pack_chunk`] drops
/// them wholesale — the pack "succeeds" and the map is silently near-empty. The
/// value is a judgment call (a few dozen mid-size features per chunk), kept in
/// lock-step with the schema's `chunk_size.minimum`.
pub const MIN_CHUNK_SIZE: usize = 256;

/// Reject a `chunk_size` outside [`MIN_CHUNK_SIZE`]..=[`MAX_SAFE_CHUNK_SIZE`]:
/// above the max the reader silently truncates vertices, below the min features
/// get dropped wholesale. Either way a misconfigured pack fails loudly.
pub fn validate_chunk_size(chunk_size: usize) -> Result<(), String> {
    if chunk_size > MAX_SAFE_CHUNK_SIZE {
        return Err(format!(
            "chunk_size {chunk_size} exceeds the safe maximum {MAX_SAFE_CHUNK_SIZE}: a single feature \
             could then pack more than {} vertices, which the device reader silently truncates \
             (issue #2). Lower chunk_size, or raise the LOD's simplify tolerance.",
            obc_reader::MAX_FEAT_PTS
        ));
    }
    if chunk_size < MIN_CHUNK_SIZE {
        return Err(format!(
            "chunk_size {chunk_size} is below the minimum {MIN_CHUNK_SIZE}: features larger than the \
             chunk are dropped at pack time, so a tiny chunk_size produces a mostly-empty map."
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

/// f64 lon/lat, rounded to microdegrees and densified here. `rings[0]` is the
/// exterior; `rings[1..]` are interior rings (polygons only). Lines carry one ring.
#[derive(Debug, Clone)]
pub struct Feature {
    pub style_id: u8,
    pub kind: Kind,
    pub rings: Vec<Vec<(f64, f64)>>,
}

/// Child bboxes are re-derived by the reader, so branches store only their four
/// children (order NW, NE, SW, SE); only leaf bboxes are kept (for anchors).
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

/// `v * 1e6` to microdegrees, **round-half-to-even** — NOT `f64::round`
/// (half-away-from-zero), which would shift vertices by a microdegree. The value is
/// integer-valued before the `as i64`, so the cast is exact. Crate-visible so
/// `geom::packed_size_budget` rounds exactly like the packer does.
#[inline]
pub(crate) fn to_udeg(v: f64) -> i64 {
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
        let steps = max_dist / MAX_SEGMENT + 1;
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
/// `flags = (priority-1) & STYLE_PRIORITY_MASK`.
pub fn pack_style_dict(styles: &[Style]) -> Vec<u8> {
    let mut styles = styles.to_vec();
    styles.sort_by_key(|s| s.id);
    let mut data = Vec::with_capacity(1 + styles.len() * 6);
    data.push(styles.len() as u8);
    for s in &styles {
        let priority = (s.priority as i32).clamp(1, 4);
        let flags = (priority - 1) as u8 & STYLE_PRIORITY_MASK;
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
        flags |= FEATURE_FLAG_POLYGON;
        if f.rings.len() > 1 {
            flags |= FEATURE_FLAG_HOLES;
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
        flags |= FEATURE_FLAG_16BIT;
    }

    // The reader decodes a whole feature (exterior + holes) into one `MAX_FEAT_PTS`
    // buffer; past that `heapless` silently drops vertices. `validate_chunk_size`
    // guards it, but assert the real invariant in debug.
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

    if flags & FEATURE_FLAG_HOLES != 0 {
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
/// would overflow the chunk (and every feature after it) is dropped; the second
/// return value is the number dropped, so callers can warn instead of losing
/// map content silently.
pub fn pack_chunk(features: &[Feature], node_bbox: (i64, i64, i64, i64), chunk_size: usize) -> (Vec<u8>, usize) {
    let mut data = Vec::new();
    let mut kept = 0usize;
    for f in features {
        let packed = pack_feature(f, node_bbox);
        if data.len() + packed.len() > chunk_size {
            break;
        }
        data.extend_from_slice(&packed);
        kept += 1;
    }
    data.resize(chunk_size, 0xFF);
    (data, features.len() - kept)
}

/// One node of an abstract quadtree, for the shared BFS-flatten [`flatten_tree`]:
/// either a leaf that packs into (at most) one chunk, or a branch over its four
/// NW/NE/SW/SE children. The geometry [`Node`] and the POI [`PoiNode`] both view
/// as this so the index-byte layout (branch bit / empty-leaf sentinel / chunk id)
/// lives in exactly one place.
enum TreeNode<'a, N> {
    /// A leaf. `pack` returns `None` for an empty leaf (→ [`EMPTY_LEAF`]) or the
    /// leaf's chunk bytes + its own chunk-overflow drop count.
    Leaf(&'a N),
    /// A branch over its four children in NW/NE/SW/SE order.
    Branch(&'a [N; 4]),
}

/// A quadtree the BFS-flatten can walk: classify a node into leaf/branch, and pack
/// one leaf into its chunk. Implemented for the geometry [`Node`] and the POI
/// [`PoiNode`], so [`flatten_tree`] serializes both to the identical index layout.
trait FlattenTree: Sized {
    /// View this node as a leaf or a branch over its four children.
    fn classify(&self) -> TreeNode<'_, Self>;
    /// Pack a leaf's payload into its chunk: `None` for an empty leaf (no chunk),
    /// else `(chunk_bytes, dropped)` where `dropped` is the chunk-overflow count.
    fn pack_leaf(&self, chunk_size: usize) -> Option<(Vec<u8>, usize)>;
}

/// Flatten any [`FlattenTree`] into `(index_bytes, node_count, chunk_bytes,
/// chunk_count, dropped)` via BFS. Child order and chunk-id assignment order are
/// BFS, which fixes the byte layout: a branch's four children are appended
/// contiguously, so its first-child index is the node count at the moment it is
/// expanded (`child > idx` always — the invariant the reader's `walk_leaves`
/// relies on). `dropped` is the total chunk-overflow drop count across all leaves.
fn flatten_tree<N: FlattenTree>(root: &N, chunk_size: usize) -> (Vec<u8>, u32, Vec<u8>, u32, usize) {
    // BFS in enqueue order. Children are appended contiguously, so a branch's
    // first-child index is the length of `nodes` at the moment we expand it.
    let mut nodes: Vec<&N> = vec![root];
    let mut first_child: Vec<usize> = vec![0];
    let mut i = 0;
    while i < nodes.len() {
        if let TreeNode::Branch(children) = nodes[i].classify() {
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
    let mut dropped: usize = 0;
    for (idx, node) in nodes.iter().enumerate() {
        match node.classify() {
            TreeNode::Leaf(leaf) => match leaf.pack_leaf(chunk_size) {
                None => index.push(EMPTY_LEAF),
                Some((chunk, chunk_dropped)) => {
                    let chunk_id = chunk_count;
                    chunks.extend_from_slice(&chunk);
                    dropped += chunk_dropped;
                    chunk_count += 1;
                    index.push(chunk_id & !BRANCH_BIT);
                }
            },
            TreeNode::Branch(_) => index.push(first_child[idx] as u32 | BRANCH_BIT),
        }
    }

    let mut index_bytes = Vec::with_capacity(index.len() * 4);
    for v in &index {
        index_bytes.extend_from_slice(&v.to_le_bytes());
    }
    (index_bytes, index.len() as u32, chunks, chunk_count, dropped)
}

impl FlattenTree for Node {
    fn classify(&self) -> TreeNode<'_, Node> {
        match self {
            Node::Leaf { .. } => TreeNode::Leaf(self),
            Node::Branch(children) => TreeNode::Branch(children),
        }
    }
    fn pack_leaf(&self, chunk_size: usize) -> Option<(Vec<u8>, usize)> {
        match self {
            Node::Leaf { bbox, features } if !features.is_empty() => Some(pack_chunk(features, *bbox, chunk_size)),
            _ => None,
        }
    }
}

/// Flatten one geometry quadtree into `(index_bytes, node_count, chunk_bytes,
/// chunk_count, dropped_features)` via BFS. Thin wrapper over the shared
/// [`flatten_tree`]; `dropped_features` counts chunk-overflow drops across all
/// leaves (see [`pack_chunk`]).
pub fn serialize_tree(root: &Node, chunk_size: usize) -> (Vec<u8>, u32, Vec<u8>, u32, usize) {
    flatten_tree(root, chunk_size)
}

// --- POI section (v6, spec §7) ------------------------------------------------

/// A POI record's absolute microdegree coordinates + the fields packed into its
/// 32-byte record (§7.3). Owned so the tree can move records into leaves.
struct PoiPoint {
    lon_udeg: i32,
    lat_udeg: i32,
    subtype: u8,
    name: Option<String>,
}

/// One node of a category's POI quadtree, mirroring the geometry [`Node`] shape so
/// the shared [`flatten_tree`] serializes it to the identical index layout. A leaf
/// carries the points that fall inside its bbox; a branch its four NW/NE/SW/SE
/// children.
enum PoiNode {
    Leaf(Vec<PoiPoint>),
    Branch(Box<[PoiNode; 4]>),
}

impl FlattenTree for PoiNode {
    fn classify(&self) -> TreeNode<'_, PoiNode> {
        match self {
            PoiNode::Leaf(_) => TreeNode::Leaf(self),
            PoiNode::Branch(children) => TreeNode::Branch(children),
        }
    }
    fn pack_leaf(&self, chunk_size: usize) -> Option<(Vec<u8>, usize)> {
        match self {
            PoiNode::Leaf(points) if !points.is_empty() => Some(pack_poi_chunk(points, chunk_size)),
            _ => None,
        }
    }
}

/// Pack one 32-byte POI record (§7.3): absolute `int32 lat, int32 lon`, `u8
/// subtype`, `u8 name_len`, a 20-byte `0xFF`-padded name, and `0xFF 0xFF`
/// reserved. The name is already ASCII-folded + ≤ 20 bytes at ingest
/// ([`crate::poi::normalize_name`]); truncate defensively so a stray long name can
/// never overrun the fixed field.
fn pack_poi_record(p: &PoiPoint) -> [u8; POI_RECORD_LEN] {
    let mut rec = [0xFFu8; POI_RECORD_LEN];
    rec[0..4].copy_from_slice(&p.lat_udeg.to_le_bytes());
    rec[4..8].copy_from_slice(&p.lon_udeg.to_le_bytes());
    rec[8] = p.subtype;
    let name = p.name.as_deref().unwrap_or("");
    let bytes = name.as_bytes();
    let len = bytes.len().min(POI_NAME_LEN);
    rec[9] = len as u8;
    rec[10..10 + len].copy_from_slice(&bytes[..len]);
    // rec[10 + len .. 30] stays 0xFF (name pad); rec[30..32] stays 0xFF (reserved).
    rec
}

/// Pack a leaf's POI records into one `chunk_size`-byte chunk (§7.3): as many fixed
/// 32-byte records as fit, back-to-back, then a `0xFF` **subtype** sentinel + `0xFF`
/// padding to `chunk_size`. Returns `(bytes, dropped)`. `build_poi_tree` splits a
/// leaf before it exceeds the chunk capacity, so `dropped` is 0 in practice; the cap
/// is the safety net for the one case the tree can't split away — more than
/// `chunk_size / 32` distinct POIs inside the 10-µdeg (~1 m) recursion floor, which
/// dedup makes effectively impossible. Truncating loudly beats corrupting the chunk.
fn pack_poi_chunk(points: &[PoiPoint], chunk_size: usize) -> (Vec<u8>, usize) {
    let capacity = chunk_size / POI_RECORD_LEN;
    let kept = points.len().min(capacity);
    let mut data = Vec::with_capacity(chunk_size);
    for p in &points[..kept] {
        data.extend_from_slice(&pack_poi_record(p));
    }
    // A 0xFF subtype byte ends the records (mirrors the geometry chunk's style-id
    // sentinel). `data.resize` writes it and the rest of the padding in one go —
    // never a truncation, since `kept * 32 <= chunk_size` by construction.
    data.resize(chunk_size, 0xFF);
    (data, points.len() - kept)
}

/// Build one category's POI quadtree over the **global bbox** (§7.2), splitting a
/// leaf once it holds more points than one chunk can carry. Split geometry matches
/// the reader's `walk_leaves` exactly: floor-division midpoints (`div_euclid(2)`),
/// NW/NE/SW/SE order, and a 10-µdeg recursion guard (identical to the geometry
/// `quadtree.rs`) so a dense cluster can't recurse forever. A point on a midline is
/// assigned deterministically (east of / north of the midpoint is `>= mid`), which
/// keeps it inside its leaf's bbox for the query.
fn build_poi_tree(points: Vec<PoiPoint>, bbox: (i64, i64, i64, i64), capacity: usize) -> PoiNode {
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    // Fits a chunk, or the box is too small to subdivide — a leaf. The guard
    // matches the geometry quadtree's 10-µdeg floor so both agree on when to stop.
    if points.len() <= capacity || max_lon - min_lon < 10 || max_lat - min_lat < 10 {
        return PoiNode::Leaf(points);
    }
    let mid_lon = (min_lon + max_lon).div_euclid(2);
    let mid_lat = (min_lat + max_lat).div_euclid(2);
    // Partition into the four quadrants. West is `lon < mid`, South is `lat < mid`,
    // so a point exactly on a midline lands in the East / North child — inside that
    // child's bbox either way (the reader's boxes share the midline edge).
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

/// Serialize the full POI section (spec §7): the directory followed by each
/// category's quadtree index + data chunks. `pois` is the deduped classified list;
/// each is bucketed by its subtype's category ([`crate::poi::table_row`]). Category
/// ids are `1..=POI_CATEGORY_COUNT` and every one gets a directory entry, empty or
/// not (§7.1) — a map with no POIs writes six empty entries, never a zero offset.
/// `section_offset` is the section's absolute byte offset in the file, needed so the
/// directory's per-category `index_offset` fields are file-absolute.
pub fn serialize_poi_section(pois: &[Poi], global_bbox: (i64, i64, i64, i64), section_offset: usize) -> Vec<u8> {
    // Bucket points by category (id 1..=6). Index 0 is unused (no category 0).
    let mut by_cat: Vec<Vec<PoiPoint>> = (0..=POI_CATEGORY_COUNT as usize).map(|_| Vec::new()).collect();
    for p in pois {
        let cat = table_row(p.subtype).category as usize;
        by_cat[cat].push(PoiPoint {
            lon_udeg: p.lon_udeg,
            lat_udeg: p.lat_udeg,
            subtype: p.subtype,
            name: p.name.clone(),
        });
    }

    // Records per chunk = chunk_size / record_len (512 / 32 = 16), so a leaf holds
    // at most that many before the tree splits.
    let capacity = POI_CHUNK_SIZE / POI_RECORD_LEN;

    // Flatten every category's tree first; the directory's index offsets are then
    // laid out sequentially after the fixed-size directory itself.
    struct CatBlock {
        cat_id: u8,
        index: Vec<u8>,
        node_count: u32,
        chunks: Vec<u8>,
        chunk_count: u32,
    }
    let mut blocks = Vec::with_capacity(POI_CATEGORY_COUNT as usize);
    for cat_id in 1..=POI_CATEGORY_COUNT {
        let pts = std::mem::take(&mut by_cat[cat_id as usize]);
        if pts.is_empty() {
            blocks.push(CatBlock { cat_id, index: Vec::new(), node_count: 0, chunks: Vec::new(), chunk_count: 0 });
            continue;
        }
        let root = build_poi_tree(pts, global_bbox, capacity);
        let (index, node_count, chunks, chunk_count, dropped) = flatten_tree(&root, POI_CHUNK_SIZE);
        debug_assert_eq!(dropped, 0, "fixed-size POI records never overflow a split leaf");
        blocks.push(CatBlock { cat_id, index, node_count, chunks, chunk_count });
    }

    // Directory size: count byte + chunk_size u16 + one entry per category.
    let dir_len = 1 + 2 + POI_CATEGORY_COUNT as usize * POI_CAT_ENTRY_LEN;

    // Assign each category's index offset (file-absolute), laid out sequentially
    // after the directory: [index][chunks] per category, empties contributing zero.
    let mut cursor = section_offset + dir_len;
    let mut dir = Vec::with_capacity(dir_len);
    dir.push(POI_CATEGORY_COUNT);
    dir.extend_from_slice(&(POI_CHUNK_SIZE as u16).to_le_bytes());
    let mut payload = Vec::new();
    for b in &blocks {
        dir.push(b.cat_id);
        dir.extend_from_slice(&(cursor as u32).to_le_bytes());
        dir.extend_from_slice(&b.node_count.to_le_bytes());
        dir.extend_from_slice(&b.chunk_count.to_le_bytes());
        payload.extend_from_slice(&b.index);
        payload.extend_from_slice(&b.chunks);
        cursor += b.index.len() + b.chunks.len();
    }
    debug_assert_eq!(dir.len(), dir_len);

    let mut out = Vec::with_capacity(dir_len + payload.len());
    out.extend_from_slice(&dir);
    out.extend_from_slice(&payload);
    out
}

/// The 36-byte OBCM v6 header `<4sBiiiiIBIHI>`: magic, version, bbox stored as
/// lat,lon,lat,lon, style offset, lod count, lod-table offset, marker color, and
/// (v6) the POI section offset. Shared by both serializers.
fn header_bytes(
    lod_count: usize,
    marker_color: u16,
    global_bbox: (i64, i64, i64, i64),
    lod_table_offset: usize,
    poi_section_offset: usize,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN);
    out.extend_from_slice(b"OBCM");
    out.push(OBCM_VERSION);
    out.extend_from_slice(&(global_bbox.1 as i32).to_le_bytes()); // min_lat
    out.extend_from_slice(&(global_bbox.0 as i32).to_le_bytes()); // min_lon
    out.extend_from_slice(&(global_bbox.3 as i32).to_le_bytes()); // max_lat
    out.extend_from_slice(&(global_bbox.2 as i32).to_le_bytes()); // max_lon
    out.extend_from_slice(&(HEADER_LEN as u32).to_le_bytes());
    out.push(lod_count as u8);
    out.extend_from_slice(&(lod_table_offset as u32).to_le_bytes());
    out.extend_from_slice(&marker_color.to_le_bytes());
    out.extend_from_slice(&(poi_section_offset as u32).to_le_bytes());
    debug_assert_eq!(out.len(), HEADER_LEN);
    out
}

/// Append one LOD-table entry `<fIIHI>`: max_mpp (`None` ⇒ `+inf`), index offset,
/// node count, chunk size, chunk count.
fn push_lod_entry(table: &mut Vec<u8>, max_mpp: Option<f64>, index_offset: u32, nc: u32, cs: usize, cc: u32) {
    let mpp_f: f32 = max_mpp.map_or(f32::INFINITY, |v| v as f32);
    table.extend_from_slice(&mpp_f.to_le_bytes());
    table.extend_from_slice(&index_offset.to_le_bytes());
    table.extend_from_slice(&nc.to_le_bytes());
    table.extend_from_slice(&(cs as u16).to_le_bytes());
    table.extend_from_slice(&cc.to_le_bytes());
}

/// Serialize a pyramid of LOD layers into the full v6 `.obcm` byte stream (header
/// field order, LOD table layout, the bbox stored as lat,lon,lat,lon, and the
/// trailing POI section §7). `pois` is the deduped classified POI list (empty ⇒ an
/// empty POI directory, still written). The second return value is the total
/// chunk-overflow feature drops (see [`pack_chunk`]).
pub fn serialize_lods(
    lods: &[LodLayer],
    styles: &[Style],
    marker_color: u16,
    global_bbox: (i64, i64, i64, i64),
    pois: &[Poi],
) -> (Vec<u8>, usize) {
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
    let mut dropped = 0usize;
    for lod in lods {
        let (ib, nc, cb, cc, lod_dropped) = serialize_tree(&lod.root, lod.chunk_size);
        dropped += lod_dropped;
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

    // The POI section starts right after the last LOD's chunks (`cursor`).
    let poi_section = serialize_poi_section(pois, global_bbox, cursor);

    let mut out = Vec::with_capacity(lod_table_offset + table.len() + payload.len() + poi_section.len());
    out.extend_from_slice(&header_bytes(lod_count, marker_color, global_bbox, lod_table_offset, cursor));
    out.extend_from_slice(&style_data);
    out.extend_from_slice(&table);
    out.extend_from_slice(&payload);
    out.extend_from_slice(&poi_section);
    (out, dropped)
}

/// Streaming counterpart to [`serialize_lods`]: writes the **same** v6 byte stream,
/// but builds, serializes, and drops one LOD tree at a time. Peak memory is ~one
/// tree + one LOD's chunk bytes rather than all trees plus the whole output buffer.
///
/// The header, style table, and LOD-table offset are known up front, but the POI
/// section offset is not (it depends on every LOD's serialized size). So we write
/// header + style table (with a **placeholder** POI offset) + a **zeroed** LOD
/// table, stream each LOD's `index ++ chunks`, append the POI section at the
/// resulting cursor, then `seek` back and patch both the LOD table and the header's
/// POI-section-offset field. Byte-identical to `serialize_lods` for the same trees
/// and POIs (asserted by `streaming_matches_in_memory`). Returns `(bytes_written,
/// dropped_features)` — the latter counts chunk-overflow drops (see [`pack_chunk`])
/// so the CLI can warn.
///
/// `build(i)` yields LOD `i`'s `(root, chunk_size, max_mpp)`, called once per level
/// in order; each tree is dropped before the next call. The POI section is built in
/// memory (small — point records, not geometry) after the LODs stream out.
pub fn serialize_lods_streaming<W, F>(
    w: &mut W,
    lod_count: usize,
    styles: &[Style],
    marker_color: u16,
    global_bbox: (i64, i64, i64, i64),
    pois: &[Poi],
    mut build: F,
) -> io::Result<(u64, usize)>
where
    W: Write + Seek,
    F: FnMut(usize) -> (Node, usize, Option<f64>),
{
    let style_data = pack_style_dict(styles);
    let lod_table_offset = HEADER_LEN + style_data.len();
    let payload_start = lod_table_offset + lod_count * LOD_ENTRY_LEN;

    // 1. Header (bbox stored lat,lon,lat,lon) — needs no tree. The POI section
    // offset isn't known until the LODs are sized, so write a 0 placeholder and
    // patch it in step 5.
    w.write_all(&header_bytes(lod_count, marker_color, global_bbox, lod_table_offset, 0))?;

    // 2. Style table, then a zeroed LOD table we patch in step 5.
    w.write_all(&style_data)?;
    w.write_all(&vec![0u8; lod_count * LOD_ENTRY_LEN])?;

    // 3. Per-LOD: build → serialize → stream payload → drop the tree.
    let mut table = Vec::with_capacity(lod_count * LOD_ENTRY_LEN);
    let mut cursor = payload_start;
    let mut dropped = 0usize;
    for i in 0..lod_count {
        let (root, chunk_size, max_mpp) = build(i);
        let (ib, nc, cb, cc, lod_dropped) = serialize_tree(&root, chunk_size);
        drop(root); // free the tree before writing this LOD / building the next
        dropped += lod_dropped;
        push_lod_entry(&mut table, max_mpp, cursor as u32, nc, chunk_size, cc);
        w.write_all(&ib)?;
        w.write_all(&cb)?;
        cursor += ib.len() + cb.len();
    }

    // 4. The POI section begins at the current cursor (right after the last LOD).
    let poi_section_offset = cursor;
    let poi_section = serialize_poi_section(pois, global_bbox, poi_section_offset);
    w.write_all(&poi_section)?;
    cursor += poi_section.len();

    // 5. Back-patch the LOD table and the header's POI-section-offset field, then
    // leave the cursor at EOF. The POI offset lives at header byte 32 (§1).
    w.seek(SeekFrom::Start(lod_table_offset as u64))?;
    w.write_all(&table)?;
    w.seek(SeekFrom::Start(32))?;
    w.write_all(&(poi_section_offset as u32).to_le_bytes())?;
    w.seek(SeekFrom::Start(cursor as u64))?;
    Ok((cursor as u64, dropped))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_is_ties_even_not_away() {
        // Pins the mode `to_udeg` relies on: `round_ties_even` vs `f64::round`
        // (half-away-from-zero). Exact halves are representable in f64.
        assert_eq!(0.5_f64.round_ties_even(), 0.0);
        assert_eq!(1.5_f64.round_ties_even(), 2.0);
        assert_eq!(2.5_f64.round_ties_even(), 2.0);
        assert_eq!(3.5_f64.round_ties_even(), 4.0);
        assert_eq!((-1.5_f64).round_ties_even(), -2.0);
        // The wrong (away) mode would give 1.0 and 3.0 here — guard against it.
        assert_eq!(0.5_f64.round(), 1.0);
        assert_eq!(2.5_f64.round(), 3.0);
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
        let n = obc_reader::MAX_FEAT_PTS;
        // `n` points 1 µdeg apart: tiny deltas ⇒ densest 8-bit encoding (2 bytes/
        // vertex), no densification.
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
    fn validate_chunk_size_rejects_degenerate_minimum() {
        assert!(validate_chunk_size(MIN_CHUNK_SIZE).is_ok(), "the minimum is inclusive");
        assert!(validate_chunk_size(MIN_CHUNK_SIZE - 1).is_err(), "one under the floor is rejected");
        assert!(validate_chunk_size(1).is_err(), "a chunk_size that drops every feature is rejected");
    }

    #[test]
    fn streaming_matches_in_memory() {
        // Streaming output must be byte-identical to `serialize_lods` for the same
        // 2-LOD pyramid *and* POI section — the POI back-patch in the streaming path
        // is easy to drift, so the fixture carries POIs of a few categories.
        use crate::geom::Geom;
        use crate::poi::Poi;
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
        let poi = |subtype, lon, lat, name: Option<&str>| Poi {
            subtype,
            lon_udeg: lon,
            lat_udeg: lat,
            name: name.map(String::from),
            from_node: true,
        };
        let pois = vec![
            poi(1, 100_000, 100_000, Some("Brunnen")),
            poi(5, 200_000, 200_000, None),
            poi(17, 300_000, 300_000, Some("Apotheke")),
        ];

        let (reference, ref_dropped) = serialize_lods(&lods, &styles, 0xABCD, bbox, &pois);
        assert_eq!(ref_dropped, 0, "nothing overflows in this fixture");

        let mut cur = Cursor::new(Vec::new());
        let (total, dropped) = serialize_lods_streaming(&mut cur, lods.len(), &styles, 0xABCD, bbox, &pois, |i| {
            (lods[i].root.clone(), lods[i].chunk_size, lods[i].max_mpp)
        })
        .unwrap();

        assert_eq!(cur.into_inner(), reference, "streaming output must be byte-identical");
        assert_eq!(total as usize, reference.len());
        assert_eq!(dropped, 0, "and reports the same (zero) drop count");
    }
}
