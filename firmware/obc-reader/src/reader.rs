//! OBCM **v8** format reader: header, style table, LOD table, per-LOD
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
//! [`MapCache`] behind a `RefCell`: a geometry-chunk cache (the renderer re-runs
//! `for_each_chunk` once per priority level, so this avoids re-reading a chunk per
//! pass) plus a small block cache coalescing the 4-byte quadtree-node reads. The
//! cache changes only *when* a byte is read, never *what* decodes, so renders stay
//! byte-identical.

use core::cell::{RefCell, RefMut};
use core::sync::atomic::{AtomicU32, Ordering};

use heapless::Vec;

use crate::byte_io::{ByteSource, Error as IoError};
use crate::codec::{rd_f32, rd_i16, rd_i32, rd_u16, rd_u32};
use crate::format::{
    BRANCH_BIT, EMPTY_LEAF, FEATURE_FLAG_16BIT, FEATURE_FLAG_HOLES, FEATURE_FLAG_POLYGON, STYLE_PRIORITY_MASK,
};
use crate::geo::{cos_lat, ground_dist_m_cl};
use crate::poi_table::PoiCategory;
use crate::{BBox, Error, M_PER_DEG};

/// Upper bound on the vertices of a single decoded feature — the capacity a caller
/// sizes the `points` scratch buffer to for [`Reader::for_each_feature`].
pub const MAX_FEAT_PTS: usize = 2048;
/// Upper bound on the rings (exterior + holes) of a single decoded feature — the
/// capacity for the `ring_lens` scratch buffer of [`Reader::for_each_feature`].
pub const MAX_FEAT_RINGS: usize = 32;

/// The header is fixed-size; everything after it is reached via explicit offsets. v6 grew it from
/// 32 to 36 bytes (the trailing `POI Section Offset` u32); v8 to 40 (the `Nav Graph Offset` u32).
pub const HEADER_LEN: usize = 40;
/// Each LOD table entry: `max_mpp f32, index_off u32, node_count u32, chunk_size u16, chunk_count u32`.
pub const LOD_ENTRY_LEN: usize = 18;

/// One POI-directory category entry (spec §7.1): `u8 category_id, u32 index_offset, u32
/// index_node_count, u32 chunk_count`.
const POI_CAT_ENTRY_LEN: usize = 13;

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

/// A POI record is a fixed 36 bytes (spec §7.3): `int32 lat, int32 lon, u8 subtype, u8 name_len,
/// [u8;24] name, u16 hours_ref`. The record loop steps by this and derives records-per-chunk as
/// `chunk_size / POI_RECORD_LEN` (`512 / 36 = 14`).
const POI_RECORD_LEN: usize = 36;

/// A single hours-pool blob (spec §7.5): `flags u8` + `7 days × 2 slots × (open_q, close_q)`.
/// [`parse_poi_directory`] validates the pool region lies in-file; [`Reader::poi_hours`] reads one
/// blob on demand into a stack buffer and decodes it to a [`crate::hours::WeeklySchedule`] (#443).
pub const POI_HOURS_BLOB_LEN: usize = 29;

/// Max results the nearest-N POI query returns (locked on epic #115). The caller owns a
/// `heapless::Vec<Poi, MAX_POI_RESULTS>`; the query fills it ascending by distance and never
/// exceeds it. 16 × ≈36 B ≈ 600 B, on the caller's stack.
pub const MAX_POI_RESULTS: usize = 16;

/// Max stored POI name length (spec §7.3: the 24-byte `Name` field). A [`Poi::name`] is a
/// `heapless::String<POI_NAME_MAX>`; a record whose `name_len` exceeds this is clamped (defensive —
/// the packer never writes one).
pub const POI_NAME_MAX: usize = 24;

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

/// The v9 nav directory (spec §8.1): `index_offset u32, index_node_count u32, node_chunk_count u32,
/// edge_pool_offset u32, edge_chunk_count u32, chunk_size u16, profile_table_offset u32,
/// profile_count u8, reserved u8` — 28 bytes (v8 was 22). Parsed into [`NavDirectory`]; the profile
/// table it points at is parsed into [`MapTables::profiles`].
const NAV_DIR_LEN: usize = 28;

/// The fixed nav chunk size (spec §8.1): v9 **pins** it to 512, so [`parse_nav_directory`] rejects
/// any other value (the geometry sections' configurable chunk size is independent — nav is fixed).
pub const NAV_CHUNK_SIZE: usize = 512;

/// Fixed prefix of a §8.3 junction record: `lat i32, lon i32, node_id u32, degree u8`. The `degree`
/// byte at record offset 12 doubles as the end-of-chunk sentinel (`0xFF`, mirroring the POI
/// subtype sentinel — a real degree is capped far below it, spec §8.3).
pub const NAV_NODE_FIXED_LEN: usize = 13;

/// One v9 §8.3 neighbor entry, 15 bytes: `neighbor_id u32, dlat i16, dlon i16, edge_id u32,
/// cost_m u16, way_kind u8`. `dlat`/`dlon` are µdeg deltas from the owning record's own coord
/// (reconstructed inline so A* computes `f = g + h` at relaxation with no second fetch); `way_kind`
/// is N1's packed class byte (N3 weights edges by it). v8 was 20 bytes (absolute i32 coords, u32
/// cost, no kind).
pub const NAV_NEIGHBOR_LEN: usize = 15;

/// Fixed prefix of a v9 §8.4 edge record, 15 bytes: `length_m u32, pt_count u16, way_kind u8,
/// anchor_lat i32, anchor_lon i32`; `pt_count - 1` × `(dlat i16, dlon i16)` deltas follow. v8 was
/// 14 bytes (no `way_kind`).
pub const NAV_EDGE_FIXED_LEN: usize = 15;

/// One §8.6 profile record: `name [u8;12]` (0xFF-padded UTF-8), `highway_mult [u8;32]`,
/// `surface_mult [u8;8]`.
pub const NAV_PROFILE_LEN: usize = 52;

/// The `Name` field width inside a profile record (§8.6): 12 bytes, `0xFF`-padded.
pub const NAV_PROFILE_NAME_LEN: usize = 12;

/// Profile-count cap (§8.6): `profile_count` is a `u8` the reader rejects outside `1..=8`, so at
/// most eight profiles are ever resident.
pub const NAV_MAX_PROFILES: usize = 8;

/// Upper bound on the nav `chunk_size` the reader accepts (spec §8.1), and the byte size of one
/// [`NavTileCache`] slot. v9 pins the wire value to exactly [`NAV_CHUNK_SIZE`] (512), so this equals
/// it: the cache holds whole 512 B chunks and [`NavTileCache::chunk`]'s `debug_assert` guards that
/// no larger chunk is ever routed through a slot. Pinning the two together is what lets N4 fit **8**
/// slots in the same ~4 KB the pre-N4 `2 × 2048 B` geometry used (see [`NAV_TILE_SLOTS`]).
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
/// `nrf-mem` halves the scratch (issue #270 — the map path must coexist with the BLE stack on
/// the 256 KB DK): a map packed with `chunk_size` past 8192 loads on the host/sim but is
/// rejected on the device. The packer default (4096) clears it with room; the 512 KB LM20
/// re-decides the cap.
///
/// **Do not trim this below 8192 under `nrf-mem`** (tried in #116 R4, reverted): `nrf-mem` is an
/// *additive* feature the all-features host CI enables, so this constant is an **acceptance**
/// bound, not just a buffer size — shrinking it makes the host reader reject the deliberately
/// large chunks the round-trip suite packs (obc-pack's `max_feat_pts_boundary_survives` puts two
/// features, one at `MAX_FEAT_PTS`, into one 8192-byte chunk). Reclaiming the scratch's headroom
/// would first need acceptance decoupled from the `nrf-mem` scratch size.
#[cfg(not(feature = "nrf-mem"))]
pub const MAX_CHUNK_BYTES: usize = 16384;
#[cfg(feature = "nrf-mem")]
pub const MAX_CHUNK_BYTES: usize = 8192;

/// Size of one geometry-chunk **cache** slot. A chunk this size or smaller is cached (kept
/// resident across the frame's priority passes); a larger one — up to [`MAX_CHUNK_BYTES`] — is
/// decoded through the scratch without being cached. Matches the packer's default `chunk_size`
/// (4096) so the maps the device actually loads are fully cacheable.
const CACHE_SLOT_BYTES: usize = 4096;

/// Geometry-chunk cache slots (each [`CACHE_SLOT_BYTES`]). The renderer makes four priority
/// passes over the same visible-chunk set per frame, so the cache must hold the whole working set
/// or each pass re-reads every chunk (`miss ≈ chunks × 4`) and SD I/O dominates. Size it to the
/// **visible-chunk count**, not a fixed 16: the worst zooms (LOD1, ~3–4 m/px) put ~50 chunks in
/// view, so 64 slots keep them resident across all four passes and across frames (a slow pan
/// re-hits last frame's chunks). 64 × 4 KB = 256 KB.
///
/// The constrained `nrf-mem` profile drops to a single slot: a riding-zoom working set already
/// exceeded the previous 3 slots (measured: 0 hits, misses ≈ chunks × passes — the DK is
/// SD-bound either way), so extra slots bought nothing; what the cull buys is room for the
/// BLE stack next to the map path — and stack headroom under the combined build's deep ride
/// path — on the 256 KB part (issue #270). A one-chunk view still hits across passes and frames.
#[cfg(not(feature = "nrf-mem"))]
const MAP_CHUNK_SLOTS: usize = 64;
#[cfg(feature = "nrf-mem")]
const MAP_CHUNK_SLOTS: usize = 1;

/// Block size + count of the quadtree-index cache. The leaf walk reads 4-byte nodes (siblings
/// adjacent in the file); caching a few aligned blocks coalesces those into a handful of SD
/// reads per walk rather than one read per node. ≈4 KB total on the host; `nrf-mem` halves the
/// block count (epic #116 R4's squeeze — the nav statics needed the room back): the walks stay
/// block-coalesced, a wide index just re-reads a couple more 512 B blocks per walk on the
/// already-SD-bound device.
const INDEX_BLOCK: usize = 512;
#[cfg(not(feature = "nrf-mem"))]
const INDEX_BLOCKS: usize = 8;
#[cfg(feature = "nrf-mem")]
const INDEX_BLOCKS: usize = 4;

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

#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub id: u8,
    pub z_index: i8,
    pub color: u16, // RGB565
    pub weight: u8,
    pub priority: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Line,
    Polygon,
}

/// One level of the LOD pyramid: a self-contained quadtree index + chunk set.
#[derive(Debug, Clone, Copy)]
pub struct Lod {
    /// Upper bound of the meters-per-pixel range this level covers; the coarsest
    /// level is `f32::INFINITY`. Strictly decreasing from coarse (0) to fine.
    pub max_mpp: f32,
    pub index_offset: usize,
    pub node_count: usize,
    pub chunk_size: usize,
    pub chunk_count: usize,
}

impl Lod {
    /// Byte offset where this level's data chunks begin (right after its index).
    /// `None` if the arithmetic overflows `usize` — reachable on the 32-bit MCU
    /// from a corrupt `index_offset`/`node_count`.
    #[inline]
    fn data_start(&self) -> Option<usize> {
        self.node_count.checked_mul(4)?.checked_add(self.index_offset)
    }

    /// Byte range `[start, end)` of chunk `chunk_id`, or `None` if `chunk_id` is out
    /// of range or any offset overflows `usize`. `chunk_id` comes straight from a
    /// quadtree leaf (arbitrary in a corrupt map), so validate against `chunk_count`
    /// with checked arithmetic to keep the 32-bit device from wrapping past the
    /// caller's file-length guard. The caller still bounds-checks `end` against the buffer.
    #[inline]
    fn chunk_range(&self, chunk_id: u32) -> Option<(usize, usize)> {
        let id = chunk_id as usize;
        if id >= self.chunk_count {
            return None;
        }
        let start = id.checked_mul(self.chunk_size)?.checked_add(self.data_start()?)?;
        let end = start.checked_add(self.chunk_size)?;
        Some((start, end))
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
    /// 32-bit MCU). Mirrors [`Lod::data_start`] — the shared §7.1 convention.
    #[inline]
    pub fn data_start(&self) -> Option<usize> {
        self.node_count.checked_mul(4)?.checked_add(self.index_offset)
    }

    /// Byte range `[start, end)` of POI chunk `chunk_id` given the directory's shared `chunk_size`,
    /// or `None` if `chunk_id` is out of range or any offset overflows `usize`. Mirrors
    /// [`Lod::chunk_range`] (the §7.1 chunk-size is directory-wide, not per-entry, so it's passed
    /// in). `chunk_id` comes from a quadtree leaf (arbitrary in a corrupt map), so it's validated
    /// against `chunk_count` with checked arithmetic.
    #[inline]
    fn chunk_range(&self, chunk_id: u32, chunk_size: usize) -> Option<(usize, usize)> {
        let id = chunk_id as usize;
        if id >= self.chunk_count {
            return None;
        }
        let start = id.checked_mul(chunk_size)?.checked_add(self.data_start()?)?;
        let end = start.checked_add(chunk_size)?;
        Some((start, end))
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
    /// Number of 52-byte profile records at `profile_table_offset` (1..=8; parse rejects otherwise).
    pub profile_count: usize,
}

impl NavDirectory {
    /// The map carries no routable graph (no quadtree, no chunks, no edges).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.node_count == 0
    }

    /// Byte offset where the node data chunks begin (right after the index), or `None` on
    /// `usize` overflow (a corrupt directory on the 32-bit MCU). Mirrors [`Lod::data_start`].
    #[inline]
    pub fn data_start(&self) -> Option<usize> {
        self.node_count.checked_mul(4)?.checked_add(self.index_offset)
    }

    /// Byte range `[start, end)` of node chunk `chunk_id`, or `None` if out of range / on
    /// overflow. Mirrors [`Lod::chunk_range`].
    #[inline]
    fn chunk_range(&self, chunk_id: u32) -> Option<(usize, usize)> {
        let id = chunk_id as usize;
        if id >= self.chunk_count {
            return None;
        }
        let start = id.checked_mul(self.chunk_size)?.checked_add(self.data_start()?)?;
        let end = start.checked_add(self.chunk_size)?;
        Some((start, end))
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
}

impl MapProfile {
    /// The profile's display name (UTF-8, trailing `0xFF` padding trimmed); `""` if not valid UTF-8.
    #[inline]
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("")
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
    /// #501 alignment rule); `cost_m` widens from the `u16` wire value; `way_kind` is a raw byte.
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
        })
    }
}

/// Graph-tile cache slots. **Eight** (N4, epic #533): the earlier "two slots cover the working set"
/// assumption did not survive measurement. An A\* frontier pops the *globally* best-`f` node, so it
/// hops between many simultaneously-active quadtree leaves, not one advancing neighborhood — a
/// 2-slot cache thrashed at ~33 % hit rate on the real `grimsel.obcm` probe (giant-component
/// endpoints, 2026-07-07). Rebuilding at 8 slots roughly doubled that (~55–67 % depending on the
/// route) on the same runs; 16 bought little more (the frontier's live-leaf set is small but well
/// above two). Because N2 pins nav
/// `chunk_size` to 512 B, eight slots cost the **same ~4 KB** the old `2 × 2048 B` geometry did — a
/// pure win in the device's `.bss` next to the router's scratch, no extra RAM. Eviction stays
/// round-robin (below): at 8 slots LRU bookkeeping bought nothing measurable.
pub const NAV_TILE_SLOTS: usize = 8;

/// Empty-slot tag for [`NavTileCache`]: a chunk's absolute file offset never reaches `u32::MAX`
/// (its whole extent must lie inside a `u32`-addressed source).
const NAV_TILE_EMPTY: u32 = u32::MAX;

/// A snapshot of the [`NavTileCache`] counters. `misses` doubles as the SD-read count: every miss
/// is exactly one `chunk_size`-byte `read_at`, and hits read nothing — the number R4 logs on-glass
/// (the device is SD-bound; this cache is the epic's named thrash mitigation).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NavCacheStats {
    /// Nav-chunk requests served from a resident slot (no SD read).
    pub hits: u32,
    /// Nav-chunk requests that missed and read `chunk_size` bytes from the source.
    pub misses: u32,
}

/// A tiny caller-owned cache of whole nav chunks (node **and** edge-pool — both are `chunk_size`
/// ≤ [`NAV_MAX_CHUNK_BYTES`] bytes, spec §8.1), keyed by the chunk's absolute file offset so the
/// two chunk spaces can't collide. [`Reader::for_each_nav_node_cached`] and
/// [`Reader::nav_edge_oriented`] stream through it so the router's per-settle spatial re-fetch
/// doesn't re-read the same leaf from the SD (epic #116's named risk). Round-robin eviction: across
/// the [`NAV_TILE_SLOTS`] slots the measured hit rate matches LRU's within noise (the frontier's
/// live-leaf set has no strong recency skew), so the cheaper cursor is kept.
///
/// ~4 KB, owned by the caller (the device puts it in `.bss`); `new()` is `const` so a `static`
/// lands zero-initialized. The tags are only meaningful against one map/source — the router
/// resets it per `plan_route`, so a map switch can never cross-serve a stale chunk.
pub struct NavTileCache {
    slots: [[u8; NAV_MAX_CHUNK_BYTES]; NAV_TILE_SLOTS],
    /// Absolute file offset of the chunk each slot holds, or [`NAV_TILE_EMPTY`].
    tags: [u32; NAV_TILE_SLOTS],
    /// Round-robin eviction cursor.
    next: u8,
    hits: u32,
    misses: u32,
}

impl NavTileCache {
    pub const fn new() -> Self {
        NavTileCache {
            slots: [[0; NAV_MAX_CHUNK_BYTES]; NAV_TILE_SLOTS],
            tags: [NAV_TILE_EMPTY; NAV_TILE_SLOTS],
            next: 0,
            hits: 0,
            misses: 0,
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
    }

    /// Snapshot of the hit/miss counters since the last [`NavTileCache::reset`].
    #[inline]
    pub fn stats(&self) -> NavCacheStats {
        NavCacheStats { hits: self.hits, misses: self.misses }
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
}

impl Default for NavTileCache {
    fn default() -> Self {
        Self::new()
    }
}

/// A single POI result from [`Reader::nearest_pois`]. Coordinates are absolute microdegrees (§7.3);
/// `distance_m` is the ground distance from the query position, computed during the scan. `name` is
/// empty for an unnamed POI — the app then shows the subtype's fallback label
/// ([`label_of`](crate::label_of)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Poi {
    pub lat: i32,
    pub lon: i32,
    /// Canonical subtype id (§7.4), always in `1..=18` for a returned POI.
    pub subtype: u8,
    /// Stored name (≤ [`POI_NAME_MAX`] bytes); empty ⇒ unnamed.
    pub name: heapless::String<POI_NAME_MAX>,
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

/// The OBCM header fields that describe a map without touching any geometry — a "which map is
/// this?" probe. Read cache-free via [`read_header`] (no ≈277 KB [`MapCache`]); [`MapTables::parse`]
/// parses the same prefix on its way to the full tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapHeader {
    pub version: u8,
    pub bbox: BBox,
    /// User-position marker color (RGB565); see [`Reader::marker_color`].
    pub marker_color: u16,
}

/// Decode + validate the fixed 40-byte OBCM header (magic, version, bbox, marker color).
/// Shared by [`read_header`] and [`MapTables::parse`] so the byte layout lives in one place.
/// Offsets follow `obc-pack`'s header pack (see OBCM_Spec.md).
fn parse_header(h: &[u8; HEADER_LEN]) -> Result<MapHeader, Error> {
    if &h[0..4] != b"OBCM" {
        return Err(Error::BadMagic);
    }
    let version = h[4];
    if version != 9 {
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

/// Read just the OBCM header (version + bbox + marker color) from `src` — no style/LOD table,
/// index, or geometry, so it allocates nothing and needs no [`MapCache`]. A map shorter than the
/// header, with the wrong magic, or an unsupported version is rejected exactly as parsing would.
pub fn read_header(src: &dyn ByteSource) -> Result<MapHeader, Error> {
    if (src.len() as usize) < HEADER_LEN {
        return Err(Error::TooShort);
    }
    let mut h = [0u8; HEADER_LEN];
    src.read_at(0, &mut h).map_err(|_| Error::TooShort)?;
    parse_header(&h)
}

/// The session-resident, immutable map tables — everything [`Reader`] needs that doesn't change
/// frame to frame: header scalars, style table, LOD pyramid. Parsed **once** per `.obcm` by
/// [`MapTables::parse`], then borrowed by a cheap per-frame [`Reader::new`]. Keeping the per-frame
/// reader ~tens of bytes (no re-parse, no 1536-byte style scratch, no per-frame style-table SD
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
    /// The parsed §8.6 routing profiles (1..=8, always present). RAM: at most 8 × 52 B = 416 B
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
    /// allocating step (a 1536-byte style scratch plus the style/LOD-table SD reads), so do it
    /// **once** per map and hand the result to [`Reader::new`] each frame. A map shorter than the
    /// header, with the wrong magic / version, or with out-of-range table offsets is rejected. The
    /// magic / version / bbox / marker prefix goes through the shared (private) `parse_header` (so
    /// [`read_header`] validates identically); the style + LOD-table offsets are decoded here.
    pub fn parse(src: &dyn ByteSource) -> Result<MapTables, Error> {
        let total = src.len() as usize;
        if total < HEADER_LEN {
            return Err(Error::TooShort);
        }
        let mut header = [0u8; HEADER_LEN];
        src.read_at(0, &mut header).map_err(|_| Error::TooShort)?;
        let MapHeader { version, bbox, marker_color } = parse_header(&header)?;
        let style_offset = rd_u32(&header, 21) as usize;
        let lod_count = header[25] as usize;
        let lod_table_offset = rd_u32(&header, 26) as usize;
        let poi_section_offset = rd_u32(&header, 32) as usize;
        let nav_section_offset = rd_u32(&header, 36) as usize;

        if style_offset < HEADER_LEN || style_offset > total {
            return Err(Error::BadOffset);
        }
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

    /// The map's §8.6 routing profiles (1..=8, always present). Lets a host mirror the profile
    /// **names** into the app UI (`App::set_nav_profiles`) straight off the parsed tables, without
    /// building a per-frame [`Reader`] — the same slice [`Reader::nav_profiles`] returns.
    pub fn nav_profiles(&self) -> &[MapProfile] {
        &self.profiles
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
    /// Borrowed lazy-read cache for the streamed index + geometry — the ≈84 KB of buffers live in
    /// the caller's [`MapCache`], reusable across frames. It keeps its own `RefCell` because
    /// `read_at` takes `&self` but the cache mutates; the borrows are tightly scoped so the
    /// index-node read and the chunk decode never overlap.
    cache: &'a MapCache,
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
        cache.adopt(tables.generation);
        Reader { src, version: tables.version, bbox: tables.bbox, marker_color: tables.marker_color, tables, cache }
    }

    /// Snapshot of the chunk-cache + streaming counters. Cumulative over the cache's life, so the
    /// renderer reports the per-frame delta.
    #[inline]
    pub fn chunk_cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    /// The parsed LOD pyramid (coarsest first).
    #[inline]
    pub fn lods(&self) -> &[Lod] {
        &self.tables.lods
    }

    /// The parsed POI directory (spec §7): the shared chunk size, one entry per category, and the
    /// v7 hours-pool offset/count. Always present (six categories, some possibly empty).
    /// [`Reader::nearest_pois`] walks the per-category quadtrees; P3 (#443) reads
    /// [`PoiDirectory::hours_pool_offset`]/[`PoiDirectory::hours_pool_count`] to resolve a POI's
    /// pooled schedule.
    #[inline]
    pub fn poi_directory(&self) -> &PoiDirectory {
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
        // The no-hours sentinel and any index past the pool ⇒ no schedule.
        let dir = &self.tables.pois;
        if hours_ref == 0xFFFF || (hours_ref as usize) >= dir.hours_pool_count {
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
    /// `(lat, lon, subtype)` so it's never returned twice. Corrupt input (a `chunk_size == 0`, an
    /// out-of-range subtype, a truncated chunk) is skipped, never a panic — matching the reader's
    /// posture elsewhere.
    ///
    /// # Reentrancy
    ///
    /// Like the geometry walk, this streams from the source through the internal cache; do not call
    /// it from inside a `for_each_feature*` / `for_each_chunk` callback (it would re-borrow the
    /// cache and panic).
    pub fn nearest_pois(
        &self,
        category: PoiCategory,
        pos: (i32, i32),
        out: &mut Vec<Poi, MAX_POI_RESULTS>,
    ) -> Result<(), Error> {
        out.clear();
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
            self.poi_scan(&entry, dir.chunk_size, pos, cl, &search, out);

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

    /// One expanding-ring pass: walk `entry`'s quadtree for leaves overlapping `search` and, for each
    /// non-empty leaf, decode its 36-byte records through a single 512-byte stack scratch, folding
    /// every valid record into the nearest-16 `out` set (deduped by `(lat, lon, subtype)`). `cl` is
    /// the hoisted `cos_lat`; distances are equirectangular ground meters via the shared
    /// [`crate::geo`] core.
    ///
    /// The chunk decode runs **inside** the walk callback: `walk_leaves` releases its index-cache
    /// borrow before invoking the callback, and the POI chunk read goes through a plain
    /// `src.read_at` stack scratch (never the `MapCache`), so the two never nest — and the pass is
    /// truly streaming with **no per-leaf buffer**, so an exhaustive (map-covering) final pass can't
    /// silently drop a leaf however dense the category.
    fn poi_scan(
        &self,
        entry: &PoiCatEntry,
        chunk_size: usize,
        pos: (i32, i32),
        cl: f32,
        search: &BBox,
        out: &mut Vec<Poi, MAX_POI_RESULTS>,
    ) {
        // The whole chunk's record count. A chunk with no sentinel room (records × 32 == chunk_size)
        // is bounded by this count instead (mirrors `for_each_feature_filtered`).
        let records_per_chunk = chunk_size / POI_RECORD_LEN;
        self.walk_leaves(entry, 0, self.bbox, search, 0, &mut |cid, _node| {
            let (start, end) = match entry.chunk_range(cid, chunk_size) {
                Some(r) => r,
                None => return,
            };
            if end > self.src.len() as usize {
                return;
            }
            self.scan_poi_chunk(start as u32, records_per_chunk, pos, cl, out);
        });
    }

    /// Stream one POI chunk's records through a single **512-byte** stack scratch — `POI_SCAN_WINDOW`
    /// bytes (16 records) at a time — folding each valid record into `out`. Reading in a fixed window
    /// keeps the scratch tiny regardless of the accepted `chunk_size` (up to `POI_MAX_CHUNK_BYTES`);
    /// `POI_RECORD_LEN` divides the window so a record never straddles two reads. `start` is the
    /// chunk's byte offset, already bounds-checked by the caller. Terminates on the `0xFF` subtype
    /// sentinel or after `record_cap` records (a sentinel-less full chunk).
    fn scan_poi_chunk(
        &self,
        start: u32,
        record_cap: usize,
        pos: (i32, i32),
        cl: f32,
        out: &mut Vec<Poi, MAX_POI_RESULTS>,
    ) {
        const RECS_PER_WINDOW: usize = POI_SCAN_WINDOW / POI_RECORD_LEN;
        let mut scratch = [0u8; POI_SCAN_WINDOW];
        let mut done = 0usize;
        while done < record_cap {
            let take = (record_cap - done).min(RECS_PER_WINDOW);
            let win = &mut scratch[..take * POI_RECORD_LEN];
            if self.src.read_at(start + (done * POI_RECORD_LEN) as u32, win).is_err() {
                return; // a flaky read ends this chunk cleanly (skip, no panic)
            }
            for r in 0..take {
                let off = r * POI_RECORD_LEN;
                let subtype = win[off + 8];
                if subtype == 0xFF {
                    return; // end-of-records sentinel — nothing valid follows in this chunk
                }
                // Skip an out-of-range subtype (0, or past the table) cleanly — never panic/UB.
                if crate::poi_table::subtype_row(subtype).is_none() {
                    continue;
                }
                let lat = rd_i32(win, off);
                let lon = rd_i32(win, off + 4);
                let distance_m = ground_dist_m_cl(pos, (lon, lat), cl) as u32;
                consider_poi(out, PoiCand { lat, lon, subtype, distance_m }, win, off);
            }
            done += take;
        }
    }

    /// The parsed nav directory (spec §8.1). Always present in v9; `is_empty()` for a map with no
    /// routable ways.
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
    /// empty graph visits nothing. Corrupt input (a truncated record, a chunk range past EOF) ends
    /// that chunk cleanly — never a panic, matching the POI scan's posture.
    ///
    /// # Reentrancy
    ///
    /// Like [`Reader::nearest_pois`], the quadtree walk streams through the internal index cache;
    /// do not call this from inside a `for_each_feature*` / `for_each_chunk` callback.
    pub fn for_each_nav_node(
        &self,
        view: &BBox,
        scratch: &mut [u8],
        mut visit: impl FnMut(NavNodeRef),
    ) -> Result<(), Error> {
        let dir = self.tables.nav;
        if dir.is_empty() {
            return Ok(());
        }
        if scratch.len() < dir.chunk_size {
            return Err(Error::TooShort);
        }
        self.walk_leaves(&dir, 0, self.bbox, view, 0, &mut |cid, _node| {
            let (start, end) = match dir.chunk_range(cid) {
                Some(r) => r,
                None => return,
            };
            if end > self.src.len() as usize {
                return;
            }
            let chunk = &mut scratch[..dir.chunk_size];
            if self.src.read_at(start as u32, chunk).is_err() {
                return; // a flaky read skips this leaf cleanly
            }
            decode_nav_chunk(chunk, &mut visit);
        });
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
        let dir = &self.tables.nav;
        let cs = dir.chunk_size;
        if dir.edge_chunk_count == 0 || cs == 0 {
            return None;
        }
        // Pool bounds + intra-chunk bounds for the fixed head. All checked: `edge_id` is
        // unvalidated input (a corrupt map, or R3 handed a stale id).
        let pool_len = dir.edge_chunk_count.checked_mul(cs)?;
        let id = edge_id as usize;
        let within = id % cs;
        if within + NAV_EDGE_FIXED_LEN > cs || id + NAV_EDGE_FIXED_LEN > pool_len {
            return None;
        }
        let start = dir.edge_pool_offset.checked_add(id)?;
        let mut head = [0u8; NAV_EDGE_FIXED_LEN];
        let head_off = u32::try_from(start).ok()?;
        if start + NAV_EDGE_FIXED_LEN > self.src.len() as usize {
            return None;
        }
        self.src.read_at(head_off, &mut head).ok()?;
        let length_m = rd_u32(&head, 0);
        let pt_count = rd_u16(&head, 4) as usize;
        // byte 6 is `way_kind` (v9); the anchor shifts to 7/11.
        let anchor_lat = rd_i32(&head, 7);
        let anchor_lon = rd_i32(&head, 11);
        if pt_count == 0 {
            return None;
        }
        // The whole record must lie inside its chunk (the §8.4 no-straddle contract) and the file.
        let rec_len = NAV_EDGE_FIXED_LEN.checked_add((pt_count - 1).checked_mul(4)?)?;
        if within + rec_len > cs || id + rec_len > pool_len || start + rec_len > self.src.len() as usize {
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
                lat = lat.wrapping_add(i16::from_le_bytes([pair[0], pair[1]]) as i32);
                lon = lon.wrapping_add(i16::from_le_bytes([pair[2], pair[3]]) as i32);
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
    /// settles scatter across the frontier's several live quadtree leaves (measured 2026-07-07 —
    /// grimsel, giant-component endpoints — the reason N4 widened [`NAV_TILE_SLOTS`] to 8, where the
    /// several live leaves stay resident and the per-settle re-fetch mostly hits). Same decode, same
    /// corrupt-input posture, same reentrancy rule as the uncached walk.
    pub fn for_each_nav_node_cached(
        &self,
        view: &BBox,
        tiles: &mut NavTileCache,
        mut visit: impl FnMut(NavNodeRef),
    ) -> Result<(), Error> {
        let dir = self.tables.nav;
        if dir.is_empty() {
            return Ok(());
        }
        self.walk_leaves(&dir, 0, self.bbox, view, 0, &mut |cid, _node| {
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
            if let Some(chunk) = tiles.chunk(self.src, off, dir.chunk_size) {
                decode_nav_chunk(chunk, &mut visit);
            }
        });
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
        let dir = &self.tables.nav;
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
        // byte within+6 is `way_kind` (v9); the anchor shifts to +7 (lat) / +11 (lon).
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
            (
                lon.wrapping_add(i16::from_le_bytes([pair[2], pair[3]]) as i32),
                lat.wrapping_add(i16::from_le_bytes([pair[0], pair[1]]) as i32),
            )
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
            p = (
                p.0.wrapping_sub(i16::from_le_bytes([pair[2], pair[3]]) as i32),
                p.1.wrapping_sub(i16::from_le_bytes([pair[0], pair[1]]) as i32),
            );
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
        for (i, lod) in self.tables.lods.iter().enumerate() {
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
    fn read_node(&self, index: &dyn QuadIndex, idx: usize) -> Option<u32> {
        let off = (index.index_offset() + idx * 4) as u32;
        let mut b = [0u8; 4];
        self.cache.borrow_mut().index_read(self.src, off, &mut b).ok()?;
        Some(u32::from_le_bytes(b))
    }

    /// Visit `(chunk_id, node_bbox)` for every non-empty leaf in `lod` overlapping `view`, in
    /// quadtree order. `lod` indexes [`Reader::lods`]; out-of-range visits nothing. Unlike a
    /// capacity-bounded collect, this streams through a callback with **no upper bound** on the
    /// chunk count — the renderer relies on this so a wide viewport never silently drops chunks.
    /// The walk only reads the index (bbox tests over `u32` nodes), so re-running it once per
    /// priority pass is cheap relative to decoding.
    pub fn for_each_chunk(&self, lod: usize, view: &BBox, mut visit: impl FnMut(u32, BBox)) {
        if let Some(l) = self.tables.lods.get(lod) {
            if l.node_count > 0 {
                self.walk_leaves(l, 0, self.bbox, view, 0, &mut visit);
            }
        }
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
    ) {
        // The depth cap is the hard stack bound against a corrupt cyclic branch (see
        // `MAX_QUADTREE_DEPTH`); a well-formed tree never reaches it.
        if idx >= index.node_count() || depth > MAX_QUADTREE_DEPTH || !node.intersects(view) {
            return;
        }
        // Read the node *before* descending/visiting so the index-cache borrow is released by the
        // time a leaf's `visit` triggers a geometry-chunk read (no nested `RefCell` borrow).
        let val = match self.read_node(index, idx) {
            Some(v) => v,
            None => return,
        };
        if val & BRANCH_BIT == 0 {
            if val != EMPTY_LEAF {
                visit(val, node);
            }
            return;
        }
        let child = (val & !BRANCH_BIT) as usize;
        // The packer flattens the quadtree breadth-first, so a branch's children always lie after
        // it: `child > idx` is a well-formed-map invariant. A back-/self-reference (`child <= idx`)
        // only appears in a corrupt map and would re-enter a node already on the stack; reject it
        // (the depth cap above is the backstop, this stops the most direct cycle at its source).
        if child <= idx {
            return;
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
            self.walk_leaves(index, child + i, *kb, view, depth + 1, visit);
        }
    }

    /// Decode every feature in a chunk of `lod`, invoking `visit` once per feature with a
    /// [`FeatureRef`] borrowing the caller's `points`/`ring_lens` scratch. Allocation-free: the
    /// buffers grow to the largest feature once and are reused across features/chunks/frames.
    /// `node` is the leaf bbox yielded by [`Reader::for_each_chunk`].
    ///
    /// # Reentrancy
    ///
    /// The internal cache `RefCell` borrow is held while `visit` runs. A callback may read the
    /// resident tables ([`Reader::style`], [`Reader::backdrop_style`], [`Reader::lods`],
    /// [`Reader::select_lod_for_mpp`]) but must **not** call any `Reader` method that streams from
    /// the source — [`Reader::for_each_chunk`] or another `for_each_feature*` — which re-borrows
    /// the cache and panics at runtime with a borrow error.
    pub fn for_each_feature<const P: usize, const R: usize>(
        &self,
        lod: usize,
        chunk_id: u32,
        node: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        visit: impl FnMut(FeatureRef),
    ) {
        self.for_each_feature_filtered(lod, chunk_id, node, points, ring_lens, |_| true, visit);
    }

    /// Like [`Reader::for_each_feature`], but `should_decode` is consulted with each feature's
    /// style id **before** its coordinates are decoded: `false` skips the geometry cheaply
    /// (advancing past its bytes with no coordinate math), `true` decodes it and hands a
    /// [`FeatureRef`] to `visit`. The renderer uses this so each priority pass decodes only its own
    /// features — across all passes a feature's coordinates decode **at most once per frame**.
    ///
    /// # Reentrancy
    ///
    /// The internal cache `RefCell` borrow is held while `should_decode` and `visit` run. A
    /// callback may read the resident tables ([`Reader::style`], [`Reader::backdrop_style`],
    /// [`Reader::lods`], [`Reader::select_lod_for_mpp`]) but must **not** call any `Reader` method
    /// that streams from the source — [`Reader::for_each_chunk`] or another `for_each_feature*` —
    /// which re-borrows the cache and panics at runtime with a borrow error.
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
    ) {
        let l = match self.tables.lods.get(lod) {
            Some(l) => l,
            None => return,
        };
        // `chunk_id` is unvalidated file data: reject an out-of-range id or an offset overflowing
        // `usize` (32-bit on device) instead of panicking or decoding an adjacent region.
        let (start, end) = match l.chunk_range(chunk_id) {
            Some(range) => range,
            None => return,
        };
        if end > self.src.len() as usize {
            return;
        }
        // `chunk_size` was capped at `MAX_CHUNK_BYTES` in `parse`; this defensive check keeps a
        // corrupt LOD from indexing past the decode scratch.
        let len = end - start;
        if len > MAX_CHUNK_BYTES {
            return;
        }
        // Pull the chunk through the cache, then decode from the resident bytes. The borrow is held
        // across `decode_chunk_into` — safe because `should_decode`/`visit` only touch
        // `self.tables.styles`, never the cache.
        let mut cache = self.cache.borrow_mut();
        let loc = match cache.load_chunk(self.src, lod as u8, chunk_id, start as u32, len) {
            Ok(loc) => loc,
            Err(_) => return,
        };
        let chunk = match loc {
            ChunkLoc::Slot(i) => &cache.chunks[i].buf[..len],
            ChunkLoc::Scratch => &cache.scratch[..len],
        };
        decode_chunk_into(chunk, node, points, ring_lens, should_decode, visit);
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
    /// yields `None`, never a panic or an out-of-chunk read. Fetches the chunk through the same
    /// cache as the full walk, so consecutive calls for one `cid` (pass B visits a chunk's winners
    /// together) hit the resident slot instead of re-reading it.
    ///
    /// # Reentrancy
    ///
    /// Same rule as [`Reader::for_each_feature_filtered`]: this borrows the internal cache for the
    /// fetch + decode. Call it from a [`Reader::for_each_chunk`] visit callback (which holds no
    /// cache borrow) — as pass B does — but not from inside a `for_each_feature*` callback, which
    /// still holds the borrow.
    pub fn decode_feature_at<'p, const P: usize, const R: usize>(
        &self,
        lod: usize,
        cid: u32,
        offset: usize,
        node: &BBox,
        points: &'p mut Vec<(i32, i32), P>,
        ring_lens: &'p mut Vec<usize, R>,
    ) -> Option<FeatureRef<'p>> {
        let l = self.tables.lods.get(lod)?;
        let (start, end) = l.chunk_range(cid)?;
        if end > self.src.len() as usize {
            return None;
        }
        let len = end - start;
        // Same corrupt-chunk guards as the full walk, plus the offset must land inside the chunk.
        if len > MAX_CHUNK_BYTES || offset >= len {
            return None;
        }
        let mut cache = self.cache.borrow_mut();
        let loc = cache.load_chunk(self.src, lod as u8, cid, start as u32, len).ok()?;
        let chunk = match loc {
            ChunkLoc::Slot(i) => &cache.chunks[i].buf[..len],
            ChunkLoc::Scratch => &cache.scratch[..len],
        };
        // The `FeatureRef` borrows `points`/`ring_lens` (its coordinates are copied there), not the
        // cache bytes, so it outlives the `cache` borrow dropped at return.
        decode_one_feature(chunk, offset, node, points, ring_lens).map(|(fref, _)| fref)
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

/// Decode a POI record's name (spec §7.3) from `buf` at record offset `off`: `name_len` at `off+9`,
/// the up-to-24-byte `Name` at `off+10` (bytes `[off+10 .. off+34]`; `hours_ref` follows at
/// `[off+34 .. off+36]`). Empty for an unnamed record (`name_len == 0`). The stored name is already
/// pre-folded printable ASCII, but this stays defensive — `name_len` is clamped to what the field
/// and the buffer hold, and any non-printable byte (a corrupt record) is dropped — so a bad chunk
/// yields a short/empty name, never a panic or garbage glyph.
fn decode_poi_name(buf: &[u8], off: usize) -> heapless::String<POI_NAME_MAX> {
    let mut name = heapless::String::new();
    let name_off = off + 10;
    // Clamp to the 24-byte field and to the bytes actually present in the buffer.
    let len = (buf[off + 9] as usize).min(POI_NAME_MAX).min(buf.len().saturating_sub(name_off));
    for &b in &buf[name_off..name_off + len] {
        // Printable ASCII only (the device font's range); drop anything else rather than trust a
        // corrupt byte. `push` can't fail — `len <= POI_NAME_MAX` == the String capacity.
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
) {
    let cs = chunk.len();
    let mut off = 0usize;

    while off + 12 <= cs {
        if chunk[off] == 0xFF {
            break;
        }
        let style_id = chunk[off];

        // Skip path: the caller doesn't want this style this pass, so advance past the geometry
        // without decoding (read only the header fields the skip needs).
        if !should_decode(style_id) {
            let ext_pt_count = rd_u16(chunk, off + 1) as usize;
            let flags = chunk[off + 11];
            let is_16 = flags & FEATURE_FLAG_16BIT != 0;
            let is_poly = flags & FEATURE_FLAG_POLYGON != 0;
            let has_holes = flags & FEATURE_FLAG_HOLES != 0;
            let dsize = if is_16 { 2 } else { 1 };
            off += 12;
            off = skip_ring(chunk, off, ext_pt_count, false, dsize);
            if is_poly && has_holes {
                off = for_each_hole(chunk, off, |c, o, hpc| skip_ring(c, o, hpc, true, dsize));
            }
            continue;
        }

        match decode_one_feature(chunk, off, node, points, ring_lens) {
            Some((fref, next)) => {
                visit(fref);
                off = next;
            }
            // A `0xFF` end-marker or a header that runs off the chunk end stops the walk.
            None => break,
        }
    }
}

/// Decode the single feature whose 12-byte header starts at `off` in `chunk`, into `points`/
/// `ring_lens` (cleared first), returning its [`FeatureRef`] (borrowing those buffers, with
/// [`FeatureRef::offset`] set to `off`) plus the offset just past it. `None` if `off` leaves no room
/// for a header or lands on the `0xFF` end-marker — so it is safe to call with an untrusted `off`
/// (issue #564's pass-B re-decode hands back a `FeatureRef::offset` from earlier this frame). `node`
/// gives the leaf's min corner, the per-feature anchor base. This is the exact decode
/// [`decode_chunk_into`] runs, so a feature decodes byte-for-byte identically whether it comes from
/// the full-chunk walk or from [`Reader::decode_feature_at`].
fn decode_one_feature<'b, const P: usize, const R: usize>(
    chunk: &[u8],
    off: usize,
    node: &BBox,
    points: &'b mut Vec<(i32, i32), P>,
    ring_lens: &'b mut Vec<usize, R>,
) -> Option<(FeatureRef<'b>, usize)> {
    if off + 12 > chunk.len() || chunk[off] == 0xFF {
        return None;
    }
    let feat_off = off;
    let style_id = chunk[off];
    let ext_pt_count = rd_u16(chunk, off + 1) as usize;
    let ax = rd_i32(chunk, off + 3);
    let ay = rd_i32(chunk, off + 7);
    let flags = chunk[off + 11];
    let mut off = off + 12;

    let is_16 = flags & FEATURE_FLAG_16BIT != 0;
    let is_poly = flags & FEATURE_FLAG_POLYGON != 0;
    let has_holes = flags & FEATURE_FLAG_HOLES != 0;
    let dsize = if is_16 { 2 } else { 1 };

    let anchor = (node.min_lon + ax, node.min_lat + ay);

    points.clear();
    ring_lens.clear();
    let mut bounds = Bounds::new();

    off = read_ring(chunk, off, ext_pt_count, anchor, is_16, dsize, false, points, &mut bounds);
    let _ = ring_lens.push(points.len());

    if is_poly && has_holes {
        off = for_each_hole(chunk, off, |c, o, hpc| {
            let before = points.len();
            let o = read_ring(c, o, hpc, anchor, is_16, dsize, true, points, &mut bounds);
            let _ = ring_lens.push(points.len() - before);
            o
        });
    }

    let fref = FeatureRef {
        style_id,
        kind: if is_poly { Kind::Polygon } else { Kind::Line },
        points,
        ring_lens,
        bbox: bounds.to_bbox(),
        offset: feat_off,
    };
    Some((fref, off))
}

/// Walk a polygon's hole list: read the 1-byte hole count at `off`, then per hole read its `u16`
/// point count and hand `(chunk, off, hpc)` to `ring` (which decodes or skips and returns the
/// post-ring offset); returns the offset past the whole block. The skip and decode paths share this
/// framing so their byte arithmetic can't drift. No-op when `off` is already at the chunk end.
#[inline]
fn for_each_hole(chunk: &[u8], mut off: usize, mut ring: impl FnMut(&[u8], usize, usize) -> usize) -> usize {
    let cs = chunk.len();
    if off >= cs {
        return off;
    }
    let hole_count = chunk[off] as usize;
    off += 1;
    for _ in 0..hole_count {
        if off + 2 > cs {
            break;
        }
        let hpc = rd_u16(chunk, off) as usize;
        off += 2;
        off = ring(chunk, off, hpc);
    }
    off
}

/// Advance `off` past one ring's encoded deltas without decoding, mirroring [`read_ring`]'s offset
/// arithmetic exactly so skip and decode stay byte-aligned. `is_hole` selects the hole encoding
/// (every point a delta) vs the exterior encoding (first point is the anchor, not stored).
fn skip_ring(chunk: &[u8], off: usize, pt_count: usize, is_hole: bool, dsize: usize) -> usize {
    if pt_count == 0 {
        return off;
    }
    let num_deltas = if is_hole { pt_count } else { pt_count - 1 };
    let step = dsize * 2;
    // Common case: the whole ring fits in the chunk — one multiply, no division.
    let want = num_deltas * step;
    let remain = chunk.len().saturating_sub(off);
    if want <= remain {
        return off + want;
    }
    // Truncated ring: advance by whole delta steps only — mirrors the old loop's
    // `off + step > len ⇒ break`, so skip and decode stay byte-for-byte aligned.
    off + (remain / step) * step
}

#[allow(clippy::too_many_arguments)]
fn read_ring<const P: usize>(
    chunk: &[u8],
    mut off: usize,
    pt_count: usize,
    anchor: (i32, i32),
    is_16: bool,
    dsize: usize,
    is_hole: bool,
    out: &mut Vec<(i32, i32), P>,
    bounds: &mut Bounds,
) -> usize {
    if pt_count == 0 {
        return off;
    }
    let (mut px, mut py) = anchor;
    let num_deltas = if is_hole {
        // holes store all points as deltas (first relative to anchor)
        pt_count
    } else {
        let _ = out.push(anchor);
        bounds.add(anchor.0, anchor.1);
        pt_count - 1
    };
    for _ in 0..num_deltas {
        if off + dsize * 2 > chunk.len() {
            break;
        }
        let (dx, dy) = if is_16 {
            (
                i16::from_le_bytes([chunk[off], chunk[off + 1]]) as i32,
                i16::from_le_bytes([chunk[off + 2], chunk[off + 3]]) as i32,
            )
        } else {
            (chunk[off] as i8 as i32, chunk[off + 1] as i8 as i32)
        };
        off += dsize * 2;
        px += dx;
        py += dy;
        let _ = out.push((px, py));
        bounds.add(px, py);
    }
    off
}

/// Parse the style table, read resident from `src` at `style_offset` (file is `total` bytes) into
/// the caller's `styles` (cleared first). The table is small (≤ `1 + 256*6` bytes) so it's pulled
/// in two reads (count byte, then records). A truncated *table* is tolerated — the `o + 6 > want`
/// break stops at the last whole record rather than reading past it — but a failed *read* (flaky
/// card) or a `style_offset` at/past EOF (corrupt header) is [`Error::BadOffset`]: an all-`None`
/// table would let the map load "fine" and render nothing, with no error to surface.
///
/// Out-param + `inline(never)`, deliberately: with the array in the return value this single-call-
/// site function inlined its ~3.5 KB of scratch (the 1.5 KB record buffer plus the `Result` array
/// temporaries) into `MapTables::parse` and on into the device `main`'s **permanent** frame — every
/// stack watermark rose by ~3.8 KB and the DK's ride path overflowed (HardFault). The scratch must
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
    src.read_at(style_offset as u32, &mut cb).map_err(|_| Error::BadOffset)?;
    let count = cb[0] as usize;
    // `count*6` record bytes follow the count, clamped to what the file holds so the `o + 6 > want`
    // break below stops at the last whole record in a truncated table.
    let avail = total - (style_offset + 1);
    let want = (count * 6).min(avail);
    let mut buf = [0u8; 256 * 6];
    if want > 0 {
        src.read_at((style_offset + 1) as u32, &mut buf[..want]).map_err(|_| Error::BadOffset)?;
    }
    let mut o = 0usize;
    for _ in 0..count {
        if o + 6 > want {
            break;
        }
        let id = buf[o];
        let z_index = buf[o + 1] as i8;
        let color = rd_u16(&buf, o + 2);
        let weight = buf[o + 4];
        let flags = buf[o + 5];
        let priority = (flags & STYLE_PRIORITY_MASK) + 1;
        styles[id as usize] = Some(Style { id, z_index, color, weight, priority });
        o += 6;
    }
    Ok(())
}

/// Parse the `lod_count` LOD-table entries (resident from `src`); validates each layer's
/// index/chunk region lies within the file (`total` bytes) so `for_each_chunk`/`decode_chunk` can skip
/// bounds math, and that its `chunk_size` fits the decode scratch ([`MAX_CHUNK_BYTES`]).
fn parse_lod_table(src: &dyn ByteSource, offset: usize, lod_count: usize, total: usize) -> Result<Vec<Lod, 16>, Error> {
    let mut lods = Vec::new();
    let mut e = [0u8; LOD_ENTRY_LEN];
    for k in 0..lod_count {
        let o = offset + k * LOD_ENTRY_LEN;
        src.read_at(o as u32, &mut e).map_err(|_| Error::BadOffset)?;
        let lod = Lod {
            max_mpp: rd_f32(&e, 0),
            index_offset: rd_u32(&e, 4) as usize,
            node_count: rd_u32(&e, 8) as usize,
            chunk_size: rd_u16(&e, 12) as usize,
            chunk_count: rd_u32(&e, 14) as usize,
        };
        // Checked: a corrupt entry's `node_count`/`chunk_count`/`chunk_size` products can wrap
        // `usize` on the 32-bit target, so an unchecked `chunks_end` could land below `total` and
        // admit a layer indexing out of the file.
        let chunks_end = lod
            .data_start()
            .and_then(|start| lod.chunk_count.checked_mul(lod.chunk_size).and_then(|len| start.checked_add(len)))
            .ok_or(Error::BadOffset)?;
        if lod.index_offset < HEADER_LEN || chunks_end > total {
            return Err(Error::BadOffset);
        }
        // A chunk decodes into the resident scratch, so reject a `chunk_size` over
        // [`MAX_CHUNK_BYTES`] rather than silently dropping its geometry at render time.
        if lod.chunk_size > MAX_CHUNK_BYTES {
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
    src.read_at(offset as u32, &mut hdr).map_err(|_| Error::BadOffset)?;
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
        src.read_at(o as u32, &mut e).map_err(|_| Error::BadOffset)?;
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
    src.read_at(pool_fields_off as u32, &mut pf).map_err(|_| Error::BadOffset)?;
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
/// **Byte-wise by contract — never a typed view.** The record stride is 13 + 20·degree bytes
/// (odd + even), so records — and every multi-byte field in them — sit at **odd offsets** inside
/// the chunk by design; all decoding goes through the `rd_*` `from_le_bytes`-on-`&[u8]` helpers.
/// Two guards keep it that way (PR #501's on-glass HardFault dossier): the board build compiles
/// with `+strict-align` (the ARM backend fused even these byte-wise decodes into an
/// alignment-trapping `ldrd` under fat LTO — see `obc-fw-nrf54l/.cargo/config.toml`), and the
/// obc-route nav suite runs clean under **Miri** (`cargo +nightly miri test -p obc-route --test
/// nav`), which fails loudly if a typed view over these bytes ever creeps in.
fn decode_nav_chunk(chunk: &[u8], visit: &mut impl FnMut(NavNodeRef)) {
    let mut off = 0usize;
    while off + NAV_NODE_FIXED_LEN <= chunk.len() {
        let degree = chunk[off + 12] as usize;
        if degree == 0xFF {
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

/// Parse the v9 nav directory (spec §8.1) at `offset` from `src` (file is `total` bytes).
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
    src.read_at(offset as u32, &mut d).map_err(|_| Error::BadOffset)?;
    let dir = NavDirectory {
        index_offset: rd_u32(&d, 0) as usize,
        node_count: rd_u32(&d, 4) as usize,
        chunk_count: rd_u32(&d, 8) as usize,
        edge_pool_offset: rd_u32(&d, 12) as usize,
        edge_chunk_count: rd_u32(&d, 16) as usize,
        chunk_size: rd_u16(&d, 20) as usize,
        profile_table_offset: rd_u32(&d, 22) as usize,
        profile_count: d[26] as usize,
    };
    // v9 pins the nav chunk size to 512 (§8.1) — a v8 file, or any other value, is rejected. This is
    // a distinct error from the header's version check, so an old file and a mis-sized v9 file are
    // told apart.
    if dir.chunk_size != NAV_CHUNK_SIZE {
        return Err(Error::BadOffset);
    }
    // The profile table is always present with 1..=8 records (§8.6) — a zero or oversize count is a
    // malformed file, not a degraded one.
    if dir.profile_count == 0 || dir.profile_count > NAV_MAX_PROFILES {
        return Err(Error::BadOffset);
    }
    // Profile-table region (52 B × count) at `profile_table_offset` must lie in-file.
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
    Ok(dir)
}

/// Parse the §8.6 profile table into `MapTables`: `dir.profile_count` (1..=8, already range-checked
/// by [`parse_nav_directory`]) consecutive 52-byte records at `dir.profile_table_offset`. Each
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
        src.read_at(off, &mut buf).map_err(|_| Error::BadOffset)?;
        let mut name = [0u8; NAV_PROFILE_NAME_LEN];
        name.copy_from_slice(&buf[0..NAV_PROFILE_NAME_LEN]);
        // Name length = bytes up to the first 0xFF pad (the §7/§8 name convention).
        let name_len = name.iter().position(|&b| b == 0xFF).unwrap_or(NAV_PROFILE_NAME_LEN);
        let mut highway = [0u8; 32];
        highway.copy_from_slice(&buf[12..44]);
        let mut surface = [0u8; 8];
        surface.copy_from_slice(&buf[44..52]);
        for m in highway.iter_mut().chain(surface.iter_mut()) {
            if *m != 0 && *m < 16 {
                *m = 16; // clamp an inadmissible weight up to 1.0× (defensive; the packer forbids it)
            }
        }
        // `push` can't fail: the loop runs `profile_count ≤ NAV_MAX_PROFILES` times (checked above).
        let _ = out.push(MapProfile { name, name_len, highway, surface });
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
    Scratch,
}

/// One geometry-chunk cache slot: a resident copy of a `chunk_size`-byte chunk, tagged with its
/// `(lod, chunk_id)` and a recency stamp for LRU eviction. `valid` (over `Option`) makes the
/// all-zero state a valid *empty* slot, so [`MapCacheInner::new`] can zero-init the whole cache.
struct ChunkSlot {
    valid: bool,
    lod: u8,
    cid: u32,
    len: usize,
    used: u32,
    buf: [u8; CACHE_SLOT_BYTES],
}

/// One quadtree-index cache block: a resident, block-aligned window of the index region. `valid`
/// plays the same all-zero-is-empty role as in [`ChunkSlot`].
struct IndexBlock {
    valid: bool,
    off: u32,
    len: usize,
    used: u32,
    buf: [u8; INDEX_BLOCK],
}

/// The streamed-map cache: an LRU set of geometry-chunk slots (absorbing the renderer's
/// per-priority-pass re-reads) plus a small block cache for the quadtree-node reads, with the
/// streaming counters. Caller-owned and reusable across frames (a chunk read one frame can hit the
/// next). ≈277 KB, dominated by the slots + decode scratch; tune the slot count / `CACHE_SLOT_BYTES`
/// against the on-device RAM budget.
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
    /// A fresh, empty cache. ≈277 KB of zeroed buffers — on the device, place it once in the
    /// reserved region (e.g. `ptr::write`, like the `App`) so it stays off the 192 KB main stack.
    pub fn new() -> Self {
        MapCache { inner: RefCell::new(MapCacheInner::new()) }
    }

    /// Drop every resident chunk + index slot. Slots are keyed only by `(lod, chunk_id, len)` /
    /// index offset, *not* by which map produced them, so a slot left resident across a map switch
    /// would cross-serve as a (wrong-geometry) hit — but [`Reader::new`] guards that structurally
    /// via [`MapCache::adopt`], so calling this on a switch is still correct, just no longer
    /// load-bearing. Cheap — only the `valid` flags + counters are touched, not the buffers.
    pub fn clear(&self) {
        self.inner.borrow_mut().clear();
    }

    /// Bind the cache to a [`MapTables`] parse `generation`, running the [`MapCache::clear`] logic
    /// first if it last served a different one. Called by [`Reader::new`], which is what makes the
    /// forgotten-`clear()`-on-map-switch cross-serve impossible by construction. A zeroed cache
    /// sits at generation 0 — never a live generation — so the first adopt after boot clears an
    /// already-empty cache (harmless).
    fn adopt(&self, generation: u32) {
        let mut inner = self.inner.borrow_mut();
        if inner.generation != generation {
            inner.clear();
            inner.generation = generation;
        }
    }

    #[inline]
    fn borrow_mut(&self) -> RefMut<'_, MapCacheInner> {
        self.inner.borrow_mut()
    }

    #[inline]
    fn stats(&self) -> CacheStats {
        self.inner.borrow().stats()
    }
}

/// The cache's mutable interior (see [`MapCache`]). Recency is a monotonic `tick` stamped on each
/// access; eviction picks the lowest stamp.
struct MapCacheInner {
    /// The [`MapTables::parse`] generation the resident slots belong to; 0 (the zero-init state)
    /// means "unowned". Written only by [`MapCache::adopt`] — deliberately *not* reset by `clear`,
    /// which empties the slots and so is safe under any generation.
    generation: u32,
    tick: u32,
    chunks: [ChunkSlot; MAP_CHUNK_SLOTS],
    index: [IndexBlock; INDEX_BLOCKS],
    /// Decode buffer for a chunk too large to cache (`> CACHE_SLOT_BYTES`, up to the accepted
    /// `MAX_CHUNK_BYTES`); never keyed, so such a chunk is re-read every pass.
    scratch: [u8; MAX_CHUNK_BYTES],
    chunk_hits: u32,
    chunk_misses: u32,
    sd_reads: u32,
    bytes_read: u32,
}

impl MapCacheInner {
    fn new() -> Self {
        // Zero-init via `zeroed()` (all-zero is a valid empty state for every field). This lowers
        // to a `memset` / `.bss`, whereas a struct literal zeroing the ~84 KB of buffers emits them
        // as a `.rodata` const that is then `memcpy`'d — which overflowed flash on the device.
        //
        // SAFETY: `MapCacheInner` is inhabited and valid for the all-zero bit pattern — it has no
        // references, no enums with a non-zero discriminant, and no `bool` that must be non-zero
        // (the only `bool`s are the `valid` flags, false at zero). No padding is read.
        unsafe { core::mem::MaybeUninit::zeroed().assume_init() }
    }

    /// Drop every resident slot and zero the counters, touching only the `valid` flags + counters,
    /// not the ≈84 KB of backing buffers. See [`MapCache::clear`].
    fn clear(&mut self) {
        for s in &mut self.chunks {
            s.valid = false;
        }
        for b in &mut self.index {
            b.valid = false;
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
    fn touch(&mut self) -> u32 {
        self.tick = self.tick.wrapping_add(1);
        self.tick
    }

    #[inline]
    fn count_read(&mut self, bytes: usize) {
        self.sd_reads = self.sd_reads.saturating_add(1);
        self.bytes_read = self.bytes_read.saturating_add(bytes as u32);
    }

    /// Ensure chunk `(lod, cid)` — the `len` bytes at `start` — is resident, returning where its
    /// bytes are. A chunk that fits a cache slot is cached: a hit bumps recency + the hit
    /// counter, a miss evicts the LRU slot and reads from the source. A chunk larger than a slot
    /// is read into the uncached scratch every call (counted as a miss + a read).
    fn load_chunk(
        &mut self,
        src: &dyn ByteSource,
        lod: u8,
        cid: u32,
        start: u32,
        len: usize,
    ) -> Result<ChunkLoc, IoError> {
        if len > CACHE_SLOT_BYTES {
            src.read_at(start, &mut self.scratch[..len])?;
            self.chunk_misses = self.chunk_misses.saturating_add(1);
            self.count_read(len);
            return Ok(ChunkLoc::Scratch);
        }
        if let Some(i) = self.chunks.iter().position(|s| s.valid && s.lod == lod && s.cid == cid && s.len == len) {
            self.chunk_hits = self.chunk_hits.saturating_add(1);
            let t = self.touch();
            self.chunks[i].used = t;
            return Ok(ChunkLoc::Slot(i));
        }
        let i = lru(self.chunks.iter().map(|s| (!s.valid, s.used)));
        // Invalidate before the read: a flaky source can fail partway, half-overwriting the buffer.
        // Committing `valid`/keys only after the read succeeds means a failed read leaves an empty
        // slot, not a poisoned one keyed to the old chunk (which would serve as a corrupt hit).
        self.chunks[i].valid = false;
        src.read_at(start, &mut self.chunks[i].buf[..len])?;
        self.chunks[i].valid = true;
        self.chunks[i].lod = lod;
        self.chunks[i].cid = cid;
        self.chunks[i].len = len;
        let t = self.touch();
        self.chunks[i].used = t;
        self.chunk_misses = self.chunk_misses.saturating_add(1);
        self.count_read(len);
        Ok(ChunkLoc::Slot(i))
    }

    /// Fill `out` from index-region offset `off`, assembling from cached blocks (reading any
    /// missing block from the source). A node read is 4 bytes and may straddle a block edge, so
    /// this loops over blocks.
    fn index_read(&mut self, src: &dyn ByteSource, off: u32, out: &mut [u8]) -> Result<(), IoError> {
        let mut filled = 0usize;
        while filled < out.len() {
            let cur = off + filled as u32;
            let block_off = cur - cur % INDEX_BLOCK as u32;
            let slot = self.index_block(src, block_off)?;
            let within = (cur - block_off) as usize;
            let blen = self.index[slot].len;
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
    fn index_block(&mut self, src: &dyn ByteSource, block_off: u32) -> Result<usize, IoError> {
        if let Some(i) = self.index.iter().position(|b| b.valid && b.off == block_off) {
            let t = self.touch();
            self.index[i].used = t;
            return Ok(i);
        }
        let want = ((src.len() - block_off) as usize).min(INDEX_BLOCK);
        if want == 0 {
            return Err(IoError::BadOffset);
        }
        let i = lru(self.index.iter().map(|b| (!b.valid, b.used)));
        // Invalidate before the read (see `load_chunk`): a partial read failure must not leave a
        // poisoned slot still keyed to the old block offset.
        self.index[i].valid = false;
        src.read_at(block_off, &mut self.index[i].buf[..want])?;
        self.index[i].valid = true;
        self.index[i].off = block_off;
        self.index[i].len = want;
        let t = self.touch();
        self.index[i].used = t;
        self.count_read(want);
        Ok(i)
    }
}

/// Pick a slot to (re)fill: the first empty slot if any, else the least-recently-used. Input is
/// `(is_empty, used)` per slot in order; returns the chosen index. Never empty in practice (the
/// cache always has ≥ 1 slot).
fn lru(slots: impl Iterator<Item = (bool, u32)>) -> usize {
    let mut best = 0usize;
    let mut best_used = u32::MAX;
    for (i, (empty, used)) in slots.enumerate() {
        if empty {
            return i;
        }
        if used < best_used {
            best_used = used;
            best = i;
        }
    }
    best
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

    /// The N4 graph-tile cache holds [`NAV_TILE_SLOTS`] (8) distinct chunks resident at once, and
    /// round-robin eviction drops the **oldest** on the next miss: prime 8 → re-touch all 8 hit → a
    /// 9th evicts slot 0's chunk while the rest stay resident. Guards the widened geometry (2→8) the
    /// measurements motivated.
    #[test]
    fn nav_tile_cache_holds_eight_and_evicts_round_robin() {
        const LEN: usize = NAV_CHUNK_SIZE; // 512, = one pinned v9 nav chunk
                                           // NAV_TILE_SLOTS + 1 distinct chunks; every byte of chunk k is `k`, so contents are checkable.
        let mut data = [0u8; (NAV_TILE_SLOTS + 1) * LEN];
        for (k, b) in data.iter_mut().enumerate() {
            *b = (k / LEN) as u8;
        }
        let src = SliceSource(&data);
        let mut cache = NavTileCache::new();
        let off = |i: usize| (i * LEN) as u32;

        // Prime all 8 slots: 8 misses, contents correct.
        for i in 0..NAV_TILE_SLOTS {
            assert_eq!(cache.chunk(&src, off(i), LEN).unwrap()[0], i as u8);
        }
        assert_eq!(cache.stats(), NavCacheStats { hits: 0, misses: NAV_TILE_SLOTS as u32 });

        // Re-touch all 8 — every one still resident ⇒ 8 hits, no new read.
        for i in 0..NAV_TILE_SLOTS {
            assert_eq!(cache.chunk(&src, off(i), LEN).unwrap()[0], i as u8);
        }
        assert_eq!(cache.stats(), NavCacheStats { hits: NAV_TILE_SLOTS as u32, misses: NAV_TILE_SLOTS as u32 });

        // A 9th distinct chunk misses and evicts the oldest (round-robin cursor = slot 0 = chunk 0).
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

    /// A read that fails partway must leave the evicted slot *empty*, not poisoned with the old key
    /// over a half-overwritten buffer — otherwise a later request for the old key is served as a
    /// corrupt hit.
    #[test]
    fn partial_read_failure_does_not_poison_evicted_slot() {
        const LEN: usize = 64;
        // One LEN-chunk per slot, plus one more past them for the failing eviction read — sized off
        // MAP_CHUNK_SLOTS so the test tracks the cache size rather than a hard-coded buffer length.
        let mut data = [0u8; (MAP_CHUNK_SLOTS + 1) * LEN];
        for (k, b) in data.iter_mut().enumerate() {
            *b = (k as u8).wrapping_mul(31).wrapping_add(7); // distinct, offset-derived bytes
        }
        // The eviction read (K_new) lives past the primed chunks (one per slot) and fails partway.
        let fail_at = (MAP_CHUNK_SLOTS as u32) * LEN as u32;
        let src = FlakySource { data: &data, fail_at, partial: 8 };

        let cache = MapCache::new();
        let mut inner = cache.borrow_mut();

        // Prime all slots, oldest first — so the LRU victim of the next miss is slot 0 (cid 0).
        for cid in 0..MAP_CHUNK_SLOTS as u32 {
            let loc = inner.load_chunk(&src, 0, cid, cid * LEN as u32, LEN).unwrap();
            assert!(matches!(loc, ChunkLoc::Slot(_)));
        }
        let primed = inner.stats();
        assert_eq!(primed.chunk_misses, MAP_CHUNK_SLOTS as u32);
        assert_eq!(primed.chunk_hits, 0);

        // The true bytes of K_old (cid 0), for an uncorrupted-content check after re-read.
        let mut k_old = [0u8; LEN];
        src.read_at(0, &mut k_old).unwrap();

        // Eviction read of K_new fails partway through filling slot 0's buffer.
        assert!(matches!(inner.load_chunk(&src, 0, 99, fail_at, LEN), Err(IoError::Io)));

        // Request K_old again: it must be a *miss* (re-read), not a hit on the poisoned slot.
        let before = inner.stats();
        let loc = inner.load_chunk(&src, 0, 0, 0, LEN).unwrap();
        let after = inner.stats();
        assert_eq!(after.chunk_hits, before.chunk_hits, "K_old must not hit the poisoned slot");
        assert_eq!(after.chunk_misses, before.chunk_misses + 1, "K_old must be re-read");

        // …and the re-read returns the real K_old bytes, not the half-written K_new.
        match loc {
            ChunkLoc::Slot(i) => assert_eq!(&inner.chunks[i].buf[..LEN], &k_old[..]),
            ChunkLoc::Scratch => panic!("a slot-sized chunk should land in a slot"),
        }
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
                &[(1, 3, 0xF800, 2, 3)],
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
            });
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

    /// A style-table read that *fails* (flaky card) must surface as a parse error, not an
    /// all-`None` table that loads "fine" and renders nothing. Exercises both reads — the count
    /// byte and the record block — via a `FlakySource` failing at exactly that offset. (A
    /// physically *truncated* table is still tolerated; see `extremes.rs`.)
    #[test]
    fn failed_style_table_read_errors_map_parse() {
        use obcm_testkit::{build_file, pack_line, pad, LodSpec};
        let bytes = build_file(
            (0, 0, 1000, 1000),
            &[(1, 3, 0xF800, 2, 3)],
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
                matches!(MapTables::parse(&src), Err(Error::BadOffset)),
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
        let mut inner = cache.borrow_mut();

        // Resident, then a hit (no source read).
        inner.index_block(&src, 0).unwrap();
        let before = inner.stats();
        inner.index_block(&src, 0).unwrap();
        assert_eq!(inner.stats().sd_reads, before.sd_reads, "a resident block must hit, not re-read");
        drop(inner);

        // After clear the same offset must miss and re-read from the source.
        cache.clear();
        let mut inner = cache.borrow_mut();
        let before = inner.stats();
        inner.index_block(&src, 0).unwrap();
        assert_eq!(inner.stats().sd_reads, before.sd_reads + 1, "post-clear index read must re-read");
    }

    /// A minimal 40-byte OBCM header with the given bbox/marker, enough for the cache-free
    /// [`read_header`] (no style/LOD tables, which it doesn't touch).
    fn synth_header(min_lon: i32, min_lat: i32, max_lon: i32, max_lat: i32, marker: u16) -> [u8; HEADER_LEN] {
        let mut h = [0u8; HEADER_LEN];
        h[0..4].copy_from_slice(b"OBCM");
        h[4] = 9;
        h[5..9].copy_from_slice(&min_lat.to_le_bytes()); // field order is lat,lon,lat,lon
        h[9..13].copy_from_slice(&min_lon.to_le_bytes());
        h[13..17].copy_from_slice(&max_lat.to_le_bytes());
        h[17..21].copy_from_slice(&max_lon.to_le_bytes());
        h[30..32].copy_from_slice(&marker.to_le_bytes());
        // h[32..36] (POI section offset) and h[36..40] (nav offset) are untouched by read_header.
        h
    }

    /// `read_header` pulls version + bbox + marker out of the header alone.
    #[test]
    fn read_header_decodes_bbox_and_marker() {
        let h = synth_header(-34, 12, 78, 56, 0xBEEF);
        let got = read_header(&SliceSource(&h)).unwrap();
        assert_eq!(
            got,
            MapHeader {
                version: 9,
                bbox: BBox { min_lon: -34, min_lat: 12, max_lon: 78, max_lat: 56 },
                marker_color: 0xBEEF
            }
        );
    }

    /// The same magic / version / length guards as `MapTables::parse` — a bad card never decodes to
    /// a bogus bbox.
    #[test]
    fn read_header_rejects_short_bad_magic_and_version() {
        assert_eq!(read_header(&SliceSource(&[0u8; HEADER_LEN - 1])), Err(Error::TooShort));
        let mut h = synth_header(0, 0, 1, 1, 0);
        h[0..4].copy_from_slice(b"NOPE");
        assert_eq!(read_header(&SliceSource(&h)), Err(Error::BadMagic));
        h[0..4].copy_from_slice(b"OBCM");
        h[4] = 8; // v8 (and earlier) no longer supported — only v9 is read
        assert_eq!(read_header(&SliceSource(&h)), Err(Error::BadVersion));
    }
}
