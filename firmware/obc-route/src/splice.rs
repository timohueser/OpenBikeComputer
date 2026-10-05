//! Resumable detour splice through the shared OBCR emitter.
//!
//! Valid sampled detour elevations are aligned to the seam elevations by a linear residual blend.
//! Missing heights stay unknown. Output totals measure the retained geometry.

pub use crate::trim::trim_detour_to_tail;

use heapless::Vec;

use crate::compose::{Blend, Compose, Part, Seam, Segment, Source, WaypointMap};
use crate::convert::{ObcrWriter, RouteStats, WpPlace};
use crate::reader::{RouteReader, WaypointCursor, MAX_WAYPOINTS};
use obc_elevation::ELE_DEADBAND_M;
use obc_formats::io::{ByteSink, Error};
use obc_formats::obcr::NAME_CAP;
use obc_map_scene::ground_dist_m;

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
    Compose,
    Measure,
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
    aligned: bool,
    split_m: u32,
    rejoin_m: u32,
    name: heapless::String<NAME_CAP>,
    compose: Compose,
    /// Seam elevations read off the original route.
    ele_split: i16,
    ele_rejoin: i16,
    /// Stored endpoint elevations used to align the detour's datum to the original route.
    det_ele_first: i16,
    det_ele_last: i16,
    /// Valid seam elevation minus the corresponding detour endpoint elevation.
    res_start: f32,
    res_end: f32,
    /// Measured detour polyline length.
    det_total: f32,
    /// Previous source point in the elevation-alignment measurement pass.
    prev_det: Option<(i32, i32)>,
    /// The first tail point's cumulative distance in the spliced route, which is the shift base
    /// for the tail waypoints. `None` while the tail has not started or is empty.
    tail_first_along: Option<u32>,
    waypoints: Vec<WpPlace, MAX_WAYPOINTS>,
    waypoint_cursor: Option<WaypointCursor>,
}

impl Splicer {
    /// A splicer for `original[0..split_m] + detour + original[rejoin_m..]`. The stored point
    /// facts and the final geometry determine the output. `orig_name` supplies the derived name.
    ///
    /// A [`Leg::Approach`] leads to `rejoin_m`, then follows the route from there. It adds no
    /// avoidance. One height offset aligns its end with the join point. Tail waypoints follow it.
    ///
    /// A [`Leg::Rest`] is a stored route, so its heights stay as stored. The route follows from
    /// `rejoin_m`, and the output is a built day. The rest's waypoints are not kept.
    pub fn new(leg: Leg, split_m: u32, rejoin_m: u32, orig_name: &str) -> Splicer {
        let mut slot = core::mem::MaybeUninit::uninit();
        // SAFETY: the local slot is aligned, writable and exclusively owned.
        unsafe {
            Self::init_in_place(slot.as_mut_ptr(), leg, split_m, rejoin_m, orig_name);
            slot.assume_init()
        }
    }

    /// # Safety
    /// `slot` must be aligned, writable and exclusively owned for a complete splicer.
    #[inline(never)]
    pub unsafe fn init_in_place(slot: *mut Self, leg: Leg, split_m: u32, rejoin_m: u32, orig_name: &str) {
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
        unsafe {
            core::ptr::addr_of_mut!((*slot).leg).write(leg);
            core::ptr::addr_of_mut!((*slot).adds_avoidance).write(leg == Leg::Detour);
            core::ptr::addr_of_mut!((*slot).assistant_candidate).write(false);
            core::ptr::addr_of_mut!((*slot).aligned).write(false);
            core::ptr::addr_of_mut!((*slot).phase).write(Phase::Init);
            core::ptr::addr_of_mut!((*slot).split_m).write(split_m);
            core::ptr::addr_of_mut!((*slot).rejoin_m).write(rejoin_m);
            core::ptr::addr_of_mut!((*slot).name).write(name);
            Compose::init_in_place(core::ptr::addr_of_mut!((*slot).compose));
            core::ptr::addr_of_mut!((*slot).ele_split).write(0);
            core::ptr::addr_of_mut!((*slot).ele_rejoin).write(0);
            core::ptr::addr_of_mut!((*slot).det_ele_first).write(0);
            core::ptr::addr_of_mut!((*slot).det_ele_last).write(0);
            core::ptr::addr_of_mut!((*slot).res_start).write(0.0);
            core::ptr::addr_of_mut!((*slot).res_end).write(0.0);
            core::ptr::addr_of_mut!((*slot).det_total).write(0.0);
            core::ptr::addr_of_mut!((*slot).prev_det).write(None);
            core::ptr::addr_of_mut!((*slot).tail_first_along).write(None);
            core::ptr::addr_of_mut!((*slot).waypoints).write(Vec::new());
            core::ptr::addr_of_mut!((*slot).waypoint_cursor).write(None);
            let Self {
                leg: _,
                adds_avoidance: _,
                assistant_candidate: _,
                aligned: _,
                phase: _,
                split_m: _,
                rejoin_m: _,
                name: _,
                compose: _,
                ele_split: _,
                ele_rejoin: _,
                det_ele_first: _,
                det_ele_last: _,
                res_start: _,
                res_end: _,
                det_total: _,
                prev_det: _,
                tail_first_along: _,
                waypoints: _,
                waypoint_cursor: _,
            } = &*slot;
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

    /// Run one bounded unit of splicing. `orig` is the route being detoured, `detour` the
    /// detour-only OBCR from the plan phase, and `sink` the spliced route's output.
    pub fn step(&mut self, orig: &RouteReader, detour: &RouteReader, sink: &mut dyn ByteSink) -> SpliceStep {
        match self.advance(orig, detour, sink) {
            Ok(None) => SpliceStep::Running,
            Ok(Some(stats)) => {
                self.phase = Phase::Terminal(Ok(stats));
                SpliceStep::Done(stats)
            }
            Err(error) => {
                self.phase = Phase::Terminal(Err(error));
                SpliceStep::Failed(error)
            }
        }
    }

    fn advance(
        &mut self,
        orig: &RouteReader,
        detour: &RouteReader,
        sink: &mut dyn ByteSink,
    ) -> Result<Option<RouteStats>, Error> {
        match &self.phase {
            Phase::Init => {
                match (orig.visit_descriptor(), detour.visit_descriptor()) {
                    (Ok(None), Ok(None)) => {}
                    _ => return Err(Error::BadOffset),
                }
                let ele = orig.elevation_at(self.split_m).unwrap_or(i16::MIN);
                self.ele_split = ele;
                self.phase = Phase::Rejoin;
                Ok(None)
            }
            Phase::Rejoin => {
                let ele = orig.elevation_at(self.rejoin_m).unwrap_or(i16::MIN);
                self.ele_rejoin = ele;
                ObcrWriter::begin(sink)?;
                let original_map = orig.attribution_map()?;
                let detour_map = detour.attribution_map()?;
                self.compose.writer.set_attribution_map(if original_map == detour_map { original_map } else { None });
                self.compose.writer.set_bike_type(orig.bike_type());
                self.compose.writer.set_flags(
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
                    self.compose.writer.keep_elevation_detail(ELE_SPLICE_KEEP_M);
                }
                let parts = [Part {
                    source: Source::Original,
                    segment: Segment::Stored { lo: 0, hi: self.split_m, retain_end: false, read_to_end: false },
                    seam: Seam::Exact,
                    chunk: 0,
                }];
                self.compose.plan(if self.split_m == 0 { &[] } else { &parts })?;
                self.phase = Phase::Compose;
                Ok(None)
            }
            Phase::Compose => {
                let source = self.compose.source();
                let step = match source {
                    Some(Source::Original) => self.compose.step(orig, sink)?,
                    Some(Source::Leg) => self.compose.step(detour, sink)?,
                    None => crate::compose::Composed { more: false, first_m: None, connector_m: None },
                };
                if self.aligned && source == Some(Source::Original) && self.tail_first_along.is_none() {
                    self.tail_first_along = step.first_m;
                }
                if !step.more {
                    self.phase = if self.aligned { Phase::Waypoints } else { Phase::Measure };
                }
                Ok(None)
            }
            Phase::Measure => {
                if let Leg::Rest { .. } = self.leg {
                    self.start_remaining(orig)?;
                    return Ok(None);
                }

                if self.compose.chunk >= detour.chunks().len() {
                    let (s0, s1) = (self.det_ele_first, self.det_ele_last);
                    self.res_end = f32::from(self.ele_rejoin) - f32::from(s1);
                    // An approach starts off the route, so only its end has a seam to meet.
                    self.res_start = if self.leg == Leg::Approach {
                        self.res_end
                    } else {
                        f32::from(self.ele_split) - f32::from(s0)
                    };
                    // Restart the detour cursor for the emit pass.
                    self.prev_det = None;
                    self.start_remaining(orig)?;
                    return Ok(None);
                }
                self.det_total += self.measure_detour_chunk(detour, self.compose.chunk)?;
                self.compose.chunk += 1;
                Ok(None)
            }
            Phase::Waypoints => {
                if self.waypoint_cursor.is_none() {
                    self.waypoint_cursor = Some(WaypointCursor::new(orig.source())?);
                    return Ok(None);
                }
                match self.waypoint_cursor.as_mut().unwrap().next(orig.source())? {
                    Some(w) => {
                        let map = WaypointMap::Splice {
                            split: (!self.leg.leads_in()).then_some(self.split_m),
                            rejoin: self.rejoin_m,
                            tail: self.tail_first_along.unwrap_or_else(|| self.compose.writer.distance_m()),
                        };
                        let along = map.map(w.dist_along_m, orig.total_distance_m, self.compose.writer.distance_m())?;
                        if let Some(along) = along {
                            let _ = self.waypoints.push(WpPlace::from_stored(&w, along));
                        }
                    }
                    None => self.phase = Phase::Finish,
                }
                Ok(None)
            }
            Phase::Finish => self.finish_splice(sink).map(Some),
            Phase::Terminal(r) => r.map(Some),
        }
    }

    fn start_remaining(&mut self, original: &RouteReader) -> Result<(), Error> {
        let mut parts = Vec::<Part, 2>::new();
        let segment = match self.leg {
            Leg::Rest { from_m, to_m } if from_m < to_m => {
                Some(Segment::Stored { lo: from_m, hi: to_m, retain_end: false, read_to_end: true })
            }
            Leg::Rest { .. } => None,
            _ => Some(Segment::Leg(Blend {
                start: self.res_start,
                end: self.res_end,
                length: self.det_total,
                valid: ![self.ele_split, self.ele_rejoin, self.det_ele_first, self.det_ele_last].contains(&i16::MIN),
            })),
        };
        if let Some(segment) = segment {
            let _ = parts.push(Part { source: Source::Leg, segment, seam: Seam::Exact, chunk: 0 });
        }
        if self.rejoin_m < original.total_distance_m {
            let chunks = original.chunks();
            let chunk = (0..chunks.len())
                .find(|&k| {
                    chunks.get(k + 1).map_or(original.total_distance_m, |next| next.cum_distance_m) >= self.rejoin_m
                })
                .unwrap_or(chunks.len());
            let _ = parts.push(Part {
                source: Source::Original,
                segment: Segment::Stored {
                    lo: self.rejoin_m,
                    hi: original.total_distance_m,
                    retain_end: false,
                    read_to_end: true,
                },
                seam: Seam::Exact,
                chunk,
            });
        }
        self.compose.plan(&parts)?;
        self.aligned = true;
        self.phase = Phase::Compose;
        Ok(())
    }

    /// Measure one detour chunk's polyline length with the same per-segment metric the emit pass
    /// accumulates, so the blend's denominator matches its numerator, and latch the detour's
    /// first and last sampled heights on the way past.
    fn measure_detour_chunk(&mut self, detour: &RouteReader, k: usize) -> Result<f32, Error> {
        detour.with_chunk(k, |points| {
            let mut len = 0.0f32;
            for (i, p) in points.enumerate() {
                if k == 0 && i == 0 {
                    self.det_ele_first = p.ele;
                }
                self.det_ele_last = p.ele;
                let c = (p.lon, p.lat);
                if let Some(prev) = self.prev_det {
                    if prev != c {
                        len += ground_dist_m(prev, c);
                    }
                }
                self.prev_det = Some(c);
            }
            len
        })
    }

    /// Write the collected waypoints and patch the header, the splice's last writes.
    ///
    /// Must stay `#[inline(never)]`: the index write scratch stays out of the per-step frame.
    #[inline(never)]
    fn finish_splice(&mut self, sink: &mut dyn ByteSink) -> Result<RouteStats, Error> {
        self.compose.writer.finish(sink, &self.name, &mut self.waypoints)
    }
}

/// One-shot convenience over [`Splicer`] for the headless sim and the tests. Interactive hosts
/// step the splicer themselves.
pub fn splice_detour(
    leg: Leg,
    orig: &RouteReader,
    detour: &RouteReader,
    split_m: u32,
    rejoin_m: u32,
    sink: &mut dyn ByteSink,
) -> Result<RouteStats, Error> {
    let mut sp = Splicer::new(leg, split_m, rejoin_m, orig.name());
    loop {
        match sp.step(orig, detour, sink) {
            SpliceStep::Running => {}
            SpliceStep::Done(stats) => return Ok(stats),
            SpliceStep::Failed(e) => return Err(e),
        }
    }
}
