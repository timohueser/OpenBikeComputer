//! OBCR route reader: header, chunk index, and on-demand chunk decode.
//!
//! [`RouteReader`] loads the fixed header and the (small) chunk index into RAM, then
//! pulls individual geometry chunks through the [`ByteSource`] only when asked — a
//! hundreds-of-km route never has to be resident. It holds a `&dyn ByteSource` (not a
//! generic), so it threads through the app/render layers without making them generic.

use core::{
    cell::RefCell,
    sync::atomic::{AtomicU32, Ordering},
};

use heapless::{String, Vec};

use obc_formats::io::{rd_i16, rd_i32, rd_u16, rd_u32, ByteSource, Error};
use obc_reader::BBox;

// The OBCR format constants this reader parses against are owned by `obc-formats`; imported here.
// Not re-exported — consumers reach the format authority via `obc_formats::obcr`.
use obc_formats::obcr::{
    CHUNK_META_LEN, HEADER_FULL_LEN, HEADER_LEN, NAME_CAP, WAYPOINT_LEN, WAYPOINT_NAME_CAP, WAYPOINT_NAME_OFF,
};
use obc_formats::obcr::{MAGIC, POINT_RECORD_LEN, VERSIONS};
use obc_reader::PoiCategory;
/// The device's waypoint cap — one number for both roles: the converter's `<wpt>` emission cap
/// ([`gpx_to_obcr`](crate::gpx_to_obcr)) and the resident [`Waypoints`] table the ride loop holds
/// (~40 B/entry ≈ 1.3 KB — negligible on the 512 KB target). The *format* allows up to `u16::MAX`
/// waypoints (a phone-side encoder isn't bound by this), so [`RouteReader::load_waypoints`] windows
/// + truncates a longer file rather than overflowing.
pub const MAX_WAYPOINTS: usize = 32;
/// Resident chunk-index capacity — **the one knob that sets both the max route length and a
/// large slice of the device's stack peak**, because a [`RouteIndex`] is `MAX_ROUTE_CHUNKS × 48 B`
/// and several call paths hold one (or more) on the stack. A route past the cap fails conversion
/// with [`Error::TooLarge`] rather than being silently coarsened; the value is shared with the
/// host packer, so anything that packs, loads.
///
/// **256** ≈ 65 k points (~650 km at 10 m spacing) for a ~12.3 KB index. 512 was tried during the
/// LM20 retarget and measured *far* too expensive on glass: it put a 73.7 KB frame in
/// [`elevation_sparkline`](crate::elevation_sparkline) — larger than the whole 69 KB stack region,
/// i.e. a guaranteed overflow on the first phone route upload (2026-07-24). Raising it again means
/// first making the by-value paths resident (see [`read_into`](RouteIndex::read_into)).
pub const MAX_ROUTE_CHUNKS: usize = 256;
const _: () = assert!(MAX_ROUTE_CHUNKS < u16::MAX as usize);
/// Max points a single chunk may hold (bounds the per-chunk decode buffer).
pub const MAX_POINTS_PER_CHUNK: usize = 256;

/// One decoded route point: position in microdegrees + elevation in meters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutePoint {
    pub lon: i32,
    pub lat: i32,
    pub ele: i16,
}

/// An interpolated position on the route polyline at an exact, clamped along-route distance.
///
/// The public fields are the coordinate/distance a map chooser needs; the containing chunk and
/// segment stay crate-private so [`RouteMatch`](crate::RouteMatch) can move its forward cursor to
/// the same point without exposing file-layout details to applications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutePosition {
    pub progress_m: u32,
    pub lon: i32,
    pub lat: i32,
    pub(crate) chunk: usize,
    pub(crate) seg: usize,
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
/// [`RouteSummary`]'s display-rounded km) plus the waypoint count from the header
/// extension. Reads the base header and the 16-byte extension; never the chunk index.
#[derive(Debug, Clone)]
pub struct RouteObjectInfo {
    pub name: String<NAME_CAP>,
    pub distance_m: u32,
    pub ascent_m: u32,
    pub point_count: u32,
    pub waypoint_count: u16,
}

impl RouteObjectInfo {
    /// Read the header (+ extension) into the wire facts. Same validation as any header
    /// read (bad magic/version/name reject), which the upload commit path relies on to keep
    /// a non-OBCR payload — or a pre-v3 route — out of the catalog.
    pub fn read(src: &dyn ByteSource) -> Result<RouteObjectInfo, Error> {
        let h = read_header(src)?;
        let waypoint_count = {
            let mut ext = [0u8; HEADER_FULL_LEN - HEADER_LEN];
            src.read_at(HEADER_LEN as u32, &mut ext).map_err(|_| Error::BadOffset)?;
            rd_u16(&ext, 4)
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
    /// (∑ `point_count − 1`, the shared seam point not double-counted). The total is derived in
    /// O(1) from the last prefix + last chunk, avoiding a redundant trailing word; this offsets
    /// the identity word so [`RouteIndex`] does not grow. Built once at [`read`](Self::read) so
    /// [`global_seg_index`](Self::global_seg_index) — on the matcher's per-fix hot path — remains
    /// O(1), not a prefix scan.
    cum_seg: Vec<u32, MAX_ROUTE_CHUNKS>,
    /// Non-persisted identity of this successful parse. Moves preserve it, so a by-value host
    /// index and the board's in-place resident slot have identical cache-adoption semantics.
    /// Zero belongs only to [`empty`](Self::empty) / a failed parse.
    identity: u32,
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
    /// An empty, chunk-less index — the resident slot [`read_into`](Self::read_into) fills.
    /// Queryable but matches nothing; callers that need "is there a route?" keep their own
    /// validity flag (a failed `read_into` leaves the slot in exactly this state).
    pub fn empty() -> RouteIndex {
        RouteIndex {
            bbox: BBox { min_lon: 0, min_lat: 0, max_lon: 0, max_lat: 0 },
            start_lon: 0,
            start_lat: 0,
            point_count: 0,
            total_distance_m: 0,
            total_ascent_m: 0,
            total_descent_m: 0,
            min_ele_m: 0,
            max_ele_m: 0,
            name: String::new(),
            index: Vec::new(),
            cum_seg: Vec::new(),
            identity: 0,
        }
    }

    /// Parse the header and chunk index from `src`. Validates magic/version and that
    /// every chunk lies within the source and within the resident buffers.
    ///
    /// Returns the ~12.3 KB index **by value** — fine on a std host (the sim, tests, `obc-pack`),
    /// but on the MCU that value transits the stack right where the ride pass is deepest. A
    /// board caller must use [`read_into`](Self::read_into) on its resident slot instead: the
    /// by-value return is exactly what overflowed the 44 KB main stack on the 256 KB DK when the
    /// post-upload rescan rebuilt the index (STKOF HardFault in this frame, 2026-07-12).
    ///
    /// `#[inline(never)]` is load-bearing, not a hint: inlined into a caller, the index-building
    /// temporaries coexist with the returned value in *one* frame — measured as ~3 live copies
    /// (73.7 KB) in `elevation_sparkline` on the LM20. Kept out of line, the build temporaries
    /// live in this frame and pop before the caller continues, so a caller pays for one index.
    #[inline(never)]
    pub fn read(src: &dyn ByteSource) -> Result<RouteIndex, Error> {
        let mut idx = RouteIndex::empty();
        idx.read_into(src)?;
        Ok(idx)
    }

    /// The in-place twin of [`read`](Self::read): fill `self` — the caller's **resident** slot —
    /// field by field, so the index never exists as a stack temporary. On any error `self` is
    /// left as [`empty`](Self::empty) (never half-filled), and the caller's validity flag stays
    /// down.
    pub fn read_into(&mut self, src: &dyn ByteSource) -> Result<(), Error> {
        let r = self.fill_from(src);
        if r.is_err() {
            self.name.clear();
            self.index.clear();
            self.cum_seg.clear();
            self.point_count = 0;
            self.identity = 0;
        }
        r
    }

    fn fill_from(&mut self, src: &dyn ByteSource) -> Result<(), Error> {
        self.name.clear();
        self.index.clear();
        self.cum_seg.clear();
        self.identity = 0;

        let h = read_header(src)?;
        if h.chunk_count as usize > MAX_ROUTE_CHUNKS {
            return Err(Error::TooLarge);
        }

        let mut seg_acc: u32 = 0;
        let mut meta = [0u8; CHUNK_META_LEN];
        for k in 0..h.chunk_count {
            let off = h.index_offset + k * CHUNK_META_LEN as u32;
            src.read_at(off, &mut meta)?;
            let cm = parse_chunk_meta(&meta, src.len())?;
            // Running segment prefix sum, built alongside the index so the matcher never
            // re-walks the chunk list per fix.
            self.cum_seg.push(seg_acc).map_err(|_| Error::TooLarge)?;
            seg_acc += (cm.point_count as u32).saturating_sub(1);
            self.index.push(cm).map_err(|_| Error::TooLarge)?;
        }
        self.bbox = h.bbox;
        self.start_lon = h.start_lon;
        self.start_lat = h.start_lat;
        self.point_count = h.point_count;
        self.total_distance_m = h.total_distance_m;
        self.total_ascent_m = h.total_ascent_m;
        self.total_descent_m = h.total_descent_m;
        self.min_ele_m = h.min_ele_m;
        self.max_ele_m = h.max_ele_m;
        self.name = h.name;
        self.identity = next_route_identity()?;
        Ok(())
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
        self.cum_seg.get(c).copied().unwrap_or_else(|| self.segment_count()) as usize + seg
    }

    /// Total seam-deduplicated segments. The last prefix excludes the last chunk, so add that
    /// chunk's own `point_count - 1`; empty indexes have no segments.
    #[inline]
    fn segment_count(&self) -> u32 {
        match (self.cum_seg.last(), self.index.last()) {
            (Some(before_last), Some(last)) => before_last + (last.point_count as u32).saturating_sub(1),
            _ => 0,
        }
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

    /// The underlying byte source — for the crate's own whole-file passes over sections the
    /// chunk index doesn't cover (the splicer's [`for_each_waypoint`] sweep).
    pub(crate) fn source(&self) -> &dyn ByteSource {
        self.src
    }

    /// Like [`new`](Self::new), but back [`decode_chunk`](Self::decode_chunk) with a resident
    /// [`RouteCache`], so a redraw of an unchanged route — and the matcher's per-fix decode —
    /// hit RAM instead of re-reading geometry from the SD card. The cache adopts the index's
    /// parse identity here, automatically invalidating same-key slots from a different route;
    /// callers do not need to coordinate a [`RouteCache::clear`] on route switches.
    pub fn new_cached(idx: &'a RouteIndex, src: &'a dyn ByteSource, cache: &'a RouteCache) -> RouteReader<'a> {
        cache.adopt(idx.identity);
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
        // A hit fills `out` with no SD read; a miss decodes and stores it. Revalidate the reader
        // identity for both operations: another safe reader can use the same cache between calls,
        // including reentrantly from `ByteSource::read_at` while this miss is being decoded.
        if let Some(cache) = self.cache {
            if cache.get(self.idx.identity, k, out) {
                return Ok(());
            }
            decode_chunk_from(self.src, m, n, out)?;
            cache.put(self.idx.identity, k, out);
            return Ok(());
        }
        decode_chunk_from(self.src, m, n, out)
    }

    /// Locate `progress_m` on the route, clamping it to the route end and linearly interpolating
    /// inside the containing segment. Uses caller-owned decode scratch so the matcher can seek its
    /// resident buffer without adding a stack-sized route copy.
    pub(crate) fn locate_progress(
        &self,
        progress_m: u32,
        buf: &mut Vec<RoutePoint, MAX_POINTS_PER_CHUNK>,
    ) -> Option<RoutePosition> {
        let target = progress_m.min(self.total_distance_m);
        let (p, chunk, seg) = self.locate_interpolated(target, buf)?;
        Some(RoutePosition { progress_m: target, lon: p.lon, lat: p.lat, chunk, seg })
    }

    /// The shared clamped-walk core of [`locate_progress`](Self::locate_progress) and
    /// [`elevation_at`](Self::elevation_at): the interpolated [`RoutePoint`] at `target`
    /// (already clamped by the caller) plus its containing chunk and segment.
    fn locate_interpolated(
        &self,
        target: u32,
        buf: &mut Vec<RoutePoint, MAX_POINTS_PER_CHUNK>,
    ) -> Option<(RoutePoint, usize, usize)> {
        let chunks = self.chunks();
        let k = chunks.iter().rposition(|cm| cm.cum_distance_m <= target).unwrap_or(0);
        let cm = chunks.get(k)?;
        self.decode_chunk(k, buf).ok()?;
        let first = *buf.first()?;
        if buf.len() == 1 {
            return Some((first, k, 0));
        }

        let cl = crate::geo::cos_lat(first.lat);
        let mut s = cm.cum_distance_m as f32;
        for i in 0..buf.len() - 1 {
            let a = buf[i];
            let b = buf[i + 1];
            let dl = crate::geo::seg_dist_m_cl((a.lon, a.lat), (b.lon, b.lat), cl);
            let last = i + 2 == buf.len();
            if target as f32 <= s + dl || last {
                let t = if dl > 1e-3 { ((target as f32 - s) / dl).clamp(0.0, 1.0) } else { 0.0 };
                return Some((interpolate_point(a, b, t), k, i));
            }
            s += dl;
        }
        None
    }

    /// The interpolated elevation at `progress_m`, clamped to the route end — the splice path's
    /// seam-endpoint sampler ([`locate_progress`](Self::locate_progress) keeps position only;
    /// this keeps the elevation those callers drop). Cold path with its own decode scratch.
    #[inline(never)]
    pub(crate) fn elevation_at(&self, progress_m: u32) -> Option<i16> {
        let mut buf = Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
        let target = progress_m.min(self.total_distance_m);
        Some(self.locate_interpolated(target, &mut buf)?.0.ele)
    }

    /// Return the coordinate at `progress_m`, clamped to the route end. This is the cold UI-facing
    /// wrapper around [`locate_progress`](Self::locate_progress); the hot matcher supplies its own
    /// resident scratch instead.
    #[inline(never)]
    pub fn position_at(&self, progress_m: u32) -> Option<RoutePosition> {
        let mut buf = Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
        self.locate_progress(progress_m, &mut buf)
    }

    /// Stream only the polyline stretch in the inclusive along-route interval `[start_m, end_m]`.
    /// Each callback slice is one clipped chunk: its first and last coordinates are interpolated at
    /// the interval boundary, with no retained route copy. Decode failures skip that chunk, matching
    /// the normal route-overlay contract.
    #[inline(never)]
    pub fn visit_points_between(&self, start_m: u32, end_m: u32, mut visit: impl FnMut(&[(i32, i32)])) {
        let lo = start_m.min(self.total_distance_m);
        let hi = end_m.min(self.total_distance_m);
        if lo >= hi {
            return;
        }
        let chunks = self.chunks();
        // Keep only coordinate scratch live across `visit`: the deeper RoutePoint decode frame is
        // `#[inline(never)]` below and has returned before a renderer's stroke/fill stack starts.
        // This mirrors `obc-app::route::decode_lonlat`'s measured stack-lifetime discipline.
        let mut lonlat = [(0i32, 0i32); MAX_POINTS_PER_CHUNK];
        for (k, cm) in chunks.iter().enumerate() {
            let chunk_hi = chunks.get(k + 1).map_or(self.total_distance_m, |next| next.cum_distance_m);
            if chunk_hi < lo || cm.cum_distance_m > hi {
                continue;
            }
            if let Some(n) = decode_points_between(self, k, lo, hi, &mut lonlat) {
                visit(&lonlat[..n]);
            }
        }
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
        // Distinct points = total segments + 1; an index with no chunks has nothing to walk.
        if self.idx.index.is_empty() || N == 0 {
            return out;
        }
        let total = self.idx.segment_count() as usize + 1;
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

/// The route-corridor POI query's geometry seam (epic #946, U2). `obc-reader` sits **below** this
/// crate, so it cannot name a [`RouteReader`]; it declares [`RoutePath`](obc_reader::RoutePath) and
/// the OBCR side implements it — the same inversion `obc-render`'s `RouteOverlaySource` uses for
/// the map overlay.
///
/// Everything but [`visit_chunk_points`](obc_reader::RoutePath::visit_chunk_points) reads the
/// **resident** chunk index (no I/O); the point visit decodes one chunk through
/// [`decode_chunk`](RouteReader::decode_chunk), so with a [`RouteCache`] attached a snapshot over a
/// route the ride loop is already streaming costs no extra card reads for the chunks it has seen.
impl obc_reader::RoutePath for RouteReader<'_> {
    #[inline]
    fn chunk_count(&self) -> usize {
        self.chunks().len()
    }

    #[inline]
    fn chunk_start_m(&self, k: usize) -> u32 {
        // Past the last chunk the answer is "the route end" — the contract the corridor query's
        // chunk-extent arithmetic relies on.
        self.chunks().get(k).map_or(self.total_distance_m, |cm| cm.cum_distance_m)
    }

    #[inline]
    fn chunk_bbox(&self, k: usize) -> BBox {
        self.chunks().get(k).map(|cm| cm.bbox).unwrap_or(BBox { min_lon: 0, min_lat: 0, max_lon: 0, max_lat: 0 })
    }

    fn visit_chunk_points(&self, k: usize, visit: &mut dyn FnMut(&[(i32, i32)])) {
        // Only coordinate scratch stays live across `visit`: the deeper `RoutePoint` decode frame is
        // `#[inline(never)]` and has returned before the query descends into the POI quadtree walk.
        // Same measured stack-lifetime discipline as `visit_points_between`.
        let mut lonlat = [(0i32, 0i32); MAX_POINTS_PER_CHUNK];
        if let Some(n) = decode_chunk_lonlat(self, k, &mut lonlat) {
            visit(&lonlat[..n]);
        }
    }
}

/// Decode chunk `k` into caller-owned `(lon, lat)` scratch, returning the point count. Kept out of
/// line so its `Vec<RoutePoint, 256>` frame is gone before the caller's callback runs (see
/// [`decode_points_between`], the same rule).
#[inline(never)]
fn decode_chunk_lonlat(route: &RouteReader, k: usize, out: &mut [(i32, i32); MAX_POINTS_PER_CHUNK]) -> Option<usize> {
    let mut buf = Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
    route.decode_chunk(k, &mut buf).ok()?;
    for (dst, p) in out.iter_mut().zip(buf.iter()) {
        *dst = (p.lon, p.lat);
    }
    Some(buf.len())
}

fn interpolate_point(a: RoutePoint, b: RoutePoint, t: f32) -> RoutePoint {
    RoutePoint {
        lon: libm::roundf(a.lon as f32 + (b.lon - a.lon) as f32 * t) as i32,
        lat: libm::roundf(a.lat as f32 + (b.lat - a.lat) as f32 * t) as i32,
        ele: libm::roundf(a.ele as f32 + (b.ele - a.ele) as f32 * t) as i16,
    }
}

/// Decode and clip one route chunk into caller-owned `(lon, lat)` scratch. Kept out of line so its
/// `Vec<RoutePoint, 256>` frame is gone before [`RouteReader::visit_points_between`]'s callback
/// enters the renderer's stroke/fill stack.
#[inline(never)]
fn decode_points_between(
    route: &RouteReader,
    k: usize,
    lo: u32,
    hi: u32,
    out: &mut [(i32, i32); MAX_POINTS_PER_CHUNK],
) -> Option<usize> {
    let mut buf = Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
    let n = decode_route_points_between(route, k, lo, hi, &mut buf)?;
    for (dst, p) in out.iter_mut().zip(buf.iter()) {
        *dst = (p.lon, p.lat);
    }
    Some(n)
}

/// Decode chunk `k` and clip it in place to the inclusive along-route interval `[lo, hi]`,
/// keeping the full [`RoutePoint`] records: `buf` ends up holding only the clipped stretch, its
/// first and last points interpolated at the interval boundary (elevation included). This is the
/// splice path's chunk primitive; [`decode_points_between`] layers the render-facing `(lon, lat)`
/// view on top so there is exactly one clipping implementation. Returns the kept point count;
/// `None` when the chunk misses the interval or fails to decode.
#[inline(never)]
pub(crate) fn decode_route_points_between(
    route: &RouteReader,
    k: usize,
    lo: u32,
    hi: u32,
    buf: &mut Vec<RoutePoint, MAX_POINTS_PER_CHUNK>,
) -> Option<usize> {
    let cm = route.chunks().get(k)?;
    route.decode_chunk(k, buf).ok()?;
    if buf.len() < 2 {
        return None;
    }
    let cl = crate::geo::cos_lat(buf[0].lat);
    let mut s = cm.cum_distance_m as f32;
    let mut first: Option<(usize, RoutePoint)> = None;
    let mut last: Option<(usize, RoutePoint)> = None;
    for i in 0..buf.len() - 1 {
        let a = buf[i];
        let b = buf[i + 1];
        let dl = crate::geo::seg_dist_m_cl((a.lon, a.lat), (b.lon, b.lat), cl);
        let seg_hi = s + dl;
        if seg_hi >= lo as f32 && s <= hi as f32 {
            let t0 = if dl > 1e-3 { ((lo as f32 - s) / dl).clamp(0.0, 1.0) } else { 0.0 };
            let t1 = if dl > 1e-3 { ((hi as f32 - s) / dl).clamp(0.0, 1.0) } else { 1.0 };
            first.get_or_insert((i, interpolate_point(a, b, t0)));
            last = Some((i + 1, interpolate_point(a, b, t1)));
        }
        s = seg_hi;
        if s > hi as f32 {
            break;
        }
    }
    let (a, pa) = first?;
    let (b, pb) = last?;
    let n = b - a + 1;
    // Shift the kept stretch to the front in place — no second point buffer on the stack.
    for i in 0..n {
        buf[i] = buf[a + i];
    }
    buf.truncate(n);
    buf[0] = pa;
    buf[n - 1] = pb;
    Some(n)
}

/// Decode chunk `m` (its `n` points) from `src` into the already-cleared `out`: the anchor,
/// then each delta-stepped point. Shared by the cached and uncached decode paths.
/// Decode one §2 chunk-meta record (validating its point count and that its data region lies
/// inside `src_len`). Factored out of [`RouteIndex::fill_from`] so a **streaming** consumer —
/// one that walks chunks without ever materialising the whole index — parses metas through the
/// exact same code path; see [`elevation_sparkline`](crate::elevation_sparkline).
pub(crate) fn parse_chunk_meta(meta: &[u8; CHUNK_META_LEN], src_len: u32) -> Result<ChunkMeta, Error> {
    let point_count = rd_u16(meta, 26);
    if point_count as usize > MAX_POINTS_PER_CHUNK {
        return Err(Error::TooLarge);
    }
    let cm = ChunkMeta {
        bbox: BBox {
            min_lon: rd_i32(meta, 0),
            min_lat: rd_i32(meta, 4),
            max_lon: rd_i32(meta, 8),
            max_lat: rd_i32(meta, 12),
        },
        anchor_lon: rd_i32(meta, 16),
        anchor_lat: rd_i32(meta, 20),
        anchor_ele: rd_i16(meta, 24),
        point_count,
        cum_distance_m: rd_u32(meta, 28),
        cum_ascent_m: rd_u32(meta, 32),
        byte_offset: rd_u32(meta, 36),
        byte_len: rd_u32(meta, 40),
    };
    // Bounds-check the chunk's data region up front (no per-decode checks).
    let end = cm.byte_offset.checked_add(cm.byte_len).ok_or(Error::BadOffset)?;
    if end > src_len {
        return Err(Error::BadOffset);
    }
    Ok(cm)
}

pub(crate) fn decode_chunk_from(
    src: &dyn ByteSource,
    m: &ChunkMeta,
    n: usize,
    out: &mut Vec<RoutePoint, MAX_POINTS_PER_CHUNK>,
) -> Result<(), Error> {
    let _ = out.push(RoutePoint { lon: m.anchor_lon, lat: m.anchor_lat, ele: m.anchor_ele });

    // Remaining n-1 points are fixed 6-byte records; read the chunk in one go.
    let want = (n - 1) * POINT_RECORD_LEN;
    let mut buf = [0u8; (MAX_POINTS_PER_CHUNK - 1) * POINT_RECORD_LEN];
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
        o += POINT_RECORD_LEN;
        let _ = out.push(RoutePoint { lon, lat, ele });
    }
    Ok(())
}

/// Allocate a non-zero, process-local parse identity. This is deliberately independent of the
/// OBCR bytes and source: parsing the same bytes into a new resident session gets a new identity,
/// while moving/reborrowing that parsed [`RouteIndex`] keeps its identity and cache hits.
///
/// A 32-bit token is the target's native atomic width and takes one word in [`RouteIndex`] plus one
/// owner word in [`RouteCache`]. Zero preserves the all-zero empty/cache initialization contract,
/// and the counter never wraps: after exhausting the non-zero token space, parses fail closed
/// instead of reusing an identity that a long-lived cache could still own.
fn next_route_identity() -> Result<u32, Error> {
    // Zero-init keeps the allocator in `.bss`; the returned token is the successfully stored next
    // value, so zero itself is never live.
    static LAST: AtomicU32 = AtomicU32::new(0);
    LAST.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |identity| identity.checked_add(1))
        .map(|identity| identity + 1)
        .map_err(|_| Error::TooLarge)
}

/// Resident decoded-route-chunk cache slots. Only the chunks crossing the view are decoded,
/// so a small LRU holds a frame's working set, sized to also absorb a wide zoomed-out view of
/// a winding route: the matcher's chunk, the riding-zoom view, and one spare for a zoomed-out
/// pan (3 × ~3 KB ≈ 9 KB; a very wide view of a winding route still re-decodes, accepted).
const ROUTE_CHUNK_SLOTS: usize = 3;

/// One cache slot: a decoded chunk's points, keyed by chunk index, with LRU recency. The owning
/// route identity lives once on [`RouteCacheInner`]. The key is stored as `index + 1`, reserving
/// zero as the empty tag so the cache remains safe to create from all-zero memory without a
/// separate validity byte.
struct RouteSlot {
    tag: u16,
    // LRU order, not a diagnostic counter. It is rebased before exhaustion, preserving exact
    // ordering while keeping the slot header at four bytes on the target.
    used: u16,
    pts: Vec<RoutePoint, MAX_POINTS_PER_CHUNK>,
}

/// A small resident cache of **decoded** route-geometry chunks — the route analogue of
/// `obc_reader::MapCache`. Without it, a per-frame map redraw and the matcher's per-fix decode
/// re-pull the same visible chunks from the SD card every time; holding the decoded points
/// resident turns those repeats into RAM copies.
///
/// Caller-owned and reused across frames (the device places one in its reserved region; the
/// host skips it), paired with the per-frame [`RouteReader`] via
/// [`new_cached`](RouteReader::new_cached). Slots remain keyed by chunk index, while the cache as a
/// whole adopts the parsed [`RouteIndex`]'s identity. A different route therefore invalidates all
/// same-key slots by construction; [`clear`](Self::clear) remains an optional explicit reset.
///
/// State is in a `RefCell` so a `&RouteCache` `decode_chunk` (`&self`) can fill it; the borrow
/// is scoped to a single get/put.
pub struct RouteCache {
    inner: RefCell<RouteCacheInner>,
}

struct RouteCacheInner {
    /// The successful [`RouteIndex`] parse whose chunks occupy the slots. Zero is the unowned
    /// all-zero initialization state and is never assigned to a parsed index.
    identity: u32,
    tick: u16,
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

    /// Drop every resident slot and zero the counters. Route switches already invalidate
    /// automatically through [`RouteReader::new_cached`]; this remains useful for diagnostics and
    /// explicit resets. Only the slot tags + counters are touched, not the point buffers.
    pub fn clear(&self) {
        self.inner.borrow_mut().clear();
    }

    /// Cumulative `(hits, misses)` since the last [`clear`](Self::clear) — for the device's RTT
    /// route-cache log.
    pub fn stats(&self) -> (u32, u32) {
        let inner = self.inner.borrow();
        (inner.hits, inner.misses)
    }

    /// Bind the cache to one parsed route session. A different identity clears all same-index
    /// slots before the reader can decode; reborrowing or moving the same [`RouteIndex`] preserves
    /// its identity and therefore preserves hits. Identity zero is accepted for the public empty
    /// index: it owns no decodable chunks, and a later non-zero parsed identity still invalidates.
    fn adopt(&self, identity: u32) {
        self.inner.borrow_mut().adopt(identity);
    }

    /// If chunk `key` is resident, copy its points into `out` (cleared first), bump recency + the
    /// hit counter, and return `true`; otherwise leave `out` untouched and return `false`. Identity
    /// adoption and lookup share one borrow so an interleaved reader cannot cross-serve a slot.
    fn get(&self, identity: u32, key: usize, out: &mut Vec<RoutePoint, MAX_POINTS_PER_CHUNK>) -> bool {
        let mut inner = self.inner.borrow_mut();
        inner.adopt(identity);
        // Bounded by `RouteIndex::index`; the compile-time assertion above leaves zero available
        // as the empty tag after adding one.
        let tag = key as u16 + 1;
        let Some(i) = inner.slots.iter().position(|s| s.tag == tag) else {
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
    /// count the miss that prompted it. The identity is deliberately re-adopted here, after the
    /// source read, because a reentrant source can fill the shared cache for another reader while
    /// this reader's miss is in flight.
    fn put(&self, identity: u32, key: usize, pts: &[RoutePoint]) {
        let mut inner = self.inner.borrow_mut();
        inner.adopt(identity);
        inner.misses = inner.misses.saturating_add(1);
        let i = route_lru(inner.slots.iter().map(|s| (s.tag == 0, s.used)));
        let t = inner.touch();
        let s = &mut inner.slots[i];
        // Bounded by `RouteIndex::index`; zero remains reserved for an empty slot.
        s.tag = key as u16 + 1;
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
        // SAFETY: all-zero is a valid `RouteCacheInner` — no references or non-zero-discriminant
        // enums, a zero slot tag means empty, and each `heapless::Vec` is
        // `{ len: 0, uninit buffer }` whose `MaybeUninit<RoutePoint>` backing is not read while
        // `len == 0`.
        unsafe { core::mem::MaybeUninit::zeroed().assume_init() }
    }

    fn adopt(&mut self, identity: u32) {
        if self.identity != identity {
            self.clear();
            self.identity = identity;
        }
    }

    /// Invalidate slots and reset diagnostics without changing the adopted identity. Keeping the
    /// owner means an explicit clear followed by another reader over the same resident index simply
    /// starts cold; a later different identity still runs this path before any lookup.
    fn clear(&mut self) {
        for s in &mut self.slots {
            s.tag = 0;
        }
        self.tick = 0;
        self.hits = 0;
        self.misses = 0;
    }

    #[inline]
    fn touch(&mut self) -> u16 {
        if self.tick == u16::MAX {
            // This path is extremely rare (once per 65,535 cache touches). Compress the live
            // timestamps to their ranks before incrementing, preserving exact LRU order without
            // allocating or letting an old slot become recent across integer wraparound.
            let old = core::array::from_fn::<_, ROUTE_CHUNK_SLOTS, _>(|i| self.slots[i].used);
            let mut live = 0;
            for i in 0..ROUTE_CHUNK_SLOTS {
                if self.slots[i].tag == 0 {
                    continue;
                }
                let rank =
                    1 + old.iter().enumerate().filter(|(j, used)| self.slots[*j].tag != 0 && **used < old[i]).count()
                        as u16;
                self.slots[i].used = rank;
                live += 1;
            }
            self.tick = live;
        }
        self.tick += 1;
        self.tick
    }
}

/// Pick a slot to (re)fill: the first empty slot, else the least-recently-used. Input is
/// `(is_empty, used)` per slot in order. Mirrors `obc_reader`'s `lru`.
fn route_lru(slots: impl Iterator<Item = (bool, u16)>) -> usize {
    let mut best = 0usize;
    let mut best_used = u16::MAX;
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
mod cache_tests {
    use super::*;

    #[test]
    fn lru_clock_rebases_without_changing_eviction_order() {
        let mut inner = RouteCacheInner::new();
        inner.slots[0].tag = 1;
        inner.slots[0].used = 1;
        inner.slots[1].tag = 2;
        inner.slots[1].used = u16::MAX - 1;
        inner.tick = u16::MAX;

        assert_eq!(inner.touch(), 3);
        assert_eq!(inner.slots[0].used, 1);
        assert_eq!(inner.slots[1].used, 2);
        assert_eq!(route_lru(inner.slots[..2].iter().map(|s| (s.tag == 0, s.used))), 0);
    }
}

impl core::ops::Deref for RouteReader<'_> {
    type Target = RouteIndex;
    fn deref(&self) -> &RouteIndex {
        self.idx
    }
}

/// Parsed header fields (shared by [`RouteIndex::read`] and [`RouteSummary::read`]). No `version`
/// field: [`read_header`] accepts exactly one version, so every reader below it is v3 by
/// construction.
pub(crate) struct Header {
    pub(crate) bbox: BBox,
    pub(crate) start_lon: i32,
    pub(crate) start_lat: i32,
    pub(crate) point_count: u32,
    pub(crate) total_distance_m: u32,
    pub(crate) total_ascent_m: u32,
    pub(crate) total_descent_m: u32,
    pub(crate) min_ele_m: i16,
    pub(crate) max_ele_m: i16,
    pub(crate) chunk_count: u32,
    pub(crate) index_offset: u32,
    pub(crate) name: String<NAME_CAP>,
}

pub(crate) fn read_header(src: &dyn ByteSource) -> Result<Header, Error> {
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
/// sits on disk — every field, `ele`/`category_id` included. The ride *geometry* path still skips
/// the section entirely; [`RouteReader::load_waypoints`] distils the named ones into the resident
/// [`Waypoints`] table the waypoint UI reads. Also serves hosts and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waypoint {
    /// Cumulative distance from the route start to this waypoint's position, meters.
    pub dist_along_m: u32,
    /// The waypoint's own coordinate (microdegrees) — may sit off the polyline.
    pub lon: i32,
    pub lat: i32,
    /// Elevation in meters; [`WAYPOINT_ELE_NONE`](obc_formats::obcr::WAYPOINT_ELE_NONE) when the source carried none.
    pub ele: i16,
    /// The stored category byte (§4): `0` = generic, `1..=6` the OBCM §7.4 [`PoiCategory`] wire
    /// ids. Kept **raw** so a rewrite (the detour splice) can carry an unknown value through
    /// byte-for-byte; [`category`](Self::category) is the typed read.
    pub category_id: u8,
    /// Signed lateral offset from the route line in meters, positive = **right** of the direction
    /// of travel, `0` = on-route (§4). Saturating: a waypoint further than `i16` metres off route
    /// clamps rather than wrapping.
    pub lateral_offset_m: i16,
    pub name: String<WAYPOINT_NAME_CAP>,
}

impl Waypoint {
    /// The typed category, or `None` for **generic** — an unmapped source symbol, a hand-placed
    /// waypoint, or a category byte outside `1..=6` (the spec's "render unknown as generic").
    #[inline]
    pub fn category(&self) -> Option<PoiCategory> {
        PoiCategory::from_id(self.category_id)
    }
}

/// Visit each stored waypoint in route order (ascending `dist_along_m`), streaming one fixed
/// [`WAYPOINT_LEN`] record at a time — the low-level cursor over the whole (unfiltered, any-count)
/// section. [`RouteReader::load_waypoints`] layers the resident-table policy (name filter, window,
/// cap) on top of it. Returns the number visited; a route without waypoints yields none.
pub fn for_each_waypoint<F: FnMut(&Waypoint)>(src: &dyn ByteSource, mut f: F) -> Result<u16, Error> {
    // The header read is the version gate: a pre-v3 file is rejected there, so no record decoded
    // here can be an old 40-byte one.
    read_header(src)?;
    let mut ext = [0u8; HEADER_FULL_LEN - HEADER_LEN];
    src.read_at(HEADER_LEN as u32, &mut ext)?;
    let offset = rd_u32(&ext, 0);
    let count = rd_u16(&ext, 4);

    let mut rec = [0u8; WAYPOINT_LEN];
    for k in 0..count {
        src.read_at(offset + k as u32 * WAYPOINT_LEN as u32, &mut rec)?;
        let name_len = (rec[15] as usize).min(WAYPOINT_NAME_CAP);
        let mut name = String::new();
        if let Ok(s) = core::str::from_utf8(&rec[WAYPOINT_NAME_OFF..WAYPOINT_NAME_OFF + name_len]) {
            let _ = name.push_str(s);
        }
        f(&Waypoint {
            dist_along_m: rd_u32(&rec, 0),
            lon: rd_i32(&rec, 4),
            lat: rd_i32(&rec, 8),
            ele: rd_i16(&rec, 12),
            category_id: rec[14],
            lateral_offset_m: rd_i16(&rec, 16),
            name,
        });
    }
    Ok(count)
}

/// One resident waypoint: the compact subset of a stored [`Waypoint`] the ride UI actually needs —
/// its along-route position, its own coordinate, its category, how far off the route it sits, and
/// its (non-empty) name. `ele` is **dropped** on purpose (the UI never shows it; distances come
/// from `dist_along_m`), so the entry stays cheap to hold [`MAX_WAYPOINTS`] resident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WptEntry {
    /// Cumulative distance from the route start to this waypoint, meters — the axis the ride
    /// progress, the progress-bar ticks, and the chip's distance-to-go all share.
    pub dist_along_m: u32,
    /// The waypoint's own coordinate (microdegrees) — where its map diamond is drawn.
    pub lon: i32,
    pub lat: i32,
    /// The waypoint's category, or `None` for **generic** — the diamond a hand-placed waypoint
    /// keeps. Shares the map's [`PoiCategory`] ids, so one icon language covers both sources.
    pub category: Option<PoiCategory>,
    /// Signed lateral offset from the route line, meters — positive = **right** of the direction
    /// of travel, `0` = on-route. The `←`/`→` side hint reads this.
    pub lateral_offset_m: i16,
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
    /// frame. A route whose waypoint section is empty — or whose every waypoint is unnamed or
    /// behind `min_dist_m` — yields an empty table. (Pre-v3 files never reach here: the header
    /// read inside [`for_each_waypoint`] is the version gate and rejects them.)
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
            let entry = WptEntry {
                dist_along_m: w.dist_along_m,
                lon: w.lon,
                lat: w.lat,
                category: w.category(),
                lateral_offset_m: w.lateral_offset_m,
                name: w.name.clone(),
            };
            // Full: keep the first-by-distance ones already pushed and flag the overflow. Keep
            // streaming (don't break) so `truncated` reflects the whole file, not the first extra.
            if wpts.entries.push(entry).is_err() {
                wpts.truncated = true;
            }
        });
        wpts
    }
}
