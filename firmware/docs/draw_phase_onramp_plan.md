# Draw-phase on-ramp — implementation plan (STATUS: PLANNED)

This is **step 6** of the rendering refactor (`rendering_pipeline.md` §10). Steps
1–5 (the structural refactor) are **done and merged into the working tree**; this
doc plans the remaining, deliberately-deferred work that turns the refactored draw
phase into the foundation the line roadmap builds on:

* **[line styles](line_styles_plan.md)** (part 2: dashed / two-colour lines), and
* **[road casing](road_casing_plan.md)** (part 3: outlined roads at the finest LOD).

It exists because two decisions were made now but their *code* deferred to the
feature work — capturing the plan so it isn't re-derived cold later. The
line/page anchors here are **post-refactor** (the numbers inside
`line_styles_plan.md` / `road_casing_plan.md` predate it — trust this doc for
current locations).

## Decisions recorded

1. **Projection stays in microdegrees through collect; project at draw time.**
   We considered pre-projecting geometry during collect (`rendering_pipeline.md`
   §9c) — it would fuse decode→project and let the casing pass re-stroke without
   re-projecting. **Decision: keep µdeg for now.** It keeps collect a pure data
   stage and the camera out of the frame buffers. *Revisit trigger:* if the timing
   harness shows the casing pass's re-projection of road lines is a measurable
   cost on a dense finest-LOD scene, pre-project **lines only** at that point (see
   §"If we later pre-project").
2. **`style_id`-in-`Span` is planned, not yet coded.** The draw loop needs a
   feature's `line_style` + secondary colour, which the 14-byte `Span` doesn't
   carry. The change is cheap (a spare padding byte) but pointless until there's a
   v6 format with those fields to look up — so it lands **with line styles
   (part 2)**, per the detailed plan below.

## What steps 1–5 already gave us (the substrate)

The draw phase is now isolated and factored, which is the whole point — the
roadmap features slot into named seams instead of an inline loop:

* **`MapRenderer::draw_map`** (`obc-render/src/lib.rs:482`) — the draw phase, its
  own method: iterate painter-ordered `frame.spans`, dispatch per `Kind`. This is
  where a **casing pre-pass** will be added (part 3).
* **`draw_line`** (`lib.rs:866`) — the single `Kind::Line` stroke, factored out.
  This is where **`line_style` dispatch** branches (part 2). Doc comment already
  flags it as that seam.
* **`fill_polygon_proj`** (`lib.rs:840`) / **`fill_polygon`** (`lib.rs:919`) —
  polygon path, untouched by the line roadmap.
* **`Span`** (`lib.rs:414`) — `{ kind, z:i8, weight:u8, color:u16, pt_start:u16,
  ring_start:u16, ring_count:u16, seq:u16 }` = **13 bytes used, 14 sized** → one
  spare byte for `style_id` at zero RAM cost.
* **`FrameScratch`/`DrawScratch`** (`lib.rs:267`/`378`) — grouped scratch; a casing
  pass needs **no new buffers** (it re-iterates `spans`, reusing `DrawScratch`).

Critically, `draw_map` is already a standalone `&mut self` method that destructures
into disjoint `frame` (read) + `draw` (write) borrows, so splitting it into ordered
sub-passes is a local change.

---

## Part A — `style_id` into `Span` (lands with line styles)

**Goal:** let the draw loop reach a feature's `line_style` + `color2` without
growing per-feature RAM.

1. **Format (v6) first** — follow `line_styles_plan.md` "Format change → v6":
   style record `6 → 8` bytes (`line_style` in flag bits 2–3, append `color2`
   u16); reader `Style` (`obc-reader/src/reader.rs:28`) gains `line_style: u8` +
   `color2: u16`; `parse_styles` reads them; version gate `5 → 6`. Extend the
   `format.rs` byte-contract tests for the 8-byte record.
2. **`Span` carries `style_id`** — add `style_id: u8` to `Span` (`lib.rs:414`),
   filling the spare padding byte (still 14 bytes; the
   `MCU_RENDERER_BYTES` assert is unaffected). Set it in `collect_level`'s
   `spans.push` (`lib.rs:~340`) — `f.style_id` is already in scope there.
3. **Draw-time lookup** — `draw_map` already calls `color_fn(span.color)`; for
   lines it will also fetch `reader.style(span.style_id)` to read
   `line_style`/`color2`. **`draw_map` doesn't currently take `reader`** — thread
   `reader: &Reader` into `draw_map` (and from `render`, which has it). Polygons
   ignore it.

> **Why look up vs. cache in `Span`:** option (b) in `line_styles_plan.md` (widen
> `Span` with `line_style`+`color2`, ≈ +9 KB) is also fine, but the spare-byte
> lookup is zero-RAM and the style table is a hot `O(1)` array already resolved
> for `color`. Prefer the lookup.

## Part B — `line_style` dispatch in `draw_line` (line styles)

With Part A done, `draw_line` (`lib.rs:866`) switches on the resolved style:

```
solid   → today's single Polyline (unchanged)
dashed  → stroke_dashed(color)                         // admin borders
railway → solid base in `color`, then stroke_dashed(color2) on top
```

* Add **`stroke_dashed`** next to `stroke_overlay` (`lib.rs`): walk the
  already-projected screen points, accumulate arc-length in **screen pixels**
  (zoom-independent dash look), emit short eg `Line` strokes for the "on"
  intervals. **Reuse the existing clip + run machinery** — it should clip first
  (`clip_segment`) exactly like `stroke_overlay`, so off-screen dashes cost
  nothing. The `DrawScratch::screen` run buffer is reused.
* `draw_line`'s signature gains the style (or just `line_style: u8, color2:
  D::Color`, resolved by the caller through `color_fn` so dashes quantize on the
  device like everything else). Keep `weight`.
* Dash on/off lengths = screen-space consts (open question in part 2: whether they
  scale with zoom — default no).

This keeps polygons and the route/breadcrumb overlays untouched; only the map
`Kind::Line` arm changes.

## Part C — casing pre-pass in `draw_map` (road casing)

The correctness constraint (`road_casing_plan.md` "The correctness catch"): **all
casings must be drawn before all fills**, or a casing slices through a crossing
road's fill at a junction. So this is a **two-sub-pass** restructure of `draw_map`,
not a per-feature double-stroke.

Restructure `draw_map` (`lib.rs:482`) into:

```
fn draw_map(reader, target, vp, color_fn):
    if lod == reader.lods().len() - 1:        // finest-LOD gate (cheap at coarse zoom)
        for span in spans where is_cased_road(span.style_id):   // CASING PASS
            draw_line(target, vp, pts, color2, weight + 2*CASING_PX, screen)
    for span in spans:                         // MAIN PASS (today's loop)
        match kind { Polygon => fill_polygon_proj(...), Line => draw_line(...) }
```

* **No new buffers** — the casing pass re-iterates the same `frame.spans` and
  reuses `DrawScratch`. Both passes keep the global `(z, seq)` order (spans are
  already sorted), so same-class overlaps stay deterministic.
* **Gate on finest LOD** via the `lod` the renderer already computes in `render`;
  thread it into `draw_map` (alongside `reader` from Part A). Coarser zooms pay
  nothing and casing on simplified geometry looks wrong anyway.
* **`is_cased_road`** = a `line_style`/flag check via `reader.style(span.style_id)`
  (Part A). Start with the top 1–2 road classes (motorway/trunk/primary) to bound
  the cost; widen later.
* **Casing width** `CASING_PX = 1` (i.e. `weight + 2`). Per-style width is a later
  knob.
* **Join quality** — at casing widths (5–7 px) eg `Polyline` miter spikes get more
  visible; the existing `push_run` 150-px subdivision (`lib.rs`) already fights
  this. If junctions look ragged, hand-roll the stroke as quad + disc per segment
  (the `arm`/`chevron` pattern in `screen/map.rs`) — more code/CPU; escalate only
  if visibly bad.

**This is the only part with real CPU cost** (every cased road rasterised a second
time, wider, where lines are densest). Measure `render_us` before/after on a dense
street scene (the timing harness from the route-arrows work is in place) — see
`road_casing_plan.md` "Cost"/"Verify".

## If we later pre-project (the §9c revisit)

Only if the casing pass's re-projection shows up in `render_us`: store **projected
`Point`s for line spans** in the frame buffer during collect (µdeg stays for
polygons, or move wholesale). Then both the casing pass and the main pass read
screen points directly — project once, stroke twice. This bakes the viewport into
the frame buffer (fine: we never re-draw at a new camera within a frame) and moves
the per-feature bbox cull to screen space. Treat as a follow-up, not part of the
first casing landing.

---

## Sequencing

```
(done) steps 1–5 refactor ── byte-identical output, 146 tests green
   │
   ├─ v6 format bump + reader Style{line_style,color2}        ← Part A.1
   ├─ Span.style_id + draw_map(reader,…) lookup               ← Part A.2–3
   ├─ stroke_dashed + draw_line dispatch  → ship line styles  ← Part B   (line_styles_plan.md)
   └─ draw_map casing pre-pass (finest-LOD gate) → ship casing ← Part C  (road_casing_plan.md)
```

Parts A+B land together (line styles is the first feature needing the substrate);
Part C reuses A's `style_id`/`color2` and the finest-LOD gate.

## Verification bar (each part)

* `cargo test --workspace` green; `cargo clippy --workspace --tests` clean.
* `no_std` crates cross-build: `cargo build -p obc-render -p obc-reader
  -p obc-route --target thumbv8m.main-none-eabihf`.
* **Repack** `freiburg.obcm` with the v6 config, then headless-render a railway, an
  admin border (part B), and a dense junction (part C) — see each feature plan's
  "Verify". For parts that must not change existing output (e.g. solid styles after
  part B), reuse the **before/after md5 diff** that validated steps 1–5
  (`--png` at fixed center/zoom/heading; identical md5 = no regression).
* Part C only: compare `render_us` before/after on the dense scene.
