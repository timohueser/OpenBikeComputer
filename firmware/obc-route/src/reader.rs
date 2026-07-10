//! OBCR route reader: header, chunk index, and on-demand chunk decode.
//!
//! [`RouteReader`] loads the fixed header and the (small) chunk index into RAM, then
//! pulls individual geometry chunks through the [`ByteSource`] only when asked — a
//! hundreds-of-km route never has to be resident. It holds a `&dyn ByteSource` (not a
//! generic), so it threads through the app/render layers without making them generic.

use core::cell::RefCell;

use heapless::{String, Vec};

use crate::byte_io::{ByteSource, Error};
use obc_reader::codec::{rd_i16, rd_i32, rd_u16, rd_u32};
use obc_reader::BBox;

/// Base header length, common to v1 and v2 (`OBCR_Spec.md` §1). Every field the ride path
/// needs is in these bytes, so the reader parses only them regardless of version; the v2
/// extension is read on demand by [`for_each_waypoint`].
pub const HEADER_LEN: usize = 112;
/// Full v2 header length: the base header plus the 16-byte waypoint extension (§1.1).
pub const HEADER_V2_LEN: usize = 128;
/// Per-chunk index entry length (§2).
pub const CHUNK_META_LEN: usize = 44;
/// Capacity of the inline route-name field, bytes.
pub const NAME_CAP: usize = 48;
/// Fixed waypoint record length (§4).
pub const WAYPOINT_LEN: usize = 40;
/// Capacity of a waypoint record's inline name, bytes.
pub const WAYPOINT_NAME_CAP: usize = 24;
/// Waypoint-elevation sentinel: "no elevation known" (§4).
pub const WAYPOINT_ELE_NONE: i16 = i16::MIN;
/// The device's waypoint cap — one number for both roles: the converter's `<wpt>` emission cap
/// ([`gpx_to_obcr`](crate::gpx_to_obcr)) and the resident [`Waypoints`] table the ride loop holds
/// (~40 B/entry ≈ 1.3 KB — negligible on the 512 KB target). The *format* allows up to `u16::MAX`
/// waypoints (a phone-side encoder isn't bound by this), so [`RouteReader::load_waypoints`] windows
/// + truncates a longer file rather than overflowing.
pub const MAX_WAYPOINTS: usize = 32;
/// Resident chunk-index capacity. A route past the cap fails conversion with
/// [`Error::TooLarge`] rather than being silently coarsened (full profile: ~131 k points,
/// ~24 KB index; `nrf-mem`: 128 chunks, ~33 k points, ~6 KB). The `nrf-mem` trim is the
/// tightest RAM knob because a `RouteIndex` is held resident across frames *and*
/// [`read`](RouteIndex::read) builds the index/`cum_seg` `Vec`s on the stack before returning
/// by value — a 24 KB index would overflow the 256 KB part's stack during that build. The
/// host packer keeps 512, so a route packed past 128 chunks won't load on `nrf-mem` firmware.
#[cfg(not(feature = "nrf-mem"))]
pub const MAX_ROUTE_CHUNKS: usize = 512;
#[cfg(feature = "nrf-mem")]
pub const MAX_ROUTE_CHUNKS: usize = 128;
/// Max points a single chunk may hold (bounds the per-chunk decode buffer).
pub const MAX_POINTS_PER_CHUNK: usize = 256;

const MAGIC: &[u8; 4] = b"OBCR";
/// Accepted format versions: v1 (no waypoints) and v2 (optional waypoints section). The v2
/// additions live entirely *outside* the byte ranges this reader touches — a 16-byte header
/// extension at offset 112 and a record table reached only via it — so v2 routes ride through
/// the exact v1 code path; waypoints are skipped by construction, not by branching.
const VERSIONS: core::ops::RangeInclusive<u8> = 1..=2;

/// One decoded route point: position in microdegrees + elevation in meters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutePoint {
    pub lon: i32,
    pub lat: i32,
    pub ele: i16,
}

/// One chunk's index entry — its bbox (for viewport query), the absolute anchor it
/// decodes from, and the cumulative stats at its first point (for remaining-distance/
/// climb). See `OBCR_Spec.md` §2.
#[derive(Debug, Clone, Copy)]
pub struct ChunkMeta {
    pub bbox: BBox,
    pub anchor_lon: i32,
    pub anchor_lat: i32,
    pub anchor_ele: i16,
    pub point_count: u16,
    pub cum_distance_m: u32,
    pub cum_ascent_m: u32,
    pub byte_offset: u32,
    pub byte_len: u32,
}

/// The lightweight route description for the Route menu — readable from the header alone
/// (no chunk index), so a catalog scan is one small read per file.
#[derive(Debug, Clone)]
pub struct RouteSummary {
    pub name: String<NAME_CAP>,
    /// Total distance, km (rounded) — the v1 stat display unit.
    pub distance_km: u32,
    /// Total ascent, m.
    pub climb_m: u32,
    pub bbox: BBox,
    /// First route point, for centering the camera on load.
    pub start_lon: i32,
    pub start_lat: i32,
}

impl RouteSummary {
    /// Read just the header into a summary — cheap enough to call per file when building
    /// the Route-menu catalog.
    pub fn read(src: &dyn ByteSource) -> Result<RouteSummary, Error> {
        let h = read_header(src)?;
        Ok(RouteSummary {
            name: h.name,
            distance_km: (h.total_distance_m + 500) / 1000,
            climb_m: h.total_ascent_m,
            bbox: h.bbox,
            start_lon: h.start_lon,
            start_lat: h.start_lat,
        })
    }
}

/// The stored-route facts a BLE `routeList` entry serves: raw metres (not
/// [`RouteSummary`]'s display-rounded km) plus the waypoint count from the v2 header
/// extension. Reads the base header and, on v2, the 16-byte extension; never the chunk index.
#[derive(Debug, Clone)]
pub struct RouteObjectInfo {
    pub name: String<NAME_CAP>,
    pub distance_m: u32,
    pub ascent_m: u32,
    pub point_count: u32,
    pub waypoint_count: u16,
}

impl RouteObjectInfo {
    /// Read the header (+ v2 extension) into the wire facts. Same validation as any header
    /// read (bad magic/version/name reject), which the upload commit path relies on to keep
    /// a non-OBCR payload out of the catalog.
    pub fn read(src: &dyn ByteSource) -> Result<RouteObjectInfo, Error> {
        let h = read_header(src)?;
        let waypoint_count = if h.version >= 2 {
            let mut ext = [0u8; HEADER_V2_LEN - HEADER_LEN];
            src.read_at(HEADER_LEN as u32, &mut ext).map_err(|_| Error::BadOffset)?;
            rd_u16(&ext, 4)
        } else {
            0
        };
        Ok(RouteObjectInfo {
            name: h.name,
            distance_m: h.total_distance_m,
            ascent_m: h.total_ascent_m,
            point_count: h.point_count,
            waypoint_count,
        })
    }
}

/// The resident, source-independent parse of a route: the header summary fields plus the
/// chunk index and its segment prefix sums. [`read`](Self::read) does the route's only
/// up-front cost — the header read **and the full chunk-meta walk** — so afterwards a
/// [`RouteReader`] streams geometry chunk-by-chunk without re-reading the index.
///
/// Build it **once** when the active route changes and reuse it across frames, so a redraw
/// pays only the geometry reads, not an N+1 re-walk of the index off the SD card.
pub struct RouteIndex {
    pub bbox: BBox,
    pub start_lon: i32,
    pub start_lat: i32,
    pub point_count: u32,
    pub total_distance_m: u32,
    pub total_ascent_m: u32,
    pub total_descent_m: u32,
    pub min_ele_m: i16,
    pub max_ele_m: i16,
    name: String<NAME_CAP>,
    index: Vec<ChunkMeta, MAX_ROUTE_CHUNKS>,
    /// Prefix sum of segments per chunk: `cum_seg[c]` = segments before chunk `c`
    /// (∑ `point_count − 1`, the shared seam point not double-counted). A trailing entry
    /// holds the total segment count, so an index at `chunk_count` is valid. Built once at
    /// [`read`](Self::read) so [`global_seg_index`](Self::global_seg_index) — on the matcher's
    /// per-fix hot path — is O(1), not a prefix scan.
    cum_seg: Vec<u32, { MAX_ROUTE_CHUNKS + 1 }>,
}

/// A parsed route, ready to query and decode: a [`RouteIndex`] (resident, reusable across
/// frames) paired with a shared borrow of the byte source its geometry chunks stream from.
/// Cheap to build via [`new`](Self::new) — the expensive parse lives in [`RouteIndex::read`].
///
/// Derefs to its [`RouteIndex`], so the summary fields and resident-only queries read through
/// `route.field` / `route.method()`; only [`decode_chunk`](Self::decode_chunk) needs the source.
pub struct RouteReader<'a> {
    src: &'a dyn ByteSource,
    idx: &'a RouteIndex,
    /// Optional resident decoded-chunk cache: when present,
    /// [`decode_chunk`](Self::decode_chunk) serves an unchanged route from RAM instead of
    /// re-reading its geometry every redraw / matcher fix. `None` streams every call (the host
    /// store is fast, so the sim/tests skip it).
    cache: Option<&'a RouteCache>,
}

impl RouteIndex {
    /// Parse the header and chunk index from `src`. Validates magic/version and that
    /// every chunk lies within the source and within the resident buffers.
    pub fn read(src: &dyn ByteSource) -> Result<RouteIndex, Error> {
        let h = read_header(src)?;
        if h.chunk_count as usize > MAX_ROUTE_CHUNKS {
            return Err(Error::TooLarge);
        }

        let mut index = Vec::new();
        let mut cum_seg = Vec::new();
        let mut seg_acc: u32 = 0;
        let mut meta = [0u8; CHUNK_META_LEN];
        for k in 0..h.chunk_count {
            let off = h.index_offset + k * CHUNK_META_LEN as u32;
            src.read_at(off, &mut meta)?;
            let point_count = rd_u16(&meta, 26);
            if point_count as usize > MAX_POINTS_PER_CHUNK {
                return Err(Error::TooLarge);
            }
            let cm = ChunkMeta {
                bbox: BBox {
                    min_lon: rd_i32(&meta, 0),
                    min_lat: rd_i32(&meta, 4),
                    max_lon: rd_i32(&meta, 8),
                    max_lat: rd_i32(&meta, 12),
                },
                anchor_lon: rd_i32(&meta, 16),
                anchor_lat: rd_i32(&meta, 20),
                anchor_ele: rd_i16(&meta, 24),
                point_count,
                cum_distance_m: rd_u32(&meta, 28),
                cum_ascent_m: rd_u32(&meta, 32),
                byte_offset: rd_u32(&meta, 36),
                byte_len: rd_u32(&meta, 40),
            };
            // Bounds-check the chunk's data region up front (no per-decode checks).
            let end = cm.byte_offset.checked_add(cm.byte_len).ok_or(Error::BadOffset)?;
            if end > src.len() {
                return Err(Error::BadOffset);
            }
            // Running segment prefix sum, built alongside the index so the matcher never
            // re-walks the chunk list per fix.
            cum_seg.push(seg_acc).map_err(|_| Error::TooLarge)?;
            seg_acc += (point_count as u32).saturating_sub(1);
            index.push(cm).map_err(|_| Error::TooLarge)?;
        }
        // Trailing total, so `cum_seg[chunk_count]` is the route's full segment count.
        cum_seg.push(seg_acc).map_err(|_| Error::TooLarge)?;

        Ok(RouteIndex {
            bbox: h.bbox,
            start_lon: h.start_lon,
            start_lat: h.start_lat,
            point_count: h.point_count,
            total_distance_m: h.total_distance_m,
            total_ascent_m: h.total_ascent_m,
            total_descent_m: h.total_descent_m,
            min_ele_m: h.min_ele_m,
            max_ele_m: h.max_ele_m,
            name: h.name,
            index,
            cum_seg,
        })
    }

    /// The route name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The chunk index (in route order).
    pub fn chunks(&self) -> &[ChunkMeta] {
        &self.index
    }

    /// Global index (from the route start) of segment `seg` in chunk `c`. O(1) via the
    /// `cum_seg` prefix sum. `c` past the last chunk clamps to the total.
    pub(crate) fn global_seg_index(&self, c: usize, seg: usize) -> usize {
        let c = c.min(self.index.len());
        self.cum_seg.get(c).copied().unwrap_or(0) as usize + seg
    }

    // Cumulative ascent at a position is read from the elevation `Profile`
    // (`Profile::ascent_to`) at column resolution, not from the coarse per-chunk
    // `cum_ascent_m` (too few chunks to place "to climb" accurately).

    /// A [`RouteSummary`] for this route (for the menu / centering).
    pub fn summary(&self) -> RouteSummary {
        RouteSummary {
            name: self.name.clone(),
            distance_km: (self.total_distance_m + 500) / 1000,
            climb_m: self.total_ascent_m,
            bbox: self.bbox,
            start_lon: self.start_lon,
            start_lat: self.start_lat,
        }
    }

    /// Visit each chunk whose bbox intersects `view`, in route order, passing its
    /// index `k` and `ChunkMeta`. The caller decodes the ones it wants with
    /// [`RouteReader::decode_chunk`] into its own reused buffer — keeping the
    /// streaming draw allocation-free.
    pub fn for_each_visible_chunk<F: FnMut(usize, &ChunkMeta)>(&self, view: &BBox, mut f: F) {
        for (k, cm) in self.index.iter().enumerate() {
            if cm.bbox.intersects(view) {
                f(k, cm);
            }
        }
    }
}

impl<'a> RouteReader<'a> {
    /// Pair an already-parsed [`RouteIndex`] with the byte source its geometry chunks stream
    /// from. No I/O; [`decode_chunk`](Self::decode_chunk) pulls chunks on demand. Build the
    /// index once per route and call this per frame.
    pub fn new(idx: &'a RouteIndex, src: &'a dyn ByteSource) -> RouteReader<'a> {
        RouteReader { src, idx, cache: None }
    }

    /// Like [`new`](Self::new), but back [`decode_chunk`](Self::decode_chunk) with a resident
    /// [`RouteCache`], so a redraw of an unchanged route — and the matcher's per-fix decode —
    /// hit RAM instead of re-reading geometry from the SD card. Slots are keyed by chunk index
    /// only, so the caller must [`RouteCache::clear`] it whenever the active route changes.
    pub fn new_cached(idx: &'a RouteIndex, src: &'a dyn ByteSource, cache: &'a RouteCache) -> RouteReader<'a> {
        RouteReader { src, idx, cache: Some(cache) }
    }

    /// Decode chunk `k` into `out` (cleared first): its anchor followed by each
    /// delta-stepped point. The chunk's last point equals chunk `k+1`'s anchor (seam
    /// sharing), so adjacent chunks stitch without a gap.
    ///
    /// With a [`RouteCache`] attached ([`new_cached`](Self::new_cached)) a chunk decoded earlier
    /// is served from RAM; otherwise its geometry is read from the source every call.
    pub fn decode_chunk(&self, k: usize, out: &mut Vec<RoutePoint, MAX_POINTS_PER_CHUNK>) -> Result<(), Error> {
        out.clear();
        let m = self.idx.index.get(k).ok_or(Error::BadOffset)?;
        let n = m.point_count as usize;
        if n == 0 {
            return Ok(());
        }
        // A hit fills `out` with no SD read; a miss decodes and stores it.
        if let Some(cache) = self.cache {
            if cache.get(k, out) {
                return Ok(());
            }
            decode_chunk_from(self.src, m, n, out)?;
            cache.put(k, out);
            return Ok(());
        }
        decode_chunk_from(self.src, m, n, out)
    }

    /// The route's polyline decimated to at most `N` points — uniform by point index, the first
    /// and last point always kept — the computed-route overview's shape-preview seam (#685 §4:
    /// the host hands the app this bounded copy; ≤ 64 points is plenty for a ~212×90 px sketch).
    ///
    /// Streams every chunk once in route order (a chunk seam's shared point is skipped, so the
    /// walk is over **distinct** points, matching the segment prefix sums) — call it once per
    /// plan, never per frame. A chunk that fails to decode is skipped: the preview just loses
    /// its points (a sketch, not navigation data).
    pub fn preview_polyline<const N: usize>(&self) -> Vec<(i32, i32), N> {
        let mut out: Vec<(i32, i32), N> = Vec::new();
        // Distinct points = total segments + 1 (`cum_seg`'s trailing entry); an index with no
        // chunks has nothing to walk.
        if self.idx.index.is_empty() || N == 0 {
            return out;
        }
        let total = *self.idx.cum_seg.last().unwrap_or(&0) as usize + 1;
        let keep = N.min(total);
        let mut kept = 0usize; // points pushed so far
        let mut next = 0usize; // distinct-point index of the next kept point
        let mut gi = 0usize; // running distinct-point index
        let mut buf: Vec<RoutePoint, MAX_POINTS_PER_CHUNK> = Vec::new();
        for k in 0..self.idx.index.len() {
            if self.decode_chunk(k, &mut buf).is_err() {
                continue;
            }
            // Chunk k>0 re-decodes chunk k−1's last point as its anchor — skip the duplicate.
            let skip = usize::from(k > 0);
            for p in buf.iter().skip(skip) {
                if gi == next {
                    let _ = out.push((p.lon, p.lat));
                    kept += 1;
                    if kept == keep {
                        return out;
                    }
                    // The j-th kept point sits at j × (total−1) / (keep−1): endpoints exact,
                    // the rest an even stride (keep ≥ 2 here — keep == 1 returned above).
                    next = kept * (total - 1) / (keep - 1);
                }
                gi += 1;
            }
        }
        out
    }
}

/// Decode chunk `m` (its `n` points) from `src` into the already-cleared `out`: the anchor,
/// then each delta-stepped point. Shared by the cached and uncached decode paths.
fn decode_chunk_from(
    src: &dyn ByteSource,
    m: &ChunkMeta,
    n: usize,
    out: &mut Vec<RoutePoint, MAX_POINTS_PER_CHUNK>,
) -> Result<(), Error> {
    let _ = out.push(RoutePoint { lon: m.anchor_lon, lat: m.anchor_lat, ele: m.anchor_ele });

    // Remaining n-1 points are fixed 6-byte records; read the chunk in one go.
    let want = (n - 1) * 6;
    let mut buf = [0u8; (MAX_POINTS_PER_CHUNK - 1) * 6];
    let bytes = buf.get_mut(..want).ok_or(Error::TooLarge)?;
    if want > 0 {
        src.read_at(m.byte_offset, bytes)?;
    }

    let (mut lon, mut lat) = (m.anchor_lon, m.anchor_lat);
    let mut o = 0;
    for _ in 1..n {
        lon += rd_i16(bytes, o) as i32;
        lat += rd_i16(bytes, o + 2) as i32;
        let ele = rd_i16(bytes, o + 4);
        o += 6;
        let _ = out.push(RoutePoint { lon, lat, ele });
    }
    Ok(())
}

/// Resident decoded-route-chunk cache slots. Only the chunks crossing the view are decoded,
/// so a small LRU holds a frame's working set, sized to also absorb a wide zoomed-out view of
/// a winding route. `nrf-mem` trims to 2 slots (~6 KB) — the matcher's chunk plus one more for
/// the riding-zoom view, accepting re-decodes on a wide zoomed-out pan (issue #270: the cull
/// makes room for the BLE stack next to the map path on the 256 KB DK).
#[cfg(not(feature = "nrf-mem"))]
const ROUTE_CHUNK_SLOTS: usize = 32;
#[cfg(feature = "nrf-mem")]
const ROUTE_CHUNK_SLOTS: usize = 2;

/// One cache slot: a decoded chunk's points, keyed by chunk index, with LRU recency.
struct RouteSlot {
    valid: bool,
    key: u32,
    used: u32,
    pts: Vec<RoutePoint, MAX_POINTS_PER_CHUNK>,
}

/// A small resident cache of **decoded** route-geometry chunks — the route analogue of
/// `obc_reader::MapCache`. Without it, a per-frame map redraw and the matcher's per-fix decode
/// re-pull the same visible chunks from the SD card every time; holding the decoded points
/// resident turns those repeats into RAM copies.
///
/// Caller-owned and reused across frames (the device places one in its reserved region; the
/// host skips it), paired with the per-frame [`RouteReader`] via
/// [`new_cached`](RouteReader::new_cached). Slots are keyed by chunk index only, so
/// [`clear`](Self::clear) **must** be called on a route change, or a new route's chunk `k`
/// would hit the old route's stale slot.
///
/// State is in a `RefCell` so a `&RouteCache` `decode_chunk` (`&self`) can fill it; the borrow
/// is scoped to a single get/put.
pub struct RouteCache {
    inner: RefCell<RouteCacheInner>,
}

struct RouteCacheInner {
    tick: u32,
    slots: [RouteSlot; ROUTE_CHUNK_SLOTS],
    hits: u32,
    misses: u32,
}

impl Default for RouteCache {
    fn default() -> Self {
        Self::new()
    }
}

impl RouteCache {
    /// A fresh, empty cache. On the device, place it once in the reserved region (e.g.
    /// `ptr::write`) so it stays off the main stack.
    pub fn new() -> Self {
        RouteCache { inner: RefCell::new(RouteCacheInner::new()) }
    }

    /// Drop every resident slot and zero the counters — call on a route switch so the next
    /// decode misses. Only the `valid` flags + counters are touched, not the point buffers.
    pub fn clear(&self) {
        let mut inner = self.inner.borrow_mut();
        for s in &mut inner.slots {
            s.valid = false;
        }
        inner.tick = 0;
        inner.hits = 0;
        inner.misses = 0;
    }

    /// Cumulative `(hits, misses)` since the last [`clear`](Self::clear) — for the device's RTT
    /// route-cache log.
    pub fn stats(&self) -> (u32, u32) {
        let inner = self.inner.borrow();
        (inner.hits, inner.misses)
    }

    /// If chunk `key` is resident, copy its points into `out` (cleared first), bump recency + the
    /// hit counter, and return `true`; otherwise leave `out` untouched and return `false`.
    fn get(&self, key: usize, out: &mut Vec<RoutePoint, MAX_POINTS_PER_CHUNK>) -> bool {
        let mut inner = self.inner.borrow_mut();
        let key = key as u32;
        let Some(i) = inner.slots.iter().position(|s| s.valid && s.key == key) else {
            return false;
        };
        inner.hits = inner.hits.saturating_add(1);
        let t = inner.touch();
        inner.slots[i].used = t;
        out.clear();
        let _ = out.extend_from_slice(&inner.slots[i].pts);
        true
    }

    /// Store chunk `key`'s decoded `pts` into the LRU slot (evicting the least-recently-used) and
    /// count the miss that prompted it.
    fn put(&self, key: usize, pts: &[RoutePoint]) {
        let mut inner = self.inner.borrow_mut();
        inner.misses = inner.misses.saturating_add(1);
        let i = route_lru(inner.slots.iter().map(|s| (!s.valid, s.used)));
        let t = inner.touch();
        let s = &mut inner.slots[i];
        s.valid = true;
        s.key = key as u32;
        s.used = t;
        s.pts.clear();
        let _ = s.pts.extend_from_slice(pts);
    }
}

impl RouteCacheInner {
    fn new() -> Self {
        // `zeroed()` lowers to a `memset` (`.bss`); a struct literal zeroing the point buffers
        // would emit a `.rodata` const then `memcpy` it — which overflowed flash for the larger
        // `MapCache`.
        //
        // SAFETY: all-zero is a valid `RouteCacheInner` — no references, no non-zero-discriminant
        // enums, its only `bool` (`RouteSlot::valid`) is false at zero, and each `heapless::Vec`
        // is `{ len: 0, uninit buffer }` whose `MaybeUninit<RoutePoint>` backing is not read while
        // `len == 0`.
        unsafe { core::mem::MaybeUninit::zeroed().assume_init() }
    }

    #[inline]
    fn touch(&mut self) -> u32 {
        self.tick = self.tick.wrapping_add(1);
        self.tick
    }
}

/// Pick a slot to (re)fill: the first empty slot, else the least-recently-used. Input is
/// `(is_empty, used)` per slot in order. Mirrors `obc_reader`'s `lru`.
fn route_lru(slots: impl Iterator<Item = (bool, u32)>) -> usize {
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

impl core::ops::Deref for RouteReader<'_> {
    type Target = RouteIndex;
    fn deref(&self) -> &RouteIndex {
        self.idx
    }
}

/// Parsed header fields (shared by [`RouteIndex::read`] and [`RouteSummary::read`]).
struct Header {
    version: u8,
    bbox: BBox,
    start_lon: i32,
    start_lat: i32,
    point_count: u32,
    total_distance_m: u32,
    total_ascent_m: u32,
    total_descent_m: u32,
    min_ele_m: i16,
    max_ele_m: i16,
    chunk_count: u32,
    index_offset: u32,
    name: String<NAME_CAP>,
}

fn read_header(src: &dyn ByteSource) -> Result<Header, Error> {
    let mut h = [0u8; HEADER_LEN];
    src.read_at(0, &mut h).map_err(|_| Error::BadOffset)?;
    if &h[0..4] != MAGIC {
        return Err(Error::BadMagic);
    }
    if !VERSIONS.contains(&h[4]) {
        return Err(Error::BadVersion);
    }
    let name_len = (h[6] as usize).min(NAME_CAP);
    let mut name = String::new();
    if let Ok(s) = core::str::from_utf8(&h[64..64 + name_len]) {
        let _ = name.push_str(s);
    }
    Ok(Header {
        version: h[4],
        bbox: BBox {
            min_lon: rd_i32(&h, 8),
            min_lat: rd_i32(&h, 12),
            max_lon: rd_i32(&h, 16),
            max_lat: rd_i32(&h, 20),
        },
        start_lon: rd_i32(&h, 24),
        start_lat: rd_i32(&h, 28),
        point_count: rd_u32(&h, 32),
        total_distance_m: rd_u32(&h, 36),
        total_ascent_m: rd_u32(&h, 40),
        total_descent_m: rd_u32(&h, 44),
        min_ele_m: rd_i16(&h, 48),
        max_ele_m: rd_i16(&h, 50),
        chunk_count: rd_u32(&h, 52),
        index_offset: rd_u32(&h, 56),
        name,
    })
}

/// One stored route waypoint (`OBCR_Spec.md` §4): a POI pinned to a position along the route, as it
/// sits on disk — every field, `ele`/`kind` included. The ride *geometry* path still skips it (a
/// v2 route rides through the v1 code); [`RouteReader::load_waypoints`] distils the named ones into
/// the resident [`Waypoints`] table the waypoint UI reads. Also serves hosts and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waypoint {
    /// Cumulative distance from the route start to this waypoint's position, meters.
    pub dist_along_m: u32,
    /// The waypoint's own coordinate (microdegrees) — may sit off the polyline.
    pub lon: i32,
    pub lat: i32,
    /// Elevation in meters; [`WAYPOINT_ELE_NONE`] when the source carried none.
    pub ele: i16,
    /// Category byte (§4); `0` = generic. Render unknown values as generic.
    pub kind: u8,
    pub name: String<WAYPOINT_NAME_CAP>,
}

/// Visit each stored waypoint in route order (ascending `dist_along_m`), streaming one fixed
/// [`WAYPOINT_LEN`] record at a time — the low-level cursor over the whole (unfiltered, any-count)
/// section. [`RouteReader::load_waypoints`] layers the resident-table policy (name filter, window,
/// cap) on top of it. Returns the number visited; a v1 route (or a v2 route without waypoints)
/// yields none.
pub fn for_each_waypoint<F: FnMut(&Waypoint)>(src: &dyn ByteSource, mut f: F) -> Result<u16, Error> {
    let h = read_header(src)?;
    if h.version < 2 {
        return Ok(0);
    }
    let mut ext = [0u8; HEADER_V2_LEN - HEADER_LEN];
    src.read_at(HEADER_LEN as u32, &mut ext)?;
    let offset = rd_u32(&ext, 0);
    let count = rd_u16(&ext, 4);

    let mut rec = [0u8; WAYPOINT_LEN];
    for k in 0..count {
        src.read_at(offset + k as u32 * WAYPOINT_LEN as u32, &mut rec)?;
        let name_len = (rec[15] as usize).min(WAYPOINT_NAME_CAP);
        let mut name = String::new();
        if let Ok(s) = core::str::from_utf8(&rec[16..16 + name_len]) {
            let _ = name.push_str(s);
        }
        f(&Waypoint {
            dist_along_m: rd_u32(&rec, 0),
            lon: rd_i32(&rec, 4),
            lat: rd_i32(&rec, 8),
            ele: rd_i16(&rec, 12),
            kind: rec[14],
            name,
        });
    }
    Ok(count)
}

/// One resident waypoint: the compact subset of a stored [`Waypoint`] the ride UI actually needs —
/// its along-route position, its own coordinate, and its (non-empty) name. `ele` and `kind` are
/// **dropped** on purpose: the waypoint UI ignores both (plain diamonds; distances come from
/// `dist_along_m`), so a `Copy`-ish 40-byte entry stays cheap to hold [`MAX_WAYPOINTS`] resident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WptEntry {
    /// Cumulative distance from the route start to this waypoint, meters — the axis the ride
    /// progress, the progress-bar ticks, and the chip's distance-to-go all share.
    pub dist_along_m: u32,
    /// The waypoint's own coordinate (microdegrees) — where its map diamond is drawn.
    pub lon: i32,
    pub lat: i32,
    /// The waypoint's name (non-empty: an unnamed waypoint never enters the table).
    pub name: String<WAYPOINT_NAME_CAP>,
}

/// A route's resident named-waypoint table, in route order (ascending `dist_along_m`) — the
/// waypoint sibling of [`Climbs`](crate::Climbs). Built once per route load by
/// [`RouteReader::load_waypoints`] and cached in the app; the riding views then read it per frame.
///
/// Capacity is fixed at [`MAX_WAYPOINTS`]. When a file carries more named, in-window waypoints than
/// fit, the first-by-distance ones are kept and [`truncated`](Self::truncated) is set, so the ride
/// loop can slide the window forward once the rider passes the resident tail (re-window on
/// exhaustion — see the app's `tick`). A normal route (≤ cap) never truncates.
#[derive(Debug, Clone, Default)]
pub struct Waypoints {
    /// The kept named waypoints, route order (ascending `dist_along_m`).
    pub entries: Vec<WptEntry, MAX_WAYPOINTS>,
    /// `true` when the file had more qualifying (named, at/after the load window) waypoints than
    /// [`MAX_WAYPOINTS`] — the re-window signal. `false` for any route within the cap.
    pub truncated: bool,
}

impl Waypoints {
    /// An empty table (no route loaded, or a route without waypoints).
    #[inline]
    pub fn new() -> Self {
        Waypoints { entries: Vec::new(), truncated: false }
    }

    /// The kept waypoints in route order.
    #[inline]
    pub fn as_slice(&self) -> &[WptEntry] {
        &self.entries
    }

    /// Number of resident waypoints.
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table holds no waypoints.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl RouteReader<'_> {
    /// Load the route's **named** waypoints into a resident [`Waypoints`] table, windowed and capped
    /// for the device — the waypoint sibling of [`detect_climbs`](Self::detect_climbs). Streams the
    /// stored waypoint section via [`for_each_waypoint`] and keeps each record that
    ///
    /// - sits at or past `min_dist_m` (`dist_along_m >= min_dist_m`), and
    /// - has a non-empty name after trimming ASCII whitespace — an unnamed waypoint surfaces nowhere
    ///   in the UI (no diamond, tick, chip, or row), so it never enters the table.
    ///
    /// Records arrive in ascending `dist_along_m`, so the first [`MAX_WAYPOINTS`] kept are the nearest
    /// ahead of `min_dist_m`; a file with more qualifying waypoints stops filling and sets
    /// [`truncated`](Waypoints::truncated), so the caller can re-window forward with a larger
    /// `min_dist_m` once the rider passes the tail.
    ///
    /// O(waypoints), one small read per record — call on route load (and on re-window), never per
    /// frame. A v1 route, or a v2 route without waypoints, yields an empty table.
    pub fn load_waypoints(&self, min_dist_m: u32) -> Waypoints {
        let mut wpts = Waypoints::new();
        // A read error (a torn waypoint section) ends the stream early; the partial table is still
        // safe to hand back, matching `for_each_waypoint`'s best-effort contract.
        let _ = for_each_waypoint(self.src, |w| {
            if w.dist_along_m < min_dist_m {
                return;
            }
            // Unnamed = empty, or only ASCII whitespace: `all()` on the bytes is true for both.
            if w.name.as_bytes().iter().all(u8::is_ascii_whitespace) {
                return;
            }
            let entry = WptEntry { dist_along_m: w.dist_along_m, lon: w.lon, lat: w.lat, name: w.name.clone() };
            // Full: keep the first-by-distance ones already pushed and flag the overflow. Keep
            // streaming (don't break) so `truncated` reflects the whole file, not the first extra.
            if wpts.entries.push(entry).is_err() {
                wpts.truncated = true;
            }
        });
        wpts
    }
}
