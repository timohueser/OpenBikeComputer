# OBCM map format

A map (OpenStreetMap Binary Chunked Map): light and dark style tables, geometry in levels of
detail, POIs, a routable navigation graph, optional landmark and peak content, and an optional
embedded terrain region.

- Version: 19 (`0x13`). A reader MUST check `Version` before it reads any later field and MUST
  refuse every other value. The header version applies to the whole file. Repack older maps.
  Style flag bits get new meanings without a version change (§2).
- Writers: `host/obc-bake` (`serialize.rs`, `serialize_lods`) composes a complete map from the
  crates under [Reference implementations](#reference-implementations).
- Readers: `firmware/obc-reader` (`src/reader/mod.rs`, `src/reader/nav.rs`), shared by the desktop
  simulator and the nRF54L firmware.
- Code authority: `firmware/obc-formats/src/obcm.rs` (version numbers, fixed lengths, flags,
  sentinels, the canonical POI id table); `firmware/obc-formats/src/io.rs` (little-endian
  primitives). Producers and consumers import these facts directly.
- Test vectors: [`vectors/`](vectors/README.md) (`place-train-v15.bin` §7.3,
  `landmark-section-v16.bin` §9, `peak-section-v17.bin` §10).
- Related: [OBCA](obca-assembly.md), [OBCT](obct-terrain.md), [OBCC](obcc-catalog.md),
  [OBCR](obcr-route.md).

## Conventions

- Multi-byte integers are little-endian.
- Coordinates are integer microdegrees (1e-6 degrees).
- Every section is reached through an explicit offset and every count is stored. A reader does no
  traversal or sizing work to parse the structure.

## File layout

```
[Header]                            (71 bytes, fixed)
[Light Style Table]                 (global — shared by all LODs)
[LOD Table]                         (LOD Count entries)
[LOD 0 Index][LOD 0 Offset Table][LOD 0 Data Chunks]    (coarsest)
[LOD 1 Index][LOD 1 Offset Table][LOD 1 Data Chunks]
...
[LOD N-1 Index][LOD N-1 Offset Table][LOD N-1 Data Chunks] (finest)
[POI Directory][POI Indexes + Chunks] (§7)
[Hours-Pool Section]                  (§7.5)
[Nav Directory][Profile Table][Node Index + Chunks][Edge Pool][Snap Index + Chunks]  (§8)
[Landmark Directory + Content]        (§9 — optional)
[Peak Associations + Articles]        (§10 — optional)
[Terrain Region]                      (§1.3 — an OBCT container, absent when the header says 0)
[Dark Style Table]                  (global — shares IDs with the light table)
```

Every structure a header or directory offset reaches begins on a **unit boundary** (§1.1). Where
the previous structure does not end on one, `0..U-1` bytes of `0xFF` filler follow it (§1.2).

---

## 1. Header (71 bytes)

Packed as `struct "<4sBiiiiIBIHIIBIIIIIIIH"`.

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Magic | 4 | `char[4]` | Must be `b"OBCM"` |
| 4 | Version | 1 | `uint8` | `0x13` |
| 5 | Min Lat | 4 | `int32` | Global bbox min latitude (microdegrees) |
| 9 | Min Lon | 4 | `int32` | Global bbox min longitude |
| 13 | Max Lat | 4 | `int32` | Global bbox max latitude |
| 17 | Max Lon | 4 | `int32` | Global bbox max longitude |
| 21 | Style Offset | 4 | `uint32` | **Scaled** offset to the Style Table |
| 25 | LOD Count | 1 | `uint8` | Number of LOD levels (≥ 1) |
| 26 | LOD Table Offset | 4 | `uint32` | **Scaled** offset to the LOD Table |
| 30 | Marker Color | 2 | `uint16` | User-position marker color (RGB565) |
| 32 | POI Section Offset | 4 | `uint32` | **Scaled** offset to the POI Directory (§7) |
| 36 | Nav Graph Offset | 4 | `uint32` | **Scaled** offset to the Nav Directory (§8) |
| 40 | Offset Scale | 1 | `uint8` | Base-2 logarithm of the offset unit in bytes, `0..=9`; producers write `4` (§1.1) |
| 41 | Terrain Offset | 4 | `uint32` | Scaled offset to the embedded OBCT region, or `0` for a map with no elevation (§1.3) |
| 45 | Terrain Length | 4 | `uint32` | That region's length **in units**; `0` exactly when `Terrain Offset` is `0` |
| 49 | Landmark Offset | 4 | `uint32` | Scaled offset to the optional landmark section (§9) |
| 53 | Landmark Length | 4 | `uint32` | Section length in offset units; `0` exactly when Landmark Offset is `0` |
| 57 | Peak Offset | 4 | `uint32` | Scaled offset to the separate peak collection (§10) |
| 61 | Peak Length | 4 | `uint32` | Section length in offset units; `0` exactly when Peak Offset is `0` |
| 65 | Dark Style Offset | 4 | `uint32` | **Scaled** offset to the Dark Style Table (§2) |
| 69 | Dark Marker Color | 2 | `uint16` | Dark user-position marker color (RGB565) |

The bbox field order in the file is **lat, lon, lat, lon**. A **scaled** offset is a count of
`2^Offset Scale`-byte units, not of bytes (§1.1). This applies to every field marked scaled, here
and in the LOD table (§3), the offset tables (§5.1) and the POI (§7.1) and nav (§8.1) directories.

The 71-byte header is not a whole number of units at any scale above `0`. The Light Style Table
begins at the first unit boundary at or after it: `80` at the default `U = 16`, so
`Style Offset = 5`, and bytes `71..80` are `0xFF` filler (§1.2). A reader follows `Style Offset`; it
does not assume that the section follows the header. The POI and nav sections are always present,
so their offsets are never `0`: a map with no POIs (or no routable ways) writes an **empty**
directory. `Terrain Offset`, `Landmark Offset` and `Peak Offset` may be `0`; each is zero exactly
when its length is zero.

### 1.1 Offset scale

`Offset Scale` is the base-2 logarithm of the **unit** every scaled offset in the file counts:

```
U           = 1 << Offset Scale        # bytes per unit
byte_offset = u64(field) * U           # 64-bit arithmetic, always
```

A reader MUST widen to 64 bits before it multiplies: a `u32(field) * U` product wraps silently and
returns the wrong section.

Producers write `4` (this also pins byte determinism), so `U = 16` and a file's addressable
interior is `2^32 × 16 = 64 GiB`. Legal values are **`0..=9`**; a reader MUST refuse any other
value with an error **distinct from the version check**, because a bad scale is a corrupt file, not
an old one. At scale `0` a unit is one byte.

At every legal scale `U` divides 512, the card block. §7's POI chunks and §8's node, edge and snap
chunks are 512-byte strides from their region's start, so each of them starts on a unit boundary
with no filler between them.

**The scale MUST cover the file it writes**: `2^32 × U` MUST be at least the file's total length.
The largest legal file is exactly `2^32 × U` bytes. A file whose bytes reach past what its scale can
address is malformed.

### 1.2 Alignment, filler, and what it costs

A scaled offset can name only a multiple of `U`, so **every structure a scaled offset reaches begins
on a unit boundary**. Where the next such structure would otherwise begin mid-unit, a writer writes
`0xFF` filler up to the boundary. Filler is never content. It occurs:

- **between sections**: the 71-byte header and a style table, and any two sections a header or
  directory offset names;
- **before a region's chunks**: chunk data begins at the first unit boundary at or after the
  structure before it, which is the index (§7.1, §8.1) or the index plus the offset table (§3);
- **between offset-table-addressed chunks** (§5.1): chunk `k`'s content ends at its `0xFF`
  sentinel, and chunk `k+1` starts at the next unit boundary.

No offset in the file names a filler byte. A chunk's content ends at its sentinel and a record's at
its own length. `0xFF` is the fill because it is the "nothing here" byte of every chunked section
(the style-id sentinel §5.1, the POI subtype sentinel §7.3, the nav degree sentinel §8.3, the edge
`Pt Count` sentinel §8.4), so filler that leaks into a decode path meets a stop. Reserved
**fields** are still written `0`.

A walk through a chunk ends at the sentinel **or at the chunk's end, whichever comes first**. A
reader MUST apply both bounds. A 512-byte §8.7 snap chunk holds at most `floor(512 / 12) = 42`
twelve-byte anchors and leaves an eight-byte tail that cannot hold a full record (its `Edge Id`
would be at bytes `512..516`), so the byte count bounds that walk and the sentinel bounds the
others.

**What it costs.** Only §5's offset-table-addressed geometry chunks have a gap per chunk, of
`0..U-1` bytes. §7 and §8 chunks start on unit boundaries already (§1.1).

Each region and section boundary has one gap of `0..U-1` bytes: two per LOD (its index and its
`data_start`), plus the fixed ones at the header, the style and LOD tables, the POI categories, the
hours pool, the nav section and the terrain region. The gaps are part of the file, and two bakes of
the same input have the same gaps.

### 1.3 The terrain region

`Terrain Offset` and `Terrain Length` are a scaled pointer to a region at the file tail that holds
one [OBCT](obct-terrain.md) container, byte for byte the bytes that `obc-dem` bakes and the
assembler splices. Terrain is last so that splicing it moves no other offset.

**Terrain is part of the map.** A terrain re-bake re-emits the map. There is no terrain-only update
path and no separable raster object.

`Terrain Offset == 0` means **the map carries no elevation**, and `Terrain Length` MUST then be `0`;
a reader MUST refuse a file that sets one without the other. No region can begin at byte `0`,
because the header occupies it.

**A reader hands the region over; it does not parse it.** A reader forms a **window**: a byte
source whose offset `0` is the region's first byte and whose length is `Terrain Length × U`. It
gives the window to the terrain consumer, which reads it as [`obct-terrain.md`](obct-terrain.md)
describes for a terrain file. Every offset inside the container is relative to its own first byte,
so a window is sufficient and a copy is not necessary.

- **The window is up to `U - 1` bytes longer than the container.** `Terrain Length` is the
  container's byte length rounded up to units, and the tail is §1.2 filler. A reader MUST NOT derive
  the payload length from the region length, and a consumer MUST NOT read past what the container's
  own structure addresses.
- **A terrain region that does not parse is not a broken map.** Elevation is an enhancement
  (`obcc-catalog.md` §13): a reader whose OBCT parse fails MUST fall back to no elevation, MUST still
  mount, render and route, and MUST NOT present the map as faulty. A **writer** MUST verify the
  region it splices.

### Marker Colors

The **user-position marker** is a chevron at the user's GPS fix that points along the course. It is
not an OSM feature, so its colors are header fields, not style records. They are RGB565. The
renderer selects the light or dark marker with the matching style table. The renderer fixes the
marker's shape and size.

---

## 2. Style Tables

The light and dark tables map the same numeric style IDs to independently authored rendering
properties. Style IDs are global across every LOD and both tables. Both tables MUST contain exactly
the same nonzero IDs. A reader MUST reject a missing, duplicate or mismatched ID. Geometry is stored
once and selects the active record by its shared ID, so a table change does not reload or convert
geometry. Each table is:

1. **Count** (`uint8`): number of styles.
2. **Style Records** (`Count` × 8 bytes):

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | ID | 1 | `uint8` | Style ID, referenced by feature headers |
| 1 | Z-Index | 1 | `int8` | Painter's-order layer (lower drawn first) |
| 2 | Color | 2 | `uint16` | RGB565 (the primary color) |
| 4 | Weight | 1 | `uint8` | Stroke width in pixels (lines) |
| 5 | Flags | 1 | `uint8` | Bits 0-1: priority level (1=highest/render first, 4=lowest/render last). **Bit 2: dashed** line (ignored for polygons). **Bit 3: color2 present.** **Bit 4: fixed width.** **Bit 5: terrain layer.** **Bit 6: ticked** line — a solid stroke with regular perpendicular ticks, the cableway mark; a writer MUST leave bit 2 clear with it, and a reader that meets both MUST draw the line ticked. A line with neither bit 2 nor bit 6 is solid. Bit 7 reserved, written 0 — a reader MUST **ignore** it, not reject the record (see below) |
| 6 | Color2 | 2 | `uint16` | RGB565 **secondary color**. Written `0x0000` when flag bit 3 is clear; readers MUST ignore it then (`0x0000` is a legit color — black — not a "no color2" sentinel) |

The **secondary color** and **line style** drive the finest-LOD line and polygon embellishments
(road casing, dashed admin borders, railway stripes, polygon ring outlines); the renderer owns their
semantics. A solid, single-color style has flag bits 2-3 clear and `Color2 = 0x0000`.

**Bit 4 — fixed width.** `Weight` is the stroke width in **device pixels**, used verbatim: the
renderer's zoom-to-width ramp does not apply to this style. The renderer still clamps the width to
its `1..=12` px range. Ignored for polygons.

**Bit 5 — terrain layer.** The style belongs to the **terrain layer**: the group a device may
suppress as one user choice. It is presentation metadata; no reader behavior depends on it, and a
renderer that ignores it draws a correct map.

A reader MUST **ignore** the undefined bits of a style record, never reject the record. Defining a
bit is therefore not a version change: no offset, length or count moves.

**The packer assigns style IDs.** No reader depends on a specific value, only on uniqueness within
the file. The packer ignores any `id` in `config.json` and numbers every feature type sequentially
(`1`-based, in document order). `0xFF` is reserved as the end-of-features sentinel (§4), so a file
holds at most 254 distinct styles.

---

## 3. LOD Table

`LOD Count` entries, ordered **coarsest (index 0) → finest (index N-1)**. Each
entry is 18 bytes, packed as `struct "<fIIHI"`.

| Field | Size | Type | Description |
| :-- | :-- | :-- | :-- |
| Max Meters/Pixel | 4 | `float32` | Upper bound of the m/px range this LOD covers. Strictly decreasing down the list; the coarsest level is `+inf` (`f32::INFINITY`). |
| Index Offset | 4 | `uint32` | **Scaled** offset to this LOD's quadtree index (§1.1) |
| Index Node Count | 4 | `uint32` | Number of `uint32` nodes in the index |
| Chunk Size | 2 | `uint16` | **Capacity bound** of one data chunk (bytes), per-LOD. Not a stride; see below |
| Chunk Count | 4 | `uint32` | Number of data chunks in this LOD |

A LOD's region is three parts:

```
[Quadtree Index]   Index Node Count × uint32          at index_start
[Offset Table]     (Chunk Count + 1) × uint32         at table_start, immediately after the index
[Chunk Data]       unit-aligned chunks                at data_start (below)
```

```
index_start = Index Offset * U                                       # U = 1 << Offset Scale
table_start = index_start + Index Node Count * 4
data_start  = align_up(table_start + (Chunk Count + 1) * 4, U)       # = table_start + ... , rounded up
chunk k     = data_start + offsets[k] * U .. data_start + offsets[k+1] * U
```

where `align_up(x, U) = (x + U - 1) & !(U - 1)`. All of it is `u64` arithmetic (§1.1). The index
and the offset table are read by 4-byte indexing and need no unit boundary of their own. The
**chunks** are addressed by scaled offsets, so `data_start` must be a unit boundary. The `0..U-1`
bytes it rounds past are §1.2 filler.

**`Chunk Size` is a bound, not a stride.** It is the packer's leaf-split threshold and the largest
length any single chunk may have; a reader MUST reject a chunk whose offset pair spans more than
`Chunk Size` rounded up to the unit (§5.1). Chunk lengths come from the offset table (§5).

`Index Node Count` and `Chunk Count` are stored, so the reader never walks the tree to learn its
size. The offset table's last entry (`offsets[Chunk Count]`) is the LOD's total chunk
bytes, so one `uint32` read at parse bounds every later chunk fetch.

---

## 4. Quadtree Index (per LOD)

A flat array of `Index Node Count` × `uint32`. **Every LOD's quadtree is built over the same
global bbox** (from the header), so node bboxes are identical at every level.

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

### 5.1 Offset table and tight chunks

A LOD's chunk data is addressed by its own **offset table**, written between the
quadtree index and the chunks (§3):

- `Chunk Count + 1` `uint32` entries. Each is a **scaled** offset (§1.1) relative to `data_start`,
  the start of the chunk-data region — so entry `e` names byte `data_start + e * U`.
- `offsets[0]` is always `0`. Offsets are non-decreasing. `offsets[Chunk Count]` is
  the region's total chunk **units**, and `offsets[Chunk Count] * U` its bytes.
- Chunk `k` occupies `data_start + offsets[k] * U .. data_start + offsets[k+1] * U`; its **span** is
  the difference in bytes, and its **content** is the shorter run ending at its sentinel.
- The table is written even when `Chunk Count == 0`, where it is the single `0` entry.

Each chunk is its packed features, then **exactly one** `0xFF` `CHUNK_END` sentinel byte, then
`0..U-1` bytes of `0xFF` filler up to the next unit boundary (§1.2). A `0xFF` style ID is
impossible, so the sentinel marks end-of-features; the offset-derived end is a second, independent
bound. A reader MUST treat a chunk whose feature stream reaches the offset-derived end **without**
meeting the sentinel as malformed (truncated), not as a clean finish.

A reader MUST validate an offset pair before using it, because `Chunk ID` comes from
a quadtree leaf and is arbitrary in a corrupt map: `k < Chunk Count`,
`offsets[k] <= offsets[k+1]`, `offsets[k+1] <= offsets[Chunk Count]`, and
`(offsets[k+1] - offsets[k]) * U <= align_up(Chunk Size, U)`.

A chunk's *content* may not exceed `Chunk Size`; its *span* is that content rounded up to a unit,
so `align_up(Chunk Size, U)` is the bound (4,096 for the shipped 4,096-byte bound at `U = 16`). Do
not use `Chunk Size + U - 1`: it admits spans that no writer can produce.

### 5.2 Feature Header (7 or 12 bytes)

`Flags` is at byte **1** in both layouts. Its `0x08` **WIDE** bit selects the layout, so a reader
knows the header's width before it reads any later field.

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
components are in `0..=65535`, and wide otherwise. A coarse-LOD leaf can span more than
65 535 µdeg (about 7 km), so an anchor in it can need the wide field. Everything after the header
(hole bookkeeping and the delta streams) is identical in both layouts.

The **anchor** is the feature's first absolute coordinate, stored relative to the containing leaf
node's min corner:

```
anchor_abs = (node_bbox.min_lon + AnchorX, node_bbox.min_lat + AnchorY)
```

### Geometry encoding (delta)

Rings are delta-encoded. Bit depth is chosen **per feature**:
if every `dx`/`dy` fits in `int8` (|d| ≤ 127), `Flags & 0x01 == 0` and deltas are
`int8`; otherwise the flag is set and all deltas are `int16`.

Polygon rings are **implicitly closed** from their last vertex back to their first. A writer
should omit a repeated final copy of the first vertex; readers and renderers must accept either
representation. Line geometry is never implicitly closed.

- **Exterior ring** (`Pt Count` vertices): the first vertex *is* the anchor;
  the remaining `Pt Count - 1` vertices follow as `(dx, dy)` pairs, each relative
  to the previous vertex.
- **Holes** (only if `Flags & 0x04`, after the exterior deltas):
  - **Hole Count** (`uint8`)
  - per hole: **Pt Count** (`uint16`), then `Pt Count` `(dx, dy)` delta pairs.
    Holes store **all** vertices as deltas — the first is relative to the feature
    anchor, the rest chain from the previous vertex.

Lines use only the exterior ring (`Flags & 0x02 == 0`, no holes).

> **Long-segment densification:** the packer inserts intermediate vertices on any segment longer
> than `30000` microdegrees so that no delta exceeds the 16-bit range. Readers treat them as
> ordinary vertices.

> **Per-feature vertex cap:** although `Pt Count` is a `uint16`, a single feature (exterior
> plus all holes, densification included) must not exceed **2048 vertices**. A feature past the
> cap is dropped whole with an explicit capacity outcome; no truncated line or polygon is
> exposed. The packer enforces the bound through `Chunk Size`: a feature cannot outgrow its
> chunk, and its packed bytes are at least `7 + 2·(V−1) = 2·V + 5` for `V` total vertices. So
> `Chunk Size ≤ (2048−1)·2 + 7 = 4101` keeps every feature within the cap, and `obc-pack` rejects
> a larger `Chunk Size` at build time.

> **Per-feature ring cap:** although `Hole Count` is a `uint8`, a single feature must not
> exceed **32 rings** (exterior + 31 holes). A feature past it is dropped whole, with the same
> capacity outcome as the vertex cap. Byte size does not imply this bound, so `obc-pack` enforces
> it structurally: a quadtree node that holds an over-cap polygon splits, and at the 10 µdeg split
> floor the smallest holes are dropped to fit.

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

Within a selected LOD, query the quadtree for the viewport, decode the visible chunks, sort
features by style `Z-Index` (painter's algorithm), then draw: polygons by even-odd scanline
fill (holes fall out of the even-odd rule), lines as weighted polylines.

**Backdrop convention.** Before it draws geometry, a renderer clears the screen to the
**backdrop color**: the color of the style with the lowest `Z-Index` (in the shipped schema
`natural.land` at `z_index 0`). It comes from the style table, not from a fixed style ID, because
the packer assigns IDs (§2). The shipped packer writes the coastline complement as `natural.sea`
geometry on top; schemas with a different lowest style remain valid.

---

## 7. POI Section

Point-of-interest features the packer classifies from OSM nodes and closed-way centroids
(§7.4). The service categories are indexed for a nearest-N query and are not rendered on the
map. The optional settlement category 9 is walked by viewport and drawn on the map. Each
category gets its own quadtree over 64-byte point records.

The section is reached from `POI Section Offset` (header offset 32) and is
**always present**: a map with no POIs writes a directory of seven empty service
categories, never a zero offset. Service POI records reference the **hours-pool section**
(§7.5).

### 7.1 POI Directory

```
uint8   Category Count            (7 to 12: seven base services plus optional
                                  landmarks, settlements and additional services)
uint16  Chunk Size                (POI chunk capacity in bytes — the packer writes 512)
per category (Category Count entries, 13 bytes each):
  uint8   Category ID
  uint32  Index Offset            (SCALED offset to this category's quadtree index, §1.1)
  uint32  Index Node Count        (number of uint32 nodes; 0 ⇒ category empty)
  uint32  Chunk Count             (number of data chunks in this category)
uint32  Hours Pool Offset         (SCALED offset to the hours-pool section, §7.5)
uint16  Hours Pool Count          (number of 29-byte blobs; 0 ⇒ no hours in this map)
```

`Chunk Size` is shared by every category. As with a LOD, a category's data chunks begin at
`align_up(Index Offset * U + Index Node Count * 4, U)` (the §3/§4 convention), so the reader reuses
its `walk_leaves` leaf-walk and chunk-offset math. Chunk `k` is that start plus `k * Chunk Size`,
with no filler between chunks (§1.1). An empty category (`Index Node Count == 0`) still has a
directory entry; its `Index Offset` points at where its (zero-length) index would start and
`Chunk Count` is `0`.

The two **hours-pool fields** follow the category entries. The pool begins at
`Hours Pool Offset * U` (§7.5). `Hours Pool Count` is the number of 29-byte blobs there and MUST
equal the `count` written at that offset. `Hours Pool Count == 0` means the map has no hours (the
pool is a bare `0` count); a record's `HoursRef == 0xFFFF` also means "no hours."

### 7.2 Per-category quadtree

Identical to the geometry quadtree (§4): a flat `uint32` array using the same
node encoding (branch bit / empty-leaf sentinel / chunk id), built over the **same
global bbox from the header**, with the same floor-division-midpoint NW/NE/SW/SE
subdivision. A reader walks one as it walks a LOD index, collecting `(chunk_id, node_bbox)` for each
non-empty leaf; the `node_bbox` only prunes the walk, because records store absolute coordinates.

### 7.3 POI records — fixed 64 bytes

Records are packed into `Chunk Size`-byte chunks (512 ⇒ `512 / 64 = 8`
records/chunk). Each record is exactly 64 bytes. A `0xFF` **Subtype** byte marks the end of records
in a chunk; trailing bytes of a partial final chunk are `0xFF`-padded.

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Lat | 4 | `int32` | Latitude, **absolute** microdegrees |
| 4 | Lon | 4 | `int32` | Longitude, **absolute** microdegrees |
| 8 | Subtype | 1 | `uint8` | Canonical subtype id (§7.4); `0xFF` = end-of-chunk sentinel |
| 9 | Name Len | 1 | `uint8` | Length of the stored name in bytes (`0` = unnamed) |
| 10 | Name | 24 | `char[24]` | Printable ASCII for services; UTF-8 for summits and settlements; unused tail bytes are `0xFF` |
| 34 | Payload | 2 | `uint16` or `int16` | Services: `HoursRef`, a 0-based pool index (`0xFFFF` = none). Summits: signed elevation in metres (`-32768` = unknown). Settlements: population in hundreds of people, saturated at `0xFFFE`; `0xFFFF` = unknown. |
| 36 | Source | 8 | `uint64` | OSM source identity: upper two bits node=1, way=2, relation=3; lower 62 bits positive OSM ID |
| 44 | Approach Source | 8 | `uint64` | Explicit mapped access-node identity, or zero when unavailable |
| 52 | Approach Lat | 4 | `int32` | Access coordinate latitude in microdegrees |
| 56 | Approach Lon | 4 | `int32` | Access coordinate longitude in microdegrees |
| 60 | Approach Profiles | 1 | `uint8` | Allowed profile-table indices as bits; zero means unavailable |
| 61 | Reserved | 3 | bytes | Zero |

The metadata tail is shared with landmark source records. An unavailable approach has all
20 approach/reserved bytes zero. An available approach comes from the source node itself or
an explicit source-way member on routable topology. Proximity is not an association. The
source key, approach and profile bits belong to the installed map revision. A runtime planner
must check the current profile and actual graph connectivity before it accepts a visit.
Assemblers preserve the metadata tail and deduplicate service records by source identity,
not coordinate or subtype. Only the hours-pool reference is remapped.

Coordinates are **absolute**, not anchor-relative as in geometry §5. The **category** is not stored
per record: each subtype maps to exactly one category (§7.4), and the quadtree the record came from
also gives it.

Service names are ASCII-folded at pack time to printable ASCII (`0x20..=0x7E`) and capped at
**24 bytes**; the packer transliterates umlauts and accents (`ä → ae`). An unnamed POI
(`Name Len == 0`) shows its subtype's fallback label on the device.

Every stored name, whatever the subtype, is inside the glyph repertoire of the device font: ASCII,
Latin-1 Supplement and Latin Extended-A. The producer spells any other character in Latin or takes
another name tag.

`HoursRef` is a 0-based index into the hours-pool section (§7.5). `0xFFFF` means the POI has no
(parseable) hours. Identical weekly schedules collapse to one pooled blob, so many POIs can share
one `HoursRef`.

Summits (subtype 19) retain their UTF-8 name, case and diacritics. Names must be nonempty,
contain no control characters, and end on a complete character within the 24-byte field.
Their trailer is a signed little-endian elevation in metres, not a pool reference.
The producer uses the OSM `ele` tag and fills missing elevations from the map DEM when available.
`-32768` means unknown; all other `int16` values are valid, including negative heights.
Assemblers preserve this trailer and do not include summits in the hours pool.

Settlements (subtypes 21 to 24) keep their UTF-8 name, case and diacritics, under the same name
rules as summits. The trailer is the population in hundreds of people, as an unsigned
little-endian value. `0xFFFF` means unknown. The producer saturates a larger population at
`0xFFFE`. A settlement has no routable approach, so the producer writes all 20 approach and
reserved bytes as zero. Assemblers keep this trailer and do not put settlements in the hours pool.

### 7.4 Canonical category / subtype table (normative)

This is the **normative home** of the id table; `obc-formats/src/obcm.rs` is its code authority for
subtype ids, categories and fallback labels. `obc-places`' `metadata.rs` adds only the OSM
`key=value` classification that produces each subtype; the device reads the shared table directly.
**Ids are append-only**: an existing row's category or subtype id must never be renumbered.
Subtype `0` is reserved; `0xFF` is the end-of-chunk sentinel and can never be a subtype id.

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
| 7 | Summit landmark | 19 | Named `natural=peak` node | Summit |
| 8 | Train station | 20 | `railway=station` | Train station |
| 9 | Settlement | 21 | `place=city` | City |
| 9 | Settlement | 22 | `place=town` | Town |
| 9 | Settlement | 23 | `place=village` | Village |
| 9 | Settlement | 24 | `place=hamlet` | Hamlet |
| 10 | Restaurant | 25 | `amenity=restaurant` | Restaurant |
| 11 | Cafe | 26 | `amenity=cafe` | Cafe |
| 12 | Fuel station | 27 | `amenity=fuel` | Fuel station |

Additional service categories 10–12 are optional. Fuel stations are distinct from shops.

The seven base service categories (IDs 1–6 and 8) are always present in the directory. Category 7
is an optional landmark index for Peak View and is not part of the service POI browser. The
producer emits it only when named summit nodes exist. Closed-way centroids and unnamed peaks are
excluded. Each subtype belongs to exactly one category; its record must be stored in that category.
Summit coordinates are the original node coordinates rounded to microdegrees. Distinct summits
less than 50 metres apart remain separate; only repeated source identities collapse.

Category 9 is an optional settlement-name index for the map overlay. It is not part of the
service POI browser. The producer writes it only when named settlement nodes or areas exist.
Unnamed settlements are excluded. A settlement area contributes its ring centroid.

### 7.5 Hours-pool section

A single deduplicated pool of weekly opening-hours schedules, written after the last POI category's
chunks and reached from the directory's `Hours Pool Offset` (§7.1). A POI record's `HoursRef`
(§7.3) is a 0-based index into it; identical schedules collapse to one blob.

```
uint16  Count                     (number of blobs; equals Hours Pool Count in the directory)
per blob (Count entries, 29 bytes each):
  uint8   Flags
  per day (7 days, Mon..Sun, 2 slots each):
    uint8  Open Q                 (quarter-hours from midnight, 0..=96)
    uint8  Close Q
```

Blob `i` (a record's `HoursRef == i`) lives at `Hours Pool Offset * U + 2 + i*29`. An
empty pool is the 2-byte `Count == 0`. Hours are parsed and normalized from OSM `opening_hours`
**at pack time**; the device does only a weekday lookup.

**Blob layout (29 bytes).** `Flags` bit 0 is **seasonal**. Bit 1 is **uncertain**:
a rule or interval was dropped, or compilation rounded a source boundary. This includes
all public- or school-holiday exceptions, including `PH off`. Other bits are reserved zero.
Any nonzero flag makes the current opening status **Unknown**. Invalid endpoint bytes,
missing schedules and read failures also produce **Unknown**. Only definite **Closed**
excludes a place; unknown hours must never be presented as open.

**Time convention.** A time-of-day is quarter-hours from midnight, `0..=96` (`96` =
24:00), so the resolution is 15 minutes. Per interval:

- **Unused slot** — `(0, 0)`.
- **Closed day** — both slots `(0, 0)`.
- **Open all day (24 h)** — slot 0 `(0, 96)`, slot 1 `(0, 0)`.
- **Overnight wrap** — `Close Q <= Open Q` (both nonzero): the interval runs past
  midnight, stored as-is (never split across days). E.g. `22:00-02:00` → `(88, 8)`.
- An overnight interval opens on its stored start day and closes on the following day.
  Sunday spillover is evaluated on Monday. Opening is inclusive; closing is exclusive.
- Current status uses trusted UTC plus the configured local UTC offset. An unavailable
  clock or offset authority yields Unknown. Hours are evaluated now, not at predicted arrival.
- A day with more than two intervals is truncated to the first two and the blob's
  `Flags` truncated bit is set.

---

## 8. Navigation-Graph Section

The **routable graph** for the on-device A\* router: junction **nodes** (from OSM node ids shared
across routable `highway=*` ways) joined by undirected **edges** (the polyline between two
junctions, with no junction inside). The packer builds the graph in `nav.rs` (way-kind
classification, bike-legality filter, island pruning, junction split, dedup, edge splits); this
section is its on-wire form.

The section is reached from `Nav Graph Offset` (header offset 36) and is **always
present**: a map with no routable ways writes an empty directory (`Index Node
Count == 0`) — but still carries its profile table (§8.6), never a zero offset.
Layout, in file order:

```
[Nav Directory]     (40 bytes — the graph's resident header, §8.1)
[Filler]            (0..U-1 bytes of 0xFF — the directory is 40 bytes, §1.2)
[Profile Table]     (§8.6 — the four bike-type profiles, always present)
[Filler]            (0..511 bytes of 0xFF in populated files — the producer's 512-byte alignment)
[Node Quadtree]     (§4 encoding over the header global bbox)
[Filler]            (0..U-1 bytes of 0xFF — align_up to the first node chunk, §8.1)
[Node Chunks]       (variable-length junction records, bin-packed, §8.3)
[Edge Pool]         (512-byte chunks; a record is named by (chunk, ordinal), §8.4)
[Filler]            (0..511 bytes of 0xFF)
[Snap Index]        (§8.7 — the sparse exact-edge anchor quadtree)
[Filler]            (0..U-1 bytes of 0xFF)
[Snap Chunks]       (fixed 512-byte anchor chunks, §8.7)
```

There is no id-to-offset table. A\* **re-fetches spatially**: settling a node is one quadtree
descent to its coordinate's leaf plus one chunk read. Each record carries its neighbors'
coordinates **inline**, so relaxation (`f = g + h`) needs no second fetch. Edge geometry is read
only to resolve the two exact projected endpoints and to emit the final route; the A\* search
between those virtual endpoints never fetches geometry. Only the directory and the profile table
(≤ `8 × 56 = 448` B) are resident.

### 8.1 Nav Directory (40 bytes)

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Index Offset | 4 | `uint32` | **Scaled** offset to the node quadtree index (§8.2) |
| 4 | Index Node Count | 4 | `uint32` | Number of `uint32` nodes in the index; `0` ⇒ **empty graph** |
| 8 | Node Chunk Count | 4 | `uint32` | Number of node data chunks (§8.3) |
| 12 | Edge Pool Offset | 4 | `uint32` | **Scaled** offset to the edge pool (§8.4) |
| 16 | Edge Chunk Count | 4 | `uint32` | Number of `Chunk Size`-byte chunks in the edge pool; **at most `2^27`**, the reach of an `Edge Id`'s chunk field (§8.4) |
| 20 | Chunk Size | 2 | `uint16` | Fixed capacity of every nav chunk — **must be `512`** (the reader rejects any other value) |
| 22 | Profile Table Offset | 4 | `uint32` | **Scaled** offset of the §8.6 profile table |
| 26 | Profile Count | 1 | `uint8` | Number of 56-byte profile records; **`1..=8`** (reader rejects `0` or `> 8`) |
| 27 | Reserved | 1 | `uint8` | `0` (keeps the directory even-sized; no other meaning) |
| 28 | Snap Index Offset | 4 | `uint32` | **Scaled** offset to the §8.7 snap-anchor quadtree index |
| 32 | Snap Index Node Count | 4 | `uint32` | Number of `uint32` nodes in the snap index; `0` ⇒ no interior anchors |
| 36 | Snap Chunk Count | 4 | `uint32` | Number of fixed 512-byte snap-anchor chunks following that index |

Node data chunks begin at `align_up(Index Offset * U + Index Node Count * 4, U)` (the §3/§4
convention), so the reader reuses its leaf-walk and chunk-offset math. The packer writes the
**profile table just after this 40-byte directory** (before the node index), so `Index Offset` and
`Edge Pool Offset` point past it. `Profile Table Offset` is scaled, so at `U = 16` the table starts
at the directory's byte 48, after eight bytes of §1.2 filler; at `U = 1` it starts at byte 40.

For a populated graph, producers insert `0..511` bytes of `0xFF` filler after the profile table so
that the first node chunk lands on a 512-byte file offset. Every node chunk is 512 bytes, so
`Edge Pool Offset` is also 512-byte aligned, and a full-chunk read is one physical card command, not
two. This is a **producer guarantee, not a reader validity requirement**: a file without the
alignment is valid and slower.

**The two alignments do not conflict.** 512 is a multiple of `U` at every legal scale (§1.1), so a
512-byte boundary is also a unit boundary. The index start must be a unit multiple, and
`align_up(index_start + 4 × N, U)` must be the 512-byte boundary; both hold for every node count
`N`, because the rounding lets the index end anywhere in the `U` bytes below the target. §8.5 shows
an example.

The edge pool is followed by optional `0xFF` filler, the §8.7 snap index, and its chunks. Producers
align the first snap chunk to a 512-byte file offset, as for the node chunks. An empty graph still
writes `Chunk Size` and the profile table, and points all zero-length data offsets just past the
profile table, as for an empty POI category. A populated graph with no edge longer than 300 m
sets both snap counts to zero and points `Snap Index Offset` just past the edge pool.

**All of §8's filler is `0xFF`**, including the 512-byte alignment run; a reserved field is `0`
(§1.2).

**`Chunk Size` is pinned to 512**, so one chunk read serves one A\* settle. The reader **rejects a
directory whose `Chunk Size` is not 512**, with a parse error distinct from the header version
check. The geometry sections' configurable `chunk_size` (§5) is independent.

### 8.2 Node quadtree

Identical to §4 / §7.2: a flat `uint32` array with the same node encoding (branch
bit / empty-leaf sentinel / chunk id), built over the **same global bbox from the
header**, with the same floor-division-midpoint NW/NE/SW/SE subdivision and BFS
flattening. The packer splits a leaf once its packed records (§8.3) exceed one chunk, by
**bytes** (records are variable-length), with the same 10-µdeg recursion floor. As with POIs, the
`node_bbox` only prunes the walk; record coordinates are absolute.

**Bin-packed chunks.** After building the tree, the packer assigns chunk ids **first-fit over
the leaves in BFS emission order**: each leaf's record block goes into the first open chunk with
room, and a new chunk opens only when none fits.

> **Distinct index leaves may reference the same chunk id**, and leaves that share a chunk can be
> far apart. A walk decodes a shared chunk once per leaf, so a consumer may see the same junction
> record **more than once per query**, and records outside the leaf's own bbox. Consumers must
> therefore be **idempotent**. The reference consumers: A\* settle matches by `Node Id` (a repeat
> is a no-op), and snap keeps the best candidate (a repeat cannot change it). A single leaf's
> records never straddle a chunk boundary.

The index stores exactly one chunk id per leaf; the leaf-to-chunk mapping is many-to-one.

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

Per neighbor entry (17 bytes):

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Neighbor Id | 4 | `uint32` | The adjacent junction's `Node Id` |
| 4 | Neighbor dLat | 2 | `int16` | Its latitude as a **delta from this record's `Lat`** (µdeg) |
| 6 | Neighbor dLon | 2 | `int16` | Its longitude as a delta from this record's `Lon` |
| 8 | Edge Id | 4 | `uint32` | The connecting edge, §8.4 addressing |
| 12 | Cost M | 2 | `uint16` | The edge's raw ground length in meters (the unweighted distance) |
| 14 | Way Kind | 1 | `uint8` | The edge's packed class byte (§8.6) — the input to profile weighting |
| 15 | Ascent M | 2 | `uint16` | **Directional**: the integrated climb, in metres, of riding this edge *from this record's node toward the neighbor*. Saturating; `0` on a map packed without terrain |

The neighbor's absolute coordinate is `(Lat + dLat, Lon + dLon)`; the packer guarantees that both
endpoints of every edge are within `int16` of each other (§8.4), so the delta never overflows.
`Cost M` is the **unweighted** ground distance; the profile-weighted cost A\* accumulates is
`Cost M × effective_multiplier(Way Kind) >> 4 + Ascent M × Climb Weight` (§8.6), computed on the
device at relaxation. The file stores distance and climb, not weight.

`Ascent M` is an **integral over the edge's polyline, not an endpoint difference** (a pass road
between two junctions at 500 m has hundreds of metres of climb in each direction and no net
change). The producer samples elevation along the edge's densified polyline (one sample per vertex
plus interpolated points, so no gap exceeds about 50 m of ground) and folds the
`(distance, elevation)` stream through the shared dead-banded integrator. A stretch with no
elevation coverage contributes nothing: the integrator re-anchors across the gap.

Rules:

- **Sentinel.** Chunks are `0xFF`-padded, so the byte where the next record's `Degree` would be
  reads `0xFF`, and the reader stops there. A record never straddles a chunk boundary, so a chunk
  decodes in isolation.
- **Degree cap: 24.** `13 + 24 × 17 = 421 ≤ 512`, so a cap-degree record always fits one
  chunk. A node past the cap keeps its **first 24** adjacency entries (edge-pool order,
  deterministic) and the packer warns; a dropped arc survives one-way via the neighbor's own
  record. `0xFF` can therefore never be a real degree.
- **Undirected, with one exception.** Every edge appears in both endpoints' records with the
  **same** `Edge Id`, `Cost M`, and `Way Kind`. **`Ascent M` MUST NOT be assumed equal**: the entry
  `a→b` carries `ascent(a→b)` and the entry `b→a` carries `ascent(b→a)`, which is the first
  direction's *descent*. A consumer that verifies "both sides agree" must exclude this field. A
  self-loop (`a == b`, e.g. a lollipop loop) appears **once** in its node's record, carrying its
  forward direction's ascent.
- **Seam determinism.** A producer that cuts one edge into pieces at a cell border
  (`obca-assembly.md` §3) integrates each piece over the **same global elevation
  lattice**, so two neighbouring cells' stubs are each the integral of their own
  geometry over one surface and the pieces' ascents sum to the uncut edge's.
- Degree `0` is valid to decode but the packer never emits it (every junction
  comes from at least one edge endpoint).

### 8.4 Edge pool

Deduplicated edge geometry, fetched at route emit (stitching the A\* came-from chain into the
output polyline) and by endpoint projection. The sum of `Length M` over the chain is the
route's **displayed** distance; the weighted `g` is not a distance. The pool is
`Edge Chunk Count` × 512-byte chunks from `Edge Pool Offset * U`. Records are packed back-to-back;
a record that would cross a chunk boundary moves to the next chunk start (`0xFF` filler fills the
gap), so **no record straddles a chunk**: one chunk read always covers one edge, and "the *n*th
record of a chunk" is well defined.

**Addressing: `Edge Id` is a packed `(chunk, ordinal)` pair.** The `uint32` splits at bit 5:

```
chunk_index = Edge Id >> 5             # 27 bits
ordinal     = Edge Id & 0x1F           #  5 bits
chunk_start = Edge Pool Offset * U + chunk_index * 512
```

`ordinal` is the record's **position within its chunk**, counting from `0`, not a byte offset.
Ids are opaque to consumers (assigned at pack time, meaningless across files), and the pool has
**zero resident index bytes**.

**Resolving one.** A reader reads the 512-byte chunk at `chunk_start` and walks `ordinal` records
from its first byte, taking each record's length from its own `Pt Count`. Every record the walk
touches, intermediate and target, gets the **same four checks** (`ordinal + 1` times):

```
# `p` is a byte position in 0..=512. Every bound below is written ADDITIVELY on `p`.
step(p):
    if p + 19 > 512:            refuse   # no record fits: 19 B is the format's smallest
    n = u16_at(p + 4)                    # Pt Count
    if n == 0xFFFF:             refuse   # end-of-chunk sentinel: no record here
    n = n & 0x7FFF                       # low 15 bits are the point count
    if n < 2:                   refuse   # impossible count; also what stops 4*(n-1) underflowing
    len = 15 + 4 * (n - 1)
    if p + len > 512:           refuse   # record claims bytes past its chunk
    return len

p = 0
repeat ordinal times:
    p += step(p)                         # every intermediate record is bounds-checked too
step(p)                                  # the target record, same four checks
```

**`512 - p` MUST NOT appear anywhere in that walk, in any width**: once a corrupt `Pt Count` moves
`p` past `512`, the unsigned subtraction wraps, the guard passes, and `u16_at(p + 4)` reads outside
the chunk.

**Evaluate `15 + 4 × (n - 1)` in at least 32 bits** (§1.1's widening rule). `n` is a `uint16`, but
for the largest `n` the guards let through, `0xFFFE`, `len = 15 + 4 × 65 533 = 262 147`; in 16 bits
that wraps to `3` and passes the `record claims bytes past its chunk` check.

Four refusal rules, and a reader MUST apply all of them, because an `Edge Id` reaches it from an
adjacency entry or a snap record and is arbitrary in a corrupt map: `chunk_index < Edge Chunk
Count`; no record may start where one cannot fit; the walk MUST NOT pass the chunk's last record (an
`ordinal` past it is invalid, never a neighbouring record, and never a record of the *next* chunk,
so the bound is `512` and not the pool's end); and no record may claim bytes past its chunk. A
refused id is a malformed map, not an absent edge.

**`Pt Count == 0xFFFF` is the end-of-chunk sentinel.** Chunks are `0xFF`-filled, so the two bytes
at `p + 4` of a gap already spell it. `Pt Count` is at least `2` in every real record, so `0xFFFF`
is impossible content (as for the sentinels of §5.1, §7.3 and §8.3). A gap shorter than six bytes
cannot be read for it, which is what the `512 - p < 19` test covers: no record fits there either
way.

**Five ordinal bits.** An edge record is `15 + 4 × (Pt Count − 1)` bytes with `Pt Count ≥ 2`, so
the smallest record is **19 bytes** and a 512-byte chunk holds at most `floor(512 / 19) = 26` of
them. Five bits name `0..=31`, which covers 26, and leave **27** for the chunk index:

```
pool ceiling = 2^27 chunks × 512 B/chunk = 2^36 B = 64 GiB
interior     = 2^32 units × 16 B/unit    = 2^36 B = 64 GiB      (§1.1, at the default scale 4)
```

`Edge Chunk Count` MUST be at most `2^27`, and a reader MUST refuse a directory that exceeds it: no
`Edge Id` can name the chunks past that point.

**A chunk holds at most 31 records**: a producer MUST NOT write a 32nd, so `ordinal` is never more
than `30`. With the 19-byte minimum record the real maximum is 26. The cap is 31, not 32, so that
`0xFFFFFFFF` (ordinal `31` of chunk `2^27 − 1`) is always an impossible id, which keeps §8.7's
sentinel working.

Edge record (`15 + 4 × (Pt Count - 1)` bytes):

Bit 15 of the count word records complete elevation integration. The packer sets it only when
every sample in both directions resolved. The assembler preserves the bit. Readers check the
sentinel before masking the count and reject impossible counts. A missing bit means incomplete
or absent terrain, even when both endpoint heights are valid.

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Length M | 4 | `uint32` | Ground length in meters (equals the adjacency entries' `Cost M`) |
| 4 | Count / elevation validity | 2 | `uint16` | low 15 bits: vertex count (2..125); bit 15: all DEM integration samples present; `0xFFFF` remains the end-of-chunk sentinel |
| 6 | Way Kind | 1 | `uint8` | The edge's packed class byte (§8.6), same value as the adjacency entries' |
| 7 | Anchor Lat | 4 | `int32` | First vertex latitude, **absolute** microdegrees |
| 11 | Anchor Lon | 4 | `int32` | First vertex longitude |
| 15 | Deltas | 4 × (Pt Count − 1) | | Per vertex: `dlat int16, dlon int16`, chained from the previous vertex |

The polyline runs from endpoint `a` to endpoint `b` inclusive (first vertex = `a`'s coordinate,
last = `b`'s); a consumer walking the edge from `b` reverses it. Deltas are **lat-first** like
every §7/§8 record (the §5 geometry sections are lon-first: anchors there are viewport-space
`x, y`).

Packer guarantees that make the fixed `int16` deltas, the `int16` neighbor deltas (§8.3), the
`uint16` cost, the no-straddle rule and the ordinal's 5-bit field hold **by construction**:

- **Densification.** Any segment whose lat **or** lon delta exceeds `30000` microdegrees is
  subdivided with interpolated vertices (the same threshold as §5 geometry and the OBCR track
  encoding). Readers need no special handling.
- **Edge splits.** `nav.rs` splits any edge whose endpoint-to-endpoint lat/lon
  delta exceeds `32000` µdeg (so the §8.3 neighbor delta fits `int16`) or whose
  `Length M` exceeds `60000` m (so `Cost M` fits `uint16`), into pieces joined by
  **synthetic degree-2 junctions** (new dense ids past the real ones). The serializer also splits
  any piece whose densified record would exceed one chunk (`Pt Count > (512 − 15) / 4 + 1`, i.e.
  125 points) or whose endpoint span would exceed the `int16` bound after densification. A chunk
  then holds `1..=26` records, inside the 31-record cap and the ordinal's `0..=30`. Each piece's
  `Length M` is re-measured over its sub-polyline, so costs still sum to the original.

### 8.5 Worked example

A minimal graph — two junctions `A`(lat 100, lon 200) and `B`(lat 900, lon 800)
joined by one 3-vertex edge of 1234 m and way-kind `0x2A` (tertiary/paved: highway
class 10 `| (`surface class 1 `<< 5)`) that climbs 300 m from `A` to `B` and
re-climbs 42 m of dips on the way back — with one profile "`Road`" (climb weight 10; a real
producer writes four), at the default `Offset Scale = 4` (`U = 16`), with the section at a
512-byte-aligned file offset `S`. Directory fields are **units** (a byte offset divided by 16), and
`s = S / 16` is the section's own scaled address:

```
S+0    Nav Directory (40 B):
         index_offset          = s+31     (byte S+496; node chunks begin at S+512)
         index_node_count      = 1
         node_chunk_count      = 1
         edge_pool_offset      = s+64     (byte S+1024 = S+512 + one 512 B node chunk)
         edge_chunk_count      = 1
         chunk_size            = 512
         profile_table_offset  = s+3      (byte S+48)
         profile_count         = 1
         reserved              = 0
         snap_index_offset     = s+127    (byte S+2032; snap chunks begin at S+2048)
         snap_index_node_count = 1
         snap_chunk_count      = 1
S+40   Filler (8 B, 0xFF)                          the directory ends mid-unit
S+48   Profile Table (56 B):
         profile 0: name="Road"      (12 B, 0xFF-padded)
                    highway[32]       (u8 1/16 multipliers)
                    surface[8]
                    climb_weight=10   (1 B)
                    reserved          (3 B, zero)
S+104  Alignment Filler (392 B, 0xFF)              the producer's 512-byte run
S+496  Node Quadtree (4 B):  [0x00000000]          single leaf → node chunk 0
S+500  Filler (12 B, 0xFF)                         align_up(S+500, 16) = S+512
S+512  Node Chunk 0 (512 B):
         rec A: lat=100 lon=200 id=0 degree=1
                nbr { id=1, dLat=+800, dLon=+600, edge_id=0, cost_m=1234,
                      way_kind=0x2A, ascent_m=300 }                          (30 B)
         rec B: lat=900 lon=800 id=1 degree=1
                nbr { id=0, dLat=-800, dLon=-600, edge_id=0, cost_m=1234,
                      way_kind=0x2A, ascent_m=42 }                           (30 B)
         0xFF × 452                                (padding = sentinel)
S+1024 Edge Pool chunk 0 (512 B):
         edge 0 (chunk 0, ordinal 0 ⇒ edge_id = (0 << 5) | 0 = 0):
           length_m=1234  pt_count=3  way_kind=0x2A  anchor=(lat 100, lon 200)
           deltas: (+400,+300) (+400,+300)          → (500,500), (900,800)   (23 B)
         0xFF × 489                    (filler; its pt_count reads 0xFFFF = end of records)
S+1536 Alignment Filler (496 B, 0xFF)
S+2032 Snap Quadtree (4 B): [0x00000000]            single leaf → snap chunk 0
S+2036 Filler (12 B, 0xFF)                          align_up(S+2036, 16) = S+2048
S+2048 Snap Chunk 0 (512 B):
         four 12-byte interior anchors naming edge_id=0   (ceil(1234 / 300) = 5 intervals)
         0xFF × 464                                 (padding = sentinel)
```

The section ends at `S+2560`. The two offsets that the scaling constrains:

- **`profile_table_offset`.** `S+40`, immediately behind the directory, is not a multiple of 16, so
  no offset can name it; the table is at `S+48` and the eight bytes behind the directory are
  filler.
- **`index_offset`.** The producer wants the first node chunk at `S+512`, and the reader computes it
  as `align_up(index_offset × 16 + 1 × 4, 16)`. So `index_offset × 16` must be a multiple of 16 in
  `(S+492, S+508]`, which leaves `S+496`: `index_offset = s+31`, the index occupies
  `S+496..S+500`, and twelve bytes of filler carry it to the boundary.

Node `A` reconstructs neighbor `B` as `(100 + 800, 200 + 600) = (900, 800)` with no edge fetch for
`h`. The only edge is the first record of the first chunk, so `(0 << 5) | 0` is `edge_id = 0`; a
second edge behind it would be `edge_id = 1`. Both directions of the edge carry `edge_id = 0`,
`cost_m = 1234` and `way_kind = 0x2A`; only `ascent_m` differs (the §8.3 exception): 300 m of
climb uphill and 42 m down. Under "`Road`" the uphill arc weighs
`(1234 × 16) >> 4 + 300 × 10 = 4234` and the downhill one `1234 + 42 × 10 = 1654`. Fetching the
edge decodes the polyline `(100,200) → (500,500) → (900,800)`, its way-kind `0x2A`, and its 1234 m
length in one ≤ 512-byte read.

### 8.6 Profile table (bike-type routing)

`Profile Count` consecutive **56-byte** records at `Profile Table Offset`, one per bike
type. A producer writes **exactly four**, in the bike-type order of
[`obcr-route.md`](obcr-route.md) §1.2: Road = 0, Gravel = 1, MTB = 2, Touring = 3. The count,
the order and the meaning are fixed; only the weights are tunable. The device picks a record
by bike-type value; A\* weights each edge by it. The table is **always present**, even for an
empty graph. The reader rejects a `Profile Count` of `0` or `> 8`, and routes a bike type past
the table under profile 0.

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Name | 12 | `char[12]` | UTF-8, `0xFF`-padded (the §7.3 POI-name convention). A producer writes `Road`, `Gravel`, `MTB` or `Touring`. The device shows its own names, not this field |
| 12 | Highway Multipliers | 32 | `uint8[32]` | Weight per **highway class**, `1/16` fixed-point; `16` = 1.0×, `0` = **forbidden** |
| 44 | Surface Multipliers | 8 | `uint8[8]` | Weight per **surface class**, same encoding |
| 52 | Climb Weight | 1 | `uint8` | Flat metres charged per metre of §8.3 `Ascent M`. `0` = climb-blind |
| 53 | Reserved | 3 | `uint8[3]` | Written `0`; readers MUST ignore |

Stock `Climb Weight` values are Road `10`, Gravel `8`, MTB `6`, Touring `8`. `0` is legal and
means climb-blind costing; a producer with no opinion writes it.

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

The addition saturates into the `uint16` frontier cost.

**Admissibility invariant (normative).** Every **non-zero** multiplier is `≥ 16` (that is,
`≥ 1.0×`), which keeps the great-circle heuristic admissible. The packer **rejects** a config
whose quantized weight is non-zero but `< 16`; the reader **clamps** a non-zero multiplier
`< 16` up to `16`, so a hand-forged file cannot hand the router an inadmissible weight.

**The climb term is additive and non-negative (normative).** `Ascent M` and `Climb Weight` are
both unsigned and the term is *added*, so a descent MUST NOT reduce an edge's cost below its
profile-weighted ground length. `Climb Weight` therefore needs no lower bound: every `uint8`, `0`
included, is admissible.

#### Canonical way-kind table (normative)

`Way Kind = (surface_class << 5) | highway_class`. The packer's single source of truth is
`obc-places/src/routing.rs` (`highway_class` / `surface_class` / `classify`); profile configs and
the web builder key their multipliers by these class names.

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

### 8.7 Sparse exact-edge snap index

The edge pool is followed by a second quadtree index at `Snap Index Offset * U` and
`Snap Chunk Count` fixed 512-byte chunks beginning at
`align_up(Snap Index Offset * U + Snap Index Node Count * 4, U)` (the §8.1 convention). The
quadtree has §8.2's flat encoding, global bbox, subdivision, split floor and first-fit leaf bin
packing, so distinct leaves may reference one shared chunk and readers MUST filter records by their
absolute coordinate.

Each record is 12 bytes:

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Lat | 4 | `int32` | Anchor latitude, absolute microdegrees |
| 4 | Lon | 4 | `int32` | Anchor longitude, absolute microdegrees |
| 8 | Edge Id | 4 | `uint32` | Pool-relative id of the §8.4 edge geometry to project |

Unused chunk tails are `0xFF`; `Edge Id == 0xFFFFFFFF` is the sentinel, impossible as a real id
(§8.4). A final serialized edge piece contributes no record when its measured geometry is at most
300 m. Otherwise the producer chooses `ceil(length / 300)` equal-length intervals along the
polyline and writes the `intervals − 1` interior boundaries. So no endpoint/anchor gap is more than
300 m, and no routable graph node or A* topology change is added.

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

- **Format authority (Rust, no_std):** `firmware/obc-formats/src/obcm.rs` (also `OffsetScale` /
  `ScaledOffset` for §1.1, and `UnitWriter`, §1.2's boundary-and-filler rule as a cursor) and
  `firmware/obc-formats/src/io.rs` (checked little-endian primitives, byte source and sink). It
  contains no reader, packer, cache, or rendering policy.
- **Writer (Rust, std host):** `host/obc-draw/src/serialize.rs` encodes feature trees.
  `host/obc-network/src/serialize.rs` encodes POI, graph and snap sections.
  `host/obc-map-core/src/serialize.rs` frames these bytes, styles and routing profiles.
  `host/obc-bake/src/serialize.rs` composes a complete map. Shared OSM place classification
  lives in `host/obc-places/src/metadata.rs`, hours in `host/obc-places/src/hours.rs`,
  and way-kind classification in `host/obc-places/src/routing.rs`. Graph construction lives
  in `host/obc-network/src/nav.rs`. Routing config and profile quantization live in
  `host/obc-map-core/src/config.rs`.
- **Reader + renderer (Rust, no_std):** `firmware/obc-reader` — `reader.rs`
  (`Reader`, `for_each_feature`, `select_lod_for_mpp`, the POI + nav directories +
  the profile table in `MapTables`, `for_each_nav_node`, `NavNeighbor` delta
  decode, `nav_edge`, `MapProfile::multiplier`, `MapProfile::climb_weight`) — and
  `firmware/obc-render`
  (`Viewport`, `RenderScratch`). Format-contract tests in
  `firmware/obc-reader/tests/format.rs` (byte pins) and
  `host/obc-bake/tests/packer_cases/nav_round_trip.rs` (writer↔reader §8 round trip, incl.
  the profile table, kinds, delta reconstruction, and the bin-packing fill floor).

## 9. Landmark section

The optional section holds geographic discovery records and selected-item content in the same map
object. Both header fields are zero when absent. Otherwise the scaled region starts after the
header, has at least 16 bytes, and ends within the map. Its directory records its exact byte length;
only offset-unit padding may follow it. Internal offsets are **bytes relative to this section**.
They do not use the map offset scale.

### 9.1 Directory and spatial records

The directory is 16 bytes:

| Offset | Field | Type | Constraint |
| :-- | :-- | :-- | :-- |
| 0 | Record count | `uint32` | 0..65,535 |
| 4 | Record length | `uint16` | 84 |
| 6 | Section version | `uint16` | 1; other versions are unsupported |
| 8 | Payload start | `uint32` | 16 + count × 84 |
| 12 | Section length | `uint32` | At least payload start, within the header region |

Fixed records follow the directory, sorted strictly by `(latitude, longitude, QID)`.
Each QID occurs once. Readers bisect this latitude index and scan the relevant latitude band in
bounded steps. Discovery does not read article or photo payloads. Nearby pages use deterministic
`(distance in whole metres, QID)` keys. A caller binds each query to its installed map revision,
position and filter generation; changing any of these cancels the old query.

| Offset | Field | Size | Constraint |
| :-- | :-- | :-- | :-- |
| 0 | Wikidata QID number | 8 | Nonzero `uint64`; the leading Q is implicit |
| 8 | Longitude | 4 | Signed microdegrees, −180,000,000..180,000,000 |
| 12 | Latitude | 4 | Signed microdegrees, −90,000,000..90,000,000 |
| 16 | Category | 1 | 1..16, from the pinned landmark category policy |
| 17 | Reserved | 3 | Zero |
| 20 | Hours reference | 2 | Shared §7.5 pool index, or `0xFFFF` |
| 22 | Reserved | 2 | Zero |
| 24 | OSM metadata | 28 | The §7 service identity/approach encoding; all zero means absent |
| 52 | Name reference | 8 | Required, at most 256 UTF-8 bytes |
| 60 | Article bundle reference | 8 | Required multilingual bundle (§9.2), at most 20,652 bytes |
| 68 | Photo reference | 8 | Optional independent stream (§9.3), at most 52,096 bytes |
| 76 | Photo attribution reference | 8 | Present exactly when the photo is present; at most 1,024 bytes |

Each reference is `(offset uint32, length uint32)`. Only `(0, 0)` means absent. A present reference
has nonzero length, starts at or after payload start, and ends within the exact section length.
Addition must be checked for overflow. Identical immutable content can share a reference.

A display coordinate does not establish routable access. The optional OSM metadata carries only an
explicit source-topology association and its allowed profile mask. Without OSM metadata, the hours
reference must be `0xFFFF`. A linked non-service entity uses the same hours pool as service POIs.
Missing, unsupported or failed hours remain Unknown under the shared §7 opening-status rules.

### 9.2 Multilingual article bundles

Each article bundle is self-contained. Its header contains the default language (two ASCII bytes)
and variant count (`uint16`, 1..4). Each following variant occupies 20 bytes:

| Offset | Field | Size | Constraint |
| :-- | :-- | :-- | :-- |
| 0 | Language | 2 | One of `en`, `de`, `fr`, `es`; unique in the bundle |
| 2 | Text page count | 1 | 1..4 |
| 3 | Reserved | 1 | Zero |
| 4 | Text reference | 8 | Required page bundle, at most 4,118 bytes |
| 12 | Article attribution reference | 8 | Required, at most 1,024 bytes |

References inside this directory are **relative to the article bundle**, not the landmark section.
They start at or after `4 + count × 20` and end within the bundle. The default language must have a
variant. Writers use the shared UI language order (`en`, `de`, `fr`, `es`); readers reject duplicate
languages and invalid ranges. Selection prefers the device UI language, then English, then the
stored default. The host selects the default from pinned local language facts and a fixed fallback
rule. The device does not determine the country. Assembly copies the complete bundle unchanged.

Each place has one name and one optional photo shared by all variants. Article attribution belongs
to its variant; photo attribution belongs to the shared photo. A UI language change must invalidate
the loaded text/credits and reset their page positions.

Text and attribution use the same field-bundle layout:

A bundle starts with `count uint16`, followed by `count + 1` byte offsets (`uint32`) relative to the
bundle, then UTF-8 fields. The first offset equals `2 + (count + 1) × 4`; offsets are nondecreasing,
and the final offset equals the bundle length. A selected field is the bytes between its two
offsets. Readers check its range, UTF-8 and destination capacity before use.

A text bundle has the variant's 1..4 fields. Each field is one prepared page of at most 1,024 bytes.
The language identifies the actual article text.

An attribution bundle is the credit of one work. It has exactly four fields, in this order: source,
title, creator and licence. Each field is one line in the device glyph set. Only the creator can be
empty, and only for a public-domain dedication (CC0). The bundle, including its offset table, is at
most 1,024 bytes. Readers wrap the fields themselves.

| Field | Article | Photo |
| :-- | :-- | :-- |
| Source | URL of the exact revision, `<language>.wikipedia.org/?oldid=<revision>` | `Wikimedia Commons` |
| Title | Article title | Commons file name, which identifies the file page |
| Creator | `Wikipedia contributors` | The licensor's requested attribution, else the author |
| Licence | Creative Commons short name and licence URI, for example `CC BY-SA 4.0 creativecommons.org/licenses/by-sa/4.0/` | Same as the article |

### 9.3 Independent photo stream

Each photo is exactly 216 × 240 row-major RGB222 pixels. One pixel occupies one byte, in `0..63`,
with red in bits 5..4, green in bits 3..2 and blue in bits 1..0. There is one lossless zlib stream
with DEFLATE compression, an Adler-32 trailer, and at most a 4,096-byte history window
(`CINFO <= 4`). Preset dictionaries are not permitted. There is no codec selector or fallback.

The decoder accepts 6..52,096 compressed bytes and exactly 51,840 output bytes. It rejects an
invalid header, unsupported window, bad checksum, malformed or truncated stream, extra trailing
bytes, wrong output count or pixel outside `0..63`. Each work step reads at most 256 compressed
bytes and emits at most 4,096 pixels into the selected mutable framebuffer region. The decoder
retains only its bounded history/state between steps. No second full photo or framebuffer is
required. A caller must bind this work to the selected map/QID and render generation, cancel it
under an overlay, and replay it after a fresh base render. Source and frame borrows end before
asynchronous presentation.

A malformed photo reference or stream is a selected-photo error. It must not erase valid text,
attribution, Back or an otherwise available Visit. Clear old pixels on error or identity changes.
A medium read error remains a read error, distinct from a source with no photo. Malformed discovery
metadata or directory reads fail the query instead of producing a completed empty list.

### 9.4 Cell ownership and assembly

Cut cells own display coordinates in half-open longitude/latitude bounds `[west, east)` and
`[south, north)`. Core cells carry the section; geometry-only cells do not duplicate it. Whole-map
packs use their stated geographic coverage. The compiled input contains unique QIDs.

Assembly deduplicates QIDs. Prefer a record with an explicit approach, then the lower OSM source
identity (missing identity last), then the canonical record/content digest. Input order must not
change the result. Collect service and landmark schedules in one shared pool and remap all hours
references. Intern equal content, copy it with bounded source windows, and rewrite every reference
against the output section. Empty inputs produce an absent section. Content, producer policy,
encoder code and dependency hashes belong to cell cache identity; verify each declared photo hash
before reusing a cached cell. Ordinary map transfer, flat-store checksums and revision ownership
apply to this section as they do to the rest of the map.

## 10. Peak article collection

This optional section is independent of the landmark collection (§9). It contains only explicit
OSM summit-to-article links. It has no radius, approach, category or article coordinate index.
The summit's existing §7.3 SourceId is the lookup key. The collection is accessible only through
Peak View; landmark and service-place queries MUST NOT return these records.

The section follows landmarks and precedes terrain. The header offset and length use the map's
Offset Scale. Interior offsets are unsigned byte offsets relative to the start of this section.
Final unit padding is `0xFF`; it is outside the exact section length below.

### 10.1 Directory and identity tables

The 24-byte section header is:

| Offset | Field | Bytes | Rule |
| --- | --- | --- | --- |
| 0 | Section Version | 2 | `1` |
| 2 | Article Record Length | 2 | `64` |
| 4 | Association Count | 4 | At most 262,144 |
| 8 | Article Count | 4 | At most 65,535 |
| 12 | Payload Offset | 4 | Exactly `24 + 44 × Association Count + 64 × Article Count` |
| 16 | Section Length | 4 | Exact bytes, excluding final padding; at least Payload Offset |
| 20 | Reserved | 4 | Zero |

Associations begin at byte 24. Each 44-byte association contains:

| Offset | Field | Bytes | Rule |
| --- | --- | --- | --- |
| 0 | SourceId | 8 | Valid OSM node SourceId (§7.3); no ways or relations |
| 8 | Article Identity | 32 | SHA-256 of the compiler's canonical UTF-8 article identity |
| 40 | Article Index | 4 | Zero-based index into the article table |

Associations MUST be strictly ordered by SourceId, with one record per source node. Several nodes
can refer to the same article. Article Index MUST be in range, and the indexed article's identity
MUST equal the association's identity. A conflicting link for the same node is a producer error.

The article table follows the association table. Each 64-byte record contains its 32-byte Article
Identity, then four 8-byte references in this order: display name, optional multilingual article
bundle, optional compressed photo, optional photo attribution. §10.2 says which are present. Each
reference is `(offset u32, length u32)`.
Records MUST be strictly ordered by identity. An article is stored once per canonical identity.
A producer MUST reject different canonical strings with the same identity digest.

### 10.2 Guarded content and shared article payloads

Every present reference points to a 33-byte guard followed by its payload. The guard contains the
article's 32-byte identity and one slot byte (`0` name, `1` articles, `2` photo, `3` photo attribution).
The reference length includes this guard. A consumer MUST check the guard against the selected
record and slot before it exposes the payload. A reference redirected to another article, or to a
different content slot of the same article, MUST fail. This is a reference-identity check; normal map
object checksums protect the content bytes.

The name is required. Both photo references are `(0, 0)` when absent; otherwise both are present.
The article bundle reference is `(0, 0)` when the record has no text; a record MUST carry the
article bundle, the photo, or both. A present reference MUST start at or after Payload Offset,
contain more than 33 bytes, and end at or before Section Length without integer overflow. Payload
limits, excluding the guard, are those of §9.1: name at most 256 bytes, article bundle at most the
shared article limit, compressed photo at most 52,096 bytes, and photo attribution at most 1,024
bytes. The article bundle uses §9.2 unchanged. Its internal offsets are relative to the start of
that bundle, after the guard. The photo stream uses §9.3 unchanged. One optional photo serves all
text variants of an article. A record with a photo and no bundle has no language: a consumer shows
the photo and its attribution, and never an empty text page. Name, text, attribution and photo use
the existing bounded content readers.

### 10.3 Direct access and map changes

A reader locates an association by binary search on SourceId. With the format's maximum count,
lookup needs at most 19 association comparisons, one exact-key check and one article-record read.
It does not scan nearby landmarks or require a routable approach. Content reads are separate and
bounded by their payload contracts.

A selection retains the map generation and the full association. Before each content access, the
reader MUST require the same active generation, SourceId, article identity and article index.
A missing section or missing source key means no article. A malformed index, reference, guard or
bundle is an error; it MUST NOT open another article. An old selection MUST fail after a map change,
even when the new map reuses the same table index. Text language selection follows §9.2. It does
not change the summit association.

### 10.4 Packing, clipping and assembly

Only named OSM summit nodes carried by the output map select associations. Match their full
SourceId to the compiler's explicit node association; never match names, truncated labels,
coordinates or proximity. A linked summit selects its entire article, irrespective of the
article's geographic location or the coordinates recorded in the source catalogue.

Core cells contain peak content. A cell's existing half-open summit ownership rule selects its
associations. Geometry-only cells have no peak section. Clipping MUST keep an article whenever
any linked summit remains. Assembly selects associations from the summit SourceIds in its merged
POI set. It unions associations, deduplicates article identities and remaps all article indexes
and section-relative content references. It validates all input references, including losing
duplicates, and copies source-backed content with bounded buffers. Changed source content or I/O
failure aborts output.

When regional inputs contain different compiled versions of the same article, select the
lexicographically smallest array of four SHA-256 payload digests in slot order. The digest covers
the complete guarded blob; an absent blob uses SHA-256 of the empty byte string. This rule applies
to both catalogue packing and cell assembly. It retains one complete compiled article, including
its baked default and all its language variants. Catalogue order and cell arrival order MUST NOT
change the result. Conflicting summit-to-article identities MUST fail instead of choosing a link.

Peak catalogue bytes, declared photo bytes, encoder policy and dependency identity belong to the
cell cache key. Verify declared photo digests before a cache hit. The same normal pack, cut and
assembly entry points carry this section.
