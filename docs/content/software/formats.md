---
title: Data formats
description: The binary map, route, ride, terrain, catalog, and map-assembly formats.
copy: ai
---

# Data formats

The device reads binary objects straight from storage through a random-access byte interface, and
parses no text format while it rides.

The files in [`specs/`](src:specs) are the normative contracts and define every byte. This page
says what each format is for and why it has that shape.
[`obc-formats`](src:firmware/obc-formats) holds the shared constants and byte primitives.

| Format | Use | Main consumer |
| --- | --- | --- |
| OBCM | Map, POIs, navigation, terrain, and article collections | Device |
| OBCR | Route geometry, statistics, and waypoints | Device |
| Ride object | Recorded samples and summary | Device and companion |
| OBCT | Terrain height raster | Device and map tools |
| OBCC | Map-builder catalog | Website and desktop app |
| OBCA | Cell and assembly rules | Map tools |

Values are little-endian and coordinates are signed microdegrees. A reader uses checked arithmetic
and refuses an unsupported version at the header, not halfway through a frame.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
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
  <text class="d-sub" x="234" y="207" text-anchor="middle" style="fill:#a9501c;font-size:12px">device · sim · browser</text>
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
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>OBCM and OBCR use the same byte and streaming conventions.</figcaption>
</figure>

## OBCM — the map

One OBCM object holds everything the device draws and routes on, so a rider installs one file.
Offsets are stored in scaled units, so a 32-bit offset still reaches the end of a large map.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 940px">
<svg viewBox="0 0 940 320" role="img" aria-label="A file ribbon shows the header, styles, LOD table, LOD regions, POIs and hours, navigation, optional landmarks, optional peak articles, and optional terrain. LOD 0 expands into its quadtree, chunk offsets, and geometry chunks.">
<defs><marker id="r9arrow" viewBox="0 0 10 10" refX="10" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker></defs>
<text class="d-tag" x="20" y="26" text-anchor="start">OBCM · follow the file, then open one LOD</text>
<rect x="20" y="65" width="60" height="55" fill="#d6cda8" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-label" x="50.0" y="87" text-anchor="middle">Header</text>
<text class="d-sub" x="50.0" y="107" text-anchor="middle">65 B</text>
<rect x="80" y="65" width="70" height="55" fill="#e3ad33" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-label" x="115.0" y="87" text-anchor="middle">Styles</text>
<text class="d-sub" x="115.0" y="107" text-anchor="middle">global</text>
<rect x="150" y="65" width="80" height="55" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-label" x="190.0" y="87" text-anchor="middle">LOD table</text>
<text class="d-sub" x="190.0" y="107" text-anchor="middle">N × 18 B</text>
<rect x="230" y="65" width="75" height="55" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-label" x="267.5" y="87" text-anchor="middle">LOD 0</text>
<text class="d-sub" x="267.5" y="107" text-anchor="middle">coarsest</text>
<rect x="305" y="65" width="60" height="55" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-label" x="335.0" y="87" text-anchor="middle">…</text>
<text class="d-sub" x="335.0" y="107" text-anchor="middle"></text>
<rect x="365" y="65" width="80" height="55" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-label" x="405.0" y="87" text-anchor="middle">LOD N−1</text>
<text class="d-sub" x="405.0" y="107" text-anchor="middle">finest</text>
<rect x="445" y="65" width="70" height="55" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-label" x="480.0" y="87" text-anchor="middle">POIs</text>
<text class="d-sub" x="480.0" y="107" text-anchor="middle">+ hours</text>
<rect x="515" y="65" width="70" height="55" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-label" x="550.0" y="87" text-anchor="middle">Nav</text>
<text class="d-sub" x="550.0" y="107" text-anchor="middle">graph</text>
<rect x="585" y="65" width="115" height="55" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-label" x="642.5" y="87" text-anchor="middle">Landmarks</text>
<text class="d-sub" x="642.5" y="107" text-anchor="middle">optional</text>
<rect x="700" y="65" width="90" height="55" fill="#e3ad33" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-label" x="745" y="87" text-anchor="middle">Peak articles</text>
<text class="d-sub" x="745" y="107" text-anchor="middle">optional</text>
<rect x="790" y="65" width="115" height="55" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-label" x="847.5" y="87" text-anchor="middle">Terrain</text>
<text class="d-sub" x="847.5" y="107" text-anchor="middle">OBCT · optional</text>
<text class="d-sub" x="20" y="51" text-anchor="start">File order; region widths depend on the data.</text>
<path d="M230 120 L110 198" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M305 120 L690 198" fill="none" stroke="#9aa884" stroke-width="1.3" />
<text class="d-title" x="240" y="177" text-anchor="start">One LOD, expanded</text>
<rect x="110" y="199" width="160" height="57" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-label" x="190.0" y="222" text-anchor="middle">Quadtree index</text>
<text class="d-sub" x="190.0" y="242" text-anchor="middle">flat u32 nodes</text>
<rect x="270" y="199" width="160" height="57" fill="#e3ad33" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-label" x="350.0" y="222" text-anchor="middle">Chunk offsets</text>
<text class="d-sub" x="350.0" y="242" text-anchor="middle">one scaled u32 each</text>
<rect x="430" y="199" width="130" height="57" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-label" x="495.0" y="222" text-anchor="middle">Chunk 0</text>
<text class="d-sub" x="495.0" y="242" text-anchor="middle">geometry records</text>
<rect x="560" y="199" width="130" height="57" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-label" x="625.0" y="222" text-anchor="middle">… Chunk k</text>
<text class="d-sub" x="625.0" y="242" text-anchor="middle">aligned data</text>
<path d="M350 256 V282 H495 V258" fill="none" stroke="#3c6b39" stroke-width="1.5" marker-end="url(#r9arrow)"/>
<text class="d-sub" x="110" y="300" text-anchor="start">The offset table locates each geometry chunk. Global addresses use scaled units.</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Each LOD repeats the same index-and-chunks structure. The ribbon shows file order, not relative region sizes.</figcaption>
</figure>

The map is a pyramid of pre-simplified levels of detail. Each level is independent and states the
coarsest scale it serves, so the renderer opens one level and reads nothing else.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 502" role="img" aria-label="Three byte rulers show all 65 bytes with equal byte widths: magic, version, four bounds, style offset, LOD count and table offset, marker color, POI and navigation offsets, scale, terrain, landmark and peak offsets and lengths.">
<defs><marker id="r10arrow" viewBox="0 0 10 10" refX="10" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker></defs>
<text class="d-tag" x="20" y="26" text-anchor="start">OBCM header · every byte in order and to scale</text>
<text class="d-sub" x="20" y="53" text-anchor="start">65 bytes · three consecutive rows · equal width per byte</text>
<rect x="40" y="102" width="96" height="36" fill="#d6cda8" stroke="#3c6b39" stroke-width="1.2" />
<path d="M64 102 L64 138" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M88 102 L88 138" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M112 102 L112 138" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<text class="d-sub" x="88.0" y="125" text-anchor="middle">OBCM</text>
<text class="d-sub" x="88.0" y="157" text-anchor="middle">0–3</text>
<rect x="136" y="102" width="24" height="36" fill="#e3ad33" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="148.0" y="125" text-anchor="middle">17</text>
<text class="d-sub" x="148.0" y="157" text-anchor="middle">4</text>
<rect x="160" y="102" width="96" height="36" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<path d="M184 102 L184 138" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M208 102 L208 138" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M232 102 L232 138" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<text class="d-sub" x="208.0" y="125" text-anchor="middle">Min lat</text>
<text class="d-sub" x="208.0" y="157" text-anchor="middle">5–8</text>
<rect x="256" y="102" width="96" height="36" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<path d="M280 102 L280 138" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M304 102 L304 138" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M328 102 L328 138" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<text class="d-sub" x="304.0" y="125" text-anchor="middle">Min lon</text>
<text class="d-sub" x="304.0" y="157" text-anchor="middle">9–12</text>
<rect x="352" y="102" width="96" height="36" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<path d="M376 102 L376 138" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M400 102 L400 138" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M424 102 L424 138" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<text class="d-sub" x="400.0" y="125" text-anchor="middle">Max lat</text>
<text class="d-sub" x="400.0" y="157" text-anchor="middle">13–16</text>
<rect x="448" y="102" width="96" height="36" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<path d="M472 102 L472 138" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M496 102 L496 138" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M520 102 L520 138" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<text class="d-sub" x="496.0" y="125" text-anchor="middle">Max lon</text>
<text class="d-sub" x="496.0" y="157" text-anchor="middle">17–20</text>
<rect x="544" y="102" width="96" height="36" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<path d="M568 102 L568 138" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M592 102 L592 138" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M616 102 L616 138" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<text class="d-sub" x="592.0" y="125" text-anchor="middle">Style off</text>
<text class="d-sub" x="592.0" y="157" text-anchor="middle">21–24</text>
<text class="d-sub" x="88" y="84" text-anchor="middle">Magic</text>
<text class="d-sub" x="164" y="70" text-anchor="middle">Version</text>
<path d="M164 75 H148 V100" fill="none" stroke="#3c6b39" stroke-width="1.5" />
<text class="d-sub" x="352" y="84" text-anchor="middle">Bounds · four i32 coordinates in µdeg</text>
<text class="d-sub" x="590" y="84" text-anchor="middle">u32 offset</text>
<rect x="40" y="242" width="24" height="36" fill="#e3ad33" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="52.0" y="265" text-anchor="middle">N</text>
<text class="d-sub" x="52.0" y="297" text-anchor="middle">25</text>
<rect x="64" y="242" width="96" height="36" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<path d="M88 242 L88 278" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M112 242 L112 278" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M136 242 L136 278" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<text class="d-sub" x="112.0" y="265" text-anchor="middle">LOD table off</text>
<text class="d-sub" x="112.0" y="297" text-anchor="middle">26–29</text>
<rect x="160" y="242" width="48" height="36" fill="#e3ad33" stroke="#3c6b39" stroke-width="1.2" />
<path d="M184 242 L184 278" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<text class="d-sub" x="184.0" y="265" text-anchor="middle">RGB</text>
<text class="d-sub" x="184.0" y="297" text-anchor="middle">30–31</text>
<rect x="208" y="242" width="96" height="36" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" />
<path d="M232 242 L232 278" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M256 242 L256 278" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M280 242 L280 278" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<text class="d-sub" x="256.0" y="265" text-anchor="middle">POI off</text>
<text class="d-sub" x="256.0" y="297" text-anchor="middle">32–35</text>
<rect x="304" y="242" width="96" height="36" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<path d="M328 242 L328 278" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M352 242 L352 278" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M376 242 L376 278" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<text class="d-sub" x="352.0" y="265" text-anchor="middle">Nav off</text>
<text class="d-sub" x="352.0" y="297" text-anchor="middle">36–39</text>
<rect x="400" y="242" width="24" height="36" fill="#e3ad33" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="412.0" y="265" text-anchor="middle">s</text>
<text class="d-sub" x="412.0" y="297" text-anchor="middle">40</text>
<rect x="424" y="242" width="96" height="36" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<path d="M448 242 L448 278" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M472 242 L472 278" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M496 242 L496 278" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<text class="d-sub" x="472.0" y="265" text-anchor="middle">Terrain off</text>
<text class="d-sub" x="472.0" y="297" text-anchor="middle">41–44</text>
<rect x="520" y="242" width="96" height="36" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<path d="M544 242 L544 278" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M568 242 L568 278" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M592 242 L592 278" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<text class="d-sub" x="568.0" y="265" text-anchor="middle">Terrain len</text>
<text class="d-sub" x="568.0" y="297" text-anchor="middle">45–48</text>
<text class="d-sub" x="40" y="197" text-anchor="start">N: LOD count</text>
<path d="M52 203 V240" fill="none" stroke="#3c6b39" stroke-width="1.5" />
<text class="d-sub" x="184" y="226" text-anchor="middle">RGB565</text>
<text class="d-sub" x="412" y="207" text-anchor="middle">s: offset scale</text>
<path d="M412 213 V240" fill="none" stroke="#3c6b39" stroke-width="1.5" />
<text class="d-sub" x="40" y="455" text-anchor="start">Rows: bytes 0–24, 25–48, then 49–64. All multi-byte values are little-endian.</text>
<text class="d-sub" x="40" y="477" text-anchor="start">Section address = stored offset × 2ˢ. Region lengths use the same units; writers set s = 4.</text>
<rect x="40" y="362" width="96" height="36" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" />
<path d="M64 362 V398" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M88 362 V398" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M112 362 V398" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<text class="d-sub" x="88" y="385" text-anchor="middle" style="font-size:12px">Landmark off</text>
<text class="d-sub" x="88" y="417" text-anchor="middle">49–52</text>
<rect x="136" y="362" width="96" height="36" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" />
<path d="M160 362 V398" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M184 362 V398" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M208 362 V398" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<text class="d-sub" x="184" y="385" text-anchor="middle" style="font-size:12px">Landmark len</text>
<text class="d-sub" x="184" y="417" text-anchor="middle">53–56</text>
<rect x="232" y="362" width="96" height="36" fill="#e3ad33" stroke="#3c6b39" stroke-width="1.2" />
<path d="M256 362 V398" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M280 362 V398" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M304 362 V398" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<text class="d-sub" x="280" y="385" text-anchor="middle">Peak off</text>
<text class="d-sub" x="280" y="417" text-anchor="middle">57–60</text>
<rect x="328" y="362" width="96" height="36" fill="#e3ad33" stroke="#3c6b39" stroke-width="1.2" />
<path d="M352 362 V398" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M376 362 V398" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<path d="M400 362 V398" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".18"/>
<text class="d-sub" x="376" y="385" text-anchor="middle">Peak len</text>
<text class="d-sub" x="376" y="417" text-anchor="middle">61–64</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Field widths show their actual byte sizes. Small fields have leader labels. The second row continues directly after byte 24.</figcaption>
</figure>

The header addresses the global sections. An absent section has a zero offset and length, which is
how a map without terrain, landmarks, or peak articles says so. The style table applies to every
level. [OBCM](src:specs/OBCM_Spec.md) defines each field.

### The quadtree index

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
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
  <text class="d-sub" x="65" y="92" text-anchor="middle" style="fill:#a9501c;font-size:12px">branch flag</text>

  <!-- interpretations -->
  <g>
    <rect x="56" y="110" width="14" height="14" rx="3" class="d-hot-fill" />
    <text class="d-label" x="80" y="121" style="font-size:12px">high bit set</text>
    <text class="d-sub" x="200" y="121">branch → low 31 bits = index of the first child (NW)</text>

    <rect x="56" y="136" width="14" height="14" rx="3" class="d-muted" />
    <text class="d-label" x="80" y="147" style="font-size:12px">0x7FFF_FFFF</text>
    <text class="d-sub" x="200" y="147">empty leaf → nothing to draw here</text>

    <rect x="56" y="162" width="14" height="14" rx="3" class="d-forest" />
    <text class="d-label" x="80" y="173" style="font-size:12px">anything else</text>
    <text class="d-sub" x="200" y="173">leaf → the value is a chunk id into this LOD's chunks</text>
  </g>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Branches point to four consecutive children. Readers derive child bounds from the parent bounds.</figcaption>
</figure>

Each level indexes its geometry with a quadtree of 32-bit words. A reader derives a child's bounds
from its parent's, so no node stores a box and one comparison prunes a whole branch.

### Features: an anchor, then deltas

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
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
  <text class="d-sub" x="60" y="150" style="fill:#a9501c;font-size:12px">anchor</text>
  <text class="d-sub" x="44" y="208" style="font-size:12px">(47 123 456, 8 654 321)</text>
  <!-- small delta hints -->
  <text class="d-sub" x="104" y="132" style="font-size:12px">+Δ</text>
  <text class="d-sub" x="158" y="92"  style="font-size:12px">+Δ</text>
  <text class="d-sub" x="226" y="126" style="font-size:12px">+Δ</text>

  <!-- arrow -->
  <line class="d-flow" x1="332" y1="136" x2="392" y2="136" marker-end="url(#aF3)" />
  <text class="d-sub" x="362" y="126" text-anchor="middle" style="font-size:12px">encode</text>

  <!-- RIGHT: encoded -->
  <rect class="d-panel" x="400" y="44" width="296" height="184" rx="12" />
  <text class="d-tag" x="416" y="66">encoded</text>
  <!-- anchor cell -->
  <rect x="416" y="78" width="120" height="30" rx="5" class="d-hot-fill" />
  <text class="d-sub" x="476" y="97" text-anchor="middle" style="fill:#fff;font-size:12px">anchor X,Y</text>
  <text class="d-sub" x="544" y="90" style="font-size:12px">stored vs the</text>
  <text class="d-sub" x="544" y="108" style="font-size:12px">leaf's corner</text>
  <!-- delta cells -->
  <g stroke="#3c6b39" stroke-width="1">
    <rect x="416" y="118" width="44" height="26" rx="4" class="d-muted" />
    <rect x="462" y="118" width="44" height="26" rx="4" class="d-muted" />
    <rect x="508" y="118" width="44" height="26" rx="4" class="d-muted" />
    <rect x="554" y="118" width="44" height="26" rx="4" class="d-muted" />
  </g>
  <text class="d-sub" x="438" y="135" text-anchor="middle" style="font-size:12px">Δx,Δy</text>
  <text class="d-sub" x="484" y="135" text-anchor="middle" style="font-size:12px">Δx,Δy</text>
  <text class="d-sub" x="530" y="135" text-anchor="middle" style="font-size:12px">Δx,Δy</text>
  <text class="d-sub" x="576" y="135" text-anchor="middle" style="font-size:12px">…</text>
  <!-- per-feature width choice -->
  <text class="d-sub" x="416" y="170" style="font-size:12px">every |Δ| ≤ 127  →  <tspan style="fill:#3c6b39">int8</tspan>  · 2 B / point</text>
  <text class="d-sub" x="416" y="190" style="font-size:12px">otherwise           →  <tspan style="fill:#a9501c">int16</tspan> · 4 B / point</text>
  <text class="d-sub" x="416" y="212" style="font-size:12px">chosen once per feature (flag bit 0)</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Anchor and delta encoding keeps common geometry records small.</figcaption>
</figure>

A feature stores one anchor relative to its leaf and then coordinate deltas, because map geometry
is dense and local.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 300" role="img" aria-label="A compact feature header is 7 bytes. A wide header is 12 bytes. Flags select delta width, polygon data, holes, and header width.">
  <text class="d-tag" x="20" y="24">A feature on disk — both rulers to scale, 1 byte = 40 px</text>

  <!-- compact header ruler: 7 B -->
  <text class="d-sub" x="140" y="52" text-anchor="middle" style="font-size:12px">style</text>
  <text class="d-sub" x="180" y="52" text-anchor="middle" style="font-size:12px">flags</text>
  <text class="d-sub" x="220" y="52" text-anchor="middle" style="font-size:12px">pts</text>
  <text class="d-sub" x="280" y="52" text-anchor="middle" style="font-size:12px">anchor X</text>
  <text class="d-sub" x="360" y="52" text-anchor="middle" style="font-size:12px">anchor Y</text>
  <g stroke="#20301d" stroke-width="1">
    <rect x="120" y="60" width="40" height="32" class="d-forest" />
    <rect x="160" y="60" width="40" height="32" class="d-hot-fill" />
    <rect x="200" y="60" width="40" height="32" class="d-water" />
    <rect x="240" y="60" width="80" height="32" class="d-muted" />
    <rect x="320" y="60" width="80" height="32" class="d-muted" />
  </g>
  <text class="d-tag" x="110" y="80" text-anchor="end" style="font-size:12px">compact · 7 B</text>
  <text class="d-sub" x="280" y="80" text-anchor="middle" style="font-size:12px">u16 · 2 B</text>
  <text class="d-sub" x="360" y="80" text-anchor="middle" style="font-size:12px">u16 · 2 B</text>
  <text class="d-sub" x="140" y="106" text-anchor="middle" style="font-size:12px">1 B</text>
  <text class="d-sub" x="180" y="106" text-anchor="middle" style="font-size:12px">1 B</text>
  <text class="d-sub" x="220" y="106" text-anchor="middle" style="font-size:12px">1 B</text>

  <!-- wide header ruler: 12 B, same scale, same left edge -->
  <g stroke="#20301d" stroke-width="1">
    <rect x="120" y="122" width="40"  height="32" class="d-forest" />
    <rect x="160" y="122" width="40"  height="32" class="d-hot-fill" />
    <rect x="200" y="122" width="80"  height="32" class="d-water" />
    <rect x="280" y="122" width="160" height="32" class="d-muted" />
    <rect x="440" y="122" width="160" height="32" class="d-muted" />
  </g>
  <text class="d-tag" x="110" y="142" text-anchor="end" style="font-size:12px">wide · 12 B</text>
  <text class="d-sub" x="240" y="142" text-anchor="middle" style="fill:#fff;font-size:12px">pts · 2 B</text>
  <text class="d-sub" x="360" y="142" text-anchor="middle" style="font-size:12px">anchor X · i32 · 4 B</text>
  <text class="d-sub" x="520" y="142" text-anchor="middle" style="font-size:12px">anchor Y · i32 · 4 B</text>

  <!-- flags expand: the byte that decides which ruler you are reading -->
  <line x1="180" y1="154" x2="112" y2="182" stroke="#cf6a2a" stroke-width="1.2" />
  <g>
    <rect x="60"  y="182" width="104" height="22" rx="4" class="d-panel-2" />
    <text class="d-sub" x="112" y="197" text-anchor="middle" style="font-size:12px">bit 0 · 16-bit Δ</text>
    <rect x="170" y="182" width="96"  height="22" rx="4" class="d-panel-2" />
    <text class="d-sub" x="218" y="197" text-anchor="middle" style="font-size:12px">bit 1 · polygon</text>
    <rect x="272" y="182" width="82"  height="22" rx="4" class="d-panel-2" />
    <text class="d-sub" x="313" y="197" text-anchor="middle" style="font-size:12px">bit 2 · holes</text>
    <rect x="360" y="182" width="80"  height="22" rx="4" class="d-hot-fill" />
    <text class="d-sub" x="400" y="197" text-anchor="middle" style="fill:#fff;font-size:12px">bit 3 · wide</text>
  </g>
  <text class="d-sub" x="448" y="197" style="fill:#a9501c;font-size:12px">← picks the ruler</text>

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
  <text class="d-sub" x="72"  y="263" text-anchor="middle" style="fill:#fff;font-size:12px">7 or 12 B hdr</text>
  <text class="d-sub" x="195" y="263" text-anchor="middle" style="font-size:12px">exterior deltas</text>
  <text class="d-sub" x="305" y="263" text-anchor="middle" style="fill:#3a2c10;font-size:12px">hole cnt</text>
  <text class="d-sub" x="372" y="263" text-anchor="middle" style="fill:#fff;font-size:12px">h1 pts</text>
  <text class="d-sub" x="469" y="263" text-anchor="middle" style="font-size:12px">hole 1 deltas</text>
  <text class="d-sub" x="566" y="263" text-anchor="middle" style="fill:#fff;font-size:12px">h2 pts</text>
  <text class="d-sub" x="647" y="263" text-anchor="middle" style="font-size:12px">hole 2 …</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The compact header is the common form. The wide form supports large anchors or point counts.</figcaption>
</figure>

A reader validates a whole feature before it publishes any geometry, and drops an invalid one as
one unit. Half a coastline is worse than no coastline.

### POIs: services and named summits

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 556" role="img" aria-label="A category directory selects a spatial quadtree. Its leaf addresses a 512-byte chunk with eight 64-byte records. Two byte rulers show display fields, source identity, and explicit approach metadata at the same scale. Services store hours references; summits store elevation.">
<defs><marker id="r14arrow" viewBox="0 0 10 10" refX="10" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker></defs>
<text class="d-tag" x="20" y="26" text-anchor="start">POIs · category index to packed 64-byte records</text>
<text class="d-title" x="20" y="60" text-anchor="start">Category directory</text>
<rect x="20" y="74" width="165" height="115" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<path d="M20 100 L185 100" fill="none" stroke="#9aa884" stroke-width="1.3" />
<text class="d-sub" x="30" y="92" text-anchor="start">Food</text>
<path d="M20 122 L185 122" fill="none" stroke="#9aa884" stroke-width="1.3" />
<text class="d-sub" x="30" y="114" text-anchor="start">Water</text>
<path d="M20 144 L185 144" fill="none" stroke="#9aa884" stroke-width="1.3" />
<text class="d-sub" x="30" y="136" text-anchor="start">…</text>
<path d="M20 166 L185 166" fill="none" stroke="#9aa884" stroke-width="1.3" />
<text class="d-sub" x="30" y="158" text-anchor="start">Summit (optional)</text>
<path d="M185 111 H220" fill="none" stroke="#3c6b39" stroke-width="1.5" marker-end="url(#r14arrow)"/>
<text class="d-title" x="232" y="60" text-anchor="start">Category quadtree</text>
<rect x="230" y="75" width="112" height="112" fill="#eef2df" stroke="#3c6b39" stroke-width="1.2" />
<path d="M286 75 L286 187" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M230 131 L342 131" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M314 131 L314 187" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M286 159 L342 159" fill="none" stroke="#9aa884" stroke-width="1.3" />
<rect x="314" y="159" width="28" height="28" fill="#f1cfb4" stroke="#cf6a2a" stroke-width="1.2" />
<path d="M343 173 H386" fill="none" stroke="#3c6b39" stroke-width="1.5" marker-end="url(#r14arrow)"/>
<text class="d-title" x="403" y="60" text-anchor="start">One 512-byte chunk</text>
<rect x="400.0" y="75" width="35.5" height="85" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" />
<rect x="435.5" y="75" width="35.5" height="85" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="471.0" y="75" width="35.5" height="85" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="506.5" y="75" width="35.5" height="85" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="542.0" y="75" width="35.5" height="85" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="577.5" y="75" width="35.5" height="85" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="613.0" y="75" width="35.5" height="85" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="648.5" y="75" width="35.5" height="85" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="540" y="185" text-anchor="middle">8 × 64-byte records</text>
<path d="M400 160 V201 H40 V260" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M435.5 160 V207 H688 V260" fill="none" stroke="#9aa884" stroke-width="1.3" />
<text class="d-title" x="65" y="235" text-anchor="start">Record bytes 0–35 · field widths to scale</text>
<rect x="40" y="262" width="72" height="38" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="76.0" y="286" text-anchor="middle">Latitude</text>
<text class="d-sub" x="76.0" y="319" text-anchor="middle">0–3</text>
<rect x="112" y="262" width="72" height="38" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="148.0" y="286" text-anchor="middle">Longitude</text>
<text class="d-sub" x="148.0" y="319" text-anchor="middle">4–7</text>
<rect x="184" y="262" width="18" height="38" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="193.0" y="286" text-anchor="middle">s</text>
<text class="d-sub" x="193.0" y="319" text-anchor="middle">8</text>
<rect x="202" y="262" width="18" height="38" fill="#e3ad33" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="211.0" y="286" text-anchor="middle">n</text>
<text class="d-sub" x="211.0" y="319" text-anchor="middle">9</text>
<rect x="220" y="262" width="432" height="38" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="436.0" y="286" text-anchor="middle">Name · 24 bytes</text>
<text class="d-sub" x="436.0" y="319" text-anchor="middle">10–33</text>
<rect x="652" y="262" width="36" height="38" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="670.0" y="286" text-anchor="middle">t</text>
<text class="d-sub" x="670.0" y="319" text-anchor="middle">34–35</text>
<text class="d-title" x="40" y="354">Record bytes 36–63 · same scale</text>
<rect x="40" y="376" width="144" height="38" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="112.0" y="400" text-anchor="middle">Source ID</text>
<text class="d-sub" x="112.0" y="435" text-anchor="middle">36–43</text>
<rect x="184" y="376" width="144" height="38" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="256.0" y="400" text-anchor="middle">Approach ID</text>
<text class="d-sub" x="256.0" y="435" text-anchor="middle">44–51</text>
<rect x="328" y="376" width="72" height="38" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="364.0" y="400" text-anchor="middle">Latitude</text>
<text class="d-sub" x="364.0" y="435" text-anchor="middle">52–55</text>
<rect x="400" y="376" width="72" height="38" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="436.0" y="400" text-anchor="middle">Longitude</text>
<text class="d-sub" x="436.0" y="435" text-anchor="middle">56–59</text>
<rect x="472" y="376" width="18" height="38" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="481.0" y="400" text-anchor="middle">p</text>
<text class="d-sub" x="481.0" y="435" text-anchor="middle">60</text>
<rect x="490" y="376" width="54" height="38" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="517.0" y="400" text-anchor="middle">Zero</text>
<text class="d-sub" x="517.0" y="435" text-anchor="middle">61–63</text>
<text class="d-sub" x="40" y="470">s: subtype · n: name length · t: two-byte trailer · p: profile mask</text>
<text class="d-title" x="40" y="502">Services 1–6, 8</text>
<text class="d-sub" x="270" y="502">ASCII name; trailer = HoursRef u16</text>
<text class="d-title" x="40" y="532">Summit 7</text>
<text class="d-sub" x="270" y="532">UTF-8 name; trailer = elevation i16 (m)</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The two ruler rows show the 64-byte record at one scale. Source identity and an optional mapped approach follow the display fields.</figcaption>
</figure>

Each POI category has its own index, so a search for water reads no campsite records. A record
carries its source identity and, where a source node lies on a routable way, a mapped approach.
That approach is what lets the device plan a ride to a place instead of to a coordinate beside it.
Separate categories hold named summits and settlement names. See
[OBCM section 7](src:specs/OBCM_Spec.md).

### Opening hours: a pooled weekly schedule

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 328" role="img" aria-label="A 29-byte schedule contains flags and two time intervals for each weekday. POI records reference deduplicated schedules.">
  <defs>
    <marker id="aH7" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">One 29-byte schedule blob — flags + 7 days × 2 slots</text>

  <!-- blob ruler: flags + Mon..Sun (each 2 slots of open_q/close_q) -->
  <g stroke="#20301d" stroke-width="1">
    <rect x="24" y="44" width="22" height="34" class="d-amber" />
    <rect x="46" y="44" width="88" height="34" class="d-water" />
    <rect x="134" y="44" width="88" height="34" class="d-forest" />
    <rect x="222" y="44" width="88" height="34" class="d-water" />
    <rect x="310" y="44" width="88" height="34" class="d-forest" />
    <rect x="398" y="44" width="88" height="34" class="d-water" />
    <rect x="486" y="44" width="88" height="34" class="d-forest" />
    <rect x="574" y="44" width="88" height="34" class="d-water" />
  </g>
  <text class="d-sub" x="35"  y="65" text-anchor="middle" style="fill:#000;font-size:12px">f</text>
  <text class="d-sub" x="90" y="65" text-anchor="middle" style="fill:#fff;font-size:12px">Mon</text>
  <text class="d-sub" x="178" y="65" text-anchor="middle" style="fill:#fff;font-size:12px">Tue</text>
  <text class="d-sub" x="266" y="65" text-anchor="middle" style="fill:#fff;font-size:12px">Wed</text>
  <text class="d-sub" x="354" y="65" text-anchor="middle" style="fill:#fff;font-size:12px">Thu</text>
  <text class="d-sub" x="442" y="65" text-anchor="middle" style="fill:#fff;font-size:12px">Fri</text>
  <text class="d-sub" x="530" y="65" text-anchor="middle" style="fill:#fff;font-size:12px">Sat</text>
  <text class="d-sub" x="618" y="65" text-anchor="middle" style="fill:#fff;font-size:12px">Sun</text>
  <text class="d-sub" x="35"  y="92" text-anchor="middle" style="font-size:12px">0</text>
  <text class="d-sub" x="90" y="92" text-anchor="middle" style="font-size:12px">1–4</text>
  <text class="d-sub" x="618" y="92" text-anchor="middle" style="font-size:12px">25–28</text>

  <!-- one day exploded into 2 slots × (open_q, close_q) -->
  <line x1="46"  y1="78" x2="120" y2="110" stroke="#9aa884" stroke-width="1.1" />
  <line x1="134" y1="78" x2="420" y2="110" stroke="#9aa884" stroke-width="1.1" />
  <g stroke="#20301d" stroke-width="1">
    <rect x="120" y="112" width="76" height="30" class="d-panel" />
    <rect x="196" y="112" width="76" height="30" class="d-panel" />
    <rect x="272" y="112" width="76" height="30" class="d-panel-2" />
    <rect x="348" y="112" width="76" height="30" class="d-panel-2" />
  </g>
  <text class="d-sub" x="158" y="131" text-anchor="middle" style="font-size:12px">open q</text>
  <text class="d-sub" x="234" y="131" text-anchor="middle" style="font-size:12px">close q</text>
  <text class="d-sub" x="310" y="131" text-anchor="middle" style="font-size:12px">open q</text>
  <text class="d-sub" x="386" y="131" text-anchor="middle" style="font-size:12px">close q</text>
  <text class="d-sub" x="196" y="156" text-anchor="middle" style="font-size:12px;fill:#a9501c">slot 0</text>
  <text class="d-sub" x="348" y="156" text-anchor="middle" style="font-size:12px;fill:#a9501c">slot 1</text>
  <text class="d-sub" x="470" y="126" style="font-size:12px">each byte = quarter-hours</text>
  <text class="d-sub" x="470" y="140" style="font-size:12px">from midnight, 0…96 (96 = 24:00)</text>

  <!-- dedup pool -->
  <text class="d-tag" x="20" y="192">the pool — identical schedules collapse to one blob</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="30" y="216" style="font-size:12px">POI · HoursRef 0</text>
    <text class="d-sub" x="30" y="234" style="font-size:12px">POI · HoursRef 0</text>
    <text class="d-sub" x="30" y="252" style="font-size:12px">POI · HoursRef 2</text>
    <text class="d-sub" x="30" y="270" style="font-size:12px">POI · HoursRef 0xFFFF</text>
  </g>
  <line class="d-flow" x1="180" y1="212" x2="300" y2="221" marker-end="url(#aH7)" />
  <line class="d-flow" x1="180" y1="230" x2="300" y2="223" marker-end="url(#aH7)" />
  <line class="d-flow" x1="180" y1="248" x2="300" y2="279" marker-end="url(#aH7)" />
  <text class="d-sub" x="30" y="312" style="font-size:12px;fill:#a9501c">0xFFFF = no hours (no arrow)</text>

  <!-- pool blobs -->
  <g stroke="#3c6b39" stroke-width="1.1">
    <rect x="306" y="210" width="180" height="26" class="d-water" />
    <rect x="306" y="238" width="180" height="26" class="d-muted" />
    <rect x="306" y="266" width="180" height="26" class="d-water" />
  </g>
  <text class="d-sub" x="316" y="227" style="fill:#fff;font-size:12px">blob 0 — 29 B</text>
  <text class="d-sub" x="316" y="255" style="fill:#24331c;font-size:12px">blob 1 — 29 B</text>
  <text class="d-sub" x="316" y="283" style="fill:#fff;font-size:12px">blob 2 — 29 B</text>
  <text class="d-sub" x="504" y="227" style="font-size:12px">count u16, then</text>
  <text class="d-sub" x="504" y="241" style="font-size:12px">count × 29-byte blobs;</text>
  <text class="d-sub" x="504" y="255" style="font-size:12px">blob i at</text>
  <text class="d-sub" x="504" y="269" style="font-size:12px" font-family="var(--mono)">byte_offset + 2 + i·29</text>
<text class="d-sub" x="470" y="162" text-anchor="start">f: flags · ruler widths are to scale</text></svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The packer converts opening-hours text to fixed weekly schedules. The device does not parse the source grammar.</figcaption>
</figure>

The packer converts the opening-hours grammar into fixed weekly schedules in a shared pool, so the
device never parses text and identical hours cost one copy. A rounded time or a rule the packer
cannot express sets a flag, and a flagged schedule reports Unknown.

### The navigation graph: a routable network

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 570" role="img" aria-label="The 40-byte navigation directory addresses profiles, a node quadtree with junction chunks, an edge geometry pool, and a snap-anchor index. Search reads junctions; endpoint projection and route output also use geometry. Proportional rulers show a junction record and its 17-byte neighbor fields.">
  <defs><marker id="software-formats-9" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker></defs>
  <text class="d-tag" x="20" y="26" text-anchor="start">Navigation data · four regions addressed by one directory</text>
  <rect class="d-panel" x="20" y="56" width="680" height="66" rx="8" />
  <text class="d-title" x="360" y="81" text-anchor="middle">Navigation directory · 40 bytes</text>
  <text class="d-sub" x="360" y="101" text-anchor="middle">Offsets, counts, chunk size, and profile count</text>
  <path class="d-flow" d="M360 122 L360 153" />
<path class="d-flow" d="M100 153 H625" />
  <path class="d-flow" d="M100 153 L100 183" marker-end="url(#software-formats-9)" />
  <path class="d-flow" d="M275 153 L275 183" marker-end="url(#software-formats-9)" />
  <path class="d-flow" d="M450 153 L450 183" marker-end="url(#software-formats-9)" />
  <path class="d-flow" d="M625 153 L625 183" marker-end="url(#software-formats-9)" />
  <rect class="d-panel" x="20" y="186" width="160" height="94" rx="8" />
  <text class="d-title" x="100" y="211" text-anchor="middle">Profiles</text>
  <text class="d-sub" x="100" y="231" text-anchor="middle">1–8 profiles</text>
  <text class="d-sub" x="100" y="248" text-anchor="middle">56 bytes each</text>
  <rect class="d-panel" x="195" y="186" width="160" height="94" rx="8" />
  <text class="d-title" x="275" y="211" text-anchor="middle">Node index</text>
  <text class="d-sub" x="275" y="231" text-anchor="middle">Quadtree + junctions</text>
  <text class="d-sub" x="275" y="248" text-anchor="middle">512-byte chunks</text>
  <rect class="d-panel" x="370" y="186" width="160" height="94" rx="8" />
  <text class="d-title" x="450" y="211" text-anchor="middle">Edge geometry</text>
  <text class="d-sub" x="450" y="231" text-anchor="middle">Stored road shapes</text>
  <text class="d-sub" x="450" y="248" text-anchor="middle">Exact projection</text>
  <rect class="d-panel" x="545" y="186" width="155" height="94" rx="8" />
  <text class="d-title" x="622.5" y="211" text-anchor="middle">Snap anchors</text>
  <text class="d-sub" x="622.5" y="231" text-anchor="middle">Find long edges</text>
  <text class="d-sub" x="622.5" y="248" text-anchor="middle">Near the endpoint</text>

<text class="d-title" x="20" y="324" text-anchor="start">One junction · degree 3 example · 13 + 3 × 17 = 64 bytes</text>
<rect x="40" y="341" width="130" height="48" fill="#d6cda8" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-label" x="105.0" y="361" text-anchor="middle">Header</text>
<text class="d-sub" x="105.0" y="379" text-anchor="middle">13 B</text>
<rect x="170" y="341" width="170" height="48" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-label" x="255.0" y="361" text-anchor="middle">Neighbor A</text>
<text class="d-sub" x="255.0" y="379" text-anchor="middle">17 B</text>
<rect x="340" y="341" width="170" height="48" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-label" x="425.0" y="361" text-anchor="middle">Neighbor B</text>
<text class="d-sub" x="425.0" y="379" text-anchor="middle">17 B</text>
<rect x="510" y="341" width="170" height="48" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-label" x="595.0" y="361" text-anchor="middle">Neighbor C</text>
<text class="d-sub" x="595.0" y="379" text-anchor="middle">17 B</text>
<path d="M255 389 V412 H40 V449" fill="none" stroke="#3c6b39" stroke-width="1.5" />
<text class="d-title" x="65" y="436" text-anchor="start">One 17-byte neighbor entry · fields to scale</text>
<rect x="40" y="449" width="144" height="38" fill="#d6cda8" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="112.0" y="474" text-anchor="middle">Neighbor ID</text>
<text class="d-sub" x="112.0" y="508" text-anchor="middle">0–3</text>
<rect x="184" y="449" width="72" height="38" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="220.0" y="474" text-anchor="middle">Δ lat</text>
<text class="d-sub" x="220.0" y="508" text-anchor="middle">4–5</text>
<rect x="256" y="449" width="72" height="38" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="292.0" y="474" text-anchor="middle">Δ lon</text>
<text class="d-sub" x="292.0" y="508" text-anchor="middle">6–7</text>
<rect x="328" y="449" width="144" height="38" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="400.0" y="474" text-anchor="middle">Edge ID</text>
<text class="d-sub" x="400.0" y="508" text-anchor="middle">8–11</text>
<rect x="472" y="449" width="72" height="38" fill="#e3ad33" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="508.0" y="474" text-anchor="middle">Cost m</text>
<text class="d-sub" x="508.0" y="508" text-anchor="middle">12–13</text>
<rect x="544" y="449" width="36" height="38" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="562.0" y="474" text-anchor="middle">Kind</text>
<text class="d-sub" x="562.0" y="508" text-anchor="middle">14</text>
<rect x="580" y="449" width="72" height="38" fill="#e3ad33" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="616.0" y="474" text-anchor="middle">Ascent</text>
<text class="d-sub" x="616.0" y="508" text-anchor="middle">15–16</text>
<text class="d-sub" x="20" y="543" text-anchor="start">Header: lat, lon, node ID, degree. At degree 24, the 421-byte record still fits one 512-byte chunk.</text></svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The directory addresses four regions. A junction record is 13 + 17 × degree bytes. Inline neighbor coordinates, cost, way kind, and ascent avoid another record read during relaxation.</figcaption>
</figure>

The routing network is baked into the map. A junction record repeats each neighbor's coordinate,
cost, way kind, and ascent. That costs bytes and saves reads: relaxing a junction uses the chunk
that is already open, and a search on this device is limited by storage reads.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 461" role="img" aria-label="A spatial quadtree identifies the settled junction leaf. One pinned 512-byte chunk contains the junction and inline neighbor data. The algorithm updates three adjacent graph nodes using costs, coordinates, way kind and ascent, without fetching neighbor records or edge geometry.">
<defs><marker id="r17arrow" viewBox="0 0 10 10" refX="10" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker></defs>
<text class="d-tag" x="20" y="26" text-anchor="start">One A* settle · a point lookup, one chunk, several neighbor updates</text>
<text class="d-title" x="20" y="60" text-anchor="start">1 · Locate the junction</text>
<rect x="28" y="83" width="164" height="164" fill="#eef2df" stroke="#3c6b39" stroke-width="1.2" />
<path d="M110 83 L110 247" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M28 165 L192 165" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M151 165 L151 247" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M110 206 L192 206" fill="none" stroke="#9aa884" stroke-width="1.3" />
<rect x="151" y="206" width="41" height="41" fill="#f1cfb4" stroke="#cf6a2a" stroke-width="1.2" />
<circle cx="174" cy="224" r="4" fill="#cf6a2a"/>
<text class="d-sub" x="40" y="276" text-anchor="start">Point query → one leaf</text>
<path d="M193 223 H244" fill="none" stroke="#3c6b39" stroke-width="1.5" marker-end="url(#r17arrow)"/>
<text class="d-sub" x="218" y="203" text-anchor="middle">read</text>
<text class="d-title" x="260" y="60" text-anchor="start">2 · Pin its 512-byte chunk</text>
<rect x="255" y="83" width="214" height="164" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="267" y="108" text-anchor="start">Junction: lat · lon · id · degree</text>
<text class="d-sub" x="267" y="128" text-anchor="start">Each neighbor carries:</text>
<rect x="265" y="141" width="194" height="28" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="274" y="159" text-anchor="start">A · coordinate · edge id</text>
<rect x="265" y="172" width="194" height="28" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="274" y="190" text-anchor="start">B · coordinate · edge id</text>
<rect x="265" y="203" width="194" height="28" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="274" y="221" text-anchor="start">C · coordinate · edge id</text>
<path d="M470 173 H508" fill="none" stroke="#3c6b39" stroke-width="1.5" marker-end="url(#r17arrow)"/>
<text class="d-title" x="524" y="60" text-anchor="start">3 · Relax neighbors</text>
<path d="M574 176 L547 113" fill="none" stroke="#9aa884" stroke-width="1.3" />
<circle cx="547" cy="113" r="7" fill="#3c6b39"/>
<text class="d-sub" x="559" y="117" text-anchor="start">A</text>
<path d="M574 176 L666 164" fill="none" stroke="#9aa884" stroke-width="1.3" />
<circle cx="666" cy="164" r="7" fill="#3c6b39"/>
<text class="d-sub" x="678" y="168" text-anchor="start">B</text>
<path d="M574 176 L551 237" fill="none" stroke="#9aa884" stroke-width="1.3" />
<circle cx="551" cy="237" r="7" fill="#3c6b39"/>
<text class="d-sub" x="563" y="241" text-anchor="start">C</text>
<circle cx="574" cy="176" r="8" fill="#cf6a2a"/>
<text class="d-sub" x="590" y="208" text-anchor="start">settled</text>
<text class="d-sub" x="520" y="276" text-anchor="start">No neighbor-record fetch</text>
<path d="M20 300 L700 300" fill="none" stroke="#9aa884" stroke-width="1.3" />
<text class="d-title" x="20" y="328" text-anchor="start">Use the bytes already in the chunk</text>
<text class="d-sub" x="20" y="357" text-anchor="start">g′ = g + distance × road weight + ascent × climb weight</text>
<text class="d-sub" x="20" y="382" text-anchor="start">h = distance from neighbor to goal</text>
<text class="d-sub" x="400" y="382" text-anchor="start">f = g′ + ε × h</text>
<text class="d-sub" x="20" y="416" text-anchor="start">The road profile supplies weights. Inline coordinates supply the heuristic.</text>
<text class="d-sub" x="20" y="438" text-anchor="start">Edge geometry is read for endpoint projection and final route output, not neighbor relaxation.</text>
<text class="d-sub" x="255" y="276" text-anchor="start">All entries also store cost, kind, ascent.</text></svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The junction chunk supplies the data for each relaxation. Costs include distance, road-profile weight, and directional ascent; ε weights the goal-distance heuristic.</figcaption>
</figure>

The search uses junction records alone, and reads edge geometry only to project the endpoints and
write the finished route. Long edges carry sparse lookup anchors, so a road stays discoverable far
from its junctions. See [the router seam](../architecture/#on-device-routing-the-router-seam).

## OBCR — the route

An OBCR route holds the ordered geometry, the measured statistics, the waypoints, and, for an
accepted visit, the tie back to the original route. The statistics are measured from the geometry
the file keeps, so the summary and the route agree.

Missing elevation is an explicit unknown value, because zero meters is a valid height at the coast.
An unknown point pauses ascent integration instead of adding a false climb. See the
[OBCR specification](src:specs/OBCR_Spec.md).

## Recorded rides

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 258" role="img" aria-label="A recorded ride contains 20-byte samples. Finalization appends one fixed summary footer.">
  <defs>
    <marker id="rr1" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">The 20-byte ride sample — final bytes from the first write</text>

  <!-- field names -->
  <text class="d-sub" x="106" y="56" text-anchor="middle" style="font-size:12px">lon (i32)</text>
  <text class="d-sub" x="210" y="56" text-anchor="middle" style="font-size:12px">lat (i32)</text>
  <text class="d-sub" x="288" y="56" text-anchor="middle" style="font-size:12px">ele</text>
  <text class="d-sub" x="340" y="56" text-anchor="middle" style="font-size:12px">flags</text>
  <text class="d-sub" x="418" y="56" text-anchor="middle" style="font-size:12px">t_ms (u32)</text>
  <text class="d-sub" x="483" y="56" text-anchor="middle" style="fill:#a9501c;font-size:12px">hr</text>
  <text class="d-sub" x="509" y="56" text-anchor="middle" style="fill:#a9501c;font-size:12px">cad</text>
  <text class="d-sub" x="548" y="56" text-anchor="middle" style="fill:#a9501c;font-size:12px">pwr</text>

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
  <text class="d-sub" x="340" y="85" text-anchor="middle" style="font-size:12px">bit 0</text>
  <text class="d-sub" x="418" y="85" text-anchor="middle" style="fill:#e7ead8;font-size:12px">millis</text>

  <!-- byte ranges -->
  <text class="d-sub" x="106" y="112" text-anchor="middle" style="font-size:12px">0–3</text>
  <text class="d-sub" x="210" y="112" text-anchor="middle" style="font-size:12px">4–7</text>
  <text class="d-sub" x="288" y="112" text-anchor="middle" style="font-size:12px">8–9</text>
  <text class="d-sub" x="340" y="112" text-anchor="middle" style="font-size:12px">10–11</text>
  <text class="d-sub" x="418" y="112" text-anchor="middle" style="font-size:12px">12–15</text>
  <text class="d-sub" x="483" y="112" text-anchor="middle" style="font-size:12px">16</text>
  <text class="d-sub" x="509" y="112" text-anchor="middle" style="font-size:12px">17</text>
  <text class="d-sub" x="548" y="112" text-anchor="middle" style="font-size:12px">18–19</text>
  <text class="d-sub" x="590" y="86" style="fill:#a9501c;font-size:12px">sensor tail</text>
  <text class="d-sub" x="590" y="104" style="fill:#a9501c;font-size:12px">0xFF/0xFFFF = absent</text>

  <!-- Finish append -->
  <rect class="d-panel-2" x="40" y="168" width="158" height="64" rx="10" />
  <text class="d-label" x="119" y="192" text-anchor="middle" style="font-size:12px">ride payload</text>
  <text class="d-sub" x="119" y="208" text-anchor="middle" style="font-size:12px">N × 20 B samples</text>
  <text class="d-sub" x="119" y="222" text-anchor="middle" style="font-size:12px;fill:#a9501c">written in place</text>

  <line class="d-flow" x1="198" y1="200" x2="302" y2="200" marker-end="url(#rr1)" />
  <text class="d-sub" x="250" y="190" text-anchor="middle" style="font-size:12px">Finish</text>
  <text class="d-sub" x="250" y="216" text-anchor="middle" style="font-size:12px">append only</text>

  <rect class="d-panel" x="308" y="164" width="384" height="34" rx="8" />
  <text class="d-sub" x="320" y="185" style="font-size:12px"><tspan class="d-label">144-byte footer</tspan> — totals · sensors · name · bike · trip</text>

  <rect class="d-hot" x="308" y="206" width="384" height="34" rx="8" style="fill:#f8efe4" />
  <text class="d-sub" x="320" y="227" style="font-size:12px"><tspan class="d-label" style="fill:#a9501c">one commit</tspan> — final length + CRC, RECORDING cleared</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Ride finalization does not rewrite sample data.</figcaption>
</figure>

A ride is a run of fixed-size samples with a summary footer appended at the end. Finalization does
not rewrite the samples, so a long ride closes in constant time and an interrupted recording keeps
everything written before the interruption. The footer also records the bike type and the trip day
that the ride started on, so the device and the phone group the rides of a trip without dates. The
byte contract is in [the BLE interface specification](src:specs/obc-ble-interface-spec.md).

## OBCT — the terrain raster

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 732" role="img" aria-label="A sample lattice expands into a 16 by 16 tile and a 64 by 64 tile cell. Adjacent cells show single ownership of seam samples. A container ribbon shows its directory, optional cross-cell index and cell blocks. Indexed v3 adds progressively coarser samples and conservative bounds while retaining native heights.">
<defs><marker id="r22arrow" viewBox="0 0 10 10" refX="10" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker></defs>
<text class="d-tag" x="20" y="26" text-anchor="start">Terrain storage · lattice → tile → cell → indexed container</text>
<text class="d-title" x="20" y="61" text-anchor="start">Sample lattice</text>
<text class="d-title" x="245" y="61" text-anchor="start">One tile</text>
<text class="d-title" x="494" y="61" text-anchor="start">One terrain cell</text>
<path d="M35 90 L160 90" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M35 90 L35 190" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M35 115 L160 115" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M60 90 L60 190" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M35 140 L160 140" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M85 90 L85 190" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M35 165 L160 165" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M110 90 L110 190" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M35 190 L160 190" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M135 90 L135 190" fill="none" stroke="#9aa884" stroke-width="1.3" />
<circle cx="35" cy="90" r="2" fill="#3c6b39"/>
<circle cx="35" cy="115" r="2" fill="#3c6b39"/>
<circle cx="35" cy="140" r="2" fill="#3c6b39"/>
<circle cx="35" cy="165" r="2" fill="#3c6b39"/>
<circle cx="35" cy="190" r="2" fill="#3c6b39"/>
<circle cx="60" cy="90" r="2" fill="#3c6b39"/>
<circle cx="60" cy="115" r="2" fill="#3c6b39"/>
<circle cx="60" cy="140" r="2" fill="#3c6b39"/>
<circle cx="60" cy="165" r="2" fill="#3c6b39"/>
<circle cx="60" cy="190" r="2" fill="#3c6b39"/>
<circle cx="85" cy="90" r="2" fill="#3c6b39"/>
<circle cx="85" cy="115" r="2" fill="#3c6b39"/>
<circle cx="85" cy="140" r="2" fill="#3c6b39"/>
<circle cx="85" cy="165" r="2" fill="#3c6b39"/>
<circle cx="85" cy="190" r="2" fill="#3c6b39"/>
<circle cx="110" cy="90" r="2" fill="#3c6b39"/>
<circle cx="110" cy="115" r="2" fill="#3c6b39"/>
<circle cx="110" cy="140" r="2" fill="#3c6b39"/>
<circle cx="110" cy="165" r="2" fill="#3c6b39"/>
<circle cx="110" cy="190" r="2" fill="#3c6b39"/>
<circle cx="135" cy="90" r="2" fill="#3c6b39"/>
<circle cx="135" cy="115" r="2" fill="#3c6b39"/>
<circle cx="135" cy="140" r="2" fill="#3c6b39"/>
<circle cx="135" cy="165" r="2" fill="#3c6b39"/>
<circle cx="135" cy="190" r="2" fill="#3c6b39"/>
<circle cx="160" cy="90" r="2" fill="#3c6b39"/>
<circle cx="160" cy="115" r="2" fill="#3c6b39"/>
<circle cx="160" cy="140" r="2" fill="#3c6b39"/>
<circle cx="160" cy="165" r="2" fill="#3c6b39"/>
<circle cx="160" cy="190" r="2" fill="#3c6b39"/>
<circle cx="85" cy="140" r="4" fill="#cf6a2a"/>
<text class="d-sub" x="20" y="222" text-anchor="start">2⁹ µdeg between samples</text>
<text class="d-sub" x="20" y="242" text-anchor="start">Each height: signed i16 metres</text>
<path d="M178 139 H222" fill="none" stroke="#3c6b39" stroke-width="1.5" marker-end="url(#r22arrow)"/>
<rect x="245" y="83" width="128" height="128" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<path d="M253 83 L253 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M245 91 L373 91" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M261 83 L261 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M245 99 L373 99" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M269 83 L269 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M245 107 L373 107" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M277 83 L277 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M245 115 L373 115" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M285 83 L285 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M245 123 L373 123" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M293 83 L293 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M245 131 L373 131" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M301 83 L301 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M245 139 L373 139" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M309 83 L309 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M245 147 L373 147" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M317 83 L317 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M245 155 L373 155" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M325 83 L325 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M245 163 L373 163" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M333 83 L333 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M245 171 L373 171" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M341 83 L341 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M245 179 L373 179" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M349 83 L349 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M245 187 L373 187" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M357 83 L357 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M245 195 L373 195" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M365 83 L365 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<path d="M245 203 L373 203" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".45"/>
<rect x="285" y="147" width="8" height="8" fill="#cf6a2a" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="245" y="232" text-anchor="start">16 × 16 samples = 512 B</text>
<text class="d-sub" x="245" y="252" text-anchor="start">One aligned storage read</text>
<path d="M388 139 H469" fill="none" stroke="#3c6b39" stroke-width="1.5" marker-end="url(#r22arrow)"/>
<rect x="495" y="83" width="128" height="128" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<path d="M511 83 L511 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".4"/>
<path d="M495 99 L623 99" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".4"/>
<path d="M527 83 L527 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".4"/>
<path d="M495 115 L623 115" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".4"/>
<path d="M543 83 L543 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".4"/>
<path d="M495 131 L623 131" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".4"/>
<path d="M559 83 L559 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".4"/>
<path d="M495 147 L623 147" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".4"/>
<path d="M575 83 L575 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".4"/>
<path d="M495 163 L623 163" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".4"/>
<path d="M591 83 L591 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".4"/>
<path d="M495 179 L623 179" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".4"/>
<path d="M607 83 L607 211" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".4"/>
<path d="M495 195 L623 195" fill="none" stroke="#9aa884" stroke-width="1.3" opacity=".4"/>
<rect x="495" y="83" width="2" height="2" fill="#cf6a2a" stroke="#3c6b39" stroke-width="1.2" />
<path d="M497 84 L644 98" fill="none" stroke="#3c6b39" stroke-width="1.5" />
<text class="d-sub" x="642" y="117" text-anchor="start">one tile</text>
<text class="d-sub" x="494" y="232" text-anchor="start">64 × 64 tiles · 2¹⁹ µdeg</text>
<text class="d-sub" x="494" y="252" text-anchor="start">1024² samples · 2 MiB native</text>
<text class="d-sub" x="494" y="270" text-anchor="start">Grid lines shown every 8 tiles</text>
<text class="d-title" x="20" y="308" text-anchor="start">Cell edges · each sample has one owner</text>
<rect x="24" y="331" width="110" height="75" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<rect x="134" y="331" width="110" height="75" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<path d="M134 322 L134 414" fill="none" stroke="#cf6a2a" stroke-width="1.3" />
<circle cx="134" cy="341" r="3" fill="#33575b"/>
<circle cx="134" cy="361" r="3" fill="#33575b"/>
<circle cx="134" cy="381" r="3" fill="#33575b"/>
<circle cx="134" cy="401" r="3" fill="#33575b"/>
<text class="d-sub" x="63" y="355" text-anchor="middle">west</text>
<text class="d-sub" x="189" y="355" text-anchor="middle">east</text>
<text class="d-sub" x="269" y="348" text-anchor="start">The seam belongs to the east cell.</text>
<text class="d-sub" x="269" y="371" text-anchor="start">Each cell includes its minimum edges; maximum edges belong</text>
<text class="d-sub" x="269" y="391" text-anchor="start">to the next cell. Sampling can fetch corners across that seam.</text>
<text class="d-title" x="20" y="449" text-anchor="start">Container · directory entries locate geographic cells</text>
<rect x="20" y="468" width="85" height="56" fill="#d6cda8" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="62.5" y="490" text-anchor="middle">Header</text>
<text class="d-sub" x="62.5" y="509" text-anchor="middle">32 B</text>
<rect x="105" y="468" width="155" height="56" fill="#e3ad33" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="182.5" y="490" text-anchor="middle">Cell directory</text>
<text class="d-sub" x="182.5" y="509" text-anchor="middle">rows × cols × u32</text>
<rect x="260" y="468" width="155" height="56" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="337.5" y="490" text-anchor="middle">Cross-cell index</text>
<text class="d-sub" x="337.5" y="509" text-anchor="middle">v3 · when flagged</text>
<rect x="415" y="468" width="140" height="56" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="485.0" y="490" text-anchor="middle">Cell block 0</text>
<text class="d-sub" x="485.0" y="509" text-anchor="middle">height levels</text>
<rect x="555" y="468" width="145" height="56" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="627.5" y="490" text-anchor="middle">… Cell block k</text>
<text class="d-sub" x="627.5" y="509" text-anchor="middle">height levels</text>
<text class="d-sub" x="20" y="548" text-anchor="start">A zero directory offset means absent terrain. The ribbon is not to scale.</text>
<text class="d-title" x="20" y="581" text-anchor="start">Indexed v3 cells keep the native heights, then add coarser samples and bounds.</text>
<rect x="40" y="602" width="80" height="80" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<path d="M50 602 L50 682" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M40 612 L120 612" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M60 602 L60 682" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M40 622 L120 622" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M70 602 L70 682" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M40 632 L120 632" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M80 602 L80 682" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M40 642 L120 642" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M90 602 L90 682" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M40 652 L120 652" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M100 602 L100 682" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M40 662 L120 662" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M110 602 L110 682" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M40 672 L120 672" fill="none" stroke="#9aa884" stroke-width="1.3" />
<rect x="205" y="602" width="80" height="80" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />

<rect x="355" y="602" width="80" height="80" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />

<path d="M131 634 H190" fill="none" stroke="#3c6b39" stroke-width="1.5" marker-end="url(#r22arrow)"/>
<path d="M299 634 H340" fill="none" stroke="#3c6b39" stroke-width="1.5" marker-end="url(#r22arrow)"/>
<text class="d-sub" x="470" y="617" text-anchor="start">Same area; lattice posts farther apart.</text>
<text class="d-sub" x="470" y="641" text-anchor="start">Conservative height / error bounds</text>
<text class="d-sub" x="470" y="663" text-anchor="start">let Peak View skip or merge regions.</text>
<text class="d-sub" x="20" y="710" text-anchor="start">Default lattice sizes shown above. The header stores the posting and cell size.</text>
<path d="M225 602 L225 682" fill="none" stroke="#9aa884" stroke-width="1.3" /><path d="M205 622 L285 622" fill="none" stroke="#9aa884" stroke-width="1.3" /><path d="M245 602 L245 682" fill="none" stroke="#9aa884" stroke-width="1.3" /><path d="M205 642 L285 642" fill="none" stroke="#9aa884" stroke-width="1.3" /><path d="M265 602 L265 682" fill="none" stroke="#9aa884" stroke-width="1.3" /><path d="M205 662 L285 662" fill="none" stroke="#9aa884" stroke-width="1.3" /><path d="M395 602 L395 682" fill="none" stroke="#9aa884" stroke-width="1.3" /><path d="M355 642 L435 642" fill="none" stroke="#9aa884" stroke-width="1.3" /></svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Native height bytes retain the same lattice and tile order. Indexed OBCT v3 adds a height pyramid and conservative bounds; ordinary elevation sampling still reads native tiles.</figcaption>
</figure>

OBCT stores heights as signed meters on a fixed lattice, in small tiles, with a directory of cells.
A tile is the unit of reading, so sampling one position touches a small part of the file. An
assembled map carries one OBCT container, and the map reader hands that region to the terrain
reader as a byte window. See [terrain and elevation](../terrain/).

## Streaming: resident against on-demand

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 270" role="img" aria-label="The map reader keeps small tables in memory. It streams map indexes and geometry through bounded caches. The route reader keeps its small index and streams route chunks.">
  <defs>
    <marker id="aF4" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">What stays in RAM, what streams from the card</text>

  <!-- file on card -->
  <rect class="d-panel-2" x="36" y="48" width="128" height="180" rx="10" />
  <text class="d-tag" x="52" y="68">OBCM object</text>
  <rect x="48" y="78" width="104" height="16" class="d-forest" /><text class="d-sub" x="100" y="90" text-anchor="middle" style="fill:#fff;font-size:12px">header·styles·LOD</text>
  <rect x="48" y="96" width="104" height="56" class="d-muted" /><text class="d-sub" x="100" y="128" text-anchor="middle" style="font-size:12px">quadtree index</text>
  <rect x="48" y="154" width="104" height="66" class="d-water" /><text class="d-sub" x="100" y="190" text-anchor="middle" style="fill:#fff;font-size:12px">geometry chunks</text>
  <text class="d-sub" x="100" y="244" text-anchor="middle" style="font-size:12px">megabytes ≫ RAM</text>

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
  <text class="d-sub" x="340" y="218" style="font-size:12px"><tspan style="fill:#a9501c">OBCR:</tspan> header + the whole (small, flat) index resident;</text>
  <text class="d-sub" x="340" y="232" style="font-size:12px">only geometry chunks stream. The list is cheap to keep.</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Large map, route, and terrain objects do not have to fit in RAM.</figcaption>
</figure>

Every reader takes bytes through [`ByteSource`](src:firmware/obc-formats/src/io.rs) and keeps only
its small tables in memory: the map header, styles, and level table; the route index; the terrain
header and a few tiles. Indexes and geometry stream as the frame needs them. That is what lets a
map larger than the device's RAM be drawn at all.

## The catalog — the map builder's source of truth

OBCC is the map-builder catalog, and the device never reads it. Its root publishes the map schema,
the skins, the region selections, the cell index for each band, the terrain metadata, and the
license information. It pins every object by length and digest: a map is assembled from files
fetched over a network, so the catalog, not the transport, decides what the right bytes are.

The split between schema and skin is why a map can be restyled cheaply. The schema controls
geometry, levels, style identifiers, routing, and chunk size, and changing it needs a rebake. A
skin controls only colors, weights, line style, paint order, and priority, and changing it does
not. See [`OBCC_Spec.md`](src:specs/OBCC_Spec.md).

## Cells and assemblies

OBCA defines a global grid of power-of-two cells and the rules for joining them. A region is baked
once as cells, and every map a rider selects is assembled from those cells.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
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
    <text class="d-sub" x="38" y="118" style="font-size:12px">cell A</text>
    <text class="d-sub" x="110" y="118" style="font-size:12px">cell B</text>
    <text class="d-sub" x="182" y="118" style="font-size:12px;fill:#b9b09a">unused</text>
  </g>
  <text class="d-sub" x="30" y="243" style="font-size:12px;fill:#a9501c">selection (dashed) → the cells it touches</text>

  <!-- arrow -->
  <line class="d-flow" x1="256" y1="150" x2="360" y2="150" marker-end="url(#aCA)" />
  <text class="d-sub" x="308" y="132" text-anchor="middle" style="fill:#a9501c;font-size:12px">chunk bytes</text>
  <text class="d-sub" x="308" y="145" text-anchor="middle" style="fill:#a9501c;font-size:12px">copied verbatim</text>
  <text class="d-sub" x="308" y="172" text-anchor="middle" style="font-size:12px">no decode</text>
  <text class="d-sub" x="308" y="190" text-anchor="middle" style="font-size:12px">no GEOS</text>

  <!-- right: the assembled tree -->
  <text class="d-label" x="392" y="52">the assembly</text>
  <text class="d-sub" x="392" y="67">bbox = grid-aligned 2ⁿ square</text>
  <circle cx="470" cy="92" r="11" class="d-forest" />
  <text class="d-sub" x="490" y="96" style="font-size:12px">root — rebuilt</text>
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
  <text class="d-sub" x="400" y="196" text-anchor="middle" style="font-size:12px">cell</text>
  <text class="d-sub" x="456" y="196" text-anchor="middle" style="font-size:12px">cell</text>
  <text class="d-sub" x="512" y="196" text-anchor="middle" style="font-size:12px">cell</text>
  <text class="d-sub" x="568" y="196" text-anchor="middle" style="font-size:12px;fill:#b9b09a">empty</text>
  <path class="d-hot" d="M370 170 L598 170" stroke-dasharray="4 3" />
  <text class="d-sub" x="604" y="174" style="font-size:12px;fill:#a9501c">cell depth</text>
  <text class="d-sub" x="392" y="228" style="font-size:12px">rebuilt: header · style table · upper index</text>
  <text class="d-sub" x="392" y="243" style="font-size:12px">rebuilt: POIs + hours · the navigation graph</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Exact grid alignment preserves leaf-relative feature anchors. The assembler copies geometry bytes without decoding them.</figcaption>
</figure>

The cell grid and the map quadtree share one global origin, so a leaf of the assembled map is a
leaf of a cell. Feature anchors are relative to their leaf and stay correct, and the assembler
copies geometry chunks without decoding them. It rebuilds only the global parts: the header, the
tables, the POIs, the hours pool, the navigation graph, and the terrain container. Routing nodes on
a seam merge when their coordinates are equal.

All cells in one assembly share the schema revision and the map version, and the assembler replaces
their presentation records with the selected skin. The output is one file. The browser runs the
same engine as the command line, through [`obc-web-assemble`](src:apps/obc-web-assemble), and
verifies the result with the production readers. See [`OBCA_Spec.md`](src:specs/OBCA_Spec.md).

## Source index

- Format constants and byte I/O: [`obc-formats`](src:firmware/obc-formats)
- OBCM reader: [`obc-reader`](src:firmware/obc-reader)
- OBCR reader, converter, and router: [`obc-route`](src:firmware/obc-route)
- OBCT reader and sampler: [`obc-elevation`](src:firmware/obc-elevation)
- OBCM packer: [`obc-pack`](src:host/obc-pack)
- Terrain baker: [`obc-dem`](src:host/obc-dem)
- Map assembler: [`obcm-assemble`](src:host/obcm-assemble)
