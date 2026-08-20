//! Navigation-graph directories, profiles, streaming decode, and reader operations.

mod cache;

pub use cache::{NavCacheStats, NavTileCache};

use super::{aligned_index_end, fixed_chunk_range, resolve, MapReadError, QuadIndex, Reader, MAX_QUADTREE_DEPTH};
use crate::Error;
use heapless::Vec;
use obc_formats::io::{rd_i16, rd_i32, rd_u16, rd_u32, ByteSource, Error as IoError};
use obc_formats::obcm::{
    nav_edge_id_chunk, nav_edge_id_ordinal, nav_edge_record_range, OffsetScale, BRANCH_BIT, CHUNK_END, EMPTY_LEAF,
    HEADER_LEN, NAV_CHUNK_SIZE, NAV_DIR_LEN, NAV_EDGE_FIXED_LEN, NAV_EDGE_MAX_CHUNKS, NAV_MAX_PROFILES,
    NAV_NEIGHBOR_ASCENT_OFF, NAV_NEIGHBOR_LEN, NAV_NODE_FIXED_LEN, NAV_PROFILE_CLIMB_WEIGHT_OFF, NAV_PROFILE_LEN,
    NAV_PROFILE_NAME_LEN, NAV_SNAP_RECORD_LEN,
};
use obc_map_scene::{cos_lat, ground_dist_m_cl, BBox, M_PER_DEG};

/// Upper bound on the nav `chunk_size` the reader accepts (spec §8.1), and the byte size of one
/// [`NavTileCache`] slot. v9 pins the wire value to exactly [`NAV_CHUNK_SIZE`] (512), so this equals
/// it: the cache holds whole 512 B chunks and `NavTileCache::chunk`'s `debug_assert` guards that
/// no larger chunk is ever routed through a slot.
pub const NAV_MAX_CHUNK_BYTES: usize = NAV_CHUNK_SIZE;

/// Every byte [`Reader::nav_edge`] is allowed to put on the stack: **one** §8.4 chunk.
///
/// This is a budget rather than an alias so that a regression has to argue with a name. `nav_edge`
/// is the one edge-resolve site with no caller-owned [`NavTileCache`] — it resolves a single id,
/// once, from a `&self` that any task may hold — and that is exactly what makes reaching for a
/// cache there so easy and so expensive: `NavTileCache` is 24,852 B (pinned in
/// `nav/cache.rs`) against a device stack of roughly 36 KB, so one `NavTileCache::new()` in this
/// frame spends about two thirds of the task's stack to hold a 512-byte read. It shipped that way
/// in the v14 rewrite and #1422's review caught it.
///
/// **What the assertions below do, and what they do not.** They pin *this constant* — that the
/// budget is one chunk and stays an order of magnitude under a `NavTileCache`. That catches the
/// budget drifting upward until it is a cache by another name.
///
/// They cannot see inside [`Reader::nav_edge`]. Adding a `NavTileCache::new()` to that function
/// while leaving this constant at 512 passes both, because a `const` assertion is not a frame-size
/// analysis and Rust gives us no way to write one here. The real guard against *that* is the
/// board's measured `residual_stack` in `firmware/tools/resource_baseline.json`, which is a
/// whole-task figure rather than a per-function one — so this constant is a drift pin, and the
/// review of the function body is what keeps the body honest.
pub const NAV_EDGE_STACK_BUDGET: usize = NAV_CHUNK_SIZE;
const _: () = assert!(NAV_EDGE_STACK_BUDGET == NAV_CHUNK_SIZE, "nav_edge holds exactly one §8.4 chunk");
const _: () = assert!(
    NAV_EDGE_STACK_BUDGET * 8 < core::mem::size_of::<cache::NavTileCache>(),
    "nav_edge's stack budget must stay an order of magnitude under a NavTileCache, or it has become one"
);

/// The parsed nav directory (spec §8.1) — the graph's **entire resident state** (the quadtree and
/// every record stream on demand). Empty graph (`node_count == 0`) ⇒ no walk, exactly like an
/// empty POI category. Parse-only in R2: [`Reader::for_each_nav_node`] walks the node quadtree and
/// [`Reader::nav_edge`] fetches one polyline; the A* over them is R3 (#465).
#[derive(Debug, Clone, Copy)]
pub struct NavDirectory {
    /// Byte offset to the node quadtree index (§8.2 — the §4 encoding over the global bbox).
    pub index_offset: u64,
    /// Number of `uint32` nodes in the index; `0` ⇒ the map has no routable graph.
    pub node_count: usize,
    /// Number of node data chunks (they begin at `index_offset + node_count * 4`, the §3/§4
    /// convention).
    pub chunk_count: usize,
    /// Byte offset of the edge pool (§8.4).
    pub edge_pool_offset: u64,
    /// Number of `chunk_size`-byte chunks in the edge pool.
    pub edge_chunk_count: usize,
    /// Fixed capacity (bytes) of every nav chunk — node chunks and edge-pool chunks alike. v9 pins
    /// this to [`NAV_CHUNK_SIZE`] (512); `parse_nav_directory` rejects any other value.
    pub chunk_size: usize,
    /// Absolute byte offset of the §8.6 profile table (written immediately after this directory).
    pub profile_table_offset: u64,
    /// Number of 56-byte profile records at `profile_table_offset` (1..=8; parse rejects otherwise).
    pub profile_count: usize,
    /// Byte offset to the v13 §8.7 snap-anchor quadtree index.
    pub snap_index_offset: u64,
    /// Number of `uint32` nodes in the snap-anchor quadtree; `0` means no interior anchors.
    pub snap_node_count: usize,
    /// Number of fixed-size snap-anchor data chunks following that index.
    pub snap_chunk_count: usize,
    /// This file's offset unit (§1.1), retained for the `align_up` step that places the node and
    /// snap chunks behind their indexes.
    pub scale: OffsetScale,
}

impl NavDirectory {
    /// The directory a reader with no graph of its own reports — every offset zero and
    /// `node_count == 0`, so [`NavDirectory::is_empty`] is true and no walk starts. It is what a
    /// **volume-set shard** reader answers: the nav graph lives in the core file alone (`OBCA_Spec`
    /// §5.1), and the core's offsets mean nothing against a shard's bytes.
    pub const EMPTY: NavDirectory = NavDirectory {
        index_offset: 0,
        node_count: 0,
        chunk_count: 0,
        edge_pool_offset: 0,
        edge_chunk_count: 0,
        chunk_size: 0,
        profile_table_offset: 0,
        profile_count: 0,
        snap_index_offset: 0,
        snap_node_count: 0,
        snap_chunk_count: 0,
        scale: OffsetScale::DEFAULT,
    };

    /// The map carries no routable graph (no quadtree, no chunks, no edges).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.node_count == 0
    }

    /// Byte offset where the node data chunks begin (right after the index), or `None` on
    /// `u64` overflow (a corrupt directory) — see `index_end`.
    #[inline]
    pub fn data_start(&self) -> Option<u64> {
        aligned_index_end(self.scale, self.index_offset, self.node_count)
    }

    /// Byte range `[start, end)` of node chunk `chunk_id`, or `None` if out of range / on
    /// overflow. See [`fixed_chunk_range`]; the nav chunk size is directory-wide (§8.1).
    #[inline]
    fn chunk_range(&self, chunk_id: u32) -> Option<(u64, u64)> {
        fixed_chunk_range(self.data_start(), self.chunk_count, self.chunk_size, chunk_id)
    }

    /// Byte offset where v13's snap-anchor chunks begin (right after their quadtree index).
    #[inline]
    pub fn snap_data_start(&self) -> Option<u64> {
        aligned_index_end(self.scale, self.snap_index_offset, self.snap_node_count)
    }

    /// Byte range of one v13 snap-anchor chunk.
    #[inline]
    fn snap_chunk_range(&self, chunk_id: u32) -> Option<(u64, u64)> {
        fixed_chunk_range(self.snap_data_start(), self.snap_chunk_count, self.chunk_size, chunk_id)
    }
}

impl QuadIndex for NavDirectory {
    #[inline]
    fn index_offset(&self) -> u64 {
        self.index_offset
    }
    #[inline]
    fn node_count(&self) -> usize {
        self.node_count
    }
}

#[derive(Clone, Copy)]
struct NavSnapIndex {
    index_offset: u64,
    node_count: usize,
}

impl QuadIndex for NavSnapIndex {
    #[inline]
    fn index_offset(&self) -> u64 {
        self.index_offset
    }
    #[inline]
    fn node_count(&self) -> usize {
        self.node_count
    }
}

/// One §8.6 routing profile resident in [`super::MapTables`]: a display name plus the two multiplier
/// tables (`u8` fixed-point 1/16, indexed by way-kind's highway class 0..=31 / surface class
/// 0..=7; `16` = 1.0×, `0` = forbidden). N3 selects one by index and weights each edge by
/// [`MapProfile::multiplier`]. Parsed by `parse_nav_profiles`, which clamps any non-zero byte
/// below 16 up to 16 (defensive — the packer already enforces the admissibility invariant).
#[derive(Debug, Clone, Copy)]
pub struct MapProfile {
    /// Raw name field (0xFF-padded); read via [`MapProfile::name`].
    name: [u8; NAV_PROFILE_NAME_LEN],
    /// Name length in bytes (up to the first 0xFF pad).
    name_len: usize,
    /// Multiplier per highway class (5-bit index). `16` = 1.0×, `0` = forbidden.
    pub highway: [u8; 32],
    /// Multiplier per surface class (3-bit index). Same encoding.
    pub surface: [u8; 8],
    /// Flat-metres-equivalent charged per metre of a neighbor entry's `Ascent M` (§8.6, v12).
    /// `0` = climb-blind, which is what a map packed before terrain existed decodes to.
    climb_weight: u8,
}

impl MapProfile {
    /// The profile's display name (UTF-8, trailing `0xFF` padding trimmed); `""` if not valid UTF-8.
    #[inline]
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("")
    }

    /// The profile's climb weight (§8.6, v12): flat metres charged per metre of ascent. `0` means
    /// climb-blind — the router costs the edge by ground length alone, exactly as v11 did.
    ///
    /// The term it feeds is **additive and non-negative**: weighted cost is
    /// `(cost_m × effective) >> 4 + ascent_m × climb_weight`, saturating. Descent can never make an
    /// edge cheaper than its profile-weighted ground length, which is what keeps the great-circle
    /// heuristic admissible (EL6 is the router side; the reader only serves the number).
    #[inline]
    pub fn climb_weight(&self) -> u8 {
        self.climb_weight
    }

    /// Effective edge-weight multiplier for a packed `way_kind` byte, in 1/16 fixed-point:
    /// `(highway[kind & 31] × surface[kind >> 5]) >> 4` (u32 math). `None` if either class is
    /// forbidden (a `0` byte) — the edge is not routable under this profile (§8.6).
    #[inline]
    pub fn multiplier(&self, way_kind: u8) -> Option<u32> {
        let mh = self.highway[(way_kind & 0x1F) as usize] as u32;
        let ms = self.surface[(way_kind >> 5) as usize] as u32;
        if mh == 0 || ms == 0 {
            None
        } else {
            Some((mh * ms) >> 4)
        }
    }
}

/// One adjacency entry of a decoded §8.3 junction record. Coordinates are absolute microdegrees,
/// **reconstructed** from the record's own coord + the stored `i16` deltas; `edge_id` addresses the
/// §8.4 edge pool ([`Reader::nav_edge`]); `cost_m` is the edge's raw ground length in meters (the
/// unweighted distance — N3 weights it by profile at relaxation); `way_kind` is N1's packed class
/// byte, the input to profile weighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavNeighbor {
    pub id: u32,
    pub lat: i32,
    pub lon: i32,
    pub edge_id: u32,
    pub cost_m: u32,
    pub way_kind: u8,
    /// Integrated climb (m) of riding this edge **from the record's node toward this neighbor**
    /// (§8.3, v12). **Directional**: the opposite entry of the same edge carries that direction's
    /// ascent, i.e. this direction's descent — the one field of an adjacency entry that legitimately
    /// differs between the two sides. `0` everywhere on a map packed without terrain.
    pub ascent_m: u16,
}

/// One exact position along a §8.4 edge polyline. `segment` names the forward `a → b` segment and
/// `fraction` is its 0..=65535 interpolation parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavEdgePosition {
    pub segment: u16,
    pub fraction: u16,
    pub coord: (i32, i32),
}

/// A projected candidate before its graph endpoints are resolved. Keeping endpoint lookup out of
/// the candidate scan means only the winning edge pays the two point-quadtree queries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavEdgeCandidate {
    pub edge_id: u32,
    pub way_kind: u8,
    pub length_m: u32,
    pub from_a_m: u32,
    pub distance_m: f32,
    pub position: NavEdgePosition,
    pub a_coord: (i32, i32),
    pub b_coord: (i32, i32),
    pub a_position: NavEdgePosition,
    pub b_position: NavEdgePosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavEdgeEndpoint {
    pub id: u32,
    pub coord: (i32, i32),
    pub position: NavEdgePosition,
}

/// A winning exact edge projection, connected to its two real graph endpoints. Directional ascent
/// is carried so the virtual partial edges use the same climb-aware cost model as ordinary A* arcs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavEdgeSnap {
    pub edge_id: u32,
    pub way_kind: u8,
    pub length_m: u32,
    pub from_a_m: u32,
    pub distance_m: f32,
    pub position: NavEdgePosition,
    pub a: NavEdgeEndpoint,
    pub b: NavEdgeEndpoint,
    pub ascent_ab: u16,
    pub ascent_ba: u16,
}

#[inline]
fn project_to_nav_segment(a: (i32, i32), b: (i32, i32), p: (i32, i32), cl: f32) -> (f32, f32) {
    let scale = M_PER_DEG as f32 / 1_000_000.0;
    let bx = (b.0 - a.0) as f32 * scale * cl;
    let by = (b.1 - a.1) as f32 * scale;
    let px = (p.0 - a.0) as f32 * scale * cl;
    let py = (p.1 - a.1) as f32 * scale;
    let len2 = bx * bx + by * by;
    if len2 <= 1e-9 {
        return (0.0, libm::sqrtf(px * px + py * py));
    }
    let t = ((px * bx + py * by) / len2).clamp(0.0, 1.0);
    let (dx, dy) = (px - bx * t, py - by * t);
    (t, libm::sqrtf(dx * dx + dy * dy))
}

#[inline]
fn candidate_beats(new: &NavEdgeCandidate, old: &NavEdgeCandidate) -> bool {
    new.distance_m < old.distance_m || (new.distance_m == old.distance_m && new.edge_id < old.edge_id)
}

/// One §8.3 junction record, borrowed from the chunk scratch for a single
/// [`Reader::for_each_nav_node`] callback. Neighbor entries decode lazily through
/// [`NavNodeRef::neighbors`] — A* relaxes them straight off the record with no intermediate copy.
#[derive(Debug, Clone, Copy)]
pub struct NavNodeRef<'a> {
    /// Absolute microdegrees.
    pub lat: i32,
    /// Absolute microdegrees.
    pub lon: i32,
    /// The pack-run-dense node id (the A* hash key).
    pub id: u32,
    /// Raw neighbor bytes, `degree` × [`NAV_NEIGHBOR_LEN`] — length-validated by the walk.
    neighbors: &'a [u8],
}

impl<'a> NavNodeRef<'a> {
    /// This junction's degree (0..=254; the packer caps it far lower, spec §8.3).
    #[inline]
    pub fn degree(&self) -> usize {
        self.neighbors.len() / NAV_NEIGHBOR_LEN
    }

    /// Iterate the adjacency entries in record order. Each neighbor's absolute coord is
    /// reconstructed as `record coord + i16 delta` (cast-free `from_le_bytes` on the slice — the
    /// #501 alignment rule); `cost_m` widens from the `u16` wire value; `way_kind` is a raw byte;
    /// `ascent_m` is the v12 directional climb at [`NAV_NEIGHBOR_ASCENT_OFF`].
    #[inline]
    pub fn neighbors(&self) -> impl Iterator<Item = NavNeighbor> + 'a {
        let (base_lat, base_lon) = (self.lat, self.lon);
        self.neighbors.as_chunks::<NAV_NEIGHBOR_LEN>().0.iter().map(move |e| NavNeighbor {
            id: rd_u32(e, 0),
            lat: base_lat.wrapping_add(rd_i16(e, 4) as i32),
            lon: base_lon.wrapping_add(rd_i16(e, 6) as i32),
            edge_id: rd_u32(e, 8),
            cost_m: rd_u16(e, 12) as u32,
            way_kind: e[14],
            ascent_m: rd_u16(e, NAV_NEIGHBOR_ASCENT_OFF),
        })
    }
}

impl<'a> Reader<'a> {
    /// The parsed nav directory (spec §8.1). Always present in v9; `is_empty()` for a map with no
    /// routable ways.
    ///
    /// It used to answer [`NavDirectory::EMPTY`] on a volume-set shard, because a shard reader
    /// borrowed the **core's** tables and this directory's offsets would then have described the
    /// core file against the shard's bytes. One file, one set of tables, one graph (FS7.5, #1420) —
    /// there is no longer a reader whose `tables` are not its `src`'s, so the guard has nothing to
    /// guard against and every nav, POI and hours accessor below simply answers.
    #[inline]
    pub fn nav_directory(&self) -> &NavDirectory {
        &self.tables.nav
    }

    /// The map's §8.6 routing profiles (1..=8, always present even for an empty graph). N5 exposes
    /// their names on the device; N3 selects one by index and weights edges by
    /// [`MapProfile::multiplier`].
    #[inline]
    pub fn nav_profiles(&self) -> &[MapProfile] {
        &self.tables.profiles
    }

    /// Visit every §8.3 junction record whose quadtree leaf overlaps `view`, in quadtree order —
    /// the R3 A* spatial-refetch primitive: settling a node is one descent to its coord's leaf
    /// (a degenerate one-point `view`) + one chunk read + this decode. Parse-only here: no
    /// traversal state, no ordering beyond the walk.
    ///
    /// `scratch` is the caller-owned chunk buffer and must hold at least the directory's
    /// `chunk_size` bytes (≤ [`NAV_MAX_CHUNK_BYTES`], enforced at parse) — `Err(Error::TooShort)`
    /// otherwise. R3/R4 point this at their graph-tile cache slot; tests pass a stack array. An
    /// empty graph visits nothing. A truncated record ends that chunk cleanly; source or
    /// index-cache failures return a typed [`Error`] rather than being mistaken for an empty leaf.
    ///
    /// # Reentrancy
    ///
    /// Like [`Reader::nearest_pois`], the quadtree walk streams through the internal index cache;
    /// legal re-entry returns [`Error::CacheBusy`].
    pub fn for_each_nav_node(
        &self,
        view: &BBox,
        scratch: &mut [u8],
        mut visit: impl FnMut(NavNodeRef),
    ) -> Result<(), Error> {
        let dir = *self.nav_directory();
        if dir.is_empty() {
            return Ok(());
        }
        if scratch.len() < dir.chunk_size {
            return Err(Error::TooShort);
        }
        let mut read_error = None;
        self.walk_leaves(&dir, 0, self.bbox, view, 0, &mut |cid, _node| {
            if read_error.is_some() {
                return;
            }
            let (start, end) = match dir.chunk_range(cid) {
                Some(r) => r,
                None => return,
            };
            if end > self.src.len() {
                return;
            }
            let chunk = &mut scratch[..dir.chunk_size];
            if let Err(error) = self.src.read_at(start, chunk) {
                read_error = Some(error);
                return;
            }
            decode_nav_chunk(chunk, &mut visit);
        })
        .map_err(Error::from)?;
        if let Some(error) = read_error {
            return Err(Error::Source(error));
        }
        Ok(())
    }

    /// Resolve a §8.4 `Edge Id` — a packed `(chunk_index, ordinal)` pair since v14 — to the
    /// 512-byte chunk holding the record and the record's byte position inside it.
    ///
    /// The `ordinal` is the record's **position** within its chunk, not a byte offset into it, so
    /// this reads the one chunk and walks `ordinal` records from its first byte, taking each
    /// record's length from its own `Pt Count`. That walk is
    /// [`nav_edge_record_range`](obc_formats::obcm::nav_edge_record_range), transcribed from the
    /// spec and applied `ordinal + 1` times, so the intermediate records are bounds-checked exactly
    /// as the target is. There is **no extra I/O**: the chunk that holds the record is the chunk
    /// that holds every record before it, which is the whole reason the ordinal is *within a chunk*
    /// and not within the pool.
    ///
    /// A refused id is a malformed map, not an absent edge — but this is a reader on a card that
    /// ages, so every caller degrades to "no geometry" rather than panicking.
    fn nav_edge_record<'t>(&self, tiles: &'t mut NavTileCache, edge_id: u32) -> Option<(&'t [u8], usize)> {
        let chunk_start = self.nav_edge_chunk_start(edge_id)?;
        let chunk = tiles.chunk(self.src, chunk_start, NAV_CHUNK_SIZE)?;
        let (start, _end) = nav_edge_record_range(chunk, nav_edge_id_ordinal(edge_id))?;
        Some((chunk, start))
    }

    /// The absolute file offset of the chunk holding `edge_id`'s record — the half of the §8.4
    /// resolve that does not care *how* the chunk is read.
    ///
    /// Split out so the two readers below can differ in exactly one line: the router's sites
    /// already own a [`NavTileCache`] and go through its working set, while [`Reader::nav_edge`]
    /// reads into 512 bytes of its own stack.
    fn nav_edge_chunk_start(&self, edge_id: u32) -> Option<u64> {
        let dir = self.nav_directory();
        let cs = dir.chunk_size;
        if dir.edge_chunk_count == 0 || cs != NAV_CHUNK_SIZE {
            return None;
        }
        let chunk_index = nav_edge_id_chunk(edge_id) as u64;
        if chunk_index >= dir.edge_chunk_count as u64 {
            return None;
        }
        let chunk_start = dir.edge_pool_offset.checked_add(chunk_index.checked_mul(cs as u64)?)?;
        if chunk_start.checked_add(cs as u64)? > self.src.len() {
            return None;
        }
        Some(chunk_start)
    }

    /// [`Reader::nav_edge_record`] with the chunk read straight into a caller-owned 512-byte
    /// buffer instead of through a [`NavTileCache`] working set.
    ///
    /// This exists because the cache is **24,852 bytes** and `nav_edge` has no business owning
    /// one: it reads exactly one chunk, once, and never comes back for a neighbour. Materialising
    /// a cache to hold a single 512-byte read put two thirds of the device's ~36 KB stack into one
    /// frame — see the pin below.
    fn nav_edge_record_uncached<'b>(
        &self,
        buf: &'b mut [u8; NAV_CHUNK_SIZE],
        edge_id: u32,
    ) -> Option<(&'b [u8], usize)> {
        let chunk_start = self.nav_edge_chunk_start(edge_id)?;
        self.src.read_at(chunk_start, &mut buf[..]).ok()?;
        let (start, _end) = nav_edge_record_range(&buf[..], nav_edge_id_ordinal(edge_id))?;
        Some((&buf[..], start))
    }

    /// Fetch one §8.4 edge polyline by its `edge_id` (a packed `(chunk, ordinal)` pair since v14),
    /// decoding anchor + deltas into `points` as the crate's `(lon, lat)` µdeg pairs. Returns the
    /// edge's `length_m`. R3 calls this only at OBCR emit, stitching the came-from chain's geometry.
    ///
    /// `None` for an empty graph, an id whose chunk or ordinal the §8.4 walk refuses, a read
    /// failure, or a polyline exceeding `P` — a corrupt id degrades to "no geometry", never a
    /// panic. No cache is touched, so (like [`Reader::poi_hours`]) this is safe to call from
    /// anywhere; it holds one 512-byte chunk on the stack, which is both what the ordinal walk
    /// needs and *fewer* source reads than v13's windowed delta stream, since the deltas were
    /// always inside the chunk the head came from.
    ///
    /// The sentence above is now true. Between the v14 rewrite and #1422's review it was not: the
    /// body built a [`NavTileCache`] to perform that one read, so the frame was
    /// [`NAV_EDGE_STACK_BUDGET`] + 24,852 B on a device with about 36 KB of stack — 174× v13's
    /// frame, in a `no_std` crate, for a single 512-byte chunk.
    pub fn nav_edge<const P: usize>(&self, edge_id: u32, points: &mut Vec<(i32, i32), P>) -> Option<u32> {
        points.clear();
        let mut chunk_buf = [0u8; NAV_EDGE_STACK_BUDGET];
        let (chunk, within) = self.nav_edge_record_uncached(&mut chunk_buf, edge_id)?;
        let length_m = rd_u32(chunk, within);
        let pt_count = rd_u16(chunk, within + 4) as usize;
        // byte 6 is `way_kind` (§8.4); the anchor sits behind it, at 7 (lat) / 11 (lon).
        let anchor_lat = rd_i32(chunk, within + 7);
        let anchor_lon = rd_i32(chunk, within + 11);
        // The walk already refused `Pt Count < 2` and any record claiming bytes past its chunk.
        let rec_len = NAV_EDGE_FIXED_LEN + (pt_count - 1) * 4;
        if pt_count > P {
            return None; // caller's buffer can't hold the polyline — corrupt or mis-sized
        }
        points.push((anchor_lon, anchor_lat)).ok()?;
        let (mut lat, mut lon) = (anchor_lat, anchor_lon);
        for pair in chunk[within + NAV_EDGE_FIXED_LEN..within + rec_len].as_chunks::<4>().0 {
            lat = lat.wrapping_add(rd_i16(pair, 0) as i32);
            lon = lon.wrapping_add(rd_i16(pair, 2) as i32);
            points.push((lon, lat)).ok()?;
        }
        Some(length_m)
    }

    /// [`Reader::for_each_nav_node`] with the chunk read routed through a caller-owned
    /// [`NavTileCache`] instead of a bare scratch — the router's settle primitive (#465). A*'s
    /// spatial re-fetch settles one node at a time (a degenerate one-point `view`). It does **not**
    /// walk a single advancing neighborhood: the heap pops the globally best-`f` node, so successive
    /// settles scatter across the frontier's several live quadtree leaves. The route-private
    /// navigation-cache working set keeps those leaves resident so the per-settle re-fetch mostly
    /// hits. Same decode, same corrupt-input posture, same reentrancy rule as the uncached walk.
    pub fn for_each_nav_node_cached(
        &self,
        view: &BBox,
        tiles: &mut NavTileCache,
        mut visit: impl FnMut(NavNodeRef),
    ) -> Result<(), Error> {
        let dir = *self.nav_directory();
        if dir.is_empty() {
            return Ok(());
        }
        let mut read_error = None;
        self.walk_nav_leaves(&dir, 0, self.bbox, view, 0, tiles, &mut |tiles, cid, _node| {
            if read_error.is_some() {
                return;
            }
            let (start, end) = match dir.chunk_range(cid) {
                Some(r) => r,
                None => return,
            };
            if end > self.src.len() {
                return;
            }
            // A failed fill skips this leaf cleanly (the cache never keeps a bad slot).
            match tiles.chunk(self.src, start, dir.chunk_size) {
                Some(chunk) => decode_nav_chunk(chunk, &mut visit),
                None => read_error = Some(IoError::Io),
            }
        })
        .map_err(Error::from)?;
        if let Some(error) = read_error {
            return Err(Error::Source(error));
        }
        Ok(())
    }

    /// Find the nearest exact edge projection among (a) edges incident to graph nodes in `view`
    /// and (b) edges named by v13 interior anchors in `view`. The two indexes together are the
    /// completeness argument: endpoints cover short edges; long edges have interior anchors no
    /// farther than 300 m apart. Endpoint graph ids are deliberately resolved only after the
    /// lookup windows have selected a winner; see
    /// [`resolve_nav_edge_candidate_cached`](Self::resolve_nav_edge_candidate_cached).
    #[inline(never)] // keep the one 512-byte node-chunk copy in this bounded snap-only frame
    pub fn nearest_nav_edge_candidate_cached(
        &self,
        view: &BBox,
        tiles: &mut NavTileCache,
        p: (i32, i32),
        max_distance_m: f32,
    ) -> Result<Option<NavEdgeCandidate>, Error> {
        let dir = *self.nav_directory();
        if dir.is_empty() {
            return Ok(None);
        }
        let mut best: Option<NavEdgeCandidate> = None;
        let mut read_error = None;

        // First the ordinary node tree: this supplies every short edge and also helps long edges
        // near a junction. Copy each chunk before edge-pool reads are allowed to evict its cache
        // slot.
        self.walk_nav_leaves(&dir, 0, self.bbox, view, 0, tiles, &mut |tiles, cid, _node| {
            if read_error.is_some() {
                return;
            }
            let Some((start, end)) = dir.chunk_range(cid) else { return };
            if end > self.src.len() {
                return;
            }
            let mut local = [0u8; NAV_MAX_CHUNK_BYTES];
            {
                let Some(chunk) = tiles.chunk(self.src, start, dir.chunk_size) else {
                    read_error = Some(IoError::Io);
                    return;
                };
                local[..dir.chunk_size].copy_from_slice(chunk);
            }
            decode_nav_chunk(&local[..dir.chunk_size], &mut |n| {
                if n.lon < view.min_lon || n.lon > view.max_lon || n.lat < view.min_lat || n.lat > view.max_lat {
                    return;
                }
                for nb in n.neighbors() {
                    let Some(candidate) = self.project_nav_edge_cached(tiles, nb.edge_id, p) else { continue };
                    if candidate.distance_m <= max_distance_m
                        && best.is_none_or(|old| candidate_beats(&candidate, &old))
                    {
                        best = Some(candidate);
                    }
                }
            });
        })
        .map_err(Error::from)?;

        // Then the sparse long-edge anchors. Leaves may share chunks, so filter by the absolute
        // record coordinate; repeated edge ids are harmless and normally hit the edge cache.
        if dir.snap_node_count > 0 && read_error.is_none() {
            let index = NavSnapIndex { index_offset: dir.snap_index_offset, node_count: dir.snap_node_count };
            self.walk_nav_leaves(&index, 0, self.bbox, view, 0, tiles, &mut |tiles, cid, _node| {
                if read_error.is_some() {
                    return;
                }
                let Some((start, end)) = dir.snap_chunk_range(cid) else { return };
                if end > self.src.len() {
                    return;
                }
                let mut local = [CHUNK_END; NAV_CHUNK_SIZE];
                {
                    let Some(chunk) = tiles.chunk(self.src, start, dir.chunk_size) else {
                        read_error = Some(IoError::Io);
                        return;
                    };
                    local[..dir.chunk_size].copy_from_slice(chunk);
                }
                for rec in local[..dir.chunk_size].as_chunks::<NAV_SNAP_RECORD_LEN>().0 {
                    let edge_id = rd_u32(rec, 8);
                    if edge_id == u32::MAX {
                        break;
                    }
                    let (lat, lon) = (rd_i32(rec, 0), rd_i32(rec, 4));
                    if lon < view.min_lon || lon > view.max_lon || lat < view.min_lat || lat > view.max_lat {
                        continue;
                    }
                    let Some(candidate) = self.project_nav_edge_cached(tiles, edge_id, p) else { continue };
                    if candidate.distance_m <= max_distance_m
                        && best.is_none_or(|old| candidate_beats(&candidate, &old))
                    {
                        best = Some(candidate);
                    }
                }
            })
            .map_err(Error::from)?;
        }
        if let Some(error) = read_error {
            return Err(Error::Source(error));
        }
        Ok(best)
    }

    /// Resolve the winning candidate's endpoint ids and directional ascents through two degenerate
    /// node-tree queries. Only this winner pays the lookups; all rejected candidates needed only
    /// their edge-pool geometry.
    pub fn resolve_nav_edge_candidate_cached(
        &self,
        candidate: NavEdgeCandidate,
        tiles: &mut NavTileCache,
    ) -> Result<Option<NavEdgeSnap>, Error> {
        let mut ids: Option<(u32, u32)> = None;
        let mut ascent_ab = None;
        let mut ascent_ba = None;
        for (coord, other, forward) in
            [(candidate.a_coord, candidate.b_coord, true), (candidate.b_coord, candidate.a_coord, false)]
        {
            let view = BBox { min_lon: coord.0, min_lat: coord.1, max_lon: coord.0, max_lat: coord.1 };
            self.for_each_nav_node_cached(&view, tiles, |n| {
                if (n.lon, n.lat) != coord {
                    return;
                }
                for nb in n.neighbors() {
                    if nb.edge_id != candidate.edge_id || (nb.lon, nb.lat) != other {
                        continue;
                    }
                    let pair = if forward { (n.id, nb.id) } else { (nb.id, n.id) };
                    if ids.is_none_or(|old| old == pair) {
                        ids = Some(pair);
                        if forward {
                            ascent_ab = Some(nb.ascent_m);
                        } else {
                            ascent_ba = Some(nb.ascent_m);
                        }
                    }
                }
            })?;
        }
        let Some((a_id, b_id)) = ids else { return Ok(None) };
        Ok(Some(NavEdgeSnap {
            edge_id: candidate.edge_id,
            way_kind: candidate.way_kind,
            length_m: candidate.length_m,
            from_a_m: candidate.from_a_m,
            distance_m: candidate.distance_m,
            position: candidate.position,
            a: NavEdgeEndpoint { id: a_id, coord: candidate.a_coord, position: candidate.a_position },
            b: NavEdgeEndpoint { id: b_id, coord: candidate.b_coord, position: candidate.b_position },
            ascent_ab: ascent_ab.unwrap_or(0),
            ascent_ba: ascent_ba.unwrap_or(0),
        }))
    }

    /// Stream an inclusive edge slice between two exact projected positions, in either direction.
    pub fn nav_edge_slice_oriented(
        &self,
        tiles: &mut NavTileCache,
        edge_id: u32,
        from: NavEdgePosition,
        to: NavEdgePosition,
        mut emit: impl FnMut((i32, i32)),
    ) -> Option<u32> {
        let dir = self.nav_directory();
        let cs = dir.chunk_size;
        if dir.edge_chunk_count == 0 || cs == 0 {
            return None;
        }
        let (chunk, within) = self.nav_edge_record(tiles, edge_id)?;
        let length_m = rd_u32(chunk, within);
        let pt_count = rd_u16(chunk, within + 4) as usize;
        if pt_count < 2 || from.segment as usize + 1 >= pt_count || to.segment as usize + 1 >= pt_count {
            return None;
        }
        let rec_len = NAV_EDGE_FIXED_LEN.checked_add((pt_count - 1).checked_mul(4)?)?;
        if within + rec_len > cs {
            return None;
        }
        let deltas = &chunk[within + NAV_EDGE_FIXED_LEN..within + rec_len];
        let anchor = (rd_i32(chunk, within + 11), rd_i32(chunk, within + 7));
        let step = |(lon, lat): (i32, i32), pair: &[u8]| {
            (
                lon.wrapping_add(i16::from_le_bytes([pair[2], pair[3]]) as i32),
                lat.wrapping_add(i16::from_le_bytes([pair[0], pair[1]]) as i32),
            )
        };
        let forward = (from.segment, from.fraction) <= (to.segment, to.fraction);
        emit(from.coord);
        if forward {
            let mut point = anchor;
            for (i, pair) in deltas.as_chunks::<4>().0.iter().enumerate() {
                point = step(point, pair);
                let vertex = i + 1;
                if vertex > from.segment as usize && vertex <= to.segment as usize {
                    emit(point);
                }
            }
        } else {
            let mut point = anchor;
            for pair in deltas.as_chunks::<4>().0 {
                point = step(point, pair);
            }
            for (i, pair) in deltas.as_chunks::<4>().0.iter().enumerate().rev() {
                point = (
                    point.0.wrapping_sub(i16::from_le_bytes([pair[2], pair[3]]) as i32),
                    point.1.wrapping_sub(i16::from_le_bytes([pair[0], pair[1]]) as i32),
                );
                let vertex = i;
                if vertex <= from.segment as usize && vertex > to.segment as usize {
                    emit(point);
                }
            }
        }
        if to.coord != from.coord {
            emit(to.coord);
        }
        Some(length_m)
    }

    fn project_nav_edge_cached(
        &self,
        tiles: &mut NavTileCache,
        edge_id: u32,
        p: (i32, i32),
    ) -> Option<NavEdgeCandidate> {
        let dir = self.nav_directory();
        let cs = dir.chunk_size;
        if dir.edge_chunk_count == 0 || cs == 0 {
            return None;
        }
        let (chunk, within) = self.nav_edge_record(tiles, edge_id)?;
        let length_m = rd_u32(chunk, within);
        let pt_count = rd_u16(chunk, within + 4) as usize;
        if pt_count < 2 || pt_count - 1 > u16::MAX as usize {
            return None;
        }
        let way_kind = chunk[within + 6];
        let rec_len = NAV_EDGE_FIXED_LEN.checked_add((pt_count - 1).checked_mul(4)?)?;
        if within + rec_len > cs {
            return None;
        }
        let deltas = &chunk[within + NAV_EDGE_FIXED_LEN..within + rec_len];
        let anchor = (rd_i32(chunk, within + 11), rd_i32(chunk, within + 7));
        let step = |(lon, lat): (i32, i32), pair: &[u8]| {
            (
                lon.wrapping_add(i16::from_le_bytes([pair[2], pair[3]]) as i32),
                lat.wrapping_add(i16::from_le_bytes([pair[0], pair[1]]) as i32),
            )
        };
        let cl = cos_lat(p.1).max(1e-3);
        let mut a = anchor;
        let mut along = 0.0f32;
        let mut best_distance = f32::INFINITY;
        let mut best_along = 0.0f32;
        let mut best_position = NavEdgePosition { segment: 0, fraction: 0, coord: anchor };
        for (i, pair) in deltas.as_chunks::<4>().0.iter().enumerate() {
            let b = step(a, pair);
            let (t, distance) = project_to_nav_segment(a, b, p, cl);
            let segment_m = ground_dist_m_cl(a, b, cl);
            if distance < best_distance {
                best_distance = distance;
                best_along = along + t * segment_m;
                let lon = a.0.saturating_add(libm::roundf((b.0 - a.0) as f32 * t) as i32);
                let lat = a.1.saturating_add(libm::roundf((b.1 - a.1) as f32 * t) as i32);
                best_position = NavEdgePosition {
                    segment: i as u16,
                    fraction: libm::roundf(t * u16::MAX as f32) as u16,
                    coord: (lon, lat),
                };
            }
            along += segment_m;
            a = b;
        }
        let from_a_m = if along <= f32::EPSILON {
            0
        } else {
            (libm::roundf(length_m as f32 * best_along / along) as u32).min(length_m)
        };
        Some(NavEdgeCandidate {
            edge_id,
            way_kind,
            length_m,
            from_a_m,
            distance_m: best_distance,
            position: best_position,
            a_coord: anchor,
            b_coord: a,
            a_position: NavEdgePosition { segment: 0, fraction: 0, coord: anchor },
            b_position: NavEdgePosition { segment: (pt_count - 2) as u16, fraction: u16::MAX, coord: a },
        })
    }

    /// Route-private §8.2 walk. Unlike the renderer/POI [`Reader::walk_leaves`] path, node words are
    /// served from [`NavTileCache`]'s sixteen-window working set, so thousands of point descents do
    /// not churn the seven render-index windows. The callback receives the same mutable cache only
    /// after the node-word borrow has ended, allowing a leaf to fetch its graph chunk without nested
    /// mutable borrows.
    #[allow(clippy::too_many_arguments)]
    fn walk_nav_leaves<F: FnMut(&mut NavTileCache, u32, BBox)>(
        &self,
        index: &dyn QuadIndex,
        idx: usize,
        node: BBox,
        view: &BBox,
        depth: u32,
        tiles: &mut NavTileCache,
        visit: &mut F,
    ) -> Result<(), MapReadError> {
        if idx >= index.node_count() || depth > MAX_QUADTREE_DEPTH || !node.intersects(view) {
            return Ok(());
        }
        let val = tiles.index_node(self.src, index, idx).map_err(MapReadError::Source)?;
        if val & BRANCH_BIT == 0 {
            if val != EMPTY_LEAF {
                visit(tiles, val, node);
            }
            return Ok(());
        }
        let child = (val & !BRANCH_BIT) as usize;
        if child <= idx {
            return Err(MapReadError::Malformed);
        }
        let mid_lon = (node.min_lon + node.max_lon).div_euclid(2);
        let mid_lat = (node.min_lat + node.max_lat).div_euclid(2);
        let kids = [
            BBox { min_lon: node.min_lon, min_lat: mid_lat, max_lon: mid_lon, max_lat: node.max_lat },
            BBox { min_lon: mid_lon, min_lat: mid_lat, max_lon: node.max_lon, max_lat: node.max_lat },
            BBox { min_lon: node.min_lon, min_lat: node.min_lat, max_lon: mid_lon, max_lat: mid_lat },
            BBox { min_lon: mid_lon, min_lat: node.min_lat, max_lon: node.max_lon, max_lat: mid_lat },
        ];
        for (i, child_bbox) in kids.iter().enumerate() {
            self.walk_nav_leaves(index, child + i, *child_bbox, view, depth + 1, tiles, visit)?;
        }
        Ok(())
    }

    /// Fetch one §8.4 edge polyline **oriented to begin at `start`** (a `(lon, lat)` µdeg node
    /// coord), streaming each point through `emit` and returning the edge's `length_m`. Edge
    /// records run `a → b`; the router traverses them either way, so this picks the direction by
    /// matching `start` against the record's endpoints (node coords and polyline endpoints are
    /// bit-identical by the packer's construction) and reverses on the fly when the hop runs
    /// `b → a` — no caller-side point buffer, however long the polyline.
    ///
    /// The record's whole containing chunk comes through `tiles` (the §8.4 no-straddle contract
    /// keeps a record inside one chunk): consecutive path edges usually share a pool chunk, and a
    /// resident chunk makes the reversed decode free — the deltas are summed forward once for the
    /// `b` endpoint, then walked backward subtracting. `None` for an out-of-pool / misaligned id,
    /// a record matching neither endpoint, or a read failure — a corrupt id degrades to "no
    /// geometry", never a panic, mirroring [`Reader::nav_edge`].
    pub fn nav_edge_oriented(
        &self,
        tiles: &mut NavTileCache,
        edge_id: u32,
        start: (i32, i32),
        mut emit: impl FnMut((i32, i32)),
    ) -> Option<u32> {
        let dir = self.nav_directory();
        let cs = dir.chunk_size;
        if dir.edge_chunk_count == 0 || cs == 0 {
            return None;
        }
        let (chunk, within) = self.nav_edge_record(tiles, edge_id)?;
        let length_m = rd_u32(chunk, within);
        let pt_count = rd_u16(chunk, within + 4) as usize;
        // byte within+6 is `way_kind` (§8.4); the anchor sits behind it, at +7 (lat) / +11 (lon).
        let anchor = (rd_i32(chunk, within + 11), rd_i32(chunk, within + 7)); // (lon, lat)
        if pt_count == 0 {
            return None;
        }
        let rec_len = NAV_EDGE_FIXED_LEN.checked_add((pt_count - 1).checked_mul(4)?)?;
        if within + rec_len > cs {
            return None;
        }
        let deltas = &chunk[within + NAV_EDGE_FIXED_LEN..within + rec_len];
        let step = |(lon, lat): (i32, i32), pair: &[u8]| {
            (lon.wrapping_add(rd_i16(pair, 2) as i32), lat.wrapping_add(rd_i16(pair, 0) as i32))
        };
        if anchor == start {
            // Forward: the record already runs `start → …`.
            let mut p = anchor;
            emit(p);
            for pair in deltas.as_chunks::<4>().0 {
                p = step(p, pair);
                emit(p);
            }
            return Some(length_m);
        }
        // Maybe reversed: forward-sum the deltas for the `b` endpoint…
        let mut p = anchor;
        for pair in deltas.as_chunks::<4>().0 {
            p = step(p, pair);
        }
        if p != start {
            return None; // matches neither endpoint — a stale/corrupt edge id
        }
        // …then walk them backward, undoing one delta per point.
        emit(p);
        for pair in deltas.as_chunks::<4>().0.iter().rev() {
            p = (p.0.wrapping_sub(rd_i16(pair, 2) as i32), p.1.wrapping_sub(rd_i16(pair, 0) as i32));
            emit(p);
        }
        Some(length_m)
    }
}

/// Decode one nav node chunk's §8.3 records, handing each to `visit`. Records are back-to-back;
/// the `degree` byte at record offset 12 is `0xFF` in the `0xFF` padding, ending the walk (the POI
/// sentinel trick — the padding's first byte always lands on a would-be degree slot). A record
/// whose declared neighbors run past the chunk is corrupt: stop cleanly, decode nothing further.
///
/// **Byte-wise by contract — never a typed view.** The record stride is 13 + 17·degree bytes
/// (both odd in v12), so records — and every multi-byte field in them — sit at **odd offsets**
/// inside the chunk by design; all decoding goes through the `rd_*` `from_le_bytes`-on-`&[u8]`
/// helpers.
/// Two guards keep it that way (PR #501's on-glass HardFault dossier): the board build compiles
/// with `+strict-align` (the ARM backend fused even these byte-wise decodes into an
/// alignment-trapping `ldrd` under fat LTO — see `obc-fw-nrf54l/.cargo/config.toml`), and the
/// obc-route nav suite runs clean under **Miri** (`cargo +nightly miri test -p obc-route --test
/// nav`), which fails loudly if a typed view over these bytes ever creeps in.
fn decode_nav_chunk(chunk: &[u8], visit: &mut impl FnMut(NavNodeRef)) {
    let mut off = 0usize;
    while off + NAV_NODE_FIXED_LEN <= chunk.len() {
        let degree = chunk[off + 12] as usize;
        if degree == usize::from(CHUNK_END) {
            break;
        }
        let end = off + NAV_NODE_FIXED_LEN + degree * NAV_NEIGHBOR_LEN;
        if end > chunk.len() {
            break;
        }
        visit(NavNodeRef {
            lat: rd_i32(chunk, off),
            lon: rd_i32(chunk, off + 4),
            id: rd_u32(chunk, off + 8),
            neighbors: &chunk[off + NAV_NODE_FIXED_LEN..end],
        });
        off = end;
    }
}

/// Parse the current v13 nav directory (spec §8.1) at `offset` from `src` (file is `total` bytes).
/// Parse-only: validates the directory scalars, the node index + chunk region, the edge-pool
/// region, and the profile-table region lie in-file, but walks/decodes nothing (the profiles
/// themselves are read by [`parse_nav_profiles`]). The section is always present, so `offset`
/// at/past EOF, a `chunk_size` other than the pinned 512, a `profile_count` outside `1..=8`, or any
/// out-of-file region is a corrupt/old file ⇒ [`Error::BadOffset`] (distinct from the v8 file's
/// [`Error::BadVersion`]). Every offset/length product is checked (32-bit target).
pub(super) fn parse_nav_directory(
    src: &dyn ByteSource,
    scale: OffsetScale,
    offset: u64,
    total: u64,
) -> Result<NavDirectory, Error> {
    // The lowest byte a scaled offset in this file can name past the header (§1.2).
    let floor = scale.align_up(HEADER_LEN as u64).ok_or(Error::BadOffset)?;
    if offset < floor || offset.checked_add(NAV_DIR_LEN as u64).is_none_or(|end| end > total) {
        return Err(Error::BadOffset);
    }
    let mut d = [0u8; NAV_DIR_LEN];
    src.read_at(offset, &mut d).map_err(Error::Source)?;
    let dir = NavDirectory {
        index_offset: resolve(scale.offset(rd_u32(&d, 0))),
        node_count: rd_u32(&d, 4) as usize,
        chunk_count: rd_u32(&d, 8) as usize,
        edge_pool_offset: resolve(scale.offset(rd_u32(&d, 12))),
        edge_chunk_count: rd_u32(&d, 16) as usize,
        chunk_size: rd_u16(&d, 20) as usize,
        profile_table_offset: resolve(scale.offset(rd_u32(&d, 22))),
        profile_count: d[26] as usize,
        snap_index_offset: resolve(scale.offset(rd_u32(&d, 28))),
        snap_node_count: rd_u32(&d, 32) as usize,
        snap_chunk_count: rd_u32(&d, 36) as usize,
        scale,
    };
    // The nav chunk size is pinned to 512 (§8.1) — a v8 file, or any other value, is rejected. This
    // is a distinct error from the header's version check, so an old file and a mis-sized current
    // one are told apart.
    if dir.chunk_size != NAV_CHUNK_SIZE {
        return Err(Error::BadOffset);
    }
    // The profile table is always present with 1..=8 records (§8.6) — a zero or oversize count is a
    // malformed file, not a degraded one.
    if dir.profile_count == 0 || dir.profile_count > NAV_MAX_PROFILES {
        return Err(Error::BadOffset);
    }
    // Profile-table region (56 B × count) at `profile_table_offset` must lie in-file.
    if dir.profile_table_offset < floor {
        return Err(Error::BadOffset);
    }
    let profile_end = (dir.profile_count as u64)
        .checked_mul(NAV_PROFILE_LEN as u64)
        .and_then(|len| dir.profile_table_offset.checked_add(len))
        .ok_or(Error::BadOffset)?;
    if profile_end > total {
        return Err(Error::BadOffset);
    }
    // Node index + chunk region: like an empty POI category, an empty graph only needs its
    // (zero-length) offsets in-file; a populated one the whole region.
    if dir.node_count > 0 {
        let region_end = dir
            .data_start()
            .and_then(|start| {
                (dir.chunk_count as u64).checked_mul(dir.chunk_size as u64).and_then(|len| start.checked_add(len))
            })
            .ok_or(Error::BadOffset)?;
        if dir.index_offset < floor || region_end > total {
            return Err(Error::BadOffset);
        }
    } else if dir.index_offset > total {
        return Err(Error::BadOffset);
    }
    // Edge pool region. `Edge Chunk Count` is capped at `2^27` since v14 (§8.1/§8.4): past that
    // no `Edge Id` could name the chunks, so the tail would be bytes the directory claims and no
    // id reaches — the same posture the flat store takes toward an extent count its index cannot
    // name.
    if dir.edge_pool_offset < floor || dir.edge_chunk_count as u64 > NAV_EDGE_MAX_CHUNKS {
        return Err(Error::BadOffset);
    }
    let pool_end = (dir.edge_chunk_count as u64)
        .checked_mul(dir.chunk_size as u64)
        .and_then(|len| dir.edge_pool_offset.checked_add(len))
        .ok_or(Error::BadOffset)?;
    if pool_end > total {
        return Err(Error::BadOffset);
    }
    // v13 snap-anchor index + chunks. An empty anchor index is legal (a graph whose every edge is
    // short); a populated one follows the same fixed-chunk bounds contract as the node index.
    if dir.snap_node_count > 0 {
        let region_end = dir
            .snap_data_start()
            .and_then(|start| {
                (dir.snap_chunk_count as u64).checked_mul(dir.chunk_size as u64).and_then(|len| start.checked_add(len))
            })
            .ok_or(Error::BadOffset)?;
        if dir.snap_index_offset < floor || region_end > total {
            return Err(Error::BadOffset);
        }
    } else if dir.snap_index_offset > total || dir.snap_chunk_count != 0 {
        return Err(Error::BadOffset);
    }
    Ok(dir)
}

/// Parse the §8.6 profile table into `MapTables`: `dir.profile_count` (1..=8, already range-checked
/// by [`parse_nav_directory`]) consecutive 56-byte records at `dir.profile_table_offset`. Each
/// record's name field is `0xFF`-padded UTF-8; the two multiplier tables are copied verbatim except
/// that any **non-zero byte below 16 is clamped up to 16** — the admissibility invariant the packer
/// enforces, re-applied here so a hand-forged file can't hand N3 an inadmissible weight. A read
/// failure or out-of-file record is a corrupt file ⇒ [`Error::BadOffset`].
pub(super) fn parse_nav_profiles(
    src: &dyn ByteSource,
    dir: &NavDirectory,
) -> Result<heapless::Vec<MapProfile, NAV_MAX_PROFILES>, Error> {
    let mut out = heapless::Vec::new();
    let mut buf = [0u8; NAV_PROFILE_LEN];
    for i in 0..dir.profile_count {
        let off = dir
            .profile_table_offset
            .checked_add((i as u64).checked_mul(NAV_PROFILE_LEN as u64).ok_or(Error::BadOffset)?)
            .ok_or(Error::BadOffset)?;
        src.read_at(off, &mut buf).map_err(Error::Source)?;
        let mut name = [0u8; NAV_PROFILE_NAME_LEN];
        name.copy_from_slice(&buf[0..NAV_PROFILE_NAME_LEN]);
        // Name length = bytes up to the first 0xFF pad (the §7/§8 name convention).
        let name_len = name.iter().position(|&b| b == CHUNK_END).unwrap_or(NAV_PROFILE_NAME_LEN);
        let mut highway = [0u8; 32];
        highway.copy_from_slice(&buf[12..44]);
        let mut surface = [0u8; 8];
        surface.copy_from_slice(&buf[44..52]);
        for m in highway.iter_mut().chain(surface.iter_mut()) {
            if *m != 0 && *m < 16 {
                *m = 16; // clamp an inadmissible weight up to 1.0× (defensive; the packer forbids it)
            }
        }
        // The v12 climb weight needs no clamp: every `u8` is admissible because the term it feeds is
        // additive and non-negative (§8.6). The three reserved bytes behind it are ignored.
        let climb_weight = buf[NAV_PROFILE_CLIMB_WEIGHT_OFF];
        // `push` can't fail: the loop runs `profile_count ≤ NAV_MAX_PROFILES` times (checked above).
        let _ = out.push(MapProfile { name, name_len, highway, surface, climb_weight });
    }
    Ok(out)
}
