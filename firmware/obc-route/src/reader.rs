//! OBCR route reader: header, chunk index, and on-demand chunk decode.
//!
//! [`RouteReader`] loads the fixed header and the (small) chunk index into RAM, then
//! pulls individual geometry chunks through the [`ByteSource`] only when asked — a
//! hundreds-of-km route never has to be resident. It is **monomorphic** (holds a
//! `&dyn ByteSource`), so it can be threaded through the app/render layers without
//! making them generic.

use core::cell::RefCell;

use heapless::{String, Vec};

use crate::byte_io::{ByteSource, Error};
use obc_reader::codec::{rd_i16, rd_i32, rd_u16, rd_u32};
use obc_reader::BBox;

/// Fixed header length (see `OBCR_Spec.md` §1).
pub const HEADER_LEN: usize = 112;
/// Per-chunk index entry length (§2).
pub const CHUNK_META_LEN: usize = 44;
/// Capacity of the inline route-name field, bytes.
pub const NAME_CAP: usize = 48;
/// Resident chunk-index capacity. With [`MAX_POINTS_PER_CHUNK`] the full profile caps a route at
/// ~131 k decimated points (≈ 24 KB `RouteIndex` at the cap); a longer route fails conversion with
/// [`Error::TooLarge`] rather than being silently coarsened. The constrained `nrf-mem` profile
/// (issue #124) trims it to 128 chunks (~33 k points, ~6 KB index). Two reasons it's the L15's
/// single most important trim, not just one of the balanced ones (#127): the N6 ride loop holds a
/// `RouteIndex` resident across frames (in the map plane's task future) to stream geometry without
/// re-walking it — so the index size lands in RAM directly — **and** [`read`](RouteIndex::read)
/// builds the index/`cum_seg` `Vec`s on the *stack* before returning by value, so on the 256 KB part
/// (with only ~16 KB of stack under the resident set) a 24 KB index would overflow the stack during
/// the build. 128 chunks keeps both the resident copy and that build spike to ~6 KB. The packer
/// (host) keeps the full 512, so a route packed past 128 chunks simply won't load on the L15
/// firmware (the 512 KB LM20 restores headroom); a typical decimated bikepacking route is far under
/// 33 k points.
#[cfg(not(feature = "nrf-mem"))]
pub const MAX_ROUTE_CHUNKS: usize = 512;
#[cfg(feature = "nrf-mem")]
pub const MAX_ROUTE_CHUNKS: usize = 128;
/// Max points a single chunk may hold (bounds the per-chunk decode buffer).
pub const MAX_POINTS_PER_CHUNK: usize = 256;

const MAGIC: &[u8; 4] = b"OBCR";
const VERSION: u8 = 1;

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

/// The lightweight route description for the Route menu: everything the list needs,
/// readable from the header alone (no chunk index) — so a catalog scan is one small
/// read per file.
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
    /// Read just the header into a summary — cheap enough to call for every file when
    /// building the Route-menu catalog.
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

/// The resident, source-independent parse of a route: the header summary fields plus the
/// chunk index and its segment prefix sums. [`read`](Self::read) does the route's only
/// up-front cost — the header read **and the full chunk-meta walk** — so afterwards a
/// [`RouteReader`] streams geometry chunk-by-chunk without re-reading the index.
///
/// Splitting this out of [`RouteReader`] lets a caller build it **once** when the active
/// route changes and reuse it across frames (the firmware render loop does exactly this,
/// the way the app caches the elevation [`Profile`](crate::Profile) — issue #44): a redraw
/// then pays only the geometry reads, not an N+1 re-walk of the index off the SD card.
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
    /// (∑ `point_count − 1`, the shared seam point not double-counted). A trailing
    /// entry holds the route's total segment count, so an index at `chunk_count` is
    /// valid. Built once at [`read`](Self::read) so [`global_seg_index`](Self::global_seg_index)
    /// — on the matcher's per-fix hot path — is an O(1) lookup, not a prefix scan.
    cum_seg: Vec<u32, { MAX_ROUTE_CHUNKS + 1 }>,
}

/// A parsed route, ready to query and decode: a [`RouteIndex`] (resident, reusable across
/// frames) paired with a shared borrow of the byte source its geometry chunks stream from.
/// Cheap to build via [`new`](Self::new) — the expensive parse lives in [`RouteIndex::read`].
///
/// Derefs to its [`RouteIndex`], so the summary fields (`bbox`, `total_distance_m`, …) and
/// the resident-only queries (`chunks`, `name`, …) read straight through `route.field` /
/// `route.method()` as before; only [`decode_chunk`](Self::decode_chunk) needs the source.
pub struct RouteReader<'a> {
    src: &'a dyn ByteSource,
    idx: &'a RouteIndex,
    /// Optional resident decoded-chunk cache (issue #98 P4). When present,
    /// [`decode_chunk`](Self::decode_chunk) serves an unchanged route from RAM instead of
    /// re-reading its geometry from the source every redraw / matcher fix. `None` keeps the
    /// original stream-every-call behaviour (the host store is fast, so the sim/tests skip it).
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

    /// Global index (from the route start) of segment `seg` in chunk `c`: how many
    /// segments precede it. Segments per chunk = `point_count − 1` (the shared seam
    /// point isn't double-counted). O(1) via the `cum_seg` prefix sum built at
    /// [`read`](Self::read) — the per-fix matcher calls this on its hot path, so it
    /// must not re-scan the chunk index. `c` past the last chunk clamps to the total.
    pub(crate) fn global_seg_index(&self, c: usize, seg: usize) -> usize {
        let c = c.min(self.index.len());
        self.cum_seg.get(c).copied().unwrap_or(0) as usize + seg
    }

    // Cumulative ascent at a position is read from the elevation [`Profile`]
    // ([`Profile::ascent_to`](crate::Profile::ascent_to)) at column resolution, not from
    // the coarse per-chunk `cum_ascent_m` (which, with few chunks, spread the climb
    // uniformly over distance and left a phantom "to climb" at the top of a climb).

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
    /// from. No I/O — the expensive header + chunk-meta walk already happened in
    /// [`RouteIndex::read`]; this just couples the resident index to a source so
    /// [`decode_chunk`](Self::decode_chunk) can pull chunks on demand. Build the index once
    /// per route and call this per frame (issue #44).
    pub fn new(idx: &'a RouteIndex, src: &'a dyn ByteSource) -> RouteReader<'a> {
        RouteReader { src, idx, cache: None }
    }

    /// Like [`new`](Self::new), but back [`decode_chunk`](Self::decode_chunk) with a resident
    /// [`RouteCache`] (issue #98 P4). The cache is caller-owned and lives across frames (the
    /// device places one in its reserved region, like the map's `MapCache`), so a redraw of an unchanged route
    /// — and the matcher's per-fix chunk decode — hit RAM instead of re-reading the geometry from
    /// the SD card on every frame. The cache keys slots by chunk index only, so the caller must
    /// [`RouteCache::clear`] it whenever the active route changes.
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
        // Cache fast path: a hit fills `out` with no SD read; a miss decodes from the source and
        // stores the result, so the next redraw of the same chunk is free.
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
}

/// Decode chunk `m` (its `n` points) from `src` into the already-cleared `out`: the anchor, then
/// each delta-stepped point. Factored out of [`RouteReader::decode_chunk`] so both the cached and
/// uncached paths share the one decoder (the cache only saves the read + this work, never changes
/// the bytes produced).
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

/// Resident decoded-route-chunk cache slots. A route is at most [`MAX_ROUTE_CHUNKS`] chunks, but
/// real routes are a handful and only the chunks crossing the view are ever decoded, so a small
/// LRU holds a frame's working set; sized to also absorb a wide zoomed-out view of a winding
/// route. The win is per-redraw, not per-route — see [`RouteCache`].
///
/// The constrained `nrf-mem` profile (issue #124) trims this to 3 slots (~9 KB): enough for the
/// few chunks a riding-zoom view crosses, accepting re-decodes on a wide zoomed-out pan as part of
/// the L15 memory budget. Dropped 4→3 as a 256 KB-DK stop-gap (~3 KB more stack for the deep
/// ride-loop render — see `obc-fw-nrf54l` budget note); the 512 KB production part has the headroom
/// to restore it.
#[cfg(not(feature = "nrf-mem"))]
const ROUTE_CHUNK_SLOTS: usize = 32;
#[cfg(feature = "nrf-mem")]
const ROUTE_CHUNK_SLOTS: usize = 3;

/// One cache slot: a decoded chunk's points, keyed by chunk index, with LRU recency.
struct RouteSlot {
    valid: bool,
    key: u32,
    used: u32,
    pts: Vec<RoutePoint, MAX_POINTS_PER_CHUNK>,
}

/// A small resident cache of **decoded** route-geometry chunks — the route analogue of
/// `obc_reader::MapCache`. [`RouteReader::decode_chunk`] re-reads a chunk's geometry from the
/// byte source on every call, so without a cache a per-frame map redraw (and the matcher's
/// per-fix decode) re-pulls the same visible chunks from the SD card every time. Holding the
/// decoded points resident turns those repeats into RAM copies (issue #98 P4).
///
/// Caller-owned and reused across frames: the device places one in its reserved region for the
/// session (like `MapCache`) and pairs it with the per-frame [`RouteReader`] via
/// [`new_cached`](RouteReader::new_cached); the host just skips it. Slots are keyed by chunk
/// index only (a route has its own source), so [`clear`](Self::clear) **must** be called when the
/// active route changes, or a new route's chunk `k` would hit the old route's stale slot.
///
/// Wraps its state in a `RefCell` so a `&RouteCache` `decode_chunk` (an `&self` path) can fill it,
/// mirroring `MapCache`; the borrow is scoped to a single get/put.
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
    /// A fresh, empty cache (~99 KB of zeroed slots). On the device, place it once in the reserved
    /// region (e.g. `ptr::write`, like the `App` / `MapCache`) so it stays off the main stack.
    pub fn new() -> Self {
        RouteCache { inner: RefCell::new(RouteCacheInner::new()) }
    }

    /// Drop every resident slot (and zero the counters) — call on a route switch so the next
    /// decode misses and refills from the new route's geometry. Cheap: only the `valid` flags +
    /// counters are touched, not the point buffers.
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
    /// route-cache log (the analogue of `MapCache`'s render-stats counters).
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
        // Zero-init the whole thing. All-zero is a valid `RouteCacheInner`: `valid: false`, the
        // integer counters 0, and each `heapless::Vec` is `{ len: 0, uninit buffer }` — a valid
        // empty vec whose backing is never read while empty. `zeroed()` lowers to a `memset`
        // (`.bss`), whereas a struct literal zeroing the ~99 KB of point buffers would emit a
        // `.rodata` const that is then `memcpy`'d — which overflowed flash on the MCU for the
        // larger `MapCache`, so it uses the same trick.
        //
        // SAFETY: `RouteCacheInner` is inhabited and valid for the all-zero bit pattern — no
        // references, no non-zero-discriminant enums, and its only `bool`s (`RouteSlot::valid`)
        // are false at zero. The `MaybeUninit<RoutePoint>` backing of each `Vec` is valid for any
        // bits and is not read while `len == 0`.
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

/// Deref to the index so the summary fields and resident-only queries read straight through
/// `route.bbox` / `route.chunks()` without forwarding boilerplate (only `decode_chunk` needs
/// the source, and it's inherent).
impl core::ops::Deref for RouteReader<'_> {
    type Target = RouteIndex;
    fn deref(&self) -> &RouteIndex {
        self.idx
    }
}

/// Parsed header fields (shared by [`RouteIndex::read`] and [`RouteSummary::read`]).
struct Header {
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
    if h[4] != VERSION {
        return Err(Error::BadVersion);
    }
    let name_len = (h[6] as usize).min(NAME_CAP);
    let mut name = String::new();
    if let Ok(s) = core::str::from_utf8(&h[64..64 + name_len]) {
        let _ = name.push_str(s);
    }
    Ok(Header {
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
