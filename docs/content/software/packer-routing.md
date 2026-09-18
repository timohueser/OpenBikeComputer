---
title: Packer and routing
description: How OpenBikeComputer builds maps, navigation graphs, routes, and route matches.
copy: ai
---

# Packer and routing

`obc-pack` converts OpenStreetMap PBF extracts to the OBCM map format.
It also builds POIs, opening-hours data, contours, and a navigation graph.
The device uses this graph for route planning.

The GPX converter creates OBCR route files.
The route matcher maps live positions to an active route.

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

The pipeline performs these operations:

1. Ingest and optionally crop or merge PBF sources.
2. Calculate the content bounding box.
3. Add land, sea, and optional contours.
4. Build POIs and the navigation graph.
5. Build each level of detail (LOD).
6. Write the map and release the completed LOD.

The packer keeps approximately one LOD quadtree in memory at a time.
It removes a partial output file after an error or cancellation.

### Styling: first match wins

The `features` object in `config.json` is ordered.
The packer checks tag keys in document order.
The first matching key and value supplies the style.
An exact value has priority over the `"*"` catch-all.
The packer drops a way when no style matches.

Style IDs start at 1 and follow document order.
A style contains color, paint order, width, priority, minimum LOD, and line properties.
The style table can contain at most 254 entries.

The shipped style uses fixed one-pixel strokes for tracks, paths, footways, steps, and
bridleways. Tracks are light brown; paths and bridleways are darker solid lines. Footways and
steps use dark dashes. Via ferratas and ladders use purple dashes. These marks use the OSM way
type. They do not indicate hiking or mountain bike difficulty. These minor ways and cycleways
appear at up to 5 m per pixel, together with service roads. Residential roads remain visible
through 10 m per pixel. Passenger cableways and ski lifts
use thin grey dashes, with no new line format. Gardens, golf courses, recreation grounds, and heath share the green vegetation fill.
Rock, scree, and shingle share a grey fill. Glaciers have a separate pale cyan
fill. Cliffs use thin dark edge lines, including closed cliff rims; the lines do not indicate
which side is lower.

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

### Ingest: two passes, then assemble

The ingester normally reads each PBF twice.
Pass 1 stores node coordinates and collects renderable area relations.
Pass 2 resolves ways and captures relation-member geometry.
The ingester then uses GEOS to assemble polygons and holes.

A closed way becomes a polygon only when its tags identify an area.
A closed road loop remains a line.

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

### Cropping to a box

Use `--bbox W,S,E,N` to select data during ingest.
This option adds an ID-selection pass before the two normal passes.

The selection keeps each selected way complete.
It also keeps all required nodes and renderable area-relation members.
Thus, roads do not stop at artificial box edges.
Area relations do not lose required geometry.

Complete objects can extend outside the requested box.
Therefore, the map header bounding box can be larger than the requested box.
Do not use the output bounding box as the next crop request.

### Merging several regions

The ingester can read multiple PBF files in the same passes.
It does not create an intermediate merged file.
For a duplicate object type and ID, the first listed file wins.
The ingester emits surviving objects in ascending ID order for each type.
These rules make the result deterministic.

### Land and sea

OpenStreetMap supplies coastlines, but not a complete land fill.
The packer downloads and caches a global land-polygon dataset.
It clips this dataset to the content bounding box.
When land is the backdrop, the packer stores only the sea complement.
Use `--no-land` to skip this stage.

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

### Contours, traced from the terrain

The packer can trace contours from [OBCT terrain](../terrain/).
Set `contours.enabled` and supply `--terrain`.
The packer uses marching squares on the terrain lattice.
It skips a lattice square if one corner has no height.

The packer creates `contour.major` and `contour.index` line features.
The defaults are a 100 m interval and an index at every fifth contour.
The shipped style shows the 500 m index contours at up to 35 m per pixel and the other contours
at up to 10 m per pixel. Both use muted dashed strokes, fixed at one pixel wide. Their colour
differs from rock fills, tracks, and paths.
The default pre-LOD simplify tolerance is 15 m.
The configuration can change these values.

A contour class needs a matching style rule.
Contours then use the normal LOD and quadtree pipeline.
The terrain-layer flag lets the renderer hide their ink.
It does not remove their bytes from the map.

### Extracting POIs

The packer uses a fixed table of service-POI tag mappings.
The table covers water, campsites, accommodation, resupply, pharmacies, bicycle shops, and train stations.
The first matching table row supplies the subtype.

POI subtype IDs, categories, and fallback labels are normative.
The packer stores names as at most 24 printable ASCII bytes.
It transliterates supported Latin characters.
An unnamed POI uses its subtype fallback label.

The packer keeps the OSM type and ID for each place.
It removes repeated copies of the same source object and keeps nearby objects separate.
It then builds one spatial index for each category.

A place has a route approach only when a source node belongs to a routable way.
An area can use one of its own boundary nodes when that node also belongs to a routable way.
The stored approach has that node's identity, coordinate, and supported profile bits.
A nearby road or entrance does not establish access.
Wiki-tagged objects retain their source links and normalized hours for the landmark compiler.
Relations can retain these facts without a coordinate or approach.

Named summit nodes use a separate category for Peak View. Their names retain UTF-8 characters,
and their records carry elevation instead of opening hours. Distinct summit source identities remain separate.

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

### Parsing opening hours

The packer parses a subset of the OSM `opening_hours` grammar.
It stores a 29-byte weekly schedule in a shared pool.
The device does not parse the source text.

The subset supports these forms:

- Weekday ranges and lists.
- Up to two intervals per day.
- `24/7`, `off`, and `closed`.
- Time-only rules for all days.
- Overnight intervals.
- Representative seasonal weeks.

The parser rounds times to the nearest 15 minutes with half-to-even rounding.
It flags a partial result when it rounds a time or drops an unsupported rule.
A fully unsupported value produces no schedule.

The device uses one Open, Closed, or Unknown result for all place consumers.
A missing schedule, a partial or seasonal schedule, or untrusted local time produces Unknown.
GPS UTC alone does not establish local time: a rider or phone must also set the UTC offset.
An overnight interval continues into the next day, including the Sunday-to-Monday boundary.
Known-closed places are removed before the query fills its bounded page.
Queries continue beyond sixteen places with source-based keys.
Clock ticks update status without changing the selected identity or geometric order.
Read failures and unavailable coverage remain distinct from an empty result.

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

### Building the navigation graph

The packer always builds the navigation graph.
It keeps OSM node IDs for routable ways.
Shared node IDs form junctions.
The packer splits ways at junctions and removes duplicate edges.
It removes small disconnected components with the configured threshold.

The legality filter rejects private access, motor roads, and bicycle prohibitions.
It classifies each accepted edge by highway and surface.
The packed `way_kind` byte contains both classes.
See [OBCM section 8](src:specs/OBCM_Spec.md) for the normative tables and layout.

The serializer writes tiled nodes, adjacency records, edge geometry, and snap anchors.
It densifies and splits long geometry so each record fits its chunk.

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

### Weighting the graph: bike profiles

Each map contains one to eight bike profiles.
The defaults are Road, Gravel, MTB, and Touring.
Each profile supplies highway, surface, and climb weights.

Highway and surface multipliers use `1/16` fixed-point values.
A value of `16` means `1.0`.
Zero forbids the class.
Every nonzero multiplier must be at least `1.0`.
This limit keeps the A* distance heuristic admissible.

The router starts weighted A* with epsilon 1.3.
If the fixed search table fills, it retries with 2.0 and then 3.0.
The successful epsilon bounds the returned profile-weighted cost.

### Weighting the climb

Each adjacency stores ascent for its travel direction.
The packer calculates ascent along the edge polyline.
It samples terrain at intervals of at most 50 m.
The shared integrator uses a 3 m dead band.
A missing sample pauses the dead band without using zero elevation.
Valid samples after the gap start a new ascent segment.
The gap contributes no ascent.
An edge with no valid sample has zero ascent.

```text
edge_cost = weighted_distance + ascent_m × climb_weight
```

The climb weight is a profile value from 0 through 255.
Zero disables climb cost.
The defaults are Road 10, Gravel 8, MTB 6, and Touring 8.
A descent never reduces edge cost.

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

### Building the LOD pyramid

Each style defines the coarsest permitted LOD.
Each LOD defines its simplify and size thresholds.
The packer builds each LOD independently.

- `min_area_px` removes small polygons and holes from coarse LODs.
- `merge_fills` combines fills with the same render identity.
- `merge_lines` joins connected lines with the same render identity.
- `min_line_km` removes short joined lines.
- Coverage simplification creates coarse semantic coverage.

Rock and ice participate in the same coarse land-cover generalization as vegetation and towns.
Rock, scree, and shingle share one output class. Ice remains separate from rock and water.
The output uses ordinary polygons and the existing device renderer.

`min_line_km` requires `merge_lines`.
The finest LOD does not use `min_area_px`.
The packer joins lines before it applies the line-length filter.

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

### The quadtree: packing geometry into chunks

Each LOD uses a quadtree over the global bounding box.
A node becomes a leaf when both conditions are true:

- Its estimated feature bytes fit the configured chunk size.
- Each feature has at most 32 rings.

Otherwise, the node splits into four children.
The packer clips features to each child box.
A feature that crosses a boundary can occur in multiple children.
The renderer walks the same flat quadtree to select visible chunks.

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

### The builder

The product builder assembles published cells.
It does not run the packer.
A selection can contain named regions, boxes, lassos, and GPX corridors.
The builder unions all parts before it prices or downloads cells.
Thus, overlapping parts do not duplicate cells.

The builder verifies each object length and SHA-256 digest.
The assembler verifies the completed map before it exposes the output.
If the catalog supplies terrain, the builder downloads the required terrain cells.
The assembler puts them in the final map terrain region.
The builder does not provide a terrain switch.

### Editing a skin

The product skin editor changes presentation fields only.
It does not change feature types, style IDs, LODs, or routing profiles.
The editor uses the production reader and renderer for its preview.
The builder rejects a saved skin that does not match the current schema.

### One source, three hosts

One Svelte application supplies the website, desktop app, and maintainer server.
Host modules supply transport and storage capabilities.
They do not supply separate selection or assembly algorithms.

| Capability | Website | Desktop | Maintainer server |
| :-- | :--: | :--: | :--: |
| Coverage selection | Yes | Yes | Yes |
| WebAssembly assembly | Yes | Yes | Yes |
| Product skin editor | Yes | Yes | Yes |
| Managed ride library | No | Yes | No |
| Advanced schema editor | No | No | Yes |
| Product PBF build | No | No | No |

The maintainer server can pack a fixed reference crop for schema previews.
This preview is not a product build path.
Published cells still require an explicit maintainer bake.

### Device and ride surfaces

The builder outputs one `.obcm` file.
A device transfer commits the complete map or no map.
The device verifies a whole-object CRC-32 before it commits the map.
The desktop app also manages routes and recorded rides.
Protocol v4 has no ride-possession acknowledgment. A local import does not mark a device ride
as synced or eligible for automatic expiry. See [ride reconciliation](../companion-link/#reconciliation).

### Where the hosted tier lives

The website is a static application.
The catalog and cells use separate object storage.
A catalog update does not require a website deployment.
The publisher uploads content before it publishes the new catalog root.

## Following a route

The GPX converter creates an OBCR route.
It measures the retained geometry after decimation.
It preserves elevation changes, surface transitions, and missing-data boundaries.
It chunks the route and shares the seam point between adjacent chunks.
The same `no_std` converter runs on the device, simulator, and web host.

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

### Map-matching: a forward-biased cursor

The matcher keeps a cursor on the active route.
For each position fix, it searches a bounded segment window around that cursor.
The window has more forward range than backward range.
This bias prevents a loop from matching an earlier pass.

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

The first fix searches the complete route. An explicit skip-ahead action sets a progress floor;
later matches cannot return to the skipped section.
Normal tracking searches 3 segments backward and 64 segments forward.
Off-route tracking searches 3 segments backward and 320 segments forward.
A caller can request the wide window after an unmatched interval.

The matcher marks the rider off-route at 25 m or more.
It clears this state below 15 m.
This hysteresis prevents state changes from GPS noise.
While off-route, cross-track distance stays live and route progress stays fixed.

## Preparing landmark content

The host can prepare landmark text and photos from captured Wikidata, Wikipedia and Commons
responses. This step uses local source files with recorded digests. It selects sites inside the map
polygon, including points on its boundary. Country claims do not define coverage.

The category policy follows Wikidata types and their parent classes. Excluded types take priority.
Claims use the preferred rank when present, otherwise the normal rank. The compiler retains every
usable article in the supported UI languages: English, German, French and Spanish. Each version
has one or two complete lead sentences within four readable pages. The device selects its UI
language, then English, then the baked local fallback. All versions share one optional photo.

Text and photos have separate source and attribution records. The compiler rejects an asset when
its required credits cannot fit or use unsupported characters. A rejected photo leaves valid text
available. Photos use a fixed size and palette; there is no subject crop or image ranking.

The output reports captured sites, usable content and omissions separately. Source coverage remains
explicit. An approach count is unknown until the OSM source join runs. This host preparation step
does not by itself install content or establish that a landmark has a routeable approach.


## Preparing peak articles

Peak articles form a separate host catalogue. Discovery uses the same named OSM summit nodes as
Peak View. Only explicit Wikidata or Wikipedia tags can establish an article link. Captured source
redirects and Wikipedia language links can resolve those tags; names and coordinates never infer
a match. The OSM node determines region membership. A linked article can describe several summits
and does not need a coordinate inside that region.

The compiler stores each canonical article once and retains every summit node association. It
shares text extraction, supported languages, local fallback, photos and credits with landmark
preparation. Candidate, article, photo and omission counts remain separate. A rejected photo leaves
a text-only peak article. A summit without usable text remains an ordinary summit.

The peak catalogue cannot enter Ride Assistant landmark packaging. Map references and Peak View
access still need their own integration. Regional capture and artifact orchestration also remain
required for normal map delivery. The current host entry points and bounded captured evidence are
in the [compiler README](src:host/obc-pack/src/landmarks/README.md).

## Attribution and share-alike

OpenStreetMap data uses the Open Database License 1.0.
A rendered map is a Produced Work.
The device provides the required attribution on its About page.

A published `.obcm` map is a Derivative Database.
The catalog declares `ODbL-1.0` and publishes the license text.
A distributor of this map data must follow the same license terms.
Maps with terrain-derived contours also require the [Copernicus attribution](../terrain/#attribution).

## Implementation

- Packer pipeline: [`pipeline.rs`](src:host/obc-pack/src/pipeline.rs)
- Configuration: [`config.rs`](src:host/obc-pack/src/config.rs)
- OSM ingest: [`ingest.rs`](src:host/obc-pack/src/ingest.rs)
- POIs and opening hours: [`poi.rs`](src:host/obc-pack/src/poi.rs), [`hours.rs`](src:host/obc-pack/src/hours.rs)
- Landmark preparation: [`landmarks`](src:host/obc-pack/src/landmarks/mod.rs)
- Navigation graph: [`nav.rs`](src:host/obc-pack/src/nav.rs)
- Quadtree: [`quadtree.rs`](src:host/obc-pack/src/quadtree.rs)
- Builder: [`builder/`](src:builder)
- Web assembler: [`obc-web-assemble`](src:apps/obc-web-assemble)
- Device router: [`nav.rs`](src:firmware/obc-route/src/nav.rs)
- Route matcher: [`matcher.rs`](src:firmware/obc-route/src/matcher.rs)
- GPX converter: [`convert.rs`](src:firmware/obc-route/src/convert.rs)
