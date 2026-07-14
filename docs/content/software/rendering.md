---
title: Rendering pipeline
description: How OpenBikeComputer draws one map frame — projection, level-of-detail, the quadtree cull, the priority multi-pass, and the polygon/line rasterisers — running identically on the simulator and the device.
---

# The rendering pipeline

Drawing a map on a microcontroller is a budgeting problem. A dense city view holds far more geometry than a 512 KB-RAM device can hold at once, the panel is only 240×320, and every millisecond and every byte of RAM is spoken for. The renderer ([`obc-render`](src:firmware/obc-render)) is the machinery that turns a map far larger than memory into one frame, **without allocating a single byte on the heap**, and it runs *byte-for-byte identically* on the desktop simulator and on the device.

This page walks one frame from map bytes to lit pixels. It's the deepest corner of the project — and, I think, the most interesting.

## One render path, two surfaces

The whole renderer is a single `no_std`, zero-allocation crate. The only things that differ between the desktop and the device are **where the pixels go** and **what colour they end up** — and both are injected as parameters, so the drawing code never knows which machine it's on.

```rust
MapRenderer::render(target, scene, vp, bg, color_fn)
//                  │       │       │   │   └ RGB565 → this panel's pixel
//                  │       │       │   └ the backdrop colour
//                  │       │       └ the camera (Viewport)
//                  │       └ a streamed map scene
//                  └ where pixels land (a DrawTarget)
```

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="The shared render code in the middle connects through two pluggable seams — a DrawTarget for pixels and a colour function — to two hosts: the simulator and the device.">
  <defs>
    <marker id="aS" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Everything device-specific lives at two seams</text>

  <!-- shared core -->
  <rect class="d-hot" x="244" y="116" width="232" height="74" rx="13" style="fill:#f8efe4" />
  <text class="d-title" x="360" y="148" text-anchor="middle" style="fill:#a9501c">obc-render</text>
  <text class="d-sub" x="360" y="168" text-anchor="middle">one no_std code path · zero-alloc</text>

  <!-- inputs left -->
  <text class="d-sub" x="232" y="100" text-anchor="end">scene · viewport · bg →</text>

  <!-- seam 1: DrawTarget -->
  <rect class="d-panel-2" x="300" y="36" width="120" height="40" rx="9" />
  <text class="d-label" x="360" y="54" text-anchor="middle">DrawTarget</text>
  <text class="d-sub" x="360" y="68" text-anchor="middle">where pixels go</text>
  <line class="d-stroke" x1="360" y1="116" x2="360" y2="78" />

  <!-- seam 2: color_fn -->
  <rect class="d-panel-2" x="300" y="228" width="120" height="40" rx="9" />
  <text class="d-label" x="360" y="246" text-anchor="middle">color_fn</text>
  <text class="d-sub" x="360" y="260" text-anchor="middle">RGB565 → pixel</text>
  <line class="d-stroke" x1="360" y1="190" x2="360" y2="226" />

  <!-- host: simulator -->
  <rect class="d-panel" x="40" y="120" width="150" height="66" rx="11" />
  <text class="d-label" x="115" y="146" text-anchor="middle">obc-sim</text>
  <text class="d-sub" x="115" y="164" text-anchor="middle">RGB888 framebuffer</text>
  <text class="d-sub" x="115" y="178" text-anchor="middle">true colour</text>
  <line class="d-flow" x1="244" y1="153" x2="192" y2="153" marker-end="url(#aS)" />

  <!-- host: device -->
  <rect class="d-panel" x="530" y="120" width="150" height="66" rx="11" />
  <text class="d-label" x="605" y="146" text-anchor="middle">device</text>
  <text class="d-sub" x="605" y="164" text-anchor="middle">LS021B7DD02 panel</text>
  <text class="d-sub" x="605" y="178" text-anchor="middle">64 colours (RGB222)</text>
  <line class="d-flow" x1="476" y1="153" x2="528" y2="153" marker-end="url(#aS)" />
</svg>
<figcaption>The renderer is generic over a streamed <b>map scene</b> as well as a <b>DrawTarget</b> (the pixel sink), and takes a <b>colour function</b> (RGB565 → the panel's native pixel). The normal scene is the OBCM reader's thin adapter; tests can supply static geometry. The concrete source and pixel target are monomorphised, so identical geometry code runs between hosts without per-feature dispatch.</figcaption>
</figure>

The base-map input is the allocation-free [`obc-map-scene`](src:firmware/obc-map-scene/src/lib.rs) seam: LOD and style metadata, a visible-candidate visit, complete selected-feature decode into caller-owned scratch, and optional source counters. It exposes no OBCM offsets, quadtree records, cache slots, or retained scene graph. The production [`Reader` adapter](src:firmware/obc-reader/src/scene.rs) keeps streaming the same chunks through the same cache; its opaque six-byte candidate token replaces the same six bytes the renderer's stub formerly devoted to chunk and offset, so neither the 14-byte slot nor resident RAM grows.

Styles in the map store **device-independent RGB565**; the host's `color_fn` resolves each to a concrete pixel — true colour in the simulator, [64-colour RGB222 quantisation](src:firmware/obc-reader/src/color.rs) on the device. Because of these seams, the simulator you can [run in your browser](../../) is not a mock-up: it is the device's exact rendering code, so the two can never drift apart.

## The frame, end to end

Here's the whole journey before we slow down for each step. A frame is a short trail with a handful of waypoints — most of them cheap, two of them where the real work happens.

<figure class="fig">
<svg viewBox="0 0 820 250" role="img" aria-label="A frame's pipeline as a trail with seven waypoints: project, pick level of detail, quadtree cull, priority decode, painter sort, rasterise, overlays — from map bytes to the panel.">
  <text class="d-tag" x="20" y="24">One frame, start to finish</text>

  <!-- trail -->
  <line x1="92" y1="120" x2="734" y2="120" stroke="#5f7d3d" stroke-width="2.5" stroke-dasharray="2 7" stroke-linecap="round" />

  <!-- start flag -->
  <circle cx="58" cy="120" r="7" class="d-forest" />
  <text class="d-sub" x="58" y="150" text-anchor="middle">map +</text>
  <text class="d-sub" x="58" y="162" text-anchor="middle">route bytes</text>

  <!-- waypoints: x = 110,214,318,422,526,630,734 -->
  <!-- 1 project (above) -->
  <circle cx="110" cy="120" r="15" class="d-forest" /><text class="d-num" x="110" y="124" text-anchor="middle">1</text>
  <text class="d-label" x="110" y="74" text-anchor="middle">Project</text>
  <text class="d-sub" x="110" y="88" text-anchor="middle">camera → px</text>
  <!-- 2 LOD (below) -->
  <circle cx="214" cy="120" r="15" class="d-forest" /><text class="d-num" x="214" y="124" text-anchor="middle">2</text>
  <text class="d-label" x="214" y="160" text-anchor="middle">Pick LOD</text>
  <text class="d-sub" x="214" y="174" text-anchor="middle">for this zoom</text>
  <!-- 3 cull (above) -->
  <circle cx="318" cy="120" r="15" class="d-forest" /><text class="d-num" x="318" y="124" text-anchor="middle">3</text>
  <text class="d-label" x="318" y="74" text-anchor="middle">Quadtree cull</text>
  <text class="d-sub" x="318" y="88" text-anchor="middle">visible chunks</text>
  <!-- 4 priority (below, HOT) -->
  <circle cx="422" cy="120" r="16" class="d-hot-fill" /><text class="d-num" x="422" y="124" text-anchor="middle">4</text>
  <text class="d-label" x="422" y="160" text-anchor="middle" style="fill:#a9501c">Priority decode</text>
  <text class="d-sub" x="422" y="174" text-anchor="middle">fill the buffers</text>
  <!-- 5 sort (above) -->
  <circle cx="526" cy="120" r="15" class="d-forest" /><text class="d-num" x="526" y="124" text-anchor="middle">5</text>
  <text class="d-label" x="526" y="74" text-anchor="middle">Painter sort</text>
  <text class="d-sub" x="526" y="88" text-anchor="middle">by z-index</text>
  <!-- 6 rasterise (below, HOT) -->
  <circle cx="630" cy="120" r="16" class="d-hot-fill" /><text class="d-num" x="630" y="124" text-anchor="middle">6</text>
  <text class="d-label" x="630" y="160" text-anchor="middle" style="fill:#a9501c">Rasterise</text>
  <text class="d-sub" x="630" y="174" text-anchor="middle">fill + stroke</text>
  <!-- 7 overlays (above) -->
  <circle cx="734" cy="120" r="15" class="d-forest" /><text class="d-num" x="734" y="124" text-anchor="middle">7</text>
  <text class="d-label" x="734" y="74" text-anchor="middle">Overlays</text>
  <text class="d-sub" x="734" y="88" text-anchor="middle">route · you</text>

  <!-- end panel -->
  <rect class="d-panel" x="770" y="104" width="36" height="32" rx="5" style="fill:#e7ead8" />
  <text class="d-sub" x="788" y="156" text-anchor="middle">panel</text>
</svg>
<figcaption>Stages <b>1–3 and 5</b> only touch lightweight index data — they're cheap. The two coral waypoints, <b>priority decode</b> and <b>rasterise</b>, are where the time goes, so the rest of this page spends most of its words there.</figcaption>
</figure>

The orange waypoints matter for a reason worth stating up front: the cheap stages (walking the tree, scanning headers) are allowed to run more than once per frame, because re-running them is nearly free. The expensive work — decoding a feature's coordinates, and rasterising pixels — happens **at most once per feature per frame**. Keep that asymmetry in mind; it explains several design choices below.

## 1 · Projection: ground to screen

The camera is a [`Viewport`](src:firmware/obc-render/src/viewport.rs) — a centre point in microdegrees, a zoom, an aspect correction for the latitude, and a rotation. Its hot path is `to_screen`, called once per vertex, so it's written to keep full precision while staying fast:

```rust
let delta_lon = lon.wrapping_sub(self.cam_lon);   // i32 µdeg, relative to camera
let delta_lat = lat.wrapping_sub(self.cam_lat);
let ex = (delta_lon as f32) * self.aspect;        // squash longitude by cos(lat)
let ny =  delta_lat as f32;
let rx = self.cos_c * ex - self.sin_c * ny;       // rotate to heading-up
let ry = -self.sin_c * ex - self.cos_c * ny;
let x = rx * self.zoom + self.w / 2.0;            // scale, centre
let y = ry * self.zoom + self.h / 2.0;
(round_coord(x), round_coord(y))                  // round to nearest — a branch + add,
                                                  // no soft-float roundf on the hot path
```

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="Ground coordinates in microdegrees, relative to the camera, are squashed by cosine of latitude, rotated to heading-up, scaled by zoom and centred, then rounded to the nearest pixel.">
  <defs>
    <marker id="aP" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>

  <!-- ground panel -->
  <rect class="d-panel-2" x="24" y="44" width="300" height="216" rx="12" />
  <text class="d-tag" x="40" y="66">Ground · µdeg</text>
  <!-- graticule -->
  <g stroke="#33575b" stroke-opacity="0.25" stroke-width="1">
    <line x1="60" y1="90" x2="288" y2="90" /><line x1="60" y1="150" x2="288" y2="150" /><line x1="60" y1="210" x2="288" y2="210" />
    <line x1="110" y1="80" x2="110" y2="240" /><line x1="174" y1="80" x2="174" y2="240" /><line x1="238" y1="80" x2="238" y2="240" />
  </g>
  <!-- camera -->
  <circle cx="174" cy="150" r="5" class="d-amber" /><text class="d-sub" x="174" y="138" text-anchor="middle">camera</text>
  <!-- point P + delta -->
  <line x1="174" y1="150" x2="252" y2="106" stroke="#cf6a2a" stroke-width="1.6" stroke-dasharray="3 3" />
  <circle cx="252" cy="106" r="5" class="d-hot-fill" /><text class="d-label" x="262" y="104">P</text>
  <text class="d-sub" x="208" y="120" style="fill:#a9501c">Δ (lon,lat)</text>

  <!-- arrow to screen -->
  <line class="d-flow" x1="332" y1="150" x2="392" y2="150" marker-end="url(#aP)" />

  <!-- screen panel -->
  <rect class="d-panel" x="400" y="44" width="200" height="216" rx="12" style="fill:#e7ead8" />
  <text class="d-tag" x="416" y="66">Screen · px</text>
  <!-- north arrow (heading-up: rotated) -->
  <line x1="500" y1="150" x2="478" y2="92" stroke="#4d5b3c" stroke-width="1.6" marker-end="url(#aP)" />
  <text class="d-sub" x="470" y="86">N</text>
  <!-- projected P -->
  <circle cx="540" cy="118" r="5" class="d-hot-fill" /><text class="d-label" x="550" y="116">P</text>
  <circle cx="500" cy="150" r="3" class="d-amber" />

  <!-- steps strip -->
  <text class="d-sub" x="624" y="120" style="font-size:11px">① Δ vs camera</text>
  <text class="d-sub" x="624" y="142" style="font-size:11px">② × cos(lat)</text>
  <text class="d-sub" x="624" y="164" style="font-size:11px">③ rotate</text>
  <text class="d-sub" x="624" y="186" style="font-size:11px">④ × zoom</text>
  <text class="d-sub" x="624" y="208" style="font-size:11px">⑤ round</text>
</svg>
<figcaption>Only the small <b>delta</b> from the camera is cast to <code>f32</code>, so absolute microdegree precision is preserved no matter where on Earth you are. Longitude is squashed by <code>cos(latitude)</code> so things stay the right shape, then the whole view is rotated for heading-up navigation.</figcaption>
</figure>

Two small decisions there carry weight. Keeping the **delta** in integer microdegrees until the last moment means precision doesn't degrade far from the origin. And **rounding to nearest** (rather than truncating) is symmetric about the camera — truncation biases toward the screen centre, which, combined with how chunks are clipped, used to crack open hairline gaps at chunk seams under rotation. The inverse, `to_map`, reuses the very same coefficients because the screen↔ground rotation is its own inverse, so panning and the visible-area computation need no extra trigonometry.

## 2 · Level of detail: pick the right layer

A map at riding zoom and the same map zoomed out to the whole region are not the same data drawn smaller — they're **different, pre-simplified layers**. An OBCM map is a pyramid of level-of-detail (LOD) tiers, each with its own simplified geometry and a maximum meters-per-pixel it's good for. Zooming out reads a small coarse layer instead of decimating fine geometry on the fly.

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="A level-of-detail pyramid: coarse tiers at the top are narrow (little, simplified geometry) and cover any zoom; fine tiers at the bottom are wide (dense detail) with a small meters-per-pixel range. A current view of 0.5 meters per pixel selects LOD 3, the finest tier whose range still covers it.">
  <defs>
    <marker id="aL" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">LOD pyramid — coarse to fine</text>

  <!-- detail axis (left) -->
  <text class="d-sub" x="58" y="86" text-anchor="middle">coarse</text>
  <text class="d-sub" x="58" y="230" text-anchor="middle">fine</text>

  <!-- tier 0 (top, narrowest) -->
  <polygon points="300,58 420,58 438,98 282,98" style="fill:#cfe0c2;stroke:#3c6b39;stroke-width:1.4" />
  <text class="d-label" x="360" y="76" text-anchor="middle" style="font-size:11px">LOD 0 · coarsest</text>
  <text class="d-sub" x="360" y="90" text-anchor="middle">covers any zoom</text>

  <!-- tier 1 -->
  <polygon points="282,102 438,102 458,142 262,142" style="fill:#c3dab4;stroke:#3c6b39;stroke-width:1.4" />
  <text class="d-label" x="360" y="120" text-anchor="middle" style="font-size:11px">LOD 1</text>
  <text class="d-sub" x="360" y="134" text-anchor="middle">good to ≤ 16 m/px</text>

  <!-- tier 2 -->
  <polygon points="262,146 458,146 480,186 240,186" style="fill:#b7d4a6;stroke:#3c6b39;stroke-width:1.4" />
  <text class="d-label" x="360" y="164" text-anchor="middle" style="font-size:11px">LOD 2</text>
  <text class="d-sub" x="360" y="178" text-anchor="middle">good to ≤ 4 m/px</text>

  <!-- tier 3 (bottom, widest) — the selected tier -->
  <polygon points="240,190 480,190 506,234 214,234" style="fill:#a9cd96;stroke:#cf6a2a;stroke-width:2.6" />
  <text class="d-label" x="360" y="208" text-anchor="middle" style="font-size:11px">LOD 3 · finest</text>
  <text class="d-sub" x="360" y="222" text-anchor="middle">good to ≤ 1 m/px</text>

  <!-- selector -->
  <line x1="646" y1="212" x2="500" y2="212" stroke="#cf6a2a" stroke-width="2" marker-end="url(#aL)" />
  <text class="d-label" x="652" y="206" style="fill:#a9501c">this view</text>
  <text class="d-sub" x="652" y="222">0.5 m/px</text>
</svg>
<figcaption>The renderer turns zoom into meters-per-pixel — <i>independent of screen size</i>, so a desktop window and the 240 px panel showing the same ground span pick the same tier — then scans for the <b>finest</b> LOD whose range still covers it. Here 0.5 m/px lands on LOD 3; the coarsest tier covers any zoom, so a choice always exists.</figcaption>
</figure>

The selection itself is a short scan — keep the finest tier whose range still covers the current resolution:

```rust
pub fn select_lod_for_mpp(&self, mpp: f32) -> usize {
    let mut chosen = 0;
    for (i, lod) in self.lods.iter().enumerate() {
        if lod.max_mpp >= mpp {
            chosen = i; // a finer tier still covers this zoom — prefer it
        }
    }
    chosen
}
```

Because `meters_per_pixel` depends only on zoom and latitude, not on the display, the device and the simulator agree on the LOD for any given view.

## 3 · The quadtree cull: only the chunks you can see

Within an OBCM source's chosen LOD, geometry is bucketed into fixed-size **chunks**, indexed by a **quadtree** over the map's bounding box. The reader adapter walks that tree from the root, descending only into children whose box intersects the view, and streams every non-empty leaf's candidates through the scene seam. The renderer sees candidates, not the tree or chunks that produced them.

<figure class="fig">
<svg viewBox="0 0 720 320" role="img" aria-label="A region split into four quadrants. The viewport straddles the boundary between the north-east and south-east quadrants, so the walk descends into both and prunes the north-west and south-west quadrants whole. Within north-east only the two lower sub-cells meet the view; within south-east only the two upper sub-cells do — four visited leaves in all. The tree on the right mirrors this: the root descends into the NE and SE branches, each with two visited and two pruned leaves, while NW and SW are pruned.">
  <text class="d-tag" x="20" y="24">Descend only where the view reaches</text>

  <!-- SPATIAL (left) -->
  <!-- pruned whole quadrants (NW, SW) -->
  <rect x="40" y="52" width="128" height="128" fill="#d6cda8" fill-opacity="0.45" />
  <rect x="40" y="180" width="128" height="128" fill="#d6cda8" fill-opacity="0.45" />
  <!-- pruned sub-cells: NE top row + SE bottom row -->
  <rect x="168" y="52" width="64" height="64" fill="#d6cda8" fill-opacity="0.45" />
  <rect x="232" y="52" width="64" height="64" fill="#d6cda8" fill-opacity="0.45" />
  <rect x="168" y="244" width="64" height="64" fill="#d6cda8" fill-opacity="0.45" />
  <rect x="232" y="244" width="64" height="64" fill="#d6cda8" fill-opacity="0.45" />
  <!-- visited sub-cells: NE lower row + SE upper row (the view straddles their shared edge) -->
  <rect x="168" y="116" width="64" height="64" fill="#cf6a2a" fill-opacity="0.20" />
  <rect x="232" y="116" width="64" height="64" fill="#cf6a2a" fill-opacity="0.20" />
  <rect x="168" y="180" width="64" height="64" fill="#cf6a2a" fill-opacity="0.20" />
  <rect x="232" y="180" width="64" height="64" fill="#cf6a2a" fill-opacity="0.20" />
  <!-- region outline + root split -->
  <rect x="40" y="52" width="256" height="256" fill="none" stroke="#3c6b39" stroke-width="1.6" />
  <line x1="168" y1="52" x2="168" y2="308" stroke="#3c6b39" stroke-width="1.4" />
  <line x1="40" y1="180" x2="296" y2="180" stroke="#3c6b39" stroke-width="1.4" />
  <!-- NE + SE subdivisions -->
  <line x1="232" y1="52" x2="232" y2="180" stroke="#7c9a63" stroke-width="1" />
  <line x1="168" y1="116" x2="296" y2="116" stroke="#7c9a63" stroke-width="1" />
  <line x1="232" y1="180" x2="232" y2="308" stroke="#7c9a63" stroke-width="1" />
  <line x1="168" y1="244" x2="296" y2="244" stroke="#7c9a63" stroke-width="1" />
  <!-- quadrant labels -->
  <text class="d-sub" x="104" y="116" text-anchor="middle">NW</text>
  <text class="d-sub" x="104" y="130" text-anchor="middle" style="font-size:9px">skipped</text>
  <text class="d-sub" x="104" y="244" text-anchor="middle">SW</text>
  <text class="d-sub" x="104" y="258" text-anchor="middle" style="font-size:9px">skipped</text>
  <text class="d-sub" x="290" y="64" text-anchor="end" style="font-size:9px">NE</text>
  <text class="d-sub" x="290" y="302" text-anchor="end" style="font-size:9px">SE</text>
  <!-- viewport (straddles the NE/SE boundary) -->
  <rect x="185" y="120" width="95" height="120" fill="none" stroke="#cf6a2a" stroke-width="2.4" />
  <text class="d-label" x="206" y="137" text-anchor="middle" style="fill:#a9501c">view</text>

  <!-- TREE (right) -->
  <text class="d-sub" x="545" y="42" text-anchor="middle" style="font-size:9px">root</text>
  <circle cx="545" cy="56" r="11" class="d-forest" />
  <!-- edges root -> children -->
  <g stroke="#9aa884" stroke-width="1.4" fill="none">
    <line x1="545" y1="61" x2="432" y2="101" />
    <line x1="545" y1="61" x2="585" y2="101" />
  </g>
  <g stroke="#cf6a2a" stroke-width="2" fill="none">
    <line x1="545" y1="61" x2="505" y2="101" />
    <line x1="545" y1="61" x2="658" y2="101" />
  </g>
  <!-- level-1 children: NW NE SW SE -->
  <circle cx="432" cy="110" r="9" class="d-muted" />
  <circle cx="505" cy="110" r="10" class="d-hot-fill" />
  <circle cx="585" cy="110" r="9" class="d-muted" />
  <circle cx="658" cy="110" r="10" class="d-hot-fill" />
  <text class="d-sub" x="432" y="132" text-anchor="middle" style="font-size:9px">NW</text>
  <text class="d-sub" x="505" y="132" text-anchor="middle" style="font-size:9px">NE</text>
  <text class="d-sub" x="585" y="132" text-anchor="middle" style="font-size:9px">SW</text>
  <text class="d-sub" x="658" y="132" text-anchor="middle" style="font-size:9px">SE</text>
  <!-- edges branch -> leaves -->
  <g stroke="#cf6a2a" stroke-width="1.6" fill="none">
    <line x1="505" y1="120" x2="485" y2="153" /><line x1="505" y1="120" x2="503" y2="153" /><line x1="505" y1="120" x2="521" y2="153" /><line x1="505" y1="120" x2="539" y2="153" />
    <line x1="658" y1="120" x2="638" y2="153" /><line x1="658" y1="120" x2="656" y2="153" /><line x1="658" y1="120" x2="674" y2="153" /><line x1="658" y1="120" x2="692" y2="153" />
  </g>
  <!-- NE leaves: skip · skip · visit · visit (lower row meets the view) -->
  <rect x="478" y="153" width="14" height="14" rx="3" class="d-muted" />
  <rect x="496" y="153" width="14" height="14" rx="3" class="d-muted" />
  <rect x="514" y="153" width="14" height="14" rx="3" class="d-hot-fill" />
  <rect x="532" y="153" width="14" height="14" rx="3" class="d-hot-fill" />
  <!-- SE leaves: visit · visit · skip · skip (upper row meets the view) -->
  <rect x="631" y="153" width="14" height="14" rx="3" class="d-hot-fill" />
  <rect x="649" y="153" width="14" height="14" rx="3" class="d-hot-fill" />
  <rect x="667" y="153" width="14" height="14" rx="3" class="d-muted" />
  <rect x="685" y="153" width="14" height="14" rx="3" class="d-muted" />
  <!-- legend -->
  <rect x="430" y="214" width="13" height="13" rx="3" class="d-hot-fill" />
  <text class="d-sub" x="450" y="224" style="font-size:10px">visited leaf → a chunk to draw</text>
  <rect x="430" y="236" width="13" height="13" rx="3" class="d-muted" />
  <text class="d-sub" x="450" y="246" style="font-size:10px">skipped — bbox misses the view</text>
  <text class="d-sub" x="430" y="278" style="font-size:10px">a high bit marks a branch;</text>
  <text class="d-sub" x="430" y="292" style="font-size:10px">a sentinel marks an empty leaf</text>
</svg>
<figcaption>Children split <b>NW · NE · SW · SE</b> at floor-division midpoints — identical math at every level. Here the view <b>straddles the NE/SE boundary</b>, so the walk descends into both — and within each it visits only the sub-cells the view actually touches, pruning the other two along with the whole NW and SW quadrants. The walk reads only <code>u32</code> index nodes (no geometry), so it's cheap to re-run, and it's <b>uncapped</b> — a wide view can overlap hundreds of leaves and every one is visited.</figcaption>
</figure>

The walk is a recursive descent that prunes whole subtrees by bounding box and reads the index as raw bits — a high bit flags a branch, a sentinel marks an empty leaf ([`walk_leaves`](src:firmware/obc-reader/src/reader.rs)):

```rust
if idx >= lod.node_count || depth > MAX_DEPTH || !node.intersects(view) {
    return Ok(());                   // prune: out of range, too deep, or off-screen
}
let val = self.read_node(lod, idx)?; // medium/cache failures stay distinct
if val & BRANCH_BIT == 0 {
    if val != EMPTY_LEAF {
        visit(val, node);            // a non-empty leaf = a chunk to decode
    }
    return Ok(());
}
// branch: child must advance (child > idx) — reject a corrupt back-reference —
// then split `node`'s bbox into NW/NE/SW/SE and recurse into each child
```

The `depth > MAX_DEPTH` bound and the `child > idx` check are pure robustness. A well-formed tree is only ~30 levels deep and always stores a branch's children *after* it, so neither ever fires on a real map — but a truncated or hostile `.obcm` off the SD card could otherwise point a branch back at itself and drive the walk into unbounded recursion. On the MCU there's no MMU guard page, so that's a stack overflow straight into a HardFault; bounding the depth makes the walk safe on any bytes. (This caps recursion *depth*, not the number of chunks visited — a different axis from the next paragraph.)

That "uncapped" property is load-bearing. An earlier version capped the visited chunks at a fixed number; a wide zoomed-out view overlaps far more leaves than the cap, so it silently dropped half the map *before* any importance logic could weigh in. Streaming the leaves through a callback instead means the decision about *what to drop* belongs entirely to the next stage — where it can be made by priority, not by accident.

**A second, screen-space reject.** The quadtree test is a *map-space* one: it compares axis-aligned boxes in microdegrees. But heading-up, the on-screen rectangle is rotated, and the axis-aligned box that must enclose it (the [`Viewport::visible_bbox`](src:firmware/obc-render/src/viewport.rs) the walk descends against) has large empty corners the panel never shows — at a 35° heading those corners can be a third of the admitted features. So after a candidate's own bbox clears the map-space test, the renderer applies a second, cheaper reject in *screen* space: [`Viewport::bbox_may_touch_screen`](src:firmware/obc-render/src/viewport.rs) projects the candidate's four bbox corners through the very same `to_screen` the draw path uses, takes the integer bounding box of those projected corners, and drops the feature when that box lies wholly outside the display rectangle. Because the per-frame projection is affine, the true projected bbox is a parallelogram contained by its corner box — so a corner-box miss is a *safe* reject, never a false one. It is a conservative broad phase, not clipping: the rectangle is grown by a fixed ink margin (the widest stroke plus casing a feature can paint past its centreline, ~16 px) so a road whose centreline sits just off the edge but whose ink reaches the glass is still kept. The map-space test runs first because it is cheaper; only survivors pay the projection. The same reject guards the active route's chunks in both overlay passes. A feature dropped here skips its second decode, point and ring retention, painter sort, projection, and rasterisation — the whole tail of the pipeline.

## 4 · Decode by priority — the clever bit

Here is the central problem. A dense view holds far more geometry than the fixed frame buffers can hold. When they fill up, *something* must be dropped — and the dropped things must be the **least important features, globally**, no matter which chunk they live in. You never want to drop the coastline or a motorway because an unimportant forest patch in an early chunk got there first.

Two mechanisms work together to solve this within the memory and time budget.

**Priority, and cheap skipping.** Each feature's style carries a 2-bit **priority** (1 = keep first … 4 = drop first) — the axis the drop decision turns on. And a feature is cheap to *step over*: its header is a fixed 12 bytes, so the reader can advance past a feature it doesn't want with pure offset arithmetic — no coordinate math, no buffer writes. That skip primitive is what lets the collector touch a chunk's bytes selectively: past features whose style isn't drawn at all, and — the payoff below — straight to the handful of *winners* it must re-decode.

<figure class="fig">
<svg viewBox="0 0 720 168" role="img" aria-label="A chunk's byte stream is a row of feature cells. The OBCM adapter resolves each winning opaque token to its source position and seeks straight to that feature, skipping everything in between by advancing the read pointer.">
  <text class="d-tag" x="20" y="24">Pass B — the source resolves each opaque winner token</text>
  <!-- byte stream cells -->
  <g font-family="var(--mono)">
    <!-- cell template: x width 78, y 52 h 46 -->
    <rect x="24"  y="56" width="78" height="46" rx="6" class="d-hot-fill" /><text class="d-num" x="63" y="84" text-anchor="middle">win</text>
    <rect x="110" y="56" width="78" height="46" rx="6" class="d-muted" /><text class="d-sub" x="149" y="84" text-anchor="middle">skip</text>
    <rect x="196" y="56" width="78" height="46" rx="6" class="d-muted" /><text class="d-sub" x="235" y="84" text-anchor="middle">skip</text>
    <rect x="282" y="56" width="78" height="46" rx="6" class="d-hot-fill" /><text class="d-num" x="321" y="84" text-anchor="middle">win</text>
    <rect x="368" y="56" width="78" height="46" rx="6" class="d-muted" /><text class="d-sub" x="407" y="84" text-anchor="middle">skip</text>
    <rect x="454" y="56" width="78" height="46" rx="6" class="d-muted" /><text class="d-sub" x="493" y="84" text-anchor="middle">skip</text>
    <rect x="540" y="56" width="78" height="46" rx="6" class="d-hot-fill" /><text class="d-num" x="579" y="84" text-anchor="middle">win</text>
    <rect x="626" y="56" width="70" height="46" rx="6" class="d-muted" /><text class="d-sub" x="661" y="84" text-anchor="middle">end</text>
  </g>
  <!-- read head -->
  <text class="d-sub" x="24" y="126">read head jumps offset → offset →</text>
  <line class="d-stroke" x1="24" y1="118" x2="696" y2="118" style="stroke:#cf6a2a;stroke-dasharray:2 5" />
  <text class="d-sub" x="24" y="150" style="font-size:11px">re-decoded winners (coral) cost coordinate math; the features between them cost only a pointer add.</text>
</svg>
<figcaption>A feature header is 12 bytes, so skipping is pure offset arithmetic inside the OBCM adapter. Pass A saves an <b>opaque source token</b> in each winner's stub; pass B gives it back to the source, which seeks straight to the feature — re-decoding only survivors without exposing byte offsets to the renderer.</figcaption>
</figure>

**Stub-select.** The global-priority drop is easy to state and hard to do cheaply, because the device streams chunks off the SD card through a cache that holds **one** at a time. An earlier design made four passes over the visible chunks — one per priority level — filling the buffers level by level. That kept the guarantee, but it re-read every visible chunk *four times*; the one-slot cache absorbed none of it, so a wide view cost `4 × N` chunk reads off SPI SD and the frame crawled. The fix (issue #564) splits **selection** from **geometry**:

```rust
let candidates = self.collect_stubs(scene, lod, view, &vis_mask, stats); // pass A
let winners    = self.select(scene);                                     // RAM only
self.decode_winners(scene, lod, view, winners, stats);                   // pass B
```

- **Pass A** asks the scene source to visit visible candidates once, decoding every drawn feature just far enough to get its bounding box, and records a fixed-size *stub* — style, opaque token, vertex/ring counts — but keeps **no geometry**. When the stub buffer fills, the lowest-priority stub is evicted, so it always holds the best candidates.
- **Select** is pure RAM: sort the stubs by priority and admit them greedily against the exact point/span budget. Drops are strictly lowest-priority-first, *globally* — the same guarantee as before, now with the exact vertex cost of every candidate known before a single coordinate is copied.
- **Pass B** returns the winning tokens to the scene source, which preserves its natural chunk-major walk and re-decodes only those **winners** directly into caller-owned scratch. In the OBCM adapter, only chunks that own a winner are re-read.

There are two deliberately separate kinds of degradation here. A **frame-budget drop** happens only after a complete feature produced a stub; it follows the deterministic priority policy above and increments `features_dropped`. A **decode failure** never produces partial geometry: an over-capacity feature is consumed and dropped whole, malformed feature bytes are rejected, structural map/index corruption is distinguished from them, and medium/cache failures remain typed. The renderer counts these separately as capacity drops, malformed features, structural-map failures, read failures, and cache contentions. Pass A simply omits a failed feature's stub and continues when its byte extent is known. If a winner refetch or the second index walk fails in pass B, the collector rolls back any point/ring prefix and compacts only successfully decoded spans; failed placeholders and untouched stubs never reach the painter.

A feature that survives is decoded twice (cheap, in RAM); a chunk is fetched at most twice instead of four times, so the SD traffic that dominates a wide frame roughly halves. The stubs live in the same buffer the spans end up in — a stub is sized to fit a span slot — so the split costs no extra RAM.

<figure class="fig">
<svg viewBox="0 0 720 330" role="img" aria-label="Four priority lanes feed a fixed frame buffer in order. Priority 1, 2 and 3 fit; the buffer saturates partway through priority 4, so the remaining priority-4 features are dropped.">
  <defs>
    <marker id="aB" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Lowest priority is dropped — globally, by construction</text>

  <!-- lanes (labels kept clear of the bars) -->
  <g>
    <text class="d-label" x="18" y="70">P1</text><text class="d-sub" x="18" y="84">sea·land·motorway</text>
    <rect x="150" y="60" width="120" height="26" rx="6" class="d-water" />
    <text class="d-label" x="18" y="130">P2</text><text class="d-sub" x="18" y="144">major roads</text>
    <rect x="150" y="120" width="120" height="26" rx="6" class="d-forest" />
    <text class="d-label" x="18" y="190">P3</text><text class="d-sub" x="18" y="204">minor roads</text>
    <rect x="150" y="180" width="120" height="26" rx="6" style="fill:#b5763e" />
    <text class="d-label" x="18" y="250">P4</text><text class="d-sub" x="18" y="264">buildings·detail</text>
    <rect x="150" y="240" width="120" height="26" rx="6" class="d-muted" />
  </g>
  <line class="d-flow" x1="278" y1="163" x2="314" y2="163" marker-end="url(#aB)" />

  <!-- buffer tank -->
  <rect x="320" y="48" width="150" height="234" rx="10" style="fill:#f3f0df;stroke:#3c6b39;stroke-width:1.8" />
  <text class="d-tag" x="360" y="42">frame buffer (fixed)</text>
  <!-- filled portion -->
  <rect x="324" y="160" width="142" height="118" rx="6" class="d-water" style="fill-opacity:0.85" />
  <rect x="324" y="120" width="142" height="40" class="d-forest" style="fill-opacity:0.85" />
  <rect x="324" y="86"  width="142" height="34" style="fill:#b5763e;fill-opacity:0.85" />
  <text class="d-num" x="395" y="220" text-anchor="middle" style="fill:#fff">P1</text>
  <text class="d-num" x="395" y="142" text-anchor="middle" style="fill:#fff">P2</text>
  <text class="d-num" x="395" y="106" text-anchor="middle" style="fill:#fff">P3</text>
  <!-- saturation line -->
  <line x1="316" y1="86" x2="474" y2="86" stroke="#c0492e" stroke-width="2" stroke-dasharray="5 4" />
  <text class="d-sub" x="478" y="90" style="fill:#c0492e">FULL</text>

  <!-- dropped -->
  <g opacity="0.5">
    <rect x="540" y="120" width="150" height="26" rx="6" class="d-muted" />
    <rect x="540" y="156" width="120" height="26" rx="6" class="d-muted" />
    <rect x="540" y="192" width="140" height="26" rx="6" class="d-muted" />
  </g>
  <text class="d-label" x="540" y="112" style="fill:#8a8366">P4 — dropped</text>
  <line x1="474" y1="86" x2="536" y2="150" stroke="#c0492e" stroke-width="1.4" stroke-dasharray="3 3" />
  <text class="d-sub" x="540" y="240" style="font-size:11px">the buffer filled before</text>
  <text class="d-sub" x="540" y="256" style="font-size:11px">priority 4 fit — exactly</text>
  <text class="d-sub" x="540" y="272" style="font-size:11px">the right things to lose</text>
</svg>
<figcaption>Select admits stubs priority-1 first, so when the budget saturates the undrawn remainder is strictly the lowest priority, across <i>all</i> chunks. Only the survivors' geometry is ever built — a dropped feature cost only its stub, never a copied vertex.</figcaption>
</figure>

Every kept feature becomes a 14-byte **span** — a compact draw record that says *what* and *where* without copying the geometry again ([`Span`](src:firmware/obc-render/src/collect.rs)):

```rust
struct Span {
    kind: Kind,      // polygon or line
    z: i8,           // paint order
    weight: u8,      // nominal line width — ramped by zoom at draw (see §6)
    style_id: u8,    // → the full Style (dashed / color2) at draw time (a spare byte)
    color: u16,      // RGB565
    pt_start: u16,   // where its points sit in the frame buffer
    ring_start: u16,
    ring_count: u16,
    seq: u16,        // collection order — the stable-sort tiebreak
}
```

Thousands of spans can be buffered at coarse zoom, so they're kept small (`u16` offsets, not `usize`). The two chunk walks cost little on their own — the leaf walk reads only the cheap index — and the expensive part, copying vertices into the frame buffers, happens once, for the winners alone.

## 5 · Painter's order

With the visible features collected, the renderer sorts the spans — not the geometry, just the little records — into back-to-front draw order:

```rust
self.frame.spans_mut().sort_unstable_by_key(|s| (s.z, s.seq));
```

Each style carries a `z_index`; sea draws under land draws under forest draws under roads. Ties break on `seq`, the order the feature was collected in — a stable, allocation-free tiebreak so the result is deterministic. Note that **priority and z-index are different axes**: priority decides *whether* a feature survives the memory budget; z-index decides *where in the stack* the survivors are painted.

## 6 · Rasterising: fills and strokes

The draw loop walks the sorted spans and dispatches on kind — polygons fill, lines stroke.

### Polygons: even-odd scanline fill

Each polygon is filled with a classic scanline algorithm. For every screen row the polygon covers, the renderer collects where the row crosses each edge, sorts those crossings, and fills between consecutive pairs.

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="A polygon with a square hole on a pixel grid. A horizontal scanline crosses the outer edge twice and the hole twice; the fill covers the two outer spans and leaves the hole empty, because the even-odd rule pairs the four crossings as two filled spans.">
  <text class="d-tag" x="20" y="24">Even-odd rule — holes fall out for free</text>
  <!-- faint pixel grid -->
  <g stroke="#9aa884" stroke-opacity="0.25" stroke-width="1">
    <line x1="60" y1="56" x2="60" y2="256"/><line x1="100" y1="56" x2="100" y2="256"/><line x1="140" y1="56" x2="140" y2="256"/><line x1="180" y1="56" x2="180" y2="256"/><line x1="220" y1="56" x2="220" y2="256"/><line x1="260" y1="56" x2="260" y2="256"/><line x1="300" y1="56" x2="300" y2="256"/><line x1="340" y1="56" x2="340" y2="256"/><line x1="380" y1="56" x2="380" y2="256"/><line x1="420" y1="56" x2="420" y2="256"/>
    <line x1="60" y1="56" x2="420" y2="56"/><line x1="60" y1="96" x2="420" y2="96"/><line x1="60" y1="136" x2="420" y2="136"/><line x1="60" y1="176" x2="420" y2="176"/><line x1="60" y1="216" x2="420" y2="216"/><line x1="60" y1="256" x2="420" y2="256"/>
  </g>
  <!-- outer polygon -->
  <polygon points="100,90 360,70 400,180 320,240 130,230 80,150" style="fill:#7c9a63;fill-opacity:0.5;stroke:#3c6b39;stroke-width:2" />
  <!-- hole -->
  <polygon points="200,130 280,128 285,190 205,195" style="fill:#f3f0df;stroke:#3c6b39;stroke-width:1.6" />
  <!-- scanline -->
  <line x1="40" y1="160" x2="440" y2="160" stroke="#cf6a2a" stroke-width="2" />
  <text class="d-sub" x="40" y="152" style="fill:#a9501c">scanline y + 0.5</text>
  <!-- crossings -->
  <g fill="#cf6a2a"><circle cx="86" cy="160" r="4.5"/><circle cx="203" cy="160" r="4.5"/><circle cx="283" cy="160" r="4.5"/><circle cx="395" cy="160" r="4.5"/></g>
  <text class="d-sub" x="86" y="278" text-anchor="middle">x0</text><text class="d-sub" x="203" y="278" text-anchor="middle">x1</text><text class="d-sub" x="283" y="278" text-anchor="middle">x2</text><text class="d-sub" x="395" y="278" text-anchor="middle">x3</text>
  <!-- filled spans -->
  <rect x="86" y="153" width="117" height="14" fill="#cf6a2a" fill-opacity="0.3"/>
  <rect x="283" y="153" width="112" height="14" fill="#cf6a2a" fill-opacity="0.3"/>

  <!-- right notes -->
  <text class="d-label" x="470" y="96">fill (x0→x1)</text>
  <text class="d-label" x="470" y="124">skip (x1→x2)</text>
  <text class="d-sub" x="470" y="140">the hole</text>
  <text class="d-label" x="470" y="168">fill (x2→x3)</text>
  <line class="d-stroke" x1="460" y1="200" x2="690" y2="200" style="stroke:#9aa884"/>
  <text class="d-sub" x="470" y="224" style="font-size:11px">A hole is just another ring in</text>
  <text class="d-sub" x="470" y="240" style="font-size:11px">the same crossing list — no</text>
  <text class="d-sub" x="470" y="256" style="font-size:11px">special case needed.</text>
</svg>
<figcaption>Holes cost nothing extra: they're additional rings whose crossings join the same sorted list, so the even-odd pairing leaves them empty automatically. Each span is rounded <b>outward</b> (left floored, right ceiled) so adjacent fills overlap by at most a pixel — cheap insurance that closes hairline cracks at chunk seams.</figcaption>
</figure>

The fill leans on one `DrawTarget` primitive: a fast clipped horizontal-rectangle blit, one per span. There's no per-pixel plotting in the hot loop. A row whose crossing list would overflow its small fixed buffer is skipped entirely rather than filled from a truncated list — keeping the even-odd parity correct is more important than one stray row.

### Lines: clip first, then stroke

Lines are where a naïve approach gets expensive. A loaded route or a long road is mostly *off-screen* at riding zoom, and a thick-line rasteriser that walks the whole polyline pays for every off-screen pixel. The renderer's line path is built to never pay for what it can't see.

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="On the left, a long route crosses a small viewport; only the visible portion is stroked while the off-screen majority costs nothing. On the right, a thick line drawn as a chain of filled rectangles leaves a notch at each joint, smoothed by filling a disc at the run ends and at the vertices where the line bends sharply.">
  <text class="d-tag" x="20" y="24">Clip to the view, then smooth the joints</text>

  <!-- LEFT: clip -->
  <rect x="34" y="48" width="300" height="224" rx="8" style="fill:#eef2df;stroke:#9aa884;stroke-width:1.2;stroke-dasharray:4 4" />
  <text class="d-sub" x="44" y="66">the whole route</text>
  <!-- off-screen route (grey) -->
  <path d="M20 250 C 120 60, 180 320, 250 120 S 360 40, 460 160" fill="none" stroke="#9aa884" stroke-width="3" stroke-opacity="0.6" />
  <!-- viewport -->
  <rect x="150" y="120" width="120" height="92" style="fill:none;stroke:#cf6a2a;stroke-width:2.4" />
  <text class="d-label" x="210" y="138" text-anchor="middle" style="fill:#a9501c">view</text>
  <!-- visible portion (coral, bold) -->
  <path d="M150 196 C 170 150, 200 150, 214 150 S 250 168, 270 150" fill="none" stroke="#cf6a2a" stroke-width="4" />
  <text class="d-sub" x="44" y="262" style="font-size:11px">only the clipped part is stroked</text>

  <!-- RIGHT: joints -->
  <text class="d-sub" x="430" y="66">butt-jointed rects (notch)</text>
  <polyline points="420,110 470,140 520,104 560,150" fill="none" stroke="#4f6b43" stroke-width="12" stroke-linejoin="bevel" stroke-linecap="butt" />
  <text class="d-sub" x="430" y="196">+ round-join / cap discs</text>
  <polyline points="420,224 470,254 520,218 560,264" fill="none" stroke="#4f6b43" stroke-width="12" stroke-linejoin="bevel" stroke-linecap="butt" />
  <g fill="#3c6b39"><circle cx="420" cy="224" r="6"/><circle cx="470" cy="254" r="6"/><circle cx="520" cy="218" r="6"/><circle cx="560" cy="264" r="6"/></g>
  <text class="d-sub" x="600" y="244" style="font-size:11px">a disc (⌀ = width) at</text>
  <text class="d-sub" x="600" y="260" style="font-size:11px">corners + run ends →</text>
  <text class="d-sub" x="600" y="276" style="font-size:11px">smooth arc, no gaps</text>
</svg>
<figcaption>Each segment is clipped to the view (grown by the stroke's half-width so edge-hugging lines keep full thickness) <i>before</i> it's drawn, so the stroker only ever touches on-screen pixels. The points are also deduplicated in screen space at a subpixel tolerance — folding away the integer-projection staircase — so a dense line hands the stroker far fewer segments without ever shifting a visible pixel.</figcaption>
</figure>

A **1 px** line is stroked by embedded-graphics directly — a thin Bresenham line it draws cheaply, and the one width the span fill below can't (a zero-width rectangle has no scanline crossings). **Everything else** — width-2 roads on up, plus the route and breadcrumb — takes a different path, because a per-pixel thick-line rasteriser is exactly the trap above: it *generates* every pixel of the stroke one at a time, and that generation, not the writes, was measured to dominate the overlay (and to run ~10× a span stroke even at 2 px, the narrowest thick width). So those strokes go through the **same scanline span fill as the polygons**: each segment becomes a rectangle (the segment swept ±half-width along its perpendicular), filled row-by-row as one `fill_solid` span apiece. No per-pixel plotting; the whole overlay rides the coalesced row blit.

Two filled rectangles butt-joined at a vertex leave a small notch on the outside of a bend and don't round the line's ends, so a disc the width of the stroke is filled (also as spans) to smooth each joint and cap. The disc is the overlay's biggest remaining cost, so it's spent only where it shows: at the two **run ends** (which round the cap and close the gap to the next feature at a chunk seam) and at interior vertices where the line **bends sharply**. The uncovered notch at a turn of `θ` off straight is only about `r·sin(θ/2)` deep (`r` = half the stroke width), so a gentle bend leaves a sub-pixel notch and needs no disc at all.

The half-width `r` above is **not** the style's stored `weight` directly — a line's on-screen thickness **ramps with zoom**. A style's `weight` is its width at a *reference* ground scale (mid-riding zoom); each frame the renderer multiplies it by `(ref_mpp / mpp)^0.6` — one factor for the whole frame — so a road thickens as you zoom in and thins as you zoom out, the way a physical thing looks closer or farther. The exponent is deliberately **sub-linear**: true physical scaling (`^1`) would make every road sub-pixel at continent zoom and let one motorway swallow the panel up close, so the ramp is clamped to `1…12 px` and rounded to whole pixels (the map zooms in fixed ×1.2 detents, so the width steps cleanly with no shimmer). This also rights the cost curve — a wide overview, where hundreds of roads are on screen and the frame budget is tightest, now strokes each at 1–2 px instead of its full authored weight; the fat strokes happen only zoomed in, where a handful of features are visible. Dashed lines (admin borders, railway stripes) ride the same ramped width, so their dash rhythm tracks the line's thickness rather than fighting it.

### A second colour and a dash: line styles

Everything above strokes a line in one flat colour. But a railway isn't flat, an admin border is dashed, a road at riding zoom wants a darker *casing* down each side so it reads as a road and not a scratch, and a building at the finest zoom wants a crisp wall instead of a grey slab. Version 10 of the map format gives every style two extra knobs — a **`line_style`** (solid or dashed) and an optional **secondary colour `color2`** — and the renderer fans those two bits, crossed with whether the feature is a line or a polygon, into five distinct looks:

| Feature | `line_style` | `color2` | Renders as |
| :-- | :-- | :-- | :-- |
| Line | solid | — | a flat stroke — the unchanged path |
| Line | solid | set | **road casing** — a wider `color2` base under the fill (finest LOD only) |
| Line | dashed | — | **dashes** in `color`, gaps transparent (admin borders) |
| Line | dashed | set | **railway stripe** — a solid `color2` base with `color` dashes on top |
| Polygon | *(ignored)* | set | fill in `color`, plus a `color2` **ring outline** (finest LOD only) |

Each span already carries its one-byte `style_id`, so the draw loop re-resolves the full style from the scene source's `O(1)` style table at draw time — no extra per-span RAM. Two of the five (casing, outline) are *extra passes* that cost real time, so they're gated to the finest LOD; the other three ride the existing stroke path. A config that uses **none** of them renders byte-for-byte what it did before — the whole feature is built to be free when unused, and each renderer sub-issue proved it with a before/after PNG md5 diff on a fixed camera.

**Dashes, for free.** A dashed line reuses the entire clip-then-stroke pipeline unchanged and diverges only at the very end: instead of filling the whole visible run, it walks that run in **screen-space arc length** and emits only the "on" intervals. Because the clip happens *first*, the walker only ever sees on-screen geometry — so a dashed line is actually *cheaper* than the solid one, not dearer: it clips away the same off-screen majority, then paints only half of what survives. The phase resets at each clipped run (each time the line re-enters the view), which lets a dash straddle a bend seamlessly but also means the pattern can "crawl" a pixel or two as you pan — every slippy map does this, and it isn't worth carrying feature-space arc length to avoid. A **railway stripe** is just this composed with a base: stroke the whole line once in `color2`, then stroke `color` dashes on top, so the gaps between the dark dashes show the light base through — alternating stripes, no perpendicular crossties.

### Road casing, and the z-boundary that makes junctions work

A casing is a `weight + 2 px` stroke in `color2` painted **under** a road's fill, giving the road a darker edge down each side — the OSM-carto / Komoot "roads have borders" look. The subtlety isn't the stroke; it's *when* to paint it. Paint all the casings first, before the map, and the low-z fills that blanket a town (landuse, forest, water) paint straight over them — the casings vanish. Paint each casing right before its own road, and where two roads cross, one road's casing slices across the other's fill and the junction reads as cut.

The spans are already sorted by `(z, seq)`, so the cased road lines form one contiguous z-band. The casing pass is inserted at exactly the **z boundary where that band begins** — a split index into the sorted array, not a re-sort — so the casings land *above* the low-z fills that would erase them yet *under* every road fill.

<figure class="fig">
<svg viewBox="0 0 800 320" role="img" aria-label="Left: the frame's spans, sorted by z-index and drawn bottom-up — water, landuse and building fills at the bottom, then a dashed split line marking the first cased road line, then the road casings, then the road fills on top. The casing pass is inserted at that split: after the low-z fills, so they cannot paint over it, but under every road fill. Right: a junction where two roads cross — the road fills stay continuous through the crossing and the darker casing hugs only the outside of each road, with no casing line slicing across the junction.">
  <text class="d-tag" x="20" y="24">The casing pass goes at the z boundary — not before the frame</text>

  <!-- LEFT: the sorted span stack -->
  <text class="d-sub" x="150" y="52" text-anchor="middle">spans · sorted (z, seq) · drawn bottom-up</text>
  <text class="d-sub" x="42" y="186" text-anchor="middle" transform="rotate(-90 42 186)" style="font-size:9px">z ↑ · paint order ↑</text>

  <!-- bands: bottom (low z) drawn first -->
  <rect x="90" y="248" width="110" height="26" rx="4" class="d-water" />
  <text class="d-num" x="145" y="265" text-anchor="middle">water</text>
  <rect x="90" y="220" width="110" height="26" rx="4" class="d-forest" />
  <text class="d-num" x="145" y="237" text-anchor="middle">landuse</text>
  <rect x="90" y="192" width="110" height="26" rx="4" style="fill:#d6cda8;stroke:#3c6b39;stroke-width:0.8" />
  <text class="d-sub" x="145" y="209" text-anchor="middle">buildings</text>

  <!-- split line -->
  <line x1="72" y1="186" x2="214" y2="186" stroke="#cf6a2a" stroke-width="2" stroke-dasharray="5 4" />

  <!-- casing pass -->
  <rect x="90" y="152" width="110" height="26" rx="4" style="fill:#5a4326" />
  <text class="d-num" x="145" y="169" text-anchor="middle">casings</text>
  <!-- road fills -->
  <rect x="90" y="124" width="110" height="26" rx="4" style="fill:#cdb894;stroke:#5a4326;stroke-width:0.8" />
  <text class="d-sub" x="145" y="141" text-anchor="middle">road fills</text>

  <!-- step annotations -->
  <text class="d-sub" x="216" y="140" style="fill:#a9501c;font-size:9.5px">③ spans[split..) — road band, on top</text>
  <text class="d-sub" x="216" y="168" style="fill:#a9501c;font-size:9.5px">② casing pass — wide color2, finest LOD</text>
  <text class="d-sub" x="216" y="184" style="font-size:9px">split — first cased road line</text>
  <text class="d-sub" x="216" y="230" style="fill:#a9501c;font-size:9.5px">① spans[0..split) — the base pass</text>

  <!-- RIGHT: a junction -->
  <text class="d-sub" x="620" y="52" text-anchor="middle">at a crossing</text>
  <!-- casing (dark), drawn first -->
  <rect x="520" y="162" width="200" height="36" style="fill:#5a4326" />
  <rect x="602" y="100" width="36" height="160" style="fill:#5a4326" />
  <!-- fills (tan), on top — cover the casing where the roads cross -->
  <rect x="520" y="166" width="200" height="28" style="fill:#cdb894" />
  <rect x="606" y="100" width="28" height="160" style="fill:#cdb894" />
  <!-- callouts -->
  <line class="d-stroke" x1="620" y1="180" x2="620" y2="180" />
  <text class="d-sub" x="620" y="284" text-anchor="middle" style="font-size:9.5px">fills continuous through the junction</text>
  <text class="d-sub" x="620" y="298" text-anchor="middle" style="font-size:9.5px">casing hugs the outside of each road only</text>
</svg>
<figcaption>Because the spans are already <code>(z, seq)</code>-sorted, the cased road lines are a contiguous band; the draw phase draws <b>①</b> everything below that band, then runs the <b>②</b> casing pass, then draws <b>③</b> the road band on top. So a casing survives the landuse/water fills below it, yet sits under all the road fills above it — at a junction (right) the fills stay continuous and no casing slices across the crossing. With no cased style the split is the whole array, the casing pass is empty, and ①+③ collapse to today's single pass; coarser LODs skip the pass outright.</figcaption>
</figure>

### Building outlines: all the fills, then all the walls

A polygon whose style carries a `color2` gets **every ring — its exterior and any courtyard holes — stroked closed** in that colour at the finest LOD, so a building stops reading as a flat grey slab when you zoom in. Here too the interesting part is ordering. Touching row-house buildings share a wall, so if you outline each building right after its own fill, the *next* building's fill paints over the wall the previous one just drew — the terrace merges into one blob.

The fix reuses the same `(z, seq)` sort. Within each contiguous **equal-z group** the draw loop runs two passes: **all the fills first**, then **all the outlines**. Nothing paints a fill after an outline within the group, so a neighbour can no longer erase a shared wall — both buildings stroke that edge, and it survives. The group closes before the next z begins, so a road at a higher z still paints over a building outline where it crosses.

<figure class="fig">
<svg viewBox="0 0 760 300" role="img" aria-label="Top row, per-feature order (wrong): building A is filled and outlined, then building B's fill lands over the shared edge and erases A's wall there, then B is outlined — the two touching buildings merge into one block with no divider. Bottom row, per-z-group order (right): both buildings are filled first, then both are outlined — the shared middle wall is drawn last, after every fill, so it survives and the two buildings read as distinct.">
  <text class="d-tag" x="20" y="24">Outline after every fill in the z-group, so shared walls survive</text>

  <!-- TOP ROW: per-feature (wrong) -->
  <text class="d-label" x="30" y="70" style="fill:#a9501c">per feature</text>
  <text class="d-sub" x="30" y="84" style="font-size:9px">outline right after each fill</text>

  <!-- frame 1: A filled + outlined -->
  <rect x="182" y="52" width="44" height="44" style="fill:#d6cda8;stroke:#cf6a2a;stroke-width:2.5" />
  <text class="d-sub" x="204" y="112" text-anchor="middle" style="font-size:9px">① fill + outline A</text>

  <!-- frame 2: B's fill lands over the shared edge -->
  <rect x="352" y="52" width="44" height="44" style="fill:#d6cda8" />
  <rect x="396" y="52" width="44" height="44" style="fill:#d6cda8" />
  <!-- A's surviving outer edges (coral), but the shared edge is covered by B's fill -->
  <path d="M352 52 h44 M352 96 h44 M352 52 v44" fill="none" stroke="#cf6a2a" stroke-width="2.5" />
  <line x1="399" y1="50" x2="399" y2="98" stroke="#c0492e" stroke-width="2" stroke-dasharray="3 3" />
  <text x="446" y="70" style="font-family:var(--mono);font-size:9px;fill:#c0492e">B's fill erased</text>
  <text x="446" y="82" style="font-family:var(--mono);font-size:9px;fill:#c0492e">A's shared wall</text>
  <text class="d-sub" x="374" y="112" text-anchor="middle" style="font-size:9px">② B's fill lands</text>

  <!-- frame 3: B outlined -> blob -->
  <rect x="600" y="52" width="88" height="44" style="fill:#d6cda8;stroke:#cf6a2a;stroke-width:2.5" />
  <text class="d-sub" x="644" y="112" text-anchor="middle" style="font-size:9px">③ one merged blob ✗</text>

  <!-- divider -->
  <line class="d-stroke" x1="30" y1="150" x2="730" y2="150" style="stroke:#9aa884;stroke-width:1" />

  <!-- BOTTOM ROW: per-z-group (right) -->
  <text class="d-label" x="30" y="192" style="fill:#3c6b39">per z-group</text>
  <text class="d-sub" x="30" y="206" style="font-size:9px">all fills, then all outlines</text>

  <!-- frame 1: both fills, no outlines -->
  <rect x="182" y="176" width="44" height="44" style="fill:#d6cda8" />
  <rect x="226" y="176" width="44" height="44" style="fill:#d6cda8" />
  <text class="d-sub" x="226" y="236" text-anchor="middle" style="font-size:9px">① all fills first</text>

  <!-- frame 2: both outlined -> wall kept -->
  <rect x="374" y="176" width="44" height="44" style="fill:#d6cda8;stroke:#cf6a2a;stroke-width:2.5" />
  <rect x="418" y="176" width="44" height="44" style="fill:#d6cda8;stroke:#cf6a2a;stroke-width:2.5" />
  <line x1="418" y1="174" x2="418" y2="222" stroke="#cf6a2a" stroke-width="2.5" />
  <text class="d-sub" x="418" y="236" text-anchor="middle" style="font-size:9px">② all outlines</text>

  <!-- result -->
  <text x="560" y="196" style="font-family:var(--mono);font-size:9.5px;fill:#3c6b39">the shared wall is</text>
  <text x="560" y="210" style="font-family:var(--mono);font-size:9.5px;fill:#3c6b39">drawn after every fill</text>
  <text x="560" y="224" style="font-family:var(--mono);font-size:9.5px;fill:#3c6b39">→ two crisp buildings ✓</text>
</svg>
<figcaption>Both shared-wall neighbours sit in the same z-group (buildings share a z), so drawing every fill before any outline is what keeps the wall. The outline is a <b>fixed 1-px hairline</b>, not the zoom-ramped stroke width: ramped, a closed ring hits 3–4 px at the sub-metre finest-LOD scale and its round joins flood a small footprint until the fill drowns and the building reads as a dark slab — the opposite of the goal. With no outlined polygon the group takes the single-loop path, byte-identical to before.</figcaption>
</figure>

**What the finest-LOD passes cost.** Both casing and outlines run **only at the finest LOD**, so every coarser zoom — where the frame budget is already tightest — pays exactly nothing. Where they do run, the numbers (measured `draw_us`, dense street scenes) are modest: casing adds **~20–25%** to the draw (at 4 m/px on a dense grid, ~170–179 µs climbs to ~206–250 µs for 152 cased roads; ~+55–80 µs at a busier zoom), because a casing is one extra wide stroke — wider than the fill it underlies, so its cost is the raster, not the re-projection. Building outlines are far cheaper still — **~7–9% of the casing pass** (about +10 µs at 2 m/px, +25 µs at 4 m/px) — because a hairline ring is the thin Bresenham line path, a fraction of a filled stroke.

## 7 · The overlays

The map underneath is the base; everything that moves with *you* is drawn on top, after the map, in a fixed order:

1. **Route** — the planned line (magenta), stroked through the same clip-then-stroke path, with direction **chevrons** laid down in a second sweep so they sit on top even where the route doubles back. Chevrons are spaced by *ground distance* derived from a fixed pixel cadence, so they stay evenly spread as you zoom and stay pinned to the ground as you pan.
2. **Breadcrumb** — the recorded trail behind you (navy), a two-tier coarse-spine-plus-recent-tail path.
3. **Marker** — a course-pointing chevron (or a stationary diamond) at your fix, a fixed screen size, culled when off-view.
4. **HUD chrome** — the off-route readout and pan-mode indicators.

Each is just another polyline through the stroker or a triangle through the polygon fill — and since a thick stroke is itself rectangles and discs filled as spans, nearly everything on screen comes down to the one scanline span fill, reused.

## To the panel: the banded push

Everything above is shared — byte-for-byte identical on the simulator and the device. This last step is where the *transport* parts, because the device has neither the memory nor the display hardware a desktop takes for granted. The device has **512 KB of RAM and no external memory**, and **no scan-out engine** that would stream a framebuffer to the panel on its own. So it does two things a PC never has to: it draws into a *device-native* RGB222 framebuffer, and then it ships that framebuffer to the panel itself, a strip at a time. The **simulator's interactive window is the second backend of the very same [display contracts](src:firmware/obc-display/src/display_contracts/mod.rs)**: it renders into the identical RGB222 framebuffer and runs the identical self-diffing present, differing only in the final hop — it uploads the changed rows to an `egui` texture instead of scanning them to a panel. (The headless `--png` dump is the one path that still writes a full true-colour framebuffer in one blit — the un-quantized reference.)

### The RGB222 framebuffer

The renderer draws into a single resident **RGB222** plane: one byte per pixel over the 240×320 panel — 75 KB, the whole frame held in `.bss`. Each byte is `0b00_RR_GG_BB`, the top two bits of each channel: the 64-colour gamut the [style table is tuned to](src:firmware/obc-reader/src/color.rs). The renderer's `color_fn` is the identity (styles are already RGB565) and the framebuffer quantises to those 64 colours *on store* — so the expensive geometry code from sections 1–7 is exactly the simulator's, and only the pixel sink differs. This is the [`DrawTarget` seam](../architecture/#two-hosts-one-core-and-the-seams-between-them) from the top of the page, realised for the device.

### A band at a time (the bring-up path)

A finished frame now sits in RAM, but nothing is putting it on glass. During bring-up the firmware did it in software over SPI: walk the framebuffer top to bottom and DMA it to a stand-in panel a **band** of a few rows at a time. That band push *was* the visible refresh — a top-to-bottom wipe you could watch sweep down the panel. The shipping device hands that same job to the **FLPR** coprocessor (below), which scans the framebuffer itself; the banding picture still frames the shape of the problem — get a resident frame onto a panel with no host-side scan-out engine, a strip at a time — so it's worth seeing first.

<figure class="fig">
<svg viewBox="0 0 720 290" role="img" aria-label="The resident RGB222 framebuffer on the left is sliced into horizontal bands. One band is read out, packed to the panel's wire format in a small reused scratch buffer, and DMA'd over SPI into a matching CASET/RASET window on the panel to the right. The band scratch is only a few rows and is reused for every band, so there is never a second full-frame copy.">
  <defs>
    <marker id="aG" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">One finished frame → the panel, a band at a time</text>

  <!-- framebuffer (left) -->
  <rect class="d-panel-2" x="44" y="52" width="116" height="196" rx="8" />
  <g stroke="#9aa884" stroke-opacity="0.5" stroke-width="1">
    <line x1="44" y1="76" x2="160" y2="76"/><line x1="44" y1="100" x2="160" y2="100"/><line x1="44" y1="124" x2="160" y2="124"/><line x1="44" y1="148" x2="160" y2="148"/><line x1="44" y1="172" x2="160" y2="172"/><line x1="44" y1="196" x2="160" y2="196"/><line x1="44" y1="220" x2="160" y2="220"/>
  </g>
  <!-- the band being pushed -->
  <rect x="44" y="100" width="116" height="24" class="d-hot-fill" />
  <text class="d-label" x="102" y="40" text-anchor="middle" style="font-size:11px">RGB222 framebuffer</text>
  <text class="d-sub" x="102" y="266" text-anchor="middle">240×320 · 75 KB · .bss</text>

  <!-- pack box (middle) -->
  <line class="d-flow" x1="160" y1="112" x2="300" y2="112" marker-end="url(#aG)" />
  <rect class="d-hot" x="300" y="88" width="132" height="48" rx="10" style="fill:#f8efe4" />
  <text class="d-label" x="366" y="108" text-anchor="middle" style="fill:#a9501c;font-size:11px">pack → RGB444</text>
  <text class="d-sub" x="366" y="124" text-anchor="middle">2 px → 3 bytes</text>
  <text class="d-sub" x="366" y="158" text-anchor="middle" style="font-size:10px">band scratch ≈ 7 KB · reused ×23</text>

  <!-- arrow to panel -->
  <line class="d-flow" x1="432" y1="112" x2="556" y2="112" marker-end="url(#aG)" />
  <text class="d-sub" x="494" y="104" text-anchor="middle" style="font-size:10px">SPI · DMA</text>
  <text class="d-sub" x="494" y="128" text-anchor="middle" style="font-size:10px">CASET/RASET</text>

  <!-- panel (right) -->
  <rect class="d-panel" x="560" y="52" width="116" height="196" rx="8" style="fill:#e7ead8" />
  <rect x="560" y="100" width="116" height="24" class="d-hot-fill" style="fill-opacity:0.55" />
  <text class="d-label" x="618" y="40" text-anchor="middle" style="font-size:11px">ST7789 · bring-up</text>
  <text class="d-sub" x="618" y="266" text-anchor="middle">addressed window</text>
  <!-- scanline progression -->
  <line x1="690" y1="60" x2="690" y2="240" stroke="#cf6a2a" stroke-width="1.4" stroke-dasharray="3 3" marker-end="url(#aG)" />
  <text class="d-sub" x="700" y="154" style="font-size:9px" transform="rotate(90 700 154)">top → bottom</text>
</svg>
<figcaption>The seam that hides the panel's wire format is the presenter half of the display contracts (in <b>obc-display</b>): the renderer draws each band through a frame-absolute <code>Band</code> view, and the backend reformats + transports it. The band scratch is only a few rows, <b>reused for every band</b>, so the frame lives once in the RGB222 plane and never as a second full RGB565 copy.</figcaption>
</figure>

The wire format lives behind the board-agnostic [display contracts](src:firmware/obc-display/src/display_contracts/mod.rs) (in **obc-display**), so the render stack never couples to it — and the seam has **two live implementations** keeping it honest: the LS021/FLPR panel on-device and the simulator on the host, the latter compiled and tested in the workspace on every CI run. The original bring-up stand-in was an **ST7789** over SPI: each band was packed to the panel's **12-bit RGB444** format — two pixels into three bytes, ~25% fewer bytes than RGB565, and the RGB222 gamut survives 4-bit channels losslessly — then a `CASET`/`RASET` window addressed and the bytes streamed by DMA. Because the scratch was just a few rows (~7 KB), the 320-row frame tiled through it in ~23 pushes; the frame itself never got a second full-frame buffer. The shipping panel drops the band scratch entirely — the FLPR packs each line straight from the resident frame.

That seam is **two separable contracts**, spelled out by the [generic display contracts](src:firmware/obc-display/src/display_contracts/mod.rs) both backends now implement directly: a **native-frame format** — geometry, the device's own pixel storage, stride, and a `DrawTarget` writing straight into the backing (the shipping frame is the 240×320 RGB222 plane above) — and **presenter capabilities** — presenting the clean resident frame, and compositing a bounded transient overlay over it (next section). The frame lives *next to* its presenter at each host's composition edge, so Rust's borrows encode when render and present may touch the bytes: rendering borrows the frame mutably, a base present shares it for the whole scan (on the device the FLPR is reading those very bytes for ~44 ms), and the overlay present borrows it mutably for its transient composite-and-restore. What counts as a *changed region* deliberately belongs to the presenter, not the contract: the per-row hashing and span masking described below are the [LS021 pairing's own damage strategy](src:firmware/obc-display/src/ls021/mod.rs), and a different panel could diff tiles or lean on a controller's dirty window without the render stack noticing. A swapped display remains what it always was — a new (frame, presenter) pairing at the board's composition edge, with different geometry and pixel storage now first-class rather than implicit.

The panel the device ships on is a reflective **memory-LCD (LS021B7DD02-class MIP)**, driven by the nRF's **FLPR** coprocessor — the only display path. The FLPR scans the frame top-to-bottom in one pass, so the M33 renders into the RGB222 plane and then **presents** it — and since [issue #347](https://github.com/timohueser/OpenBikeComputer/issues/347) that present costs the M33 almost nothing: the FLPR reads the framebuffer **directly** out of shared SRAM and packs each line to the panel wire itself, while the M33 just publishes the dirty-row list, rings a doorbell, and *awaits* the coprocessor's end-of-frame interrupt (free to run storage or sensor work for the whole scan). Worth saying plainly: the FLPR is **not** a free scan-out engine, and a *full* MIP frame is ~44 ms after the [issue #348](https://github.com/timohueser/OpenBikeComputer/issues/348) timing pass. So the present doesn't rewrite the whole frame when it needn't. It keeps a **per-row hash of the last-pushed frame**, and on each present re-hashes the rows and drives a **span-masked scan** ([issue #163](https://github.com/timohueser/OpenBikeComputer/issues/163)) over only the spans whose hash changed — the FLPR fast-forwarding its gate over the unchanged rows and early-stopping after the last, so frame cost scales with *changed rows*, not a flat 320.

The screens never say *where* they changed — they stay immediate-mode, clearing and redrawing the whole frame — so the present detects the changed region **automatically** ([issue #201](https://github.com/timohueser/OpenBikeComputer/issues/201)): a Home clock ticking a minute re-hashes to find just its clock band and repaints that (~44 ms → a few ms), the contour backdrop behind it untouched, with zero per-screen code. A collision — a changed row hashing equal, so skipped — is ~2⁻³² per row-change and self-heals the next time the row changes; the simulator runs an exact full-frame diff as a CI oracle, so only random, self-healing misses ever reach glass. The hash earns that rate the hard way: it folds four bytes per multiply for speed, and plain word-FNV mixed that way turns out to have a structural blind spot — a change confined to the top byte of its words (pixel columns 3, 7, 11…) kept 8 bits of discrimination instead of 32, a *~2⁻⁸* miss the oracle caught the moment a demo parked on a static screen ([issue #626](https://github.com/timohueser/OpenBikeComputer/issues/626)). Each word is now avalanche-mixed before it meets the accumulator, restoring the ~2⁻³² figure for structured changes too. That's render-on-demand carried onto the glass; the overlay below rides the very same masked scan.

<figure class="fig">
<svg viewBox="0 0 800 366" role="img" aria-label="The self-diffing present. Left: the framebuffer, drawn as 16 stacked rows; an immediate-mode screen redraws all 320 rows every frame, but only a band in the middle — the clock — actually changed. Middle: a per-row 32-bit hash (a 1.28 KB store of one hash per row) is compared to last frame; rows whose hash equals the stored one are skipped, the contiguous run of changed rows coalesces into one span. Right: the FLPR runs one masked scan of the panel — it fast-forwards its gate over the unchanged rows, writes only the changed span, and stops early, so only those rows reach the glass and the rest of the image is retained. A one-minute clock tick costs a few rows instead of a full ~44 ms frame.">
  <defs>
    <marker id="rdA" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Redraw the whole frame — push only the rows that changed</text>
  <text class="d-sub" x="103" y="46" text-anchor="middle" style="font-size:9px;fill:#6b7758">① the frame</text>
  <rect class="d-panel-2" x="52" y="58" width="102" height="208" rx="4" />
  <g stroke="#9aa884" stroke-opacity="0.45" stroke-width="1">
    <line x1="52" y1="71" x2="154" y2="71" />
    <line x1="52" y1="84" x2="154" y2="84" />
    <line x1="52" y1="97" x2="154" y2="97" />
    <line x1="52" y1="110" x2="154" y2="110" />
    <line x1="52" y1="123" x2="154" y2="123" />
    <line x1="52" y1="136" x2="154" y2="136" />
    <line x1="52" y1="149" x2="154" y2="149" />
    <line x1="52" y1="162" x2="154" y2="162" />
    <line x1="52" y1="175" x2="154" y2="175" />
    <line x1="52" y1="188" x2="154" y2="188" />
    <line x1="52" y1="201" x2="154" y2="201" />
    <line x1="52" y1="214" x2="154" y2="214" />
    <line x1="52" y1="227" x2="154" y2="227" />
    <line x1="52" y1="240" x2="154" y2="240" />
    <line x1="52" y1="253" x2="154" y2="253" />
  </g>
  <rect x="52" y="149" width="102" height="39" fill="#cf6a2a" fill-opacity="0.5" />
  <text x="103" y="171" text-anchor="middle" style="font-family:var(--mono);font-size:9px;fill:#7a3b16">clock</text>
  <text class="d-label" x="103" y="288" text-anchor="middle" style="font-size:10.5px">framebuffer</text>
  <text class="d-sub" x="103" y="302" text-anchor="middle" style="font-size:9.5px">screen redrew all 320 rows</text>
  <line class="d-flow" x1="158" y1="162" x2="204" y2="162" marker-end="url(#rdA)" />
  <text class="d-sub" x="181" y="154" text-anchor="middle" style="font-size:9px">hash each</text>
  <text class="d-sub" x="232" y="46" text-anchor="middle" style="font-size:9px;fill:#6b7758">② the diff</text>
  <rect x="216" y="60" width="32" height="9" rx="1.5" fill="#c7cdb6" fill-opacity="0.85" stroke="#9aa884" stroke-opacity="0.5" stroke-width="0.6" />
  <rect x="216" y="73" width="32" height="9" rx="1.5" fill="#c7cdb6" fill-opacity="0.85" stroke="#9aa884" stroke-opacity="0.5" stroke-width="0.6" />
  <rect x="216" y="86" width="32" height="9" rx="1.5" fill="#c7cdb6" fill-opacity="0.85" stroke="#9aa884" stroke-opacity="0.5" stroke-width="0.6" />
  <rect x="216" y="99" width="32" height="9" rx="1.5" fill="#c7cdb6" fill-opacity="0.85" stroke="#9aa884" stroke-opacity="0.5" stroke-width="0.6" />
  <rect x="216" y="112" width="32" height="9" rx="1.5" fill="#c7cdb6" fill-opacity="0.85" stroke="#9aa884" stroke-opacity="0.5" stroke-width="0.6" />
  <rect x="216" y="125" width="32" height="9" rx="1.5" fill="#c7cdb6" fill-opacity="0.85" stroke="#9aa884" stroke-opacity="0.5" stroke-width="0.6" />
  <rect x="216" y="138" width="32" height="9" rx="1.5" fill="#c7cdb6" fill-opacity="0.85" stroke="#9aa884" stroke-opacity="0.5" stroke-width="0.6" />
  <rect x="216" y="151" width="32" height="9" rx="1.5" fill="#cf6a2a" fill-opacity="1" stroke="#9aa884" stroke-opacity="0.5" stroke-width="0.6" />
  <rect x="216" y="164" width="32" height="9" rx="1.5" fill="#cf6a2a" fill-opacity="1" stroke="#9aa884" stroke-opacity="0.5" stroke-width="0.6" />
  <rect x="216" y="177" width="32" height="9" rx="1.5" fill="#cf6a2a" fill-opacity="1" stroke="#9aa884" stroke-opacity="0.5" stroke-width="0.6" />
  <rect x="216" y="190" width="32" height="9" rx="1.5" fill="#c7cdb6" fill-opacity="0.85" stroke="#9aa884" stroke-opacity="0.5" stroke-width="0.6" />
  <rect x="216" y="203" width="32" height="9" rx="1.5" fill="#c7cdb6" fill-opacity="0.85" stroke="#9aa884" stroke-opacity="0.5" stroke-width="0.6" />
  <rect x="216" y="216" width="32" height="9" rx="1.5" fill="#c7cdb6" fill-opacity="0.85" stroke="#9aa884" stroke-opacity="0.5" stroke-width="0.6" />
  <rect x="216" y="229" width="32" height="9" rx="1.5" fill="#c7cdb6" fill-opacity="0.85" stroke="#9aa884" stroke-opacity="0.5" stroke-width="0.6" />
  <rect x="216" y="242" width="32" height="9" rx="1.5" fill="#c7cdb6" fill-opacity="0.85" stroke="#9aa884" stroke-opacity="0.5" stroke-width="0.6" />
  <rect x="216" y="255" width="32" height="9" rx="1.5" fill="#c7cdb6" fill-opacity="0.85" stroke="#9aa884" stroke-opacity="0.5" stroke-width="0.6" />
  <line class="d-stroke" x1="248" y1="90" x2="262" y2="90" style="stroke-width:1;stroke:#9aa884" />
  <text class="d-sub" x="266" y="93" style="font-size:9px">hash = stored → skip</text>
  <path d="M254 151 h6 v35 h-6" fill="none" stroke="#cf6a2a" stroke-width="1.6" />
  <text x="266" y="167" style="font-family:var(--mono);font-size:9px;fill:#a9501c">hash ≠ stored</text>
  <text x="266" y="180" style="font-family:var(--mono);font-size:9.5px;fill:#a9501c">→ span (y₀, 3)</text>
  <text class="d-sub" x="246" y="288" text-anchor="middle" style="font-size:9.5px">32-bit hash per row</text>
  <text class="d-sub" x="246" y="302" text-anchor="middle" style="font-size:9.5px">320×u32 = 1.28 KB store</text>
  <line class="d-flow" x1="398" y1="162" x2="484" y2="162" marker-end="url(#rdA)" />
  <text class="d-sub" x="441" y="148" text-anchor="middle" style="font-size:9px">span list</text>
  <text class="d-sub" x="441" y="159" text-anchor="middle" style="font-size:9px">(start, count)</text>
  <text class="d-sub" x="556" y="46" text-anchor="middle" style="font-size:9px;fill:#6b7758">③ the push</text>
  <line x1="491" y1="58" x2="491" y2="149" stroke="#3c6b39" stroke-opacity="0.4" stroke-width="1.5" stroke-dasharray="2 3" />
  <line x1="491" y1="149" x2="491" y2="188" stroke="#cf6a2a" stroke-width="4" />
  <line x1="491" y1="188" x2="491" y2="266" stroke="#9aa884" stroke-opacity="0.3" stroke-width="1.2" stroke-dasharray="1 4" />
  <line x1="485" y1="192" x2="497" y2="192" stroke="#a9501c" stroke-width="1.6" />
  <rect x="496" y="58" width="108" height="208" rx="4" style="fill:#e7ead8;stroke:#3c6b39;stroke-width:1.2" />
  <rect x="500" y="59.5" width="100" height="11" fill="none" stroke="#9aa884" stroke-opacity="0.35" stroke-width="0.8" stroke-dasharray="2 3" />
  <rect x="500" y="72.5" width="100" height="11" fill="none" stroke="#9aa884" stroke-opacity="0.35" stroke-width="0.8" stroke-dasharray="2 3" />
  <rect x="500" y="85.5" width="100" height="11" fill="none" stroke="#9aa884" stroke-opacity="0.35" stroke-width="0.8" stroke-dasharray="2 3" />
  <rect x="500" y="98.5" width="100" height="11" fill="none" stroke="#9aa884" stroke-opacity="0.35" stroke-width="0.8" stroke-dasharray="2 3" />
  <rect x="500" y="111.5" width="100" height="11" fill="none" stroke="#9aa884" stroke-opacity="0.35" stroke-width="0.8" stroke-dasharray="2 3" />
  <rect x="500" y="124.5" width="100" height="11" fill="none" stroke="#9aa884" stroke-opacity="0.35" stroke-width="0.8" stroke-dasharray="2 3" />
  <rect x="500" y="137.5" width="100" height="11" fill="none" stroke="#9aa884" stroke-opacity="0.35" stroke-width="0.8" stroke-dasharray="2 3" />
  <rect x="500" y="150.5" width="100" height="11" fill="#cf6a2a" fill-opacity="0.55" />
  <rect x="500" y="163.5" width="100" height="11" fill="#cf6a2a" fill-opacity="0.55" />
  <rect x="500" y="176.5" width="100" height="11" fill="#cf6a2a" fill-opacity="0.55" />
  <rect x="500" y="189.5" width="100" height="11" fill="none" stroke="#9aa884" stroke-opacity="0.35" stroke-width="0.8" stroke-dasharray="2 3" />
  <rect x="500" y="202.5" width="100" height="11" fill="none" stroke="#9aa884" stroke-opacity="0.35" stroke-width="0.8" stroke-dasharray="2 3" />
  <rect x="500" y="215.5" width="100" height="11" fill="none" stroke="#9aa884" stroke-opacity="0.35" stroke-width="0.8" stroke-dasharray="2 3" />
  <rect x="500" y="228.5" width="100" height="11" fill="none" stroke="#9aa884" stroke-opacity="0.35" stroke-width="0.8" stroke-dasharray="2 3" />
  <rect x="500" y="241.5" width="100" height="11" fill="none" stroke="#9aa884" stroke-opacity="0.35" stroke-width="0.8" stroke-dasharray="2 3" />
  <rect x="500" y="254.5" width="100" height="11" fill="none" stroke="#9aa884" stroke-opacity="0.35" stroke-width="0.8" stroke-dasharray="2 3" />
  <text class="d-label" x="550" y="288" text-anchor="middle" style="font-size:10.5px">to glass</text>
  <text class="d-sub" x="550" y="302" text-anchor="middle" style="font-size:9.5px">3 rows pushed, rest retained</text>
  <text class="d-sub" x="614" y="101" style="font-size:9.5px;fill:#6b7758">fast-forward the gate</text>
  <text x="614" y="171" style="font-family:var(--mono);font-size:9.5px;fill:#a9501c">write the span</text>
  <text class="d-sub" x="614" y="204" style="font-size:9.5px;fill:#6b7758">stop early — rest not scanned</text>
  <rect x="250" y="324" width="360" height="28" rx="9" style="fill:#f8efe4;stroke:#cf6a2a;stroke-width:1.3" />
  <text x="430" y="342" text-anchor="middle" style="font-family:var(--sans);font-size:11.5px;fill:#a9501c">one-minute clock tick: <tspan font-weight="700">~44 ms full frame → a few ms</tspan></text>
</svg>
<figcaption>Screens stay <b>immediate-mode</b> — they clear and redraw the whole frame, so they never declare <i>where</i> they changed. The present works it out: one <b>32-bit hash per row</b> (320×u32 = 1.28 KB, word-folded FNV over avalanche-mixed words) compared against last frame, the changed rows coalesced into a single <b>span</b>, and one masked FLPR scan that fast-forwards its gate over the unchanged rows and <b>stops early</b>. A minute's clock tick then repaints a few rows, not a full ~44 ms frame — the rest of the picture is simply retained on the glass. A hash collision (a changed row skipped) is ~2⁻³² per change and self-heals; the simulator runs an exact full-frame diff as a CI oracle.</figcaption>
</figure>

### The overlay composites on the push

The hold-progress bulge — the little arc that swells as you hold a button — is never baked into the framebuffer. If it were, dismissing it would force a full map re-render just to paint it back out. Instead it **composites on the push**: over a static map the [input plane](../architecture/#staying-responsive-the-two-planes) re-presents *only* the region the bulge occupies, reading the clean framebuffer back as the backdrop and drawing the bulge over it — the resident frame stays the untouched map. The MIP/FLPR addresses that region in its native grain: the bulge's **rows**, through the span-masked scan above (fast-forward the gate to them, write just those, early-stop). And when the map *does* redraw mid-animation, the present goes *around* the bulge's rows so it never flashes off — the display seam's present takes the live bulge span as an exclude and clips its push with the same shared span logic (issues [#163](https://github.com/timohueser/OpenBikeComputer/issues/163), [#345](https://github.com/timohueser/OpenBikeComputer/issues/345)). Either way the map underneath is never touched and the expensive `render_map` stays asleep — [render-on-demand](../architecture/#the-per-frame-loop) taken to its limit: repaint the region that changed, and nothing else.

## Zero allocation, by budget

Everything above happens in fixed-size buffers owned by the renderer and **cleared, not freed, each frame** — so steady-state rendering does no heap work at all, which is what makes it safe on a microcontroller. The capacities are tuned for a 512 KB-RAM device and checked at compile time:

| Buffer | Holds | Capacity |
| :-- | :-- | --: |
| `frame_points` | every visible feature's vertices, concatenated | 12 288 |
| `frame_ring_lens` | per-feature ring lengths | 3 072 |
| `spans` | per-feature 14-byte draw records | 3 072 |
| `dec_points` | one feature's vertices during decode | 2 048 |
| `screen` | projected points for the feature being drawn | 4 096 |
| `xs` | scanline crossings for one row | 256 |

A compile-time assertion fails the build if the renderer's total buffer footprint grows past its RAM budget — so you can't accidentally blow the memory ceiling by bumping a constant. The frame buffers are the reason the priority multi-pass exists: they're deliberately *too small* for the densest views, and the priority order is what makes "too small" degrade gracefully instead of catastrophically.

---

## Where this lives

- The renderer and all its rasterisers: [`obc-render/src/`](src:firmware/obc-render/src) — the frame loop and buffers in `lib.rs`; projection, collection, stroking, polygon fill and the overlays in `viewport.rs` / `collect.rs` / `stroke.rs` / `fill.rs` / `overlay.rs`
- The streamed scene contract: [`obc-map-scene/src/lib.rs`](src:firmware/obc-map-scene/src/lib.rs); the production OBCM adapter: [`obc-reader/src/scene.rs`](src:firmware/obc-reader/src/scene.rs)
- The map parsing, quadtree walk, and skip-don't-decode: [`obc-reader/src/reader.rs`](src:firmware/obc-reader/src/reader.rs)
- A from-scratch reference walkthrough with `file:line` anchors: [`firmware/docs/rendering_pipeline.md`](src:firmware/docs/rendering_pipeline.md)

For how this renderer gets *driven* — the camera, the screen stack, and the per-frame loop — see [system architecture](../architecture/). For the map format it reads, see [data formats](../formats/).
