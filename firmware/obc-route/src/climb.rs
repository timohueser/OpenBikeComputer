//! Climb detection: offline segmentation of a route's elevation-against-distance signal into a
//! small resident list of climbs.
//!
//! A planned route's elevation profile is known up front, so one load-time sweep turns "which
//! climb am I on?" into the interval lookup [`Climbs::active_at`], which the ride loop can call
//! per frame. It uses the same chunk decode, distance metric and [`DeadBand`] smoothing as
//! [`elevation_profile`](crate::profile), then feeds the smoothed stream through the hysteresis
//! state machine [`segment_climbs`].
//!
//! A single grade threshold would split a real climb at every false flat and merge a
//! descent-linked pair of passes. The hysteresis here bridges a dip shallower than [`MAX_DROP`]
//! but splits at a col deeper than it, and tolerates a flat stretch up to [`MAX_FLAT`].
//!
//! The detector runs on the decimated stored geometry, not the original track, so it reads what
//! is on the card.

use heapless::Vec;

use crate::reader::{RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK};
use obc_elevation::DeadBand;
use obc_map_scene::ground_dist_m;

// The five consts below are the whole "what counts as a climb" policy. They stay plain module
// consts: there is one policy for the device, and a config struct would invite per-call variation.

/// Minimum net gain (m) for a candidate to be kept. Below this it is a bump, not a climb.
pub const MIN_GAIN: u16 = 80;

/// Minimum average grade, whole percent, over the climb's length. It rejects a long shallow drag
/// that clears [`MIN_GAIN`] only by being long.
pub const MIN_AVG_GRADE: i16 = 3;

/// Col tolerance (m): how far the profile can drop below a candidate's running max before the
/// climb is closed at that max. A shallower dip is bridged; a deeper col splits the route into two
/// climbs. This is the primary feel knob: raising it merges passes, lowering it fragments them.
pub const MAX_DROP: i16 = 25;

/// Flat tolerance, meters of distance, that the profile can run without a new max before the
/// candidate is closed, even when it never drops far enough to trip [`MAX_DROP`].
pub const MAX_FLAT: u32 = 300;

/// Minimum length (m) for a kept climb. With the other gates it rejects a short sharp ramp.
pub const MIN_LEN: u32 = 400;

/// Resident cap on detected climbs. A route with more keeps the largest-gain ones rather than
/// truncating in route order.
pub const MAX_CLIMBS: usize = 24;

/// One detected climb: a distance interval along the route, its base and top elevations, and the
/// derived gain and grade. Distances are cumulative meters from the route start, the same axis as
/// the profile and the matcher progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClimbSeg {
    /// Distance (m) at the pre-climb trough, where the sustained rise begins.
    pub start_m: u32,
    /// Distance (m) at the summit.
    pub end_m: u32,
    pub base_ele_m: i16,
    pub top_ele_m: i16,
    /// Net gain (m), cached so the ride UI and the largest-gain cap do not recompute it.
    pub gain_m: u16,
    /// Average grade over the climb, whole percent. The per-point grade lives in the profile.
    pub avg_grade_pct: i16,
}

impl ClimbSeg {
    /// Length of the climb along the route (m). The [`MIN_LEN`] gate keeps it at 1 or more, so a
    /// caller can divide by it without guarding zero.
    #[inline]
    pub fn len_m(&self) -> u32 {
        self.end_m.saturating_sub(self.start_m)
    }
}

/// A route's detected climbs, in route order and non-overlapping. Built once per route and
/// queried per frame with [`active_at`](Self::active_at). A route with more than [`MAX_CLIMBS`]
/// climbs keeps the largest-gain ones.
#[derive(Debug, Clone, Default)]
pub struct Climbs(pub Vec<ClimbSeg, MAX_CLIMBS>);

impl Climbs {
    #[inline]
    pub fn new() -> Self {
        Climbs(Vec::new())
    }

    #[inline]
    pub fn as_slice(&self) -> &[ClimbSeg] {
        &self.0
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Index of the climb whose interval contains `progress_m`. This is a raw lookup with no
    /// enter or exit hysteresis: the margin that stops the banner flickering at a boundary is
    /// applied at the call site, so the stored intervals stay the detected geometry.
    pub fn active_at(&self, progress_m: u32) -> Option<usize> {
        self.0.iter().position(|c| progress_m >= c.start_m && progress_m <= c.end_m)
    }

    /// The climb the rider is on at `start_m`, or else the first one that starts by `end_m`.
    pub fn ahead(&self, start_m: u32, end_m: u32) -> Option<&ClimbSeg> {
        self.0
            .iter()
            .find(|c| c.start_m <= start_m && c.end_m > start_m)
            .or_else(|| self.0.iter().find(|c| c.start_m > start_m && c.start_m <= end_m))
    }

    /// Insert `seg`, capping the list at [`MAX_CLIMBS`] by largest gain: once full, `seg`
    /// replaces the smallest-gain climb only if it is bigger. The caller re-sorts into route order
    /// afterwards, because this leaves the list unordered once the cap is hit.
    fn push_keeping_largest(&mut self, seg: ClimbSeg) {
        if self.0.push(seg).is_ok() {
            return;
        }
        let (min_i, min_gain) =
            self.0.iter().enumerate().fold(
                (0usize, u16::MAX),
                |(bi, bg), (i, c)| {
                    if c.gain_m < bg {
                        (i, c.gain_m)
                    } else {
                        (bi, bg)
                    }
                },
            );
        if seg.gain_m > min_gain {
            self.0[min_i] = seg;
        }
    }

    /// Re-sort into route order after largest-gain capping left the tail unordered.
    fn sort_by_route_order(&mut self) {
        self.0.sort_unstable_by_key(|c| c.start_m);
    }
}

/// One sample fed to the segmenter. Bundling the two fields lets [`segment_climbs`] take a single
/// iterator and stay a pure function of the stream, testable from a hand-built list of samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElePt {
    /// Cumulative distance from the route start, meters. `f64` matches the profile's running
    /// total, which does not drift over a long route.
    pub dist_m: f64,
    /// Dead-band-smoothed elevation, meters.
    pub ele_m: f32,
}

/// An in-flight climb candidate: the trough it rises from and its running summit.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    /// Distance (m) at the trough, the lowest point since the last close.
    trough_dist: f64,
    trough_ele: f32,
    /// Distance (m) at the running summit, the highest point since the trough.
    max_dist: f64,
    max_ele: f32,
}

/// Fold an ordered `(distance, smoothed elevation)` stream into the route's kept climbs. This is
/// the whole detection policy; [`RouteReader::detect_climbs`] only feeds it.
///
/// A candidate closes when, measured from its running summit, the elevation drops more than
/// [`MAX_DROP`] or more than [`MAX_FLAT`] meters pass without a new summit. It is then kept only
/// if it clears [`MIN_GAIN`], [`MIN_AVG_GRADE`] and [`MIN_LEN`]. Scanning continues from the
/// summit either way, so a deep col becomes the base of the next climb.
pub fn segment_climbs<I: IntoIterator<Item = ElePt>>(stream: I) -> Climbs {
    let mut detector = ClimbDetector::new();
    for point in stream {
        detector.push(point);
    }
    detector.finish()
}

pub(crate) struct ClimbDetector {
    climbs: Climbs,
    capped: bool,
    cand: Option<Candidate>,
    trough: Option<ElePt>,
}

impl ClimbDetector {
    pub(crate) fn new() -> Self {
        Self { climbs: Climbs::new(), capped: false, cand: None, trough: None }
    }

    pub(crate) fn push(&mut self, p: ElePt) {
        if !p.ele_m.is_finite() {
            self.cand = None;
            self.trough = None;
            return;
        }
        match self.cand.as_mut() {
            // The gates are applied at close, not at open, so any rise opens a candidate and it
            // can grow into a keeper.
            None => {
                let base = match self.trough {
                    None => {
                        self.trough = Some(p);
                        return;
                    }
                    Some(t) => t,
                };
                if p.ele_m < base.ele_m {
                    self.trough = Some(p);
                } else if p.ele_m > base.ele_m {
                    self.cand = Some(Candidate {
                        trough_dist: base.dist_m,
                        trough_ele: base.ele_m,
                        max_dist: p.dist_m,
                        max_ele: p.ele_m,
                    });
                    self.trough = None;
                }
            }

            Some(c) => {
                if p.ele_m > c.max_ele {
                    // A new summit resets both counters, which are measured from it.
                    c.max_ele = p.ele_m;
                    c.max_dist = p.dist_m;
                } else {
                    // Either counter tripping closes the candidate at its summit.
                    let give_back = c.max_ele - p.ele_m;
                    let flat_run = p.dist_m - c.max_dist;
                    if give_back > MAX_DROP as f32 || flat_run > MAX_FLAT as f64 {
                        // Copy the summit out before the borrow ends, so the close can reassign
                        // `self.cand`.
                        let summit = ElePt { dist_m: c.max_dist, ele_m: c.max_ele };
                        if let Some(seg) = close_candidate(c) {
                            self.climbs.push_keeping_largest(seg);
                            self.capped |= self.climbs.len() == MAX_CLIMBS;
                        }
                        // This sample is below the summit, so it seeds the next trough; a close
                        // on the flat counter falls back to the summit itself.
                        self.cand = None;
                        self.trough = Some(if p.ele_m < summit.ele_m { p } else { summit });
                    }
                }
            }
        }
    }

    pub(crate) fn finish(mut self) -> Climbs {
        // The route ends on a climb: close the open candidate at its summit.
        if let Some(c) = self.cand {
            if let Some(seg) = close_candidate(&c) {
                self.climbs.push_keeping_largest(seg);
                self.capped |= self.climbs.len() == MAX_CLIMBS;
            }
        }

        if self.capped {
            self.climbs.sort_by_route_order();
        }
        self.climbs
    }
}

/// Turn a closed candidate into a kept [`ClimbSeg`], or `None` when it fails a gate.
fn close_candidate(c: &Candidate) -> Option<ClimbSeg> {
    // A candidate only opens on a rise, so the gain cannot be negative here, but the check keeps
    // the `u16` cast from wrapping.
    let gain_f = c.max_ele - c.trough_ele;
    if gain_f <= 0.0 {
        return None;
    }
    let len_f = c.max_dist - c.trough_dist;
    if len_f < MIN_LEN as f64 {
        return None;
    }

    let gain_m = gain_f as u32;
    if gain_m < MIN_GAIN as u32 {
        return None;
    }
    // `len_m` is at least `MIN_LEN`, so the divide is safe.
    let len_m = len_f as u32;
    let avg_grade = (gain_m * 100 / len_m) as i16;
    if avg_grade < MIN_AVG_GRADE {
        return None;
    }

    Some(ClimbSeg {
        start_m: c.trough_dist as u32,
        end_m: c.max_dist as u32,
        base_ele_m: c.trough_ele as i16,
        top_ele_m: c.max_ele as i16,
        gain_m: gain_m.min(u16::MAX as u32) as u16,
        avg_grade_pct: avg_grade,
    })
}

impl RouteReader<'_> {
    /// Detect the route's climbs in one streaming pass over the geometry. Each chunk is decoded
    /// once, so cache the result on route load and do not call this per frame.
    ///
    /// The distance metric and dead-band match the profile's ascent integrator, so the summed
    /// gains land near the header's `total_ascent_m`. They do not equal it: detection drops
    /// sub-threshold bumps and the descents between climbs.
    pub fn detect_climbs(&self) -> Climbs {
        // The stream is lazy, so only the current chunk's points are ever buffered.
        let stream = ClimbStream {
            reader: self,
            buf: Vec::new(),
            chunk: 0,
            in_chunk: 0,
            prev: None,
            dist: 0.0,
            smooth: DeadBand::<f32>::new(),
        };
        segment_climbs(stream)
    }
}

/// Turns [`RouteReader`]'s chunk sweep into the [`ElePt`] stream the segmenter consumes, one
/// point at a time, so the whole route is never buffered.
struct ClimbStream<'a, 'b> {
    reader: &'b RouteReader<'a>,
    buf: Vec<RoutePoint, MAX_POINTS_PER_CHUNK>,
    /// The chunk loaded in `buf`, or the next one to load once `in_chunk` has consumed it.
    chunk: usize,
    in_chunk: usize,
    /// The previous point, for the per-segment distance step.
    prev: Option<(i32, i32)>,
    /// Cumulative distance (m), re-anchored per chunk to that chunk's stored `cum_distance_m`, so
    /// it cannot drift over a long route.
    dist: f64,
    /// Smooths across chunk seams too: a seam point compares equal to itself and books nothing.
    smooth: DeadBand<f32>,
}

impl Iterator for ClimbStream<'_, '_> {
    type Item = ElePt;

    fn next(&mut self) -> Option<ElePt> {
        loop {
            // Refill when the current chunk is exhausted, skipping any that fails to decode.
            if self.in_chunk >= self.buf.len() {
                if self.chunk >= self.reader.chunks().len() {
                    return None;
                }
                let k = self.chunk;
                self.chunk += 1;
                if self.reader.decode_chunk(k, &mut self.buf).is_err() || self.buf.is_empty() {
                    continue;
                }
                // Re-anchor the running distance to this chunk's stored value. `prev` is reset
                // so the seam segment is not measured twice.
                self.dist = self.reader.chunks()[k].cum_distance_m as f64;
                self.prev = None;
                self.in_chunk = 0;
            }

            let p = self.buf[self.in_chunk];
            self.in_chunk += 1;
            if let Some(pr) = self.prev {
                self.dist += ground_dist_m(pr, (p.lon, p.lat)) as f64;
            }
            self.prev = Some((p.lon, p.lat));
            // The segmenter reads the dead-band's reference, not the raw sample, so noise below
            // the band neither opens nor closes a climb.
            if p.elevation().is_none() || p.elevation_incomplete {
                self.smooth.pause();
                return Some(ElePt { dist_m: self.dist, ele_m: f32::NAN });
            }
            self.smooth.push(p.ele as f32);
            return Some(ElePt { dist_m: self.dist, ele_m: self.smooth.smoothed().unwrap_or(p.ele as f32) });
        }
    }
}
