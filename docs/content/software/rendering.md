---
title: Rendering pipeline
description: How OpenBikeComputer selects, draws, and presents one map frame.
copy: ai
---

# The rendering pipeline

The renderer converts streamed map data into a 240×320-pixel frame. It uses fixed memory and does not allocate heap memory.

## Shared render path

[obc-render](src:firmware/obc-render) is a no_std crate. The simulator and device use the same geometry code.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
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
  <text class="d-sub" x="115" y="164" text-anchor="middle">RGB222 framebuffer</text>
  <text class="d-sub" x="115" y="178" text-anchor="middle">64 colours</text>
  <line class="d-flow" x1="244" y1="153" x2="192" y2="153" marker-end="url(#aS)" />

  <!-- host: device -->
  <rect class="d-panel" x="530" y="120" width="150" height="66" rx="11" />
  <text class="d-label" x="605" y="146" text-anchor="middle">device</text>
  <text class="d-sub" x="605" y="164" text-anchor="middle">LS021B7DD02 panel</text>
  <text class="d-sub" x="605" y="178" text-anchor="middle">64 colours (RGB222)</text>
  <line class="d-flow" x1="476" y1="153" x2="528" y2="153" marker-end="url(#aS)" />
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The renderer receives a map scene, a pixel target, and a color conversion function.</figcaption>
</figure>

The render call receives these inputs:

| Input | Purpose |
| --- | --- |
| MapScene | Supplies styles, LOD data, candidates, geometry, and diagnostics. |
| Viewport | Defines camera position, scale, rotation, and panel size. |
| RenderConfig | Selects per-frame presentation options. |
| DrawTarget | Receives pixels. |
| Color function | Converts RGB565 styles to target pixels. |
| RenderScratch | Supplies all per-frame work buffers. |

The [Reader adapter](src:firmware/obc-reader/src/scene.rs) streams OBCM chunks through MapScene. The interface does not expose file offsets or cache slots.

## Frame stages

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 820px">
<svg viewBox="0 0 820 236" role="img" aria-label="A frame's pipeline as a trail with seven waypoints: project, pick level of detail, quadtree cull, selection and decode, painter sort, rasterise, overlays — from map bytes to the panel.">
  <text class="d-tag" x="20" y="24">One frame, start to finish</text>

  <!-- trail -->
  <line x1="92" y1="120" x2="734" y2="120" stroke="#5f7d3d" stroke-width="2.5" stroke-dasharray="2 7" stroke-linecap="round" />

  <!-- start flag -->
  <circle cx="58" cy="120" r="7" class="d-forest" />
  <text class="d-sub" x="58" y="150" text-anchor="middle">map +</text>
  <text class="d-sub" x="58" y="168" text-anchor="middle">route bytes</text>

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
  <text class="d-label" x="422" y="160" text-anchor="middle" style="fill:#a9501c">Select + decode</text>
  <text class="d-sub" x="422" y="174" text-anchor="middle">complete features</text>
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
<text class="d-sub" x="20" y="212" text-anchor="start">Priority decides what fits; z-index decides paint order.</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>A frame selects visible data, draws it in z-order, and adds overlays.</figcaption>
</figure>

The frame uses these operations:

1. Use the viewport to select a level of detail and find visible chunks.
2. Select complete features against priority, point, and ring budgets.
3. Decode the selected geometry and project it to the screen.
4. Sort selected features by paint order.
5. Rasterize polygons and lines.
6. Draw route and rider overlays.

## Projection

The [Viewport](src:firmware/obc-render/src/viewport.rs) stores camera position, zoom, latitude correction, and rotation.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
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
  <text class="d-sub" x="624" y="120" style="font-size:12px">① Δ vs camera</text>
  <text class="d-sub" x="624" y="142" style="font-size:12px">② × cos(lat)</text>
  <text class="d-sub" x="624" y="164" style="font-size:12px">③ rotate</text>
  <text class="d-sub" x="624" y="186" style="font-size:12px">④ × zoom</text>
  <text class="d-sub" x="624" y="208" style="font-size:12px">⑤ round</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Projection keeps the camera delta precise. It then corrects longitude, rotates, scales, and rounds.</figcaption>
</figure>

to_screen keeps the camera delta as an integer before conversion to f32. It then corrects longitude, rotates, scales, and rounds.

to_map applies the inverse transform. Panning and viewport bounds use this operation.

## Level of detail

An OBCM file contains pre-simplified level-of-detail (LOD) tiers. Each tier specifies its maximum meters per pixel.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 300" role="img" aria-label="A level-of-detail pyramid: coarse tiers at the top are narrow (little, simplified geometry) and cover any zoom; fine tiers at the bottom are wide (dense detail) with a small meters-per-pixel range. A current view of 0.5 meters per pixel selects LOD 3, the finest tier whose range still covers it.">
  <defs>
    <marker id="aL" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">LOD selection · illustrative thresholds</text>

  <!-- detail axis (left) -->
  <text class="d-sub" x="58" y="86" text-anchor="middle">coarse</text>
  <text class="d-sub" x="58" y="230" text-anchor="middle">fine</text>

  <!-- tier 0 (top, narrowest) -->
  <polygon points="300,58 420,58 438,98 282,98" style="fill:#cfe0c2;stroke:#3c6b39;stroke-width:1.4" />
  <text class="d-label" x="360" y="76" text-anchor="middle" style="font-size:12px">LOD 0 · coarsest</text>
  <text class="d-sub" x="360" y="90" text-anchor="middle">covers any zoom</text>

  <!-- tier 1 -->
  <polygon points="282,102 438,102 458,142 262,142" style="fill:#c3dab4;stroke:#3c6b39;stroke-width:1.4" />
  <text class="d-label" x="360" y="120" text-anchor="middle" style="font-size:12px">LOD 1</text>
  <text class="d-sub" x="360" y="134" text-anchor="middle">good to ≤ 16 m/px</text>

  <!-- tier 2 -->
  <polygon points="262,146 458,146 480,186 240,186" style="fill:#b7d4a6;stroke:#3c6b39;stroke-width:1.4" />
  <text class="d-label" x="360" y="164" text-anchor="middle" style="font-size:12px">LOD 2</text>
  <text class="d-sub" x="360" y="178" text-anchor="middle">good to ≤ 4 m/px</text>

  <!-- tier 3 (bottom, widest) — the selected tier -->
  <polygon points="240,190 480,190 506,234 214,234" style="fill:#a9cd96;stroke:#cf6a2a;stroke-width:2.6" />
  <text class="d-label" x="360" y="208" text-anchor="middle" style="font-size:12px">LOD 3 · finest</text>
  <text class="d-sub" x="360" y="222" text-anchor="middle">good to ≤ 1 m/px</text>

  <!-- selector -->
  <line x1="646" y1="212" x2="500" y2="212" stroke="#cf6a2a" stroke-width="2" marker-end="url(#aL)" />
  <text class="d-label" x="652" y="206" style="fill:#a9501c">this view</text>
  <text class="d-sub" x="652" y="222">0.5 m/px</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The renderer selects the finest LOD that supports the current meters-per-pixel value.</figcaption>
</figure>

The renderer selects the finest supported tier. The selection depends on zoom and latitude.

The selection does not depend on display size. Equal geographic views select the same tier on all hosts.

## Visible chunks

Each LOD stores geometry in chunks. A quadtree indexes the chunks by geographic bounds.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
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
  <text class="d-sub" x="104" y="130" text-anchor="middle" style="font-size:12px">skipped</text>
  <text class="d-sub" x="104" y="244" text-anchor="middle">SW</text>
  <text class="d-sub" x="104" y="258" text-anchor="middle" style="font-size:12px">skipped</text>
  <text class="d-sub" x="290" y="64" text-anchor="end" style="font-size:12px">NE</text>
  <text class="d-sub" x="290" y="302" text-anchor="end" style="font-size:12px">SE</text>
  <!-- viewport (straddles the NE/SE boundary) -->
  <rect x="185" y="120" width="95" height="120" fill="none" stroke="#cf6a2a" stroke-width="2.4" />
  <text class="d-label" x="206" y="137" text-anchor="middle" style="fill:#a9501c">view</text>

  <!-- TREE (right) -->
  <text class="d-sub" x="545" y="42" text-anchor="middle" style="font-size:12px">root</text>
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
  <text class="d-sub" x="432" y="132" text-anchor="middle" style="font-size:12px">NW</text>
  <text class="d-sub" x="505" y="132" text-anchor="middle" style="font-size:12px">NE</text>
  <text class="d-sub" x="585" y="132" text-anchor="middle" style="font-size:12px">SW</text>
  <text class="d-sub" x="658" y="132" text-anchor="middle" style="font-size:12px">SE</text>
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
  <text class="d-sub" x="450" y="224" style="font-size:12px">visited leaf → a chunk to draw</text>
  <rect x="430" y="236" width="13" height="13" rx="3" class="d-muted" />
  <text class="d-sub" x="450" y="246" style="font-size:12px">skipped — bbox misses the view</text>
  <text class="d-sub" x="430" y="278" style="font-size:12px">a high bit marks a branch;</text>
  <text class="d-sub" x="430" y="292" style="font-size:12px">a sentinel marks an empty leaf</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The quadtree walk prunes nodes outside the viewport. It streams candidates from intersecting leaves.</figcaption>
</figure>

The reader descends only into nodes that intersect the viewport. It streams candidates from each nonempty leaf.

The walk limits recursion depth and rejects backward child references. These checks protect the device from invalid map data.

The walk does not limit the total number of visible chunks. The next stage applies the global feature budget.

## Feature selection

Dense views can exceed the frame buffers. Each style supplies a retention priority from 1 through 4.

Priority 1 has the highest retention priority. The z-index does not affect retention.

A 256-bit style mask removes hidden styles before geometry decode. The terrain-layer setting uses this mask.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
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
  <line class="d-stroke" x1="24" y1="112" x2="696" y2="112" style="stroke:#cf6a2a;stroke-dasharray:2 5" />
  <text class="d-sub" x="24" y="130">read head jumps offset → offset →</text>
  <text class="d-sub" x="24" y="152" style="font-size:12px">Pass B decodes the selected features. Other geometry is not decoded.</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Pass A stores candidate metadata and an opaque token. Pass B decodes selected candidates.</figcaption>
</figure>

Selection uses two passes:

- Pass A stores style, bounds, size, and an opaque source token.
- An in-memory selection admits candidates against point and ring budgets.
- Pass B decodes only admitted candidates into caller-owned buffers.

A higher-priority candidate can evict a lower-priority candidate. The decision applies across all visible chunks.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 412" role="img" aria-label="Four candidate shapes have priorities P1 to P4 and 4, 4, 3 and 4 points. A twelve-slot point budget retains P1, P2 and P3, using eleven slots. The last free slot cannot hold the complete P4 shape. Ring capacity is also checked.">
<defs><marker id="r31arrow" viewBox="0 0 10 10" refX="10" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker></defs>
<text class="d-tag" x="20" y="26" text-anchor="start">Feature budgets · preserve whole shapes, drop lower priorities</text>
<text class="d-title" x="20" y="60" text-anchor="start">Candidates · illustrative priorities</text>
<text class="d-title" x="420" y="60" text-anchor="start">Point budget · 12 slots</text>
<path d="M45 95 L160 91 L128 135 L62 137 Z" fill="none" stroke="#33575b" stroke-width="2"/>
<circle cx="45" cy="95" r="3" fill="#33575b"/>
<circle cx="160" cy="91" r="3" fill="#33575b"/>
<circle cx="128" cy="135" r="3" fill="#33575b"/>
<circle cx="62" cy="137" r="3" fill="#33575b"/>
<text class="d-sub" x="188" y="116" text-anchor="start">P1 · 4 points</text>
<path d="M42 185 L71 159 L110 186 L155 160" fill="none" stroke="#3c6b39" stroke-width="2"/>
<circle cx="42" cy="185" r="3" fill="#3c6b39"/>
<circle cx="71" cy="159" r="3" fill="#3c6b39"/>
<circle cx="110" cy="186" r="3" fill="#3c6b39"/>
<circle cx="155" cy="160" r="3" fill="#3c6b39"/>
<text class="d-sub" x="188" y="183" text-anchor="start">P2 · 4 points</text>
<path d="M46 226 L150 224 L119 268 Z" fill="none" stroke="#a96930" stroke-width="2"/>
<circle cx="46" cy="226" r="3" fill="#a96930"/>
<circle cx="150" cy="224" r="3" fill="#a96930"/>
<circle cx="119" cy="268" r="3" fill="#a96930"/>
<text class="d-sub" x="188" y="250" text-anchor="start">P3 · 3 points</text>
<path d="M45 317 L140 304 L153 335 L67 345 Z" fill="none" stroke="#9aa884" stroke-width="2"/>
<circle cx="45" cy="317" r="3" fill="#9aa884"/>
<circle cx="140" cy="304" r="3" fill="#9aa884"/>
<circle cx="153" cy="335" r="3" fill="#9aa884"/>
<circle cx="67" cy="345" r="3" fill="#9aa884"/>
<text class="d-sub" x="188" y="317" text-anchor="start">P4 · 4 points</text>
<rect x="420" y="82" width="51" height="36" fill="#33575b" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="445" y="105" text-anchor="middle" style="fill:#fff">P1</text>
<rect x="477" y="82" width="51" height="36" fill="#33575b" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="502" y="105" text-anchor="middle" style="fill:#fff">P1</text>
<rect x="534" y="82" width="51" height="36" fill="#33575b" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="559" y="105" text-anchor="middle" style="fill:#fff">P1</text>
<rect x="591" y="82" width="51" height="36" fill="#33575b" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="616" y="105" text-anchor="middle" style="fill:#fff">P1</text>
<rect x="420" y="126" width="51" height="36" fill="#3c6b39" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="445" y="149" text-anchor="middle" style="fill:#fff">P2</text>
<rect x="477" y="126" width="51" height="36" fill="#3c6b39" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="502" y="149" text-anchor="middle" style="fill:#fff">P2</text>
<rect x="534" y="126" width="51" height="36" fill="#3c6b39" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="559" y="149" text-anchor="middle" style="fill:#fff">P2</text>
<rect x="591" y="126" width="51" height="36" fill="#3c6b39" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="616" y="149" text-anchor="middle" style="fill:#fff">P2</text>
<rect x="420" y="170" width="51" height="36" fill="#a96930" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="445" y="193" text-anchor="middle" style="fill:#fff">P3</text>
<rect x="477" y="170" width="51" height="36" fill="#a96930" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="502" y="193" text-anchor="middle" style="fill:#fff">P3</text>
<rect x="534" y="170" width="51" height="36" fill="#a96930" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="559" y="193" text-anchor="middle" style="fill:#fff">P3</text>
<rect x="591" y="170" width="51" height="36" fill="#f3f0df" stroke="#3c6b39" stroke-width="1.2" />
<path d="M326 171 H405" fill="none" stroke="#3c6b39" stroke-width="1.5" marker-end="url(#r31arrow)"/>
<text class="d-sub" x="420" y="248" text-anchor="start">11 / 12 points used</text>
<text class="d-sub" x="420" y="270" text-anchor="start">One free slot cannot hold P4.</text>
<text class="d-sub" x="420" y="293" text-anchor="start">Keep the complete feature or drop it.</text>
<text class="d-sub" x="420" y="325" text-anchor="start">Ring capacity is checked too.</text>
<text class="d-sub" x="20" y="388" text-anchor="start">Priority decides admission across all visible chunks. Paint order is a separate sort.</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The point counts and capacity are illustrative. Selection compares complete candidates across all visible chunks; higher-priority candidates can displace lower-priority ones.</figcaption>
</figure>

The renderer drops an invalid or oversized feature as one unit. It does not publish partial geometry.

Each selected feature becomes a compact span. The span references points and rings in the frame buffers.

RenderStats reports budget drops, decode failures, malformed data, source failures, cache activity, and stage time.

## Paint order

The renderer sorts spans by z-index and collection sequence.

Priority controls feature retention. The z-index controls paint order. Collection sequence gives deterministic order for equal z-index values.

## Polygon fill

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
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
  <text class="d-sub" x="40" y="146" style="fill:#a9501c">scanline y + 0.5</text>
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
  <text class="d-sub" x="470" y="224" style="font-size:12px">A hole is just another ring in</text>
  <text class="d-sub" x="470" y="240" style="font-size:12px">the same crossing list — no</text>
  <text class="d-sub" x="470" y="256" style="font-size:12px">special case needed.</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The even-odd fill rule supports concave polygons and holes.</figcaption>
</figure>

The polygon filler uses the even-odd scanline rule. It sorts edge crossings for each row and fills between pairs.

The filler writes clipped horizontal rectangles. It skips a row if its crossing buffer is full.

## Line stroke

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
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
  <text class="d-sub" x="44" y="262" style="font-size:12px">only the clipped part is stroked</text>

  <!-- RIGHT: joints -->
  <text class="d-sub" x="430" y="66">butt-jointed rects (notch)</text>
  <polyline points="420,110 470,140 520,104 560,150" fill="none" stroke="#4f6b43" stroke-width="12" stroke-linejoin="bevel" stroke-linecap="butt" />
  <text class="d-sub" x="430" y="196">+ round-join / cap discs</text>
  <polyline points="420,224 470,254 520,218 560,264" fill="none" stroke="#4f6b43" stroke-width="12" stroke-linejoin="bevel" stroke-linecap="butt" />
  <g fill="#3c6b39"><circle cx="420" cy="224" r="6"/><circle cx="470" cy="254" r="6"/><circle cx="520" cy="218" r="6"/><circle cx="560" cy="264" r="6"/></g>
  <text class="d-sub" x="576" y="244" style="font-size:12px">a disc (⌀ = width) at</text>
  <text class="d-sub" x="576" y="260" style="font-size:12px">corners + run ends →</text>
  <text class="d-sub" x="576" y="276" style="font-size:12px">smooth arc, no gaps</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The stroker clips lines before rasterization. It removes subpixel duplicate points.</figcaption>
</figure>

Embedded Graphics strokes 1-pixel lines. The renderer converts wider segments to convex quadrilaterals and fills them as spans.

Discs close run ends and sharp joints. Normal line width changes with zoom and stays from 1 through 12 pixels.

Fixed-width styles bypass the zoom scale. Contours use this style property.

### Style combinations

| Feature | Dashed | color2 | Result |
| --- | --- | --- | --- |
| Line | No | None | Solid stroke |
| Line | No | Set | Road casing and road fill |
| Line | Yes | None | Dashed stroke |
| Line | Yes | Set | Solid base and dashed top stroke |
| Polygon | Ignored | Set | Fill and ring outline |

Dashed lines use screen-space arc length. The renderer measures the arc from the first point of the
line, and includes the parts that the view clip removes. Thus the dashes stay at the same map
positions when the camera moves. A railway style draws a solid color2 base and color dashes.

### Road casing

A road casing uses color2 and adds 2 pixels to the road width. Casings run only at the finest LOD.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 800px">
<svg viewBox="0 0 800 320" role="img" aria-label="Left: the frame's spans, sorted by z-index and drawn bottom-up — water, landuse and building fills at the bottom, then a dashed split line marking the first cased road line, then the road casings, then the road fills on top. The casing pass is inserted at that split: after the low-z fills, so they cannot paint over it, but under every road fill. Right: a junction where two roads cross — the road fills stay continuous through the crossing and the darker casing hugs only the outside of each road, with no casing line slicing across the junction.">
  <text class="d-tag" x="20" y="24">The casing pass goes at the z boundary — not before the frame</text>

  <!-- LEFT: the sorted span stack -->
  <text class="d-sub" x="150" y="52" text-anchor="middle">spans · sorted (z, seq) · drawn bottom-up</text>
  <text class="d-sub" x="42" y="186" text-anchor="middle" transform="rotate(-90 42 186)" style="font-size:12px">z ↑ · paint order ↑</text>

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
  <text class="d-sub" x="216" y="140" style="fill:#a9501c;font-size:12px">③ spans[split..) — road band, on top</text>
  <text class="d-sub" x="216" y="168" style="fill:#a9501c;font-size:12px">② casing pass — wide color2, finest LOD</text>
  <text class="d-sub" x="216" y="184" style="font-size:12px">split — first cased road line</text>
  <text class="d-sub" x="216" y="230" style="fill:#a9501c;font-size:12px">① spans[0..split) — the base pass</text>

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
  <text class="d-sub" x="620" y="284" text-anchor="middle" style="font-size:12px">fills continuous through the junction</text>
  <text class="d-sub" x="620" y="298" text-anchor="middle" style="font-size:12px">casing hugs the outside of each road only</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The casing pass starts at the road z-band. Road fills then cover casing inside intersections.</figcaption>
</figure>

The renderer draws casings at the start of the road z-band. It then draws all road fills.

This order keeps the casing above land fills. It also prevents casing lines inside road intersections.

### Polygon outlines

A polygon with color2 receives a closed outline at the finest LOD.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 760px">
<svg viewBox="0 0 760 300" role="img" aria-label="Top row, per-feature order (wrong): building A is filled and outlined, then building B's fill lands over the shared edge and erases A's wall there, then B is outlined — the two touching buildings merge into one block with no divider. Bottom row, per-z-group order (right): both buildings are filled first, then both are outlined — the shared middle wall is drawn last, after every fill, so it survives and the two buildings read as distinct.">
  <text class="d-tag" x="20" y="24">Outline after every fill in the z-group, so shared walls survive</text>

  <!-- TOP ROW: per-feature (wrong) -->
  <text class="d-label" x="30" y="70" style="fill:#a9501c">per feature</text>
  <text class="d-sub" x="30" y="84" style="font-size:12px">outline after each fill</text>

  <!-- frame 1: A filled + outlined -->
  <rect x="182" y="52" width="44" height="44" style="fill:#d6cda8;stroke:#cf6a2a;stroke-width:2.5" />
  <text class="d-sub" x="204" y="112" text-anchor="middle" style="font-size:12px">① fill + outline A</text>

  <!-- frame 2: B's fill lands over the shared edge -->
  <rect x="352" y="52" width="44" height="44" style="fill:#d6cda8" />
  <rect x="396" y="52" width="44" height="44" style="fill:#d6cda8" />
  <!-- A's surviving outer edges (coral), but the shared edge is covered by B's fill -->
  <path d="M352 52 h44 M352 96 h44 M352 52 v44" fill="none" stroke="#cf6a2a" stroke-width="2.5" />
  <line x1="399" y1="50" x2="399" y2="98" stroke="#c0492e" stroke-width="2" stroke-dasharray="3 3" />
  <text x="446" y="70" style="font-family:var(--mono);font-size:12px;fill:#c0492e">B's fill erased</text>
  <text x="446" y="88" style="font-family:var(--mono);font-size:12px;fill:#c0492e">A's shared wall</text>
  <text class="d-sub" x="374" y="112" text-anchor="middle" style="font-size:12px">② B's fill lands</text>

  <!-- frame 3: B outlined -> blob -->
  <rect x="600" y="52" width="88" height="44" style="fill:#d6cda8;stroke:#cf6a2a;stroke-width:2.5" />
  <text class="d-sub" x="644" y="112" text-anchor="middle" style="font-size:12px">③ one merged blob ✗</text>

  <!-- divider -->
  <line class="d-stroke" x1="30" y1="150" x2="730" y2="150" style="stroke:#9aa884;stroke-width:1" />

  <!-- BOTTOM ROW: per-z-group (right) -->
  <text class="d-label" x="30" y="192" style="fill:#3c6b39">per z-group</text>
  <text class="d-sub" x="30" y="206" style="font-size:12px">all fills, then outlines</text>

  <!-- frame 1: both fills, no outlines -->
  <rect x="182" y="176" width="44" height="44" style="fill:#d6cda8" />
  <rect x="226" y="176" width="44" height="44" style="fill:#d6cda8" />
  <text class="d-sub" x="226" y="236" text-anchor="middle" style="font-size:12px">① all fills first</text>

  <!-- frame 2: both outlined -> wall kept -->
  <rect x="374" y="176" width="44" height="44" style="fill:#d6cda8;stroke:#cf6a2a;stroke-width:2.5" />
  <rect x="418" y="176" width="44" height="44" style="fill:#d6cda8;stroke:#cf6a2a;stroke-width:2.5" />
  <line x1="418" y1="174" x2="418" y2="222" stroke="#cf6a2a" stroke-width="2.5" />
  <text class="d-sub" x="418" y="236" text-anchor="middle" style="font-size:12px">② all outlines</text>

  <!-- result -->
  <text x="560" y="196" style="font-family:var(--mono);font-size:12px;fill:#3c6b39">the shared wall is</text>
  <text x="560" y="210" style="font-family:var(--mono);font-size:12px;fill:#3c6b39">drawn after every fill</text>
  <text x="560" y="224" style="font-family:var(--mono);font-size:12px;fill:#3c6b39">→ two crisp buildings ✓</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The renderer fills every polygon in a z-group before it draws the outlines.</figcaption>
</figure>

The renderer fills all polygons in one z-group first. It then draws all outlines in that group.

This order keeps shared walls between adjacent buildings.

## Map overlays

The renderer draws moving map content after the base map:

1. Active route and direction chevrons
2. Breadcrumb trail
3. Waypoints and rider marker
4. Map status and tool indicators

The route and breadcrumb use the shared line stroker. Markers use the shared polygon filler.

## Frame storage and presentation

The device stores one RGB222 frame byte per pixel. The 240×320 frame uses 75 KiB.

Each byte has the 00_RR_GG_BB format. The framebuffer converts RGB565 pixels when it stores them.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 375" role="img" aria-label="Twelve schematic frame rows include a changed clock band. Equal row hashes are skipped; three adjacent changed hashes form one span. The panel scan skips to that span, writes it, then stops. The remaining rows retain their image.">
<defs><marker id="r36arrow" viewBox="0 0 10 10" refX="10" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker></defs>
<text class="d-tag" x="20" y="26" text-anchor="start">Row diff · hash each row, combine changes, write one span</text>
<text class="d-title" x="20" y="62" text-anchor="start">1 · Rendered frame</text>
<text class="d-title" x="254" y="62" text-anchor="start">2 · Compare row hashes</text>
<text class="d-title" x="525" y="62" text-anchor="start">3 · Panel scan</text>
<rect x="30" y="86" width="132" height="16" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="269" y="88" width="35" height="12" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<rect x="530" y="86" width="100" height="16" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="30" y="102" width="132" height="16" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="269" y="104" width="35" height="12" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<rect x="530" y="102" width="100" height="16" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="317" y="114" text-anchor="start">same</text>
<rect x="30" y="118" width="132" height="16" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="269" y="120" width="35" height="12" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<rect x="530" y="118" width="100" height="16" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="30" y="134" width="132" height="16" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="269" y="136" width="35" height="12" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<rect x="530" y="134" width="100" height="16" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="30" y="150" width="132" height="16" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" />
<rect x="269" y="152" width="35" height="12" fill="#cf6a2a" stroke="#3c6b39" stroke-width="1.2" />
<rect x="530" y="150" width="100" height="16" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="317" y="162" text-anchor="start">changed</text>
<rect x="30" y="166" width="132" height="16" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" />
<rect x="269" y="168" width="35" height="12" fill="#cf6a2a" stroke="#3c6b39" stroke-width="1.2" />
<rect x="530" y="166" width="100" height="16" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="317" y="178" text-anchor="start">changed</text>
<rect x="30" y="182" width="132" height="16" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" />
<rect x="269" y="184" width="35" height="12" fill="#cf6a2a" stroke="#3c6b39" stroke-width="1.2" />
<rect x="530" y="182" width="100" height="16" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="317" y="194" text-anchor="start">changed</text>
<rect x="30" y="198" width="132" height="16" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="269" y="200" width="35" height="12" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<rect x="530" y="198" width="100" height="16" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="30" y="214" width="132" height="16" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="269" y="216" width="35" height="12" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<rect x="530" y="214" width="100" height="16" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="30" y="230" width="132" height="16" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="269" y="232" width="35" height="12" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<rect x="530" y="230" width="100" height="16" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="317" y="242" text-anchor="start">same</text>
<rect x="30" y="246" width="132" height="16" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="269" y="248" width="35" height="12" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<rect x="530" y="246" width="100" height="16" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="30" y="262" width="132" height="16" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="269" y="264" width="35" height="12" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<rect x="530" y="262" width="100" height="16" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="95" y="180" text-anchor="middle">clock</text>
<path d="M174 176 H251" fill="none" stroke="#3c6b39" stroke-width="1.5" marker-end="url(#r36arrow)"/>
<path d="M394 150 H402 V198 H394" fill="none" stroke="#cf6a2a" stroke-width="1.5" />
<path d="M410 176 H514" fill="none" stroke="#3c6b39" stroke-width="1.5" marker-end="url(#r36arrow)"/>
<text class="d-sub" x="423" y="154" text-anchor="middle">span</text>
<path d="M520 86 L520 150" fill="none" stroke="#9aa884" stroke-width="1.3" stroke-dasharray="3 4"/>
<path d="M520 150 L520 198" fill="none" stroke="#cf6a2a" stroke-width="3"/>
<text class="d-sub" x="638" y="117" text-anchor="start">skip</text>
<text class="d-sub" x="638" y="176" text-anchor="start">write</text>
<text class="d-sub" x="638" y="222" text-anchor="start">stop</text>
<text class="d-sub" x="20" y="310" text-anchor="start">12 rows shown; real frame: 320</text>
<text class="d-sub" x="265" y="310" text-anchor="start">320 × u32 = 1,280 bytes</text>
<text class="d-sub" x="524" y="310" text-anchor="start">Other rows retained</text>
<text class="d-sub" x="20" y="350" text-anchor="start">Adjacent changed rows form one span (start, count). Unchanged rows do not need a pixel write.</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The presenter stores one u32 hash for each of the 320 panel rows. It combines adjacent changes into spans and sends only those rows to the panel.</figcaption>
</figure>

The LS021 presenter hashes each row. It sends the changed row spans through the FLPR coprocessor.

The M33 renders the frame and publishes dirty rows. The FLPR reads shared SRAM and writes the panel wire format.

The simulator implements the same display contracts. Its final presenter writes changed rows to the host texture.

### Transient overlays

A transient overlay is not stored in the clean base frame. The presenter reads the required base-frame window and composites the overlay.

Clearing the overlay presents the clean window again. It does not require a map render.

A base-frame update can exclude a live overlay region. This rule prevents overlay flicker during a map update.

## Memory budgets

RenderScratch contains fixed-capacity buffers. The device initializes it in place in the shared scratch arena.

| Buffer | Purpose | Capacity |
| --- | --- | ---: |
| Frame points | Selected projected vertices | 16,323 |
| Frame ring lengths | Selected feature rings | 3,328 |
| Candidate spans | Candidate and draw records | 3,072 |
| Decode points | One decoded feature | 2,048 |
| Screen points | One drawn feature | 2,048 |
| Scanline crossings | One polygon row | 384 |

The board build checks the complete scratch size against its arena budget. Increasing a capacity is a device-memory decision.

## Landmark photo preparation

A selected landmark photo uses the same resident frame as the map. The screen stores the map generation, record index, QID, title and render state. It does not store image pixels or a decoder.

The base draw clears the photo rectangle and draws its header. A separate mutable phase then reads the selected photo through the normal map reader. Each step reads at most 256 compressed bytes and produces at most 4,096 pixels. Header and record reads also remain bounded. The board borrows its shared scratch arena for one step and releases it before presentation or any await. No source or frame reference survives the step.

The next step preserves completed pixels. A full base redraw, a fresh capture or a return from a covering page starts the photo again. An ordinary drawer preserves the prepared photo and suspends its work. When a fresh frame or a drawer transition requires the base to be rebuilt, the shared frame pipeline prepares photo pixels before it draws the drawer. Interactive reconstruction advances one bounded step per pass; the drawer is composed after each step. A fresh headless capture finishes the background photo before it composes the drawer. If another arena user replaces the decoder state, the next photo step clears the rectangle and starts again. A map generation or QID mismatch removes the previous image. An unreadable photo shows **Photo unavailable**; an absent photo shows **No photo available**.

The simulator and browser use the same preparation phase. Interactive hosts advance one step per frame. A fresh headless capture runs bounded steps until the image is complete or unavailable. Text pages, Visit actions and Sources navigation use the content screen's own controls.

## Source map

- Renderer and scratch budgets: [lib.rs](src:firmware/obc-render/src/lib.rs)
- Projection: [viewport.rs](src:firmware/obc-render/src/viewport.rs)
- Collection and selection: [collect.rs](src:firmware/obc-render/src/collect.rs)
- Polygon and line rasterization: [fill.rs](src:firmware/obc-render/src/fill.rs), [stroke.rs](src:firmware/obc-render/src/stroke.rs)
- Streamed map contract: [obc-map-scene](src:firmware/obc-map-scene/src/lib.rs)
- OBCM adapter and quadtree walk: [scene.rs](src:firmware/obc-reader/src/scene.rs), [reader/mod.rs](src:firmware/obc-reader/src/reader/mod.rs)
- Frame and presenters: [display contracts](src:firmware/obc-display/src/display_contracts), [LS021](src:firmware/obc-display/src/ls021)

See [system architecture](../architecture/) for the host loop. See [data formats](../formats/) for the OBCM format.
