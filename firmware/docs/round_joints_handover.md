# Thick-line round joints + subpixel stroke simplification — HANDOVER (STATUS: EXPERIMENTAL)

> **Experimental, landed as one commit on `develop` so it can be reverted wholesale.** Fixes the
> "beading"/scalloping on thick lines (route + thick roads) without coarsening them. The risk is
> per-vertex disc fill cost on the MCU — **must be profiled on the STM32F429 before this is
> blessed** (see *Next step* below).

## The problem

Thick lines (the magenta route at `ROUTE_WEIGHT = 11`, heavy road classes) rendered with a
lumpy, scalloped, "beaded" edge — worst when zoomed out. Cause: `embedded-graphics` joins thick
`Polyline` segments with a flat **bevel** (and butt-caps the ends), so a densely sampled curve
becomes a fan of facets. It is *not* a sub-pixel artifact — the vertices genuinely bend more than
a pixel; eg just renders the joints flat.

Decimating the line away (an aggressive simplify) hides the facets but **coarsens** the shape —
the two are the same operation, so simplification alone can't win. Confirmed visually with
`obc-sim --png` at several zooms.

## The fix — two independent pieces

1. **Round joints + caps (the actual fix).** [`flush_run`](../obc-render/src/lib.rs#L746): after
   eg strokes each run, fill a disc (⌀ = stroke width) at every run vertex, for `weight > 2`. Each
   joint/cap becomes a smooth arc instead of a flat bevel — **every vertex kept, full shape detail,
   no decimation.** The disc at a shared chunk-seam vertex also closes the butt-cap gap between
   adjacent features. **This is the per-vertex cost the F429 test must vet.**

2. **Subpixel stroke simplification (a pure optimization, *not* the beading fix).**
   [`stroke_overlay`](../obc-render/src/lib.rs#L866) runs [`simplify`](../obc-render/src/lib.rs#L823)
   /[`within_eps`](../obc-render/src/lib.rs#L782) over the projected points first, at
   [`SIMPLIFY_EPS_PX`](../obc-render/src/lib.rs#L776)` = 0.75`. A streaming one-lookahead collinear
   drop: a vertex is removed if it sits within 0.75 px **perpendicular of the straight line through
   the vertices kept on either side of it** (i.e. it barely bends the line). It folds away the
   integer-projection staircase and same-pixel vertex pile-ups a dense route/road carries when
   zoomed out, *without moving the drawn line a visible (≥1 px) pixel*. It keeps the disc count
   bounded to real bends — without it, a straight diagonal would get a disc on every pixel.

`eps = 0.75` because `to_screen` rounds to whole pixels (staircase steps sit ≤ 0.5 px off the true
line, so 0.75 clears them with margin) and 0.75 < 1 px guarantees invisibility. It's a streaming
O(1) approximation (Reumann–Witkam-ish), not a global Douglas–Peucker — fine for lines; **not**
direction-symmetric, which matters for polygons (see *Future work*).

## Cost (sim, release, `freiburg_thick.obcm`)

Disc fill measured by toggling the `weight > 2` gate, min of 6 frames:

| view | discs off | discs on | delta |
|------|-----------|----------|-------|
| riding (`--zoom 450`)      | 0.71 ms | 0.72 ms | ~0 (free) |
| extreme zoom-out (`--zoom 65`) | 5.12 ms | 5.58 ms | +0.46 ms (~9 %) |

Riding (the common case) is free — few thick vertices on screen, and thin roads (`weight ≤ 2`)
skip discs entirely. Zoom-out adds ~9 % on the Mac; **disc fill is pixel-bound, so the MCU figure
will be larger and is the open question.**

## Next step — merge to `mcu-render-bench`, profile on STM32F429

- Merge this `develop` work into **`mcu-render-bench`** and run the existing F429 bench (defmt/RTT
  on real silicon — see [`rp2040-render-bench` notes / `render_followups.md`](render_followups.md)).
- Watch the **per-frame render time at a zoomed-out, thick-road view** (where disc fill peaks),
  and compare against the same frame with discs gated off.
- **If the discs are too expensive,** revert this single EXPERIMENTAL commit, or reach for a knob
  (cheapest first):
  - Raise the disc gate (e.g. `weight ≥ 5`) — skips the breadcrumb and minor roads; accepts slight
    beading on thin-ish lines. The route (11) and major roads keep theirs.
  - Skip discs on `push_run`'s collinear subdivision points (pure overdraw on long straights; only
    the simplify-kept *bends* actually need a disc).
  - Only disc where the turn angle exceeds a threshold.
  - Keep the simplify (it's a cheap win on its own) and drop only the discs (back to eg bevels).

## Future work (separate)

**Subpixel dedup for polygons.** Would cut ring vertex counts → faster scanline fill **and** fewer
points hitting `MAX_FRAME_POINTS`/`MAX_SPANS`, i.e. fewer *dropped* features when zoomed out (a real
completeness win). But polygons fill (no joints → no beading), so it's optimization-only — and
adjacent areas share edges traversed in opposite directions, where this greedy non-symmetric
`simplify` would diverge and crack seams (cf. the outward-overdraw seam decision in
`render_followups.md`). Needs a direction-symmetric simplifier (Douglas–Peucker) or to be done at
pack/LOD time, not naively at render time.

## Verification

- `cargo test -p obc-render` — `thick_line_end_gets_a_round_cap` (round joint/cap probe, in
  [tests/stroke.rs](../obc-render/tests/stroke.rs); confirmed to fail with discs gated off),
  `within_eps_*` / `simplify_*` units.
- Visual: `obc-sim <map> --png out.png --center 7820000,47980000 --zoom 90 --scale 3` (thick roads
  smooth, full detail) vs the same with the `weight > 2` gate disabled (beaded).
