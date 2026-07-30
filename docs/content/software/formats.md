---
title: Data formats
description: OBCM maps and OBCR routes — the binary, table-driven formats a microcontroller reads directly off flash, a chunk at a time, with no JSON, no reparsing, and no heap.
---

# Data formats

The device reads two kinds of file: an **OBCM** map and an **OBCR** route. Both are binary, and both exist for the same reason — a microcontroller should read them *directly off flash*, with no JSON to parse, no structure to rebuild in RAM, and no heap to churn. A host produces them once; the device just points at the bytes and draws. (A third format, the [catalog manifest](#the-catalog-a-third-format-for-finding-the-first-two), never reaches the device at all — it's how a host decides *which* map to hand it.)

This page is the guided tour of what's actually in those files. The exhaustive byte-level tables live in the repo specs — [`OBCM_Spec.md`](src:specs/OBCM_Spec.md) and [`OBCR_Spec.md`](src:specs/OBCR_Spec.md) — so here we focus on *why* the bytes are shaped the way they are. Those root specifications remain the normative contracts; the dependency-free [`obc-formats`](src:firmware/obc-formats) crate is the code authority beneath them for version numbers, fixed record lengths, flags, sentinels, endian primitives, and the shared byte-I/O seam. Parsing, caching, conversion, and file assembly stay in the reader, route, and packer crates.

## Two binaries, one philosophy

The map and the route are siblings: they were designed to feel identical to the code that reads them, so the renderer can treat a route chunk and a map chunk with the same instincts.

<figure class="fig">
<svg viewBox="0 0 720 290" role="img" aria-label="Two production pipelines. An OSM extract is turned into a dot-obcm map file offline by the obc-pack packer; a GPX upload is turned into a dot-obcr route file by obc-route, wherever it lands — on the device, in the simulator, or in a browser tab. Both files are read back by the same no_std reader code on the simulator and the device. They share a common design: little-endian, microdegree integers, anchor-plus-delta geometry, explicit offsets, and streaming.">
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
<figcaption>The map is baked <b>offline</b> from an OpenStreetMap extract by the packer; the route is converted <b>wherever the GPX lands</b> — on the device, in the simulator, or in a browser tab — from one <code>no_std</code> routine. Different origins, but the very same reader code parses them on every machine — so what you draw in the browser is what the device draws.</figcaption>
</figure>

Four principles run through both formats:

- **Binary and table-driven.** Numbers, not text. Colours and widths live in a small style table the map references by id; geometry is raw integers. Nothing is parsed from strings at read time.
- **Integer microdegrees.** Every coordinate is an `i32` in millionths of a degree. There are no floats on disk and no projection baked in — turning ground coordinates into pixels is the [renderer's](../rendering/) job, not the file's.
- **No runtime discovery.** Every section is reached through an explicit byte offset, and every count is stored. A `no_std` reader does *zero* traversal or sizing work to understand the file's structure — it reads a header and jumps.
- **Streamed, never resident.** Both files are read through a [`ByteSource`](src:firmware/obc-formats/src/io.rs) a piece at a time, so a map far larger than RAM — or a route hundreds of kilometres long — never has to fit in memory at once.

Where they differ is *shape*: a map is a 2-D area indexed by a quadtree; a route is a 1-D path indexed by a flat list. Everything below follows from that.

## OBCM — the map

### The file, front to back

An OBCM file (current version **10**) opens with a fixed 40-byte header, then a global style table and a level-of-detail (LOD) table, then the LOD layers themselves — coarsest first. Each LOD layer is wholly self-contained: its own quadtree index immediately followed by its own geometry chunks. After the finest layer come three more sections — the [POIs](#pois-a-nearest-list-not-a-map-layer), their shared [hours pool](#opening-hours-a-pooled-weekly-schedule), and, at the very tail, the [navigation graph](#the-navigation-graph-a-routable-network) the device routes over — each reached, like everything else, by an offset stored earlier in the file.

<figure class="fig">
<svg viewBox="0 0 720 210" role="img" aria-label="The OBCM file as a horizontal ribbon: a 40-byte header, a global style table, an LOD table, LOD layer 0 (coarsest) through LOD layer N minus 1 (finest), then a POI section and a navigation-graph section at the tail. Detail increases left to right across the LOD layers. One LOD layer is exploded below to show it is a quadtree index followed by data chunks.">
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
  <text class="d-sub"   x="54"  y="94" text-anchor="middle" style="fill:#e7ead8">40 B</text>
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
  <text class="d-sub"   x="669" y="92" text-anchor="middle" style="fill:#e7ead8;font-size:8.5px">§8 · tail</text>

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
  <text class="d-label" x="468" y="170" text-anchor="middle">data chunks</text>
  <text class="d-sub"   x="468" y="184" text-anchor="middle">tight, offset-addressed</text>
</svg>
<figcaption>The header, style table and LOD table are read once when the file opens — they're tiny. The bulk of the file is the LOD pyramid: each layer its own <b>(index + chunks)</b> pair, simplified to that zoom. Two tail sections are different beasts — not map layers: the <b>POI section</b> (coral), a nearest-list index covered <a href="#pois-a-nearest-list-not-a-map-layer">below</a>, and the <b>navigation graph</b> (teal), a routable network the device runs A\* over, covered <a href="#the-navigation-graph-a-routable-network">last</a>. Reaching any section is an explicit offset, so there is no scanning to "find" where a layer begins.</figcaption>
</figure>

Why a pyramid, rather than one detailed tree with a min-zoom tag on every feature? Because the latter forces the device to *decode* fine geometry just to discover it should be skipped when zoomed out. With independent layers, zooming out reads a small coarse layer and touches nothing else. The renderer's job of [picking the right layer](../rendering/#2-level-of-detail-pick-the-right-layer) for the current zoom is covered on the rendering page; here we only care that the layers exist side by side in the file.

Each entry in the **LOD table** is the directory to one layer — the zoom it serves and where its bytes are:

| Field | Type | What it is |
| :-- | :-- | :-- |
| Max meters/pixel | `f32` | Upper bound of the zoom range this layer covers; the coarsest is `+∞`, strictly decreasing toward fine |
| Index offset | `u32` | Byte offset to this layer's quadtree |
| Node count | `u32` | Number of `u32` nodes in that index |
| Chunk size | `u16` | The **most** bytes one data chunk in this layer may hold — a capacity bound, not a stride (v11) |
| Chunk count | `u32` | Number of data chunks |

Eighteen bytes per entry — the `N × 18 B` in the ribbon above. Every count is stored, so the reader never walks anything to learn a size: that's "no runtime discovery" made concrete. Where the *k*-th chunk begins is one small table away, and that table is worth its own aside.

#### Where a chunk lives: the offset table

Between a layer's index and its chunk bytes sits a run of `chunk_count + 1` `u32` **offsets**. Chunk *k* is `offsets[k] … offsets[k+1]`, measured from the first chunk byte:

```
table_start = index_offset + node_count·4
data_start  = table_start  + (chunk_count + 1)·4
chunk k     = data_start + offsets[k] … data_start + offsets[k+1]
```

Still one multiplication and one read — but chunks no longer have to be all the same size, and that is the point. Until **v11** the *k*-th chunk was simply `data_start + k·chunk_size`, which is beautifully cheap and quietly expensive: a fixed stride means every chunk must be **padded** to `chunk_size` with `0xFF`. And because a quadtree node splits the moment its features overflow one chunk, leaves settle somewhere between a quarter and half full — so the padding isn't a mis-tuned knob, it's structural. Measured on real maps: **53% of a Freiburg map and 65% of a Grimsel map were trailing `0xFF`**, evenly across every layer.

Shrinking `chunk_size` doesn't help (nodes split more often, so the slack per chunk halves while the chunk count doubles); growing it adds slack directly. One `u32` per chunk does help: on that Freiburg map, 1,534 chunks × 4 B = 6 KB of table in exchange for 3.8 MB of padding. Real maps come out **~2.3× smaller**, and the win is proportional — every map, every region.

It pays a second time at read time. The device's bottleneck is the SD card, and a chunk miss used to read a fixed 4,096 B — eight 512-byte blocks — no matter how little of it was real. A tight chunk averages closer to 1,600 B, so a miss reads three or four blocks instead of eight. Unaligned reads cost nothing extra here: the reader's block-buffered source already handles them.

The last table entry (`offsets[chunk_count]`) is the layer's total chunk bytes, which the reader keeps resident — one `u32` read when the file opens, and afterwards every chunk lookup is bounds-checked against it for free. That matters because a chunk id comes out of a quadtree leaf, which in a damaged file is an arbitrary number: the reader validates the pair (in range, non-decreasing, inside the region, no longer than `chunk_size`) before it addresses anything.

### The header

The 40-byte header is the one fixed-size, always-present part of the file. Everything else is found through offsets it stores.

<figure class="fig">
<svg viewBox="0 0 720 170" role="img" aria-label="The 40-byte OBCM header drawn as a byte ruler: bytes 0 to 3 are the magic OBCM, byte 4 is the version (11), bytes 5 to 20 are the global bounding box as four 32-bit integers, bytes 21 to 24 are the style-table offset, byte 25 is the LOD count, bytes 26 to 29 are the LOD-table offset, bytes 30 to 31 are the marker colour, bytes 32 to 35 are the POI-section offset, and bytes 36 to 39 are the navigation-graph offset appended in version 8.">
  <text class="d-tag" x="20" y="24">The 40-byte header, byte by byte</text>

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
  <text class="d-label" x="112" y="93" text-anchor="middle" style="font-size:11px">11</text>
  <text class="d-sub" x="74"  y="122" text-anchor="middle" style="font-size:9px">0–3</text>
  <text class="d-sub" x="112" y="122" text-anchor="middle" style="font-size:9px">4</text>
  <text class="d-sub" x="239" y="122" text-anchor="middle" style="font-size:9px">5–20</text>
  <text class="d-sub" x="389" y="122" text-anchor="middle" style="font-size:9px">21–24</text>
  <text class="d-sub" x="426" y="122" text-anchor="middle" style="font-size:9px">25</text>
  <text class="d-sub" x="464" y="122" text-anchor="middle" style="font-size:9px">26–29</text>
  <text class="d-sub" x="509" y="122" text-anchor="middle" style="font-size:9px">30–31</text>
  <text class="d-sub" x="554" y="122" text-anchor="middle" style="fill:#a9501c;font-size:9px">32–35</text>
  <text class="d-sub" x="614" y="122" text-anchor="middle" style="fill:#2c5230;font-size:9px">36–39</text>

  <text class="d-sub" x="44" y="150" style="font-size:11px">A short read here is the only "is this even a map?" check the reader needs.</text>
</svg>
<figcaption>Fixed offsets, no surprises. A few details a reader notices: the bbox is stored <b>lat, lon</b> (a packer ordering quirk); the <b>marker colour</b> — the you-are-here chevron — rides in the header because the marker isn't an OpenStreetMap feature; and the <b>POI</b> (coral) and <b>navigation-graph</b> (teal) offsets at the tail are the growth that carried the header from 32 → 36 → 40 bytes. Earlier fields never move — a v7 reader that stops at byte 36 still parses everything it knew — and v9, v10 and v11 changed section internals, the style record and the chunk layout without touching the header, only ticking the version byte (now <code>11</code>).</figcaption>
</figure>

The **style table** that follows maps small numeric ids to how a feature looks. Each record is eight bytes (v10 grew it from six):

```rust
pub struct Style {
    pub id: u8,             // referenced by feature headers
    pub z_index: i8,        // painter's order: lower draws first
    pub color: u16,         // RGB565 — device-independent
    pub weight: u8,         // nominal line width (px at a reference zoom; the renderer ramps it — see rendering)
    pub priority: u8,       // 1 = keep first … 4 = drop first (flags bits 0–1)
    pub dashed: bool,       // flags bit 2 — a dashed line (v10)
    pub color2: Option<u16>,// flags bit 3 + a trailing RGB565 (v10)
}
```

The last two fields are packed into the record's **flags byte** (dashed = bit 2, "color2 present" = bit 3) plus a trailing `u16`. `color2` is a **secondary** colour: **v10** carries it so a later render pass can draw road casings, dashed admin borders, railway stripes and building outlines at the finest zoom — the [line-styles work](https://github.com/timohueser/OpenBikeComputer/issues/556). It's written `0x0000` with the bit clear when unused, and readers ignore it then — black is a real colour (rails), not a "none" sentinel — so a map that uses no line styles is byte-for-byte the old record padded to eight, and renders identically.

Two things worth knowing about style ids. First, they're **assigned by the packer, not authored** — the reader never depends on a specific value, only that ids are unique within the file, so the format can't be broken by an id collision. Second, `0xFF` is reserved as the "end of features" sentinel inside a chunk (more below), which caps a file at 254 distinct styles. The colour is stored once, device-independently, and resolved to the panel's palette at render time — the same RGB565 looks right on a true-colour desktop window and on the device's 64-colour panel.

### The quadtree index

Within a LOD layer, geometry is bucketed into fixed-size chunks indexed by a quadtree over the map's bounding box. On disk that tree is just a **flat array of `u32`** — one word per node — and a single high bit tells you what kind of node you're looking at.

<figure class="fig">
<svg viewBox="0 0 720 205" role="img" aria-label="A 32-bit node word with the high bit highlighted as the branch flag. Below, three interpretations: high bit set means a branch whose low 31 bits index the first of four children; the value 0x7FFFFFFF means an empty leaf; any other value with the high bit clear is a leaf holding a chunk id.">
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
<figcaption>A single bit-test classifies every node, so the walk needs no separate node-type table. A branch stores only the index of its <b>first</b> child; the four children (NW · NE · SW · SE) sit consecutively, and their bounding boxes are <b>re-derived</b> on the fly by halving the parent box — never stored. Identical math at every level and every LOD.</figcaption>
</figure>

```rust
const BRANCH_BIT: u32 = 0x8000_0000; // high bit set ⇒ a branch
const EMPTY_LEAF: u32 = 0x7FFF_FFFF; // a leaf with no chunk
// otherwise: the value is a chunk id into this LOD's data chunks
```

That children's boxes are *computed*, not stored, is the reason the renderer's subdivision and the packer's must agree exactly — both split at floor-division midpoints. How the renderer walks this tree to cull invisible chunks is the [quadtree cull](../rendering/#3-the-quadtree-cull-only-the-chunks-you-can-see) step on the rendering page. The format's contribution is just this: a tree compact enough to stream, where one word says everything about a node.

### Features: an anchor, then deltas

Inside a chunk, each feature's geometry is stored as one absolute starting point — the **anchor** — followed by a chain of small **deltas**. This is where the format earns its compactness.

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="On the left, a polygon ring in absolute microdegrees: the anchor vertex carries a full large coordinate, and each subsequent edge is a small step. On the right, the encoded form: the anchor stored relative to the leaf's corner, then a list of small delta pairs. A per-feature decision picks one-byte deltas when every step fits in a signed byte, otherwise two-byte deltas.">
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
<figcaption>The first vertex is the anchor, stored once and relative to the leaf's corner so even it stays small; every other vertex is a step from the one before. Most steps are a handful of microdegrees, so a whole ring usually fits in <b>single bytes</b> — and a feature with one long edge simply bumps the whole ring to 16-bit deltas. (The packer also pre-splits any segment longer than 30 000 µdeg, so a delta can never overflow 16 bits.)</figcaption>
</figure>

The device decodes one complete feature into fixed caller-owned point and ring buffers. Before it publishes that geometry, it validates every encoded ring and checks that the whole feature fits. An over-capacity or malformed feature is therefore consumed and **dropped whole** with a typed outcome; it is never exposed as a shortened line or an open polygon. Production maps stay within the format's 2,048-vertex cap, while this rule keeps smaller device profiles and damaged files honest without changing a byte on disk. The [rendering pipeline](../rendering/#4-decode-by-priority) explains how those outcomes are counted separately from ordinary frame-budget drops.

Decoding a ring is exactly as simple as it looks — pick the delta width once, then walk:

```rust
let (dx, dy) = if is_16 {
    (rd_i16(chunk, off) as i32, rd_i16(chunk, off + 2) as i32) // flag bit 0 set
} else {
    (chunk[off] as i8 as i32, chunk[off + 1] as i8 as i32)     // 8-bit — the common case
};
px += dx;  py += dy;   // each delta steps to the next vertex
```

A feature is introduced by a **7-byte header** — or a 12-byte one when it needs the room — and a flags byte in it says how to read the rest:

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="Two feature-header byte rulers drawn to the same scale, forty pixels per byte, sharing a left edge so the seven-byte compact header is visibly shorter than the twelve-byte wide one. Compact: one byte style id, one byte flags, one byte exterior point count, two bytes unsigned anchor X, two bytes unsigned anchor Y. Wide: one byte style id, one byte flags, two bytes point count, four bytes signed anchor X, four bytes signed anchor Y. The flags byte expands into four bits: 16-bit deltas, polygon, has-holes, and wide, with the wide bit highlighted as the one that selects the layout. Below, the polygon-with-holes byte layout as a ribbon: the header, the exterior deltas, a hole count, then each hole's point count and deltas.">
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
<figcaption>Two layouts, one decision. <b>Flags sits at byte 1</b> in both, because its <b>wide</b> bit is what tells the reader how many bytes the header is — it has to be readable before anything behind it. The <b>compact</b> form spends one byte on the point count and two on each anchor; the <b>wide</b> form restores the <code>u16</code> count and signed <code>i32</code> anchors for the features that need them. Everything after the header is identical either way: the exterior ring first, then the hole count and each hole's deltas <b>only if</b> the holes flag is set, so a line or a simple polygon pays nothing for machinery it doesn't use. A <code>0xFF</code> style id — an impossible style — ends the features in a chunk.</figcaption>
</figure>

Why two forms? The 12-byte header was, at 66,910 features on that Freiburg map, **803 KB** — a third of the real data, for an average feature of 7.6 vertices, and eight of those twelve bytes were the anchor. But the anchor is already stored *relative to its leaf's corner*, and at fine zooms a leaf spans far less than the 65,535 µdeg (~7 km) a `u16` covers, so most of that width is zeroes. Hence the split: `u8` point count and `u16` anchors for the common feature, and a flag bit for the ones that genuinely don't fit — a feature with more than 255 vertices in its exterior, or a coarse-layer leaf big enough that its own corner is kilometres away. That is another ~335 KB, and it is why the reader must read flags first and derive the width, rather than assume it.

There's a quiet payoff to the holes layout: a polygon's holes are just extra rings appended after the exterior. The [scanline fill](../rendering/#polygons-even-odd-scanline-fill) treats them as additional edges in the same crossing list, so holes "fall out" of the even-odd rule with no special case — the format and the rasteriser were designed to meet in the middle.

### POIs: a nearest-list, not a map layer

Everything so far serves one question — *what's on screen right now?* — and the quadtree answers it by viewport: give me the chunks a rectangle touches. Version **6** added a section that answers a different question — *where's the nearest water / campsite / bakery?* — and that changes the shape of the index. The [points of interest](../packer-routing/#extracting-pois) the packer harvests from OpenStreetMap aren't drawn on the map at all; the device surfaces them as a category → nearest-list [browser](../ui/#the-pois-browser). So they're indexed for a **nearest-N** query, not a viewport walk. Version **7** widened each record to carry the POI's opening hours, pooled into a [shared section](#opening-hours-a-pooled-weekly-schedule) at the file tail.

The section is a small **directory** followed by, per category, a familiar pair: a quadtree index and its data chunks. There are six categories — Water, Campsite, Accommodation, Resupply, Pharmacy, Bike shop — and each gets *its own* quadtree, so "nearest bakery" scans only bakeries.

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="The POI section. On the left, the directory: a category count of 6, a shared chunk size, then one entry per category holding a category id, index offset, node count and chunk count, plus the trailing hours-pool offset and count. An arrow leads to one category's quadtree index followed by its data chunks — the same index-then-chunks shape as an LOD layer. On the right, one POI record drawn as a 36-byte ruler: four bytes latitude, four bytes longitude, one byte subtype, one byte name length, twenty-four bytes name, and a two-byte HoursRef index into the hours pool.">
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
  <text class="d-tag" x="20" y="152">one record — a fixed 36 bytes <tspan style="fill:#a9501c">(v7)</tspan></text>
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
<figcaption>Each category's index and chunks are laid out exactly like a LOD layer — the same flat <code>u32</code> quadtree over the same header bbox — so the reader walks a POI category with the very same leaf-walk it uses for geometry. Coordinates are stored <b>absolute</b>: at a fixed 36 bytes the delta saving isn't worth breaking symmetry, and fixed-size records make chunk packing trivial (<code>512 / 36 = 14</code> per chunk, no length bookkeeping). The final two bytes are a <b>HoursRef</b> (coral) — an index into the hours pool below, or <code>0xFFFF</code> when the POI has no listed hours.</figcaption>
</figure>

Two design notes are worth pulling out. First, the **category is never stored in the record** — it's implied by *which* category's quadtree the record came from, and each subtype maps to exactly one category anyway. Second, **names are folded to printable ASCII at pack time** and capped at 24 bytes, because the `Name` field is a fixed-width, one-byte-per-character slot (the [packer](../packer-routing/#extracting-pois) transliterates umlauts and accents — `ä → ae` — rather than store variable-width UTF-8); an unnamed POI (name length `0`) shows its subtype's fallback label on-device. A `0xFF` subtype byte ends a chunk, mirroring geometry's `0xFF`-style-id sentinel.

**The same index now answers a second question.** *Nearest-N around the fix* was the query the section was designed for, and it's still the only one that shaped the bytes — but the per-category quadtrees also serve a **route-corridor walk**: give me the POIs of these categories within **300 m** of the route *still ahead of me*, ordered along the route. Nothing in the format changed for it; what changed is the window the walk is given. Instead of one disc around the rider, the reader takes the route chunk by chunk in route order, inflates each chunk's bounding box by the corridor half-width, and walks the same leaves inside that window — then projects each surviving record onto the chunk's polyline to get a **distance along the route** and a **signed lateral offset** (positive = right of travel, the identical convention [OBCR waypoints](#waypoints-a-category-and-a-side) store). The result is capped at **16** and the walk stops early once the slots are full and the next chunk starts farther along than the worst held entry, so the cost tracks POI *density*, not route length. That's what feeds the riding [Up ahead timeline](../ui/#up-ahead-one-timeline-for-the-route) — and why the two spatial questions the device can ask, *near me* and *up ahead*, run off one index.

The full directory bytes, the canonical category/subtype id table, and the record fields are in [`OBCM_Spec.md` §7](src:specs/OBCM_Spec.md). What the packer harvests and how, and how the device browses the result, are the [extraction stage](../packer-routing/#extracting-pois) and the [POIs browser](../ui/#the-pois-browser).

### Opening hours: a pooled weekly schedule

That `HoursRef` at the end of every record points into a pooled section written near the **file tail** (the [navigation graph](#the-navigation-graph-a-routable-network) is the only thing after it): a single **hours pool**. OSM tags opening hours as a terse little grammar — `Mo-Fr 08:00-18:00; Sa 09:00-13:00; PH off` — that a microcontroller has no business parsing. So the packer [parses it once, at pack time](../packer-routing/#extracting-pois), into a fixed **29-byte weekly schedule** the device can read with a single array lookup. No `opening_hours` string ever reaches the device.

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="The hours pool. A single 29-byte schedule blob is drawn as a ruler: one flags byte, then seven days from Monday to Sunday, each day two interval slots, each slot an open-quarter and a close-quarter byte. Below, the dedup pool: many POI records with a HoursRef index point into a small list of unique blobs, so shops that share hours share one entry.">
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
<figcaption>A schedule is a <b>flags byte</b> then seven days, each holding up to two open intervals of two bytes — an <b>open</b> and a <b>close</b> quarter-hour from midnight (<code>0…96</code>, so <code>96</code> = 24:00). A closed day is <code>(0, 0)</code>; a 24-hour day is <code>(0, 96)</code>; an overnight interval stores <code>close ≤ open</code> and wraps in place. Two flag bits record what the packer couldn't keep verbatim — <b>seasonal</b> (a date rule flattened to an in-season week) and <b>truncated</b> (a public-holiday rule, <code>sunrise/sunset</code> time, or third interval dropped). Because a region's shops share the same handful of schedules, identical blobs collapse to one, and a record's <code>HoursRef</code> is just its pool index — <code>0xFFFF</code> meaning "no hours listed."</figcaption>
</figure>

The pool's exact layout — the leading `count`, the blob byte order, the flag bits, and the overnight/24-hour conventions — is [`OBCM_Spec.md` §7.5](src:specs/OBCM_Spec.md). The pack-time parser that fills it is the [`opening_hours` stage](../packer-routing/#parsing-opening-hours); the device-side lookup that turns a blob into *today's hours* and an *open-now* answer drives the [POI detail view](../ui/#the-poi-detail-view).

### The navigation graph: a routable network

Everything so far is geometry you *look at*. Version **8** added a section for geometry you *travel* — a **routable graph** the device runs A\* over, so a rider can [pick a POI and get a route to it](../architecture/#on-device-routing-the-router-seam) with no phone and no pre-planning. Highways in the map are drawn but carry no *topology* — a road is just a styled polyline, with no notion of what connects to what. The [packer builds the graph](../packer-routing/#building-the-navigation-graph) from the OSM node ids highways *share*: junction **nodes** joined by **edges** whose interiors hold no junctions. This section is that graph on disk, at the very tail of the file. Version **9** kept that shape but made routing *bike-type-aware* — each edge now carries a **way-kind** byte and the section opens with a small **profile table** — and slimmed the records so a chunk holds more of the graph (details below).

Its shape is set by one hard fact: the device has **no room for a node-id → offset table**. A real region has millions of graph elements; an index over all of them can't stay resident. So the section is arranged for the only access pattern that fits RAM — **spatial re-fetch**. A node lives in a leaf of a quadtree over the same global bbox, and each junction record carries its neighbours' coordinates *inline*.

<figure class="fig">
<svg viewBox="0 0 720 256" role="img" aria-label="The version 9 navigation-graph section, five parts in file order. A 28-byte nav directory — the graph's resident footprint — is followed by a profile table of one to eight 52-byte bike profiles, then a node quadtree (the same flat u32 encoding as an LOD, over the header bbox), then variable-length junction records bin-packed into 512-byte chunks so distinct leaves may share a chunk, and separately a deduplicated edge pool addressed by byte offset. One junction record is exploded to show it stores its own coordinate and dense id, then a list of 15-byte neighbour entries, each carrying the neighbour's id, its coordinate inline, the connecting edge id, the edge cost in metres, and the edge's way-kind byte.">
  <defs>
    <marker id="aN1" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="22">§8 (v9) — directory · profile table · node quadtree · junction records · edge pool</text>

  <!-- directory -->
  <rect class="d-panel-2" x="24" y="42" width="140" height="64" rx="9" />
  <text class="d-label" x="38" y="60" style="font-size:11px">nav directory</text>
  <text class="d-sub" x="38" y="76"  style="font-size:9px">28 B — resident</text>
  <text class="d-sub" x="38" y="89"  style="font-size:9px">offsets · counts</text>
  <text class="d-sub" x="38" y="102" style="font-size:9px">chunk size · profiles</text>

  <!-- profile table (v9) -->
  <line class="d-flow" x1="166" y1="74" x2="196" y2="74" marker-end="url(#aN1)" />
  <rect class="d-water" x="200" y="48" width="128" height="52" rx="9" stroke="#3c6b39" stroke-width="1.2" />
  <text class="d-label" x="264" y="70" text-anchor="middle" style="fill:#fff;font-size:10.5px">profile table</text>
  <text class="d-sub" x="264" y="86" text-anchor="middle" style="fill:#dfe6e0;font-size:8.5px">1..8 × 52 B</text>

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
  <text class="d-sub" x="184" y="158" style="font-size:8.5px;fill:#a9501c">edge id = pool-relative byte offset</text>
  <text class="d-sub" x="184" y="171" style="font-size:8.5px">(chunk = id / 512) — zero index bytes</text>
  <text class="d-sub" x="184" y="184" style="font-size:8.5px">fetched only at route emit</text>

  <!-- explode one junction record -->
  <line x1="518" y1="98" x2="410" y2="150" stroke="#9aa884" stroke-width="1.1" />
  <line x1="696" y1="98" x2="700" y2="150" stroke="#9aa884" stroke-width="1.1" />
  <rect class="d-hot" x="392" y="150" width="308" height="96" rx="10" style="fill:#f8efe4" />
  <text class="d-tag" x="408" y="168" style="fill:#a9501c">one junction record — 13 + 15 × degree B</text>
  <text class="d-sub" x="408" y="186" style="font-size:9.5px">lat · lon · dense id · degree</text>
  <text class="d-sub" x="408" y="202" style="font-size:9.5px">then <tspan style="font-weight:700">degree</tspan> × neighbour (15 B each):</text>
  <text class="d-sub" x="420" y="218" style="font-size:8.5px" font-family="var(--mono)">nbr id · nbr lat,lon · edge id · cost m · way-kind</text>
  <text class="d-sub" x="420" y="234" style="font-size:8px;fill:#a9501c">coord + way-kind inline — a settle relaxes with no extra fetch</text>
</svg>
<figcaption>The section's resident cost is a <b>28-byte directory</b>: offsets, counts, the pinned 512 B chunk size, and the profile table's location. The <b>node quadtree</b> is byte-for-byte the same flat-<code>u32</code> encoding as an <a href="#the-quadtree-index">LOD index</a>, built over the same header bbox, so the reader walks it with the identical leaf-walk. Its leaves point at <b>junction records</b>, bin-packed first-fit into 512-byte chunks — distinct leaves may share a chunk, so the walk is written to decode a shared chunk idempotently. Each record stores its own coordinate and dense id, then one <b>15-byte</b> entry per neighbour with the neighbour's <b>coordinate inline</b>, the connecting edge's id, its cost in metres, and its <b>way-kind</b> byte. The <b>edge pool</b> — the actual polylines — is touched only when a route is emitted, and an edge is addressed by byte offset into the pool, so there's no edge-id table to keep resident either.</figcaption>
</figure>

Why store the neighbour's coordinate twice — once in its own record, once in every record that points at it? Because that redundancy is exactly what makes the router cheap. A\* settling a node needs, for each neighbour, the straight-line distance to the goal (the heuristic `h`). With the coordinate inline, that number falls straight out of the record already in hand — no chase to the neighbour's own record just to read where it is. **One quadtree descent, one chunk read, then relax every neighbour from bytes already decoded.**

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="One A-star settle over the nav graph. On the left, the router descends the node quadtree to the leaf containing the settled node's coordinate — a single point query, not a viewport. That leaf resolves to one chunk id, and one chunk read brings the settled junction's record into RAM. On the right, that record's neighbour entries are relaxed in place: each entry already carries the neighbour's coordinate, so the great-circle distance to the goal is computed with no further read. Only when the final route is emitted is the edge pool touched, to stitch the came-from chain into a polyline.">
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

  <!-- 3 relax each neighbour -->
  <line x1="452" y1="150" x2="508" y2="150" stroke="#3c6b39" stroke-width="2" marker-end="url(#aN3)" />
  <text x="480" y="142" text-anchor="middle" style="font-family:var(--mono);font-size:8.5px;fill:#3c6b39">relax</text>
  <text class="d-sub" x="520" y="52" style="font-size:9px;fill:#6b7758">③ relax — no further read</text>
  <rect class="d-hot" x="520" y="60" width="180" height="150" rx="10" style="fill:#f8efe4" />
  <text class="d-sub" x="536" y="84" style="font-size:9.5px">per neighbour, from bytes</text>
  <text class="d-sub" x="536" y="98" style="font-size:9.5px">already in hand:</text>
  <text class="d-sub" x="536" y="118" style="font-family:var(--mono);font-size:9px">g' = g + cost_m · w</text>
  <text class="d-sub" x="536" y="134" style="font-family:var(--mono);font-size:8.5px;fill:#a9501c">w  = profile(way_kind)</text>
  <text class="d-sub" x="536" y="150" style="font-family:var(--mono);font-size:9px">h  = gc_dist(nbr, goal)</text>
  <text class="d-sub" x="536" y="166" style="font-family:var(--mono);font-size:9px;fill:#a9501c">f  = g' + ε·h</text>
  <text class="d-sub" x="536" y="186" style="font-size:8px">coord inline → <tspan style="font-weight:700">h</tspan>: zero fetches</text>
  <text class="d-sub" x="536" y="197" style="font-size:8px">way-kind inline → <tspan style="font-weight:700">w</tspan>: none either</text>

  <!-- edge-pool footnote -->
  <rect class="d-panel-2" x="34" y="258" width="666" height="30" rx="8" />
  <text class="d-sub" x="366" y="277" text-anchor="middle" style="font-size:9px">the <tspan style="font-weight:700">edge pool</tspan> is untouched until the route is <tspan style="font-weight:700">emitted</tspan> — then the came-from chain's edge ids stitch into one polyline</text>
</svg>
<figcaption>A settle is <b>one descent + one read + a straight relax</b>. The quadtree walk is a <i>point</i> query resolving to a single chunk; that read brings the whole junction record into RAM, and every neighbour relaxes straight off it — the cost step <code>g</code> scales the stored metres by the bike profile's weight for the edge's way-kind, and the heuristic <code>h</code> is the great-circle distance from the <b>inline</b> coordinate to the goal, with no chase to the neighbour's own record. An eight-slot tile cache holds the frontier's active leaves resident (see <a href="../architecture/#on-device-routing-the-router-seam">the router seam</a>); the edge pool is read once, at the end, to stitch the winning chain into the route's line.</figcaption>
</figure>

The full byte layout — the 28-byte directory fields, the 52-byte profile records and their `1/16` fixed-point multipliers, the 15-byte neighbour entry, the `0xFF` degree sentinel and degree-24 cap, the edge record with its densified `int16` deltas, and how an over-long edge is split at synthetic junctions so no record ever straddles a chunk — is [`OBCM_Spec.md` §8](src:specs/OBCM_Spec.md). What the packer does to *build* this graph from raw highways, and how a profile weights it, is the [extraction stage](../packer-routing/#building-the-navigation-graph); how the device turns it into a route the rest of the system can't tell from a GPX is [the router seam](../architecture/#on-device-routing-the-router-seam).

## OBCR — the route

A route is a single ordered polyline with elevation, plus precomputed ride statistics. It borrows every OBCM convention — little-endian, microdegrees, anchor + delta — but where the map is 2-D and indexed by a quadtree, a route is a *path*, so its index is a flat list scanned in order.

### The file

<figure class="fig">
<svg viewBox="0 0 720 215" role="img" aria-label="The OBCR file as a horizontal ribbon: a 128-byte header, then each chunk's data back to back, then the chunk index, then an optional waypoint table last. One chunk is exploded below into fixed 6-byte records of delta-longitude, delta-latitude, and absolute elevation.">
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
<figcaption>The index and waypoint table are written <b>last</b>: a streaming converter doesn't know how many chunks a route needs until it has emitted them all, so it patches the header's offsets at the end. Because every section is reached by an explicit offset, the physical order isn't load-bearing — the reader would accept the index first just as happily.</figcaption>
</figure>

The header carries the route's bounding box, its start point (for centering the camera), and the **precomputed totals** — distance, ascent, descent, elevation range — plus the route name; a small header extension points at the **waypoint table**: fixed 44-byte records for the points of interest a planner attaches to a route (covered [next](#waypoints-a-category-and-a-side)). A reader that ignores them still skips the section in O(1) by construction. A 44-byte index entry per chunk holds that chunk's bounding box (for the viewport query), its anchor, its point count, the **cumulative distance and ascent at its first point**, and where its bytes live.

Those cumulative stats are the trick that makes "42 km / 600 m to go" an O(1) subtraction once you know which segment you're on, rather than a walk over the whole route every frame.

### Waypoints: a category and a side

A `<wpt>` in a planner's GPX carries more than a name. Komoot, RideWithGPS and Garmin BaseCamp all tag their waypoints with a symbol — *Water*, *Campground*, *Lodging* — and the device used to drop it on the floor. Format **v3** keeps it, along with one more fact the rider actually decides on: **how far off the route the stop sits, and on which side**.

<figure class="fig">
<svg viewBox="0 0 720 166" role="img" aria-label="One OBCR waypoint record drawn as a 44-byte ruler: bytes 0 to 3 the distance along the route as an unsigned 32-bit integer, 4 to 7 longitude, 8 to 11 latitude, 12 to 13 elevation, byte 14 the category id, byte 15 the name length, bytes 16 to 17 the signed lateral offset in metres, 18 to 19 reserved, and 20 to 43 the 24-byte name. The category and lateral-offset fields are highlighted as the two fields version 3 added.">
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
  <text class="d-sub" x="24" y="140" style="font-size:10px">the two coral fields are what v3 brought — <tspan style="fill:#a9501c">category</tspan> (byte 14, reusing the retired Type byte) and</text>
  <text class="d-sub" x="24" y="156" style="font-size:10px">the signed <tspan style="fill:#a9501c">lateral offset</tspan> (16–17), which with its pad pushed Name to 20 and the record 40 → 44 B</text>
</svg>
<figcaption>Records are sorted ascending by <b>Distance Along</b>, which is defined by <b>nearest-point placement</b>: the cumulative route distance at the raw track point closest to the waypoint's own coordinate. A free-standing GPX <code>&lt;wpt&gt;</code> carries no ride-order of its own, so both the firmware converter and the phone importer place it that way — and the <b>lateral offset</b> falls straight out of the same projection, which is exactly why it's stored rather than derived on the device: re-measuring it would need the <i>raw</i> track the converter saw, not the decimated geometry it wrote.</figcaption>
</figure>

**Category is the map's own id space, not a second taxonomy.** Byte 14 holds `0` for generic or `1..=6` — the same six ids the [POI section](#pois-a-nearest-list-not-a-map-layer) uses (`1` water · `2` campsite · `3` accommodation · `4` resupply · `5` pharmacy · `6` bike shop, `OBCM_Spec.md` §7.4). That's the whole point: a stored waypoint and a map POI drawn from the same icon can be sorted into [one list](../ui/#up-ahead-one-timeline-for-the-route) without the rider having to remember which file a stop came from. `0` is first-class rather than an error case — most hand-placed waypoints ("turn left here") map to nothing and draw as a plain diamond — and an unrecognised value renders generic while surviving a rewrite byte-for-byte.

**The offset is signed, and the sign is the side.** Its magnitude is the ground distance from the waypoint to the track point that won the placement; its sign is the cross product of the local direction of travel with the offset vector — **positive = right**, negative = left, `0` = on the line. That's a deliberate agreement with the [route-corridor query](#pois-a-nearest-list-not-a-map-layer) on the map side, which computes its own offsets the same way, so the riding UI reads one rule for both sources and can draw `←300m` or `→300m` without asking where the entry came from.

**Symbols are freeform, so the mapping is a curation.** A producer takes `<sym>` if non-empty, else `<type>`, and matches it **case- and separator-insensitively** (`Drinking Water` = `drinking_water` = `drinking-water`; every non-alphanumeric byte is a word break, runs collapse, ends trim). Sixty-nine strings gathered from real Komoot / RideWithGPS / Garmin BaseCamp exports land on the six ids; anything else stores `0`, and a waypoint is **never dropped** for its symbol:

| Category | A few of the symbols that map to it |
| :-- | :-- |
| `1` water | `Water` · `Drinking Water` · `Fountain` · `Spring` · `Well` |
| `2` campsite | `Campground` · `Campsite` · `Tent` · `RV Park` |
| `3` accommodation | `Lodging` · `Hotel` · `Hostel` · `Guest House` · `Alpine Hut` |
| `4` resupply | `Convenience Store` · `Supermarket` · `Bakery` · `Restaurant` · `Cafe` · `Gas Station` |
| `5` pharmacy | `Pharmacy` · `Chemist` · `Drugstore` |
| `6` bike shop | `Bike Shop` · `Bicycle Repair` · `Cyclery` |

Two curation calls are worth naming. **Eating and shopping share resupply** — there is no food category, because a rider hunting supplies wants the bakery and the café in the same list. And symbols with **no honest home** among the six stay generic rather than being forced into the nearest one: *Restroom*, *Parking*, *Ferry*, *Hospital*, *First Aid*, *Viewpoint* and *Summit* are all deliberately absent. A wrong icon is worse than a diamond.

The full table, row for row, is [`OBCR_Spec.md` §4.1](src:specs/OBCR_Spec.md) — normative, and mirrored in code by [`obc-route/src/symbol.rs`](src:firmware/obc-route/src/symbol.rs), with a test asserting every table key is already in normal form so an unreachable row can't make the spec a lie.

> **v3 rejects, it doesn't reinterpret.** The category byte reuses the offset the old 10-value waypoint taxonomy (water/food/…/danger) occupied, so a stored v2 file's byte would decode as a *different* category — silently wrong is the one outcome worth avoiding. The reader accepts **v3 only**; a route written by older firmware fails at the header and re-imports from its GPX through the phone or a USB drop. Recorded rides are a separate format and are unaffected. Same posture as the OBCM v8→v9 bump.

### Chunks, seams, and deltas

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="On the left, a route polyline split into three coloured chunks; the boundary vertices are shared, so chunk k's last point equals chunk k plus one's anchor. On the right, one chunk's parts: its index entry holds the anchor, bbox and cumulative stats, while its data holds delta-longitude and delta-latitude steps with absolute elevation.">
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
<figcaption>Consecutive chunks repeat their boundary vertex, so each chunk's polyline can be drawn on its own with no gap at the seam, and the cumulative stats join up continuously. Longitude and latitude chain as deltas; elevation is stored <b>absolute</b> — it's small enough to fit the same two bytes, and skipping the running sum keeps the decode trivial.</figcaption>
</figure>

```rust
let (mut lon, mut lat) = (anchor_lon, anchor_lat); // first point IS the anchor
for record in records {            // each record: (dLon: i16, dLat: i16, ele: i16)
    lon += record.d_lon as i32;
    lat += record.d_lat as i32;
    out.push(RoutePoint { lon, lat, ele: record.ele }); // ele is absolute
}
```

### Exact stats, decimated geometry

A route you draw doesn't need every GPS sample — a thinned polyline looks identical at the device's pixel pitch. But a route you *plan with* does need exact numbers. OBCR keeps both honest by separating them at conversion time: the stored geometry is **decimated** (drop a vertex within a metre of the line it sits on, but force-keep one at least every ~1.2 km), while the header totals are computed from **every raw GPX point**. So the line is cheap to draw and the "total climb" you read is real. One last guard runs the other way: a leg longer than 30 000 µdeg is **densified** with interpolated vertices, so the `i16` deltas above can't overflow even when a sparse two-point upload has no intermediate vertex to keep — the same split the [packer applies](#features-an-anchor-then-deltas) to map geometry.

> **Convert where it lands.** There is no offline route step. The GPX→OBCR converter is one portable `no_std` routine, and every place a GPX can land runs *that* routine: the device on a USB upload, the simulator on import, and — compiled to wasm — the web builder in the browser tab, so a route you drop on the site is converted client-side with no server involved at all. All three produce the same bytes, and the [shared fixtures](src:specs/vectors) hold them to it. It streams the GPX in a single pass — O(1) RAM regardless of route length — emitting each finished chunk while keeping only a bounded index in memory. (BLE uploads arrive **already converted**: the companion app encodes imported GPX/TCX to OBCR on the phone, per the [BLE interface spec](src:specs/obc-ble-interface-spec.md), so the device just writes the bytes to storage.)

One thing the file *doesn't* store is the elevation **profile** the Statistics screen draws. That's rebuilt once when a route loads — a multi-resolution min/max pyramid over distance, the same coarse-to-fine idea as the map's LODs, so the profile can be zoomed and panned without ever re-reading geometry. It's a runtime structure rather than a format concern; the [UI page](../ui/) covers how it's drawn. The route's **climbs** are the same kind of runtime derivation — segmented from that profile when the route loads, never stored in the file or sent over the link (the [Climb panel](../ui/) draws them).

## Recorded rides — the track log and the ride object

[`obc-route`](src:firmware/obc-route) owns one more pair of formats beyond the route you *load*: the two the device *writes* when it records a ride. They share the family's DNA — little-endian, integer coordinates — but they're **logs, not decimated drawings**, so they keep every accepted fix at full fidelity. This is also where a ride's **BLE-sensor** data lives ([epic #707](https://github.com/timohueser/OpenBikeComputer/issues/707)): heart rate, cadence, and power ride along inside both records.

<figure class="fig">
<svg viewBox="0 0 720 258" role="img" aria-label="The 20-byte recorded-track record drawn as a byte ruler: bytes 0 to 3 longitude i32, 4 to 7 latitude i32, 8 to 9 elevation i16, 10 to 11 a flags word whose bit 0 marks a segment start, 12 to 15 a millisecond timestamp, then the version-2 sensor tail — byte 16 heart rate, byte 17 cadence, bytes 18 to 19 power — each with a sentinel meaning absent. Below, the Finish-time fan-out: the headerless .obct log is converted in one streaming pass into a GPX file with heart-rate, cadence and power extensions and into a ride object v2 file, the exact bytes the phone downloads.">
  <defs>
    <marker id="rr1" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">The 20-byte track record — 16 bytes of fix, then a 4-byte sensor tail (v2)</text>

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

  <!-- Finish fan-out -->
  <rect class="d-panel-2" x="40" y="168" width="158" height="64" rx="10" />
  <text class="d-label" x="119" y="192" text-anchor="middle" style="font-size:10.5px">recorded log</text>
  <text class="d-sub" x="119" y="208" text-anchor="middle" style="font-size:9px">.obct · N × 20 B</text>
  <text class="d-sub" x="119" y="222" text-anchor="middle" style="font-size:9px;fill:#a9501c">headerless</text>

  <line class="d-flow" x1="198" y1="200" x2="302" y2="200" marker-end="url(#rr1)" />
  <text class="d-sub" x="250" y="190" text-anchor="middle" style="font-size:9px">Finish —</text>
  <text class="d-sub" x="250" y="216" text-anchor="middle" style="font-size:8.5px">one streaming pass</text>

  <rect class="d-panel" x="308" y="164" width="384" height="34" rx="8" />
  <text class="d-sub" x="320" y="185" style="font-size:9.5px"><tspan class="d-label">GPX</tspan> — gpxtpx:hr / gpxtpx:cad + a bare &lt;power&gt;</text>

  <rect class="d-hot" x="308" y="206" width="384" height="34" rx="8" style="fill:#f8efe4" />
  <text class="d-sub" x="320" y="227" style="font-size:9.5px"><tspan class="d-label" style="fill:#a9501c">ride object v2</tspan> — RD{id}.ORD, the phone's download</text>
</svg>
<figcaption>One record is a fixed <b>20 bytes</b>: the original 16 (position, elevation, a segment-start flag, a millisecond timestamp) plus a <b>4-byte sensor tail</b> — <code>hr u8 · cad u8 · pwr u16</code>, each written as a sentinel (<code>0xFF</code> / <code>0xFF</code> / <code>0xFFFF</code>) when the value was absent or stale. At <b>Finish</b> the headerless log is converted in one streaming pass into a <b>GPX</b> (with <code>gpxtpx</code> heart-rate/cadence extensions and a bare <code>&lt;power&gt;</code>) and a <b>ride object v2</b> — the durable per-ride file that <i>is</i> the BLE wire object, so a ride download is a verbatim copy.</figcaption>
</figure>

**The track log (`.obct`) — a headerless record array.** While you ride, the device appends one fixed 20-byte record per accepted GPS fix. There is **no header**: the file is just the array, so truncating it to any 20-byte boundary is always valid, and the worst a power-loss can cost is the one in-flight record. That headerlessness is exactly why there's no in-band version byte to tell a 20-byte (v2) log from an old 16-byte one — so the upgrade guard is *structural* instead. The log is only ever converted through an in-RAM handle set by **this boot's** Finish; that handle can't survive a reboot, and the next ride's start opens the temp file truncating — so an orphaned 16-byte log left by older firmware can never reach the converter to be misparsed as 20-byte records. Boot provably discards it, so no versioned temp filename is needed.

**The ride object (`RD{id}.ORD`) — what the phone downloads.** At Finish the log is converted into the durable per-ride file that *is* the BLE wire object, so a ride download is a verbatim byte copy with no re-encode. It is **not** OBCR: coordinates are stored as **degrees × 1e7** in `lat, lon` order (the layout the companion app pins — the extra digit over OBCR's microdegrees buys a ~1 cm grid for nothing), and the header carries precomputed totals — distance, moving time, average speed, climb — plus a UTF-8 name.

**v1 and v2 coexist.** Version 2 is a pure **additive** widening for sensor data: the header grows an 8-byte summary — `avg_hr` / `max_hr` / `avg_cad` (+ a reserved pad) / `avg_pwr` / `max_pwr`, each sentinel-marked — and each point record grows the same `hr u8 · cad u8 · pwr u16` tail as the track log. The byte length stays **fully determined per version** — v1 is `23 + name_len + 14 × points`, v2 is `31 + name_len + 18 × points` — so a reader takes the version byte, then rejects any payload whose length disagrees for that version. That length check is also the torn-write guard: an interrupted save leaves a short file, and the version byte is written **last** as the commit point, so a half-written object is rejected rather than mistaken for a ride. A device that has never seen a sensor keeps writing v1, and **both firmware and app accept either** — old v1 rides on the card still list, download, and delete. Because it's an additive object version, there's **no `protocolVersion` bump**.

The exhaustive byte tables — every header and point field in both versions — are the normative [BLE interface spec §7.2](src:specs/obc-ble-interface-spec.md); this is the readable tour. The ride object crosses to a phone as [an object on the companion link](../companion-link/#objects-are-files-the-device-already-speaks), where the [Sensors section](../companion-link/#sensors-the-device-as-ble-central) covers how those sensor values were captured in the first place.

## Streaming: resident vs on-demand

Both formats are read through one trait. Neither reader touches a filesystem directly — they ask a [`ByteSource`](src:firmware/obc-formats/src/io.rs) for bytes at an offset:

```rust
pub trait ByteSource {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), Error>;
    fn len(&self) -> u32;
}
```

On the host that's a slice of memory; on the device it's a file on the SD card. The reader holds a `&dyn ByteSource` and stays monomorphic, so the genericity never leaks into the renderer or the screen stack — it's [one of the project's four seams](../architecture/#two-hosts-one-core-and-the-seams-between-them). What changes between the two formats is *how much* they keep resident.

<figure class="fig">
<svg viewBox="0 0 720 270" role="img" aria-label="A large map file on the SD card, much bigger than RAM. When the file opens, only the small header, style table and LOD table are read resident; the quadtree index and geometry chunks are pulled on demand through small caches. A note contrasts the route, which keeps its whole small index resident and streams only geometry.">
  <defs>
    <marker id="aF4" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">What stays in RAM, what streams from the card</text>

  <!-- file on card -->
  <rect class="d-panel-2" x="36" y="48" width="128" height="180" rx="10" />
  <text class="d-tag" x="52" y="68">.obcm on SD</text>
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
  <text class="d-sub" x="340" y="162">index nodes → 512 B block cache</text>
  <text class="d-sub" x="340" y="178">geometry chunks → 4 KB slot cache</text>

  <!-- route contrast -->
  <rect class="d-panel-2" x="324" y="200" width="360" height="40" rx="10" />
  <text class="d-sub" x="340" y="218" style="font-size:10px"><tspan style="fill:#a9501c">OBCR:</tspan> header + the whole (small, flat) index resident;</text>
  <text class="d-sub" x="340" y="232" style="font-size:10px">only geometry chunks stream. The list is cheap to keep.</text>
</svg>
<figcaption>A map never has to fit in RAM — the device has 512 KB and no external memory to hold the whole file. Even the quadtree index streams, through a small block cache that coalesces the 4-byte node reads; geometry chunks stream through a slot cache too, and the renderer touches each visible chunk at most twice a frame (once to pick features, once to draw the survivors) so the SD reads stay bounded. The route's index is a short flat list, so it's read whole at open; its geometry streams chunk-by-chunk through a small resident cache of its own, so a redraw of the same route re-reads nothing either.</figcaption>
</figure>

The map's caches matter because the [stub-select collector](../rendering/#4-decode-by-priority) walks the same visible chunks twice per frame — once in pass A to pick the surviving features, once in pass B to re-decode the winners; without a cache, pass B would re-read every winner chunk off the SD. With it, pass B's winner chunks are already resident, and a slow pan re-hits last frame's chunks. The cache changes *when* a byte is read, never *what* decodes — so a render stays byte-identical whether the whole file was resident or streamed one chunk at a time.

## The catalog — a third format, for finding the first two

Everything above is read by the device. There is one more format in the family that the device never sees, and it exists because of a single number in the OBCM header: the **version byte**.

The reader supports exactly one OBCM version at a time — v11 today, and the versions before it were hard cuts, not fallbacks. That's the right trade for a microcontroller (no branching parsers, no dead code paths in 512 KB), and it costs nothing while maps are built one at a time on the rider's own machine. But a distribution that **bakes maps centrally** — build the popular regions once, serve them as static files — turns that one byte into a hazard: the artifacts are large, cached at the edge, and the day the format moves, every one of them silently becomes a file the device will refuse.

The fix is to make the version *knowable before the download*. **OBCC** — the [catalog manifest](src:specs/OBCC_Spec.md) — is a single JSON document listing every baked artifact with its region, preset, coverage box, size, digest, and the OBCM version **read out of the artifact's own header** rather than taken from the build recipe. The other half of the comparison comes from the device itself: it reports the OBCM version its reader accepts in the same open, pre-pairing identity read that carries its [store epoch](../companion-link/#store-epochs-which-id-era-youre-talking-to) — one byte, taken straight from the constant the reader validates every header against, so what a device claims to read and what it does read can't drift apart. A site holding the manifest can then grey out a map the connected device can't read, instead of streaming two hundred megabytes and failing at the last byte.

<figure class="fig">
<svg viewBox="0 0 720 232" role="img" aria-label="The catalog flow. A bake job packs regions into .obcm artifacts and generates the OBCC manifest, recording each artifact's obcm version read from its own header plus its size, digest and coverage box. A static site reads the manifest. A connected device reports the obcm version its reader accepts in its open identity read. The site compares the two versions per map and either offers the download or greys it out with the reason.">
  <defs>
    <marker id="aCC" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
    <marker id="aCC2" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Both versions known before a byte moves</text>

  <!-- bakery -->
  <rect class="d-panel-2" x="24" y="60" width="140" height="56" rx="10" />
  <text class="d-label" x="94" y="84" text-anchor="middle">bake job</text>
  <text class="d-sub" x="94" y="100" text-anchor="middle">packs regions</text>

  <!-- manifest -->
  <line class="d-flow" x1="164" y1="88" x2="204" y2="88" marker-end="url(#aCC)" />
  <rect class="d-panel" x="210" y="48" width="190" height="80" rx="10" />
  <text class="d-label" x="305" y="70" text-anchor="middle">OBCC manifest</text>
  <text class="d-sub" x="305" y="88" text-anchor="middle" style="font-size:9px">per artifact: region · preset</text>
  <text class="d-sub" x="305" y="102" text-anchor="middle" style="font-size:9px">bbox · size · SHA-256</text>
  <text class="d-sub" x="305" y="118" text-anchor="middle" style="font-size:9px;fill:#a9501c">obcm_version — from the header</text>

  <!-- site -->
  <line class="d-flow" x1="400" y1="88" x2="440" y2="88" marker-end="url(#aCC)" />
  <rect class="d-hot" x="446" y="56" width="130" height="64" rx="11" style="fill:#f8efe4" />
  <text class="d-label" x="511" y="82" text-anchor="middle" style="fill:#a9501c">static site</text>
  <text class="d-sub" x="511" y="100" text-anchor="middle" style="font-size:9px">compares per map</text>

  <!-- device -->
  <rect class="d-panel-2" x="446" y="156" width="130" height="56" rx="10" />
  <text class="d-label" x="511" y="180" text-anchor="middle" style="font-size:10.5px">device</text>
  <text class="d-sub" x="511" y="196" text-anchor="middle" style="font-size:9px">reads OBCM v11</text>
  <line class="d-flow" x1="511" y1="154" x2="511" y2="122" marker-end="url(#aCC)" />
  <text class="d-sub" x="522" y="142" style="font-size:8.5px">identity read</text>

  <!-- outcomes -->
  <line class="d-flow" x1="576" y1="76" x2="612" y2="66" marker-end="url(#aCC)" />
  <text class="d-sub" x="620" y="62" style="font-size:9px;fill:#2c5230">match → offered,</text>
  <text class="d-sub" x="620" y="75" style="font-size:9px;fill:#2c5230">digest-verified</text>
  <line x1="576" y1="100" x2="612" y2="110" stroke="#cf6a2a" stroke-width="1.6" marker-end="url(#aCC2)" />
  <text class="d-sub" x="620" y="108" style="font-size:9px;fill:#c0492e">mismatch → greyed</text>
  <text class="d-sub" x="620" y="121" style="font-size:9px;fill:#c0492e">out, with reason</text>
</svg>
<figcaption>The manifest states each artifact's version as read from its own bytes; the device states the version its reader accepts in its open identity read. The site compares the two per map — a mismatch is shown as unsupported with the reason, never hidden (that would read as a coverage hole) and never offered (the download would be refused).</figcaption>
</figure>

The generator keeps the manifest honest in a few deliberate ways:

- **All-or-nothing versioning.** It refuses to emit a manifest whose artifacts aren't all at the packer's current OBCM version, so a half-re-baked catalog is a failed build rather than a published one; and it writes temp-file-then-rename, so a consumer sees the whole previous manifest or the whole new one — a truncated catalog can't parse.
- **The `bbox` is the coverage box** copied from the artifact header, not the extract box it was cut from — completing partially-in-box ways pushes the packed box outward, so the header box is the honest answer to *what does this download cover?* (and must never be fed back in as an extract box, or it ratchets wider on every re-pack).
- **Facts the bytes can't state** — region name, build time, source extract, the style-preset revision it was packed with — are recorded by the bake job in a sidecar and read back verbatim, never re-derived at generation time. A preset restyle invalidates only that preset's artifacts, so partial re-bakes are normal; recording the preset revision at bake time lets the manifest say plainly that a map is a revision behind.
- **Deterministic output.** The generator reads a wall clock exactly once (`generated_at`); build times come from the bake and ordering is content-derived, so the same tree produces byte-identical output.

The manifest layout, the bake-tree shape, the sidecar, and the version law are normative in [`OBCC_Spec.md`](src:specs/OBCC_Spec.md); it is generated by `obc-pack catalog`.

The "bake job" in all of that is [`obc-bake`](src:host/obc-bake): a curated list of regions ([`regions.toml`](src:host/obc-bake/regions.toml) — DACH to start, one line per region so adding coverage is a one-line change) crossed with the shipped style presets. Per (region, preset) it fetches or reuses the Geofabrik extract, packs, and — before the artifact is allowed to exist under a name the generator would walk — **opens it with the real `obc-reader` and walks every feature of every chunk**. A corrupt or truncated artifact therefore never reaches the manifest, because it never reaches the tree; a re-run skips whatever is unchanged, keyed on the content hashes of the extract and the preset rather than on timestamps; and a region that fails is loud in the summary and in the exit status, because a silently missing region reads to a rider as *not covered*, which is indistinguishable from a curation choice. (The facts that live only in the *sidecar* — a region's display name, the extract's date — are keyed separately, so re-dating an otherwise identical extract rewrites four lines of JSON instead of re-packing a country.)

Publishing follows the same one-way order the spec demands: every artifact uploaded and re-checked at the destination first, the manifest swapped in last. It also refuses, by default, to publish a catalog *smaller* than the one already live — ordering makes a publish atomic, but nothing about it stops a partial bake from completing successfully and quietly un-offering fifteen regions, which is the same coverage hole arriving by a different road.

## Cells and assemblies — the shape the catalog is moving to

*Designed and specified, not yet built: this section describes where the catalog is going, and the contract it will follow. The catalog on the site today is the region × preset one above.*

The catalog above has a shape problem that no amount of care fixes. A bakery can only offer what it pre-baked, so **Germany + Switzerland in one file** does not exist unless somebody baked that exact pair — and pre-baking every plausible combination of neighbours explodes the store. It already pays double by design (Germany sits alongside its sixteen Bundesländer, covering the same ground twice), and every new style preset multiplies the whole pile again.

The fix is to change the **unit of baking** from a region to a **grid cell**, and to make the map a rider downloads an **assembly** of cells built on their own machine. N cells cover the coverage area exactly once; every selection — a country, two countries, a drawn box, a corridor buffered around a trip's routes — becomes an assembly rather than a bake. Combinations cost bandwidth and stop costing storage.

That only works if assembling is nearly free, and the reason it is nearly free is a piece of arithmetic already sitting in the format.

### The alignment trick

Recall two facts from further up this page: an OBCM [quadtree](#the-quadtree-index) subdivides its header bbox at **integer floor-midpoints**, and a feature's [anchor](#features-an-anchor-then-deltas) is stored relative to its **leaf's** minimum corner. Now put cells on a fixed global lattice of **power-of-two microdegree squares** sharing one origin, and give an assembly a bbox that is a grid-aligned power-of-two square.

Halving a power of two is exact, so the assembly's quadtree lands on cell boundaries *to the microdegree* — at one specific depth, the tree's nodes **are** the cells. And because anchors are leaf-relative and the leaf boxes are bit-identical in the cell file and in the assembly, every byte of a cell's chunks decodes to the same absolute geometry in both. The assembler copies chunk payloads **verbatim**.

<figure class="fig">
<svg viewBox="0 0 720 268" role="img" aria-label="The alignment trick. On the left, a fixed lattice of power-of-two microdegree cells with a selection drawn across four of them; each selected cell is a separately baked .obcm file. On the right, the assembled file's quadtree: a root over a grid-aligned power-of-two square, subdividing by floor midpoints until, at one depth, its nodes are exactly the cells. An arrow between them is labelled chunk bytes copied verbatim, while the header, style table, POIs and navigation graph are rebuilt.">
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
<figcaption>The cells are the quadtree's own nodes at one depth, so the assembler writes fresh index nodes above them and <b>copies</b> everything below. A missing cell is an empty leaf, which the renderer already paints backdrop over — so a selection with holes is legal by construction, not a special case. What it cannot copy is anything addressed absolutely or file-locally: POI coordinates, the hours pool's indices, and every id in the navigation graph.</figcaption>
</figure>

So assembly is a streaming concatenation plus a handful of rebuilds — no GEOS, no re-simplification, nothing decoded — which is exactly the budget a browser tab has. The [`OBCA_Spec.md`](src:specs/OBCA_Spec.md) §2 states this as a theorem and proves it in integer arithmetic, because "the midpoints happen to line up" is not something to leave to a comment.

### Seams that are correct, not close

Every cell edge is a border, so the navigation graph is where this gets interesting. A cell cuts routable ways at the **exact** edge and materialises a junction on the boundary line, computed the same way by both neighbours — canonically ordered endpoints, integer interpolation, banker's rounding — so the two cells produce the *same integer coordinate* and the assembler joins the stubs by exact equality.

Exact equality is not a simplification; it is the only safe rule, and that came out of measurement. Across two independently packed extracts, 21 144 junctions matched **bit-identically** and *nothing* fell in the 1 m / 10 m / 100 m near-miss buckets: the whole graph path is integer and deterministic, so a junction is either the same pair of integers or it is not there. But at a *cell* seam cut from one source, genuinely different junctions sit as close as **3.9 metres** — because a node is only a junction if two of the ways *that run saw* touch it. An epsilon snap of any useful size would fuse a bridge to the road beneath it. So there is no tolerance knob, and the spec says there must never be one.

The other seam rule is about what a cell is allowed to throw away. The packer drops tiny disconnected components as "islands", and at cell scale that judgement is wrong: a good road can look like a stub in *each* of two cells while being continuous once assembled. So a cell bake prunes only components **strictly interior** to it, and the real pruning pass runs at assembly, where component sizes are finally true.

### Schema and skin

A style preset today mixes two very different things. **Schema** — which feature types exist, at which LOD, the ladder, the simplification tolerances — is baked into chunk bytes, and in particular it fixes the [style *ids*](#the-header) every feature header references. **Skin** — the colours, weights, dashes, z-order, and the marker colour — is the ~2 KB style table and nothing else.

Cells are stored per schema; the hosted catalog has exactly **one** (the seven-LOD bikepacking ladder, the one measured to render inside the device's RAM and complexity budget), so the planet-shaped store exists once. Skins are then free: stamping one onto an assembly rewrites two kilobytes. That is what brings a style editor back to a browser with no server behind it — and it is also the reason a restyle can no longer leave an artifact a revision behind, because nothing is baked to be behind.

### One map, several files

The last piece is a ceiling. Germany alone projects to 5.8–7.1 GiB at this schema and DACH to 7.6–8.9 GiB, and **two independent 4 GiB walls** stand in the way: FAT32 caps one file at 4 GiB − 1, and OBCM's own offsets — section offsets, per-LOD chunk offset tables, and `Edge Id` as a pool byte offset — are `uint32` throughout, so a single `.obcm` cannot address past 4 GiB on *any* filesystem.

So a logical map is a **volume set**: a tiny fixed-layout manifest plus 1..N ordinary OBCM files, each inside the ceiling, and invisible in every interface — the device and the builder both show one map.

- **One core file** carries the style table, the single unified navigation graph, and the POIs — and *no map geometry whatsoever*. Routing therefore never crosses a file, and the router is untouched.
- **One coarse shard** spans the whole map and carries the three coarsest LODs, so a zoomed-out view is still a single-file read. It is a shard rather than part of the core for one reason, below.
- **Geometry shards** carry the finer LODs and tile the assembly bbox. They are cell-aligned squares, so each one is a *valid* OBCM map in its own right and `uint32` offsets stay valid per file — no 64-bit bump anywhere.
- **A viewport query goes to every shard whose box it touches.** A shard that does not carry the requested LOD has an empty index for it and contributes nothing, so the dispatch needs no notion of roles. Each file's "which LODs am I empty at" answer is cached at mount from its own LOD table — seven bits per file — so the role-free dispatch costs no reads either.
- **The manifest is written last.** A half-uploaded set has no manifest and never mounts — and a shard on its own is never mounted as a standalone map, even though it would open, because a map with no roads is exactly the kind of quiet wrongness a rider cannot diagnose.
- **A small map is a set of one**, which is nearly every selection: a country is under a gigabyte, a 300 km corridor around a trip's routes projects to about a quarter of one.

The reason the coarse LODs are a shard and not part of the core is that the core is the **one component of a set that cannot be split by box**. Every shard tiles: more ground means more shards, each one comfortably inside the ceiling. But the navigation graph is *one* graph, in one file, until the router learns to route across a seam — so the core's remaining headroom under 4 GiB − 1 is the scarcest number in the design, and nothing that has somewhere else to go may spend it. Coarse geometry has somewhere else to go.

What that leaves is a map whose limit is a single sentence: **one map reaches the ceiling when its navigation graph alone does.** Nav plus POIs measure 3.8–7.1 MiB per 1000 km², so a DACH core is 2.8–3.0 GiB, and the graph alone hits the wall at roughly 640–700 thousand km² — enough for DACH and its northern and eastern neighbours, not enough for DACH plus France. No geometry decision can move that number in either direction, which is exactly the property worth having.

And the limit is never met at runtime. Every file's size is *computable from the catalog before a single byte is downloaded* — the sum of the cell sizes it will carry plus fixed overheads — so a builder refuses an over-ceiling selection up front, naming the navigation graph, and the assembler and its verify pass reject one again before writing. Density growth degrades to a sentence in a dialog, never to a truncated offset on a card.

The measurement behind all of these numbers — where the bytes actually are, per LOD, per cell, at candidate cell sizes — is [`cell_size_survey.rs`](src:host/obc-pack/examples/cell_size_survey.rs), and it is what settled the band sizes and the DACH shape above: a core of 2.8–3.0 GiB, one coarse shard of 225–296 MiB, and about six geometry shards holding 4.6–5.5 GiB.

The grid, the theorem, the seam rules, the assembly contract, the volume-set manifest bytes, and the provenance rule that stops a partially-covered border cell from passing as canonical are all normative in [`OBCA_Spec.md`](src:specs/OBCA_Spec.md); the catalog that publishes cells, skins, and cell-set regions is [`OBCC_Spec.md` §11](src:specs/OBCC_Spec.md).

---

## Where this lives

- Map reader, quadtree walk, chunk decode, the POI nearest-16 query, and the nav directory / node-leaf walk / edge fetch: [`obc-reader/src/reader.rs`](src:firmware/obc-reader/src/reader.rs)
- The canonical POI category/subtype ids and fallback labels (shared by reader + packer): [`obc-formats/src/obcm.rs`](src:firmware/obc-formats/src/obcm.rs); the packer's OSM-tag classifier stays in [`obc-pack/src/poi.rs`](src:host/obc-pack/src/poi.rs)
- The route-corridor POI query, its `RoutePath` seam and the projection maths: [`obc-reader/src/corridor.rs`](src:firmware/obc-reader/src/corridor.rs)
- Route reader, index, and decode: [`obc-route/src/reader.rs`](src:firmware/obc-route/src/reader.rs); the GPX `<sym>`/`<type>` → category table: [`obc-route/src/symbol.rs`](src:firmware/obc-route/src/symbol.rs)
- The recorded-track log + its GPX export: [`obc-route/src/track.rs`](src:firmware/obc-route/src/track.rs); the ride object (v1/v2) codec + the Finish-time converter: [`obc-route/src/ride.rs`](src:firmware/obc-route/src/ride.rs)
- The browser's copy of both converters — a thin wasm shim over the same routines, plus the error vocabulary a dropped file needs: [`obc-web-convert`](src:apps/obc-web-convert)
- Checked-in bytes both directions are held to (a route and its OBCR, a track log and its GPX export): [`specs/vectors/`](src:specs/vectors)
- Normative OBCM / OBCR / ride / track constants, primitive codecs, and the shared byte seam: [`obc-formats`](src:firmware/obc-formats)
- The byte-level specs: [`OBCM_Spec.md`](src:specs/OBCM_Spec.md) · [`OBCR_Spec.md`](src:specs/OBCR_Spec.md) · [`obc-ble-interface-spec.md`](src:specs/obc-ble-interface-spec.md) (the wire contract routes/rides cross to the companion app)
- The catalog manifest — spec [`OBCC_Spec.md`](src:specs/OBCC_Spec.md), generator [`obc-pack/src/catalog.rs`](src:host/obc-pack/src/catalog.rs), JSON Schema [`catalog.schema.json`](src:host/obc-pack/schema/catalog.schema.json)
- The cell catalog the section above describes — the `schema_version 2` producer [`catalog/v2.rs`](src:host/obc-pack/src/catalog/v2.rs), the region-outline reduction [`catalog/boundary.rs`](src:host/obc-pack/src/catalog/boundary.rs), JSON Schema [`catalog.v2.schema.json`](src:host/obc-pack/schema/catalog.v2.schema.json)
- The cell grid, the assembly contract, and the volume-set manifest: [`OBCA_Spec.md`](src:specs/OBCA_Spec.md); the byte-density measurement its band sizes come from: [`cell_size_survey.rs`](src:host/obc-pack/examples/cell_size_survey.rs)
- The cell cutter — the grid arithmetic, cell ids and band table in [`obc-pack/src/grid.rs`](src:host/obc-pack/src/grid.rs), the cut itself (clip at the edge, the deterministic boundary junctions, interior-only pruning, provenance) in [`obc-pack/src/cut.rs`](src:host/obc-pack/src/cut.rs)
- The assembler that puts the cells back together — [`obcm-assemble`](src:host/obcm-assemble): the verbatim graft in [`graft.rs`](src:host/obcm-assemble/src/graft.rs), the POI/hours merge in [`poi.rs`](src:host/obcm-assemble/src/poi.rs), the seam unification and graph rewrite in [`nav.rs`](src:host/obcm-assemble/src/nav.rs), the volume set and its manifest in [`shard.rs`](src:host/obcm-assemble/src/shard.rs), and the read-it-back gate in [`verify.rs`](src:host/obcm-assemble/src/verify.rs). It carries no geometry library and compiles for the browser, which is the whole point. The proof that a grafted map is the map: [`tests/oracle.rs`](src:host/obcm-assemble/tests/oracle.rs) renders and routes `assemble(cut(X))` against `pack(X)`
- The bakery that fills the tree the generator walks, and publishes it — the curated region list [`regions.toml`](src:host/obc-bake/regions.toml), the runner [`obc-bake/src/bake.rs`](src:host/obc-bake/src/bake.rs), the read-it-back gate [`obc-bake/src/verify.rs`](src:host/obc-bake/src/verify.rs), the ordered publish [`obc-bake/src/publish.rs`](src:host/obc-bake/src/publish.rs)

Maps are produced by the packer and routes by the GPX converter — how those work, and how a route is matched to the map you're riding, is the subject of [packer & routing](../packer-routing/). For how these bytes become pixels, see the [rendering pipeline](../rendering/). Routes and rides also cross to a phone over Bluetooth as *these same bytes* — how that link is shaped is [the companion link](../companion-link/).
