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
MapRenderer::render(target, reader, vp, bg, color_fn)
//                  │       │       │   │   └ RGB565 → this panel's pixel
//                  │       │       │   └ the backdrop colour
//                  │       │       └ the camera (Viewport)
//                  │       └ the parsed map (Reader)
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
  <text class="d-sub" x="232" y="100" text-anchor="end">reader · viewport · bg →</text>

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
<figcaption>The renderer is generic over a <b>DrawTarget</b> (the pixel sink) and takes a <b>colour function</b> (RGB565 → the panel's native pixel). The simulator plugs in a true-colour framebuffer; the device plugs in its 64-colour panel. Identical geometry code runs between them.</figcaption>
</figure>

Styles in the map store **device-independent RGB565**; the host's `color_fn` resolves each to a concrete pixel — true colour in the simulator, [64-colour RGB222 quantisation](src:firmware/obc-reader/src/color.rs) on the device. Because of this seam, the simulator you can [run in your browser](../../) is not a mock-up: it is the device's exact rendering code, so the two can never drift apart.

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

The camera is a [`Viewport`](src:firmware/obc-render/src/lib.rs) — a centre point in microdegrees, a zoom, an aspect correction for the latitude, and a rotation. Its hot path is `to_screen`, called once per vertex, so it's written to keep full precision while staying fast:

```rust
let delta_lon = lon.wrapping_sub(self.cam_lon);   // i32 µdeg, relative to camera
let delta_lat = lat.wrapping_sub(self.cam_lat);
let ex = (delta_lon as f32) * self.aspect;        // squash longitude by cos(lat)
let ny =  delta_lat as f32;
let rx = self.cos_c * ex - self.sin_c * ny;       // rotate to heading-up
let ry = -self.sin_c * ex - self.cos_c * ny;
let x = rx * self.zoom + self.w / 2.0;            // scale, centre
let y = ry * self.zoom + self.h / 2.0;
(roundf(x) as i32, roundf(y) as i32)              // round to nearest
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

Within a chosen LOD, geometry is bucketed into fixed-size **chunks**, indexed by a **quadtree** over the map's bounding box. To find the visible chunks, the renderer walks the tree from the root, descending only into children whose box intersects the view, and visits every non-empty leaf it reaches.

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
    return;                          // prune: out of range, too deep, or off-screen
}
let val = self.read_node(lod, idx);
if val & BRANCH_BIT == 0 {
    if val != EMPTY_LEAF {
        visit(val, node);            // a non-empty leaf = a chunk to decode
    }
    return;
}
// branch: child must advance (child > idx) — reject a corrupt back-reference —
// then split `node`'s bbox into NW/NE/SW/SE and recurse into each child
```

The `depth > MAX_DEPTH` bound and the `child > idx` check are pure robustness. A well-formed tree is only ~30 levels deep and always stores a branch's children *after* it, so neither ever fires on a real map — but a truncated or hostile `.obcm` off the SD card could otherwise point a branch back at itself and drive the walk into unbounded recursion. On the MCU there's no MMU guard page, so that's a stack overflow straight into a HardFault; bounding the depth makes the walk safe on any bytes. (This caps recursion *depth*, not the number of chunks visited — a different axis from the next paragraph.)

That "uncapped" property is load-bearing. An earlier version capped the visited chunks at a fixed number; a wide zoomed-out view overlaps far more leaves than the cap, so it silently dropped half the map *before* any importance logic could weigh in. Streaming the leaves through a callback instead means the decision about *what to drop* belongs entirely to the next stage — where it can be made by priority, not by accident.

## 4 · Decode by priority — the clever bit

Here is the central problem. A dense view holds far more geometry than the fixed frame buffers can hold. When they fill up, *something* must be dropped — and the dropped things must be the **least important features, globally**, no matter which chunk they live in. You never want to drop the coastline or a motorway because an unimportant forest patch in an early chunk got there first.

Two mechanisms work together to solve this within the memory and time budget.

**Skip, don't decode.** Each feature's style carries a 2-bit **priority** (1 = keep first … 4 = drop first). When the reader walks a chunk, it checks a feature's priority *before* touching its coordinates. If this isn't the feature we want right now, it advances past the bytes with pure offset arithmetic — no coordinate math, no buffer writes.

<figure class="fig">
<svg viewBox="0 0 720 168" role="img" aria-label="A chunk's byte stream is a row of feature cells tagged by priority. During the pass for priority 1, only priority-1 features are decoded; the rest are skipped by advancing the read pointer.">
  <text class="d-tag" x="20" y="24">Pass for priority 1 — decode P1, skip the rest</text>
  <!-- byte stream cells -->
  <g font-family="var(--mono)">
    <!-- cell template: x width 78, y 52 h 46 -->
    <rect x="24"  y="56" width="78" height="46" rx="6" class="d-hot-fill" /><text class="d-num" x="63" y="84" text-anchor="middle">P1</text>
    <rect x="110" y="56" width="78" height="46" rx="6" class="d-muted" /><text class="d-sub" x="149" y="84" text-anchor="middle">P3 skip</text>
    <rect x="196" y="56" width="78" height="46" rx="6" class="d-muted" /><text class="d-sub" x="235" y="84" text-anchor="middle">P4 skip</text>
    <rect x="282" y="56" width="78" height="46" rx="6" class="d-hot-fill" /><text class="d-num" x="321" y="84" text-anchor="middle">P1</text>
    <rect x="368" y="56" width="78" height="46" rx="6" class="d-muted" /><text class="d-sub" x="407" y="84" text-anchor="middle">P2 skip</text>
    <rect x="454" y="56" width="78" height="46" rx="6" class="d-muted" /><text class="d-sub" x="493" y="84" text-anchor="middle">P3 skip</text>
    <rect x="540" y="56" width="78" height="46" rx="6" class="d-hot-fill" /><text class="d-num" x="579" y="84" text-anchor="middle">P1</text>
    <rect x="626" y="56" width="70" height="46" rx="6" class="d-muted" /><text class="d-sub" x="661" y="84" text-anchor="middle">end</text>
  </g>
  <!-- read head -->
  <text class="d-sub" x="24" y="126">read head advances →</text>
  <line class="d-stroke" x1="24" y1="118" x2="696" y2="118" style="stroke:#cf6a2a;stroke-dasharray:2 5" />
  <text class="d-sub" x="24" y="150" style="font-size:11px">decoded features (coral) cost coordinate math; skipped ones cost only a pointer add.</text>
</svg>
<figcaption>A feature header is 12 bytes; skipping is pure offset arithmetic. So scanning a chunk four times — once per priority level — is cheap, because each scan only <i>decodes</i> the quarter of features it's responsible for.</figcaption>
</figure>

**The multi-pass.** The renderer makes four passes over the visible chunks, priority 1 first. Each pass fills the buffers with every visible feature *at that level, across all chunks*, before the next pass begins. So when the buffers saturate, whatever is left undrawn is — by construction — the lowest priority, wherever it lived.

```rust
for level in 1..=4u8 {
    self.collect_level(reader, lod, level, view, stats);
}
```

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
<figcaption>Because passes run low-number-first and each fills the buffers before the next starts, saturation drops strictly by priority across <i>all</i> chunks. Each feature matches exactly one level, so its coordinates are decoded only once per frame even though the chunks are walked four times.</figcaption>
</figure>

Every kept feature becomes a 14-byte **span** — a compact draw record that says *what* and *where* without copying the geometry again ([`Span`](src:firmware/obc-render/src/lib.rs)):

```rust
struct Span {
    kind: Kind,      // polygon or line
    z: i8,           // paint order
    weight: u8,      // line width
    color: u16,      // RGB565
    pt_start: u16,   // where its points sit in the frame buffer
    ring_start: u16,
    ring_count: u16,
    seq: u16,        // collection order — the stable-sort tiebreak
}
```

Thousands of spans can be buffered at coarse zoom, so they're kept small (`u16` offsets, not `usize`). The "walk the tree four times" cost is paid only on the cheap index data; the expensive decode is still strictly once-per-feature.

## 5 · Painter's order

With the visible features collected, the renderer sorts the spans — not the geometry, just the little records — into back-to-front draw order:

```rust
self.frame.spans.sort_unstable_by_key(|s| (s.z, s.seq));
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

Thin lines (≤ 2 px — most of the map's roads) are stroked by embedded-graphics directly; at that width its per-pixel cost is negligible. **Thick** lines — the route, the breadcrumb, fat road classes — take a different path, because a per-pixel thick-line rasteriser is exactly the trap above: it *generates* every pixel of the stroke one at a time, and that generation, not the writes, was measured to dominate the overlay. So a thick stroke is laid down through the **same scanline span fill as the polygons**: each segment becomes a rectangle (the segment swept ±half-width along its perpendicular), filled row-by-row as one `fill_solid` span apiece. No per-pixel plotting; the whole overlay rides the coalesced row blit.

Two filled rectangles butt-joined at a vertex leave a small notch on the outside of a bend and don't round the line's ends, so a disc the width of the stroke is filled (also as spans) to smooth each joint and cap. The disc is the overlay's biggest remaining cost, so it's spent only where it shows: at the two **run ends** (which round the cap and close the gap to the next feature at a chunk seam) and at interior vertices where the line **bends sharply**. The uncovered notch at a turn of `θ` off straight is only about `r·sin(θ/2)` deep (`r` = half the stroke width), so a gentle bend — or one of the synthetic points inserted to subdivide a long straight segment — leaves a sub-pixel notch and needs no disc at all.

## 7 · The overlays

The map underneath is the base; everything that moves with *you* is drawn on top, after the map, in a fixed order:

1. **Route** — the planned line (magenta), stroked through the same clip-then-stroke path, with direction **chevrons** laid down in a second sweep so they sit on top even where the route doubles back. Chevrons are spaced by *ground distance* derived from a fixed pixel cadence, so they stay evenly spread as you zoom and stay pinned to the ground as you pan.
2. **Breadcrumb** — the recorded trail behind you (navy), a two-tier coarse-spine-plus-recent-tail path.
3. **Marker** — a course-pointing chevron (or a stationary diamond) at your fix, a fixed screen size, culled when off-view.
4. **HUD chrome** — the off-route readout and pan-mode indicators.

Each is just another polyline through the stroker or a triangle through the polygon fill — and since a thick stroke is itself rectangles and discs filled as spans, nearly everything on screen comes down to the one scanline span fill, reused.

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

- The renderer and all its rasterisers: [`obc-render/src/lib.rs`](src:firmware/obc-render/src/lib.rs)
- The map parsing, quadtree walk, and skip-don't-decode: [`obc-reader/src/reader.rs`](src:firmware/obc-reader/src/reader.rs)
- A from-scratch reference walkthrough with `file:line` anchors: [`firmware/docs/rendering_pipeline.md`](src:firmware/docs/rendering_pipeline.md)

For how this renderer gets *driven* — the camera, the screen stack, and the per-frame loop — see [system architecture](../architecture/). For the map format it reads, see [data formats](../formats/).
