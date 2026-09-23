//! OBCR route reader: header, chunk index, and on-demand chunk decode.
//!
//! [`RouteReader`] keeps the header and the chunk index in RAM and pulls geometry chunks through
//! the [`ByteSource`] only when asked, so a route of hundreds of km is never resident. It holds a
//! `&dyn ByteSource` rather than a generic, so it threads through the app and render layers
//! without making them generic.

use core::{
    cell::RefCell,
    sync::atomic::{AtomicU32, Ordering},
};

use heapless::{String, Vec};

use obc_formats::bike::BikeType;
use obc_formats::cache::lru_victim;
use obc_formats::io::{rd_i16, rd_i32, rd_u16, rd_u32, ByteSource, DecodeError, Error};
use obc_map_scene::BBox;

use obc_formats::obcr::{validate_header_prefix, POINT_RECORD_LEN};
use obc_formats::obcr::{
    CHUNK_META_LEN, HEADER_FULL_LEN, HEADER_LEN, NAME_CAP, WAYPOINT_LEN, WAYPOINT_NAME_CAP, WAYPOINT_NAME_OFF,
};
use obc_reader::PoiCategory;
/// The device's waypoint cap, for both the converter's emission and the resident [`Waypoints`]
/// table. The format allows up to `u16::MAX`, so [`RouteReader::load_waypoints`] windows and
/// truncates a longer file rather than overflowing.
pub const MAX_WAYPOINTS: usize = 32;
/// Resident chunk-index capacity. It sets both the maximum route length and a large part of the
/// device's stack peak, because a [`RouteIndex`] is `MAX_ROUTE_CHUNKS * 48 B` and several call
/// paths hold one on the stack. A route past the cap fails conversion with [`Error::TooLarge`];
/// the host packer shares the value, so anything that packs, loads.
///
/// 256 is about 65 k points, a ~12.3 KB index. Raising it needs the by-value paths to become
/// resident first (see [`read_into`](RouteIndex::read_into)); 512 measured as a 73.7 KB frame in
/// [`elevation_sparkline`](crate::elevation_sparkline), larger than the whole stack region.
pub const MAX_ROUTE_CHUNKS: usize = 256;
const _: () = assert!(MAX_ROUTE_CHUNKS < u16::MAX as usize);
/// Max points a single chunk may hold (bounds the per-chunk decode buffer).
pub const MAX_POINTS_PER_CHUNK: usize = 256;

/// Position in microdegrees, elevation in meters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutePoint {
    pub lon: i32,
    pub lat: i32,
    pub ele: i16,
    /// Surface class of the incoming segment; 0 means unknown.
    pub surface: u8,
    pub elevation_incomplete: bool,
}

impl RoutePoint {
    pub fn elevation(self) -> Option<i16> {
        (self.ele != obc_formats::obcr::ELEVATION_NONE).then_some(self.ele)
    }
}

/// An interpolated position on the route polyline at a clamped along-route distance. The chunk
/// and segment stay crate-private so [`RouteMatch`](crate::RouteMatch) can move its cursor to the
/// same point without exposing the file layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutePosition {
    pub progress_m: u32,
    pub lon: i32,
    pub lat: i32,
    pub(crate) chunk: usize,
    pub(crate) seg: usize,
}

/// One chunk's index entry: its bbox, the absolute anchor it decodes from, and the cumulative
/// stats at its first point. See `OBCR_Spec.md`.
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

/// The route description for the Route menu. It reads from the header alone, so a catalog scan is
/// one small read per file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSummary {
    pub name: String<NAME_CAP>,
    /// Total distance, km, rounded.
    pub distance_km: u32,
    pub climb_m: u32,
    pub bbox: BBox,
    /// First route point, for centering the camera on load.
    pub start_lon: i32,
    pub start_lat: i32,
}

impl RouteSummary {
    pub fn read(src: &dyn ByteSource) -> Result<RouteSummary, Error> {
        Self::read_with_flags(src).map(|(summary, _)| summary)
    }
    /// The summary and the header's `FLAG_*` bits.
    pub fn read_with_flags(src: &dyn ByteSource) -> Result<(RouteSummary, u8), Error> {
        let h = read_header(src)?;
        Ok((
            RouteSummary {
                name: h.name,
                distance_km: (h.total_distance_m + 500) / 1000,
                climb_m: h.total_ascent_m,
                bbox: h.bbox,
                start_lon: h.start_lon,
                start_lat: h.start_lat,
            },
            h.flags,
        ))
    }
}

/// The stored-route facts a BLE `routeList` entry serves: raw meters, not
/// [`RouteSummary`]'s rounded km, plus the waypoint count. It never reads the chunk index.
#[derive(Debug, Clone)]
pub struct RouteObjectInfo {
    pub name: String<NAME_CAP>,
    pub distance_m: u32,
    pub ascent_m: u32,
    pub descent_m: u32,
    pub attribution_map: Option<obc_formats::obcr::RouteSourceKey>,
    pub visit: Option<obc_formats::obcr::VisitDescriptor>,
    pub unresolved_avoidance: bool,
    pub assistant_candidate: bool,
    pub point_count: u32,
    pub waypoint_count: u16,
}

impl RouteObjectInfo {
    /// Read the header and its extension. The validation is the same as any header read, which
    /// is what keeps a non-OBCR payload out of the catalog on the upload commit path.
    pub fn read(src: &dyn ByteSource) -> Result<RouteObjectInfo, Error> {
        let h = read_header(src)?;
        let waypoint_count = {
            let mut ext = [0u8; HEADER_FULL_LEN - HEADER_LEN];
            src.read_at(HEADER_LEN as u64, &mut ext).map_err(|_| Error::BadOffset)?;
            rd_u16(&ext, 4)
        };
        let mut tail = [0; 48];
        src.read_at(112, &mut tail)?;
        let attribution_map = if h.flags & obc_formats::obcr::FLAG_ATTRIBUTION_MAP != 0 {
            Some(obc_formats::obcr::RouteSourceKey::decode(&tail[16..48]).map_err(|_| Error::BadOffset)?)
        } else {
            None
        };
        let visit = if tail[6] != 0 {
            let mut bytes = [0; obc_formats::obcr::VISIT_DESCRIPTOR_LEN];
            src.read_at(rd_u32(&tail, 8) as u64, &mut bytes)?;
            let descriptor = obc_formats::obcr::VisitDescriptor::decode(&bytes).map_err(|_| Error::BadOffset)?;
            if descriptor.accepted_anchors_m[2] > h.total_distance_m {
                return Err(Error::BadOffset);
            }
            Some(descriptor)
        } else {
            None
        };
        Ok(RouteObjectInfo {
            name: h.name,
            distance_m: h.total_distance_m,
            ascent_m: h.total_ascent_m,
            descent_m: h.total_descent_m,
            attribution_map,
            visit,
            unresolved_avoidance: h.flags & obc_formats::obcr::FLAG_UNRESOLVED_AVOIDANCE != 0,
            assistant_candidate: h.flags & obc_formats::obcr::FLAG_ASSISTANT_CANDIDATE != 0,
            point_count: h.point_count,
            waypoint_count,
        })
    }
}

/// The last route point, `(lon, lat)` µdeg. It reads the header, the last chunk meta and that
/// chunk's point records in small blocks, never the whole index or a decoded chunk.
pub fn route_end(src: &dyn ByteSource) -> Result<(i32, i32), Error> {
    let h = read_header(src)?;
    let Some(last) = h.chunk_count.checked_sub(1) else { return Ok((h.start_lon, h.start_lat)) };
    let off = last
        .checked_mul(CHUNK_META_LEN as u32)
        .and_then(|rel| h.index_offset.checked_add(rel))
        .ok_or(Error::BadOffset)?;
    let mut meta = [0u8; CHUNK_META_LEN];
    src.read_at(off.into(), &mut meta)?;
    let cm = parse_chunk_meta(&meta, src.len())?;
    let (mut lon, mut lat) = (cm.anchor_lon, cm.anchor_lat);
    const BLOCK: usize = 16 * POINT_RECORD_LEN;
    let mut block = [0u8; BLOCK];
    let (mut at, end) = (u64::from(cm.byte_offset), u64::from(cm.byte_offset) + u64::from(cm.byte_len));
    while at < end {
        let bytes = &mut block[..(end - at).min(BLOCK as u64) as usize];
        src.read_at(at, bytes)?;
        for p in bytes.chunks_exact(POINT_RECORD_LEN) {
            lon = lon.wrapping_add(rd_i16(p, 0).into());
            lat = lat.wrapping_add(rd_i16(p, 2).into());
        }
        at += bytes.len() as u64;
    }
    Ok((lon, lat))
}

/// The resident, source-independent parse of a route: the header fields plus the chunk index and
/// its segment prefix sums. [`read`](Self::read) pays the route's only up-front cost, the header
/// read and the full chunk-meta walk.
///
/// Build it once when the active route changes and reuse it across frames, so a redraw pays only
/// the geometry reads.
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
    /// Segments before chunk `c`, the shared seam point not counted twice. The total comes from
    /// the last prefix plus the last chunk, so there is no trailing word. Built once, which keeps
    /// [`global_seg_index`](Self::global_seg_index) O(1) on the matcher's hot path.
    cum_seg: Vec<u32, MAX_ROUTE_CHUNKS>,
    /// Identity of this parse, not persisted. A move preserves it, so a by-value host index and
    /// the board's resident slot adopt caches the same way. Zero means empty or a failed parse.
    identity: u32,
    flags: u8,
    bike: BikeType,
}

/// A [`RouteIndex`] paired with a borrow of the byte source its geometry chunks stream from.
/// Cheap to build: the expensive parse lives in [`RouteIndex::read`]. It derefs to its index, so
/// only [`decode_chunk`](Self::decode_chunk) needs the source.
pub struct RouteReader<'a> {
    src: &'a dyn ByteSource,
    idx: &'a RouteIndex,
    /// When present, [`decode_chunk`](Self::decode_chunk) serves an unchanged route from RAM
    /// instead of re-reading its geometry. `None` streams every call.
    cache: Option<&'a RouteCache>,
}

impl RouteIndex {
    /// An empty index, which is what [`read_into`](Self::read_into) fills. It is queryable but
    /// matches nothing, and a failed `read_into` leaves the slot in this state, so a caller that
    /// needs "is there a route?" keeps its own validity flag.
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
            flags: 0,
            bike: BikeType::Road,
        }
    }

    /// Parse the header and chunk index from `src`, validating that every chunk lies inside the
    /// source and inside the resident buffers.
    ///
    /// It returns the ~12.3 KB index by value, which transits the stack. A board caller must use
    /// [`read_into`](Self::read_into) on its resident slot instead.
    ///
    /// Must stay `#[inline(never)]`: inlined, the index-building temporaries share one frame with
    /// the returned value. Out of line they pop before the caller continues.
    #[inline(never)]
    pub fn read(src: &dyn ByteSource) -> Result<RouteIndex, Error> {
        let mut idx = RouteIndex::empty();
        idx.read_into(src)?;
        Ok(idx)
    }

    /// The in-place twin of [`read`](Self::read): fill the caller's resident slot field by field,
    /// so the index is never a stack temporary. On an error `self` is left empty, never half
    /// filled.
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
        self.flags = h.flags;
        self.bike = h.bike;
        if h.chunk_count as usize > MAX_ROUTE_CHUNKS {
            return Err(Error::TooLarge);
        }

        let mut seg_acc: u32 = 0;
        let mut meta = [0u8; CHUNK_META_LEN];
        for k in 0..h.chunk_count {
            // `index_offset` is untrusted header input, so a forged offset near `u32::MAX` must
            // surface as a bad offset and never wrap back into the file.
            let off = k
                .checked_mul(CHUNK_META_LEN as u32)
                .and_then(|rel| h.index_offset.checked_add(rel))
                .ok_or(Error::BadOffset)?;
            src.read_at(off.into(), &mut meta)?;
            let cm = parse_chunk_meta(&meta, src.len())?;
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

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The chunk index, in route order.
    pub fn chunks(&self) -> &[ChunkMeta] {
        &self.index
    }

    /// Index from the route start of segment `seg` in chunk `c`. `c` past the last chunk clamps
    /// to the total.
    pub(crate) fn global_seg_index(&self, c: usize, seg: usize) -> usize {
        let c = c.min(self.index.len());
        self.cum_seg.get(c).copied().unwrap_or_else(|| self.segment_count()) as usize + seg
    }

    /// Total segments, seams not counted twice. The last prefix excludes the last chunk, so that
    /// chunk's own count is added.
    #[inline]
    fn segment_count(&self) -> u32 {
        match (self.cum_seg.last(), self.index.last()) {
            (Some(before_last), Some(last)) => before_last + (last.point_count as u32).saturating_sub(1),
            _ => 0,
        }
    }

    // Cumulative ascent at a position comes from `Profile::ascent_to` at column resolution. The
    // per-chunk `cum_ascent_m` has too few chunks to place "to climb" accurately.

    /// At least one retained point has a valid elevation. Flat sea-level routes remain valid.
    pub fn has_elevation(&self) -> bool {
        self.flags & obc_formats::obcr::FLAG_HAS_ELEVATION != 0
    }

    pub fn bike_type(&self) -> BikeType {
        self.bike
    }

    pub fn is_assistant_candidate(&self) -> bool {
        self.flags & obc_formats::obcr::FLAG_ASSISTANT_CANDIDATE != 0
    }

    pub fn has_unresolved_avoidance(&self) -> bool {
        self.flags & obc_formats::obcr::FLAG_UNRESOLVED_AVOIDANCE != 0
    }

    pub fn identity(&self) -> u32 {
        self.identity
    }

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

    /// Visit each chunk whose bbox intersects `view`, in route order. The caller decodes the ones
    /// it wants with [`RouteReader::decode_chunk`] into its own reused buffer.
    pub fn for_each_visible_chunk<F: FnMut(usize, &ChunkMeta)>(&self, view: &BBox, mut f: F) {
        for (k, cm) in self.index.iter().enumerate() {
            if cm.bbox.intersects(view) {
                f(k, cm);
            }
        }
    }
}

impl<'a> RouteReader<'a> {
    /// Pair a parsed [`RouteIndex`] with the byte source its chunks stream from. No I/O. Build
    /// the index once per route and call this per frame.
    pub fn new(idx: &'a RouteIndex, src: &'a dyn ByteSource) -> RouteReader<'a> {
        RouteReader { src, idx, cache: None }
    }

    /// The byte source, for this crate's whole-file passes over sections the chunk index does not
    /// cover.
    /// Start a bounded cursor over the complete authored waypoint section.
    pub fn waypoint_cursor(&self) -> Result<WaypointCursor, Error> {
        WaypointCursor::new(self.source())
    }

    pub fn next_waypoint(&self, cursor: &mut WaypointCursor) -> Result<Option<Waypoint>, Error> {
        cursor.next(self.source())
    }

    pub(crate) fn source(&self) -> &dyn ByteSource {
        self.src
    }

    /// Like [`new`](Self::new), but backs [`decode_chunk`](Self::decode_chunk) with a resident
    /// [`RouteCache`]. The cache adopts the index's parse identity here, which invalidates
    /// same-key slots from a different route, so a caller never has to clear it on a switch.
    pub fn new_cached(idx: &'a RouteIndex, src: &'a dyn ByteSource, cache: &'a RouteCache) -> RouteReader<'a> {
        cache.adopt(idx.identity);
        RouteReader { src, idx, cache: Some(cache) }
    }

    /// Decode chunk `k` into `out`, which is cleared first: its anchor, then each delta-stepped
    /// point. The chunk's last point is chunk `k+1`'s anchor, so adjacent chunks stitch without a
    /// gap. With a [`RouteCache`] attached, a chunk decoded earlier is served from RAM.
    pub fn decode_chunk(&self, k: usize, out: &mut Vec<RoutePoint, MAX_POINTS_PER_CHUNK>) -> Result<(), Error> {
        out.clear();
        let m = self.idx.index.get(k).ok_or(Error::BadOffset)?;
        let n = m.point_count as usize;
        if n == 0 {
            return Ok(());
        }
        // The reader identity is revalidated for both the hit and the miss: another reader can
        // use the same cache between calls, including reentrantly from `ByteSource::read_at`
        // while this miss is being decoded.
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

    pub fn attribution_map(&self) -> Result<Option<obc_formats::obcr::RouteSourceKey>, Error> {
        if self.flags & obc_formats::obcr::FLAG_ATTRIBUTION_MAP == 0 {
            return Ok(None);
        }
        let mut bytes = [0; 32];
        self.src.read_at(128, &mut bytes)?;
        obc_formats::obcr::RouteSourceKey::decode(&bytes).map(Some).map_err(|_| Error::BadOffset)
    }

    pub fn visit_descriptor(&self) -> Result<Option<obc_formats::obcr::VisitDescriptor>, Error> {
        if self.flags & 128 == 0 {
            return Ok(None);
        }
        let mut ext = [0; 16];
        self.src.read_at(112, &mut ext)?;
        if ext[6] == 0 {
            return Ok(None);
        }
        let mut bytes = [0; obc_formats::obcr::VISIT_DESCRIPTOR_LEN];
        self.src.read_at(rd_u32(&ext, 8) as u64, &mut bytes)?;
        obc_formats::obcr::VisitDescriptor::decode(&bytes).map(Some).map_err(|_| Error::BadOffset)
    }

    /// Locate `progress_m` on the route, clamped to the route end and interpolated inside the
    /// containing segment. It uses caller-owned decode scratch, so the matcher adds no
    /// stack-sized route copy.
    pub(crate) fn locate_progress(
        &self,
        progress_m: u32,
        buf: &mut Vec<RoutePoint, MAX_POINTS_PER_CHUNK>,
    ) -> Option<RoutePosition> {
        let target = progress_m.min(self.total_distance_m);
        let (p, chunk, seg) = self.locate_interpolated(target, buf)?;
        Some(RoutePosition { progress_m: target, lon: p.lon, lat: p.lat, chunk, seg })
    }

    /// The shared walk behind [`locate_progress`](Self::locate_progress) and
    /// [`elevation_at`](Self::elevation_at): the interpolated point at `target`, which the caller
    /// has already clamped, plus its chunk and segment.
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

        let mut s = cm.cum_distance_m as f64;
        for i in 0..buf.len() - 1 {
            let a = buf[i];
            let b = buf[i + 1];
            let dl = obc_map_scene::ground_dist_m((a.lon, a.lat), (b.lon, b.lat)) as f64;
            let last = i + 2 == buf.len();
            if target as f64 <= s + dl || last {
                let t = if dl > 1e-3 { ((target as f64 - s) / dl).clamp(0.0, 1.0) } else { 0.0 };
                return Some((interpolate_point(a, b, t as f32), k, i));
            }
            s += dl;
        }
        None
    }

    /// The interpolated elevation at `progress_m`, clamped to the route end: the splice path's
    /// seam-endpoint sampler. A cold path with its own decode scratch. It is public so the
    /// splice's seam contract can be asserted against the same lookup the splice uses.
    #[inline(never)]
    pub fn elevation_at(&self, progress_m: u32) -> Option<i16> {
        let mut buf = Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
        let target = progress_m.min(self.total_distance_m);
        self.locate_interpolated(target, &mut buf)?.0.elevation()
    }

    /// The coordinate at `progress_m`, clamped to the route end. The UI-facing wrapper around
    /// [`locate_progress`](Self::locate_progress); the matcher supplies its own scratch instead.
    #[inline(never)]
    pub fn position_at(&self, progress_m: u32) -> Option<RoutePosition> {
        let mut buf = Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
        self.locate_progress(progress_m, &mut buf)
    }

    /// Stream only the polyline stretch in the inclusive interval `[start_m, end_m]`. Each
    /// callback slice is one clipped chunk, its first and last coordinates interpolated at the
    /// boundary. A chunk that fails to decode is skipped.
    #[inline(never)]
    pub fn visit_points_between(&self, start_m: u32, end_m: u32, mut visit: impl FnMut(&[(i32, i32)])) {
        let lo = start_m.min(self.total_distance_m);
        let hi = end_m.min(self.total_distance_m);
        if lo >= hi {
            return;
        }
        let chunks = self.chunks();
        // Only coordinate scratch stays live across `visit`. The deeper decode frame is
        // `#[inline(never)]` and has returned before a renderer's stroke and fill stack starts.
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

    /// Assistant Visit shape from departure through rejoin; other reviews show the full route.
    /// The stored continuation stays intact and ordinary route overviews still use `preview_polyline`.
    pub fn assistant_preview_polyline<const N: usize>(&self) -> Result<Vec<(i32, i32), N>, Error> {
        let Some(visit) = self.visit_descriptor()? else {
            let shape = self.preview_polyline::<N>();
            let count = if self.chunks().is_empty() { 0 } else { self.idx.segment_count() as usize + 1 };
            return if shape.len() == count.min(N) { Ok(shape) } else { Err(Error::BadOffset) };
        };
        let [lo, stop, hi] = visit.accepted_anchors_m;
        let mut shape = Vec::new();
        if N >= 3 {
            self.append_preview_span(lo, stop, N / 2 + 1, &mut shape)?;
            self.append_preview_span(stop, hi, N, &mut shape)?;
        } else if N > 0 {
            self.append_preview_span(lo, hi, N, &mut shape)?;
        }
        Ok(shape)
    }

    fn append_preview_span<const N: usize>(
        &self,
        lo: u32,
        hi: u32,
        limit: usize,
        shape: &mut Vec<(i32, i32), N>,
    ) -> Result<(), Error> {
        let mut count = 0;
        self.preview_span(lo, hi, |_| count += 1)?;
        let continues = !shape.is_empty();
        let keep = limit.min(N - shape.len() + usize::from(continues)).min(count);
        let mut ordinal = 0;
        let mut selected = 0;
        self.preview_span(lo, hi, |point| {
            let next = if keep > 1 { selected * (count - 1) / (keep - 1) } else { 0 };
            if selected < keep && ordinal == next {
                // Both spans share the stop occurrence. Keep the first representation even when a
                // chunk boundary quantizes the second span's start to another coordinate.
                if !(continues && selected == 0) && shape.last() != Some(&point) {
                    let _ = shape.push(point);
                }
                selected += 1;
            }
            ordinal += 1;
        })
    }

    #[inline(never)]
    fn preview_span(&self, lo: u32, hi: u32, mut visit: impl FnMut((i32, i32))) -> Result<(), Error> {
        let mut points = Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
        let mut previous = None;
        for (k, chunk) in self.chunks().iter().enumerate() {
            if chunk.cum_distance_m > hi {
                break;
            }
            if self.chunks().get(k + 1).is_some_and(|next| next.cum_distance_m < lo) {
                continue;
            }
            let upper = if hi == self.total_distance_m { u32::MAX } else { hi };
            let found = if chunk.point_count == 1 {
                self.decode_chunk(k, &mut points)?;
                Some(points.len())
            } else {
                decode_route_points_between_checked(self, k, lo, upper, &mut points)?
            };
            if found.is_some() {
                for point in &points {
                    let point = (point.lon, point.lat);
                    if previous != Some(point) {
                        visit(point);
                        previous = Some(point);
                    }
                }
            }
        }
        Ok(())
    }

    /// The route's polyline decimated to at most `N` points, uniform by point index with the
    /// first and last always kept: the overview's shape preview.
    ///
    /// It streams every chunk once in route order, skipping each seam's shared point, so the walk
    /// is over distinct points. Call it once per plan, never per frame. A chunk that fails to
    /// decode is skipped; the preview is a sketch, not navigation data.
    pub fn preview_polyline<const N: usize>(&self) -> Vec<(i32, i32), N> {
        let mut out: Vec<(i32, i32), N> = Vec::new();
        // Distinct points are the total segments plus one.
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
            // Every chunk after the first re-decodes the previous chunk's last point.
            let skip = usize::from(k > 0);
            for p in buf.iter().skip(skip) {
                if gi == next {
                    let _ = out.push((p.lon, p.lat));
                    kept += 1;
                    if kept == keep {
                        return out;
                    }
                    // The j-th kept point sits at `j * (total-1) / (keep-1)`: the endpoints are
                    // exact and the rest are an even stride.
                    next = kept * (total - 1) / (keep - 1);
                }
                gi += 1;
            }
        }
        out
    }
}

/// The route-corridor POI query's geometry seam. `obc-reader` sits below this crate, so it cannot
/// name a [`RouteReader`]: it declares [`RoutePath`](obc_reader::RoutePath) and the OBCR side
/// implements it.
///
/// Everything but [`visit_chunk_points`](obc_reader::RoutePath::visit_chunk_points) reads the
/// resident chunk index and does no I/O.
impl obc_reader::RoutePath for RouteReader<'_> {
    #[inline]
    fn chunk_count(&self) -> usize {
        self.chunks().len()
    }

    #[inline]
    fn chunk_start_m(&self, k: usize) -> u32 {
        // Past the last chunk the answer is the route end, which the corridor query's
        // chunk-extent arithmetic relies on.
        self.chunks().get(k).map_or(self.total_distance_m, |cm| cm.cum_distance_m)
    }

    #[inline]
    fn chunk_bbox(&self, k: usize) -> BBox {
        self.chunks().get(k).map(|cm| cm.bbox).unwrap_or(BBox { min_lon: 0, min_lat: 0, max_lon: 0, max_lat: 0 })
    }

    fn visit_chunk_points(&self, k: usize, visit: &mut dyn FnMut(&[(i32, i32)])) {
        // Only coordinate scratch stays live across `visit`. The deeper decode frame is
        // `#[inline(never)]` and has returned before the query descends into the POI walk.
        let mut lonlat = [(0i32, 0i32); MAX_POINTS_PER_CHUNK];
        if let Some(n) = decode_chunk_lonlat(self, k, &mut lonlat) {
            visit(&lonlat[..n]);
        }
    }
}

/// Decode chunk `k` into caller-owned `(lon, lat)` scratch, returning the point count. Must stay
/// `#[inline(never)]`: its `Vec<RoutePoint, 256>` frame must be gone before the caller's callback
/// runs.
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
        lon: a.lon + libm::roundf((b.lon - a.lon) as f32 * t) as i32,
        lat: a.lat + libm::roundf((b.lat - a.lat) as f32 * t) as i32,
        ele: if t <= 0.0 {
            a.ele
        } else if t >= 1.0 {
            b.ele
        } else if a.elevation().is_none() || b.elevation().is_none() || b.elevation_incomplete {
            i16::MIN
        } else {
            libm::roundf(a.ele as f32 + (i32::from(b.ele) - i32::from(a.ele)) as f32 * t) as i16
        },
        surface: b.surface,
        elevation_incomplete: b.elevation_incomplete,
    }
}

/// Decode and clip one route chunk into caller-owned `(lon, lat)` scratch. Must stay
/// `#[inline(never)]`: its `Vec<RoutePoint, 256>` frame must be gone before the callback enters
/// the renderer's stroke and fill stack.
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

/// Decode chunk `k` and clip it in place to the inclusive interval `[lo, hi]`, keeping the full
/// [`RoutePoint`] records with the boundary points interpolated. [`decode_points_between`] layers
/// the `(lon, lat)` view on top, so there is one clipping implementation. `None` when the chunk
/// misses the interval or fails to decode.
#[inline(never)]
pub(crate) fn decode_route_points_between(
    route: &RouteReader,
    k: usize,
    lo: u32,
    hi: u32,
    buf: &mut Vec<RoutePoint, MAX_POINTS_PER_CHUNK>,
) -> Option<usize> {
    decode_route_points_between_checked(route, k, lo, hi, buf).ok().flatten()
}

/// The transform path must distinguish a missing span from unreadable geometry.
#[inline(never)]
pub(crate) fn decode_route_points_between_checked(
    route: &RouteReader,
    k: usize,
    lo: u32,
    hi: u32,
    buf: &mut Vec<RoutePoint, MAX_POINTS_PER_CHUNK>,
) -> Result<Option<usize>, Error> {
    let Some(cm) = route.chunks().get(k) else { return Ok(None) };
    route.decode_chunk(k, buf)?;
    if buf.len() < 2 {
        return Ok(None);
    }
    let mut s = cm.cum_distance_m as f64;
    let mut first: Option<(usize, RoutePoint)> = None;
    let mut last: Option<(usize, RoutePoint)> = None;
    for i in 0..buf.len() - 1 {
        let a = buf[i];
        let b = buf[i + 1];
        let dl = obc_map_scene::ground_dist_m((a.lon, a.lat), (b.lon, b.lat)) as f64;
        let seg_hi = s + dl;
        if seg_hi >= lo as f64 && s <= hi as f64 {
            let t0 = if dl > 1e-3 { ((lo as f64 - s) / dl).clamp(0.0, 1.0) } else { 0.0 };
            let t1 = if dl > 1e-3 { ((hi as f64 - s) / dl).clamp(0.0, 1.0) } else { 1.0 };
            first.get_or_insert((i, interpolate_point(a, b, t0 as f32)));
            last = Some((i + 1, interpolate_point(a, b, t1 as f32)));
        }
        s = seg_hi;
        if s > hi as f64 {
            break;
        }
    }
    let (Some((a, pa)), Some((b, pb))) = (first, last) else { return Ok(None) };
    let n = b - a + 1;
    // Shift the kept stretch to the front in place: no second point buffer on the stack.
    for i in 0..n {
        buf[i] = buf[a + i];
    }
    buf.truncate(n);
    buf[0] = pa;
    buf[n - 1] = pb;
    Ok(Some(n))
}

/// Decode one chunk-meta record, validating its point count and that its data region lies inside
/// `src_len`. It is separate from [`RouteIndex::fill_from`] so a streaming consumer, one that
/// never materialises the whole index, parses metas through the same code.
pub(crate) fn parse_chunk_meta(meta: &[u8; CHUNK_META_LEN], src_len: u64) -> Result<ChunkMeta, Error> {
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
    // The data region is bounds-checked once here, so the decode does no per-point check. OBCR
    // offsets are `uint32`, so the sum is widened for the comparison.
    let end = u64::from(cm.byte_offset) + u64::from(cm.byte_len);
    if end > src_len {
        return Err(Error::BadOffset);
    }
    // The region is cross-checked against the point count. The decode sizes its read from
    // `point_count` alone, so a forged meta whose `byte_len` disagrees would hand the decoder the
    // next chunk's bytes as this chunk's geometry.
    if cm.byte_len != (point_count as u32).saturating_sub(1) * POINT_RECORD_LEN as u32 {
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
    let _ = out.push(RoutePoint {
        lon: m.anchor_lon,
        lat: m.anchor_lat,
        ele: m.anchor_ele,
        surface: 0,
        elevation_incomplete: false,
    });

    // The points after the anchor are fixed 7-byte records, read in one go.
    let want = (n - 1) * POINT_RECORD_LEN;
    let mut buf = [0u8; (MAX_POINTS_PER_CHUNK - 1) * POINT_RECORD_LEN];
    let bytes = buf.get_mut(..want).ok_or(Error::TooLarge)?;
    if want > 0 {
        src.read_at(m.byte_offset.into(), bytes)?;
    }

    let (mut lon, mut lat) = (m.anchor_lon, m.anchor_lat);
    let mut o = 0;
    for _ in 1..n {
        lon += rd_i16(bytes, o) as i32;
        lat += rd_i16(bytes, o + 2) as i32;
        let ele = rd_i16(bytes, o + 4);
        if bytes[o + 6] & !15 != 0 {
            return Err(Error::BadOffset);
        }
        o += POINT_RECORD_LEN;
        let _ = out.push(RoutePoint {
            lon,
            lat,
            ele,
            surface: bytes[o - 1] & 7,
            elevation_incomplete: bytes[o - 1] & 8 != 0,
        });
    }
    Ok(())
}

/// Allocate a non-zero, process-local parse identity. It is independent of the OBCR bytes, so
/// parsing the same bytes again gets a new identity while a move of a parsed [`RouteIndex`] keeps
/// its identity and its cache hits. Zero keeps the all-zero initialization contract. The counter
/// never wraps: once the token space is exhausted, a parse fails rather than reuse an identity a
/// live cache could still own.
fn next_route_identity() -> Result<u32, Error> {
    // Zero-init keeps the allocator in `.bss`, and the returned token is the stored next value,
    // so zero itself is never live.
    static LAST: AtomicU32 = AtomicU32::new(0);
    LAST.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |identity| identity.checked_add(1))
        .map(|identity| identity + 1)
        .map_err(|_| Error::TooLarge)
}

/// Resident decoded-chunk slots. Only the chunks crossing the view are decoded, so a small LRU
/// holds a frame's working set: the matcher's chunk, the riding-zoom view, and one spare for a
/// zoomed-out pan. A very wide view of a winding route still re-decodes.
const ROUTE_CHUNK_SLOTS: usize = 3;

/// A decoded chunk's points, keyed by chunk index, with LRU recency. The owning route identity
/// lives once on [`RouteCacheInner`]. The key is stored as `index + 1`, so zero is the empty tag
/// and the cache is safe to create from all-zero memory.
struct RouteSlot {
    tag: u16,
    // LRU order, not a diagnostic counter. It is rebased before it can overflow, which keeps the
    // slot header at four bytes.
    used: u16,
    pts: Vec<RoutePoint, MAX_POINTS_PER_CHUNK>,
}

/// A small resident cache of decoded route-geometry chunks, the route analogue of
/// `obc_reader::MapCache`. Without it, a redraw and the matcher's per-fix decode re-pull the same
/// visible chunks from the card every time.
///
/// Caller-owned and reused across frames, paired with the per-frame [`RouteReader`] via
/// [`new_cached`](RouteReader::new_cached). Slots are keyed by chunk index and the cache as a whole
/// adopts the parsed index's identity, so a different route invalidates every same-key slot.
///
/// The state is in a `RefCell` so a `&RouteCache` can fill it; the borrow is scoped to one
/// get or put.
pub struct RouteCache {
    inner: RefCell<RouteCacheInner>,
}

struct RouteCacheInner {
    /// The [`RouteIndex`] parse whose chunks occupy the slots. Zero is the unowned initial state
    /// and is never a parsed index.
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
    /// A fresh, empty cache. On the device, place it once in the reserved region so it stays off
    /// the main stack.
    pub fn new() -> Self {
        RouteCache { inner: RefCell::new(RouteCacheInner::new()) }
    }

    /// Drop every resident slot and zero the counters. A route switch already invalidates through
    /// [`RouteReader::new_cached`]. Only the slot tags and counters are touched.
    pub fn clear(&self) {
        self.inner.borrow_mut().clear();
    }

    /// `(hits, misses)` since the last [`clear`](Self::clear), for the tests.
    pub fn stats(&self) -> (u32, u32) {
        let inner = self.inner.borrow();
        (inner.hits, inner.misses)
    }

    /// Bind the cache to one parsed route. A different identity clears every same-index slot
    /// before the reader can decode; a move of the same index preserves its hits. Identity zero is
    /// accepted for the empty index, which owns no decodable chunks.
    fn adopt(&self, identity: u32) {
        self.inner.borrow_mut().adopt(identity);
    }

    /// If chunk `key` is resident, copy its points into `out` and return `true`. Identity
    /// adoption and lookup share one borrow, so an interleaved reader cannot cross-serve a slot.
    fn get(&self, identity: u32, key: usize, out: &mut Vec<RoutePoint, MAX_POINTS_PER_CHUNK>) -> bool {
        let mut inner = self.inner.borrow_mut();
        inner.adopt(identity);
        // Bounded by `RouteIndex::index`, and the assertion above leaves zero free as the empty
        // tag after adding one.
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

    /// Store chunk `key`'s decoded points, evicting the least recently used slot. The identity is
    /// re-adopted here, after the source read, because a reentrant source can fill the shared
    /// cache for another reader while this miss is in flight.
    fn put(&self, identity: u32, key: usize, pts: &[RoutePoint]) {
        let mut inner = self.inner.borrow_mut();
        inner.adopt(identity);
        inner.misses = inner.misses.saturating_add(1);
        let i = lru_victim(inner.slots.iter().map(|s| (s.tag == 0, s.used)));
        let t = inner.touch();
        let s = &mut inner.slots[i];
        // Bounded by `RouteIndex::index`; zero stays reserved for an empty slot.
        s.tag = key as u16 + 1;
        s.used = t;
        s.pts.clear();
        let _ = s.pts.extend_from_slice(pts);
    }
}

impl RouteCacheInner {
    /// A `const` struct literal, not a zeroed `assume_init`: `heapless::Vec::new()` is `const`,
    /// so the whole value is a constant and the buffers are never put in `.rodata` to be copied
    /// from. The `.rodata` plus `memcpy` lowering bricks the boot.
    const fn new() -> Self {
        RouteCacheInner {
            identity: 0,
            tick: 0,
            slots: [const { RouteSlot { tag: 0, used: 0, pts: Vec::new() } }; ROUTE_CHUNK_SLOTS],
            hits: 0,
            misses: 0,
        }
    }

    fn adopt(&mut self, identity: u32) {
        if self.identity != identity {
            self.clear();
            self.identity = identity;
        }
    }

    /// Invalidate the slots and reset the counters without changing the adopted identity, so a
    /// reader over the same index simply starts cold.
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
            // Once per 65 535 touches, compress the live timestamps to their ranks. This keeps
            // the exact LRU order and stops an old slot becoming recent across a wraparound.
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
        assert_eq!(lru_victim(inner.slots[..2].iter().map(|s| (s.tag == 0, s.used))), 0);
    }
}

impl core::ops::Deref for RouteReader<'_> {
    type Target = RouteIndex;
    fn deref(&self) -> &RouteIndex {
        self.idx
    }
}

/// Parsed header fields. There is no `version` field: [`read_header`] accepts exactly one
/// version, so every reader below it has that version by construction.
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
    pub(crate) flags: u8,
    pub(crate) bike: BikeType,
}

pub(crate) fn read_header(src: &dyn ByteSource) -> Result<Header, Error> {
    let mut h = [0u8; HEADER_FULL_LEN];
    src.read_at(0, &mut h).map_err(|_| Error::BadOffset)?;
    // `obc-formats` owns the magic and version gate: one prefix check for every OBCR consumer. It
    // reports `Version` only after the magic matched, which keeps the "not an OBCR" and "an OBCR
    // we cannot read" answers apart.
    match validate_header_prefix(&h) {
        Ok(_) => {}
        Err(DecodeError::Version) => return Err(Error::BadVersion),
        Err(_) => return Err(Error::BadMagic),
    }
    let Some(bike) = BikeType::from_u8(h[obc_formats::obcr::BIKE_TYPE_OFF]) else {
        return Err(Error::BadOffset);
    };
    if h[5] & !31 != 0 || h[119] != 0 {
        return Err(Error::BadOffset);
    }
    if h[5] & obc_formats::obcr::FLAG_ATTRIBUTION_MAP == 0 && h[128..160].iter().any(|b| *b != 0) {
        return Err(Error::BadOffset);
    }
    if h[5] & obc_formats::obcr::FLAG_ATTRIBUTION_MAP != 0 {
        obc_formats::obcr::RouteSourceKey::decode(&h[128..160]).map_err(|_| Error::BadOffset)?;
    }
    let descriptor_offset = rd_u32(&h, 120);
    let descriptor_len = rd_u32(&h, 124);
    if h[118] == 0 {
        if descriptor_offset != 0 || descriptor_len != 0 {
            return Err(Error::BadOffset);
        }
    } else if h[118] != obc_formats::obcr::VISIT_DESCRIPTOR_VERSION {
        return Err(Error::BadVersion);
    } else if descriptor_len != obc_formats::obcr::VISIT_DESCRIPTOR_LEN as u32
        || descriptor_offset < HEADER_FULL_LEN as u32
        || u64::from(descriptor_offset) + u64::from(descriptor_len) > src.len()
    {
        return Err(Error::BadOffset);
    }
    if h[118] != 0 {
        let mut bytes = [0; obc_formats::obcr::VISIT_DESCRIPTOR_LEN];
        src.read_at(u64::from(descriptor_offset), &mut bytes)?;
        let descriptor = obc_formats::obcr::VisitDescriptor::decode(&bytes).map_err(|_| Error::BadOffset)?;
        if descriptor.accepted_anchors_m[2] > rd_u32(&h, 36) {
            return Err(Error::BadOffset);
        }
        let overlap = |offset: u32, length: u64| {
            u64::from(descriptor_offset) < u64::from(offset) + length
                && u64::from(offset) < u64::from(descriptor_offset) + u64::from(descriptor_len)
        };
        if overlap(rd_u32(&h, 56), u64::from(rd_u32(&h, 52)) * CHUNK_META_LEN as u64)
            || overlap(rd_u32(&h, 112), u64::from(rd_u16(&h, 116)) * WAYPOINT_LEN as u64)
        {
            return Err(Error::BadOffset);
        }
    }
    let name_len = (h[6] as usize).min(NAME_CAP);
    let mut name = String::new();
    if let Ok(s) = core::str::from_utf8(&h[64..64 + name_len]) {
        let _ = name.push_str(s);
    }
    Ok(Header {
        flags: h[5] | if h[118] != 0 { 128 } else { 0 },
        bike,
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

/// Stream every stored point of a complete route once, in order, without a route index. Each chunk
/// must repeat the previous chunk's last point, and the header's point count must match.
///
/// Must stay `#[inline(never)]`: the bounded chunk buffer lives in this popped frame.
#[inline(never)]
pub(crate) fn for_each_stored_point(
    src: &dyn ByteSource,
    h: &Header,
    mut visit: impl FnMut(RoutePoint),
) -> Result<(), Error> {
    if h.chunk_count == 0 || h.chunk_count as usize > crate::MAX_ROUTE_CHUNKS {
        return Err(Error::BadOffset);
    }
    let mut points = Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
    let mut previous = None;
    let mut count = 0u32;
    for k in 0..h.chunk_count {
        let offset =
            k.checked_mul(CHUNK_META_LEN as u32).and_then(|n| h.index_offset.checked_add(n)).ok_or(Error::BadOffset)?;
        let mut bytes = [0; CHUNK_META_LEN];
        src.read_at(u64::from(offset), &mut bytes)?;
        let meta = parse_chunk_meta(&bytes, src.len())?;
        if meta.point_count == 0 {
            return Err(Error::BadOffset);
        }
        points.clear();
        decode_chunk_from(src, &meta, meta.point_count as usize, &mut points)?;
        if previous.is_some_and(|last| last != (points[0].lon, points[0].lat, points[0].ele)) {
            return Err(Error::BadOffset);
        }
        for &point in points.iter().skip(usize::from(k > 0)) {
            visit(point);
            count += 1;
            previous = Some((point.lon, point.lat, point.ele));
        }
    }
    if count != h.point_count {
        return Err(Error::BadOffset);
    }
    Ok(())
}

/// One stored route waypoint as it sits on disk, every field included. The ride geometry path
/// skips the section; [`RouteReader::load_waypoints`] distils the named ones into the resident
/// [`Waypoints`] table the UI reads. See `OBCR_Spec.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waypoint {
    /// Distance from the route start to this waypoint's position, meters.
    pub dist_along_m: u32,
    /// The waypoint's own coordinate, microdegrees. It can sit off the polyline.
    pub lon: i32,
    pub lat: i32,
    /// Elevation in meters; [`WAYPOINT_ELE_NONE`](obc_formats::obcr::WAYPOINT_ELE_NONE) when the source carried none.
    pub ele: i16,
    /// The stored category byte: `0` is generic and `1..=6` are the [`PoiCategory`] wire ids.
    /// Kept raw so a rewrite carries an unknown value through byte for byte.
    pub category_id: u8,
    /// Lateral offset from the route line, meters. Positive is right of the direction of travel
    /// and `0` is on-route. It clamps rather than wraps.
    pub lateral_offset_m: i16,
    pub name: String<WAYPOINT_NAME_CAP>,
    pub provenance: Option<obc_formats::obcr::WaypointProvenance>,
}

impl Waypoint {
    /// The typed category, or `None` for generic, which also covers a byte outside `1..=6`.
    #[inline]
    pub fn category(&self) -> Option<PoiCategory> {
        PoiCategory::from_id(self.category_id)
    }
}

/// Visit each stored waypoint in route order, one [`WAYPOINT_LEN`] record at a time: the cursor
/// over the whole unfiltered section. [`RouteReader::load_waypoints`] layers the resident-table
/// policy on top. Returns the number visited.
pub fn for_each_waypoint<F: FnMut(&Waypoint)>(src: &dyn ByteSource, mut f: F) -> Result<u16, Error> {
    let mut cursor = WaypointCursor::new(src)?;
    let count = cursor.count;
    while let Some(waypoint) = cursor.next(src)? {
        f(&waypoint);
    }
    Ok(count)
}

/// A source-free cursor. Each advance reads at most one stored record.
#[derive(Debug)]
pub struct WaypointCursor {
    offset: u32,
    count: u16,
    next: u16,
}

impl WaypointCursor {
    pub fn new(src: &dyn ByteSource) -> Result<Self, Error> {
        read_header(src)?;
        let mut ext = [0u8; HEADER_FULL_LEN - HEADER_LEN];
        src.read_at(HEADER_LEN as u64, &mut ext)?;
        Ok(Self { offset: rd_u32(&ext, 0), count: rd_u16(&ext, 4), next: 0 })
    }

    pub fn next(&mut self, src: &dyn ByteSource) -> Result<Option<Waypoint>, Error> {
        if self.next == self.count {
            return Ok(None);
        }
        let at = u32::from(self.next)
            .checked_mul(WAYPOINT_LEN as u32)
            .and_then(|rel| self.offset.checked_add(rel))
            .ok_or(Error::BadOffset)?;
        let mut rec = [0u8; WAYPOINT_LEN];
        src.read_at(at.into(), &mut rec)?;
        self.next += 1;
        let name_len = (rec[15] as usize).min(WAYPOINT_NAME_CAP);
        let mut name = String::new();
        if let Ok(s) = core::str::from_utf8(&rec[WAYPOINT_NAME_OFF..WAYPOINT_NAME_OFF + name_len]) {
            let _ = name.push_str(s);
        }
        Ok(Some(Waypoint {
            dist_along_m: rd_u32(&rec, 0),
            lon: rd_i32(&rec, 4),
            lat: rd_i32(&rec, 8),
            ele: rd_i16(&rec, 12),
            provenance: obc_formats::obcr::WaypointProvenance::decode(&rec[44..80]).map_err(|_| Error::BadOffset)?,
            category_id: rec[14],
            lateral_offset_m: rd_i16(&rec, 16),
            name,
        }))
    }
}

/// The subset of a stored [`Waypoint`] the ride UI needs. `ele` is dropped on purpose, so the
/// entry stays cheap to hold [`MAX_WAYPOINTS`] of resident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WptEntry {
    /// Distance from the route start, meters: the axis the ride progress, the progress-bar ticks
    /// and the chip's distance-to-go all share.
    pub dist_along_m: u32,
    /// The waypoint's own coordinate, where its map diamond is drawn.
    pub lon: i32,
    pub lat: i32,
    /// The waypoint's category, or `None` for generic. It shares the map's [`PoiCategory`] ids,
    /// so one icon language covers both sources.
    pub category: Option<PoiCategory>,
    /// Lateral offset from the route line, meters. Positive is right of the direction of travel.
    /// The side hint reads this.
    pub lateral_offset_m: i16,
    /// The waypoint's name. An unnamed waypoint never enters the table.
    pub name: String<WAYPOINT_NAME_CAP>,
}

/// A route's resident named-waypoint table, in route order. Built once per route load and read
/// per frame by the riding views.
///
/// When a file carries more qualifying waypoints than [`MAX_WAYPOINTS`], the nearest are kept and
/// [`truncated`](Self::truncated) is set, so the ride loop can slide the window forward once the
/// rider passes the resident tail.
#[derive(Debug, Clone, Default)]
pub struct Waypoints {
    /// The kept named waypoints, in route order.
    pub entries: Vec<WptEntry, MAX_WAYPOINTS>,
    /// `true` when the file had more qualifying waypoints than [`MAX_WAYPOINTS`]: the re-window
    /// signal.
    pub truncated: bool,
}

impl Waypoints {
    /// An empty table.
    #[inline]
    pub fn new() -> Self {
        Waypoints { entries: Vec::new(), truncated: false }
    }

    #[inline]
    pub fn as_slice(&self) -> &[WptEntry] {
        &self.entries
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl RouteReader<'_> {
    /// Load the route's named waypoints into a resident table. It keeps each record that sits at
    /// or past `min_dist_m` and has a non-empty name after trimming: an unnamed waypoint appears
    /// nowhere in the UI.
    ///
    /// Records arrive in ascending distance, so the first [`MAX_WAYPOINTS`] kept are the nearest
    /// ahead of `min_dist_m`. A file with more sets [`truncated`](Waypoints::truncated), so the
    /// caller can re-window forward once the rider passes the tail.
    ///
    /// One small read per record. Call it on route load and on a re-window, never per frame.
    pub fn load_waypoints(&self, min_dist_m: u32) -> Waypoints {
        let mut wpts = Waypoints::new();
        // A torn waypoint section ends the stream early. The partial table is still safe to hand
        // back, which matches `for_each_waypoint`'s contract.
        let _ = for_each_waypoint(self.src, |w| {
            if w.dist_along_m < min_dist_m {
                return;
            }
            // Unnamed means empty or only whitespace; `all()` is true for both.
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
            // Keep streaming rather than break, so `truncated` reflects the whole file.
            if wpts.entries.push(entry).is_err() {
                wpts.truncated = true;
            }
        });
        wpts
    }
}
