---
title: Packer & routing
description: How OpenStreetMap data becomes a device-ready OBCM map (the obc-pack pipeline), and how your uploaded route is followed — converted to OBCR and map-matched to your live position as you ride.
---

# Packer & routing

Two jobs bracket the device's own work. **Packing** turns raw OpenStreetMap data into a styled `.obcm` map — a heavy job, run once on a computer, and (as of v8) it also bakes a [routable navigation graph](#building-the-navigation-graph) into the map — one that v9 makes [bike-type-aware](#weighting-the-graph-bike-profiles). **Routing** is the lighter on-device work: turning a GPX you upload into a navigable `.obcr`, **map-matching** your live position onto it as you ride, and — new this iteration — **computing** a route to a POI on the device itself over that baked graph. So "routing" here is mostly *following* a line you brought, with a memory-bounded bit of *pathfinding* when you ask the device to reach a POI on its own.

The packer ([`obc-pack`](src:host/obc-pack)) lives in the same Rust workspace as the device firmware and depends on the same [`obc-reader`](src:firmware/obc-reader), so the program that *writes* the format and the program that *reads* it can never disagree about a byte.

## Packing a map

The pipeline is a straight line from an `.osm.pbf` extract to a finished `.obcm`. Two stages carry the weight — ingest and the per-LOD build — and the rest are quick.

<figure class="fig">
<svg viewBox="0 0 820 240" role="img" aria-label="The packer pipeline as a trail: starting from one or more OSM .pbf files plus a config, the stages are ingest (which also merges and crops), compute bounding box, generate land, trace contours from terrain, build the per-LOD pyramid (simplify then quadtree), and serialize, ending at a .obcm file. Ingest and the per-LOD build are marked as the expensive stages; contour tracing only runs when the config asks for it and terrain was supplied.">
  <text class="d-tag" x="20" y="24">From OpenStreetMap to a device map</text>

  <!-- trail -->
  <line x1="96" y1="120" x2="742" y2="120" stroke="#5f7d3d" stroke-width="2.5" stroke-dasharray="2 7" stroke-linecap="round" />

  <!-- start -->
  <circle cx="58" cy="120" r="7" class="d-forest" />
  <text class="d-sub" x="58" y="150" text-anchor="middle">.pbf(s) +</text>
  <text class="d-sub" x="58" y="162" text-anchor="middle">config</text>

  <!-- 1 ingest (below, HOT) — merge + crop happen inside it -->
  <circle cx="150" cy="120" r="16" class="d-hot-fill" /><text class="d-num" x="150" y="124" text-anchor="middle">1</text>
  <text class="d-label" x="150" y="160" text-anchor="middle" style="fill:#a9501c">Ingest</text>
  <text class="d-sub" x="150" y="174" text-anchor="middle">ways · relations</text>
  <text class="d-sub" x="150" y="188" text-anchor="middle">merge · crop</text>
  <!-- 2 bbox (above) -->
  <circle cx="254" cy="120" r="15" class="d-forest" /><text class="d-num" x="254" y="124" text-anchor="middle">2</text>
  <text class="d-label" x="254" y="74" text-anchor="middle">BBox</text>
  <text class="d-sub" x="254" y="88" text-anchor="middle">truncate µdeg</text>
  <!-- 3 land (below) -->
  <circle cx="358" cy="120" r="15" class="d-forest" /><text class="d-num" x="358" y="124" text-anchor="middle">3</text>
  <text class="d-label" x="358" y="160" text-anchor="middle">Land</text>
  <text class="d-sub" x="358" y="174" text-anchor="middle">clip to bbox</text>
  <!-- 4 contours (above) — only with terrain + a config that asks -->
  <circle cx="462" cy="120" r="15" class="d-forest" /><text class="d-num" x="462" y="124" text-anchor="middle">4</text>
  <text class="d-label" x="462" y="74" text-anchor="middle">Contours</text>
  <text class="d-sub" x="462" y="88" text-anchor="middle">trace · clamp 15 m</text>
  <!-- 5 per-LOD (below, HOT) -->
  <circle cx="566" cy="120" r="16" class="d-hot-fill" /><text class="d-num" x="566" y="124" text-anchor="middle">5</text>
  <text class="d-label" x="566" y="160" text-anchor="middle" style="fill:#a9501c">Per-LOD</text>
  <text class="d-sub" x="566" y="174" text-anchor="middle">simplify → quadtree</text>
  <!-- 6 serialize (above) -->
  <circle cx="670" cy="120" r="15" class="d-forest" /><text class="d-num" x="670" y="124" text-anchor="middle">6</text>
  <text class="d-label" x="670" y="74" text-anchor="middle">Serialize</text>
  <text class="d-sub" x="670" y="88" text-anchor="middle">stream out</text>

  <!-- end -->
  <rect class="d-panel" x="772" y="104" width="40" height="32" rx="5" style="fill:#e7ead8" />
  <text class="d-sub" x="792" y="124" text-anchor="middle" style="font-size:9px">.obcm</text>
</svg>
<figcaption>There is no separate merge or crop stage: giving the packer several regions, or <a href="#cropping-to-a-box">a box</a> to cut them down to, changes what ingest reads rather than adding a step in front of it — which is why the whole pipeline needs no external tool at all. Stages 3 and 4 both <i>generate</i> features rather than read them, and both do it before any tier exists, so what they make is cut and simplified like everything else. Each LOD tier is then built and streamed to disk before the next begins, so peak memory is roughly <i>one</i> tier's quadtree rather than the whole pyramid plus the output — the same "never resident if it doesn't have to be" instinct the device's reader uses.</figcaption>
</figure>

### Styling: first match wins

What a feature *is* — and whether it's kept at all — comes from a `config.json`. It's an **ordered** map of `tag_key → value → style`, and a way is styled by the **first** rule (in document order) whose tag it carries. Style IDs are assigned 1-based in that same document order; the config never names them.

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="A way's tags, highway=primary and building=yes, are matched against the config rules in document order: highway comes before building, so the highway=primary rule wins and produces a style with id 5, a colour, a z-index, a priority, and a min-LOD.">
  <defs>
    <marker id="aP2" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">First matching rule, in document order</text>

  <!-- tags -->
  <rect class="d-panel-2" x="32" y="78" width="150" height="86" rx="10" />
  <text class="d-label" x="48" y="100">a way's tags</text>
  <text class="d-sub" x="48" y="124" font-family="var(--mono)">highway = primary</text>
  <text class="d-sub" x="48" y="144" font-family="var(--mono)">building = yes</text>

  <line class="d-flow" x1="186" y1="120" x2="232" y2="120" marker-end="url(#aP2)" />

  <!-- config rules -->
  <rect class="d-panel" x="240" y="48" width="244" height="160" rx="10" />
  <text class="d-tag" x="256" y="68">config.json · in order</text>
  <g font-family="var(--mono)">
    <rect x="252" y="78" width="220" height="24" rx="5" class="d-hot-fill" />
    <text class="d-sub" x="262" y="94" style="fill:#fff">highway → { primary ★ … }</text>
    <text class="d-sub" x="262" y="120">railway → { … }</text>
    <text class="d-sub" x="262" y="142">natural → { water, land … }</text>
    <text class="d-sub" x="262" y="164">building → { yes … }</text>
    <text class="d-sub" x="262" y="186">admin_level → { 2 … }</text>
  </g>
  <text class="d-sub" x="362" y="226" text-anchor="middle" style="font-size:9px">first key the way carries wins — building is never reached</text>

  <line class="d-flow" x1="488" y1="92" x2="540" y2="92" marker-end="url(#aP2)" />

  <!-- style out -->
  <rect class="d-hot" x="548" y="52" width="150" height="140" rx="10" style="fill:#f8efe4" />
  <text class="d-label" x="564" y="72" style="fill:#a9501c">style #5</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="564" y="94">color (RGB565)</text>
    <text class="d-sub" x="564" y="112">z_index · weight</text>
    <text class="d-sub" x="564" y="130">priority 1–4</text>
    <text class="d-sub" x="564" y="148">min_lod</text>
    <text class="d-sub" x="564" y="166">line_style · color2</text>
  </g>
</svg>
<figcaption>A style carries everything the renderer later needs: a colour, a paint order (<code>z_index</code>), a line weight, a drop-priority, a <code>min_lod</code> (the zoom tier below which the feature isn't included), and — since <b>v10</b> — a <code>line_style</code> and an optional secondary colour <code>color2</code> for dashes, casings, stripes and outlines. These become the <a href="../formats/#the-header">style table</a> in the file, and the colours resolve through the very same <a href="../architecture/#two-hosts-one-core-and-the-seams-between-them"><code>color_fn</code></a> the UI uses.</figcaption>
</figure>

```rust
pub fn get_style(&self, tags: &HashMap<&str, &str>) -> Option<&FeatureStyle> {
    for (tag_key, by_value) in &self.features {   // walked in document order
        if let Some(val) = tags.get(tag_key.as_str()) {
            // exact value first, then the category's "*" catch-all
            if let Some(style) = by_value.get(*val).or_else(|| by_value.get("*")) {
                return Some(style);               // first match wins
            }
        }
    }
    None                                          // unstyled → dropped
}
```

Within a `tag_key`, the value `"*"` is a **catch-all**: an exact value match still wins, but any other value that key carries falls back to the `"*"` rule. So `building → { warehouse: …, "*": … }` gives warehouses their own style and paints every other `building=*` with the catch-all — without enumerating OSM's ~50 building values by hand. The catch-all is an ordinary rule (it takes one style ID like any other), so it's purely a packer-side convenience; the file format and the device never know it existed.

### Ingest: two passes, then assemble

OSM is nodes, ways and relations, stored in that order. The ingester reads the `.pbf` twice. **Pass 1** builds a `node id → coordinate` store and notes which *area relations* exist (lakes-with-islands, multi-part forests). **Pass 2** turns ways into lines and polygons — and captures the geometry of any way a relation needs. Then each relation's member ways are assembled into a polygon-with-holes. (Cropping to a box adds a third pass in front of these — [below](#cropping-to-a-box); several regions are read by these same passes — [below](#merging-several-regions).)

<figure class="fig">
<svg viewBox="0 0 720 280" role="img" aria-label="Ingest in two passes. Pass 1 reads the pbf into a node store and collects area relations. Pass 2 turns ways into lines and closed-way polygons and coastlines, capturing member geometry. Then relation member ways are assembled via build_area into a polygon with a hole — a lake with an island.">
  <defs>
    <marker id="aP3" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Two passes over the .pbf, then relation assembly</text>

  <!-- pbf -->
  <rect class="d-panel" x="30" y="96" width="70" height="60" rx="9" />
  <text class="d-sub" x="65" y="130" text-anchor="middle">.pbf</text>

  <!-- pass 1 -->
  <line class="d-flow" x1="104" y1="112" x2="150" y2="92" marker-end="url(#aP3)" />
  <rect class="d-panel-2" x="156" y="56" width="200" height="60" rx="9" />
  <text class="d-label" x="172" y="76">Pass 1</text>
  <text class="d-sub" x="172" y="94">node store · id → coord</text>
  <text class="d-sub" x="172" y="108">+ collect area relations</text>

  <!-- pass 2 -->
  <line class="d-flow" x1="104" y1="140" x2="150" y2="160" marker-end="url(#aP3)" />
  <rect class="d-panel-2" x="156" y="140" width="200" height="76" rx="9" />
  <text class="d-label" x="172" y="160">Pass 2</text>
  <text class="d-sub" x="172" y="178">ways → lines · polygons</text>
  <text class="d-sub" x="172" y="194">coastlines (always)</text>
  <text class="d-sub" x="172" y="210">capture member geometry</text>

  <!-- assemble -->
  <line class="d-flow" x1="360" y1="135" x2="412" y2="135" marker-end="url(#aP3)" />
  <rect class="d-panel" x="420" y="80" width="120" height="110" rx="10" />
  <text class="d-tag" x="436" y="100">build_area</text>
  <!-- lake with island -->
  <path d="M436 110 L524 110 L524 178 L436 178 Z" fill="#bcd3da" stroke="#33575b" stroke-width="1.4" />
  <path d="M462 130 L498 130 L490 160 L468 158 Z" fill="#f3f0df" stroke="#33575b" stroke-width="1.2" />
  <text class="d-sub" x="480" y="200" text-anchor="middle" style="font-size:9px">lake + island = 1 hole</text>

  <!-- closed-way classification note -->
  <rect class="d-panel-2" x="560" y="80" width="140" height="110" rx="10" />
  <text class="d-tag" x="574" y="100">closed way?</text>
  <text class="d-sub" x="574" y="122" style="font-size:9.5px">area tag → polygon</text>
  <text class="d-sub" x="574" y="142" style="font-size:9.5px">else → line</text>
  <text class="d-sub" x="574" y="168" style="font-size:9px">never both — a closed</text>
  <text class="d-sub" x="574" y="180" style="font-size:9px">road loop is a line only</text>
</svg>
<figcaption>Relations are assembled with GEOS <code>build_area</code>, which sorts member rings into outers and holes by geometry — so a lake with an island comes out as a polygon with one interior ring, ready for the <a href="../formats/#features-an-anchor-then-deltas">holes encoding</a>. A closed way is a polygon only if its tags say it encloses an area; a closed <code>highway</code> loop stays a line, never also a filled blob.</figcaption>
</figure>

### Cropping to a box

You rarely want a whole country. `--bbox W,S,E,N` crops the source **during ingest**, in a **pass 0** that reads only ids: the nodes inside the box, the ways touching one of them, the renderable area relations reached by those ways, and — the part that matters — every member way and node those selected objects still need *outside* the box.

That last clause is the whole design. The naive filter is "drop everything outside the box", and it fails in a way you'd only notice on the road. A way that leaves the box would be missing node positions, and the ingester drops a way it cannot fully resolve rather than guess at half of it — so every road crossing the boundary would vanish back to its last node inside. The map would fray inwards from its own edge, and the [navigation graph](#building-the-navigation-graph) would lose real exits: not a road drawn short, an exit the router doesn't know exists.

So a way is kept **whole** or not at all. An edge ends where the *way* ends, a little outside the box, rather than at an arbitrary vertex on the box edge — no phantom junction, no invented dead-end at the border. A renderable area relation touched by one of those ways is completed too: all of its member ways and their nodes are selected before assembly. This is crucial for OSM land cover, where one residential or forest multipolygon can be split across many ways. Dropping one out-of-box member would otherwise make the ingester reject the relation whole and leave an apparently random hole *inside* the requested map.

Two consequences worth expecting. The finished map's header bounding box is always a little **wider** than the box you asked for, because it is measured from the packed content and complete ways or relation members stick out — which is why an extract box must never be re-derived from a packed map's header, or it ratchets outward on every re-pack. And peak memory tracks the *selection*, not the source file: relation ids are streamed and the coordinate store holds only the box plus the complete objects it reached, so cropping a 500 MB country remains close to the cost of packing the resulting small map.

This begins with the strategy [`osmium extract`](https://osmcode.org/osmium-tool/) calls `complete_ways`, down to the integer grid used by its edge test, then applies the relation completion associated with osmium's `smart` strategy only to area relations the active schema can render. Route and administrative-boundary relations therefore cannot pull continent-scale geometry into a small map. A relation already missing members at the source extract's own edge is still rejected rather than guessed from an incomplete shape.

### Merging several regions

A tour that crosses a border needs two extracts, and the two genuinely overlap. Geofabrik cuts its regions along administrative boundaries and completes the ways that cross them, so every bridge, river and border road exists in *both* files under the *same* OSM ids. Merging is therefore not concatenation: it is deciding, object by object, which copy is the real one.

That decision used to belong to `osmium merge` — the packer's second C++ dependency. It now happens inside the passes already described: **every pass reads every file**, and the results are folded together afterwards, under two rules.

**On a duplicate id, the first file listed wins — the whole object, not a blend.** The tie-break is decided on the `(type, id)` alone. This matters because two extracts downloaded a week apart can hold two *versions* of the same object; `osmium merge` keeps both (it is built to handle history files), and the packer then drew both — one building rendered twice where somebody had retagged it between the two downloads. Picking a whole-object winner by id makes that impossible.

**The survivors come out in ascending id order**, per type — the order a single merged, sorted file would have produced. Feature order decides which [quadtree chunk](#the-quadtree-packing-geometry-into-chunks) a feature lands in and therefore the packed bytes, and on overlapping regions with no version skew the native merge reproduces the external chain's output byte for byte.

Memory tracks the box, not the source: nothing is re-written to disk and nothing holds a whole merged region resident, where the external chain's node-location index was sized by the largest node id in all of OSM — a two-region boxed build peaked around 2.3 GB there versus ~340 MB reading the regions in place, at about the same wall clock (the files are read in parallel).

### Land and sea

OSM ways draw the *coast*, but not the sea or the land fill. Those come from a separate global dataset of land polygons, clipped to the map's bounding box and added as features styled `natural.land`. The sea needs no geometry at all: it's the **backdrop** the renderer clears to before drawing, and land is simply painted on top.

That dataset is ~950 MB, downloaded and unpacked once into the packer's shared cache, so CLI and bakery runs reuse one copy. Both steps are the packer's own code rather than a `curl` and an `unzip` it hopes are installed, which keeps the operation portable, cancellable, and able to report progress. The first clip in a process scans the shapefile's record headers into a compact offset-and-bounds index; later planet leaves seek straight to intersecting polygon bodies instead of rescanning more than a gigabyte for every leaf.

<figure class="fig">
<svg viewBox="0 0 720 230" role="img" aria-label="The global land-polygons dataset, a world map of land shapes, is clipped to the map's bounding box, producing land faces. On the device these are drawn over a sea-coloured backdrop, so land sits on top of sea.">
  <defs>
    <marker id="aP4" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Land is clipped in; sea is the backdrop</text>

  <!-- world dataset -->
  <rect x="36" y="50" width="200" height="150" rx="8" style="fill:#bcd3da;stroke:#33575b;stroke-width:1.4" />
  <text class="d-tag" x="48" y="68" style="fill:#2c5230">global land polygons</text>
  <path d="M60 110 C 90 80, 130 90, 150 110 C 175 135, 140 175, 100 170 C 70 166, 48 140, 60 110 Z" fill="#cfe0c2" stroke="#3c6b39" stroke-width="1.2" />
  <path d="M170 150 C 190 135, 220 150, 214 175 C 208 192, 178 190, 170 175 Z" fill="#cfe0c2" stroke="#3c6b39" stroke-width="1.2" />
  <!-- bbox window -->
  <rect x="96" y="110" width="80" height="64" fill="none" stroke="#cf6a2a" stroke-width="2.2" />
  <text class="d-sub" x="136" y="105" text-anchor="middle" style="fill:#a9501c;font-size:9px">bbox</text>

  <line class="d-flow" x1="244" y1="125" x2="300" y2="125" marker-end="url(#aP4)" />
  <text class="d-sub" x="272" y="115" text-anchor="middle" style="font-size:9px">clip</text>

  <!-- result: land over sea -->
  <rect x="312" y="50" width="200" height="150" rx="8" style="fill:#bcd3da;stroke:#33575b;stroke-width:1.4" />
  <text class="d-tag" x="324" y="68" style="fill:#2c5230">sea backdrop</text>
  <path d="M312 120 C 340 96, 380 104, 400 120 C 430 144, 470 120, 512 132 L512 200 L312 200 Z" fill="#cfe0c2" stroke="#3c6b39" stroke-width="1.4" />
  <text class="d-sub" x="360" y="186" text-anchor="middle" style="font-size:9px">land faces, on top</text>

  <!-- note -->
  <rect class="d-panel-2" x="540" y="74" width="160" height="100" rx="10" />
  <text class="d-sub" x="556" y="98" style="font-size:10px">sea = the lowest-z</text>
  <text class="d-sub" x="556" y="114" style="font-size:10px">style; the screen is</text>
  <text class="d-sub" x="556" y="130" style="font-size:10px">cleared to it, then</text>
  <text class="d-sub" x="556" y="146" style="font-size:10px">land + roads paint</text>
  <text class="d-sub" x="556" y="162" style="font-size:10px">over it.</text>
</svg>
<figcaption>The packer reads the land shapefile directly and reprojects it from Web Mercator with closed-form math — no GIS stack — decoding only the records whose bounding box touches the query. The result flows through the same simplify-and-quadtree path as every other feature, so by the time the device sees it, land is just more geometry.</figcaption>
</figure>

### Contours, traced from the terrain

A map packed with [terrain](../terrain/) can carry contour lines, and they are made the same way land is: generated **once**, over the whole extract, before any tier exists — and then they are ordinary features. Same styles, same simplify ladder, same densify, same quadtree. Nothing downstream, on the host or on the device, is told that a contour is anything other than a line.

That is the entire design. The alternative — a raster band, or a dedicated section with its own renderer — was measured against it and lost on a single fact: contours are *styled vector features*, so the moment they exist as `Geom::Line` with a style id, every problem left is one the packer already solved. The [renderer](../rendering/) gained no contour code: the one thing it did learn is a **general** style property — that some marks are used at the width they were authored — which contours are simply the first shipped user of.

The trace itself is **marching squares** over the terrain's own sample lattice, one elevation level at a time, read back through the same `no_std` sampler the device runs (at exact lattice coordinates, where the [bilinear interpolation](../terrain/#one-sampling-truth) collapses to the stored sample — so the packer cannot see a surface the firmware would not). A lattice cell any of whose four corners is unknown is skipped whole: a contour is never drawn across ground the DEM does not know, which is the raster's ["a hole is silence"](../terrain/#coverage-edges-honestly) rule showing up in vector form. The classic ambiguity — a cell with high ground on one diagonal and low on the other, where two contours could be joined two ways — is resolved by the cell's mean, which both neighbours compute identically from the same four numbers, so the segments meet.

Two classes come out: a `major` contour at every level, and an `index` contour at every fifth one (100 m and 500 m by default). They are styled independently, like any two feature types, and a class the config gives no style to is not packed — nor even traced.

**How they're styled is the whole of how they read**, and the shipped answer is deliberately spare: one grey, both classes at `weight 1`, on the map from the planning tier down, `major` **dashed** and `index` **solid**. Emphasis by *continuity* rather than mass — the index line is the unbroken one among dashes — keeps the ladder to a single colour and a single weight, and nothing else dashed in the palette is rideable, so a contour can't be misread as a trail. Both classes also set the [fixed-width bit](../formats/#the-header), which takes them off the renderer's zoom width ramp: a contour has no width on the ground, and ramped it would draw thinnest at the zoom where the landform read matters most and thickest where it buries the streets. Every one of those choices lives in the **style table** — the same extract packs to byte-identical bytes dashed or solid, hairline or ramped — so what a contour costs is decided entirely by which tiers carry it.

**One number is a rule rather than a setting.** Traced geometry is simplified to **15 m** before the ladder ever sees it. The finest tiers simplify at 3 m and 0.5 m — one to two orders finer than a ~40 m DEM posting can justify — so without that clamp the fine LODs faithfully store the interpolation ramp between samples, which is not terrain and is not visible. On an alpine test extract the clamp removed 905 KB and changed nothing on the glass.

The cost is real and worth stating plainly: contours are the most expensive optional thing a map can carry. On the corpus's worst case for them — a 628 km² alpine extract, dense terrain and sparse OSM — 100 m contours from the planning tier down add about **a quarter** to the map. Flat ground adds almost nothing, because flat ground crosses almost no levels.

### Extracting POIs

The same OSM extract carries more than geometry. Amenities a bikepacker actually looks for — water, campsites, lodging, resupply, pharmacies, bike shops — are tagged on nodes and areas the geometry pipeline would otherwise style-and-forget. A separate stage harvests them into the map's [POI section](../formats/#pois-a-nearest-list-not-a-map-layer), where the device browses them by category. It's config-free on purpose: the tag → category mapping is **hardcoded in the packer** (a locked decision), so packing the same extract always yields the same POIs.

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="POI extraction. On the left, two OSM sources: a tagged node used as-is, and a closed way whose polygon centroid becomes a point. Both are classified against a fixed table of tag-equals-value rules mapping to a category and subtype. Names are folded to ASCII and capped at 24 bytes. Finally a dedup step collapses a node and a way-centroid of the same category within 50 metres into one POI, keeping the node.">
  <defs>
    <marker id="aP6" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Nodes + way-centroids → classify → fold names → dedup</text>

  <!-- sources -->
  <rect class="d-panel-2" x="24" y="44" width="150" height="40" rx="9" />
  <text class="d-sub" x="40" y="62" style="font-size:10px">a tagged node</text>
  <text class="d-sub" x="40" y="77" style="font-size:9px">amenity=drinking_water</text>
  <circle cx="158" cy="64" r="4" class="d-hot-fill" />

  <rect class="d-panel-2" x="24" y="94" width="150" height="52" rx="9" />
  <text class="d-sub" x="40" y="112" style="font-size:10px">a closed way</text>
  <text class="d-sub" x="40" y="127" style="font-size:9px">tourism=camp_site</text>
  <!-- small polygon with centroid dot -->
  <path d="M120 118 L150 116 L156 136 L126 140 Z" fill="none" stroke="#3c6b39" stroke-width="1.2" />
  <circle cx="138" cy="127" r="3" class="d-hot-fill" />
  <text class="d-sub" x="120" y="152" style="font-size:8.5px;fill:#a9501c">→ ring centroid</text>

  <!-- classify -->
  <line class="d-flow" x1="178" y1="64"  x2="224" y2="86" marker-end="url(#aP6)" />
  <line class="d-flow" x1="178" y1="120" x2="224" y2="98" marker-end="url(#aP6)" />
  <rect class="d-panel" x="230" y="56" width="204" height="92" rx="10" />
  <text class="d-tag" x="246" y="76">fixed table · first match</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="246" y="96"  style="font-size:9.5px">amenity=drinking_water → Water</text>
    <text class="d-sub" x="246" y="112" style="font-size:9.5px">natural=spring &nbsp;&nbsp;&nbsp;&nbsp;&nbsp;→ Water</text>
    <text class="d-sub" x="246" y="128" style="font-size:9.5px">tourism=camp_site &nbsp;&nbsp;→ Campsite</text>
    <text class="d-sub" x="246" y="142" style="font-size:9px;fill:#a9501c">… 18 subtypes, 6 categories</text>
  </g>

  <!-- fold names -->
  <line class="d-flow" x1="438" y1="102" x2="484" y2="102" marker-end="url(#aP6)" />
  <rect class="d-panel-2" x="490" y="72" width="206" height="60" rx="10" />
  <text class="d-tag" x="506" y="90">fold name → ASCII, ≤ 24 B</text>
  <text class="d-sub" x="506" y="110" font-family="var(--mono)" style="font-size:10px">"Bäckerei Müller"</text>
  <text class="d-sub" x="506" y="124" font-family="var(--mono)" style="font-size:10px;fill:#a9501c">→ "Baeckerei Mueller"</text>

  <!-- dedup -->
  <line class="d-flow" x1="360" y1="150" x2="360" y2="196" marker-end="url(#aP6)" />
  <rect class="d-hot" x="120" y="200" width="480" height="76" rx="12" style="fill:#f8efe4" />
  <text class="d-tag" x="138" y="220" style="fill:#a9501c">dedup — same category within 50 m = one POI</text>
  <!-- node + centroid merging -->
  <circle cx="168" cy="248" r="5" class="d-hot-fill" /><text class="d-sub" x="150" y="268" style="font-size:9px">node</text>
  <circle cx="210" cy="248" r="4" class="d-water" /><text class="d-sub" x="196" y="268" style="font-size:9px">centroid</text>
  <line x1="176" y1="248" x2="202" y2="248" stroke="#9aa884" stroke-width="1.2" stroke-dasharray="3 2" />
  <line class="d-flow" x1="250" y1="248" x2="300" y2="248" marker-end="url(#aP6)" />
  <circle cx="330" cy="248" r="5" class="d-hot-fill" /><text class="d-sub" x="346" y="252" style="font-size:9.5px">the node wins</text>
  <text class="d-sub" x="470" y="240" style="font-size:9px">priority: node beats centroid,</text>
  <text class="d-sub" x="470" y="254" style="font-size:9px">then named beats unnamed,</text>
  <text class="d-sub" x="470" y="268" style="font-size:9px">then first-seen.</text>
</svg>
<figcaption>Both an OSM <b>node</b> and a <b>closed way</b> can be a POI — a way is reduced to its shoelace-weighted <b>ring centroid</b>, so a campsite polygon becomes a single point. Each candidate is classified against a fixed <code>key=value</code> table (first match in table order wins, the same rule as the style config), and its name is <b>folded to printable ASCII</b> and capped at 24 bytes. The last step matters because OSM double-maps: a drinking-water node sitting inside a same-tagged area, an entrance node beside a campsite polygon. Two candidates of the <b>same category within ~50 m</b> collapse to one, and the winner is chosen by priority — a node (a real placed point) beats a derived centroid, a named POI beats an unnamed one.</figcaption>
</figure>

Why fold names at all? The OBCM `Name` field is a **fixed 24-byte, printable-ASCII** slot: fixed-width keeps records seekable, and one byte per character keeps the budget a predictable 24 glyphs (a raw UTF-8 `ö` is two bytes). So rather than store variable-width UTF-8, the packer transliterates at pack time: German umlauts get their proper digraphs (`ä → ae`, `ß → ss`), the rest of Latin strips to its base letter, and anything genuinely unmappable (CJK, Cyrillic, Greek) becomes a word break rather than gluing neighbours together. (The [device font](../ui/#the-field-map-look) itself covers Latin-1 + Latin Extended-A, so *phone-supplied* route and ride names render their umlauts directly on-glass — it's only these fixed-width **packed** POI names that fold.) A name that folds away to nothing is stored as unnamed, and the device falls back to the subtype's label ("Spring", "Bakery"). The 24-byte cap is a device-row width, not a storage worry — POI bytes are noise next to geometry.

```rust
pub const POI_TABLE: [PoiKind; 18] = [
    kind(1, "amenity", "drinking_water"), // → Water / "Drinking water"
    kind(2, "natural", "spring"),         // → Water / "Spring"
    kind(5, "tourism", "camp_site"),      // → Campsite
    kind(13, "shop", "supermarket"),      // → Resupply / "Supermarket"
    // … 18 rows, ids append-only — the subtype id is normative (OBCM_Spec §7.4)
];
```

The subtype *ids* are normative and shared: the packer owns only the OSM `key=value` half of the table, while each subtype's category and fallback label live once in [`obc-formats`'s POI table](src:firmware/obc-formats/src/obcm.rs) — the same table the device reads through `obc-reader` — so the two crates can't drift, and a pinning test asserts every row agrees. The extracted, deduped, name-folded POIs are handed to the serializer, which builds the [per-category quadtrees](../formats/#pois-a-nearest-list-not-a-map-layer) of the POI section.

### Parsing opening hours

A POI carries one more thing worth harvesting: when it's *open*. OSM stores that in the [`opening_hours`](https://wiki.openstreetmap.org/wiki/Key:opening_hours) tag — a compact grammar like `Mo-Fr 08:00-18:00; Sa 09:00-13:00; PH off`. Parsing that grammar is a **host job that never runs on the device**: the microcontroller has no room for a date library and no reason to re-derive the same answer every frame. So the packer parses `opening_hours` **once, at pack time**, into the fixed [29-byte weekly schedule](../formats/#opening-hours-a-pooled-weekly-schedule) the device reads with a single array lookup.

The parser is a deliberate **subset**, not a full `opening_hours` engine — the real data (town shops, campsites) exercises a small corner of the grammar, and a full engine would drag a time model into a build tool that has no clock. It handles weekday ranges and lists (`Mo-Fr`, `Mo,We,Fr`), `HH:MM-HH:MM` intervals (including a split lunch, two per day), `24/7`, `off`/`closed`, bare time-only rules that apply every day, and overnight wrap. Times are rounded to the nearest quarter-hour with the same **round-half-to-even** convention the packer uses for coordinates. Anything it *can't* model it **drops and flags** rather than guessing — it never invents hours that aren't there.

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="The opening_hours stage. A raw OSM opening_hours string is parsed at pack time into a normalized weekly schedule of seven days. A seasonal date rule is flattened to a representative in-season week and flagged seasonal; a public-holiday or unmodellable rule is dropped and flagged truncated. All resulting schedules are then deduplicated into a small pool, and each POI stores only its pool index.">
  <defs>
    <marker id="aOH" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">opening_hours → normalize → flag → dedup pool</text>

  <!-- raw string -->
  <rect class="d-panel-2" x="24" y="44" width="238" height="52" rx="9" />
  <text class="d-sub" x="40" y="62" style="font-size:9.5px">raw OSM tag</text>
  <text class="d-sub" x="40" y="80" font-family="var(--mono)" style="font-size:9px">Mo-Fr 08:00-18:00; Sa 09-13</text>
  <text class="d-sub" x="40" y="92" font-family="var(--mono)" style="font-size:9px">Apr-Oct: …; PH off</text>

  <!-- parse -->
  <line class="d-flow" x1="266" y1="70" x2="312" y2="70" marker-end="url(#aOH)" />
  <rect class="d-panel" x="318" y="44" width="196" height="52" rx="10" />
  <text class="d-tag" x="334" y="64">parse (subset grammar)</text>
  <text class="d-sub" x="334" y="84" style="font-size:9px">→ 7 days × ≤2 intervals</text>

  <!-- flags -->
  <line class="d-flow" x1="416" y1="96" x2="416" y2="120" marker-end="url(#aOH)" />
  <rect class="d-panel-2" x="300" y="126" width="232" height="58" rx="10" />
  <text class="d-sub" x="316" y="146" style="font-size:9.5px;fill:#a9501c">seasonal — Apr-Oct flattened to</text>
  <text class="d-sub" x="316" y="159" style="font-size:9px">a representative in-season week</text>
  <text class="d-sub" x="316" y="176" style="font-size:9.5px;fill:#a9501c">truncated — PH / 3rd interval dropped</text>

  <!-- dedup pool -->
  <line class="d-flow" x1="300" y1="155" x2="230" y2="200" marker-end="url(#aOH)" />
  <rect class="d-hot" x="24" y="196" width="300" height="44" rx="12" style="fill:#f8efe4" />
  <text class="d-tag" x="40" y="216" style="fill:#a9501c">dedup — a region's shops share hours</text>
  <text class="d-sub" x="40" y="232" style="font-size:9px">identical schedules → one blob; POI stores its index</text>

  <!-- only-with-hours note -->
  <rect class="d-panel-2" x="344" y="196" width="352" height="44" rx="10" />
  <text class="d-sub" x="360" y="214" style="font-size:9.5px">a POI with no parseable hours stores <tspan font-family="var(--mono)">0xFFFF</tspan></text>
  <text class="d-sub" x="360" y="230" style="font-size:9px">— only POIs that actually have hours cost a pool slot</text>
</svg>
<figcaption>The stage runs per POI, after classification. Two cases need flattening. A <b>seasonal</b> rule (<code>Apr-Oct: …</code>) carries a date selector the weekly blob can't express, so the packer bakes a <b>representative in-season week</b> and sets the <i>seasonal</i> flag — the v1 device ignores it, but the bit is there for a future season-aware pass. A rule the subset genuinely can't model — a public-holiday <code>PH</code> clause, a <code>sunrise/sunset</code> time, or a third interval on a day — is <b>dropped</b> and the <i>truncated</i> flag set, so the device knows the schedule is partial rather than wrong. Finally every schedule is <b>deduplicated</b> into a shared pool: because a whole town's shops so often keep the same hours, the pool stays tiny, and a POI with no parseable hours (the common case) costs nothing but a <code>0xFFFF</code> sentinel in its record.</figcaption>
</figure>

The subset grammar, the flag semantics, and the quarter-hour encoding live in [`obc-pack/src/hours.rs`](src:host/obc-pack/src/hours.rs); the pooled bytes are described in [`OBCM_Spec.md` §7.5](src:specs/OBCM_Spec.md). The device end — turning a pooled blob into *today's hours* and an *open-now* badge — is the [POI detail view](../ui/#the-poi-detail-view).

### Building the navigation graph

The map so far is geometry the device *draws*. To let the device *route* — compute its own way to a POI — the packer builds one more thing the raw data doesn't contain: a **navigation graph**. Highways in OSM are ways, and the geometry pipeline turns them into styled polylines the moment it resolves their coordinates — dropping the node ids as it goes. But those node ids are the topology: two roads that *share* an OSM node meet there. This stage keeps the node ids for routable ways and recovers the graph from the shared ones. It's serialized into the map's [navigation-graph section](../formats/#the-navigation-graph-a-routable-network), and it's **always built** — a config-free, always-present section like the POIs, so packing the same extract always yields the same graph (there's no toggle to forget).

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="Building the navigation graph in three steps. First, a bike-legality filter keeps most highway ways but excludes motorway and its links, excludes trunk unless it is tagged bicycle equals yes, and hard-excludes anything tagged access equals no or private, bicycle equals no, or motorroad equals yes. Second, junction detection: a node touched by two or more routable ways, or sitting at a way's endpoint, becomes a junction; interior shape points do not. Third, each way is split at its junctions into edges, duplicate and reversed parallel ways are deduplicated by an unordered endpoint pair plus geometry key, and each edge's great-circle length becomes its cost. The result is junction nodes joined by edges.">
  <defs>
    <marker id="aNG" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Filter routable ways → find junctions → split + dedup into edges</text>

  <!-- 1 class/access filter -->
  <rect class="d-panel" x="24" y="52" width="216" height="120" rx="10" />
  <text class="d-tag" x="40" y="72">① routable? highway + access</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="40" y="92"  style="font-size:8px;fill:#2c5230">residential · track · path · cycleway ✓</text>
    <text class="d-sub" x="40" y="107" style="font-size:8px;fill:#2c5230">footway · steps · service ✓ (walk a bike)</text>
    <text class="d-sub" x="40" y="125" style="font-size:8px;fill:#a9501c">motorway ✗ · trunk ✗ unless bicycle=yes</text>
    <text class="d-sub" x="40" y="140" style="font-size:8px;fill:#a9501c">access=no|private · bicycle=no ✗</text>
  </g>
  <text class="d-sub" x="40" y="162" style="font-size:8.5px">independent of render styling</text>

  <!-- 2 junction detection -->
  <line class="d-flow" x1="246" y1="112" x2="276" y2="112" marker-end="url(#aNG)" />
  <text class="d-sub" x="270" y="52" style="font-size:9px;fill:#6b7758">② a shared node = a junction</text>
  <!-- two ways crossing at a node -->
  <line x1="288" y1="140" x2="392" y2="96" stroke="#9aa884" stroke-width="2.4" />
  <line x1="300" y1="86"  x2="380" y2="150" stroke="#9aa884" stroke-width="2.4" />
  <!-- interior shape points (open) -->
  <circle cx="322" cy="128" r="2.6" fill="#ece8cf" stroke="#9aa884" stroke-width="1"/>
  <circle cx="360" cy="112" r="2.6" fill="#ece8cf" stroke="#9aa884" stroke-width="1"/>
  <!-- the shared junction -->
  <circle cx="340" cy="119" r="5" class="d-hot-fill" />
  <text class="d-sub" x="340" y="172" text-anchor="middle" style="font-size:8.5px;fill:#a9501c">touched ≥2× → junction</text>
  <text class="d-sub" x="270" y="192" style="font-size:8.5px">endpoints are always junctions;</text>
  <text class="d-sub" x="270" y="204" style="font-size:8.5px">interior shape points never are</text>

  <!-- 3 split + dedup -->
  <line class="d-flow" x1="404" y1="112" x2="440" y2="112" marker-end="url(#aNG)" />
  <rect class="d-hot" x="448" y="52" width="248" height="120" rx="10" style="fill:#f8efe4" />
  <text class="d-tag" x="464" y="72" style="fill:#a9501c">③ split into edges + dedup</text>
  <text class="d-sub" x="464" y="94"  style="font-size:9.5px">cut each way at every junction</text>
  <text class="d-sub" x="464" y="110" style="font-size:9.5px">→ edge interiors are junction-free</text>
  <text class="d-sub" x="464" y="130" style="font-size:9.5px">dedup key: unordered (a,b) + geometry</text>
  <text class="d-sub" x="464" y="146" style="font-size:9px;fill:#a9501c">a way + its reverse = one edge</text>
  <text class="d-sub" x="464" y="162" style="font-size:9px">cost = great-circle length, metres</text>

  <!-- result graph -->
  <line class="d-flow" x1="360" y1="216" x2="360" y2="242" marker-end="url(#aNG)" />
  <rect class="d-panel-2" x="180" y="246" width="360" height="46" rx="10" />
  <text class="d-sub" x="360" y="266" text-anchor="middle" style="font-size:10px">junction <tspan style="font-weight:700">nodes</tspan> (dense pack-run ids) joined by undirected <tspan style="font-weight:700">edges</tspan></text>
  <text class="d-sub" x="360" y="282" text-anchor="middle" style="font-size:9px">→ serialized as the map's §8 navigation graph</text>
</svg>
<figcaption>The <b>routable predicate</b> reads only a way's routing tags (<code>highway</code>, <code>access</code>, <code>bicycle</code>, <code>motorroad</code>) — never the style config, so a road can be drawn but not routable, or the reverse. <b>Motorway</b> is always out; <b>trunk</b> is out unless tagged <code>bicycle=yes</code>; <code>access=no|private</code>, <code>bicycle=no|use_sidepath</code>, and <code>motorroad=yes</code> are hard excludes; <code>footway</code> and <code>steps</code> stay in because it is legal to <i>walk</i> a bike there — preference is the <a href="#weighting-the-graph-bike-profiles">profiles'</a> job. <b>Junction detection</b> is pure counting: a node touched by two-or-more routable ways, or any way's endpoint, is a junction. Ways <b>split</b> at their junctions into edges with junction-free interiors, and duplicates collapse on (unordered endpoint pair + geometry + way-kind), so two genuinely different roads between the same pair both survive. Cost is great-circle length in metres, summed with the same helper the route format uses. A hygiene pass <b>prunes islands</b> (components under <code>min_component_edges</code>, default 50) so the device can't snap a rider onto an unroutable islet, and long edges split at synthetic junctions so every field fits the §8 record's <code>int16</code>/<code>uint16</code>.</figcaption>
</figure>

A way is routable exactly when it can be *classified* — the same pass that decides legality also computes the edge's **way-kind** byte (its highway + surface class, [below](#weighting-the-graph-bike-profiles)), so `is_routable` is just `classify(tags).is_some()`:

```rust
pub fn is_routable<'a, I: IntoIterator<Item = (&'a str, &'a str)>>(tags: I) -> bool {
    classify(tags).is_some()
}

/// The packed way-kind byte, or None when the way is bike-illegal.
pub fn classify<'a, I: IntoIterator<Item = (&'a str, &'a str)>>(tags: I) -> Option<u8> {
    // ... read highway / surface / bicycle / access / motorroad from tags ...
    if matches!(access, Some("no" | "private")) { return None; }            // hard excludes
    if motorroad == Some("yes") { return None; }
    if matches!(bicycle, Some("no" | "use_sidepath")) { return None; }
    let hclass = match highway? {
        "trunk" | "trunk_link" if bicycle == Some("yes") => 13,             // else excluded, like motorway
        other => highway_class(other)?,                                     // None ⇒ not routable (motorway, …)
    };
    Some((surface_class(surface) << 5) | hclass)                            // way_kind = surface<<5 | highway
}
```

A real pack run logs the graph next to the POI counts, so a glance at the build output confirms it's there and plausibly sized:

```
nav graph: 12874 nodes, 15903 edges, 8421.6 km
```

The in-memory build — the way-kind classification, the bike-legality filter, junction detection, edge split and dedup, island pruning, and great-circle lengths — lives in [`obc-pack/src/nav.rs`](src:host/obc-pack/src/nav.rs); turning that graph into the tiled, chunked [§8 section](../formats/#the-navigation-graph-a-routable-network) (the node quadtree, the inline-adjacency records, the byte-addressed edge pool, and the densify + long-edge split that keep every record inside one chunk) is the serializer's job, described in [`OBCM_Spec.md` §8](src:specs/OBCM_Spec.md). What the device *does* with it — snap, profile-weighted A\*, emit — is [the router seam](../architecture/#on-device-routing-the-router-seam).

### Weighting the graph: bike profiles

The graph so far is bike-*legal* but undifferentiated: every edge costs its metres, so the device would route a road bike down a muddy singletrack if it were a few metres shorter. What makes an MTB route differ from a road route — *why your MTB route differs* — is two more things the packer bakes in. Each edge carries a **way-kind** byte, and the section opens with a small table of **bike profiles**; on the device, A\* multiplies each edge's raw metres by the chosen profile's weight for that edge's way-kind, so "shortest" becomes "cheapest *for this bike*."

**Way-kind** is one byte per edge, `way_kind = (surface_class << 5) | highway_class` — a 5-bit **highway class** (0 `cycleway`, 1 `path`, 2 `track`, 3 `footway`, … 10 `tertiary`, 11 `secondary`, 12 `primary`, and 13 `trunk_cycl` for a bike-legal trunk) and a 3-bit **surface class** (`paved`, `compacted`, `gravel`, `dirt`, `rough`, `cobbles`, `grass`, plus `unknown`). Both tables are **locked** and config-free — the same OSM extract always yields the same bytes — and they are the *single vocabulary* profiles are written against. The full canonical table is [`OBCM_Spec.md` §8.6](src:specs/OBCM_Spec.md) (mirrored from the one source of truth, `nav.rs`); the device never sees a raw OSM tag, only this byte.

A **profile** is a display name plus a multiplier for every highway class and every surface class, stored in `1/16` fixed-point (so `16` = 1.0×, and `0` means **forbidden** — that class is dropped from the profile's graph entirely), plus a **climb weight** (below). The map carries 1–8 of them (the default pack ships four); the device's effective weight for an edge is `(highway_mult × surface_mult) >> 4`. Here are the four default profiles' highway weights for a handful of classes (the [shipped schema](src:builder/presets/schema.json) has the rest, plus the surface axis):

| highway class | Road | Gravel | MTB | Touring |
|---|---|---|---|---|
| cycleway | 1.0 | 1.1 | 1.3 | 1.0 |
| primary  | 1.8 | 2.2 | 3.5 | 2.6 |
| track    | 6.0 | 1.2 | 1.0 | 1.6 |
| path     | 7.0 | 1.5 | 1.0 | 2.0 |
| steps    | forbidden | forbidden | 3.0 | 6.0 |

Read a column and you can predict the routing. **Worked example:** suppose a rider can reach the same destination two ways — a **1 km stretch of `primary`** road, or a **2 km `cycleway`** that loops around it (both `paved`, so each profile's surface weight scales both sides equally and drops out of the comparison). Multiply length by the highway weight:

- **Road** — primary `1000 × 1.8 = 1800`, cycleway `2000 × 1.0 = 2000`. Road takes the **primary** (1800 < 2000): a road cyclist would rather ride 1 km of quiet main road than 2 km out of the way.
- **MTB** — primary `1000 × 3.5 = 3500`, cycleway `2000 × 1.3 = 2600`. MTB takes the **cycleway** (2600 < 3500): to the MTB profile the primary is so heavily penalised that the 2× detour is still cheaper.

Same two roads, same start and finish, opposite choice — that difference is entirely the profile, and you could have called it from the table above.

**One rule constrains every profile: no weight below 1.0×** (a non-zero multiplier is always ≥ `16`). The on-device A\* uses a great-circle heuristic, which is only admissible — only *safe to trust* — if no edge can cost less than its straight-line distance. A weight under 1.0× would make some edge cheaper than the crow flies and quietly break the ε bound. So the packer **rejects** a config with a non-zero weight below 1.0 (naming the A\* bound in the error), and the reader **clamps** one up to 1.0× defensively. Which is what keeps **ε** meaningful: the router's `f = g + ε·h` with ε = 1.3 returns a path at most 1.3× the *cheapest route under the profile* — not the geometrically shortest, the cheapest once your bike's weights are applied (and, once the map carries terrain, once its [climb weight](#weighting-the-climb) is applied too: the same bound, read against a cost that already counts the climbing). (When even the tight 1.3× bound exhausts the device's fixed search table, ε **escalates** — 1.3 → 2.0 → 3.0 — to reach farther in the same memory; the bound is then the successful rung's ε. That range mechanism lives with [the router seam](../architecture/#on-device-routing-the-router-seam).)

Which profile the device uses is a single **Bike-type** setting — a bare index into the loaded map's profile table, persisted across reboots. Pick "MTB" and every plan re-weights accordingly; the created-route overview shows the profile it used. If the setting points past a particular map's profile count (a smaller map, a stale setting), the router falls back to profile 0 and the UI honestly shows *profile 0's* name rather than a profile the map doesn't have.

### Weighting the climb

Way-kind says what an edge is *made of*; it says nothing about what it costs your legs. In the Alps that is not a rounding error: "the cheapest route under the profile" will happily buy 400 m of climbing to save 200 m of distance, which is the wrong answer for every rider on exactly the terrain this device is for. So a map packed against terrain stores, per **direction** of every edge, the **ascent** of riding it.

**It is an integral, not an endpoint difference.** A pass road between two junctions at the same height has hundreds of metres of climbing in it and no net change at all; a difference of endpoints would price it as flat and send you over the col. So the packer walks the edge's *final* polyline — after every split the serializer performs, since an edge's climb cannot be divided among its pieces after the fact — sampling at every vertex plus interpolated points, so no gap exceeds about **50 m of ground**. That figure is chosen against the raster rather than the road: the shipped terrain posting is ≈ 57 m in latitude, so 50 m guarantees at least one sample per posting and a hill between two far-apart OSM shape nodes cannot be stepped over. Sampling finer would only re-read the same interpolated plane. A stretch with no elevation coverage contributes nothing — the integrator re-anchors across the hole rather than booking the climb over it.

Because ascent is directional, it is the **one field the two sides of an edge disagree on**: the entry `a→b` carries `ascent(a→b)` and the entry `b→a` carries what is the first direction's descent. Everything else about an edge — its id, its ground length, its way-kind — still matches on both sides, and a verifier that checks "both sides agree" has to exclude exactly this field.

**Where the heights come from — and what the packer deliberately cannot read.** The packer takes a `--terrain` input: an [OBCT](../formats/#obct-the-terrain-raster) container, or a directory of them, which it opens through the *same* `no_std` sampling crate the firmware runs. It has **no DEM decoder at all**. Turning Copernicus GLO-30 GeoTIFFs into terrain cells is a separate host tool, [`obc-dem`](src:host/obc-dem), run once per dataset release by the bakery — so the packer gains no GeoTIFF dependency (libGEOS stays the last native one in the tree), and, more importantly, the surface it costs a climb against is byte-for-byte the surface the device later draws your profile from. That is the whole point of baking terrain first: [one sampling truth](../terrain/#one-sampling-truth), not two pipelines that ought to agree.

The integrator is shared end to end. The same **±3 m dead-band** that the GPX converter runs over an imported track, that the device's live barometric climb uses, and that the elevation profile is built with, is the one the packer folds each edge's samples through — deliberately not a packer-private threshold, because the epic's entire claim is that the ascent a route is *costed* by and the ascent you are *shown* are the same number, and two thresholds would make them incomparable by construction.

Each profile then says what a metre of that climbing is worth in flat metres — the **climb weight**. The shipped four are Road 10, Gravel 8, MTB 6, Touring 8: a road rider will ride a kilometre out of their way to avoid a hundred metres of climb, a mountain biker rather less. The term is **added**, after the way-kind scaling and not inside it:

```
cost = (metres × way-kind weight) >> 4  +  ascent × climb weight
```

Adding rather than crediting descents is not a simplification but the thing that keeps the router honest. The great-circle heuristic is admissible only while no edge can cost *less* than its straight-line distance; a descent discount would let one, and the ε bound the plan's quality rests on would go with it. Which is also why the climb weight, unlike a way-kind multiplier, needs no `≥ 1.0×` floor: every value including `0` is admissible, because the term can only ever add. A weight of `0` means climb-blind, which is exactly how a map packed without terrain routes — its ascents are all `0`, so the term vanishes and the router reproduces its pre-elevation behaviour bit for bit.

**What ε bounds now.** The ladder itself did not change — `f = g + ε·h`, starting at 1.3× and escalating 1.3 → 2.0 → 3.0 only when the fixed search table exhausts. What changed is what the guarantee *reads as*: the returned path is at most the successful rung's ε times the best **climb-aware** route under the profile. Not the shortest; not even the cheapest by way-kind alone; the cheapest once your bike's weights *and* its appetite for climbing are both applied.

That is not free, and the honest version is worth stating: charging for climb makes `g` larger relative to `ε·h`, so the search is a little less goal-greedy and the frontier does more work. Measured over hundreds of endpoint pairs on an alpine extract, aggregate settles rose 4–31 % depending on profile, with the median pair settling the same nodes it did before — but 1–3 % of pairs that planned climb-blind now exhaust the table instead. Every one of those fails as the device's honest "too far to route here" after climbing the whole ε ladder, never as a wrong route.

<figure class="fig">
<svg viewBox="0 0 720 320" role="img" aria-label="A plan view of one measured pair of points above Innertkirchen, with a hill drawn as three nested contour rings peaking at 1380 metres. A solid orange line runs from the start straight over the hill: 8340 metres and 1008 metres of ascent, topping out at 1380 metres. A dashed green line curves around the hill instead: 10914 metres and 784 metres of ascent, never climbing above the goal's own 1066 metres. Notes below say the answer switches between climb weights 8 and 10, buying 2.6 kilometres of extra ground for 224 metres less climb and a crest 314 metres lower, and that the mountain-bike profile at weight 6 declines the same detour.">
  <text class="d-tag" x="20" y="22">the same two points, two climb weights — measured on grimsel</text>

  <!-- the hill, as nested contours -->
  <ellipse cx="360" cy="180" rx="132" ry="68" fill="#eae4cb" fill-opacity="0.6" stroke="#9aa884" stroke-width="1.2" />
  <ellipse cx="360" cy="178" rx="90" ry="46" fill="none" stroke="#9aa884" stroke-width="1.1" />
  <ellipse cx="360" cy="176" rx="48" ry="25" fill="none" stroke="#9aa884" stroke-width="1.1" />
  <text class="d-sub" x="360" y="192" text-anchor="middle" style="font-size:9px;fill:#a9501c">crest</text>
  <text class="d-sub" x="360" y="205" text-anchor="middle" style="font-size:9px;fill:#a9501c">1 380 m</text>

  <!-- over the top -->
  <path d="M78 214 C 170 200, 262 176, 360 170 C 458 164, 566 186, 646 192"
        fill="none" stroke="#cf6a2a" stroke-width="2.8" />
  <!-- round the valley -->
  <path d="M78 214 C 190 268, 320 280, 440 268 C 540 258, 606 218, 646 192"
        fill="none" stroke="#3c6b39" stroke-width="2.6" stroke-dasharray="7 5" />
  <circle cx="78" cy="214" r="4.5" class="d-hot-fill" />
  <circle cx="646" cy="192" r="4.5" class="d-hot-fill" />
  <text class="d-sub" x="78" y="234" text-anchor="middle" style="font-size:8.5px">start</text>
  <text class="d-sub" x="654" y="212" style="font-size:8.5px">goal 1 066 m</text>

  <!-- labels -->
  <rect class="d-panel-2" x="24" y="34" width="316" height="38" rx="8" />
  <text class="d-sub" x="38" y="50" style="font-size:9.5px;fill:#a9501c"><tspan style="font-weight:700">climb weight 0</tspan> &#8212; straight over the top</text>
  <text class="d-sub" x="38" y="64" style="font-size:9.5px">8 340 m &#183; &#9650; 1 008 m &#183; high point 1 380 m</text>

  <rect class="d-panel-2" x="380" y="34" width="316" height="38" rx="8" />
  <text class="d-sub" x="394" y="50" style="font-size:9.5px;fill:#2c5230"><tspan style="font-weight:700">climb weight 10</tspan> &#8212; round the valley</text>
  <text class="d-sub" x="394" y="64" style="font-size:9.5px">10 914 m &#183; &#9650; 784 m &#183; never above the goal</text>

  <text class="d-sub" x="24" y="300" style="font-size:9px">the answer switches between <tspan font-family="var(--mono)">w = 8</tspan> and <tspan font-family="var(--mono)">w = 10</tspan>: +2.6 km of ground bought for &#8722;224 m of climb, and a crest 314 m lower</text>
  <text class="d-sub" x="24" y="313" style="font-size:9px;fill:#6b7758">the MTB profile at <tspan font-family="var(--mono)">w = 6</tspan> declines the same detour and keeps its line — the product statement, not a bug</text>
</svg>
<figcaption>Weights are not a dial that makes routes "better"; they are a statement about what a given rider trades. Sweeping the weight on one real alpine pair moves the answer once, sharply, between 8 and 10 — and the two lines share only a quarter of their corridor, so it is a genuinely different route rather than emit jitter. Read down the shipped column and you can predict who diverts: Road and Gravel take the valley, MTB and Touring already had a cheaper way-kind line and keep it.</figcaption>
</figure>

Profiles are the one part of the routing graph that **is** configurable (the topology is not). The web builder's advanced editor has a **Bike-profiles panel**: one card per profile — its name, its climb weight, and a grid with one cell per way-kind and surface class, each with a **forbidden** toggle for the `0` case — schema-driven from the same class vocabulary above, and it enforces the ≥ 1.0× floor in the editor so a config that the packer would reject can't be exported in the first place. Like every other field, it round-trips to a plain CLI config.

The **climb weight** is a per-profile `0..255` field of that same config — flat metres charged per metre of ascent, described and bounded in the [packer's JSON Schema](src:host/obc-pack/schema/config.schema.json) alongside the multipliers — and it sits in each card's header, beside the name. The panel is careful not to treat it as a multiplier, because it isn't one: whole numbers over the schema's full `0..255` rather than a `≥ 1.0` floor (the term is *added*, so no value of it can make an edge cheaper than the crow flies — there is no admissibility bound to defend), and no inheritance, because a climb weight falls back to nothing. A profile that states none simply *is* climb-blind, which is how the packer reads it — so a config written before v12 opens showing a plain `0`, not a blank. The shipped values are the ones in [`builder/presets/schema.json`](src:builder/presets/schema.json) — Road 10, Gravel 8, MTB 6, Touring 8 — and they are what every published map is baked with.

### Building the LOD pyramid

The file is a [pyramid of detail levels](../formats/#the-file-front-to-back), and the packer builds each one independently. Two knobs from the config drive it: every feature's **`min_lod`** (the coarsest tier it's allowed into) and each tier's **simplify tolerance**. So the country tier holds a handful of feature types, heavily simplified; the street tier holds everything at (near-)full detail. The presets pick each tolerance pixel-accurately: one pixel at the finest scale the tier is drawn at, which is the next finer tier's `max_mpp` ceiling. The finest tier has no finer fallback, but still carries a small **sub-pixel** simplify (0.5–2 m in the presets) — enough to shed OSM's redundant, GPS-jitter vertices, which dominate the renderer's point budget at street zoom, with no visible change.

An optional third knob, **`min_area_px`**, declutters the coarse tiers: after simplify, a **polygon** whose projected area falls below that many square pixels (measured at the tier's finest on-screen scale) is dropped, so a region's worth of sub-pixel forest and landuse slivers stops crowding the render's [point budget](../rendering/#4-decode-by-priority). It never touches the finest tier, and lines are left alone — an OSM way is stored as many short segments, so an area test would drop a road's shortest links and leave it holed. The same threshold also trims **sub-pixel holes** out of the polygons it keeps: a hole smaller than a pixel is painted straight over anyway, so dropping it is invisible yet frees a ring plus its vertices.

A fourth knob, **`merge_fills`**, attacks redundancy. Rural OSM is wall-to-wall farmland and meadow parcels, each mapped as its own way, and on a 64-colour panel many landuse types collapse onto the same green — every shared parcel boundary stored twice and drawn as two polygons even though the screen shows one flat fill. With the flag on, the packer unions every polygon whose style renders pixel-identically (same `z_index`, `color`, `priority`, and no `color2` — an outline's walls must not be dissolved) into one shape per tier. The union runs **before** simplify: adjacent parcels share boundary *nodes*, so unioning first dissolves them exactly, where simplifying first would nudge each copy independently and leave hairline cracks along every seam.

A fifth knob, **`merge_lines`**, is the same idea for lines — and it targets the budget a dense frame runs out of first. OSM splits one continuous road, river, or railway into many `way`s, each packed as its own line feature; since a line's whole look lives in the [style table](../formats/#the-header), same-styled fragments that meet end-to-end draw identically to one polyline. The packer stitches them into maximal polylines per tier, stopping at genuine junctions so distinct roads never fuse; each join reclaims one **[span](../rendering/#4-decode-by-priority) and one ring**, and span/ring budgets saturate long before the point budget on a dense map. Two lines stitch only if their styles agree on the full render identity (`z_index`, `color`, `weight`, `priority`, `dashed`, `color2`), and stitching runs before simplify so a merged run's now-interior junction vertices simplify away too. The one visible difference is an improvement: a dashed or cased line's pattern runs continuously across a former join instead of restarting.

All three knobs are off by default, so the packed bytes are unchanged unless you ask for them.

<figure class="fig">
<svg viewBox="0 0 720 270" role="img" aria-label="A pool of features each tagged with a min-LOD flows into three tiers. The country tier takes only features with min-LOD 0 and simplifies them at 120 metres. The region tier adds min-LOD 1 features at 18 metres. The street tier adds everything at full detail. Each tier becomes its own quadtree.">
  <defs>
    <marker id="aP5" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Each tier: filter by min_lod, dissolve fills + stitch lines, simplify, cull tiny areas, then quadtree</text>

  <!-- feature pool -->
  <rect class="d-panel-2" x="30" y="70" width="140" height="130" rx="10" />
  <text class="d-tag" x="44" y="90">features</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="44" y="112" style="font-size:9.5px">coast · sea  min 0</text>
    <text class="d-sub" x="44" y="132" style="font-size:9.5px">motorway   min 0</text>
    <text class="d-sub" x="44" y="152" style="font-size:9.5px">forest     min 1</text>
    <text class="d-sub" x="44" y="172" style="font-size:9.5px">footway    min 2</text>
    <text class="d-sub" x="44" y="192" style="font-size:9.5px">building   min 2</text>
  </g>

  <!-- tiers -->
  <g>
    <line class="d-flow" x1="174" y1="100" x2="214" y2="92" marker-end="url(#aP5)" />
    <rect class="d-panel" x="220" y="62" width="220" height="42" rx="8" />
    <text class="d-label" x="234" y="80">LOD 0 · country</text>
    <text class="d-sub" x="234" y="96" style="font-size:9px">min_lod ≤ 0 · simplify 120 m</text>

    <line class="d-flow" x1="174" y1="135" x2="214" y2="135" marker-end="url(#aP5)" />
    <rect class="d-panel" x="220" y="114" width="220" height="42" rx="8" />
    <text class="d-label" x="234" y="132">LOD 1 · region</text>
    <text class="d-sub" x="234" y="148" style="font-size:9px">+ min_lod ≤ 1 · simplify 18 m</text>

    <line class="d-flow" x1="174" y1="170" x2="214" y2="178" marker-end="url(#aP5)" />
    <rect class="d-panel" x="220" y="166" width="220" height="42" rx="8" />
    <text class="d-label" x="234" y="184">LOD 2 · street</text>
    <text class="d-sub" x="234" y="200" style="font-size:9px">+ everything · full detail</text>
  </g>

  <!-- each → quadtree -->
  <line class="d-flow" x1="444" y1="83"  x2="492" y2="83"  marker-end="url(#aP5)" />
  <line class="d-flow" x1="444" y1="135" x2="492" y2="135" marker-end="url(#aP5)" />
  <line class="d-flow" x1="444" y1="187" x2="492" y2="187" marker-end="url(#aP5)" />
  <g>
    <rect x="500" y="64" width="56" height="38" rx="5" class="d-muted" /><text class="d-sub" x="528" y="87" text-anchor="middle" style="font-size:9px">quadtree</text>
    <rect x="500" y="116" width="56" height="38" rx="5" class="d-muted" /><text class="d-sub" x="528" y="139" text-anchor="middle" style="font-size:9px">quadtree</text>
    <rect x="500" y="168" width="56" height="38" rx="5" class="d-muted" /><text class="d-sub" x="528" y="191" text-anchor="middle" style="font-size:9px">quadtree</text>
  </g>
  <text class="d-sub" x="600" y="120" style="font-size:10px">→ the LOD</text>
  <text class="d-sub" x="600" y="136" style="font-size:10px">pyramid in</text>
  <text class="d-sub" x="600" y="152" style="font-size:10px">the .obcm</text>
</svg>
<figcaption>The per-feature simplify runs in parallel across CPU cores (each worker with its own GEOS context, so no geometry crosses threads) while feature order is preserved. This is exactly the structure the device exploits at the other end: zooming out reads one small coarse tier instead of decimating fine geometry — see <a href="../rendering/#2-level-of-detail-pick-the-right-layer">LOD selection</a>.</figcaption>
</figure>

```rust
for i in 0..lods.len() {                              // coarse (0) → fine
    let tol = lods[i].simplify_m / M_PER_DEG;
    let cull_mpp = lods.get(i + 1).and_then(|l| l.max_mpp); // finest tier: None → never cull
    let level: Vec<(u8, Geom)> = features
        .par_iter()                                   // rayon — one GEOS context per thread
        .filter(|f| f.min_lod <= i)                   // the LOD gate
        .filter_map(|f| {
            let g = simplify(&f.geom, tol);
            let too_small = cull_mpp                  // drop sub-min_area_px polygons (lines: never)
                .is_some_and(|mpp| footprint_below(&g, mpp, lods[i].min_area_px));
            (!too_small).then(|| (f.style_id, g))
        })
        .collect();                                   // order preserved
    let tree = build_lod(level, global_bbox, chunk_size); // this tier's quadtree
    serialize_and_stream(tree);                       // write to disk, then drop
}
```

### The quadtree: packing geometry into chunks

Within a tier, features are bucketed into a quadtree over the global bounding box. A node holds every feature reaching it; if their combined packed size — budgeted at `12 + point_count·4` bytes each, a deliberate over-estimate of the [7-or-12-byte header](../formats/#features-an-anchor-then-deltas) and 16-bit-worst-case deltas — fits the chunk size **and** every feature fits the reader's per-feature ring cap (32 rings: exterior + 31 holes), it becomes a leaf, otherwise it **splits** into four (NW · NE · SW · SE), hands each child the features it reaches, and recurses. The ring test matters because bytes don't imply it: at a coarse tier a [merged](#building-the-lod-pyramid) forest can carry dozens of clearings on a handful of simplified vertices, and the device would drop such a feature whole rather than truncate it — splitting instead clips it, spreading the holes across the children. A feature that straddles a child boundary is **clipped** to each child's box. The four child subtrees are built **in parallel** — they share no state, and only plain geometry (never a live GEOS handle) crosses a thread — which is what keeps the per-LOD build, otherwise the packer's heaviest stage, off the critical path.

<figure class="fig">
<svg viewBox="0 0 720 290" role="img" aria-label="A region with features being bucketed into a quadtree. A dense corner has been subdivided into four smaller cells, one of them subdivided again. A line feature crossing a cell boundary is clipped into two pieces, one per cell.">
  <text class="d-tag" x="20" y="24">A leaf splits when its chunk fills; straddlers are clipped</text>

  <!-- region -->
  <rect x="40" y="48" width="256" height="256" fill="none" stroke="#3c6b39" stroke-width="1.6" />
  <!-- root split -->
  <line x1="168" y1="48" x2="168" y2="304" stroke="#3c6b39" stroke-width="1.3" />
  <line x1="40" y1="176" x2="296" y2="176" stroke="#3c6b39" stroke-width="1.3" />
  <!-- NE subdivided (dense) -->
  <line x1="232" y1="48" x2="232" y2="176" stroke="#7c9a63" stroke-width="1" />
  <line x1="168" y1="112" x2="296" y2="112" stroke="#7c9a63" stroke-width="1" />
  <!-- one sub-cell subdivided again -->
  <line x1="264" y1="112" x2="264" y2="176" stroke="#9aa884" stroke-width="0.8" />
  <line x1="232" y1="144" x2="296" y2="144" stroke="#9aa884" stroke-width="0.8" />
  <!-- dense dots in NE -->
  <g fill="#3c6b39"><circle cx="248" cy="128" r="1.6"/><circle cx="256" cy="158" r="1.6"/><circle cx="276" cy="124" r="1.6"/><circle cx="284" cy="160" r="1.6"/><circle cx="240" cy="160" r="1.6"/><circle cx="272" cy="150" r="1.6"/></g>
  <text class="d-sub" x="232" y="70" text-anchor="middle" style="font-size:9px">dense → deeper</text>
  <!-- a polygon in SW -->
  <path d="M70 210 L130 205 L140 260 L80 270 Z" fill="#cfe0c2" stroke="#3c6b39" stroke-width="1.2" />
  <!-- a line straddling the vertical boundary -->
  <line x1="120" y1="120" x2="220" y2="150" stroke="#cf6a2a" stroke-width="2.5" />
  <circle cx="168" cy="134" r="3.5" class="d-hot-fill" />
  <text class="d-sub" x="120" y="106" style="fill:#a9501c;font-size:9px">clipped at the boundary →</text>

  <!-- right notes -->
  <text class="d-label" x="330" y="84">leaf fills:</text>
  <text class="d-sub" x="330" y="104" font-family="var(--mono)" style="font-size:11px">size += 12 + pts·4</text>
  <text class="d-sub" x="330" y="124" style="font-size:11px">&gt; chunk_size → split 4</text>
  <line class="d-stroke" x1="330" y1="140" x2="690" y2="140" style="stroke:#9aa884" />
  <text class="d-sub" x="330" y="164" style="font-size:11px">a straddling feature is clipped to</text>
  <text class="d-sub" x="330" y="180" style="font-size:11px">each child's box, so every leaf's</text>
  <text class="d-sub" x="330" y="196" style="font-size:11px">geometry is self-contained — which</text>
  <text class="d-sub" x="330" y="212" style="font-size:11px">is what lets the device decode one</text>
  <text class="d-sub" x="330" y="228" style="font-size:11px">chunk without touching its neighbours.</text>
  <text class="d-sub" x="330" y="258" style="font-size:10px;fill:#a9501c">this is the same tree the device walks to cull</text>
</svg>
<figcaption>Splitting at floor-division midpoints — and clipping straddlers per child — is what makes a chunk independently decodable: each holds whole geometry anchored to its own corner. That's the exact tree the reader walks at render time, so the packer's subdivision math and the renderer's must agree byte for byte. (A recursion guard stops splitting below 10 µdeg, so degenerate density can't recurse forever.)</figcaption>
</figure>

```rust
let total: usize = feats.iter().map(|f| 12 + pt_count(&f.geom) * 4).sum();
let fits = total <= chunk_size && feats.iter().all(|f| ring_count(&f.geom) <= MAX_FEAT_RINGS);
if fits || !splittable {
    return Node::Leaf { bbox, features: feats };   // fits the chunk + the reader's caps → a leaf
}
// too big → split NW/NE/SW/SE, clip straddlers into each child, recurse in parallel
let (nw, ne, sw, se) = distribute_to_quadrants(feats, bbox);
rayon::join(|| (build(nw), build(ne)), || (build(sw), build(se)))
```

That this is the *same* quadtree the device walks closes the loop with the other pages: the packer writes it, the [format](../formats/#the-quadtree-index) stores it as a flat `u32` array, and the [renderer](../rendering/#3-the-quadtree-cull-only-the-chunks-you-can-see) walks it to cull.

### The builder

The builder no longer runs the packer as a product feature. The expensive,
schema-dependent work happens once in the bakery; both shipping products consume
the same published cell catalog. That removes a second map pipeline and makes
"same coverage + same skin" mean identical assembled bytes on the web and
desktop.

Coverage is composed from three kinds of parts:

- named regions, whose exact per-band cell ids are stored in the catalog;
- boxes, resolved directly against the global cell grid;
- corridors, formed by buffering dropped GPX routes and resolving the resulting
  shape against that same grid.

The parts are unioned before pricing, so overlaps never charge or download a
cell twice. Partial cells remain visible as warnings. The selected cells are
downloaded with byte-length and SHA-256 checks, then passed to
[`obc-web-assemble`](src:apps/obc-web-assemble) in a worker. Cancelling
terminates the worker; there is no verification bypass.

**Elevation rides along with the selection, and there is no switch for it.** The
terrain squares a map needs are the ones its selection touches — the same intersect
rule the bands use, run on the [terrain lattice](../terrain/) — so they are resolved,
priced and downloaded exactly like cells, then handed to the assembler, which
*places* them into one raster file rather than grafting anything. The summary card
gains two lines and no controls: the raster's own size, stated separately from the
map's because a rider may legitimately take one without the other, and the dataset's
required credit — read **from the catalog**, never hard-coded, so a change of source
carries its own notice instead of leaving a stale string behind. There is deliberately
no toggle: elevation is roughly five per cent of a download, and a switch would ask a
rider to decide something they have no way to decide well.

An assembled map — even a one-shard one — names its raster in the manifest and writes
it as `MS<id>.OBD`. A map that never went through the assembler, such as one packed
straight from an extract for the simulator, gets the same file as a plain **sidecar**
next to the `.obcm`, under the same stem. Those are deliberately the same convention
seen from two sides: `MS<id>.OBD` *is* the sidecar of `MS<id>.OBS`, so a host that
resolves terrain by looking beside the map and one that reads the manifest role open
the same file. What the manifest adds is the two things a filename cannot say — that
this set claims a raster, and how many bytes of one — which is what stops a leftover
`.OBD` from a replaced set being read as this map's terrain.

The same digest appears in each referenced object's published key. Cells,
per-band indexes, region cell lists, and previews are therefore immutable below
one root: a CDN can keep serving an older root and its objects while a new root
propagates, and unchanged planet cells keep their existing keys instead of being
uploaded into a duplicate generation directory.

### Editing a skin

Default and Dusk remain catalog objects with digest-pinned Teningen preview
images. **Customize** clones either one into the product skin editor shared by
the website and desktop app. It exposes only colours (including the optional
second colour), line widths, dashes, drawing order, and the route-marker colour.
Feature types, style ids, LODs, routing, and the inherited overflow priority are
not editable there, so a product restyle cannot turn into a new cell schema.

The editor lazy-loads one canonical Teningen `.obcm` and
[`obc-skin-preview`](src:apps/obc-skin-preview) only while it is open. Every edit
restamps the resident style table and renders a 240×240 scene through
`obc-reader` and `obc-render`; the preview therefore uses the device palette and
renderer rather than a browser approximation. It opens over Teningen at
5 m/px, then supports pointer-drag panning, cursor-centred wheel zoom, keyboard
camera controls, and a reset button. The camera stays inside the fixture while
the production LOD selector moves across the complete published ladder; the
caption reports its actual m/px and selected LOD. Closing the editor releases
the map, renderer, and frame.

Saving creates a browser-local custom skin. Its record is pinned to the current
catalog schema id and revision and must still cover the exact schema-ordered
feature list when reloaded; stale or malformed records are ignored. The custom
skin is handed to the same assembler as a hosted skin, so it changes the stamped
presentation bytes without changing which cells are fetched. Its picker card is
also a real Teningen render: on load, one temporary preview instance restamps
all saved skins in turn and keeps only their RGBA frames. PNG/base64 thumbnails
are not persisted in browser storage, so they cannot consume the storage quota
or outlive a renderer/schema update.

### One source, three hosts

One Svelte source is compiled for the static website, the Tauri desktop app, and
the local maintainer server. Host modules provide only capabilities at the
edges—catalog transport, file output, USB, ride storage—not alternate selection
or assembly algorithms.

| | Static website | Desktop app | Maintainer server |
| :-- | :-- | :-- | :-- |
| Catalog coverage UI | yes | yes | yes |
| Regions, boxes, GPX corridors | yes | yes | yes |
| Shared wasm assembly | yes | yes | yes |
| Product skin editor | yes | yes | yes |
| Output | browser downloads | grouped local folder | browser downloads |
| Advanced schema editor | no | no | yes |
| Native fixed-crop schema preview | no | no | yes |
| Product PBF build | no | no | no |
| Managed ride library and device dashboard | no | yes | no |

The website uses browser fetch. The desktop root, satellites, and cells use its
native HTTP client and are restricted to the configured catalog origin; this is
how the same catalog works without widening the webview's content-security
policy. The desktop writes every file of one assembled volume set into a unique
folder under `Documents/OpenBikeComputer`, using a temporary file and atomic
rename for each part. It closes the folder only after the assembler emits and
verifies the manifest; cancellation or failure discards the incomplete folder.
In a browser, files already handed to the download manager cannot be recalled,
so the failure card names how many incomplete downloads the user should discard.
Saving changes where bytes land, never what the assembler emitted.

The local Python server remains useful while developing the one hosted schema.
Its Maps tab resolves `OBC_CATALOG_URL` at server runtime and proxies only the
configured catalog tree, avoiding both a stale build-time `./data/catalog.json`
fallback and a dependency on object-storage CORS. Its Advanced route reads the
real packer JSON Schema, exports a complete config, and keeps working state in
the browser. On a new browser profile, the route snapshots the sole buildable
schema preset once; a restored or imported working config always wins, and a
future shelf with multiple buildable schemas requires an explicit choice rather
than silently picking one.

That route also provides a deliberately **semi-live schema lab**. One setup
command reuses or downloads Freiburg-regbez and has Osmium atomically prepare a
small, reference-complete crop around Teningen. Schema edits are debounced and
cancel the superseded request; the server gives only that fixed source, one
temporary config, and one temporary output to the exact native `obc-pack`
binary. There is no request-controlled path or command, only one pack process
at a time, and a timeout terminates it. Packing the small crop typically takes
5–15 seconds, so this is feedback after an edit rather than a frame-by-frame
restyle.

The resulting OBCM is opened without restamping and rendered through the same
`obc-reader` + `obc-render` bridge on a 240×320 device map plane. Controls visit
every authored LOD by its real m/px dispatch. The panel reports features tried,
drawn and dropped; chunks and points; the 2,048-point/32-ring per-feature decode
limits; and the production 1,152-span/4,768-point/1,024-ring frame limits and
errors. It is therefore honest about the device's selection pressure, not a
browser drawing of what the schema might mean.

This remains a maintainer-only preview, not a fourth product build path. It
never cuts published cells, bakes a region, or teaches the website or desktop
app to accept a local PBF. An exported schema becomes published only through an
explicit maintainer bake; riders never accidentally trigger a country-scale
compile.

### Device and ride surfaces

The assembled multi-file set can either be saved or streamed directly to a
connected device. Direct send keeps one verified shard in flight, waits for the
device before releasing it, and commits the manifest last; cancellation abandons
an incomplete set. Manual single-file upload remains for maps obtained elsewhere.
There is no old whole-region catalog fallback hiding behind that button.

Routes and firmware still use the shared object protocol. The cable also runs in
the other direction: the desktop app maintains a durable ride library, writes a
GPX plus the device's original ride object, and acknowledges a ride only after
the files and index are fsynced. The browser offers one-shot export and does not
acknowledge it as durable.

With a connected device, the desktop Device page lists routes and trips, supports
rename/delete/group operations, and keeps rides read-only so a ride that exists
only on the card cannot be lost from a desk.

### Where the hosted tier lives

The website has no map-building backend. It is deployed as static assets, while
the catalog and cells live on separately published object storage. The catalog
root can therefore change without redeploying the site, and the bakery can keep
its essential content-first, root-last publish order.

All frontend asset URLs remain mount-relative, so the same site artifact works
at a domain root or under a project-pages prefix. Catalog object URLs come from
the catalog root and are digest-verified before use.
## Following a route

You plan a route elsewhere and upload a GPX. Converting it to an `.obcr` — decimating the geometry for drawing while keeping the stats exact, then chunking it with shared seams — is covered on the [data formats](../formats/#obcr-the-route) page. The converter is one portable `no_std` routine, so it runs on the device, in the simulator, or in a browser tab.

<figure class="fig">
<svg viewBox="0 0 760 322" role="img" aria-label="Converting a GPX track to an OBCR route in one streaming pass. Left panel, the shape: the stored line keeps only the corners (and one vertex at least every 1.2 km) — vertices within 1 metre of the line between their neighbours are dropped — yet distance and climb are summed over every original point, so the stats stay exact even though the stored geometry is sparse. Right panel, the climb: a raw elevation trace is integrated through a 3-metre dead-band; small wiggles inside the band book no ascent, and only once the trace leaves the band is the climb booked and the reference re-anchored. The same dead-band is shared by the elevation profile and the live barometric climb on the device.">
  <text class="d-tag" x="20" y="22">GPX → OBCR · one streaming pass</text>
  <text class="d-sub" x="20" y="38" style="font-size:9.5px">decimate the geometry, but measure distance + climb from every raw point</text>
  <line x1="384" y1="58" x2="384" y2="300" stroke="#9aa884" stroke-opacity="0.45" stroke-width="1" />
  <text class="d-sub" x="26" y="62" style="font-size:9px;fill:#6b7758">① the shape — decimate, keep the corners</text>
  <polyline points="55,206 130,131 225,166 330,116" fill="none" stroke="#cf6a2a" stroke-width="2" />
  <circle cx="75" cy="187" r="3" fill="#ece8cf" stroke="#9aa884" stroke-width="1.2" />
  <circle cx="92" cy="169" r="3" fill="#ece8cf" stroke="#9aa884" stroke-width="1.2" />
  <circle cx="110" cy="150" r="3" fill="#ece8cf" stroke="#9aa884" stroke-width="1.2" />
  <circle cx="155" cy="142" r="3" fill="#ece8cf" stroke="#9aa884" stroke-width="1.2" />
  <circle cx="180" cy="151" r="3" fill="#ece8cf" stroke="#9aa884" stroke-width="1.2" />
  <circle cx="200" cy="160" r="3" fill="#ece8cf" stroke="#9aa884" stroke-width="1.2" />
  <circle cx="255" cy="156" r="3" fill="#ece8cf" stroke="#9aa884" stroke-width="1.2" />
  <circle cx="285" cy="140" r="3" fill="#ece8cf" stroke="#9aa884" stroke-width="1.2" />
  <circle cx="305" cy="129" r="3" fill="#ece8cf" stroke="#9aa884" stroke-width="1.2" />
  <circle cx="55" cy="206" r="3.6" fill="#cf6a2a" />
  <circle cx="130" cy="131" r="3.6" fill="#cf6a2a" />
  <circle cx="225" cy="166" r="3.6" fill="#cf6a2a" />
  <circle cx="330" cy="116" r="3.6" fill="#cf6a2a" />
  <line class="d-stroke" x1="92" y1="169" x2="92" y2="205" style="stroke-width:0.9;stroke:#9aa884;stroke-dasharray:2 2" />
  <text class="d-sub" x="86" y="220" style="font-size:8.5px">dropped — within 1 m of the line</text>
  <text x="130" y="121" text-anchor="middle" style="font-family:var(--mono);font-size:8.5px;fill:#a9501c">kept — a corner</text>
  <text class="d-sub" x="330" y="107" text-anchor="middle" style="font-size:8.5px;fill:#6b7758">+ force-keep every ~1.2 km</text>
  <rect x="34" y="246" width="320" height="42" rx="7" style="fill:#eef2df;stroke:#9aa884;stroke-width:0.8" />
  <text x="194" y="263" text-anchor="middle" style="font-family:var(--mono);font-size:9px;fill:#3c6b39">distance &amp; climb are summed over EVERY raw point</text>
  <text x="194" y="277" text-anchor="middle" style="font-family:var(--mono);font-size:9px;fill:#3c6b39">— exact, though the stored line is decimated</text>
  <text class="d-sub" x="402" y="62" style="font-size:9px;fill:#6b7758">② the climb — a ±3 m dead-band</text>
  <polyline points="430.0,232.0 444.5,237.0 459.0,227.0 473.5,229.5 488.0,222.0 502.5,212.0 517.0,202.0 531.5,192.0 546.0,187.0 560.5,189.5 575.0,197.0 589.5,187.0 604.0,172.0 618.5,157.0 633.0,147.0 647.5,152.0 662.0,137.0 676.5,122.0 691.0,112.0 705.5,117.0 720.0,107.0" fill="none" stroke="#9aa884" stroke-width="1.3" stroke-opacity="0.65" />
  <rect x="531.5" y="177.0" width="72.5" height="30.0" fill="#cf6a2a" fill-opacity="0.08" stroke="#cf6a2a" stroke-opacity="0.35" stroke-width="0.8" stroke-dasharray="3 3" />
  <text x="608.0" y="195.0" style="font-family:var(--mono);font-size:8.5px;fill:#a9501c">±3 m</text>
  <polyline points="430.0,232.0 502.5,232.0 502.5,212.0 531.5,212.0 531.5,192.0 604.0,192.0 604.0,172.0 618.5,172.0 618.5,157.0 662.0,157.0 662.0,137.0 676.5,137.0 676.5,122.0 720.0,122.0 720.0,107.0 720.0,107.0" fill="none" stroke="#cf6a2a" stroke-width="2" />
  <text class="d-sub" x="430" y="252" style="font-size:8.5px;fill:#6b7758">along the route →</text>
  <text class="d-sub" x="402" y="100" style="font-size:8.5px;fill:#6b7758">elev</text>
  <text class="d-sub" x="455" y="210" style="font-size:8.5px;fill:#6b7758">wiggle &lt; 3 m → ignored</text>
  <text x="592" y="120" text-anchor="middle" style="font-family:var(--mono);font-size:8.5px;fill:#a9501c">past ±3 m → book + re-anchor</text>
  <rect x="410" y="266" width="316" height="22" rx="7" style="fill:#eef2df;stroke:#9aa884;stroke-width:0.8" />
  <text x="568" y="281" text-anchor="middle" style="font-family:var(--mono);font-size:8.5px;fill:#3c6b39">one dead-band, shared: converter · profile · live baro climb</text>
</svg>
<figcaption>The GPX → OBCR conversion is one streaming pass. It <b>decimates</b> the geometry for storage — dropping any vertex within a metre of the line between its neighbours, keeping the corners (and one vertex at least every ~1.2 km, so a long straight still holds its shape and the deltas stay in <code>int16</code>) — but accumulates <b>distance and climb over every original point</b>, so the route's stats are exact even though the stored line is sparse. Climb runs through a <b>±3 m dead-band</b>: a move smaller than that books nothing and doesn't move the reference, so sampling jitter can't inflate the ascent. That one dead-band is shared by the converter, the elevation profile, the device's live barometric climb, and the on-device router's own route emit (which fills a planned route's heights from the terrain carried beside the map) — so those numbers can't drift apart.</figcaption>
</figure>

What's left is the interesting part of *following*: snapping each GPS fix onto that route.

### Map-matching: a forward-biased cursor

The matcher keeps a cursor — *which segment of the route you're on* — and for each fix searches a **bounded window** around it for the nearest segment. On-route that window is small and looks mostly *forward*, which is the whole trick: on a loop or an out-and-back, the cursor follows you forward instead of snapping to the nearby segment you rode an hour ago.

<figure class="fig">
<svg viewBox="0 0 720 290" role="img" aria-label="A route polyline with a cursor on it. A bounded forward window of dozens of segments ahead (and a few behind) is highlighted around the cursor. A GPS fix off to the side is projected onto the nearest segment in that window, giving a progress distance along the route and a cross-track distance to it. A far fix is flagged off-route and freezes progress.">
  <text class="d-tag" x="20" y="24">Snap each fix to the nearest segment in a forward window</text>

  <!-- route -->
  <path d="M40 210 C 120 120, 200 120, 270 170 C 320 205, 380 150, 470 120" fill="none" stroke="#9aa884" stroke-width="3" />
  <!-- forward window (highlighted) -->
  <path d="M200 138 C 235 128, 255 150, 285 168" fill="none" stroke="#cf6a2a" stroke-width="5" stroke-opacity="0.3" />
  <text class="d-sub" x="262" y="114" text-anchor="middle" style="fill:#a9501c;font-size:9px">forward window</text>
  <!-- cursor -->
  <circle cx="210" cy="135" r="5" class="d-hot-fill" /><text class="d-sub" x="170" y="130" style="font-size:9px">cursor</text>
  <!-- a fix near the route -->
  <circle cx="262" cy="178" r="5" class="d-water" />
  <text class="d-sub" x="274" y="188" style="font-size:9px">fix</text>
  <!-- cross-track to nearest segment -->
  <line x1="262" y1="178" x2="259" y2="159" stroke="#33575b" stroke-width="1.5" stroke-dasharray="3 2" />
  <circle cx="259" cy="159" r="3" class="d-water" />
  <text class="d-sub" x="300" y="170" style="font-size:9px">cross-track dist</text>
  <!-- progress label -->
  <text class="d-sub" x="90" y="205" style="font-size:9px">← progress along the route</text>

  <!-- off-route fix -->
  <circle cx="430" cy="210" r="5" class="d-muted" stroke="#c0492e" stroke-width="1.5" />
  <text class="d-sub" x="430" y="230" text-anchor="middle" style="font-size:9px">far fix</text>
  <line x1="430" y1="210" x2="452" y2="135" stroke="#c0492e" stroke-width="1.2" stroke-dasharray="3 3" />
  <text class="d-sub" x="430" y="244" text-anchor="middle" style="fill:#c0492e;font-size:9px">off-route → freeze</text>

  <!-- hysteresis band note -->
  <rect class="d-panel-2" x="500" y="60" width="196" height="120" rx="10" />
  <text class="d-tag" x="516" y="80">off-route hysteresis</text>
  <text class="d-sub" x="516" y="102" style="font-size:10px">&gt; 25 m  → off-route</text>
  <text class="d-sub" x="516" y="122" style="font-size:10px">&lt; 15 m  → back on</text>
  <text class="d-sub" x="516" y="146" style="font-size:9px">the gap is the dead-band that</text>
  <text class="d-sub" x="516" y="160" style="font-size:9px">keeps the flag from flapping</text>
  <text class="d-sub" x="516" y="174" style="font-size:9px">on GPS jitter at the edge</text>
</svg>
<figcaption>The window is the key to cost and correctness both: it makes each match O(window) instead of O(route) — only the few chunks it spans are decoded — and its forward bias stops a second lap from latching onto the first. The very first fix is the exception: it scans the whole route once to lock on from anywhere, preferring the <i>earliest</i> of near-equal matches so an out-and-back doesn't start at the finish line.</figcaption>
</figure>

```rust
pub struct Match {
    pub progress_m: u32, // distance travelled along the route — frozen while off-route
    pub off_route: bool, // nearest segment is past the hysteresis threshold
    pub dist_m: u32,     // cross-track distance to the route — always live
}
```

Two rules keep it honest when you wander. **Off-route freezes progress**: a fix 200 m away can't drag your route position with it — but the search window *widens* so a rejoin further along is still found. And the off-route flag has **hysteresis** (off past 25 m, back under 15 m), so it doesn't flicker while you straddle the threshold. The live cross-track distance feeds the UI's [off-route readout](../ui/#the-whole-flow), and `progress_m` drives "distance to go." The projection it uses is shared with the GPX converter, so the matcher and the stored geometry measure the route the same way.

```rust
let (back, fwd) = if !self.started {
    (i64::MAX, i64::MAX)        // first fix: scan the whole route to lock on
} else if self.off_route {
    (BACK_SEGS, FWD_SEGS_OFF)   // off-route: widen the forward search to find a rejoin
} else {
    (BACK_SEGS, FWD_SEGS_ON)    // on-route: a tight forward window — O(window), forward-biased
};
```

---

## Where this lives

- The packer pipeline, end to end and callable from either host: [`obc-pack/src/pipeline.rs`](src:host/obc-pack/src/pipeline.rs); the phase vocabulary and the cancel token it carries: [`obc-pack/src/progress.rs`](src:host/obc-pack/src/progress.rs); the CLI around it: [`obc-pack/src/main.rs`](src:host/obc-pack/src/main.rs)
- Config + first-match styling: [`obc-pack/src/config.rs`](src:host/obc-pack/src/config.rs)
- The config's generated JSON Schema fallback (served from the live model by `obc-pack schema`): [`obc-pack/schema/config.schema.json`](src:host/obc-pack/schema/config.schema.json)
- The shared catalog builder UI and maintainer FastAPI host: [`builder/`](src:builder); the desktop shell and native transport: [`obc-desktop`](src:apps/obc-desktop); browser-side GPX↔OBCR conversion and cell assembly: [`obc-web-convert`](src:apps/obc-web-convert) and [`obc-web-assemble`](src:apps/obc-web-assemble)
- OSM ingest + relation assembly: [`obc-pack/src/ingest.rs`](src:host/obc-pack/src/ingest.rs)
- The quadtree build: [`obc-pack/src/quadtree.rs`](src:host/obc-pack/src/quadtree.rs)
- Land generation: [`obc-pack/src/land.rs`](src:host/obc-pack/src/land.rs)
- POI extraction, classification, name folding + dedup: [`obc-pack/src/poi.rs`](src:host/obc-pack/src/poi.rs)
- The navigation-graph build (routable filter, junction detection, edge split + dedup) **and the per-edge ascent integration**: [`obc-pack/src/nav.rs`](src:host/obc-pack/src/nav.rs)
- The DEM stage that runs before the packer, never inside it — GLO-30 → terrain cells: [`obc-dem`](src:host/obc-dem); the sampler both it and the firmware use: [`obc-elevation`](src:firmware/obc-elevation)
- The on-device router (snap + profile-weighted A\* + OBCR emit): [`obc-route/src/nav.rs`](src:firmware/obc-route/src/nav.rs)
- The route map-matcher: [`obc-route/src/matcher.rs`](src:firmware/obc-route/src/matcher.rs)
- GPX → OBCR conversion: [`obc-route/src/convert.rs`](src:firmware/obc-route/src/convert.rs) — one routine, three hosts (device, simulator, browser)

This is the offline bookend to the on-device story: the packer produces the [map format](../formats/) the [renderer](../rendering/) draws, and the matcher drives the navigation the [UI](../ui/) shows. The raster the climb weights are measured against — where it comes from, what it costs, and what happens without it — is [terrain & elevation](../terrain/).
