# The OBCM Rendering Pipeline — reference & review

A from-scratch walkthrough of how a map frame is drawn, written so we can read it
together, decide whether the core algorithm is solid or can be simplified, and
then refactor with those findings in hand. Part 1–8 describe **what exists today**
(with `file:line` anchors); part **9 is the review** — opinionated observations and
opportunities; part **10** proposes a sequenced refactor.

Primary sources read for this doc:
`obcm-render/src/lib.rs` (the renderer), `obcm-reader/src/reader.rs` (decode +
quadtree), `obcm-app/src/screen/map.rs` + `app.rs` (orchestration),
`obcm-sim/src/{framebuffer,gui}.rs` (host), `OBCM_Spec.md` (format), and the
design docs `two_pass_descriptor_plan.md`, `render_followups.md`,
`{route_arrows,line_styles,road_casing}_plan.md`.

---

## 1. Architecture at a glance

The renderer is one `no_std`, zero-allocation crate (`obcm-render`) that runs
**byte-for-byte identically** on the desktop simulator and on the nRF5340
firmware. The only thing that differs between host and device is the surface it
draws into and the colour policy — both injected as generics:

```
MapRenderer::render<D: DrawTarget, F: Fn(u16) -> D::Color>(target, reader, vp, bg, color_fn)
                     │                  │                    │       │     │    └ RGB565 → panel pixel
                     │                  │                    │       │     └ camera (Viewport)
                     │                  │                    │       └ parsed map (Reader)
                     │                  │                    └ where pixels go (host framebuffer / LS021B7DD02)
                     └ the device colour type (Rgb888 on host, panel-native on device)
```

* **`DrawTarget`** (`embedded-graphics` 0.8) is the pixel sink. The sim's is
  `Framebuffer` (`obcm-sim/src/framebuffer.rs`), a plain row-major RGB888 buffer
  whose `fill_solid` is a fast clipped rectangle blit — the renderer leans on that
  for its scanline fills and `clear`. The device implements `DrawTarget` over its
  panel driver. Out-of-bounds writes are clipped silently on both.
* **`color_fn: Fn(u16) -> D::Color`** is the colour seam. Styles store
  device-independent **RGB565**; the host resolves each to a concrete pixel — true
  colour in the sim, RGB222/64-colour quantization on the device
  (`obcm-reader/src/color.rs`). The renderer never sees panel specifics.

Everything else (projection, LOD pick, quadtree walk, decode, priority ordering,
polygon fill, line stroke, marker, route, chevrons) lives in the shared crate.

Crate map:

| Crate | Role in a frame |
| :-- | :-- |
| `obcm-reader` | Parse header/styles/LOD table; quadtree walk; streaming feature decode. |
| `obcm-route`  | Parse `.obcr` routes; per-chunk decode; ground-distance math. |
| `obcm-render` | Projection + the whole draw pipeline (this doc). `canvas`/`text` are UI chrome, not map. |
| `obcm-app`    | Orchestration: screen stack, camera state, what to draw on top of the map. |
| `obcm-sim`    | Host shell: window/event loop, `Framebuffer`, colour policy, frame timing. |

---

## 2. The data the renderer consumes

Full byte layout is in `OBCM_Spec.md`; the renderer-relevant model:

* **Style table** (global, ≤254 entries, `reader.rs:508 parse_styles`). Each
  `Style` = `{ id, z_index: i8, color: u16 RGB565, weight: u8, priority: 1..=4 }`.
  Indexed `O(1)` by id into a `[Option<Style>; 256]`.
  * `z_index` → painter's order (low drawn first).
  * `priority` (2 bits of the flags byte, `1` = keep-first … `4` = drop-first) →
    which features survive buffer saturation.
  * `backdrop_style()` (`reader.rs:226`) = the lowest `(z_index, id)` style; its
    colour clears the screen before geometry.
* **LOD pyramid** (`reader.rs:44 Lod`). N self-contained levels, **coarsest (0) →
  finest (N-1)**. Each owns a quadtree index + a chunk set and a `max_mpp`
  threshold. Geometry is pre-simplified per level, so zooming out reads a small
  coarse layer instead of decimating fine geometry at runtime.
* **Quadtree index** (per LOD, flat `[u32]`). High bit set = branch
  (`0x8000_0000 | first_child`); else leaf (`0x7FFF_FFFF` = empty, otherwise a
  chunk id). Children are **NW, NE, SW, SE** over the *global* bbox split at
  floor-division midpoints — identical math at every level (`reader.rs:287
  walk_leaves`).
* **Chunks** (fixed `chunk_size` bytes, `0xFF` style-id = end sentinel). Each holds
  delta-encoded features; a feature's first vertex (the **anchor**) is stored
  relative to its leaf's min corner, the rest chain as `int8`/`int16` deltas.
  Polygons may carry holes; lines use only the exterior ring.
* **Route** (`.obcr`, `obcm-route`): chunked polyline with per-chunk bbox +
  cumulative ground distance, decoded on demand into a 256-point scratch.

---

## 3. Per-frame data flow

```
host frame (gui.rs / main.rs)
└─ App::render_frame                                   app.rs:515
   ├─ (rebuild cached elevation profile if route changed)
   ├─ for each screen on the stack from the base:      app.rs:557
   │   └─ Screen::draw → MapScreen::draw               screen/map.rs:90
   │      ├─ vp = state.viewport(w, h)                 (camera → Viewport)
   │      ├─ bg = color_fn(backdrop_style.color)
   │      ├─ stats = renderer.render(target, reader, &vp, bg, color_fn)   ← THE MAP
   │      ├─ renderer.draw_route(... arrows_at)        (route stroke + chevrons)
   │      ├─ renderer.stroke_path(... breadcrumb)      (travelled trail)
   │      ├─ renderer.draw_marker(... user fix)        (you-are-here glyph)
   │      └─ off-route pill / pan HUD (Canvas chrome)
   └─ hold_hints.draw (global long-press overlay)
   host wraps the whole draw in a timer → stats.render_us   gui.rs:278
```

`render` paints the **map**; the overlays (route, breadcrumb, marker, HUD) are
separate calls layered on top in a fixed order. Only the base screen's
`RenderStats` is returned for the panel.

The camera is assembled in `AppState::viewport` (`app.rs:155`) →
`Viewport::new_rotated`, taking the shared `cam_lon/cam_lat/zoom` plus a
`course_rad` that is `0` (north-up) or the live/frozen heading (heading-up / pan).

---

## 4. Stage by stage

### 4.1 Camera & projection — `Viewport` (`lib.rs:131`)

State: `w, h, cam_lon, cam_lat (µdeg), zoom (px per µdeg-lat), aspect = cos(lat),
course_rad`, plus `sin_c/cos_c` precomputed once per frame (rotation is per-point
hot).

**`to_screen(lon, lat)` (`lib.rs:176`)** — the per-vertex hot path:

1. `delta = coord.wrapping_sub(cam)` in **i32 µdeg** (keeps full precision
   relative to the camera; only the small delta is cast to f32).
2. `ex = delta_lon * aspect`, `ny = delta_lat` (longitude squashed by `cos(lat)`).
3. Rotate by `course_rad` about centre: `rx = cos·ex − sin·ny`,
   `ry = −sin·ex − cos·ny` (at course 0 this is `(ex, −ny)` — y flips for
   screen-down).
4. Scale by `zoom`, offset to screen centre, **round to nearest** (`libm::roundf`).

Round-to-nearest (not truncation) is deliberate: truncation biases toward the
screen centre and was the second half of the chunk-seam crack story (see §8).

`to_map` (`lib.rs:197`) is the exact inverse and reuses the same coefficients (the
screen↔ground rotation is its own inverse). `visible_bbox` (`lib.rs:214`)
un-projects all four screen corners and takes their AABB, so a **rotated** view
still culls correctly. `north_screen_unit` (`lib.rs:253`) gives the compass needle
for free as `(−sin_c, −cos_c)`.

### 4.2 LOD selection — `select_lod_for_mpp` (`reader.rs:236`)

`mpp = 0.11132 / zoom` (`Viewport::meters_per_pixel`, `lib.rs:243`; independent of
display size). Pick the **finest** LOD whose `max_mpp >= mpp`; the coarsest is
`+inf` so a result always exists. Linear scan over ≤16 levels.

### 4.3 Visible bbox & quadtree walk — `for_each_chunk` (`reader.rs:279`)

`walk_leaves` recurses from node 0 with the global bbox, descending only into
children whose bbox intersects the view, and invokes `visit(chunk_id, node_bbox)`
for every non-empty leaf. **Streaming, uncapped** — this is the fix for the old
silent `MAX_CHUNKS` cap (a wide LOD-0 view overlaps ~250 leaves; capping at 128
silently dropped half the map *before* priority logic, see
`two_pass_descriptor_plan.md`). The walk only reads `u32` nodes — no decode — so
re-running it is cheap.

### 4.4 Decode — `for_each_feature_filtered` (`reader.rs:341`)

Walks a chunk's bytes; for each 12-byte feature header it consults
`should_decode(style_id)` **before** touching coordinates:

* `false` → advance past the geometry with `skip_ring` (`reader.rs:452`), pure
  offset arithmetic, **no coordinate math, no buffer writes**.
* `true` → `read_ring` (`reader.rs:467`) reconstructs absolute µdeg points
  (anchor + chained deltas) into the caller's `points`/`ring_lens` scratch and
  hands back a borrowed `FeatureRef`.

`skip_ring` mirrors `read_ring`'s offset arithmetic exactly; a reader test
(`filtered_decode_skips_without_drifting`) pins them together so they can't drift.
This skip-don't-decode filter is what lets the priority multi-pass decode each
feature's coordinates **at most once per frame** (§4.5).

### 4.5 Collect — the priority multi-pass (`lib.rs:441` / `collect_features:270`)

The frame buffers are fixed-size and a dense view holds far more geometry than
fits, so when they saturate the **dropped features must be the lowest priority,
globally across chunks** — never land/sea/major roads, wherever they live.

The shipped design (a "header-scan multi-pass", `two_pass_descriptor_plan.md`):

```
clear frame_points, frame_ring_lens, spans
for_each_chunk(...) { stats.chunks_visited += 1 }          // a 5th, count-only walk
for level in 1..=4:                                        // lib.rs:469
   for_each_chunk(lod, view):                              // re-walk the tree
      for_each_feature_filtered(should_decode = style.priority == level):
         compute feature bbox from decoded points          // lib.rs:305
         if !feat_bbox.intersects(view): return            // tighter than the leaf cull
         if buffers can't fit this feature: drop, return   // capacity check
         push Span{kind,z,weight,color, pt/ring offsets, seq}
         frame_points.extend(pts); frame_ring_lens.extend(lens)
```

Because passes run lowest-number-first and each fills the buffers *before* the
next begins, saturation drops strictly by priority across all chunks. Each feature
matches exactly one level, so its coordinates decode once per frame (header-scanned
and skipped in the other three passes). Utilization is recorded for the stats panel
(`lib.rs:485`).

`Span` (`lib.rs:382`) is the 14-byte per-feature draw record:
`{ kind, z: i8, weight: u8, color: u16, pt_start: u16, ring_start: u16,
ring_count: u16, seq: u16 }`. Offsets are `u16` (buffers asserted ≤ `u16::MAX`) to
keep it small — thousands are buffered at coarse zoom.

> Why not buffer-all-then-sort? The rejected "descriptor" alternative
> (`two_pass_descriptor_plan.md` §"Why not") needs a descriptor entry per *visible*
> feature; that buffer overflows on early chunks in a 15k-feature view and
> reintroduces the exact bug. The multi-pass has no intermediate cap.

### 4.6 Painter sort (`lib.rs:490`)

`spans.sort_unstable_by_key(|s| (s.z, s.seq))` — z-index first, `seq` (insertion
order) as a stable, alloc-free tiebreak. Sorts spans only; geometry stays put.

### 4.7 Draw: polygon scanline fill — `fill_polygon` (`lib.rs:877`)

For each span in painter order, the `Kind::Polygon` arm projects every ring vertex
into the reused `screen: Vec<Point>` and calls `fill_polygon`:

* Clamp the polygon's `[ymin, ymax]` to the screen.
* For each scanline `y + 0.5`: for each ring, for each edge, collect x-crossings
  into `xs` (even-odd rule — **holes fall out for free** because they're just more
  rings in the same crossing list).
* Sort `xs`, fill `(xs[2k], xs[2k+1])` spans with `fill_solid` (one rect per span).
* **Outward span rounding**: `x0 = floor(left)`, `x1 = ceil(right)` so adjacent
  fills overlap by ≤1px. This is the cheap insurance that closes hairline
  background cracks at chunk seams (see §8) and at thin junctions.

`fill_polygon` is shared: the map polygons, the marker glyph, and the route
chevrons all fill through it.

### 4.8 Draw: line stroke — `stroke_overlay` (`lib.rs:808`)

The `Kind::Line` arm (and every overlay polyline) strokes through one path:

* **Clip first** (Cohen–Sutherland, `outcode`/`clip_segment`, `lib.rs:716/735`)
  against the screen grown by the stroke half-width, so an edge-hugging line keeps
  full thickness. This is the fix for the old ~62 ms/frame overlay cost: the route
  is ~96 % off-screen at riding zoom, and `embedded-graphics`' thick `Polyline`
  rasterises width pixel-by-pixel over the *whole* line. Clipping means eg only
  pays for the visible part.
* Sub-pixel steps (`|dx|+|dy| < 2`) are dropped; each on-screen run is accumulated
  in `screen` and flushed as one eg `Polyline::with_stroke(color, weight)` —
  keeping eg's properly **jointed** thick line within a run.
* `push_run` (`lib.rs:773`) subdivides any > 150 px hop into ≤150 px steps so eg's
  thick-line intersection math doesn't overflow (debug) or spike miters (release).
* A run is flushed and restarted whenever a segment leaves the view or doesn't
  continue the previous one.

### 4.9 Overlays (drawn after `render`, on top, in this order)

1. **Route** — `draw_route` (`lib.rs:624`). *Pass 1*: stroke every view-intersecting
   route chunk via `stroke_overlay` (consecutive chunks share a seam vertex, so the
   per-chunk strokes join). *Pass 2* (only when `arrows_at` is `Some`, i.e. finest
   LOD + a fix): direction **chevrons**, in a separate pass so they sit on top even
   where the route doubles back. `walk_route_arrows` (`lib.rs:852`) emits a chevron
   at every multiple of `ARROW_SPACING_M` of **route distance** within a
   `[progress−behind, progress+ahead]` metre window, so each chevron is pinned to a
   ground spot (doesn't crawl with the rider) and an out-and-back's two legs never
   collide. Each chevron is a 3-point triangle filled via `fill_polygon`.
2. **Breadcrumb** — `stroke_path` (`lib.rs:702`): the two-tier travelled trail
   (coarse spine → full-res recent tail) as one chained `stroke_overlay`, navy,
   over the route.
3. **Marker** — `draw_marker` (`lib.rs:539`): a course-pointing chevron (or a
   stationary diamond) at the fix, fixed screen size, culled when off-view by a
   margin. "Forward" is found by projecting a point a ground-step ahead and taking
   the screen delta — correct for north-up and heading-up with no special case.
4. **HUD chrome** (`screen/map.rs`): off-route pill, pan-mode chevrons/compass/
   back-to-you arrow — drawn through `Canvas` (the `text`/shape helper), not the
   map path.

---

## 5. Memory model & budgets (`lib.rs:32–81`)

Every buffer is a `heapless::Vec` owned by `MapRenderer`, cleared (not freed) each
frame — steady state does **zero heap work**. Capacities, tuned for a 512 KB-RAM
MCU:

| Buffer | Const | Holds |
| :-- | :-- | :-- |
| `dec_points` | `MAX_DECODE_POINTS` 2048 | one feature's vertices during decode |
| `dec_ring_lens` | `MAX_DECODE_RINGS` 32 | one feature's ring lengths |
| `frame_points` | `MAX_FRAME_POINTS` 12288 | all visible features' vertices, concatenated |
| `frame_ring_lens` | `MAX_FRAME_RINGS` 3072 | per-feature ring lengths |
| `spans` | `MAX_SPANS` 3072 | per-feature draw records (14 B each) |
| `screen` | `MAX_SCREEN_POINTS` 4096 | projected points for the feature being drawn / line run |
| `xs` | `MAX_CROSSINGS` 256 | scanline x-crossings |

Compile-time guards: buffers indexed by `u16` offsets are asserted `≤ u16::MAX`
(`lib.rs:66`), and `MCU_RENDERER_BYTES` is asserted `≤ 200 KB` (`lib.rs:81`) so
growing a buffer fails the build if it blows the budget.

---

## 6. Host integration & timing

The sim builds a `Framebuffer`, calls `App::render_frame` with
`color_fn = |c| color_of(c, true_color)`, then uploads the RGB888 buffer to an
egui texture (or PNGs it headless). The whole draw is wrapped in
`Instant::now()`/`elapsed()` and folded into `stats.render_us` (`gui.rs:278`,
`main.rs:584`) — `obcm-render` is clockless `no_std`, so the **host** times it (the
device will use the Cortex-M DWT cycle counter). Surfaced as a "Render" row in the
control panel (`gui/panel.rs:430`) and on the headless line. This is the harness
that makes the casing/line-style cost (§9d) measurable.

---

## 7. Test coverage

* **`obcm-render/tests/priority.rs`** — the payoff test: a synthetic two-chunk map
  that *saturates* `MAX_SPANS` with priority-4 polygons in an early chunk plus one
  priority-1 polygon in a late chunk, rendered through the real `render`; asserts
  saturation occurred **and** the priority-1 colour survived. Fails if collection
  reverts to chunk-order dropping.
* **`tests/arrows.rs`** — chevron gate (finest-LOD only), windowing, and route
  stroke width, through a recording `DrawTarget`.
* **`tests/marker.rs`** — glyph pixels at the anchor, stationary vs course glyph,
  off-screen cull, chevron tip follows course.
* **`tests/text.rs`** — palette colour survives device-64 quantization; alignment.
* **Reader** (`obcm-reader/tests/format.rs`) — `filtered_decode_skips_without_drifting`,
  `for_each_chunk_has_no_cap`, plus the format byte-contract.
* Renderer unit tests in `lib.rs` cover `walk_route_arrows` grid/window/anchoring.

---

## 8. Known tradeoffs (from `render_followups.md`)

* **Chunk-seam cracks → kept ≤1px outward overdraw.** Independent per-chunk
  clipping (`obcm/quadtree.py`) emits different boundary vertices on each side of a
  shared edge; combined with the *old* truncating `to_screen` this opened diagonal
  cracks under rotation. Round-to-nearest (§4.1) removed the divergence; the
  remaining outward overdraw (~2 % of pixels, ≤1px, invisible for same-colour
  fills) is cheap insurance, kept by decision over a fiddly ingestion/repack fix.
* **Within-level drops are quadtree-ordered** — same-priority features bunch NW
  under saturation. Acceptable; a spatial stride/importance weight could spread
  them.
* **Static buffer split isn't usage-tuned** — coarse overview saturates `spans`
  *and* `rings`; retune once there's device data.

---

## 9. Review: is the core solid? Observations & opportunities

**Verdict up front:** the *algorithm* is sound and well-reasoned — the priority
multi-pass, the streaming uncapped walk, the clip-then-stroke line path, and the
shared scanline fill are all good choices with documented rationale. The redundant
work that exists sits on the *cheap* operations (tree walk, header scan), and we
just gained a timing harness to confirm that. The biggest wins are in **code
structure** (param bundling, decode/cull fusion, draw-phase shape) and in **setting
up for the planned casing/line-style features**, not in replacing the core. Items
are tagged **[algo]** (does less work) / **[code]** (reads better) / **[future]**
(eases the roadmap), with a rough confidence.

### 9a. Redundant passes over cheap data — `[algo]`, low–medium value

* **The 5th, count-only quadtree walk** (`lib.rs:460`) exists purely to set
  `stats.chunks_visited`. Every priority pass already walks the identical chunk
  set, so pass 1 (`level == 1`) could increment the counter and the dedicated walk
  deleted. Trivial, unambiguous win.
* **4× tree walk + 4× header scan.** Each priority pass re-walks the quadtree and
  re-scans *every* feature header in *every* visited chunk, decoding only its
  quarter. Decode (the expensive part) is already at-most-once; the 4× is on the
  walk + the 12-byte-header + `skip_ring` arithmetic. The harness should tell us
  whether that's material at LOD 0 (≈250 chunks). If it is, the cheapest mitigation
  that keeps the "uncapped, priority-correct" guarantees is to **walk the tree once
  into a reused `(chunk_id, node_bbox)` list, then iterate that list 4×** — trading
  one bounded buffer for 3 tree walks. Worth holding until measured; flagged in
  `render_followups.md` "Later".

### 9b. Decode/cull/copy fusion — `[algo]` + `[code]`, medium value

The collect path currently touches each *drawn* feature's points up to three times
before the draw phase even starts:

1. `read_ring` writes them into `dec_points`,
2. `collect_features` loops them again to compute the feature bbox for the cull
   (`lib.rs:305-314`),
3. `extend_from_slice` copies the accepted ones into `frame_points`
   (`lib.rs:344`).

Two concrete simplifications:

* **Fold the bbox into decode.** `read_ring` already visits every vertex; having it
  track running min/max and expose a `bbox` on `FeatureRef` makes step 2 free and
  deletes the manual loop in `collect_features`. (`skip_ring` is unaffected.)
* **Decode straight into the frame tail.** Since the priority filter already avoids
  decoding rejected features, the only post-decode rejection is the bbox cull + the
  capacity check. If decode wrote into `frame_points` at its current tail (after a
  capacity check) and we truncated on cull, the **`dec_points`/`dec_ring_lens`
  buffers and the per-feature copy both disappear** (−~16 KB scratch, −one memcpy
  per drawn feature). The cost is coupling the reader's decode to the frame layout;
  doable by handing the frame buffers in as the decode scratch. This is the single
  biggest structural simplification available in collect.

### 9c. Where projection happens — `[algo]` + `[future]`, medium value

Today geometry is stored in **µdeg** in the frame buffers and projected to screen
**at draw time** (`render`'s draw loop, and again independently by each overlay).
Because the camera is fixed for the frame, projection could equally happen **during
collect**, storing `Point`s (same 8 bytes as `(i32,i32)`). That would:

* fuse decode→project into one pass and drop the separate draw-time projection;
* **directly help road casing** (§9d): the casing pass and the fill pass both want
  the same projected line — project once, stroke twice, instead of re-projecting.

The tradeoff: it bakes the viewport into the frame buffer (we never re-draw at a
new camera within a frame, so this is free in practice) and the per-feature bbox
cull would move to screen space. I lean toward **keeping µdeg in collect for now**
but pre-projecting *lines* feels worth it once casing lands — worth a deliberate
decision rather than drift.

### 9d. Draw-phase shape vs. the line roadmap — `[future]`, high value (do before parts 2–3)

The draw phase is a single sorted loop with a `match span.kind { Polygon | Line }`
(`lib.rs:494`). The three planned features push on exactly this spot, so the
refactor should shape it for them **now**:

* **`line_styles_plan.md`** needs the `Line` arm to dispatch on a `line_style`
  (solid/dashed/railway) and reach a secondary colour. `Span` has a spare padding
  byte — putting `style_id` there (zero RAM) lets the draw loop look up
  `line_style`/`color2` from the style table. Factor the current single-`Polyline`
  stroke into a `draw_line(span, …)` helper so adding `stroke_dashed` is local.
* **`road_casing_plan.md`** needs **all casings before all fills** within the road
  band (else a casing slices through a crossing road's fill at junctions). That is
  fundamentally a **two-sub-pass** draw over the road spans — incompatible with the
  current single interleaved loop. Designing the draw phase as
  `casing_pass(); main_pass();` (each honouring `(z, seq)`, casing gated to finest
  LOD) is the change to make while we're in here.

So the highest-leverage refactor is: **split `render` into `collect()` and a
`draw()` that's structured as separable passes**, with line drawing factored out
and `Span` carrying `style_id`. This is mostly mechanical today and saves a painful
restructure mid-feature later.

### 9e. Three chevron implementations — `[code]`, low–medium value

The "triangle/caret pointing along a unit vector, with the alpha-max-plus-beta-min
`|v|` normal trick" appears at least three times: `draw_marker`'s chevron
(`lib.rs:581`), `draw_route`'s chevron closure (`lib.rs:687`), and
`screen/map.rs`'s `chevron`/`arm`/`outlined_arrow` (`map.rs:347–386`). The
`(−uy, ux)` perpendicular and the `/ (|dx|max + 0.41|dy|min)` normalisation recur
in `back_to_you` too. A small shared "directional glyph" helper in `obcm-render`
(tip/back/half + fill via `fill_polygon`) would absorb the two map-overlay chevrons
and the marker; the `Canvas`-based HUD ones may stay (different surface). Pure
readability, no behaviour change.

### 9f. `MapRenderer` ergonomics — `[code]`, medium value

* **`collect_features` takes 10 positional args** (clippy-silenced) and `render`
  hand-destructures `Self` into seven locals to split borrows
  (`lib.rs:444`); `draw_marker`/`draw_route`/`stroke_path` each repeat a
  `let Self { screen, xs, .. } = self`. Grouping the collect buffers into a
  `FrameScratch { dec_points, dec_ring_lens, frame_points, frame_ring_lens, spans }`
  and the draw scratch into `{ screen, xs }` collapses the param lists and the
  split-borrow boilerplate, and makes the "these are reused, cleared per frame"
  contract a type rather than a comment.
* **`render` is long** (clear → LOD → bbox → collect → sort → draw). Extracting
  `collect(reader, vp, lod) -> &spans` and `draw(target, vp, color_fn)` makes each
  testable in isolation and is the precondition for §9d.

### 9g. Polygon fill scaling — `[algo]`, low value unless measured

`fill_polygon` is `O(rows × edges)`: every scanline re-tests every edge of the
polygon. For a screen-filling coastline at coarse zoom (thousands of edges) that is
the likely hot spot. The textbook fix is an **active-edge table** (`O(edges log
edges + spans)`), but it needs more state/RAM and more code. The current naive
version is simple and correct; **don't pre-optimise** — let `render_us` at LOD 0
decide. Polygons also aren't clipped before fill (only the y-range and x-spans are
clamped), so off-screen edges still cost per-scanline tests; Sutherland–Hodgman
polygon clipping is the option there, again only if measured to matter.

### 9h. Small notes — `[code]`/correctness, low value

* `MapRenderer::new()` is just `Self::default()`; fine, but the `#[derive(Default)]`
  + manual `new` is slightly redundant.
* The header constant comment says "v4 header is fixed-size" (`reader.rs:19`) and
  `HEADER_LEN`/`LOD_ENTRY_LEN` docs mention v4 — stale wording now that v5 is the
  only version; tidy while nearby.
* `draw_route` pass 2 re-decodes chunks pass 1 already decoded for the windowed
  region. The window is ~300 m (1–2 chunks), so it's cheap, and fusing it would
  fight the deliberate "chevrons in a second pass, on top" structure — **leave it**.
* `stats.render_us` being mutated by the host onto a struct the renderer returns is
  a slightly leaky seam, but pragmatic for a clockless `no_std` core — fine.

### 9i. What to explicitly NOT change

The priority multi-pass core, the uncapped streaming walk, the clip-then-stroke
line path, round-to-nearest projection, and the ≤1px overdraw seam insurance are
all load-bearing and well-justified by tests + measurements. The refactor should
preserve their behaviour exactly (the `priority.rs` saturation test and the seam
measurements are the guardrails).

---

## 10. Refactor sequence (low-risk → higher)

Ordered so each step is independently shippable and test-green. **Steps 1–5 are
done** (in the working tree): output verified **byte-identical** (md5-equal `--png`
at three center/zoom/heading scenes), 146 tests green, clippy clean incl. tests,
`no_std` crates cross-build for `thumbv8m.main-none-eabihf`.

1. ✅ **Tidy** — count-only chunk walk folded into the level-1 pass (§9a); stale
   v4 doc wording fixed (§9h). (`new()` kept: it's the public constructor used by
   `App` + 8 tests — removing it was churn, not an improvement.)
2. ✅ **Bundle scratch into `FrameScratch` + `DrawScratch`** (§9f) — collapsed the
   10-arg `collect_features` (now `FrameScratch::collect_level`) and the
   seven-field split-borrow in `render`/overlays.
3. ✅ **Split `render` into `FrameScratch::collect` + `MapRenderer::draw_map`** and
   factored line drawing into `draw_line` (+ `fill_polygon_proj`) (§9d, §9f).
   `draw_map` is now the named seam the casing/line-style passes slot into.
4. ✅ **Fused the feature bbox into decode** (§9b) — `FeatureRef::bbox`, accumulated
   in `read_ring` (no extra pass); `collect_level` culls on it. Mathematically the
   same point set, so output is identical (not merely ≤1px). *(Decoding into the
   frame tail to drop `dec_points` was left for later — a bigger reader/render
   coupling; the bbox win was the cheap half.)*
5. ✅ **Consolidated the directional-glyph chevrons** (§9e) into `fill_chevron`,
   shared by `draw_marker` and `draw_route`.
6. ⏳ **Draw-phase on-ramp** (§9c projection placement + §9d `style_id`-in-`Span`)
   — **decided, code deferred to the feature work.** Projection stays in µdeg;
   `style_id`-in-`Span` lands with line styles. Detailed plan:
   **`draw_phase_onramp_plan.md`**, feeding `line_styles_plan.md` →
   `road_casing_plan.md`.
7. ⏳ **Only if the harness flags it:** chunk-list caching (§9a) and/or AET polygon
   fill (§9g). Measure `render_us` first.
