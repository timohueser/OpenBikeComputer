//! [`RowDiff`]: the self-diffing present core. It keeps a per-row hash of the last presented
//! framebuffer, so the present path pushes only the rows that changed.
//!
//! Screens are immediate-mode (`clear()` then redraw), so tracking writes would mark everything
//! dirty. Instead the present layer keeps a 32-bit hash per row and, on present, re-hashes each
//! row, pushes only the contiguous spans whose hash changed, and updates the store. A Home clock
//! ticking a minute re-presents its handful of rows instead of all 320: a few ms against a 44 ms
//! full frame on the LS021.
//!
//! - [`row_hash`] is FNV-1a over one row as pre-mixed `u32` words. The only failure mode, a
//!   changed row hashing equal and so being skipped, is about 2⁻³² per row change and self-heals.
//!   The pre-mix is load-bearing; see [`row_hash`].
//! - [`diff_rows`] is the core diff, generic over the hash function and the row count, so the
//!   device's fixed-size store and the simulator's runtime-sized one share one implementation.
//! - [`RowDiff`] is the fixed-height store: a `[u32; H]` in `.bss` plus a priming flag.
//! - [`spans_missed_changes`] is the exact-diff oracle: a full byte compare, host and tests only,
//!   reporting how many real changes the hash-diff's spans missed. Non-zero means a systematic bug.
//!
//! Pixel-format-agnostic: the diff is over raw row bytes with a caller-supplied stride, so it
//! serves the device's 1-byte-per-pixel RGB222 plane and the simulator's 3-byte RGB888 alike. The
//! hash pass is word-at-a-time and well under a millisecond over the 75 KB device plane.

/// FNV-1a (32-bit) over one framebuffer row, mixed a pre-avalanched `u32` word at a time. This
/// runs over the whole framebuffer on every map-dirty present, so it is the diff pass's floor:
/// folding four bytes per multiply instead of one cuts the pass about fourfold. The values differ
/// from byte-FNV-1a, but the store never leaves this module.
///
/// Each word is pre-mixed before the FNV step. Plain word-FNV (`h = (h ^ w) * prime`) only moves
/// information toward higher bits, so two rows differing only in the top byte of their words,
/// that is pixel columns `x % 4 == 3`, keep their hash difference in bits 24..32, and a second
/// such changed word cancels it with probability about 2⁻⁸ rather than 2⁻³². Real frames hit that
/// family constantly, and on a static screen the skipped row never self-heals, so it stays stale
/// on glass. Multiplying each word by the golden-ratio constant and folding the high half down
/// (`k ^= k >> 15`) avalanches the difference before it meets the accumulator, so a cancellation
/// again needs a full 32-bit match. It costs one extra multiply and shift-xor per word.
#[inline]
pub fn row_hash(row: &[u8]) -> u32 {
    /// Avalanche one injected word so no sparse difference stays confined to a byte lane: the
    /// golden-ratio multiply spreads low bits up, and the shift-xor folds the high half down.
    #[inline(always)]
    fn premix(w: u32) -> u32 {
        let k = w.wrapping_mul(0x9e37_79b1);
        k ^ (k >> 15)
    }
    let mut h: u32 = 0x811c_9dc5; // FNV-1a offset basis
    let (words, remainder) = row.as_chunks::<4>();
    for w in words {
        h ^= premix(u32::from_le_bytes([w[0], w[1], w[2], w[3]]));
        h = h.wrapping_mul(0x0100_0193); // FNV prime
    }
    // Byte tail for strides that are not a multiple of 4. The device stride (240) and the
    // simulator stride (720) are both exact.
    for &b in remainder {
        h ^= premix(b as u32);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// The self-diff core: re-hash each of `prev.len()` rows of `fb` (`stride` bytes per row), compare
/// to `prev`, update it in place, and emit each maximal run of changed rows as one span through
/// `push_span(y0, rows)`. An unchanged row between two changed ones splits the run, so nothing
/// unchanged is ever pushed.
///
/// `hash` is the per-row hash, [`row_hash`] in production and a colliding stub for the oracle.
/// `force_all` treats every row as changed, for the first present after construction or a
/// [`RowDiff::reset`], where the store holds no prior frame.
///
/// The store's length is the row count. Panics in debug if `fb` is shorter than `rows * stride`.
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
    // Walk the rows tracking the start of the current changed run, and emit the run as soon as an
    // unchanged row, or the end of the frame, closes it.
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

/// A fixed-height per-row hash store, the [`diff_rows`] wrapper a board owns in `.bss`. `H` is the
/// frame's row count, so the store is `[u32; H]`, and the priming flag forces a full first
/// present.
pub struct RowDiff<const H: usize> {
    /// Last-presented per-row hashes (one per frame row). Seeded by the first [`diff`](RowDiff::diff).
    prev: [u32; H],
    /// `false` until the first diff: the stored hashes hold no real prior frame yet, so the first
    /// present must push and seed the whole frame whatever the zero-init store says.
    primed: bool,
}

impl<const H: usize> RowDiff<H> {
    /// An unprimed store (zeroed hashes); the first [`diff`](RowDiff::diff) pushes the whole frame.
    pub const fn new() -> Self {
        Self { prev: [0; H], primed: false }
    }

    /// Force the next [`diff`](RowDiff::diff) to push the whole frame again, for a repaint such
    /// as a panel re-init where the on-glass frame no longer matches the store.
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

    /// [`diff`](RowDiff::diff) with a live overlay's rows clipped out: the shared present
    /// skeleton both display backends run.
    ///
    /// It diffs the whole frame, so the store is updated for every row, the excluded ones
    /// included: the store tracks the clean framebuffer, so when the overlay goes quiet its rows
    /// re-push clean with no stale entry. Each changed span is clipped around the exclude
    /// interval `[y0, y0+rows)` and collected into the caller's `spans` scratch. If they do not
    /// fit, it falls back to the whole frame minus the exclude, at most 2 spans, rather than
    /// silently dropping rows. Returns the filled prefix, ascending and disjoint; empty means
    /// nothing changed outside the overlay.
    ///
    /// `spans` must hold at least 2 entries, the fallback's worst case.
    pub fn diff_clipped<'s>(
        &mut self,
        fb: &[u8],
        stride: usize,
        exclude: Option<(u16, u16)>,
        spans: &'s mut [(u16, u16)],
    ) -> &'s [(u16, u16)] {
        debug_assert!(spans.len() >= 2, "span scratch too small for the whole-frame fallback");
        // Half-open exclude interval [e0, e1) the clip removes from each changed span.
        let ex = exclude.map(|(y0, rows)| (y0, y0 + rows));
        let mut n = 0;
        let mut overflow = false;
        self.diff(fb, stride, |y0, cnt| {
            clip_span(y0, cnt, ex, &mut |s, c| {
                if n < spans.len() {
                    spans[n] = (s, c);
                    n += 1;
                } else {
                    overflow = true;
                }
            });
        });
        if overflow {
            n = 0;
            clip_span(0, H as u16, ex, &mut |s, c| {
                spans[n] = (s, c);
                n += 1;
            });
        }
        &spans[..n]
    }
}

impl<const H: usize> Default for RowDiff<H> {
    fn default() -> Self {
        Self::new()
    }
}

/// Emit the changed-row span `[y0, y0+n)` with the half-open `exclude` interval `[e0, e1)`
/// removed, as up to two ascending, disjoint sub-spans.
///
/// When the hold bulge is live, the map present pushes the changed rows around it and leaves the
/// bulge's rows to the overlay composite that follows: presenting them clean here would blank the
/// bulge until the composite repaints them. A span straddling the bulge splits in two, one
/// entirely inside it emits nothing, and one clear of it passes whole. `None` means no live
/// bulge, so everything passes through.
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

/// The exact-diff oracle: count how many rows that really changed between `prev_fb` and `cur_fb`
/// the hash-diff's `spans` failed to cover. `0` means honest; non-zero means a systematic miss for
/// CI to fail on, because a real device only sees random, self-healing collisions.
///
/// A full byte compare of the two frames, independent of the hashes, and never run on the device.
/// `covered` is a caller-provided `rows`-long scratch, rewritten each call. Panics in debug if a
/// frame or the scratch is too short.
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

    // Tests run on the host, so a `Vec` span sink is fine even though the crate is no_std. `std`
    // is not in a no_std crate's extern prelude, so name it.
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
        // An empty row is the bare offset basis: a stable, non-zero seed.
        assert_eq!(row_hash(&[]), 0x811c_9dc5);
    }

    #[test]
    fn word_and_tail_bytes_both_reach_the_hash() {
        // A 6-byte row = one 4-byte word + a 2-byte tail (the strides above are all-tail, so this
        // is the only test walking both loops). A change in either part must change the hash.
        let row = [1u8, 2, 3, 4, 5, 6];
        assert_eq!(row_hash(&row), row_hash(&row));
        for i in 0..row.len() {
            let mut changed = row;
            changed[i] ^= 0x80;
            assert_ne!(row_hash(&changed), row_hash(&row), "byte {i} didn't affect the hash");
        }
        // Word-path position sensitivity: swapping bytes within a word changes the word value.
        assert_ne!(row_hash(&[1, 2, 3, 4]), row_hash(&[4, 3, 2, 1]));
    }

    /// The pre-fix word-FNV hash collided on rows differing only in the top byte of their words,
    /// that is pixel columns ≡ 3 mod 4, so the self-diff skipped a genuinely changed row. Pin the
    /// measured pair, then require the whole two-pixel family, every byte lane, to be
    /// collision-free.
    #[test]
    fn byte_lane_confined_changes_never_collide() {
        use std::collections::HashSet;

        // The exact measured colliding pair of the pre-fix hash.
        let zero = [0u8; 8];
        let mut pair = zero;
        pair[3] = 0x02;
        pair[7] = 0x2E;
        assert_ne!(row_hash(&pair), row_hash(&zero), "the #626 pair must not collide");

        // Every two-word row whose device-64 pixels vary only in one byte lane hashes uniquely.
        for lane in 0..4 {
            let mut seen = HashSet::new();
            for a in 0u8..64 {
                for b in 0u8..64 {
                    let mut r = [0u8; 8];
                    r[lane] = a;
                    r[4 + lane] = b;
                    assert!(seen.insert(row_hash(&r)), "lane {lane} collision at a={a:#x} b={b:#x}");
                }
            }
        }
    }

    #[test]
    fn unchanged_frame_emits_no_spans() {
        let fb = [10u8, 20, 30, 40, 50, 60]; // 3 rows × 2 bytes
        let mut prev = [0u32; 3];
        // Prime the store, then re-diff the identical frame: zero spans, because a redraw that
        // changes nothing pushes nothing.
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
        // An unprimed RowDiff: the first diff is one full-frame span whatever the zeroed store
        // holds, so a row that happens to hash to a stored value is still pushed.
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

    /// Run `diff_clipped` over a 6-row × 2-byte frame with a 16-slot scratch (the device shape).
    fn diff_clipped(rd: &mut RowDiff<6>, fb: &[u8], exclude: Option<(u16, u16)>) -> Vec<(u16, u16)> {
        let mut scratch = [(0u16, 0u16); 16];
        rd.diff_clipped(fb, 2, exclude, &mut scratch).to_vec()
    }

    #[test]
    fn diff_clipped_clips_changed_spans_around_the_exclude() {
        let mut rd = RowDiff::<6>::new();
        let fb0 = [0u8; 6 * 2];
        let _ = diff_clipped(&mut rd, &fb0, None); // prime
        let mut fb1 = fb0;
        // Rows 1..=4 change; the exclude [2, 4) splits the span into (1,1) and (4,1).
        for y in 1..=4 {
            fb1[y * 2] = 0x55;
        }
        assert_eq!(diff_clipped(&mut rd, &fb1, Some((2, 2))), vec![(1, 1), (4, 1)]);
    }

    #[test]
    fn diff_clipped_updates_the_store_for_excluded_rows() {
        let mut rd = RowDiff::<6>::new();
        let fb0 = [0u8; 6 * 2];
        let _ = diff_clipped(&mut rd, &fb0, None); // prime
        let mut fb1 = fb0;
        fb1[3 * 2] = 0x77; // row 3 changes, but is excluded this present
        assert_eq!(diff_clipped(&mut rd, &fb1, Some((3, 1))), Vec::new());
        // The store tracked the clean fb anyway, so a later present with no exclude does not
        // re-push the unchanged excluded row.
        assert_eq!(diff_clipped(&mut rd, &fb1, None), Vec::new());
    }

    #[test]
    fn diff_clipped_priming_pushes_the_whole_frame_minus_the_exclude() {
        let mut rd = RowDiff::<6>::new();
        let fb = [9u8; 6 * 2];
        // First present: everything is dirty; the exclude still clips its rows out.
        assert_eq!(diff_clipped(&mut rd, &fb, Some((2, 2))), vec![(0, 2), (4, 2)]);
    }

    #[test]
    fn diff_clipped_overflow_falls_back_to_whole_frame_minus_exclude() {
        let mut rd = RowDiff::<6>::new();
        let fb0 = [0u8; 6 * 2];
        let _ = diff_clipped(&mut rd, &fb0, None); // prime
        let mut fb1 = fb0;
        // Rows 0, 2 and 4 change, so three disjoint spans overflow a 2-slot scratch: the fallback
        // must cover the whole frame while still respecting the exclude.
        fb1[0] = 1;
        fb1[2 * 2] = 1;
        fb1[4 * 2] = 1;
        let mut scratch = [(0u16, 0u16); 2];
        let spans = rd.diff_clipped(&fb1, 2, Some((1, 1)), &mut scratch).to_vec();
        assert_eq!(spans, vec![(0, 1), (2, 4)]);
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
        // A deliberately colliding hash: every row hashes the same, so the diff sees no change
        // and emits no spans, yet rows really did change. The oracle must catch the miss.
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
