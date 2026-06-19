# Renderer Follow-ups

Open items after the priority-rendering work (commit `6189fd4`, "Priority-based
rendering: global cross-chunk ordering, seam fix, bigger span budget"). Ordered
by priority.

**Status:** items 1–3 are all resolved (see each section). Remaining work is in
"Later / lower priority" below.

Useful repro command (headless render at an arbitrary spot/zoom/rotation):

```
cd firmware
cargo run -q -p obc-sim --release -- ../freiburg.obcm --size 240x320 --scale 3 \
  --center 7849000,47996000 --zoom 900 --heading 35 --png /tmp/out.png
```

`freiburg.obcm` (v5, ~213 MB) is the test map. The headless `--png` line prints
`features drawn/tried, chunks, LOD, dropped, spans/points/rings utilization` to
stderr.

---

## 1. Render-level test for priority under saturation — DONE

Resolved by `firmware/obc-render/tests/priority.rs`
(`priority_one_survives_saturation_across_chunks`). The test builds a synthetic
two-chunk map — `MAX_SPANS + 64` priority-4 polygons overflowing the span buffer
in an *early* NW chunk, plus one large priority-1 polygon in a *late* NE chunk —
renders it through the real `MapRenderer::render` into a recording `DrawTarget`,
and asserts saturation actually occurred *and* the priority-1 polygon's color is
present. Verified to fail (0 priority-1 pixels) when the priority-pass order is
reversed, so it guards exactly against a regression to chunk-order dropping. The
original analysis is kept below for context.

**Gap.** Reader-level invariants are covered (`filtered_decode_skips_without_drifting`,
`for_each_chunk_has_no_cap` in `firmware/obc-reader/tests/format.rs`), and
`firmware/obc-app/tests/marker.rs` exercises the render path — but nothing
asserts the actual payoff: **under buffer saturation, priority-1 features survive
and priority-4 features are dropped, across chunks.**

**Why it's awkward.** Saturation needs more features than `MAX_SPANS = 3072`
(or `MAX_FRAME_POINTS`/`MAX_FRAME_RINGS`), which a tiny synthetic map won't hit.

**Approach (pick one):**
- Add test-only (smaller) buffer capacities behind a `cfg(test)`/feature so a
  modest synthetic map saturates, then assert the drawn set is exactly the
  highest-priority features. The buffer consts live at the top of
  `firmware/obc-render/src/lib.rs`; they'd need to become overridable.
- Or build a synthetic multi-chunk map (the `format.rs` byte builders are a good
  model) with, say, 1 priority-1 polygon in a *late* chunk and many priority-4
  polygons in *early* chunks that exceed a (lowered) buffer, and assert the
  priority-1 one is drawn. A `DrawTarget` that records filled pixels/spans (see
  `marker.rs`'s `Buf`) makes the assertion concrete.

**Acceptance:** a test that fails if collection ever reverts to chunk-order
(non-priority) dropping.

---

## 2. Seam fix root cause vs. the 1px-overdraw tradeoff — RESOLVED (keep overdraw)

**Decision.** Keep the ≤1px outward overdraw; do **not** do the ingestion +
repack fix. Once item 3 landed, the rotation-dependent seam divergence was
already gone, so the ingestion fix's gate ("only worth it if the overdraw
becomes visible") is not met.

**Why (measured, doc repro at zoom 900, heading 35 vs. a heading-0 control).**
The cracks came from *two* compounding factors: (1) per-chunk independent
clipping in `packer/obcm/quadtree.py` emits different boundary vertices on each side of
a shared edge, and (2) the old `to_screen` *truncation* mapped that one diagonal
edge to two divergent pixel staircases. Item 3's round-to-nearest removes factor
(2) — near-coincident boundary points now round to the same pixel instead of
falling off truncation's hard integer cliff. With the overdraw removed
(zero-overdraw fill + rounding), the rotated view reopened only ~100 isolated
1px gaps — *fewer* than the heading-0 control's ~125 and scattered at building
corners / line junctions, **not** clustered along any diagonal seam. So no
seam-localized crack signal remains; the outward overdraw now just closes
incidental ≤1px edge gaps present at every heading, at a cost of ~2% of pixels
in the test view (≤1px, invisible for same-colored fills).

**Why not the ingestion fix.** Beyond not being warranted: making adjacent
pieces share identical boundary vertices is fiddly to coordinate across
independently-processed sibling chunks, and "stop clipping per chunk" would
break the quadtree/AABB cull (features must stay bounded by their chunk or a
boundary feature is dropped when its home chunk falls outside the view). Plus a
full ~213 MB repack. Revisit only if the overdraw ever becomes visible at some
zoom/style combination. The original root-cause analysis is kept below.

### Original analysis

**Current state.** Chunk-seam "cracks" (background showing through where a
polygon clipped across a chunk boundary splits into two pieces) are fixed by
**outward span rounding** in `fill_polygon` (`firmware/obc-render/src/lib.rs`):
`x0 = floor(left)`, `x1 = ceil(right)` so adjacent pieces overlap by ≤1px. See
the long comment there. Verified: seam pixels 198 → 0 at heading 35; no
regression at heading 0.

**Cost.** It grows *every* polygon by ≤1px per side (~2.7% of pixels changed in
the test view). Invisible for same-colored fills, ≤1px elsewhere — acceptable,
but not free.

**Root cause (for a zero-overdraw fix).** Adjacent clipped pieces carry
*different* boundary-vertex sets (chunk-size-dependent densification in
`packer/obcm/serialize.py::_densify` / the per-chunk clip in `packer/obcm/quadtree.py`), so
the one shared map-space line gets two different `i32`-truncated pixel
staircases in `Viewport::to_screen`. They diverge only when the seam is diagonal
on screen — i.e. only when the view is rotated.

**Approach if the 1px growth ever bothers us:** make adjacent pieces share
identical boundary vertices — e.g. snap/normalize the densification of
clip-boundary segments so both sides emit the same points, or stop clipping
features per chunk. Bigger change (ingestion + repack); only worth it if the
overdraw becomes visible. Pairs naturally with item 3.

---

## 3. `to_screen` truncates instead of rounding — DONE

Resolved: `Viewport::to_screen` now ends with
`(libm::roundf(x) as i32, libm::roundf(y) as i32)`. Visual pass over
`freiburg.obcm` at headings 0/35/90 and zooms 200/900/2500 showed only sub-pixel
(≤1px) edge shifts — `round(v) − trunc(v) ∈ {0, ±1}` — with no new seam cracks,
no vanished features, and the marker still landing at the camera center; render
stats are unchanged (collection is unaffected) and the marker-placement tests
still pass. This also turned out to do most of item 2's job (see below). Original
analysis kept below.

**Issue.** `Viewport::to_screen` (`firmware/obc-render/src/lib.rs`) ends with
`(x as i32, y as i32)`, which truncates toward zero — asymmetric around the
origin, and it feeds the staircase divergence in item 2.

**Approach.** Switch to round-to-nearest (`libm::roundf`). It's more correct and
may quietly improve sub-pixel quality, but it touches *all* projected geometry
(polygon fill, lines, marker, and the `to_map` inverse used by `visible_bbox`),
so it needs a visual pass with the repro command above at several headings/zooms
and a check that the marker still lands correctly. Cheap to try; revert if it
regresses anything. Won't fix the seam alone (vertex sets still differ) but is a
prerequisite for a cleaner item-2 fix and good on its own.

---

## Later / lower priority

- **Buffer balance at extreme zoom.** Whole-region overview saturates `spans`
  *and* `rings` (points ~80%). The compile-time budget assert in `lib.rs` guards
  ≤200 KB on the MCU, but the static spans/points/rings split isn't tuned to
  real usage. Retune once there's device data.
- **Within-level drops are quadtree-ordered** (same-priority features bunch NW
  under saturation). A spatial stride / importance weighting would spread them.
- **Stale planning doc.** `firmware/docs/priority_rendering_plan.md` describes
  the superseded boolean 2-pass design; `two_pass_descriptor_plan.md` is the
  as-built record. Consolidate or remove the stale one.
- **nRF54L firmware bring-up** (separate track). Renderer is MCU-ready —
  heapless, zero-alloc, budget-asserted ≤200 KB. Remaining work is the embassy +
  LS021B7DD02 `DrawTarget` front-end. See the `obcm-followups` memory.
