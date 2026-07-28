# OpenBikeComputer docs — plan & build notes

The roadmap for the project documentation under `/docs/`. Not published (Trunk only
emits `index.html` + the `copy-dir` output); this is the working spec we write each
page against and the record of *why* the docs are shaped the way they are.

Decisions locked with the maintainer (2026-06-22):

- **Authoring:** human-editable **Markdown** sources rendered to themed HTML by a small
  **stdlib-only Python** script. SVG figures are embedded directly in the markdown.
- **Diagrams:** "**field-guide blend**" — technically precise (data layouts, pipeline
  flows, state machines) but wearing the site's skin (parchment/ink, the topo + trail-
  marker palette, naturalist-plate annotation). Bespoke inline SVG, no diagram-as-code.
- **First pass:** the **scaffold** (renderer, theme, sidebar nav, overview page,
  hardware/build skeletons) **+ one fully-polished page**: the **rendering pipeline**.
  That page sets the quality/diagram bar; the rest scale to it in later passes.
- **Code references:** concept + philosophy + diagrams, with lightweight "where to look"
  **file links** (no line numbers — they rot) and the **occasional inline snippet** where
  it genuinely clarifies.

The docs are a *conceptual* companion to the code, **not** an API reference. The code is
already meticulously documented in-source; the docs exist to explain the **core concepts,
the philosophy, and the load-bearing boundaries** — the things a newcomer can't easily
reconstruct by reading files one at a time.

> **Caveat for every page:** the design notes in `firmware/docs/` and `packer/docs/` are
> point-in-time drafts and have drifted from the code (e.g. the "Elevation" screen is
> `Statistics` in code; `RouteSwap` exists; `Transition` grew `Root`/`Home`). Treat them
> as a starting draft and **verify every claim against source** before writing it down.

---

## Information architecture

```
/docs/                         Overview — what OBC is, the one-glance system map,
│                              "what's where" cards into each section
│
├─ software/                   THE priority
│   ├─ architecture            "Two hosts, one render path": the crate graph, the
│   │                          per-frame loop, and the load-bearing seams (DrawTarget,
│   │                          the color_fn quantization seam, ByteSource streaming)
│   ├─ formats                 OBCM map + OBCR route — the "read directly off flash,
│   │                          no JSON, no heap" philosophy; LOD pyramid + quadtree
│   ├─ rendering               ★ FIRST POLISHED PAGE — the whole draw pipeline
│   ├─ ui                      Screen stack + Transitions, the 5 gestures, render-on-
│   │                          demand / two-plane model, guarded actions, visual language
│   └─ packer-and-routing      (secondary) OSM → OBCM packing; route import + map-matching
│
├─ hardware/                   Placeholder skeleton — display/MIP, MCU, schematic, PCB, power
└─ build/                      Placeholder skeleton — BOM, tools, flashing, assembly
```

Layout: a shared **left sidebar** (sections + pages) on every doc page, a centered prose
column, and an on-page "on this page" mini-TOC for long pages. The marketing landing
(`/`, the wasm demo) stays separate; docs live under `/docs/`.

---

## Toolchain

Dependency-free so the build never breaks on a missing package and `trunk build` stays
self-sufficient (the CI image already has `python3`).

```
docs/
├─ Trunk.toml            + a [[hooks]] pre_build that runs build_docs.py
├─ index.html            the marketing landing (unchanged)
├─ build_docs.py         the renderer (stdlib only): markdown → themed HTML + sidebar
├─ content/              ← edit here
│   ├─ site.toml         nav manifest (sections, page order, titles)
│   ├─ index.md          Docs overview
│   ├─ software/*.md
│   ├─ hardware/index.md
│   └─ build/index.md
├─ templates/
│   └─ page.html         the shared shell (header, sidebar slot, content slot, footer)
├─ assets/
│   └─ docs.css          the field-guide docs stylesheet (shares the landing tokens)
└─ docs/                 ← GENERATED (gitignored), what Trunk copy-dirs to dist/docs/
```

**Build flow:** `python3 docs/build_docs.py` reads `content/*.md` + `site.toml`, renders
each page into the `page.html` shell with the sidebar, and writes `docs/docs/**`. A Trunk
`pre_build` hook runs it automatically on `trunk serve`/`trunk build`, so the deploy needs
no change. Output is gitignored (same pattern as `dist/`). The `--check-links` flag adds a
cross-page anchor audit (every `../page/#anchor` must resolve to a real heading id) and
exits non-zero on a break — the CI `docs` job runs this.

### The renderer (supported markdown)

A focused CommonMark-ish subset — everything the docs actually use, kept small and
predictable so the markdown is comfortable to hand-edit:

- ATX headings `#`..`######` → auto-slugged `id`s (sidebar/TOC anchors)
- paragraphs, `**bold**`, `*italic*`, `` `code` ``, `[text](href)`
- unordered (`-`) and ordered (`1.`) lists, including nesting
- fenced code blocks ` ``` ` with a language label (no syntax highlighting — flat, on-theme)
- pipe tables (used heavily in the existing specs)
- blockquotes `>` → styled **callout/aside** boxes
- horizontal rule `---`
- **raw block HTML/SVG passthrough** — a line starting with `<` at column 0 is emitted
  verbatim through to its matching close (this is how SVG figures embed)
- front-matter (`--- … ---` at top) for `title` / `description`
- two small conveniences: a `::: figure … :::` fence (figure + caption) and a `[src:path]`
  shorthand that expands to a GitHub file link

---

## Visual system (field-guide blend)

Reuse the landing page's tokens so the docs feel like the same world:

- parchment `#ece8cf` / `#e4dec0` / `#d6cda8`, ink `#24331c`, forest `#3c6b39`,
  wood `#5f7d3d`, amber `#e3ad33`, coral `#cf6a2a`; serif (Iowan) headings, sans body,
  mono for code/labels; the drifting topo contours as a faint backdrop.
- **Diagram conventions** (consistency across every figure):
  - parchment panels with thin forest/ink strokes; coral = "the hot path / the thing to
    notice"; amber = the user/route accent; muted greens/blues for map features.
  - mono labels, small-caps section tags, hand-numbered call-outs ①②③ like a plate.
  - flows read left→right or top→down with clear arrowheads; data layouts drawn as
    labeled byte cells; state machines as rounded nodes + labeled edges.
  - every figure has a one-line italic caption.
  - one reusable inline SVG stylesheet (via `<style>` inside each SVG, or shared classes)
    so strokes/fills/fonts stay uniform.

---

## Page-by-page content outline

### Overview (`/docs/`)
What OBC is in three sentences; the **system map** (one hero diagram: OSM data → packer →
OBCM; GPX → OBCR; both → the shared app/render path → simulator *and* device); a "what's
where" grid linking the four software pages + hardware + build; a short "how to read these
docs" note (conceptual, not an API ref).

### ★ Rendering pipeline (`/docs/software/rendering/`) — first polished page
The full draw of one map frame, originally distilled from the since-retired
`firmware/docs/rendering_pipeline.md` and **re-verified** against `obc-render/src/lib.rs` + `obc-reader/src/reader.rs`. Sections &
planned figures:

1. **One render path, two surfaces** — the `DrawTarget` + `color_fn` seam; identical code
   on sim and device. *(small seam diagram)*
2. **The pipeline at a glance** — ★ hero conveyor figure: bytes → project → LOD → quadtree
   cull → priority passes → painter sort → fill/stroke → overlays → panel.
3. **Projection** — µdeg deltas vs. the camera, `cos(lat)` squash, heading-up rotation,
   round-to-nearest (and *why* — the chunk-seam crack story). *(globe→screen figure)*
4. **LOD pyramid** — pre-simplified tiers, `mpp` threshold pick. *(tier-stack figure)*
5. **Quadtree cull** — descend only into intersecting NW/NE/SW/SE children; streaming &
   uncapped (the old `MAX_CHUNKS` bug). *(quadtree-over-bbox figure)*
6. **Skip, don't decode** — `should_decode` before touching coordinates. *(chunk byte-stream figure)*
7. **The priority multi-pass** — ★ why dropped features must be globally lowest-priority;
   the 4 passes filling fixed buffers; decode-once. *(passes-filling-buffers figure)*
8. **Painter's order** — `(z, seq)` stable sort. *(layer-stack figure)*
9. **Filling polygons** — even-odd scanline, holes for free, outward rounding. *(scanline figure)*
10. **Stroking lines** — clip → simplify → stroke, round joints/caps, the off-screen route
    cost win. *(clip+joints figure)*
11. **Overlays** — route → chevrons → breadcrumb → marker order. *(overlay-stack figure)*
12. **Zero allocation** — fixed `heapless` buffers, cleared not freed; the budget table.
    *(supply-box figure)*

### Architecture / Formats / UI / Packer+Routing — later passes
Stubbed this pass (title + outline + "coming next") so the nav is complete and navigable.
Outlines kept here for when we write them:

- **Architecture:** crate dependency graph; the `App::tick → handle_input → render_frame
  → take_dirty` loop; the two-host model & where the device host slots in; the seams
  (DrawTarget, color_fn, ByteSource, LocationSource/Sensors HAL); render-on-demand & the
  two-plane (input preempts map) model at a high level.
- **Formats:** the binary-by-design philosophy; OBCM (header/styles/LOD/quadtree/chunks,
  delta geometry); OBCR (chunked polyline + profile + ByteSource); how the packer/styles
  feed them.
- **UI:** the screen enum + `Transition` stack; "adding a screen is a local edit"; the 5
  gestures + hold ring; Idle/Riding/Paused modes; guarded actions; the MIP visual language
  (redraw-on-change, dither, flat fills); per-screen gesture map (state diagram).
- **Packer + routing:** OSM ingest → multipolygon assembly → land → quadtree → serialize;
  GPX → OBCR conversion, deadband, map-matching, the elevation LOD profile.

### Hardware (`/docs/hardware/`) — placeholder skeleton
Real section page with "coming soon" subsections: the MIP display (LS021B7DD02 class,
240×320, 64-color, reflective/sunlight-readable, holds image), the nRF54L MCU, schematic,
PCB, power. No invented specifics.

### Build guide (`/docs/build/`) — placeholder skeleton
"Coming soon" subsections: bill of materials, tools, flashing the firmware, assembly.
**No mention of any kit** (undecided whether one will exist).

---

## Build order (first pass)

1. Renderer engine: `build_docs.py` + `templates/page.html` + `assets/docs.css` + `site.toml`.
2. A trivial page + preview to validate the toolchain & the look early (fail fast on SVG
   passthrough before writing all the figures).
3. Iterate the theme until it feels like the landing page's sibling.
4. Write the rendering page with its figures (the bar-setter).
5. Overview page + hardware/build skeletons + software stubs.
6. Wire the Trunk `pre_build` hook; gitignore the generated dir; verify a clean
   `trunk build` produces the whole site.
7. Present for reaction.
```
