//! OBCM v8 serializer — lay out the `.obcm` bytes per `OBCM_Spec.md`.
//!
//! Deterministic: same feature list + quadtree → same output. Geometry arrives
//! already clipped + simplified; this module rounds lon/lat to microdegrees
//! (round-half-to-even), densifies long segments, delta-encodes rings, and lays out
//! the chunk / index / LOD-table / header bytes. The **POI section** (§7 of the
//! spec) is a per-category quadtree over fixed 36-byte point records (each carrying
//! a `hours_ref` u16 into the shared hours-pool section), reusing the same
//! BFS-flatten + u32 node encoding as the geometry tree. The trailing **nav-graph
//! section** (v8, §8) tiles the routable graph ([`crate::nav`]): a node quadtree
//! (§4 encoding again) over variable-length junction records with inline neighbor
//! coords, plus a chunked edge pool addressed by pool-relative byte offset.

use std::io::{self, Seek, SeekFrom, Write};

use obc_formats::obcm::{
    BRANCH_BIT, CHUNK_END, EMPTY_LEAF, FEATURE_FLAG_16BIT, FEATURE_FLAG_HOLES, FEATURE_FLAG_POLYGON,
    FEATURE_HEADER_LEN, MAGIC, STYLE_DASHED_BIT, STYLE_HAS_COLOR2_BIT, STYLE_PRIORITY_MASK, STYLE_RECORD_LEN,
};

// The OBCM constants the serializer lays out are owned by `obc-formats`; imported here (the
// `VERSION as OBCM_VERSION` rename is a module-local readability alias). Not re-exported.
use obc_formats::obcm::{
    HEADER_LEN, LOD_ENTRY_LEN, NAV_CHUNK_SIZE, NAV_DIR_LEN, NAV_EDGE_FIXED_LEN, NAV_MAX_DEGREE, NAV_MAX_PROFILES,
    NAV_NEIGHBOR_LEN, NAV_NODE_FIXED_LEN, NAV_PROFILE_LEN, NAV_PROFILE_NAME_LEN, POI_CATEGORY_COUNT, POI_CAT_ENTRY_LEN,
    POI_CHUNK_SIZE, POI_HOURS_BLOB_LEN, POI_HOURS_REF_NONE, POI_NAME_LEN, POI_RECORD_LEN, VERSION as OBCM_VERSION,
};

use crate::nav::{polyline_len_m, NavGraph};
use crate::poi::{table_row, Poi};

/// Max delta (microdegrees) before a segment is densified to keep deltas in
/// 16-bit range. Crate-visible so `geom::packed_size_budget` can count the
/// midpoints `densify` will insert.
pub(crate) const MAX_SEGMENT: i64 = 30_000;

// The serializer's blob length must equal `hours.rs`'s `Schedule::encode` width, or
// the pool bytes and the `POI_HOURS_BLOB_LEN` the directory advertises disagree.
const _: () = assert!(POI_HOURS_BLOB_LEN == crate::hours::BLOB_LEN, "hours blob length must match hours.rs");

// A cap-degree record must fit one chunk, or `pack_nav_chunk` would drop real junctions.
const _: () = assert!(NAV_NODE_FIXED_LEN + NAV_MAX_DEGREE * NAV_NEIGHBOR_LEN <= NAV_CHUNK_SIZE);

/// Max endpoint-to-endpoint µdeg delta (lat **or** lon) a serialized adjacency piece may span, so
/// every neighbor entry's `dlat`/`dlon` fits `i16`. Mirrors `nav::MAX_ENDPOINT_DELTA_UDEG` (N1
/// bounds whole edges to this); the serializer's long-edge split re-checks it because splitting a
/// densified polyline into ≤ [`NAV_MAX_EDGE_PTS`] pieces can otherwise land a synthetic junction
/// farther than `i16` from its neighbor. `32 000 < i16::MAX (32 767)` keeps a margin.
const NAV_MAX_NEIGHBOR_DELTA: i64 = 32_000;

// Every non-empty v9 map carries at least one profile; the packer must never write zero (the
// reader treats `profile_count == 0` as malformed).
const _: () = assert!(NAV_MAX_PROFILES <= u8::MAX as usize);

/// Max polyline points of one serialized edge record: the record must never
/// straddle a chunk boundary (§8.4), so it is bounded by the chunk itself. An edge
/// whose **densified** polyline is longer (or whose pieces would span more than
/// [`NAV_MAX_NEIGHBOR_DELTA`]) is split at a vertex into pieces joined by synthetic
/// degree-2 nodes — routing-neutral, and it keeps the reader to one chunk-sized read
/// per edge. `(512 − 15) / 4 + 1 = 125`, unchanged from v8 (the +1-byte `way_kind`
/// head doesn't cross a 4-byte delta boundary).
pub const NAV_MAX_EDGE_PTS: usize = (NAV_CHUNK_SIZE - NAV_EDGE_FIXED_LEN) / 4 + 1;

/// A per-map routing profile ready to serialize into §8.6: a display name plus the two multiplier
/// tables in `u8` fixed-point 1/16 (indexed by way-kind's highway class 0..=31 / surface class
/// 0..=7; `16` = 1.0×, `0` = forbidden). Built + validated in [`crate::config`] (every non-zero
/// multiplier ≥ 16 so the great-circle A* heuristic stays admissible); the serializer only writes
/// the bytes. Kept small and `Clone` so the four defaults can be handed around cheaply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavProfile {
    /// Display name (UTF-8), truncated to [`NAV_PROFILE_NAME_LEN`] bytes on write, `0xFF`-padded.
    pub name: String,
    /// Multiplier per highway class (5-bit index, 0..=31). `16` = 1.0×, `0` = forbidden.
    pub highway: [u8; 32],
    /// Multiplier per surface class (3-bit index, 0..=7). Same encoding.
    pub surface: [u8; 8],
}

/// Largest `chunk_size` (bytes) that keeps every feature within the reader's
/// [`obc_reader::MAX_FEAT_PTS`] vertex cap. Densest encoding is 8-bit deltas
/// (12-byte header + 2 bytes/vertex), so a chunk carries at most
/// `(chunk_size - 12) / 2 + 1` vertices. Above this the reader **silently
/// truncates** past-cap vertices (`heapless` push fails, no error either side),
/// corrupting the feature's fill/stroke.
pub const MAX_SAFE_CHUNK_SIZE: usize = (obc_reader::MAX_FEAT_PTS - 1) * 2 + FEATURE_HEADER_LEN;

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
    /// v10: dashed line style (flag bit 2). Ignored for polygons by the renderer.
    pub dashed: bool,
    /// v10: optional RGB565 secondary color (flag bit 3 + the trailing u16). `None` ⇒ bit clear and
    /// `0x0000` on the wire (which the reader ignores — black is a legit color, not a sentinel).
    pub color2: Option<u16>,
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

/// Pack the style table (OBCM v10): `Count(u8)` then, sorted by id, `<BbHBBH>` per style — `id,
/// z_index, color, weight, flags, color2`. `flags = (priority-1) & STYLE_PRIORITY_MASK`, plus
/// `STYLE_DASHED_BIT` when `dashed` and `STYLE_HAS_COLOR2_BIT` when `color2` is `Some`. `color2`
/// writes its RGB565 value when present, else `0x0000` (which the reader ignores, bit 3 being clear).
pub fn pack_style_dict(styles: &[Style]) -> Vec<u8> {
    let mut styles = styles.to_vec();
    styles.sort_by_key(|s| s.id);
    let mut data = Vec::with_capacity(1 + styles.len() * STYLE_RECORD_LEN);
    data.push(styles.len() as u8);
    for s in &styles {
        let priority = (s.priority as i32).clamp(1, 4);
        let mut flags = (priority - 1) as u8 & STYLE_PRIORITY_MASK;
        if s.dashed {
            flags |= STYLE_DASHED_BIT;
        }
        if s.color2.is_some() {
            flags |= STYLE_HAS_COLOR2_BIT;
        }
        data.push(s.id);
        data.push(s.z_index as u8);
        data.extend_from_slice(&s.color.to_le_bytes());
        data.push(s.weight);
        data.push(flags);
        data.extend_from_slice(&s.color2.unwrap_or(0).to_le_bytes());
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

    // The reader buffers ring lengths in a `MAX_FEAT_RINGS` heapless vec and
    // discards the whole feature past it; the quadtree splits (or floor-trims)
    // features over the cap before they reach here. (Also keeps the hole-count
    // byte below from ever wrapping u8.)
    debug_assert!(
        packed_rings.len() <= obc_reader::MAX_FEAT_RINGS,
        "feature ring count exceeds the reader's MAX_FEAT_RINGS — quadtree cap enforcement missed it"
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
    data.resize(chunk_size, CHUNK_END);
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

// --- POI section (v7, spec §7) ------------------------------------------------

/// A POI record's absolute microdegree coordinates + the fields packed into its
/// 36-byte record (§7.3). Owned so the tree can move records into leaves.
/// `hours_ref` is the 0-based index into the map's hours-pool section (§7.5), or
/// [`POI_HOURS_REF_NONE`] when the POI has no pooled hours — resolved before
/// tree-building so it travels with the record.
struct PoiPoint {
    lon_udeg: i32,
    lat_udeg: i32,
    subtype: u8,
    name: Option<String>,
    hours_ref: u16,
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

/// Pack one 36-byte POI record (§7.3): absolute `int32 lat, int32 lon`, `u8
/// subtype`, `u8 name_len`, a 24-byte `0xFF`-padded name, and a `u16 hours_ref`
/// (0-based hours-pool index, [`POI_HOURS_REF_NONE`] = none). The name is already
/// ASCII-folded + ≤ 24 bytes at ingest ([`crate::poi::normalize_name`]); truncate
/// defensively so a stray long name can never overrun the fixed field.
fn pack_poi_record(p: &PoiPoint) -> [u8; POI_RECORD_LEN] {
    let mut rec = [CHUNK_END; POI_RECORD_LEN];
    rec[0..4].copy_from_slice(&p.lat_udeg.to_le_bytes());
    rec[4..8].copy_from_slice(&p.lon_udeg.to_le_bytes());
    rec[8] = p.subtype;
    let name = p.name.as_deref().unwrap_or("");
    let bytes = name.as_bytes();
    let len = bytes.len().min(POI_NAME_LEN);
    rec[9] = len as u8;
    rec[10..10 + len].copy_from_slice(&bytes[..len]);
    // rec[10 + len .. 34] stays 0xFF (name pad).
    rec[34..36].copy_from_slice(&p.hours_ref.to_le_bytes());
    rec
}

/// Pack a leaf's POI records into one `chunk_size`-byte chunk (§7.3): as many fixed
/// 36-byte records as fit, back-to-back, then a `0xFF` **subtype** sentinel + `0xFF`
/// padding to `chunk_size`. Returns `(bytes, dropped)`. `build_poi_tree` splits a
/// leaf before it exceeds the chunk capacity, so `dropped` is 0 in practice; the cap
/// is the safety net for the one case the tree can't split away — more than
/// `chunk_size / 36` distinct POIs inside the 10-µdeg (~1 m) recursion floor, which
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
    data.resize(chunk_size, CHUNK_END);
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
/// category's quadtree index + data chunks, then the shared **hours-pool section**
/// (§7.5) at the tail. `pois` is the deduped classified list; each is bucketed by
/// its subtype's category ([`crate::poi::table_row`]). Category ids are
/// `1..=POI_CATEGORY_COUNT` and every one gets a directory entry, empty or not
/// (§7.1) — a map with no POIs writes six empty entries, never a zero offset.
/// `section_offset` is the section's absolute byte offset in the file, needed so the
/// directory's per-category `index_offset` fields and the `hours_pool_offset` are
/// file-absolute.
///
/// The hours pool is built **once** over the whole `pois` slice ([`build_hours_pool`]):
/// identical 29-byte weekly-schedule blobs collapse to one, and each POI's 0-based
/// `hours_ref` (or [`POI_HOURS_REF_NONE`]) is stamped onto its record **before**
/// tree-building so it travels into the right leaf. The pool bytes (`count u16` +
/// `count × 29-byte blobs`) are appended after every category's index+chunks, and
/// the directory records the pool's absolute offset + count.
pub fn serialize_poi_section(pois: &[Poi], global_bbox: (i64, i64, i64, i64), section_offset: usize) -> Vec<u8> {
    // Dedup the weekly schedules into a pool once over the whole list; `refs[k]` is
    // POI k's 0-based pool index (or `None` ⇒ no hours). Aligned to `pois`.
    let (pool, refs) = crate::hours::build_hours_pool(pois, |p| p.hours.as_ref());

    // Bucket points by category (id 1..=6). Index 0 is unused (no category 0). Each
    // point carries its resolved `hours_ref` so it survives tree-building + chunking.
    let mut by_cat: Vec<Vec<PoiPoint>> = (0..=POI_CATEGORY_COUNT as usize).map(|_| Vec::new()).collect();
    for (p, hours_ref) in pois.iter().zip(refs.iter()) {
        let cat = table_row(p.subtype).category() as usize;
        by_cat[cat].push(PoiPoint {
            lon_udeg: p.lon_udeg,
            lat_udeg: p.lat_udeg,
            subtype: p.subtype,
            name: p.name.clone(),
            hours_ref: hours_ref.unwrap_or(POI_HOURS_REF_NONE),
        });
    }

    // Records per chunk = chunk_size / record_len (512 / 36 = 14), so a leaf holds
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

    // Directory size: count byte + chunk_size u16 + one entry per category + the two
    // v7 hours-pool fields (offset u32 + count u16).
    let dir_len = 1 + 2 + POI_CATEGORY_COUNT as usize * POI_CAT_ENTRY_LEN + 4 + 2;

    // Lay categories out sequentially after the directory: [index][chunks] per
    // category, empties contributing zero. The hours pool follows the last category.
    let mut cursor = section_offset + dir_len;
    let mut payload = Vec::new();
    let mut cat_entries = Vec::with_capacity(POI_CATEGORY_COUNT as usize);
    for b in &blocks {
        cat_entries.push((b.cat_id, cursor as u32, b.node_count, b.chunk_count));
        payload.extend_from_slice(&b.index);
        payload.extend_from_slice(&b.chunks);
        cursor += b.index.len() + b.chunks.len();
    }

    // The hours-pool section begins right after the last category's chunks (`cursor`).
    let hours_pool_offset = cursor;
    let hours_pool = pack_hours_pool(&pool);
    payload.extend_from_slice(&hours_pool);

    // Now emit the directory with the resolved offsets + pool fields.
    let mut dir = Vec::with_capacity(dir_len);
    dir.push(POI_CATEGORY_COUNT);
    dir.extend_from_slice(&(POI_CHUNK_SIZE as u16).to_le_bytes());
    for (cat_id, index_offset, node_count, chunk_count) in cat_entries {
        dir.push(cat_id);
        dir.extend_from_slice(&index_offset.to_le_bytes());
        dir.extend_from_slice(&node_count.to_le_bytes());
        dir.extend_from_slice(&chunk_count.to_le_bytes());
    }
    dir.extend_from_slice(&(hours_pool_offset as u32).to_le_bytes());
    dir.extend_from_slice(&(pool.len() as u16).to_le_bytes());
    debug_assert_eq!(dir.len(), dir_len);

    let mut out = Vec::with_capacity(dir_len + payload.len());
    out.extend_from_slice(&dir);
    out.extend_from_slice(&payload);
    out
}

/// Pack the hours-pool section (§7.5): `count u16` then `count × 29-byte` blobs,
/// back-to-back. `blob i` (as referenced by a record's `hours_ref`) lands at
/// `hours_pool_offset + 2 + i * 29`. An empty pool is just the `0` count (2 bytes).
fn pack_hours_pool(pool: &[[u8; POI_HOURS_BLOB_LEN]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + pool.len() * POI_HOURS_BLOB_LEN);
    out.extend_from_slice(&(pool.len() as u16).to_le_bytes());
    for blob in pool {
        out.extend_from_slice(blob);
    }
    out
}

// --- Nav-graph section (v9, spec §8) --------------------------------------------

/// One adjacency entry of a junction record (§8.3), holding the neighbor's **absolute** µdeg coords
/// (the serializer turns them into `i16` deltas from the owning record at pack time), the resolved
/// wire `edge_id` (pool-relative byte offset), the edge's ground `cost_m` (written `u16`), and its
/// `way_kind` class byte.
struct WireNeighbor {
    id: u32,
    lat: i32,
    lon: i32,
    edge_id: u32,
    cost_m: u32,
    way_kind: u8,
}

/// A junction node ready to serialize: absolute µdeg coords, R1's dense id, and
/// its (capped) neighbor list. `record_len` is the §8.3 on-wire size.
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

/// One node of the nav quadtree, mirroring [`PoiNode`] so the shared
/// [`flatten_tree`] serializes it to the identical index layout. Leaves split on
/// **packed bytes** (records are variable-length), not record count.
enum NavTreeNode {
    Leaf(Vec<NavPoint>),
    Branch(Box<[NavTreeNode; 4]>),
}

/// Pack one v9 §8.3 junction record: `lat i32, lon i32, node_id u32, degree u8`, then one 15-byte
/// entry per neighbor (`id u32, dlat i16, dlon i16, edge_id u32, cost_m u16, way_kind u8`).
/// Coordinates are absolute µdeg in the head, lat first (the §7.3/§8 record convention); each
/// neighbor's coord is stored as an `i16` **delta from this record's own lat/lon** (N1's edge
/// splits guarantee the delta fits), so relaxation reconstructs `neighbor = node + delta` exactly.
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
    }
}

/// Bin-pack the tree's leaves into 512-byte node chunks (§8.3, the v9 optimization). `build_nav_tree`
/// already split every leaf to ≤ one chunk of records; v8 then gave each leaf its own chunk, wasting
/// the ~58% of every leaf that didn't fill 512 B. v9 assigns chunk ids **first-fit over the leaves in
/// BFS emission order**: each leaf's record block is placed in the **first already-open chunk with
/// room**, opening a new chunk only when none fits — a small leaf therefore back-fills the slack an
/// earlier large leaf left, which is what lifts the fill rate to ~90 %+ (plain next-fit, considering
/// only the newest chunk, stalls near ~75 % on real graphs). **Distinct leaves may share a chunk id**,
/// and because first-fit reaches back to earlier chunks those leaves can be spatially distant — so a
/// quadtree walk that visits several leaves sharing a chunk decodes that chunk's records once per
/// leaf, handing a consumer the same junction more than once. The reference consumers (A\* settle
/// match-by-id, snap best-tracking) are idempotent, so this is the documented §8.3 contract, not a
/// bug. A single leaf's records never straddle chunks; a pathological leaf larger than one chunk
/// (co-located junctions past the 10-µdeg split floor) keeps what fits and drops the rest, counted in
/// `dropped` (the same safety net v8's `pack_nav_chunk` had).
///
/// Returns `(index_bytes, node_count, chunk_bytes, chunk_count, dropped)` — the same shape as
/// [`flatten_tree`], so the directory-writing code is shared. BFS order and the branch/leaf/chunk-id
/// index encoding are identical to [`flatten_tree`]; only the leaf→chunk assignment differs (many
/// leaves per chunk instead of one).
fn flatten_nav_tree(root: &NavTreeNode) -> (Vec<u8>, u32, Vec<u8>, u32, usize) {
    // BFS in enqueue order — children appended contiguously, so a branch's first-child index is the
    // node count when it is expanded (`child > idx`, the reader's `walk_leaves` invariant).
    let mut nodes: Vec<&NavTreeNode> = vec![root];
    let mut first_child: Vec<usize> = vec![0];
    let mut i = 0;
    while i < nodes.len() {
        if let NavTreeNode::Branch(children) = nodes[i] {
            first_child[i] = nodes.len();
            for c in children.iter() {
                nodes.push(c);
                first_child.push(0);
            }
        }
        i += 1;
    }

    // Each open chunk is built as its own ≤ 512-byte record block; `bins[c]` is chunk `c`'s bytes so
    // far. First-fit scans them in creation order for the first with room. (Grimsel/monaco pack a few
    // thousand chunks — the O(leaves × chunks) scan is a blink at pack time.)
    let mut index: Vec<u32> = Vec::with_capacity(nodes.len());
    let mut bins: Vec<Vec<u8>> = Vec::new();
    let mut dropped: usize = 0;
    for (idx, node) in nodes.iter().enumerate() {
        let points = match node {
            NavTreeNode::Branch(_) => {
                index.push(first_child[idx] as u32 | BRANCH_BIT);
                continue;
            }
            NavTreeNode::Leaf(points) if !points.is_empty() => points,
            NavTreeNode::Leaf(_) => {
                index.push(EMPTY_LEAF);
                continue;
            }
        };
        let leaf_len: usize = points.iter().map(NavPoint::record_len).sum();
        // First-fit: the first open chunk whose remaining space holds the whole leaf; else a new one.
        // A leaf larger than a whole chunk can't fit anywhere, so it opens a fresh chunk and drops its
        // overflow (build_nav_tree makes this effectively impossible).
        let bin = match bins.iter().position(|b| b.len() + leaf_len <= NAV_CHUNK_SIZE) {
            Some(c) => c,
            None => {
                bins.push(Vec::with_capacity(NAV_CHUNK_SIZE));
                bins.len() - 1
            }
        };
        index.push((bin as u32) & !BRANCH_BIT);
        for p in points {
            if bins[bin].len() + p.record_len() > NAV_CHUNK_SIZE {
                dropped += 1;
                continue; // co-located overflow inside one leaf — effectively impossible in real OSM
            }
            pack_nav_record(p, &mut bins[bin]);
        }
    }

    // Concatenate the bins, each 0xFF-padded to a full chunk (the padding's first byte lands on a
    // `degree` slot, giving the reader its end-of-chunk sentinel).
    let chunk_count = bins.len() as u32;
    let mut chunks: Vec<u8> = Vec::with_capacity(bins.len() * NAV_CHUNK_SIZE);
    for mut b in bins {
        b.resize(NAV_CHUNK_SIZE, CHUNK_END);
        chunks.extend_from_slice(&b);
    }

    let mut index_bytes = Vec::with_capacity(index.len() * 4);
    for v in &index {
        index_bytes.extend_from_slice(&v.to_le_bytes());
    }
    (index_bytes, index.len() as u32, chunks, chunk_count, dropped)
}

/// Build the node quadtree over the **global bbox** (§8.2), splitting a leaf once
/// its packed records exceed one chunk. Split geometry is identical to
/// [`build_poi_tree`] (floor-division midpoints, NW/NE/SW/SE, 10-µdeg floor), so
/// the reader's `walk_leaves` resolves it verbatim.
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

/// A working edge on its way into the pool: endpoints (dense node ids), the
/// **densified** polyline, the cost carried into both endpoints' records, and the
/// parent way's `kind` class byte (inherited by every split piece).
struct WorkEdge {
    a: u32,
    b: u32,
    polyline: Vec<(i32, i32)>,
    cost_m: u32,
    kind: u8,
}

/// Densify one nav polyline: insert midpoints on any segment whose lon **or** lat
/// delta exceeds [`MAX_SEGMENT`] so every §8.4 `(dlat, dlon)` fits an `i16` — the
/// exact [`densify`] the geometry rings use, over the same 30 000-µdeg threshold.
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

/// Pack one v9 §8.4 edge record: `length_m u32, pt_count u16, way_kind u8, anchor_lat i32,
/// anchor_lon i32`, then `pt_count - 1` × `(dlat i16, dlon i16)`. `length_m` **stays** in v9 (N3
/// sums it at emit for the displayed distance — weighted `g` is no longer a distance). The polyline
/// is already densified, so every delta fits.
fn pack_edge_record(e: &WorkEdge, out: &mut Vec<u8>) {
    out.extend_from_slice(&e.cost_m.to_le_bytes());
    out.extend_from_slice(&(e.polyline.len() as u16).to_le_bytes());
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

/// Pack the §8.6 profile table: `profiles.len()` consecutive 52-byte records (`name [u8;12]`,
/// `highway_mult [u8;32]`, `surface_mult [u8;8]`). The name is UTF-8 truncated to 12 bytes and
/// `0xFF`-padded (the POI-name convention). `profiles` is `1..=8` (the packer never writes an empty
/// table; the reader rejects `profile_count` outside that range).
fn pack_profile_table(profiles: &[NavProfile]) -> Vec<u8> {
    debug_assert!((1..=NAV_MAX_PROFILES).contains(&profiles.len()), "1..=8 profiles");
    let mut out = Vec::with_capacity(profiles.len() * NAV_PROFILE_LEN);
    for p in profiles {
        let name = p.name.as_bytes();
        let n = name.len().min(NAV_PROFILE_NAME_LEN);
        out.extend_from_slice(&name[..n]);
        out.resize(out.len() + (NAV_PROFILE_NAME_LEN - n), CHUNK_END); // 0xFF-pad the name field
        out.extend_from_slice(&p.highway);
        out.extend_from_slice(&p.surface);
    }
    debug_assert_eq!(out.len(), profiles.len() * NAV_PROFILE_LEN);
    out
}

/// Serialize the full nav-graph section (spec §8, v9) at absolute byte `section_offset`:
/// `[directory (28 B)][profile table (§8.6)][node quadtree index][node chunks][edge pool]`. The
/// **profile table is written immediately after the directory**, before the node index, so even an
/// empty graph (no routable ways) still carries its profiles; the section is **always present**.
/// `profiles` is `1..=8` entries (validated in [`crate::config`]).
///
/// Graph normalizations happen here, on working copies (the caller's [`NavGraph`] is untouched):
/// - **Densify + split.** Polylines are densified to the 30 000-µdeg segment bound; an edge whose
///   record would overflow one chunk ([`NAV_MAX_EDGE_PTS`]) **or** whose piece would span more than
///   [`NAV_MAX_NEIGHBOR_DELTA`] (so its neighbor delta wouldn't fit `i16`) is split at a vertex into
///   pieces joined by synthetic degree-2 nodes (each piece's cost re-measured over its sub-polyline
///   and `way_kind` inherited), so **no record straddles a chunk** and **every neighbor delta fits
///   `i16`**.
/// - **Degree cap.** A node keeps its first [`NAV_MAX_DEGREE`] adjacency entries (edge-pool order —
///   deterministic); the packer warns on stderr about the rest.
/// - **Bin-packed node chunks.** Leaves are first-fit-packed into shared 512-byte chunks
///   ([`flatten_nav_tree`]) — distinct leaves may reference one chunk.
///
/// Wire `edge_id` = the record's **pool-relative byte offset** (§8.4): the reader derives
/// `(chunk, offset)` as `id / 512`, `id % 512` with **zero resident index**. A self-loop edge
/// (`a == b`) contributes **one** adjacency entry, not two.
pub fn serialize_nav_section(
    graph: &NavGraph,
    profiles: &[NavProfile],
    global_bbox: (i64, i64, i64, i64),
    section_offset: usize,
) -> Vec<u8> {
    let profile_table = pack_profile_table(profiles);
    // The profile table sits right after the 28-byte directory; the node index (and, for an empty
    // graph, the zero-length edge pool) start after it.
    let profile_table_offset = section_offset + NAV_DIR_LEN;
    let index_offset = profile_table_offset + profile_table.len();

    // Directory writer, shared by the empty and populated paths. `idx_off`/`edge_off` point at the
    // node index and edge pool; an empty graph passes `index_offset` for both (zero-length regions).
    let write_dir =
        |out: &mut Vec<u8>, idx_off: usize, node_count: u32, node_chunks: u32, edge_off: usize, edge_chunks: u32| {
            out.extend_from_slice(&(idx_off as u32).to_le_bytes()); // index_offset
            out.extend_from_slice(&node_count.to_le_bytes()); // index_node_count
            out.extend_from_slice(&node_chunks.to_le_bytes()); // node_chunk_count
            out.extend_from_slice(&(edge_off as u32).to_le_bytes()); // edge_pool_offset
            out.extend_from_slice(&edge_chunks.to_le_bytes()); // edge_chunk_count
            out.extend_from_slice(&(NAV_CHUNK_SIZE as u16).to_le_bytes()); // chunk_size (pinned 512)
            out.extend_from_slice(&(profile_table_offset as u32).to_le_bytes()); // profile_table_offset
            out.push(profiles.len() as u8); // profile_count
            out.push(0u8); // reserved
            debug_assert_eq!(out.len(), NAV_DIR_LEN);
        };

    if graph.nodes.is_empty() {
        // Empty graph: 28-byte directory (both regions zero-length, just past the profile table) +
        // the always-present profile table.
        let mut out = Vec::with_capacity(NAV_DIR_LEN + profile_table.len());
        write_dir(&mut out, index_offset, 0, 0, index_offset, 0);
        out.extend_from_slice(&profile_table);
        return out;
    }

    // R1 assigns dense ids in push order — the serializer indexes `coords` by id.
    debug_assert!(graph.nodes.iter().enumerate().all(|(i, n)| n.id as usize == i), "node ids are dense");
    let mut coords: Vec<(i32, i32)> = graph.nodes.iter().map(|n| n.coord).collect();

    // Densify, then split anything over one chunk's worth of points OR whose piece would span more
    // than the i16-delta bound. Splitting appends synthetic nodes, so it runs before adjacency.
    let mut edges: Vec<WorkEdge> = Vec::with_capacity(graph.edges.len());
    for e in &graph.edges {
        let poly = densify_polyline(&e.polyline);
        // Fast path: short polyline whose endpoints (= junctions a,b) already fit the i16 neighbor
        // delta. N1 guarantees this for real packs, but a hand-built graph (tests) may not, so we
        // re-check the span here rather than trust it — a violating edge falls into the split loop.
        let (a, z) = (poly[0], *poly.last().unwrap());
        let span_ok = (a.0 as i64 - z.0 as i64).abs() <= NAV_MAX_NEIGHBOR_DELTA
            && (a.1 as i64 - z.1 as i64).abs() <= NAV_MAX_NEIGHBOR_DELTA;
        if poly.len() <= NAV_MAX_EDGE_PTS && span_ok {
            edges.push(WorkEdge { a: e.a, b: e.b, polyline: poly, cost_m: e.length_m, kind: e.kind });
            continue;
        }
        // Walk the long polyline in pieces bounded by BOTH the chunk point-cap and the i16 endpoint
        // span; each interior cut vertex becomes a synthetic junction. Densify caps each segment to
        // 30 000 µdeg (< the span bound), so a piece always advances ≥ 1 vertex (termination).
        // Pieces are re-measured so their costs sum to (within rounding of) the original.
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

    // Edge pool: records back-to-back in `edges` order, each pushed to the next
    // chunk start if it would straddle a boundary; the wire edge_id is its
    // pool-relative byte offset.
    let mut pool: Vec<u8> = Vec::new();
    let mut edge_ids: Vec<u32> = Vec::with_capacity(edges.len());
    for e in &edges {
        let rec_len = NAV_EDGE_FIXED_LEN + (e.polyline.len() - 1) * 4;
        debug_assert!(rec_len <= NAV_CHUNK_SIZE, "split bounded every record to one chunk");
        let within = pool.len() % NAV_CHUNK_SIZE;
        if within + rec_len > NAV_CHUNK_SIZE {
            pool.resize(pool.len() + (NAV_CHUNK_SIZE - within), CHUNK_END);
        }
        edge_ids.push(pool.len() as u32);
        pack_edge_record(e, &mut pool);
    }
    pool.resize(pool.len().div_ceil(NAV_CHUNK_SIZE) * NAV_CHUNK_SIZE, CHUNK_END);
    let edge_chunk_count = (pool.len() / NAV_CHUNK_SIZE) as u32;

    // Adjacency with inline neighbor coords, capped at NAV_MAX_DEGREE.
    let mut adj: Vec<Vec<WireNeighbor>> = (0..coords.len()).map(|_| Vec::new()).collect();
    let mut truncated = 0usize;
    for (e, &edge_id) in edges.iter().zip(&edge_ids) {
        let mut push = |from: u32, to: u32| {
            let list = &mut adj[from as usize];
            if list.len() >= NAV_MAX_DEGREE {
                truncated += 1;
                return;
            }
            let (lon, lat) = coords[to as usize];
            list.push(WireNeighbor { id: to, lat, lon, edge_id, cost_m: e.cost_m, way_kind: e.kind });
        };
        push(e.a, e.b);
        if e.a != e.b {
            push(e.b, e.a);
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
        // Co-located junctions inside the 10-µdeg split floor — effectively
        // impossible in real OSM, but never silent.
        eprintln!("warning: {dropped} nav node record(s) dropped (leaf overflow at the split floor)");
    }

    // Layout: [directory][profile table][node index][node chunks][edge pool]. `index_offset` was
    // fixed above (after the profile table); the edge pool follows the node chunks.
    debug_assert_eq!(index_offset, section_offset + NAV_DIR_LEN + profile_table.len());
    let edge_pool_offset = index_offset + index.len() + chunks.len();
    let mut out = Vec::with_capacity(NAV_DIR_LEN + profile_table.len() + index.len() + chunks.len() + pool.len());
    write_dir(&mut out, index_offset, node_count, chunk_count, edge_pool_offset, edge_chunk_count);
    out.extend_from_slice(&profile_table);
    out.extend_from_slice(&index);
    out.extend_from_slice(&chunks);
    out.extend_from_slice(&pool);
    out
}

/// The 40-byte OBCM header `<4sBiiiiIBIHII>`: magic, version ([`OBCM_VERSION`]), bbox stored as
/// lat,lon,lat,lon, style offset, lod count, lod-table offset, marker color, the
/// POI section offset, and the nav-graph section offset. The header layout has been
/// 40 bytes since v8 (v9's and v10's additions hang off the nav directory / style record,
/// not the header). Shared by both serializers.
fn header_bytes(
    lod_count: usize,
    marker_color: u16,
    global_bbox: (i64, i64, i64, i64),
    lod_table_offset: usize,
    poi_section_offset: usize,
    nav_section_offset: usize,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN);
    out.extend_from_slice(&MAGIC);
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
    out.extend_from_slice(&(nav_section_offset as u32).to_le_bytes());
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

/// Serialize a pyramid of LOD layers into the full v9 `.obcm` byte stream (header
/// field order, LOD table layout, the bbox stored as lat,lon,lat,lon, the POI
/// section §7, and the trailing nav-graph section §8). `pois` is the deduped
/// classified POI list, `nav` the routable graph, and `profiles` the `1..=8` routing
/// profiles (§8.6) — all **always** get a section, empty or not. The second return
/// value is the total chunk-overflow feature drops (see [`pack_chunk`]).
pub fn serialize_lods(
    lods: &[LodLayer],
    styles: &[Style],
    marker_color: u16,
    global_bbox: (i64, i64, i64, i64),
    pois: &[Poi],
    nav: &NavGraph,
    profiles: &[NavProfile],
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

    // The POI section starts right after the last LOD's chunks (`cursor`); the
    // nav-graph section follows it at the file tail.
    let poi_section_offset = cursor;
    let poi_section = serialize_poi_section(pois, global_bbox, poi_section_offset);
    let nav_section_offset = poi_section_offset + poi_section.len();
    let nav_section = serialize_nav_section(nav, profiles, global_bbox, nav_section_offset);

    let mut out =
        Vec::with_capacity(lod_table_offset + table.len() + payload.len() + poi_section.len() + nav_section.len());
    out.extend_from_slice(&header_bytes(
        lod_count,
        marker_color,
        global_bbox,
        lod_table_offset,
        poi_section_offset,
        nav_section_offset,
    ));
    out.extend_from_slice(&style_data);
    out.extend_from_slice(&table);
    out.extend_from_slice(&payload);
    out.extend_from_slice(&poi_section);
    out.extend_from_slice(&nav_section);
    (out, dropped)
}

/// Streaming counterpart to [`serialize_lods`]: writes the **same** v8 byte stream,
/// but builds, serializes, and drops one LOD tree at a time. Peak memory is ~one
/// tree + one LOD's chunk bytes rather than all trees plus the whole output buffer.
///
/// The header, style table, and LOD-table offset are known up front, but the POI
/// and nav section offsets are not (they depend on every LOD's serialized size). So
/// we write header + style table (with **placeholder** POI/nav offsets) + a
/// **zeroed** LOD table, stream each LOD's `index ++ chunks`, append the POI then
/// nav sections at the resulting cursors, then `seek` back and patch the LOD table
/// and both header offset fields. Byte-identical to `serialize_lods` for the same
/// trees, POIs and graph (asserted by `streaming_matches_in_memory`). Returns
/// `(bytes_written, dropped_features)` — the latter counts chunk-overflow drops
/// (see [`pack_chunk`]) so the CLI can warn.
///
/// `build(i)` yields LOD `i`'s `(root, chunk_size, max_mpp)`, called once per level
/// in order; each tree is dropped before the next call. The POI and nav sections
/// are built in memory (small — point/junction records, not geometry) after the
/// LODs stream out.
#[allow(clippy::too_many_arguments)]
pub fn serialize_lods_streaming<W, F>(
    w: &mut W,
    lod_count: usize,
    styles: &[Style],
    marker_color: u16,
    global_bbox: (i64, i64, i64, i64),
    pois: &[Poi],
    nav: &NavGraph,
    profiles: &[NavProfile],
    mut build: F,
) -> io::Result<(u64, usize)>
where
    W: Write + Seek,
    F: FnMut(usize) -> (Node, usize, Option<f64>),
{
    let style_data = pack_style_dict(styles);
    let lod_table_offset = HEADER_LEN + style_data.len();
    let payload_start = lod_table_offset + lod_count * LOD_ENTRY_LEN;

    // 1. Header (bbox stored lat,lon,lat,lon) — needs no tree. The POI/nav section
    // offsets aren't known until the LODs are sized, so write 0 placeholders and
    // patch them in step 5.
    w.write_all(&header_bytes(lod_count, marker_color, global_bbox, lod_table_offset, 0, 0))?;

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

    // 4. The POI section begins at the current cursor (right after the last LOD);
    // the nav-graph section follows it at the file tail.
    let poi_section_offset = cursor;
    let poi_section = serialize_poi_section(pois, global_bbox, poi_section_offset);
    w.write_all(&poi_section)?;
    cursor += poi_section.len();
    let nav_section_offset = cursor;
    let nav_section = serialize_nav_section(nav, profiles, global_bbox, nav_section_offset);
    w.write_all(&nav_section)?;
    cursor += nav_section.len();

    // 5. Back-patch the LOD table and the header's two section-offset fields, then
    // leave the cursor at EOF. POI offset at header byte 32, nav at 36 (§1).
    w.seek(SeekFrom::Start(lod_table_offset as u64))?;
    w.write_all(&table)?;
    w.seek(SeekFrom::Start(32))?;
    w.write_all(&(poi_section_offset as u32).to_le_bytes())?;
    w.write_all(&(nav_section_offset as u32).to_le_bytes())?;
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
        // 2-LOD pyramid *and* POI + nav sections — the two back-patched header
        // offsets and the trailing sections in the streaming path are easy to
        // drift, so the fixture carries POIs of a few categories (two sharing one
        // pooled schedule) *and* a small nav graph.
        use crate::geom::Geom;
        use crate::nav::{Edge, Node as NavNode};
        use crate::poi::Poi;
        use std::io::Cursor;

        let bbox = (0, 0, 1_000_000, 1_000_000);
        let styles =
            vec![Style { id: 1, z_index: 0, color: 0x1234, weight: 2, priority: 1, dashed: false, color2: None }];
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
        let poi = |subtype, lon, lat, name: Option<&str>, hours: Option<&str>| Poi {
            subtype,
            lon_udeg: lon,
            lat_udeg: lat,
            name: name.map(String::from),
            from_node: true,
            hours: hours.and_then(crate::hours::parse),
        };
        let pois = vec![
            poi(1, 100_000, 100_000, Some("Brunnen"), None),
            poi(5, 200_000, 200_000, None, Some("Mo-Fr 08:00-18:00")),
            poi(17, 300_000, 300_000, Some("Apotheke"), Some("Mo-Fr 08:00-18:00")),
            poi(18, 400_000, 400_000, Some("Velowerkstatt"), Some("24/7")),
        ];
        let nav = NavGraph {
            nodes: vec![NavNode { id: 0, coord: (100_000, 100_000) }, NavNode { id: 1, coord: (200_000, 200_000) }],
            edges: vec![Edge {
                a: 0,
                b: 1,
                polyline: vec![(100_000, 100_000), (150_000, 160_000), (200_000, 200_000)],
                length_m: 15_700,
                kind: 0,
            }],
        };

        // Two profiles so §8.6 is non-trivial and the streaming/in-memory paths must agree on its
        // bytes as well as the graph's.
        let profiles = vec![
            NavProfile { name: "Road".into(), highway: [16; 32], surface: [16; 8] },
            NavProfile { name: "Gravel".into(), highway: [24; 32], surface: [32; 8] },
        ];

        let (reference, ref_dropped) = serialize_lods(&lods, &styles, 0xABCD, bbox, &pois, &nav, &profiles);
        assert_eq!(ref_dropped, 0, "nothing overflows in this fixture");

        let mut cur = Cursor::new(Vec::new());
        let (total, dropped) =
            serialize_lods_streaming(&mut cur, lods.len(), &styles, 0xABCD, bbox, &pois, &nav, &profiles, |i| {
                (lods[i].root.clone(), lods[i].chunk_size, lods[i].max_mpp)
            })
            .unwrap();

        assert_eq!(cur.into_inner(), reference, "streaming output must be byte-identical");
        assert_eq!(total as usize, reference.len());
        assert_eq!(dropped, 0, "and reports the same (zero) drop count");
    }
}
