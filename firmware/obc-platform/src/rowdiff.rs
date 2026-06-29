//! [`RowDiff`] — the **self-diffing present** core (epic #199 / issue #200): a per-row hash of the
//! last-presented framebuffer, so the present path pushes only the rows that actually changed.
//!
//! The map plane is render-on-demand at the *frame* granularity ([`Dirty`](obc_app::Dirty)): a
//! coarse `animate -> bool` decides *whether* to re-render + present a frame, but says nothing about
//! *where* it changed. Screens stay immediate-mode — they `clear()` and redraw the whole frame — so
//! "track what was written" would mark everything dirty every frame. Instead the present layer
//! detects the changed region **automatically**: keep a 32-bit hash per framebuffer row, and on
//! present re-hash each row, push only the contiguous spans whose hash changed, and update the
//! store. A Home clock ticking a minute then re-presents the clock's handful of rows instead of all
//! 320 — on the LS021/FLPR the difference between a ~97 ms full frame and a few ms (epic #199).
//!
//! ## Where the pieces live
//!
//! - [`row_hash`] — FNV-1a over one row's bytes. 32-bit: 320 rows = 1.28 KB of store, and a
//!   collision (a changed row hashing equal, so skipped and left stale until it next changes) is
//!   ~2⁻³² per row-change event — once per several-hundred device-years, and **self-healing**. The
//!   simulator runs an exact full-frame diff ([`spans_missed_changes`]) as a CI oracle, so any
//!   *systematic* bug is caught; only random, self-healing collisions reach the field.
//! - [`diff_rows`] — the core diff: hash each row, compare to the previous hashes, coalesce changed
//!   rows into contiguous spans, update the store. Generic over the hash fn (the oracle's collision
//!   test injects a deliberately-colliding stub) and over the *row count* (the store is a plain
//!   `&mut [u32]` slice), so the fixed-size device store and the simulator's runtime-sized one share
//!   one implementation — the same code the oracle validates.
//! - [`RowDiff`] — the ergonomic store for a board with a **fixed** frame height: a `[u32; H]` in
//!   `.bss` plus the priming flag, calling [`diff_rows`] under the hood. The device present path
//!   owns one of these (issue #201, D2); D1 builds + proves the mechanism here.
//! - [`spans_missed_changes`] — the exact-diff **oracle**: independently compute which rows actually
//!   changed (a full byte compare — cheap where RAM/CPU are free, i.e. host & tests, *never* on the
//!   device) and report how many the hash-diff's spans missed. `0` ⇒ honest; non-zero ⇒ a systematic
//!   miss for CI to fail on.
//!
//! Pixel-format-agnostic: the diff is over raw row bytes with a caller-supplied stride, so it works
//! on the device's 1-byte/px RGB222 plane and the simulator's 3-byte/px RGB888 alike. On a banded
//! device backend the per-row hash **piggybacks** on the pack pass that already reads every byte; in
//! the simulator (which uploads a whole texture) it is a cheap separate pass — the simulator is the
//! oracle, not the perf target.

/// FNV-1a (32-bit) over one framebuffer row's bytes — the per-row hash the self-diff compares.
///
/// FNV-1a is a byte-at-a-time mix with good avalanche for this use (whole-row equality detection),
/// no table, and a one-line inner loop the compiler folds into the present's existing per-byte read.
/// 32-bit is deliberate: 320 rows is 1.28 KB of store, and the only failure mode — two *different*
/// rows hashing equal so a change is skipped — is ~2⁻³² per row-change and self-heals the next time
/// the row changes (see the module docs).
#[inline]
pub fn row_hash(row: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5; // FNV-1a offset basis
    for &b in row {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193); // FNV prime
    }
    h
}

/// The self-diff core: re-hash each of `prev.len()` rows of `fb` (`stride` bytes per row), compare
/// to `prev`, update it in place, and emit each **maximal run of changed rows** as one span via
/// `push_span(y0, rows)`. A span only ever contains changed rows — an unchanged row between two
/// changed ones splits the run, so nothing unchanged is ever pushed.
///
/// `hash` is the per-row hash — [`row_hash`] in production; the oracle's collision test passes a
/// stub to force a systematic miss. `force_all` treats *every* row as changed regardless of the
/// stored hash: the first present after construction / a [`RowDiff::reset`], where the store holds
/// no meaningful prior frame and the whole frame must be pushed (and the store seeded).
///
/// The store is a plain `&mut [u32]` whose length *is* the row count, so the device's fixed-size
/// array and the simulator's runtime-sized `Vec` drive the identical coalescing logic. Panics
/// (debug) if `fb` is shorter than `rows * stride` — a caller wiring bug.
pub fn diff_rows(
    fb: &[u8],
    stride: usize,
    prev: &mut [u32],
    force_all: bool,
    hash: impl Fn(&[u8]) -> u32,
    mut push_span: impl FnMut(u16, u16),
) {
    let rows = prev.len();
    debug_assert!(fb.len() >= rows * stride, "framebuffer shorter than rows*stride");
    // Walk the rows tracking the start of the current changed run; emit the run the moment an
    // unchanged row (or the end of the frame) closes it.
    let mut run_start: Option<usize> = None;
    for y in 0..rows {
        let h = hash(&fb[y * stride..y * stride + stride]);
        let changed = force_all || h != prev[y];
        prev[y] = h; // the store always tracks the latest frame, pushed or not (self-healing).
        match run_start {
            None if changed => run_start = Some(y),
            Some(s) if !changed => {
                push_span(s as u16, (y - s) as u16);
                run_start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = run_start {
        push_span(s as u16, (rows - s) as u16);
    }
}

/// A fixed-height per-row hash store — the ergonomic [`diff_rows`] wrapper a board with a **fixed**
/// frame height owns in `.bss`. `H` is the frame's row count, so the store is `[u32; H]` (320 rows =
/// 1.28 KB) and the priming flag forces a full first present.
///
/// The device present path holds one and calls [`diff`](RowDiff::diff) once per frame (issue #201,
/// D2); the simulator drives the same [`diff_rows`] core over a runtime-sized store, so both exercise
/// the implementation the oracle validates.
pub struct RowDiff<const H: usize> {
    /// Last-presented per-row hashes (one per frame row). Seeded by the first [`diff`](RowDiff::diff).
    prev: [u32; H],
    /// `false` until the first diff: the stored hashes hold no real prior frame yet, so the first
    /// present must push (and seed) the whole frame regardless of the zero-init store.
    primed: bool,
}

impl<const H: usize> RowDiff<H> {
    /// An unprimed store (zeroed hashes); the first [`diff`](RowDiff::diff) pushes the whole frame.
    pub const fn new() -> Self {
        Self { prev: [0; H], primed: false }
    }

    /// Force the next [`diff`](RowDiff::diff) to push the whole frame again — for a forced full
    /// repaint (a panel re-init, a resolution change) where the on-glass frame no longer matches the
    /// store.
    pub fn reset(&mut self) {
        self.primed = false;
    }

    /// Diff `fb` (`H` rows of `stride` bytes) against the store using [`row_hash`], emitting each
    /// changed-row span via `push_span(y0, rows)` and updating the store. The first call after
    /// [`new`](RowDiff::new) / [`reset`](RowDiff::reset) pushes the whole frame as one span.
    pub fn diff(&mut self, fb: &[u8], stride: usize, push_span: impl FnMut(u16, u16)) {
        diff_rows(fb, stride, &mut self.prev, !self.primed, row_hash, push_span);
        self.primed = true;
    }
}

impl<const H: usize> Default for RowDiff<H> {
    fn default() -> Self {
        Self::new()
    }
}

/// Emit the changed-row span `[y0, y0+n)` with the half-open `exclude` interval `[e0, e1)` removed, as
/// up to two ascending, disjoint sub-spans — the **bulge-coordination clip** the LS021/FLPR
/// self-diffing present runs (issue #201/#163).
///
/// When the hold bulge is live, the map present pushes the changed rows *around* it and leaves the
/// bulge's rows for the overlay composite that immediately follows: presenting them clean here would
/// blank the bulge until the composite repaints them (the "pop flicker" when a redraw lands mid-pop, on
/// the long-press screen transition). So a changed span straddling the bulge splits in two, a span
/// entirely inside it emits nothing, and a span clear of it passes through whole. `None` ⇒ no live
/// bulge, the span always passes through. (Pure span arithmetic, kept here beside [`diff_rows`] so its
/// edge cases are exercised in CI rather than only on glass.)
pub fn clip_span(y0: u16, n: u16, exclude: Option<(u16, u16)>, emit: &mut impl FnMut(u16, u16)) {
    let (a, b) = (y0, y0 + n); // the changed span [a, b)
    let (e0, e1) = match exclude {
        Some(e) => e,
        None => return emit(a, n),
    };
    // Left piece: rows of [a, b) below the bulge start.
    let left_end = b.min(e0);
    if a < left_end {
        emit(a, left_end - a);
    }
    // Right piece: rows of [a, b) at/after the bulge end.
    let right_start = a.max(e1);
    if right_start < b {
        emit(right_start, b - right_start);
    }
}

/// The exact-diff **oracle** (epic #199): count how many rows that *actually* changed between
/// `prev_fb` and `cur_fb` the hash-diff's `spans` failed to cover. `0` ⇒ the hash-diff covered every
/// real change (the honest case); non-zero ⇒ a *systematic* miss — a hash-diff bug for CI to fail
/// on (a real device only ever sees random, self-healing collisions, never a systematic gap).
///
/// Independently of the hashes, this does a full byte compare of the two frames (`rows` rows of
/// `stride` bytes) — cheap where RAM/CPU are free (the simulator and tests), and **never** run on
/// the device. `covered` is a caller-provided `rows`-long scratch (so this stays no-alloc): it is
/// rewritten from `spans` each call. Panics (debug) if a frame or the scratch is too short.
pub fn spans_missed_changes(
    prev_fb: &[u8],
    cur_fb: &[u8],
    stride: usize,
    rows: usize,
    spans: &[(u16, u16)],
    covered: &mut [bool],
) -> usize {
    debug_assert!(prev_fb.len() >= rows * stride && cur_fb.len() >= rows * stride, "frame shorter than rows*stride");
    debug_assert!(covered.len() >= rows, "covered scratch shorter than rows");
    covered[..rows].fill(false);
    for &(y0, n) in spans {
        for c in covered[y0 as usize..y0 as usize + n as usize].iter_mut() {
            *c = true;
        }
    }
    let mut missed = 0;
    for (y, &cov) in covered[..rows].iter().enumerate() {
        let r = y * stride..y * stride + stride;
        if !cov && prev_fb[r.clone()] != cur_fb[r] {
            missed += 1;
        }
    }
    missed
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests run on the host (the test harness links std), so a `Vec` span sink is fine here even
    // though the crate is no_std — the shipping callers (device `.bss`, sim `Vec`) supply their own.
    // `std` isn't in a no_std crate's extern prelude, so name it; `std::vec!` builds the expected sets.
    extern crate std;
    use std::vec;
    use std::vec::Vec;

    /// Run the production diff over `prev`/`cur` framebuffers (`rows`×`stride`), returning the
    /// emitted spans. `force_all=false` so it exercises the hash compare, not the priming path.
    fn diff(prev: &mut [u32], fb: &[u8], stride: usize) -> Vec<(u16, u16)> {
        let mut spans = Vec::new();
        diff_rows(fb, stride, prev, false, row_hash, |y0, n| spans.push((y0, n)));
        spans
    }

    #[test]
    fn equal_rows_hash_equal_and_differ_otherwise() {
        assert_eq!(row_hash(&[1, 2, 3]), row_hash(&[1, 2, 3]));
        assert_ne!(row_hash(&[1, 2, 3]), row_hash(&[1, 2, 4]));
        // Empty row is the bare offset basis — a stable, non-zero seed.
        assert_eq!(row_hash(&[]), 0x811c_9dc5);
    }

    #[test]
    fn unchanged_frame_emits_no_spans() {
        let fb = [10u8, 20, 30, 40, 50, 60]; // 3 rows × 2 bytes
        let mut prev = [0u32; 3];
        // Prime the store, then re-diff the identical frame: zero spans (the "spurious dirty is
        // free" property — a redraw that changes nothing pushes nothing).
        let _ = diff(&mut prev, &fb, 2);
        assert_eq!(diff(&mut prev, &fb, 2), Vec::new());
    }

    #[test]
    fn single_changed_row_is_a_one_row_span() {
        let mut prev = [0u32; 4];
        let fb0 = [0u8; 4 * 2];
        let _ = diff(&mut prev, &fb0, 2); // prime
                                          // Change only row 2.
        let mut fb1 = fb0;
        fb1[2 * 2] = 0xAB;
        assert_eq!(diff(&mut prev, &fb1, 2), vec![(2, 1)]);
    }

    #[test]
    fn adjacent_changed_rows_coalesce_into_one_span() {
        let mut prev = [0u32; 5];
        let fb0 = [0u8; 5 * 2];
        let _ = diff(&mut prev, &fb0, 2);
        let mut fb1 = fb0;
        // Rows 1,2,3 change → one span (1, 3); row 0 and 4 unchanged bracket it.
        for y in 1..=3 {
            fb1[y * 2] = 0x11;
        }
        assert_eq!(diff(&mut prev, &fb1, 2), vec![(1, 3)]);
    }

    #[test]
    fn an_unchanged_row_between_changes_splits_the_span() {
        let mut prev = [0u32; 5];
        let fb0 = [0u8; 5 * 2];
        let _ = diff(&mut prev, &fb0, 2);
        let mut fb1 = fb0;
        // Rows 0 and 4 change, 1..=3 stay → two disjoint one-row spans, nothing in between pushed.
        fb1[0] = 0x22;
        fb1[4 * 2] = 0x33;
        assert_eq!(diff(&mut prev, &fb1, 2), vec![(0, 1), (4, 1)]);
    }

    #[test]
    fn a_changed_run_reaching_the_last_row_is_emitted() {
        let mut prev = [0u32; 4];
        let fb0 = [0u8; 4 * 2];
        let _ = diff(&mut prev, &fb0, 2);
        let mut fb1 = fb0;
        // Rows 2,3 (the tail) change → the run-at-EOF branch emits (2, 2).
        fb1[2 * 2] = 1;
        fb1[3 * 2] = 1;
        assert_eq!(diff(&mut prev, &fb1, 2), vec![(2, 2)]);
    }

    #[test]
    fn first_diff_pushes_the_whole_frame_via_priming() {
        // An unprimed RowDiff: the first diff is one full-frame span regardless of the zeroed store
        // (so a row that happens to hash to a stored value is still pushed on the first present).
        let mut rd = RowDiff::<6>::new();
        let fb = [7u8; 6 * 3];
        let mut spans = Vec::new();
        rd.diff(&fb, 3, |y0, n| spans.push((y0, n)));
        assert_eq!(spans, vec![(0, 6)]);
        // Re-presenting the same frame now pushes nothing.
        spans.clear();
        rd.diff(&fb, 3, |y0, n| spans.push((y0, n)));
        assert_eq!(spans, Vec::new());
        // reset() re-arms the full push.
        rd.reset();
        spans.clear();
        rd.diff(&fb, 3, |y0, n| spans.push((y0, n)));
        assert_eq!(spans, vec![(0, 6)]);
    }

    /// Collect [`clip_span`]'s emitted sub-spans for `[y0, y0+n)` minus `exclude`.
    fn clip(y0: u16, n: u16, exclude: Option<(u16, u16)>) -> Vec<(u16, u16)> {
        let mut out = Vec::new();
        clip_span(y0, n, exclude, &mut |s, c| out.push((s, c)));
        out
    }

    #[test]
    fn clip_span_no_exclude_passes_through() {
        assert_eq!(clip(10, 5, None), vec![(10, 5)]);
    }

    #[test]
    fn clip_span_clear_of_exclude_passes_through() {
        // Span entirely below the bulge, and entirely above it.
        assert_eq!(clip(0, 10, Some((20, 30))), vec![(0, 10)]);
        assert_eq!(clip(40, 10, Some((20, 30))), vec![(40, 10)]);
    }

    #[test]
    fn clip_span_straddling_exclude_splits_in_two() {
        // [0, 100) minus the bulge [40, 60) → [0, 40) and [60, 100).
        assert_eq!(clip(0, 100, Some((40, 60))), vec![(0, 40), (60, 40)]);
    }

    #[test]
    fn clip_span_inside_exclude_emits_nothing() {
        // A changed span fully within the bulge is owned by the overlay composite — push nothing.
        assert_eq!(clip(45, 10, Some((40, 60))), Vec::new());
    }

    #[test]
    fn clip_span_boundaries_are_half_open() {
        // Touching the exclude start from below keeps the [a, e0) part; reaching exactly e1 keeps from e1.
        assert_eq!(clip(30, 20, Some((40, 60))), vec![(30, 10)]); // [30,50) minus [40,60) → [30,40)
        assert_eq!(clip(50, 20, Some((40, 60))), vec![(60, 10)]); // [50,70) minus [40,60) → [60,70)
                                                                  // A span that exactly equals the exclude interval emits nothing.
        assert_eq!(clip(40, 20, Some((40, 60))), Vec::new());
    }

    #[test]
    fn oracle_passes_when_spans_cover_every_real_change() {
        let stride = 2;
        let rows = 5;
        let prev_fb = [0u8; 5 * 2];
        let mut cur_fb = prev_fb;
        // Change rows 1 and 3 (bytes 1*stride and 3*stride).
        cur_fb[2] = 9;
        cur_fb[6] = 9;
        // Run the real diff to get the spans, then check the oracle is satisfied.
        let mut store = [0u32; 5];
        let _ = diff(&mut store, &prev_fb, stride); // seed the store from prev_fb
        let spans = diff(&mut store, &cur_fb, stride);
        assert_eq!(spans, vec![(1, 1), (3, 1)]);
        let mut covered = [false; 5];
        assert_eq!(spans_missed_changes(&prev_fb, &cur_fb, stride, rows, &spans, &mut covered), 0);
    }

    #[test]
    fn oracle_catches_a_systematic_miss_from_a_colliding_hash() {
        // A deliberately-colliding hash: every row hashes to the same value, so the diff sees *no*
        // change and emits no spans — yet rows really did change. The oracle must catch the miss.
        let stride = 2;
        let rows = 4;
        let prev_fb = [0u8; 4 * 2];
        let mut cur_fb = prev_fb;
        cur_fb[2 * 2] = 0xFF; // row 2 genuinely changes

        let mut store = [0u32; 4];
        // Seed + diff through the colliding stub instead of `row_hash`.
        diff_rows(&prev_fb, stride, &mut store, false, |_| 0, |_, _| {});
        let mut spans = Vec::new();
        diff_rows(&cur_fb, stride, &mut store, false, |_| 0, |y0, n| spans.push((y0, n)));
        assert_eq!(spans, Vec::new(), "the colliding hash sees no change");

        let mut covered = [false; 4];
        let missed = spans_missed_changes(&prev_fb, &cur_fb, stride, rows, &spans, &mut covered);
        assert_eq!(missed, 1, "the oracle flags the one row the colliding hash skipped");
    }
}
