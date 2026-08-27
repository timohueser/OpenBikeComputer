---
title: Data formats
description: The binary map, route, ride, terrain, weather, catalog, and map-assembly formats.
copy: ai
---

# Data formats

OpenBikeComputer uses binary objects for device data.
The device reads these objects through a random-access byte interface.
It does not parse host data formats during normal operation.

The files in [`specs/`](src:specs) are the normative contracts.
[`obc-formats`](src:firmware/obc-formats) defines shared constants and byte primitives.
Reader and writer crates own parsing, caching, and conversion policy.

## Format summary

| Format | Current version | Use | Main consumer |
| --- | ---: | --- | --- |
| OBCM | 14 | Map, POIs, navigation graph, and optional terrain | Device |
| OBCR | 3 | Route geometry, statistics, and waypoints | Device |
| Ride object | 3 | Recorded samples and summary | Device and companion |
| OBCT | 1 | Terrain height raster | Device and map tools |
| OBCW | 1 | Hourly weather and rain frames | Device |
| OBCG | 1 | Published precipitation grid frame | Companion and host tools |
| OBCC | Schema 2 | Map-builder catalog | Website and desktop app |
| OBCA | 1 | Cell and assembly rules | Map tools |

All multi-byte values use little-endian order unless a specification says otherwise.
Coordinates use signed integer microdegrees.
Each format stores offsets and counts.
Readers use checked arithmetic and reject unsupported versions.

<figure class="fig">
<svg viewBox="0 0 720 290" role="img" aria-label="OSM data becomes an OBCM map. GPX data becomes an OBCR route. The device, simulator, and browser use the shared readers and converters.">
  <defs>
    <marker id="aF1" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Two binaries, one philosophy</text>

  <!-- MAP lane -->
  <rect class="d-panel-2" x="32" y="64" width="132" height="50" rx="10" />
  <text class="d-label" x="98" y="86" text-anchor="middle">OSM extract</text>
  <text class="d-sub" x="98" y="102" text-anchor="middle">a slice of the planet</text>
  <line class="d-flow" x1="170" y1="89" x2="298" y2="89" marker-end="url(#aF1)" />
  <text class="d-sub" x="234" y="80" text-anchor="middle" style="fill:#a9501c">obc-pack · offline</text>
  <rect class="d-panel" x="304" y="64" width="120" height="50" rx="10" />
  <text class="d-label" x="364" y="86" text-anchor="middle">.obcm</text>
  <text class="d-sub" x="364" y="102" text-anchor="middle">map</text>

  <!-- ROUTE lane -->
  <rect class="d-panel-2" x="32" y="166" width="132" height="50" rx="10" />
  <text class="d-label" x="98" y="188" text-anchor="middle">GPX upload</text>
  <text class="d-sub" x="98" y="204" text-anchor="middle">a ride you planned</text>
  <line class="d-flow" x1="170" y1="191" x2="298" y2="191" marker-end="url(#aF1)" />
  <text class="d-sub" x="234" y="182" text-anchor="middle" style="fill:#a9501c">obc-route</text>
  <text class="d-sub" x="234" y="207" text-anchor="middle" style="fill:#a9501c;font-size:9px">device · sim · browser</text>
  <rect class="d-panel" x="304" y="166" width="120" height="50" rx="10" />
  <text class="d-label" x="364" y="188" text-anchor="middle">.obcr</text>
  <text class="d-sub" x="364" y="204" text-anchor="middle">route</text>

  <!-- converge to readers -->
  <line class="d-flow" x1="424" y1="89"  x2="536" y2="134" marker-end="url(#aF1)" />
  <line class="d-flow" x1="424" y1="191" x2="536" y2="152" marker-end="url(#aF1)" />
  <rect class="d-hot" x="540" y="110" width="160" height="66" rx="13" style="fill:#f8efe4" />
  <text class="d-title" x="620" y="134" text-anchor="middle" style="fill:#a9501c">the readers</text>
  <text class="d-sub" x="620" y="151" text-anchor="middle">obc-reader · obc-route</text>
  <text class="d-sub" x="620" y="167" text-anchor="middle">no_std — sim &amp; device</text>

  <!-- shared DNA strip -->
  <rect class="d-panel-2" x="32" y="244" width="668" height="34" rx="9" />
  <text class="d-sub" x="366" y="265" text-anchor="middle">shared DNA — little-endian · µdeg integers · anchor + delta geometry · explicit offsets · streamed</text>
</svg>
<figcaption>OBCM and OBCR use the same byte and streaming conventions.</figcaption>
</figure>

## OBCM — the map

OBCM v14 is the only supported map version.
One OBCM object contains all map data.
Its global offsets are 32-bit values in scaled units.
Current writers use 16-byte units.
This gives the file a 64 GiB address space.

### The file, front to back

<figure class="fig">
<svg viewBox="0 0 720 210" role="img" aria-label="An OBCM v14 file contains a 49-byte header, styles, an LOD table, independent LOD regions, POIs, opening hours, a navigation graph, and optional embedded OBCT terrain.">
  <defs>
    <marker id="aF2" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">OBCM — the whole file, front to back</text>

  <!-- ribbon -->
  <g stroke="#3c6b39" stroke-width="1.4">
    <rect x="24"  y="56" width="60"  height="44" class="d-forest" />
    <rect x="84"  y="56" width="82"  height="44" class="d-amber" />
    <rect x="166" y="56" width="80"  height="44" class="d-water" />
    <rect x="246" y="56" width="112" height="44" class="d-muted" />
    <rect x="358" y="56" width="100" height="44" class="d-muted" />
    <rect x="458" y="56" width="130" height="44" class="d-muted" />
    <rect x="588" y="56" width="54"  height="44" class="d-hot-fill" />
    <rect x="642" y="56" width="54"  height="44" class="d-water" />
  </g>
  <text class="d-label" x="54"  y="80" text-anchor="middle" style="fill:#fff">Header</text>
  <text class="d-sub"   x="54"  y="94" text-anchor="middle" style="fill:#e7ead8">49 B</text>
  <text class="d-label" x="125" y="80" text-anchor="middle">Style table</text>
  <text class="d-sub"   x="125" y="94" text-anchor="middle">global</text>
  <text class="d-label" x="206" y="80" text-anchor="middle" style="fill:#fff">LOD table</text>
  <text class="d-sub"   x="206" y="94" text-anchor="middle" style="fill:#dfe6e0">N × 18 B</text>
  <text class="d-label" x="302" y="80" text-anchor="middle">LOD 0</text>
  <text class="d-sub"   x="302" y="94" text-anchor="middle">coarsest</text>
  <text class="d-label" x="408" y="80" text-anchor="middle">LOD 1</text>
  <text class="d-label" x="523" y="80" text-anchor="middle">LOD N−1</text>
  <text class="d-sub"   x="523" y="94" text-anchor="middle">finest</text>
  <text class="d-label" x="615" y="78" text-anchor="middle" style="fill:#fff;font-size:11px">POIs</text>
  <text class="d-sub"   x="615" y="92" text-anchor="middle" style="fill:#f6e6d8;font-size:8.5px">§7</text>
  <text class="d-label" x="669" y="78" text-anchor="middle" style="fill:#fff;font-size:11px">Nav</text>
  <text class="d-sub"   x="669" y="92" text-anchor="middle" style="fill:#e7ead8;font-size:8.5px">§8 · OBCT after</text>

  <!-- detail arrow -->
  <line x1="250" y1="114" x2="576" y2="114" stroke="#cf6a2a" stroke-width="1.6" marker-end="url(#aF2)" />
  <text class="d-sub" x="413" y="128" text-anchor="middle" style="fill:#a9501c">detail increases →</text>

  <!-- explode LOD 0 -->
  <line x1="246" y1="100" x2="232" y2="152" stroke="#9aa884" stroke-width="1.2" />
  <line x1="358" y1="100" x2="544" y2="152" stroke="#9aa884" stroke-width="1.2" />
  <rect class="d-panel-2" x="232" y="152" width="160" height="40" rx="7" />
  <text class="d-label" x="312" y="170" text-anchor="middle">quadtree index</text>
  <text class="d-sub"   x="312" y="184" text-anchor="middle">flat u32 nodes</text>
  <rect class="d-panel" x="392" y="152" width="152" height="40" rx="7" />
  <text class="d-label" x="468" y="170" text-anchor="middle">offsets + chunks</text>
  <text class="d-sub"   x="468" y="184" text-anchor="middle">unit-aligned chunks</text>
</svg>
<figcaption>Each LOD has a quadtree, a chunk-offset table, and geometry chunks. Global offsets use 16-byte units.</figcaption>
</figure>

The 49-byte header addresses the global sections.
The style table applies to all LODs.
The LOD table orders detail levels from coarse to fine.
Each LOD is independent.
A renderer reads only the LOD for the current meters-per-pixel value.

A LOD table entry is 18 bytes:

| Field | Type | Meaning |
| --- | --- | --- |
| Maximum meters per pixel | `f32` | Upper display threshold for this LOD |
| Index offset | `u32` | Scaled offset to the quadtree |
| Node count | `u32` | Number of quadtree words |
| Chunk size | `u16` | Maximum chunk content size |
| Chunk count | `u32` | Number of geometry chunks |

The chunk size is a capacity limit.
It is not a stride.
A table with `chunk_count + 1` scaled offsets addresses the unit-aligned chunks.

### The header

<figure class="fig">
<svg viewBox="0 0 720 170" role="img" aria-label="The OBCM v14 header is 49 bytes. The diagram shows the 40-byte core. Bytes 40 through 48 contain the offset scale, terrain offset, and terrain length.">
  <text class="d-tag" x="20" y="24">The v14 header: 40-byte core + 9-byte extension</text>

  <!-- field names -->
  <text class="d-sub" x="74"  y="56" text-anchor="middle">Magic</text>
  <text class="d-sub" x="112" y="56" text-anchor="middle" style="font-size:9px">ver</text>
  <text class="d-sub" x="247" y="50" text-anchor="middle">global bbox</text>
  <text class="d-sub" x="247" y="62" text-anchor="middle" style="font-size:9px">4 × i32 · µdeg</text>
  <text class="d-sub" x="404" y="56" text-anchor="middle">style off</text>
  <text class="d-sub" x="446" y="56" text-anchor="middle" style="font-size:9px">n</text>
  <text class="d-sub" x="490" y="56" text-anchor="middle">LOD-tbl off</text>
  <text class="d-sub" x="541" y="56" text-anchor="middle">mkr</text>
  <text class="d-sub" x="597" y="50" text-anchor="middle" style="fill:#a9501c">POI off</text>
  <text class="d-sub" x="597" y="62" text-anchor="middle" style="fill:#a9501c;font-size:9px">→ §7</text>
  <text class="d-sub" x="657" y="50" text-anchor="middle" style="fill:#2c5230">Nav off</text>
  <text class="d-sub" x="657" y="62" text-anchor="middle" style="fill:#2c5230;font-size:9px">→ §8 nav</text>

  <!-- ruler fields (15 px / byte) -->
  <g stroke="#20301d" stroke-width="1">
    <rect x="44"  y="72" width="60"  height="32" class="d-forest" />
    <rect x="104" y="72" width="15"  height="32" class="d-amber" />
    <rect x="119" y="72" width="240" height="32" class="d-water" />
    <rect x="359" y="72" width="60"  height="32" class="d-muted" />
    <rect x="419" y="72" width="15"  height="32" class="d-amber" />
    <rect x="434" y="72" width="60"  height="32" class="d-muted" />
    <rect x="494" y="72" width="30"  height="32" style="fill:#e3ad33" />
    <rect x="524" y="72" width="60"  height="32" class="d-hot-fill" />
    <rect x="584" y="72" width="60"  height="32" class="d-water" />
  </g>
  <!-- per-byte ticks -->
  <g stroke="#20301d" stroke-opacity="0.18" stroke-width="1">
    <line x1="59" y1="72" x2="59" y2="104"/><line x1="74" y1="72" x2="74" y2="104"/><line x1="89" y1="72" x2="89" y2="104"/>
    <line x1="134" y1="72" x2="134" y2="104"/><line x1="149" y1="72" x2="149" y2="104"/><line x1="164" y1="72" x2="164" y2="104"/><line x1="179" y1="72" x2="179" y2="104"/><line x1="194" y1="72" x2="194" y2="104"/><line x1="209" y1="72" x2="209" y2="104"/><line x1="224" y1="72" x2="224" y2="104"/><line x1="239" y1="72" x2="239" y2="104"/><line x1="254" y1="72" x2="254" y2="104"/><line x1="269" y1="72" x2="269" y2="104"/><line x1="284" y1="72" x2="284" y2="104"/><line x1="299" y1="72" x2="299" y2="104"/><line x1="314" y1="72" x2="314" y2="104"/><line x1="329" y1="72" x2="329" y2="104"/><line x1="344" y1="72" x2="344" y2="104"/>
    <line x1="374" y1="72" x2="374" y2="104"/><line x1="389" y1="72" x2="389" y2="104"/><line x1="404" y1="72" x2="404" y2="104"/>
    <line x1="449" y1="72" x2="449" y2="104"/><line x1="464" y1="72" x2="464" y2="104"/><line x1="479" y1="72" x2="479" y2="104"/>
    <line x1="509" y1="72" x2="509" y2="104"/>
    <line x1="539" y1="72" x2="539" y2="104"/><line x1="554" y1="72" x2="554" y2="104"/><line x1="569" y1="72" x2="569" y2="104"/>
    <line x1="599" y1="72" x2="599" y2="104"/><line x1="614" y1="72" x2="614" y2="104"/><line x1="629" y1="72" x2="629" y2="104"/>
  </g>
  <!-- value + byte ranges -->
  <text class="d-label" x="74" y="93" text-anchor="middle" style="fill:#fff;font-size:11px">OBCM</text>
  <text class="d-label" x="112" y="93" text-anchor="middle" style="font-size:11px">14</text>
  <text class="d-sub" x="74"  y="122" text-anchor="middle" style="font-size:9px">0–3</text>
  <text class="d-sub" x="112" y="122" text-anchor="middle" style="font-size:9px">4</text>
  <text class="d-sub" x="239" y="122" text-anchor="middle" style="font-size:9px">5–20</text>
  <text class="d-sub" x="389" y="122" text-anchor="middle" style="font-size:9px">21–24</text>
  <text class="d-sub" x="426" y="122" text-anchor="middle" style="font-size:9px">25</text>
  <text class="d-sub" x="464" y="122" text-anchor="middle" style="font-size:9px">26–29</text>
  <text class="d-sub" x="509" y="122" text-anchor="middle" style="font-size:9px">30–31</text>
  <text class="d-sub" x="554" y="122" text-anchor="middle" style="fill:#a9501c;font-size:9px">32–35</text>
  <text class="d-sub" x="614" y="122" text-anchor="middle" style="fill:#2c5230;font-size:9px">36–39</text>

  <text class="d-sub" x="44" y="150" style="font-size:11px">bytes 40–48: scale u8 · terrain offset u32 · terrain length u32</text>
</svg>
<figcaption>The version is 14. Global offsets count units of 2 to the offset-scale power. Current writers use 16-byte units.</figcaption>
</figure>

The core header fields are:

| Bytes | Field |
| ---: | --- |
| 0–3 | Magic `OBCM` |
| 4 | Version `14` |
| 5–20 | Latitude/longitude bounding box |
| 21–24 | Style-table offset |
| 25 | LOD count |
| 26–29 | LOD-table offset |
| 30–31 | Marker color in RGB565 |
| 32–35 | POI-section offset |
| 36–39 | Navigation-section offset |
| 40 | Base-2 offset scale |
| 41–44 | Optional terrain offset |
| 45–48 | Optional terrain length |

The POI and navigation sections are always present.
An empty section has a valid nonzero offset.
A zero terrain offset and length mean that the map has no terrain.

Each style record is 8 bytes.
It contains the style identifier, z-index, RGB565 color, weight, flags, and optional secondary color.
Flags contain priority, dashed, secondary-color, fixed-width, and terrain-layer bits.
The packer assigns style identifiers from 1 through 254.
Value `0xFF` ends the features in a chunk.

### The quadtree index

<figure class="fig">
<svg viewBox="0 0 720 205" role="img" aria-label="A quadtree node is one 32-bit word. The high bit identifies a branch. Other values identify an empty leaf or a chunk.">
  <text class="d-tag" x="20" y="24">One u32 per node — the high bit decides</text>

  <!-- 32-bit strip -->
  <g stroke="#20301d" stroke-width="0.8">
    <rect x="56" y="48" width="19" height="26" class="d-hot-fill" />
    <rect x="75" y="48" width="589" height="26" fill="#eae4cb" />
  </g>
  <g stroke="#20301d" stroke-opacity="0.12" stroke-width="1">
    <line x1="94" y1="48" x2="94" y2="74"/><line x1="132" y1="48" x2="132" y2="74"/><line x1="170" y1="48" x2="170" y2="74"/><line x1="246" y1="48" x2="246" y2="74"/><line x1="322" y1="48" x2="322" y2="74"/><line x1="398" y1="48" x2="398" y2="74"/><line x1="474" y1="48" x2="474" y2="74"/><line x1="550" y1="48" x2="550" y2="74"/><line x1="626" y1="48" x2="626" y2="74"/>
  </g>
  <text class="d-num" x="65" y="65" text-anchor="middle">b31</text>
  <text class="d-sub" x="370" y="65" text-anchor="middle">bits 30 … 0</text>
  <text class="d-sub" x="65" y="92" text-anchor="middle" style="fill:#a9501c;font-size:9px">branch flag</text>

  <!-- interpretations -->
  <g>
    <rect x="56" y="110" width="14" height="14" rx="3" class="d-hot-fill" />
    <text class="d-label" x="80" y="121" style="font-size:11.5px">high bit set</text>
    <text class="d-sub" x="200" y="121">branch → low 31 bits = index of the first child (NW)</text>

    <rect x="56" y="136" width="14" height="14" rx="3" class="d-muted" />
    <text class="d-label" x="80" y="147" style="font-size:11.5px">0x7FFF_FFFF</text>
    <text class="d-sub" x="200" y="147">empty leaf → nothing to draw here</text>

    <rect x="56" y="162" width="14" height="14" rx="3" class="d-forest" />
    <text class="d-label" x="80" y="173" style="font-size:11.5px">anything else</text>
    <text class="d-sub" x="200" y="173">leaf → the value is a chunk id into this LOD's chunks</text>
  </g>
</svg>
<figcaption>Branches point to four consecutive children. Readers derive child bounds from the parent bounds.</figcaption>
</figure>

The quadtree is a flat array of `u32` words:

- A set high bit identifies a branch.
- The low 31 bits give the first of four consecutive children.
- `0x7FFF_FFFF` identifies an empty leaf.
- Other values identify a geometry chunk.

The child order is northwest, northeast, southwest, and southeast.
The reader calculates child bounds with integer floor midpoints.

### Features: an anchor, then deltas

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="A feature stores one anchor coordinate and then coordinate deltas. Each feature selects 8-bit or 16-bit deltas.">
  <defs>
    <marker id="aF3" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Geometry: one anchor, then a chain of deltas</text>

  <!-- LEFT: absolute -->
  <rect class="d-panel-2" x="24" y="44" width="300" height="184" rx="12" />
  <text class="d-tag" x="40" y="66">absolute · µdeg</text>
  <!-- ring -->
  <polygon points="96,170 120,108 196,96 244,150 188,196" fill="#7c9a63" fill-opacity="0.35" stroke="#3c6b39" stroke-width="1.6" />
  <!-- vertices -->
  <g fill="#3c6b39"><circle cx="120" cy="108" r="3.5"/><circle cx="196" cy="96" r="3.5"/><circle cx="244" cy="150" r="3.5"/><circle cx="188" cy="196" r="3.5"/></g>
  <!-- anchor -->
  <circle cx="96" cy="170" r="5.5" class="d-hot-fill" />
  <text class="d-sub" x="60" y="150" style="fill:#a9501c;font-size:9.5px">anchor</text>
  <text class="d-sub" x="44" y="208" style="font-size:9.5px">(47 123 456, 8 654 321)</text>
  <!-- small delta hints -->
  <text class="d-sub" x="104" y="132" style="font-size:9px">+Δ</text>
  <text class="d-sub" x="158" y="92"  style="font-size:9px">+Δ</text>
  <text class="d-sub" x="226" y="126" style="font-size:9px">+Δ</text>

  <!-- arrow -->
  <line class="d-flow" x1="332" y1="136" x2="392" y2="136" marker-end="url(#aF3)" />
  <text class="d-sub" x="362" y="126" text-anchor="middle" style="font-size:9px">encode</text>

  <!-- RIGHT: encoded -->
  <rect class="d-panel" x="400" y="44" width="296" height="184" rx="12" />
  <text class="d-tag" x="416" y="66">encoded</text>
  <!-- anchor cell -->
  <rect x="416" y="78" width="120" height="30" rx="5" class="d-hot-fill" />
  <text class="d-sub" x="476" y="97" text-anchor="middle" style="fill:#fff;font-size:9.5px">anchor X,Y (i32)</text>
  <text class="d-sub" x="544" y="90" style="font-size:9px">stored vs the</text>
  <text class="d-sub" x="544" y="102" style="font-size:9px">leaf's corner</text>
  <!-- delta cells -->
  <g stroke="#3c6b39" stroke-width="1">
    <rect x="416" y="118" width="44" height="26" rx="4" class="d-muted" />
    <rect x="462" y="118" width="44" height="26" rx="4" class="d-muted" />
    <rect x="508" y="118" width="44" height="26" rx="4" class="d-muted" />
    <rect x="554" y="118" width="44" height="26" rx="4" class="d-muted" />
  </g>
  <text class="d-sub" x="438" y="135" text-anchor="middle" style="font-size:9px">Δx,Δy</text>
  <text class="d-sub" x="484" y="135" text-anchor="middle" style="font-size:9px">Δx,Δy</text>
  <text class="d-sub" x="530" y="135" text-anchor="middle" style="font-size:9px">Δx,Δy</text>
  <text class="d-sub" x="576" y="135" text-anchor="middle" style="font-size:9px">…</text>
  <!-- per-feature width choice -->
  <text class="d-sub" x="416" y="170" style="font-size:10px">every |Δ| ≤ 127  →  <tspan style="fill:#3c6b39">int8</tspan>  · 2 B / point</text>
  <text class="d-sub" x="416" y="190" style="font-size:10px">otherwise           →  <tspan style="fill:#a9501c">int16</tspan> · 4 B / point</text>
  <text class="d-sub" x="416" y="212" style="font-size:9px">chosen once per feature (flag bit 0)</text>
</svg>
<figcaption>Anchor and delta encoding keeps common geometry records small.</figcaption>
</figure>

A feature stores an anchor relative to its leaf.
Subsequent points use signed coordinate deltas.
One flag selects 8-bit or 16-bit delta pairs.

The reader validates the complete feature before it publishes geometry.
An invalid or over-capacity feature is dropped as one unit.
The reader does not return truncated polygons or lines.

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="A compact feature header is 7 bytes. A wide header is 12 bytes. Flags select delta width, polygon data, holes, and header width.">
  <text class="d-tag" x="20" y="24">A feature on disk — both rulers to scale, 1 byte = 40 px</text>

  <!-- compact header ruler: 7 B -->
  <text class="d-sub" x="140" y="52" text-anchor="middle" style="font-size:9px">style</text>
  <text class="d-sub" x="180" y="52" text-anchor="middle" style="font-size:9px">flags</text>
  <text class="d-sub" x="220" y="52" text-anchor="middle" style="font-size:9px">pts</text>
  <text class="d-sub" x="280" y="52" text-anchor="middle" style="font-size:9px">anchor X</text>
  <text class="d-sub" x="360" y="52" text-anchor="middle" style="font-size:9px">anchor Y</text>
  <g stroke="#20301d" stroke-width="1">
    <rect x="120" y="60" width="40" height="32" class="d-forest" />
    <rect x="160" y="60" width="40" height="32" class="d-hot-fill" />
    <rect x="200" y="60" width="40" height="32" class="d-water" />
    <rect x="240" y="60" width="80" height="32" class="d-muted" />
    <rect x="320" y="60" width="80" height="32" class="d-muted" />
  </g>
  <text class="d-tag" x="110" y="80" text-anchor="end" style="font-size:10px">compact · 7 B</text>
  <text class="d-sub" x="280" y="80" text-anchor="middle" style="font-size:9px">u16 · 2 B</text>
  <text class="d-sub" x="360" y="80" text-anchor="middle" style="font-size:9px">u16 · 2 B</text>
  <text class="d-sub" x="140" y="106" text-anchor="middle" style="font-size:8.5px">1 B</text>
  <text class="d-sub" x="180" y="106" text-anchor="middle" style="font-size:8.5px">1 B</text>
  <text class="d-sub" x="220" y="106" text-anchor="middle" style="font-size:8.5px">1 B</text>

  <!-- wide header ruler: 12 B, same scale, same left edge -->
  <g stroke="#20301d" stroke-width="1">
    <rect x="120" y="122" width="40"  height="32" class="d-forest" />
    <rect x="160" y="122" width="40"  height="32" class="d-hot-fill" />
    <rect x="200" y="122" width="80"  height="32" class="d-water" />
    <rect x="280" y="122" width="160" height="32" class="d-muted" />
    <rect x="440" y="122" width="160" height="32" class="d-muted" />
  </g>
  <text class="d-tag" x="110" y="142" text-anchor="end" style="font-size:10px">wide · 12 B</text>
  <text class="d-sub" x="240" y="142" text-anchor="middle" style="fill:#fff;font-size:9px">pts · 2 B</text>
  <text class="d-sub" x="360" y="142" text-anchor="middle" style="font-size:9px">anchor X · i32 · 4 B</text>
  <text class="d-sub" x="520" y="142" text-anchor="middle" style="font-size:9px">anchor Y · i32 · 4 B</text>

  <!-- flags expand: the byte that decides which ruler you are reading -->
  <line x1="180" y1="154" x2="112" y2="182" stroke="#cf6a2a" stroke-width="1.2" />
  <g>
    <rect x="60"  y="182" width="104" height="22" rx="4" class="d-panel-2" />
    <text class="d-sub" x="112" y="197" text-anchor="middle" style="font-size:9px">bit 0 · 16-bit Δ</text>
    <rect x="170" y="182" width="96"  height="22" rx="4" class="d-panel-2" />
    <text class="d-sub" x="218" y="197" text-anchor="middle" style="font-size:9px">bit 1 · polygon</text>
    <rect x="272" y="182" width="82"  height="22" rx="4" class="d-panel-2" />
    <text class="d-sub" x="313" y="197" text-anchor="middle" style="font-size:9px">bit 2 · holes</text>
    <rect x="360" y="182" width="80"  height="22" rx="4" class="d-hot-fill" />
    <text class="d-sub" x="400" y="197" text-anchor="middle" style="fill:#fff;font-size:9px">bit 3 · wide</text>
  </g>
  <text class="d-sub" x="448" y="197" style="fill:#a9501c;font-size:9px">← picks the ruler</text>

  <!-- holes layout ribbon -->
  <text class="d-tag" x="20" y="232">…and a polygon with holes, laid out</text>
  <g stroke="#3c6b39" stroke-width="1.2">
    <rect x="24"  y="242" width="96"  height="34" class="d-hot-fill" />
    <rect x="120" y="242" width="150" height="34" class="d-muted" />
    <rect x="270" y="242" width="70"  height="34" class="d-amber" />
    <rect x="340" y="242" width="64"  height="34" class="d-water" />
    <rect x="404" y="242" width="130" height="34" class="d-muted" />
    <rect x="534" y="242" width="64"  height="34" class="d-water" />
    <rect x="598" y="242" width="98"  height="34" class="d-muted" />
  </g>
  <text class="d-sub" x="72"  y="263" text-anchor="middle" style="fill:#fff;font-size:9.5px">7 or 12 B hdr</text>
  <text class="d-sub" x="195" y="263" text-anchor="middle" style="font-size:9.5px">exterior deltas</text>
  <text class="d-sub" x="305" y="263" text-anchor="middle" style="fill:#3a2c10;font-size:9px">hole cnt</text>
  <text class="d-sub" x="372" y="263" text-anchor="middle" style="fill:#fff;font-size:9px">h1 pts</text>
  <text class="d-sub" x="469" y="263" text-anchor="middle" style="font-size:9.5px">hole 1 deltas</text>
  <text class="d-sub" x="566" y="263" text-anchor="middle" style="fill:#fff;font-size:9px">h2 pts</text>
  <text class="d-sub" x="647" y="263" text-anchor="middle" style="font-size:9.5px">hole 2 …</text>
</svg>
<figcaption>The compact header is the common form. The wide form supports large anchors or point counts.</figcaption>
</figure>

A compact header uses an 8-bit point count and two 16-bit anchor components.
A wide header uses a 16-bit point count and two 32-bit anchor components.
Polygon holes follow the exterior ring.
The even-odd fill rule uses all rings.

### POIs: a nearest-list, not a map layer

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="The POI directory addresses one quadtree per category. Each POI record contains coordinates, subtype, name, and an opening-hours reference.">
  <defs>
    <marker id="aF5" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">The POI section — a quadtree per category over 36-byte records</text>

  <!-- directory -->
  <rect class="d-panel-2" x="24" y="44" width="162" height="88" rx="9" />
  <text class="d-label" x="40" y="62" style="font-size:11px">directory</text>
  <text class="d-sub" x="40" y="78"  style="font-size:9px">count = 6 · chunk size</text>
  <text class="d-sub" x="40" y="92"  style="font-size:9px">per cat: id · index off</text>
  <text class="d-sub" x="40" y="104" style="font-size:9px">node count · chunk count</text>
  <text class="d-sub" x="40" y="122" style="font-size:9px;fill:#a9501c">+ hours-pool off · count</text>

  <!-- one category's index + chunks (LOD-shaped) -->
  <line class="d-flow" x1="188" y1="80" x2="214" y2="80" marker-end="url(#aF5)" />
  <g stroke="#3c6b39" stroke-width="1.2">
    <rect x="220" y="56" width="96"  height="44" class="d-muted" />
    <rect x="316" y="56" width="112" height="44" class="d-water" />
  </g>
  <text class="d-label" x="268" y="76" text-anchor="middle" style="font-size:10.5px">quadtree</text>
  <text class="d-sub"   x="268" y="90" text-anchor="middle" style="font-size:9px">flat u32 · §4</text>
  <text class="d-label" x="372" y="76" text-anchor="middle" style="fill:#fff;font-size:10.5px">POI chunks</text>
  <text class="d-sub"   x="372" y="90" text-anchor="middle" style="fill:#dfe6e0;font-size:9px">512 B · 14 recs</text>
  <text class="d-sub" x="324" y="118" text-anchor="middle" style="font-size:9px;fill:#a9501c">same index-then-chunks shape as a LOD</text>

  <!-- one record: 36-byte ruler -->
  <text class="d-tag" x="20" y="152">one record — a fixed 36 bytes <tspan style="fill:#a9501c">(v14)</tspan></text>
  <g stroke="#20301d" stroke-width="1">
    <rect x="24"  y="164" width="74"  height="34" class="d-water" />
    <rect x="98"  y="164" width="74"  height="34" class="d-water" />
    <rect x="172" y="164" width="19"  height="34" class="d-hot-fill" />
    <rect x="191" y="164" width="19"  height="34" class="d-amber" />
    <rect x="210" y="164" width="408" height="34" class="d-forest" />
    <rect x="618" y="164" width="74"  height="34" style="fill:#cf6a2a" />
  </g>
  <text class="d-sub" x="61"  y="185" text-anchor="middle" style="fill:#fff;font-size:9.5px">Lat (i32)</text>
  <text class="d-sub" x="135" y="185" text-anchor="middle" style="fill:#fff;font-size:9.5px">Lon (i32)</text>
  <text class="d-sub" x="181" y="180" text-anchor="middle" style="fill:#fff;font-size:8px">sub</text>
  <text class="d-sub" x="181" y="192" text-anchor="middle" style="fill:#fff;font-size:7.5px">type</text>
  <text class="d-sub" x="200" y="184" text-anchor="middle" style="font-size:8px">len</text>
  <text class="d-sub" x="414" y="185" text-anchor="middle" style="fill:#fff;font-size:9.5px">Name — 24 B printable ASCII</text>
  <text class="d-sub" x="655" y="180" text-anchor="middle" style="fill:#fff;font-size:8px">Hours</text>
  <text class="d-sub" x="655" y="192" text-anchor="middle" style="fill:#fff;font-size:7.5px">Ref u16</text>
  <text class="d-sub" x="61"  y="214" text-anchor="middle" style="font-size:9px">0–3</text>
  <text class="d-sub" x="135" y="214" text-anchor="middle" style="font-size:9px">4–7</text>
  <text class="d-sub" x="181" y="214" text-anchor="middle" style="font-size:9px">8</text>
  <text class="d-sub" x="200" y="214" text-anchor="middle" style="font-size:9px">9</text>
  <text class="d-sub" x="414" y="214" text-anchor="middle" style="font-size:9px">10–33</text>
  <text class="d-sub" x="655" y="214" text-anchor="middle" style="font-size:9px">34–35</text>
</svg>
<figcaption>Category-specific indexes support nearest and route-corridor queries.</figcaption>
</figure>

The map has one POI quadtree for each category.
The category comes from the selected directory entry.
It is not repeated in each record.

A POI record is 36 bytes.
It contains coordinates, subtype, a 24-byte printable-ASCII name, and `HoursRef`.
The same indexes support nearest-item and route-corridor queries.

### Opening hours: a pooled weekly schedule

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="A 29-byte schedule contains flags and two time intervals for each weekday. POI records reference deduplicated schedules.">
  <defs>
    <marker id="aH7" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">One 29-byte schedule blob — flags + 7 days × 2 slots</text>

  <!-- blob ruler: flags + Mon..Sun (each 2 slots of open_q/close_q) -->
  <g stroke="#20301d" stroke-width="1">
    <rect x="24" y="44" width="34" height="34" class="d-amber" />
    <rect x="58" y="44" width="88" height="34" class="d-water" />
    <rect x="146" y="44" width="88" height="34" class="d-forest" />
    <rect x="234" y="44" width="88" height="34" class="d-water" />
    <rect x="322" y="44" width="88" height="34" class="d-forest" />
    <rect x="410" y="44" width="88" height="34" class="d-water" />
    <rect x="498" y="44" width="88" height="34" class="d-forest" />
    <rect x="586" y="44" width="88" height="34" class="d-water" />
  </g>
  <text class="d-sub" x="41"  y="65" text-anchor="middle" style="fill:#000;font-size:9px">flags</text>
  <text class="d-sub" x="102" y="65" text-anchor="middle" style="fill:#fff;font-size:10px">Mon</text>
  <text class="d-sub" x="190" y="65" text-anchor="middle" style="fill:#fff;font-size:10px">Tue</text>
  <text class="d-sub" x="278" y="65" text-anchor="middle" style="fill:#fff;font-size:10px">Wed</text>
  <text class="d-sub" x="366" y="65" text-anchor="middle" style="fill:#fff;font-size:10px">Thu</text>
  <text class="d-sub" x="454" y="65" text-anchor="middle" style="fill:#fff;font-size:10px">Fri</text>
  <text class="d-sub" x="542" y="65" text-anchor="middle" style="fill:#fff;font-size:10px">Sat</text>
  <text class="d-sub" x="630" y="65" text-anchor="middle" style="fill:#fff;font-size:10px">Sun</text>
  <text class="d-sub" x="41"  y="92" text-anchor="middle" style="font-size:9px">0</text>
  <text class="d-sub" x="102" y="92" text-anchor="middle" style="font-size:9px">1–4</text>
  <text class="d-sub" x="630" y="92" text-anchor="middle" style="font-size:9px">25–28</text>

  <!-- one day exploded into 2 slots × (open_q, close_q) -->
  <line x1="58"  y1="78" x2="120" y2="110" stroke="#9aa884" stroke-width="1.1" />
  <line x1="146" y1="78" x2="420" y2="110" stroke="#9aa884" stroke-width="1.1" />
  <g stroke="#20301d" stroke-width="1">
    <rect x="120" y="112" width="76" height="30" class="d-panel" />
    <rect x="196" y="112" width="76" height="30" class="d-panel" />
    <rect x="272" y="112" width="76" height="30" class="d-panel-2" />
    <rect x="348" y="112" width="76" height="30" class="d-panel-2" />
  </g>
  <text class="d-sub" x="158" y="131" text-anchor="middle" style="font-size:9.5px">open q</text>
  <text class="d-sub" x="234" y="131" text-anchor="middle" style="font-size:9.5px">close q</text>
  <text class="d-sub" x="310" y="131" text-anchor="middle" style="font-size:9.5px">open q</text>
  <text class="d-sub" x="386" y="131" text-anchor="middle" style="font-size:9.5px">close q</text>
  <text class="d-sub" x="196" y="156" text-anchor="middle" style="font-size:8.5px;fill:#a9501c">slot 0</text>
  <text class="d-sub" x="348" y="156" text-anchor="middle" style="font-size:8.5px;fill:#a9501c">slot 1</text>
  <text class="d-sub" x="470" y="126" style="font-size:9.5px">each byte = quarter-hours</text>
  <text class="d-sub" x="470" y="140" style="font-size:9.5px">from midnight, 0…96 (96 = 24:00)</text>

  <!-- dedup pool -->
  <text class="d-tag" x="20" y="192">the pool — identical schedules collapse to one blob</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="30" y="216" style="font-size:9.5px">POI · HoursRef 0</text>
    <text class="d-sub" x="30" y="234" style="font-size:9.5px">POI · HoursRef 0</text>
    <text class="d-sub" x="30" y="252" style="font-size:9.5px">POI · HoursRef 2</text>
    <text class="d-sub" x="30" y="270" style="font-size:9.5px">POI · HoursRef 0xFFFF</text>
  </g>
  <line class="d-flow" x1="180" y1="212" x2="300" y2="221" marker-end="url(#aH7)" />
  <line class="d-flow" x1="180" y1="230" x2="300" y2="223" marker-end="url(#aH7)" />
  <line class="d-flow" x1="180" y1="248" x2="300" y2="279" marker-end="url(#aH7)" />
  <text class="d-sub" x="150" y="286" style="font-size:8.5px;fill:#a9501c">0xFFFF = no hours (no arrow)</text>

  <!-- pool blobs -->
  <g stroke="#3c6b39" stroke-width="1.1">
    <rect x="306" y="210" width="180" height="26" class="d-water" />
    <rect x="306" y="238" width="180" height="26" class="d-muted" />
    <rect x="306" y="266" width="180" height="26" class="d-water" />
  </g>
  <text class="d-sub" x="316" y="227" style="fill:#fff;font-size:9.5px">blob 0 — 29 B</text>
  <text class="d-sub" x="316" y="255" style="fill:#fff;font-size:9.5px">blob 1 — 29 B</text>
  <text class="d-sub" x="316" y="283" style="fill:#fff;font-size:9.5px">blob 2 — 29 B</text>
  <text class="d-sub" x="504" y="227" style="font-size:9px">count u16, then</text>
  <text class="d-sub" x="504" y="241" style="font-size:9px">count × 29-byte blobs;</text>
  <text class="d-sub" x="504" y="255" style="font-size:9px">blob i at</text>
  <text class="d-sub" x="504" y="269" style="font-size:9px" font-family="var(--mono)">pool_off + 2 + i·29</text>
</svg>
<figcaption>The packer converts opening-hours text to fixed weekly schedules. The device does not parse the source grammar.</figcaption>
</figure>

Each schedule contains two intervals for each weekday.
Times use 15-minute units.
`HoursRef = 0xFFFF` means that no parsed schedule is available.
Seasonal or unsupported source rules set schedule flags.

### The navigation graph: a routable network

<figure class="fig">
<svg viewBox="0 0 720 256" role="img" aria-label="The OBCM v14 navigation section contains profiles, a node quadtree, junction records, edge geometry, and snap anchors.">
  <defs>
    <marker id="aN1" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="22">§8 (v14) — graph · edge pool · sparse exact-snap index</text>

  <!-- directory -->
  <rect class="d-panel-2" x="24" y="42" width="140" height="64" rx="9" />
  <text class="d-label" x="38" y="60" style="font-size:11px">nav directory</text>
  <text class="d-sub" x="38" y="76"  style="font-size:9px">40 B — resident</text>
  <text class="d-sub" x="38" y="89"  style="font-size:9px">offsets · counts</text>
  <text class="d-sub" x="38" y="102" style="font-size:9px">chunk size · profiles</text>

  <!-- profile table -->
  <line class="d-flow" x1="166" y1="74" x2="196" y2="74" marker-end="url(#aN1)" />
  <rect class="d-water" x="200" y="48" width="128" height="52" rx="9" stroke="#3c6b39" stroke-width="1.2" />
  <text class="d-label" x="264" y="70" text-anchor="middle" style="fill:#fff;font-size:10.5px">profile table</text>
  <text class="d-sub" x="264" y="86" text-anchor="middle" style="fill:#dfe6e0;font-size:8.5px">1..8 × 56 B</text>

  <!-- quadtree -->
  <line class="d-flow" x1="330" y1="74" x2="360" y2="74" marker-end="url(#aN1)" />
  <rect class="d-panel" x="364" y="50" width="118" height="48" rx="9" />
  <text class="d-label" x="423" y="70" text-anchor="middle" style="font-size:10px">node quadtree</text>
  <text class="d-sub" x="423" y="86" text-anchor="middle" style="font-size:8.5px">flat u32 · §4</text>

  <!-- junction chunks -->
  <line class="d-flow" x1="484" y1="74" x2="514" y2="74" marker-end="url(#aN1)" />
  <rect class="d-water" x="518" y="50" width="178" height="48" rx="9" stroke="#3c6b39" stroke-width="1.2" />
  <text class="d-label" x="607" y="68" text-anchor="middle" style="fill:#fff;font-size:10px">junction records</text>
  <text class="d-sub" x="607" y="84" text-anchor="middle" style="fill:#dfe6e0;font-size:8px">variable · 512 B chunks</text>
  <text class="d-sub" x="607" y="114" text-anchor="middle" style="font-size:8px;fill:#a9501c">bin-packed — leaves may share a chunk</text>

  <!-- edge pool (separate offset) -->
  <line class="d-flow" x1="94" y1="106" x2="94" y2="140" marker-end="url(#aN1)" />
  <rect class="d-muted" x="24" y="142" width="150" height="46" rx="9" stroke="#3c6b39" stroke-width="1.2" />
  <text class="d-label" x="38" y="162" style="font-size:10.5px">edge pool</text>
  <text class="d-sub" x="38" y="178" style="font-size:9px">polylines · own offset</text>
  <text class="d-sub" x="184" y="158" style="font-size:8.5px;fill:#a9501c">edge id = (chunk, ordinal)</text>
  <text class="d-sub" x="184" y="171" style="font-size:8.5px">chunk = id &gt;&gt; 5 · ordinal = id &amp; 31</text>
  <text class="d-sub" x="184" y="184" style="font-size:8.5px">fetched for exact projection + route emit</text>

  <!-- explode one junction record -->
  <line x1="518" y1="98" x2="410" y2="150" stroke="#9aa884" stroke-width="1.1" />
  <line x1="696" y1="98" x2="700" y2="150" stroke="#9aa884" stroke-width="1.1" />
  <rect class="d-hot" x="392" y="150" width="308" height="96" rx="10" style="fill:#f8efe4" />
  <text class="d-tag" x="408" y="168" style="fill:#a9501c">one junction record — 13 + 17 × degree B</text>
  <text class="d-sub" x="408" y="186" style="font-size:9.5px">lat · lon · dense id · degree</text>
  <text class="d-sub" x="408" y="202" style="font-size:9.5px">then <tspan style="font-weight:700">degree</tspan> × neighbor (17 B each):</text>
  <text class="d-sub" x="420" y="218" style="font-size:8.5px" font-family="var(--mono)">nbr id · nbr lat,lon · edge id · cost m · way-kind · ascent m</text>
  <text class="d-sub" x="420" y="234" style="font-size:8px;fill:#a9501c">coord, way-kind + ascent inline — a settle relaxes with no extra fetch</text>
</svg>
<figcaption>Junction records include neighbor coordinates, way kind, and ascent. Edge identifiers use a chunk and record ordinal.</figcaption>
</figure>

The navigation section uses 512-byte chunks.
Its 40-byte directory addresses these regions:

- Profile table
- Node quadtree and junction chunks
- Edge geometry pool
- Sparse snap-anchor quadtree and chunks

Each junction record includes its neighbor coordinates.
The router can calculate its heuristic without another read.
Each directional neighbor entry also stores way kind, cost, and integrated ascent.

Edges longer than 300 m get sparse lookup anchors.
The anchors make each accepted road discoverable within the 251 m lookup radius.
The router then projects the endpoint onto the complete stored polyline.

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="One A-star settle reads one junction chunk. The record contains the data required to relax each neighbor.">
  <defs>
    <marker id="aN2" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
    <marker id="aN3" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">One A* settle — descend, read one chunk, relax inline</text>

  <!-- 1 descend quadtree to leaf -->
  <text class="d-sub" x="30" y="52" style="font-size:9px;fill:#6b7758">① descend to the settled node's leaf</text>
  <!-- quadtree box -->
  <rect x="34" y="60" width="150" height="150" fill="none" stroke="#9aa884" stroke-width="1.3" />
  <line x1="109" y1="60" x2="109" y2="210" stroke="#9aa884" stroke-width="0.9" />
  <line x1="34" y1="135" x2="184" y2="135" stroke="#9aa884" stroke-width="0.9" />
  <!-- descend into SE quadrant, subdivide again -->
  <line x1="146" y1="135" x2="146" y2="210" stroke="#c9bfa0" stroke-width="0.8" />
  <line x1="109" y1="172" x2="184" y2="172" stroke="#c9bfa0" stroke-width="0.8" />
  <!-- the leaf highlighted -->
  <rect x="146" y="172" width="38" height="38" fill="#cf6a2a" fill-opacity="0.16" stroke="#cf6a2a" stroke-width="1.4" />
  <!-- settled node point -->
  <circle cx="165" cy="191" r="4" class="d-hot-fill" />
  <text class="d-sub" x="150" y="228" style="font-size:8.5px;fill:#a9501c">settled node</text>
  <text class="d-sub" x="40" y="245" style="font-size:8.5px">a point query — one leaf, not a viewport</text>

  <!-- arrow: one chunk read -->
  <line x1="196" y1="150" x2="252" y2="150" stroke="#cf6a2a" stroke-width="2" marker-end="url(#aN2)" />
  <text x="224" y="142" text-anchor="middle" style="font-family:var(--mono);font-size:8.5px;fill:#a9501c">1 chunk read</text>

  <!-- 2 the record in RAM -->
  <text class="d-sub" x="264" y="52" style="font-size:9px;fill:#6b7758">② its record — one 512 B chunk in RAM</text>
  <rect class="d-panel" x="264" y="60" width="180" height="150" rx="10" />
  <text class="d-tag" x="280" y="80">junction record</text>
  <text class="d-sub" x="280" y="100" style="font-size:9px">lat · lon · id · degree = 3</text>
  <g stroke="#3c6b39" stroke-width="1">
    <rect x="280" y="112" width="148" height="26" rx="4" class="d-water" />
    <rect x="280" y="142" width="148" height="26" rx="4" class="d-water" />
    <rect x="280" y="172" width="148" height="26" rx="4" class="d-water" />
  </g>
  <text class="d-sub" x="288" y="129" style="fill:#fff;font-size:8.5px">nbr A · coord · edge · cost</text>
  <text class="d-sub" x="288" y="159" style="fill:#fff;font-size:8.5px">nbr B · coord · edge · cost</text>
  <text class="d-sub" x="288" y="189" style="fill:#fff;font-size:8.5px">nbr C · coord · edge · cost</text>

  <!-- 3 relax each neighbor -->
  <line x1="452" y1="150" x2="508" y2="150" stroke="#3c6b39" stroke-width="2" marker-end="url(#aN3)" />
  <text x="480" y="142" text-anchor="middle" style="font-family:var(--mono);font-size:8.5px;fill:#3c6b39">relax</text>
  <text class="d-sub" x="520" y="52" style="font-size:9px;fill:#6b7758">③ relax — no further read</text>
  <rect class="d-hot" x="520" y="60" width="180" height="150" rx="10" style="fill:#f8efe4" />
  <text class="d-sub" x="536" y="84" style="font-size:9.5px">per neighbor, from bytes</text>
  <text class="d-sub" x="536" y="98" style="font-size:9.5px">already in hand:</text>
  <text class="d-sub" x="536" y="118" style="font-family:var(--mono);font-size:9px">g' = g + cost_m · w + asc · c</text>
  <text class="d-sub" x="536" y="134" style="font-family:var(--mono);font-size:8.5px;fill:#a9501c">w  = profile(way_kind)</text>
  <text class="d-sub" x="536" y="150" style="font-family:var(--mono);font-size:9px">h  = gc_dist(nbr, goal)</text>
  <text class="d-sub" x="536" y="166" style="font-family:var(--mono);font-size:9px;fill:#a9501c">f  = g' + ε·h</text>
  <text class="d-sub" x="536" y="186" style="font-size:8px">coord inline → <tspan style="font-weight:700">h</tspan>: zero fetches</text>
  <text class="d-sub" x="536" y="197" style="font-size:8px">way-kind + ascent inline → <tspan style="font-weight:700">w</tspan>, <tspan style="font-weight:700">c</tspan>: none either</text>

  <!-- edge-pool footnote -->
  <rect class="d-panel-2" x="34" y="258" width="666" height="30" rx="8" />
  <text class="d-sub" x="366" y="277" text-anchor="middle" style="font-size:9px">during <tspan style="font-weight:700">A*</tspan> the edge pool is untouched; exact endpoint projection and final emit stream only the geometry they need</text>
</svg>
<figcaption>A-star reads edge geometry only for endpoint projection and final route output.</figcaption>
</figure>

The route search uses node records only.
It reads edge geometry for exact endpoint projection and final route output.
For routing behavior and limits, see [the router seam](../architecture/#on-device-routing-the-router-seam).

## OBCR — the route

OBCR v3 is the only supported route version.
A route is one ordered polyline with elevations.
The header also stores exact route statistics.
An optional table stores named waypoints.

### The file

<figure class="fig">
<svg viewBox="0 0 720 215" role="img" aria-label="An OBCR v3 file contains a 128-byte header, route chunks, a chunk index, and an optional waypoint table.">
  <text class="d-tag" x="20" y="24">OBCR — the route, front to back</text>

  <!-- ribbon -->
  <g stroke="#3c6b39" stroke-width="1.4">
    <rect x="24"  y="56" width="88"  height="44" class="d-forest" />
    <rect x="112" y="56" width="104" height="44" class="d-muted" />
    <rect x="216" y="56" width="104" height="44" class="d-muted" />
    <rect x="320" y="56" width="76"  height="44" class="d-muted" />
    <rect x="396" y="56" width="104" height="44" class="d-muted" />
    <rect x="500" y="56" width="108" height="44" class="d-water" />
    <rect x="608" y="56" width="88"  height="44" class="d-amber" />
  </g>
  <text class="d-label" x="68"  y="80" text-anchor="middle" style="fill:#fff">Header</text>
  <text class="d-sub"   x="68"  y="94" text-anchor="middle" style="fill:#e7ead8">128 B</text>
  <text class="d-label" x="164" y="82" text-anchor="middle">Chunk 0</text>
  <text class="d-label" x="268" y="82" text-anchor="middle">Chunk 1</text>
  <text class="d-label" x="358" y="82" text-anchor="middle">···</text>
  <text class="d-label" x="448" y="82" text-anchor="middle">Chunk N−1</text>
  <text class="d-label" x="554" y="80" text-anchor="middle" style="fill:#fff">Chunk index</text>
  <text class="d-sub"   x="554" y="94" text-anchor="middle" style="fill:#dfe6e0">N × 44 B</text>
  <text class="d-label" x="652" y="80" text-anchor="middle">Waypoints</text>
  <text class="d-sub"   x="652" y="94" text-anchor="middle">W × 44 B</text>

  <!-- offsets -->
  <text class="d-sub" x="164" y="120" text-anchor="middle" style="font-size:9px">↑ Data Offset = 128</text>
  <text class="d-sub" x="554" y="120" text-anchor="middle" style="font-size:9px">↑ Index Offset</text>
  <text class="d-sub" x="668" y="120" text-anchor="middle" style="font-size:9px">↑ Waypoint Offset</text>

  <!-- explode a chunk -->
  <line x1="216" y1="100" x2="232" y2="150" stroke="#9aa884" stroke-width="1.2" />
  <line x1="320" y1="100" x2="540" y2="150" stroke="#9aa884" stroke-width="1.2" />
  <rect class="d-panel-2" x="232" y="150" width="308" height="44" rx="8" />
  <text class="d-sub" x="250" y="168" style="font-size:10px">data = (point count − 1) × 6 B records:</text>
  <g stroke="#3c6b39" stroke-width="1">
    <rect x="392" y="172" width="44" height="16" class="d-muted" />
    <rect x="436" y="172" width="44" height="16" class="d-muted" />
    <rect x="480" y="172" width="44" height="16" class="d-water" />
  </g>
  <text class="d-sub" x="414" y="184" text-anchor="middle" style="font-size:8.5px">dLon</text>
  <text class="d-sub" x="458" y="184" text-anchor="middle" style="font-size:8.5px">dLat</text>
  <text class="d-sub" x="502" y="184" text-anchor="middle" style="fill:#fff;font-size:8.5px">ele</text>
</svg>
<figcaption>The writer puts the index and waypoints after streamed chunk data.</figcaption>
</figure>

The 128-byte header contains the route name, bounds, start point, statistics, and section offsets.
Each 44-byte chunk-index entry contains its anchor, bounds, cumulative statistics, byte offset, and point count.
Each route point record stores longitude delta, latitude delta, and absolute elevation.

### Waypoints: a category and a side

<figure class="fig">
<svg viewBox="0 0 720 166" role="img" aria-label="An OBCR v3 waypoint record is 44 bytes. It contains route distance, position, elevation, category, lateral offset, and name.">
  <text class="d-tag" x="20" y="24">One waypoint — a fixed 44 bytes <tspan style="fill:#a9501c">(v3)</tspan></text>
  <g stroke="#20301d" stroke-width="1">
    <rect x="24"  y="40" width="61"  height="34" class="d-forest" />
    <rect x="85"  y="40" width="61"  height="34" class="d-water" />
    <rect x="146" y="40" width="61"  height="34" class="d-water" />
    <rect x="207" y="40" width="30"  height="34" class="d-muted" />
    <rect x="237" y="40" width="15"  height="34" class="d-hot-fill" />
    <rect x="252" y="40" width="15"  height="34" class="d-muted" />
    <rect x="267" y="40" width="30"  height="34" class="d-hot-fill" />
    <rect x="297" y="40" width="30"  height="34" class="d-muted" />
    <rect x="327" y="40" width="365" height="34" class="d-forest" />
  </g>
  <text class="d-sub" x="54"  y="55" text-anchor="middle" style="fill:#fff;font-size:8.5px">Distance</text>
  <text class="d-sub" x="54"  y="67" text-anchor="middle" style="fill:#e7ead8;font-size:8px">Along (u32)</text>
  <text class="d-sub" x="115" y="61" text-anchor="middle" style="fill:#fff;font-size:9.5px">Lon (i32)</text>
  <text class="d-sub" x="176" y="61" text-anchor="middle" style="fill:#fff;font-size:9.5px">Lat (i32)</text>
  <text class="d-sub" x="222" y="55" text-anchor="middle" style="font-size:8px">ele</text>
  <text class="d-sub" x="222" y="67" text-anchor="middle" style="font-size:7.5px">i16</text>
  <text class="d-sub" x="244" y="61" text-anchor="middle" style="fill:#fff;font-size:8px">c</text>
  <text class="d-sub" x="259" y="61" text-anchor="middle" style="font-size:8px">n</text>
  <text class="d-sub" x="282" y="55" text-anchor="middle" style="fill:#fff;font-size:8px">off</text>
  <text class="d-sub" x="282" y="67" text-anchor="middle" style="fill:#fff;font-size:7.5px">i16</text>
  <text class="d-sub" x="312" y="61" text-anchor="middle" style="font-size:8px">rsv</text>
  <text class="d-sub" x="509" y="61" text-anchor="middle" style="fill:#fff;font-size:9.5px">Name — 24 B UTF-8, null-padded</text>
  <text class="d-sub" x="54"  y="90" text-anchor="middle" style="font-size:9px">0–3</text>
  <text class="d-sub" x="115" y="90" text-anchor="middle" style="font-size:9px">4–7</text>
  <text class="d-sub" x="176" y="90" text-anchor="middle" style="font-size:9px">8–11</text>
  <text class="d-sub" x="222" y="90" text-anchor="middle" style="font-size:9px">12–13</text>
  <text class="d-sub" x="244" y="102" text-anchor="middle" style="font-size:9px">14</text>
  <text class="d-sub" x="259" y="114" text-anchor="middle" style="font-size:9px">15</text>
  <text class="d-sub" x="282" y="90" text-anchor="middle" style="font-size:9px">16–17</text>
  <text class="d-sub" x="312" y="102" text-anchor="middle" style="font-size:9px">18–19</text>
  <text class="d-sub" x="509" y="90" text-anchor="middle" style="font-size:9px">20–43</text>
  <text class="d-sub" x="24" y="140" style="font-size:10px"><tspan style="fill:#a9501c">category</tspan> identifies the waypoint kind</text>
  <text class="d-sub" x="24" y="156" style="font-size:10px"><tspan style="fill:#a9501c">lateral offset</tspan> is signed meters; positive is right of travel</text>
</svg>
<figcaption>A positive lateral offset is to the right of travel.</figcaption>
</figure>

The converter maps GPX symbols and types to canonical waypoint categories.
It projects each waypoint onto the route.
The stored distance uses the route axis.
The signed lateral offset shows which side of the route contains the waypoint.

### Chunks, seams, and deltas

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="Route chunks share their boundary point. Each index entry contains an anchor, bounds, and cumulative statistics.">
  <text class="d-tag" x="20" y="24">Chunks share their seam; position chains by delta</text>

  <!-- LEFT: seam sharing -->
  <path d="M40 150 C 80 90, 120 100, 150 130" fill="none" stroke="#3c6b39" stroke-width="3.5" />
  <path d="M150 130 C 180 160, 210 180, 250 150" fill="none" stroke="#cf6a2a" stroke-width="3.5" />
  <path d="M250 150 C 285 124, 300 96, 332 86" fill="none" stroke="#33575b" stroke-width="3.5" />
  <!-- interior vertices -->
  <g fill="#6b7758"><circle cx="92" cy="100" r="2.6"/><circle cx="196" cy="166" r="2.6"/><circle cx="288" cy="110" r="2.6"/></g>
  <!-- shared seam vertices -->
  <g fill="#cf6a2a" stroke="#20301d" stroke-width="0.8"><circle cx="150" cy="130" r="5.5"/><circle cx="250" cy="150" r="5.5"/></g>
  <text class="d-sub" x="40"  y="178" style="font-size:9.5px">chunk 0</text>
  <text class="d-sub" x="196" y="200" text-anchor="middle" style="font-size:9.5px">chunk 1</text>
  <text class="d-sub" x="312" y="74"  style="font-size:9.5px">chunk 2</text>
  <text class="d-sub" x="150" y="112" text-anchor="middle" style="fill:#a9501c;font-size:9px">shared</text>
  <text class="d-sub" x="40" y="224" style="font-size:10px">chunk k's last point = chunk k+1's anchor</text>

  <!-- RIGHT: one chunk's parts -->
  <rect class="d-panel-2" x="404" y="48" width="292" height="78" rx="10" />
  <text class="d-tag" x="420" y="68">index entry (resident)</text>
  <text class="d-sub" x="420" y="88"  style="font-size:10px">anchor (lon, lat, ele) · bbox</text>
  <text class="d-sub" x="420" y="106" style="font-size:10px">cum distance · cum ascent · byte off/len</text>

  <rect class="d-panel" x="404" y="138" width="292" height="78" rx="10" />
  <text class="d-tag" x="420" y="158">chunk data (streamed)</text>
  <g stroke="#3c6b39" stroke-width="1">
    <rect x="420" y="170" width="50" height="20" class="d-muted" />
    <rect x="470" y="170" width="50" height="20" class="d-muted" />
    <rect x="520" y="170" width="50" height="20" class="d-water" />
    <rect x="578" y="170" width="100" height="20" fill="none" stroke="none" />
  </g>
  <text class="d-sub" x="445" y="184" text-anchor="middle" style="font-size:9px">dLon</text>
  <text class="d-sub" x="495" y="184" text-anchor="middle" style="font-size:9px">dLat</text>
  <text class="d-sub" x="545" y="184" text-anchor="middle" style="fill:#fff;font-size:9px">ele</text>
  <text class="d-sub" x="588" y="184" style="font-size:11px">× (n−1)</text>
  <text class="d-sub" x="420" y="208" style="font-size:9.5px">position = delta · elevation = absolute</text>
</svg>
<figcaption>Shared seam points let a renderer draw each chunk without a gap.</figcaption>
</figure>

A route chunk starts with an absolute anchor.
Its remaining points use 16-bit coordinate deltas.
Adjacent chunks repeat their shared boundary point.
This rule prevents visible gaps.

### Exact stats, decimated geometry

The converter calculates totals from all input points.
It can decimate the stored geometry after this calculation.
Distance, ascent, descent, and elevation range remain exact.

## Recorded rides — the v3 ride object

<figure class="fig">
<svg viewBox="0 0 720 258" role="img" aria-label="A recorded ride contains 20-byte samples. Finalization appends one fixed summary footer.">
  <defs>
    <marker id="rr1" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">The 20-byte ride sample — final bytes from the first write</text>

  <!-- field names -->
  <text class="d-sub" x="106" y="56" text-anchor="middle" style="font-size:9.5px">lon (i32)</text>
  <text class="d-sub" x="210" y="56" text-anchor="middle" style="font-size:9.5px">lat (i32)</text>
  <text class="d-sub" x="288" y="56" text-anchor="middle" style="font-size:9.5px">ele</text>
  <text class="d-sub" x="340" y="56" text-anchor="middle" style="font-size:9px">flags</text>
  <text class="d-sub" x="418" y="56" text-anchor="middle" style="font-size:9.5px">t_ms (u32)</text>
  <text class="d-sub" x="483" y="56" text-anchor="middle" style="fill:#a9501c;font-size:9px">hr</text>
  <text class="d-sub" x="509" y="56" text-anchor="middle" style="fill:#a9501c;font-size:9px">cad</text>
  <text class="d-sub" x="548" y="56" text-anchor="middle" style="fill:#a9501c;font-size:9px">pwr</text>

  <!-- ruler rects (26 px / byte, origin x=54) -->
  <g stroke="#20301d" stroke-width="1">
    <rect x="54"  y="64" width="104" height="34" class="d-water" />
    <rect x="158" y="64" width="104" height="34" class="d-water" />
    <rect x="262" y="64" width="52"  height="34" class="d-muted" />
    <rect x="314" y="64" width="52"  height="34" class="d-amber" />
    <rect x="366" y="64" width="104" height="34" class="d-forest" />
    <rect x="470" y="64" width="26"  height="34" class="d-hot-fill" />
    <rect x="496" y="64" width="26"  height="34" class="d-hot-fill" />
    <rect x="522" y="64" width="52"  height="34" class="d-hot-fill" />
  </g>
  <!-- field values -->
  <text class="d-sub" x="340" y="85" text-anchor="middle" style="font-size:8px">bit0 = seg</text>
  <text class="d-sub" x="418" y="85" text-anchor="middle" style="fill:#e7ead8;font-size:8px">millis</text>

  <!-- byte ranges -->
  <text class="d-sub" x="106" y="112" text-anchor="middle" style="font-size:9px">0–3</text>
  <text class="d-sub" x="210" y="112" text-anchor="middle" style="font-size:9px">4–7</text>
  <text class="d-sub" x="288" y="112" text-anchor="middle" style="font-size:9px">8–9</text>
  <text class="d-sub" x="340" y="112" text-anchor="middle" style="font-size:9px">10–11</text>
  <text class="d-sub" x="418" y="112" text-anchor="middle" style="font-size:9px">12–15</text>
  <text class="d-sub" x="483" y="112" text-anchor="middle" style="font-size:9px">16</text>
  <text class="d-sub" x="509" y="112" text-anchor="middle" style="font-size:9px">17</text>
  <text class="d-sub" x="548" y="112" text-anchor="middle" style="font-size:9px">18–19</text>
  <text class="d-sub" x="590" y="86" style="fill:#a9501c;font-size:8.5px">sensor tail</text>
  <text class="d-sub" x="590" y="98" style="fill:#a9501c;font-size:8px">0xFF/0xFFFF = absent</text>

  <!-- Finish append -->
  <rect class="d-panel-2" x="40" y="168" width="158" height="64" rx="10" />
  <text class="d-label" x="119" y="192" text-anchor="middle" style="font-size:10.5px">ride payload</text>
  <text class="d-sub" x="119" y="208" text-anchor="middle" style="font-size:9px">N × 20 B samples</text>
  <text class="d-sub" x="119" y="222" text-anchor="middle" style="font-size:9px;fill:#a9501c">written in place</text>

  <line class="d-flow" x1="198" y1="200" x2="302" y2="200" marker-end="url(#rr1)" />
  <text class="d-sub" x="250" y="190" text-anchor="middle" style="font-size:9px">Finish</text>
  <text class="d-sub" x="250" y="216" text-anchor="middle" style="font-size:8.5px">append only</text>

  <rect class="d-panel" x="308" y="164" width="384" height="34" rx="8" />
  <text class="d-sub" x="320" y="185" style="font-size:9.5px"><tspan class="d-label">84-byte footer</tspan> — start · totals · sensors · points · name</text>

  <rect class="d-hot" x="308" y="206" width="384" height="34" rx="8" style="fill:#f8efe4" />
  <text class="d-sub" x="320" y="227" style="font-size:9.5px"><tspan class="d-label" style="fill:#a9501c">one commit</tspan> — final length + CRC, RECORDING cleared</text>
</svg>
<figcaption>Ride finalization does not rewrite sample data.</figcaption>
</figure>

Each 20-byte sample contains position, elevation, flags, time, heart rate, cadence, and power.
A segment-start flag separates discontinuous track segments.
Finalization appends a fixed summary footer.
The device can recover a summary without scanning all samples.

The byte contract is in [the BLE interface specification](src:specs/obc-ble-interface-spec.md).
Shared vectors include [`ride-v3.bin`](src:specs/vectors/ride-v3.bin).

## OBCT — the terrain raster

OBCT v1 stores orthometric heights as signed 16-bit meters.
Value `-32768` means `NODATA`.
The sample posting and cell size are header values.

<figure class="fig">
<svg viewBox="0 0 720 340" role="img" aria-label="OBCT v1 uses an integer sample lattice, 16 by 16 sample tiles, terrain cells, and a directory-based container.">
  <defs>
    <marker id="aTF" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="22">§1 lattice → §2 tile → §3 cell → §4 container</text>

  <!-- lattice -->
  <rect class="d-panel-2" x="24" y="40" width="150" height="118" rx="9" />
  <text class="d-label" x="38" y="60" style="font-size:10.5px">sample lattice</text>
  <g stroke="#9aa884" stroke-width="0.8">
    <line x1="42" y1="72" x2="158" y2="72" /><line x1="42" y1="88" x2="158" y2="88" />
    <line x1="42" y1="104" x2="158" y2="104" /><line x1="42" y1="120" x2="158" y2="120" />
    <line x1="58" y1="68" x2="58" y2="124" /><line x1="82" y1="68" x2="82" y2="124" />
    <line x1="106" y1="68" x2="106" y2="124" /><line x1="130" y1="68" x2="130" y2="124" />
  </g>
  <circle cx="82" cy="88" r="3" class="d-hot-fill" />
  <text class="d-sub" x="38" y="137" style="font-size:8.5px">posting 2&#8313; &#181;deg</text>
  <text class="d-sub" x="38" y="149" style="font-size:8.5px">&#8776; 57 &#215; 39 m &#183; int16 m</text>

  <line class="d-flow" x1="178" y1="92" x2="204" y2="92" marker-end="url(#aTF)" />

  <!-- tile -->
  <rect class="d-panel" x="208" y="40" width="150" height="118" rx="9" />
  <text class="d-label" x="222" y="60" style="font-size:10.5px">tile</text>
  <rect x="238" y="68" width="96" height="56" style="fill:#e7ead8;stroke:#3c6b39;stroke-width:1.2" />
  <g stroke="#3c6b39" stroke-opacity="0.35" stroke-width="0.7">
    <line x1="238" y1="82" x2="334" y2="82" /><line x1="238" y1="96" x2="334" y2="96" /><line x1="238" y1="110" x2="334" y2="110" />
    <line x1="262" y1="68" x2="262" y2="124" /><line x1="286" y1="68" x2="286" y2="124" /><line x1="310" y1="68" x2="310" y2="124" />
  </g>
  <circle cx="238" cy="124" r="3.5" class="d-hot-fill" />
  <text class="d-sub" x="222" y="137" style="font-size:8.5px">16 &#215; 16 = <tspan style="font-weight:700">512 B</tspan></text>
  <text class="d-sub" x="222" y="149" style="font-size:8.5px">one SD block</text>

  <line class="d-flow" x1="362" y1="92" x2="388" y2="92" marker-end="url(#aTF)" />

  <!-- cell -->
  <rect class="d-panel" x="392" y="40" width="150" height="118" rx="9" />
  <text class="d-label" x="406" y="60" style="font-size:10.5px">terrain cell</text>
  <rect x="422" y="68" width="88" height="56" style="fill:#dfe6e0;stroke:#3c6b39;stroke-width:1.4" />
  <g stroke="#3c6b39" stroke-opacity="0.28" stroke-width="0.6">
    <line x1="422" y1="82" x2="510" y2="82" /><line x1="422" y1="96" x2="510" y2="96" /><line x1="422" y1="110" x2="510" y2="110" />
    <line x1="444" y1="68" x2="444" y2="124" /><line x1="466" y1="68" x2="466" y2="124" /><line x1="488" y1="68" x2="488" y2="124" />
  </g>
  <rect x="422" y="68" width="22" height="14" style="fill:#cf6a2a;fill-opacity:0.35;stroke:#cf6a2a;stroke-width:1" />
  <text class="d-sub" x="406" y="137" style="font-size:8.5px">2&#185;&#8313; &#181;deg &#183; 64&#178; tiles</text>
  <text class="d-sub" x="406" y="149" style="font-size:8.5px">1024&#178; samples &#183; 2 MiB</text>

  <!-- half-open note -->
  <rect class="d-panel-2" x="560" y="40" width="136" height="118" rx="9" />
  <text class="d-tag" x="574" y="60">half-open</text>
  <text class="d-sub" x="574" y="80" style="font-size:9px">a cell owns its</text>
  <text class="d-sub" x="574" y="93" style="font-size:9px">minimum edges,</text>
  <text class="d-sub" x="574" y="106" style="font-size:9px">not its maximum</text>
  <text class="d-sub" x="574" y="126" style="font-size:8.5px;fill:#a9501c">no sample stored twice</text>

  <!-- container ribbon -->
  <text class="d-sub" x="24" y="190" style="font-size:9px;fill:#6b7758">the container — one format for a published cell and a map's spliced raster</text>
  <g stroke="#20301d" stroke-width="1">
    <rect x="24" y="200" width="86" height="40" class="d-forest" />
    <rect x="110" y="200" width="170" height="40" class="d-amber" />
    <rect x="280" y="200" width="138" height="40" class="d-water" />
    <rect x="418" y="200" width="138" height="40" class="d-muted" />
    <rect x="556" y="200" width="140" height="40" class="d-water" />
  </g>
  <text class="d-label" x="67" y="219" text-anchor="middle" style="fill:#fff;font-size:10px">header</text>
  <text class="d-sub" x="67" y="233" text-anchor="middle" style="fill:#fff;font-size:8.5px">32 B</text>
  <text class="d-label" x="195" y="219" text-anchor="middle" style="font-size:10px">offset directory</text>
  <text class="d-sub" x="195" y="233" text-anchor="middle" style="font-size:8.5px">rows &#215; cols &#215; u32</text>
  <text class="d-label" x="349" y="219" text-anchor="middle" style="fill:#fff;font-size:10px">cell block</text>
  <text class="d-sub" x="349" y="233" text-anchor="middle" style="fill:#dfe6e0;font-size:8.5px">T&#178; &#215; 512 B</text>
  <text class="d-label" x="487" y="219" text-anchor="middle" style="font-size:10px">cell block</text>
  <text class="d-label" x="626" y="219" text-anchor="middle" style="fill:#fff;font-size:10px">cell block</text>

  <text class="d-sub" x="24" y="262" style="font-size:9px">slot = (ci &#8722; CellMinI) &#215; CellCols + (cj &#8722; CellMinJ) &#8594; a byte offset, or <tspan style="font-weight:700">0</tspan> = absent</text>
  <text class="d-sub" x="24" y="278" style="font-size:9px;fill:#a9501c">no bbox field — the cell rectangle <tspan style="font-style:italic">is</tspan> the bounding box</text>

  <rect class="d-panel-2" x="24" y="296" width="672" height="32" rx="8" />
  <text class="d-sub" x="360" y="316" text-anchor="middle" style="font-size:9.5px">a <tspan style="font-weight:700">cell</tspan> is a 1 &#215; 1 container; a map's <tspan style="font-weight:700">spliced region</tspan> covers a selection — one format, no branch</text>
</svg>
<figcaption>One tile is 512 bytes. A zero directory offset means that the terrain cell is absent.</figcaption>
</figure>

A tile contains 16 × 16 samples and is exactly 512 bytes.
Tiles and cells use row-major order.
Rows increase latitude.
The first sample is at the minimum corner.

The 32-byte header defines the lattice and a rectangular cell directory.
Each nonzero directory entry addresses one cell block.
The device applies bilinear interpolation.
If one required corner is `NODATA`, the sample result is unavailable.

Published terrain cells use the `.obcd` extension.
An assembled OBCM map contains one OBCT container in its terrain region.
The map reader gives that region to the OBCT reader as a byte-source window.

## OBCW — provider-neutral weather

OBCW v1 contains a 112-byte header, 24 hourly records, rain-frame descriptors, tile directories, and tile data.
The header contains generation, request, time, bounds, offsets, and a whole-object CRC-32.
Hourly record `i` describes the interval that starts `i` hours after `valid_from`.

Rain frames use their actual UTC validity times.
Each rain tile contains 16 × 16 four-bit intensity values.
The format supports canonical raw and run-length encodings.
Missing precipitation is different from dry precipitation.

[`obc-weather`](src:firmware/obc-weather) validates the complete object.
It decodes one tile into caller-owned memory.
See [`OBCW_Spec.md`](src:specs/OBCW_Spec.md) for byte fields and rejection rules.

### Upstream of the phone: OBCG

OBCG v1 is the published precipitation-grid format.
One object contains one frame for one geographic shard.
The device does not read OBCG.

An OBCG object contains:

- A self-checked 128-byte header
- A paged tile directory with page CRCs
- Tile payloads with individual CRCs
- A whole-object CRC

A range client reads only directory pages and tiles that intersect its corridor.
OBCG supports raw, run-length, and DEFLATE tile codecs.
The companion decodes OBCG and writes device-safe OBCW tiles.
The device does not include a DEFLATE decoder.

The service manifest selects current objects and states freshness, geometry, presence, and attribution.
See [`OBCG_Spec.md`](src:specs/OBCG_Spec.md).

## Streaming: resident vs on-demand

All device readers use [`ByteSource`](src:firmware/obc-formats/src/io.rs):

```rust
pub trait ByteSource {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error>;
    fn len(&self) -> u64;
}
```

The device implementation reads a flat-store object.
Host implementations can read memory or files.
The `u64` offset supports large OBCM objects.

<figure class="fig">
<svg viewBox="0 0 720 270" role="img" aria-label="The map reader keeps small tables in memory. It streams map indexes and geometry through bounded caches. The route reader keeps its small index and streams route chunks.">
  <defs>
    <marker id="aF4" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">What stays in RAM, what streams from the card</text>

  <!-- file on card -->
  <rect class="d-panel-2" x="36" y="48" width="128" height="180" rx="10" />
  <text class="d-tag" x="52" y="68">OBCM object</text>
  <rect x="48" y="78" width="104" height="16" class="d-forest" /><text class="d-sub" x="100" y="90" text-anchor="middle" style="fill:#fff;font-size:8.5px">header·styles·LOD</text>
  <rect x="48" y="96" width="104" height="56" class="d-muted" /><text class="d-sub" x="100" y="128" text-anchor="middle" style="font-size:9px">quadtree index</text>
  <rect x="48" y="154" width="104" height="66" class="d-water" /><text class="d-sub" x="100" y="190" text-anchor="middle" style="fill:#fff;font-size:9px">geometry chunks</text>
  <text class="d-sub" x="100" y="244" text-anchor="middle" style="font-size:9px">megabytes ≫ RAM</text>

  <!-- arrows -->
  <line class="d-flow" x1="170" y1="86"  x2="318" y2="86"  marker-end="url(#aF4)" />
  <line class="d-flow" x1="170" y1="170" x2="318" y2="170" marker-end="url(#aF4)" />

  <!-- resident box -->
  <rect class="d-panel" x="324" y="60" width="360" height="48" rx="10" />
  <text class="d-label" x="340" y="80">resident — read once at open</text>
  <text class="d-sub" x="340" y="98">header · style table · LOD table  (a few hundred bytes)</text>

  <!-- streamed box -->
  <rect class="d-panel" x="324" y="124" width="360" height="64" rx="10" />
  <text class="d-label" x="340" y="144">streamed — pulled on demand</text>
  <text class="d-sub" x="340" y="162">index nodes → 512 B blocks + bounded leaf lists</text>
  <text class="d-sub" x="340" y="178">geometry chunks → five 4 KiB working slots</text>

  <!-- route contrast -->
  <rect class="d-panel-2" x="324" y="200" width="360" height="40" rx="10" />
  <text class="d-sub" x="340" y="218" style="font-size:10px"><tspan style="fill:#a9501c">OBCR:</tspan> header + the whole (small, flat) index resident;</text>
  <text class="d-sub" x="340" y="232" style="font-size:10px">only geometry chunks stream. The list is cheap to keep.</text>
</svg>
<figcaption>Large map, route, terrain, and weather objects do not have to fit in RAM.</figcaption>
</figure>

OBCM keeps its header, styles, and LOD table in memory.
It streams quadtree blocks and geometry chunks.
OBCR keeps its small flat index in memory and streams geometry.
OBCT keeps its header in memory and uses a four-tile cache.
OBCW validates and decodes in bounded windows.

## The catalog — the map builder's source of truth

OBCC schema 2 is the map-builder catalog.
The device does not read it.
The root document publishes:

- One map schema
- Presentation-only skins
- Named region selections
- One cell index for each band
- Optional terrain metadata and index
- Source and license information

The root pins referenced objects by byte length and SHA-256.
Published object keys also contain the digest.
A consumer verifies each object before use.

The schema controls geometry, LODs, style identifiers, routing, and chunk size.
A skin controls colors, weights, line style, z-index, priority, and marker color.
A skin change does not require a cell rebake.
A schema or OBCM-version change does require a consistent cell-store rebake.

Terrain cells have a separate revision.
The catalog also states which terrain revision supplied navigation ascent values.
See [`OBCC_Spec.md`](src:specs/OBCC_Spec.md).

## Cells and assemblies

OBCA defines the global cell grid and the assembly rules.
Cells are power-of-two microdegree squares on one global origin.
Each schema band assigns a cell size and a subset of map content.

### The alignment trick

<figure class="fig">
<svg viewBox="0 0 720 268" role="img" aria-label="Power-of-two map cells align with an assembled map quadtree. The assembler copies geometry chunks and rebuilds global sections.">
  <defs>
    <marker id="aCA" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Subdivision lands on cell boundaries</text>

  <!-- left: the lattice -->
  <text class="d-label" x="30" y="52">the catalog</text>
  <text class="d-sub" x="30" y="67">one cell = one baked .obcm</text>
  <g>
    <rect class="d-panel-2" x="30" y="80" width="72" height="72" />
    <rect class="d-panel-2" x="102" y="80" width="72" height="72" />
    <rect class="d-panel-2" x="30" y="152" width="72" height="72" />
    <rect class="d-panel-2" x="102" y="152" width="72" height="72" />
    <rect class="d-panel-2" x="174" y="80" width="72" height="72" style="fill:#f2efe2" />
    <rect class="d-panel-2" x="174" y="152" width="72" height="72" style="fill:#f2efe2" />
    <!-- the selection -->
    <path class="d-hot" d="M46 96 L152 96 L152 138 L128 138 L128 200 L46 200 Z" stroke-dasharray="5 3" />
    <text class="d-sub" x="38" y="118" style="font-size:9px">18/1204/1052</text>
    <text class="d-sub" x="110" y="118" style="font-size:9px">…/1053</text>
    <text class="d-sub" x="182" y="118" style="font-size:9px;fill:#b9b09a">not selected</text>
  </g>
  <text class="d-sub" x="30" y="243" style="font-size:9px;fill:#a9501c">selection (dashed) → the cells it touches</text>

  <!-- arrow -->
  <line class="d-flow" x1="256" y1="150" x2="360" y2="150" marker-end="url(#aCA)" />
  <text class="d-sub" x="308" y="132" text-anchor="middle" style="fill:#a9501c;font-size:9.5px">chunk bytes</text>
  <text class="d-sub" x="308" y="145" text-anchor="middle" style="fill:#a9501c;font-size:9.5px">copied verbatim</text>
  <text class="d-sub" x="308" y="172" text-anchor="middle" style="font-size:9px">no decode</text>
  <text class="d-sub" x="308" y="184" text-anchor="middle" style="font-size:9px">no GEOS</text>

  <!-- right: the assembled tree -->
  <text class="d-label" x="392" y="52">the assembly</text>
  <text class="d-sub" x="392" y="67">bbox = grid-aligned 2ⁿ square</text>
  <circle cx="470" cy="92" r="11" class="d-forest" />
  <text class="d-sub" x="490" y="96" style="font-size:9px">root — rebuilt</text>
  <line class="d-flow" x1="463" y1="101" x2="432" y2="126" />
  <line class="d-flow" x1="477" y1="101" x2="508" y2="126" />
  <circle cx="426" cy="136" r="10" class="d-forest" />
  <circle cx="514" cy="136" r="10" class="d-forest" />
  <line class="d-flow" x1="420" y1="145" x2="400" y2="170" />
  <line class="d-flow" x1="432" y1="145" x2="452" y2="170" />
  <line class="d-flow" x1="508" y1="145" x2="488" y2="170" />
  <line class="d-flow" x1="520" y1="145" x2="540" y2="170" />
  <rect class="d-panel" x="376" y="176" width="48" height="30" rx="5" />
  <rect class="d-panel" x="432" y="176" width="48" height="30" rx="5" />
  <rect class="d-panel" x="488" y="176" width="48" height="30" rx="5" />
  <rect class="d-panel-2" x="544" y="176" width="48" height="30" rx="5" style="fill:#f2efe2" />
  <text class="d-sub" x="400" y="196" text-anchor="middle" style="font-size:9px">cell</text>
  <text class="d-sub" x="456" y="196" text-anchor="middle" style="font-size:9px">cell</text>
  <text class="d-sub" x="512" y="196" text-anchor="middle" style="font-size:9px">cell</text>
  <text class="d-sub" x="568" y="196" text-anchor="middle" style="font-size:9px;fill:#b9b09a">empty</text>
  <path class="d-hot" d="M370 170 L598 170" stroke-dasharray="4 3" />
  <text class="d-sub" x="604" y="174" style="font-size:9px;fill:#a9501c">cell depth</text>
  <text class="d-sub" x="392" y="228" style="font-size:9.5px">rebuilt: header · style table · upper index</text>
  <text class="d-sub" x="392" y="243" style="font-size:9.5px">rebuilt: POIs + hours · the navigation graph</text>
</svg>
<figcaption>Exact grid alignment preserves leaf-relative feature anchors. The assembler copies geometry bytes without decoding them.</figcaption>
</figure>

Exact alignment lets the assembler copy geometry chunks without decoding them.
The assembler rebuilds the header, tables, POIs, opening-hours pool, and navigation graph.
It also inserts the selected OBCT terrain container.
Routing seam nodes merge only when their coordinates are equal.

### Schema and skin

All cells in one assembly use the same schema revision and OBCM version.
The assembler replaces cell presentation records with the selected skin.
It does not change geometry.

### One map, one file

OBCM v14 uses scaled offsets and stores terrain in the map.
The assembler produces one OBCM object.
It does not produce map shards or a set manifest.

### Browser assembly

[`obcm-assemble`](src:host/obcm-assemble) is the shared native assembly engine.
[`obc-web-assemble`](src:apps/obc-web-assemble) is its WebAssembly interface.
The browser can stream cells, scratch data, and output through origin-private storage.
The assembler verifies the completed file through the production readers.

See [`OBCA_Spec.md`](src:specs/OBCA_Spec.md) for the grid, seam, and verification rules.

## Source index

- Format constants and byte I/O: [`obc-formats`](src:firmware/obc-formats)
- OBCM reader: [`obc-reader`](src:firmware/obc-reader)
- OBCR reader, converter, and router: [`obc-route`](src:firmware/obc-route)
- OBCT reader and sampler: [`obc-elevation`](src:firmware/obc-elevation)
- OBCW reader: [`obc-weather`](src:firmware/obc-weather)
- OBCM packer: [`obc-pack`](src:host/obc-pack)
- Terrain baker: [`obc-dem`](src:host/obc-dem)
- Weather-grid baker: [`obc-wx-bake`](src:host/obc-wx-bake)
- Map assembler: [`obcm-assemble`](src:host/obcm-assemble)
- Catalog and assembly specifications: [`OBCC_Spec.md`](src:specs/OBCC_Spec.md) and [`OBCA_Spec.md`](src:specs/OBCA_Spec.md)
