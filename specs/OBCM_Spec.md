# OBCM File Format Specification (v13)

OBCM (OpenStreetMap Binary Chunked Map) is a compact binary map format designed
for efficient rendering on memory-constrained devices such as microcontrollers
(MCUs). It is written by the Rust packer (`host/obc-pack`) and read by the
Rust crate (`firmware/obc-reader`, shared by the desktop simulator and the nRF54L
firmware).

This document is the normative byte contract. Its code authority for version numbers,
fixed lengths, flags, sentinels, the canonical POI id table, and endian primitives is
`firmware/obc-formats/src/obcm.rs`; producers and consumers import those facts directly.

**Version 3** introduced a **level-of-detail (LOD) pyramid**: a file holds N
self-contained detail levels, each its own quadtree + chunk set with geometry
simplified to that level's resolution. The renderer reads only the level that
matches the current zoom, so zooming out touches a small coarse layer instead of
decoding fine geometry just to skip it.

**Version 4** appends a single 2-byte field to the header — the **user-position
marker color** (RGB565).

**Version 5** adds a 6th byte to style records for flags (bit 0 = priority).

**Version 6** appends a 4-byte **POI Section Offset** to the header (32 → 36
bytes) and a new **POI section** (§7): the OSM points-of-interest the packer
bakes in (water, campsites, accommodation, resupply, pharmacies, bike shops),
indexed by a small quadtree per category over 32-byte records. The section is
**always present** — a map with no POIs writes an empty directory, never a
sentinel-zero offset.

**Version 7** widens the POI record 32 → 36 bytes: the `Name` field grows 20 → 24
bytes and the two trailing reserved bytes become a `HoursRef` u16 into a new
**hours-pool section** (§7.5). The POI directory (§7.1) gains
`hours_pool_offset` + `hours_pool_count`. The pool holds deduplicated 29-byte
weekly-schedule blobs (today's opening hours, normalized at pack time from OSM
`opening_hours`), so a POI's hours are a single index lookup on-device.

**Version 8** appends a 4-byte **Nav Graph Offset** to the header (36 → 40 bytes)
and a new **navigation-graph section** (§8) at the file tail: the routable graph
the packer derives from OSM `highway=*` topology (junction nodes with inline
adjacency, indexed by a §4-style quadtree, plus a chunked edge-geometry pool), so
the device can run point-to-point A\* (epic #116) with only a small directory
resident. The section is **always present** — a map with no routable ways writes
an empty directory, never a sentinel-zero offset.

**Version 10** (epic #556 #557) grows the **style record** 6 → 8 bytes (§2): the
flags byte gains a **dashed** bit (bit 2, line style) and a **color2-present** bit
(bit 3), and a trailing **`color2`** u16 (RGB565 secondary color) is appended. The
header, geometry, POI, and nav sections are byte-identical to v9. `color2` is written
`0x0000` when its flag bit is clear, and readers MUST ignore it then (`0x0000` is a
legit color — black rails — not a "no color2" sentinel). (v10 was the only supported
version until v11, below; the interesting detail for v11 is that the §8 padding
lesson of v9 turned out to apply to the geometry chunks too.)

**Version 11** (issue #1009) stops paying for padding. Two changes, both to the
per-LOD geometry region; the header, POI section (§7), hours pool (§7.5) and
nav-graph section (§8) are byte-identical to v10.

1. **Data chunks are packed tight, behind a per-chunk offset table** (§5). v10
   padded every chunk to `Chunk Size` because `data_start + k * Chunk Size` was the
   O(1) addressing scheme — measured **53% of `freiburg.obcm` and 65% of
   `grimsel.obcm` was trailing `0xFF`**, structurally: a quadtree node splits when
   its features overflow one chunk, so leaves land between a quarter and half full.
   A LOD now writes `Chunk Count + 1` `uint32` offsets between its index and its
   chunk data; chunk `k` is `offsets[k]..offsets[k+1]`, still O(1), and each chunk
   carries exactly **one** trailing `0xFF` sentinel instead of padding. `Chunk Size`
   keeps its 18-byte LOD-table slot but changes meaning: it is now the chunk
   **capacity bound**, not a stride.
2. **The feature header shrinks 12 → 7 bytes for the common case** (§5). `Flags`
   moves to byte 1 so its new `0x08` **WIDE** bit tells a reader the header's width
   before it reads anything behind it. The compact layout stores `Pt Count` as a
   `uint8` and both anchors as `uint16`; a feature with more than 255 exterior
   vertices, or a leaf-relative anchor outside `0..=65535` (a coarse-LOD leaf can
   span far more than that), sets WIDE and keeps v10's `uint16` count + `int32`
   anchors.

Stacked, real maps land at **~2.3–2.5× smaller** (monaco 1 597 945 → 683 532 B;
grimsel 6 189 979 → 2 614 924 B) with byte-for-byte the same decoded geometry.
Tight chunks are also a read win: a chunk miss reads the chunk's real length —
averaging ~1 600 B, 3–4 SD blocks — instead of a fixed 4096 B / 8 blocks.
(v11 was the only supported version until v12, below; its geometry sections are
unchanged by it.)

**Version 12** (issue #1073, elevation epic #1068) makes the routable graph
**climb-aware**. Two fields, one section — §8. The header, style table, geometry
(§5), POI section (§7), hours pool (§7.5), nav directory (§8.1), node quadtree
(§8.2) and edge pool (§8.4) are **byte-identical to v11**.

1. **The §8.3 neighbor entry grows 15 → 17 bytes**: a trailing `uint16 Ascent M`,
   the **integrated** climb of riding that edge *from this record's node toward
   the neighbor*, in metres, saturating. Integrated rather than an endpoint
   difference, because a pass between two equal-height junctions has hundreds of
   metres of climb and no net change — the number A\* needs is the integral. It
   lives in the adjacency entry and nowhere else because relaxation reads exactly
   that record; §8's "no second fetch" intent is the whole reason the entry
   carries its neighbor's coordinate inline.
2. **The §8.6 profile record grows 52 → 56 bytes**: a `uint8 Climb Weight` (flat
   metres charged per metre of ascent; `0` = climb-blind) plus three reserved
   bytes written `0`.

The degree cap survives untouched: `13 + 17 × 24 = 421 ≤ 512`, so a cap-degree
junction record still fits one pinned nav chunk. Real maps grow ~0.3–0.6 %.

A map packed **without** terrain writes `Ascent M = 0` everywhere and is
decode-valid: it routes exactly as v11 did — the degrade path, and what the
smaller fixtures (`monaco.obcm`, `grimsel-demo.obcm`) still carry.
`grimsel.obcm` is packed **with** its terrain sidecar since 2026-08-03 (#1096
follow-up), so it exercises real integrated ascent and the traced contours.
**Version 13** adds a sparse exact-edge lookup index to §8 so routing can recover when the rider is
close to a road but farther than 250 m from every graph junction. Only final serialized edge pieces
longer than 300 m receive interior anchors, evenly spaced so every endpoint/anchor gap is at most
300 m. Each 12-byte anchor stores an absolute coordinate plus its edge-pool id; it is a lookup aid,
not a graph node and not the snapped position. The router projects the rider onto the named full
§8.4 polyline and connects that exact point virtually to the edge's real endpoints. The §8.1
directory grows 28 → 40 bytes to address the new quadtree and fixed 512-byte chunks (§8.7). All
other records retain their v12 layouts.

The coverage bound is geometric: a point on a road is at most 150 m along the polyline from an edge
endpoint or anchor. A rider at most 100 m from that road point is therefore at most 250 m from one
lookup record by the triangle inequality, regardless of curvature. The reference router uses a
251 m node-or-anchor search (the mathematical 250 m plus one metre of coordinate-rounding slack),
which is thus complete for the stated 100 m road-proximity envelope; the final projection is exact
within the stored polyline geometry. The guarantee assumes the producer reports zero dropped snap anchors;
shipping pack jobs treat any quadtree split-floor capacity warning as a failed coverage audit rather
than silently claiming complete lookup coverage.

**v13 is the only supported version**; earlier maps get repacked.

**Within v12** (issue #1095, same elevation epic) two of the style record's reserved
flag bits gained meanings — bit 4 **fixed width** and bit 5 **terrain layer** (§2).
This is deliberately *not* a version bump: nothing about the record's length, layout
or any offset moves, and §2's reader obligation for undefined style bits has always
been to ignore them, so a reader that does not know these two parses the same fields
and draws a slightly different-looking contour. §2 carries the argument in full.

**Version 9** (epic #533 N2) is a §8-only bump that makes the router **bike-type
aware** and shrinks the section it reads (measured ~58% padding in v8 node
chunks). The header stays 40 bytes; everything new hangs off the nav directory,
which grows 22 → **28 bytes** to add a **Profile Table Offset/Count** (§8.6). The
byte-level changes: each way now carries a packed **`way_kind`** class byte
(5-bit highway class + 3-bit surface class) on both its adjacency entries and its
edge record; neighbor entries slim **20 → 15 bytes** by storing each neighbor's
coord as an `int16` delta from the record's own coord and its cost as a `uint16`;
nav chunks are **pinned to 512 bytes** (the reader rejects any other value); node
chunks are **bin-packed** so distinct index leaves may share a chunk; and a
per-map **profile table** (§8.6) of `1..=8` bike profiles is baked in. (v9 was a
hard cut from v8; earlier versions v8 down to v2 were dropped — old maps get
repacked. v10 and then v11 superseded it, see above.)

## Design principles

1. **Pyramid layers.** Each LOD is independent: zoomed out ⇒ read one small
   coarse layer. (vs. tagging every feature with a min-zoom in a single fine
   tree, which forces the MCU to decode fine chunks just to skip them.)
2. **RGB565 in the file, quantized at render.** The style table is
   device-independent and matches the web builder editor. The renderer quantizes the
   small style palette to the target display depth once at load (RGB222 /
   64 colors for the LS021B7DD02).
3. **Meters-per-pixel LOD selection.** Each LOD stores a ground-meters-per-pixel
   threshold; the renderer computes current m/px from zoom + display size and
   picks the level. The same file looks right on a 1024 px desktop and a 240 px
   device.
4. **No runtime discovery.** Every section is reached via an explicit offset and
   every count is stored, so a no_std reader does zero traversal/sizing work to
   parse the structure.

All coordinates are integer **microdegrees** (1e-6 degrees). Projection to
screen space is the renderer's responsibility, not the format's.

## File layout

```
[Header]                            (40 bytes, fixed)
[Style Table]                       (global — shared by all LODs)
[LOD Table]                         (LOD Count entries)
[LOD 0 Index][LOD 0 Offset Table][LOD 0 Data Chunks]    (coarsest)
[LOD 1 Index][LOD 1 Offset Table][LOD 1 Data Chunks]
...
[LOD N-1 Index][LOD N-1 Offset Table][LOD N-1 Data Chunks] (finest)
[POI Directory][POI Indexes + Chunks] (§7)
[Hours-Pool Section]                  (§7.5)
[Nav Directory][Profile Table][Node Index + Chunks][Edge Pool] (§8 — file tail)
```

The byte layout is produced by `host/obc-pack/src/serialize.rs` (`serialize_lods`) and
parsed by `firmware/obc-reader/src/reader.rs`. All multi-byte integers are **little-endian**.

---

## 1. Header (40 bytes)

Packed as `struct "<4sBiiiiIBIHII"`.

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Magic | 4 | `char[4]` | Must be `b"OBCM"` |
| 4 | Version | 1 | `uint8` | `0x0C` |
| 5 | Min Lat | 4 | `int32` | Global bbox min latitude (microdegrees) |
| 9 | Min Lon | 4 | `int32` | Global bbox min longitude |
| 13 | Max Lat | 4 | `int32` | Global bbox max latitude |
| 17 | Max Lon | 4 | `int32` | Global bbox max longitude |
| 21 | Style Offset | 4 | `uint32` | Byte offset to the Style Table |
| 25 | LOD Count | 1 | `uint8` | Number of LOD levels (≥ 1) |
| 26 | LOD Table Offset | 4 | `uint32` | Byte offset to the LOD Table |
| 30 | Marker Color | 2 | `uint16` | User-position marker color (RGB565) |
| 32 | POI Section Offset | 4 | `uint32` | Byte offset to the POI Directory (§7) |
| 36 | Nav Graph Offset | 4 | `uint32` | Byte offset to the Nav Directory (§8) |

Note the bbox field order in the file is **lat, lon, lat, lon**. In practice the
Style Table immediately follows the header, so `Style Offset` is `40` (it was `36`
in v7, before the Nav Graph Offset was appended). The POI and nav sections are
always present, so neither offset is ever `0` — a map with no POIs (or no routable
ways) writes an **empty** directory there instead.

### Marker Color

The **user-position marker** (a chevron drawn at the user's GPS fix, pointing
along their course) is a single global map-presentation property, so its color
lives in the header rather than the per-feature Style Table — the marker is not an
OSM feature. It is RGB565 like every style color and is resolved to a device pixel
through the same render-time color policy (quantized to 64 colors on the
LS021B7DD02, true-color in the simulator). The marker's **shape and size are fixed**
in the renderer; only its color is map-configurable (the web builder editor sets it).
The default is `0xF800` (bright red), which reads well over both sea and land.

---

## 2. Style Table

Maps numeric style IDs to rendering properties. **Global**: style IDs are shared
across every LOD. Packed as `Count`, then `Count` records.

1. **Count** (`uint8`): number of styles.
2. **Style Records** (`Count` × 8 bytes, v10 — v5..v9 were 6 bytes):

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | ID | 1 | `uint8` | Style ID, referenced by feature headers |
| 1 | Z-Index | 1 | `int8` | Painter's-order layer (lower drawn first) |
| 2 | Color | 2 | `uint16` | RGB565 (the primary color) |
| 4 | Weight | 1 | `uint8` | Stroke width in pixels (lines) |
| 5 | Flags | 1 | `uint8` | Bits 0-1: priority level (1=highest/render first, 4=lowest/render last). **Bit 2 (v10): dashed** line (else solid; ignored for polygons). **Bit 3 (v10): color2 present.** **Bit 4: fixed width.** **Bit 5: terrain layer.** Bits 6-7 reserved, written 0 — a reader MUST **ignore** them, not reject the record (see below) |
| 6 | Color2 | 2 | `uint16` | RGB565 **secondary color** (v10). Written `0x0000` when flag bit 3 is clear; readers MUST ignore it then (`0x0000` is a legit color — black — not a "no color2" sentinel) |

The **secondary color** and **line style** drive the finest-LOD line/polygon
embellishments (road casing, dashed admin borders, railway stripes, polygon ring
outlines — epic #556); the semantics are the renderer's, not the format's. A solid,
single-color style (flags bits 2-3 clear, `Color2 = 0x0000`) is the pre-v10 record
padded to 8 bytes, so a map that uses no line styles renders identically.

**Bit 4 — fixed width.** `Weight` is the stroke's width in **device pixels**, used
verbatim: the renderer's zoom→width ramp does not apply to this style. It marks a
style as *a mark on the map* rather than *a thing with width on the ground*, which is
a general property and not the property of any one feature type. The ramp exists
because a road genuinely is wider than a footpath and both are wider seen from 1 m/px
than from 100 — a mark has no ground width at all, so ramping it is not merely wrong
but backwards: it draws thinnest where the mark carries the most meaning and thickest
where it does the most damage. The width is still clamped to the renderer's `1..=12`
px range; the bit opts out of the ramp, not out of the panel. Ignored for polygons,
whose fills have no stroke width (their §5 outline accent is a fixed hairline already).

> **Why no shipped style but the contours takes it (yet).** Every other line in the
> shipped presets *is* a thing on the ground — roads, tracks, rail, waterways, admin
> borders that follow ridges and rivers — so the ramp is what they want, and a style
> that opted out would freeze at one width across a 100× zoom range. Contours (#1095)
> are the first shipped mark: a 100 m isoline is a statement about the terrain, not an
> object with a footprint. A future grid, hatch or hairline annotation would take the
> same bit; that is why it is spelled as a property of the style and not as
> "contours draw thin".

**Bit 5 — terrain layer.** The style belongs to the **terrain layer**: the group a
device may suppress wholesale as one user-facing choice, rather than by naming feature
types. It is presentation metadata carried on the style record and nothing else — no
reader behaviour in this version depends on it, and a renderer that ignores it draws a
correct map. It is written so the device Settings toggle (#1096) has something to read.

> **Defining bits 4-5 is not a version bump, and this section is why.** Unlike a
> *feature*'s `Flags` (§5.2), where "a reader MUST reject a feature with any [reserved
> bit] set", the reader obligation for a style record's undefined bits has always been
> to **ignore** them — the reference reader masks bits 0-1 and tests bits 2-3, and has
> never looked at the rest. So a v12 reader meeting a v12 record with bit 4 set parses
> every field correctly and renders a contour at the ramped width instead of the
> authored one: a presentation degrade, inside one style record, with no offset,
> length or count affected anywhere in the file. A version is this format's hard cut —
> it makes every existing map unreadable until repacked and every existing reader
> refuse every new map — and it is reserved for changes that would otherwise be
> *misparsed*, not for ones that are merely rendered older. **Bits 6-7 keep exactly
> this contract**: written `0`, ignored by readers, and definable in place the same way.

> **Style IDs are assigned by the packer, not authored.** A style ID is a
> purely internal reference into this table — no reader depends on a specific
> value, only on global uniqueness within the file. The packer ignores any `id`
> in `config.json` and numbers every feature type sequentially (`1`-based, in
> document order) at load time, so collisions are impossible by construction.
> `0xFF` is reserved as the end-of-features sentinel (see §4), so a file holds at
> most 254 distinct styles.

---

## 3. LOD Table

`LOD Count` entries, ordered **coarsest (index 0) → finest (index N-1)**. Each
entry is 18 bytes, packed as `struct "<fIIHI"`.

| Field | Size | Type | Description |
| :-- | :-- | :-- | :-- |
| Max Meters/Pixel | 4 | `float32` | Upper bound of the m/px range this LOD covers. Strictly decreasing down the list; the coarsest level is `+inf` (`f32::INFINITY`). |
| Index Offset | 4 | `uint32` | Byte offset to this LOD's quadtree index |
| Index Node Count | 4 | `uint32` | Number of `uint32` nodes in the index |
| Chunk Size | 2 | `uint16` | **Capacity bound** of one data chunk (bytes) — per-LOD. v11: not a stride; see below |
| Chunk Count | 4 | `uint32` | Number of data chunks in this LOD |

A LOD's region is three parts, back to back:

```
[Quadtree Index]   Index Node Count × uint32          at Index Offset
[Offset Table]     (Chunk Count + 1) × uint32         at Index Offset + Index Node Count * 4
[Chunk Data]       tightly packed chunks              at data_start (below)
```

```
table_start = Index Offset + Index Node Count * 4
data_start  = table_start + (Chunk Count + 1) * 4
chunk k     = data_start + offsets[k] .. data_start + offsets[k+1]
```

**`Chunk Size` is a bound, not a stride** (v11). It is the packer's leaf-split
threshold and the largest length any single chunk may have; a reader MUST reject a
chunk whose offset pair spans more than it. Chunk lengths come from the offset table
(§5), which is what lets chunks be packed tight — v10's fixed stride is why every
chunk had to be padded to `Chunk Size`.

Storing `Index Node Count` and `Chunk Count` explicitly is what removes any
runtime discovery: the reader never has to walk the tree to learn its size. The
offset table's last entry (`offsets[Chunk Count]`) is the LOD's total chunk bytes,
so one `uint32` read at parse bounds every later chunk fetch.

---

## 4. Quadtree Index (per LOD)

A flat array of `Index Node Count` × `uint32`. **Every LOD's quadtree is built
over the same global bbox** (from the header), so node bboxes are computed
identically at every level and the renderer's subdivision math is
LOD-independent. Coarse levels hold few features ⇒ shallow trees.

Each node value:

- **Leaf** — high bit (`0x80000000`) clear:
  - `0x7FFFFFFF` → **empty** leaf (no chunk).
  - otherwise → the **Chunk ID** into this LOD's data chunks.
- **Branch** — high bit set: `0x80000000 | first_child_index`. The four children
  are stored sequentially in the order **NW, NE, SW, SE**.

Children bboxes are derived by splitting the parent bbox at its **floor-division
midpoints** (`mid = (min + max) // 2` for both axes), matching the packer:

```
NW = (min_lon, mid_lat, mid_lon, max_lat)
NE = (mid_lon, mid_lat, max_lon, max_lat)
SW = (min_lon, min_lat, mid_lon, mid_lat)
SE = (mid_lon, min_lat, max_lon, mid_lat)
```

To query a viewport: start at node 0 with the global bbox, recurse into children
whose bbox intersects the view, and collect `(chunk_id, node_bbox)` for every
non-empty leaf reached. The `node_bbox` is required to decode the chunk (see
§5.2, anchors). A `Chunk ID` addresses the LOD's offset table (§5.1), not a fixed
stride.

---

## 5. Data Chunks (per LOD)

### 5.1 Offset table + tight chunks (v11)

A LOD's chunk data is addressed by its own **offset table**, written between the
quadtree index and the chunks (§3):

- `Chunk Count + 1` `uint32` entries. Each is a byte offset **relative to the start
  of the chunk-data region** (i.e. to the byte just past the table itself).
- `offsets[0]` is always `0`. Offsets are non-decreasing. `offsets[Chunk Count]` is
  the region's total chunk bytes.
- Chunk `k` occupies `offsets[k] .. offsets[k+1]`; its length is the difference.
- The table is written even when `Chunk Count == 0`, where it is the single `0` entry.

Each chunk is its packed features followed by **exactly one** `0xFF` `CHUNK_END`
sentinel byte, and nothing else — no padding. A `0xFF` style-ID byte is an
impossible style, so the sentinel still marks end-of-features for a reader walking
the stream; the offset-derived end is then a second, independent bound. A reader
MUST treat a chunk whose feature stream reaches the offset-derived end **without**
meeting the sentinel as malformed (truncated), not as a clean finish.

A reader MUST validate an offset pair before using it, because `Chunk ID` comes from
a quadtree leaf and is arbitrary in a corrupt map: `k < Chunk Count`,
`offsets[k] <= offsets[k+1]`, `offsets[k+1] <= offsets[Chunk Count]`, and
`offsets[k+1] - offsets[k] <= Chunk Size`.

> **Why.** v10 addressed chunk `k` at `data_start + k * Chunk Size`, which forces
> every chunk to be padded to `Chunk Size`. Because a quadtree node splits as soon as
> its features overflow one chunk, leaves settle between a quarter and half full, so
> the padding is structural rather than a tuning problem — measured 53% of
> `freiburg.obcm`, 65% of `grimsel.obcm`. One `uint32` per chunk buys all of it back
> (freiburg: 1 534 chunks × 4 B = 6 KB of table for 3.8 MB of padding).

### 5.2 Feature Header (7 or 12 bytes, v11)

`Flags` is at byte **1** in both layouts — its `0x08` **WIDE** bit selects the
layout, so a reader knows the header's width before it reads any field behind it.

**Compact** (WIDE clear), 7 bytes, `struct "<BBBHH"` — the common case:

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Style ID | 1 | `uint8` | Reference into the Style Table |
| 1 | Flags | 1 | `uint8` | `0x01` 16-bit deltas · `0x02` polygon · `0x04` has holes · `0x08` WIDE (**clear** here) |
| 2 | Pt Count | 1 | `uint8` | Vertex count of the **exterior** ring, `1..=255` |
| 3 | Anchor X | 2 | `uint16` | Exterior start relative to the **leaf node's min longitude** (microdegrees), `0..=65535` |
| 5 | Anchor Y | 2 | `uint16` | Exterior start relative to the leaf node's min latitude |

**Wide** (WIDE set), 12 bytes, `struct "<BBHii"` — the escape:

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Style ID | 1 | `uint8` | Reference into the Style Table |
| 1 | Flags | 1 | `uint8` | Same bits, `0x08` WIDE **set** |
| 2 | Pt Count | 2 | `uint16` | Vertex count of the exterior ring |
| 4 | Anchor X | 4 | `int32` | Exterior start, relative to the leaf node's min longitude |
| 8 | Anchor Y | 4 | `int32` | Exterior start, relative to the leaf node's min latitude |

Bits 4-7 of `Flags` are reserved and written `0`; a reader MUST reject a feature with
any of them set. `Pt Count == 0` is malformed in both layouts. Compact anchors are
**unsigned** — zero-extended, never sign-extended.

A writer MUST choose compact when `Pt Count` is in `1..=255` **and** both anchor
components are in `0..=65535`, and wide otherwise. The escape is not hypothetical: a
coarse-LOD leaf can span far more than 65 535 µdeg (~7 km), so an anchor inside it
genuinely needs the wider field. Everything after the header — hole bookkeeping and
the delta streams — is identical in both layouts and unchanged from v10.

The **anchor** is the feature's first absolute coordinate, stored relative to the
containing leaf node's min corner to keep it small:

```
anchor_abs = (node_bbox.min_lon + AnchorX, node_bbox.min_lat + AnchorY)
```

### Geometry encoding (delta)

Rings are delta-encoded to minimize size. Bit depth is chosen **per feature**:
if every `dx`/`dy` fits in `int8` (|d| ≤ 127), `Flags & 0x01 == 0` and deltas are
`int8`; otherwise the flag is set and all deltas are `int16`.

- **Exterior ring** (`Pt Count` vertices): the first vertex *is* the anchor;
  the remaining `Pt Count - 1` vertices follow as `(dx, dy)` pairs, each relative
  to the previous vertex.
- **Holes** (only if `Flags & 0x04`, after the exterior deltas):
  - **Hole Count** (`uint8`)
  - per hole: **Pt Count** (`uint16`), then `Pt Count` `(dx, dy)` delta pairs.
    Holes store **all** vertices as deltas — the first is relative to the feature
    anchor, the rest chain from the previous vertex.

Lines use only the exterior ring (`Flags & 0x02 == 0`, no holes).

> **Long-segment densification:** the packer inserts intermediate vertices on any
> segment longer than `30000` microdegrees so that no single delta exceeds the
> 16-bit range. Readers need no special handling — these are ordinary vertices.

> **Per-feature vertex cap:** although `Pt Count` is a `uint16`, a single feature
> (exterior plus all holes, densification included) must not exceed **2048
> vertices**. The reference reader decodes a whole feature into one fixed buffer
> (`MAX_FEAT_PTS`). It validates and consumes the complete encoded feature before
> publishing geometry: if the caller's fixed point/ring scratch is too small, the
> whole feature is dropped with an explicit capacity outcome — no truncated line or
> polygon is exposed. The packer guarantees the format bound through `Chunk Size`:
> a feature can't outgrow its chunk, and its packed bytes are at least
> `7 + 2·(V−1) = 2·V + 5` for `V` total vertices (the smallest header v11 writes is
> the 7-byte compact one, and the densest geometry is 8-bit deltas at 2 bytes per
> vertex after the anchor; holes and the wide header only add). So
> `Chunk Size ≤ (2048−1)·2 + 7 = 4101` keeps every feature within the cap — 5 bytes
> tighter than v10's `4106`, which was derived off the 12-byte header. `obc-pack`
> rejects a larger `Chunk Size` at build time rather than emit a feature the
> reference buffer cannot hold. (The bound is deliberately the *loosest* encoding: a
> genuinely 2048-vertex feature needs the wide header, so it packs to 4106 bytes and
> could never fit a `4101`-byte chunk in the first place.)

> **Per-feature ring cap:** although `Hole Count` is a `uint8`, a single feature
> must not exceed **32 rings** (exterior + 31 holes). The reference reader's ring
> scratch (`MAX_FEAT_RINGS`) is fixed at 32 and a feature past it is dropped whole,
> with the same explicit capacity outcome as the vertex cap. Bytes do not imply
> this bound — a heavily simplified polygon can carry dozens of holes on a handful
> of vertices — so `obc-pack` enforces it structurally: a quadtree node holding an
> over-cap polygon splits (clipping spreads the holes across the children), and at
> the 10 µdeg split floor the smallest holes are dropped to fit.

### Polygon-with-holes byte layout

```
[Feature Header (7 B compact | 12 B wide)]
[Exterior deltas]                ((Pt Count - 1) × (int8|int16) pairs)
[Hole Count (uint8)]
  [Hole 1 Pt Count (uint16)]
  [Hole 1 deltas]                (Pt Count × pairs)
  [Hole 2 Pt Count (uint16)]
  [Hole 2 deltas]
  ...
```

---

## 6. LOD selection (renderer)

The renderer computes the current ground **meters-per-pixel** from zoom and
display size. Using a latitude-based definition, 1 microdegree of latitude ≈
`0.11132` m, so with `zoom` in pixels-per-microdegree-of-latitude:

```
mpp = 0.11132 / zoom
```

Among the LODs whose range covers `mpp` (`Max Meters/Pixel[i] >= mpp`), pick the
**finest** (largest index). The coarsest level's `+inf` always qualifies, so the
result is always valid; clamp to `[0, N-1]`.

Worked example (the 3-level default):

| LOD | content | Max m/px |
| :-- | :-- | :-- |
| 0 country | coastline/land, sea, motorway/trunk, major rivers, admin borders | `+inf` |
| 1 region | + primary/secondary roads, lakes, forests | 50 |
| 2 city/street | + residential/service, footways, buildings, parks | 10 |

- `mpp = 70` → only LOD 0 covers it → **LOD 0**
- `mpp = 30` → LOD 0 & 1 cover it; finest = **LOD 1**
- `mpp = 5`  → all cover it; finest = **LOD 2**

Within a selected LOD, query the quadtree for the viewport, decode the visible
chunks, sort features by style `Z-Index` (painter's algorithm), then draw —
polygons via even-odd scanline fill (holes fall out of the even-odd rule for
free), lines as weighted polylines.

**Backdrop convention.** Before drawing geometry, a renderer clears the screen to
the **backdrop color**: the color of the style with the lowest `Z-Index` (the
bottom of the paint order — by convention the sea/background, e.g. `natural.sea`
at `z_index 0`). This is derived from the style table, not a fixed style ID, so
it survives the packer's automatic ID assignment. Land is then painted on top.

---

## 7. POI Section (v7)

Point-of-interest features the packer classifies from OSM nodes and closed-way
centroids (see the category table below). Unlike geometry, POIs are **not**
rendered on the map; the device surfaces them as a category → nearest-list
browser. They are indexed for a nearest-N query, not a viewport walk, so each
category gets its own small quadtree over 36-byte point records (v7 widened them
from 32).

The section is reached from `POI Section Offset` (header offset 32) and is
**always present**: a map with no POIs writes a directory of six empty
categories, never a zero offset. Each POI record carries a `HoursRef` u16 into
the trailing **hours-pool section** (§7.5), reached from the directory's
`hours_pool_offset`.

### 7.1 POI Directory

```
uint8   Category Count            (= 6 in v7)
uint16  Chunk Size                (POI chunk capacity in bytes — the packer writes 512)
per category (Category Count entries, 13 bytes each):
  uint8   Category ID
  uint32  Index Offset            (byte offset to this category's quadtree index)
  uint32  Index Node Count        (number of uint32 nodes; 0 ⇒ category empty)
  uint32  Chunk Count             (number of data chunks in this category)
uint32  Hours Pool Offset         (byte offset to the hours-pool section, §7.5)
uint16  Hours Pool Count          (number of 29-byte blobs; 0 ⇒ no hours in this map)
```

`Chunk Size` is shared by every category (all POI chunks are the same fixed
capacity). As with a LOD, a category's data chunks begin at
`Index Offset + Index Node Count * 4` — the exact §3/§4 convention, so the reader's
`walk_leaves` leaf-walk and chunk-offset math are reused verbatim. An empty
category (`Index Node Count == 0`) still has a directory entry; its `Index Offset`
points at where its (zero-length) index would start and `Chunk Count` is `0`.

The two **v7 hours-pool fields** trail the per-category entries. `Hours Pool
Offset` is the absolute byte offset of the hours-pool section (§7.5); `Hours Pool
Count` is the number of 29-byte blobs there and MUST equal the `count` written at
that offset. `Hours Pool Count == 0` means the map has no hours (the pool is a bare
`0` count); a record's `HoursRef == 0xFFFF` likewise means "no hours."

### 7.2 Per-category quadtree

Identical to the geometry quadtree (§4): a flat `uint32` array using the same
node encoding (branch bit / empty-leaf sentinel / chunk id), built over the **same
global bbox from the header**, with the same floor-division-midpoint NW/NE/SW/SE
subdivision. Point features make these trees small and shallow. A reader walks
one exactly as it walks a LOD index, collecting `(chunk_id, node_bbox)` for each
non-empty leaf; the `node_bbox` is **not** needed to decode a POI record (records
store absolute coordinates), only to prune the walk.

### 7.3 POI records — fixed 36 bytes

Records are packed into `Chunk Size`-byte chunks (512 ⇒ `512 / 36 = 14`
records/chunk). Each record is exactly 36 bytes (v7 widened them from 32). A `0xFF`
**Subtype** byte marks the end of records in a chunk (mirrors the geometry chunk's
`0xFF` style-ID sentinel); trailing bytes of a partial final chunk are
`0xFF`-padded.

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Lat | 4 | `int32` | Latitude, **absolute** microdegrees |
| 4 | Lon | 4 | `int32` | Longitude, **absolute** microdegrees |
| 8 | Subtype | 1 | `uint8` | Canonical subtype id (§7.4); `0xFF` = end-of-chunk sentinel |
| 9 | Name Len | 1 | `uint8` | Length of the stored name in bytes (`0` = unnamed) |
| 10 | Name | 24 | `char[24]` | Pre-folded printable ASCII; unused tail bytes are `0xFF` |
| 34 | HoursRef | 2 | `uint16` | 0-based index into the hours pool (§7.5); `0xFFFF` = no hours |

Coordinates are **absolute** (no per-node anchor/delta as in geometry §5): at a
fixed 36 bytes the delta win isn't worth the decode asymmetry with geometry
chunks, and fixed-size records keep chunk packing trivial (`Chunk Size / 36`
records per chunk, no per-record length bookkeeping). The **category** is not
stored per record — it is derived on-device from the subtype (each subtype maps to
exactly one category, §7.4) — and is implicit anyway from which category's
quadtree the record came from.

Names are ASCII-folded at pack time to printable ASCII (`0x20..=0x7E`) and
capped at **24 bytes** (v7 widened the field from 20) — a fixed-width,
one-byte-per-character slot, so the packer transliterates umlauts/accents
(e.g. `ä → ae`) rather than store variable-width UTF-8; an unnamed POI
(`Name Len == 0`) shows its subtype's fallback label on-device. The 24-byte
`Name` field is `0xFF`-padded past `Name Len`.

`HoursRef` is a 0-based index into the hours-pool section (§7.5): blob `i` lives at
`hours_pool_offset + 2 + i*29`. `0xFFFF` means the POI has no (parseable) hours.
Duplicate weekly schedules collapse to one pooled blob, so many POIs in a region
can share a single `HoursRef`.

### 7.4 Canonical category / subtype table (normative)

This is the **normative home** of the id table; `obc-formats/src/obcm.rs` is its code
authority for subtype ids, categories, and fallback labels. `obc-pack`'s `poi.rs`
adds only the OSM `key=value` classification that produces each subtype, while the
device reads the shared table directly. **Ids are append-only** — an existing
row's category or subtype id must never be renumbered (an old map's records would
then decode as the wrong POI). Subtype `0` is reserved; `0xFF` is the
end-of-chunk sentinel and can never be a subtype id.

| Category ID | Category | Subtype ID | OSM tag (`key=value`) | Fallback label |
| :-- | :-- | :-- | :-- | :-- |
| 1 | Water | 1 | `amenity=drinking_water` | Drinking water |
| 1 | Water | 2 | `natural=spring` | Spring |
| 1 | Water | 3 | `man_made=water_tap` | Water tap |
| 1 | Water | 4 | `amenity=water_point` | Water point |
| 2 | Campsite | 5 | `tourism=camp_site` | Campsite |
| 2 | Campsite | 6 | `tourism=caravan_site` | Caravan site |
| 3 | Accommodation | 7 | `tourism=hotel` | Hotel |
| 3 | Accommodation | 8 | `tourism=hostel` | Hostel |
| 3 | Accommodation | 9 | `tourism=guest_house` | Guest house |
| 3 | Accommodation | 10 | `tourism=motel` | Motel |
| 3 | Accommodation | 11 | `tourism=wilderness_hut` | Wilderness hut |
| 3 | Accommodation | 12 | `tourism=alpine_hut` | Alpine hut |
| 4 | Resupply | 13 | `shop=supermarket` | Supermarket |
| 4 | Resupply | 14 | `shop=convenience` | Convenience |
| 4 | Resupply | 15 | `shop=bakery` | Bakery |
| 4 | Resupply | 16 | `amenity=marketplace` | Marketplace |
| 5 | Pharmacy | 17 | `amenity=pharmacy` | Pharmacy |
| 6 | Bike shop | 18 | `shop=bicycle` | Bike shop |

Subtype ids are dense and 1-based, so a subtype id indexes directly into the
table (`row = subtype - 1`). The category count in the directory (`6`) equals the
number of distinct category ids; every subtype belongs to exactly one category.

### 7.5 Hours-pool section (v7)

A single deduplicated pool of weekly opening-hours schedules, written after the
last POI category's chunks and reached from the directory's `Hours Pool Offset`
(§7.1). A POI
record's `HoursRef` (§7.3) is a 0-based index into it; identical schedules collapse
to one blob, so a region's shops share entries and the pool stays small (only POIs
with parseable hours cost anything).

```
uint16  Count                     (number of blobs; equals Hours Pool Count in the directory)
per blob (Count entries, 29 bytes each):
  uint8   Flags
  per day (7 days, Mon..Sun, 2 slots each):
    uint8  Open Q                 (quarter-hours from midnight, 0..=96)
    uint8  Close Q
```

Blob `i` (a record's `HoursRef == i`) lives at `Hours Pool Offset + 2 + i*29`. An
empty pool is just the 2-byte `Count == 0`. Hours are parsed and normalized from
OSM `opening_hours` **at pack time** (the grammar never runs on the device); the
device does a trivial weekday lookup.

**Blob layout (29 bytes).** `Flags` bit 0 = **seasonal** (the source rule carried a
month/date/season selector and a representative in-season week was baked — the UI
ignores this in v1), bit 1 = **truncated** (a rule the encoding can't model — a
`PH`/`SH` non-`off` rule, `sunrise`/`sunset`, or a 3rd+ interval on a day — was
dropped); other bits reserved `0`. The seven days run **Mon (index 0) .. Sun (index
6)**, each with up to two `(Open Q, Close Q)` intervals.

**Time convention.** A time-of-day is quarter-hours from midnight, `0..=96` (`96` =
24:00), so the resolution is 15 minutes. Per interval:

- **Unused slot** — `(0, 0)`.
- **Closed day** — both slots `(0, 0)`.
- **Open all day (24 h)** — slot 0 `(0, 96)`, slot 1 `(0, 0)`.
- **Overnight wrap** — `Close Q <= Open Q` (both nonzero): the interval runs past
  midnight, stored as-is (never split across days). E.g. `22:00-02:00` → `(88, 8)`.
- A day with more than two intervals is truncated to the first two and the blob's
  `Flags` truncated bit is set.

---

## 8. Navigation-Graph Section (v9)

The **routable graph** the on-device router (epic #116, made bike-type-aware by
#533) runs A\* over: junction **nodes** (derived from OSM node ids shared across
routable `highway=*` ways) joined by undirected **edges** (the polyline between
two junctions, junction-free inside). The packer builds the graph in `nav.rs`
(way-kind classification, bike-legality filter, island pruning, junction split,
dedup, edge splits) and this section is its on-wire form.

The section is reached from `Nav Graph Offset` (header offset 36) and is **always
present**: a map with no routable ways writes an empty directory (`Index Node
Count == 0`) — but still carries its profile table (§8.6), never a zero offset.
Layout, in file order:

```
[Nav Directory]     (28 bytes — the graph's resident header, §8.1)
[Profile Table]     (§8.6 — 1..=8 bike profiles, always present)
[Alignment Padding] (0..511 zero bytes in populated files)
[Node Quadtree]     (§4 encoding over the header global bbox)
[Node Chunks]       (variable-length junction records, bin-packed, §8.3)
[Edge Pool]         (chunked edge records addressed by byte offset, §8.4)
```

Design intent: the device is too RAM-tight for any id → offset table (a real
region has millions of graph elements), so A\* **re-fetches spatially** — settling
a node is one quadtree descent to its coord's leaf + one chunk read — and each
record carries its neighbors' coords **inline** so relaxation (`f = g + h`) needs
no second fetch. Edge geometry is touched while resolving the two exact projected endpoints and
when the final route is emitted; the A\* search between those virtual endpoints still never fetches
geometry.
Only the directory and the profile table (≤ `8 × 56 = 448` B) are resident.

### 8.1 Nav Directory (40 bytes)

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Index Offset | 4 | `uint32` | Byte offset to the node quadtree index (§8.2) |
| 4 | Index Node Count | 4 | `uint32` | Number of `uint32` nodes in the index; `0` ⇒ **empty graph** |
| 8 | Node Chunk Count | 4 | `uint32` | Number of node data chunks (§8.3) |
| 12 | Edge Pool Offset | 4 | `uint32` | Byte offset to the edge pool (§8.4) |
| 16 | Edge Chunk Count | 4 | `uint32` | Number of `Chunk Size`-byte chunks in the edge pool |
| 20 | Chunk Size | 2 | `uint16` | Fixed capacity of every nav chunk — **must be `512`** (the reader rejects any other value) |
| 22 | Profile Table Offset | 4 | `uint32` | Absolute byte offset of the §8.6 profile table |
| 26 | Profile Count | 1 | `uint8` | Number of 56-byte profile records; **`1..=8`** (reader rejects `0` or `> 8`) |
| 27 | Reserved | 1 | `uint8` | `0` (keeps the directory even-sized; no other meaning) |
| 28 | Snap Index Offset | 4 | `uint32` | Byte offset to the §8.7 snap-anchor quadtree index |
| 32 | Snap Index Node Count | 4 | `uint32` | Number of `uint32` nodes in the snap index; `0` ⇒ no interior anchors |
| 36 | Snap Chunk Count | 4 | `uint32` | Number of fixed 512-byte snap-anchor chunks following that index |

Node data chunks begin at `Index Offset + Index Node Count * 4` — the §3/§4
convention, so the reader's leaf-walk and chunk-offset math are reused verbatim.
The packer writes the **profile table immediately after this 40-byte directory**
(before the node index), so `Index Offset` and `Edge Pool Offset` point past it.
For a populated graph, current producers insert `0..511` zero bytes after the
profile table such that `Index Offset + Index Node Count × 4` (the first node
chunk) is a 512-byte file offset. Because every node chunk is 512 bytes, this
also makes `Edge Pool Offset` 512-byte aligned. A full-chunk read can therefore
be served by one physical card command instead of the two commands required when
the same logical read straddles sectors. This is a **producer guarantee, not a
reader validity requirement**: existing compact v12 files remain valid because
all boundaries are explicitly addressed by the directory.
The edge pool is followed by optional zero padding, the §8.7 snap index, and its chunks. Producers
align the first snap chunk to a 512-byte file offset just like the node chunks. An empty graph still
writes `Chunk Size` and the profile table, and points all zero-length data offsets just past the
profile table, exactly like an empty POI category. A populated graph with no edge longer than 300 m
sets both snap counts to zero and points `Snap Index Offset` just past the edge pool.

**`Chunk Size` is pinned to 512 in v9.** Earlier versions let it vary (up to
2048); v9 fixes it so a leaf holds a handful of junction records — one chunk read
serves one A\* settle — and the reader **rejects a directory whose `Chunk Size`
is not 512** (a distinct parse error from the header version check, so an old
file and a mis-sized current file are told apart). The geometry sections' configurable
`chunk_size` (§5) is independent — that knob governs §5 only; nav is pinned.

### 8.2 Node quadtree

Identical to §4 / §7.2: a flat `uint32` array with the same node encoding (branch
bit / empty-leaf sentinel / chunk id), built over the **same global bbox from the
header**, with the same floor-division-midpoint NW/NE/SW/SE subdivision and BFS
flattening. The packer splits a leaf once its packed records (§8.3) exceed one
chunk — by **bytes**, since records are variable-length — with the same 10-µdeg
recursion floor. As with POIs, a node's `node_bbox` is not needed to decode its
records (coordinates are absolute); the walk only uses it to prune.

**Bin-packed chunks (v9).** After building the tree, the packer assigns chunk ids
**first-fit over the leaves in BFS emission order**: each leaf's record block goes
into the first already-open chunk with room, opening a new chunk only when none
fits (v8 gave every leaf its own chunk, wasting the ~58% of a chunk a half-full
leaf left empty). One consequence is load-bearing:

> **Distinct index leaves may reference the same chunk id.** First-fit reaches
> back to earlier chunks, so leaves sharing a chunk can be spatially distant. A
> walk that visits several leaves sharing a chunk decodes that chunk once per
> leaf, so a consumer may see the same junction record **more than once per
> query** — and see records outside the leaf's own bbox. Consumers must therefore
> be **idempotent**. The reference consumers are: A\* settle matches by `Node Id`
> (a repeat is a no-op), and snap tracks the best candidate (a repeat can't
> change the best). A single leaf's records never straddle a chunk boundary.

The index still stores exactly one chunk id per leaf; only the leaf→chunk mapping
changed (many-to-one instead of one-to-one). The reader's leaf-walk and
chunk-decode are unchanged.

### 8.3 Junction records (variable length)

Records are packed back-to-back into 512-byte chunks; unused trailing bytes are
`0xFF`. A record is `13 + 17 × Degree` bytes:

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Lat | 4 | `int32` | Latitude, **absolute** microdegrees |
| 4 | Lon | 4 | `int32` | Longitude, **absolute** microdegrees |
| 8 | Node Id | 4 | `uint32` | Dense pack-run node id (the A\* hash key; stable within one file) |
| 12 | Degree | 1 | `uint8` | Neighbor count; **`0xFF` = end-of-chunk sentinel** |
| 13 | Neighbors | 17 × Degree | | `Degree` entries, layout below |

Per neighbor entry (17 bytes, v12):

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Neighbor Id | 4 | `uint32` | The adjacent junction's `Node Id` |
| 4 | Neighbor dLat | 2 | `int16` | Its latitude as a **delta from this record's `Lat`** (µdeg) |
| 6 | Neighbor dLon | 2 | `int16` | Its longitude as a delta from this record's `Lon` |
| 8 | Edge Id | 4 | `uint32` | The connecting edge, §8.4 addressing |
| 12 | Cost M | 2 | `uint16` | The edge's raw ground length in meters (the unweighted distance) |
| 14 | Way Kind | 1 | `uint8` | The edge's packed class byte (§8.6) — the input to profile weighting |
| 15 | Ascent M | 2 | `uint16` | **Directional** (v12): the integrated climb, in metres, of riding this edge *from this record's node toward the neighbor*. Saturating; `0` on a map packed without terrain |

The neighbor's absolute coord is reconstructed as `(Lat + dLat, Lon + dLon)`; the
packer guarantees both endpoints of every edge sit within `int16` of each other
(see §8.4) so the delta never overflows. `Cost M` is the **unweighted** ground
distance; the profile-weighted cost A\* actually accumulates is
`Cost M × effective_multiplier(Way Kind) >> 4 + Ascent M × Climb Weight` (§8.6),
computed on device at relaxation — the file stores distance and climb, not weight.

`Ascent M` is an **integral over the edge's polyline, not an endpoint
difference**. A pass road between two 500 m junctions has hundreds of metres of
climb in each direction and no net change at all; an endpoint delta would price
it as flat. The producer samples elevation along the edge's densified polyline
(one sample per vertex plus interpolated points, so no gap exceeds ~50 m of
ground) and folds the `(distance, elevation)` stream through the shared
dead-banded integrator, so the number a route is *costed* by is the number the
rider is later *shown*. A stretch with no elevation coverage contributes nothing:
the integrator re-anchors across the hole rather than booking the climb over it.

Rules:

- **Sentinel.** Because chunks are `0xFF`-padded, the byte where the next record's
  `Degree` would sit reads `0xFF` — the reader stops there (mirrors the POI
  subtype sentinel; the geometry chunks' style-id sentinel likewise). A record
  never straddles a chunk boundary, so a chunk decodes in isolation.
- **Degree cap: 24.** `13 + 24 × 17 = 421 ≤ 512`, so a cap-degree record always
  fits one chunk; real OSM junction degrees never approach it. A pathological
  node keeps its **first 24** adjacency entries (edge-pool order, deterministic)
  and the packer warns; a dropped arc survives one-way via the neighbor's own
  record. `0xFF` can therefore never be a real degree.
- **Undirected, with one exception.** Every edge appears in both endpoints'
  records with the **same** `Edge Id`, `Cost M`, and `Way Kind`. **`Ascent M` is
  the exception and MUST NOT be assumed equal**: the entry `a→b` carries
  `ascent(a→b)` and the entry `b→a` carries `ascent(b→a)`, which is the first
  direction's *descent*. A consumer that verifies "both sides agree" must exclude
  this field. A self-loop (`a == b`, e.g. a lollipop loop) appears **once** in its
  node's record, carrying its forward direction's ascent.
- **Seam determinism.** A producer that cuts one edge into pieces at a cell border
  (`OBCA_Spec.md` §3) integrates each piece over the **same global elevation
  lattice**, so two neighbouring cells' stubs are each the integral of their own
  geometry over one surface and the pieces' ascents sum to the uncut edge's.
- Degree `0` is valid to decode but the packer never emits it (every junction
  comes from at least one edge endpoint).

### 8.4 Edge pool

*(Byte-identical to v9/v11. The v12 climb lives in the adjacency entry, not here: v13 reads the
pool during endpoint projection, but A\* relaxation still must not have to touch it.)*

Deduplicated edge geometry, fetched at route emit (stitching the A\*
came-from chain into the output polyline) and by v13's endpoint projection; also the sum of `Length M` over the
chain is the route's **displayed** distance — the weighted `g` is no longer a
distance). The pool is a run of `Edge Chunk Count` × 512-byte chunks; records are
packed back-to-back, and a record that would cross a chunk boundary is pushed to
the next chunk start (`0xFF` padding fills the gap), so **no record straddles a
chunk** — one chunk-granular read always covers one edge.

**Addressing: `Edge Id` is the record's pool-relative byte offset.** The reader
derives `chunk = Edge Id / 512`, `offset = Edge Id % 512` — zero resident index
bytes, which is why this packing was chosen over a separate edge-id table. Ids are
opaque to consumers (assigned at pack time, meaningless across files).

Edge record (`15 + 4 × (Pt Count - 1)` bytes):

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Length M | 4 | `uint32` | Ground length in meters (equals the adjacency entries' `Cost M`) |
| 4 | Pt Count | 2 | `uint16` | Polyline vertex count (≥ 2) |
| 6 | Way Kind | 1 | `uint8` | The edge's packed class byte (§8.6), same value as the adjacency entries' |
| 7 | Anchor Lat | 4 | `int32` | First vertex latitude, **absolute** microdegrees |
| 11 | Anchor Lon | 4 | `int32` | First vertex longitude |
| 15 | Deltas | 4 × (Pt Count − 1) | | Per vertex: `dlat int16, dlon int16`, chained from the previous vertex |

The polyline runs from endpoint `a` to endpoint `b` inclusive (first vertex = `a`'s
coord, last = `b`'s); a consumer walking the edge from `b` reverses it. Deltas are
**lat-first** like every §7/§8 record (the geometry sections §5 are lon-first —
anchors there are viewport-space `x, y`).

Packer guarantees that make the fixed `int16` deltas, the `int16` neighbor deltas
(§8.3), the `uint16` cost, and the no-straddle rule all hold **by construction**:

- **Densification.** Any segment whose lat **or** lon delta exceeds `30000`
  microdegrees is subdivided with interpolated vertices — the same threshold as
  §5 geometry and the OBCR track encoding. Readers need no special handling.
- **Edge splits.** `nav.rs` splits any edge whose endpoint-to-endpoint lat/lon
  delta exceeds `32000` µdeg (so the §8.3 neighbor delta fits `int16`) or whose
  `Length M` exceeds `60000` m (so `Cost M` fits `uint16`), into pieces joined by
  **synthetic degree-2 junctions** (new dense ids past the real ones). The
  serializer additionally splits any piece whose densified record would exceed one
  chunk (`Pt Count > (512 − 15) / 4 + 1`, i.e. 125 points) or whose endpoint span
  would exceed the `int16` bound after densification. Routing-neutral: each piece's
  `Length M` is re-measured over its sub-polyline, so costs still sum to the
  original.

### 8.5 Worked example

A minimal graph — two junctions `A`(lat 100, lon 200) and `B`(lat 900, lon 800)
joined by one 3-vertex edge of 1234 m and way-kind `0x2A` (tertiary/paved: highway
class 10 `| (`surface class 1 `<< 5)`) that climbs 300 m from `A` to `B` and
re-climbs 42 m of dips on the way back — with one profile "`Road`" (climb weight
10), with the section at a 512-byte-aligned file offset `S`:

```
S+0    Nav Directory (40 B):
         index_offset          = S+508     (node chunks begin at S+512)
         index_node_count      = 1
         node_chunk_count      = 1
         edge_pool_offset      = S+1024    (= S+508 + 4 index + 512 node chunk)
         edge_chunk_count      = 1
         chunk_size            = 512
         profile_table_offset  = S+40
         profile_count         = 1
         reserved              = 0
         snap_index_offset     = S+2044   (snap chunks begin at S+2048)
         snap_index_node_count = 1
         snap_chunk_count      = 1
S+40   Profile Table (56 B):
         profile 0: name="Road"      (12 B, 0xFF-padded)
                    highway[32]       (u8 1/16 multipliers)
                    surface[8]
                    climb_weight=10   (1 B)
                    reserved          (3 B, zero)
S+96   Alignment Padding (412 B, zero)
S+508  Node Quadtree (4 B):  [0x00000000]        single leaf → node chunk 0
S+512  Node Chunk 0 (512 B):
         rec A: lat=100 lon=200 id=0 degree=1
                nbr { id=1, dLat=+800, dLon=+600, edge_id=0, cost_m=1234,
                      way_kind=0x2A, ascent_m=300 }                          (30 B)
         rec B: lat=900 lon=800 id=1 degree=1
                nbr { id=0, dLat=-800, dLon=-600, edge_id=0, cost_m=1234,
                      way_kind=0x2A, ascent_m=42 }                           (30 B)
         0xFF × 452                                (padding = sentinel)
S+1024 Edge Pool chunk 0 (512 B):
         edge 0 (at pool offset 0 ⇒ edge_id = 0):
           length_m=1234  pt_count=3  way_kind=0x2A  anchor=(lat 100, lon 200)
           deltas: (+400,+300) (+400,+300)          → (500,500), (900,800)   (23 B)
         0xFF × 489                                 (padding)
S+1536 Snap Alignment Padding (508 B, zero)
S+2044 Snap Quadtree (4 B): [0x00000000]            single leaf → snap chunk 0
S+2048 Snap Chunk 0 (512 B):
         four 12-byte interior anchors naming edge_id=0
         0xFF × 464                                 (padding = sentinel)
```

Node `A` reconstructs neighbor `B` as `(100 + 800, 200 + 600) = (900, 800)` — no
edge fetch needed for `h`. Both directions of the edge carry `edge_id = 0`,
`cost_m = 1234` and `way_kind = 0x2A`; only `ascent_m` differs, and that is the
v12 exception above — the same road costs 300 m of climb uphill and 42 m down.
Under "`Road`" the uphill arc weighs `(1234 × 16) >> 4 + 300 × 10 = 4234` and the
downhill one `1234 + 42 × 10 = 1654`. Fetching the edge decodes the polyline
`(100,200) → (500,500) → (900,800)`, its way-kind `0x2A`, and its 1234 m length in
one ≤ 512-byte read.

### 8.6 Profile table (bike-type routing)

`Profile Count` (1..=8) consecutive **56-byte** records at `Profile Table Offset`,
one per selectable bike profile (Road / Gravel / MTB / Touring by default). The
device picks one by index; A\* weights each edge by it. The table is **always
present** — even an empty graph carries ≥ 1 profile — and the reader rejects a
`Profile Count` of `0` or `> 8`.

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Name | 12 | `char[12]` | UTF-8, `0xFF`-padded (the §7.3 POI-name convention) |
| 12 | Highway Multipliers | 32 | `uint8[32]` | Weight per **highway class**, `1/16` fixed-point; `16` = 1.0×, `0` = **forbidden** |
| 44 | Surface Multipliers | 8 | `uint8[8]` | Weight per **surface class**, same encoding |
| 52 | Climb Weight | 1 | `uint8` | **v12**: flat metres charged per metre of §8.3 `Ascent M`. `0` = climb-blind |
| 53 | Reserved | 3 | `uint8[3]` | Written `0`; readers MUST ignore |

Stock values are Road `10` / Gravel `8` / MTB `6` / Touring `8` — a road rider
detours further to avoid a climb than a mountain biker does. `0` is a legal and
meaningful value: it reproduces v11's costing exactly, and it is what a producer
writes when it has no opinion.

The **effective multiplier** for an edge whose packed `Way Kind` is `k` is:

```
mh = highway_mult[k & 0x1F]      # low 5 bits = highway class (0..=31)
ms = surface_mult[k >> 5]        # high 3 bits = surface class (0..=7)
effective = (mh × ms) >> 4       # u32 math; 16×16>>4 = 16 = 1.0×
```

The edge is **forbidden** (not routable under this profile) if either byte is `0`.
The weighted A\* cost of the edge is

```
weighted = (Cost M × effective) >> 4  +  Ascent M × Climb Weight     # saturating
```

(saturating into the `uint16` frontier cost exactly as v8 did).

**Admissibility invariant (normative).** Every **non-zero** multiplier is `≥ 16`
(i.e. `≥ 1.0×`). This keeps the great-circle heuristic admissible, so the existing
`ε = 1.3` bound survives — now meaning "≤ 1.3× the best route *under the profile*".
The packer **rejects** a config whose quantized weight is non-zero but `< 16` with
an error naming this A\* heuristic bound; the reader **clamps** a non-zero
multiplier `< 16` up to `16` defensively (a hand-forged file can't hand the router
an inadmissible weight).

**The climb term is additive and non-negative (normative, v12).** `Ascent M` and
`Climb Weight` are both unsigned and the term is *added*, so a descent MUST NOT
reduce an edge's cost below its profile-weighted ground length. That is what keeps
the great-circle heuristic admissible in the presence of elevation — a
descent-credit formulation would let an edge cost less than the straight-line
distance the heuristic assumes, and the `ε`-ladder's guarantee would go with it.
`Climb Weight` therefore needs no lower bound the way a multiplier does: every
`uint8`, `0` included, is admissible. Range check: the worst real edge (60 km,
3000 m of ascent, the §8.4 split bounds) at `Climb Weight = 15` is
`60 000 + 45 000`, inside the existing saturating arithmetic.

#### Canonical way-kind table (normative)

`Way Kind = (surface_class << 5) | highway_class`. This mirrors the packer's single
source of truth (`obc-pack/src/nav.rs` — `highway_class` / `surface_class` /
`classify`); profile configs and the web builder key their multipliers by these
class names.

**Highway class** (5 bits, `0..=31`; `0..=13` assigned, `14..=31` reserved):

| id | class | OSM `highway=` |
|----|-------|----------------|
| 0  | cycleway | `cycleway`, `cycleway_link` |
| 1  | path | `path`, `path_link` |
| 2  | track | `track` |
| 3  | footway | `footway`, `pedestrian`, `footway_link` |
| 4  | steps | `steps` |
| 5  | bridleway | `bridleway`, `bridleway_link` |
| 6  | living_street | `living_street`, `living_street_link` |
| 7  | residential | `residential` |
| 8  | service | `service`, `service_link` |
| 9  | unclassified | `unclassified`, `road` |
| 10 | tertiary | `tertiary`, `tertiary_link` |
| 11 | secondary | `secondary`, `secondary_link` |
| 12 | primary | `primary`, `primary_link` |
| 13 | trunk_cycl | `trunk`/`trunk_link` **only when** `bicycle=yes` |

**Surface class** (3 bits, `0..=7`):

| id | class | OSM `surface=` |
|----|-------|----------------|
| 0  | unknown | absent / unrecognized |
| 1  | paved | `paved`, `asphalt`, `concrete`, `paving_stones`, `concrete:plates`, `concrete:lanes` |
| 2  | compacted | `compacted`, `fine_gravel` |
| 3  | gravel | `gravel`, `pebblestone`, `unpaved` |
| 4  | dirt | `ground`, `dirt`, `earth` |
| 5  | rough | `sand`, `mud` |
| 6  | cobbles | `cobblestone`, `sett`, `unhewn_cobblestone` |
| 7  | grass | `grass`, `grass_paver` |

**Bike legality** (which ways make it into the graph at all): a way is dropped when
`highway=motorway|motorway_link`; `highway=trunk|trunk_link` without `bicycle=yes`;
`motorroad=yes`; `bicycle=no|use_sidepath`; or `access=no|private`. Everything else
— including `footway`/`steps` (legal to *walk* a bike) — is kept; preference (not
legality) is the profile's job.

### 8.7 Sparse exact-edge snap index (v13)

The edge pool is followed by a second quadtree index and `Snap Chunk Count` fixed 512-byte chunks.
The quadtree has §8.2's identical flat encoding, global bbox, subdivision, split floor and first-fit
leaf bin packing. Consequently distinct leaves may reference one shared chunk and readers MUST
filter records by their absolute coordinate.

Each record is 12 bytes:

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Lat | 4 | `int32` | Anchor latitude, absolute microdegrees |
| 4 | Lon | 4 | `int32` | Anchor longitude, absolute microdegrees |
| 8 | Edge Id | 4 | `uint32` | Pool-relative id of the §8.4 edge geometry to project |

Unused chunk tails are `0xFF`; `Edge Id == 0xFFFFFFFF` is the sentinel. A final serialized edge
piece contributes no record when its measured geometry is at most 300 m. Otherwise the producer
chooses `ceil(length / 300)` equal-length intervals along the polyline and writes the `intervals − 1`
interior boundaries. Thus endpoint/anchor gaps are no more than 300 m without adding routable graph
nodes or changing A* topology.

The coverage guarantee above requires every generated record to reach the index. A producer MUST
report a split-floor leaf overflow and its dropped-record count; a map release claiming complete
100 m lookup coverage MUST have a dropped count of zero.

The anchor coordinate is never returned as the route endpoint. A reader uses it only to obtain a
small candidate `Edge Id` set, projects the requested coordinate segment-by-segment onto each full
§8.4 polyline, selects the nearest projection (lower `Edge Id` breaks an exact distance tie), and
resolves the winning edge's two endpoint node records. Routing represents an interior projection as
a virtual node with two partial-edge arcs; emission clips the first/last polyline at the same stored
segment/fraction. Exact edge projection is the normal endpoint operation; the 251 m query is only
the candidate-discovery window, while the final result is accepted against its true point-to-road
distance (100 m in the reference router).

---

## Reference implementations

- **Format authority (Rust, no_std):** `firmware/obc-formats/src/obcm.rs`
  (version, fixed record lengths, flags, sentinels, POI ids/categories/labels) and
  `firmware/obc-formats/src/io.rs` (checked little-endian primitives + the neutral
  byte-source/sink seam). It contains no reader, packer, cache, or rendering policy.
- **Writer (Rust, std host):** `host/obc-pack/src/serialize.rs` (`serialize_lods`,
  `serialize_tree`, `serialize_poi_section`, `serialize_nav_section`,
  `flatten_nav_tree` (§8.2 bin-packing), `pack_nav_record`, `pack_edge_record`,
  `pack_profile_table`, `pack_feature`, `pack_chunk`, `pack_style_dict`),
  `host/obc-pack/src/poi.rs` (the OSM-tag classifier for the shared §7.4 ids),
  `host/obc-pack/src/hours.rs` (the `opening_hours` parser + 29-byte blob
  encoder + dedup pool for §7.5), `host/obc-pack/src/nav.rs` (the routable-graph
  builder + the canonical way-kind table behind §8.6), and
  `host/obc-pack/src/config.rs` (the `routing` config + profile quantization).
- **Reader + renderer (Rust, no_std):** `firmware/obc-reader` — `reader.rs`
  (`Reader`, `for_each_feature`, `select_lod_for_mpp`, the POI + nav directories +
  the profile table in `MapTables`, `for_each_nav_node`, `NavNeighbor` delta
  decode, `nav_edge`, `MapProfile::multiplier`, `MapProfile::climb_weight`) — and
  `firmware/obc-render`
  (`Viewport`, `RenderScratch`). Format-contract tests in
  `firmware/obc-reader/tests/format.rs` (byte pins) and
  `host/obc-pack/tests/nav_round_trip.rs` (writer↔reader §8 round trip, incl.
  the profile table, kinds, delta reconstruction, and the bin-packing fill floor).
