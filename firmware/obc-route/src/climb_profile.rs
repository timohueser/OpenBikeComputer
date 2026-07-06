//! Per-climb detail elevation profile: one detected [`ClimbSeg`] sampled to a small fixed
//! buffer of elevation columns, scoped to just that climb's `[start_m, end_m]` interval.
//!
//! **Why a separate profile from [`Profile`](crate::profile).** The whole-route
//! [`Profile`] is decimated to [`PROFILE_COLS`](crate::profile::PROFILE_COLS) columns for the
//! *entire* route — 256 on `nrf-mem`. A single 5 km climb on a 100 km route lands in ~12 of
//! those columns: far too coarse for the ClimbPro-style striped panel the Climb screen (C4)
//! draws. This module re-buckets **one climb** into [`COLS`] columns, so a climb of any length
//! gets the same detail (~climb_len / 200 m per column), without growing `PROFILE_COLS` (the
//! ~10 kB we're deliberately avoiding).
//!
//! **Why fill-in-place.** The app (C3) keeps a *single* resident [`ClimbProfile`] and refills
//! it only on climb *entry* — not per frame. So the primary API is [`ClimbProfile::fill`],
//! which writes into a pre-placed buffer; there is no large `[i16; COLS]` returned or moved on
//! the hot path. (This crate has a documented ~36 kB device stack ceiling and a history of
//! HardFaults from large temporaries — see the `nrf54l-await-stack-trap` notes.)
//!
//! **Why only overlapping chunks are read.** A mid-route climb touches only the geometry chunks
//! whose distance span intersects `[start_m, end_m]`. Using the chunk index's
//! [`cum_distance_m`](crate::ChunkMeta::cum_distance_m) (each chunk's start distance) we skip
//! every non-overlapping chunk *without decoding it* — so entering climb 5 of a route never
//! decodes chunk 0. That cheap, bounded rebuild is the whole point of scoping to one climb.
//!
//! Grade is **derived, not stored**: one `i16` per column is all that's kept, and
//! [`grade_at`](ClimbProfile::grade_at) computes the local grade from a small window of columns
//! at draw time. That halves the buffer versus storing an `i8` grade too, and lets the screen
//! pick its own smoothing window (locked in the epic's open questions).

use crate::climb::ClimbSeg;
use crate::geo::seg_dist_m;
use crate::reader::{RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK};
use heapless::Vec;

/// Columns in one climb's detail buffer. One elevation sample per column; the local grade is
/// derived from a small window ([`grade_at`](ClimbProfile::grade_at)). At `i16` per column this
/// is ~400 B — a fixed cost the app holds resident for the single active climb regardless of the
/// climb's length, so a long pass and a short ramp get the same on-screen detail.
///
/// The one RAM knob for this feature; not tied to any display width (the screen maps these
/// columns onto its chart pixels). Confirm the value against on-glass stripe legibility in C4.
pub const COLS: usize = 200;

/// Sentinel elevation for an as-yet-unfilled column, distinct from any real height (heights are
/// whole meters, well inside `i16`). [`fill`](ClimbProfile::fill) seeds every column with this,
/// buckets points over it, then gap-fills the ones no point reached — the same "empty column
/// inherits its neighbour" trick as [`profile::fill_gaps`](crate::profile), adapted to a scalar
/// column.
const EMPTY: i16 = i16::MIN;

/// A single detected climb's elevation profile: [`COLS`] height samples spanning the climb's
/// `[start_m, end_m]` interval, bucketed by **within-climb** distance. Column `0` is the base
/// (at `start_m`), the last column the summit (at `end_m`).
///
/// Built in place by [`fill`](ClimbProfile::fill) from a [`ClimbSeg`] + a [`RouteReader`]; the
/// app keeps one resident and refills it on climb entry. The screen-facing reads
/// ([`at`](Self::at), [`grade_at`](Self::grade_at), [`cursor_frac`](Self::cursor_frac)) are pure
/// index math over the buffer — cheap to call per frame.
#[derive(Debug, Clone)]
pub struct ClimbProfile {
    /// Per-column elevation (m), indexed by within-climb column. Always gap-free after a
    /// [`fill`](Self::fill); a freshly [`new`](Self::new)ed (or empty) profile holds a flat line
    /// at the seg's base so a draw before/without geometry still has a shape.
    cols: [i16; COLS],
    /// The seg's base distance (m, cumulative from the route start) — the anchor
    /// [`cursor_frac`](Self::cursor_frac) subtracts to map a live route progress into `[0, 1]`.
    start_m: u32,
    /// The seg's length (m), cached from `end_m - start_m` (≥ 1 for a kept climb). The divisor
    /// for the within-climb fraction; kept so the cursor mapping doesn't re-read the seg.
    len_m: u32,
    /// The seg's base elevation (m) — column `0` is pinned to this so the base always reads
    /// exactly the detected trough regardless of where the first geometry point landed.
    base_ele_m: i16,
    /// The seg's summit elevation (m) — the last column is pinned to this for the same reason.
    top_ele_m: i16,
}

impl Default for ClimbProfile {
    fn default() -> Self {
        Self::new()
    }
}

impl ClimbProfile {
    /// An empty profile: a flat zero line, zero-length climb. This is the buffer the app places
    /// resident once; call [`fill`](Self::fill) to scope it to an actual climb. Reads on an empty
    /// profile return `0` / a flat line rather than panicking, so a draw before the first
    /// [`fill`](Self::fill) is harmless.
    ///
    /// Const-constructible (`.bss`-friendly) so the device can place it without a large stack
    /// temporary.
    pub const fn new() -> Self {
        ClimbProfile { cols: [0; COLS], start_m: 0, len_m: 0, base_ele_m: 0, top_ele_m: 0 }
    }

    /// Fill this profile **in place** for climb `seg`, reading only the geometry chunks whose
    /// distance span overlaps `[seg.start_m, seg.end_m]`.
    ///
    /// The sweep mirrors [`elevation_profile`](crate::profile) but scoped to one climb: for each
    /// overlapping chunk it accumulates cumulative distance re-anchored at the chunk's stored
    /// [`cum_distance_m`](crate::ChunkMeta::cum_distance_m) (so column placement matches the
    /// format's distance metric exactly), and buckets each point that falls inside the climb into
    /// its within-climb column `(dist - start_m) / len`. Columns no point reached are gap-filled
    /// from their neighbours, and the first/last columns are pinned to the seg's
    /// `base_ele_m` / `top_ele_m` so the endpoints read the exact detected trough/summit.
    ///
    /// `reader` is borrowed, not consumed — the app rebuilds into the same buffer on each climb
    /// entry. O(points *in the climb*); each overlapping chunk is decoded once, none outside.
    pub fn fill(&mut self, reader: &RouteReader, seg: &ClimbSeg) {
        let start = seg.start_m;
        let end = seg.end_m;
        let len = seg.len_m().max(1); // MIN_LEN guarantees ≥ 1, but guard the divide defensively.

        self.start_m = start;
        self.len_m = len;
        self.base_ele_m = seg.base_ele_m;
        self.top_ele_m = seg.top_ele_m;

        // Seed every column empty; the sweep fills the ones points land in, gap-fill does the
        // rest. A climb with no decodable geometry falls through to a flat base line.
        self.cols = [EMPTY; COLS];
        let last_col = COLS - 1;
        let len_f = len as f64;

        // One reused per-chunk decode buffer (bounded, on the stack) — no whole-route buffer.
        let mut buf: Vec<RoutePoint, MAX_POINTS_PER_CHUNK> = Vec::new();
        let chunks = reader.chunks();
        let n = chunks.len();
        for k in 0..n {
            // The chunk's distance span is `[cum_distance_m, next chunk's cum_distance_m)`; the
            // last chunk runs to the route's total distance. A chunk overlaps the climb iff its
            // span intersects `[start, end]` — skip (never decode) any that don't. This is what
            // keeps a mid-route climb from touching chunk 0.
            let chunk_start = chunks[k].cum_distance_m;
            let chunk_end = if k + 1 < n { chunks[k + 1].cum_distance_m } else { reader.total_distance_m };
            // Strict at the boundaries: a chunk that only *touches* the climb at a single
            // distance (`chunk_end == start` or `chunk_start == end`) carries no interior point of
            // the climb — its lone boundary point is the seam, and the endpoints are pinned to the
            // seg anyway — so skip it. That's what keeps the chunk immediately before a mid-route
            // climb (e.g. chunk 0) from being decoded.
            if chunk_end <= start || chunk_start >= end {
                continue;
            }

            if reader.decode_chunk(k, &mut buf).is_err() {
                continue;
            }
            // Re-anchor the running distance to this chunk's stored cumulative distance, exactly
            // as `elevation_profile` does, so per-point placement matches the format's metric and
            // can't drift. `prev` resets per chunk (the seam point contributes zero).
            let mut dist = chunk_start as f64;
            let mut prev: Option<(i32, i32)> = None;
            for p in &buf {
                if let Some(pr) = prev {
                    dist += seg_dist_m(pr, (p.lon, p.lat)) as f64;
                }
                prev = Some((p.lon, p.lat));
                // Only points inside the climb's interval bucket into a column; points in the
                // overlapping-but-outside tail of a boundary chunk are ignored.
                let d = dist as u32;
                if d < start || d > end {
                    continue;
                }
                let within = (dist - start as f64) / len_f; // 0..=1
                let col = ((within * last_col as f64) as usize).min(last_col);
                // Later points in the same column overwrite; with a dense route the column ends on
                // a representative height. (Unlike the whole-route Profile we keep a single sample,
                // not a min/max band — the climb screen draws a line, not a filled range.)
                self.cols[col] = p.ele;
            }
        }

        // Pin the endpoints to the detected trough/summit before gap-filling, so the base and top
        // read exactly the seg's values even if no point landed in column 0 / the last column.
        self.cols[0] = seg.base_ele_m;
        self.cols[last_col] = seg.top_ele_m;
        fill_gaps(&mut self.cols, seg.base_ele_m);
    }

    /// Convenience constructor: a fresh profile filled for `seg`. Prefer [`new`](Self::new) +
    /// [`fill`](Self::fill) on the device (the app owns one resident buffer); this by-value build
    /// is for hosts/tests where the ~400 B move is free.
    pub fn build(reader: &RouteReader, seg: &ClimbSeg) -> Self {
        let mut p = Self::new();
        p.fill(reader, seg);
        p
    }

    /// Elevation (m) at within-climb fraction `frac` (`0.0` = base, `1.0` = summit). The column
    /// read the screen uses to draw the profile line and place the cursor's readout. `frac` is
    /// clamped into `[0, 1]`, so an out-of-range cursor reads the nearest endpoint.
    #[inline]
    pub fn at(&self, frac: f32) -> i16 {
        let last = COLS - 1;
        let col = (frac.clamp(0.0, 1.0) * last as f32) as usize;
        self.cols[col.min(last)]
    }

    /// Elevation (m) at within-climb column `col` (clamped). The raw column read backing
    /// [`at`](Self::at) and [`grade_at`](Self::grade_at); the screen may also walk columns
    /// directly to draw one stripe per column.
    #[inline]
    pub fn col(&self, col: usize) -> i16 {
        self.cols[col.min(COLS - 1)]
    }

    /// The whole per-column elevation buffer, base → summit. For the screen to draw a stripe per
    /// column; local grade per stripe is [`grade_at`](Self::grade_at).
    #[inline]
    pub fn cols(&self) -> &[i16] {
        &self.cols
    }

    /// Local grade (whole percent) at within-climb fraction `frac`, derived from a small window
    /// of columns centred on `frac`: `Δelevation / Δdistance × 100` over that window.
    ///
    /// **Window choice.** Grade is the slope between two columns, so a one-column difference is
    /// dominated by per-column quantization noise (each column is only ~climb_len/200 m wide). We
    /// instead take a symmetric window of [`GRADE_WIN`] columns each side (clamped at the ends)
    /// and divide the elevation change across it by the ground distance those columns span — a
    /// centred finite difference. That smooths single-column jitter while staying local enough to
    /// show where a climb ramps up. Returns signed percent (negative on an internal dip).
    pub fn grade_at(&self, frac: f32) -> i32 {
        let last = COLS - 1;
        let center = (frac.clamp(0.0, 1.0) * last as f32) as usize;
        // Symmetric window, clamped to the buffer ends so the endpoints use a one-sided slope.
        let lo = center.saturating_sub(GRADE_WIN);
        let hi = (center + GRADE_WIN).min(last);
        if hi == lo {
            return 0; // degenerate (COLS ≤ 1) — no slope to measure.
        }
        let d_ele = self.cols[hi] as i32 - self.cols[lo] as i32; // meters, signed
                                                                 // Ground distance the window spans: its column fraction of the whole climb length.
        let span_cols = (hi - lo) as u32;
        let d_dist = (self.len_m as u64 * span_cols as u64 / last as u64).max(1) as i32; // meters ≥ 1
        d_ele * 100 / d_dist
    }

    /// Map a live route `progress_m` (cumulative distance from the route start, as the matcher
    /// reports) to the within-climb fraction `[0, 1]` — the "you are here" cursor position. Below
    /// the base clamps to `0.0`, past the summit to `1.0`, so a cursor just outside the climb sits
    /// at the nearest end rather than off-screen.
    #[inline]
    pub fn cursor_frac(&self, progress_m: u32) -> f32 {
        // A zero-length (empty / never-filled) profile has no interval to map into — pin the
        // cursor at the base rather than dividing by the guarded `1` and overshooting to 1.0.
        if self.len_m == 0 {
            return 0.0;
        }
        let into = progress_m.saturating_sub(self.start_m);
        (into as f32 / self.len_m as f32).clamp(0.0, 1.0)
    }

    /// The climb's base distance (m) from the route start — the interval this profile spans is
    /// `[start_m, start_m + len_m]`. Lets the screen relate a route-scoped progress to this climb.
    #[inline]
    pub fn start_m(&self) -> u32 {
        self.start_m
    }

    /// The climb's length (m). The divisor behind [`cursor_frac`](Self::cursor_frac); exposed for
    /// the screen's "to top" / "to climb" tiles.
    #[inline]
    pub fn len_m(&self) -> u32 {
        self.len_m
    }

    /// The climb's base elevation (m) — column `0`. Equal to the seg's `base_ele_m`.
    #[inline]
    pub fn base_ele_m(&self) -> i16 {
        self.base_ele_m
    }

    /// The climb's summit elevation (m) — the last column. Equal to the seg's `top_ele_m`.
    #[inline]
    pub fn top_ele_m(&self) -> i16 {
        self.top_ele_m
    }
}

/// Columns to reach on each side for the [`grade_at`](ClimbProfile::grade_at) centred difference.
/// A ±3-column window (~7 columns, ~3.5 % of the climb) smooths single-column quantization
/// without blurring where the grade actually changes; the screen reads through `grade_at`, so a
/// retune here is invisible to callers.
const GRADE_WIN: usize = 3;

/// Make `cols` gap-free in place: each still-[`EMPTY`] column inherits the nearest filled column
/// — forward carry first, then a backward carry for any leading run of empties. Any column still
/// empty after both (only when the climb had no decodable geometry at all) falls back to
/// `fallback` (the seg's base), so the buffer is never left with a sentinel.
///
/// This is the scalar analogue of [`profile::fill_gaps`](crate::profile) (which fills a `(min,
/// max)` band); a climb profile keeps one sample per column, so the carry is a single `i16`.
fn fill_gaps(cols: &mut [i16; COLS], fallback: i16) {
    // Forward: carry the last seen height into the empties that follow it.
    let mut last: Option<i16> = None;
    for c in cols.iter_mut() {
        if *c != EMPTY {
            last = Some(*c);
        } else if let Some(v) = last {
            *c = v;
        }
    }
    // Backward: fill any leading empties the forward pass couldn't reach.
    let mut next: Option<i16> = None;
    for c in cols.iter_mut().rev() {
        if *c != EMPTY {
            next = Some(*c);
        } else if let Some(v) = next {
            *c = v;
        }
    }
    // Only reachable when the whole climb had no points (both endpoints are pinned before this,
    // so in practice at least columns 0 and last are set and this loop is a no-op).
    for c in cols.iter_mut() {
        if *c == EMPTY {
            *c = fallback;
        }
    }
}
