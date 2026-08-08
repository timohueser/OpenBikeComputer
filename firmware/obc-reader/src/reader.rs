//! OBCM **v13** format reader: header, style table, LOD table, per-LOD
//! quadtree query + chunk decode, the POI directory + hours-pool section, and
//! the trailing nav-graph section (parse + leaf-walk/record-decode only here —
//! the A* traversal over it is R3, #465).
//!
//! All coordinates are integer microdegrees (1e-6 degrees), as stored in the
//! file. Projection to screen space is the renderer's job.
//!
//! The reader **streams** through a [`ByteSource`]: only the small header / style
//! table / LOD table are resident (parsed once in [`MapTables`]); the quadtree
//! index and geometry chunks are pulled on demand via `read_at`, so the whole
//! `.obcm` never has to fit in RAM. A [`SliceSource`](crate::SliceSource) makes
//! "the whole file is resident" a one-line wrapper for the sim and tests.
//!
//! Because `read_at` takes `&self`, the lazy reads go through an internal
//! [`MapCache`] behind a `RefCell`: a geometry-chunk cache (the renderer walks the
//! visible chunks twice — pass A to select candidates, pass B to re-decode the
//! winners — so this keeps pass B's winner chunks resident and reuses chunks across
//! frames), a small block cache coalescing the 4-byte quadtree-node reads, and a
//! bounded expanded-view leaf cache that avoids repeating the walk during a slow
//! pan. The cache changes only *when* a byte is read, never *what* decodes, so
//! renders stay byte-identical.

use core::cell::{RefCell, RefMut};
use core::sync::atomic::{AtomicU32, Ordering};

use heapless::Vec;

use crate::corridor::{
    inflate_bbox, project_onto_chunk, CorridorPoi, PoiCategorySet, RoutePath, CORRIDOR_HALF_WIDTH_M,
    MAX_CORRIDOR_RESULTS,
};
use crate::Error;
use obc_formats::io::{rd_f32, rd_i16, rd_i32, rd_u16, rd_u32, ByteSource, Error as IoError};
use obc_formats::obcm::PoiCategory;
use obc_formats::obcm::{
    BRANCH_BIT, EMPTY_LEAF, FEATURE_FLAG_16BIT, FEATURE_FLAG_HOLES, FEATURE_FLAG_POLYGON, FEATURE_FLAG_WIDE,
    STYLE_DASHED_BIT, STYLE_FIXED_WIDTH_BIT, STYLE_HAS_COLOR2_BIT, STYLE_PRIORITY_MASK, STYLE_TERRAIN_LAYER_BIT,
};
use obc_formats::obcm::{
    CHUNK_END, FEATURE_HEADER_COMPACT_LEN, FEATURE_HEADER_WIDE_LEN, MAGIC, NAV_DIR_LEN, POI_CAT_ENTRY_LEN,
    POI_HOURS_REF_NONE, POI_RECORD_LEN, STYLE_RECORD_LEN, VERSION,
};
use obc_formats::obcm::{
    HEADER_LEN, LOD_ENTRY_LEN, NAV_CHUNK_SIZE, NAV_EDGE_FIXED_LEN, NAV_MAX_PROFILES, NAV_NEIGHBOR_ASCENT_OFF,
    NAV_NEIGHBOR_LEN, NAV_NODE_FIXED_LEN, NAV_PROFILE_CLIMB_WEIGHT_OFF, NAV_PROFILE_LEN, NAV_PROFILE_NAME_LEN,
    NAV_SNAP_RECORD_LEN, POI_HOURS_BLOB_LEN, POI_NAME_LEN,
};
use obc_map_scene::{cos_lat, ground_dist_m_cl, BBox, Kind, Style, StyleFlags, M_PER_DEG};

/// Upper bound on the vertices of a single decoded feature — the capacity a caller
/// sizes the `points` scratch buffer to for [`Reader::for_each_feature`].
pub const MAX_FEAT_PTS: usize = 2048;
/// Upper bound on the rings (exterior + holes) of a single decoded feature — the
/// capacity for the `ring_lens` scratch buffer of [`Reader::for_each_feature`].
pub const MAX_FEAT_RINGS: usize = 32;

/// POI directory categories in v7 (spec §7.1): category ids `1..=6`. The parsed [`MapTables::pois`]
/// bounds its `heapless::Vec` at this so a corrupt `category_count` can't request an unbounded
/// allocation; a directory declaring more categories than this is rejected.
pub const POI_MAX_CATEGORIES: usize = 8;

/// Upper bound on the POI `chunk_size` the reader accepts (spec §7.1). POI records are a fixed 32
/// bytes and the packer writes 512-byte chunks (16 records); this caps the on-wire `u16` well below
/// the geometry [`MAX_CHUNK_BYTES`] so a corrupt directory can't advertise a huge chunk the
/// nearest-N query (#424) would try to buffer. Generous headroom over the packer's 512 without
/// approaching the geometry scratch.
pub const POI_MAX_CHUNK_BYTES: usize = 4096;

/// Max results the nearest-N POI query returns (locked on epic #115). The caller owns a
/// `heapless::Vec<Poi, MAX_POI_RESULTS>`; the query fills it ascending by distance and never
/// exceeds it. 16 × ≈36 B ≈ 600 B, on the caller's stack.
pub const MAX_POI_RESULTS: usize = 16;

/// Initial half-extent of the POI search bbox, in latitude µdeg (~2 km: `2000 / 111.32e-3 m/µdeg ≈
/// 17 966`, rounded up). Doubled each pass until the nearest-16 are provably found (see
/// [`Reader::nearest_pois`]). The longitude half-extent is this scaled by `1/cos_lat`.
const POI_SEARCH_HALF_UDEG: i32 = 18_000;

/// The POI-scan stack scratch window, in bytes (spec §7.1's default chunk size, 14 records of 36 =
/// 504 bytes, plus a few slack bytes). One chunk streams through this fixed window at a time
/// regardless of the accepted `chunk_size`, so the query's scratch stays tiny (no `MapCache`
/// growth). Each read pulls a whole number of records (`take * POI_RECORD_LEN`), so a record never
/// straddles two reads.
const POI_SCAN_WINDOW: usize = 512;

/// Upper bound on the nav `chunk_size` the reader accepts (spec §8.1), and the byte size of one
/// [`NavTileCache`] slot. v9 pins the wire value to exactly [`NAV_CHUNK_SIZE`] (512), so this equals
/// it: the cache holds whole 512 B chunks and [`NavTileCache::chunk`]'s `debug_assert` guards that
/// no larger chunk is ever routed through a slot.
pub const NAV_MAX_CHUNK_BYTES: usize = NAV_CHUNK_SIZE;

/// The per-read window of [`Reader::nav_edge`]'s delta stream, bytes (a multiple of the 4-byte
/// delta pair, so a pair never straddles two reads). Edge polylines are fetched once per route
/// emit, so a small fixed stack window is plenty.
const NAV_EDGE_WINDOW: usize = 128;

/// Upper bound on a single map data chunk, in bytes — the size of the decode scratch, and the
/// largest `chunk_size` the reader accepts ([`MapTables::parse`] rejects a bigger one). The
/// format stores `chunk_size` as a `u16` (≤ 65535) but real maps pack far smaller (the packer
/// defaults to 4096), so this caps the scratch below the format ceiling to save RAM. A chunk
/// between a cache slot and this decodes through the scratch, uncached.
///
/// This is an **acceptance** bound, not just a buffer size: shrinking it makes the reader reject
/// deliberately large chunks the round-trip suite packs (obc-pack's
/// `max_feat_pts_boundary_survives` puts two features, one at `MAX_FEAT_PTS`, into one 8192-byte
/// chunk), and device and host share the one profile — so a map that packs, loads.
pub const MAX_CHUNK_BYTES: usize = 16384;

/// Size of one geometry-chunk **cache** slot. A chunk this size or smaller is cached (kept
/// resident across the frame's two collect passes); a larger one — up to [`MAX_CHUNK_BYTES`] — is
/// decoded through the scratch without being cached. Matches the packer's default `chunk_size`
/// (4096) so the maps the device actually loads are fully cacheable.
const CACHE_SLOT_BYTES: usize = 4096;

/// Dedicated geometry-chunk buffers (each [`CACHE_SLOT_BYTES`]). The first 4 KiB of the oversized
/// decode scratch doubles as a fifth slot, so the common four/five-chunk riding views remain fully
/// resident without increasing [`MapCache`]. Replacement is scan-resistant RRIP: a view just over
/// capacity retains protected chunks instead of cyclically missing every one. Coarse overview
/// zooms can expose dozens of chunks and remain intentionally streaming on the LM20 RAM budget.
pub(crate) const MAP_CHUNK_SLOTS: usize = 4;

/// Block size + count of the quadtree-index cache. The leaf walk reads 4-byte nodes (siblings
/// adjacent in the file); caching a few aligned blocks coalesces those into a handful of SD
/// reads per walk rather than one read per node. ≈3.5 KB total. Its replacement is scan-resistant
/// RRIP rather than LRU: the renderer repeats an ordered walk whose working set can exceed these
/// seven slots, and LRU otherwise evicts every block just before the next frame asks for it. The
/// eighth former block's 520-byte budget holds the expanded-view leaf cache below.
const INDEX_BLOCK: usize = 512;
const INDEX_BLOCKS: usize = 7;

/// Two recent geometry walks (normally the one or two volume shards touching the viewport), each
/// retaining up to twelve `(chunk, leaf bbox)` results over a view widened by 1/8 on every side.
/// A moving camera can then reuse the list until it crosses that margin without reading the
/// quadtree again. Two 260-byte records exactly replace the eighth tagged index block: no net RAM.
const WALK_CACHE_SLOTS: usize = 2;
const WALK_CACHE_ENTRIES: usize = 12;

// A slot must fit any chunk it caches, and the scratch any chunk the reader accepts; `chunk_size`
// is a `u16`, so the accepted cap stays within range.
const _: () = assert!(CACHE_SLOT_BYTES <= MAX_CHUNK_BYTES, "a cached chunk must fit the scratch");
const _: () = assert!(MAX_CHUNK_BYTES <= u16::MAX as usize, "chunk_size is a u16 in the format");

/// Hard cap on quadtree recursion depth in [`Reader::walk_leaves`]. A well-formed tree is far
/// shallower (the node bbox halves each level, bottoming out at the coordinate bit-width ≤32), so
/// this never rejects a real map. It matters for a *corrupt* one: once the node bbox subdivides to
/// a degenerate point the quadrants stop shrinking while `intersects(view)` stays true, so an
/// unbounded walk recurses forever → stack overflow → HardFault (no MMU guard page on the MCU).
const MAX_QUADTREE_DEPTH: u32 = 32;

/// A flat `uint32` quadtree index over the header's global bbox — the layout shared by a geometry
/// [`Lod`] and a POI category ([`PoiCatEntry`]), per spec §4/§7.2. The leaf walk needs only where
/// the index starts and how many nodes it holds; [`Reader::walk_leaves`] is generic over this so
/// the geometry `for_each_chunk` and the POI query drive one implementation (continuing the
/// packer's shared `FlattenTree` DRY).
trait QuadIndex {
    /// Byte offset of node 0 in the file.
    fn index_offset(&self) -> usize;
    /// Number of `uint32` nodes in the index; `0` ⇒ empty (no walk).
    fn node_count(&self) -> usize;
}

impl QuadIndex for Lod {
    #[inline]
    fn index_offset(&self) -> usize {
        self.index_offset
    }
    #[inline]
    fn node_count(&self) -> usize {
        self.node_count
    }
}

impl QuadIndex for PoiCatEntry {
    #[inline]
    fn index_offset(&self) -> usize {
        self.index_offset
    }
    #[inline]
    fn node_count(&self) -> usize {
        self.node_count
    }
}

/// Which caller-owned feature scratch bound rejected a complete encoded feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityError {
    Points,
    Rings,
}

/// Why a feature was consumed but not published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureDecodeError {
    Capacity(CapacityError),
    Malformed,
}

/// A single-feature refetch failure, retaining decode/capacity vs. source/cache identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureReadError {
    Decode(FeatureDecodeError),
    Read(MapReadError),
}

/// A cache access failed without panicking through the safe reader API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheError {
    Busy,
}

/// Failures while streaming a map index or geometry chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapReadError {
    Source(IoError),
    Cache(CacheError),
    Malformed,
}

impl From<MapReadError> for Error {
    fn from(error: MapReadError) -> Self {
        match error {
            MapReadError::Source(error) => Error::Source(error),
            MapReadError::Cache(CacheError::Busy) => Error::CacheBusy,
            MapReadError::Malformed => Error::BadOffset,
        }
    }
}

/// Outcome of a feature-chunk walk. Failed features are consumed whole and never visited.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecodeStatus {
    pub complete: u32,
    pub capacity_dropped: u32,
    pub malformed: u32,
}

impl DecodeStatus {
    #[inline]
    fn dropped(&mut self, error: FeatureDecodeError) {
        match error {
            FeatureDecodeError::Capacity(_) => self.capacity_dropped = self.capacity_dropped.saturating_add(1),
            FeatureDecodeError::Malformed => self.malformed = self.malformed.saturating_add(1),
        }
    }
}

/// One level of the LOD pyramid: a self-contained quadtree index + chunk set.
#[derive(Debug, Clone, Copy)]
pub struct Lod {
    /// Upper bound of the meters-per-pixel range this level covers; the coarsest
    /// level is `f32::INFINITY`. Strictly decreasing from coarse (0) to fine.
    pub max_mpp: f32,
    pub index_offset: usize,
    pub node_count: usize,
    /// The **capacity bound** on one chunk (spec §3): the packer's leaf-split threshold and the
    /// largest length any single chunk may have. Not a stride — chunks are packed tight and
    /// addressed through the offset table.
    pub chunk_size: usize,
    pub chunk_count: usize,
    /// Total bytes of this level's chunk-data region — `offsets[chunk_count]`, the last entry of
    /// the offset table, read once in [`parse_lod_table`]. Resident so a per-chunk fetch can
    /// bound its offset pair without a second read.
    pub chunk_bytes_total: usize,
}

/// The convention every quadtree-indexed section shares (§3/§4): what follows a `node_count`-node
/// index begins right behind it, at `index_offset + node_count * 4`. `None` on `usize` overflow —
/// reachable on the 32-bit MCU from a corrupt `index_offset`/`node_count`. What "what follows"
/// *is* differs per section (a LOD's offset table, a POI category's or the nav graph's chunks),
/// which is why the callers below name it and this doesn't.
#[inline]
fn index_end(index_offset: usize, node_count: usize) -> Option<usize> {
    node_count.checked_mul(4)?.checked_add(index_offset)
}

/// Byte range `[start, end)` of chunk `chunk_id` in a section whose chunks are a **fixed**
/// `chunk_size` apart from `data_start` (the POI §7.1 and nav §8.1 sections; LOD chunk data is
/// packed tight behind an offset table instead). `None` if `chunk_id` is out of range or any offset
/// overflows `usize` — `chunk_id` comes straight from a quadtree leaf, so it is arbitrary in a
/// corrupt map and is validated against `chunk_count` with checked arithmetic.
#[inline]
fn fixed_chunk_range(
    data_start: Option<usize>,
    chunk_count: usize,
    chunk_size: usize,
    chunk_id: u32,
) -> Option<(usize, usize)> {
    let id = chunk_id as usize;
    if id >= chunk_count {
        return None;
    }
    let start = id.checked_mul(chunk_size)?.checked_add(data_start?)?;
    let end = start.checked_add(chunk_size)?;
    Some((start, end))
}

impl Lod {
    /// Byte offset of the LOD's per-chunk **offset table** (spec §5): `chunk_count + 1` `uint32`
    /// entries sitting between the quadtree index and the chunk data. `None` on `usize` overflow —
    /// reachable on the 32-bit MCU from a corrupt `index_offset`/`node_count`.
    #[inline]
    fn offset_table(&self) -> Option<usize> {
        index_end(self.index_offset, self.node_count)
    }

    /// Byte offset where this level's chunk **data** begins: after the index *and* the offset
    /// table. `None` on `usize` overflow (see [`Lod::offset_table`]).
    #[inline]
    fn data_start(&self) -> Option<usize> {
        let table_len = self.chunk_count.checked_add(1)?.checked_mul(4)?;
        self.offset_table()?.checked_add(table_len)
    }

    /// Byte offset of chunk `chunk_id`'s entry in the offset table, or `None` if `chunk_id` is out
    /// of range or the arithmetic overflows `usize`. `chunk_id` comes straight from a quadtree leaf
    /// (arbitrary in a corrupt map), so it is validated against `chunk_count` with checked
    /// arithmetic. Entries `k` and `k+1` are adjacent, which is what makes a chunk extent **one**
    /// 8-byte read ([`Reader::chunk_range`]).
    #[inline]
    fn offset_entry(&self, chunk_id: u32) -> Option<usize> {
        let id = chunk_id as usize;
        if id >= self.chunk_count {
            return None;
        }
        self.offset_table()?.checked_add(id.checked_mul(4)?)
    }
}

/// One category's entry in the parsed POI directory (spec §7.1). Parse-only in v6:
/// the nearest-N query (#424) walks this category's quadtree exactly as it walks a
/// [`Lod`] index — the layout is shared, so its `data_start`/`chunk_range` math
/// reuses the same convention.
#[derive(Debug, Clone, Copy)]
pub struct PoiCatEntry {
    /// Canonical category id (1..=6; spec §7.4).
    pub category_id: u8,
    /// Byte offset to this category's quadtree index.
    pub index_offset: usize,
    /// Number of `uint32` nodes in the index; `0` ⇒ the category is empty in this map.
    pub node_count: usize,
    /// Number of POI data chunks in this category.
    pub chunk_count: usize,
}

impl PoiCatEntry {
    /// This category is empty in this map (no quadtree, no chunks).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.node_count == 0
    }

    /// Byte offset where this category's data chunks begin (right after its index),
    /// or `None` if the arithmetic overflows `usize` (a corrupt directory on the
    /// 32-bit MCU) — the shared §7.1 convention, see [`index_end`].
    #[inline]
    pub fn data_start(&self) -> Option<usize> {
        index_end(self.index_offset, self.node_count)
    }

    /// Byte range `[start, end)` of POI chunk `chunk_id` given the directory's shared `chunk_size`
    /// (the §7.1 chunk size is directory-wide, not per-entry, so it's passed in). See
    /// [`fixed_chunk_range`].
    #[inline]
    fn chunk_range(&self, chunk_id: u32, chunk_size: usize) -> Option<(usize, usize)> {
        fixed_chunk_range(self.data_start(), self.chunk_count, chunk_size, chunk_id)
    }
}

/// The parsed nav directory (spec §8.1) — the graph's **entire resident state** (the quadtree and
/// every record stream on demand). Empty graph (`node_count == 0`) ⇒ no walk, exactly like an
/// empty POI category. Parse-only in R2: [`Reader::for_each_nav_node`] walks the node quadtree and
/// [`Reader::nav_edge`] fetches one polyline; the A* over them is R3 (#465).
#[derive(Debug, Clone, Copy)]
pub struct NavDirectory {
    /// Byte offset to the node quadtree index (§8.2 — the §4 encoding over the global bbox).
    pub index_offset: usize,
    /// Number of `uint32` nodes in the index; `0` ⇒ the map has no routable graph.
    pub node_count: usize,
    /// Number of node data chunks (they begin at `index_offset + node_count * 4`, the §3/§4
    /// convention).
    pub chunk_count: usize,
    /// Byte offset of the edge pool (§8.4).
    pub edge_pool_offset: usize,
    /// Number of `chunk_size`-byte chunks in the edge pool.
    pub edge_chunk_count: usize,
    /// Fixed capacity (bytes) of every nav chunk — node chunks and edge-pool chunks alike. v9 pins
    /// this to [`NAV_CHUNK_SIZE`] (512); [`parse_nav_directory`] rejects any other value.
    pub chunk_size: usize,
    /// Absolute byte offset of the §8.6 profile table (written immediately after this directory).
    pub profile_table_offset: usize,
    /// Number of 56-byte profile records at `profile_table_offset` (1..=8; parse rejects otherwise).
    pub profile_count: usize,
    /// Byte offset to the v13 §8.7 snap-anchor quadtree index.
    pub snap_index_offset: usize,
    /// Number of `uint32` nodes in the snap-anchor quadtree; `0` means no interior anchors.
    pub snap_node_count: usize,
    /// Number of fixed-size snap-anchor data chunks following that index.
    pub snap_chunk_count: usize,
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
    };

    /// The map carries no routable graph (no quadtree, no chunks, no edges).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.node_count == 0
    }

    /// Byte offset where the node data chunks begin (right after the index), or `None` on
    /// `usize` overflow (a corrupt directory on the 32-bit MCU) — see [`index_end`].
    #[inline]
    pub fn data_start(&self) -> Option<usize> {
        index_end(self.index_offset, self.node_count)
    }

    /// Byte range `[start, end)` of node chunk `chunk_id`, or `None` if out of range / on
    /// overflow. See [`fixed_chunk_range`]; the nav chunk size is directory-wide (§8.1).
    #[inline]
    fn chunk_range(&self, chunk_id: u32) -> Option<(usize, usize)> {
        fixed_chunk_range(self.data_start(), self.chunk_count, self.chunk_size, chunk_id)
    }

    /// Byte offset where v13's snap-anchor chunks begin (right after their quadtree index).
    #[inline]
    pub fn snap_data_start(&self) -> Option<usize> {
        index_end(self.snap_index_offset, self.snap_node_count)
    }

    /// Byte range of one v13 snap-anchor chunk.
    #[inline]
    fn snap_chunk_range(&self, chunk_id: u32) -> Option<(usize, usize)> {
        fixed_chunk_range(self.snap_data_start(), self.snap_chunk_count, self.chunk_size, chunk_id)
    }
}

impl QuadIndex for NavDirectory {
    #[inline]
    fn index_offset(&self) -> usize {
        self.index_offset
    }
    #[inline]
    fn node_count(&self) -> usize {
        self.node_count
    }
}

#[derive(Clone, Copy)]
struct NavSnapIndex {
    index_offset: usize,
    node_count: usize,
}

impl QuadIndex for NavSnapIndex {
    #[inline]
    fn index_offset(&self) -> usize {
        self.index_offset
    }
    #[inline]
    fn node_count(&self) -> usize {
        self.node_count
    }
}

/// One §8.6 routing profile resident in [`MapTables`]: a display name plus the two multiplier
/// tables (`u8` fixed-point 1/16, indexed by way-kind's highway class 0..=31 / surface class
/// 0..=7; `16` = 1.0×, `0` = forbidden). N3 selects one by index and weights each edge by
/// [`MapProfile::multiplier`]. Parsed by [`parse_nav_profiles`], which clamps any non-zero byte
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
        self.neighbors.chunks_exact(NAV_NEIGHBOR_LEN).map(move |e| NavNeighbor {
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

/// Graph-tile cache slots. **Thirty-two**: the earlier 8-slot measurement covered one route, while
/// the 2026-08-08 physical-command study covered Grimsel, Monaco and failure/escalation paths. The
/// larger frontier working set remained useful through 32 slots, cutting node-chunk misses by
/// roughly 1.5–2.5× over 8 depending on density. The cache lives in the route-only scratch-arena arm,
/// which had ~69 KiB below the 128 KiB USB maximum, so this growth costs zero linked resident RAM.
/// Fully-associative round-robin is retained: 32 tag compares are negligible beside a card command
/// and preserve the measured hit rate without conflict misses.
pub(crate) const NAV_TILE_SLOTS: usize = 32;

/// Route-private aligned quadtree-index windows. Real nav indexes are about 8 KiB; the render cache's
/// seven windows repeatedly scanned and thrashed because every settled node re-descends the tree.
/// Sixteen scan-resistant RRIP windows keep that working set inside the route-only arena arm and
/// leave the renderer's carefully-budgeted seven-window cache untouched.
const NAV_INDEX_BLOCKS: usize = 16;

/// Empty-slot tag for [`NavTileCache`]: a chunk's absolute file offset never reaches `u32::MAX`
/// (its whole extent must lie inside a `u32`-addressed source).
const NAV_TILE_EMPTY: u32 = u32::MAX;

/// A snapshot of the [`NavTileCache`] counters. These are **logical** `ByteSource::read_at` counts;
/// physical command counts are lower-level transport diagnostics. Sector-aligned current producers make
/// every full node/edge/index fill one physical command, while old unaligned maps may need two.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NavCacheStats {
    /// Nav-chunk requests served from a resident slot (no SD read).
    pub hits: u32,
    /// Nav-chunk requests that missed and read `chunk_size` bytes from the source.
    pub misses: u32,
    /// Quadtree-index node reads served by one of the route-private aligned windows.
    pub index_hits: u32,
    /// Route-private index windows filled from the source.
    pub index_misses: u32,
}

impl NavCacheStats {
    /// Total logical source fills attributable to route traversal. This is the scheduler's expensive
    /// unit: graph chunks plus index windows, never resident hits.
    #[inline]
    pub const fn source_reads(self) -> u32 {
        self.misses.saturating_add(self.index_misses)
    }
}

/// A tiny caller-owned cache of whole nav chunks (node **and** edge-pool — both are `chunk_size`
/// ≤ [`NAV_MAX_CHUNK_BYTES`] bytes, spec §8.1), keyed by the chunk's absolute file offset so the
/// two chunk spaces can't collide. [`Reader::for_each_nav_node_cached`] and
/// [`Reader::nav_edge_oriented`] stream through it so the router's per-settle spatial re-fetch
/// doesn't re-read the same leaf from the SD (epic #116's named risk). Round-robin eviction: across
/// the [`NAV_TILE_SLOTS`] slots the measured hit rate matches LRU's within noise (the frontier's
/// live-leaf set has no strong recency skew), so the cheaper cursor is kept.
///
/// ~25 KB, owned by the caller (the device puts it in the route-only scratch-arena arm); `new()` is
/// `const` so a static/arena initialization stays deterministic. The tags are only meaningful
/// against one map/source — the router resets it per plan, so a map switch cannot cross-serve stale
/// graph or index bytes.
pub struct NavTileCache {
    slots: [[u8; NAV_MAX_CHUNK_BYTES]; NAV_TILE_SLOTS],
    /// Absolute file offset of the chunk each slot holds, or [`NAV_TILE_EMPTY`].
    tags: [u32; NAV_TILE_SLOTS],
    /// Round-robin eviction cursor.
    next: u8,
    hits: u32,
    misses: u32,
    index: [IndexBlock; NAV_INDEX_BLOCKS],
    index_hits: u32,
    index_misses: u32,
}

// On-device: 32 graph sectors + tags/counters and sixteen 520-byte index windows.
#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<NavTileCache>() == 24_852);

impl NavTileCache {
    pub const fn new() -> Self {
        NavTileCache {
            slots: [[0; NAV_MAX_CHUNK_BYTES]; NAV_TILE_SLOTS],
            tags: [NAV_TILE_EMPTY; NAV_TILE_SLOTS],
            next: 0,
            hits: 0,
            misses: 0,
            index: [IndexBlock::EMPTY; NAV_INDEX_BLOCKS],
            index_hits: 0,
            index_misses: 0,
        }
    }

    /// Invalidate every slot and zero the counters — call before a fresh route computation (or
    /// after a map switch) so stale tags can't serve another file's bytes and the counters read
    /// as "this run's I/O".
    pub fn reset(&mut self) {
        self.tags = [NAV_TILE_EMPTY; NAV_TILE_SLOTS];
        self.next = 0;
        self.hits = 0;
        self.misses = 0;
        for block in &mut self.index {
            block.meta = 0;
        }
        self.index_hits = 0;
        self.index_misses = 0;
    }

    /// Snapshot of the hit/miss counters since the last [`NavTileCache::reset`].
    #[inline]
    pub fn stats(&self) -> NavCacheStats {
        NavCacheStats {
            hits: self.hits,
            misses: self.misses,
            index_hits: self.index_hits,
            index_misses: self.index_misses,
        }
    }

    /// The `len`-byte chunk at absolute `offset`, from a resident slot or (on miss) read from
    /// `src` into the round-robin victim. `None` on a read failure — the victim's tag is cleared
    /// *before* the read so a short/failed fill can never leave a stale tag over garbage bytes.
    fn chunk(&mut self, src: &dyn ByteSource, offset: u32, len: usize) -> Option<&[u8]> {
        debug_assert!(len <= NAV_MAX_CHUNK_BYTES);
        for i in 0..NAV_TILE_SLOTS {
            if self.tags[i] == offset {
                self.hits += 1;
                return Some(&self.slots[i][..len]);
            }
        }
        let i = self.next as usize % NAV_TILE_SLOTS;
        self.tags[i] = NAV_TILE_EMPTY;
        src.read_at(offset, &mut self.slots[i][..len]).ok()?;
        self.tags[i] = offset;
        self.next = self.next.wrapping_add(1);
        self.misses += 1;
        Some(&self.slots[i][..len])
    }

    /// Read one quadtree node through the route-private aligned index working set.
    fn index_node(
        &mut self,
        src: &dyn ByteSource,
        file: u8,
        index: &dyn QuadIndex,
        idx: usize,
    ) -> Result<u32, IoError> {
        let byte_index = idx.checked_mul(4).ok_or(IoError::BadOffset)?;
        let off = u32::try_from(index.index_offset().checked_add(byte_index).ok_or(IoError::BadOffset)?)
            .map_err(|_| IoError::BadOffset)?;
        let mut word = [0u8; 4];
        self.index_read(src, file, off, &mut word)?;
        Ok(u32::from_le_bytes(word))
    }

    fn index_read(&mut self, src: &dyn ByteSource, file: u8, off: u32, out: &mut [u8]) -> Result<(), IoError> {
        let mut filled = 0usize;
        while filled < out.len() {
            let cur = off.checked_add(filled as u32).ok_or(IoError::BadOffset)?;
            let block_off = cur - cur % INDEX_BLOCK as u32;
            let slot = self.index_block(src, file, block_off)?;
            let within = (cur - block_off) as usize;
            let blen = self.index[slot].len as usize;
            if within >= blen {
                return Err(IoError::BadOffset);
            }
            let take = (blen - within).min(out.len() - filled);
            out[filled..filled + take].copy_from_slice(&self.index[slot].buf[within..within + take]);
            filled += take;
        }
        Ok(())
    }

    fn index_block(&mut self, src: &dyn ByteSource, file: u8, block_off: u32) -> Result<usize, IoError> {
        if let Some(i) = self.index.iter().position(|b| b.valid() && b.file() == file && b.off == block_off) {
            self.index[i].set_rrpv(0);
            self.index_hits = self.index_hits.saturating_add(1);
            return Ok(i);
        }
        let remaining = src.len().checked_sub(block_off).ok_or(IoError::BadOffset)? as usize;
        let want = remaining.min(INDEX_BLOCK);
        if want == 0 {
            return Err(IoError::BadOffset);
        }
        let empty = self.index.iter().position(|b| !b.valid());
        let i = empty.unwrap_or_else(|| rrip_victim(&mut self.index));
        self.index[i].meta = 0;
        src.read_at(block_off, &mut self.index[i].buf[..want])?;
        self.index[i].off = block_off;
        self.index[i].len = want as u16;
        self.index_misses = self.index_misses.saturating_add(1);
        let rrpv = if empty.is_some() || self.index_misses.is_multiple_of(8) { 2 } else { 3 };
        self.index[i].commit(file, rrpv);
        Ok(i)
    }
}

impl Default for NavTileCache {
    fn default() -> Self {
        Self::new()
    }
}

/// A single POI result from [`Reader::nearest_pois`]. Coordinates are absolute microdegrees (§7.3);
/// `distance_m` is the ground distance from the query position, computed during the scan. `name` is
/// empty for an unnamed POI — the app then shows the subtype's fallback label
/// ([`poi_label_of`](obc_formats::obcm::poi_label_of)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Poi {
    pub lat: i32,
    pub lon: i32,
    /// Canonical subtype id (§7.4), always in `1..=18` for a returned POI.
    pub subtype: u8,
    /// Stored name (≤ [`POI_NAME_LEN`] bytes); empty ⇒ unnamed.
    pub name: heapless::String<POI_NAME_LEN>,
    /// 0-based index into the hours pool (§7.5), decoded from record bytes `[34..36]`; `0xFFFF` = no
    /// hours. Carried into the detail screen (#444) so it can resolve the schedule via
    /// [`Reader::poi_hours`] without re-running the query.
    pub hours_ref: u16,
    /// Ground distance from the query position, rounded to whole meters.
    pub distance_m: u32,
}

/// The parsed POI directory (spec §7.1): the shared chunk size, one bounded entry per category, and
/// (v7) the hours-pool section's absolute offset + blob count. [`MapTables::parse`] fills it; the
/// nearest-N query walks each `entries[i]`'s quadtree, and the hours fields locate the pool for the
/// P3 (#443) per-POI hours lookup + open-now evaluation — parse-only here, the pool bytes are just
/// bounds-validated to lie in-file.
#[derive(Debug, Clone)]
pub struct PoiDirectory {
    /// Fixed capacity (bytes) of every POI chunk, shared by all categories (spec §7.1).
    pub chunk_size: usize,
    /// One entry per category present in the directory (bounded at [`POI_MAX_CATEGORIES`]).
    pub entries: Vec<PoiCatEntry, POI_MAX_CATEGORIES>,
    /// Absolute byte offset of the hours-pool section (spec §7.5): a `count u16` then `count ×
    /// 29-byte` blobs. Blob `i` (a record's `hours_ref`) lives at `hours_pool_offset + 2 + i*29`.
    /// Meaningful only when `hours_pool_count > 0`.
    pub hours_pool_offset: usize,
    /// Number of 29-byte blobs in the hours pool (spec §7.5); `0` ⇒ no hours in this map. Equals the
    /// `count u16` written at `hours_pool_offset`, validated equal at parse.
    pub hours_pool_count: usize,
}

/// The one shared empty POI directory a shard reader hands out — a `static` rather than a
/// promoted temporary because [`PoiDirectory`] holds a `heapless::Vec` and does not const-promote.
static EMPTY_POI_DIRECTORY: PoiDirectory = PoiDirectory::EMPTY;
/// The nav twin of [`EMPTY_POI_DIRECTORY`], kept a `static` for symmetry with it.
static EMPTY_NAV_DIRECTORY: NavDirectory = NavDirectory::EMPTY;

impl PoiDirectory {
    /// The directory a reader with no POI section of its own reports — no categories, no chunks,
    /// no hours pool. The POI twin of [`NavDirectory::EMPTY`], and what a **volume-set shard**
    /// reader answers (`OBCA_Spec` §5.1: POIs live in the core file alone).
    pub const EMPTY: PoiDirectory =
        PoiDirectory { chunk_size: 0, entries: Vec::new(), hours_pool_offset: 0, hours_pool_count: 0 };
}

/// A feature decoded into caller-owned scratch buffers, borrowed for one
/// [`Reader::for_each_feature`] callback. No per-feature allocation: `points`
/// holds every ring's vertices concatenated, `ring_lens[0]` is the exterior
/// length, the rest are holes. Coordinates are microdegrees.
#[derive(Debug, Clone, Copy)]
pub struct FeatureRef<'a> {
    pub style_id: u8,
    pub kind: Kind,
    points: &'a [(i32, i32)],
    ring_lens: &'a [usize],
    bbox: BBox,
    /// Byte offset of this feature's header within its chunk — what [`Reader::decode_feature_at`]
    /// needs to re-decode exactly this feature in a later pass without re-walking the whole chunk
    /// (the renderer's two-phase collect, issue #564). Always `< chunk_size ≤ MAX_CHUNK_BYTES`.
    offset: usize,
}

impl<'a> FeatureRef<'a> {
    /// Axis-aligned bounds (microdegrees) of every vertex, computed during decode.
    /// Empty for a zero-vertex feature.
    #[inline]
    pub fn bbox(&self) -> BBox {
        self.bbox
    }

    /// Byte offset of this feature's header within its chunk (see the field). Hand this back to
    /// [`Reader::decode_feature_at`] to re-decode just this feature.
    #[inline]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// The exterior ring's vertices.
    #[inline]
    pub fn exterior(&self) -> &'a [(i32, i32)] {
        let n = self.ring_lens.first().copied().unwrap_or(0);
        &self.points[..n]
    }

    /// Iterator over the interior (hole) rings, if any.
    #[inline]
    pub fn interiors(&self) -> Interiors<'a> {
        let start = self.ring_lens.first().copied().unwrap_or(0);
        let rest = if self.ring_lens.is_empty() { &[][..] } else { &self.ring_lens[1..] };
        Interiors { points: self.points, lens: rest, offset: start }
    }

    /// All rings' vertices, concatenated (exterior first); partition with [`FeatureRef::ring_lens`].
    #[inline]
    pub fn points(&self) -> &'a [(i32, i32)] {
        self.points
    }

    /// Per-ring vertex counts: `[0]` exterior, `[1..]` holes.
    #[inline]
    pub fn ring_lens(&self) -> &'a [usize] {
        self.ring_lens
    }
}

/// Iterator over a feature's hole rings (see [`FeatureRef::interiors`]).
pub struct Interiors<'a> {
    points: &'a [(i32, i32)],
    lens: &'a [usize],
    offset: usize,
}

impl<'a> Iterator for Interiors<'a> {
    type Item = &'a [(i32, i32)];
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let (&len, rest) = self.lens.split_first()?;
        self.lens = rest;
        let s = self.offset;
        self.offset += len;
        Some(&self.points[s..s + len])
    }
}

/// The OBCM header fields that describe a map without touching any geometry — the "which map is
/// this?" prefix every parse starts from, decoded before a single byte of style table, index or
/// chunk is read. [`MapTables::parse`] carries on into the full tables; the volume-set
/// [`ShardTables::parse`](crate::volume::ShardTables::parse) stops just past it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MapHeader {
    pub version: u8,
    pub bbox: BBox,
    /// User-position marker color (RGB565); see [`Reader::marker_color`].
    pub marker_color: u16,
}

/// Decode + validate the fixed 40-byte OBCM header (magic, version, bbox, marker color).
/// Shared by [`MapTables::parse`] and the volume-set shard parse so the byte layout lives in one
/// place. Offsets follow `obc-pack`'s header pack (see OBCM_Spec.md).
pub(crate) fn parse_header(h: &[u8; HEADER_LEN]) -> Result<MapHeader, Error> {
    if h[0..4] != MAGIC {
        return Err(Error::BadMagic);
    }
    let version = h[4];
    if version != VERSION {
        return Err(Error::BadVersion);
    }
    // Header field order: lat,lon,lat,lon (see `obc-pack`'s `serialize.rs` header pack).
    let min_lat = rd_i32(h, 5);
    let min_lon = rd_i32(h, 9);
    let max_lat = rd_i32(h, 13);
    let max_lon = rd_i32(h, 17);
    let marker_color = rd_u16(h, 30);
    Ok(MapHeader { version, bbox: BBox { min_lon, min_lat, max_lon, max_lat }, marker_color })
}

/// The prefix **every** OBCM parse begins with, decoded and bounds-checked once: the fixed 40-byte
/// header plus the LOD table's position. [`MapTables::parse`] goes on to the style table and the
/// POI/nav sections (whose offsets it reads straight out of the retained `header` bytes); a
/// volume-set shard ([`ShardTables::parse`](crate::volume::ShardTables::parse)) has none of those
/// and stops here.
pub(crate) struct HeaderPrologue {
    /// The raw 40 header bytes, kept so a caller can decode the fields the prologue doesn't own.
    pub header: [u8; HEADER_LEN],
    pub map: MapHeader,
    pub lod_count: usize,
    pub lod_table_offset: usize,
    /// The file's length, already read from the source — every offset check above used it.
    pub total: usize,
}

/// Read + validate the shared prologue from `src`. A file shorter than the header, with the wrong
/// magic / version, or with a LOD table that is empty, wraps `usize`, or runs past EOF is rejected
/// here, so neither parse has to restate the check.
pub(crate) fn parse_prologue(src: &dyn ByteSource) -> Result<HeaderPrologue, Error> {
    let total = src.len() as usize;
    if total < HEADER_LEN {
        return Err(Error::TooShort);
    }
    let mut header = [0u8; HEADER_LEN];
    src.read_at(0, &mut header).map_err(Error::Source)?;
    let map = parse_header(&header)?;
    let lod_count = header[25] as usize;
    let lod_table_offset = rd_u32(&header, 26) as usize;
    if lod_count == 0 {
        return Err(Error::BadOffset);
    }
    // Checked: `lod_table_offset` is an arbitrary header u32, so on the 32-bit
    // target the table-end can wrap `usize` and slip past the guard below.
    let lod_table_end = lod_count
        .checked_mul(LOD_ENTRY_LEN)
        .and_then(|len| lod_table_offset.checked_add(len))
        .ok_or(Error::BadOffset)?;
    if lod_table_end > total {
        return Err(Error::BadOffset);
    }
    Ok(HeaderPrologue { header, map, lod_count, lod_table_offset, total })
}

/// The session-resident, immutable map tables — everything [`Reader`] needs that doesn't change
/// frame to frame: header scalars, style table, LOD pyramid. Parsed **once** per `.obcm` by
/// [`MapTables::parse`], then borrowed by a cheap per-frame [`Reader::new`]. Keeping the per-frame
/// reader ~tens of bytes (no re-parse, no 2048-byte style scratch, no per-frame style-table SD
/// read) is what keeps the deep route-load render path inside the nRF's stack reserve.
pub struct MapTables {
    pub version: u8,
    pub bbox: BBox,
    /// User-position marker color (RGB565), a global header property; resolved to a device pixel
    /// by the host's color policy like style colors.
    pub marker_color: u16,
    /// LOD layers ordered coarsest (0) → finest (N-1). Always at least one.
    lods: Vec<Lod, 16>,
    /// The parsed POI directory (spec §7). Always present (six categories, some possibly
    /// empty, plus the hours-pool offset/count). Parse-only here — exposed via
    /// [`Reader::poi_directory`] for the nearest-N query and the P3 (#443) hours lookup.
    pois: PoiDirectory,
    /// The parsed nav directory (spec §8.1). Always present in v9 (possibly empty graph). The
    /// graph's only resident state besides the profile table — everything else streams via
    /// [`Reader::for_each_nav_node`] / [`Reader::nav_edge`].
    nav: NavDirectory,
    /// The parsed §8.6 routing profiles (1..=8, always present). RAM: at most 8 × 56 B = 448 B
    /// resident — the whole profile table stays in `.bss`, exposed via [`Reader::nav_profiles`].
    profiles: heapless::Vec<MapProfile, NAV_MAX_PROFILES>,
    /// Styles indexed by id (0..=255) for O(1) lookup during rendering.
    styles: [Option<Style>; 256],
    /// The backdrop style (bottom of the paint order; see [`Reader::backdrop_style`]), resolved
    /// once at parse so the per-frame lookup is a field read, not a 256-slot scan.
    backdrop: Option<Style>,
    /// Session-unique parse identity, never 0 (a zeroed [`MapCache`] sits at generation 0 =
    /// "unowned"). [`Reader::new`] hands it to the cache, which self-clears when it last served a
    /// different parse — the structural guard against a map switch cross-serving stale chunks.
    generation: u32,
}

impl MapTables {
    /// Parse the header scalars + style table + LOD pyramid from `src`. The one expensive,
    /// allocating step (a 2048-byte style scratch plus the style/LOD-table SD reads), so do it
    /// **once** per map and hand the result to [`Reader::new`] each frame. A map shorter than the
    /// header, with the wrong magic / version, or with out-of-range table offsets is rejected. The
    /// magic / version / bbox / marker prefix and the LOD table's position go through the shared
    /// `parse_prologue` (so a shard's own parse validates identically); the style table and the
    /// POI/nav section offsets are decoded here.
    pub fn parse(src: &dyn ByteSource) -> Result<MapTables, Error> {
        let HeaderPrologue { header, map, lod_count, lod_table_offset, total } = parse_prologue(src)?;
        let MapHeader { version, bbox, marker_color } = map;
        let style_offset = rd_u32(&header, 21) as usize;
        let poi_section_offset = rd_u32(&header, 32) as usize;
        let nav_section_offset = rd_u32(&header, 36) as usize;

        if style_offset < HEADER_LEN || style_offset > total {
            return Err(Error::BadOffset);
        }

        let mut styles = [None; 256];
        parse_styles(src, style_offset, total, &mut styles)?;
        let lods = parse_lod_table(src, lod_table_offset, lod_count, total)?;
        let pois = parse_poi_directory(src, poi_section_offset, total)?;
        let nav = parse_nav_directory(src, nav_section_offset, total)?;
        let profiles = parse_nav_profiles(src, &nav)?;
        // Resolve the backdrop (lowest `z_index`, ties broken by lowest id) once here; the table is
        // immutable after parse, so `Reader::backdrop_style` never has to re-scan the 256 slots.
        let backdrop = styles.iter().filter_map(|s| s.as_ref()).min_by_key(|s| (s.z_index, s.id)).copied();
        // Stamp a session-unique generation. `fetch_add + 1` starts the first parse at 1, so 0 is
        // never live — a zero-initialized `MapCacheInner` must always read as "unowned". `Relaxed`
        // suffices: the counter is the only shared state and only uniqueness matters.
        static GEN: AtomicU32 = AtomicU32::new(0);
        let generation = GEN.fetch_add(1, Ordering::Relaxed) + 1;
        Ok(MapTables { version, bbox, marker_color, lods, pois, nav, profiles, styles, backdrop, generation })
    }

    /// Whether LOD `lod` is written **empty** in this file's LOD table (`Index Node Count == 0`).
    /// The §5.6 mount-time predicate: pure I/O avoidance over one file's own table, never a
    /// statement about band membership or role.
    pub fn lod_is_empty(&self, lod: usize) -> bool {
        self.lods.get(lod).is_none_or(|entry| entry.node_count() == 0)
    }

    /// The parsed LOD pyramid (coarsest first) — the same slice [`Reader::lods`] returns, reachable
    /// without building a per-frame reader.
    #[inline]
    pub fn lods(&self) -> &[Lod] {
        &self.lods
    }

    /// The style table, indexed by id. Shards of one set carry byte-identical tables (they are the
    /// skin, §4.7), so a mount validates rather than re-loads.
    #[inline]
    pub fn styles(&self) -> &[Option<Style>; 256] {
        &self.styles
    }

    /// The map's §8.6 routing profiles (1..=8, always present). Lets a host mirror the profile
    /// **names** into the app UI (`App::set_nav_profiles`) straight off the parsed tables, without
    /// building a per-frame [`Reader`] — the same slice [`Reader::nav_profiles`] returns.
    pub fn nav_profiles(&self) -> &[MapProfile] {
        &self.profiles
    }

    /// The pre-resolved bottom-most style shared by every reader over these tables.
    #[inline]
    pub fn backdrop_style(&self) -> Option<&Style> {
        self.backdrop.as_ref()
    }

    /// Whether the map carries a non-empty §8 nav graph — the once-per-map-load feed behind
    /// `App::set_map_nav_graph` (#882: a graph-less map dims the Detour station instead of
    /// failing a plan). Same parsed-tables convenience rationale as [`nav_profiles`](Self::nav_profiles).
    pub fn has_nav_graph(&self) -> bool {
        !self.nav.is_empty()
    }
}

pub struct Reader<'a> {
    /// The byte source the index + geometry chunks stream from. `&dyn` (not a generic) so
    /// signatures holding a `&Reader` need no `<S>` parameter.
    src: &'a dyn ByteSource,
    /// Header scalars, **copied** from [`MapTables`] so `reader.version` / `.bbox` / `.marker_color`
    /// field access stays direct while the big tables stay borrowed.
    pub version: u8,
    pub bbox: BBox,
    pub marker_color: u16,
    /// The session-resident immutable tables (style table + LOD pyramid), parsed once and borrowed
    /// here so a per-frame `Reader` carries no styles/lods of its own.
    tables: &'a MapTables,
    /// Borrowed lazy-read cache for the streamed index + geometry — the ≈36 KB of buffers live in
    /// the caller's [`MapCache`], reusable across frames. It keeps its own `RefCell` because
    /// `read_at` takes `&self` but the cache mutates; the borrows are tightly scoped so the
    /// index-node read and the chunk decode never overlap.
    cache: &'a MapCache,
    /// False only when construction legally re-entered an already borrowed cache. Streamed calls
    /// then return `CacheError::Busy`; reconstructing the cheap reader is the retry.
    cache_ready: bool,
    /// Which file of a mounted map this reader reads — `0` for a single `.obcm`, the shard index
    /// for a member of a volume set (`OBCA_Spec.md` §5). It tags every cache key so the shards of
    /// one set can share a single ≈37 KB [`MapCache`] (and one parse generation) without
    /// cross-serving each other's chunks.
    file: u8,
    /// A volume-set shard's **own** LOD table, borrowed from its [`crate::volume::ShardTables`].
    /// `None` for a single map and for the core shard, whose ladder is `tables.lods`. A shard
    /// carries the full ladder with the LODs it does not hold written empty (`OBCA_Spec.md`
    /// §5.1), so this is what makes a per-shard reader address its own chunk offsets.
    shard_lods: Option<&'a [Lod]>,
}

impl<'a> Reader<'a> {
    /// Build a per-frame reader over the pre-parsed [`MapTables`], a fresh `src`, and a `cache` the
    /// geometry + index reads stream through. **Cheap**: borrows the tables and copies only the
    /// header scalars (no parse, no SD read). The cache is caller-owned and reusable across frames;
    /// pass a fresh [`MapCache::new`] if you don't keep one. The cache *adopts* these tables' parse
    /// generation here: building a reader over a different map's tables auto-clears the stale
    /// slots, so a map switch (a re-`parse`) can never cross-serve the old map's chunks — no manual
    /// [`MapCache::clear`] required.
    pub fn new(src: &'a dyn ByteSource, tables: &'a MapTables, cache: &'a MapCache) -> Reader<'a> {
        let cache_ready = cache.adopt(tables.generation).is_ok();
        Reader {
            src,
            version: tables.version,
            bbox: tables.bbox,
            marker_color: tables.marker_color,
            tables,
            cache,
            cache_ready,
            file: 0,
            shard_lods: None,
        }
    }

    /// Build a reader over shard `file` of a mounted volume set. Identical to [`Reader::new`]
    /// except that every cache key it writes is tagged with the shard index, which is what lets a
    /// set's shards share one ≈37 KB [`MapCache`] without cross-serving each other's chunks.
    ///
    /// `tables` is always the **core**'s: the whole set is stamped from one skin (`OBCA_Spec.md`
    /// §4.7), so one style table serves every shard, and one parse generation means no shard
    /// clears the cache the previous one filled. `shard` supplies the parts that are *not* shared
    /// — the shard's own header bbox (the quadtree root) and its own LOD table (its chunk-offset
    /// tables live at its own offsets); `None` means the core, whose bbox and ladder are already
    /// `tables`'.
    ///
    /// Crate-private on purpose. Because `tables` is the core's, the POI and nav directories a
    /// non-core reader would report are the *core file's* offsets against a *shard's* bytes —
    /// meaningless. [`crate::volume::MountedSet`] therefore uses these readers for geometry only
    /// and routes nav/POI/hours to [`crate::volume::MountedSet::core_reader`] (§5.1).
    pub(crate) fn new_in_set(
        src: &'a dyn ByteSource,
        tables: &'a MapTables,
        cache: &'a MapCache,
        file: u8,
        shard: Option<&'a crate::volume::ShardTables>,
    ) -> Reader<'a> {
        let mut reader = Reader { file, ..Reader::new(src, tables, cache) };
        if let Some(shard) = shard {
            reader.bbox = shard.bbox();
            reader.shard_lods = Some(shard.lods());
        }
        reader
    }

    /// Which file of the mounted map this reader reads (`0` for a single `.obcm`).
    #[inline]
    pub fn file(&self) -> u8 {
        self.file
    }

    /// Whether this reader reads a **non-core shard** of a volume set (`OBCA_Spec.md` §5).
    ///
    /// It is the one structural fact that separates the geometry path from the nav/POI/hours one.
    /// A shard reader borrows the **core's** [`MapTables`] — that is the whole RAM argument of a
    /// set — so its `pois`/`nav` directories describe offsets into the *core file* while `self.src`
    /// is the shard's bytes. Reading one against the other is not a degraded answer, it is a read
    /// at an unrelated offset. Every nav, POI and hours accessor below therefore answers **empty**
    /// on a shard rather than trusting a doc comment to keep callers away; `MountedSet` routes
    /// those queries to [`crate::volume::MountedSet::core_reader`] (§5.1).
    ///
    /// Deliberately not a `debug_assert`: the empty answer *is* the contract (a set's dispatch is
    /// role-blind, so a caller reaching a shard is normal), and an assertion would make the tests
    /// that pin the contract unrunnable.
    #[inline]
    pub fn is_set_shard(&self) -> bool {
        self.shard_lods.is_some()
    }

    /// Snapshot of the chunk-cache + streaming counters. Cumulative over the cache's life, so the
    /// renderer reports the per-frame delta.
    #[inline]
    pub fn chunk_cache_stats(&self) -> CacheStats {
        self.try_chunk_cache_stats().unwrap_or_default()
    }

    /// Fallible cache-counter snapshot for callers that must distinguish legal contention from an
    /// actually empty cache. The compatibility [`Reader::chunk_cache_stats`] view never panics.
    #[inline]
    pub(crate) fn try_chunk_cache_stats(&self) -> Result<CacheStats, CacheError> {
        if !self.cache_ready {
            return Err(CacheError::Busy);
        }
        self.cache.stats()
    }

    /// The parsed LOD pyramid (coarsest first).
    #[inline]
    pub fn lods(&self) -> &[Lod] {
        self.shard_lods.unwrap_or(&self.tables.lods)
    }

    /// The parsed POI directory (spec §7): the shared chunk size, one entry per category, and the
    /// v7 hours-pool offset/count. Always present (six categories, some possibly empty).
    /// [`Reader::nearest_pois`] walks the per-category quadtrees; P3 (#443) reads
    /// [`PoiDirectory::hours_pool_offset`]/[`PoiDirectory::hours_pool_count`] to resolve a POI's
    /// pooled schedule.
    ///
    /// [`PoiDirectory::EMPTY`] on a volume-set shard — see [`Reader::is_set_shard`].
    #[inline]
    pub fn poi_directory(&self) -> &PoiDirectory {
        if self.is_set_shard() {
            return &EMPTY_POI_DIRECTORY;
        }
        &self.tables.pois
    }

    /// Resolve a POI's pooled weekly schedule (spec §7.5) from its `hours_ref`. `None` for the
    /// no-hours sentinel `0xFFFF`, an index `>= hours_pool_count`, or any read/decode failure — so a
    /// corrupt directory (a bad `hours_pool_offset`/`count`) or a flaky read yields `None`, never a
    /// panic/UB. On-demand: the detail screen (#444) calls this once with the [`Poi::hours_ref`] the
    /// list snapshot carried; it reads the single 29-byte blob into a **stack** buffer via
    /// [`ByteSource::read_at`] (no [`MapCache`] growth, no static/`.bss` buffer).
    ///
    /// Blob `hours_ref` lives at `hours_pool_offset + 2 + hours_ref*29` (the `+2` skips the pool's
    /// `count u16`). Every step is checked 32-bit so a corrupt offset/count can't wrap or read past
    /// the file.
    ///
    /// # Reentrancy
    ///
    /// Unlike [`Reader::nearest_pois`], this does **not** touch the [`MapCache`] — it's a plain
    /// stack read, safe to call from anywhere (including inside a `for_each_*` callback).
    pub fn poi_hours(&self, hours_ref: u16) -> Option<crate::hours::WeeklySchedule> {
        // A volume-set shard carries no hours pool (see `is_set_shard`).
        if self.is_set_shard() {
            return None;
        }
        // The no-hours sentinel and any index past the pool ⇒ no schedule.
        let dir = &self.tables.pois;
        if hours_ref == POI_HOURS_REF_NONE || (hours_ref as usize) >= dir.hours_pool_count {
            return None;
        }
        // Byte offset of blob `hours_ref`: hours_pool_offset + 2 + hours_ref*29. All checked so a
        // corrupt directory can't wrap `u32` or address past the file.
        let blob_off = (hours_ref as u32)
            .checked_mul(POI_HOURS_BLOB_LEN as u32)?
            .checked_add(2)?
            .checked_add(u32::try_from(dir.hours_pool_offset).ok()?)?;
        let end = blob_off.checked_add(POI_HOURS_BLOB_LEN as u32)?;
        if end > self.src.len() {
            return None;
        }
        // A single small stack read — no cache, no static buffer.
        let mut blob = [0u8; POI_HOURS_BLOB_LEN];
        self.src.read_at(blob_off, &mut blob).ok()?;
        crate::hours::WeeklySchedule::decode(&blob)
    }

    /// The nearest [`MAX_POI_RESULTS`] POIs of `category` to `pos` (a `(lon, lat)` µdeg pair, the
    /// crate's coordinate order), ascending by ground distance. Fills the caller-owned `out`
    /// (cleared first) — fewer than 16 when the category holds fewer in the whole map, empty when
    /// the category is empty. On-demand (a user opening a list), never per-frame.
    ///
    /// **Expanding-ring scan (spec §7.2 / epic #115).** Walks the category's quadtree over a square
    /// search bbox that starts ~2 km half-extent around `pos` and **doubles** until the nearest-16
    /// are provably found — the set is full *and* its 16th is no farther than the bbox half-extent
    /// (anything outside a square bbox is at least half-extent away), or the bbox has grown to
    /// contain the whole map (then the pass was exhaustive). No new persistent state: each chunk
    /// streams through a single 512-byte stack scratch, `pos`'s `cos_lat` is hoisted once, and the
    /// 16-slot best-set lives in `out`. A record revisited on a wider pass is deduped by its
    /// `(lat, lon, subtype)` so it's never returned twice. Structurally invalid records such as an
    /// out-of-range subtype are skipped; source or index-cache failures return a typed [`Error`]
    /// rather than being mistaken for an empty category.
    ///
    /// # Reentrancy
    ///
    /// Like the geometry walk, this streams through the internal cache. Legal re-entry while a
    /// feature callback holds that cache returns [`Error::CacheBusy`] instead of panicking.
    pub fn nearest_pois(
        &self,
        category: PoiCategory,
        pos: (i32, i32),
        out: &mut Vec<Poi, MAX_POI_RESULTS>,
    ) -> Result<(), Error> {
        out.clear();
        // A volume-set shard carries no POI section (see `is_set_shard`).
        if self.is_set_shard() {
            return Ok(());
        }
        let dir = &self.tables.pois;
        let entry = match dir.entries.iter().find(|e| e.category_id == category.id()) {
            // An absent or empty category is a valid "no POIs here" answer, not an error.
            Some(e) if !e.is_empty() => *e,
            _ => return Ok(()),
        };
        // `chunk_size / POI_RECORD_LEN` is the record cap; a corrupt 0 would divide-by-zero / loop
        // forever, so treat the whole (unwalkable) section as empty.
        if dir.chunk_size < POI_RECORD_LEN {
            return Ok(());
        }

        // Hoist `cos_lat` once for the query band. Guard a degenerate `cl` (≈0 near the poles, or a
        // corrupt latitude) so the lon half-extent below can't divide by zero / overflow.
        let cl = cos_lat(pos.1).max(1e-3);
        let map = self.bbox;

        let mut half = POI_SEARCH_HALF_UDEG;
        loop {
            // Square in ground meters: the lon half-extent is scaled by 1/cos_lat so both axes span
            // the same ~ `half`-µdeg-of-latitude ground distance. Saturating so a huge `half` (late
            // passes) can't wrap i32.
            let lon_half = ((half as f32 / cl) as i32).max(1);
            let search = BBox {
                min_lon: pos.0.saturating_sub(lon_half),
                min_lat: pos.1.saturating_sub(half),
                max_lon: pos.0.saturating_add(lon_half),
                max_lat: pos.1.saturating_add(half),
            };
            // Re-walk from scratch each pass (the set dedups revisits). The set only ever holds the
            // true nearest-16 seen so far, so a superset pass converges it.
            self.poi_scan(&entry, dir.chunk_size, pos, cl, &search, out)?;

            // The half-extent as a ground radius: everything outside the square is at least this far
            // (the tighter of the two axes' meter half-extents — they're ~equal by construction, but
            // take the min to stay a sound lower bound). `half` µdeg-of-latitude → meters.
            let half_m = (half as f32) * (M_PER_DEG as f32) * 1e-6;
            let full = out.len() == MAX_POI_RESULTS;
            if full && (out[MAX_POI_RESULTS - 1].distance_m as f32) <= half_m {
                return Ok(());
            }
            // The search bbox already covers the whole map ⇒ this pass was exhaustive; whatever is in
            // the set is the final answer (even if < 16).
            if search.min_lon <= map.min_lon
                && search.min_lat <= map.min_lat
                && search.max_lon >= map.max_lon
                && search.max_lat >= map.max_lat
            {
                return Ok(());
            }
            // Double the ring and re-walk. Saturating so we can't overflow before the map-cover check
            // above trips.
            half = half.saturating_mul(2);
        }
    }

    /// One expanding-ring pass: walk `entry`'s quadtree for leaves overlapping `search` and fold
    /// every valid record of every non-empty leaf into the nearest-16 `out` set (deduped by
    /// `(lat, lon, subtype)`). `cl` is the hoisted `cos_lat`; distances are equirectangular ground
    /// meters via the shared `obc-map-scene` distance core. The walk and the record streaming are
    /// [`Reader::scan_poi_leaves`] / [`Reader::stream_poi_records`]; this is only the tail that
    /// scores a record.
    fn poi_scan(
        &self,
        entry: &PoiCatEntry,
        chunk_size: usize,
        pos: (i32, i32),
        cl: f32,
        search: &BBox,
        out: &mut Vec<Poi, MAX_POI_RESULTS>,
    ) -> Result<(), Error> {
        self.scan_poi_leaves(entry, chunk_size, search, |start, record_cap| {
            self.stream_poi_records(start, record_cap, |win, off, lat, lon, subtype| {
                let distance_m = ground_dist_m_cl(pos, (lon, lat), cl) as u32;
                consider_poi(out, PoiCand { lat, lon, subtype, distance_m }, win, off);
            })
        })
    }

    /// Walk `entry`'s quadtree for leaves overlapping `search` and stream every non-empty leaf's
    /// chunk through `scan`, which is handed the chunk's byte offset and the per-chunk record cap.
    /// The shared skeleton behind both POI queries — the expanding-ring
    /// [`nearest_pois`](Reader::nearest_pois) pass and the per-route-chunk
    /// [`corridor_pois`](Reader::corridor_pois) pass — which differ only in what they do with a
    /// record.
    ///
    /// The chunk decode runs **inside** the walk callback: `walk_leaves` releases its index-cache
    /// borrow before invoking the callback, and the POI chunk read goes through a plain
    /// `src.read_at` stack scratch (never the `MapCache`), so the two never nest — and the pass is
    /// truly streaming with **no per-leaf buffer**, so an exhaustive (map-covering) final pass can't
    /// silently drop a leaf however dense the category. A leaf whose chunk id is out of range or
    /// whose extent runs past EOF is skipped; the first read failure stops the walk and is replayed
    /// as the return value (a `walk_leaves` callback cannot itself fail).
    fn scan_poi_leaves(
        &self,
        entry: &PoiCatEntry,
        chunk_size: usize,
        search: &BBox,
        mut scan: impl FnMut(u32, usize) -> Result<(), IoError>,
    ) -> Result<(), Error> {
        // The whole chunk's record count. A chunk with no sentinel room (records × 32 == chunk_size)
        // is bounded by this count instead (mirrors `for_each_feature_filtered`).
        let records_per_chunk = chunk_size / POI_RECORD_LEN;
        let mut read_error = None;
        self.walk_leaves(entry, 0, self.bbox, search, 0, &mut |cid, _node| {
            if read_error.is_some() {
                return;
            }
            let (start, end) = match entry.chunk_range(cid, chunk_size) {
                Some(r) => r,
                None => return,
            };
            if end > self.src.len() as usize {
                return;
            }
            if let Err(error) = scan(start as u32, records_per_chunk) {
                read_error = Some(error);
            }
        })
        .map_err(Error::from)?;
        if let Some(error) = read_error {
            return Err(Error::Source(error));
        }
        Ok(())
    }

    /// Stream one POI chunk's records through a single **512-byte** stack scratch — `POI_SCAN_WINDOW`
    /// bytes (16 records) at a time — handing each *valid* record to `visit` as
    /// `(window, record offset, lat, lon, subtype)`; the window slice stays borrowed so the caller
    /// can pull the name/hours fields out of it without a copy. Reading in a fixed window keeps the
    /// scratch tiny regardless of the accepted `chunk_size` (up to `POI_MAX_CHUNK_BYTES`);
    /// `POI_RECORD_LEN` divides the window so a record never straddles two reads. `start` is the
    /// chunk's byte offset, already bounds-checked by the caller. Terminates on the `0xFF` subtype
    /// sentinel or after `record_cap` records (a sentinel-less full chunk).
    fn stream_poi_records(
        &self,
        start: u32,
        record_cap: usize,
        mut visit: impl FnMut(&[u8], usize, i32, i32, u8),
    ) -> Result<(), IoError> {
        const RECS_PER_WINDOW: usize = POI_SCAN_WINDOW / POI_RECORD_LEN;
        let mut scratch = [0u8; POI_SCAN_WINDOW];
        let mut done = 0usize;
        while done < record_cap {
            let take = (record_cap - done).min(RECS_PER_WINDOW);
            let win = &mut scratch[..take * POI_RECORD_LEN];
            self.src.read_at(start + (done * POI_RECORD_LEN) as u32, win)?;
            for r in 0..take {
                let off = r * POI_RECORD_LEN;
                let subtype = win[off + 8];
                if subtype == CHUNK_END {
                    return Ok(()); // end-of-records sentinel — nothing valid follows in this chunk
                }
                // Skip an out-of-range subtype (0, or past the table) cleanly — never panic/UB.
                if obc_formats::obcm::poi_subtype_row(subtype).is_none() {
                    continue;
                }
                visit(win, off, rd_i32(win, off), rd_i32(win, off + 4), subtype);
            }
            done += take;
        }
        Ok(())
    }

    /// The POIs of `cats` sitting within [`CORRIDOR_HALF_WIDTH_M`] of the route **ahead** of
    /// `progress_m`, ascending by along-route distance, capped at [`MAX_CORRIDOR_RESULTS`] — the
    /// data source behind the "Up ahead" timeline (epic #946). Fills the caller-owned `out` (cleared
    /// first). On-demand (a snapshot taken on screen entry / filter change), **never** per frame.
    ///
    /// Each result carries where it projects onto the route ([`CorridorPoi::dist_along_m`], on the
    /// same axis stored waypoints use) and a **signed** lateral offset
    /// ([`CorridorPoi::offset_m`]: positive = right of the direction of travel).
    ///
    /// # The walk
    ///
    /// One pass over the route's chunks in route order, driven by [`RoutePath`] — the resident chunk
    /// index the breadcrumb/progress machinery already holds, so no full-route re-read. For each
    /// chunk still ahead of `progress_m`:
    ///
    /// 1. its bbox is inflated by the corridor half-width ([`inflate_bbox`]) — a tight window, since
    ///    a route chunk spans a few hundred meters, not the whole route;
    /// 2. the chunk's polyline is decoded **once** into the path's own scratch;
    /// 3. each selected category's quadtree is walked over that window (the same
    ///    [`walk_leaves`](Reader::walk_leaves) the geometry and nearest-N queries use), and every POI
    ///    record streams through a 512-byte stack scratch exactly as in
    ///    [`nearest_pois`](Reader::nearest_pois) — no per-leaf buffer, no [`MapCache`] growth;
    /// 4. each record is projected onto that chunk ([`project_onto_chunk`]) and folded into `out` if
    ///    it is inside the corridor and at or past `progress_m`.
    ///
    /// **Cost bound.** At most one route-chunk decode plus one quadtree descent per (remaining
    /// chunk × selected non-empty category); an absent or empty category costs nothing. The walk
    /// **stops early** once `out` is full and the current chunk starts farther along than the
    /// worst-held result — no POI from there on could displace one — so a POI-dense route pays for
    /// the first ~16 results, not for its whole length.
    ///
    /// **Dedupe.** A POI is keyed by `(lat, lon, subtype)` and appears once, at its nearest
    /// projection: `project_onto_chunk` already resolves a switchback *within* one chunk, and a POI
    /// re-found from a later chunk replaces the held entry only when its offset is smaller.
    /// (Refinement is naturally bounded to the chunks actually walked — the early exit above stops
    /// at the point where nothing new can enter the list anyway.)
    ///
    /// # Reentrancy
    ///
    /// Like [`nearest_pois`](Reader::nearest_pois) the quadtree walk streams through the internal
    /// index cache; legal re-entry returns [`Error::CacheBusy`]. The POI chunk reads go through
    /// plain stack `read_at`s, so they never nest with it.
    pub fn corridor_pois(
        &self,
        cats: PoiCategorySet,
        path: &dyn RoutePath,
        progress_m: u32,
        out: &mut Vec<CorridorPoi, MAX_CORRIDOR_RESULTS>,
    ) -> Result<(), Error> {
        out.clear();
        // A volume-set shard carries no POI section (see `is_set_shard`).
        if self.is_set_shard() {
            return Ok(());
        }
        let dir = &self.tables.pois;
        // `chunk_size / POI_RECORD_LEN` is the per-chunk record cap; a corrupt 0 would divide by
        // zero, so treat the whole (unwalkable) section as empty — same guard as `nearest_pois`.
        if dir.chunk_size < POI_RECORD_LEN || cats.is_empty() {
            return Ok(());
        }
        // Resolve the filter to the directory entries once, dropping absent/empty categories so the
        // per-chunk loop below never pays for a category this map doesn't carry.
        let mut entries: Vec<PoiCatEntry, POI_MAX_CATEGORIES> = Vec::new();
        for cat in cats.iter() {
            if let Some(e) = dir.entries.iter().find(|e| e.category_id == cat.id() && !e.is_empty()) {
                let _ = entries.push(*e);
            }
        }
        if entries.is_empty() {
            return Ok(());
        }

        let chunks = path.chunk_count();
        for k in 0..chunks {
            let start_m = path.chunk_start_m(k);
            // The chunk ends where the next one starts; the last chunk runs to the route end.
            let end_m = if k + 1 < chunks { path.chunk_start_m(k + 1) } else { u32::MAX };
            if end_m < progress_m {
                continue; // wholly behind the rider — nothing here can be "ahead"
            }
            // Early exit: `out` is sorted ascending and this chunk (and every later one) projects no
            // nearer than its own start, so a full set whose worst is already nearer is final.
            if out.len() == MAX_CORRIDOR_RESULTS && start_m >= out[MAX_CORRIDOR_RESULTS - 1].dist_along_m {
                break;
            }
            let search = inflate_bbox(path.chunk_bbox(k), CORRIDOR_HALF_WIDTH_M);
            let mut scan_error = None;
            // The chunk's polyline is decoded into the path's scratch and borrowed for this
            // callback; the quadtree walks run inside it so the geometry is never copied.
            path.visit_chunk_points(k, &mut |pts| {
                if pts.len() < 2 || scan_error.is_some() {
                    return;
                }
                for entry in &entries {
                    if let Err(error) =
                        self.corridor_scan_category(entry, dir.chunk_size, &search, pts, start_m, progress_m, out)
                    {
                        scan_error = Some(error);
                        return;
                    }
                }
            });
            if let Some(error) = scan_error {
                return Err(error);
            }
        }
        Ok(())
    }

    /// Walk one category's quadtree over `search` and fold every record that projects inside the
    /// corridor of `pts` into `out`. Shares its walk and its record streaming with the nearest-N
    /// query ([`Reader::scan_poi_leaves`] / [`Reader::stream_poi_records`], and so the same "no
    /// per-leaf buffer, never nested with the index cache" discipline); this is only the tail that
    /// projects a record onto the route.
    #[allow(clippy::too_many_arguments)]
    fn corridor_scan_category(
        &self,
        entry: &PoiCatEntry,
        chunk_size: usize,
        search: &BBox,
        pts: &[(i32, i32)],
        chunk_start_m: u32,
        progress_m: u32,
        out: &mut Vec<CorridorPoi, MAX_CORRIDOR_RESULTS>,
    ) -> Result<(), Error> {
        self.scan_poi_leaves(entry, chunk_size, search, |start, record_cap| {
            self.stream_poi_records(start, record_cap, |win, off, lat, lon, subtype| {
                // The corridor half-width is handed to the projection so it can prune segments as it
                // walks (a chunk is up to 256 points); `None` **is** the outside-the-corridor reject.
                let Some(proj) = project_onto_chunk(pts, chunk_start_m, (lon, lat), CORRIDOR_HALF_WIDTH_M) else {
                    return;
                };
                // The route axis is non-negative and the projection is clamped to the chunk, so the
                // round is a plain truncation; entries behind the rider are dropped here.
                let dist_along_m = proj.dist_along_m.max(0.0) as u32;
                if dist_along_m < progress_m {
                    return;
                }
                let cand = CorridorCand {
                    lat,
                    lon,
                    subtype,
                    dist_along_m,
                    offset_m: libm::roundf(proj.offset_m) as i32,
                    to_go_m: dist_along_m - progress_m,
                };
                consider_corridor_poi(out, cand, win, off);
            })
        })
    }

    /// The parsed nav directory (spec §8.1). Always present in v9; `is_empty()` for a map with no
    /// routable ways, and [`NavDirectory::EMPTY`] on a volume-set shard (see
    /// [`Reader::is_set_shard`]) — the graph lives in the core file alone.
    #[inline]
    pub fn nav_directory(&self) -> &NavDirectory {
        if self.is_set_shard() {
            return &EMPTY_NAV_DIRECTORY;
        }
        &self.tables.nav
    }

    /// The map's §8.6 routing profiles (1..=8, always present even for an empty graph). N5 exposes
    /// their names on the device; N3 selects one by index and weights edges by
    /// [`MapProfile::multiplier`]. Empty on a volume-set shard, which has no graph to profile —
    /// the set's profiles are the core's, through
    /// [`crate::volume::MountedSet::core_reader`].
    #[inline]
    pub fn nav_profiles(&self) -> &[MapProfile] {
        if self.is_set_shard() {
            return &[];
        }
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
        // A volume-set shard carries no nav graph (see `is_set_shard`).
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
            if end > self.src.len() as usize {
                return;
            }
            let chunk = &mut scratch[..dir.chunk_size];
            if let Err(error) = self.src.read_at(start as u32, chunk) {
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

    /// Fetch one §8.4 edge polyline by its `edge_id` (a pool-relative byte offset — chunk
    /// `id / chunk_size`, offset `id % chunk_size`, spec §8.4), decoding anchor + deltas into
    /// `points` as the crate's `(lon, lat)` µdeg pairs. Returns the edge's `length_m`. R3 calls
    /// this only at OBCR emit, stitching the came-from chain's geometry.
    ///
    /// `None` for an empty graph, an out-of-pool or non-record-aligned id, a record that would
    /// straddle its chunk (the packer never writes one), a read failure, or a polyline exceeding
    /// `P` — a corrupt id degrades to "no geometry", never a panic. The deltas stream through a
    /// small fixed stack window; no cache is touched, so (like [`Reader::poi_hours`]) this is safe
    /// to call from anywhere.
    pub fn nav_edge<const P: usize>(&self, edge_id: u32, points: &mut Vec<(i32, i32), P>) -> Option<u32> {
        points.clear();
        // A volume-set shard carries no edge pool (see `is_set_shard`).
        let dir = self.nav_directory();
        let cs = dir.chunk_size;
        if dir.edge_chunk_count == 0 || cs == 0 {
            return None;
        }
        // Pool bounds + intra-chunk bounds for the fixed head. All checked: `edge_id` is
        // unvalidated input (a corrupt map, or R3 handed a stale id).
        let pool_len = dir.edge_chunk_count.checked_mul(cs)?;
        let id = edge_id as usize;
        let within = id % cs;
        if within + NAV_EDGE_FIXED_LEN > cs || id.checked_add(NAV_EDGE_FIXED_LEN)? > pool_len {
            return None;
        }
        let start = dir.edge_pool_offset.checked_add(id)?;
        let mut head = [0u8; NAV_EDGE_FIXED_LEN];
        let head_off = u32::try_from(start).ok()?;
        if start.checked_add(NAV_EDGE_FIXED_LEN)? > self.src.len() as usize {
            return None;
        }
        self.src.read_at(head_off, &mut head).ok()?;
        let length_m = rd_u32(&head, 0);
        let pt_count = rd_u16(&head, 4) as usize;
        // byte 6 is `way_kind` (§8.4); the anchor sits behind it, at 7 (lat) / 11 (lon).
        let anchor_lat = rd_i32(&head, 7);
        let anchor_lon = rd_i32(&head, 11);
        if pt_count == 0 {
            return None;
        }
        // The whole record must lie inside its chunk (the §8.4 no-straddle contract) and the file.
        let rec_len = NAV_EDGE_FIXED_LEN.checked_add((pt_count - 1).checked_mul(4)?)?;
        if within + rec_len > cs
            || id.checked_add(rec_len)? > pool_len
            || start.checked_add(rec_len)? > self.src.len() as usize
        {
            return None;
        }
        if pt_count > P {
            return None; // caller's buffer can't hold the polyline — corrupt or mis-sized
        }
        points.push((anchor_lon, anchor_lat)).ok()?;
        // Stream the (dlat, dlon) pairs through a fixed window, accumulating absolutes.
        let (mut lat, mut lon) = (anchor_lat, anchor_lon);
        let mut win = [0u8; NAV_EDGE_WINDOW];
        let mut done = 0usize; // delta pairs decoded
        while done < pt_count - 1 {
            let take = (pt_count - 1 - done).min(NAV_EDGE_WINDOW / 4);
            let buf = &mut win[..take * 4];
            let off = start + NAV_EDGE_FIXED_LEN + done * 4;
            self.src.read_at(off as u32, buf).ok()?;
            for pair in buf.chunks_exact(4) {
                lat = lat.wrapping_add(rd_i16(pair, 0) as i32);
                lon = lon.wrapping_add(rd_i16(pair, 2) as i32);
                points.push((lon, lat)).ok()?;
            }
            done += take;
        }
        Some(length_m)
    }

    /// [`Reader::for_each_nav_node`] with the chunk read routed through a caller-owned
    /// [`NavTileCache`] instead of a bare scratch — the router's settle primitive (#465). A*'s
    /// spatial re-fetch settles one node at a time (a degenerate one-point `view`). It does **not**
    /// walk a single advancing neighborhood: the heap pops the globally best-`f` node, so successive
    /// settles scatter across the frontier's several live quadtree leaves. The route-private
    /// [`NAV_TILE_SLOTS`] working set keeps those leaves resident so the per-settle re-fetch mostly
    /// hits. Same decode, same corrupt-input posture, same reentrancy rule as the uncached walk.
    pub fn for_each_nav_node_cached(
        &self,
        view: &BBox,
        tiles: &mut NavTileCache,
        mut visit: impl FnMut(NavNodeRef),
    ) -> Result<(), Error> {
        // A volume-set shard carries no nav graph (see `is_set_shard`).
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
            if end > self.src.len() as usize {
                return;
            }
            let off = match u32::try_from(start) {
                Ok(o) => o,
                Err(_) => return,
            };
            // A failed fill skips this leaf cleanly (the cache never keeps a bad slot).
            match tiles.chunk(self.src, off, dir.chunk_size) {
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
            if end > self.src.len() as usize {
                return;
            }
            let Some(off) = u32::try_from(start).ok() else { return };
            let mut local = [0u8; NAV_MAX_CHUNK_BYTES];
            {
                let Some(chunk) = tiles.chunk(self.src, off, dir.chunk_size) else {
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
                if end > self.src.len() as usize {
                    return;
                }
                let Some(off) = u32::try_from(start).ok() else { return };
                let mut local = [CHUNK_END; NAV_CHUNK_SIZE];
                {
                    let Some(chunk) = tiles.chunk(self.src, off, dir.chunk_size) else {
                        read_error = Some(IoError::Io);
                        return;
                    };
                    local[..dir.chunk_size].copy_from_slice(chunk);
                }
                for rec in local[..dir.chunk_size].chunks_exact(NAV_SNAP_RECORD_LEN) {
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
        let id = edge_id as usize;
        let within = id % cs;
        if id / cs >= dir.edge_chunk_count || within + NAV_EDGE_FIXED_LEN > cs {
            return None;
        }
        let chunk_start = dir.edge_pool_offset.checked_add(id - within)?;
        if chunk_start.checked_add(cs)? > self.src.len() as usize {
            return None;
        }
        let chunk = tiles.chunk(self.src, u32::try_from(chunk_start).ok()?, cs)?;
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
            for (i, pair) in deltas.chunks_exact(4).enumerate() {
                point = step(point, pair);
                let vertex = i + 1;
                if vertex > from.segment as usize && vertex <= to.segment as usize {
                    emit(point);
                }
            }
        } else {
            let mut point = anchor;
            for pair in deltas.chunks_exact(4) {
                point = step(point, pair);
            }
            for (i, pair) in deltas.chunks_exact(4).enumerate().rev() {
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
        let id = edge_id as usize;
        let within = id % cs;
        if id / cs >= dir.edge_chunk_count || within + NAV_EDGE_FIXED_LEN > cs {
            return None;
        }
        let chunk_start = dir.edge_pool_offset.checked_add(id - within)?;
        if chunk_start.checked_add(cs)? > self.src.len() as usize {
            return None;
        }
        let chunk = tiles.chunk(self.src, u32::try_from(chunk_start).ok()?, cs)?;
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
        for (i, pair) in deltas.chunks_exact(4).enumerate() {
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
        let val = tiles.index_node(self.src, self.file, index, idx).map_err(MapReadError::Source)?;
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
        // A volume-set shard carries no edge pool (see `is_set_shard`).
        let dir = self.nav_directory();
        let cs = dir.chunk_size;
        if dir.edge_chunk_count == 0 || cs == 0 {
            return None;
        }
        // Pool + intra-chunk bounds, checked exactly as `nav_edge` (`edge_id` is unvalidated).
        let id = edge_id as usize;
        let within = id % cs;
        if id / cs >= dir.edge_chunk_count || within + NAV_EDGE_FIXED_LEN > cs {
            return None;
        }
        let chunk_start = dir.edge_pool_offset.checked_add(id - within)?;
        if chunk_start.checked_add(cs)? > self.src.len() as usize {
            return None;
        }
        let chunk = tiles.chunk(self.src, u32::try_from(chunk_start).ok()?, cs)?;
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
            for pair in deltas.chunks_exact(4) {
                p = step(p, pair);
                emit(p);
            }
            return Some(length_m);
        }
        // Maybe reversed: forward-sum the deltas for the `b` endpoint…
        let mut p = anchor;
        for pair in deltas.chunks_exact(4) {
            p = step(p, pair);
        }
        if p != start {
            return None; // matches neither endpoint — a stale/corrupt edge id
        }
        // …then walk them backward, undoing one delta per point.
        emit(p);
        for pair in deltas.chunks_exact(4).rev() {
            p = (p.0.wrapping_sub(rd_i16(pair, 2) as i32), p.1.wrapping_sub(rd_i16(pair, 0) as i32));
            emit(p);
        }
        Some(length_m)
    }

    #[inline]
    pub fn style(&self, id: u8) -> Option<&Style> {
        self.tables.styles.get(id as usize).and_then(|s| s.as_ref())
    }

    /// The backdrop style: the one at the bottom of the paint order (lowest
    /// `z_index`, ties broken by lowest id). By convention the map's sea/
    /// background style sits here, so its color fills the screen before any
    /// geometry is drawn. Resolved once in [`MapTables::parse`]; returns `None`
    /// only for an empty style table.
    pub fn backdrop_style(&self) -> Option<&Style> {
        self.tables.backdrop.as_ref()
    }

    /// Pick the finest LOD whose range still covers `mpp` (meters/pixel). The
    /// coarsest level (`max_mpp == +inf`) always qualifies, so the result is a
    /// valid index in `0..lods().len()`.
    pub fn select_lod_for_mpp(&self, mpp: f32) -> usize {
        let mut chosen = 0;
        for (i, lod) in self.lods().iter().enumerate() {
            if lod.max_mpp >= mpp {
                chosen = i;
            }
        }
        chosen
    }

    /// Read node `idx` of a [`QuadIndex`] (a `u32`), streamed through the index block cache. `None`
    /// on a read failure — the walk then skips that subtree. `idx < node_count` and the index
    /// region lies within the file (both guaranteed by `walk_leaves`/`parse_lod_table` /
    /// `parse_poi_directory`), so the offset never overflows `u32`.
    #[inline]
    fn read_node(&self, index: &dyn QuadIndex, idx: usize) -> Result<u32, MapReadError> {
        if !self.cache_ready {
            return Err(MapReadError::Cache(CacheError::Busy));
        }
        let off = (index.index_offset() + idx * 4) as u32;
        let mut b = [0u8; 4];
        self.cache
            .try_borrow_mut()
            .map_err(MapReadError::Cache)?
            .index_read(self.src, self.file, off, &mut b)
            .map_err(MapReadError::Source)?;
        Ok(u32::from_le_bytes(b))
    }

    /// Byte range `[start, end)` of geometry chunk `chunk_id` in `l`, resolved through the LOD's
    /// per-chunk offset table (spec §5). `offsets[k]` and `offsets[k+1]` are adjacent, so this is a
    /// single 8-byte read — routed through the **index** block cache, the same one `read_node`
    /// uses: the table is index-like metadata and lies immediately after the index, so a walk's
    /// node reads and its chunk lookups share blocks instead of hitting the card twice.
    ///
    /// `chunk_id` is unvalidated file data, so the pair is checked before it can address anything:
    /// in range, monotonic, inside the region [`parse_lod_table`] bounded, and no longer than the
    /// LOD's declared `chunk_size` (nor the decode scratch). A corrupt table therefore yields
    /// [`MapReadError::Malformed`], never an out-of-region read.
    ///
    /// The cache borrow is taken and **released here**, before the caller borrows it again for
    /// `load_chunk` — the same read-before-you-need-it discipline as `read_node` in `walk_leaves`.
    fn chunk_range(&self, l: &Lod, chunk_id: u32) -> Result<(usize, usize), MapReadError> {
        if !self.cache_ready {
            return Err(MapReadError::Cache(CacheError::Busy));
        }
        let entry = l.offset_entry(chunk_id).ok_or(MapReadError::Malformed)?;
        let entry = u32::try_from(entry).map_err(|_| MapReadError::Malformed)?;
        let mut b = [0u8; 8];
        self.cache
            .try_borrow_mut()
            .map_err(MapReadError::Cache)?
            .index_read(self.src, self.file, entry, &mut b)
            .map_err(MapReadError::Source)?;
        let (off0, off1) = (rd_u32(&b, 0) as usize, rd_u32(&b, 4) as usize);
        if off1 < off0 || off1 > l.chunk_bytes_total {
            return Err(MapReadError::Malformed);
        }
        let len = off1 - off0;
        if len > l.chunk_size || len > MAX_CHUNK_BYTES {
            return Err(MapReadError::Malformed);
        }
        let start = l.data_start().and_then(|d| d.checked_add(off0)).ok_or(MapReadError::Malformed)?;
        let end = start.checked_add(len).ok_or(MapReadError::Malformed)?;
        if end > self.src.len() as usize {
            return Err(MapReadError::Malformed);
        }
        Ok((start, end))
    }

    /// Visit `(chunk_id, node_bbox)` for every non-empty leaf in `lod` overlapping `view`, in
    /// quadtree order. `lod` indexes [`Reader::lods`]; out-of-range visits nothing. Unlike a
    /// capacity-bounded collect, this streams through a callback with **no upper bound** on the
    /// chunk count — the renderer relies on this so a wide viewport never silently drops chunks.
    /// The walk only reads the index (bbox tests over `u32` nodes), so re-running it for the second
    /// (pass B) traversal is cheap relative to decoding.
    pub fn for_each_chunk(
        &self,
        lod: usize,
        view: &BBox,
        mut visit: impl FnMut(u32, BBox),
    ) -> Result<(), MapReadError> {
        let Some(l) = self.lods().get(lod) else {
            return Ok(());
        };
        if l.node_count == 0 {
            return Ok(());
        }
        let Some(query) = intersect_bbox(view, &self.bbox) else {
            return Ok(());
        };
        if !self.cache_ready {
            return Err(MapReadError::Cache(CacheError::Busy));
        }

        // A successful prior expanded walk is a complete ordered leaf list for every query wholly
        // inside its cover. Copy it out before invoking the callback: a callback loads geometry and
        // therefore borrows this same RefCell.
        let cached =
            self.cache.try_borrow_mut().map_err(MapReadError::Cache)?.cached_walk(self.file, lod as u8, &query);
        if let Some(entries) = cached {
            for entry in entries {
                if entry.node.intersects(&query) {
                    visit(entry.cid, entry.node);
                }
            }
            return Ok(());
        }

        let cover = expand_walk_bbox(&query, &self.bbox);
        let mut entries: Vec<WalkEntry, WALK_CACHE_ENTRIES> = Vec::new();
        let mut cacheable = true;
        self.walk_geometry_prefetch(l, 0, self.bbox, &query, &cover, 0, &mut entries, &mut cacheable, &mut visit)?;
        if cacheable {
            self.cache.try_borrow_mut().map_err(MapReadError::Cache)?.store_walk(self.file, lod as u8, cover, &entries);
        }
        Ok(())
    }

    /// Geometry-only walk that opportunistically explores `cover`, but preserves the exact
    /// `primary` query's behavior. Once the twelve-entry result budget overflows, subsequent
    /// recursion immediately shrinks back to `primary`; errors found solely in the speculative
    /// margin likewise abandon caching rather than failing a query that never touched that node.
    /// Leaves in `primary` are always streamed once and in ordinary quadtree order.
    #[allow(clippy::too_many_arguments)]
    fn walk_geometry_prefetch<F: FnMut(u32, BBox)>(
        &self,
        index: &dyn QuadIndex,
        idx: usize,
        node: BBox,
        primary: &BBox,
        cover: &BBox,
        depth: u32,
        entries: &mut Vec<WalkEntry, WALK_CACHE_ENTRIES>,
        cacheable: &mut bool,
        visit: &mut F,
    ) -> Result<(), MapReadError> {
        let target = if *cacheable { cover } else { primary };
        if idx >= index.node_count() || depth > MAX_QUADTREE_DEPTH || !node.intersects(target) {
            return Ok(());
        }
        let val = match self.read_node(index, idx) {
            Ok(val) => val,
            Err(error) if node.intersects(primary) => return Err(error),
            Err(_) => {
                *cacheable = false;
                return Ok(());
            }
        };
        if val & BRANCH_BIT == 0 {
            if val != EMPTY_LEAF {
                if *cacheable && entries.push(WalkEntry { cid: val, node }).is_err() {
                    *cacheable = false;
                }
                if node.intersects(primary) {
                    visit(val, node);
                }
            }
            return Ok(());
        }
        let child = (val & !BRANCH_BIT) as usize;
        if child <= idx {
            if node.intersects(primary) {
                return Err(MapReadError::Malformed);
            }
            *cacheable = false;
            return Ok(());
        }
        let mid_lon = (node.min_lon + node.max_lon).div_euclid(2);
        let mid_lat = (node.min_lat + node.max_lat).div_euclid(2);
        let kids = [
            BBox { min_lon: node.min_lon, min_lat: mid_lat, max_lon: mid_lon, max_lat: node.max_lat },
            BBox { min_lon: mid_lon, min_lat: mid_lat, max_lon: node.max_lon, max_lat: node.max_lat },
            BBox { min_lon: node.min_lon, min_lat: node.min_lat, max_lon: mid_lon, max_lat: mid_lat },
            BBox { min_lon: mid_lon, min_lat: node.min_lat, max_lon: node.max_lon, max_lat: mid_lat },
        ];
        for (i, kb) in kids.iter().enumerate() {
            self.walk_geometry_prefetch(index, child + i, *kb, primary, cover, depth + 1, entries, cacheable, visit)?;
        }
        Ok(())
    }

    /// Visit `(chunk_id, node_bbox)` for every non-empty leaf of a [`QuadIndex`] overlapping `view`,
    /// walking the flat `u32` tree over the header's global `bbox` (§4/§7.2). Shared by the geometry
    /// [`Reader::for_each_chunk`] and the POI query — both indexes use the identical node encoding
    /// and subdivision, so one implementation serves both. `index` is `&dyn` so the two call sites
    /// don't monomorphize the (recursive) walk twice.
    fn walk_leaves<F: FnMut(u32, BBox)>(
        &self,
        index: &dyn QuadIndex,
        idx: usize,
        node: BBox,
        view: &BBox,
        depth: u32,
        visit: &mut F,
    ) -> Result<(), MapReadError> {
        // The depth cap is the hard stack bound against a corrupt cyclic branch (see
        // `MAX_QUADTREE_DEPTH`); a well-formed tree never reaches it.
        if idx >= index.node_count() || depth > MAX_QUADTREE_DEPTH || !node.intersects(view) {
            return Ok(());
        }
        // Read the node *before* descending/visiting so the index-cache borrow is released by the
        // time a leaf's `visit` triggers a geometry-chunk read (no nested `RefCell` borrow).
        let val = self.read_node(index, idx)?;
        if val & BRANCH_BIT == 0 {
            if val != EMPTY_LEAF {
                visit(val, node);
            }
            return Ok(());
        }
        let child = (val & !BRANCH_BIT) as usize;
        // The packer flattens the quadtree breadth-first, so a branch's children always lie after
        // it: `child > idx` is a well-formed-map invariant. A back-/self-reference (`child <= idx`)
        // only appears in a corrupt map and would re-enter a node already on the stack; reject it
        // (the depth cap above is the backstop, this stops the most direct cycle at its source).
        if child <= idx {
            return Err(MapReadError::Malformed);
        }
        // Floor-division midpoints (`div_euclid` floors toward −∞) — must match the packer's
        // `quadtree.rs` split so reader and writer agree on every node bbox.
        let mid_lon = (node.min_lon + node.max_lon).div_euclid(2);
        let mid_lat = (node.min_lat + node.max_lat).div_euclid(2);
        // NW, NE, SW, SE
        let kids = [
            BBox { min_lon: node.min_lon, min_lat: mid_lat, max_lon: mid_lon, max_lat: node.max_lat },
            BBox { min_lon: mid_lon, min_lat: mid_lat, max_lon: node.max_lon, max_lat: node.max_lat },
            BBox { min_lon: node.min_lon, min_lat: node.min_lat, max_lon: mid_lon, max_lat: mid_lat },
            BBox { min_lon: mid_lon, min_lat: node.min_lat, max_lon: node.max_lon, max_lat: mid_lat },
        ];
        for (i, kb) in kids.iter().enumerate() {
            self.walk_leaves(index, child + i, *kb, view, depth + 1, visit)?;
        }
        Ok(())
    }

    /// Decode every feature in a chunk of `lod`, invoking `visit` once per feature with a
    /// [`FeatureRef`] borrowing the caller's `points`/`ring_lens` scratch. Allocation-free: the
    /// buffers grow to the largest feature once and are reused across features/chunks/frames.
    /// `node` is the leaf bbox yielded by [`Reader::for_each_chunk`].
    ///
    /// # Reentrancy
    ///
    /// The internal cache borrow is held while `visit` runs. Resident-table calls remain available;
    /// a nested streaming call returns [`MapReadError::Cache`] instead of panicking.
    pub fn for_each_feature<const P: usize, const R: usize>(
        &self,
        lod: usize,
        chunk_id: u32,
        node: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        visit: impl FnMut(FeatureRef),
    ) -> Result<DecodeStatus, MapReadError> {
        self.for_each_feature_filtered(lod, chunk_id, node, points, ring_lens, |_| true, visit)
    }

    /// Like [`Reader::for_each_feature`], but `should_decode` is consulted with each feature's
    /// style id **before** its coordinates are decoded: `false` skips the geometry cheaply
    /// (advancing past its bytes with no coordinate math), `true` decodes it and hands a
    /// [`FeatureRef`] to `visit`. The renderer uses this so a collect traversal decodes only the
    /// features it needs — pass B decodes just the **selected winners** and skips everything else
    /// cheaply, advancing past their bytes with no coordinate math.
    ///
    /// # Reentrancy
    ///
    /// The internal cache borrow is held while `should_decode` and `visit` run. Resident-table calls
    /// remain available; a nested streaming call returns [`MapReadError::Cache`].
    #[allow(clippy::too_many_arguments)]
    pub fn for_each_feature_filtered<const P: usize, const R: usize>(
        &self,
        lod: usize,
        chunk_id: u32,
        node: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        should_decode: impl Fn(u8) -> bool,
        visit: impl FnMut(FeatureRef),
    ) -> Result<DecodeStatus, MapReadError> {
        let l = match self.lods().get(lod) {
            Some(l) => l,
            None => return Err(MapReadError::Malformed),
        };
        // Resolve the chunk's extent from the offset table first — that read borrows the cache, so
        // it must finish before the `load_chunk` borrow below. `chunk_range` validates the pair
        // (range, monotonicity, `chunk_size`, `MAX_CHUNK_BYTES`, file length) so nothing here can
        // index past the decode scratch or the file.
        let (start, end) = self.chunk_range(l, chunk_id)?;
        let len = end - start;
        // Pull the chunk through the cache, then decode from the resident bytes. The borrow is held
        // across `decode_chunk_into` — safe because `should_decode`/`visit` only touch
        // `self.tables.styles`, never the cache.
        if !self.cache_ready {
            return Err(MapReadError::Cache(CacheError::Busy));
        }
        let mut cache = self.cache.try_borrow_mut().map_err(MapReadError::Cache)?;
        let loc = match cache.load_chunk(self.src, self.file, lod as u8, chunk_id, start as u32, len, node) {
            Ok(loc) => loc,
            Err(error) => return Err(MapReadError::Source(error)),
        };
        let chunk = match loc {
            ChunkLoc::Slot(i) => &cache.chunks[i].buf[..len],
            ChunkLoc::Scratch => &cache.scratch[..len],
        };
        Ok(decode_chunk_into(chunk, node, points, ring_lens, should_decode, visit))
    }

    /// Decode exactly the feature at byte `offset` within chunk `cid` of `lod`, into the caller's
    /// `points`/`ring_lens` scratch, returning its [`FeatureRef`]. The renderer's two-phase collect
    /// (issue #564) uses this in pass B to re-materialize a *winning* feature's geometry — one it
    /// selected in pass A by a lightweight stub ([`FeatureRef::offset`]) — without re-decoding the
    /// rest of the chunk.
    ///
    /// `node` is the leaf bbox [`Reader::for_each_chunk`] yields for `cid` (the per-feature anchor
    /// base). `offset` came from a [`FeatureRef::offset`] earlier this frame, but it is still
    /// validated against the chunk length and the `0xFF` end-marker, so a stale/corrupt offset
    /// yields [`FeatureReadError::Decode`], never a panic or an out-of-chunk read. Fetches the chunk
    /// through the same cache as the full walk, so consecutive calls for one `cid` (pass B visits a
    /// chunk's winners together) hit the resident slot instead of re-reading it.
    ///
    /// # Reentrancy
    ///
    /// Same rule as [`Reader::for_each_feature_filtered`]: this borrows the internal cache for the
    /// fetch + decode. Calling it from [`Reader::for_each_chunk`] is the normal pass-B path; legal
    /// re-entry from a `for_each_feature*` callback returns a typed cache error.
    pub fn decode_feature_at<'p, const P: usize, const R: usize>(
        &self,
        lod: usize,
        cid: u32,
        offset: usize,
        node: &BBox,
        points: &'p mut Vec<(i32, i32), P>,
        ring_lens: &'p mut Vec<usize, R>,
    ) -> Result<FeatureRef<'p>, FeatureReadError> {
        // Every error leaves caller scratch empty. Besides making retries deterministic, this is
        // the public expression of the whole-feature contract: no stale geometry from a previous
        // success and no prefix decoded before a malformed hole may escape through these buffers.
        points.clear();
        ring_lens.clear();
        let l = self.lods().get(lod).ok_or(FeatureReadError::Decode(FeatureDecodeError::Malformed))?;
        // Same offset-table lookup + validation as the full walk (and the same borrow-then-release
        // ordering ahead of `load_chunk`); a read failure there is a read failure here.
        let (start, end) = match self.chunk_range(l, cid) {
            Ok(range) => range,
            Err(MapReadError::Malformed) => return Err(FeatureReadError::Decode(FeatureDecodeError::Malformed)),
            Err(error) => return Err(FeatureReadError::Read(error)),
        };
        let len = end - start;
        // The re-decode offset must additionally land inside *this* chunk — `len` now comes from the
        // offset table, so a stale offset from a differently-sized chunk is rejected here.
        if offset >= len {
            return Err(FeatureReadError::Decode(FeatureDecodeError::Malformed));
        }
        if !self.cache_ready {
            return Err(FeatureReadError::Read(MapReadError::Cache(CacheError::Busy)));
        }
        let mut cache =
            self.cache.try_borrow_mut().map_err(|error| FeatureReadError::Read(MapReadError::Cache(error)))?;
        let loc = cache
            .load_chunk(self.src, self.file, lod as u8, cid, start as u32, len, node)
            .map_err(|error| FeatureReadError::Read(MapReadError::Source(error)))?;
        let chunk = match loc {
            ChunkLoc::Slot(i) => &cache.chunks[i].buf[..len],
            ChunkLoc::Scratch => &cache.scratch[..len],
        };
        // The `FeatureRef` borrows `points`/`ring_lens` (its coordinates are copied there), not the
        // cache bytes, so it outlives the `cache` borrow dropped at return.
        match decode_one_feature(chunk, offset, node, points, ring_lens) {
            DecodeOne::Complete(fref, _) => Ok(fref),
            DecodeOne::Dropped(error, _) => Err(FeatureReadError::Decode(error)),
        }
    }

    /// Decode a pass-A feature straight from its resident geometry slot, without re-walking the
    /// quadtree or re-reading the chunk-offset table. The renderer's pass B asks for features that
    /// pass A selected only moments earlier; when the chunk survived the four-slot cache, its leaf
    /// bbox is resident beside the bytes and is the complete anchor needed by `decode_one_feature`.
    ///
    /// `Ok(None)` is an ordinary cache miss (the caller may fall back to the index walk). Errors
    /// retain the same typed cache/decode behavior as [`Reader::decode_feature_at`].
    pub(crate) fn decode_cached_feature_at<'p, const P: usize, const R: usize>(
        &self,
        lod: usize,
        cid: u32,
        offset: usize,
        points: &'p mut Vec<(i32, i32), P>,
        ring_lens: &'p mut Vec<usize, R>,
    ) -> Result<Option<FeatureRef<'p>>, FeatureReadError> {
        if !self.cache_ready {
            return Err(FeatureReadError::Read(MapReadError::Cache(CacheError::Busy)));
        }
        let mut cache =
            self.cache.try_borrow_mut().map_err(|error| FeatureReadError::Read(MapReadError::Cache(error)))?;
        let Some(i) = cache
            .chunks
            .iter()
            .position(|slot| slot.valid() && slot.file == self.file && slot.lod() == lod as u8 && slot.cid == cid)
        else {
            return Ok(None);
        };

        points.clear();
        ring_lens.clear();
        let len = cache.chunks[i].len as usize;
        if offset >= len {
            return Err(FeatureReadError::Decode(FeatureDecodeError::Malformed));
        }
        cache.chunk_hits = cache.chunk_hits.saturating_add(1);
        cache.chunks[i].used = 0;
        let slot = &cache.chunks[i];
        match decode_one_feature(&slot.buf[..len], offset, &slot.node, points, ring_lens) {
            DecodeOne::Complete(fref, _) => Ok(Some(fref)),
            DecodeOne::Dropped(error, _) => Err(FeatureReadError::Decode(error)),
        }
    }
}

/// A decoded POI record's scalar fields, before the (lazy) name decode — the value
/// [`consider_poi`] folds into the nearest-16 set.
struct PoiCand {
    lat: i32,
    lon: i32,
    subtype: u8,
    distance_m: u32,
}

/// Fold one decoded record into the sorted nearest-16 `out` set: reject it if the set is full and
/// it's no closer than the current 16th, dedup an already-present `(lat, lon, subtype)` (a record
/// revisited on a wider ring), else insert it in distance order (ties keep the earlier-seen, a
/// stable order). `buf`/`off` locate the record for the **lazy** name decode, which only runs once
/// the record is known to belong in the set.
fn consider_poi(out: &mut Vec<Poi, MAX_POI_RESULTS>, cand: PoiCand, buf: &[u8], off: usize) {
    let PoiCand { lat, lon, subtype, distance_m } = cand;
    // Cheap rejection before any dedup scan or name decode: a full set whose worst is closer.
    if out.len() == MAX_POI_RESULTS && distance_m >= out[MAX_POI_RESULTS - 1].distance_m {
        return;
    }
    // Dedup: the same POI reappears on every wider ring. Key on (lat, lon, subtype).
    if out.iter().any(|p| p.lat == lat && p.lon == lon && p.subtype == subtype) {
        return;
    }
    // Insertion index: first slot whose distance is strictly greater (so equal distances keep
    // insertion order — a stable, deterministic tie-break).
    let at = out.iter().position(|p| p.distance_m > distance_m).unwrap_or(out.len());
    // If the set is full, drop the current last to make room (its distance is > this one, since the
    // cheap-reject above let this through).
    if out.len() == MAX_POI_RESULTS {
        let _ = out.pop();
    }
    // The record's `hours_ref` at `[off+34 .. off+36]` (§7.3); carried so the detail screen can
    // resolve the pooled schedule without a re-query. The scan window always holds a whole record
    // (`take * POI_RECORD_LEN`), so these two bytes are in-bounds.
    let hours_ref = rd_u16(buf, off + 34);
    let poi = Poi { lat, lon, subtype, name: decode_poi_name(buf, off), hours_ref, distance_m };
    // `at` is a valid index in `0..=out.len()` and the set has room now; the insert can't fail.
    let _ = out.insert(at, poi);
}

/// A projected POI record's scalar fields, before the (lazy) name decode — the value
/// [`consider_corridor_poi`] folds into the corridor set.
struct CorridorCand {
    lat: i32,
    lon: i32,
    subtype: u8,
    dist_along_m: u32,
    offset_m: i32,
    /// Along-route distance still to go from the query's progress anchor (what the row shows).
    to_go_m: u32,
}

/// Fold one projected record into the along-route-sorted corridor set.
///
/// Order of business, cheapest-first but **dedupe before the capacity reject** so a POI already held
/// can still improve its projection when the set is full:
///
/// 1. an already-held `(lat, lon, subtype)` keeps its **nearest** projection — a strictly smaller
///    `|offset_m|` removes the old entry so the better one re-inserts in its new order slot, an
///    equal-or-worse one is dropped (this is the switchback dedupe across chunks);
/// 2. a full set whose farthest entry is already nearer rejects the candidate;
/// 3. otherwise insert in ascending `dist_along_m`, evicting the farthest when full. Ties keep the
///    earlier-seen entry, so the order is stable and deterministic.
///
/// `buf`/`off` locate the record for the **lazy** name decode, which only runs once the record is
/// known to belong in the set.
fn consider_corridor_poi(out: &mut Vec<CorridorPoi, MAX_CORRIDOR_RESULTS>, cand: CorridorCand, buf: &[u8], off: usize) {
    let CorridorCand { lat, lon, subtype, dist_along_m, offset_m, to_go_m } = cand;
    if let Some(at) = out.iter().position(|c| c.poi.lat == lat && c.poi.lon == lon && c.poi.subtype == subtype) {
        if offset_m.abs() >= out[at].offset_m.abs() {
            return; // already held at an equal or nearer projection
        }
        out.remove(at);
    }
    // Cheap rejection: a full set whose farthest entry is already nearer along the route.
    if out.len() == MAX_CORRIDOR_RESULTS && dist_along_m >= out[MAX_CORRIDOR_RESULTS - 1].dist_along_m {
        return;
    }
    // Insertion index: first slot strictly farther along (equal distances keep insertion order).
    let at = out.iter().position(|c| c.dist_along_m > dist_along_m).unwrap_or(out.len());
    if out.len() == MAX_CORRIDOR_RESULTS {
        let _ = out.pop();
    }
    // `hours_ref` at `[off+34 .. off+36]` (§7.3), carried so the detail screen resolves the pooled
    // schedule without a re-query. The scan window always holds a whole record, so this is in bounds.
    let hours_ref = rd_u16(buf, off + 34);
    let poi = Poi { lat, lon, subtype, name: decode_poi_name(buf, off), hours_ref, distance_m: to_go_m };
    // `at` is a valid index in `0..=out.len()` and the set has room now; the insert can't fail.
    let _ = out.insert(at, CorridorPoi { poi, dist_along_m, offset_m });
}

/// Decode a POI record's name (spec §7.3) from `buf` at record offset `off`: `name_len` at `off+9`,
/// the up-to-24-byte `Name` at `off+10` (bytes `[off+10 .. off+34]`; `hours_ref` follows at
/// `[off+34 .. off+36]`). Empty for an unnamed record (`name_len == 0`). The stored name is already
/// pre-folded printable ASCII, but this stays defensive — `name_len` is clamped to what the field
/// and the buffer hold, and any non-printable byte (a corrupt record) is dropped — so a bad chunk
/// yields a short/empty name, never a panic or garbage glyph.
fn decode_poi_name(buf: &[u8], off: usize) -> heapless::String<POI_NAME_LEN> {
    let mut name = heapless::String::new();
    let name_off = off + 10;
    // Clamp to the 24-byte field and to the bytes actually present in the buffer.
    let len = (buf[off + 9] as usize).min(POI_NAME_LEN).min(buf.len().saturating_sub(name_off));
    for &b in &buf[name_off..name_off + len] {
        // Printable ASCII only (the device font's range); drop anything else rather than trust a
        // corrupt byte. `push` can't fail — `len <= POI_NAME_LEN` == the String capacity.
        if (0x20..=0x7E).contains(&b) {
            let _ = name.push(b as char);
        }
    }
    name
}

/// Running vertex bounds, accumulated as a feature decodes so its bbox is ready
/// with no extra pass over the points. Seeded inverted; widened per vertex.
#[derive(Clone, Copy)]
struct Bounds {
    min_lon: i32,
    min_lat: i32,
    max_lon: i32,
    max_lat: i32,
}

impl Bounds {
    #[inline]
    fn new() -> Self {
        Bounds { min_lon: i32::MAX, min_lat: i32::MAX, max_lon: i32::MIN, max_lat: i32::MIN }
    }
    #[inline]
    fn add(&mut self, x: i32, y: i32) {
        if x < self.min_lon {
            self.min_lon = x;
        }
        if x > self.max_lon {
            self.max_lon = x;
        }
        if y < self.min_lat {
            self.min_lat = y;
        }
        if y > self.max_lat {
            self.max_lat = y;
        }
    }
    #[inline]
    fn to_bbox(self) -> BBox {
        BBox { min_lon: self.min_lon, min_lat: self.min_lat, max_lon: self.max_lon, max_lat: self.max_lat }
    }
}

/// Walk a single chunk's bytes, decoding each feature into the shared `points`/`ring_lens` buffers
/// (cleared and refilled per feature) and handing a [`FeatureRef`] to `visit`. A feature whose
/// style `should_decode` rejects is skipped without decoding — its geometry bytes are advanced past
/// with no coordinate math ([`skip_ring`] mirrors [`read_ring`]'s offset arithmetic exactly, so the
/// two stay byte-for-byte in sync). Accepted features decode through [`decode_one_feature`], the
/// same path [`Reader::decode_feature_at`] takes, so a feature decodes identically either way.
fn decode_chunk_into<const P: usize, const R: usize>(
    chunk: &[u8],
    node: &BBox,
    points: &mut Vec<(i32, i32), P>,
    ring_lens: &mut Vec<usize, R>,
    should_decode: impl Fn(u8) -> bool,
    mut visit: impl FnMut(FeatureRef),
) -> DecodeStatus {
    let cs = chunk.len();
    let mut off = 0usize;
    let mut status = DecodeStatus::default();
    // A chunk is `features ++ [CHUNK_END]` (§5) — exactly one sentinel, no padding. Running off the
    // end without meeting it means the chunk was truncated or its offset-table length is wrong, so
    // the walk owes the caller a malformed drop rather than a silent clean finish. `verdict` also
    // covers the paths that already reported one and consumed the rest of the chunk.
    let mut verdict = false;

    while off < cs {
        if chunk[off] == CHUNK_END {
            verdict = true;
            break;
        }
        let style_id = chunk[off];

        // Skip path: the caller doesn't want this style this pass, so advance past the geometry
        // without decoding (read only the header fields the skip needs).
        if !should_decode(style_id) {
            match skip_feature(chunk, off) {
                Ok(next) => off = next,
                Err(error) => {
                    // A filtered feature still participates in the public whole-feature scratch
                    // contract. If its framing is malformed, discard geometry left by a previous
                    // selected feature (or stale caller prefill) before reporting the drop.
                    points.clear();
                    ring_lens.clear();
                    status.dropped(error);
                    verdict = true;
                    break;
                }
            }
            continue;
        }

        match decode_one_feature(chunk, off, node, points, ring_lens) {
            DecodeOne::Complete(fref, next) => {
                visit(fref);
                status.complete = status.complete.saturating_add(1);
                off = next;
            }
            DecodeOne::Dropped(FeatureDecodeError::Malformed, _) => {
                // Malformed framing consumes the whole rest of the chunk (there is no trustworthy
                // next offset), so this drop *is* the chunk's verdict — don't also charge it for a
                // missing sentinel it never got to.
                status.dropped(FeatureDecodeError::Malformed);
                verdict = true;
                break;
            }
            DecodeOne::Dropped(error, next) => {
                status.dropped(error);
                off = next;
            }
        }
    }
    if !verdict {
        status.dropped(FeatureDecodeError::Malformed);
    }
    status
}

/// One parsed §5 feature header, in either layout, plus the header's own `len` so the caller
/// knows where the deltas start. Built only by [`read_feature_header`], which is the single place
/// that decides what "malformed framing" means — the decode and the skip path share it, so they can
/// never disagree about which byte a feature ends on.
struct FeatHeader {
    style_id: u8,
    flags: u8,
    ext_pt_count: usize,
    /// Anchor, leaf-relative µdeg. Compact headers zero-extend their `uint16` fields; wide ones
    /// carry the full `int32`.
    ax: i32,
    ay: i32,
    len: usize,
}

/// Read the feature header at `off`, or `None` for every malformed-framing case: the end-of-features
/// sentinel, a header running past the chunk, an unknown flag bit, `pt_count == 0`, or holes on a
/// line. `style` + `flags` are the fixed 2-byte prefix of both layouts, and the `WIDE` bit lives in
/// `flags` — so the width is known before any field behind it is needed (v10's trailing flags byte
/// made that impossible, which is why the layout was reordered).
#[inline]
fn read_feature_header(chunk: &[u8], off: usize) -> Option<FeatHeader> {
    if off.checked_add(2).is_none_or(|end| end > chunk.len()) || chunk[off] == CHUNK_END {
        return None;
    }
    let style_id = chunk[off];
    let flags = chunk[off + 1];
    if flags & !(FEATURE_FLAG_16BIT | FEATURE_FLAG_POLYGON | FEATURE_FLAG_HOLES | FEATURE_FLAG_WIDE) != 0 {
        return None;
    }
    let wide = flags & FEATURE_FLAG_WIDE != 0;
    let len = if wide { FEATURE_HEADER_WIDE_LEN } else { FEATURE_HEADER_COMPACT_LEN };
    if off.checked_add(len).is_none_or(|end| end > chunk.len()) {
        return None;
    }
    let (ext_pt_count, ax, ay) = if wide {
        (rd_u16(chunk, off + 2) as usize, rd_i32(chunk, off + 4), rd_i32(chunk, off + 8))
    } else {
        // Compact anchors are **unsigned** — zero-extended, not sign-extended: the packer only picks
        // this layout when both fit `0..=65535`.
        (chunk[off + 2] as usize, rd_u16(chunk, off + 3) as i32, rd_u16(chunk, off + 5) as i32)
    };
    if ext_pt_count == 0 || (flags & FEATURE_FLAG_HOLES != 0 && flags & FEATURE_FLAG_POLYGON == 0) {
        return None;
    }
    Some(FeatHeader { style_id, flags, ext_pt_count, ax, ay, len })
}

/// Decode the single feature whose header starts at `off` in `chunk`, into `points`/
/// `ring_lens` (cleared first), returning its [`FeatureRef`] (borrowing those buffers, with
/// [`FeatureRef::offset`] set to `off`) plus the offset just past it. A malformed/capacity result
/// also leaves both buffers empty, so it is safe to call with an untrusted `off` (issue #564's
/// pass-B re-decode hands back a `FeatureRef::offset` from earlier this frame). `node` gives the
/// leaf's min corner, the per-feature anchor base. This is the exact decode
/// [`decode_chunk_into`] runs, so a feature decodes byte-for-byte identically whether it comes from
/// the full-chunk walk or from [`Reader::decode_feature_at`].
enum DecodeOne<'a> {
    Complete(FeatureRef<'a>, usize),
    Dropped(FeatureDecodeError, usize),
}

fn decode_one_feature<'b, const P: usize, const R: usize>(
    chunk: &[u8],
    off: usize,
    node: &BBox,
    points: &'b mut Vec<(i32, i32), P>,
    ring_lens: &'b mut Vec<usize, R>,
) -> DecodeOne<'b> {
    points.clear();
    ring_lens.clear();
    let head = match read_feature_header(chunk, off) {
        Some(head) => head,
        None => return dropped_feature(points, ring_lens, FeatureDecodeError::Malformed, chunk.len()),
    };
    let FeatHeader { style_id, flags, ext_pt_count, ax, ay, len } = head;
    let feat_off = off;
    let mut off = off + len;

    let is_16 = flags & FEATURE_FLAG_16BIT != 0;
    let is_poly = flags & FEATURE_FLAG_POLYGON != 0;
    let has_holes = flags & FEATURE_FLAG_HOLES != 0;
    let dsize = if is_16 { 2 } else { 1 };

    let anchor = (node.min_lon.wrapping_add(ax), node.min_lat.wrapping_add(ay));

    let mut bounds = Bounds::new();

    let mut failure = None;
    let ext_end = match ring_end(chunk, off, ext_pt_count, false, dsize) {
        Some(end) => end,
        None => return dropped_feature(points, ring_lens, FeatureDecodeError::Malformed, chunk.len()),
    };
    if ext_pt_count > points.capacity() {
        failure = Some(FeatureDecodeError::Capacity(CapacityError::Points));
    } else if ring_lens.is_full() {
        failure = Some(FeatureDecodeError::Capacity(CapacityError::Rings));
    } else {
        read_ring(chunk, off, ext_pt_count, anchor, is_16, false, points, &mut bounds);
        ring_lens.push(ext_pt_count).unwrap();
    }
    off = ext_end;

    if is_poly && has_holes {
        let hole_count = match chunk.get(off) {
            Some(count) => *count as usize,
            None => return dropped_feature(points, ring_lens, FeatureDecodeError::Malformed, chunk.len()),
        };
        off += 1;
        for _ in 0..hole_count {
            if off.checked_add(2).is_none_or(|end| end > chunk.len()) {
                return dropped_feature(points, ring_lens, FeatureDecodeError::Malformed, chunk.len());
            }
            let hpc = rd_u16(chunk, off) as usize;
            off += 2;
            if hpc == 0 {
                return dropped_feature(points, ring_lens, FeatureDecodeError::Malformed, chunk.len());
            }
            let end = match ring_end(chunk, off, hpc, true, dsize) {
                Some(end) => end,
                None => return dropped_feature(points, ring_lens, FeatureDecodeError::Malformed, chunk.len()),
            };
            if failure.is_none() {
                if hpc > points.capacity() - points.len() {
                    failure = Some(FeatureDecodeError::Capacity(CapacityError::Points));
                } else if ring_lens.is_full() {
                    failure = Some(FeatureDecodeError::Capacity(CapacityError::Rings));
                } else {
                    read_ring(chunk, off, hpc, anchor, is_16, true, points, &mut bounds);
                    ring_lens.push(hpc).unwrap();
                }
            }
            off = end;
        }
    }

    if let Some(error) = failure {
        return dropped_feature(points, ring_lens, error, off);
    }

    let fref = FeatureRef {
        style_id,
        kind: if is_poly { Kind::Polygon } else { Kind::Line },
        points,
        ring_lens,
        bbox: bounds.to_bbox(),
        offset: feat_off,
    };
    DecodeOne::Complete(fref, off)
}

#[inline]
fn dropped_feature<'a, const P: usize, const R: usize>(
    points: &mut Vec<(i32, i32), P>,
    ring_lens: &mut Vec<usize, R>,
    error: FeatureDecodeError,
    next: usize,
) -> DecodeOne<'a> {
    points.clear();
    ring_lens.clear();
    DecodeOne::Dropped(error, next)
}

fn ring_end(chunk: &[u8], off: usize, pt_count: usize, is_hole: bool, dsize: usize) -> Option<usize> {
    if pt_count == 0 {
        return None;
    }
    let num_deltas = if is_hole { pt_count } else { pt_count - 1 };
    let bytes = num_deltas.checked_mul(dsize.checked_mul(2)?)?;
    let end = off.checked_add(bytes)?;
    (end <= chunk.len()).then_some(end)
}

fn skip_feature(chunk: &[u8], off: usize) -> Result<usize, FeatureDecodeError> {
    let head = read_feature_header(chunk, off).ok_or(FeatureDecodeError::Malformed)?;
    let FeatHeader { flags, ext_pt_count, len, .. } = head;
    let is_poly = flags & FEATURE_FLAG_POLYGON != 0;
    let has_holes = flags & FEATURE_FLAG_HOLES != 0;
    let dsize = if flags & FEATURE_FLAG_16BIT != 0 { 2 } else { 1 };
    let mut next = ring_end(chunk, off + len, ext_pt_count, false, dsize).ok_or(FeatureDecodeError::Malformed)?;
    if is_poly && has_holes {
        let hole_count = *chunk.get(next).ok_or(FeatureDecodeError::Malformed)? as usize;
        next += 1;
        for _ in 0..hole_count {
            if next.checked_add(2).is_none_or(|end| end > chunk.len()) {
                return Err(FeatureDecodeError::Malformed);
            }
            let hpc = rd_u16(chunk, next) as usize;
            next += 2;
            next = ring_end(chunk, next, hpc, true, dsize).ok_or(FeatureDecodeError::Malformed)?;
        }
    }
    Ok(next)
}

#[allow(clippy::too_many_arguments)]
fn read_ring<const P: usize>(
    chunk: &[u8],
    mut off: usize,
    pt_count: usize,
    anchor: (i32, i32),
    is_16: bool,
    is_hole: bool,
    out: &mut Vec<(i32, i32), P>,
    bounds: &mut Bounds,
) {
    let (mut px, mut py) = anchor;
    let num_deltas = if is_hole {
        // holes store all points as deltas (first relative to anchor)
        pt_count
    } else {
        out.push(anchor).unwrap();
        bounds.add(anchor.0, anchor.1);
        pt_count - 1
    };
    for _ in 0..num_deltas {
        let (dx, dy) = if is_16 {
            (rd_i16(chunk, off) as i32, rd_i16(chunk, off + 2) as i32)
        } else {
            (chunk[off] as i8 as i32, chunk[off + 1] as i8 as i32)
        };
        off += if is_16 { 4 } else { 2 };
        px = px.wrapping_add(dx);
        py = py.wrapping_add(dy);
        out.push((px, py)).unwrap();
        bounds.add(px, py);
    }
}

/// Parse the style table, read resident from `src` at `style_offset` (file is `total` bytes) into
/// the caller's `styles` (cleared first). The table is small (≤ `1 + 256*8` bytes) so it's pulled
/// in two reads (count byte, then records). A truncated *table* is tolerated — the `o + 8 > want`
/// break stops at the last whole record rather than reading past it — but a failed *read* (flaky
/// card) or a `style_offset` at/past EOF (corrupt header) is [`Error::BadOffset`]: an all-`None`
/// table would let the map load "fine" and render nothing, with no error to surface.
///
/// Out-param + `inline(never)`, deliberately: with the array in the return value this single-call-
/// site function inlined its several KB of scratch (the 2 KB record buffer plus the `Result` array
/// temporaries) into `MapTables::parse` and on into the device `main`'s **permanent** frame — every
/// stack watermark rose by multiple KB and the DK's ride path overflowed (HardFault). The scratch must
/// stay in a frame that pops before `run_app` starts.
#[inline(never)]
fn parse_styles(
    src: &dyn ByteSource,
    style_offset: usize,
    total: usize,
    styles: &mut [Option<Style>; 256],
) -> Result<(), Error> {
    styles.fill(None);
    // `MapTables::parse`'s header guard admits `style_offset == total`; there is no count byte to
    // read there, so treat it as the corrupt header it is rather than a silently-empty table.
    if style_offset >= total {
        return Err(Error::BadOffset);
    }
    let mut cb = [0u8; 1];
    src.read_at(style_offset as u32, &mut cb).map_err(Error::Source)?;
    let count = cb[0] as usize;
    // `count*8` record bytes follow the count, clamped to what the file holds so the `o + 8 > want`
    // break below stops at the last whole record in a truncated table.
    let avail = total - (style_offset + 1);
    let want = (count * STYLE_RECORD_LEN).min(avail);
    let mut buf = [0u8; 256 * STYLE_RECORD_LEN];
    if want > 0 {
        src.read_at((style_offset + 1) as u32, &mut buf[..want]).map_err(Error::Source)?;
    }
    let mut o = 0usize;
    for _ in 0..count {
        if o + STYLE_RECORD_LEN > want {
            break;
        }
        let id = buf[o];
        let z_index = buf[o + 1] as i8;
        let color = rd_u16(&buf, o + 2);
        let weight = buf[o + 4];
        let flags = buf[o + 5];
        let priority = (flags & STYLE_PRIORITY_MASK) + 1;
        // The two color2 bytes are always present; the flag bit — not a `0x0000` sentinel — decides
        // whether they carry a color (black `0x0000` is a legal secondary color).
        let color2 = if flags & STYLE_HAS_COLOR2_BIT != 0 { Some(rd_u16(&buf, o + 6)) } else { None };
        // #1095: bit 4 takes the style off the width ramp, bit 5 files it under the terrain layer.
        // Bits 6-7 stay reserved and are **ignored**, not rejected (§2) — that reader tolerance is
        // exactly what let these two be defined without a format bump. The wire byte is re-packed
        // into the seam's own [`StyleFlags`] rather than carried through: the table is resident.
        let style_flags = StyleFlags::new(
            flags & STYLE_DASHED_BIT != 0,
            flags & STYLE_FIXED_WIDTH_BIT != 0,
            flags & STYLE_TERRAIN_LAYER_BIT != 0,
        );
        styles[id as usize] = Some(Style { id, z_index, color, weight, priority, flags: style_flags, color2 });
        o += STYLE_RECORD_LEN;
    }
    Ok(())
}

/// Parse the `lod_count` LOD-table entries (resident from `src`); validates each layer's
/// index/table/chunk region lies within the file (`total` bytes) so `for_each_chunk`/`decode_chunk`
/// can skip bounds math, and that its `chunk_size` fits the decode scratch ([`MAX_CHUNK_BYTES`]).
///
/// The offset-table layout costs one extra `uint32` read per LOD: the table's **last** entry is the layer's total
/// chunk bytes ([`Lod::chunk_bytes_total`]), which both bounds the region here and bounds every
/// later per-chunk offset pair in [`Reader::chunk_range`] with no further reads.
pub(crate) fn parse_lod_table(
    src: &dyn ByteSource,
    offset: usize,
    lod_count: usize,
    total: usize,
) -> Result<Vec<Lod, 16>, Error> {
    let mut lods = Vec::new();
    let mut e = [0u8; LOD_ENTRY_LEN];
    for k in 0..lod_count {
        let o = offset + k * LOD_ENTRY_LEN;
        src.read_at(o as u32, &mut e).map_err(Error::Source)?;
        let mut lod = Lod {
            max_mpp: rd_f32(&e, 0),
            index_offset: rd_u32(&e, 4) as usize,
            node_count: rd_u32(&e, 8) as usize,
            chunk_size: rd_u16(&e, 12) as usize,
            chunk_count: rd_u32(&e, 14) as usize,
            chunk_bytes_total: 0,
        };
        // Checked: a corrupt entry's `node_count`/`chunk_count` products can wrap `usize` on the
        // 32-bit target, so an unchecked `data_start` could land below `total` and admit a layer
        // indexing out of the file.
        let data_start = lod.data_start().ok_or(Error::BadOffset)?;
        if lod.index_offset < HEADER_LEN || data_start > total {
            return Err(Error::BadOffset);
        }
        // A chunk decodes into the resident scratch, so reject a `chunk_size` over
        // [`MAX_CHUNK_BYTES`] rather than silently dropping its geometry at render time.
        if lod.chunk_size > MAX_CHUNK_BYTES {
            return Err(Error::BadOffset);
        }
        // `offsets[chunk_count]` sits in the last 4 bytes before the chunk data — in range by the
        // `data_start` guard above, since the table always carries at least this one entry.
        let last = data_start.checked_sub(4).ok_or(Error::BadOffset)?;
        let mut t = [0u8; 4];
        src.read_at(last as u32, &mut t).map_err(Error::Source)?;
        lod.chunk_bytes_total = rd_u32(&t, 0) as usize;
        if data_start.checked_add(lod.chunk_bytes_total).is_none_or(|end| end > total) {
            return Err(Error::BadOffset);
        }
        let _ = lods.push(lod);
    }
    Ok(lods)
}

/// Parse the POI directory (spec §7.1) at `offset` from `src` (file is `total` bytes): the count
/// byte, the shared `chunk_size`, one 13-byte entry per category, then (v7) the `hours_pool_offset
/// u32` + `hours_pool_count u16`. Parse-only — validates the directory layout, each category's
/// index/chunk region, and that the hours-pool region lies in-file, but does **not** walk the trees
/// or decode any blob (the nearest-N query and the P3 (#443) hours lookup do). The directory is
/// always present, so `offset` at/past EOF, a `category_count` past [`POI_MAX_CATEGORIES`], a
/// `chunk_size` past [`POI_MAX_CHUNK_BYTES`], an out-of-file index/chunk region, or an out-of-file
/// hours-pool region is a corrupt header ⇒ [`Error::BadOffset`].
///
/// Every offset/length product is checked (32-bit target): a corrupt `node_count`/`chunk_count`/
/// `hours_pool_count` can wrap `usize`, so the region-end could land below `total` and admit a
/// category (or a pool blob) indexing out of the file — the same overflow guard style as
/// [`parse_lod_table`]/[`Lod::chunk_range`].
fn parse_poi_directory(src: &dyn ByteSource, offset: usize, total: usize) -> Result<PoiDirectory, Error> {
    // The directory header is 3 bytes (count + chunk_size u16); it must fit the file.
    if offset < HEADER_LEN || offset.checked_add(3).is_none_or(|end| end > total) {
        return Err(Error::BadOffset);
    }
    let mut hdr = [0u8; 3];
    src.read_at(offset as u32, &mut hdr).map_err(Error::Source)?;
    let category_count = hdr[0] as usize;
    let chunk_size = rd_u16(&hdr, 1) as usize;
    if category_count > POI_MAX_CATEGORIES || chunk_size > POI_MAX_CHUNK_BYTES {
        return Err(Error::BadOffset);
    }
    // The whole directory (header + entries + the two v7 pool fields) must lie within the file.
    let pool_fields_off = category_count
        .checked_mul(POI_CAT_ENTRY_LEN)
        .and_then(|len| offset.checked_add(3)?.checked_add(len))
        .ok_or(Error::BadOffset)?;
    // 4 (hours_pool_offset u32) + 2 (hours_pool_count u16) trail the per-category entries.
    let dir_end = pool_fields_off.checked_add(6).ok_or(Error::BadOffset)?;
    if dir_end > total {
        return Err(Error::BadOffset);
    }

    let mut entries = Vec::new();
    let mut e = [0u8; POI_CAT_ENTRY_LEN];
    for k in 0..category_count {
        let o = offset + 3 + k * POI_CAT_ENTRY_LEN;
        src.read_at(o as u32, &mut e).map_err(Error::Source)?;
        let entry = PoiCatEntry {
            category_id: e[0],
            index_offset: rd_u32(&e, 1) as usize,
            node_count: rd_u32(&e, 5) as usize,
            chunk_count: rd_u32(&e, 9) as usize,
        };
        // An empty category (node_count 0) still carries an entry; its index/chunk region is
        // zero-length, so only the offset itself needs to be in-file. A populated one must have its
        // whole index + chunk region inside the file — checked, so a corrupt count can't wrap past
        // `total`.
        if entry.node_count > 0 {
            let region_end = entry
                .data_start()
                .and_then(|start| entry.chunk_count.checked_mul(chunk_size).and_then(|len| start.checked_add(len)))
                .ok_or(Error::BadOffset)?;
            if entry.index_offset < HEADER_LEN || region_end > total {
                return Err(Error::BadOffset);
            }
        } else if entry.index_offset > total {
            return Err(Error::BadOffset);
        }
        let _ = entries.push(entry);
    }

    // The two v7 hours-pool directory fields (spec §7.5): the section's absolute offset + blob
    // count. When the count is non-zero, the whole pool region (`count u16` + `count × 29-byte`
    // blobs) must lie in-file — checked, so a corrupt count can't wrap `usize` past `total`. An
    // empty pool (count 0) still validates its 2-byte `count` header lies in-file.
    let mut pf = [0u8; 6];
    src.read_at(pool_fields_off as u32, &mut pf).map_err(Error::Source)?;
    let hours_pool_offset = rd_u32(&pf, 0) as usize;
    let hours_pool_count = rd_u16(&pf, 4) as usize;
    if hours_pool_offset < HEADER_LEN {
        return Err(Error::BadOffset);
    }
    let pool_end = hours_pool_count
        .checked_mul(POI_HOURS_BLOB_LEN)
        .and_then(|blobs| hours_pool_offset.checked_add(2)?.checked_add(blobs))
        .ok_or(Error::BadOffset)?;
    if pool_end > total {
        return Err(Error::BadOffset);
    }

    Ok(PoiDirectory { chunk_size, entries, hours_pool_offset, hours_pool_count })
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
fn parse_nav_directory(src: &dyn ByteSource, offset: usize, total: usize) -> Result<NavDirectory, Error> {
    if offset < HEADER_LEN || offset.checked_add(NAV_DIR_LEN).is_none_or(|end| end > total) {
        return Err(Error::BadOffset);
    }
    let mut d = [0u8; NAV_DIR_LEN];
    src.read_at(offset as u32, &mut d).map_err(Error::Source)?;
    let dir = NavDirectory {
        index_offset: rd_u32(&d, 0) as usize,
        node_count: rd_u32(&d, 4) as usize,
        chunk_count: rd_u32(&d, 8) as usize,
        edge_pool_offset: rd_u32(&d, 12) as usize,
        edge_chunk_count: rd_u32(&d, 16) as usize,
        chunk_size: rd_u16(&d, 20) as usize,
        profile_table_offset: rd_u32(&d, 22) as usize,
        profile_count: d[26] as usize,
        snap_index_offset: rd_u32(&d, 28) as usize,
        snap_node_count: rd_u32(&d, 32) as usize,
        snap_chunk_count: rd_u32(&d, 36) as usize,
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
    if dir.profile_table_offset < HEADER_LEN {
        return Err(Error::BadOffset);
    }
    let profile_end = dir
        .profile_count
        .checked_mul(NAV_PROFILE_LEN)
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
            .and_then(|start| dir.chunk_count.checked_mul(dir.chunk_size).and_then(|len| start.checked_add(len)))
            .ok_or(Error::BadOffset)?;
        if dir.index_offset < HEADER_LEN || region_end > total {
            return Err(Error::BadOffset);
        }
    } else if dir.index_offset > total {
        return Err(Error::BadOffset);
    }
    // Edge pool region.
    if dir.edge_pool_offset < HEADER_LEN {
        return Err(Error::BadOffset);
    }
    let pool_end = dir
        .edge_chunk_count
        .checked_mul(dir.chunk_size)
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
            .and_then(|start| dir.snap_chunk_count.checked_mul(dir.chunk_size).and_then(|len| start.checked_add(len)))
            .ok_or(Error::BadOffset)?;
        if dir.snap_index_offset < HEADER_LEN || region_end > total {
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
fn parse_nav_profiles(
    src: &dyn ByteSource,
    dir: &NavDirectory,
) -> Result<heapless::Vec<MapProfile, NAV_MAX_PROFILES>, Error> {
    let mut out = heapless::Vec::new();
    let mut buf = [0u8; NAV_PROFILE_LEN];
    for i in 0..dir.profile_count {
        let off = dir
            .profile_table_offset
            .checked_add(i.checked_mul(NAV_PROFILE_LEN).ok_or(Error::BadOffset)?)
            .ok_or(Error::BadOffset)?;
        let off = u32::try_from(off).map_err(|_| Error::BadOffset)?;
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

/// A snapshot of the [`Reader`]'s streaming counters: chunk-cache hit/miss tally and raw SD-read
/// overhead (read calls + bytes). The renderer reports the per-frame delta.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Geometry-chunk requests served from a resident slot (no SD read).
    pub chunk_hits: u32,
    /// Geometry-chunk requests that missed and read from the source.
    pub chunk_misses: u32,
    /// Total `read_at` calls to the source (index blocks + chunk fills).
    pub sd_reads: u32,
    /// Total bytes pulled from the source.
    pub bytes_read: u32,
}

/// Where `load_chunk` left a chunk's bytes: a cache slot (cacheable, ≤ `CACHE_SLOT_BYTES`) or the
/// shared decode scratch (an oversized chunk, read uncached).
#[derive(Clone, Copy)]
enum ChunkLoc {
    Slot(usize),
    /// The shared oversized-decode scratch doubles as a fifth 4-KiB cache slot while no oversized
    /// chunk is using it. `load_chunk` invalidates that tag before a larger read.
    Scratch,
}

#[derive(Clone, Copy)]
enum ChunkVictim {
    Slot(usize),
    Scratch,
}

/// One geometry-chunk cache slot: a resident copy of a `chunk_size`-byte chunk, tagged with its
/// `(lod, chunk_id)` and an RRIP prediction in `used`. The validity bit makes the all-zero state a
/// valid *empty* slot, so [`MapCacheInner::new`] can zero-init the whole cache.
const CHUNK_META_LOD_MASK: u8 = 0x0f;
const CHUNK_META_VALID: u8 = 0x80;

#[repr(C)]
struct ChunkSlot {
    cid: u32,
    used: u32,
    /// Leaf anchor captured with the chunk fill. This lets pass B decode a resident winner without
    /// repeating the whole quadtree walk merely to reconstruct the same bbox.
    node: BBox,
    len: u16,
    /// Which mounted file the bytes came from (a volume set's shard index, `0` for a single
    /// map). Part of the key, not decoration: a set shares one cache and one parse generation
    /// across its shards, so `(lod, cid)` alone would cross-serve one shard's chunk for
    /// another's.
    file: u8,
    /// Validity plus the four-bit LOD. Packing the former fields makes each slot four bytes smaller;
    /// across four slots that funds the scratch-cache tag below without growing `MapCache`.
    meta: u8,
    buf: [u8; CACHE_SLOT_BYTES],
}

impl ChunkSlot {
    #[inline]
    fn valid(&self) -> bool {
        self.meta & CHUNK_META_VALID != 0
    }

    #[inline]
    fn lod(&self) -> u8 {
        self.meta & CHUNK_META_LOD_MASK
    }

    #[inline]
    fn commit(&mut self, lod: u8) {
        debug_assert!(lod <= CHUNK_META_LOD_MASK);
        self.meta = CHUNK_META_VALID | (lod & CHUNK_META_LOD_MASK);
    }
}

#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<ChunkSlot>() == CACHE_SLOT_BYTES + 28);

const SCRATCH_META_LOD_MASK: u8 = 0x0f;
const SCRATCH_META_RRPV_SHIFT: u8 = 4;
const SCRATCH_META_VALID: u8 = 0x80;

/// Tag for the fifth cache slot backed by the first 4 KiB of `MapCacheInner::scratch`. It needs no
/// length or bbox: a hit is checked only after `chunk_range` supplied the current length, and the
/// caller already carries the leaf bbox used for decode.
#[repr(C)]
struct ScratchSlot {
    cid: u32,
    file: u8,
    meta: u8,
    _reserved: [u8; 2],
}

impl ScratchSlot {
    #[inline]
    fn valid(&self) -> bool {
        self.meta & SCRATCH_META_VALID != 0
    }

    #[inline]
    fn lod(&self) -> u8 {
        self.meta & SCRATCH_META_LOD_MASK
    }

    #[inline]
    fn rrpv(&self) -> u8 {
        (self.meta >> SCRATCH_META_RRPV_SHIFT) & 0x03
    }

    #[inline]
    fn set_rrpv(&mut self, rrpv: u8) {
        self.meta = (self.meta & !(0x03 << SCRATCH_META_RRPV_SHIFT)) | ((rrpv & 0x03) << SCRATCH_META_RRPV_SHIFT);
    }

    #[inline]
    fn commit(&mut self, file: u8, lod: u8, rrpv: u8) {
        debug_assert!(lod <= SCRATCH_META_LOD_MASK);
        self.file = file;
        self.meta = SCRATCH_META_VALID | (lod & SCRATCH_META_LOD_MASK) | ((rrpv & 0x03) << SCRATCH_META_RRPV_SHIFT);
    }
}

#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<ScratchSlot>() == 8);

const INDEX_META_FILE_MASK: u8 = 0x1f;
const INDEX_META_RRPV_SHIFT: u8 = 5;
const INDEX_META_VALID: u8 = 0x80;

/// One quadtree-index cache block: a resident, block-aligned window of the index region. The
/// validity bit, five-bit shard index, and two-bit RRIP prediction share `meta`; `len` is bounded
/// by the 512-byte window. Compacting those tags pays for the leaf bbox stored in each chunk slot,
/// so the pass-B fast path adds no net resident RAM.
#[derive(Clone, Copy)]
#[repr(C)]
struct IndexBlock {
    off: u32,
    len: u16,
    meta: u8,
    /// Keep `buf` word-aligned so a full-sector extent read bypasses the board's alignment bounce.
    _align: u8,
    buf: [u8; INDEX_BLOCK],
}

// On-device each compact tagged window is 520 bytes including alignment.
#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<IndexBlock>() == INDEX_BLOCK + 8);

impl IndexBlock {
    const EMPTY: Self = Self { off: 0, len: 0, meta: 0, _align: 0, buf: [0; INDEX_BLOCK] };

    #[inline]
    fn valid(&self) -> bool {
        self.meta & INDEX_META_VALID != 0
    }

    #[inline]
    fn file(&self) -> u8 {
        self.meta & INDEX_META_FILE_MASK
    }

    /// Re-reference prediction (0 = near, 3 = distant). A hit promotes to 0; most one-pass fills
    /// enter at 3 so an ordered tree scan churns one probation slot instead of flushing all seven.
    #[inline]
    fn rrpv(&self) -> u8 {
        (self.meta >> INDEX_META_RRPV_SHIFT) & 0x03
    }

    #[inline]
    fn set_rrpv(&mut self, rrpv: u8) {
        self.meta = (self.meta & !(0x03 << INDEX_META_RRPV_SHIFT)) | ((rrpv & 0x03) << INDEX_META_RRPV_SHIFT);
    }

    #[inline]
    fn commit(&mut self, file: u8, rrpv: u8) {
        debug_assert!(file <= INDEX_META_FILE_MASK);
        self.meta = INDEX_META_VALID | (file & INDEX_META_FILE_MASK) | ((rrpv & 0x03) << INDEX_META_RRPV_SHIFT);
    }
}

#[derive(Clone, Copy)]
struct WalkEntry {
    cid: u32,
    node: BBox,
}

/// One complete expanded-view leaf result. The all-zero form is an empty cache record, like the
/// geometry/index slots. Field order and `repr(C)` pin this to 260 B on the 32-bit target; two
/// records replace one 520-byte [`IndexBlock`] exactly.
#[repr(C)]
struct WalkCache {
    cover: BBox,
    entries: [WalkEntry; WALK_CACHE_ENTRIES],
    valid: bool,
    file: u8,
    lod: u8,
    len: u8,
}

#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<WalkCache>() * WALK_CACHE_SLOTS == core::mem::size_of::<IndexBlock>());

/// The streamed-map cache: a scan-resistant five-slot geometry working set (absorbing the
/// renderer's per-priority-pass re-reads), a small block cache for quadtree-node reads, and two
/// bounded expanded-walk results. Caller-owned and reusable across frames. ≈37 KB, dominated by
/// the geometry buffers and decode scratch.
///
/// Wraps its mutable state in a `RefCell` so a [`Reader`] can read through it on `&self` paths; the
/// borrows are tightly scoped (one index-node read, or one chunk load + decode) so they never overlap.
pub struct MapCache {
    inner: RefCell<MapCacheInner>,
}

impl Default for MapCache {
    fn default() -> Self {
        Self::new()
    }
}

impl MapCache {
    /// A fresh, empty cache. ≈37 KB of zeroed buffers — on the device, place it once in the
    /// reserved region (e.g. `ptr::write`, like the `App`) so it stays off the main stack.
    pub fn new() -> Self {
        MapCache { inner: RefCell::new(MapCacheInner::new()) }
    }

    /// Allocate a fresh, empty cache **directly on the heap**, never on the stack.
    ///
    /// The cache is ≈37 KB, so `Box::new(MapCache::new())` first builds the whole value on the
    /// stack and then copies it — and a debug build walks [`MapCacheInner::new`]'s `zeroed()`
    /// interior across the stack several more un-elided-copy times — a silent overflow on a
    /// small stack (the web demo's default 1 MiB wasm shadow stack, PR #661). Like `obc-route`'s
    /// `NavScratch::new_boxed`, this owns the crate-private invariant that a zeroed allocation
    /// *is* [`MapCache::new`]:
    /// - a zeroed [`MapCacheInner`] is exactly [`MapCacheInner::new`] (which zero-inits — see
    ///   the field-by-field argument there);
    /// - the `RefCell` around it keeps its borrow state in a `Cell<isize>` whose *not borrowed*
    ///   value is 0, so the zeroed wrapper reads as unborrowed. That is a `core` implementation
    ///   detail rather than a documented guarantee — the `new_boxed_is_a_fresh_unborrowed_cache`
    ///   test is the tripwire (its first `borrow_mut` panics if this ever changes).
    ///
    /// Host-only (`alloc` feature): the device `ptr::write`s its cache into a reserved region
    /// and never calls this.
    #[cfg(feature = "alloc")]
    pub fn new_boxed() -> alloc::boxed::Box<Self> {
        // SAFETY: an all-zero `MapCache` is bit-identical to a fresh `new()` — see above.
        unsafe { alloc::boxed::Box::<Self>::new_zeroed().assume_init() }
    }

    /// Drop every resident chunk, index block, and expanded walk. Slots are keyed only inside one
    /// parse generation, so [`Reader::new`] also guards map switches via [`MapCache::adopt`]. Cheap
    /// — only validity metadata and counters are touched, not the backing buffers.
    pub fn clear(&self) -> Result<(), CacheError> {
        self.inner.try_borrow_mut().map_err(|_| CacheError::Busy)?.clear();
        Ok(())
    }

    /// Bind the cache to a [`MapTables`] parse `generation`, running the [`MapCache::clear`] logic
    /// first if it last served a different one. Called by [`Reader::new`], which is what makes the
    /// forgotten-`clear()`-on-map-switch cross-serve impossible by construction. A zeroed cache
    /// sits at generation 0 — never a live generation — so the first adopt after boot clears an
    /// already-empty cache (harmless).
    fn adopt(&self, generation: u32) -> Result<(), CacheError> {
        let mut inner = self.inner.try_borrow_mut().map_err(|_| CacheError::Busy)?;
        if inner.generation != generation {
            inner.clear();
            inner.generation = generation;
        }
        Ok(())
    }

    #[inline]
    fn stats(&self) -> Result<CacheStats, CacheError> {
        Ok(self.inner.try_borrow().map_err(|_| CacheError::Busy)?.stats())
    }

    #[inline]
    fn try_borrow_mut(&self) -> Result<RefMut<'_, MapCacheInner>, CacheError> {
        self.inner.try_borrow_mut().map_err(|_| CacheError::Busy)
    }
}

/// The cache's mutable interior (see [`MapCache`]). `tick` counts geometry fills and supplies the
/// occasional protected RRIP insertion used to resist a repeated scan just over capacity.
struct MapCacheInner {
    /// The [`MapTables::parse`] generation the resident slots belong to; 0 (the zero-init state)
    /// means "unowned". Written only by [`MapCache::adopt`] — deliberately *not* reset by `clear`,
    /// which empties the slots and so is safe under any generation.
    generation: u32,
    tick: u32,
    chunks: [ChunkSlot; MAP_CHUNK_SLOTS],
    index: [IndexBlock; INDEX_BLOCKS],
    walks: [WalkCache; WALK_CACHE_SLOTS],
    scratch_slot: ScratchSlot,
    /// The packed four regular chunk tags save sixteen bytes; the scratch tag uses eight. Keep the
    /// other eight explicit so the resource baseline stays byte-identical and future fields have a
    /// named place to live rather than silently spending stack margin.
    _chunk_layout_reserve: [u8; 8],
    /// Decode buffer for a chunk too large to cache (`> CACHE_SLOT_BYTES`, up to the accepted
    /// `MAX_CHUNK_BYTES`). Its first 4 KiB are also the fifth ordinary-chunk slot; an oversized
    /// load invalidates that tag before overwriting it.
    scratch: [u8; MAX_CHUNK_BYTES],
    chunk_hits: u32,
    chunk_misses: u32,
    sd_reads: u32,
    bytes_read: u32,
}

impl MapCacheInner {
    fn new() -> Self {
        // Zero-init via `zeroed()` (all-zero is a valid empty state for every field). This lowers
        // to a `memset` / `.bss`, whereas a struct literal zeroing the ~36 KB of buffers emits them
        // as a `.rodata` const that is then `memcpy`'d — which overflowed flash on the device.
        //
        // SAFETY: `MapCacheInner` is inhabited and valid for the all-zero bit pattern — it has no
        // references, no enums with a non-zero discriminant, and no `bool` that must be non-zero
        // (the only `bool`s are the `valid` flags, false at zero). No padding is read.
        unsafe { core::mem::MaybeUninit::zeroed().assume_init() }
    }

    /// Drop every resident slot and zero the counters, touching only the `valid` flags + counters,
    /// not the ≈36 KB of backing buffers. See [`MapCache::clear`].
    fn clear(&mut self) {
        for s in &mut self.chunks {
            s.meta = 0;
        }
        self.scratch_slot.meta = 0;
        for b in &mut self.index {
            b.meta = 0;
        }
        for walk in &mut self.walks {
            walk.valid = false;
        }
        self.tick = 0;
        self.chunk_hits = 0;
        self.chunk_misses = 0;
        self.sd_reads = 0;
        self.bytes_read = 0;
    }

    #[inline]
    fn stats(&self) -> CacheStats {
        CacheStats {
            chunk_hits: self.chunk_hits,
            chunk_misses: self.chunk_misses,
            sd_reads: self.sd_reads,
            bytes_read: self.bytes_read,
        }
    }

    #[inline]
    fn count_read(&mut self, bytes: usize) {
        self.sd_reads = self.sd_reads.saturating_add(1);
        self.bytes_read = self.bytes_read.saturating_add(bytes as u32);
    }

    fn cached_walk(&self, file: u8, lod: u8, query: &BBox) -> Option<Vec<WalkEntry, WALK_CACHE_ENTRIES>> {
        let slot = self
            .walks
            .iter()
            .find(|slot| slot.valid && slot.file == file && slot.lod == lod && bbox_contains(&slot.cover, query))?;
        let mut out = Vec::new();
        for entry in slot.entries.iter().take(slot.len as usize) {
            let _ = out.push(*entry);
        }
        Some(out)
    }

    fn store_walk(&mut self, file: u8, lod: u8, cover: BBox, entries: &Vec<WalkEntry, WALK_CACHE_ENTRIES>) {
        let i = self
            .walks
            .iter()
            .position(|slot| slot.valid && slot.file == file && slot.lod == lod)
            .or_else(|| self.walks.iter().position(|slot| !slot.valid))
            .unwrap_or(file as usize % WALK_CACHE_SLOTS);
        let slot = &mut self.walks[i];
        slot.valid = false;
        slot.cover = cover;
        for (dst, src) in slot.entries.iter_mut().zip(entries.iter()) {
            *dst = *src;
        }
        slot.file = file;
        slot.lod = lod;
        slot.len = entries.len() as u8;
        slot.valid = true;
    }

    /// Ensure chunk `(lod, cid)` — the `len` bytes at `start` — is resident, returning where its
    /// bytes are. A chunk that fits a cache slot is cached across the four dedicated buffers plus
    /// the otherwise-idle decode scratch. A larger chunk invalidates that fifth tag and uses the
    /// scratch uncached. Both paths count source fills as misses and resident service as hits.
    #[allow(clippy::too_many_arguments)]
    fn load_chunk(
        &mut self,
        src: &dyn ByteSource,
        file: u8,
        lod: u8,
        cid: u32,
        start: u32,
        len: usize,
        node: &BBox,
    ) -> Result<ChunkLoc, IoError> {
        if len > CACHE_SLOT_BYTES {
            self.scratch_slot.meta = 0;
            src.read_at(start, &mut self.scratch[..len])?;
            self.chunk_misses = self.chunk_misses.saturating_add(1);
            self.count_read(len);
            return Ok(ChunkLoc::Scratch);
        }
        if let Some(i) = self
            .chunks
            .iter()
            .position(|s| s.valid() && s.file == file && s.lod() == lod && s.cid == cid && s.len as usize == len)
        {
            self.chunk_hits = self.chunk_hits.saturating_add(1);
            self.chunks[i].used = 0;
            return Ok(ChunkLoc::Slot(i));
        }
        if self.scratch_slot.valid()
            && self.scratch_slot.file == file
            && self.scratch_slot.lod() == lod
            && self.scratch_slot.cid == cid
        {
            self.chunk_hits = self.chunk_hits.saturating_add(1);
            self.scratch_slot.set_rrpv(0);
            return Ok(ChunkLoc::Scratch);
        }
        let (victim, empty) = if let Some(i) = self.chunks.iter().position(|slot| !slot.valid()) {
            (ChunkVictim::Slot(i), true)
        } else if !self.scratch_slot.valid() {
            (ChunkVictim::Scratch, true)
        } else {
            (chunk_rrip_victim(&mut self.chunks, &mut self.scratch_slot), false)
        };
        // Invalidate before the read: a flaky source can fail partway, half-overwriting the buffer.
        // Committing `valid`/keys only after the read succeeds means a failed read leaves an empty
        // slot, not a poisoned one keyed to the old chunk (which would serve as a corrupt hit).
        self.tick = self.tick.wrapping_add(1);
        let rrpv = if empty || self.tick.is_multiple_of(5) { 2 } else { 3 };
        let loc = match victim {
            ChunkVictim::Slot(i) => {
                self.chunks[i].meta = 0;
                src.read_at(start, &mut self.chunks[i].buf[..len])?;
                self.chunks[i].file = file;
                self.chunks[i].cid = cid;
                self.chunks[i].len = len as u16;
                self.chunks[i].node = *node;
                self.chunks[i].used = rrpv;
                self.chunks[i].commit(lod);
                ChunkLoc::Slot(i)
            }
            ChunkVictim::Scratch => {
                self.scratch_slot.meta = 0;
                src.read_at(start, &mut self.scratch[..len])?;
                self.scratch_slot.cid = cid;
                self.scratch_slot.commit(file, lod, rrpv as u8);
                ChunkLoc::Scratch
            }
        };
        self.chunk_misses = self.chunk_misses.saturating_add(1);
        self.count_read(len);
        Ok(loc)
    }

    /// Fill `out` from index-region offset `off`, assembling from cached blocks (reading any
    /// missing block from the source). A node read is 4 bytes and may straddle a block edge, so
    /// this loops over blocks.
    fn index_read(&mut self, src: &dyn ByteSource, file: u8, off: u32, out: &mut [u8]) -> Result<(), IoError> {
        let mut filled = 0usize;
        while filled < out.len() {
            let cur = off + filled as u32;
            let block_off = cur - cur % INDEX_BLOCK as u32;
            let slot = self.index_block(src, file, block_off)?;
            let within = (cur - block_off) as usize;
            let blen = self.index[slot].len as usize;
            if within >= blen {
                return Err(IoError::BadOffset);
            }
            let take = (blen - within).min(out.len() - filled);
            out[filled..filled + take].copy_from_slice(&self.index[slot].buf[within..within + take]);
            filled += take;
        }
        Ok(())
    }

    /// Ensure the `INDEX_BLOCK`-aligned block at `block_off` is resident, returning its slot.
    fn index_block(&mut self, src: &dyn ByteSource, file: u8, block_off: u32) -> Result<usize, IoError> {
        if let Some(i) = self.index.iter().position(|b| b.valid() && b.file() == file && b.off == block_off) {
            self.index[i].set_rrpv(0);
            return Ok(i);
        }
        let want = ((src.len() - block_off) as usize).min(INDEX_BLOCK);
        if want == 0 {
            return Err(IoError::BadOffset);
        }
        let empty = self.index.iter().position(|b| !b.valid());
        let i = empty.unwrap_or_else(|| rrip_victim(&mut self.index));
        // Invalidate before the read (see `load_chunk`): a partial read failure must not leave a
        // poisoned slot still keyed to the old block offset.
        self.index[i].meta = 0;
        src.read_at(block_off, &mut self.index[i].buf[..want])?;
        self.index[i].off = block_off;
        self.index[i].len = want as u16;
        // Bimodal RRIP insertion: an initial fill gets a normal prediction so all slots seed;
        // thereafter seven of eight source misses enter as immediate probation (3), while the
        // periodic 2 ages out stale protected blocks after the viewport genuinely moves. Stable
        // repeated scans keep their hit blocks at 0 and churn the probation slot.
        let rrpv = if empty.is_some() || self.sd_reads.is_multiple_of(8) { 2 } else { 3 };
        self.index[i].commit(file, rrpv);
        self.count_read(want);
        Ok(i)
    }
}

/// Pick the next RRIP victim. If no entry currently predicts a distant re-reference, age every
/// entry one step and try again. Bounded: predictions saturate at 3, so at most three passes.
fn rrip_victim(slots: &mut [IndexBlock]) -> usize {
    loop {
        if let Some(i) = slots.iter().position(|slot| slot.rrpv() >= 3) {
            return i;
        }
        for slot in slots.iter_mut() {
            slot.set_rrpv((slot.rrpv() + 1).min(3));
        }
    }
}

fn chunk_rrip_victim(slots: &mut [ChunkSlot], scratch: &mut ScratchSlot) -> ChunkVictim {
    loop {
        if let Some(i) = slots.iter().position(|slot| slot.used >= 3) {
            return ChunkVictim::Slot(i);
        }
        if scratch.rrpv() >= 3 {
            return ChunkVictim::Scratch;
        }
        for slot in slots.iter_mut() {
            slot.used = (slot.used + 1).min(3);
        }
        scratch.set_rrpv((scratch.rrpv() + 1).min(3));
    }
}

#[inline]
fn bbox_contains(outer: &BBox, inner: &BBox) -> bool {
    outer.min_lon <= inner.min_lon
        && outer.min_lat <= inner.min_lat
        && outer.max_lon >= inner.max_lon
        && outer.max_lat >= inner.max_lat
}

fn intersect_bbox(a: &BBox, b: &BBox) -> Option<BBox> {
    let out = BBox {
        min_lon: a.min_lon.max(b.min_lon),
        min_lat: a.min_lat.max(b.min_lat),
        max_lon: a.max_lon.min(b.max_lon),
        max_lat: a.max_lat.min(b.max_lat),
    };
    (out.min_lon <= out.max_lon && out.min_lat <= out.max_lat).then_some(out)
}

/// Widen a query by one eighth of its span per side, clamped to the quadtree root. Arithmetic is
/// promoted so a hostile header at the i32 extremes cannot overflow while the margin is formed.
fn expand_walk_bbox(query: &BBox, root: &BBox) -> BBox {
    let lon_span = i64::from(query.max_lon) - i64::from(query.min_lon);
    let lat_span = i64::from(query.max_lat) - i64::from(query.min_lat);
    let lon_margin = (lon_span + 7) / 8;
    let lat_margin = (lat_span + 7) / 8;
    BBox {
        min_lon: (i64::from(query.min_lon) - lon_margin).max(i64::from(root.min_lon)) as i32,
        min_lat: (i64::from(query.min_lat) - lat_margin).max(i64::from(root.min_lat)) as i32,
        max_lon: (i64::from(query.max_lon) + lon_margin).min(i64::from(root.max_lon)) as i32,
        max_lat: (i64::from(query.max_lat) + lat_margin).min(i64::from(root.max_lat)) as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SliceSource;

    /// A `ByteSource` reproducing a flaky-SD partial-overwrite: the read at offset `fail_at` copies
    /// `partial` bytes into the destination and then returns `Err` (like `SdByteSource` filling
    /// block-by-block). Every other read is filled from `data`. `SliceSource` copies in one shot and
    /// can't reproduce this.
    struct FlakySource<'a> {
        data: &'a [u8],
        fail_at: u32,
        partial: usize,
    }

    impl ByteSource for FlakySource<'_> {
        fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), IoError> {
            let start = offset as usize;
            let end = start.checked_add(buf.len()).ok_or(IoError::BadOffset)?;
            let bytes = self.data.get(start..end).ok_or(IoError::BadOffset)?;
            if offset == self.fail_at {
                let n = self.partial.min(buf.len());
                buf[..n].copy_from_slice(&bytes[..n]); // partial write, then fail
                return Err(IoError::Io);
            }
            buf.copy_from_slice(bytes);
            Ok(())
        }

        fn len(&self) -> u32 {
            self.data.len() as u32
        }
    }

    /// The map renderer replays an ordered quadtree walk every frame. When that cycle is larger
    /// than the seven-window index cache, LRU has zero hits forever (the next scan evicts every
    /// block just before reuse). BRRIP must retain a protected subset while one probation slot
    /// absorbs the scan: the device's measured 18-sector pattern warms to five hits / thirteen reads.
    #[test]
    fn index_cache_resists_a_repeated_scan_larger_than_capacity() {
        const WORKING_BLOCKS: usize = 18;
        let data = [0u8; WORKING_BLOCKS * INDEX_BLOCK];
        let src = SliceSource(&data);
        let cache = MapCache::new();
        let mut inner = cache.inner.borrow_mut();
        let mut word = [0u8; 4];

        for block in 0..WORKING_BLOCKS {
            inner.index_read(&src, 0, (block * INDEX_BLOCK) as u32, &mut word).unwrap();
        }
        assert_eq!(inner.stats().sd_reads, WORKING_BLOCKS as u32, "the cold scan fills from the source");

        for block in 0..WORKING_BLOCKS {
            inner.index_read(&src, 0, (block * INDEX_BLOCK) as u32, &mut word).unwrap();
        }
        assert_eq!(
            inner.stats().sd_reads,
            (WORKING_BLOCKS + 13) as u32,
            "the repeated scan must retain five protected windows instead of LRU-thrashing all eighteen"
        );
    }

    #[test]
    fn expanded_walk_cache_hits_only_inside_its_cover_and_clear_invalidates_it() {
        let cache = MapCache::new();
        let cover = BBox { min_lon: -100, min_lat: -80, max_lon: 100, max_lat: 80 };
        let entry = WalkEntry { cid: 7, node: BBox { min_lon: -50, min_lat: -40, max_lon: 0, max_lat: 0 } };
        let mut entries = Vec::new();
        assert!(entries.push(entry).is_ok());
        cache.inner.borrow_mut().store_walk(2, 3, cover, &entries);

        let inside = BBox { min_lon: -20, min_lat: -10, max_lon: 20, max_lat: 10 };
        let hit = cache.inner.borrow().cached_walk(2, 3, &inside).unwrap();
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].cid, 7);
        assert_eq!(hit[0].node, entry.node);

        let outside = BBox { min_lon: -20, min_lat: -10, max_lon: 101, max_lat: 10 };
        assert!(cache.inner.borrow().cached_walk(2, 3, &outside).is_none());
        assert!(cache.inner.borrow().cached_walk(2, 4, &inside).is_none());
        cache.clear().unwrap();
        assert!(cache.inner.borrow().cached_walk(2, 3, &inside).is_none());
    }

    #[test]
    fn expanded_walk_bbox_clamps_without_overflow_at_i32_extremes() {
        let root = BBox { min_lon: i32::MIN, min_lat: i32::MIN, max_lon: i32::MAX, max_lat: i32::MAX };
        let query = BBox { min_lon: -8, min_lat: -16, max_lon: 8, max_lat: 16 };
        assert_eq!(expand_walk_bbox(&query, &root), BBox { min_lon: -10, min_lat: -20, max_lon: 10, max_lat: 20 });

        let edge = BBox { min_lon: i32::MIN, min_lat: i32::MAX - 8, max_lon: i32::MIN + 8, max_lat: i32::MAX };
        assert_eq!(expand_walk_bbox(&edge, &root).min_lon, i32::MIN);
        assert_eq!(expand_walk_bbox(&edge, &root).max_lat, i32::MAX);
    }

    #[test]
    fn decode_scratch_is_a_fifth_chunk_cache_slot() {
        const LEN: usize = 64;
        const CHUNKS: usize = MAP_CHUNK_SLOTS + 1;
        let data = [0u8; CHUNKS * LEN];
        let src = SliceSource(&data);
        let cache = MapCache::new();
        let mut inner = cache.inner.borrow_mut();
        let node = BBox { min_lon: 0, min_lat: 0, max_lon: 1, max_lat: 1 };

        for cid in 0..CHUNKS as u32 {
            inner.load_chunk(&src, 0, 0, cid, cid * LEN as u32, LEN, &node).unwrap();
        }
        assert_eq!(inner.stats().chunk_misses, CHUNKS as u32);

        for cid in 0..CHUNKS as u32 {
            inner.load_chunk(&src, 0, 0, cid, cid * LEN as u32, LEN, &node).unwrap();
        }
        let stats = inner.stats();
        assert_eq!(stats.chunk_misses, CHUNKS as u32, "all five chunks should remain resident");
        assert_eq!(stats.chunk_hits, CHUNKS as u32, "the second five-chunk scan must be entirely RAM-only");
    }

    /// The graph-tile cache holds [`NAV_TILE_SLOTS`] distinct chunks resident at once, and
    /// round-robin eviction drops the **oldest** on the next miss.
    #[test]
    fn nav_tile_cache_holds_the_full_working_set_and_evicts_round_robin() {
        const LEN: usize = NAV_CHUNK_SIZE; // 512, = one pinned v9 nav chunk
                                           // NAV_TILE_SLOTS + 1 distinct chunks; every byte of chunk k is `k`, so contents are checkable.
        let mut data = [0u8; (NAV_TILE_SLOTS + 1) * LEN];
        for (k, b) in data.iter_mut().enumerate() {
            *b = (k / LEN) as u8;
        }
        let src = SliceSource(&data);
        let mut cache = NavTileCache::new();
        let off = |i: usize| (i * LEN) as u32;

        // Prime every slot: misses only, contents correct.
        for i in 0..NAV_TILE_SLOTS {
            assert_eq!(cache.chunk(&src, off(i), LEN).unwrap()[0], i as u8);
        }
        assert_eq!(cache.stats(), NavCacheStats { hits: 0, misses: NAV_TILE_SLOTS as u32, ..NavCacheStats::default() });

        // Re-touch all slots — every one still resident, so no new read.
        for i in 0..NAV_TILE_SLOTS {
            assert_eq!(cache.chunk(&src, off(i), LEN).unwrap()[0], i as u8);
        }
        assert_eq!(
            cache.stats(),
            NavCacheStats { hits: NAV_TILE_SLOTS as u32, misses: NAV_TILE_SLOTS as u32, ..NavCacheStats::default() }
        );

        // One more distinct chunk evicts the oldest (round-robin cursor = slot 0 = chunk 0).
        assert_eq!(cache.chunk(&src, off(NAV_TILE_SLOTS), LEN).unwrap()[0], NAV_TILE_SLOTS as u8);
        assert_eq!(cache.stats().misses, NAV_TILE_SLOTS as u32 + 1);

        // Chunk 1 survived the eviction ⇒ hits; chunk 0 was evicted ⇒ re-reads. (Order matters: the
        // chunk-0 re-read then evicts the next round-robin victim, so check the hit first.)
        let s = cache.stats();
        cache.chunk(&src, off(1), LEN).unwrap();
        assert_eq!(cache.stats().hits, s.hits + 1, "a still-resident chunk hits");
        let s = cache.stats();
        cache.chunk(&src, off(0), LEN).unwrap();
        assert_eq!(cache.stats().misses, s.misses + 1, "the evicted oldest re-reads");
    }

    /// A route re-descends the same quadtree for every settled node. The private index cache is
    /// deliberately scan-resistant: a cycle one sector larger than capacity should churn one
    /// probation slot, not evict the entire warm index as LRU would.
    #[test]
    fn nav_index_cache_resists_a_repeated_scan_larger_than_capacity() {
        const WORKING_BLOCKS: usize = NAV_INDEX_BLOCKS + 1;
        let data = [0u8; WORKING_BLOCKS * INDEX_BLOCK];
        let src = SliceSource(&data);
        let mut cache = NavTileCache::new();
        let mut word = [0u8; 4];

        for block in 0..WORKING_BLOCKS {
            cache.index_read(&src, 0, (block * INDEX_BLOCK) as u32, &mut word).unwrap();
        }
        assert_eq!(cache.stats().index_misses, WORKING_BLOCKS as u32);

        for block in 0..WORKING_BLOCKS {
            cache.index_read(&src, 0, (block * INDEX_BLOCK) as u32, &mut word).unwrap();
        }
        assert_eq!(cache.stats().index_hits, (WORKING_BLOCKS - 2) as u32);
        assert_eq!(cache.stats().index_misses, (WORKING_BLOCKS + 2) as u32);
    }

    /// A read that fails partway must leave the evicted slot *empty*, not poisoned with the old key
    /// over a half-overwritten buffer — otherwise a later request for the old key is served as a
    /// corrupt hit.
    #[test]
    fn partial_read_failure_does_not_poison_evicted_slot() {
        const LEN: usize = 64;
        const CACHE_SLOTS: usize = MAP_CHUNK_SLOTS + 1; // four dedicated buffers + decode scratch
                                                        // One LEN-chunk per slot, plus one more past them for the failing eviction read — sized off
                                                        // MAP_CHUNK_SLOTS so the test tracks the cache size rather than a hard-coded buffer length.
        let mut data = [0u8; (CACHE_SLOTS + 1) * LEN];
        for (k, b) in data.iter_mut().enumerate() {
            *b = (k as u8).wrapping_mul(31).wrapping_add(7); // distinct, offset-derived bytes
        }
        // The eviction read (K_new) lives past the primed chunks (one per slot) and fails partway.
        let fail_at = CACHE_SLOTS as u32 * LEN as u32;
        let src = FlakySource { data: &data, fail_at, partial: 8 };

        let cache = MapCache::new();
        let mut inner = cache.inner.borrow_mut();
        let node = BBox { min_lon: 0, min_lat: 0, max_lon: 1, max_lat: 1 };

        // Prime all five slots. RRIP's first victim of the next miss is slot 0 (cid 0).
        for cid in 0..CACHE_SLOTS as u32 {
            let loc = inner.load_chunk(&src, 0, 0, cid, cid * LEN as u32, LEN, &node).unwrap();
            if cid < MAP_CHUNK_SLOTS as u32 {
                assert!(matches!(loc, ChunkLoc::Slot(_)));
            } else {
                assert!(matches!(loc, ChunkLoc::Scratch));
            }
        }
        let primed = inner.stats();
        assert_eq!(primed.chunk_misses, CACHE_SLOTS as u32);
        assert_eq!(primed.chunk_hits, 0);

        // The true bytes of K_old (cid 0), for an uncorrupted-content check after re-read.
        let mut k_old = [0u8; LEN];
        src.read_at(0, &mut k_old).unwrap();

        // Eviction read of K_new fails partway through filling slot 0's buffer.
        assert!(matches!(inner.load_chunk(&src, 0, 0, 99, fail_at, LEN, &node), Err(IoError::Io)));

        // Request K_old again: it must be a *miss* (re-read), not a hit on the poisoned slot.
        let before = inner.stats();
        let loc = inner.load_chunk(&src, 0, 0, 0, 0, LEN, &node).unwrap();
        let after = inner.stats();
        assert_eq!(after.chunk_hits, before.chunk_hits, "K_old must not hit the poisoned slot");
        assert_eq!(after.chunk_misses, before.chunk_misses + 1, "K_old must be re-read");

        // …and the re-read returns the real K_old bytes, not the half-written K_new.
        match loc {
            ChunkLoc::Slot(i) => assert_eq!(&inner.chunks[i].buf[..LEN], &k_old[..]),
            ChunkLoc::Scratch => panic!("a slot-sized chunk should land in a slot"),
        }
    }

    /// [`MapCache::new_boxed`] leans on a `core` implementation detail: a zeroed `RefCell`
    /// borrow flag means *unborrowed*. This is the tripwire — the first `borrow_mut` panics if
    /// that ever changes — plus a check that the zeroed allocation behaves like a fresh
    /// [`MapCache::new`] (empty stats, first load is a miss into a slot with the right bytes).
    #[cfg(feature = "alloc")]
    #[test]
    fn new_boxed_is_a_fresh_unborrowed_cache() {
        let cache = MapCache::new_boxed();
        let mut inner = cache.inner.borrow_mut(); // panics here if zeroed ≠ unborrowed
        assert_eq!(inner.generation, 0, "a zeroed cache must sit at the never-live generation 0");
        assert_eq!(inner.stats(), MapCache::new().stats().unwrap(), "counters must start where `new()`'s do");

        const LEN: usize = 64;
        let data = [0xA5u8; LEN];
        let src = SliceSource(&data);
        let node = BBox { min_lon: 0, min_lat: 0, max_lon: 1, max_lat: 1 };
        let loc = inner.load_chunk(&src, 0, 0, 0, 0, LEN, &node).unwrap();
        match loc {
            ChunkLoc::Slot(i) => assert_eq!(&inner.chunks[i].buf[..LEN], &data[..]),
            ChunkLoc::Scratch => panic!("a slot-sized chunk should land in a slot"),
        }
        assert_eq!(inner.stats().chunk_misses, 1, "an empty cache's first load is a miss");
    }

    /// Two maps sharing a chunk key `(lod, cid, len)` but holding *different* bytes must not
    /// cross-serve through a shared cache. Slots are keyed only by `(lod, cid, len)`, not the
    /// source — the guarantee lives in the generation adopt inside `Reader::new`, so this drives a
    /// map switch through the public path **without ever calling `clear()`** and asserts the
    /// same-key load misses and serves the new map's bytes.
    #[test]
    fn map_switch_without_clear_cannot_cross_serve() {
        use obcm_testkit::{build_file, pack_line, pad, LodSpec};

        // Two byte-identical layouts (same style table / index / chunk_size ⇒ same chunk key and
        // offsets) whose one feature differs only in its delta — the decoded endpoint tells the
        // maps apart.
        let build = |delta: (i8, i8)| {
            build_file(
                (0, 0, 1000, 1000),
                &[(1, 3, 0xF800, 2, 3, false, None)],
                &[LodSpec {
                    max_mpp: f32::INFINITY,
                    index: vec![0],
                    chunks: vec![pad(pack_line(1, 10, 10, &[delta]), 64)],
                    chunk_size: 64,
                }],
            )
        };
        let a = build((1, 1));
        let b = build((2, 2));

        /// Decode chunk `(0, 0)` and return the feature's last exterior point + the cache stats.
        fn last_point(r: &Reader) -> ((i32, i32), CacheStats) {
            let mut points = Vec::<(i32, i32), 8>::new();
            let mut ring_lens = Vec::<usize, 2>::new();
            let mut last = (0, 0);
            let node = r.bbox;
            r.for_each_feature(0, 0, &node, &mut points, &mut ring_lens, |f| {
                last = *f.exterior().last().unwrap();
            })
            .unwrap();
            (last, r.chunk_cache_stats())
        }

        let sa = SliceSource(&a);
        let sb = SliceSource(&b);
        let ta = MapTables::parse(&sa).unwrap();
        let tb = MapTables::parse(&sb).unwrap();
        assert_ne!(ta.generation, 0, "0 must stay the unowned sentinel");
        assert_ne!(ta.generation, tb.generation, "each parse gets its own generation");

        // Map A through the shared cache: a miss that leaves A's chunk resident under key (0,0,64).
        let cache = MapCache::new();
        let ra = Reader::new(&sa, &ta, &cache);
        let (pa, stats_a) = last_point(&ra);
        assert_eq!(pa, (11, 11), "map A's geometry");
        assert_eq!((stats_a.chunk_hits, stats_a.chunk_misses), (0, 1));

        // Map B over the same cache, same chunk key, *no* `clear()`: `Reader::new` adopts B's
        // generation, so the load must miss (not hit A's resident slot) and serve B's bytes.
        let rb = Reader::new(&sb, &tb, &cache);
        let (pb, stats_b) = last_point(&rb);
        assert_eq!(pb, (12, 12), "map B's geometry, not stale A bytes");
        assert_eq!((stats_b.chunk_hits, stats_b.chunk_misses), (0, 1), "the switch must miss + re-read");
    }

    #[test]
    fn safe_cache_reentry_returns_busy_and_recovers() {
        use obcm_testkit::{build_file, pack_line, pad, LodSpec};

        let bytes = build_file(
            (0, 0, 1000, 1000),
            &[(1, 3, 0xF800, 2, 3, false, None)],
            &[LodSpec {
                max_mpp: f32::INFINITY,
                index: vec![0],
                chunks: vec![pad(pack_line(1, 10, 10, &[(1, 1)]), 64)],
                chunk_size: 64,
            }],
        );
        let src = SliceSource(&bytes);
        let tables = MapTables::parse(&src).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&src, &tables, &cache);
        let mut points = Vec::<(i32, i32), 8>::new();
        let mut rings = Vec::<usize, 2>::new();
        let mut nested = None;

        reader
            .for_each_feature(0, 0, &reader.bbox, &mut points, &mut rings, |_| {
                nested = Some(reader.for_each_chunk(0, &reader.bbox, |_, _| {}));
                assert_eq!(reader.try_chunk_cache_stats(), Err(CacheError::Busy));
                assert_eq!(cache.clear(), Err(CacheError::Busy));
                let reentered = Reader::new(&src, &tables, &cache);
                assert_eq!(
                    reentered.for_each_chunk(0, &reader.bbox, |_, _| {}),
                    Err(MapReadError::Cache(CacheError::Busy))
                );
            })
            .unwrap();

        assert_eq!(nested, Some(Err(MapReadError::Cache(CacheError::Busy))));
        assert!(reader.for_each_chunk(0, &reader.bbox, |_, _| {}).is_ok(), "outer borrow must be released");
    }

    /// A style-table read that *fails* (flaky card) must surface as a parse error, not an
    /// all-`None` table that loads "fine" and renders nothing. Exercises both reads — the count
    /// byte and the record block — via a `FlakySource` failing at exactly that offset. (A
    /// physically *truncated* table is still tolerated; see `extremes.rs`.)
    #[test]
    fn failed_style_table_read_errors_map_parse() {
        use obcm_testkit::{build_file, pack_line, pad, LodSpec};
        let bytes = build_file(
            (0, 0, 1000, 1000),
            &[(1, 3, 0xF800, 2, 3, false, None)],
            &[LodSpec {
                max_mpp: f32::INFINITY,
                index: vec![0],
                chunks: vec![pad(pack_line(1, 10, 10, &[(1, 1)]), 64)],
                chunk_size: 64,
            }],
        );
        let style_off = u32::from_le_bytes(bytes[21..25].try_into().unwrap());
        // The count-byte read (at style_off), then the record-block read (at style_off + 1).
        for fail_at in [style_off, style_off + 1] {
            let src = FlakySource { data: &bytes, fail_at, partial: 0 };
            assert!(
                matches!(MapTables::parse(&src), Err(Error::Source(IoError::Io))),
                "a failed style read at {fail_at} must error the parse"
            );
        }
        // Control: the same bytes through a healthy source parse fine.
        assert!(MapTables::parse(&SliceSource(&bytes)).is_ok());
    }

    /// The index-block cache keys a block by its absolute offset into the index region, which means
    /// nothing across maps — so `clear()` must drop index blocks too, or a switched map's quadtree
    /// read at the same offset would hit a stale block. Detected via the source-read counters: a
    /// post-clear read of the same offset must re-read (a hit reads nothing).
    #[test]
    fn clear_invalidates_index_blocks() {
        let data = [0x5Au8; 1024];
        let src = SliceSource(&data);

        let cache = MapCache::new();
        let mut inner = cache.inner.borrow_mut();

        // Resident, then a hit (no source read).
        inner.index_block(&src, 0, 0).unwrap();
        let before = inner.stats();
        inner.index_block(&src, 0, 0).unwrap();
        assert_eq!(inner.stats().sd_reads, before.sd_reads, "a resident block must hit, not re-read");
        drop(inner);

        // After clear the same offset must miss and re-read from the source.
        cache.clear().unwrap();
        let mut inner = cache.inner.borrow_mut();
        let before = inner.stats();
        inner.index_block(&src, 0, 0).unwrap();
        assert_eq!(inner.stats().sd_reads, before.sd_reads + 1, "post-clear index read must re-read");
    }
}
