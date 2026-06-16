# OBCM v3 — Level-of-Detail (LOD) Design

Status: **proposed** (format not yet implemented). Supersedes the single-detail
v2 layout described in [OBCM_Spec.md](../../../OBCM_Spec.md).

## Context

The map should show different OSM features at different zoom levels: motorways,
coastline/land, major water and country borders when zoomed out to a whole
country; residential streets, buildings and footways only when zoomed into a
city. This cuts both the data the MCU must decode and on-screen clutter.

The renderer is moving to Rust (shared between the desktop simulator and the
nRF5340 + LS021B7DD02 firmware via `embedded-graphics`). The format must be
cheap to consume on a memory-constrained MCU: read little when zoomed out, no
runtime discovery work.

### Locked decisions

1. **Pyramid layers.** Each LOD is its own quadtree + chunk set holding only
   that level's features, with geometry simplified to the level's resolution.
   Zoomed out ⇒ read one small coarse layer. (vs. tagging every feature with a
   min-zoom in a single fine tree, which forces the MCU to decode fine chunks
   just to skip them.)
2. **Keep RGB565 in the file; quantize at render.** The style table stays
   device-independent and matches the webapp editor. The renderer quantizes the
   small style palette to the target display depth (RGB222 / 64 colors for the
   LS021B7DD02) once at load.
3. **Meters-per-pixel LOD selection.** Each LOD stores a ground-meters-per-pixel
   threshold; the renderer computes current m/px from zoom + display size and
   picks the level. The same file looks right on the 1024px desktop sim and the
   240px device.

## File layout (v3)

```
[Header]
[Style Table]                      (global — unchanged from v2)
[LOD Table]                        (new: LOD Count entries)
[LOD 0 Index][LOD 0 Data Chunks]   (coarsest)
[LOD 1 Index][LOD 1 Data Chunks]
...
[LOD N-1 Index][LOD N-1 Data Chunks] (finest)
```

### Header

Version bumps to `0x03`. The single `Index Offset` / `Chunk Size` of v2 are
replaced by the LOD table; all section locations are explicit offsets so a
no_std reader needs zero discovery.

| Field | Size | Type | Description |
| :-- | :-- | :-- | :-- |
| Magic | 4 | char | `"OBCM"` |
| Version | 1 | u8 | `0x03` |
| Min Lat | 4 | i32 | Global bbox, microdegrees |
| Min Lon | 4 | i32 | |
| Max Lat | 4 | i32 | |
| Max Lon | 4 | i32 | |
| Style Offset | 4 | u32 | Byte offset to Style Table |
| LOD Count | 1 | u8 | Number of LOD levels (≥ 1) |
| LOD Table Offset | 4 | u32 | Byte offset to LOD Table |

### Style Table

Unchanged from v2: `Count (u8)` then `Count` × `{ID u8, Z-Index i8, Color u16
(RGB565), Weight u8}`. Style IDs are global across all LODs.

### LOD Table

`LOD Count` entries, ordered **coarsest (0) → finest (N-1)**. `Max Meters Per
Pixel` is strictly decreasing down the list.

| Field | Size | Type | Description |
| :-- | :-- | :-- | :-- |
| Max Meters/Pixel | 4 | f32 | Upper bound of the m/px range this LOD covers. Coarsest level uses `+inf` (`f32::INFINITY`). |
| Index Offset | 4 | u32 | Byte offset to this LOD's quadtree index |
| Index Node Count | 4 | u32 | Number of `u32` nodes (chunks start at `Index Offset + Node Count*4`) |
| Chunk Size | 2 | u16 | Per-LOD data chunk capacity (bytes) |
| Chunk Count | 4 | u32 | Number of data chunks in this LOD |

Each entry is 18 bytes. Storing `Index Node Count` and `Chunk Count` explicitly
removes v2's index-size-by-traversal hack (`reader.py::_load_index`).

### Quadtree Index + Data Chunks (per LOD)

Identical encoding to v2 (flat `u32` quadtree; fixed-capacity chunks; 12-byte
feature headers; delta geometry). **Every LOD's quadtree is built over the same
global bbox**, so node bboxes are computed the same way at every level and the
renderer's subdivision math is LOD-independent. Coarse levels naturally produce
shallow trees (few features ⇒ few splits).

## LOD selection (renderer)

Compute current ground **meters-per-pixel** `mpp` from zoom and display size.
Among LODs whose range covers `mpp` (`max_mpp[i] >= mpp`), pick the **finest**
(smallest `max_mpp`). Clamp to `[0, N-1]`.

Worked example (3 levels):

| LOD | content | max m/px |
| :-- | :-- | :-- |
| 0 country | coastline/land, sea, motorway/trunk, major rivers, admin_level 2 | +inf |
| 1 region | + primary/secondary, lakes, forests | 50 |
| 2 city/street | + residential, service, footway, buildings, parks | 10 |

- `mpp = 70` → only LOD 0 covers it → **LOD 0**
- `mpp = 30` → LOD 0 & 1 cover it; finest = **LOD 1**
- `mpp = 5` → all cover it; finest = **LOD 2**

## Component impact

### Packer (`obcm_pack.py`, Python — later task)
- Config gains a `lods` list: each level defines its included feature types
  (subset of the existing `features` tree), a `max_mpp` threshold, and a
  geometry `simplify_tolerance`.
- Ingest once, then per level: filter features to the level's types,
  `shapely.simplify(tolerance)`, build a quadtree, serialize an index+chunk
  block. Land/sea belong to the coarser levels.
- Recommended authoring model: **cumulative** (each finer LOD includes all
  coarser features plus new ones) — simplest mental model for the webapp editor.
- `serialize_all` writes the style table once, then the LOD table, then each
  level's index+chunks.

### Reader (`obcm/reader.py` + the new Rust crate)
- Parse the LOD table; `query(bbox, mpp)` selects the LOD, then traverses that
  level's index exactly like v2.
- Back-compat: branch on version. v2 files = a single implicit LOD (`max_mpp =
  +inf`, the existing index/chunk_size).

### Webapp (later task)
- The feature/style editor gains LOD bands: assign feature types to levels, set
  each level's m/px threshold and simplification. Reuses the existing per-feature
  styling; LOD only governs inclusion.

### Desktop simulator / MCU renderer (Rust — task (c), starts against v2)
- The Rust `obcm` crate parses v2 now; the v3 LOD table is an additive parse
  step. The renderer's `query(bbox, mpp)` gains the level-selection step above;
  drawing is unchanged.

## Migration

- v2 files remain readable (version branch). No re-pack required to keep viewing
  existing maps.
- v3 is produced once the packer LOD work lands; the format is forward-designed
  so the Rust renderer written now (against v2) extends to it without
  structural change.
