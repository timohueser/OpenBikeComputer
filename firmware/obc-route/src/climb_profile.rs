//! Per-climb detail elevation profile: one detected [`ClimbSeg`] sampled into a small fixed buffer
//! of columns, scoped to that climb's interval.
//!
//! The whole-route [`Profile`](crate::profile) decimates the entire route to its columns, so a
//! 5 km climb on a 100 km route lands in a handful of them. This module re-buckets one climb into
//! [`COLS`] columns, so a climb of any length gets the same detail without growing the route
//! profile.
//!
//! The primary API is [`ClimbProfile::fill`], which writes into a pre-placed buffer: the device
//! stack cannot carry a large `[i16; COLS]` temporary on the hot path.
//!
//! Only the chunks whose distance span intersects the climb are decoded, so entering climb 5 of a
//! route never decodes chunk 0.
//!
//! Grade is derived, not stored: [`grade_at`](ClimbProfile::grade_at) computes it from a small
//! window of columns at draw time, which halves the buffer and lets the screen pick its own
//! smoothing window.

use crate::climb::ClimbSeg;
use crate::profile::fill_gaps;
use crate::reader::{RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK};
use heapless::Vec;
use obc_map_scene::ground_dist_m;

/// Columns in one climb's detail buffer, one elevation sample each. At `i16` per column this is
/// ~400 B, a fixed cost whatever the climb's length, so a long pass and a short ramp get the same
/// detail. It is not tied to any display width: the screen maps these columns onto its pixels.
pub const COLS: usize = 200;

/// Sentinel for an unfilled column, outside any real height. [`fill`](ClimbProfile::fill) seeds
/// every column with it, then gap-fills through the shared [`fill_gaps`](crate::profile) carry.
pub(crate) const EMPTY: i16 = i16::MIN;

/// One climb's elevation profile: [`COLS`] height samples bucketed by within-climb distance.
/// Column `0` is the base and the last column the summit.
///
/// The app keeps one resident and refills it on climb entry. The screen-facing reads are index
/// math over the buffer, so they are cheap per frame.
#[derive(Debug, Clone)]
pub struct ClimbProfile {
    /// Per-column elevation (m). Gap-free after a [`fill`](Self::fill); a new profile holds a
    /// flat line at the base, so a draw before any geometry still has a shape.
    cols: [i16; COLS],
    /// The climb's base distance from the route start, which
    /// [`cursor_frac`](Self::cursor_frac) subtracts to map a live progress into `[0, 1]`.
    start_m: u32,
    /// The climb's length (m), cached so the cursor mapping does not re-read the seg.
    len_m: u32,
    /// Column `0` is pinned to this, so the base reads the detected trough wherever the first
    /// geometry point landed.
    base_ele_m: i16,
    /// The last column is pinned to this, for the same reason.
    top_ele_m: i16,
}

impl Default for ClimbProfile {
    fn default() -> Self {
        Self::new()
    }
}

impl ClimbProfile {
    /// An empty profile: a flat zero line over a zero-length climb, so a draw before the first
    /// [`fill`](Self::fill) is harmless. It is `const` so the device can place it without a large
    /// stack temporary.
    pub const fn new() -> Self {
        ClimbProfile { cols: [0; COLS], start_m: 0, len_m: 0, base_ele_m: 0, top_ele_m: 0 }
    }

    /// Fill this profile in place for climb `seg`, reading only the chunks whose distance span
    /// overlaps it.
    ///
    /// The sweep is [`elevation_profile`](crate::profile)'s, scoped to one climb: each overlapping
    /// chunk re-anchors at its stored
    /// [`cum_distance_m`](crate::ChunkMeta::cum_distance_m), so column placement matches the
    /// format's metric. Empty columns are gap-filled and the endpoints are pinned to the seg's own
    /// base and top.
    pub fn fill(&mut self, reader: &RouteReader, seg: &ClimbSeg) {
        let start = seg.start_m;
        let end = seg.end_m;
        let len = seg.len_m().max(1);

        self.start_m = start;
        self.len_m = len;
        self.base_ele_m = seg.base_ele_m;
        self.top_ele_m = seg.top_ele_m;

        // A climb with no decodable geometry falls through to a flat base line.
        self.cols = [EMPTY; COLS];
        let last_col = COLS - 1;
        let len_f = len as f64;

        let mut buf: Vec<RoutePoint, MAX_POINTS_PER_CHUNK> = Vec::new();
        let chunks = reader.chunks();
        let n = chunks.len();
        for k in 0..n {
            // The last chunk runs to the route's total distance.
            let chunk_start = chunks[k].cum_distance_m;
            let chunk_end = if k + 1 < n { chunks[k + 1].cum_distance_m } else { reader.total_distance_m };
            // A chunk that only touches the climb at one distance carries no interior point of
            // it, and the endpoints are pinned to the seg anyway, so it is skipped.
            if chunk_end <= start || chunk_start >= end {
                continue;
            }

            if reader.decode_chunk(k, &mut buf).is_err() {
                continue;
            }
            // Re-anchor the running distance to this chunk's stored value, so placement cannot
            // drift. `prev` resets per chunk; the seam point contributes zero.
            let mut dist = chunk_start as f64;
            let mut prev: Option<(i32, i32)> = None;
            for p in &buf {
                if let Some(pr) = prev {
                    dist += ground_dist_m(pr, (p.lon, p.lat)) as f64;
                }
                prev = Some((p.lon, p.lat));
                let d = dist as u32;
                if d < start || d > end {
                    continue;
                }
                let within = (dist - start as f64) / len_f;
                let col = ((within * last_col as f64) as usize).min(last_col);
                // A later point in the same column overwrites this one. Unlike the whole-route
                // profile this keeps a single sample, not a band: the screen draws a line.
                self.cols[col] = p.ele;
            }
        }

        // Pinned before the gap-fill, so the base and top read the seg's values even when no
        // point landed in either end column.
        self.cols[0] = seg.base_ele_m;
        self.cols[last_col] = seg.top_ele_m;
        fill_gaps(&mut self.cols, seg.base_ele_m, |c| *c != EMPTY);
    }

    /// A fresh profile filled for `seg`. The device uses [`new`](Self::new) and
    /// [`fill`](Self::fill) on its one resident buffer instead; this by-value build is for hosts
    /// and tests.
    pub fn build(reader: &RouteReader, seg: &ClimbSeg) -> Self {
        let mut p = Self::new();
        p.fill(reader, seg);
        p
    }

    /// A profile whose columns rise linearly from base to top, with no geometry and no
    /// [`RouteReader`]: the synthetic constant-grade climb a UI test checks its tile maths
    /// against, without staging an `.obcr` fixture.
    pub fn from_linear_ramp(seg: &ClimbSeg) -> Self {
        let mut p = Self::new();
        p.start_m = seg.start_m;
        p.len_m = seg.len_m().max(1);
        p.base_ele_m = seg.base_ele_m;
        p.top_ele_m = seg.top_ele_m;
        let last = COLS - 1;
        let (base, gain) = (seg.base_ele_m as f32, (seg.top_ele_m - seg.base_ele_m) as f32);
        for (i, c) in p.cols.iter_mut().enumerate() {
            *c = (base + gain * (i as f32 / last as f32)) as i16;
        }
        p
    }

    /// Elevation (m) at within-climb fraction `frac`, clamped, so an out-of-range cursor reads
    /// the nearest endpoint.
    #[inline]
    pub fn at(&self, frac: f32) -> i16 {
        let last = COLS - 1;
        let col = (frac.clamp(0.0, 1.0) * last as f32) as usize;
        self.cols[col.min(last)]
    }

    /// Elevation (m) at within-climb column `col`, clamped.
    #[inline]
    pub fn col(&self, col: usize) -> i16 {
        self.cols[col.min(COLS - 1)]
    }

    /// The whole per-column elevation buffer, base to summit.
    #[inline]
    pub fn cols(&self) -> &[i16] {
        &self.cols
    }

    /// Local grade, whole signed percent, at within-climb fraction `frac`.
    ///
    /// A one-column difference would be dominated by per-column quantization noise, so this is a
    /// centred finite difference over [`GRADE_WIN`] columns each side. That smooths the jitter and
    /// stays local enough to show where a climb ramps up.
    pub fn grade_at(&self, frac: f32) -> i32 {
        let last = COLS - 1;
        let center = (frac.clamp(0.0, 1.0) * last as f32) as usize;
        // Clamped to the buffer ends, so an endpoint uses a one-sided slope.
        let lo = center.saturating_sub(GRADE_WIN);
        let hi = (center + GRADE_WIN).min(last);
        if hi == lo {
            return 0;
        }
        let d_ele = self.cols[hi] as i32 - self.cols[lo] as i32;
        // The ground distance the window spans is its column fraction of the climb length.
        let span_cols = (hi - lo) as u32;
        let d_dist = (self.len_m as u64 * span_cols as u64 / last as u64).max(1) as i32;
        d_ele * 100 / d_dist
    }

    /// Map a live route progress to the within-climb fraction, clamped, so a cursor just outside
    /// the climb sits at the nearest end rather than off-screen.
    #[inline]
    pub fn cursor_frac(&self, progress_m: u32) -> f32 {
        // A never-filled profile has no interval to map into, so the cursor pins at the base
        // rather than overshooting to 1.0.
        if self.len_m == 0 {
            return 0.0;
        }
        let into = progress_m.saturating_sub(self.start_m);
        (into as f32 / self.len_m as f32).clamp(0.0, 1.0)
    }

    /// The climb's base distance (m) from the route start.
    #[inline]
    pub fn start_m(&self) -> u32 {
        self.start_m
    }

    /// The climb's length (m).
    #[inline]
    pub fn len_m(&self) -> u32 {
        self.len_m
    }

    #[inline]
    pub fn base_ele_m(&self) -> i16 {
        self.base_ele_m
    }

    #[inline]
    pub fn top_ele_m(&self) -> i16 {
        self.top_ele_m
    }
}

/// Columns to reach on each side for the [`grade_at`](ClimbProfile::grade_at) difference. About
/// 3.5 % of the climb each way: enough to smooth quantization, not enough to blur where the grade
/// changes.
const GRADE_WIN: usize = 3;
