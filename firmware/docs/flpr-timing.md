# FLPR frame-time work — baseline, calibration, and lever log (issue #348)

The measured record behind the #348 timing pass on `obc-fw-nrf54l/src/flpr/flpr_scan.c`.
Method notes first, then the baseline table every lever is judged against, then the per-lever
before/after log.

## Method: software round-trip calibration (the LA is retired)

The original plan called for LA captures of `busy(120)`. The bench logic analyzer (and the
webcam rig) are no longer available, so step 0 was re-based on a **software round-trip
measurement**, which is *more* precise for what we actually need (the cost model of `busy()`,
not wire waveforms):

- Temporary FLPR commands (never merged): `0xC0DE0001` = one `busy(spans[0])`;
  `0xC0DE0002` = `spans[0]` calls of `busy(spans[1])`, harness-unrolled ×8 so the outer loop's
  add+branch is noise.
- The M33 rings them and times the ack round-trip with `embassy_time::Instant` (µs clock).
  The measured round-trip floor (ring → no-op ack) is **2 µs**; runs are 80–2300 ms, so the
  floor and the clock granularity are negligible.
- What this *cannot* measure: actual wire pulse widths (ns-scale GPIO edge-to-edge). Those are
  now accepted on the issue's explicit policy — measured frame-time wins + visual parity on
  glass, with the over-spec question a documented decision (see the timing-policy header in
  `flpr_scan.c`).

Raw capture (2026-07-04, DK @ 128 MHz, blob at `-Os`, commit = the #347 merge state):

```
CAL: round-trip floor 2 µs
CAL: busy(1000000) = 80444 µs
CAL: busy(2000000) = 163813 µs
CAL: busy(4000000) = 321853 µs
CAL: 1M × busy(0) = 102899 µs (per call ≈ 102 ns)
CAL: 1M × busy(1) = 183239 µs (per call ≈ 183 ns)
CAL: 1M × busy(2) = 264058 µs (per call ≈ 264 ns)
CAL: 1M × busy(3) = 354474 µs (per call ≈ 354 ns)
CAL: 1M × busy(26) = 2280202 µs (per call ≈ 2280 ns)
```

Derived:

- **Per-iteration slope: 80.5 ns** (4M−1M fit: (321853−80444)/3 M) ⇒ the true calibration is
  **12.4 iters/µs**, not the labelled `ITERS_PER_US = 13` — every labelled duration runs
  ~4.6 % long. (At 128 MHz that is ~10.3 cycles/iteration: the `volatile` counter forces a
  load/add/store + compare/branch round trip through SRAM each iteration.)
- **Per-call overhead: ~102 ns** (`busy(0)`) — call + prologue + the first volatile compare.
  Consistency check: 102 + n×80.5 predicts busy(1)/(2)/(3)/(26) at 183/263/344/2196 ns;
  measured 183/264/354/2280 — within a few percent (small-n costs also carry the argument
  `li`, larger n a hair of I-fetch variance).

## Baseline (the #347 merge state, before any #348 lever)

Full 320-row frame: **95 910–95 959 µs** (~299.7 µs/row), repeatable within ±50 µs across
boots. (`frame OK` line at `DEFMT_LOG=debug`.)

Constants vs reality — labelled duration is `iters / 13` per the old calibration; measured is
`102 ns + iters × 80.5 ns` per the table above:

| Constant | iters | Labelled | Measured | Spec bound (bring-up table) |
|---|---|---|---|---|
| `BCK_HALF_ITERS` | 2 | "~180 ns" | **264 ns** | ≥660 ns hi & lo (over-spec, accepted #348 policy) |
| `DATA_SETUP_TOPUP_ITERS` | 1 | "~280 ns incl. pack" | **183 ns** + pack | data setup ~335 ns (over-spec, accepted) |
| `GCK_SETTLE_ITERS` | 65 | 5 µs | **5.33 µs** | *not an enumerated minimum* (audited in lever 2) |
| `GEN_SETUP_ITERS` | 221 | 17 µs | **17.9 µs** | ≥16.37 µs ✓ |
| `GEN_HIGH_ITERS` | 325 | 25 µs | **26.3 µs** | ≥24.56 µs ✓ |
| `GCK_HIGH_ITERS` | 130 | 10 µs | **10.6 µs** | fast-forward GCK ≥1 µs (10× margin) |
| `FRAME_SETUP_ITERS` | 130 | 10 µs | **10.6 µs** | chart framing setup |

Where the 299.7 µs/row goes (model, cross-checked against the measured frame):

- Gate fixed cost: 2 × `gen_pulse` (26.3 + 17.9 + edges) + 2 × `GCK_SETTLE` (5.33) ≈ **99 µs**.
- 2 × sub-line ≈ **200 µs** (≈100 µs each = 62 word-pairs × ~1 616 ns). Of each pair's
  1 616 ns, the four small `busy()` calls are **894 ns** (183+264+183+264) — and ~408 ns of
  that is pure *call overhead*, not delay. The rest (~722 ns) is the two packs + four to six
  GPIO stores + loop control at `-Os`.

So the recoverable budget, in descending order: the four inner-loop `busy()` calls
(≤ ~35 ms/frame), pack/loop codegen (the ~722 ns residue), `GCK_SETTLE` (~2.7 ms/frame if
trimmed to ~1 µs), dummy-advance width (partial-push fast-forward only).

## Lever log

Each lever lands as its own commit with the before/after `frame OK` measurement here.

| Lever | Frame (µs, 320 rows) | Δ | Glass check |
|---|---|---|---|
| Baseline (#347 merge) | 95 910–95 959 | — | ✓ (#347) |
| 1a: drop the 2 × `DATA_SETUP_TOPUP` busys (the pack **is** the setup window) | 82 714–82 729 | −13.2 ms | ✓ (with 1b) |
| 1b: drop the 2 × `BCK_HALF` busys (presents + next pack **are** the half-width) | 60 053 | −22.7 ms | ✓ contours crisp, no doubling/sparkle/banding |
| 2: `GCK_SETTLE` 5 µs → 1 µs (not an enumerated minimum; spec's only GCK floor is the ≥1 µs fast-forward width) + dummy-advance high 10 µs → 2 µs | 57 655 | −2.4 ms | ✓ (with 2b) |
| 2b: recalibrate `GEN_HIGH`/`GEN_SETUP` to the measured 80.5 ns/iter — 310/207 iters ≈ 25.06/16.77 µs, in-spec with ~2 % margin (were ~7 % over via the stale 13 iters/µs label) | 56 215 | −1.4 ms | ✓ (with 2) |
| 3a: blob `-Os` → `-O2` (build.rs; blob 814 → 1 268 B, ≪ the 4 KB carve) | 47 712 | −8.5 ms | ✓ (with 3b) |
| 3b: non-volatile fb reads + hot loop over the 118 pipelined data words only (last pair peeled, 4 flush words clocked separately — the per-word bounds branch gone) | 44 081 | −3.6 ms | ✓ home + map + bulge all clean at ~210 ns halves |

**End state: 44.1 ms full frame (−54 % from 95.9 ms), past the ~53 ms stretch goal.** The
sub-line is now 100 % useful work — pack + GPIO presents pace the wire (~210 ns BCK halves,
~3× under the 660 ns spec minimum; owner decision 2026-07-04: keep max speed, the policy
header documents the over-spec margins + the single-unit caveat). Fully in-spec BCK halves
(660 ns) were costed at ~+36 ms/frame (~80 ms total) and declined per the issue's policy.
Overlay (bulge) partial pushes, measured on glass at the end state: **64 dirty rows = 9.43 ms,
88 rows = 12.3 ms, 112 rows = 15.7 ms** (~140–147 µs/row incl. the gate fast-forward) — vs
~21 ms for the 64-row case pre-#348: no regression, a ~2.2× improvement.
A side effect worth knowing: bulge overlay pushes dropped ~21 → ~9 ms (5× faster gate
fast-forward), which makes the hold-pop animation's designed "fast lunge" visibly snappier —
the animation is wall-clock-paced (`hold_hint.rs`), so durations are unchanged; retuning
`POP_MS`/`POP_ATTACK` is a UI preference, not a #348 regression.
