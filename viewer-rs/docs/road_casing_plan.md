# Road casing / borders at the finest LOD — PLAN (STATUS: PLANNED)

**Part 3 of 3** of the line-rendering roadmap ([route arrows](route_arrows_plan.md) →
[line styles](line_styles_plan.md) → road casing). **Depends on [part 2](line_styles_plan.md)**: it
reuses that part's `color2` style field as the casing colour and its line-style flag bits. This is the
**only part with real CPU cost** — land [part 1](route_arrows_plan.md)'s timing harness first and
measure before/after.

> **Note (post-refactor):** the `lib.rs` line numbers below predate the renderer refactor and are
> stale. The draw phase is now `MapRenderer::draw_map`, with line drawing factored into `draw_line`
> — the casing pre-pass slots in there. See **[`draw_phase_onramp_plan.md`](draw_phase_onramp_plan.md)**
> for current anchors and the two-sub-pass structure this plan needs.

## Goal

At high zoom (the finest LOD), draw roads with a **casing**: a darker outline on each side of the road
fill — the "roads have borders" look of the reference image (and of OSM-carto / Komoot). Coarser LODs
keep today's flat strokes.

## Context — what exists today (cited)

- Lines and polygons are drawn **interleaved in one painter's-order loop**, sorted by `(z_index, seq)`
  ([`render` draw phase, lib.rs:461](../obcm-render/src/lib.rs#L461)). Each road line is a single
  `Polyline::with_stroke(color, weight)` ([Kind::Line arm, lib.rs:478](../obcm-render/src/lib.rs#L478)).
- Roads occupy a **z-band ~24–60** ([`config.json` highway block](../../config.json#L9)); areas/water sit
  below, admin/labels above.
- `render()` already selects and reports the LOD (`stats.lod`); the finest is
  `reader.lods().len() - 1`. The config's finest level is **LOD 2** (`simplify: 0`,
  [config.json:5](../../config.json#L5)).
- The `Span` doesn't carry `style_id`; part 2 adds it (or the fields) so the draw loop can see a style's
  casing flag + `color2`.

## The correctness catch

Casing = "draw each road twice: a **wider casing-colour** stroke, then the **narrower fill** on top."
The trap: **all casings must be drawn before all fills**, or one road's casing paints over a *crossing*
road's fill at a junction — you'd see the casing colour slicing through intersections. So this is **not**
a per-feature double-stroke in the existing loop (that interleaves by z and produces exactly that
artifact); it needs a **casing pass before the fill pass** within the road band.

## Design

- **Two sub-passes over road-class line spans**, both inside the existing sorted-`spans` iteration (no
  new buffers):
  1. **Casing pass:** for spans that are road lines with casing enabled, stroke at
     `weight + 2*CASING_PX` in `color2` (casing colour).
  2. **Fill pass:** the normal stroke (today's loop), drawn after, on top.

  Simplest structure: factor line-drawing into a helper, then in the draw phase run a casing pre-pass
  over the road spans, then the existing single loop for fills + everything else. Keep the global
  `(z, seq)` order within each pass so same-class overlaps stay deterministic.
- **Finest-LOD gate.** Run the casing pass **only when `lod == reader.lods().len() - 1`**. Coarser LODs
  pay nothing (and casing on simplified coarse geometry looks wrong anyway). The reference look is a
  high-zoom-only feature.
- **Which styles get casing.** Reuse part 2's line-style/flag: e.g. a `casing` line-style value (or a
  spare flag bit — bits 4–7 are free). Casing colour = `color2`. **Start with just the top 1–2 road
  classes** (motorway/trunk/primary) to bound cost; widen later.
- **Casing width.** Start fixed (`CASING_PX = 1`, i.e. +2 px total — 1 px each side). A per-style casing
  width is a later knob.
- **eg join quality.** At casing widths (5–7 px) embedded-graphics `Polyline` miter spikes get more
  visible — the renderer already fights these by subdividing > 150 px segments
  ([lib.rs:490](../obcm-render/src/lib.rs#L490)). If junctions look ragged, hand-roll the stroke as
  quad + disc per segment for round joins (the [`arm`/`chevron`](../obcm-app/src/screen/map.rs#L325)
  pattern) — more code + CPU; start with eg `Polyline` and only escalate if it's visibly bad.

## Steps

- **`obcm-render/src/lib.rs`** — restructure the `render` draw phase into casing-pass + main-pass; the
  casing pass needs each road span's casing flag + `color2` (via the `style_id`-in-`Span` lookup from
  part 2). Gate on finest LOD.
- **`config.json`** — mark the cased road classes with a casing line-style + a `color2` that is a
  *darker* step on the RGB222 grid (visibly distinct from the fill — the panel is 64-colour).
- **`screen/map.rs`** — nothing required (optional debug toggle).

## Cost

- **RAM:** ~zero — re-iterates the existing `spans`, reuses part 2's `color2`. No new buffers.
- **CPU:** the real cost. Every cased road is rasterised a **second time, wider**, exactly where lines
  are densest (finest LOD: residential/service/footway everywhere — though start with only the top
  classes). Rough budget **~1.5–3× the road-line raster work at the finest LOD only**; the gate keeps
  every coarser zoom free. This is a *reasoned* bound (extra pixel-writes + extra eg join math), **not
  measured** — there's no profiling in-repo yet, which is why part 1 lands the timing harness.
  **Measure render-ms before/after** on a dense street scene; mitigate by fewer cased classes / smaller
  `CASING_PX` / hand-rolled stroke only if needed.

## Verify

With part 1's timing telemetry on, headless-render a dense street area at the finest zoom, before/after:

```
cd viewer-rs && cargo run -q -p obcm-sim --release -- ../freiburg.obcm --size 240x320 --scale 3 \
  --center 7849000,47996000 --zoom 2500 --png /tmp/casing.png
```

- Cased roads show a clean darker border; **junctions do not show casing cutting through crossing
  fills** (the painter-order correctness — this is the thing to scrutinise).
- Casing absent at coarse zoom (zoom out past the LOD-2 boundary).
- Compare `render: … ms` in the panel/headless line before vs. after to quantify the cost.
- `cargo test --workspace` green; no_std crates build for `thumbv8m.main-none-eabihf`.

## Open questions

- Casing on which classes (just motorway/trunk/primary, or down to residential)?
- Fixed `CASING_PX` vs per-style casing width.
- eg `Polyline` vs hand-rolled quad+disc stroke for join cleanliness — decide from the first render.
