//! Streaming GPX conversion and the shared bounded OBCR emitter.
//! The emitter owns retained geometry, incoming facts, chunk seams and measured totals.

use heapless::Vec;

use crate::gpx::{GpxScanner, RawWaypoint, WptScanner};
use crate::reader::{ChunkMeta, MAX_POINTS_PER_CHUNK, MAX_ROUTE_CHUNKS, MAX_WAYPOINTS};
use crate::symbol::category_for_symbol;
use obc_elevation::DeadBand;
use obc_formats::io::{put_i16, put_i32, put_u16, put_u32, ByteSink, ByteSource, Error};
use obc_formats::obcr::{
    CHUNK_META_LEN, HEADER_FULL_LEN, MAGIC, NAME_CAP, POINT_RECORD_LEN, VERSION, WAYPOINT_CATEGORY_GENERIC,
    WAYPOINT_ELE_NONE, WAYPOINT_LEN, WAYPOINT_NAME_OFF,
};
use obc_map_scene::BBox;
use obc_map_scene::{cos_lat, delta_m, ground_dist_m};

/// Decimation tolerance: drop a vertex within this perpendicular distance of the chord.
const EPSILON_M: f32 = 1.0;
/// Force a kept geometry vertex at least this often, so a long near-straight run keeps shape
/// fidelity at real (not interpolated) points. (The stored-delta `int16` bound is guaranteed
/// unconditionally by [`MAX_SEGMENT_UDEG`] densification — even a segment with no candidate.)
const MAX_SPAN_M: f32 = 1200.0;
/// Largest stored per-vertex coordinate delta (µdeg). A longer segment is split with
/// interpolated vertices so `(x - px) as i16` never wraps — including a 2-point track whose one
/// segment has no intermediate candidate for the `MAX_SPAN_M` rule to keep. Mirrors the OBCM
/// packer's `MAX_SEGMENT` so both formats densify on the same threshold.
const MAX_SEGMENT_UDEG: i64 = 30_000;

/// Max bytes of one chunk's record body (`(points-1) × 7`).
const BODY_CAP: usize = (MAX_POINTS_PER_CHUNK - 1) * POINT_RECORD_LEN;

/// Stats computed during conversion (also written into the header).
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
}

/// Convert a GPX byte source into a `.obcr` written to `sink`, naming the route
/// `name`. Returns the computed [`RouteStats`].
pub fn gpx_to_obcr(src: &dyn ByteSource, name: &str, sink: &mut dyn ByteSink) -> Result<RouteStats, Error> {
    gpx_to_obcr_attributed(src, name, sink, None, |_, _| Ok(0))
}

/// Import with a bounded incoming-segment attribution provider. Plain GPX stays unknown.
pub fn gpx_to_obcr_attributed(
    src: &dyn ByteSource,
    name: &str,
    sink: &mut dyn ByteSink,
    map_source: Option<obc_formats::obcr::RouteSourceKey>,
    mut surface: impl FnMut((i32, i32), (i32, i32)) -> Result<u8, Error>,
) -> Result<RouteStats, Error> {
    let mut em = ObcrEmitter::new(sink)?;
    em.set_attribution_map(map_source);

    // Waypoint pass first (GPX carries `<wpt>` file-level, before the track): collect
    // up to MAX_WAYPOINTS into a bounded resident set, then place each on the track
    // during the main pass below. Scoped so the scanner's block buffer is gone before
    // the track scanner's exists — the passes are sequential, never co-resident.
    let mut wps: Vec<WpPlace, MAX_WAYPOINTS> = Vec::new();
    {
        let mut scan = WptScanner::new(src);
        while let Some(wp) = scan.next_waypoint()? {
            // The symbol → category mapping happens once, here: what's stored is the category, so
            // the freeform `<sym>` text never has to be carried past the import.
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
                break; // cap reached — keep the first MAX_WAYPOINTS
            }
        }
    }

    let mut scan = GpxScanner::new(src);

    let mut raw_index = 0;
    // The previous raw point, kept only for the waypoint offset's *direction of travel*.
    let mut prev_raw: Option<(i32, i32)> = None;

    while !wps.is_empty() {
        let Some(p) = scan.next_point()? else { break };

        // Waypoint placement: nearest **raw** track point wins; its cumulative distance is
        // the waypoint's position along the route. Matches the phone importer's nearest-point
        // (not segment-projection) placement so the two OBCR producers agree. The same
        // projection also yields the stored lateral offset: its magnitude is the distance to
        // that winning point, its sign which side of the direction of travel the waypoint fell.
        if !wps.is_empty() {
            let cl = cos_lat(p.lat);
            let here = (p.lon, p.lat);
            for w in wps.iter_mut() {
                // A waypoint whose best point was the track's *first* has no incoming segment to
                // take a heading from, so its sign waits one point for the outgoing one.
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
                        // Magnitude now, side on the next point (or, for a one-point track, never
                        // — `signed_offset_m`'s positive default stands).
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
/// ([`crate::nav`]): reserves the v4 header up front, feeds raw points through the
/// 1-step-lookahead decimator and the `int16`-delta densify guard into the chunk
/// [`Encoder`], then backfills the header once offsets and totals are known. Owns every
/// format/geometry invariant (bbox growth, start point, cumulative distance, chunk
/// seams) so the two OBCR producers stay byte-compatible by construction; the final encoded geometry owns all route facts.
pub(crate) struct ObcrEmitter {
    enc: Encoder,
    /// Cumulative raw-path distance in `f64`: each per-segment distance is a small `f32`
    /// (see `geo`), but a long route's running total needs the dynamic range — `f32`'s
    /// ~7 significant digits resolve a 300 km total to only ~3 cm and would drift over
    /// thousands of segments.
    cum_dist: f64,
    prev: Option<(i32, i32)>,
    bbox: Option<BBox>,
    start: (i32, i32),
    emitted: u32,
    // Decimation state (1-step lookahead).
    last_kept: Option<Cand>,
    pending: Option<Cand>,
    /// Elevation-detail keep threshold (m), `0` = off — see
    /// [`keep_elevation_detail`](ObcrEmitter::keep_elevation_detail).
    ele_keep_m: i16,
    surface: u8,
    flags: u8,
    map_source: Option<obc_formats::obcr::RouteSourceKey>,
}

impl ObcrEmitter {
    /// Reserve the v4 header on `sink`; the body follows immediately
    /// (`data_offset = HEADER_FULL_LEN`).
    pub(crate) fn new(sink: &mut dyn ByteSink) -> Result<ObcrEmitter, Error> {
        Self::begin(sink)?;
        Ok(Self::empty())
    }

    /// Construct in the owner's workspace before streaming starts. No sink write occurs here.
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
            flags: 0,
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

    pub(crate) fn set_attribution_map(&mut self, source: Option<obc_formats::obcr::RouteSourceKey>) {
        self.map_source = source;
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

    /// Feed one raw point: accumulate distance/bbox, then run the decimator — each kept
    /// point is emitted (densified) to the encoder.
    pub(crate) fn push(&mut self, sink: &mut dyn ByteSink, lon: i32, lat: i32, ele: i16) -> Result<(), Error> {
        // Distance from the previous raw point.
        if let Some(pr) = self.prev {
            self.cum_dist += ground_dist_m(pr, (lon, lat)) as f64;
        } else {
            self.start = (lon, lat);
        }
        self.prev = Some((lon, lat));
        self.bbox = Some(grow(self.bbox, lon, lat));

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

    /// Stored route geometry is already simplified. Keep its vertices and measure its
    /// output axis before a transform records a waypoint or seam anchor.
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

    /// Flush the trailing point, write the chunk index + waypoint table, and backfill the
    /// header. `Error::Empty` if no point was ever pushed. `wps` is the (already collected)
    /// waypoint set — pass an empty one for a waypoint-free route. Call once; after any error
    /// discard this stream. The owner keeps the emitter in place until its terminal phase.
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
        };

        let mut header = build_header(name, &bbox, self.start, index_offset, wpt_offset, &stats);
        header[5] |= self.flags;
        if let Some(source) = self.map_source {
            header[5] |= obc_formats::obcr::FLAG_ATTRIBUTION_MAP;
            source.encode(header[128..160].as_mut().try_into().unwrap());
        }
        sink.patch_at(0, &header)?;
        Ok(stats)
    }
}

/// A waypoint being placed: the raw `<wpt>` plus the best (squared) distance to any
/// raw track point seen so far and the cumulative route distance there. `pub(crate)`
/// only so the nav router can hand [`ObcrEmitter::finish`] an empty set and the
/// splicer can re-place stored waypoints.
pub(crate) struct WpPlace {
    wp: RawWaypoint,
    best_d2: f32,
    along_m: u32,
    /// The stored category byte (§4), mapped from `<sym>`/`<type>` at import and preserved
    /// verbatim across a splice.
    category_id: u8,
    /// Signed lateral offset from the route line, m (positive = right of travel). Recomputed
    /// whenever a nearer track point wins the placement; carried verbatim through a splice.
    lateral_offset_m: i16,
    /// The winning track point had no predecessor (it was the track's first), so the offset's
    /// magnitude is stored but its side still waits for the outgoing segment.
    sign_pending: bool,
    provenance: Option<obc_formats::obcr::WaypointProvenance>,
    raw_index: u32,
}

impl WpPlace {
    /// Re-place an already-stored [`Waypoint`](crate::reader::Waypoint) at a (possibly shifted)
    /// along-route distance — the splicer's constructor: placement is already decided, so the
    /// nearest-point search state is inert. The category byte and the lateral offset ride along
    /// unchanged: a splice only replaces the avoided span (whose waypoints are dropped), so every
    /// surviving waypoint still sits beside the very geometry its offset was measured against.
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

/// The 2-D cross product of the direction of travel `dir_a → dir_b` with the offset `at → wp`, in
/// the local-equirectangular metric (`cl = cos_lat`). Positive means `wp` lies to the **left** of
/// travel; only its sign is used.
fn cross(dir_a: (i32, i32), dir_b: (i32, i32), at: (i32, i32), wp: (i32, i32), cl: f32) -> f32 {
    let (ux, uy) = delta_m(dir_a, dir_b, cl);
    let (vx, vy) = delta_m(at, wp, cl);
    ux * vy - uy * vx
}

/// The stored lateral offset: `sqrt(d2)` metres carrying the side as its sign — negative left of
/// travel, positive right. Saturates at the `i16` range rather than wrapping, so a waypoint dropped
/// 40 km off route reads as "very far right", not "slightly left". A waypoint exactly on the line
/// of travel (`cross == 0`, including the undetermined one-point-track case) takes the positive
/// sign; at the magnitudes where the side is drawn at all, that case doesn't occur in practice.
fn signed_offset_m(d2: f32, cross: f32) -> i16 {
    let m = libm::roundf(libm::sqrtf(d2)).clamp(0.0, i16::MAX as f32) as i16;
    if cross > 0.0 {
        -m
    } else {
        m
    }
}

/// Sort the placed waypoints by position along the route and write the fixed-record
/// table (spec §4) at `offset` (right after the chunk index). Returns the table's file
/// offset for the header extension — 0 when there are no waypoints.
fn write_waypoints(sink: &mut dyn ByteSink, wps: &mut Vec<WpPlace, MAX_WAYPOINTS>, offset: u32) -> Result<u32, Error> {
    if wps.is_empty() {
        return Ok(0);
    }
    // Insertion sort by `along_m` (stable, N ≤ MAX_WAYPOINTS — no allocator).
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

/// A decimation candidate: a kept point with its cumulative stats.
#[derive(Debug, Clone, Copy)]
struct Cand {
    lon: i32,
    lat: i32,
    ele: i16,
    surface: u8,
    cum_d: u32,
}

/// Emit `c`, first inserting linearly-interpolated synthetic vertices so no stored
/// `(Δlon, Δlat)` exceeds the `int16` range. `prev` is the last-emitted vertex (the segment
/// start), or `None` for the very first point. Returns the count emitted (synthetic
/// intermediates + `c`) so the caller's running total stays exact.
///
/// `MAX_SPAN_M` only force-keeps an intermediate *raw* candidate between two kept vertices; a
/// single raw segment with no candidate (e.g. a 2-point export) would otherwise be stored as
/// one oversized delta that silently wraps `int16`. Splitting the span here makes the guard
/// candidate-independent, mirroring the OBCM packer's `densify` on `MAX_SEGMENT_UDEG`.
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
        let steps = max_dist / MAX_SEGMENT_UDEG + 1; // integer step count
        for step in 1..steps {
            enc.emit(sink, lerp(prev, c, step as f64 / steps as f64))?;
            emitted += 1;
        }
    }
    enc.emit(sink, c)?;
    Ok(emitted + 1)
}

/// A synthetic candidate fraction `t` (0..1) of the way from `a` to `b`, interpolating the
/// position, elevation and cumulative stats linearly.
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

/// Accumulates kept points into seam-sharing chunks, streaming each finished chunk's
/// body out and collecting its `ChunkMeta` in a bounded resident index.
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

    /// Flush the trailing chunk (skipping a lone seam point already in the prior chunk).
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
        sink.write(&body)?;
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
    // h[5] flags = 0, h[7] reserved = 0

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
                                                 // Waypoint header extension (§1.1): table offset + count; the rest reserved.
    put_u32(&mut h, 112, wpt_offset);
    put_u16(&mut h, 116, s.waypoint_count);
    h
}

/// Does the path reverse direction at `b` (the heading turns by more than 90°)? A perfectly
/// collinear out-and-back — a computed detour riding back to a junction, a turnaround at a
/// dead end — has zero perpendicular distance everywhere, so the chord test alone would
/// collapse the whole doubled-back stretch onto its endpoints and silently lose its length
/// from the geometry (#882). A reversal vertex is always kept.
fn reverses(a: Cand, b: Cand, c: Cand) -> bool {
    let cl = cos_lat(a.lat);
    let (ux, uy) = delta_m((a.lon, a.lat), (b.lon, b.lat), cl);
    let (vx, vy) = delta_m((b.lon, b.lat), (c.lon, c.lat), cl);
    ux * vx + uy * vy < 0.0
}

/// Perpendicular distance (m) from point `p` to the chord `a → c`, in local-equirectangular
/// meters. The decimator's straight-chord sibling of the matcher's clamped `project_to_segment`;
/// segment distance / projection live in [`geo`](crate::geo), shared with the elevation profile.
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

/// Expand `bbox` in place to include `(lon, lat)`.
fn bbox_extend(bbox: &mut BBox, lon: i32, lat: i32) {
    bbox.min_lon = bbox.min_lon.min(lon);
    bbox.min_lat = bbox.min_lat.min(lat);
    bbox.max_lon = bbox.max_lon.max(lon);
    bbox.max_lat = bbox.max_lat.max(lat);
}

fn round_i16(m: f64) -> i16 {
    libm::round(m).clamp((i16::MIN + 1) as f64, i16::MAX as f64) as i16
}
