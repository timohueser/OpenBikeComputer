//! Elevation profile: a route's height sampled to a fixed number of columns, as a
//! small **multi-resolution pyramid** so the Statistics screen can zoom and pan the
//! profile without ever re-reading the route geometry.
//!
//! The Statistics screen draws the route as a filled band under a top line, with a
//! "you are here" cursor and stat grid. It needs height-vs-distance, not the lon/lat
//! geometry the Map draws — so [`RouteReader::elevation_profile`] reduces the route to
//! a [`Profile`] of per-column min/max elevation.
//!
//! **Why a pyramid.** Zooming must not re-stream the route on every encoder detent. One
//! load-time pass builds a **fine base** level ([`PROFILE_COLS`] columns); the coarser
//! levels are pure **min/max downsamples** of the finer one (merge adjacent column pairs —
//! a few array passes, no extra chunk decodes), the same trick the OBCM map format uses
//! (v5). Drawing a view then [picks the level](Profile::window) whose resolution matches
//! the visible window and walks only ~chart-width columns, so the per-detent cost is flat
//! across every zoom level and touches no geometry.
//!
//! The resolution is decoupled from any display width: the screen maps columns onto its
//! chart pixels, so the same profile draws to the 240-px device panel or a resized
//! simulator window. Building streams every chunk, so it is built once on route load and
//! cached (see the app's resident `profile`), never rebuilt per frame.

use heapless::Vec;

use crate::deadband::DeadBand;
use crate::geo::seg_dist_m;
use crate::reader::{RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK};

/// Columns in the **finest** (base) level — the resolution one load-time sweep fills, and the
/// cap on zoom-in depth. Coarser levels halve from here, so keep this a power of two (each level
/// must stay even for the pair-merge downsample). The one RAM/zoom-depth knob: doubling it
/// doubles both (~16 KB for the whole pyramid at 2048). `nrf-mem` trims to 256 (~2.3 KB) for the
/// narrow panel — one zoom-in step over the 240-px base view — freeing RAM for the renderer, the
/// ride loop's resident route index/cache, and the BLE stack sharing the 256 KB DK (issue #270).
#[cfg(not(feature = "nrf-mem"))]
pub const PROFILE_COLS: usize = 2048;
#[cfg(feature = "nrf-mem")]
pub const PROFILE_COLS: usize = 256;

/// Per-level column counts, finest first — each a clean halving so the pair-merge downsample
/// lands exactly. On the full profile the coarsest level (256) still covers the 240-px panel,
/// so a full-route draw walks ~256 columns, not the 2048-wide base; on `nrf-mem` the *base*
/// (256) is what just covers the panel — the full-route draw uses it directly and the coarser
/// levels serve only narrower draws ([`Profile::window`] takes the coarsest level that still
/// fills the target pixels, so nothing upsamples chunkily).
const LEVEL_COLS: [usize; 4] = [PROFILE_COLS, PROFILE_COLS / 2, PROFILE_COLS / 4, PROFILE_COLS / 8];
/// Number of pyramid levels (length of [`LEVEL_COLS`]).
const NUM_LEVELS: usize = LEVEL_COLS.len();
/// Total columns across all levels, packed back-to-back in one array (finest first).
const TOTAL_COLS: usize = sum_levels();

/// Resolution of the cumulative-ascent curve. Kept separate from (and coarser than) the
/// band pyramid: ascent feeds only the live "to climb" stat, never the zoom drawing, so
/// it needs no extra detail and pays no extra RAM as the base grows.
const ASCENT_COLS: usize = 256;

/// Sum of [`LEVEL_COLS`] — a `const fn` so [`TOTAL_COLS`] tracks the table automatically.
const fn sum_levels() -> usize {
    let mut total = 0;
    let mut i = 0;
    while i < LEVEL_COLS.len() {
        total += LEVEL_COLS[i];
        i += 1;
    }
    total
}

/// Offset of `level`'s columns within the packed [`Profile::cols`] array.
const fn level_offset(level: usize) -> usize {
    let mut off = 0;
    let mut i = 0;
    while i < level {
        off += LEVEL_COLS[i];
        i += 1;
    }
    off
}

/// The visible slice of the profile a zoomed/panned view should draw: which pyramid
/// `level` to read and the fractional `[lo_frac, hi_frac]` route span it covers. Returned
/// by [`Profile::window`]; the screen maps each chart pixel to a fraction in this span and
/// reads the band via [`Profile::sample`] at `level`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    /// Pyramid level to sample (0 = finest). Picked so the window holds ≥ the chart's
    /// pixel width in columns — enough detail without walking more than ~chart-width.
    pub level: usize,
    /// Fractional start of the visible route span (`0.0` = route start).
    pub lo_frac: f32,
    /// Fractional end of the visible route span (`1.0` = route end).
    pub hi_frac: f32,
}

/// A route's elevation profile as a multi-resolution pyramid: per-column min/max height at
/// several resolutions, plus the y-axis range, the peak, and a cumulative-ascent curve —
/// everything the Statistics screen draws at any zoom without re-reading the route. Build with
/// [`RouteReader::elevation_profile`] and cache it.
#[derive(Debug, Clone)]
pub struct Profile {
    /// All pyramid levels packed finest-first (`level_offset`/`LEVEL_COLS` index in).
    /// Each column is `(min_ele_m, max_ele_m)`; the base level's empty columns inherit
    /// the nearest filled neighbour (gap-free at any sampling density), and coarser
    /// levels are min/max merges of it, so every level is gap-free too.
    cols: [(i16, i16); TOTAL_COLS],
    /// Cumulative route ascent (m) through each column — monotonic non-decreasing,
    /// normalized so the last column equals the route's total ascent. Computed from the
    /// per-point elevations at [`ASCENT_COLS`] resolution (not the coarse per-chunk
    /// samples), so "to climb" is correct even on a route with few chunks.
    cum_ascent: [u32; ASCENT_COLS],
    /// Lowest/highest elevation over the whole route (the y-axis range), from the route
    /// header. Equal for a perfectly flat route — callers guard the zero-height span.
    pub min_ele_m: i16,
    pub max_ele_m: i16,
    /// Base-level column of the highest point, for placing the peak label / readout.
    pub peak_col: usize,
}

impl Profile {
    /// The base (finest) level's per-column `(min, max)` elevations — always
    /// [`PROFILE_COLS`] long. The fully-detailed band; zoomed views use [`sample`] /
    /// [`window`] instead so they can read a coarser level when zoomed out.
    ///
    /// [`sample`]: Profile::sample
    /// [`window`]: Profile::window
    #[inline]
    pub fn cols(&self) -> &[(i16, i16)] {
        self.cols_at(0)
    }

    /// One pyramid level's columns (`0` = finest). Panics only on an out-of-range level,
    /// which the crate's own callers never pass.
    #[inline]
    fn cols_at(&self, level: usize) -> &[(i16, i16)] {
        let off = level_offset(level);
        &self.cols[off..off + LEVEL_COLS[level]]
    }

    /// The `(min, max)` elevation at fractional position `t` along the route
    /// (`0.0` = start, `1.0` = end) on the **base** level — for the "you are here"
    /// cursor's readout and the grade window.
    #[inline]
    pub fn at(&self, t: f32) -> (i16, i16) {
        self.sample(0, t)
    }

    /// The `(min, max)` elevation at fractional position `t` on a given pyramid `level`
    /// — the zoom-aware read the screen uses to draw a [`Window`]'s band column by column.
    #[inline]
    pub fn sample(&self, level: usize, t: f32) -> (i16, i16) {
        let level = level.min(NUM_LEVELS - 1);
        let cols = self.cols_at(level);
        let last = cols.len() - 1;
        let col = (t.clamp(0.0, 1.0) * last as f32) as usize;
        cols[col.min(last)]
    }

    /// Peak elevation in meters (the max at [`peak_col`](Profile::peak_col)).
    #[inline]
    pub fn peak_ele_m(&self) -> i16 {
        self.cols[self.peak_col].1
    }

    /// The peak's fractional position along the route (`0.0`–`1.0`) — for placing the
    /// peak readout relative to the live cursor regardless of zoom.
    #[inline]
    pub fn peak_frac(&self) -> f32 {
        self.peak_col as f32 / (PROFILE_COLS - 1) as f32
    }

    /// Cumulative ascent (m) climbed by fractional position `t` along the route
    /// (`0.0` = start, `1.0` = end) — for "to climb" (`total_ascent - ascent_to`).
    /// Interpolated at [`ASCENT_COLS`] resolution and normalized so `ascent_to(1.0)` is
    /// exactly the route's total ascent.
    #[inline]
    pub fn ascent_to(&self, t: f32) -> u32 {
        let last = ASCENT_COLS - 1;
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

    /// Pick the pyramid [`Window`] to draw for a view centered on `center_frac` at zoom
    /// factor `zoom` (`1.0` = whole route, larger = closer), into a chart `target_px`
    /// wide.
    ///
    /// The visible span is `1/zoom` of the route, clamped to stay within `[0, 1]`. The
    /// level is the **coarsest** one that still puts at least `target_px` columns inside
    /// that span — so the draw has a source column per pixel without walking more than
    /// ~`2·target_px`. Pure arithmetic over the cached pyramid: no geometry is read, so
    /// this is cheap to call per detent.
    pub fn window(&self, center_frac: f32, zoom: f32, target_px: u32) -> Window {
        let zoom = zoom.max(1.0);
        let span = (1.0 / zoom).min(1.0);
        let half = span * 0.5;
        // Clamp the centre so the fixed-width span never runs off either end.
        let lo = (center_frac - half).clamp(0.0, 1.0 - span);
        let hi = (lo + span).min(1.0);

        // Coarsest level first; the first that holds ≥ target_px columns in the span wins
        // (fewest columns to walk at adequate detail). Falls through to the finest level
        // when zoomed in past what even the base resolves.
        let mut level = 0;
        for l in (0..NUM_LEVELS).rev() {
            if span * LEVEL_COLS[l] as f32 >= target_px as f32 {
                level = l;
                break;
            }
        }
        Window { level, lo_frac: lo, hi_frac: hi }
    }
}

impl RouteReader<'_> {
    /// Build the route's elevation [`Profile`] by streaming every chunk in order and
    /// bucketing each point into a base-level column by its cumulative distance from the
    /// start, then downsampling the base into the coarser pyramid levels.
    ///
    /// Each chunk re-anchors to its stored
    /// [`cum_distance_m`](crate::ChunkMeta::cum_distance_m) and accumulates per-segment
    /// distance within the chunk using the same metric the converter did, so column
    /// placement matches the format's distance exactly and can't drift over a long
    /// route. O(points), reading each chunk once — cache the result, don't call it per
    /// frame.
    pub fn elevation_profile(&self) -> Profile {
        // Sentinel for "no point landed here": an empty column has min > max. Only the
        // base level is filled by the sweep; the coarser levels are derived after.
        let mut cols = [(i16::MAX, i16::MIN); TOTAL_COLS];
        // Running dead-banded ascent recorded at the last point of each ascent column
        // (0 = none yet); carried forward and scaled into `cum_ascent` below.
        let mut casc = [0f32; ASCENT_COLS];
        let total = self.total_distance_m.max(1) as f64;
        let base_last = PROFILE_COLS - 1;
        let asc_last = ASCENT_COLS - 1;

        // One sweep over the whole route: bucket each point into its distance column,
        // updating that column's elevation band and the continuous ascent integrator. The
        // integrator runs *across* chunk seams (a chunk's shared seam point compares equal
        // to itself, contributing nothing), so it stays one continuous pass.
        let mut ascent = DeadBand::<f32>::new();
        let mut buf: Vec<RoutePoint, MAX_POINTS_PER_CHUNK> = Vec::new();
        let n = self.chunks().len();
        for k in 0..n {
            if self.decode_chunk(k, &mut buf).is_err() {
                continue;
            }
            // The chunk's first point sits at its cumulative distance; the rest add up
            // segment by segment from there. Like the converter, accumulate the small
            // per-segment `f32` distances into an `f64` running total so a long route's
            // column placement can't drift (the two must match exactly — same metric).
            let mut dist = self.chunks()[k].cum_distance_m as f64;
            let mut prev: Option<(i32, i32)> = None;
            for p in &buf {
                if let Some(pr) = prev {
                    dist += seg_dist_m(pr, (p.lon, p.lat)) as f64;
                }
                prev = Some((p.lon, p.lat));
                let frac = dist / total;
                let col = ((frac * base_last as f64) as usize).min(base_last);
                let slot = &mut cols[col];
                slot.0 = slot.0.min(p.ele);
                slot.1 = slot.1.max(p.ele);
                // Record the running ascent at this column (later points in the same column
                // overwrite, so it ends on the correct value).
                let acol = ((frac * asc_last as f64) as usize).min(asc_last);
                ascent.push(p.ele as f32);
                casc[acol] = ascent.ascent();
            }
        }

        fill_gaps(&mut cols[..PROFILE_COLS], (self.min_ele_m, self.max_ele_m));
        downsample_levels(&mut cols);
        let cum_ascent = cumulative_ascent(&casc, self.total_ascent_m);
        let peak_col = peak_column(&cols[..PROFILE_COLS]);

        Profile { cols, cum_ascent, min_ele_m: self.min_ele_m, max_ele_m: self.max_ele_m, peak_col }
    }
}

/// Build the coarser pyramid levels in place: each level's column is the min/max merge of
/// the two columns below it. The base level (already gap-filled) is read by level 1, level
/// 1 by level 2, and so on — so every coarser level is gap-free without its own fill.
fn downsample_levels(cols: &mut [(i16, i16); TOTAL_COLS]) {
    for l in 1..NUM_LEVELS {
        let src_off = level_offset(l - 1);
        let dst_off = level_offset(l);
        let dst_n = LEVEL_COLS[l];
        // Source (level l-1) sits entirely before the destination (level l) in the packed
        // array, so split at the destination to borrow both at once.
        let (left, right) = cols.split_at_mut(dst_off);
        let src = &left[src_off..src_off + LEVEL_COLS[l - 1]];
        let dst = &mut right[..dst_n];
        for (j, d) in dst.iter_mut().enumerate() {
            let a = src[2 * j];
            let b = src[2 * j + 1];
            *d = (a.0.min(b.0), a.1.max(b.1));
        }
    }
}

/// Turn the per-column running ascent (`casc`, set only where points landed) into a
/// gap-free, monotonic-non-decreasing cumulative-ascent curve, scaled so the final column
/// equals the header's exact `total_ascent_m` (so "to climb" reaches 0 at the route's end).
fn cumulative_ascent(casc: &[f32; ASCENT_COLS], total_ascent_m: u32) -> [u32; ASCENT_COLS] {
    let last_col = ASCENT_COLS - 1;
    // Carry the running value across empty columns, keeping the curve non-decreasing.
    let mut raw = [0f32; ASCENT_COLS];
    let mut run = 0f32;
    for i in 0..ASCENT_COLS {
        run = run.max(casc[i]);
        raw[i] = run;
    }
    // Scale to the header's exact total, then pin the endpoint so rounding can't miss it.
    let mut cum = [0u32; ASCENT_COLS];
    if raw[last_col] > 0.0 {
        let scale = total_ascent_m as f32 / raw[last_col];
        for i in 0..ASCENT_COLS {
            cum[i] = (raw[i] * scale) as u32;
        }
    }
    cum[last_col] = total_ascent_m;
    cum
}

/// The column index of the route's highest point (for placing the peak label).
fn peak_column(cols: &[(i16, i16)]) -> usize {
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
fn fill_gaps(cols: &mut [(i16, i16)], fallback: (i16, i16)) {
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
