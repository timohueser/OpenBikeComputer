---
title: Data formats
description: OBCM maps and OBCR routes — the binary, table-driven formats a microcontroller reads directly off flash, a chunk at a time, with no JSON, no reparsing, and no heap.
---

# Data formats

The device reads two kinds of file: an **OBCM** map and an **OBCR** route. Both are binary, and both exist for the same reason — a microcontroller should read them *directly off flash*, with no JSON to parse, no structure to rebuild in RAM, and no heap to churn. A host produces them once; the device just points at the bytes and draws.

This page is the guided tour of what's actually in those files. The exhaustive byte-level tables live in the repo specs — [`OBCM_Spec.md`](src:OBCM_Spec.md) and [`OBCR_Spec.md`](src:OBCR_Spec.md) — so here we focus on *why* the bytes are shaped the way they are.

## Two binaries, one philosophy

The map and the route are siblings: they were designed to feel identical to the code that reads them, so the renderer can treat a route chunk and a map chunk with the same instincts.

<figure class="fig">
<svg viewBox="0 0 720 290" role="img" aria-label="Two production pipelines. An OSM extract is turned into a dot-obcm map file offline by the obc-pack packer; a GPX upload is turned into a dot-obcr route file on the device by obc-route. Both files are read back by the same no_std reader code on the simulator and the device. They share a common design: little-endian, microdegree integers, anchor-plus-delta geometry, explicit offsets, and streaming.">
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
  <text class="d-sub" x="234" y="182" text-anchor="middle" style="fill:#a9501c">obc-route · on device</text>
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
<figcaption>The map is baked <b>offline</b> from an OpenStreetMap extract by the packer; the route is converted <b>on the device</b> (or in the simulator) from an uploaded GPX. Different origins, but the very same <code>no_std</code> reader code parses them on both machines — so what you draw in the browser is what the device draws.</figcaption>
</figure>

Four principles run through both formats:

- **Binary and table-driven.** Numbers, not text. Colours and widths live in a small style table the map references by id; geometry is raw integers. Nothing is parsed from strings at read time.
- **Integer microdegrees.** Every coordinate is an `i32` in millionths of a degree. There are no floats on disk and no projection baked in — turning ground coordinates into pixels is the [renderer's](../rendering/) job, not the file's.
- **No runtime discovery.** Every section is reached through an explicit byte offset, and every count is stored. A `no_std` reader does *zero* traversal or sizing work to understand the file's structure — it reads a header and jumps.
- **Streamed, never resident.** Both files are read through a [`ByteSource`](src:firmware/obc-reader/src/byte_io.rs) a piece at a time, so a map far larger than RAM — or a route hundreds of kilometres long — never has to fit in memory at once.

Where they differ is *shape*: a map is a 2-D area indexed by a quadtree; a route is a 1-D path indexed by a flat list. Everything below follows from that.

## OBCM — the map

### The file, front to back

An OBCM file (current version **5**) opens with a fixed 32-byte header, then a global style table and a level-of-detail (LOD) table, then the LOD layers themselves — coarsest first. Each LOD layer is wholly self-contained: its own quadtree index immediately followed by its own geometry chunks.

<figure class="fig">
<svg viewBox="0 0 720 210" role="img" aria-label="The OBCM file as a horizontal ribbon: a 32-byte header, a global style table, an LOD table, then LOD layer 0 (coarsest) through LOD layer N minus 1 (finest). Detail increases left to right across the LOD layers. One LOD layer is exploded below to show it is a quadtree index followed by data chunks.">
  <defs>
    <marker id="aF2" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">OBCM — the whole file, front to back</text>

  <!-- ribbon -->
  <g stroke="#3c6b39" stroke-width="1.4">
    <rect x="24"  y="56" width="64"  height="44" class="d-forest" />
    <rect x="88"  y="56" width="92"  height="44" class="d-amber" />
    <rect x="180" y="56" width="92"  height="44" class="d-water" />
    <rect x="272" y="56" width="132" height="44" class="d-muted" />
    <rect x="404" y="56" width="116" height="44" class="d-muted" />
    <rect x="520" y="56" width="176" height="44" class="d-muted" />
  </g>
  <text class="d-label" x="56"  y="80" text-anchor="middle" style="fill:#fff">Header</text>
  <text class="d-sub"   x="56"  y="94" text-anchor="middle" style="fill:#e7ead8">32 B</text>
  <text class="d-label" x="134" y="80" text-anchor="middle">Style table</text>
  <text class="d-sub"   x="134" y="94" text-anchor="middle">global</text>
  <text class="d-label" x="226" y="80" text-anchor="middle" style="fill:#fff">LOD table</text>
  <text class="d-sub"   x="226" y="94" text-anchor="middle" style="fill:#dfe6e0">N × 18 B</text>
  <text class="d-label" x="338" y="80" text-anchor="middle">LOD 0</text>
  <text class="d-sub"   x="338" y="94" text-anchor="middle">coarsest</text>
  <text class="d-label" x="462" y="80" text-anchor="middle">LOD 1</text>
  <text class="d-label" x="608" y="80" text-anchor="middle">LOD N−1</text>
  <text class="d-sub"   x="608" y="94" text-anchor="middle">finest</text>

  <!-- detail arrow -->
  <line x1="276" y1="114" x2="692" y2="114" stroke="#cf6a2a" stroke-width="1.6" marker-end="url(#aF2)" />
  <text class="d-sub" x="484" y="128" text-anchor="middle" style="fill:#a9501c">detail increases →</text>

  <!-- explode LOD 0 -->
  <line x1="272" y1="100" x2="232" y2="152" stroke="#9aa884" stroke-width="1.2" />
  <line x1="404" y1="100" x2="544" y2="152" stroke="#9aa884" stroke-width="1.2" />
  <rect class="d-panel-2" x="232" y="152" width="160" height="40" rx="7" />
  <text class="d-label" x="312" y="170" text-anchor="middle">quadtree index</text>
  <text class="d-sub"   x="312" y="184" text-anchor="middle">flat u32 nodes</text>
  <rect class="d-panel" x="392" y="152" width="152" height="40" rx="7" />
  <text class="d-label" x="468" y="170" text-anchor="middle">data chunks</text>
  <text class="d-sub"   x="468" y="184" text-anchor="middle">fixed-size blocks</text>
</svg>
<figcaption>The header, style table and LOD table are read once when the file opens — they're tiny. Everything after is the LOD pyramid: each layer its own <b>(index + chunks)</b> pair, simplified to that zoom. Reaching any section is an explicit offset, so there is no scanning to "find" where a layer begins.</figcaption>
</figure>

Why a pyramid, rather than one detailed tree with a min-zoom tag on every feature? Because the latter forces the device to *decode* fine geometry just to discover it should be skipped when zoomed out. With independent layers, zooming out reads a small coarse layer and touches nothing else. The renderer's job of [picking the right layer](../rendering/#2-level-of-detail-pick-the-right-layer) for the current zoom is covered on the rendering page; here we only care that the layers exist side by side in the file.

Each entry in the **LOD table** is the directory to one layer — the zoom it serves and where its bytes are:

| Field | Type | What it is |
| :-- | :-- | :-- |
| Max meters/pixel | `f32` | Upper bound of the zoom range this layer covers; the coarsest is `+∞`, strictly decreasing toward fine |
| Index offset | `u32` | Byte offset to this layer's quadtree |
| Node count | `u32` | Number of `u32` nodes in that index |
| Chunk size | `u16` | Fixed byte size of every data chunk in this layer |
| Chunk count | `u32` | Number of data chunks |

Eighteen bytes per entry — the `N × 18 B` in the ribbon above. Because the index sits immediately before the chunks and every count is stored, the *k*-th chunk is reached by arithmetic alone — `index_offset + node_count·4 + k·chunk_size` — with no scanning and no length-prefix hunting. That's "no runtime discovery" made concrete: the table tells the reader exactly where every layer, and every chunk within it, begins.

### The header

The 32-byte header is the one fixed-size, always-present part of the file. Everything else is found through offsets it stores.

<figure class="fig">
<svg viewBox="0 0 720 170" role="img" aria-label="The 32-byte OBCM header drawn as a byte ruler: bytes 0 to 3 are the magic OBCM, byte 4 is the version, bytes 5 to 20 are the global bounding box as four 32-bit integers, bytes 21 to 24 are the style-table offset, byte 25 is the LOD count, bytes 26 to 29 are the LOD-table offset, and bytes 30 to 31 are the marker colour.">
  <text class="d-tag" x="20" y="24">The 32-byte header, byte by byte</text>

  <!-- field names -->
  <text class="d-sub" x="88"  y="56" text-anchor="middle">Magic</text>
  <text class="d-sub" x="138" y="56" text-anchor="middle" style="font-size:9px">ver</text>
  <text class="d-sub" x="308" y="50" text-anchor="middle">global bbox</text>
  <text class="d-sub" x="308" y="62" text-anchor="middle" style="font-size:9px">4 × i32 · µdeg</text>
  <text class="d-sub" x="508" y="56" text-anchor="middle">style off</text>
  <text class="d-sub" x="558" y="56" text-anchor="middle" style="font-size:9px">n</text>
  <text class="d-sub" x="608" y="56" text-anchor="middle">LOD-tbl off</text>
  <text class="d-sub" x="668" y="56" text-anchor="middle">marker</text>

  <!-- ruler fields -->
  <g stroke="#20301d" stroke-width="1">
    <rect x="48"  y="72" width="80" height="32" class="d-forest" />
    <rect x="128" y="72" width="20" height="32" class="d-amber" />
    <rect x="148" y="72" width="320" height="32" class="d-water" />
    <rect x="468" y="72" width="80" height="32" class="d-muted" />
    <rect x="548" y="72" width="20" height="32" class="d-amber" />
    <rect x="568" y="72" width="80" height="32" class="d-muted" />
    <rect x="648" y="72" width="40" height="32" class="d-hot-fill" />
  </g>
  <!-- per-byte ticks -->
  <g stroke="#20301d" stroke-opacity="0.18" stroke-width="1">
    <line x1="68" y1="72" x2="68" y2="104"/><line x1="88" y1="72" x2="88" y2="104"/><line x1="108" y1="72" x2="108" y2="104"/>
    <line x1="168" y1="72" x2="168" y2="104"/><line x1="188" y1="72" x2="188" y2="104"/><line x1="208" y1="72" x2="208" y2="104"/><line x1="228" y1="72" x2="228" y2="104"/><line x1="248" y1="72" x2="248" y2="104"/><line x1="268" y1="72" x2="268" y2="104"/><line x1="288" y1="72" x2="288" y2="104"/><line x1="308" y1="72" x2="308" y2="104"/><line x1="328" y1="72" x2="328" y2="104"/><line x1="348" y1="72" x2="348" y2="104"/><line x1="368" y1="72" x2="368" y2="104"/><line x1="388" y1="72" x2="388" y2="104"/><line x1="408" y1="72" x2="408" y2="104"/><line x1="428" y1="72" x2="428" y2="104"/><line x1="448" y1="72" x2="448" y2="104"/>
    <line x1="488" y1="72" x2="488" y2="104"/><line x1="508" y1="72" x2="508" y2="104"/><line x1="528" y1="72" x2="528" y2="104"/>
    <line x1="588" y1="72" x2="588" y2="104"/><line x1="608" y1="72" x2="608" y2="104"/><line x1="628" y1="72" x2="628" y2="104"/>
    <line x1="668" y1="72" x2="668" y2="104"/>
  </g>
  <!-- value + byte ranges -->
  <text class="d-label" x="88" y="93" text-anchor="middle" style="fill:#fff;font-size:11px">OBCM</text>
  <text class="d-sub" x="88"  y="122" text-anchor="middle" style="font-size:9px">0–3</text>
  <text class="d-sub" x="138" y="122" text-anchor="middle" style="font-size:9px">4</text>
  <text class="d-sub" x="308" y="122" text-anchor="middle" style="font-size:9px">5–20</text>
  <text class="d-sub" x="508" y="122" text-anchor="middle" style="font-size:9px">21–24</text>
  <text class="d-sub" x="558" y="122" text-anchor="middle" style="font-size:9px">25</text>
  <text class="d-sub" x="608" y="122" text-anchor="middle" style="font-size:9px">26–29</text>
  <text class="d-sub" x="668" y="122" text-anchor="middle" style="font-size:9px">30–31</text>

  <text class="d-sub" x="48" y="150" style="font-size:11px">A short read here is the only "is this even a map?" check the reader needs.</text>
</svg>
<figcaption>Fixed offsets, no surprises. Two small details a reader notices: the bbox is stored <b>lat, lon</b> (a packer ordering quirk), and the <b>marker colour</b> — the you-are-here chevron — rides in the header rather than the style table, because the marker isn't an OpenStreetMap feature. It's RGB565 like every style colour and is quantised to the panel the same way.</figcaption>
</figure>

The **style table** that follows maps small numeric ids to how a feature looks. Each record is six bytes:

```rust
pub struct Style {
    pub id: u8,        // referenced by feature headers
    pub z_index: i8,   // painter's order: lower draws first
    pub color: u16,    // RGB565 — device-independent
    pub weight: u8,    // stroke width in pixels (lines)
    pub priority: u8,  // 1 = keep first … 4 = drop first (from a flags byte)
}
```

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

Decoding a ring is exactly as simple as it looks — pick the delta width once, then walk:

```rust
let (dx, dy) = if is_16 {
    (rd_i16(chunk, off) as i32, rd_i16(chunk, off + 2) as i32) // flag bit 0 set
} else {
    (chunk[off] as i8 as i32, chunk[off + 1] as i8 as i32)     // 8-bit — the common case
};
px += dx;  py += dy;   // each delta steps to the next vertex
```

A feature is introduced by a 12-byte header, and a flags byte in it says how to read the rest:

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="A feature header drawn as a byte ruler: one byte style id, two bytes exterior point count, four bytes anchor X, four bytes anchor Y, one byte flags. The flags byte expands into three bits: 16-bit deltas, polygon, and has-holes. Below, the polygon-with-holes byte layout as a ribbon: the header, the exterior deltas, a hole count, then each hole's point count and deltas.">
  <text class="d-tag" x="20" y="24">A feature on disk</text>

  <!-- header ruler -->
  <text class="d-sub" x="140" y="54" text-anchor="middle" style="font-size:9.5px">style id</text>
  <text class="d-sub" x="200" y="54" text-anchor="middle" style="font-size:9.5px">pt count</text>
  <text class="d-sub" x="320" y="54" text-anchor="middle" style="font-size:9.5px">anchor X (i32)</text>
  <text class="d-sub" x="480" y="54" text-anchor="middle" style="font-size:9.5px">anchor Y (i32)</text>
  <text class="d-sub" x="580" y="54" text-anchor="middle" style="font-size:9.5px">flags</text>
  <g stroke="#20301d" stroke-width="1">
    <rect x="120" y="62" width="40"  height="30" class="d-forest" />
    <rect x="160" y="62" width="80"  height="30" class="d-water" />
    <rect x="240" y="62" width="160" height="30" class="d-muted" />
    <rect x="400" y="62" width="160" height="30" class="d-muted" />
    <rect x="560" y="62" width="40"  height="30" class="d-hot-fill" />
  </g>
  <text class="d-sub" x="140" y="106" text-anchor="middle" style="font-size:9px">1 B</text>
  <text class="d-sub" x="200" y="106" text-anchor="middle" style="font-size:9px">2 B</text>
  <text class="d-sub" x="320" y="106" text-anchor="middle" style="font-size:9px">4 B</text>
  <text class="d-sub" x="480" y="106" text-anchor="middle" style="font-size:9px">4 B</text>
  <text class="d-sub" x="580" y="106" text-anchor="middle" style="font-size:9px">1 B</text>

  <!-- flags expand -->
  <line x1="580" y1="92" x2="580" y2="124" stroke="#cf6a2a" stroke-width="1.2" />
  <g>
    <rect x="436" y="124" width="100" height="22" rx="4" class="d-panel-2" />
    <text class="d-sub" x="486" y="139" text-anchor="middle" style="font-size:9.5px">bit 0 · 16-bit Δ</text>
    <rect x="540" y="124" width="80" height="22" rx="4" class="d-panel-2" />
    <text class="d-sub" x="580" y="139" text-anchor="middle" style="font-size:9.5px">bit 1 · polygon</text>
    <rect x="624" y="124" width="72" height="22" rx="4" class="d-panel-2" />
    <text class="d-sub" x="660" y="139" text-anchor="middle" style="font-size:9.5px">bit 2 · holes</text>
  </g>

  <!-- holes layout ribbon -->
  <text class="d-tag" x="20" y="178">…and a polygon with holes, laid out</text>
  <g stroke="#3c6b39" stroke-width="1.2">
    <rect x="24"  y="188" width="96"  height="34" class="d-hot-fill" />
    <rect x="120" y="188" width="150" height="34" class="d-muted" />
    <rect x="270" y="188" width="70"  height="34" class="d-amber" />
    <rect x="340" y="188" width="64"  height="34" class="d-water" />
    <rect x="404" y="188" width="130" height="34" class="d-muted" />
    <rect x="534" y="188" width="64"  height="34" class="d-water" />
    <rect x="598" y="188" width="98"  height="34" class="d-muted" />
  </g>
  <text class="d-sub" x="72"  y="209" text-anchor="middle" style="fill:#fff;font-size:9.5px">12 B header</text>
  <text class="d-sub" x="195" y="209" text-anchor="middle" style="font-size:9.5px">exterior deltas</text>
  <text class="d-sub" x="305" y="209" text-anchor="middle" style="fill:#3a2c10;font-size:9px">hole cnt</text>
  <text class="d-sub" x="372" y="209" text-anchor="middle" style="fill:#fff;font-size:9px">h1 pts</text>
  <text class="d-sub" x="469" y="209" text-anchor="middle" style="font-size:9.5px">hole 1 deltas</text>
  <text class="d-sub" x="566" y="209" text-anchor="middle" style="fill:#fff;font-size:9px">h2 pts</text>
  <text class="d-sub" x="647" y="209" text-anchor="middle" style="font-size:9.5px">hole 2 …</text>
</svg>
<figcaption>The exterior ring comes first; the hole count and each hole's deltas follow <b>only if</b> the holes flag is set, so a line or a simple polygon pays nothing for machinery it doesn't use. A <code>0xFF</code> style id — an impossible style — marks the end of features in a chunk, so the reader stops without needing a per-chunk feature count.</figcaption>
</figure>

There's a quiet payoff to the holes layout: a polygon's holes are just extra rings appended after the exterior. The [scanline fill](../rendering/#polygons-even-odd-scanline-fill) treats them as additional edges in the same crossing list, so holes "fall out" of the even-odd rule with no special case — the format and the rasteriser were designed to meet in the middle.

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
  <text class="d-sub"   x="652" y="94" text-anchor="middle">W × 40 B</text>

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

The header carries the route's bounding box, its start point (for centering the camera), and the **precomputed totals** — distance, ascent, descent, elevation range — plus the route name; format v2 appends a small extension pointing at the **waypoint table**: fixed 40-byte records (position along the route, coordinate, category, short name) for the points of interest a planner attaches to a route. The device stores waypoints from day one but doesn't render them yet — a reader that ignores them skips the section in O(1) by construction, which is also why v2 routes ride through unchanged v1 code. A 44-byte index entry per chunk holds that chunk's bounding box (for the viewport query), its anchor, its point count, the **cumulative distance and ascent at its first point**, and where its bytes live.

Those cumulative stats are the trick that makes "42 km / 600 m to go" an O(1) subtraction once you know which segment you're on, rather than a walk over the whole route every frame.

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
  <text class="d-sub" x="150" y="118" text-anchor="middle" style="fill:#a9501c;font-size:9px">shared</text>
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

> **Convert where it lands.** There is no offline route step. The GPX→OBCR converter is one portable `no_std` routine: the device runs it on a USB upload, the simulator runs it on import, and both produce the same bytes. It streams the GPX in a single pass — O(1) RAM regardless of route length — emitting each finished chunk while keeping only a bounded index in memory. (BLE uploads arrive **already converted**: the companion app encodes imported GPX/TCX to OBCR on the phone, per the [BLE interface spec](src:obc-ble-interface-spec.md), so the device just writes the bytes to storage.)

One thing the file *doesn't* store is the elevation **profile** the Statistics screen draws. That's rebuilt once when a route loads — a multi-resolution min/max pyramid over distance, the same coarse-to-fine idea as the map's LODs, so the profile can be zoomed and panned without ever re-reading geometry. It's a runtime structure rather than a format concern; the [UI page](../ui/) covers how it's drawn.

## Streaming: resident vs on-demand

Both formats are read through one trait. Neither reader touches a filesystem directly — they ask a [`ByteSource`](src:firmware/obc-reader/src/byte_io.rs) for bytes at an offset:

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
<figcaption>A map never has to fit in RAM — the device has 512 KB and no external memory to hold the whole file. Even the quadtree index streams, through a small block cache that coalesces the 4-byte node reads; geometry chunks ride a slot cache so the renderer's four priority passes re-read nothing. The route's index is a short flat list, so it's read whole at open; its geometry streams chunk-by-chunk through a small resident cache of its own, so a redraw of the same route re-reads nothing either.</figcaption>
</figure>

The map's caches matter because the [priority multi-pass](../rendering/#4-decode-by-priority-the-clever-bit) walks the same visible chunks four times per frame; without a cache that would be four times the SD reads. With it, passes two through four are hits. The cache changes *when* a byte is read, never *what* decodes — so a render stays byte-identical whether the whole file was resident or streamed one chunk at a time.

---

## Where this lives

- Map reader, quadtree walk, and chunk decode: [`obc-reader/src/reader.rs`](src:firmware/obc-reader/src/reader.rs)
- Route reader, index, and decode: [`obc-route/src/reader.rs`](src:firmware/obc-route/src/reader.rs)
- The shared byte seam: [`obc-reader/src/byte_io.rs`](src:firmware/obc-reader/src/byte_io.rs)
- The byte-level specs: [`OBCM_Spec.md`](src:OBCM_Spec.md) · [`OBCR_Spec.md`](src:OBCR_Spec.md) · [`obc-ble-interface-spec.md`](src:obc-ble-interface-spec.md) (the wire contract routes/rides cross to the companion app)

Maps are produced by the packer and routes by the GPX converter — how those work, and how a route is matched to the map you're riding, is the subject of [packer & routing](../packer-routing/). For how these bytes become pixels, see the [rendering pipeline](../rendering/). Routes and rides also cross to a phone over Bluetooth as *these same bytes* — how that link is shaped is [the companion link](../companion-link/).
