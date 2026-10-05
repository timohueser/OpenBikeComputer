//! OBCR route reader: header, chunk index, and on-demand chunk walks.
//!
//! [`RouteReader`] keeps the header and the chunk index in RAM and pulls geometry chunks through
//! the [`ByteSource`] only when asked, so a route of hundreds of km is never resident. It holds a
//! `&dyn ByteSource` rather than a generic, so it threads through the app and render layers
//! without making them generic.

use core::sync::atomic::{AtomicU32, Ordering};

use heapless::{String, Vec};

use obc_formats::bike::BikeType;
use obc_formats::io::{rd_i16, rd_i32, rd_u16, rd_u32, ByteSource, DecodeError, Error};
use obc_map_scene::BBox;

use obc_formats::obcr::{validate_header_prefix, POINT_RECORD_LEN};
use obc_formats::obcr::{
    CHUNK_META_LEN, HEADER_FULL_LEN, NAME_CAP, WAYPOINT_LEN, WAYPOINT_NAME_CAP, WAYPOINT_NAME_OFF,
};
use obc_reader::PoiCategory;

use crate::cache::RouteCache;
use crate::walk::{chunk_anchor, clip, interpolate_point, validate, walk, ChunkPoints, Records};

/// The device's waypoint cap, for both the converter's emission and the resident [`Waypoints`]
/// table. The format allows up to `u16::MAX`, so [`RouteReader::load_waypoints`] windows and
/// truncates a longer file rather than overflowing.
pub const MAX_WAYPOINTS: usize = 32;
/// Resident chunk-index capacity. It sets both the maximum route length and a large part of the
/// device's stack peak, because a [`RouteIndex`] is `MAX_ROUTE_CHUNKS * 48 B` and a by-value
/// [`RouteIndex::read`] transits the stack. A route past the cap fails conversion with
/// [`Error::TooLarge`]; the host packer shares the value, so anything that packs, loads.
///
/// 256 is about 65 k points, a ~12.3 KB index. Raising it needs the by-value paths to become
/// resident first (see [`read_into`](RouteIndex::read_into)).
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
        Ok(RouteObjectInfo {
            name: h.name,
            distance_m: h.total_distance_m,
            ascent_m: h.total_ascent_m,
            descent_m: h.total_descent_m,
            attribution_map: h.attribution_map,
            visit: h.visit,
            unresolved_avoidance: h.flags & obc_formats::obcr::FLAG_UNRESOLVED_AVOIDANCE != 0,
            assistant_candidate: h.flags & obc_formats::obcr::FLAG_ASSISTANT_CANDIDATE != 0,
            point_count: h.point_count,
            waypoint_count: h.waypoint_count,
        })
    }
}

/// The last route point, `(lon, lat)` µdeg. It reads the header, the last chunk meta and that
/// chunk's records in small blocks, never the whole index or a whole chunk.
pub fn route_end(src: &dyn ByteSource) -> Result<(i32, i32), Error> {
    let h = read_header(src)?;
    let Some(last) = h.chunk_count.checked_sub(1) else { return Ok((h.start_lon, h.start_lat)) };
    let cm = stored_meta(src, &h, last)?;
    let mut end = (cm.anchor_lon, cm.anchor_lat);
    for_each_block(src, &cm, |records| {
        end = ChunkPoints::records(None, end, records).last().map_or(end, |p| (p.lon, p.lat));
        Ok(())
    })?;
    Ok(end)
}

/// The route point nearest `p`, `(lon, lat)` µdeg, as `(metres along, metres away)`. It walks every
/// chunk through one small block, so it needs no [`RouteIndex`]. As on the live first lock, a later
/// point replaces the kept one only when it is more than the matcher's tie nearer, so the outbound
/// leg of an out-and-back wins. `None` for a route without a segment.
pub fn nearest_along(src: &dyn ByteSource, p: (i32, i32)) -> Result<Option<(u32, f32)>, Error> {
    use crate::geo::project_to_segment;
    use obc_map_scene::{cos_lat, ground_dist_m_cl};
    let h = read_header(src)?;
    let mut best: Option<(u32, f32)> = None;
    for k in 0..h.chunk_count {
        let cm = stored_meta(src, &h, k)?;
        // The matcher's metric: `f32` along a chunk, at the anchor's latitude.
        let cl = cos_lat(cm.anchor_lat);
        let mut a = (cm.anchor_lon, cm.anchor_lat);
        let mut along = cm.cum_distance_m as f32;
        for_each_block(src, &cm, |records| {
            validate(records)?;
            for point in ChunkPoints::records(None, a, records) {
                let b = (point.lon, point.lat);
                let seg = ground_dist_m_cl(a, b, cl);
                let (t, dist) = project_to_segment(a, b, p, cl);
                if best.is_none_or(|(_, nearest)| dist < nearest - crate::matcher::TIE_EPS_M) {
                    best = Some(((along + t * seg) as u32, dist));
                }
                along += seg;
                a = b;
            }
            Ok(())
        })?;
    }
    Ok(best)
}

/// Visit chunk `m`'s record bytes in blocks of whole records, so one small buffer reads any chunk.
fn for_each_block(
    src: &dyn ByteSource,
    m: &ChunkMeta,
    mut visit: impl FnMut(&[u8]) -> Result<(), Error>,
) -> Result<(), Error> {
    const BLOCK: usize = 16 * POINT_RECORD_LEN;
    let mut block = [0u8; BLOCK];
    let (mut at, end) = (u64::from(m.byte_offset), u64::from(m.byte_offset) + u64::from(m.byte_len));
    while at < end {
        let bytes = &mut block[..(end - at).min(BLOCK as u64) as usize];
        src.read_at(at, bytes)?;
        visit(bytes)?;
        at += bytes.len() as u64;
    }
    Ok(())
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
/// only the chunk walks need the source.
pub struct RouteReader<'a> {
    src: &'a dyn ByteSource,
    idx: &'a RouteIndex,
    /// When present, [`with_chunk`](Self::with_chunk) serves an unchanged route from RAM instead
    /// of re-reading its geometry. `None` streams every call.
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
        for k in 0..h.chunk_count {
            let cm = stored_meta(src, &h, k)?;
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

    /// Where chunk `k` ends along the route: the next chunk's start, or the route end.
    pub(crate) fn chunk_end_m(&self, k: usize) -> u32 {
        self.index.get(k + 1).map_or(self.total_distance_m, |next| next.cum_distance_m)
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
    pub fn segment_count(&self) -> u32 {
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

    /// Visit each chunk whose bbox intersects `view`, in route order. The caller walks the ones it
    /// wants with [`RouteReader::with_chunk`].
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

    /// Like [`new`](Self::new), but backs [`with_chunk`](Self::with_chunk) with a resident
    /// [`RouteCache`]. The cache adopts the index's parse identity here, which invalidates
    /// same-key slots from a different route, so a caller never has to clear it on a switch.
    pub fn new_cached(idx: &'a RouteIndex, src: &'a dyn ByteSource, cache: &'a RouteCache) -> RouteReader<'a> {
        cache.adopt(idx.identity);
        RouteReader { src, idx, cache: Some(cache) }
    }

    /// Start a bounded cursor over the complete authored waypoint section.
    pub fn waypoint_cursor(&self) -> Result<WaypointCursor, Error> {
        WaypointCursor::new(self.source())
    }

    pub fn next_waypoint(&self, cursor: &mut WaypointCursor) -> Result<Option<Waypoint>, Error> {
        cursor.next(self.source())
    }

    /// The byte source, for this crate's whole-file passes over sections the chunk index does not
    /// cover.
    pub(crate) fn source(&self) -> &dyn ByteSource {
        self.src
    }

    /// Walk chunk `k`'s points: its anchor, then each delta-stepped point. The chunk's last point
    /// is chunk `k+1`'s anchor, so adjacent chunks stitch without a gap. A chunk decodes whole or
    /// not at all. With a [`RouteCache`] attached, a chunk read earlier is served from RAM, and
    /// the cache stays borrowed while `visit` runs.
    pub fn with_chunk<R>(&self, k: usize, visit: impl FnOnce(ChunkPoints<'_>) -> R) -> Result<R, Error> {
        let (mut visit, mut out) = (Some(visit), None);
        self.walk_chunk(k, &mut |points| out = visit.take().map(|visit| visit(points)))?;
        out.ok_or(Error::BadOffset)
    }

    // The walk itself is not generic, so every caller shares one copy of it.
    fn walk_chunk(&self, k: usize, visit: &mut dyn FnMut(ChunkPoints<'_>)) -> Result<(), Error> {
        let m = self.idx.index.get(k).ok_or(Error::BadOffset)?;
        if m.point_count == 0 {
            visit(ChunkPoints::records(None, (0, 0), &[]));
            return Ok(());
        }
        // The reader identity is revalidated for both the hit and the miss: another reader can
        // use the same cache between calls, including reentrantly from `ByteSource::read_at`
        // while this miss is being read.
        if let Some(body) = self.cache.and_then(|cache| cache.borrow_chunk(self.idx.identity, k)) {
            visit(ChunkPoints::records(Some(chunk_anchor(m)), (m.anchor_lon, m.anchor_lat), &body));
            return Ok(());
        }
        self.walk_records(k, m, visit)
    }

    // The records buffer lives in this frame, which is popped before a caller that copied points
    // out goes on to use them.
    #[inline(never)]
    fn walk_records(&self, k: usize, m: &ChunkMeta, visit: &mut dyn FnMut(ChunkPoints<'_>)) -> Result<(), Error> {
        let mut records = Records::new();
        records.read(self.src, m)?;
        if let Some(cache) = self.cache {
            cache.put(self.idx.identity, k, records.body());
        }
        visit(records.points());
        Ok(())
    }

    /// Chunk `k`'s stretch inside the inclusive interval `[lo, hi]`, its first and last points
    /// interpolated onto the boundary. `Ok(false)` when there is no such stretch.
    pub(crate) fn clip_chunk(
        &self,
        k: usize,
        lo: u32,
        hi: u32,
        emit: &mut dyn FnMut(RoutePoint) -> Result<(), Error>,
    ) -> Result<bool, Error> {
        let Some(m) = self.chunks().get(k) else { return Ok(false) };
        self.with_chunk(k, |points| clip(walk(m.cum_distance_m, points), m.point_count.into(), lo, hi, emit))?
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

    /// The interpolated point at `target`, which the caller has already clamped, plus its chunk
    /// and segment.
    // Out of line: the UI calls it from many places, and one copy is enough.
    #[inline(never)]
    fn locate_interpolated(&self, target: u32) -> Option<(RoutePoint, usize, usize)> {
        let k = self.chunks().iter().rposition(|cm| cm.cum_distance_m <= target).unwrap_or(0);
        let cm = self.chunks().get(k)?;
        let n = usize::from(cm.point_count);
        self.with_chunk(k, |points| {
            let mut previous = None;
            for (i, b) in walk(cm.cum_distance_m, points).enumerate() {
                let Some(a) = previous.replace(b) else { continue };
                // The last segment takes any target past the chunk's measured end.
                if target as f64 <= b.along || i + 1 == n {
                    let t = if b.seg > 1e-3 { ((target as f64 - a.along) / b.seg).clamp(0.0, 1.0) } else { 0.0 };
                    return Some((interpolate_point(a.p, b.p, t as f32), k, i - 1));
                }
            }
            previous.map(|only| (only.p, k, 0))
        })
        .ok()?
    }

    /// The interpolated elevation at `progress_m`, clamped to the route end: the splice path's
    /// seam-endpoint sampler. It is public so the splice's seam contract can be asserted against
    /// the same lookup the splice uses.
    pub fn elevation_at(&self, progress_m: u32) -> Option<i16> {
        let target = progress_m.min(self.total_distance_m);
        self.locate_interpolated(target)?.0.elevation()
    }

    /// The position at `progress_m`, clamped to the route end and interpolated inside the
    /// containing segment.
    pub fn position_at(&self, progress_m: u32) -> Option<RoutePosition> {
        let target = progress_m.min(self.total_distance_m);
        let (p, chunk, seg) = self.locate_interpolated(target)?;
        Some(RoutePosition { progress_m: target, lon: p.lon, lat: p.lat, chunk, seg })
    }

    /// Stream only the polyline stretch in the inclusive interval `[start_m, end_m]`. Each
    /// callback slice is one clipped chunk, its first and last coordinates interpolated at the
    /// boundary. A chunk that fails to decode is skipped.
    // Out of line, so only this coordinate scratch, not a chunk's records, is live under `visit`.
    #[inline(never)]
    pub fn visit_points_between(&self, start_m: u32, end_m: u32, mut visit: impl FnMut(&[(i32, i32)])) {
        let lo = start_m.min(self.total_distance_m);
        let hi = end_m.min(self.total_distance_m);
        if lo >= hi {
            return;
        }
        let mut lonlat = [(0i32, 0i32); MAX_POINTS_PER_CHUNK];
        for (k, cm) in self.chunks().iter().enumerate() {
            if self.chunk_end_m(k) < lo || cm.cum_distance_m > hi {
                continue;
            }
            let mut n = 0;
            let clipped = self.clip_chunk(k, lo, hi, &mut |p| {
                lonlat[n] = (p.lon, p.lat);
                n += 1;
                Ok(())
            });
            if clipped == Ok(true) {
                visit(&lonlat[..n]);
            }
        }
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

    /// Only this coordinate scratch, not the chunk's records, is live while `visit` descends into
    /// a caller's walk. A chunk that fails to decode is skipped.
    // Out of line, so the scratch stays in this frame and the renderer shares one copy.
    #[inline(never)]
    fn visit_chunk_points(&self, k: usize, visit: &mut dyn FnMut(&[(i32, i32)])) {
        let mut lonlat = [(0i32, 0i32); MAX_POINTS_PER_CHUNK];
        let decoded = self.with_chunk(k, |points| {
            let mut n = 0;
            for p in points {
                lonlat[n] = (p.lon, p.lat);
                n += 1;
            }
            n
        });
        if let Ok(n) = decoded {
            visit(&lonlat[..n]);
        }
    }
}

/// Decode one chunk-meta record, validating its point count and that its data region lies inside
/// `src_len`.
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

/// Chunk `k`'s meta read straight from the source: what a walk that holds no [`RouteIndex`] reads
/// instead.
pub(crate) fn stored_meta(src: &dyn ByteSource, h: &Header, k: u32) -> Result<ChunkMeta, Error> {
    // `index_offset` is untrusted header input, so a forged offset near `u32::MAX` must surface as
    // a bad offset and never wrap back into the file.
    let off =
        k.checked_mul(CHUNK_META_LEN as u32).and_then(|rel| h.index_offset.checked_add(rel)).ok_or(Error::BadOffset)?;
    let mut meta = [0u8; CHUNK_META_LEN];
    src.read_at(off.into(), &mut meta)?;
    parse_chunk_meta(&meta, src.len())
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
    LAST.try_update(Ordering::Relaxed, Ordering::Relaxed, |identity| identity.checked_add(1))
        .map(|identity| identity + 1)
        .map_err(|_| Error::TooLarge)
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
    pub(crate) waypoint_offset: u32,
    pub(crate) waypoint_count: u16,
    pub(crate) attribution_map: Option<obc_formats::obcr::RouteSourceKey>,
    pub(crate) visit: Option<obc_formats::obcr::VisitDescriptor>,
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
    if h[5] & !63 != 0 || h[119] != 0 {
        return Err(Error::BadOffset);
    }
    if h[5] & obc_formats::obcr::FLAG_ATTRIBUTION_MAP == 0 && h[128..160].iter().any(|b| *b != 0) {
        return Err(Error::BadOffset);
    }
    let attribution_map = if h[5] & obc_formats::obcr::FLAG_ATTRIBUTION_MAP != 0 {
        Some(obc_formats::obcr::RouteSourceKey::decode(&h[128..160]).map_err(|_| Error::BadOffset)?)
    } else {
        None
    };
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
    let mut visit = None;
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
        visit = Some(descriptor);
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
        waypoint_offset: rd_u32(&h, 112),
        waypoint_count: rd_u16(&h, 116),
        attribution_map,
        visit,
    })
}

/// Stream every stored point of a complete route once, in order, without a route index. Each chunk
/// must repeat the previous chunk's last point, and the header's point count must match.
// Out of line, so a chunk's records leave the caller's frame.
#[inline(never)]
pub(crate) fn for_each_stored_point(
    src: &dyn ByteSource,
    h: &Header,
    visit: &mut dyn FnMut(RoutePoint),
) -> Result<(), Error> {
    if h.chunk_count == 0 || h.chunk_count as usize > MAX_ROUTE_CHUNKS {
        return Err(Error::BadOffset);
    }
    let mut previous = None;
    let mut count = 0u32;
    let mut records = Records::new();
    for k in 0..h.chunk_count {
        let meta = stored_meta(src, h, k)?;
        if meta.point_count == 0 {
            return Err(Error::BadOffset);
        }
        records.read(src, &meta)?;
        if previous.is_some_and(|last| last != (meta.anchor_lon, meta.anchor_lat, meta.anchor_ele)) {
            return Err(Error::BadOffset);
        }
        for point in records.points().skip(usize::from(k > 0)) {
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
        let h = read_header(src)?;
        Ok(Self { offset: h.waypoint_offset, count: h.waypoint_count, next: 0 })
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
