---
title: Packer and routing
description: How OpenBikeComputer builds maps, navigation graphs, routes, and route matches.
copy: ai
---

# Packer and routing

[`obc-pack`](src:host/obc-pack) converts OpenStreetMap extracts into the OBCM map format, with the
POIs, the opening hours, the contours, and the navigation graph the device plans routes on. The
GPX converter writes OBCR routes, and the matcher puts the live position on the active route.

Everything expensive happens here, on a computer, so that the device only reads.

## Packing a map

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 820px">
<svg viewBox="0 0 820 278" role="img" aria-label="The packer pipeline as a trail: starting from one or more OSM .pbf files plus a config, the stages are ingest (which also merges and crops), compute bounding box, generate land, trace contours from terrain, build the per-LOD pyramid (simplify then quadtree), and serialize, ending at a .obcm file. Ingest and the per-LOD build are marked as the expensive stages; contour tracing only runs when the config asks for it and terrain was supplied.">
  <text class="d-tag" x="20" y="24">From OpenStreetMap to a device map</text>

  <!-- trail -->
  <line x1="96" y1="120" x2="742" y2="120" stroke="#5f7d3d" stroke-width="2.5" stroke-dasharray="2 7" stroke-linecap="round" />

  <!-- start -->
  <circle cx="58" cy="120" r="7" class="d-forest" />
  <text class="d-sub" x="58" y="150" text-anchor="middle">.pbf(s) +</text>
  <text class="d-sub" x="58" y="168" text-anchor="middle">config</text>

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
  <text class="d-sub" x="792" y="124" text-anchor="middle" style="font-size:12px">.obcm</text>
<text class="d-sub" x="20" y="225" text-anchor="start">Add POIs and navigation before the LOD loop. Write each LOD as it is built.</text><text class="d-sub" x="20" y="250" text-anchor="start">Contours require configuration and terrain input.</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The packer ingests OSM data, adds generated features, builds each LOD, and writes one OBCM map.</figcaption>
</figure>

The pipeline ingests the sources, measures the content, adds the land, sea, and contour features
that OSM does not supply, builds the POIs and the navigation graph, then builds each level of
detail and writes the map. It holds about one level's quadtree at a time.

### Styling: first match wins

The `features` object in the configuration is ordered, and the first matching tag key and value
gives a way its style. An exact value beats the catch-all. A way that matches nothing is dropped,
which is how the map stays small: a class of feature is in the map because the style asked for it.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
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
  <text class="d-sub" x="362" y="226" text-anchor="middle" style="font-size:12px">first key the way carries wins — building is never reached</text>

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
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>A way uses the first matching tag key. An exact value has priority over the catch-all value.</figcaption>
</figure>

The shipped style works inside the panel's 64 colors, and that limit shapes it. Two one-pixel
dashed lines have only their color to separate them, so where the colors run out a line gets a
different shape instead: cableways and ski lifts use a ticked line, the third line style beside
solid and dashed. These marks report the OSM way type. They do not rate difficulty.

### Ingest: two passes, then assemble

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
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
  <text class="d-sub" x="480" y="200" text-anchor="middle" style="font-size:12px">lake + island = 1 hole</text>

  <!-- closed-way classification note -->
  <rect class="d-panel-2" x="560" y="80" width="140" height="110" rx="10" />
  <text class="d-tag" x="574" y="100">closed way?</text>
  <text class="d-sub" x="574" y="122" style="font-size:12px">area tag → polygon</text>
  <text class="d-sub" x="574" y="142" style="font-size:12px">else → line</text>
  <text class="d-sub" x="574" y="168" style="font-size:12px">never both — a closed</text>
  <text class="d-sub" x="574" y="186" style="font-size:12px">road loop is a line only</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The ingester reads nodes and relations first. It then resolves ways and assembles area relations.</figcaption>
</figure>

The ingester reads each source twice: the first pass stores node coordinates and collects area
relations, and the second resolves ways and captures relation members. It then assembles polygons
and holes with GEOS. A closed way becomes a polygon only when its tags say it is an area, so a road
that returns to its start stays a line.

A bounding box adds a selection pass before those two. It keeps each selected way whole, with the
nodes and relation members it needs, so roads do not stop at an artificial edge. Complete objects
can reach outside the box, so the map's own bounding box is larger than the request and is not the
box to ask for next time.

Several sources can be read in the same passes. For a duplicate object the first file listed wins,
and objects are emitted in ascending identity order, so the same inputs always give the same map.

One pack covers a limited area, and the packer refuses a larger region before it reads any data.

### Land and sea

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 230" role="img" aria-label="The global land-polygons dataset is clipped to the map bounding box and subtracted from it, producing the sea complement. On the device sea is drawn over a land-coloured backdrop.">
  <defs>
    <marker id="aP4" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Sea is the clipped complement; land is the backdrop</text>

  <!-- world dataset -->
  <rect x="36" y="50" width="200" height="150" rx="8" style="fill:#bcd3da;stroke:#33575b;stroke-width:1.4" />
  <text class="d-tag" x="48" y="68" style="fill:#2c5230">global land polygons</text>
  <path d="M60 110 C 90 80, 130 90, 150 110 C 175 135, 140 175, 100 170 C 70 166, 48 140, 60 110 Z" fill="#cfe0c2" stroke="#3c6b39" stroke-width="1.2" />
  <path d="M170 150 C 190 135, 220 150, 214 175 C 208 192, 178 190, 170 175 Z" fill="#cfe0c2" stroke="#3c6b39" stroke-width="1.2" />
  <!-- bbox window -->
  <rect x="96" y="110" width="80" height="64" fill="none" stroke="#cf6a2a" stroke-width="2.2" />
  <text class="d-sub" x="136" y="105" text-anchor="middle" style="fill:#a9501c;font-size:12px">bbox</text>

  <line class="d-flow" x1="244" y1="125" x2="300" y2="125" marker-end="url(#aP4)" />
  <text class="d-sub" x="272" y="115" text-anchor="middle" style="font-size:12px">bbox − land</text>

  <!-- result: sea over land -->
  <rect x="312" y="50" width="200" height="150" rx="8" style="fill:#cfe0c2;stroke:#3c6b39;stroke-width:1.4" />
  <text class="d-tag" x="324" y="68" style="fill:#2c5230">land backdrop</text>
  <path d="M312 120 C 340 96, 380 104, 400 120 C 430 144, 470 120, 512 132 L512 50 L312 50 Z" fill="#bcd3da" stroke="#33575b" stroke-width="1.4" />
  <text class="d-sub" x="362" y="92" text-anchor="middle" style="font-size:12px">sea complement</text>

  <!-- note -->
  <rect class="d-panel-2" x="540" y="74" width="160" height="100" rx="10" />
  <text class="d-sub" x="556" y="98" style="font-size:12px">land = the lowest-z</text>
  <text class="d-sub" x="556" y="114" style="font-size:12px">style; the screen is</text>
  <text class="d-sub" x="556" y="130" style="font-size:12px">cleared to it, then</text>
  <text class="d-sub" x="556" y="146" style="font-size:12px">sea + roads paint</text>
  <text class="d-sub" x="556" y="162" style="font-size:12px">over it.</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The packer clips the land dataset to the map. It stores the sea complement when land is the backdrop.</figcaption>
</figure>

OSM supplies coastlines but no land fill. The packer clips a cached global land-polygon dataset to
the map, and where land is the backdrop it stores only the sea, because most maps are mostly land.

### Contours, traced from the terrain

The packer traces contours from the [terrain raster](../terrain/) with marching squares, and skips
a lattice square when a corner has no height. It writes ordinary line features, so contours use the
same levels, quadtree, and renderer as everything else. A flag marks them as the terrain layer,
which lets the renderer hide their ink without removing their bytes.

Their color is chosen for a 64-color panel: it keeps a clear brightness difference from rock,
forest, grass, and the land base, and stays out of the warm group that tracks and paths use.

### Extracting POIs

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 300" role="img" aria-label="POI extraction. On the left, two OSM sources: a tagged node used as-is, and a closed way whose polygon centroid becomes a point. Both are classified against a fixed table of tag-equals-value rules mapping to a category and subtype. Names are folded to ASCII and capped at 24 bytes. The packer removes repeated source identities and keeps nearby objects separate.">
  <defs>
    <marker id="aP6" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Nodes + way-centroids → classify → fold names → dedup</text>

  <!-- sources -->
  <rect class="d-panel-2" x="24" y="44" width="150" height="40" rx="9" />
  <text class="d-sub" x="40" y="62" style="font-size:12px">a tagged node</text>
  <text class="d-sub" x="40" y="77" style="font-size:12px">amenity=drinking_water</text>
  <circle cx="158" cy="64" r="4" class="d-hot-fill" />

  <rect class="d-panel-2" x="24" y="94" width="150" height="52" rx="9" />
  <text class="d-sub" x="40" y="112" style="font-size:12px">a closed way</text>
  <text class="d-sub" x="40" y="127" style="font-size:12px">tourism=camp_site</text>
  <!-- small polygon with centroid dot -->
  <path d="M120 118 L150 116 L156 136 L126 140 Z" fill="none" stroke="#3c6b39" stroke-width="1.2" />
  <circle cx="138" cy="127" r="3" class="d-hot-fill" />
  <text class="d-sub" x="120" y="152" style="font-size:12px;fill:#a9501c">→ ring centroid</text>

  <!-- classify -->
  <line class="d-flow" x1="178" y1="64"  x2="224" y2="86" marker-end="url(#aP6)" />
  <line class="d-flow" x1="178" y1="120" x2="224" y2="98" marker-end="url(#aP6)" />
  <rect class="d-panel" x="230" y="56" width="204" height="92" rx="10" />
  <text class="d-tag" x="246" y="76">fixed table · first match</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="246" y="96"  style="font-size:12px">amenity=drinking_water → Water</text>
    <text class="d-sub" x="246" y="112" style="font-size:12px">natural=spring &nbsp;&nbsp;&nbsp;&nbsp;&nbsp;→ Water</text>
    <text class="d-sub" x="246" y="128" style="font-size:12px">tourism=camp_site &nbsp;&nbsp;→ Campsite</text>
    <text class="d-sub" x="246" y="142" style="font-size:12px;fill:#a9501c">… 19 subtypes, 7 categories</text>
  </g>

  <!-- fold names -->
  <line class="d-flow" x1="438" y1="102" x2="484" y2="102" marker-end="url(#aP6)" />
  <rect class="d-panel-2" x="490" y="72" width="206" height="60" rx="10" />
  <text class="d-tag" x="506" y="90">fold name → ASCII, ≤ 24 B</text>
  <text class="d-sub" x="506" y="110" font-family="var(--mono)" style="font-size:12px">"Bäckerei Müller"</text>
  <text class="d-sub" x="506" y="124" font-family="var(--mono)" style="font-size:12px;fill:#a9501c">→ "Baeckerei Mueller"</text>

  <!-- dedup -->
  <line class="d-flow" x1="360" y1="150" x2="360" y2="196" marker-end="url(#aP6)" />
  <rect class="d-hot" x="120" y="200" width="560" height="76" rx="12" style="fill:#f8efe4" />
  <text class="d-tag" x="138" y="220" style="fill:#a9501c">dedup — repeated OSM type and ID = one POI</text>
  <!-- node + centroid merging -->
  <circle cx="168" cy="248" r="5" class="d-hot-fill" /><text class="d-sub" x="150" y="268" style="font-size:12px">copy 1</text>
  <circle cx="210" cy="248" r="4" class="d-water" /><text class="d-sub" x="196" y="268" style="font-size:12px">copy 2</text>
  <line x1="176" y1="248" x2="202" y2="248" stroke="#9aa884" stroke-width="1.2" stroke-dasharray="3 2" />
  <line class="d-flow" x1="250" y1="248" x2="300" y2="248" marker-end="url(#aP6)" />
  <circle cx="330" cy="248" r="5" class="d-hot-fill" /><text class="d-sub" x="346" y="252" style="font-size:12px">one source copy</text>
  <text class="d-sub" x="470" y="240" style="font-size:12px">keep distinct objects;</text>
  <text class="d-sub" x="470" y="254" style="font-size:12px">retain mapped approaches;</text>
  <text class="d-sub" x="470" y="268" style="font-size:12px">keep source order.</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The packer classifies, normalizes, and deduplicates POIs before it builds the POI index.</figcaption>
</figure>

A fixed table maps tags to the service categories the device browses, and the first matching row
wins. The packer keeps the source identity of each place, removes repeated copies of the same
object, and builds one index per category.

A place gets a route approach only when one of its source nodes belongs to a routable way, or, for
an area, one of its own boundary nodes. A road that merely passes nearby does not establish access:
the device must not plan a route to a gate that does not exist.

Named summits go into their own category for Peak View, with elevation in place of opening hours.

### Parsing opening hours

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 250" role="img" aria-label="The opening_hours stage. A raw OSM opening_hours string is parsed at pack time into a normalized weekly schedule of seven days. A seasonal date rule is flattened to a representative in-season week and flagged seasonal; a public-holiday or unmodellable rule is dropped and flagged truncated. All resulting schedules are then deduplicated into a small pool, and each POI stores only its pool index.">
  <defs>
    <marker id="aOH" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">opening_hours → normalize → flag → dedup pool</text>

  <!-- raw string -->
  <rect class="d-panel-2" x="24" y="44" width="238" height="52" rx="9" />
  <text class="d-sub" x="40" y="62" style="font-size:12px">raw OSM tag</text>
  <text class="d-sub" x="40" y="80" font-family="var(--mono)" style="font-size:12px">Mo-Fr 08:00-18:00; Sa 09-13</text>
  <text class="d-sub" x="40" y="98" font-family="var(--mono)" style="font-size:12px">Apr-Oct: …; PH off</text>

  <!-- parse -->
  <line class="d-flow" x1="266" y1="70" x2="312" y2="70" marker-end="url(#aOH)" />
  <rect class="d-panel" x="318" y="44" width="196" height="52" rx="10" />
  <text class="d-tag" x="334" y="64">parse (subset grammar)</text>
  <text class="d-sub" x="334" y="84" style="font-size:12px">→ 7 days × ≤2 intervals</text>

  <!-- flags -->
  <line class="d-flow" x1="416" y1="96" x2="416" y2="120" marker-end="url(#aOH)" />
  <rect class="d-panel-2" x="300" y="126" width="232" height="58" rx="10" />
  <text class="d-sub" x="316" y="146" style="font-size:12px;fill:#a9501c">seasonal — Apr-Oct flattened to</text>
  <text class="d-sub" x="316" y="159" style="font-size:12px">a representative in-season week</text>
  <text class="d-sub" x="316" y="176" style="font-size:12px;fill:#a9501c">truncated — PH / 3rd interval dropped</text>

  <!-- dedup pool -->
  <line class="d-flow" x1="300" y1="155" x2="230" y2="200" marker-end="url(#aOH)" />
  <rect class="d-hot" x="24" y="196" width="300" height="44" rx="12" style="fill:#f8efe4" />
  <text class="d-tag" x="40" y="216" style="fill:#a9501c">dedup — a region's shops share hours</text>
  <text class="d-sub" x="40" y="232" style="font-size:12px">one schedule per pool entry</text>

  <!-- only-with-hours note -->
  <rect class="d-panel-2" x="344" y="196" width="352" height="44" rx="10" />
  <text class="d-sub" x="360" y="214" style="font-size:12px">a POI with no parseable hours stores <tspan font-family="var(--mono)">0xFFFF</tspan></text>
  <text class="d-sub" x="360" y="230" style="font-size:12px">— only POIs that actually have hours cost a pool slot</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The packer converts supported opening-hours rules to one fixed weekly schedule.</figcaption>
</figure>

The packer parses the part of the grammar that fits a weekly schedule: weekday ranges and lists, up
to two intervals a day, overnight intervals, all day, closed, and a representative seasonal week.
It rounds times to a quarter of an hour and flags a result it rounded or trimmed.

The device turns a schedule into Open, Closed, or Unknown. A missing or flagged schedule is
Unknown, and so is an untrusted clock: a GPS fix gives UTC, and UTC does not say when a bakery
opens. The local offset has to come from the rider or the phone.

### Building the navigation graph

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 300" role="img" aria-label="Building the navigation graph in three steps. First, a bike-legality filter keeps most highway ways but excludes motorway and its links, excludes trunk unless it is tagged bicycle equals yes, and hard-excludes anything tagged access equals no or private, bicycle equals no, or motorroad equals yes. Second, junction detection: a node touched by two or more routable ways, or sitting at a way's endpoint, becomes a junction; interior shape points do not. Third, each way is split at its junctions into edges, duplicate and reversed parallel ways are deduplicated by an unordered endpoint pair plus geometry key, and each edge's great-circle length becomes its cost. The result is junction nodes joined by edges.">
  <defs>
    <marker id="aNG" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Filter routable ways → find junctions → split + dedup into edges</text>

  <!-- 1 class/access filter -->
  <rect class="d-panel" x="24" y="52" width="216" height="120" rx="10" />
  <text class="d-tag" x="40" y="72">① routable? highway + access</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="40" y="92"  style="font-size:12px;fill:#2c5230">residential · track · path ✓</text>
    <text class="d-sub" x="40" y="107" style="font-size:12px;fill:#2c5230">footway · steps · service ✓</text>
    <text class="d-sub" x="40" y="125" style="font-size:12px;fill:#a9501c">motorway ✗ · trunk: conditional</text>
    <text class="d-sub" x="40" y="140" style="font-size:12px;fill:#a9501c">access=no|private · bicycle=no ✗</text>
  </g>
  <text class="d-sub" x="40" y="162" style="font-size:12px">independent of render styling</text>

  <!-- 2 junction detection -->
  <line class="d-flow" x1="246" y1="112" x2="276" y2="112" marker-end="url(#aNG)" />
  <text class="d-sub" x="270" y="52" style="font-size:12px;fill:#4d5b3c">② a shared node = a junction</text>
  <!-- two ways crossing at a node -->
  <line x1="288" y1="140" x2="392" y2="96" stroke="#9aa884" stroke-width="2.4" />
  <line x1="300" y1="86"  x2="380" y2="150" stroke="#9aa884" stroke-width="2.4" />
  <!-- interior shape points (open) -->
  <circle cx="322" cy="128" r="2.6" fill="#ece8cf" stroke="#9aa884" stroke-width="1"/>
  <circle cx="360" cy="112" r="2.6" fill="#ece8cf" stroke="#9aa884" stroke-width="1"/>
  <!-- the shared junction -->
  <circle cx="340" cy="119" r="5" class="d-hot-fill" />
  <text class="d-sub" x="340" y="172" text-anchor="middle" style="font-size:12px;fill:#a9501c">touched ≥2× → junction</text>
  <text class="d-sub" x="270" y="192" style="font-size:12px">endpoints are always junctions;</text>
  <text class="d-sub" x="270" y="210" style="font-size:12px">interior shape points never are</text>

  <!-- 3 split + dedup -->
  <line class="d-flow" x1="404" y1="112" x2="440" y2="112" marker-end="url(#aNG)" />
  <rect class="d-hot" x="448" y="52" width="248" height="120" rx="10" style="fill:#f8efe4" />
  <text class="d-tag" x="464" y="72" style="fill:#a9501c">③ split into edges + dedup</text>
  <text class="d-sub" x="464" y="94"  style="font-size:12px">cut each way at every junction</text>
  <text class="d-sub" x="464" y="110" style="font-size:12px">→ edge interiors are junction-free</text>
  <text class="d-sub" x="464" y="130" style="font-size:12px">dedup key: unordered (a,b) + geometry</text>
  <text class="d-sub" x="464" y="146" style="font-size:12px;fill:#a9501c">a way + its reverse = one edge</text>
  <text class="d-sub" x="464" y="162" style="font-size:12px">cost = great-circle length, metres</text>

  <!-- result graph -->
  <line class="d-flow" x1="360" y1="216" x2="360" y2="242" marker-end="url(#aNG)" />
  <rect class="d-panel-2" x="180" y="246" width="360" height="46" rx="10" />
  <text class="d-sub" x="360" y="266" text-anchor="middle" style="font-size:12px">Junction nodes have dense IDs; edges are undirected.</text>
  <text class="d-sub" x="360" y="282" text-anchor="middle" style="font-size:12px">→ serialized as the map's §8 navigation graph</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The packer converts routable OSM ways to a navigation graph. Shared OSM nodes form graph junctions.</figcaption>
</figure>

Shared OSM node identities become junctions. The packer splits ways there, removes duplicate edges,
and drops small disconnected components. A legality filter rejects private access, motor roads, and
bicycle prohibitions, and each accepted edge is classified by highway and surface into one way-kind
byte, which is what the profile weights read. See
[OBCM section 8](src:specs/OBCM_Spec.md).

An assembled map keeps one graph: the assembler copies each surviving edge geometry record
unchanged and rebuilds the identities, the adjacency, and the indexes.

### Weighting the graph: bike profiles

A map carries a few bike profiles, and the defaults are Road, Gravel, MTB, and Touring. A profile
gives a multiplier for each highway class and surface, and a climb weight.

Every multiplier is at least one, and zero means forbidden. That is not a style choice: A* needs a
heuristic that never overestimates, and a multiplier below one would make the straight-line
distance an overestimate and the answer wrong.

### Weighting the climb

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 378" role="img" aria-label="Two routes connect the same points. Route A crosses the hill: 8 km and 500 metres ascent. Route B goes around: 10 km and 200 metres ascent. A climb weight of 10 changes the lower-cost choice from A to B, assuming equal road multipliers.">
  <text class="d-tag" x="20" y="22">Climb cost · two routes around one hill · illustrative example</text>

  <!-- the hill, as nested contours -->
  <ellipse cx="360" cy="180" rx="132" ry="68" fill="#eae4cb" fill-opacity="0.6" stroke="#9aa884" stroke-width="1.2" />
  <ellipse cx="360" cy="178" rx="90" ry="46" fill="none" stroke="#9aa884" stroke-width="1.1" />
  <ellipse cx="360" cy="176" rx="48" ry="25" fill="none" stroke="#9aa884" stroke-width="1.1" />
  <text class="d-sub" x="360" y="192" text-anchor="middle" style="font-size:12px;fill:#a9501c">crest</text>


  <!-- over the top -->
  <path d="M78 214 C 170 200, 262 176, 360 170 C 458 164, 566 186, 646 192"
        fill="none" stroke="#cf6a2a" stroke-width="2.8" />
  <!-- round the valley -->
  <path d="M78 214 C 190 268, 320 280, 440 268 C 540 258, 606 218, 646 192"
        fill="none" stroke="#3c6b39" stroke-width="2.6" stroke-dasharray="7 5" />
  <circle cx="78" cy="214" r="4.5" class="d-hot-fill" />
  <circle cx="646" cy="192" r="4.5" class="d-hot-fill" />
  <text class="d-sub" x="78" y="234" text-anchor="middle" style="font-size:12px">start</text>
  <text class="d-sub" x="636" y="214" style="font-size:12px">goal</text>

  <!-- labels -->
  <rect class="d-panel-2" x="24" y="34" width="316" height="38" rx="8" />
  <text class="d-sub" x="38" y="50" style="font-size:12px;fill:#a9501c">A · over the hill</text>
  <text class="d-sub" x="38" y="64" text-anchor="start">8 km · 500 m ascent</text>

  <rect class="d-panel-2" x="380" y="34" width="316" height="38" rx="8" />
  <text class="d-sub" x="394" y="50" style="font-size:12px;fill:#2c5230">B · around the hill</text>
  <text class="d-sub" x="394" y="64" style="font-size:12px">10 km · 200 m ascent</text>

  <text class="d-sub" x="24" y="306" text-anchor="start">Weight 0: A = 8,000 m; B = 10,000 m. A wins.</text>
  <text class="d-sub" x="24" y="330" text-anchor="start">Weight 10: A = 8,000 + 500×10 = 13,000; B = 10,000 + 200×10 = 12,000. B wins.</text>
<text class="d-sub" x="24" y="357" text-anchor="start">Assumes a road-distance multiplier of 1. Route curves and contours are schematic.</text></svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Directional ascent measures all uphill travel along an edge. A longer path can cost less when it avoids enough climb.</figcaption>
</figure>

Each adjacency stores the ascent for its own direction, integrated along the edge with a small dead
band so that sampling noise is not counted as climbing. A missing sample pauses the integration
instead of reading as flat ground.

```text
edge_cost = weighted_distance + ascent_m × climb_weight
```

A descent never reduces the cost. A profile with a higher climb weight therefore accepts a longer
way round to avoid a hill, which is the whole point of having profiles.

### Building the LOD pyramid

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 270" role="img" aria-label="A pool of features each tagged with a min-LOD flows into three tiers. The country tier takes only features with min-LOD 0 and simplifies them at 120 metres. The region tier adds min-LOD 1 features at 18 metres. The street tier adds everything at full detail. Each tier becomes its own quadtree.">
  <defs>
    <marker id="aP5" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">LOD construction · illustrative tiers</text>

  <!-- feature pool -->
  <rect class="d-panel-2" x="30" y="70" width="140" height="130" rx="10" />
  <text class="d-tag" x="44" y="90">features</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="44" y="112" style="font-size:12px">coast · sea  min 0</text>
    <text class="d-sub" x="44" y="132" style="font-size:12px">motorway   min 0</text>
    <text class="d-sub" x="44" y="152" style="font-size:12px">forest     min 1</text>
    <text class="d-sub" x="44" y="172" style="font-size:12px">footway    min 2</text>
    <text class="d-sub" x="44" y="192" style="font-size:12px">building   min 2</text>
  </g>

  <!-- tiers -->
  <g>
    <line class="d-flow" x1="174" y1="100" x2="214" y2="92" marker-end="url(#aP5)" />
    <rect class="d-panel" x="220" y="62" width="220" height="42" rx="8" />
    <text class="d-label" x="234" y="80">LOD 0 · country</text>
    <text class="d-sub" x="234" y="96" style="font-size:12px">min_lod ≤ 0 · simplify 120 m</text>

    <line class="d-flow" x1="174" y1="135" x2="214" y2="135" marker-end="url(#aP5)" />
    <rect class="d-panel" x="220" y="114" width="220" height="42" rx="8" />
    <text class="d-label" x="234" y="132">LOD 1 · region</text>
    <text class="d-sub" x="234" y="148" style="font-size:12px">+ min_lod ≤ 1 · simplify 18 m</text>

    <line class="d-flow" x1="174" y1="170" x2="214" y2="178" marker-end="url(#aP5)" />
    <rect class="d-panel" x="220" y="166" width="220" height="42" rx="8" />
    <text class="d-label" x="234" y="184">LOD 2 · street</text>
    <text class="d-sub" x="234" y="200" style="font-size:12px">+ everything · full detail</text>
  </g>

  <!-- each → quadtree -->
  <line class="d-flow" x1="444" y1="83"  x2="492" y2="83"  marker-end="url(#aP5)" />
  <line class="d-flow" x1="444" y1="135" x2="492" y2="135" marker-end="url(#aP5)" />
  <line class="d-flow" x1="444" y1="187" x2="492" y2="187" marker-end="url(#aP5)" />
  <g>
    <rect x="500" y="64" width="56" height="38" rx="5" class="d-muted" /><text class="d-sub" x="528" y="87" text-anchor="middle" style="font-size:12px">quadtree</text>
    <rect x="500" y="116" width="56" height="38" rx="5" class="d-muted" /><text class="d-sub" x="528" y="139" text-anchor="middle" style="font-size:12px">quadtree</text>
    <rect x="500" y="168" width="56" height="38" rx="5" class="d-muted" /><text class="d-sub" x="528" y="191" text-anchor="middle" style="font-size:12px">quadtree</text>
  </g>
  <text class="d-sub" x="600" y="120" style="font-size:12px">→ the LOD</text>
  <text class="d-sub" x="600" y="136" style="font-size:12px">pyramid in</text>
  <text class="d-sub" x="600" y="152" style="font-size:12px">the .obcm</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Each LOD applies its configured filters. The packer then builds and writes its quadtree.</figcaption>
</figure>

Each style names the coarsest level it may appear at, and each level applies its own filters:
sub-pixel polygons are removed, fills and lines that look the same are merged, short joined lines
are dropped, and coarse land cover is generalized.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
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
  <text class="d-sub" x="232" y="70" text-anchor="middle" style="font-size:12px">dense → deeper</text>
  <!-- a polygon in SW -->
  <path d="M70 210 L130 205 L140 260 L80 270 Z" fill="#cfe0c2" stroke="#3c6b39" stroke-width="1.2" />
  <!-- a line straddling the vertical boundary -->
  <line x1="120" y1="120" x2="220" y2="150" stroke="#cf6a2a" stroke-width="2.5" />
  <circle cx="168" cy="134" r="3.5" class="d-hot-fill" />
  <text class="d-sub" x="120" y="106" style="fill:#a9501c;font-size:12px">clipped at the boundary →</text>

  <!-- right notes -->
  <text class="d-label" x="330" y="84">leaf fills:</text>
  <text class="d-sub" x="330" y="104" font-family="var(--mono)" style="font-size:12px">feature bytes + ring counts</text>
  <text class="d-sub" x="330" y="124" style="font-size:12px">&gt; chunk_size → split 4</text>
  <line class="d-stroke" x1="330" y1="140" x2="690" y2="140" style="stroke:#9aa884" />
  <text class="d-sub" x="330" y="164" style="font-size:12px">a straddling feature is clipped to</text>
  <text class="d-sub" x="330" y="180" style="font-size:12px">each child's box, so every leaf's</text>
  <text class="d-sub" x="330" y="196" style="font-size:12px">geometry is self-contained — which</text>
  <text class="d-sub" x="330" y="212" style="font-size:12px">is what lets the device decode one</text>
  <text class="d-sub" x="330" y="228" style="font-size:12px">chunk without touching its neighbours.</text>
  <text class="d-sub" x="330" y="258" style="font-size:12px;fill:#a9501c">this is the same tree the device walks to cull</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>A quadtree node becomes a leaf when its features fit the chunk and ring limits.</figcaption>
</figure>

A node becomes a leaf when its features fit the chunk size and the ring limit; otherwise it splits
and its features are clipped into the children, so a feature that crosses a boundary appears in
both. The device walks the same flat quadtree, so the packer's decision is the renderer's index.

### The builder

The product builder assembles published cells; it does not run the packer. A selection can mix
named regions, boxes, lassos, and GPX corridors, and the builder unions them before it prices or
downloads anything, so overlapping parts do not pay twice. It verifies every object against the
catalog, and the assembler verifies the finished map with the production readers.

One Svelte application serves the website, the desktop app, and the maintainer server; the host
modules differ in transport and storage, never in the selection or assembly algorithm. The website
is static and the cells live in object storage, so publishing a new region does not deploy the
website.

## Following a route

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 760px">
<svg viewBox="0 0 760 322" role="img" aria-label="Converting GPX track points to an OBCR route in one streaming pass after waypoint collection. Left panel, the shape: the stored line keeps only the corners (and one vertex at least every 1.2 km) — vertices within 1 metre of the line between their neighbours are dropped — Distance and climb are measured from the retained points; validity gaps stop the elevation integrator. Right panel, the climb: a raw elevation trace is integrated through a 3-metre dead-band; small wiggles inside the band book no ascent, and only once the trace leaves the band is the climb booked and the reference re-anchored. The same dead-band is shared by the elevation profile and the live barometric climb on the device.">
  <text class="d-tag" x="20" y="22">GPX → OBCR · bounded streaming conversion</text>
  <text class="d-sub" x="20" y="38" style="font-size:12px">retain shape and elevation changes, then measure distance + climb</text>
  <line x1="384" y1="58" x2="384" y2="300" stroke="#9aa884" stroke-opacity="0.45" stroke-width="1" />
  <text class="d-sub" x="26" y="62" style="font-size:12px;fill:#4d5b3c">① the shape — decimate, keep the corners</text>
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
  <text class="d-sub" x="86" y="220" style="font-size:12px">dropped — within 1 m of the line</text>
  <text x="130" y="121" text-anchor="middle" style="font-family:var(--mono);font-size:12px;fill:#a9501c">kept — a corner</text>
  <text class="d-sub" x="220" y="107" text-anchor="middle" style="font-size:12px;fill:#4d5b3c">keep a point at least every 1.2 km</text>
  <rect x="34" y="246" width="320" height="42" rx="7" style="fill:#eef2df;stroke:#9aa884;stroke-width:0.8" />
  <text x="194" y="263" text-anchor="middle" style="font-family:var(--mono);font-size:12px;fill:#3c6b39">totals use every original point</text>
  <text x="194" y="277" text-anchor="middle" style="font-family:var(--mono);font-size:12px;fill:#3c6b39">stored geometry is reduced</text>
  <text class="d-sub" x="402" y="62" style="font-size:12px;fill:#4d5b3c">② the climb — a ±3 m dead-band</text>
  <polyline points="430.0,232.0 444.5,237.0 459.0,227.0 473.5,229.5 488.0,222.0 502.5,212.0 517.0,202.0 531.5,192.0 546.0,187.0 560.5,189.5 575.0,197.0 589.5,187.0 604.0,172.0 618.5,157.0 633.0,147.0 647.5,152.0 662.0,137.0 676.5,122.0 691.0,112.0 705.5,117.0 720.0,107" fill="none" stroke="#9aa884" stroke-width="1.3" stroke-opacity="0.65" />
  <rect x="531.5" y="177" width="72.5" height="30" fill="#cf6a2a" fill-opacity="0.08" stroke="#cf6a2a" stroke-opacity="0.35" stroke-width="0.8" stroke-dasharray="3 3" />
  <text x="608" y="195" style="font-family:var(--mono);font-size:12px;fill:#a9501c">±3 m</text>
  <polyline points="430.0,232.0 502.5,232.0 502.5,212.0 531.5,212.0 531.5,192.0 604.0,192.0 604.0,172.0 618.5,172.0 618.5,157.0 662.0,157.0 662.0,137.0 676.5,137.0 676.5,122.0 720.0,122.0 720.0,107.0 720.0,107" fill="none" stroke="#cf6a2a" stroke-width="2" />
  <text class="d-sub" x="430" y="252" style="font-size:12px;fill:#4d5b3c">along the route →</text>
  <text class="d-sub" x="402" y="100" style="font-size:12px;fill:#4d5b3c">elev</text>
  <text class="d-sub" x="455" y="210" style="font-size:12px;fill:#4d5b3c">wiggle &lt; 3 m → ignored</text>
  <text x="592" y="120" text-anchor="middle" style="font-family:var(--mono);font-size:12px;fill:#a9501c">past ±3 m → book + re-anchor</text>
  <rect x="410" y="266" width="316" height="22" rx="7" style="fill:#eef2df;stroke:#9aa884;stroke-width:0.8" />
  <text x="568" y="281" text-anchor="middle" style="font-family:var(--mono);font-size:12px;fill:#3c6b39">shared 3 m dead band</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The GPX converter keeps exact ride statistics and reduces only the displayed geometry.</figcaption>
</figure>

The GPX converter writes an OBCR route. It measures the geometry it keeps, so the statistics
describe the stored route and not the source file, and it preserves elevation changes, surface
transitions, and the boundaries of missing data. The same `no_std` converter runs on the device, in
the simulator, and in the browser.

### Map-matching: a forward-biased cursor

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 290" role="img" aria-label="A route polyline with a cursor on it. A bounded forward window of dozens of segments ahead (and a few behind) is highlighted around the cursor. A GPS fix off to the side is projected onto the nearest segment in that window, giving a progress distance along the route and a cross-track distance to it. A far fix is flagged off-route and freezes progress.">
  <text class="d-tag" x="20" y="24">Snap each fix to the nearest segment in a forward window</text>

  <!-- route -->
  <path d="M40 210 C 120 120, 200 120, 270 170 C 320 205, 380 150, 470 120" fill="none" stroke="#9aa884" stroke-width="3" />
  <!-- forward window (highlighted) -->
  <path d="M200 138 C 235 128, 255 150, 285 168" fill="none" stroke="#cf6a2a" stroke-width="5" stroke-opacity="0.3" />
  <text class="d-sub" x="262" y="114" text-anchor="middle" style="fill:#a9501c;font-size:12px">forward window</text>
  <!-- cursor -->
  <circle cx="210" cy="135" r="5" class="d-hot-fill" /><text class="d-sub" x="170" y="130" style="font-size:12px">cursor</text>
  <!-- a fix near the route -->
  <circle cx="262" cy="178" r="5" class="d-water" />
  <text class="d-sub" x="274" y="188" style="font-size:12px">fix</text>
  <!-- cross-track to nearest segment -->
  <line x1="262" y1="178" x2="259" y2="159" stroke="#33575b" stroke-width="1.5" stroke-dasharray="3 2" />
  <circle cx="259" cy="159" r="3" class="d-water" />
  <text class="d-sub" x="300" y="170" style="font-size:12px">cross-track dist</text>
  <!-- progress label -->
  <text class="d-sub" x="90" y="205" style="font-size:12px">← progress along the route</text>

  <!-- off-route fix -->
  <circle cx="430" cy="210" r="5" class="d-muted" stroke="#c0492e" stroke-width="1.5" />
  <text class="d-sub" x="430" y="230" text-anchor="middle" style="font-size:12px">far fix</text>
  <line x1="430" y1="210" x2="452" y2="135" stroke="#c0492e" stroke-width="1.2" stroke-dasharray="3 3" />
  <text class="d-sub" x="430" y="244" text-anchor="middle" style="fill:#c0492e;font-size:12px">off-route → freeze</text>

  <!-- hysteresis band note -->
  <rect class="d-panel-2" x="500" y="60" width="196" height="120" rx="10" />
  <text class="d-tag" x="516" y="80">off-route hysteresis</text>
  <text class="d-sub" x="516" y="102" style="font-size:12px">≥ 25 m → off-route</text>
  <text class="d-sub" x="516" y="122" style="font-size:12px">&lt; 15 m  → back on</text>
  <text class="d-sub" x="516" y="146" style="font-size:12px">the gap is the dead-band that</text>
  <text class="d-sub" x="516" y="160" style="font-size:12px">keeps the flag from flapping</text>
  <text class="d-sub" x="516" y="174" style="font-size:12px">on GPS jitter at the edge</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The matcher searches forward from its route cursor. It freezes progress when the rider is off-route.</figcaption>
</figure>

The matcher keeps a cursor on the route and searches a window around it, with more range forward
than backward. The bias is what stops a loop or a there-and-back route from matching the earlier
pass. The first fix searches the whole route, and a skip-ahead sets a floor that later matches
cannot fall back through.

The rider goes off route past one distance and back on route below a smaller one, and the gap
between the two stops GPS noise from switching the state. While off route the cross-track distance
stays live and the route progress stays where it was.

## Landmark and peak content

The host compiles landmark text and photos from captured Wikidata, Wikipedia, and Commons
responses into the map. Selection is deterministic: a fixed list of permitted types, matched
through subclass steps, with excluded types winning. There is no popularity score and no per-place
judgement, so the same snapshot always gives the same landmarks. Lakes, mountains, and glaciers are
excluded even where a permitted type also matches: lake names belong on the map and mountains
belong in Peak View. Mountain passes are kept.

Each usable article is kept in the four UI languages, and the device picks its own language, then
English, then the language the baker stored. Text and photos carry separate credits, and an asset
whose credits do not fit is rejected; a rejected photo still leaves readable text. Photos are
converted to the panel palette with ordered dithering at a fixed size, with no crop and no
image-specific correction.

Peak articles are a separate collection, linked only by an explicit tag on the OSM summit node: a
name and a coordinate cannot prove an article is about that summit. One article can serve several
summits. See the [compiler](src:host/obc-pack/src/landmarks/mod.rs).

Landmarks are an artifact class of the bake, beside the map cells and the terrain. The bakery runs
them per curated region: the region's own boundary polygon selects the sources, a cache holds the
raw capture, and the tree holds one compiled artifact for each region. The capture reads live
sources, so it is the only step of a bake that two runs can disagree on. It is resumable, the run
says when it starts one, and everything after it is a pure function of the bytes it wrote. A
region is compiled again only when its captured sources, its boundary, or the category policy
change.

## Attribution and share-alike

OpenStreetMap data is under the Open Database License 1.0. A rendered map is a Produced Work, and
the device gives the required attribution on its About page. A published `.obcm` map is a
Derivative Database: the catalog declares `ODbL-1.0` and publishes the license text, and anyone who
distributes that map data is bound by the same terms. A map with terrain-derived contours also
carries the [Copernicus attribution](../terrain/#attribution).

## Implementation

- Packer pipeline: [`pipeline.rs`](src:host/obc-pack/src/pipeline.rs)
- Configuration: [`config.rs`](src:host/obc-pack/src/config.rs)
- OSM ingest: [`ingest.rs`](src:host/obc-pack/src/ingest.rs)
- POIs and opening hours: [`poi.rs`](src:host/obc-pack/src/poi.rs), [`hours.rs`](src:host/obc-pack/src/hours.rs)
- Landmark preparation: [`landmarks`](src:host/obc-pack/src/landmarks/mod.rs)
- Landmark bake stage: [`landmarks.rs`](src:host/obc-bake/src/landmarks.rs)
- Navigation graph: [`nav.rs`](src:host/obc-pack/src/nav.rs)
- Quadtree: [`quadtree.rs`](src:host/obc-pack/src/quadtree.rs)
- Builder: [`builder/`](src:builder)
- Web assembler: [`obc-web-assemble`](src:apps/obc-web-assemble)
- Device router: [`nav.rs`](src:firmware/obc-route/src/nav.rs)
- Route matcher: [`matcher.rs`](src:firmware/obc-route/src/matcher.rs)
- GPX converter: [`convert.rs`](src:firmware/obc-route/src/convert.rs)
