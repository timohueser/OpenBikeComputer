---
title: Rendering pipeline
description: How OpenBikeComputer selects, draws, and presents one map frame.
---

# The rendering pipeline

The renderer turns streamed map data into one 240 × 320 frame. It uses fixed buffers and allocates
no memory, so a dense city cannot cost more memory than open country.

## Shared render path

[`obc-render`](src:firmware/obc-render) is a `no_std` crate. The device, the simulator, and the
browser draw with the same geometry code, so a frame on a desktop is the frame the panel shows.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<img src="../../assets/diagrams/software-rendering-01.svg" alt="The shared render code in the middle connects through two pluggable seams — a DrawTarget for pixels and a colour function — to two hosts: the simulator and the device." data-inline-svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The renderer receives a map scene, a pixel target, and a color conversion function.</figcaption>
</figure>

A render call receives the map scene, the viewport, the presentation options, a draw target, a
color function, and the scratch buffers. The
[reader adapter](src:firmware/obc-reader/src/scene.rs) streams map chunks through the scene
interface, which exposes no file offsets and no cache slots, so the renderer does not know what
storage it draws from.

## Frame stages

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 820px">
<img src="../../assets/diagrams/software-rendering-02.svg" alt="A frame's pipeline as a trail with seven waypoints: project, pick level of detail, quadtree cull, selection and decode, painter sort, rasterise, overlays — from map bytes to the panel." data-inline-svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>A frame selects visible data, draws it in z-order, and adds overlays.</figcaption>
</figure>

Each stage narrows the work: the selection stages decide what is drawn, and only what survives them
is decoded, projected, and rasterized.

## Projection

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<img src="../../assets/diagrams/software-rendering-03.svg" alt="Ground coordinates in microdegrees, relative to the camera, are squashed by cosine of latitude, rotated to heading-up, scaled by zoom and centred, then rounded to the nearest pixel." data-inline-svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Projection keeps the camera delta precise. It then corrects longitude, rotates, scales, and rounds.</figcaption>
</figure>

[`Viewport`](src:firmware/obc-render/src/viewport.rs) keeps the camera position, zoom, latitude
correction, and rotation. The transform holds the offset from the camera as an integer before it
converts to floating point, so precision does not fall away far from the origin.

## Level of detail

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<img src="../../assets/diagrams/software-rendering-04.svg" alt="A level-of-detail pyramid: coarse tiers at the top are narrow (little, simplified geometry) and cover any zoom; fine tiers at the bottom are wide (dense detail) with a small meters-per-pixel range. A current view of 0.5 meters per pixel selects LOD 3, the finest tier whose range still covers it." data-inline-svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The renderer selects the finest LOD that supports the current meters-per-pixel value.</figcaption>
</figure>

The renderer selects the finest level whose stated scale still supports the current
meters-per-pixel value. The selection uses zoom and latitude only, never the panel size, so the
same geographic view selects the same level on every host.

## Visible chunks

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<img src="../../assets/diagrams/software-rendering-05.svg" alt="A region split into four quadrants. The viewport straddles the boundary between the north-east and south-east quadrants, so the walk descends into both and prunes the north-west and south-west quadrants whole. Within north-east only the two lower sub-cells meet the view; within south-east only the two upper sub-cells do — four visited leaves in all. The tree on the right mirrors this: the root descends into the NE and SE branches, each with two visited and two pruned leaves, while NW and SW are pruned." data-inline-svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The quadtree walk prunes nodes outside the viewport. It streams candidates from intersecting leaves.</figcaption>
</figure>

The reader walks the quadtree and descends only into nodes that meet the viewport. It limits the
depth and refuses a backward child reference, because a damaged map must not be able to make the
device loop. It does not limit how many chunks it returns: the next stage owns the budget.

## Feature selection

A dense view holds more geometry than the frame buffers. Each style carries a retention priority,
and a higher-priority feature can displace a lower-priority one across the whole view. Priority
decides what survives; paint order decides what covers what.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<img src="../../assets/diagrams/software-rendering-06.svg" alt="A chunk's byte stream is a row of feature cells. The OBCM adapter resolves each winning opaque token to its source position and seeks straight to that feature, skipping everything in between by advancing the read pointer." data-inline-svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Pass A stores candidate metadata and an opaque token. Pass B decodes selected candidates.</figcaption>
</figure>

Selection runs in two passes. The first stores only the style, bounds, size, and an opaque token
for each candidate; the budget is applied in memory; the second decodes only what was admitted.
Decoding is the expensive step, so nothing is decoded to be thrown away.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<img src="../../assets/diagrams/software-rendering-07.svg" alt="Four candidate shapes have priorities P1 to P4 and 4, 4, 3 and 4 points. A twelve-slot point budget retains P1, P2 and P3, using eleven slots. The last free slot cannot hold the complete P4 shape. Ring capacity is also checked." data-inline-svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The point counts and capacity are illustrative. Selection compares complete candidates across all visible chunks; higher-priority candidates can displace lower-priority ones.</figcaption>
</figure>

A feature is dropped as one unit: the renderer never publishes part of a polygon. `RenderStats`
reports drops, decode failures, malformed data, cache activity, and stage time, which is how a map
that draws badly is diagnosed without a debugger.

## Paint order

Spans are sorted by paint order, and equal values keep collection order, so a frame is
deterministic.

## Polygon fill

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<img src="../../assets/diagrams/software-rendering-08.svg" alt="A polygon with a square hole on a pixel grid. A horizontal scanline crosses the outer edge twice and the hole twice; the fill covers the two outer spans and leaves the hole empty, because the even-odd rule pairs the four crossings as two filled spans." data-inline-svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The even-odd fill rule supports concave polygons and holes.</figcaption>
</figure>

The filler uses the even-odd scanline rule, which handles concave rings and holes with one rule and
no separate hole test.

## Line stroke

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<img src="../../assets/diagrams/software-rendering-09.svg" alt="On the left, a long route crosses a small viewport; only the visible portion is stroked while the off-screen majority costs nothing. On the right, a thick line drawn as a chain of filled rectangles leaves a notch at each joint, smoothed by filling a disc at the run ends and at the vertices where the line bends sharply." data-inline-svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The stroker clips lines before rasterization. It removes subpixel duplicate points.</figcaption>
</figure>

One-pixel lines go through Embedded Graphics. A wider segment becomes a convex quadrilateral filled
as spans, and discs close the ends and sharp joints. Line width follows zoom, except for styles
fixed at one pixel, such as contours.

A dashed line measures its pattern along the screen arc from the first point of the line, including
the part the view clip removed, so the dashes stay at the same place on the map while the camera
moves.

### Road casing

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 800px">
<img src="../../assets/diagrams/software-rendering-10.svg" alt="Left: the frame's spans, sorted by z-index and drawn bottom-up — water, landuse and building fills at the bottom, then a dashed split line marking the first cased road line, then the road casings, then the road fills on top. The casing pass is inserted at that split: after the low-z fills, so they cannot paint over it, but under every road fill. Right: a junction where two roads cross — the road fills stay continuous through the crossing and the darker casing hugs only the outside of each road, with no casing line slicing across the junction." data-inline-svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The casing pass starts at the road z-band. Road fills then cover casing inside intersections.</figcaption>
</figure>

A casing is the darker outline of a road. The renderer draws every casing at the start of the road
paint band and then every road fill, so a casing stays above the land fill and no casing line
crosses an intersection.

### Polygon outlines

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 760px">
<img src="../../assets/diagrams/software-rendering-11.svg" alt="Top row, per-feature order (wrong): building A is filled and outlined, then building B's fill lands over the shared edge and erases A's wall there, then B is outlined — the two touching buildings merge into one block with no divider. Bottom row, per-z-group order (right): both buildings are filled first, then both are outlined — the shared middle wall is drawn last, after every fill, so it survives and the two buildings read as distinct." data-inline-svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The renderer fills every polygon in a z-group before it draws the outlines.</figcaption>
</figure>

For the same reason, the renderer fills every polygon of a paint group before it outlines them,
which keeps one shared wall between two adjacent buildings.

## Map overlays

After the base map come the active route and its chevrons, the breadcrumb trail, the waypoints and
the rider marker, and the status indicators, through the same stroker and filler as the map.

## Frame storage and presentation

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<img src="../../assets/diagrams/software-rendering-12.svg" alt="Twelve schematic frame rows include a changed clock band. Equal row hashes are skipped; three adjacent changed hashes form one span. The panel scan skips to that span, writes it, then stops. The remaining rows retain their image." data-inline-svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The presenter stores one u32 hash for each of the 320 panel rows. It combines adjacent changes into spans and sends only those rows to the panel.</figcaption>
</figure>

The device holds one frame byte per pixel in the panel's own 64-color gamut. The presenter hashes
each panel row, joins the changed rows into spans, and sends only those. A partial update is what
makes the panel cheap: an unchanged row costs no transfer and no scan time. The simulator
implements the same display contracts, so its output is the same frame.

A transient overlay, such as hold feedback, is not stored in the clean base frame. The presenter
reads the base window, composites the overlay, and sends the result, so clearing the overlay needs
no map render.

## Landmark and peak photos

A photo in an article is decoded into the same resident frame as the map, in bounded steps. There
is no image buffer and no resident decoder: each step writes more pixels into the frame and
releases the shared work area before it presents, so a photo never delays a gesture. An interrupted
photo starts again; an unreadable one leaves the text and credits available.

## Source map

- Renderer and scratch budgets: [`lib.rs`](src:firmware/obc-render/src/lib.rs)
- Projection: [`viewport.rs`](src:firmware/obc-render/src/viewport.rs)
- Collection and selection: [`collect.rs`](src:firmware/obc-render/src/collect.rs)
- Polygon and line rasterization: [`fill.rs`](src:firmware/obc-render/src/fill.rs), [`stroke.rs`](src:firmware/obc-render/src/stroke.rs)
- Streamed map contract: [`obc-map-scene`](src:firmware/obc-map-scene/src/lib.rs)
- OBCM adapter and quadtree walk: [`scene.rs`](src:firmware/obc-reader/src/scene.rs)
- Frame and presenters: [display contracts](src:firmware/obc-display/src/display_contracts), [LS021](src:firmware/obc-display/src/ls021)

See [system architecture](../architecture/) for the host loop and [data formats](../formats/) for
the map format.
