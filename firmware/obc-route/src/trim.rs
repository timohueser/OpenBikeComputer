//! Resumable first-contact trim. Each step visits at most one source chunk.
use crate::convert::{ObcrEmitter, WpPlace};
use crate::geo::{inflated_bbox, project_to_segment};
use crate::reader::{
    decode_route_points_between_checked, RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK, MAX_WAYPOINTS,
};
use heapless::Vec;
use obc_elevation::ELE_DEADBAND_M;
use obc_formats::io::{ByteSink, Error};
use obc_map_scene::{cos_lat, ground_dist_m, BBox};

/// Per-point hug distance to the route tail (m). Two consecutive detour points this close to the
/// tail count as sustained contact, so a single-point crossing or a bridge overpass never
/// triggers.
pub(crate) const TRIM_CONTACT_M: f32 = 25.0;

/// How far past `target_m` the tail is materialized when looking for the detour's first contact
/// with it (m). An A* approach that rides the route backwards covers at most a few hundred metres
/// of tail, so this window is generous.
pub(crate) const TRIM_LOOKAHEAD_M: u32 = 1_500;

/// Max resident tail sample points. A longer window widens its stride to fit, so this is a hard
/// cap by construction.
const TRIM_TAIL_MAX_PTS: usize = 128;

/// Along-tail sampling interval floor (m). It is finer than the contact radius, so the chord
/// between samples never hides a point that is on the tail.
const TRIM_MIN_SAMPLE_M: f32 = 20.0;

/// A trim that advances the rejoin no further than this past `target_m`, and contacts the tail
/// only at the detour's final pair, is a no-op. Every plan's landing hugs the tail near the goal,
/// so trimming there rewrites the bytes for nothing.
const TRIM_NOOP_M: u32 = 30;

/// The result of [`trim_detour_to_tail`] when the detour is advanced to its first sustained tail
/// contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrimOutcome {
    /// The rejoin distance on the original route. It is always at least `target_m`.
    pub rejoin_m: u32,
    /// The trimmed detour's length in measured polyline metres, not the planner's summed edge
    /// `length_m`. The planner has no summed length for the shortened form.
    pub detour_len_m: u32,
    /// The trimmed detour's own dead-banded ascent (m) over its kept sampled heights. `0` when
    /// the plan carried no elevation.
    pub ascent_m: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimStep {
    Running,
    Done(Option<TrimOutcome>),
    Failed(Error),
}

#[derive(Clone, Copy)]
enum Phase {
    Tail,
    Contact,
    Begin,
    Emit,
    Finish,
    Terminal(TrimStep),
}

type TailHit = (usize, f32, f32);
struct Tail {
    pts: Vec<(i32, i32, u32), TRIM_TAIL_MAX_PTS>,
    bbox: BBox,
    cl: f32,
}
impl Tail {
    /// The nearest tail segment to `p`: `(segment index, t along it, cross-track distance m)`, or
    /// `None` when `p` is outside the inflated bbox. Requires `pts.len() >= 2`.
    fn nearest(&self, p: (i32, i32)) -> Option<TailHit> {
        if !self.bbox_contains(p) {
            return None;
        }
        let mut best: Option<TailHit> = None;
        for i in 0..self.pts.len() - 1 {
            let a = (self.pts[i].0, self.pts[i].1);
            let b = (self.pts[i + 1].0, self.pts[i + 1].1);
            let (t, d) = project_to_segment(a, b, p, self.cl);
            if best.is_none_or(|(_, _, bd)| d < bd) {
                best = Some((i, t, d));
            }
        }
        best
    }

    /// Interpolate the along-route progress at fraction `t` of tail segment `seg`.
    fn progress_at(&self, seg: usize, t: f32) -> u32 {
        let p0 = self.pts[seg].2 as f32;
        let p1 = self.pts[seg + 1].2 as f32;
        libm::roundf(p0 + (p1 - p0) * t) as u32
    }

    fn bbox_contains(&self, p: (i32, i32)) -> bool {
        p.0 >= self.bbox.min_lon && p.0 <= self.bbox.max_lon && p.1 >= self.bbox.min_lat && p.1 <= self.bbox.max_lat
    }
}

/// First sustained contact advances the rejoin. A final landing near the target is a no-op and
/// leaves the sink untouched. The caller passes the same source views and sink through all steps,
/// and discards the sink on failure.
pub struct Trimmer {
    phase: Phase,
    target_m: u32,
    has_elevation: bool,
    tail: Tail,
    chunk: usize,
    distinct: usize,
    previous: Option<(usize, Option<TailHit>)>,
    last_seen: Option<(i32, i32)>,
    since_kept: f32,
    arc: f32,
    trim_index: usize,
    rejoin_m: u32,
    emitter: ObcrEmitter,
}

impl Trimmer {
    /// An approach is never trimmed: it has no tail to meet before the start.
    pub fn new(leg: crate::splice::Leg, target_m: u32, has_elevation: bool) -> Self {
        Self {
            phase: match leg {
                crate::splice::Leg::Approach => Phase::Terminal(TrimStep::Done(None)),
                crate::splice::Leg::Detour => Phase::Tail,
            },
            target_m,
            has_elevation,
            tail: Tail { pts: Vec::new(), bbox: BBox { min_lon: 0, max_lon: 0, min_lat: 0, max_lat: 0 }, cl: 1.0 },
            chunk: 0,
            distinct: 0,
            previous: None,
            last_seen: None,
            since_kept: 0.0,
            arc: 0.0,
            trim_index: 0,
            rejoin_m: target_m,
            emitter: ObcrEmitter::empty(),
        }
    }

    pub fn step(&mut self, orig: &RouteReader, detour: &RouteReader, sink: &mut dyn ByteSink) -> TrimStep {
        match self.advance(orig, detour, sink) {
            Ok(TrimStep::Running) => TrimStep::Running,
            Ok(done) => {
                self.phase = Phase::Terminal(done);
                done
            }
            Err(error) => {
                let done = TrimStep::Failed(error);
                self.phase = Phase::Terminal(done);
                done
            }
        }
    }

    fn advance(
        &mut self,
        orig: &RouteReader,
        detour: &RouteReader,
        sink: &mut dyn ByteSink,
    ) -> Result<TrimStep, Error> {
        let mut buf = Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
        match self.phase {
            Phase::Tail => {
                let lo = self.target_m.min(orig.total_distance_m);
                let hi = self.target_m.saturating_add(TRIM_LOOKAHEAD_M).min(orig.total_distance_m);
                if self.chunk < orig.chunks().len() && lo < hi {
                    let k = self.chunk;
                    self.chunk += 1;
                    let chunk_hi = orig.chunks().get(k + 1).map_or(orig.total_distance_m, |c| c.cum_distance_m);
                    if chunk_hi < lo || orig.chunks()[k].cum_distance_m > hi {
                        return Ok(TrimStep::Running);
                    }
                    let stride = ((hi - lo) as f32 / (TRIM_TAIL_MAX_PTS - 1) as f32).max(TRIM_MIN_SAMPLE_M);
                    if let Some(n) = decode_route_points_between_checked(orig, k, lo, hi, &mut buf)? {
                        for p in &buf[..n] {
                            let p = (p.lon, p.lat);
                            if self.tail.pts.is_empty() {
                                self.tail.cl = cos_lat(p.1);
                                let _ = self.tail.pts.push((p.0, p.1, self.target_m));
                                self.last_seen = Some(p);
                                continue;
                            }
                            let d = ground_dist_m(self.last_seen.unwrap_or(p), p);
                            self.arc += d;
                            self.since_kept += d;
                            self.last_seen = Some(p);
                            if self.since_kept >= stride && !self.tail.pts.is_full() {
                                let _ = self.tail.pts.push((p.0, p.1, self.target_m.saturating_add(self.arc as u32)));
                                self.since_kept = 0.0;
                            }
                        }
                    }
                    return Ok(TrimStep::Running);
                }
                if let Some(end) = self.last_seen {
                    let point = (end.0, end.1, self.target_m.saturating_add(self.arc as u32));
                    if self.tail.pts.last().map(|&(x, y, _)| (x, y)) != Some(end) {
                        if self.tail.pts.is_full() {
                            let n = self.tail.pts.len();
                            self.tail.pts[n - 1] = point;
                        } else {
                            let _ = self.tail.pts.push(point);
                        }
                    }
                }
                if self.tail.pts.len() < 2 {
                    return Ok(TrimStep::Done(None));
                }
                self.tail.bbox =
                    inflated_bbox(self.tail.pts.iter().map(|&(x, y, _)| (x, y)), self.tail.cl, TRIM_CONTACT_M);
                self.chunk = 0;
                self.phase = Phase::Contact;
            }
            Phase::Contact => {
                if self.chunk == detour.chunks().len() {
                    return Ok(TrimStep::Done(None));
                }
                detour.decode_chunk(self.chunk, &mut buf)?;
                let skip = usize::from(self.chunk > 0);
                self.chunk += 1;
                for p in buf.iter().skip(skip) {
                    let near = self.tail.nearest((p.lon, p.lat));
                    if let Some((index, Some((seg, t, d)))) = self.previous {
                        if d <= TRIM_CONTACT_M && near.is_some_and(|(_, _, d)| d <= TRIM_CONTACT_M) {
                            self.trim_index = index;
                            self.rejoin_m = self.tail.progress_at(seg, t).max(self.target_m);
                            let last_index = detour
                                .chunks()
                                .iter()
                                .map(|c| c.point_count as usize)
                                .sum::<usize>()
                                .saturating_sub(detour.chunks().len());
                            if index + 1 >= last_index && self.rejoin_m.saturating_sub(self.target_m) <= TRIM_NOOP_M {
                                return Ok(TrimStep::Done(None));
                            }
                            self.phase = Phase::Begin;
                            return Ok(TrimStep::Running);
                        }
                    }
                    self.previous = Some((self.distinct, near));
                    self.distinct += 1;
                }
            }
            Phase::Begin => {
                if detour.visit_descriptor()?.is_some() {
                    return Err(Error::BadOffset);
                }
                ObcrEmitter::begin(sink)?;
                self.emitter.set_attribution_map(detour.attribution_map()?);
                self.emitter.set_flags(
                    (if detour.has_unresolved_avoidance() { obc_formats::obcr::FLAG_UNRESOLVED_AVOIDANCE } else { 0 })
                        | if detour.is_assistant_candidate() { obc_formats::obcr::FLAG_ASSISTANT_CANDIDATE } else { 0 },
                );
                if self.has_elevation {
                    self.emitter.keep_elevation_detail(ELE_DEADBAND_M as i16);
                }
                self.chunk = 0;
                self.distinct = 0;
                self.phase = Phase::Emit;
            }
            Phase::Emit => {
                detour.decode_chunk(self.chunk, &mut buf)?;
                let skip = usize::from(self.chunk > 0);
                self.chunk += 1;
                for p in buf.iter().skip(skip) {
                    let ele = p.ele;
                    self.emitter.set_surface(p.surface);
                    self.emitter.set_elevation_incomplete(p.elevation_incomplete);
                    self.emitter.push_retained(sink, p.lon, p.lat, ele)?;
                    if self.distinct == self.trim_index {
                        self.phase = Phase::Finish;
                        break;
                    }
                    self.distinct += 1;
                }
            }
            Phase::Finish => {
                let stats = self.emitter.finish(sink, detour.name(), &mut Vec::<WpPlace, MAX_WAYPOINTS>::new())?;
                return Ok(TrimStep::Done(Some(TrimOutcome {
                    rejoin_m: self.rejoin_m,
                    detour_len_m: stats.total_distance_m,
                    ascent_m: stats.total_ascent_m,
                })));
            }
            Phase::Terminal(done) => return Ok(done),
        }
        Ok(TrimStep::Running)
    }
}

/// One-shot host convenience over the same bounded phases the board uses.
///
/// Must stay `#[inline(never)]`: the trimmer and its decode buffer stay out of the caller's frame.
#[inline(never)]
pub fn trim_detour_to_tail(
    leg: crate::splice::Leg,
    orig: &RouteReader,
    detour: &RouteReader,
    target_m: u32,
    has_elevation: bool,
    sink: &mut dyn ByteSink,
) -> Result<Option<TrimOutcome>, Error> {
    let mut trim = Trimmer::new(leg, target_m, has_elevation);
    loop {
        match trim.step(orig, detour, sink) {
            TrimStep::Running => {}
            TrimStep::Done(outcome) => return Ok(outcome),
            TrimStep::Failed(error) => return Err(error),
        }
    }
}
