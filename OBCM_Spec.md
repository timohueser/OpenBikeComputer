# OBCM File Format Specification (v7)

OBCM (OpenStreetMap Binary Chunked Map) is a compact binary map format designed
for efficient rendering on memory-constrained devices such as microcontrollers
(MCUs). It is written by the Rust packer (`firmware/obc-pack`) and read by the
Rust crate (`firmware/obc-reader`, shared by the desktop simulator and the nRF54L
firmware).

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
**hours-pool section** (§7.5) at the file tail. The POI directory (§7.1) gains
`hours_pool_offset` + `hours_pool_count`. The pool holds deduplicated 29-byte
weekly-schedule blobs (today's opening hours, normalized at pack time from OSM
`opening_hours`), so a POI's hours are a single index lookup on-device. The header
does **not** grow (still 36 bytes) — only the version byte and the POI section
change. **v7 is the only supported version**; earlier versions (v6, v5, v4, v3
LOD-only, v2 single detail level) have been dropped.

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
[Header]                            (36 bytes, fixed)
[Style Table]                       (global — shared by all LODs)
[LOD Table]                         (LOD Count entries)
[LOD 0 Index][LOD 0 Data Chunks]    (coarsest)
[LOD 1 Index][LOD 1 Data Chunks]
...
[LOD N-1 Index][LOD N-1 Data Chunks] (finest)
[POI Directory][POI Indexes + Chunks] (§7)
[Hours-Pool Section]                  (§7.5 — file tail)
```

The byte layout is produced by `firmware/obc-pack/src/serialize.rs` (`serialize_lods`) and
parsed by `firmware/obc-reader/src/reader.rs`. All multi-byte integers are **little-endian**.

---

## 1. Header (36 bytes)

Packed as `struct "<4sBiiiiIBIHI"`.

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Magic | 4 | `char[4]` | Must be `b"OBCM"` |
| 4 | Version | 1 | `uint8` | `0x07` |
| 5 | Min Lat | 4 | `int32` | Global bbox min latitude (microdegrees) |
| 9 | Min Lon | 4 | `int32` | Global bbox min longitude |
| 13 | Max Lat | 4 | `int32` | Global bbox max latitude |
| 17 | Max Lon | 4 | `int32` | Global bbox max longitude |
| 21 | Style Offset | 4 | `uint32` | Byte offset to the Style Table |
| 25 | LOD Count | 1 | `uint8` | Number of LOD levels (≥ 1) |
| 26 | LOD Table Offset | 4 | `uint32` | Byte offset to the LOD Table |
| 30 | Marker Color | 2 | `uint16` | User-position marker color (RGB565) |
| 32 | POI Section Offset | 4 | `uint32` | Byte offset to the POI Directory (§7) |

Note the bbox field order in the file is **lat, lon, lat, lon**. In practice the
Style Table immediately follows the header, so `Style Offset` is `36` (it was `32`
in v5, before the POI Section Offset was appended). The POI section is always
present, so `POI Section Offset` is never `0`.

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
2. **Style Records** (`Count` × 6 bytes):

| Field | Size | Type | Description |
| :-- | :-- | :-- | :-- |
| ID | 1 | `uint8` | Style ID, referenced by feature headers |
| Z-Index | 1 | `int8` | Painter's-order layer (lower drawn first) |
| Color | 2 | `uint16` | RGB565 |
| Weight | 1 | `uint8` | Stroke width in pixels (lines) |
| Flags | 1 | `uint8` | Bit 0-1: priority level (1=highest/render first, 4=lowest/render last) |

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
| Chunk Size | 2 | `uint16` | Fixed capacity of each data chunk (bytes) — per-LOD |
| Chunk Count | 4 | `uint32` | Number of data chunks in this LOD |

This LOD's data chunks begin at `Index Offset + Index Node Count * 4` (i.e.
immediately after its index). Chunk `k` is at `data_start + k * Chunk Size`.

Storing `Index Node Count` and `Chunk Count` explicitly is what removes any
runtime discovery: the reader never has to walk the tree to learn its size.

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
§5, anchors).

---

## 5. Data Chunks (per LOD)

Features are packed into fixed-capacity blocks of `Chunk Size` bytes. Unused
trailing bytes are padded with `0xFF`; a `0xFF` style-ID byte (an impossible
style) marks the end of features in a chunk.

### Feature Header (12 bytes)

Packed as `struct "<BHiiB"`.

| Field | Size | Type | Description |
| :-- | :-- | :-- | :-- |
| Style ID | 1 | `uint8` | Reference into the Style Table |
| Pt Count | 2 | `uint16` | Vertex count of the **exterior** ring |
| Anchor X | 4 | `int32` | Exterior start, relative to the **leaf node's min longitude** (microdegrees) |
| Anchor Y | 4 | `int32` | Exterior start, relative to the leaf node's min latitude |
| Flags | 1 | `uint8` | `0x01` 16-bit deltas · `0x02` polygon · `0x04` has holes |

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
> (`MAX_FEAT_PTS`) and silently truncates anything beyond it. The packer guarantees
> the bound through `Chunk Size`: a feature can't outgrow its chunk, and the densest
> encoding is 8-bit deltas at 2 bytes per vertex, so `Chunk Size ≤ (2048−1)·2 + 12 =
> 4106` keeps every feature within the cap. `obc-pack` rejects a larger `Chunk Size`
> at build time rather than emit a feature the reader would corrupt.

### Polygon-with-holes byte layout

```
[Feature Header (12 B)]
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

Names are ASCII-folded at pack time to the device font's printable set
(`0x20..=0x7E`, Terminus) and capped at **24 bytes** (v7 widened the field from
20); an unnamed POI (`Name Len == 0`) shows its subtype's fallback label
on-device. The 24-byte `Name` field is `0xFF`-padded past `Name Len`.

`HoursRef` is a 0-based index into the hours-pool section (§7.5): blob `i` lives at
`hours_pool_offset + 2 + i*29`. `0xFFFF` means the POI has no (parseable) hours.
Duplicate weekly schedules collapse to one pooled blob, so many POIs in a region
can share a single `HoursRef`.

### 7.4 Canonical category / subtype table (normative)

This is the **normative home** of the id table; `obc-pack`'s `poi.rs` mirrors it
exactly and the device firmware mirrors it. **Ids are append-only** — an existing
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

A single deduplicated pool of weekly opening-hours schedules, written at the
**file tail** and reached from the directory's `Hours Pool Offset` (§7.1). A POI
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

## Reference implementations

- **Writer (Rust, std host):** `firmware/obc-pack/src/serialize.rs` (`serialize_lods`,
  `serialize_tree`, `serialize_poi_section`, `pack_feature`, `pack_chunk`,
  `pack_style_dict`), `firmware/obc-pack/src/poi.rs` (the category/subtype table
  mirrored in §7.4), and `firmware/obc-pack/src/hours.rs` (the `opening_hours`
  parser + 29-byte blob encoder + dedup pool for §7.5).
- **Reader + renderer (Rust, no_std):** `firmware/obc-reader` — `reader.rs`
  (`Reader`, `for_each_feature`, `select_lod_for_mpp`, the POI directory +
  hours-pool offset/count in `MapTables`) — and `firmware/obc-render` (`Viewport`,
  `MapRenderer`). Format-contract tests in `firmware/obc-reader/tests/format.rs`.
