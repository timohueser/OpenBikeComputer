# Route direction arrows + render-timing telemetry — PLAN (STATUS: DONE 2026-06-19)

> **Done** (revised after review — see "What changed vs the original sketch" below).
>
> `draw_route(.., arrow_color, arrows_at: Option<u32>)` now does **two passes**: (1) stroke every
> visible chunk, (2) chevrons. Chevrons are **anchored to route distance** — placed at multiples of
> a screen-relative spacing along the route's own cumulative distance (`walk_route_arrows`), so they
> stay pinned to the ground as the camera follows the rider — and **windowed** to a chevron *count*
> around the matcher cursor. The spacing is held in **screen space**: each frame the ground spacing is
> `ARROW_SPACING_PX × m/px` (≈ 33 m at riding zoom), so the cadence stays even as you zoom within the
> finest LOD instead of bunching when zoomed out, and the window is `ARROW_BEHIND_COUNT` /
> `ARROW_AHEAD_COUNT` (0 / 9) chevrons either side of the cursor
> (`Activity.progress_m`, threaded from `map.rs`). The window means an out-and-back's two passes never
> collide (only the leg you're on is marked, the right way round), and pass 2 running after the whole
> route is stroked means chevrons sit on top even where the route doubles back. Finest-LOD gate stays
> in `map.rs` (`arrows_at = …then_some(progress_m)`). `ROUTE_WEIGHT` 4→8, white `ARROW_COLOR` sits
> inside the line. `RenderStats.render_us` added (host-filled via `Instant` in `gui.rs`/`main.rs`);
> panel "Render" row. Tests: `walk_route_arrows` grid/window/anchoring (unit) + `draw_route`
> gate/window/stroke-width (tests/arrows.rs).
>
> **Render-cost fix (the headline).** The thick overlay was ~62 ms/frame for two compounding reasons:
> embedded-graphics' thick `Polyline` rasterises width pixel-by-pixel (w1 0.04 ms → w4 54 ms for the
> same geometry), **and** the route/breadcrumb are drawn essentially unclipped — measured **22 of 531
> segments on-screen (96 % off)**, all rasterised by eg (clipped only per-pixel). First attempt
> (`stroke_polyline`: one filled quad per segment via the scanline `fill_polygon`) was fast but its
> per-segment quads don't joint at kinks — curvy routes looked broken. **Final fix** (per review):
> keep eg's properly-jointed `Polyline`, but **clip the overlay to the view first** —
> `stroke_overlay` Cohen–Sutherland-clips each projected segment to the screen (grown by the stroke
> width), splits the line into on-screen runs, and strokes each with eg. eg then only pays for the
> visible part. `draw_route`, `stroke_path`, **and `render()`'s map lines** all use it. **Frame
> 85 ms → ~2.3 ms** with correct joints; routing roads through it too (measured) halved the riding
> view (2.2 → 1.0 ms — the route's underlying mountain road extends off-screen like the route did)
> and cut coarse zoom ~23 % (7.2 → 5.6 ms), for a +0.09 ms clip-overhead tax on dense city views.
>
> **What changed vs the original sketch:** the plan called for screen-arc-length spacing with a
> per-chunk accumulator (`walk_arrows`); review showed that drifts as chunks scroll in/out and points
> the wrong way on repeated roads. Replaced with route-distance anchoring + a rider window. The plan's
> "Cost: negligible" was wrong about the *baseline* stroke (not the chevrons) — hence the
> `stroke_polyline` work above, which the timing harness measured.

**Part 1 of 3** of the line-rendering roadmap (route arrows → [line styles](line_styles_plan.md) →
[road casing](road_casing_plan.md)). This part is self-contained and has **no format change** — do it
first. It also lands the **render-timing harness** the other two parts want for measuring cost.

## Goal

1. Draw **direction chevrons along the active route**, pointing in travel direction, in a contrasting
   colour over the route line — the Garmin/Komoot look. **Only at the finest LOD** (riding zoom); the
   plain stroke stays at all zooms.
2. Add a **render-loop timing harness** and surface per-frame render time in `RenderStats` → the sim's
   control panel and the headless line.

The route weight, chevron style/size/spacing, and colour are meant to be **tuned together** by eye —
this doc fixes the mechanism and leaves those as named constants to sweep.

## Context — what exists today (cited)

- The route is one solid polyline: [`MapRenderer::draw_route`](../obc-render/src/lib.rs#L602) projects
  each chunk and strokes it with `Polyline::with_stroke(color, weight)`. Called from
  [`screen/map.rs:94`](../obc-app/src/screen/map.rs#L94) with `palette::ROUTE` (deep blue) at
  `ROUTE_WEIGHT = 4` ([map.rs:37](../obc-app/src/screen/map.rs#L37)). The breadcrumb (`stroke_path`)
  strokes over it; the marker over that.
- **The chevron machinery already exists twice:**
  - [`draw_marker`](../obc-render/src/lib.rs#L529) builds an oriented chevron from a *forward unit
    vector* — derived by projecting a ground step ahead along the course and taking the screen delta
    (works for north-up and heading-up with no special case), plus its perpendicular. It's a filled
    triangle via `fill_polygon`.
  - The pan HUD's [`chevron` / `arm`](../obc-app/src/screen/map.rs#L325) draws an **open, round-capped
    caret** from filled quads + discs sharing a centreline (even halo). Two reuse models: cheap filled
    triangle vs. prettier open caret.
- The route is decoded **in travel order**, chunk by chunk, consecutive chunks sharing a seam vertex
  ([`draw_route` loop](../obc-render/src/lib.rs#L616)). So "direction of travel" is just increasing
  index; no extra data needed.
- `draw_route` runs **after** `render()` returns, and `render()` returns `stats.lod`
  ([map.rs:90](../obc-app/src/screen/map.rs#L90)). So the finest-LOD gate is
  `stats.lod == reader.lods().len() - 1` (or compare `vp.meters_per_pixel()` to the finest LOD's
  `max_mpp`). `draw_route` currently takes no LOD — thread the gate in as a `bool`.

## Design — (A) route arrows

Add chevrons inside (or right after) `draw_route`, reusing the projection it already does:

- **Arc-length walk in screen pixels.** Carry a running `dist_acc: f32` and place a chevron every
  `ARROW_SPACING_PX`. The route is chunked and each chunk currently clears `screen` and strokes
  independently — so **thread `dist_acc` (and the leftover-to-next-arrow remainder) across the
  per-chunk loop**, not per chunk, or arrows bunch at chunk seams.
- **Direction** at an arrow = the normalised local segment delta `(dx, dy)` you already have from the
  projected points; perpendicular `(-dy, dx)` for the arms (same trick as `arm`/`draw_marker`). Use the
  alpha-max-plus-beta-min `|v|` approximation from [`arm`](../obc-app/src/screen/map.rs#L353) to avoid
  `libm::sqrtf` if you want.
- **Finest-LOD gate.** Pass `draw_arrows: bool` into `draw_route` (computed in `map.rs` from
  `stats.lod`). Plain stroke at every zoom; chevrons only when `draw_arrows`.
- **Glyph style — pick by experiment.** Start with the cheap filled triangle (mirror `draw_marker`'s
  3-point chevron). If it reads too "blobby" over a thick line, switch to the open caret
  (`chevron`/`arm` style). Colour: a fixed palette pick (white or parchment) for contrast over the
  route — **no format change**, it's an overlay, not a styled map feature.
- **Route weight.** Expect to bump `ROUTE_WEIGHT` (4 → 5–6) so a chevron sits nicely *inside* the line
  like Garmin. Tune `ROUTE_WEIGHT`, `ARROW_SPACING_PX`, chevron size, and style as one set.

Optional (defer): draw arrows only on the route **ahead** of the rider (the matcher cursor /
`Activity` progress), leaving the travelled portion plain — this pairs with the "travelled-vs-ahead
styling" item already noted in the route-loading work. Not needed for v1.

### Tunables (one place, to sweep)

```
ROUTE_WEIGHT        (map.rs)   — bump from 4; find the weight a chevron reads inside
ARROW_SPACING_PX               — e.g. 40–60 px between chevrons
ARROW_LEN / ARROW_HALF         — chevron reach + half-spread (px), like draw_marker's TIP/HALF
ARROW_COLOR                    — palette pick (white/parchment), contrast over ROUTE
ARROW_STYLE                    — filled-triangle vs open-caret (compile choice while tuning)
```

### Steps

- **`obc-render/src/lib.rs`** — extend `draw_route` to take `draw_arrows: bool` and emit chevrons on
  the arc-length walk; keep the existing overflow guards (sub-pixel drop, >150 px subdivision, clamp).
  Reuse `self.screen`/`self.xs`. A small private `arrow()` helper (filled triangle) or reuse the caret
  idea.
- **`obc-app/src/screen/map.rs`** — compute `draw_arrows` from the returned `stats.lod`, pass it to
  `draw_route`; tune `ROUTE_WEIGHT` + the arrow consts.

### Cost

- **RAM:** zero new buffers — reuses `screen`/`xs`; state is one `f32` accumulator. (RAM is not a hard
  limit anyway — the 200 KB is a chosen fill-target, retune `MAX_SPANS`/`MAX_FRAME_POINTS` + LODs if a
  later part needs room.)
- **CPU:** negligible — ~5–15 small triangle fills per frame, dwarfed by a single building polygon; the
  arc-length walk piggybacks on the stroke already done.

## Design — (B) render-timing harness

`RenderStats` ([lib.rs:325](../obc-render/src/lib.rs#L325)) is the natural carrier, but `obc-render`
is `#![no_std]` — **no `std::Instant`**. Inject the clock instead of reaching for std, matching the
existing `RideClock` discipline:

- Add `render_us: u32` (0 = "not measured") to `RenderStats`.
- The **host** times the work and fills it: the sim already has a monotonic clock
  ([`device_input.rs` `now_ms`](../obc-sim/src/device_input.rs#L57) / `Instant`); time `draw()`
  (render + overlays) and write it into the stats it stores
  ([`gui.rs` `last_stats`](../obc-sim/src/gui.rs#L117)). On device, the same field is later filled from
  the Cortex-M **DWT->CYCCNT** cycle counter — no API change.
- **Display:** add a "render: X.X ms" row to the stats panel
  ([`gui/panel.rs` ~L397–433](../obc-sim/src/gui/panel.rs#L397)) and the field to the headless line
  ([`main.rs` ~L589–596](../obc-sim/src/main.rs#L589)).

This harness is the measuring stick for [road casing](road_casing_plan.md) (the part with real CPU
cost), so land it here.

## Verify

Repro (route + headless PNG), from the **repo root**:

```
cargo run -p obc-sim -- freiburg.obcm --boot --script pp --routes-dir firmware/routes \
  --gpx kandel.gpx --at 600 --png /tmp/arrows.png --scale 3
```

- Visual: at riding zoom, evenly spaced chevrons point along travel; **even across chunk seams**; none
  at coarse zoom (zoom out and confirm they vanish at the LOD boundary).
- Panel/headless show a non-zero `render: … ms`.
- Tests (`obc-render`): arrow count scales with route length / spacing; **no** arrows emitted when
  `draw_arrows = false`.
- `cargo clippy --all-targets` clean; no_std crates still build for `thumbv8m.main-none-eabihf`.

## Deferred

Arrows only on the ahead-portion (needs matcher cursor); device DWT timing wiring (firmware track).
