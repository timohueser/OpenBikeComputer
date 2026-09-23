//! Streaming GPX conversion and the shared bounded OBCR emitter.

use heapless::Vec;

use crate::gpx::{GpxScanner, RawWaypoint, WptScanner};
use crate::reader::{ChunkMeta, MAX_POINTS_PER_CHUNK, MAX_ROUTE_CHUNKS, MAX_WAYPOINTS};
use crate::symbol::category_for_symbol;
use obc_elevation::DeadBand;
use obc_formats::bike::BikeType;
use obc_formats::io::{put_i16, put_i32, put_u16, put_u32, ByteSink, ByteSource, Error};
use obc_formats::obcr::{
    BIKE_TYPE_OFF, CHUNK_META_LEN, HEADER_FULL_LEN, MAGIC, NAME_CAP, POINT_RECORD_LEN, VERSION,
    WAYPOINT_CATEGORY_GENERIC, WAYPOINT_ELE_NONE, WAYPOINT_LEN, WAYPOINT_NAME_OFF,
};
use obc_map_scene::BBox;
use obc_map_scene::{cos_lat, delta_m, ground_dist_m};

/// Decimation tolerance: drop a vertex within this perpendicular distance of the chord.
const EPSILON_M: f32 = 1.0;
/// Force a kept geometry vertex at least this often, so a long near-straight run keeps shape
/// fidelity at real (not interpolated) points.
const MAX_SPAN_M: f32 = 1200.0;
/// Largest stored per-vertex coordinate delta (µdeg). A longer segment is split with
/// interpolated vertices so `(x - px) as i16` never wraps. Mirrors the OBCM packer's
/// `MAX_SEGMENT` so both formats densify on the same threshold.
const MAX_SEGMENT_UDEG: i64 = 30_000;

/// Max bytes of one chunk's record body (`(points-1) × 7`).
const BODY_CAP: usize = (MAX_POINTS_PER_CHUNK - 1) * POINT_RECORD_LEN;

/// Stats computed during conversion and written into the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteStats {
    pub point_count: u32,
    pub chunk_count: u32,
    pub total_distance_m: u32,
    pub total_ascent_m: u32,
    pub total_descent_m: u32,
    pub min_ele_m: i16,
    pub max_ele_m: i16,
    /// Waypoints stored in the waypoint section (0 when the GPX carried no `<wpt>`).
    pub waypoint_count: u16,
    /// At least one retained point has a valid elevation; zero metres is valid.
    pub has_elevation: bool,
    /// Every pushed point carried a valid, complete elevation.
    pub elevation_complete: bool,
}

/// Convert a GPX byte source into a `.obcr` written to `sink`, naming the route `name` and
/// typing it [`BikeType::Road`].
pub fn gpx_to_obcr(src: &dyn ByteSource, name: &str, sink: &mut dyn ByteSink) -> Result<RouteStats, Error> {
    gpx_to_obcr_attributed(src, name, BikeType::Road, sink, None, |_, _| Ok(0))
}

/// Import with a bounded incoming-segment attribution provider. Plain GPX stays unknown.
pub fn gpx_to_obcr_attributed(
    src: &dyn ByteSource,
    name: &str,
    bike: BikeType,
    sink: &mut dyn ByteSink,
    map_source: Option<obc_formats::obcr::RouteSourceKey>,
    mut surface: impl FnMut((i32, i32), (i32, i32)) -> Result<u8, Error>,
) -> Result<RouteStats, Error> {
    let mut em = ObcrEmitter::new(sink)?;
    em.set_attribution_map(map_source);
    em.set_bike_type(bike);

    // GPX carries `<wpt>` before the track, so collect up to MAX_WAYPOINTS first and place each
    // on the track in the pass below. The scope keeps the two scanners' block buffers from being
    // resident at the same time.
    let mut wps: Vec<WpPlace, MAX_WAYPOINTS> = Vec::new();
    {
        let mut scan = WptScanner::new(src);
        while let Some(wp) = scan.next_waypoint()? {
            // Only the category is stored, so the freeform `<sym>` text stops at the import.
            let category_id = category_for_symbol(&wp.symbol).map_or(WAYPOINT_CATEGORY_GENERIC, |c| c.id());
            let place = WpPlace {
                wp,
                best_d2: f32::INFINITY,
                along_m: 0,
                category_id,
                lateral_offset_m: 0,
                sign_pending: false,
                provenance: None,
                raw_index: 0,
            };
            if wps.push(place).is_err() {
                break; // cap reached: keep the first MAX_WAYPOINTS
            }
        }
    }

    let mut scan = GpxScanner::new(src);

    let mut raw_index = 0;
    // The previous raw point, kept only for the waypoint offset's direction of travel.
    let mut prev_raw: Option<(i32, i32)> = None;

    while !wps.is_empty() {
        let Some(p) = scan.next_point()? else { break };

        // Placement: the nearest raw track point wins and its cumulative distance becomes the
        // waypoint's position along the route. The phone importer uses the same nearest-point
        // rule, so the two OBCR producers agree. The stored lateral offset comes from the same
        // winning point: magnitude is the distance to it, sign is the side of travel.
        if !wps.is_empty() {
            let cl = cos_lat(p.lat);
            let here = (p.lon, p.lat);
            for w in wps.iter_mut() {
                // A best point that is the track's first has no incoming segment for a heading,
                // so the sign waits one point for the outgoing segment.
                if w.sign_pending {
                    if let Some(pr) = prev_raw {
                        w.lateral_offset_m = signed_offset_m(w.best_d2, cross(pr, here, pr, (w.wp.lon, w.wp.lat), cl));
                    }
                    w.sign_pending = false;
                }
                let (dx, dy) = delta_m(here, (w.wp.lon, w.wp.lat), cl);
                let d2 = dx * dx + dy * dy;
                if d2 < w.best_d2 {
                    w.best_d2 = d2;
                    w.raw_index = raw_index;
                    match prev_raw {
                        Some(pr) => {
                            w.lateral_offset_m = signed_offset_m(d2, cross(pr, here, here, (w.wp.lon, w.wp.lat), cl));
                        }
                        // Magnitude now, side on the next point. A one-point track keeps
                        // `signed_offset_m`'s positive default.
                        None => {
                            w.lateral_offset_m = signed_offset_m(d2, 0.0);
                            w.sign_pending = true;
                        }
                    }
                }
            }
        }
        prev_raw = Some((p.lon, p.lat));
        raw_index += 1;
    }

    let mut scan = GpxScanner::new(src);
    let mut previous = None;
    let mut raw_index = 0;
    while let Some(p) = scan.next_point()? {
        let ele = p.ele.map_or(i16::MIN, |e| round_i16(e as f64));
        em.set_surface(if let Some(prev) = previous { surface(prev, (p.lon, p.lat))? } else { 0 });
        em.push(sink, p.lon, p.lat, ele)?;
        if wps.iter().any(|w| w.raw_index == raw_index) {
            em.flush_pending(sink)?;
            for w in wps.iter_mut().filter(|w| w.raw_index == raw_index) {
                w.along_m = em.enc.distance as u32;
            }
        }
        previous = Some((p.lon, p.lat));
        raw_index += 1;
    }
    em.finish(sink, name, &mut wps)
}

/// The streaming OBCR writer shared by [`gpx_to_obcr`] and the nav router's emit
/// ([`crate::nav`]). It owns every format and geometry invariant, so the two OBCR producers stay
/// byte-compatible by construction.
pub(crate) struct ObcrEmitter {
    enc: Encoder,
    /// Cumulative raw-path distance. `f64` because an `f32` running total drifts over the
    /// thousands of segments of a long route.
    cum_dist: f64,
    prev: Option<(i32, i32)>,
    bbox: Option<BBox>,
    start: (i32, i32),
    emitted: u32,
    // Decimation state (1-step lookahead).
    last_kept: Option<Cand>,
    pending: Option<Cand>,
    /// Elevation-detail keep threshold (m). `0` turns it off. See
    /// [`keep_elevation_detail`](ObcrEmitter::keep_elevation_detail).
    ele_keep_m: i16,
    surface: u8,
    /// A pushed point had no elevation, or an incomplete one.
    ele_gap: bool,
    flags: u8,
    bike: BikeType,
    map_source: Option<obc_formats::obcr::RouteSourceKey>,
}

impl ObcrEmitter {
    /// # Safety
    /// `slot` must be aligned, writable and exclusively owned for a complete emitter.
    pub(crate) unsafe fn init_in_place(slot: *mut Self) {
        use core::ptr::addr_of_mut;
        unsafe {
            Encoder::init_in_place(addr_of_mut!((*slot).enc));
            addr_of_mut!((*slot).cum_dist).write(0.0);
            addr_of_mut!((*slot).prev).write(None);
            addr_of_mut!((*slot).bbox).write(None);
            addr_of_mut!((*slot).start).write((0, 0));
            addr_of_mut!((*slot).emitted).write(0);
            addr_of_mut!((*slot).last_kept).write(None);
            addr_of_mut!((*slot).pending).write(None);
            addr_of_mut!((*slot).ele_keep_m).write(1);
            addr_of_mut!((*slot).surface).write(0);
            addr_of_mut!((*slot).ele_gap).write(false);
            addr_of_mut!((*slot).flags).write(0);
            addr_of_mut!((*slot).bike).write(BikeType::default());
            addr_of_mut!((*slot).map_source).write(None);
            let Self {
                enc: _,
                cum_dist: _,
                prev: _,
                bbox: _,
                start: _,
                emitted: _,
                last_kept: _,
                pending: _,
                ele_keep_m: _,
                surface: _,
                ele_gap: _,
                flags: _,
                bike: _,
                map_source: _,
            } = &*slot;
        }
    }
    /// Reserve the header on `sink`. The body follows at `data_offset = HEADER_FULL_LEN`.
    pub(crate) fn new(sink: &mut dyn ByteSink) -> Result<ObcrEmitter, Error> {
        Self::begin(sink)?;
        Ok(Self::empty())
    }

    /// Construct in the owner's workspace before streaming starts. Nothing is written to a sink.
    pub(crate) fn empty() -> Self {
        ObcrEmitter {
            enc: Encoder::new(HEADER_FULL_LEN as u32),
            cum_dist: 0.0,
            prev: None,
            bbox: None,
            start: (0, 0),
            emitted: 0,
            last_kept: None,
            pending: None,
            ele_keep_m: 1,
            surface: 0,
            ele_gap: false,
            flags: 0,
            bike: BikeType::default(),
            map_source: None,
        }
    }

    /// Start an empty emitter's stream without moving its bounded index and chunk buffers.
    pub(crate) fn begin(sink: &mut dyn ByteSink) -> Result<(), Error> {
        sink.write(&[0u8; HEADER_FULL_LEN])
    }

    /// Retain elevation changes of at least this many metres. The default is one metre.
    pub(crate) fn keep_elevation_detail(&mut self, threshold_m: i16) {
        self.ele_keep_m = threshold_m;
    }

    pub(crate) fn set_surface(&mut self, surface: u8) {
        self.surface = surface & 7;
    }

    pub(crate) fn set_elevation_incomplete(&mut self, incomplete: bool) {
        self.surface = (self.surface & 7) | if incomplete { 8 } else { 0 };
    }

    pub(crate) fn set_flags(&mut self, flags: u8) {
        self.flags = flags;
    }

    pub(crate) fn set_bike_type(&mut self, bike: BikeType) {
        self.bike = bike;
    }

    pub(crate) fn set_attribution_map(&mut self, source: Option<obc_formats::obcr::RouteSourceKey>) {
        self.map_source = source;
    }

    pub(crate) fn attribution_map(&self) -> Option<obc_formats::obcr::RouteSourceKey> {
        self.map_source
    }

    /// Measured retained distance, including the most recently retained transform point.
    #[inline]
    pub(crate) fn distance_m(&self) -> u32 {
        self.enc.distance as u32
    }

    /// End of the geometry and index after `finish` with no waypoint records.
    pub(crate) fn geometry_end(&self) -> u32 {
        self.enc.data_pos + self.enc.index.len() as u32 * CHUNK_META_LEN as u32
    }

    /// Feed one raw point: accumulate distance and bbox, then run the decimator. Each kept point
    /// is emitted to the encoder, densified.
    pub(crate) fn push(&mut self, sink: &mut dyn ByteSink, lon: i32, lat: i32, ele: i16) -> Result<(), Error> {
        if let Some(pr) = self.prev {
            self.cum_dist += ground_dist_m(pr, (lon, lat)) as f64;
        } else {
            self.start = (lon, lat);
        }
        self.prev = Some((lon, lat));
        self.bbox = Some(grow(self.bbox, lon, lat));
        self.ele_gap |= ele == i16::MIN || self.surface & 8 != 0;

        let c = Cand {
            lon,
            lat,
            ele,
            surface: self.surface & if ele == i16::MIN { 7 } else { 15 },
            cum_d: self.cum_dist as u32,
        };
        match (self.last_kept, self.pending) {
            (None, _) => {
                self.emitted += emit_densified(&mut self.enc, sink, None, c)?;
                self.last_kept = Some(c);
            }
            (Some(_), None) => self.pending = Some(c),
            (Some(lk), Some(pd)) => {
                let perp = perp_dist_m(lk, c, pd);
                let span = (c.cum_d - lk.cum_d) as f32;
                let ele_break =
                    self.ele_keep_m > 0 && (i32::from(pd.ele) - i32::from(lk.ele)).abs() >= i32::from(self.ele_keep_m);
                if perp > EPSILON_M
                    || span > MAX_SPAN_M
                    || ele_break
                    || pd.surface != c.surface
                    || (pd.ele == i16::MIN) != (lk.ele == i16::MIN)
                    || (pd.ele == i16::MIN) != (c.ele == i16::MIN)
                    || reverses(lk, pd, c)
                {
                    self.emitted += emit_densified(&mut self.enc, sink, Some(lk), pd)?;
                    self.last_kept = Some(pd);
                }
                self.pending = Some(c);
            }
        }
        Ok(())
    }

    /// Stored route geometry is already simplified, so keep every vertex.
    pub(crate) fn push_retained(&mut self, sink: &mut dyn ByteSink, lon: i32, lat: i32, ele: i16) -> Result<(), Error> {
        self.push(sink, lon, lat, ele)?;
        self.flush_pending(sink)
    }

    fn flush_pending(&mut self, sink: &mut dyn ByteSink) -> Result<(), Error> {
        if let Some(pd) = self.pending.take() {
            self.emitted += emit_densified(&mut self.enc, sink, self.last_kept, pd)?;
            self.last_kept = Some(pd);
        }
        Ok(())
    }

    /// Flush the trailing point, write the chunk index and waypoint table, and backfill the
    /// header. `Error::Empty` if no point was pushed. Pass an empty `wps` for a waypoint-free
    /// route. Call once; discard the stream after any error.
    pub(crate) fn finish(
        &mut self,
        sink: &mut dyn ByteSink,
        name: &str,
        wps: &mut Vec<WpPlace, MAX_WAYPOINTS>,
    ) -> Result<RouteStats, Error> {
        // The final point is always kept.
        self.flush_pending(sink)?;
        if self.emitted == 0 {
            return Err(Error::Empty);
        }

        self.enc.finish(sink)?;
        let index_offset = self.enc.write_index(sink)?;
        let wpt_offset =
            write_waypoints(sink, wps, index_offset + self.enc.index.len() as u32 * CHUNK_META_LEN as u32)?;

        let bbox = self.bbox.unwrap_or(BBox { min_lon: 0, min_lat: 0, max_lon: 0, max_lat: 0 });
        let stats = RouteStats {
            point_count: self.emitted,
            chunk_count: self.enc.index.len() as u32,
            total_distance_m: self.enc.distance as u32,
            total_ascent_m: self.enc.band.ascent() as u32,
            total_descent_m: self.enc.band.descent() as u32,
            min_ele_m: if self.enc.min_ele <= self.enc.max_ele { self.enc.min_ele } else { 0 },
            max_ele_m: if self.enc.min_ele <= self.enc.max_ele { self.enc.max_ele } else { 0 },
            waypoint_count: wps.len() as u16,
            has_elevation: self.enc.min_ele <= self.enc.max_ele,
            elevation_complete: !self.ele_gap,
        };

        let mut header = build_header(name, &bbox, self.start, index_offset, wpt_offset, &stats);
        header[5] |= self.flags;
        header[BIKE_TYPE_OFF] = self.bike as u8;
        if let Some(source) = self.map_source {
            header[5] |= obc_formats::obcr::FLAG_ATTRIBUTION_MAP;
            source.encode(header[128..160].as_mut().try_into().unwrap());
        }
        sink.patch_at(0, &header)?;
        Ok(stats)
    }
}

/// A waypoint being placed: the raw `<wpt>` plus the best squared distance to any raw track point
/// seen so far and the cumulative route distance there.
pub(crate) struct WpPlace {
    wp: RawWaypoint,
    best_d2: f32,
    along_m: u32,
    /// The stored category byte, mapped from `<sym>`/`<type>` at import and preserved verbatim
    /// across a splice.
    category_id: u8,
    /// Signed lateral offset from the route line, m. Positive is right of travel.
    lateral_offset_m: i16,
    /// The winning track point was the track's first, so the offset magnitude is stored but its
    /// side still waits for the outgoing segment.
    sign_pending: bool,
    provenance: Option<obc_formats::obcr::WaypointProvenance>,
    raw_index: u32,
}

impl WpPlace {
    /// Re-place a stored [`Waypoint`](crate::reader::Waypoint) at a possibly shifted along-route
    /// distance. The category byte and the lateral offset stay unchanged: a splice drops the
    /// waypoints of the replaced span, so every survivor still sits beside its own geometry.
    pub(crate) fn from_stored(w: &crate::reader::Waypoint, along_m: u32) -> WpPlace {
        WpPlace {
            wp: RawWaypoint {
                lon: w.lon,
                lat: w.lat,
                ele: (w.ele != WAYPOINT_ELE_NONE).then_some(w.ele as f32),
                name: w.name.clone(),
                symbol: heapless::String::new(), // already mapped into `category_id`
            },
            best_d2: 0.0,
            along_m,
            category_id: w.category_id,
            lateral_offset_m: w.lateral_offset_m,
            sign_pending: false,
            provenance: w.provenance,
            raw_index: 0,
        }
    }
}

/// The 2-D cross product of the direction of travel `dir_a` to `dir_b` with the offset `at` to
/// `wp`, in the local-equirectangular metric (`cl = cos_lat`). Positive means `wp` is left of
/// travel. Only the sign is used.
fn cross(dir_a: (i32, i32), dir_b: (i32, i32), at: (i32, i32), wp: (i32, i32), cl: f32) -> f32 {
    let (ux, uy) = delta_m(dir_a, dir_b, cl);
    let (vx, vy) = delta_m(at, wp, cl);
    ux * vy - uy * vx
}

/// The stored lateral offset: `sqrt(d2)` metres with the side as its sign, negative left of
/// travel and positive right. It saturates instead of wrapping, so a waypoint 40 km off route
/// reads as very far right, not slightly left. `cross == 0` takes the positive sign.
fn signed_offset_m(d2: f32, cross: f32) -> i16 {
    let m = libm::roundf(libm::sqrtf(d2)).clamp(0.0, i16::MAX as f32) as i16;
    if cross > 0.0 {
        -m
    } else {
        m
    }
}

/// Sort the placed waypoints by position along the route and write the fixed-record table at
/// `offset`, right after the chunk index. Returns the table's file offset for the header
/// extension, or 0 when there are no waypoints.
fn write_waypoints(sink: &mut dyn ByteSink, wps: &mut Vec<WpPlace, MAX_WAYPOINTS>, offset: u32) -> Result<u32, Error> {
    if wps.is_empty() {
        return Ok(0);
    }
    // Insertion sort by `along_m`: stable, bounded by MAX_WAYPOINTS, and needs no allocator.
    for i in 1..wps.len() {
        let mut j = i;
        while j > 0 && wps[j - 1].along_m > wps[j].along_m {
            wps.swap(j - 1, j);
            j -= 1;
        }
    }
    for w in wps.iter() {
        let mut rec = [0u8; WAYPOINT_LEN];
        put_u32(&mut rec, 0, w.along_m);
        put_i32(&mut rec, 4, w.wp.lon);
        put_i32(&mut rec, 8, w.wp.lat);
        put_i16(&mut rec, 12, w.wp.ele.map_or(WAYPOINT_ELE_NONE, |e| round_i16(e as f64)));
        rec[14] = w.category_id; // GPX pass: the mapped symbol; splice pass: the stored byte
        rec[15] = w.wp.name.len() as u8;
        put_i16(&mut rec, 16, w.lateral_offset_m); // rec[18..20] reserved
        rec[WAYPOINT_NAME_OFF..WAYPOINT_NAME_OFF + w.wp.name.len()].copy_from_slice(w.wp.name.as_bytes());
        if let Some(provenance) = w.provenance {
            rec[44..80].copy_from_slice(&provenance.encode());
        }
        sink.write(&rec)?;
    }
    Ok(offset)
}

#[derive(Debug, Clone, Copy)]
struct Cand {
    lon: i32,
    lat: i32,
    ele: i16,
    surface: u8,
    cum_d: u32,
}

/// Emit `c`, first inserting interpolated synthetic vertices so no stored `(Δlon, Δlat)` exceeds
/// the `int16` range. `prev` is the last emitted vertex, or `None` for the first point. Returns
/// the count emitted. `MAX_SPAN_M` alone is not enough: a single raw segment with no intermediate
/// candidate, such as a 2-point export, would wrap `int16`.
fn emit_densified(enc: &mut Encoder, sink: &mut dyn ByteSink, prev: Option<Cand>, c: Cand) -> Result<u32, Error> {
    let prev = match prev {
        Some(p) => p,
        None => {
            enc.emit(sink, c)?;
            return Ok(1);
        }
    };
    let dlon = (c.lon - prev.lon) as i64;
    let dlat = (c.lat - prev.lat) as i64;
    let mut emitted = 0u32;
    let max_dist = dlon.abs().max(dlat.abs());
    if max_dist > MAX_SEGMENT_UDEG {
        let steps = max_dist / MAX_SEGMENT_UDEG + 1;
        for step in 1..steps {
            enc.emit(sink, lerp(prev, c, step as f64 / steps as f64))?;
            emitted += 1;
        }
    }
    enc.emit(sink, c)?;
    Ok(emitted + 1)
}

fn lerp(a: Cand, b: Cand, t: f64) -> Cand {
    let f = |s: i32, e: i32| s + libm::round((e as f64 - s as f64) * t) as i32;
    let g = |s: u32, e: u32| (s as f64 + (e as f64 - s as f64) * t) as u32;
    Cand {
        lon: f(a.lon, b.lon),
        lat: f(a.lat, b.lat),
        ele: if a.ele == i16::MIN || b.ele == i16::MIN || b.surface & 8 != 0 {
            i16::MIN
        } else {
            round_i16(a.ele as f64 + (b.ele as f64 - a.ele as f64) * t)
        },
        surface: b.surface,
        cum_d: g(a.cum_d, b.cum_d),
    }
}

/// Accumulates kept points into seam-sharing chunks, streams each finished chunk's body out, and
/// collects its `ChunkMeta` in a bounded resident index.
struct Encoder {
    index: Vec<ChunkMeta, MAX_ROUTE_CHUNKS>,
    cur: Vec<(i32, i32, i16, u8), MAX_POINTS_PER_CHUNK>,
    data_pos: u32,
    chunk_start_dist: u32,
    chunk_start_ascent: u32,
    distance: f64,
    previous: Option<(i32, i32)>,
    band: DeadBand<f64>,
    min_ele: i16,
    max_ele: i16,
}

impl Encoder {
    unsafe fn init_in_place(slot: *mut Self) {
        use core::ptr::addr_of_mut;
        unsafe {
            addr_of_mut!((*slot).index).write(Vec::new());
            addr_of_mut!((*slot).cur).write(Vec::new());
            addr_of_mut!((*slot).data_pos).write(HEADER_FULL_LEN as u32);
            addr_of_mut!((*slot).chunk_start_dist).write(0);
            addr_of_mut!((*slot).chunk_start_ascent).write(0);
            addr_of_mut!((*slot).distance).write(0.0);
            addr_of_mut!((*slot).previous).write(None);
            addr_of_mut!((*slot).band).write(DeadBand::new());
            addr_of_mut!((*slot).min_ele).write(i16::MAX);
            addr_of_mut!((*slot).max_ele).write(i16::MIN);
            let Self {
                index: _,
                cur: _,
                data_pos: _,
                chunk_start_dist: _,
                chunk_start_ascent: _,
                distance: _,
                previous: _,
                band: _,
                min_ele: _,
                max_ele: _,
            } = &*slot;
        }
    }
    fn new(data_offset: u32) -> Self {
        Encoder {
            index: Vec::new(),
            cur: Vec::new(),
            data_pos: data_offset,
            chunk_start_dist: 0,
            chunk_start_ascent: 0,
            distance: 0.0,
            previous: None,
            band: DeadBand::new(),
            min_ele: i16::MAX,
            max_ele: i16::MIN,
        }
    }

    fn emit(&mut self, sink: &mut dyn ByteSink, c: Cand) -> Result<(), Error> {
        let before = self.distance as u32;
        let first = self.previous.is_none();
        if let Some(previous) = self.previous {
            self.distance += ground_dist_m(previous, (c.lon, c.lat)) as f64;
        }
        self.previous = Some((c.lon, c.lat));
        if c.surface & 8 != 0 {
            self.band.pause();
        }
        if c.ele == i16::MIN {
            self.band.pause();
        } else {
            if first || self.distance as u32 > before {
                self.band.push(c.ele as f64);
            }
            self.min_ele = self.min_ele.min(c.ele);
            self.max_ele = self.max_ele.max(c.ele);
        }
        if self.cur.is_empty() {
            self.chunk_start_dist = self.distance as u32;
            self.chunk_start_ascent = self.band.ascent() as u32;
        }
        let _ = self.cur.push((c.lon, c.lat, c.ele, c.surface));
        if self.cur.len() == MAX_POINTS_PER_CHUNK {
            self.finalize(sink)?;
            // Reseed the next chunk with this point as the shared seam / anchor.
            self.chunk_start_dist = self.distance as u32;
            self.chunk_start_ascent = self.band.ascent() as u32;
            let _ = self.cur.push((c.lon, c.lat, c.ele, c.surface));
        }
        Ok(())
    }

    /// Flush the trailing chunk. A lone seam point is already in the prior chunk, so it is
    /// skipped.
    fn finish(&mut self, sink: &mut dyn ByteSink) -> Result<(), Error> {
        if self.cur.len() >= 2 || (self.index.is_empty() && !self.cur.is_empty()) {
            self.finalize(sink)?;
        }
        Ok(())
    }

    fn finalize(&mut self, sink: &mut dyn ByteSink) -> Result<(), Error> {
        let n = self.cur.len();
        if n == 0 {
            return Ok(());
        }
        let (ax, ay, ae, _) = self.cur[0];
        let mut bbox = BBox { min_lon: ax, min_lat: ay, max_lon: ax, max_lat: ay };
        let mut body: Vec<u8, BODY_CAP> = Vec::new();
        for i in 1..n {
            let (x, y, e, surface) = self.cur[i];
            let (px, py, _, _) = self.cur[i - 1];
            let _ = body.extend_from_slice(&((x - px) as i16).to_le_bytes());
            let _ = body.extend_from_slice(&((y - py) as i16).to_le_bytes());
            let _ = body.extend_from_slice(&e.to_le_bytes());
            let _ = body.push(surface);
        }
        for &(x, y, _, _) in &self.cur {
            bbox_extend(&mut bbox, x, y);
        }
        sink.write_chunk((ax, ay, ae), &body)?;
        let meta = ChunkMeta {
            bbox,
            anchor_lon: ax,
            anchor_lat: ay,
            anchor_ele: ae,
            point_count: n as u16,
            cum_distance_m: self.chunk_start_dist,
            cum_ascent_m: self.chunk_start_ascent,
            byte_offset: self.data_pos,
            byte_len: body.len() as u32,
        };
        self.data_pos += body.len() as u32;
        self.index.push(meta).map_err(|_| Error::TooLarge)?;
        self.cur.clear();
        Ok(())
    }

    /// Write the chunk index after the chunk bodies; returns its file offset.
    fn write_index(&mut self, sink: &mut dyn ByteSink) -> Result<u32, Error> {
        let index_offset = self.data_pos;
        let mut m = [0u8; CHUNK_META_LEN];
        for cm in &self.index {
            put_i32(&mut m, 0, cm.bbox.min_lon);
            put_i32(&mut m, 4, cm.bbox.min_lat);
            put_i32(&mut m, 8, cm.bbox.max_lon);
            put_i32(&mut m, 12, cm.bbox.max_lat);
            put_i32(&mut m, 16, cm.anchor_lon);
            put_i32(&mut m, 20, cm.anchor_lat);
            put_i16(&mut m, 24, cm.anchor_ele);
            put_u16(&mut m, 26, cm.point_count);
            put_u32(&mut m, 28, cm.cum_distance_m);
            put_u32(&mut m, 32, cm.cum_ascent_m);
            put_u32(&mut m, 36, cm.byte_offset);
            put_u32(&mut m, 40, cm.byte_len);
            sink.write(&m)?;
        }
        Ok(index_offset)
    }
}

fn build_header(
    name: &str,
    bbox: &BBox,
    start: (i32, i32),
    index_offset: u32,
    wpt_offset: u32,
    s: &RouteStats,
) -> [u8; HEADER_FULL_LEN] {
    let mut h = [0u8; HEADER_FULL_LEN];
    h[0..4].copy_from_slice(MAGIC);
    h[4] = VERSION;
    h[5] = if s.has_elevation { obc_formats::obcr::FLAG_HAS_ELEVATION } else { 0 };

    // Name truncated to NAME_CAP on a char boundary.
    let mut nlen = 0;
    for (i, ch) in name.char_indices() {
        if i + ch.len_utf8() > NAME_CAP {
            break;
        }
        nlen = i + ch.len_utf8();
    }
    h[6] = nlen as u8;
    h[64..64 + nlen].copy_from_slice(&name.as_bytes()[..nlen]);

    put_i32(&mut h, 8, bbox.min_lon);
    put_i32(&mut h, 12, bbox.min_lat);
    put_i32(&mut h, 16, bbox.max_lon);
    put_i32(&mut h, 20, bbox.max_lat);
    put_i32(&mut h, 24, start.0);
    put_i32(&mut h, 28, start.1);
    put_u32(&mut h, 32, s.point_count);
    put_u32(&mut h, 36, s.total_distance_m);
    put_u32(&mut h, 40, s.total_ascent_m);
    put_u32(&mut h, 44, s.total_descent_m);
    put_i16(&mut h, 48, s.min_ele_m);
    put_i16(&mut h, 50, s.max_ele_m);
    put_u32(&mut h, 52, s.chunk_count);
    put_u32(&mut h, 56, index_offset);
    put_u32(&mut h, 60, HEADER_FULL_LEN as u32); // data_offset
                                                 // Waypoint header extension: table offset and count.
    put_u32(&mut h, 112, wpt_offset);
    put_u16(&mut h, 116, s.waypoint_count);
    h
}

/// Does the path reverse direction at `b`? A collinear out-and-back has zero perpendicular
/// distance everywhere, so the chord test alone collapses the doubled-back stretch onto its
/// endpoints and loses its length. A reversal vertex is always kept.
fn reverses(a: Cand, b: Cand, c: Cand) -> bool {
    let cl = cos_lat(a.lat);
    let (ux, uy) = delta_m((a.lon, a.lat), (b.lon, b.lat), cl);
    let (vx, vy) = delta_m((b.lon, b.lat), (c.lon, c.lat), cl);
    ux * vx + uy * vy < 0.0
}

/// Perpendicular distance (m) from point `p` to the chord `a` to `c`, in local-equirectangular
/// metres. Unlike the matcher's `project_to_segment`, this does not clamp to the segment.
fn perp_dist_m(a: Cand, c: Cand, p: Cand) -> f32 {
    let cl = cos_lat(a.lat);
    let (cx, cy) = delta_m((a.lon, a.lat), (c.lon, c.lat), cl);
    let (px, py) = delta_m((a.lon, a.lat), (p.lon, p.lat), cl);
    let len2 = cx * cx + cy * cy;
    if len2 <= 1e-9 {
        return libm::sqrtf(px * px + py * py);
    }
    (cx * py - cy * px).abs() / libm::sqrtf(len2)
}

fn grow(b: Option<BBox>, lon: i32, lat: i32) -> BBox {
    match b {
        None => BBox { min_lon: lon, min_lat: lat, max_lon: lon, max_lat: lat },
        Some(mut b) => {
            bbox_extend(&mut b, lon, lat);
            b
        }
    }
}

fn bbox_extend(bbox: &mut BBox, lon: i32, lat: i32) {
    bbox.min_lon = bbox.min_lon.min(lon);
    bbox.min_lat = bbox.min_lat.min(lat);
    bbox.max_lon = bbox.max_lon.max(lon);
    bbox.max_lat = bbox.max_lat.max(lat);
}

fn round_i16(m: f64) -> i16 {
    libm::round(m).clamp((i16::MIN + 1) as f64, i16::MAX as f64) as i16
}
