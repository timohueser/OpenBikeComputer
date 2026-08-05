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
//! **Elevation: sampled heights, seam-matched by a residual blend** (#1091). Since EL7 (epic
//! #1068) `plan_detour` samples the map's terrain at every emitted vertex, so a detour planned on
//! a terrain-carrying map arrives with **real heights** — and the splice keeps them. What it does
//! not keep is their absolute datum at the two joins: the stored route's heights and the raster's
//! need not agree there (a GPX imported from a phone or a barometric head unit is offset from the
//! DEM by canopy and pressure error), and a step at a seam would be a cliff in the profile and a
//! phantom climb in the totals. So the splice adds a **linear-in-arc-length blend of the two seam
//! residuals** to every detour point:
//!
//! ```text
//! r0 = route_ele(split_m)  − detour_ele(first)
//! r1 = route_ele(rejoin_m) − detour_ele(last)
//! ele(p) = detour_ele(p) + lerp(r0, r1, arc_fraction(p))
//! ```
//!
//! Both ends then equal the stored route's own seam heights **exactly** — the no-spike property
//! the old seam-to-seam lerp bought — while the interior keeps the terrain's real shape: a detour
//! over a rise reads as a rise, not as a straight ramp.
//!
//! **The degrade is an identity, not an approximation.** Whether the detour carried heights at
//! all is an *explicit* signal (`detour_has_elevation`, from the plan's
//! [`RouteStats::has_elevation`](crate::RouteStats) — EL7's `EleFill::seen`), never a guess at the
//! values, because `0 m` is a real height. When it is false the blend runs with a sampled height
//! of `0` and the residuals `(route_ele(split_m), route_ele(rejoin_m))`, which is *arithmetically*
//! the pre-#1091 seam-to-seam lerp — same expression, same rounding, same bytes (pinned by
//! `splice_without_detour_elevation_is_the_old_seam_lerp`).
//!
//! Head and tail keep their stored elevations verbatim, and ascent/descent are re-accumulated over
//! the **final** point stream with the same [`DeadBand`] hysteresis at the same threshold the GPX
//! converter and the nav emit use — so a spliced route's climb is as real as a plain planned
//! route's, not a patched-up sum of two.
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
use crate::geo::{inflated_bbox, project_to_segment};
use crate::reader::{
    decode_route_points_between, for_each_waypoint, RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK, MAX_WAYPOINTS,
};
use obc_elevation::{DeadBand, ELE_DEADBAND_M};
use obc_formats::io::{ByteSink, Error};
use obc_formats::obcr::NAME_CAP;
use obc_map_scene::BBox;
use obc_map_scene::{cos_lat, ground_dist_m, ground_dist_m_cl};

/// Source chunks decoded per [`Splicer::step`] — the splice's pacing unit (one chunk ≈ one
/// bounded decode + a burst of emitter pushes), mirroring the search's miss budget philosophy.
pub(crate) const SPLICE_CHUNKS_PER_STEP: usize = 1;

/// The spliced route's name prefix. A re-spliced detour keeps its name unchanged instead of
/// stacking prefixes.
const NAME_PREFIX: &str = "Detour · ";

/// The height move (m) that forces the splice's emitter to keep a vertex once the detour carries
/// sampled terrain (#1091) — the same [`ELE_DEADBAND_M`] the nav emit and the GPX converter
/// integrate at, for the same reason: "kept" and "booked by the dead-band" must be the same set of
/// vertices, or an export of the spliced route re-imports with a different climb than its header.
const ELE_SPLICE_KEEP_M: i16 = ELE_DEADBAND_M as i16;

/// One [`Splicer::step`] outcome.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SpliceStep {
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
    /// Pre-measure the detour polyline length (the residual blend's denominator) and read its
    /// first/last sampled heights (the blend's two residuals).
    Measure,
    /// Stream the detour, one chunk per step, offsetting each sampled elevation by the blended
    /// seam residual.
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
pub(crate) struct Splicer {
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
    /// Seam elevations read off the original route (Init).
    ele_split: i16,
    ele_rejoin: i16,
    /// Did the detour plan's terrain actually answer (`RouteStats::has_elevation`, EL7's
    /// `EleFill::seen`)? Frozen at construction — the splice never inspects the detour's stored
    /// heights to decide this, because `0 m` is a real height.
    det_sampled: bool,
    /// The detour's first/last **sampled** heights, read in Measure — the two seam residuals'
    /// subtrahends. Meaningless (and unread) while `det_sampled` is false.
    det_ele_first: i16,
    det_ele_last: i16,
    /// The blend's endpoint residuals, resolved once when Measure completes: `route seam − detour
    /// sample` at the start and the end. With no detour elevation these degrade to the two seam
    /// heights themselves, which is exactly the pre-#1091 seam lerp.
    res_start: f32,
    res_end: f32,
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
    /// the detour plan's [`RouteStats::total_distance_m`]; `detour_has_elevation` its
    /// [`RouteStats::has_elevation`] — the explicit "the terrain answered" bit that decides
    /// whether the detour's stored heights are sampled terrain to keep (residual blend) or the
    /// `0` placeholder to replace (seam lerp); `orig_name` derives the spliced name
    /// (`"Detour · <name>"`, idempotent for an already-detoured name). Touches nothing until the
    /// first [`step`](Self::step).
    pub fn new(split_m: u32, rejoin_m: u32, detour_len_m: u32, detour_has_elevation: bool, orig_name: &str) -> Splicer {
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
            det_sampled: detour_has_elevation,
            det_ele_first: 0,
            det_ele_last: 0,
            res_start: 0.0,
            res_end: 0.0,
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
                    Ok(mut em) => {
                        // The detour's densified sample points exist **only** to carry height: the
                        // emitter's purely planar decimator would drop the one standing on a crest
                        // of a straight ramp and hand the profile back the flat line #1091 is about.
                        // Same threshold, same reason as the nav emit that produced them. Left off
                        // when the detour has no elevation, which is what keeps that case's bytes
                        // identical to the pre-#1091 splice.
                        if self.det_sampled {
                            em.keep_elevation_detail(ELE_SPLICE_KEEP_M);
                        }
                        self.em = Some(em);
                    }
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
                        // Denominator and both seam samples ready → resolve the blend's residuals.
                        // With no detour elevation the sampled term is the `0` placeholder, so the
                        // residuals *are* the seam heights and the blend is the old seam lerp.
                        let (s0, s1) = if self.det_sampled { (self.det_ele_first, self.det_ele_last) } else { (0, 0) };
                        self.res_start = f32::from(self.ele_split) - f32::from(s0);
                        self.res_end = f32::from(self.ele_rejoin) - f32::from(s1);
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
    /// pass accumulates — the blend's denominator must match its numerator — and latch the
    /// detour's first/last **sampled** heights on the way past (the residuals' subtrahends); no
    /// second pass over the detour is needed for them.
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

    /// Stream one detour chunk, offsetting each point's **sampled** elevation by the blended seam
    /// residual at its arc-length position (#1091). The two ends land exactly on the stored
    /// route's seam heights; the interior keeps whatever shape the terrain gave it.
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
            // The one branch on the explicit signal: sampled terrain rides through, an
            // elevation-less detour contributes the `0` placeholder its points actually carry.
            let sampled = if self.det_sampled { p.ele } else { 0 };
            let ele = blend_ele(sampled, self.res_start, self.res_end, t);
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

        // The spliced route carries elevation iff one of its two sources did: the detour by the
        // plan's explicit bit, the head/tail by what a reader can honestly say about stored bytes.
        // Both zero ⇒ the whole stream is the `0` placeholder and the header keeps the documented
        // no-elevation shape, which is what the ascent/descent/min/max below already come out as.
        let has_elevation = self.det_sampled || orig.has_elevation();
        if self.min_ele > self.max_ele {
            self.min_ele = 0;
            self.max_ele = 0;
        }
        // Ascent/descent are the dead-band's totals over the **final** point stream — head, both
        // seam segments, the blended detour and the tail — at the same [`ELE_DEADBAND_M`] the nav
        // emit and the GPX converter run, so a spliced route's climb is produced exactly the way a
        // plain planned route's is rather than stitched from two headers.
        let stats = EmitStats {
            min_ele_m: self.min_ele,
            max_ele_m: self.max_ele,
            ascent_m: self.elev.ascent() as u32,
            descent_m: self.elev.descent() as u32,
            total_distance_m: Some(override_total),
            has_elevation,
        };
        let em = self.em.take().ok_or(Error::Empty)?;
        em.finish(sink, &self.name, stats, &mut wps)
    }
}

/// A detour point's spliced height: its own `sampled` terrain height plus the linear blend of the
/// two seam residuals at arc fraction `t` (#1091).
///
/// With `sampled == 0` and `(r0, r1)` the two seam heights this reduces — as an expression, not
/// merely as a value — to the pre-#1091 `roundf(a + (b − a) · t)` seam lerp, which is what makes
/// an elevation-less detour splice to byte-identical output.
fn blend_ele(sampled: i16, r0: f32, r1: f32, t: f32) -> i16 {
    libm::roundf(sampled as f32 + (r0 + (r1 - r0) * t)) as i16
}

// ------------------------------------------------------------------------- rejoin-at-first-contact

/// Per-point hug distance to the route tail (m). Two consecutive detour points each within this of
/// the tail count as *sustained* contact — the same both-endpoints-proximity trick
/// [`Corridor::blocks`](crate::corridor::Corridor) uses, so a single-point crossing or a bridge
/// overpass never triggers. A deliberately-untuned first value (mirrors the corridor width scale).
pub(crate) const TRIM_CONTACT_M: f32 = 25.0;

/// How far past `target_m` the tail is materialized when looking for the detour's first contact
/// with it (m). The A* approach that rides the route backwards does so over at most a few hundred
/// metres of tail; this window is generous. Deliberately-untuned first value.
pub(crate) const TRIM_LOOKAHEAD_M: u32 = 1_500;

/// Max resident tail sample points (12 B each → ~1.5 KB) — a longer window widens its stride to
/// fit, so this is a hard cap by construction (mirrors [`CORRIDOR_MAX_PTS`](crate::corridor)).
const TRIM_TAIL_MAX_PTS: usize = 128;

/// Along-tail sampling interval floor (m): finer than the contact radius so the chord between
/// samples never hides a point that is genuinely on the tail.
const TRIM_MIN_SAMPLE_M: f32 = 20.0;

/// A trim whose rejoin advances no further than this past `target_m` — and which contacts the tail
/// only at the detour's final pair — is a no-op: every plan's landing hugs the tail near the goal
/// by construction, so trimming there would rewrite the bytes for nothing.
const TRIM_NOOP_M: u32 = 30;

/// The result of [`trim_detour_to_tail`] when the detour is advanced to its first sustained tail
/// contact: the (farther) rejoin distance to splice from, and the trimmed detour's measured length
/// and climb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrimOutcome {
    /// The rejoin distance on the *original* route — always `>= target_m`.
    pub rejoin_m: u32,
    /// The trimmed detour's length: measured polyline meters (the emitter's re-measured header
    /// total), not the planner's summed edge `length_m`. A trimmed detour has no planner-summed
    /// length for its shortened form, so this is the honest basis for the preview's cost line — the
    /// same few-percent metric family as the untrimmed splice's header-total precedent.
    pub detour_len_m: u32,
    /// The trimmed detour's own dead-banded ascent (m) over its **kept** sampled heights — the
    /// preview's climb figure needs the shortened leg's climb, not the planner's, exactly as
    /// [`detour_len_m`](Self::detour_len_m) is its length rather than the planner's. `0` when the
    /// plan carried no elevation (the caller gates on its own `has_elevation`).
    pub ascent_m: u32,
}

/// Advance a planned detour's rejoin to its **first sustained contact** with the route tail (#882).
///
/// A* legally approaches the goal-snap node along the future route's own road (the tail past
/// `target_m` is deliberately not blacklisted by the [`Corridor`](crate::corridor::Corridor)), so
/// the splice would append a tail that immediately retraces that approach — ride to the rejoin ring
/// and straight back. This walks the planned detour in route order and finds the first point pair
/// where **both** points hug the tail (within [`TRIM_CONTACT_M`]); it truncates the detour there and
/// re-emits `detour[0..=trim]` into `sink`, returning the rejoin distance to splice from instead
/// (the chooser's `target_m` becomes a rejoin *minimum*).
///
/// Returns `Ok(None)` — leaving `sink` untouched, caller keeps the untrimmed bytes and `target_m` —
/// when the detour never sustains contact, or when the only contact is its final landing pair near
/// the goal (the no-op every plan produces by construction).
///
/// `#[inline(never)]` — its `Tail` sampler lives here across the (separately out-of-lined) walk and
/// re-emit frames; the ~9 kB emitter and the 2 kB decode buffers stay in their own popped frames.
#[inline(never)]
pub fn trim_detour_to_tail(
    orig: &RouteReader,
    detour: &RouteReader,
    target_m: u32,
    detour_has_elevation: bool,
    sink: &mut dyn ByteSink,
) -> Result<Option<TrimOutcome>, Error> {
    let tail = Tail::build(orig, target_m);
    // No tail past the goal (rejoin at/near the route end) → nothing to retrace.
    if tail.pts.len() < 2 {
        return Ok(None);
    }
    let Some((trim_index, rejoin_m)) = first_tail_contact(detour, &tail, target_m)? else {
        return Ok(None);
    };
    // The contact is only the detour's final pair, landing at (≈) the goal — the by-construction
    // no-op. `trim_index` is the pair's first point; `last_index - 1` is that final pair's start.
    let last_index = detour_distinct_count(detour).saturating_sub(1);
    if trim_index + 1 >= last_index && rejoin_m.saturating_sub(target_m) <= TRIM_NOOP_M {
        return Ok(None);
    }
    let stats = emit_trimmed(detour, trim_index, detour_has_elevation, sink)?;
    Ok(Some(TrimOutcome { rejoin_m, detour_len_m: stats.total_distance_m, ascent_m: stats.total_ascent_m }))
}

/// A detour point's projection onto the tail: `(segment index, t along it, cross-track distance m)`.
type TailHit = (usize, f32, f32);

/// The route tail past `target_m`, downsampled to a small resident polyline carrying each sample's
/// along-route progress — the geometry the detour walk projects against. Mirrors
/// [`Corridor::build`](crate::corridor::Corridor)'s downsample-a-span shape.
struct Tail {
    /// `(lon, lat, progress_m)` samples in route order (progress ascending from `target_m`).
    pts: heapless::Vec<(i32, i32, u32), TRIM_TAIL_MAX_PTS>,
    /// `pts`' union bbox, pre-inflated by [`TRIM_CONTACT_M`] — the cheap reject.
    bbox: BBox,
    /// `cos_lat` hoisted at the tail's first point — one band for the whole projection.
    cl: f32,
}

impl Tail {
    /// Materialize `[target_m, min(total, target_m + TRIM_LOOKAHEAD_M)]` into resident samples,
    /// streaming the span chunk-clipped (no retained route copy) and accumulating arc length so each
    /// kept sample carries its along-route progress. Always keeps the span's true end.
    #[inline(never)]
    fn build(orig: &RouteReader, target_m: u32) -> Tail {
        let total = orig.total_distance_m;
        let hi = target_m.saturating_add(TRIM_LOOKAHEAD_M).min(total);
        let window = hi.saturating_sub(target_m);
        let stride = if window == 0 {
            TRIM_MIN_SAMPLE_M
        } else {
            (window as f32 / (TRIM_TAIL_MAX_PTS - 1) as f32).max(TRIM_MIN_SAMPLE_M)
        };

        let mut pts: heapless::Vec<(i32, i32, u32), TRIM_TAIL_MAX_PTS> = heapless::Vec::new();
        let mut cl = 1.0f32;
        let mut since_kept = 0.0f32;
        let mut acc = 0.0f32; // arc length from the window start
        let mut last_seen: Option<(i32, i32)> = None;
        orig.visit_points_between(target_m, hi, |slice| {
            for &p in slice {
                if pts.is_empty() {
                    cl = cos_lat(p.1);
                    let _ = pts.push((p.0, p.1, target_m));
                    last_seen = Some(p);
                    continue;
                }
                let prev = last_seen.unwrap_or(p);
                // Chunk seams repeat the boundary point; a zero-length hop advances nothing.
                let d = ground_dist_m_cl(prev, p, cl);
                acc += d;
                since_kept += d;
                last_seen = Some(p);
                if since_kept >= stride && !pts.is_full() {
                    let _ = pts.push((p.0, p.1, target_m.saturating_add(acc as u32)));
                    since_kept = 0.0;
                }
            }
        });
        // Always keep the span's true end so the tail reaches the route's actual end.
        if let Some(end) = last_seen {
            let prog = target_m.saturating_add(acc as u32);
            if pts.last().map(|&(x, y, _)| (x, y)) != Some(end) {
                if pts.is_full() {
                    let n = pts.len();
                    pts[n - 1] = (end.0, end.1, prog);
                } else {
                    let _ = pts.push((end.0, end.1, prog));
                }
            }
        }

        let bbox = inflated_bbox(pts.iter().map(|&(lon, lat, _)| (lon, lat)), cl, TRIM_CONTACT_M);
        Tail { pts, bbox, cl }
    }

    /// The nearest tail segment to `p`: `(segment index, t along it, cross-track distance m)`, or
    /// `None` when `p` is outside the inflated bbox (the cheap reject). Requires `pts.len() >= 2`.
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

/// Walk the detour in route order and return the first hugging pair: `(trim_index, rejoin_m)` where
/// `trim_index` is the distinct-point index of the pair's **first** point and `rejoin_m` is that
/// point's along-route progress projected onto the tail (clamped `>= target_m`). `None` if no two
/// consecutive detour points both lie within [`TRIM_CONTACT_M`] of the tail.
///
/// `#[inline(never)]` — the ~2 kB decode buffer is reused across the whole walk in this popped
/// frame, never the caller's (the #419/#501 stack discipline).
#[inline(never)]
fn first_tail_contact(detour: &RouteReader, tail: &Tail, target_m: u32) -> Result<Option<(usize, u32)>, Error> {
    let mut buf = heapless::Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
    let mut gi = 0usize; // distinct-point index
    let mut prev: Option<(usize, Option<TailHit>)> = None; // (index, its tail projection)
    for k in 0..detour.chunks().len() {
        detour.decode_chunk(k, &mut buf)?;
        // Chunk k>0 re-decodes chunk k−1's last point as its anchor — skip the duplicate.
        let skip = usize::from(k > 0);
        for p in buf.iter().skip(skip) {
            let near = tail.nearest((p.lon, p.lat));
            if let Some((pgi, pnear)) = prev {
                let a_hug = pnear.is_some_and(|(_, _, d)| d <= TRIM_CONTACT_M);
                let b_hug = near.is_some_and(|(_, _, d)| d <= TRIM_CONTACT_M);
                if a_hug && b_hug {
                    let (seg, t, _) = pnear.expect("a_hug implies Some");
                    let rejoin_m = tail.progress_at(seg, t).max(target_m);
                    return Ok(Some((pgi, rejoin_m)));
                }
            }
            prev = Some((gi, near));
            gi += 1;
        }
    }
    Ok(None)
}

/// Re-emit `detour[0..=trim_index]` (distinct-point indices) through a fresh [`ObcrEmitter`] into
/// `sink` — the trimmed detour: **sampled elevations carried through verbatim** (#1091; the splice
/// blends its seam residuals onto them later, and it can only do that if they survive the trim),
/// the detour name kept, no waypoints, header total the emitter's re-measured polyline length and
/// header climb the dead-band's own over the kept points.
///
/// `has_elevation` is the plan's explicit bit, passed through rather than re-derived — with it
/// false every stored height is the `0` placeholder and the header keeps the no-elevation shape,
/// which is byte-for-byte what this emitted before #1091.
///
/// `#[inline(never)]` — owns both the ~9 kB emitter and the 2 kB decode buffer; keep the frame off
/// the caller's stack.
#[inline(never)]
fn emit_trimmed(
    detour: &RouteReader,
    trim_index: usize,
    has_elevation: bool,
    sink: &mut dyn ByteSink,
) -> Result<RouteStats, Error> {
    let mut em = ObcrEmitter::new(sink)?;
    if has_elevation {
        em.keep_elevation_detail(ELE_SPLICE_KEEP_M);
    }
    let mut buf = heapless::Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
    let mut band = DeadBand::<f64>::new();
    let (mut min_ele, mut max_ele) = (i16::MAX, i16::MIN);
    let mut gi = 0usize;
    'outer: for k in 0..detour.chunks().len() {
        detour.decode_chunk(k, &mut buf)?;
        let skip = usize::from(k > 0);
        for p in buf.iter().skip(skip) {
            let ele = if has_elevation { p.ele } else { 0 };
            min_ele = min_ele.min(ele);
            max_ele = max_ele.max(ele);
            band.push(f64::from(ele));
            em.push(sink, p.lon, p.lat, ele, band.ascent() as u32)?;
            if gi == trim_index {
                break 'outer;
            }
            gi += 1;
        }
    }
    let mut wps: Vec<WpPlace, MAX_WAYPOINTS> = Vec::new();
    // A trim that kept no point at all leaves the crossed pair; the emitter errors out anyway, but
    // the zeroing keeps the "no elevation" header shape honest on every path.
    let (min_ele, max_ele) = if has_elevation && min_ele <= max_ele { (min_ele, max_ele) } else { (0, 0) };
    let stats = EmitStats {
        min_ele_m: min_ele,
        max_ele_m: max_ele,
        ascent_m: if has_elevation { band.ascent() as u32 } else { 0 },
        descent_m: if has_elevation { band.descent() as u32 } else { 0 },
        total_distance_m: None,
        has_elevation,
    };
    em.finish(sink, detour.name(), stats, &mut wps)
}

/// Count a route's distinct points (seam duplicates excluded): `Σ point_count − (chunks − 1)`.
fn detour_distinct_count(r: &RouteReader) -> usize {
    let chunks = r.chunks();
    if chunks.is_empty() {
        return 0;
    }
    let sum: usize = chunks.iter().map(|c| c.point_count as usize).sum();
    sum - (chunks.len() - 1)
}

/// One-shot convenience over [`Splicer`]: loop [`step`](Splicer::step) to completion — the
/// headless sim and the tests; interactive hosts step the splicer themselves.
#[allow(clippy::too_many_arguments)] // the splice request plus its two readers and the sink
pub fn splice_detour(
    orig: &RouteReader,
    detour: &RouteReader,
    split_m: u32,
    rejoin_m: u32,
    detour_len_m: u32,
    detour_has_elevation: bool,
    sink: &mut dyn ByteSink,
) -> Result<RouteStats, Error> {
    let mut sp = Splicer::new(split_m, rejoin_m, detour_len_m, detour_has_elevation, orig.name());
    loop {
        match sp.step(orig, detour, sink) {
            SpliceStep::Running => {}
            SpliceStep::Done(stats) => return Ok(stats),
            SpliceStep::Failed(e) => return Err(e),
        }
    }
}
