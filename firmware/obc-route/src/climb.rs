//! Climb detection: offline segmentation of a route's elevation-vs-distance signal into a
//! small resident list of named climbs.
//!
//! A planned route's whole elevation profile is known up front, so finding its climbs is
//! **offline segmentation**, not a real-time detector: one load-time sweep over the geometry
//! turns "which climb am I on?" into an interval lookup ([`Climbs::active_at`]) the ride loop
//! can call per frame for free. This mirrors [`elevation_profile`](crate::profile) — the same
//! chunk-decode + cumulative-distance metric + [`DeadBand`] smoothing — but instead of
//! bucketing points into a fixed pyramid it feeds the smoothed `(distance, elevation)` stream
//! through a **hysteresis state machine** ([`segment_climbs`]) that opens and closes climb
//! candidates.
//!
//! **Why a state machine, not thresholding.** A single grade threshold splits a real climb at
//! every false-flat and merges a descent-linked pair of passes. The hysteresis here bridges a
//! dip shallower than [`MAX_DROP`] (one climb over a saddle) but splits at a col deeper than it
//! (two climbs), and tolerates a flat stretch up to [`MAX_FLAT`] before giving up on the
//! current candidate. That col tolerance is the one knob that decides the "feel" of the
//! result, so it — and the other four gates — are module consts, deliberately easy to retune
//! once we've eyeballed real routes.
//!
//! The detector runs on the **decimated stored geometry** (the same points the converter kept),
//! not the original GPS track: it reads what's actually on the card. The [`DeadBand`] smoothing
//! matches the ascent integrator the converter and profile already share, so the total detected
//! gain lands near the header's `total_ascent_m`.

use heapless::Vec;

use crate::reader::{RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK};
use obc_elevation::DeadBand;
use obc_map_scene::ground_dist_m;

// ---------------------------------------------------------------------------------------------
// Tuning knobs — the whole "what counts as a climb" policy in five consts.
//
// These are the *initial* defaults; we tune them AFTER eyeballing real komoot routes with the
// `detect_climbs` example, so they are grouped here and trivial to change. Keep them as plain
// module consts (not a config struct): there's one policy for the device, and a struct would
// invite per-call variation the ride loop doesn't want.
// ---------------------------------------------------------------------------------------------

/// Minimum net gain (m) for a candidate to be kept. Below this it's a bump, not a climb — the
/// coarsest "is this worth naming?" gate.
pub const MIN_GAIN: u16 = 80;

/// Minimum average grade (%) over the climb's whole length. Rejects a long shallow drag that
/// clears [`MIN_GAIN`] only by being long — a 90 m rise over 8 km isn't a climb a rider braces
/// for. Integer percent: `gain_m * 100 / len_m`.
pub const MIN_AVG_GRADE: i16 = 3;

/// Col tolerance (m): how far the profile may drop back below a candidate's running max before
/// the climb is closed at that max. A dip shallower than this is **bridged** (the climb
/// continues over the saddle); a col deeper splits the route into two separate climbs. This is
/// the primary "feel" knob — raising it merges passes, lowering it fragments them.
pub const MAX_DROP: i16 = 25;

/// Flat tolerance (m of distance): how far the profile may run without setting a new max before
/// the candidate is closed. Bounds how long a plateau or false-flat can sit mid-climb before we
/// decide the climbing has ended, even if it never drops far enough to trip [`MAX_DROP`].
pub const MAX_FLAT: u32 = 300;

/// Minimum length (m) for a kept climb. With [`MIN_GAIN`]/[`MIN_AVG_GRADE`] this rejects a
/// short sharp ramp (e.g. a bridge approach) that's steep and tall but too brief to be a climb.
pub const MIN_LEN: u32 = 400;

/// Resident cap on the number of detected climbs. A route with more keeps the **largest-gain**
/// ones (see [`Climbs::push_keeping_largest`]) rather than truncating in route order or
/// panicking — a rider cares about the big cols, and the buffer is fixed-size for the device.
pub const MAX_CLIMBS: usize = 24;

/// One detected climb: a contiguous distance interval `[start_m, end_m]` along the route, its
/// base/top elevations, and derived gain/grade. `Copy` and plain integers so the whole
/// [`Climbs`] list is cheap to hold resident and hand around.
///
/// Distances are cumulative meters from the route start (matching the profile / matcher
/// progress), so [`Climbs::active_at`] compares a live progress directly against `start_m` /
/// `end_m`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClimbSeg {
    /// Cumulative distance (m) at the pre-climb trough — where the sustained rise begins.
    pub start_m: u32,
    /// Cumulative distance (m) at the summit (the candidate's running-max point).
    pub end_m: u32,
    /// Elevation (m) at `start_m`, the trough the climb rises from.
    pub base_ele_m: i16,
    /// Elevation (m) at `end_m`, the summit.
    pub top_ele_m: i16,
    /// Net gain `top_ele_m - base_ele_m` (m). Cached so the ride UI and the largest-gain cap
    /// don't recompute it.
    pub gain_m: u16,
    /// Average grade over the climb, whole percent (`gain_m * 100 / len_m`). A summary figure
    /// for the climb readout; the per-point grade lives in the C2 detail profile.
    pub avg_grade_pct: i16,
    /// Difficulty category placeholder — **reserved, unused this iteration**. Holds a raw
    /// `gain² / len` difficulty score (saturated into the byte); the Cat 4..HC label mapping is
    /// a later sub-issue. Zero until then carries no meaning callers should read.
    pub category: u8,
}

impl ClimbSeg {
    /// Length of the climb along the route (m). Always ≥ 1 for a kept climb (the [`MIN_LEN`]
    /// gate guarantees it), so callers can divide by it without guarding zero.
    #[inline]
    pub fn len_m(&self) -> u32 {
        self.end_m.saturating_sub(self.start_m)
    }
}

/// The resident list of a route's detected climbs, in route order (ascending `start_m`) and
/// non-overlapping. Built once per route by [`RouteReader::detect_climbs`] and cached; the ride
/// loop then queries it with [`active_at`](Self::active_at) per frame.
///
/// Capacity is fixed at [`MAX_CLIMBS`]; a route with more climbs keeps the largest-gain ones, so
/// the list never overflows or panics regardless of route length.
#[derive(Debug, Clone, Default)]
pub struct Climbs(pub Vec<ClimbSeg, MAX_CLIMBS>);

impl Climbs {
    /// An empty list (a flat route, or before detection).
    #[inline]
    pub fn new() -> Self {
        Climbs(Vec::new())
    }

    /// The detected climbs in route order.
    #[inline]
    pub fn as_slice(&self) -> &[ClimbSeg] {
        &self.0
    }

    /// Number of detected climbs.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the route has no detected climbs.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Index of the climb whose interval contains `progress_m`, if any — a **raw** interval
    /// lookup (`start_m ≤ progress ≤ end_m`) with no enter/exit hysteresis. The on-climb margin
    /// that keeps the "climbing now" banner from flickering at a boundary is applied at the C3
    /// call site, not here, so the stored intervals stay the exact detected geometry.
    ///
    /// Climbs are non-overlapping and ordered, so the first containing interval is the answer;
    /// with [`MAX_CLIMBS`] ≤ 24 a linear scan is trivially cheap (no need for a binary search).
    pub fn active_at(&self, progress_m: u32) -> Option<usize> {
        self.0.iter().position(|c| progress_m >= c.start_m && progress_m <= c.end_m)
    }

    /// Insert `seg` keeping the list capped at [`MAX_CLIMBS`] by **largest gain**: while there's
    /// room it's appended; once full, `seg` replaces the smallest-gain climb only if it's
    /// bigger, otherwise it's dropped. The list is re-sorted into route order by the caller
    /// after all inserts (this method leaves it unordered once the cap is hit).
    ///
    /// This is the overflow policy: a pathological route with dozens of climbs keeps the ones a
    /// rider actually cares about instead of whichever happened to come first.
    fn push_keeping_largest(&mut self, seg: ClimbSeg) {
        if self.0.push(seg).is_ok() {
            return;
        }
        // Full: find the current smallest-gain entry and swap `seg` in if it beats it.
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

    /// Re-sort into route order (ascending `start_m`) after largest-gain capping may have left
    /// the tail unordered. A no-op when the cap was never hit (inserts arrive in order).
    fn sort_by_route_order(&mut self) {
        self.0.sort_unstable_by_key(|c| c.start_m);
    }
}

/// A raw sample fed to the segmenter: cumulative distance from the route start (m, `f64` to
/// match the profile's drift-free running total) and the [`DeadBand`]-smoothed elevation (m).
///
/// Bundling the two lets [`segment_climbs`] take one iterator and stay a **pure function** of
/// the stream — unit-testable from a hand-built `Vec` of samples, with no `.obcr` bytes or
/// reader needed. [`RouteReader::detect_climbs`] is then just the adapter that produces this
/// stream from the chunk sweep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElePt {
    /// Cumulative distance from the route start, meters.
    pub dist_m: f64,
    /// Dead-band-smoothed elevation, meters.
    pub ele_m: f32,
}

/// In-flight climb candidate: the pre-climb trough it rises from, its running summit, and how
/// far it's run since that summit without setting a new one (the flat/give-back counters that
/// trigger a close).
#[derive(Debug, Clone, Copy)]
struct Candidate {
    /// Distance (m) at the current pre-climb trough (the lowest point since the last close).
    trough_dist: f64,
    /// Elevation (m) at that trough — the candidate's base.
    trough_ele: f32,
    /// Distance (m) at the running summit (highest point since the trough).
    max_dist: f64,
    /// Elevation (m) at the running summit.
    max_ele: f32,
}

/// The pure hysteresis state machine: fold an ordered `(distance, smoothed-elevation)` stream
/// into the route's kept climbs. This is the whole detection policy; [`RouteReader::detect_climbs`]
/// only feeds it.
///
/// **State.** A single optional [`Candidate`] tracking a trough → running-summit rise, plus the
/// lowest point seen while *not* climbing (the next candidate's base). Each sample either
/// extends the current candidate's summit, or accrues give-back/flat that may close it.
///
/// **Close conditions** (both measured *from the running summit*):
/// - give-back — the elevation has dropped more than [`MAX_DROP`] below the summit (a col), or
/// - flat/false-flat — more than [`MAX_FLAT`] meters have passed without a new summit.
///
/// On close the candidate is **kept** iff gain ≥ [`MIN_GAIN`], average grade ≥ [`MIN_AVG_GRADE`],
/// and length ≥ [`MIN_LEN`]; otherwise discarded. Either way scanning continues from the summit,
/// so a deep col that closes climb 1 becomes the base of a potential climb 2 (the trough is the
/// summit's give-back low, re-derived as new lows arrive).
pub fn segment_climbs<I: IntoIterator<Item = ElePt>>(stream: I) -> Climbs {
    let mut climbs = Climbs::new();
    // Whether the largest-gain cap was ever exercised — only then does the list need a final
    // re-sort into route order (the common case appends in order and skips it).
    let mut capped = false;

    // Candidate currently being tracked (None between climbs / before the first rise).
    let mut cand: Option<Candidate> = None;
    // Lowest point seen while no candidate is open — the trough the *next* candidate opens from.
    // Seeded by the first sample.
    let mut trough: Option<ElePt> = None;

    for p in stream {
        match cand.as_mut() {
            // No open candidate: track the running low, and open a candidate as soon as the
            // profile rises at least the dead-band above it (any real rise — the MIN_* gates are
            // applied at close, not open, so a candidate can grow into a keeper).
            None => {
                let base = match trough {
                    // First sample: it's the low so far, nothing to rise above yet.
                    None => {
                        trough = Some(p);
                        continue;
                    }
                    Some(t) => t,
                };
                if p.ele_m < base.ele_m {
                    // Still descending / flat — this is the new low the next climb rises from.
                    trough = Some(p);
                } else if p.ele_m > base.ele_m {
                    // A rise off the trough: open a candidate anchored at the trough, with this
                    // sample as its first summit.
                    cand = Some(Candidate {
                        trough_dist: base.dist_m,
                        trough_ele: base.ele_m,
                        max_dist: p.dist_m,
                        max_ele: p.ele_m,
                    });
                    trough = None;
                }
            }

            // A candidate is open: either it sets a new summit (climb continues), or the
            // give-back / flat run since the summit may close it.
            Some(c) => {
                if p.ele_m > c.max_ele {
                    // New summit — extend the candidate; the flat/give-back counters reset since
                    // they're measured from the (now newer) summit.
                    c.max_ele = p.ele_m;
                    c.max_dist = p.dist_m;
                } else {
                    // Not climbing: measure give-back below the summit and the flat distance run
                    // past it. Either tripping closes the candidate AT the summit.
                    let give_back = c.max_ele - p.ele_m; // ≥ 0
                    let flat_run = p.dist_m - c.max_dist; // ≥ 0
                    if give_back > MAX_DROP as f32 || flat_run > MAX_FLAT as f64 {
                        // Copy the summit out before releasing the `&mut cand` borrow, so the
                        // close + re-seed below can reassign `cand`.
                        let summit = ElePt { dist_m: c.max_dist, ele_m: c.max_ele };
                        if let Some(seg) = close_candidate(c) {
                            climbs.push_keeping_largest(seg);
                            capped |= climbs.len() == MAX_CLIMBS;
                        }
                        // Continue scanning from the summit. This sample is already below the
                        // summit; it seeds the next trough (a deep col becomes climb 2's base),
                        // while a bridged-but-flat close falls back to the summit itself.
                        cand = None;
                        trough = Some(if p.ele_m < summit.ele_m { p } else { summit });
                    }
                }
            }
        }
    }

    // Stream ended with a candidate still open (route ends on a climb): close it at its summit.
    if let Some(c) = cand {
        if let Some(seg) = close_candidate(&c) {
            climbs.push_keeping_largest(seg);
            capped |= climbs.len() == MAX_CLIMBS;
        }
    }

    if capped {
        climbs.sort_by_route_order();
    }
    climbs
}

/// Turn a closed candidate into a kept [`ClimbSeg`], or `None` if it fails a gate. Applies the
/// [`MIN_GAIN`] / [`MIN_AVG_GRADE`] / [`MIN_LEN`] policy and derives the stored fields; the
/// summit (`max`) is the climb's top and the trough its base.
fn close_candidate(c: &Candidate) -> Option<ClimbSeg> {
    // Gain and length from trough to summit. Negative/zero gain can't reach a candidate here
    // (one only opens on a rise), but clamp defensively so the u16 cast can't wrap.
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
    let len_m = len_f as u32; // ≥ MIN_LEN ≥ 1
                              // Average grade in whole percent, integer math to match the u32 fields. `len_m ≥ MIN_LEN`
                              // so the divide is safe.
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
        category: difficulty_score(gain_m, len_m),
    })
}

/// Raw difficulty score `gain² / len`, saturated into a byte — parked in [`ClimbSeg::category`]
/// for a future Cat 4..HC mapping. `gain² / len` is the classic climb-difficulty shape (a
/// climb's score scales with its steepness and its size); the label thresholds are out of scope
/// here, so this only stores the cheap raw number. Saturates so a giant HC climb doesn't wrap.
fn difficulty_score(gain_m: u32, len_m: u32) -> u8 {
    let score = gain_m.saturating_mul(gain_m) / len_m.max(1);
    score.min(u8::MAX as u32) as u8
}

impl RouteReader<'_> {
    /// Detect the route's climbs in **one streaming pass** over the geometry: decode every
    /// chunk in order (full per-point elevation), accumulate cumulative distance exactly as
    /// [`elevation_profile`](Self::elevation_profile) does, smooth elevation through the shared
    /// [`DeadBand`], and fold the resulting `(distance, elevation)` stream through
    /// [`segment_climbs`].
    ///
    /// O(points), each chunk decoded once — cache the result on route load, don't call it per
    /// frame. Returns at most [`MAX_CLIMBS`] climbs (the largest-gain ones on an unusually
    /// climb-dense route), ordered and non-overlapping.
    ///
    /// The distance metric and dead-band deliberately match the profile's ascent integrator, so
    /// the summed climb gains land near the header's `total_ascent_m` (they won't equal it —
    /// detection drops sub-threshold bumps and the descents between climbs — but a sane
    /// fraction of it).
    pub fn detect_climbs(&self) -> Climbs {
        // The smoothed stream is produced lazily and folded by the pure segmenter, so no
        // intermediate point buffer of the whole route is held — only the current chunk's.
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

/// Lazy iterator that turns [`RouteReader`]'s chunk sweep into the `(distance, smoothed
/// elevation)` [`ElePt`] stream the segmenter consumes — the same decode + cumulative-distance +
/// dead-band the elevation profile uses, yielded one point at a time so the whole route is never
/// buffered.
struct ClimbStream<'a, 'b> {
    reader: &'b RouteReader<'a>,
    /// The current chunk's decoded points (refilled as chunks advance).
    buf: Vec<RoutePoint, MAX_POINTS_PER_CHUNK>,
    /// Index of the chunk currently loaded in `buf`, or the next one to load when `in_chunk`
    /// has consumed the buffer.
    chunk: usize,
    /// Index of the next point to yield within `buf`.
    in_chunk: usize,
    /// Previous point (lon, lat) for the per-segment distance step; `None` at the very start.
    prev: Option<(i32, i32)>,
    /// Cumulative distance (m) as an `f64` running total, re-anchored per chunk to that chunk's
    /// stored `cum_distance_m` — matching the profile so distances can't drift over a long route.
    dist: f64,
    /// The shared elevation dead-band, smoothing across chunk seams (a seam point compares equal
    /// to itself and books nothing, so the smoothing stays one continuous pass).
    smooth: DeadBand<f32>,
}

impl Iterator for ClimbStream<'_, '_> {
    type Item = ElePt;

    fn next(&mut self) -> Option<ElePt> {
        loop {
            // Refill the buffer when the current chunk is exhausted, skipping any that fail to
            // decode (mirrors the profile sweep's `continue` on a decode error).
            if self.in_chunk >= self.buf.len() {
                if self.chunk >= self.reader.chunks().len() {
                    return None;
                }
                let k = self.chunk;
                self.chunk += 1;
                if self.reader.decode_chunk(k, &mut self.buf).is_err() || self.buf.is_empty() {
                    continue;
                }
                // Re-anchor the running distance to this chunk's stored cumulative distance,
                // exactly as `elevation_profile` does. The chunk's first point sits at that
                // distance; `prev` is reset so the seam segment isn't double-measured (the seam
                // point equals the previous chunk's last, contributing zero anyway).
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
            // Smooth the elevation with the shared dead-band; the smoothed reference the
            // segmenter reads is the dead-band's *reference*, not the raw sample, so noise below
            // the band neither opens nor closes a climb.
            self.smooth.push(p.ele as f32);
            return Some(ElePt { dist_m: self.dist, ele_m: self.smooth.smoothed().unwrap_or(p.ele as f32) });
        }
    }
}
