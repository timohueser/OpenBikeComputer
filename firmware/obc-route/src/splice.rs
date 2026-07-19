//! Detour splice (#882): stream a new derived OBCR out of an original route and a planned
//! detour — `original[0..split_m] + detour + original[rejoin_m..end]` — through the shared
//! [`ObcrEmitter`], so the committed route is a completely ordinary OBCR the whole stack
//! (matcher, profile, climbs, waypoints) re-adopts with zero overlay awareness.
//!
//! Shape mirrors [`NavPlanner`](crate::nav::NavPlanner): a **resumable** [`Splicer`] owning the
//! ~9 kB emitter across steps (caller-owned object, never a stack local), one bounded unit of
//! work per [`step`](Splicer::step) — one decoded source chunk ([`SPLICE_CHUNKS_PER_STEP`]) —
//! plus a [`splice_detour`] one-shot for the headless sim and tests.
//!
//! Elevation: the nav graph has no DEM, so the detour's points arrive with `ele == 0`. They are
//! rewritten to a linear interpolation (in arc length) between the original route's elevations
//! at the two splice seams — no spike at either seam, at most `|Δele|` of monotone climb — while
//! head and tail keep their stored elevations verbatim. Ascent/descent are re-accumulated over
//! the spliced stream with the same [`DeadBand`] hysteresis as the GPX converter.
//!
//! Distance: the header total is overridden to `measured(head + seams + tail) + detour_len_m`
//! (the planner's summed raw edge lengths), so the committed route's displayed total is
//! consistent with the preview's cost figure — the same honesty convention as the nav emit's
//! total override; per-point cumulative distance stays the emitter's re-measured polyline.
//!
//! The two seams — `position_at(split_m)` → the detour's first point (the start-snap node) and
//! the detour's last point (the goal-snap node) → `position_at(rejoin_m)` — are single straight
//! segments of up to [`SNAP_RADIUS_M`](crate::nav::SNAP_RADIUS_M); the emitter measures them
//! into the cumulative distance like any other segment.

use heapless::Vec;

use crate::convert::{EmitStats, ObcrEmitter, RouteStats, WpPlace};
use crate::deadband::DeadBand;
use crate::geo::ground_dist_m;
use crate::reader::{
    decode_route_points_between, for_each_waypoint, RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK, MAX_WAYPOINTS,
};
use obc_formats::io::{ByteSink, Error};
use obc_formats::obcr::NAME_CAP;

/// Source chunks decoded per [`Splicer::step`] — the splice's pacing unit (one chunk ≈ one
/// bounded decode + a burst of emitter pushes), mirroring the search's miss budget philosophy.
pub const SPLICE_CHUNKS_PER_STEP: usize = 1;

/// The spliced route's name prefix. A re-spliced detour keeps its name unchanged instead of
/// stacking prefixes.
const NAME_PREFIX: &str = "Detour · ";

/// One [`Splicer::step`] outcome.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpliceStep {
    /// More work remains — step again next pass.
    Running,
    /// The spliced OBCR is complete (header patched); its [`RouteStats`].
    Done(RouteStats),
    /// The splice failed; the caller discards the sink's contents.
    Failed(Error),
}

/// The splice's coarse phase (one enum arm per bounded unit of work).
enum Phase {
    /// Read the two seam elevations, arm the emitter.
    Init,
    /// Stream `original[0..split_m]`, one chunk per step (real elevations).
    Head,
    /// Pre-measure the detour polyline length (the elevation lerp's denominator).
    Measure,
    /// Stream the detour, one chunk per step, rewriting elevations to the seam lerp.
    Detour,
    /// Stream `original[rejoin_m..]`, one chunk per step (real elevations).
    Tail,
    /// Re-place the original's waypoints and patch the header; terminal after this.
    Finish,
    Terminal(Result<RouteStats, Error>),
}

/// The resumable detour splicer. Construct with [`new`](Self::new), then call
/// [`step`](Self::step) with the same `orig`/`detour`/`sink` views each pass until it returns
/// [`SpliceStep::Done`]/[`Failed`](SpliceStep::Failed). Like the planner, this is a caller-owned
/// object (heap box on the host) — its one big field is the emitter.
pub struct Splicer {
    phase: Phase,
    split_m: u32,
    rejoin_m: u32,
    /// The planner's honest detour length (summed raw edge `length_m`) — the header-total
    /// override's detour term.
    detour_len_m: u32,
    name: heapless::String<NAME_CAP>,
    em: Option<ObcrEmitter>,
    /// Per-phase chunk cursors.
    head_k: usize,
    det_k: usize,
    tail_k: usize,
    /// Seam elevations sampled from the original route (Init).
    ele_split: i16,
    ele_rejoin: i16,
    /// Measured detour polyline length (Measure) and the emit pass's running position on it.
    det_total: f32,
    det_along: f32,
    /// Last point handed to the emitter (chunk-seam dedup) and the previous detour point (the
    /// arc-length accumulator).
    last_pushed: Option<(i32, i32)>,
    prev_det: Option<(i32, i32)>,
    /// The first tail point's cumulative distance in the spliced route — the tail waypoints'
    /// shift base; `None` while the tail hasn't started (or is empty).
    tail_first_along: Option<u32>,
    /// Elevation accounting over the spliced stream.
    elev: DeadBand<f64>,
    min_ele: i16,
    max_ele: i16,
}

impl Splicer {
    /// A splicer for `original[0..split_m] + detour + original[rejoin_m..]`. `detour_len_m` is
    /// the detour plan's [`RouteStats::total_distance_m`]; `orig_name` derives the spliced name
    /// (`"Detour · <name>"`, idempotent for an already-detoured name). Touches nothing until the
    /// first [`step`](Self::step).
    pub fn new(split_m: u32, rejoin_m: u32, detour_len_m: u32, orig_name: &str) -> Splicer {
        let mut name = heapless::String::new();
        if !orig_name.starts_with(NAME_PREFIX) {
            let _ = name.push_str(NAME_PREFIX);
        }
        for ch in orig_name.chars() {
            if name.push(ch).is_err() {
                break;
            }
        }
        Splicer {
            phase: Phase::Init,
            split_m,
            rejoin_m,
            detour_len_m,
            name,
            em: None,
            head_k: 0,
            det_k: 0,
            tail_k: 0,
            ele_split: 0,
            ele_rejoin: 0,
            det_total: 0.0,
            det_along: 0.0,
            last_pushed: None,
            prev_det: None,
            tail_first_along: None,
            elev: DeadBand::new(),
            min_ele: i16::MAX,
            max_ele: i16::MIN,
        }
    }

    /// Terminal-transition helper: latch and return the failure.
    fn fail(&mut self, e: Error) -> SpliceStep {
        self.phase = Phase::Terminal(Err(e));
        SpliceStep::Failed(e)
    }

    /// Run one bounded unit of splicing. `orig` is the route being detoured, `detour` the
    /// detour-only OBCR from the plan phase (in RAM), `sink` the spliced route's output —
    /// the caller passes the same three views every step.
    pub fn step(&mut self, orig: &RouteReader, detour: &RouteReader, sink: &mut dyn ByteSink) -> SpliceStep {
        match &self.phase {
            Phase::Init => {
                let (Some(es), Some(er)) = (orig.elevation_at(self.split_m), orig.elevation_at(self.rejoin_m)) else {
                    return self.fail(Error::BadOffset);
                };
                self.ele_split = es;
                self.ele_rejoin = er;
                match ObcrEmitter::new(sink) {
                    Ok(em) => self.em = Some(em),
                    Err(e) => return self.fail(e),
                }
                self.phase = Phase::Head;
                SpliceStep::Running
            }
            Phase::Head => {
                for _ in 0..SPLICE_CHUNKS_PER_STEP {
                    // A chunk intersects [0, split_m] iff it starts at or before the split.
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
                for _ in 0..SPLICE_CHUNKS_PER_STEP {
                    if self.det_k >= detour.chunks().len() {
                        // Denominator ready; restart the detour cursor for the emit pass.
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
                    if let Err(e) = self.push_detour_chunk(detour, self.det_k, sink) {
                        return self.fail(e);
                    }
                    self.det_k += 1;
                }
                SpliceStep::Running
            }
            Phase::Tail => {
                for _ in 0..SPLICE_CHUNKS_PER_STEP {
                    if self.tail_k >= orig.chunks().len() || self.rejoin_m >= orig.total_distance_m {
                        self.phase = Phase::Finish;
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

    /// Push one point into the emitter, maintaining the seam dedup and elevation accounting.
    fn push_point(&mut self, sink: &mut dyn ByteSink, lon: i32, lat: i32, ele: i16) -> Result<(), Error> {
        if self.last_pushed == Some((lon, lat)) {
            return Ok(()); // chunk-seam / hop-seam duplicate
        }
        self.min_ele = self.min_ele.min(ele);
        self.max_ele = self.max_ele.max(ele);
        self.elev.push(ele as f64);
        let em = self.em.as_mut().ok_or(Error::Empty)?;
        em.push(sink, lon, lat, ele, self.elev.ascent() as u32)?;
        self.last_pushed = Some((lon, lat));
        Ok(())
    }

    /// Stream one original-route chunk clipped to `[lo, hi]`, elevations verbatim. A chunk that
    /// misses the interval is a no-op (interval endpoints land mid-chunk on either side).
    /// `tail: true` records the first pushed point's spliced-route distance as the waypoint
    /// shift base ([`tail_first_along`](field@Splicer::tail_first_along)).
    ///
    /// `#[inline(never)]` — the ~2 kB decode buffer lives in this popped frame, never the step
    /// frame (the #419/#501 stack discipline).
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
        let Some(n) = decode_route_points_between(orig, k, lo, hi, &mut buf) else {
            return Ok(());
        };
        for p in buf[..n].iter() {
            self.push_point(sink, p.lon, p.lat, p.ele)?;
            if tail && self.tail_first_along.is_none() {
                let em = self.em.as_ref().ok_or(Error::Empty)?;
                self.tail_first_along = Some(em.cum_dist() as u32);
            }
        }
        Ok(())
    }

    /// Measure one detour chunk's polyline length with the same per-segment metric the emit
    /// pass accumulates — the elevation lerp's denominator must match its numerator.
    #[inline(never)]
    fn measure_detour_chunk(&mut self, detour: &RouteReader, k: usize) -> Result<f32, Error> {
        let mut buf = Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
        detour.decode_chunk(k, &mut buf)?;
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

    /// Stream one detour chunk, rewriting each point's elevation to the seam lerp at its
    /// arc-length position.
    #[inline(never)]
    fn push_detour_chunk(&mut self, detour: &RouteReader, k: usize, sink: &mut dyn ByteSink) -> Result<(), Error> {
        let mut buf = Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
        detour.decode_chunk(k, &mut buf)?;
        for p in buf.iter() {
            let c = (p.lon, p.lat);
            if let Some(prev) = self.prev_det {
                if prev == c {
                    continue; // chunk-seam duplicate: no arc advance, already pushed
                }
                self.det_along += ground_dist_m(prev, c);
            }
            self.prev_det = Some(c);
            let t = if self.det_total > 1e-3 { (self.det_along / self.det_total).clamp(0.0, 1.0) } else { 1.0 };
            let ele = lerp_ele(self.ele_split, self.ele_rejoin, t);
            self.push_point(sink, c.0, c.1, ele)?;
        }
        Ok(())
    }

    /// Re-place the original's waypoints and patch the header — the splice's last writes.
    ///
    /// `#[inline(never)]` — `Option::take` moves the ~9 kB emitter into a local; that temporary
    /// belongs in this popped frame, never the step frame (same rationale as the planner's
    /// `finish_emit`).
    #[inline(never)]
    fn finish_splice(&mut self, orig: &RouteReader, sink: &mut dyn ByteSink) -> Result<RouteStats, Error> {
        let em_total = self.em.as_ref().ok_or(Error::Empty)?.cum_dist() as f32;
        // Head + seams + tail measured by the emitter (`det_along` is exactly the emitted detour
        // portion — same points, same metric), detour replaced by the planner's honest length —
        // the preview's arithmetic, saturating like every stored distance.
        let override_total = ((em_total - self.det_along).max(0.0) as u32).saturating_add(self.detour_len_m);

        // Waypoints: head kept verbatim, skipped span dropped, tail shifted onto the spliced
        // distance axis. The shift base is the first tail point's spliced distance (or the
        // spliced end for a rejoin exactly at the route end).
        let tail_base = self.tail_first_along.unwrap_or(em_total as u32);
        let mut wps: Vec<WpPlace, MAX_WAYPOINTS> = Vec::new();
        let split_m = self.split_m;
        let rejoin_m = self.rejoin_m;
        for_each_waypoint(orig.source(), |w| {
            let along = if w.dist_along_m <= split_m {
                Some(w.dist_along_m)
            } else if w.dist_along_m < rejoin_m {
                None // on the avoided span
            } else {
                Some(tail_base.saturating_add(w.dist_along_m - rejoin_m))
            };
            if let Some(along) = along {
                let _ = wps.push(WpPlace::from_stored(w, along));
            }
        })?;

        if self.min_ele > self.max_ele {
            self.min_ele = 0;
            self.max_ele = 0;
        }
        let stats = EmitStats {
            min_ele_m: self.min_ele,
            max_ele_m: self.max_ele,
            ascent_m: self.elev.ascent() as u32,
            descent_m: self.elev.descent() as u32,
            total_distance_m: Some(override_total),
        };
        let em = self.em.take().ok_or(Error::Empty)?;
        em.finish(sink, &self.name, stats, &mut wps)
    }
}

/// Linear elevation between the two seam samples at arc fraction `t`.
fn lerp_ele(a: i16, b: i16, t: f32) -> i16 {
    libm::roundf(a as f32 + (b as f32 - a as f32) * t) as i16
}

/// One-shot convenience over [`Splicer`]: loop [`step`](Splicer::step) to completion — the
/// headless sim and the tests; interactive hosts step the splicer themselves.
pub fn splice_detour(
    orig: &RouteReader,
    detour: &RouteReader,
    split_m: u32,
    rejoin_m: u32,
    detour_len_m: u32,
    sink: &mut dyn ByteSink,
) -> Result<RouteStats, Error> {
    let mut sp = Splicer::new(split_m, rejoin_m, detour_len_m, orig.name());
    loop {
        match sp.step(orig, detour, sink) {
            SpliceStep::Running => {}
            SpliceStep::Done(stats) => return Ok(stats),
            SpliceStep::Failed(e) => return Err(e),
        }
    }
}
