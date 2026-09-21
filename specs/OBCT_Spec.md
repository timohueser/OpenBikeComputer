# OBCT — OpenBikeComputer Terrain Tiles (v1 and v3)

Version 3 adds the geographic surface index in section 8. Sections 1–7 describe the native
v1 raster, which remains the base height plane of v3.

OBCT is the terrain artifact: a raster of ground heights on the [OBCA](OBCA_Spec.md) cell grid. It
defines the sample lattice (§1), the 512-byte tile and the terrain cell (§2, §3), the container
(§4) and the sampling rules (§5).

Terrain is a separate artifact class with its own revision track. An assembly's raster is embedded
**verbatim** in the map file's terrain region ([`OBCM_Spec.md` §1.3](OBCM_Spec.md)), which the map
reader hands over as an opaque window rather than parsing.

This document is normative. The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are to be
interpreted as in RFC 2119.

Its code authority for magic, version, field offsets, fixed lengths, sentinels and the layout
arithmetic is [`firmware/obc-formats/src/obct.rs`](../firmware/obc-formats/src/obct.rs); producers
and consumers import those facts directly rather than transcribing them. The reference reader,
sampler and tile cache are [`firmware/obc-elevation`](../firmware/obc-elevation).

Related contracts: [`OBCA_Spec.md`](OBCA_Spec.md) §1 defines the grid this raster sits on;
[`OBCC_Spec.md`](OBCC_Spec.md) §13 publishes terrain cells as a catalog artifact class with its own
revision track; [`OBCM_Spec.md`](OBCM_Spec.md) is the map that carries an assembly's raster, and
whose §8 nav graph stores the ascent integrated *from* these samples.

---

## 1. The sample lattice

### 1.1 Origin and posting

All coordinates are integer **microdegrees** (µdeg, 1e-6 degrees), the unit of every OBCM
coordinate. The lattice is anchored on the OBCA grid origin ([`OBCA_Spec.md` §1.1](OBCA_Spec.md)):

```
GRID_ORIGIN = −268435456 µdeg   (= −2^28, on BOTH axes)
WORLD_SIDE  =  536870912 µdeg   (= 2^29)
```

A **posting** `P` MUST be a power of two in µdeg with `2^4 ≤ P ≤ 2^16`. Sample `(i, j)` of the
lattice of posting `P` sits at

```
lat(i) = GRID_ORIGIN + i·P
lon(j) = GRID_ORIGIN + j·P
```

with `0 ≤ i, j < WORLD_SIDE / P`. The same expression applies on **both** axes: the lattice is
square in µdeg, not in metres.

A `2^9` posting is therefore ≈ 57 m in latitude everywhere and ≈ 39 m in longitude at 47°N,
narrowing towards the poles. This is not corrected anywhere and MUST NOT be: the contract rests on
the lattice being derivable from the coordinate by a shift.

The lattice does not wrap, and it spans the world box rather than the geographic domain — the
antimeridian and pole rules of [`OBCA_Spec.md` §1.4](OBCA_Spec.md) apply unchanged.

### 1.2 Values

A sample is a signed 16-bit little-endian integer:

| Value | Meaning |
| :-- | :-- |
| `-32767 … 32767` | height in **whole metres**, orthometric (EGM2008) |
| `-32768` (`i16::MIN`) | **`NODATA`** — no height is known at this sample |

Heights are orthometric — height above the geoid — **not** ellipsoidal. A producer MUST NOT write
`-32768` as a real height.

A sample is a **point sample** of the source at the lattice node, with one exception. At a crest
node the sample MAY instead be the maximum reference ground height inside the node's half-posting
cell, taken from a finer reference DEM. Section 9 states the rule that selects such a node and
computes its value. No flag records the exception; every consumer reads the sample it finds.

### 1.3 Posting and cell size are data

`P` and the cell side are **header fields** (§4.2), not constants of this document. The v1 baked
values are

| Quantity | v1 value | ≈ at 47°N |
| :-- | :-- | :-- |
| Posting `P` | `2^9` µdeg | 57 × 39 m |
| Terrain cell | `2^19` µdeg | 58 × 40 km — 1024 × 1024 samples, 2 MiB raw |

They are the bakery's choice, published in the catalog. Retuning them is a terrain re-bake,
**not** a version bump of this format (the [`OBCA_Spec.md` §1.5](OBCA_Spec.md) idiom).

A reader MUST accept any pairing this document permits (§4.5), not only the v1 one.

---

## 2. Tiles

A **tile** is `16 × 16` samples = **512 bytes**, the unit of every read — one SD block, and one
OBCM §8 nav chunk.

```
tile bytes[ (row · 16 + col) · 2 ] … +2      row, col ∈ 0..16, little-endian int16
```

Row-major, and **rows advance latitude**: `row` steps north by one posting, `col` steps east by one
posting. The 32 bytes of one row are 16 consecutive longitudes at one latitude, and the tile's first
sample is its **minimum** corner in both axes. A north-up source scanline, such as a GeoTIFF, is
flipped by the baker on the way in; a consumer never flips anything.

One tile spans `16·P` µdeg on each axis — at the v1 posting, `2^13` µdeg ≈ 910 m of latitude.

---

## 3. Terrain cells

### 3.1 A cell is an OBCA grid square

A **terrain cell** of side `S = 2^cell_log2` µdeg is the OBCA cell of that size
([`OBCA_Spec.md` §1.1](OBCA_Spec.md)): the half-open square

```
cell(S, ci, cj) = [ GRID_ORIGIN + ci·S , GRID_ORIGIN + (ci+1)·S )   in latitude
                × [ GRID_ORIGIN + cj·S , GRID_ORIGIN + (cj+1)·S )   in longitude
```

`S` MUST satisfy `2^10 ≤ S ≤ 2^28` and MUST be at least one tile wide at the file's posting
(§4.5). Because both `S` and `P` are powers of two on one origin, a cell holds a whole number of
tiles and the lattice samples it owns are exactly

```
i ∈ [ ci · S/P , (ci+1) · S/P )        j ∈ [ cj · S/P , (cj+1) · S/P )
```

**A cell owns the samples on its minimum edges and not those on its maximum edges** — half-open,
like the square itself. The sample on the boundary between two cells belongs to the upper one, once,
in the whole world, so no sample is stored twice.

### 3.2 A cell block is its tiles, row-major

A cell of side `S` at posting `P` holds `T × T` tiles with

```
T = S / (16·P)          (a power of two: 1 ≤ T ≤ 2^11)
```

A **cell block** is those tiles concatenated, row-major with `ti` advancing latitude — the same
order as the samples inside a tile, one level up:

```
byte offset of tile (ti, tj) within the block = (ti · T + tj) · 512
byte length of a cell block                   = T² · 512
```

At the v1 pairing, `T = 64` and a cell block is 2 MiB. The `T ≤ 2^11` bound keeps a cell block
inside the `uint32` offsets the directory is made of.

A cell block is **complete**: every tile is present, including tiles that are entirely `NODATA`.
There is no per-tile presence bit and no sparse encoding in v1; the `flags` field (§4.2) is
reserved for a future per-tile encoding.

### 3.3 Addressing a sample

Given a lattice sample `(i, j)` inside a present cell whose block starts at absolute offset
`base`, its byte offset is a chain of shifts and masks — no division, no table, no search:

```
li = i − ci·S/P            lj = j − cj·S/P                (sample within the cell)
ti = li >> 4               tj = lj >> 4                   (tile within the cell)
r  = li & 15               c  = lj & 15                    (sample within the tile)

offset = base + (ti·T + tj)·512 + (r·16 + c)·2
```

---

## 4. The container

### 4.1 File layout

```
[Header]              32 bytes, fixed (§4.2)
[Offset Directory]    4 · CellRows · CellCols bytes (§4.3)
[Cell Block]          T² · 512 bytes, one per present cell (§4.4)
[Cell Block]
...
```

All multi-byte integers are **little-endian**.

The same layout is both published artifacts:

- a **terrain cell** — what a bakery publishes and a catalog names — is a container whose cell
  rectangle is `1 × 1`;
- a **terrain shard** — what an assembler builds for a selection and a rider carries — is a
  container whose rectangle covers that selection.

There is no separate cell format.

### 4.2 Header (32 bytes)

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Magic | 4 | `char[4]` | Must be `b"OBCT"` |
| 4 | Version | 1 | `uint8` | `0x01` |
| 5 | Posting Log2 | 1 | `uint8` | `log2(P)` in µdeg; `4 … 16`. v1 data: `9` |
| 6 | Cell Log2 | 1 | `uint8` | `log2(S)` in µdeg; `10 … 28`. v1 data: `19` |
| 7 | Flags | 1 | `uint8` | Reserved. MUST be `0` in v1 |
| 8 | Cell Min I | 4 | `uint32` | Cell rectangle's minimum **latitude** cell index |
| 12 | Cell Min J | 4 | `uint32` | Cell rectangle's minimum **longitude** cell index |
| 16 | Cell Rows | 2 | `uint16` | Cells in latitude, ≥ 1 |
| 18 | Cell Cols | 2 | `uint16` | Cells in longitude, ≥ 1 |
| 20 | Directory Offset | 4 | `uint32` | Absolute byte offset of the offset directory. A v1 producer MUST write `32` |
| 24 | Reserved | 8 | `uint8[8]` | MUST be zero |

The header carries **no bounding box**: the cell rectangle *is* the bounding box (see
[`OBCC_Spec.md` §8](OBCC_Spec.md), and §13.1 for a terrain cell's own entry).

`Flags` is this format's extension point: a future per-tile packed encoding (§3.2) sets a bit here.
A v1 reader MUST refuse a file with any bit set.

A reader follows the `Directory Offset` field rather than the fixed v1 value.

### 4.3 The offset directory

`CellRows · CellCols` `uint32` entries, row-major over the cell rectangle with the **latitude**
index as the row:

```
slot(ci, cj)  = (ci − CellMinI) · CellCols + (cj − CellMinJ)
entry         = absolute byte offset of that cell's block, or 0
```

`0` means **the cell is not in this file**. No block can start at 0, because the header does.

An entry that is not `0` MUST be even, MUST be at or after the end of the directory, and its whole
`T² · 512` bytes MUST lie inside the file. Present cells MAY appear in any order in the file; the
directory is the only thing that places them.

The rectangle is dense: it costs 4 bytes per cell in the bounding box, covered or not.

### 4.4 Cell blocks

Each present cell's block is `T² · 512` bytes laid out per §3.2. Blocks are contiguous in v1 and a
producer SHOULD write them in directory order; a reader MUST NOT rely on either, and MUST use the
directory.

### 4.5 What a reader MUST reject

A reader MUST refuse the file — not the individual query — when any of the following holds. All of
them are properties of the bytes, and are checked once at parse.

1. The file is shorter than 32 bytes, `Magic` is not `OBCT`, or `Version` is not `0x01`.
2. `Flags` is not `0`, or any reserved byte is not `0`.
3. `Posting Log2` is outside `4 … 16`, or `Cell Log2` is outside `10 … 28`.
4. `Cell Log2 − Posting Log2 < 4` — a cell smaller than one tile — or `> 15`, which would put a cell
   block past `uint32`.
5. `Cell Rows` or `Cell Cols` is `0`, or the rectangle runs off the world grid:
   `CellMinI + CellRows > WORLD_SIDE / S`, likewise for `J`.
6. `Directory Offset` is below 32, or the directory does not lie wholly inside the file.
7. Any directory entry is non-zero and is odd, or lies before the end of the directory, or its
   `T² · 512` bytes do not lie wholly inside the file.

### 4.6 File naming

A terrain artifact's file extension is **`.obcd`** (8.3: `.OBD`), *not* `.obct`: the device's
recorded ride log already uses `.obct`
([`obc-formats/src/track.rs`](../firmware/obc-formats/src/track.rs)). The magic stays `OBCT`.

An assembly's raster is not a file on the card at all: it is spliced into the map file's terrain
region ([`OBCM_Spec.md` §1.3](OBCM_Spec.md)). A **published cell** is an object of its own, named
by the catalog.

---

## 5. Sampling

This section is normative and exhaustive. **Two independent implementations MUST produce
bit-identical results for the same `(lat, lon)` against the same file.** Every step below is
integer arithmetic for that reason.

### 5.1 The algorithm

Given `lat`, `lon` in µdeg:

1. **Domain.** If either coordinate is outside `[GRID_ORIGIN, GRID_ORIGIN + WORLD_SIDE)`, the
   result is `None`.
2. **Lattice.** With `P = 2^PostingLog2`:

   ```
   i = (lat − GRID_ORIGIN) >> PostingLog2      a = (lat − GRID_ORIGIN) & (P − 1)
   j = (lon − GRID_ORIGIN) >> PostingLog2      b = (lon − GRID_ORIGIN) & (P − 1)
   ```

   The subtraction MUST be evaluated in a type wider than `int32`: `lat − GRID_ORIGIN` overflows
   `int32` for coordinates near its top end.
3. **Containing cell.** Let `(ci, cj)` be the cell owning sample `(i, j)` (§3.1). If it is outside
   the rectangle or its directory entry is `0`, the result is `None`. A query is **not** answered
   from a neighbouring cell: nothing is extrapolated into a hole or beyond coverage.
4. **Corners.** Resolve the four samples `(i, j)`, `(i+1, j)`, `(i, j+1)`, `(i+1, j+1)` per §5.3.
5. **`NODATA`.** If any resolved corner is `NODATA`, the result is `None` (§5.4).
6. **Interpolate** per §5.2.

A query exactly on a lattice point has `a = b = 0`, so the interpolation collapses to `v00` and
returns that sample unchanged.

### 5.2 Interpolation and rounding

With the four corner values `v00 = h(i, j)`, `v10 = h(i+1, j)`, `v01 = h(i, j+1)`,
`v11 = h(i+1, j+1)` and the remainders `a`, `b` from §5.1:

```
num = v00·(P−a)·(P−b) + v10·a·(P−b) + v01·(P−a)·b + v11·a·b
h   = round(num / P²)
```

`num` MUST be computed in a signed 64-bit accumulator (it reaches ≈ 1.4 · 10¹⁴ at the coarsest
permitted posting) and MUST NOT be evaluated in floating point.

**Rounding is half away from zero**:

```
h = num ≥ 0  ?   (num + P²/2) / P²   :   −((−num + P²/2) / P²)          (truncating division)
```

Since no corner is `NODATA` at this point, `num / P²` is a weighted mean of values in
`−32767 … 32767`, so the result always fits `int16`.

### 5.3 Cell seams and coverage edges

A corner may lie outside the query's containing cell — at most one sample beyond it on each axis,
by construction. Resolve each corner as follows:

1. Let `(ci', cj')` be the cell owning that corner (§3.1). If `(ci', cj') = (ci, cj)`, read the
   sample from the containing cell's block.
2. Otherwise, if `(ci', cj')` is inside the rectangle **and** present, read the sample from *its*
   block. This is the **cross-cell fetch**, which makes the surface continuous across a seam.
3. Otherwise — the corner's cell is absent or outside the rectangle — **clamp**: replace the corner
   with the nearest sample of the **containing** cell, i.e. clamp each out-of-cell axis index to
   that cell's maximum sample index on that axis. The weights `a`, `b` are **not** changed.

Step 3 applies to **absence only**. A failed read — of a directory entry, of a tile — is not
absence, and an implementation MUST NOT let one fall into the clamp: it makes the whole sample
`None` per §5.1.

Clamping is the coverage-edge rule. It makes the surface flatten over the last half posting at the
outer boundary of coverage instead of jumping to `None`. Step 3 fires for a hole *inside* the
rectangle too: terrain plateaus for at most one posting as it approaches a hole, and then step 3 of
§5.1 makes the hole itself `None`.

### 5.4 `NODATA` propagation

If **any** of the four resolved corners is `NODATA`, the sample is `None`. There is no partial
interpolation over the remaining corners and no nearest-neighbour substitution.

Consumers MUST treat `None` as "no height here" and MUST NOT substitute `0`. `0` metres is a real
elevation.

### 5.5 What a consumer may assume

- **Determinism.** Two calls with the same coordinate against the same file return the same value,
  always. A tile cache changes how many bytes are read, never what is returned.
- **Continuity.** Within a connected covered region, the sampled surface is continuous — including
  across cell and tile seams, which is the point of §5.3 step 2.
- **Exactness on a plane.** If the sampled region's heights are an affine function of the lattice
  indices, the interpolated value equals that function evaluated at the query point, rounded per
  §5.2. This is what makes a synthetic plane an *oracle* for a second implementation.

### 5.6 Worked example

The checked-in vector [`vectors/terrain-shard.obcd`](vectors/terrain-shard.obcd) is a `2 × 2` cell
rectangle at `PostingLog2 = 9`, `CellLog2 = 14` (so 32 samples and 2 × 2 tiles per cell, a 2048-byte
cell block), with `CellMinI = 19251`, `CellMinJ = 16871`, the far cell **absent**, and heights
`100 + 3·di + 5·dj` metres over the lattice offset `(di, dj)` from the rectangle's base sample —
which sits at `lat 46 972 928`, `lon 7 979 008` µdeg. One sample, at `(di, dj) = (40, 5)`, is
`NODATA`. Its full field values are in [`vectors/manifest.json`](vectors/manifest.json).

**Inside a tile.** Query `lat = 46 974 208`, `lon = 7 980 672`.

```
i − base_i = 2, a = 256          j − base_j = 3, b = 128          P = 512
v00 = h(2,3) = 121   v10 = h(3,3) = 124   v01 = h(2,4) = 126   v11 = h(3,4) = 129
num = 121·256·384 + 124·256·384 + 126·256·128 + 129·256·128 = 32 440 320
h   = (32 440 320 + 131 072) / 262 144 = 124                     (exactly 123.75, rounded away)
```

**Across a cell seam.** Query `lat = 46 989 056`, `lon = 7 980 544` — half a posting below the
latitude boundary between cell `(0,0)` and cell `(1,0)`. `v00 = h(31,3) = 208` comes from the first
cell; `v10 = h(32,3) = 211` is the **first sample of the second cell**, fetched per §5.3 step 2.
The result is `209.5 → 210`: the plane stays a plane across the seam.

**At the coverage edge.** Query `lat = 46 973 952`, `lon = 8 011 520` — half a posting east of the
rectangle's last sample column. The `j+1` corners live in a cell that is not in the file, so §5.3
step 3 clamps them back to column `63`, and both longitude corners carry `h(2,63) = 421`. The result
is `421`, not the `424` an extrapolation would have produced.

---

## 8. Version 3: geographic surface index

Version 3 adds terrain data for arbitrary-viewpoint panoramas. It stores no observer position,
view direction, panorama pixels, or baked lighting. Sections 1–5 still define the native height
lattice and ordinary elevation sampling. This section overrides the v1 container layout where
specified below.

### 8.1 Header and cells

The header remains 32 bytes. `Version` MUST be `3`, flag bit 0 MUST be set, and bytes `24..32`
MUST be zero. Flag bit 1 indicates the cross-cell maximum index in section 8.3. Other flag bits
MUST be zero, so `Flags` is `1` or `3`. Producers MUST set both bits (`Flags = 3`); readers also
accept `Flags = 1` without that index. The directory retains one little-endian `uint32` offset per geographic cell.
Zero means absent. Every present cell MUST start at a multiple of 512 bytes relative to the
container. The producer MUST pad the header and directory to that boundary with zero bytes.
When flag bit 1 is set, the cross-cell index starts at that boundary and precedes all cell blocks.
The complete container MUST fit within `uint32` byte addressing.

A cell contains a complete height pyramid. Level `i` has posting `posting_log2 + i`.
The last level has 16 posts along each axis, so the level count is
`cell_log2 - posting_log2 - 3`. Derived postings can exceed the native header's posting limit.
All cells in one container have the same levels and byte length. A reader MUST reject version 2;
its reserved group bytes do not carry the approximation contract below.

Each level contains, in this order:

1. Its signed 16-bit heights, using the 16×16 tile order of section 2.
2. Its maximum-height groups, as specified in section 8.2.
3. Zero padding to the next 512-byte boundary.

The next level starts at that boundary. The native level starts at cell offset zero. Its height
bytes MUST be identical to the equivalent v1 cell. Coarser levels select the corresponding
native lattice posts; they MUST NOT average heights or shift the lattice. Ordinary elevation
consumers continue to read the native level only.

For posting 9 and cell size 19, the seven levels have postings 9 through 15. The complete cell is
3,149,824 bytes, compared with 2,097,152 bytes for its native heights alone.

### 8.2 Maximum-height groups

A maximum node covers `2^b × 2^b` interpolation intervals, where `b` starts at 2 and ends at
that level's samples-per-cell exponent. Its signed 16-bit value MUST be at least every height
at the intervals' vertices, including the north and east edge vertices. A producer MUST include
vertices across a geographic cell seam. A vertex whose height is unknown makes the node value
`32767`; a consumer MUST treat that value as an unbounded maximum. The maximum is conservative
for bilinear interpolation because its weights are non-negative inside each interval.

Four consecutive node levels share one 256-byte group:

| Byte offset | Nodes | Order |
|---|---:|---|
| 0 | 8×8 | Row-major, latitude first |
| 128 | 4×4 | Row-major |
| 160 | 2×2 | Row-major |
| 168 | 1 | Group root |
| 170 | 85 error bytes | Same node order as the maxima |
| 255 | 1 byte | Zero padding |

The first group plane starts at `b = 2`; subsequent planes start at `b = 6, 10, …`, while
`b` does not exceed the samples-per-cell exponent. A plane's groups are row-major. It has
`2^max(0, samples_log2 - b - 3)` groups along each axis. An incomplete group retains its full
256 bytes. Nodes outside the cell or above its root MUST be zero and MUST NOT be queried.
All group planes are concatenated in increasing `b` order after the height tiles.

Each error byte describes the bilinear patch through the node's four inclusive corner heights.
Its low nibble bounds the maximum absolute height residual, in metres. Its high nibble bounds
the maximum absolute residual of each gradient component, in metres per interval of this level.
Code 0 means exact; code 15 means unknown. For codes 1 through 14, the bound is
`2^(code-1) * unit`, where `unit` is 0.25 m for height and 0.0625 m per interval for gradient.
The producer MUST round each bound upward. Any unknown vertex MUST produce code 15 for both.

Height residuals MUST cover every source vertex in the node. Gradient residuals MUST cover
both endpoints of every source grid edge. These bounds apply throughout the bilinear source
patches: height differences are bilinear, and each gradient difference is affine along an edge.
A consumer MUST use the same true corner heights; it MUST NOT substitute a clamped boundary
corner while retaining these bounds. Coarser stored levels may supply the same exact vertices.
A consumer MUST bound both projected height error and physical gradient error before merging
cells into that patch.

The exact layout arithmetic is implemented by `SurfaceLayout` and `SurfaceLevel` in
[`obct_surface.rs`](../firmware/obc-formats/src/obct_surface.rs).

### 8.3 Cross-cell maximum index

The index starts at `align512(directory_offset + cell_rows * cell_cols * 4)`. It contains
little-endian signed 16-bit maximum heights, in consecutive row-major planes. Plane 0 has
`cell_rows × cell_cols` nodes. Each node is the corresponding native cell's inclusive root
maximum from section 8.2, or `32767` when that cell is absent. A plane's node coordinates are
relative to the container rectangle, not to the world grid.

Each next plane takes the maximum of each 2×2 group in the preceding plane. Its dimensions
are `ceil(rows / 2) × ceil(cols / 2)`. Children outside the rectangle are omitted; unknown
children inside the rectangle remain `32767` and propagate to the root. The last plane is
1×1. The producer MUST pad the index end to 512 bytes with zeros. No cell block may overlap
the index or its padding. A reader MUST reject a truncated index.

One index serves all surface levels. Native cell bounds include the high-edge vertices, and
every coarser level selects a subset of those vertices. Thus the native maximum also bounds
all coarser bilinear surfaces. An 8×8 cell rectangle needs 170 index bytes plus 342 padding
bytes.

### 8.4 Assembly

The assembler MUST preserve each published cell's complete block. It MUST NOT combine native
and indexed cells in one container. An embedded v3 terrain region MUST also start on a 512-byte
boundary in the complete OBCM file. Bytes used to reach this outer boundary are OBCM filler;
bytes inside the OBCT prefix and cell blocks are zero padding.

---

## 9. Crest lifts (producer rule)

A native posting of `2^9` µdeg cannot hold a rock tower. A **lift** is the correction: at a node
where a finer reference DEM says the ground stands far above the native bilinear surface, and the
terrain there is convex, the producer raises the sample to the reference ground. This is the
section 1.2 exception, and it is a producer rule, not a container feature. The lifted value is in
the sample itself, so section 8's pyramid, maxima, error codes and cross-cell index need no rule of
their own.

Copernicus stays the base raster. A national DTM supplies lifts only: it sits below a surface model
over every forest and town, so it MUST NOT become the base.

### 9.1 The rule

For each node of the native level, let `node_max` be the maximum reference height inside the node's
half-posting cell — the square reaching half an interval from the node on both axes.

A node is **selected** when both of these hold:

1. The largest **gap** over the node's cell is more than **10 m**. The gap at a point is the
   reference height there minus the native bilinear surface there, so the largest gap is
   `max(reference - surface)` over the cell. This maximum and `node_max` are taken independently and
   need not occur at the same point.
2. `node_max` exceeds the mean of its four neighbours' `node_max` by more than **3 m**.

The selection is then **dilated by one node**, 8-connected: a node that touches a selected node,
including at a corner, is lifted too.

The lift at a lifted node is

```
lift = max(0, min(round(node_max) - native, round(gap)))   whole metres
```

and the baked sample is `native + lift`. `round` is half away from zero, the rule section 5.2 pins
for the read side.

A node with `NODATA` anywhere in the 3 × 3 native lattice around it MUST NOT be lifted, not even by
the dilation: there is no bilinear surface there to measure a gap against.

`node_max` and the gap are sampled on a probe grid the producer chooses. The v1 bakery uses **every
reference pixel inside the node's half-posting cell**: its reference is an archive on a `2^6` µdeg
lattice, so that is 64 probes per node at the v1 posting.

A probe that is a **maximum over an area** must have its gap measured against the **highest point of
the surface over that same area**, not against the surface under one point of it. Each archive pixel
is the maximum of the source pixels inside its square and may have come from anywhere in that
square, so the gap the bakery credits it is `pixel − max(surface over the pixel's square)`. Measured
this way, the gap can only read low, which loses a correction rather than inventing ground.

This document therefore does **not** promise that two producers agree byte for byte on a lifted cell.
It promises the identity of section 9.2: where there is no coverage, there is no difference.

Every part of the rule reads only a node's 2-ring of `node_max` values. A producer MUST therefore
compute a lift from that neighbourhood alone, so that a node on a cell seam gets the same lift
whichever of the two cells is being baked.

### 9.2 Coverage

Reference coverage may stop at any node: national datasets stop at borders. A node the reference
misses, at itself or at any of the four neighbours a test reads, is not selected. A cell with no
reference coverage at all MUST be byte-identical to the same cell baked without a reference, and a
cell with coverage has the same byte length as one without.

### 9.3 What a producer MUST NOT do

A lift MUST NOT be negative: section 9 raises crests, and a reference that sits below our surface in
a hollow is not a reason to edit the lattice there. A producer MUST NOT use a lift to carry any
other correction; a systematic disagreement with the source is a re-bake of the source, not a lift.

There is **no fixed ceiling** on a lift, and a producer MUST NOT impose one. The gap and `node_max`
bound a lift, and nothing else may.

A producer SHOULD report the largest lift of a run, the node it is at, and how many lifts exceed
200 m, so that an operator sees a broken reference. Nothing in a container records this and no
reader can check it; `obc-dem bake` prints it.

A change of reference archive changes the baked samples, so it is a terrain revision bump and hence
a navigation re-bake (`OBCC_Spec.md` §13.4). The reference DEM keeps its own attribution, which
MUST travel with every container derived from it.
