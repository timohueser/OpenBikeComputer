//! [`RowDiff`] — the **self-diffing present** core (epic #199): a per-row hash of the last-presented
//! framebuffer, so the present path pushes only the rows that actually changed.
//!
//! The map plane is render-on-demand at the *frame* granularity (the app's dirty signal): a
//! coarse per-screen timer poll (`tick_timers`) decides *whether* to present, but not *where* it
//! changed. Screens stay
//! immediate-mode (`clear()` + redraw), so tracking writes would mark everything dirty. Instead the
//! present layer keeps a 32-bit hash per row and, on present, re-hashes each row, pushes only the
//! contiguous spans whose hash changed, and updates the store. A Home clock ticking a minute
//! re-presents its handful of rows instead of all 320 — on the LS021/FLPR, a few ms vs. a ~44 ms
//! full frame.
//!
//! - [`row_hash`] — FNV-1a mixing over one row as **pre-mixed** `u32` words. 32-bit: 320 rows =
//!   1.28 KB of store, and the only failure mode (a changed row hashing equal, so skipped) is
//!   ~2⁻³² per row-change and **self-healing** — caught systematically by the simulator's
//!   exact-diff CI oracle ([`spans_missed_changes`]); only random collisions reach the field.
//!   (The pre-mix is load-bearing: without it, word-FNV misses certain 2-pixel changes at ~2⁻⁸ —
//!   see [`row_hash`] and issue #626.)
//! - [`diff_rows`] — the core diff, generic over the hash fn (the oracle injects a colliding stub)
//!   and the row count (a `&mut [u32]` store), so the device's fixed-size store and the simulator's
//!   runtime-sized one share one implementation.
//! - [`RowDiff`] — the ergonomic fixed-height store: a `[u32; H]` in `.bss` plus a priming flag.
//! - [`spans_missed_changes`] — the exact-diff **oracle**: a full byte compare (host/tests only,
//!   *never* on device) reporting how many real changes the hash-diff's spans missed. `0` ⇒ honest;
//!   non-zero ⇒ a systematic bug for CI to fail on.
//!
//! Pixel-format-agnostic: the diff is over raw row bytes with a caller-supplied stride (the device's
//! 1-byte/px RGB222 plane, the simulator's 3-byte/px RGB888). It's a separate full-framebuffer hash
//! pass *before* the present — word-at-a-time, well under a ms over the 75 KB device plane — and
//! that extra read earns back far more than it costs against the ~44 ms full-frame push.

/// FNV-1a (32-bit) over one framebuffer row, mixed a **pre-avalanched** `u32` word at a time — the
/// per-row hash the self-diff compares. The self-diffing present runs this over the *whole*
/// framebuffer on every map-dirty present, so it's the diff pass's floor: folding four bytes per
/// multiply (one word load on thumbv8m) instead of one cuts that pass ~4× (issue #350). The values
/// differ from byte-FNV-1a, but the store never leaves this module.
///
/// **Why each word is pre-mixed before the FNV step (issue #626).** Plain word-FNV
/// (`h = (h ^ w) * prime`) only ever moves information toward *higher* bits: XOR is bitwise and
/// multiplying by an odd prime preserves the lowest set bit of a difference (its `2^24` term only
/// adds bits further up). So two rows that differ **only in the top byte of their words** (pixel
/// columns `x % 4 == 3`) keep their hash difference confined to bits 24..32 — 8 bits of
/// discrimination — and a second such changed word cancels it with ~2⁻⁸ probability, not 2⁻³²
/// (measured: 30 784 colliding pairs among the 8.4 M two-pixel variants of one row — e.g. from an
/// all-0x00 row, `fb[3]=0x02, fb[7]=0x2E` hashed *equal*). Real frames hit that family constantly
/// (any row whose change lands only on columns ≡ 3 mod 4: marker edges, glyph updates, dashes), and
/// on a *static* screen the resulting skipped row never self-heals — a persistently stale row on
/// glass, and the oracle panic the sim demo tripped. Multiplying each word by the golden-ratio
/// constant and folding the high half down (`k ^= k >> 15`) avalanches every injected difference
/// across the word *before* it meets the accumulator, so a cancellation again needs a full 32-bit
/// match: structured-family scans (per-lane pairs, sparse two-byte changes, sliding dashes) come
/// out collision-free / at the SipHash-reference rate. Cost: one extra multiply + shift-xor per
/// word — the pass stays well under a ms over the 75 KB device plane.
#[inline]
pub fn row_hash(row: &[u8]) -> u32 {
    /// Avalanche one injected word so no sparse difference survives confined to a byte lane:
    /// golden-ratio multiply spreads low bits up, the shift-xor folds the high half back down.
    #[inline(always)]
    fn premix(w: u32) -> u32 {
        let k = w.wrapping_mul(0x9e37_79b1);
        k ^ (k >> 15)
    }
    let mut h: u32 = 0x811c_9dc5; // FNV-1a offset basis
    let mut words = row.chunks_exact(4);
    for w in &mut words {
        h ^= premix(u32::from_le_bytes([w[0], w[1], w[2], w[3]]));
        h = h.wrapping_mul(0x0100_0193); // FNV prime
    }
    // Byte tail for strides that aren't a multiple of 4 — generality only; the device stride (240)
    // and the simulator stride (720) are both exact.
    for &b in words.remainder() {
        h ^= premix(b as u32);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// The self-diff core: re-hash each of `prev.len()` rows of `fb` (`stride` bytes per row), compare
/// to `prev`, update it in place, and emit each **maximal run of changed rows** as one span via
/// `push_span(y0, rows)`. An unchanged row between two changed ones splits the run, so nothing
/// unchanged is ever pushed.
///
/// `hash` is the per-row hash — [`row_hash`] in production; the oracle passes a colliding stub.
/// `force_all` treats *every* row as changed (the first present after construction / a
/// [`RowDiff::reset`], where the store holds no meaningful prior frame).
///
/// The store's length *is* the row count. Panics (debug) if `fb` is shorter than `rows * stride`.
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

/// A fixed-height per-row hash store — the ergonomic [`diff_rows`] wrapper a board owns in `.bss`.
/// `H` is the frame's row count, so the store is `[u32; H]` (320 rows = 1.28 KB) and the priming
/// flag forces a full first present.
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

    /// [`diff`](RowDiff::diff) with a live overlay's rows **clipped out** — the shared present
    /// skeleton both display backends run (a present with a live overlay's rows excluded,
    /// issue #345).
    ///
    /// Diffs the whole frame (the store is updated for **every** row, the excluded ones included —
    /// it tracks the clean framebuffer, so when the overlay later goes quiet its rows re-push clean
    /// with no stale entry), clips each changed span around the exclude interval `[y0, y0+rows)`
    /// ([`clip_span`]), and collects the clipped spans into the caller's `spans` scratch. If they
    /// don't fit, it falls back to "the whole frame minus the exclude" (≤ 2 spans — pathological
    /// fragmentation a UI never produces) rather than silently dropping rows. Returns the filled
    /// prefix, ascending + disjoint; empty ⇒ nothing changed outside the overlay, push nothing.
    ///
    /// `spans` must hold at least 2 entries (the fallback's worst case).
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

/// Emit the changed-row span `[y0, y0+n)` with the half-open `exclude` interval `[e0, e1)` removed,
/// as up to two ascending, disjoint sub-spans — the **bulge-coordination clip** the LS021/FLPR
/// self-diffing present runs.
///
/// When the hold bulge is live, the map present pushes the changed rows *around* it and leaves the
/// bulge's rows for the overlay composite that follows: presenting them clean here would blank the
/// bulge until the composite repaints them (the "pop flicker" on the long-press transition). A span
/// straddling the bulge splits in two, one entirely inside it emits nothing, one clear of it passes
/// whole. `None` ⇒ no live bulge, always passes through.
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

/// The exact-diff **oracle**: count how many rows that *actually* changed between `prev_fb` and
/// `cur_fb` the hash-diff's `spans` failed to cover. `0` ⇒ honest; non-zero ⇒ a *systematic* miss
/// for CI to fail on (a real device only ever sees random, self-healing collisions).
///
/// A full byte compare of the two frames, independent of the hashes — **never** run on the device.
/// `covered` is a caller-provided `rows`-long scratch (so this stays no-alloc), rewritten each call.
/// Panics (debug) if a frame or the scratch is too short.
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
    // isn't in a no_std crate's extern prelude, so name it.
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
        // Empty row is the bare offset basis — a stable, non-zero seed (no words, no tail, so the
        // word-at-a-time rework left this constant intact).
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

    /// Issue #626 regression: the pre-fix word-FNV hash collided on rows differing **only in the
    /// top byte of their words** (pixel columns ≡ 3 mod 4) at ~2⁻⁸ within device-64 content —
    /// e.g. an all-zero row vs. the same row with `[3] = 0x02, [7] = 0x2E` hashed equal, so the
    /// self-diff skipped a genuinely changed row (a stale row on glass / the sim's oracle panic).
    /// Pin the measured pair, then require the whole two-pixel family — every byte lane — to be
    /// collision-free (the old hash had 30 784 duplicates in lane 3's 4 096-row family).
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

    /// Host microbenchmark for the #626 premix cost: time the full device diff-pass workload — one
    /// [`row_hash`] per row over a 320×240 device-64 plane — for the shipped hash vs. the pre-fix
    /// plain word-FNV, median of many passes. Ignored by default (a timing probe, not a
    /// correctness gate); run it with
    /// `cargo test -p obc-platform --release -- --ignored bench_row_hash --nocapture`.
    #[test]
    #[ignore = "timing probe — run explicitly with --release --ignored --nocapture"]
    fn bench_row_hash_full_plane_vs_prefix_word_fnv() {
        use std::hint::black_box;
        use std::time::Instant;

        /// The pre-fix hash (plain word-folded FNV-1a, no premix) — the #350 baseline.
        fn old_row_hash(row: &[u8]) -> u32 {
            let mut h: u32 = 0x811c_9dc5;
            let mut words = row.chunks_exact(4);
            for w in &mut words {
                h ^= u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
                h = h.wrapping_mul(0x0100_0193);
            }
            for &b in words.remainder() {
                h ^= b as u32;
                h = h.wrapping_mul(0x0100_0193);
            }
            h
        }

        const STRIDE: usize = 240; // device-64: one byte per pixel
        const ROWS: usize = 320;
        // A deterministic pseudo-random device-64 plane (bytes ≤ 0x3F, like real content).
        let mut plane = vec![0u8; STRIDE * ROWS];
        let mut s: u64 = 0x243F_6A88_85A3_08D3;
        for b in plane.iter_mut() {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            *b = ((s >> 33) as u8) & 0x3F;
        }

        /// Median time of one full-plane pass (one hash per row), over 200 passes.
        fn median_pass_ns(plane: &[u8], hash: fn(&[u8]) -> u32) -> u128 {
            const REPS: usize = 200;
            let mut times = Vec::with_capacity(REPS);
            for _ in 0..REPS {
                let t0 = Instant::now();
                let mut acc = 0u32;
                for row in plane.chunks_exact(STRIDE) {
                    acc = acc.wrapping_add(hash(black_box(row)));
                }
                black_box(acc);
                times.push(t0.elapsed().as_nanos());
            }
            times.sort_unstable();
            times[REPS / 2]
        }

        let old_ns = median_pass_ns(&plane, old_row_hash);
        let new_ns = median_pass_ns(&plane, row_hash);
        std::eprintln!(
            "row-hash full {STRIDE}x{ROWS} plane pass (median of 200): pre-fix word-FNV {:.1} us, premixed {:.1} us, ratio {:.2}x",
            old_ns as f64 / 1000.0,
            new_ns as f64 / 1000.0,
            new_ns as f64 / old_ns as f64
        );
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
        // The store tracked the clean fb anyway: a later present with no exclude does NOT re-push
        // the unchanged excluded row (the overlay plane's trailing clear owns repainting it).
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
        // Rows 0, 2, 4 change → three disjoint spans, overflowing a 2-slot scratch: the fallback
        // must cover the whole frame while still respecting the exclude [1, 2).
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
