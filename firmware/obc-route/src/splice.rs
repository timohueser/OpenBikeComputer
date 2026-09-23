//! Resumable detour splice through the shared OBCR emitter.
//!
//! Valid sampled detour elevations are aligned to the seam elevations by a linear residual blend.
//! Missing heights stay unknown. Output totals measure the retained geometry.

pub use crate::trim::trim_detour_to_tail;

use heapless::Vec;

use crate::convert::{ObcrEmitter, RouteStats, WpPlace};
use crate::reader::{
    decode_route_points_between_checked, RoutePoint, RouteReader, WaypointCursor, MAX_POINTS_PER_CHUNK, MAX_WAYPOINTS,
};
use obc_elevation::ELE_DEADBAND_M;
use obc_formats::io::{ByteSink, Error};
use obc_formats::obcr::NAME_CAP;
use obc_map_scene::ground_dist_m;

/// Source chunks decoded per [`Splicer::step`]: one bounded decode and a burst of emitter pushes.
pub(crate) const SPLICE_CHUNKS_PER_STEP: usize = 1;

/// The spliced route's name prefix. A re-spliced detour keeps its name unchanged instead of
/// stacking prefixes.
const NAME_PREFIX: &str = "Detour · ";
const APPROACH_PREFIX: &str = "To start · ";
const JOIN_PREFIX: &str = "To route · ";
const REST_PREFIX: &str = "From stop · ";

/// The name of the route a derived route was built on: `name` without its detour, approach or rest
/// prefix.
pub fn original_name(name: &str) -> &str {
    [NAME_PREFIX, APPROACH_PREFIX, JOIN_PREFIX, REST_PREFIX]
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
        .unwrap_or(name)
}

/// What a planned leg does to the route it joins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leg {
    /// Leaves the route, skips a span of it and rejoins it.
    Detour,
    /// Leads to a join point. The route follows it from `rejoin_m`.
    Approach,
    /// The rest of the previous trip day: its stored route over `[from_m, to_m]`, verbatim. The
    /// route follows it from its join point.
    Rest { from_m: u32, to_m: u32 },
}

impl Leg {
    /// The leg comes before the route: no head of the route stays, and the route's waypoints move
    /// behind the leg.
    fn leads_in(self) -> bool {
        !matches!(self, Leg::Detour)
    }
}

/// The height move (m) that forces the emitter to keep a vertex once the detour carries sampled
/// terrain. It matches [`ELE_DEADBAND_M`], the band the nav emit and the GPX converter integrate
/// at: kept vertices and vertices booked by the dead-band must be the same set, or an export of
/// the spliced route re-imports with a different climb than its header.
const ELE_SPLICE_KEEP_M: i16 = ELE_DEADBAND_M as i16;

/// One [`Splicer::step`] outcome.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpliceStep {
    Running,
    Done(RouteStats),
    /// The caller discards the sink's contents.
    Failed(Error),
}

/// The splice's coarse phase, one arm per bounded unit of work.
enum Phase {
    /// Read one seam elevation per step, then arm the emitter after the rejoin.
    Init,
    Rejoin,
    /// Stream `original[0..split_m]`, one chunk per step.
    Head,
    /// Pre-measure the detour polyline length, the blend's denominator, and read its first and
    /// last sampled heights, the blend's two residuals.
    Measure,
    /// Stream the detour, one chunk per step, offsetting each sampled elevation by the blended
    /// seam residual.
    Detour,
    /// Stream `original[rejoin_m..]`, one chunk per step.
    Tail,
    /// Re-place one stored waypoint per step, up to the existing cap.
    Waypoints,
    /// Write the bounded index and waypoint table, then patch the header.
    Finish,
    Terminal(Result<RouteStats, Error>),
}

/// The resumable detour splicer. Call [`step`](Self::step) with the same `orig`, `detour` and
/// `sink` views each pass until it returns [`SpliceStep::Done`] or
/// [`Failed`](SpliceStep::Failed). The caller owns the object; its one big field is the emitter.
pub struct Splicer {
    phase: Phase,
    leg: Leg,
    adds_avoidance: bool,
    assistant_candidate: bool,
    split_m: u32,
    rejoin_m: u32,
    name: heapless::String<NAME_CAP>,
    em: ObcrEmitter,
    /// Per-phase chunk cursors.
    head_k: usize,
    det_k: usize,
    tail_k: usize,
    /// Seam elevations read off the original route.
    ele_split: i16,
    ele_rejoin: i16,
    /// Stored endpoint elevations used to align the detour's datum to the original route.
    det_ele_first: i16,
    det_ele_last: i16,
    /// Valid seam elevation minus the corresponding detour endpoint elevation.
    res_start: f32,
    res_end: f32,
    /// Measured detour polyline length and the emit pass's running position on it.
    det_total: f32,
    det_along: f32,
    /// Last point handed to the emitter, for the seam dedup, and the previous detour point, for
    /// the arc-length accumulator.
    last_pushed: Option<(i32, i32)>,
    prev_det: Option<(i32, i32)>,
    /// The first tail point's cumulative distance in the spliced route, which is the shift base
    /// for the tail waypoints. `None` while the tail has not started or is empty.
    tail_first_along: Option<u32>,
    waypoints: Vec<WpPlace, MAX_WAYPOINTS>,
    waypoint_cursor: Option<WaypointCursor>,
}

impl Splicer {
    /// A splicer for `original[0..split_m] + detour + original[rejoin_m..]`. The stored point
    /// facts and the final geometry determine the output, so the distance and elevation hints in
    /// the call shape are unused. `orig_name` supplies the derived route name.
    ///
    /// A [`Leg::Approach`] leads to `rejoin_m`, then follows the route from there. It adds no
    /// avoidance. One height offset aligns its end with the join point. Tail waypoints follow it.
    ///
    /// A [`Leg::Rest`] is a stored route, so its heights stay as stored. The route follows from
    /// `rejoin_m`, and the output is a built day. The rest's waypoints are not kept.
    pub fn new(
        leg: Leg,
        split_m: u32,
        rejoin_m: u32,
        _detour_len_m: u32,
        _detour_has_elevation: bool,
        orig_name: &str,
    ) -> Splicer {
        let (split_m, rejoin_m) = match leg {
            Leg::Detour => (split_m, rejoin_m),
            Leg::Approach => (0, rejoin_m),
            Leg::Rest { .. } => (0, rejoin_m),
        };
        let mut name = heapless::String::new();
        let prefix = match leg {
            Leg::Detour => Some(NAME_PREFIX),
            Leg::Approach => Some(if rejoin_m == 0 { APPROACH_PREFIX } else { JOIN_PREFIX }),
            Leg::Rest { .. } => Some(REST_PREFIX),
        };
        if let Some(prefix) = prefix.filter(|_| {
            ![NAME_PREFIX, APPROACH_PREFIX, JOIN_PREFIX, REST_PREFIX].iter().any(|prefix| orig_name.starts_with(prefix))
        }) {
            let _ = name.push_str(prefix);
        }
        for ch in orig_name.chars() {
            if name.push(ch).is_err() {
                break;
            }
        }
        Splicer {
            leg,
            adds_avoidance: leg == Leg::Detour,
            assistant_candidate: false,
            phase: Phase::Init,
            split_m,
            rejoin_m,
            name,
            em: ObcrEmitter::empty(),
            head_k: 0,
            det_k: 0,
            tail_k: 0,
            ele_split: 0,
            ele_rejoin: 0,
            det_ele_first: 0,
            det_ele_last: 0,
            res_start: 0.0,
            res_end: 0.0,
            det_total: 0.0,
            det_along: 0.0,
            last_pushed: None,
            prev_det: None,
            tail_first_along: None,
            waypoints: Vec::new(),
            waypoint_cursor: None,
        }
    }

    /// Visit composition keeps the existing flags and adds no unresolved avoidance.
    pub fn set_assistant_candidate(&mut self) {
        self.assistant_candidate = true;
    }
    pub fn set_visit_composition(&mut self) {
        self.assistant_candidate = true;
        self.adds_avoidance = false;
    }

    fn fail(&mut self, e: Error) -> SpliceStep {
        self.phase = Phase::Terminal(Err(e));
        SpliceStep::Failed(e)
    }

    /// Run one bounded unit of splicing. `orig` is the route being detoured, `detour` the
    /// detour-only OBCR from the plan phase, and `sink` the spliced route's output.
    pub fn step(&mut self, orig: &RouteReader, detour: &RouteReader, sink: &mut dyn ByteSink) -> SpliceStep {
        match &self.phase {
            Phase::Init => {
                match (orig.visit_descriptor(), detour.visit_descriptor()) {
                    (Ok(None), Ok(None)) => {}
                    _ => return self.fail(Error::BadOffset),
                }
                let ele = orig.elevation_at(self.split_m).unwrap_or(i16::MIN);
                self.ele_split = ele;
                self.phase = Phase::Rejoin;
                SpliceStep::Running
            }
            Phase::Rejoin => {
                let ele = orig.elevation_at(self.rejoin_m).unwrap_or(i16::MIN);
                self.ele_rejoin = ele;
                if let Err(error) = ObcrEmitter::begin(sink) {
                    return self.fail(error);
                }
                let original_map = match orig.attribution_map() {
                    Ok(m) => m,
                    Err(e) => return self.fail(e),
                };
                let detour_map = match detour.attribution_map() {
                    Ok(m) => m,
                    Err(e) => return self.fail(e),
                };
                self.em.set_attribution_map(if original_map == detour_map { original_map } else { None });
                self.em.set_bike_type(orig.bike_type());
                self.em.set_flags(
                    if self.adds_avoidance || orig.has_unresolved_avoidance() || detour.has_unresolved_avoidance() {
                        obc_formats::obcr::FLAG_UNRESOLVED_AVOIDANCE
                    } else {
                        0
                    } | if self.assistant_candidate { obc_formats::obcr::FLAG_ASSISTANT_CANDIDATE } else { 0 }
                        | obc_formats::obcr::FLAG_TEMPORARY
                        | if matches!(self.leg, Leg::Rest { .. }) { obc_formats::obcr::FLAG_BUILT_DAY } else { 0 },
                );
                // Preserve the sampled heights the planner densified.
                if detour.has_elevation() {
                    self.em.keep_elevation_detail(ELE_SPLICE_KEEP_M);
                }
                self.phase = Phase::Head;
                SpliceStep::Running
            }
            Phase::Head => {
                for _ in 0..SPLICE_CHUNKS_PER_STEP {
                    // A chunk intersects [0, split_m] only if it starts at or before the split.
                    let intersects = orig.chunks().get(self.head_k).is_some_and(|cm| cm.cum_distance_m <= self.split_m);
                    if !intersects || self.split_m == 0 {
                        self.phase = Phase::Measure;
                        return SpliceStep::Running;
                    }
                    if let Err(e) = self.push_orig_chunk(orig, self.head_k, 0, self.split_m, sink, false) {
                        return self.fail(e);
                    }
                    self.head_k += 1;
                }
                SpliceStep::Running
            }
            Phase::Measure => {
                if let Leg::Rest { .. } = self.leg {
                    self.phase = Phase::Detour;
                    return SpliceStep::Running;
                }
                for _ in 0..SPLICE_CHUNKS_PER_STEP {
                    if self.det_k >= detour.chunks().len() {
                        let (s0, s1) = (self.det_ele_first, self.det_ele_last);
                        self.res_end = f32::from(self.ele_rejoin) - f32::from(s1);
                        // An approach starts off the route, so only its end has a seam to meet.
                        self.res_start = if self.leg == Leg::Approach {
                            self.res_end
                        } else {
                            f32::from(self.ele_split) - f32::from(s0)
                        };
                        // Restart the detour cursor for the emit pass.
                        self.det_k = 0;
                        self.prev_det = None;
                        self.phase = Phase::Detour;
                        return SpliceStep::Running;
                    }
                    match self.measure_detour_chunk(detour, self.det_k) {
                        Ok(len) => self.det_total += len,
                        Err(e) => return self.fail(e),
                    }
                    self.det_k += 1;
                }
                SpliceStep::Running
            }
            Phase::Detour => {
                for _ in 0..SPLICE_CHUNKS_PER_STEP {
                    if self.det_k >= detour.chunks().len() {
                        self.phase = Phase::Tail;
                        // First tail chunk: the earliest one reaching past the rejoin point.
                        let chunks = orig.chunks();
                        self.tail_k = (0..chunks.len())
                            .find(|&k| {
                                let hi = chunks.get(k + 1).map_or(orig.total_distance_m, |next| next.cum_distance_m);
                                hi >= self.rejoin_m
                            })
                            .unwrap_or(chunks.len());
                        return SpliceStep::Running;
                    }
                    let pushed = match self.leg {
                        Leg::Rest { from_m, to_m } if from_m < to_m => {
                            self.push_orig_chunk(detour, self.det_k, from_m, to_m, sink, false)
                        }
                        Leg::Rest { .. } => Ok(()),
                        _ => self.push_detour_chunk(detour, self.det_k, sink),
                    };
                    if let Err(e) = pushed {
                        return self.fail(e);
                    }
                    self.det_k += 1;
                }
                SpliceStep::Running
            }
            Phase::Tail => {
                for _ in 0..SPLICE_CHUNKS_PER_STEP {
                    if self.tail_k >= orig.chunks().len() || self.rejoin_m >= orig.total_distance_m {
                        self.phase = Phase::Waypoints;
                        return SpliceStep::Running;
                    }
                    if let Err(e) =
                        self.push_orig_chunk(orig, self.tail_k, self.rejoin_m, orig.total_distance_m, sink, true)
                    {
                        return self.fail(e);
                    }
                    self.tail_k += 1;
                }
                SpliceStep::Running
            }
            Phase::Waypoints => {
                if self.waypoint_cursor.is_none() {
                    match WaypointCursor::new(orig.source()) {
                        Ok(cursor) => self.waypoint_cursor = Some(cursor),
                        Err(error) => return self.fail(error),
                    }
                    return SpliceStep::Running;
                }
                match self.waypoint_cursor.as_mut().unwrap().next(orig.source()) {
                    Ok(Some(w)) => {
                        let along = if !self.leg.leads_in() && w.dist_along_m <= self.split_m {
                            Some(w.dist_along_m)
                        } else if w.dist_along_m < self.rejoin_m {
                            None
                        } else {
                            let tail_base = self.tail_first_along.unwrap_or_else(|| self.em.distance_m());
                            Some(if w.dist_along_m >= orig.total_distance_m {
                                self.em.distance_m()
                            } else {
                                tail_base.saturating_add(w.dist_along_m - self.rejoin_m).min(self.em.distance_m())
                            })
                        };
                        if let Some(along) = along {
                            let _ = self.waypoints.push(WpPlace::from_stored(&w, along));
                        }
                    }
                    Ok(None) => self.phase = Phase::Finish,
                    Err(error) => return self.fail(error),
                }
                SpliceStep::Running
            }
            Phase::Finish => match self.finish_splice(orig, sink) {
                Ok(stats) => {
                    self.phase = Phase::Terminal(Ok(stats));
                    SpliceStep::Done(stats)
                }
                Err(e) => self.fail(e),
            },
            Phase::Terminal(r) => match r {
                Ok(stats) => SpliceStep::Done(*stats),
                Err(e) => SpliceStep::Failed(*e),
            },
        }
    }

    /// Push one point into the emitter, keeping the seam dedup.
    fn push_point(&mut self, sink: &mut dyn ByteSink, lon: i32, lat: i32, ele: i16) -> Result<(), Error> {
        if self.last_pushed == Some((lon, lat)) {
            return Ok(()); // seam duplicate
        }
        self.em.push_retained(sink, lon, lat, ele)?;
        self.last_pushed = Some((lon, lat));
        Ok(())
    }

    /// Stream one chunk of a stored route clipped to `[lo, hi]`, elevations verbatim. A chunk that
    /// misses the interval is a no-op. `tail: true` records the first pushed point's
    /// spliced-route distance as the waypoint shift base.
    ///
    /// Must stay `#[inline(never)]`: the decode buffer lives in this popped frame, not the step frame.
    #[inline(never)]
    fn push_orig_chunk(
        &mut self,
        orig: &RouteReader,
        k: usize,
        lo: u32,
        hi: u32,
        sink: &mut dyn ByteSink,
        tail: bool,
    ) -> Result<(), Error> {
        let mut buf = Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
        let Some(n) = decode_route_points_between_checked(orig, k, lo, hi, &mut buf)? else {
            return Ok(());
        };
        for p in buf[..n].iter() {
            self.em.set_surface(p.surface);
            self.em.set_elevation_incomplete(p.elevation_incomplete);
            self.push_point(sink, p.lon, p.lat, p.ele)?;
            if tail && self.tail_first_along.is_none() {
                self.tail_first_along = Some(self.em.distance_m());
            }
        }
        Ok(())
    }

    /// Measure one detour chunk's polyline length with the same per-segment metric the emit pass
    /// accumulates, so the blend's denominator matches its numerator, and latch the detour's
    /// first and last sampled heights on the way past.
    ///
    /// Must stay `#[inline(never)]`: the decode buffer lives in this popped frame, not the step frame.
    #[inline(never)]
    fn measure_detour_chunk(&mut self, detour: &RouteReader, k: usize) -> Result<f32, Error> {
        let mut buf = Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
        detour.decode_chunk(k, &mut buf)?;
        if k == 0 {
            if let Some(first) = buf.first() {
                self.det_ele_first = first.ele;
            }
        }
        if let Some(last) = buf.last() {
            self.det_ele_last = last.ele;
        }
        let mut len = 0.0f32;
        for p in buf.iter() {
            let c = (p.lon, p.lat);
            if let Some(prev) = self.prev_det {
                if prev != c {
                    len += ground_dist_m(prev, c);
                }
            }
            self.prev_det = Some(c);
        }
        Ok(len)
    }

    /// Stream one detour chunk, offsetting each sampled elevation by the blended seam residual at
    /// its arc-length position. The two ends land exactly on the stored route's seam heights, and
    /// the interior keeps the shape the terrain gave it.
    ///
    /// Must stay `#[inline(never)]`: the decode buffer lives in this popped frame, not the step frame.
    #[inline(never)]
    fn push_detour_chunk(&mut self, detour: &RouteReader, k: usize, sink: &mut dyn ByteSink) -> Result<(), Error> {
        let mut buf = Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
        detour.decode_chunk(k, &mut buf)?;
        for p in buf.iter() {
            let c = (p.lon, p.lat);
            if let Some(prev) = self.prev_det {
                if prev == c {
                    continue; // seam duplicate: no arc advance, already pushed
                }
                self.det_along += ground_dist_m(prev, c);
            }
            self.prev_det = Some(c);
            let t = if self.det_total > 1e-3 { (self.det_along / self.det_total).clamp(0.0, 1.0) } else { 1.0 };
            // A missing endpoint blocks the datum alignment. Valid stored heights still survive.
            let ele = if p.elevation().is_none() {
                i16::MIN
            } else if self.ele_split == i16::MIN
                || self.ele_rejoin == i16::MIN
                || self.det_ele_first == i16::MIN
                || self.det_ele_last == i16::MIN
            {
                p.ele
            } else {
                blend_ele(p.ele, self.res_start, self.res_end, t)
            };
            self.em.set_surface(p.surface);
            self.em.set_elevation_incomplete(p.elevation_incomplete);
            self.push_point(sink, c.0, c.1, ele)?;
        }
        Ok(())
    }

    /// Write the collected waypoints and patch the header, the splice's last writes.
    ///
    /// Must stay `#[inline(never)]`: the index write scratch stays out of the per-step frame.
    #[inline(never)]
    fn finish_splice(&mut self, _orig: &RouteReader, sink: &mut dyn ByteSink) -> Result<RouteStats, Error> {
        self.em.finish(sink, &self.name, &mut self.waypoints)
    }
}

/// Align a sampled height with the two valid seam residuals.
fn blend_ele(sampled: i16, r0: f32, r1: f32, t: f32) -> i16 {
    libm::roundf(sampled as f32 + (r0 + (r1 - r0) * t)).clamp((i16::MIN + 1) as f32, i16::MAX as f32) as i16
}

/// One-shot convenience over [`Splicer`] for the headless sim and the tests. Interactive hosts
/// step the splicer themselves.
#[allow(clippy::too_many_arguments)]
pub fn splice_detour(
    leg: Leg,
    orig: &RouteReader,
    detour: &RouteReader,
    split_m: u32,
    rejoin_m: u32,
    detour_len_m: u32,
    detour_has_elevation: bool,
    sink: &mut dyn ByteSink,
) -> Result<RouteStats, Error> {
    let mut sp = Splicer::new(leg, split_m, rejoin_m, detour_len_m, detour_has_elevation, orig.name());
    loop {
        match sp.step(orig, detour, sink) {
            SpliceStep::Running => {}
            SpliceStep::Done(stats) => return Ok(stats),
            SpliceStep::Failed(e) => return Err(e),
        }
    }
}
