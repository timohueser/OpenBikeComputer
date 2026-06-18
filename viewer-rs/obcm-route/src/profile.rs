//! Elevation profile: a route's height sampled to a fixed number of columns.
//!
//! The Elevation screen draws the route as a filled band under a top line, with a
//! "you are here" / scrub cursor and a peak label. It needs height-vs-distance, not
//! the lon/lat geometry the Map draws — so [`RouteReader::elevation_profile`] reduces
//! the route to a fixed-width [`Profile`] of per-column min/max elevation.
//!
//! The resolution ([`PROFILE_COLS`]) is decoupled from any display width: the screen
//! maps these columns onto its chart pixels, so the same profile draws to the 240-px
//! device panel or a resized simulator window. Building one streams every chunk, so it
//! is built once on route load and cached (see the app's resident `profile`), never
//! rebuilt per frame.

use heapless::Vec;

use crate::geo::seg_dist_m;
use crate::reader::{RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK};

/// Number of columns a route is sampled into. A fixed resolution (≈1 KB of `i16`
/// pairs) independent of the panel width — fine enough that a 240-px chart shows
/// detail, coarse enough to stay cheap to build and store on the MCU.
pub const PROFILE_COLS: usize = 256;

/// Climb dead-band (m) for the cumulative-ascent profile — ignore wiggles below this so
/// the per-column ascent matches the converter's intent. Mirrors `convert.rs`'s
/// `ELE_THRESHOLD_M`.
const ELE_THRESHOLD_M: f32 = 3.0;

/// A route's elevation profile: per-column min/max height plus the y-axis range and
/// the peak — everything the Elevation screen needs to draw without re-reading the
/// route. Build with [`RouteReader::elevation_profile`] and cache it.
#[derive(Debug, Clone)]
pub struct Profile {
    /// Per-column `(min_ele_m, max_ele_m)`. Columns that caught no route point inherit
    /// the nearest filled neighbour, so the band is gap-free at any sampling density.
    cols: [(i16, i16); PROFILE_COLS],
    /// Cumulative route ascent (m) through each column — monotonic non-decreasing,
    /// normalized so the last column equals the route's total ascent. Computed from the
    /// per-point elevations at column resolution (not the coarse per-chunk samples), so
    /// "to climb" is correct even on a route with few chunks: at the top of a climb it
    /// reads ~total (no phantom remaining), and flat/descending stretches don't add.
    cum_ascent: [u32; PROFILE_COLS],
    /// Lowest/highest elevation over the whole route (the y-axis range), from the route
    /// header. Equal for a perfectly flat route — callers guard the zero-height span.
    pub min_ele_m: i16,
    pub max_ele_m: i16,
    /// Column of the highest point, for placing the peak label.
    pub peak_col: usize,
}

impl Profile {
    /// The per-column `(min, max)` elevations — always [`PROFILE_COLS`] long.
    #[inline]
    pub fn cols(&self) -> &[(i16, i16)] {
        &self.cols
    }

    /// The `(min, max)` elevation at fractional position `t` along the route
    /// (`0.0` = start, `1.0` = end) — for the scrub cursor / "you are here" marker.
    #[inline]
    pub fn at(&self, t: f32) -> (i16, i16) {
        let last = PROFILE_COLS - 1;
        let col = (t.clamp(0.0, 1.0) * last as f32) as usize;
        self.cols[col.min(last)]
    }

    /// Peak elevation in meters (the max at [`peak_col`](Profile::peak_col)).
    #[inline]
    pub fn peak_ele_m(&self) -> i16 {
        self.cols[self.peak_col].1
    }

    /// Cumulative ascent (m) climbed by fractional position `t` along the route
    /// (`0.0` = start, `1.0` = end) — for "to climb" (`total_ascent - ascent_to`).
    /// Interpolated at column resolution and normalized so `ascent_to(1.0)` is exactly
    /// the route's total ascent.
    #[inline]
    pub fn ascent_to(&self, t: f32) -> u32 {
        let last = PROFILE_COLS - 1;
        let x = t.clamp(0.0, 1.0) * last as f32;
        let i = x as usize;
        if i >= last {
            return self.cum_ascent[last];
        }
        let f = x - i as f32;
        let a = self.cum_ascent[i] as f32;
        let b = self.cum_ascent[i + 1] as f32;
        (a + (b - a) * f) as u32
    }
}

impl RouteReader<'_> {
    /// Build the route's elevation [`Profile`] by streaming every chunk in order and
    /// bucketing each point into a column by its cumulative distance from the start.
    ///
    /// Each chunk re-anchors to its stored
    /// [`cum_distance_m`](crate::ChunkMeta::cum_distance_m) and accumulates per-segment
    /// distance within the chunk using the same metric the converter did, so column
    /// placement matches the format's distance exactly and can't drift over a long
    /// route. O(points), reading each chunk once — cache the result, don't call it per
    /// frame.
    pub fn elevation_profile(&self) -> Profile {
        // Sentinel for "no point landed here": an empty column has min > max.
        let mut cols = [(i16::MAX, i16::MIN); PROFILE_COLS];
        // Running dead-banded ascent recorded at the last point of each column (0 = none
        // yet); carried forward and scaled into `cum_ascent` below.
        let mut casc = [0f32; PROFILE_COLS];
        let total = self.total_distance_m.max(1) as f64;
        let last_col = PROFILE_COLS - 1;

        // One sweep over the whole route: bucket each point into its distance column,
        // updating that column's elevation band and the continuous ascent integrator. The
        // integrator runs *across* chunk seams (a chunk's shared seam point compares equal
        // to itself, contributing nothing), so it stays one continuous pass.
        let mut ascent = AscentDeadband::new();
        let mut buf: Vec<RoutePoint, MAX_POINTS_PER_CHUNK> = Vec::new();
        let n = self.chunks().len();
        for k in 0..n {
            if self.decode_chunk(k, &mut buf).is_err() {
                continue;
            }
            // The chunk's first point sits at its cumulative distance; the rest add up
            // segment by segment from there.
            let mut dist = self.chunks()[k].cum_distance_m as f64;
            let mut prev: Option<(i32, i32)> = None;
            for p in &buf {
                if let Some(pr) = prev {
                    dist += seg_dist_m(pr, (p.lon, p.lat));
                }
                prev = Some((p.lon, p.lat));
                let col = (((dist / total) * last_col as f64) as usize).min(last_col);
                let slot = &mut cols[col];
                slot.0 = slot.0.min(p.ele);
                slot.1 = slot.1.max(p.ele);
                // Record the running ascent at this column (later points in the same column
                // overwrite, so it ends on the correct value).
                casc[col] = ascent.push(p.ele as f32);
            }
        }

        fill_gaps(&mut cols, (self.min_ele_m, self.max_ele_m));
        let cum_ascent = cumulative_ascent(&casc, self.total_ascent_m);
        let peak_col = peak_column(&cols);

        Profile { cols, cum_ascent, min_ele_m: self.min_ele_m, max_ele_m: self.max_ele_m, peak_col }
    }
}

/// Running dead-banded ascent: feed each point's elevation in route order, read back the
/// cumulative climb so far. Changes smaller than [`ELE_THRESHOLD_M`] neither count nor move
/// the reference, so sampling/sensor wiggle doesn't inflate the total — the same dead-band
/// the converter applies when precomputing the route's ascent.
struct AscentDeadband {
    /// Reference elevation the next sample is compared against; `None` before the first.
    ref_ele: Option<f32>,
    /// Cumulative climb (m) past the dead-band so far.
    total: f32,
}

impl AscentDeadband {
    fn new() -> Self {
        AscentDeadband { ref_ele: None, total: 0.0 }
    }

    /// Integrate one elevation sample, returning the running cumulative ascent.
    fn push(&mut self, e: f32) -> f32 {
        match self.ref_ele {
            None => self.ref_ele = Some(e),
            Some(r) => {
                let d = e - r;
                if d >= ELE_THRESHOLD_M {
                    self.total += d;
                    self.ref_ele = Some(e);
                } else if d <= -ELE_THRESHOLD_M {
                    self.ref_ele = Some(e);
                }
            }
        }
        self.total
    }
}

/// Turn the per-column running ascent (`casc`, set only where points landed) into a
/// gap-free, monotonic-non-decreasing cumulative-ascent curve, scaled so the final column
/// equals the header's exact `total_ascent_m` (so "to climb" reaches 0 at the route's end).
fn cumulative_ascent(casc: &[f32; PROFILE_COLS], total_ascent_m: u32) -> [u32; PROFILE_COLS] {
    let last_col = PROFILE_COLS - 1;
    // Carry the running value across empty columns, keeping the curve non-decreasing.
    let mut raw = [0f32; PROFILE_COLS];
    let mut run = 0f32;
    for i in 0..PROFILE_COLS {
        run = run.max(casc[i]);
        raw[i] = run;
    }
    // Scale to the header's exact total, then pin the endpoint so rounding can't miss it.
    let mut cum = [0u32; PROFILE_COLS];
    if raw[last_col] > 0.0 {
        let scale = total_ascent_m as f32 / raw[last_col];
        for i in 0..PROFILE_COLS {
            cum[i] = (raw[i] * scale) as u32;
        }
    }
    cum[last_col] = total_ascent_m;
    cum
}

/// The column index of the route's highest point (for placing the peak label).
fn peak_column(cols: &[(i16, i16); PROFILE_COLS]) -> usize {
    let mut peak_col = 0;
    let mut peak = i16::MIN;
    for (i, c) in cols.iter().enumerate() {
        if c.1 > peak {
            peak = c.1;
            peak_col = i;
        }
    }
    peak_col
}

/// Make `cols` gap-free: each empty column (sentinel `min > max`) inherits the nearest
/// filled column — forward first, then backward for any leading gap. A route with no
/// points at all falls back to the header `(min, max)` so the band still has a shape.
fn fill_gaps(cols: &mut [(i16, i16); PROFILE_COLS], fallback: (i16, i16)) {
    let is_set = |c: &(i16, i16)| c.0 <= c.1;

    let mut last: Option<(i16, i16)> = None;
    for c in cols.iter_mut() {
        if is_set(c) {
            last = Some(*c);
        } else if let Some(v) = last {
            *c = v;
        }
    }
    // Backward carry fills columns before the first set one (forward carry can't reach).
    let mut next: Option<(i16, i16)> = None;
    for c in cols.iter_mut().rev() {
        if is_set(c) {
            next = Some(*c);
        } else if let Some(v) = next {
            *c = v;
        }
    }
    // Only reachable when the route had no decodable points.
    let fallback = (fallback.0.min(fallback.1), fallback.0.max(fallback.1));
    for c in cols.iter_mut() {
        if !is_set(c) {
            *c = fallback;
        }
    }
}
