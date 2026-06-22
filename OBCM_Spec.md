# OBCM File Format Specification (v5)

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

**Version 5** adds a 6th byte to style records for flags (bit 0 = priority). **v5 is the only supported version**; earlier versions (v4, v3 LOD-only, v2 single detail level) have been dropped.

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
[Header]                            (32 bytes, fixed)
[Style Table]                       (global — shared by all LODs)
[LOD Table]                         (LOD Count entries)
[LOD 0 Index][LOD 0 Data Chunks]    (coarsest)
[LOD 1 Index][LOD 1 Data Chunks]
...
[LOD N-1 Index][LOD N-1 Data Chunks] (finest)
```

The byte layout is produced by `firmware/obc-pack/src/serialize.rs` (`serialize_lods`) and
parsed by `firmware/obc-reader/src/reader.rs`. All multi-byte integers are **little-endian**.

---

## 1. Header (32 bytes)

Packed as `struct "<4sBiiiiIBIH"`.

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Magic | 4 | `char[4]` | Must be `b"OBCM"` |
| 4 | Version | 1 | `uint8` | `0x05` |
| 5 | Min Lat | 4 | `int32` | Global bbox min latitude (microdegrees) |
| 9 | Min Lon | 4 | `int32` | Global bbox min longitude |
| 13 | Max Lat | 4 | `int32` | Global bbox max latitude |
| 17 | Max Lon | 4 | `int32` | Global bbox max longitude |
| 21 | Style Offset | 4 | `uint32` | Byte offset to the Style Table |
| 25 | LOD Count | 1 | `uint8` | Number of LOD levels (≥ 1) |
| 26 | LOD Table Offset | 4 | `uint32` | Byte offset to the LOD Table |
| 30 | Marker Color | 2 | `uint16` | User-position marker color (RGB565) |

Note the bbox field order in the file is **lat, lon, lat, lon**. In practice the
Style Table immediately follows the header, so `Style Offset` is `32`.

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

## Reference implementations

- **Writer (Rust, std host):** `firmware/obc-pack/src/serialize.rs` (`serialize_lods`,
  `serialize_tree`, `pack_feature`, `pack_chunk`, `pack_style_dict`).
- **Reader + renderer (Rust, no_std):** `firmware/obc-reader` — `reader.rs`
  (`Reader`, `for_each_feature`, `select_lod_for_mpp`) — and `firmware/obc-render`
  (`Viewport`, `MapRenderer`). Format-contract tests in
  `firmware/obc-reader/tests/format.rs`.
