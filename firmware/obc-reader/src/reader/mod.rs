//! OBCM format reader: header, style table, LOD table, per-LOD quadtree query and chunk decode,
//! the POI directory with its hours pool, and the nav-graph section (parse and leaf walk only).
//!
//! All coordinates are integer microdegrees, as stored in the file. Projection to screen space is
//! the renderer's job.
//!
//! The reader streams through a [`ByteSource`]: only the header, style table and LOD table stay
//! resident (parsed once in [`MapTables`]); the quadtree index and geometry chunks are pulled on
//! demand, so the whole `.obcm` never has to fit in RAM.
//!
//! `read_at` takes `&self`, so the lazy reads go through an internal [`MapCache`] behind a
//! `RefCell`. The cache changes only *when* a byte is read, never *what* decodes, so renders stay
//! byte-identical.

mod cache;
mod errors;
mod geometry;
mod map_points;
mod nav;
pub mod places;
mod poi;
mod settlement;
pub use map_points::MapPointQuery;
mod summit;

pub(crate) use cache::MAP_CHUNK_SLOTS;
pub use cache::{CacheStats, MapCache};
use core::sync::atomic::{AtomicU32, Ordering};
pub use errors::{CacheError, CapacityError, DecodeStatus, FeatureDecodeError, FeatureReadError, MapReadError};
pub(crate) use geometry::parse_lod_table;
pub use geometry::{FeatureRef, Interiors, Lod, MAX_CHUNK_BYTES, MAX_FEAT_PTS, MAX_FEAT_RINGS};
use nav::{parse_nav_directory, parse_nav_profiles};
pub use nav::{
    MapProfile, NavCacheStats, NavDirectory, NavEdgeCandidate, NavEdgeEndpoint, NavEdgePosition, NavEdgeSnap,
    NavNeighbor, NavNodeRef, NavTileCache, NAV_MAX_CHUNK_BYTES,
};
use poi::parse_poi_directory;
pub use poi::{MapPoint, Poi, PoiCatEntry, PoiDirectory, MAX_POI_RESULTS, POI_MAX_CATEGORIES, POI_MAX_CHUNK_BYTES};
pub use settlement::Settlement;
pub use summit::{Summit, MAX_SUMMIT_RADIUS_M};

use heapless::Vec;

use crate::Error;
use obc_formats::io::{rd_i32, rd_u16, rd_u32, ByteSource};
use obc_formats::obcm::{
    OffsetScale, ScaledOffset, HEADER_LEN, HEADER_OFFSET_SCALE_OFF, HEADER_TERRAIN_LENGTH_OFF,
    HEADER_TERRAIN_OFFSET_OFF, LOD_ENTRY_LEN, NAV_MAX_PROFILES,
};
use obc_formats::obcm::{
    BRANCH_BIT, EMPTY_LEAF, STYLE_DASHED_BIT, STYLE_FIXED_WIDTH_BIT, STYLE_HAS_COLOR2_BIT, STYLE_PRIORITY_MASK,
    STYLE_TERRAIN_LAYER_BIT, STYLE_TICKED_BIT,
};
use obc_formats::obcm::{MAGIC, STYLE_RECORD_LEN, VERSION};
use obc_map_scene::{BBox, LineStyle, Style, StyleFlags};

/// Hard cap on quadtree recursion depth in [`Reader::walk_leaves`]. A well-formed tree is far
/// shallower, so this never rejects a real map. It matters for a corrupt one: once the node bbox
/// subdivides to a point the quadrants stop shrinking while `intersects(view)` stays true, so an
/// unbounded walk recurses until the stack overflows.
const MAX_QUADTREE_DEPTH: u32 = 32;

/// A flat `uint32` quadtree index over the header's global bbox, the layout a geometry [`Lod`] and
/// a POI category share. The leaf walk needs only where the index starts and how many nodes it
/// holds, so [`Reader::walk_leaves`] serves the geometry walk and the POI query alike.
trait QuadIndex {
    /// Byte offset of node 0 in the file.
    fn index_offset(&self) -> u64;
    /// Number of `uint32` nodes in the index; `0` ⇒ empty (no walk).
    fn node_count(&self) -> usize;
}

/// What follows a `node_count`-node index begins right behind it, at `index_offset + node_count *
/// 4`. `None` on `u64` overflow, which a corrupt `index_offset` or `node_count` can reach. What
/// follows differs per section, which is why the callers name it and this does not.
#[inline]
fn index_end(index_offset: u64, node_count: usize) -> Option<u64> {
    (node_count as u64).checked_mul(4)?.checked_add(index_offset)
}

/// The same convention for a section whose chunks are addressed by a scaled offset: they begin at
/// `align_up(index_offset + node_count * 4, U)`, one rounding step past the index. The index and
/// the offset table are read by 4-byte indexing, so neither needs a boundary of its own.
#[inline]
fn aligned_index_end(scale: OffsetScale, index_offset: u64, node_count: usize) -> Option<u64> {
    scale.align_up(index_end(index_offset, node_count)?)
}

/// Resolve one stored scaled offset field to a byte position in this file.
///
/// The scale rides inside [`ScaledOffset`], so an offset read from one file cannot be resolved
/// against another file's unit. A resolved offset is bounded by the source's own length: each
/// parse refuses a section that reaches past `total`, and the [`ByteSource`] refuses the read.
#[inline]
pub(crate) fn resolve(offset: ScaledOffset) -> u64 {
    offset.bytes()
}

/// Byte range `[start, end)` of chunk `chunk_id` in a section whose chunks lie a fixed
/// `chunk_size` apart from `data_start` (the POI and nav sections; LOD chunk data is packed tight
/// behind an offset table instead). `chunk_id` comes straight from a quadtree leaf, so it is
/// arbitrary in a corrupt map: `None` covers an out-of-range id and any `u64` overflow.
#[inline]
fn fixed_chunk_range(
    data_start: Option<u64>,
    chunk_count: usize,
    chunk_size: usize,
    chunk_id: u32,
) -> Option<(u64, u64)> {
    let id = chunk_id as u64;
    if id >= chunk_count as u64 {
        return None;
    }
    let start = id.checked_mul(chunk_size as u64)?.checked_add(data_start?)?;
    let end = start.checked_add(chunk_size as u64)?;
    Some((start, end))
}

/// The OBCM header fields that describe a map without touching any geometry, decoded before a
/// single byte of style table, index or chunk is read. [`MapTables::parse`] carries on from here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MapHeader {
    pub version: u8,
    pub bbox: BBox,
    /// User-position marker color (RGB565).
    pub marker_color: u16,
    /// The file's offset unit. Every scaled field in the file resolves against this value.
    pub scale: OffsetScale,
    /// The embedded terrain region, or `None` for a map with no elevation.
    pub terrain: Option<TerrainRegion>,
}

/// The terrain region: a byte window at the file tail holding one OBCT container verbatim.
///
/// A reader hands this over; it does not parse it. Every offset inside the container is relative
/// to its own first byte, which is what makes a window sufficient and a copy unnecessary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerrainRegion {
    /// Byte offset of the region's first byte, which is the container's byte `0`.
    pub offset: u64,
    /// The region's length. `Terrain Length` counts units, so this is the container's byte length
    /// rounded up and the tail is filler: a consumer must bound its reads by the container's own
    /// structure and must not derive the payload length from this.
    pub len: u64,
}

/// Decode and validate the fixed OBCM header: magic, version, bbox, marker color, the offset scale
/// and the terrain region.
///
/// The version byte is a hard cut, and it cuts in both directions. The refusal is the file's, not
/// the section's: nothing is partially readable across the cut, because a section offset that
/// means the wrong unit lands somewhere plausible rather than somewhere obviously wrong.
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
    let scale = OffsetScale::new(h[HEADER_OFFSET_SCALE_OFF]).map_err(|_| Error::BadScale)?;
    // `0` means the map carries no elevation, and `Terrain Length` must then be `0` too. A file
    // that sets one without the other is refused rather than half-believed.
    let terrain_offset = scale.offset(rd_u32(h, HEADER_TERRAIN_OFFSET_OFF));
    let terrain_len = scale.offset(rd_u32(h, HEADER_TERRAIN_LENGTH_OFF));
    if terrain_offset.is_zero() != terrain_len.is_zero() {
        return Err(Error::BadOffset);
    }
    let terrain = if terrain_offset.is_zero() {
        None
    } else {
        Some(TerrainRegion { offset: resolve(terrain_offset), len: resolve(terrain_len) })
    };
    Ok(MapHeader { version, bbox: BBox { min_lon, min_lat, max_lon, max_lat }, marker_color, scale, terrain })
}

/// The prefix every OBCM parse begins with, decoded and bounds-checked once: the fixed header plus
/// the LOD table's position. [`MapTables::parse`] goes on to the style table and the POI and nav
/// sections, whose offsets it reads out of the retained `header` bytes.
pub(crate) struct HeaderPrologue {
    /// The raw header bytes, kept so a caller can decode the fields the prologue does not own.
    pub header: [u8; HEADER_LEN],
    pub map: MapHeader,
    pub lod_count: usize,
    pub lod_table_offset: u64,
    /// The file's length, already read from the source — every offset check above used it.
    pub total: u64,
}

/// Read + validate the shared prologue from `src`. A file shorter than the header, with the wrong
/// magic / version, or with a LOD table that is empty, wraps `u64`, or runs past EOF is rejected
/// here, so neither parse has to restate the check.
pub(crate) fn parse_prologue(src: &dyn ByteSource) -> Result<HeaderPrologue, Error> {
    let total = src.len();
    if total < HEADER_LEN as u64 {
        return Err(Error::TooShort);
    }
    let mut header = [0u8; HEADER_LEN];
    src.read_at(0, &mut header).map_err(Error::Source)?;
    let map = parse_header(&header)?;
    let lod_count = header[25] as usize;
    let lod_table_offset = resolve(map.scale.offset(rd_u32(&header, 26)));
    if lod_count == 0 {
        return Err(Error::BadOffset);
    }
    if let Some(region) = map.terrain {
        // The window has to be inside the file it is a window onto. What is in it is the terrain
        // consumer's problem: an unreadable raster is not a broken map.
        let end = region.offset.checked_add(region.len).ok_or(Error::BadOffset)?;
        if end > total {
            return Err(Error::BadOffset);
        }
    }
    crate::landmarks::map_region(&header, total)?;
    // Checked: `lod_table_offset` is an arbitrary header u32 scaled by an arbitrary unit, so the
    // table-end can wrap `u64` and slip past the guard below.
    let lod_table_end = (lod_count as u64)
        .checked_mul(LOD_ENTRY_LEN as u64)
        .and_then(|len| lod_table_offset.checked_add(len))
        .ok_or(Error::BadOffset)?;
    if lod_table_end > total {
        return Err(Error::BadOffset);
    }
    Ok(HeaderPrologue { header, map, lod_count, lod_table_offset, total })
}

/// The session-resident, immutable map tables: header scalars, style table and LOD pyramid. Parsed
/// once per `.obcm`, then borrowed by a cheap per-frame [`Reader::new`]. Keeping the per-frame
/// reader to tens of bytes is what holds the deep route-load render path inside the nRF's stack
/// reserve.
pub struct MapTables {
    pub version: u8,
    pub bbox: BBox,
    /// User-position marker color (RGB565), resolved to a device pixel by the host's color policy
    /// like style colors.
    pub marker_color: u16,
    /// The file's offset unit, retained so a lazily-read offset-table entry resolves against this
    /// file's scale and no other's.
    scale: OffsetScale,
    /// The embedded terrain region, or `None` for a map with no elevation.
    terrain: Option<TerrainRegion>,
    /// LOD layers ordered coarsest (0) → finest (N-1). Always at least one.
    lods: Vec<Lod, 16>,
    /// The parsed POI directory. Always present, with some categories possibly empty, plus the
    /// hours-pool offset and count.
    pois: PoiDirectory,
    /// The parsed nav directory, always present and possibly an empty graph. With the profile
    /// table it is the graph's only resident state; everything else streams.
    nav: NavDirectory,
    /// The parsed routing profiles (1..=8, always present): at most 8 × 56 B resident.
    profiles: heapless::Vec<MapProfile, NAV_MAX_PROFILES>,
    /// Styles indexed by id (0..=255) for O(1) lookup during rendering.
    styles: [Option<Style>; 256],
    /// The backdrop style, resolved once at parse so the per-frame lookup is a field read rather
    /// than a 256-slot scan.
    backdrop: Option<Style>,
    /// Session-unique parse identity, never 0 (a zeroed [`MapCache`] sits at generation 0, which
    /// means unowned). [`Reader::new`] hands it to the cache, which self-clears when it last
    /// served a different parse, so a map switch cannot cross-serve stale chunks.
    generation: u32,
}

impl MapTables {
    /// Parse the header scalars, style table and LOD pyramid from `src`. This is the one
    /// expensive, allocating step (a 2048-byte style scratch plus the table reads), so do it once
    /// per map and hand the result to [`Reader::new`] each frame. A map that is shorter than the
    /// header, or carries the wrong magic or version, or has out-of-range table offsets, is
    /// rejected.
    pub fn parse(src: &dyn ByteSource) -> Result<MapTables, Error> {
        let HeaderPrologue { header, map, lod_count, lod_table_offset, total } = parse_prologue(src)?;
        let MapHeader { version, bbox, marker_color, scale, terrain } = map;
        let style_offset = resolve(scale.offset(rd_u32(&header, 21)));
        let poi_section_offset = resolve(scale.offset(rd_u32(&header, 32)));
        let nav_section_offset = resolve(scale.offset(rd_u32(&header, 36)));

        // The style table cannot start inside the header, and it does not start at its end
        // either: the header is no whole number of units at any scale above `0`, so the table
        // begins at the first unit boundary at or after it and the gap is filler.
        let style_floor = scale.align_up(HEADER_LEN as u64).ok_or(Error::BadOffset)?;
        if style_offset < style_floor || style_offset > total {
            return Err(Error::BadOffset);
        }

        let mut styles = [None; 256];
        parse_styles(src, style_offset, total, &mut styles)?;
        let lods = parse_lod_table(src, scale, lod_table_offset, lod_count, total)?;
        let pois = parse_poi_directory(src, scale, poi_section_offset, total)?;
        let nav = parse_nav_directory(src, scale, nav_section_offset, total)?;
        let profiles = parse_nav_profiles(src, &nav)?;
        // Resolve the backdrop (lowest `z_index`, ties by lowest id) once here; the table is
        // immutable after parse, so the per-frame lookup never re-scans the 256 slots.
        let backdrop = styles.iter().filter_map(|s| s.as_ref()).min_by_key(|s| (s.z_index, s.id)).copied();
        // Stamp a session-unique generation. `fetch_add + 1` starts the first parse at 1, so 0 is
        // never live and a zero-initialized cache always reads as unowned. `Relaxed` suffices:
        // the counter is the only shared state and only uniqueness matters.
        static GEN: AtomicU32 = AtomicU32::new(0);
        let generation = GEN.fetch_add(1, Ordering::Relaxed) + 1;
        Ok(MapTables {
            version,
            bbox,
            marker_color,
            scale,
            terrain,
            lods,
            pois,
            nav,
            profiles,
            styles,
            backdrop,
            generation,
        })
    }

    /// This file's offset unit: the value every scaled field in it resolves against.
    #[inline]
    pub fn scale(&self) -> OffsetScale {
        self.scale
    }

    /// The embedded terrain region, or `None` for a map with no elevation.
    ///
    /// The reader forms a window and hands it over; it never parses the container. A consumer
    /// whose OBCT parse fails must fall back to no elevation and must still mount, render and
    /// route: a rider whose raster is unreadable has the map they would have had without one.
    #[inline]
    pub fn terrain(&self) -> Option<TerrainRegion> {
        self.terrain
    }

    /// Whether LOD `lod` is written empty in this file's LOD table (`Index Node Count == 0`). A
    /// mount-time predicate for I/O avoidance over one file's own table, and nothing more.
    pub fn lod_is_empty(&self, lod: usize) -> bool {
        self.lods.get(lod).is_none_or(|entry| entry.node_count() == 0)
    }

    #[inline]
    pub fn lods(&self) -> &[Lod] {
        &self.lods
    }

    #[inline]
    pub fn styles(&self) -> &[Option<Style>; 256] {
        &self.styles
    }

    /// The map's routing profiles (1..=8, always present). Lets a host mirror the profile names
    /// into the app UI straight off the parsed tables, without building a per-frame [`Reader`].
    pub fn nav_profiles(&self) -> &[MapProfile] {
        &self.profiles
    }

    #[inline]
    pub fn backdrop_style(&self) -> Option<&Style> {
        self.backdrop.as_ref()
    }

    /// Whether the map carries a non-empty nav graph. A graph-less map dims the Detour station
    /// instead of failing a plan.
    pub fn has_nav_graph(&self) -> bool {
        !self.nav.is_empty()
    }
}

pub struct Reader<'a> {
    /// The byte source the index and geometry chunks stream from. `&dyn`, not a generic, so
    /// signatures holding a `&Reader` need no `<S>` parameter.
    src: &'a dyn ByteSource,
    /// Header scalars, copied from [`MapTables`] so field access stays direct while the big
    /// tables stay borrowed.
    pub version: u8,
    pub bbox: BBox,
    pub marker_color: u16,
    /// The session-resident immutable tables, parsed once and borrowed here, so a per-frame
    /// `Reader` carries no styles or lods of its own.
    tables: &'a MapTables,
    /// Borrowed lazy-read cache for the streamed index and geometry, reusable across frames. It
    /// keeps its own `RefCell` because `read_at` takes `&self` but the cache mutates; the borrows
    /// are tightly scoped, so the index-node read and the chunk decode never overlap.
    cache: &'a MapCache,
    /// False only when construction legally re-entered an already borrowed cache. Streamed calls
    /// then return `CacheError::Busy`; reconstructing the cheap reader is the retry.
    cache_ready: bool,
}

impl<'a> Reader<'a> {
    /// Identity of the immutable map tables used by this view.
    pub fn generation(&self) -> u32 {
        self.tables.generation
    }

    /// The immutable source paired with these tables.
    pub fn source(&self) -> &dyn ByteSource {
        self.src
    }

    /// Build a per-frame reader over the pre-parsed [`MapTables`], a `src`, and a `cache` the
    /// geometry and index reads stream through. Cheap: it borrows the tables and copies only the
    /// header scalars, with no parse and no SD read. The cache is caller-owned and reusable across
    /// frames; pass a fresh [`MapCache::new`] if you keep none. The cache adopts these tables'
    /// parse generation here, so building a reader over a different map's tables auto-clears the
    /// stale slots and no manual [`MapCache::clear`] is needed.
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
        }
    }

    /// Snapshot of the chunk-cache + streaming counters. Cumulative over the cache's life, so the
    /// renderer reports the per-frame delta.
    #[inline]
    pub fn chunk_cache_stats(&self) -> CacheStats {
        self.try_chunk_cache_stats().unwrap_or_default()
    }

    /// Fallible cache-counter snapshot for callers that must tell legal contention from an
    /// actually empty cache.
    #[inline]
    pub(crate) fn try_chunk_cache_stats(&self) -> Result<CacheStats, CacheError> {
        if !self.cache_ready {
            return Err(CacheError::Busy);
        }
        self.cache.stats()
    }

    #[inline]
    pub fn style(&self, id: u8) -> Option<&Style> {
        self.tables.styles.get(id as usize).and_then(|s| s.as_ref())
    }

    /// The backdrop style: the one at the bottom of the paint order (lowest `z_index`, ties broken
    /// by lowest id). Its color fills the screen before any geometry is drawn. `None` only for an
    /// empty style table.
    pub fn backdrop_style(&self) -> Option<&Style> {
        self.tables.backdrop.as_ref()
    }

    /// Read node `idx` of a [`QuadIndex`], streamed through the index block cache. `None` on a
    /// read failure, and the walk then skips that subtree. `idx < node_count` and the index region
    /// lies within the file, both guaranteed by the callers, so the offset never overflows `u64`.
    #[inline]
    fn read_node(&self, index: &dyn QuadIndex, idx: usize) -> Result<u32, MapReadError> {
        if !self.cache_ready {
            return Err(MapReadError::Cache(CacheError::Busy));
        }
        let off = index.index_offset() + idx as u64 * 4;
        let mut b = [0u8; 4];
        self.cache
            .try_borrow_mut()
            .map_err(MapReadError::Cache)?
            .index_read(self.src, off, &mut b)
            .map_err(MapReadError::Source)?;
        Ok(u32::from_le_bytes(b))
    }

    /// Visit `(chunk_id, node_bbox)` for every non-empty leaf of a [`QuadIndex`] overlapping
    /// `view`, walking the flat `u32` tree over the header's global bbox. The geometry walk and
    /// the POI query share the node encoding, so one implementation serves both. `index` is `&dyn`
    /// so the recursive walk is not monomorphized twice.
    fn walk_leaves<F: FnMut(u32, BBox)>(
        &self,
        index: &dyn QuadIndex,
        idx: usize,
        node: BBox,
        view: &BBox,
        depth: u32,
        visit: &mut F,
    ) -> Result<(), MapReadError> {
        // The depth cap is the hard stack bound against a corrupt cyclic branch; a well-formed
        // tree never reaches it.
        if idx >= index.node_count() || depth > MAX_QUADTREE_DEPTH || !node.intersects(view) {
            return Ok(());
        }
        // Read the node before descending or visiting, so the index-cache borrow is released
        // before a leaf's `visit` triggers a geometry-chunk read.
        let val = self.read_node(index, idx)?;
        if val & BRANCH_BIT == 0 {
            if val != EMPTY_LEAF {
                visit(val, node);
            }
            return Ok(());
        }
        let child = (val & !BRANCH_BIT) as usize;
        // The packer flattens the quadtree breadth-first, so a branch's children always lie after
        // it. A back- or self-reference (`child <= idx`) appears only in a corrupt map and would
        // re-enter a node already on the stack; the depth cap above is the backstop.
        if child <= idx {
            return Err(MapReadError::Malformed);
        }
        // Floor-division midpoints must match the packer's split, so reader and writer agree on
        // every node bbox.
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
}

/// Parse the style table from `src` at `style_offset` into the caller's `styles` (cleared first).
/// The table is small, so it is pulled in two reads: the count byte, then the records. A truncated
/// table is tolerated — the walk stops at the last whole record — but a failed read or a
/// `style_offset` at or past EOF is [`Error::BadOffset`], because an all-`None` table would let
/// the map load and render nothing with no error to surface.
///
/// The out-param and `inline(never)` are deliberate: with the array in the return value, the
/// several KB of scratch inline into the device `main`'s permanent frame and the ride path
/// overflows the stack. The scratch must stay in a frame that pops before `run_app` starts.
#[inline(never)]
fn parse_styles(
    src: &dyn ByteSource,
    style_offset: u64,
    total: u64,
    styles: &mut [Option<Style>; 256],
) -> Result<(), Error> {
    styles.fill(None);
    // `MapTables::parse`'s header guard admits `style_offset == total`; there is no count byte to
    // read there, so treat it as the corrupt header it is rather than a silently-empty table.
    if style_offset >= total {
        return Err(Error::BadOffset);
    }
    let mut cb = [0u8; 1];
    src.read_at(style_offset, &mut cb).map_err(Error::Source)?;
    let count = cb[0] as usize;
    // `count*8` record bytes follow the count, clamped to what the file holds so the break below
    // stops at the last whole record in a truncated table. `count` is a single byte, so `count * 8`
    // fits `usize` on every target and the `min` can only make it smaller.
    let avail = total - (style_offset + 1);
    let want = ((count * STYLE_RECORD_LEN) as u64).min(avail) as usize;
    let mut buf = [0u8; 256 * STYLE_RECORD_LEN];
    if want > 0 {
        src.read_at(style_offset + 1, &mut buf[..want]).map_err(Error::Source)?;
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
        // Bit 4 takes the style off the width ramp, bit 5 files it under the terrain layer, and
        // bit 6 makes the line ticked. Bit 7 stays reserved and is ignored, not rejected. Ticked
        // wins over dashed, so a record setting both still draws one way.
        let line = if flags & STYLE_TICKED_BIT != 0 {
            LineStyle::Ticked
        } else if flags & STYLE_DASHED_BIT != 0 {
            LineStyle::Dashed
        } else {
            LineStyle::Solid
        };
        let style_flags =
            StyleFlags::new(line, flags & STYLE_FIXED_WIDTH_BIT != 0, flags & STYLE_TERRAIN_LAYER_BIT != 0);
        styles[id as usize] = Some(Style { id, z_index, color, weight, priority, flags: style_flags, color2 });
        o += STYLE_RECORD_LEN;
    }
    Ok(())
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
    use super::cache::{ChunkLoc, WalkEntry, MAP_CHUNK_SLOTS};
    use super::*;
    use crate::SliceSource;
    use obc_formats::cache::INDEX_BLOCK;
    use obc_formats::io::Error as IoError;

    /// A `ByteSource` reproducing a flaky-SD partial overwrite: the read at offset `fail_at`
    /// copies `partial` bytes into the destination and then returns `Err`. Every other read is
    /// filled from `data`. `SliceSource` copies in one shot and cannot reproduce this.
    struct FlakySource<'a> {
        data: &'a [u8],
        fail_at: u64,
        partial: usize,
    }

    impl ByteSource for FlakySource<'_> {
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), IoError> {
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

        fn len(&self) -> u64 {
            self.data.len() as u64
        }
    }

    /// The map renderer replays an ordered quadtree walk every frame. When that cycle is larger
    /// than the index cache, LRU has zero hits forever, because the next scan evicts every block
    /// just before reuse. BRRIP must keep a protected subset while one probation slot takes the scan.
    #[test]
    fn index_cache_resists_a_repeated_scan_larger_than_capacity() {
        const WORKING_BLOCKS: usize = 18;
        let data = [0u8; WORKING_BLOCKS * INDEX_BLOCK];
        let src = SliceSource(&data);
        let cache = MapCache::new();
        let mut inner = cache.inner.borrow_mut();
        let mut word = [0u8; 4];

        for block in 0..WORKING_BLOCKS {
            inner.index_read(&src, (block * INDEX_BLOCK) as u64, &mut word).unwrap();
        }
        assert_eq!(inner.stats().sd_reads, WORKING_BLOCKS as u32, "the cold scan fills from the source");

        for block in 0..WORKING_BLOCKS {
            inner.index_read(&src, (block * INDEX_BLOCK) as u64, &mut word).unwrap();
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
        cache.inner.borrow_mut().store_walk(3, cover, &entries);

        let inside = BBox { min_lon: -20, min_lat: -10, max_lon: 20, max_lat: 10 };
        let hit = cache.inner.borrow().cached_walk(3, &inside).unwrap();
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].cid, 7);
        assert_eq!(hit[0].node, entry.node);

        let outside = BBox { min_lon: -20, min_lat: -10, max_lon: 101, max_lat: 10 };
        assert!(cache.inner.borrow().cached_walk(3, &outside).is_none());
        assert!(cache.inner.borrow().cached_walk(4, &inside).is_none());
        cache.clear().unwrap();
        assert!(cache.inner.borrow().cached_walk(3, &inside).is_none());
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
            inner.load_chunk(&src, 0, cid, u64::from(cid) * LEN as u64, LEN, &node).unwrap();
        }
        assert_eq!(inner.stats().chunk_misses, CHUNKS as u32);

        for cid in 0..CHUNKS as u32 {
            inner.load_chunk(&src, 0, cid, u64::from(cid) * LEN as u64, LEN, &node).unwrap();
        }
        let stats = inner.stats();
        assert_eq!(stats.chunk_misses, CHUNKS as u32, "all five chunks should remain resident");
        assert_eq!(stats.chunk_hits, CHUNKS as u32, "the second five-chunk scan must be entirely RAM-only");
    }

    /// A read that fails partway must leave the evicted slot *empty*, not poisoned with the old key
    /// over a half-overwritten buffer — otherwise a later request for the old key is served as a
    /// corrupt hit.
    #[test]
    fn partial_read_failure_does_not_poison_evicted_slot() {
        const LEN: usize = 64;
        const CACHE_SLOTS: usize = MAP_CHUNK_SLOTS + 1; // four dedicated buffers + decode scratch
                                                        // One LEN-chunk per slot, plus one more past them for the failing eviction read.
        let mut data = [0u8; (CACHE_SLOTS + 1) * LEN];
        for (k, b) in data.iter_mut().enumerate() {
            *b = (k as u8).wrapping_mul(31).wrapping_add(7); // distinct, offset-derived bytes
        }
        // The eviction read (K_new) lives past the primed chunks (one per slot) and fails partway.
        let fail_at = (CACHE_SLOTS * LEN) as u64;
        let src = FlakySource { data: &data, fail_at, partial: 8 };

        let cache = MapCache::new();
        let mut inner = cache.inner.borrow_mut();
        let node = BBox { min_lon: 0, min_lat: 0, max_lon: 1, max_lat: 1 };

        // Prime all five slots. RRIP's first victim of the next miss is slot 0 (cid 0).
        for cid in 0..CACHE_SLOTS as u32 {
            let loc = inner.load_chunk(&src, 0, cid, u64::from(cid) * LEN as u64, LEN, &node).unwrap();
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
        assert!(matches!(inner.load_chunk(&src, 0, 99, fail_at, LEN, &node), Err(IoError::Io)));

        // Request K_old again: it must be a *miss* (re-read), not a hit on the poisoned slot.
        let before = inner.stats();
        let loc = inner.load_chunk(&src, 0, 0, 0, LEN, &node).unwrap();
        let after = inner.stats();
        assert_eq!(after.chunk_hits, before.chunk_hits, "K_old must not hit the poisoned slot");
        assert_eq!(after.chunk_misses, before.chunk_misses + 1, "K_old must be re-read");

        // …and the re-read returns the real K_old bytes, not the half-written K_new.
        match loc {
            ChunkLoc::Slot(i) => assert_eq!(&inner.chunks[i].buf[..LEN], &k_old[..]),
            ChunkLoc::Scratch => panic!("a slot-sized chunk should land in a slot"),
        }
    }

    /// [`MapCache::new_boxed`] leans on a `core` implementation detail: a zeroed `RefCell` borrow
    /// flag means unborrowed. The first `borrow_mut` here is the tripwire, plus a check that the
    /// zeroed allocation behaves like a fresh [`MapCache::new`].
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
        let loc = inner.load_chunk(&src, 0, 0, 0, LEN, &node).unwrap();
        match loc {
            ChunkLoc::Slot(i) => assert_eq!(&inner.chunks[i].buf[..LEN], &data[..]),
            ChunkLoc::Scratch => panic!("a slot-sized chunk should land in a slot"),
        }
        assert_eq!(inner.stats().chunk_misses, 1, "an empty cache's first load is a miss");
    }

    /// Two maps sharing a chunk key `(lod, cid, len)` but holding different bytes must not
    /// cross-serve through a shared cache. Slots are keyed only by that triple, not by the source,
    /// so the guarantee lives in the generation adopt inside `Reader::new`: this drives a map
    /// switch through the public path without ever calling `clear()`.
    #[test]
    fn map_switch_without_clear_cannot_cross_serve() {
        use obcm_testkit::{build_file, pack_line, pad, LodSpec};

        // Two byte-identical layouts (same chunk key and offsets) whose one feature differs only
        // in its delta, so the decoded endpoint tells the maps apart.
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

        // Map B over the same cache and the same chunk key, with no `clear()`: `Reader::new`
        // adopts B's generation, so the load must miss and serve B's bytes.
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

    /// A style-table read that fails must surface as a parse error, not an all-`None` table that
    /// loads and renders nothing. Both reads are exercised, the count byte and the record block. A
    /// physically truncated table is still tolerated.
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
        // `Style Offset` is scaled, so arm the failing read at the resolved byte, not at the
        // field's own value.
        let style_off = obcm_testkit::resolve_offset(&bytes, 21) as u64;
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

    /// The index-block cache keys a block by its absolute offset, which means nothing across maps,
    /// so `clear()` must drop index blocks too. Detected through the source-read counters: a
    /// post-clear read of the same offset must re-read, because a hit reads nothing.
    #[test]
    fn clear_invalidates_index_blocks() {
        let data = [0x5Au8; 1024];
        let src = SliceSource(&data);
        let mut word = [0u8; 4];

        let cache = MapCache::new();
        let mut inner = cache.inner.borrow_mut();

        // Resident, then a hit (no source read).
        inner.index_read(&src, 0, &mut word).unwrap();
        let before = inner.stats();
        inner.index_read(&src, 0, &mut word).unwrap();
        assert_eq!(inner.stats().sd_reads, before.sd_reads, "a resident block must hit, not re-read");
        drop(inner);

        // After clear the same offset must miss and re-read from the source.
        cache.clear().unwrap();
        let mut inner = cache.inner.borrow_mut();
        let before = inner.stats();
        inner.index_read(&src, 0, &mut word).unwrap();
        assert_eq!(inner.stats().sd_reads, before.sd_reads + 1, "post-clear index read must re-read");
    }
}
