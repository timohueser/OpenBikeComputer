# OBCT — OpenBikeComputer Terrain Tiles (v1)

OBCT is the terrain artifact: a raster of ground heights on the [OBCA](OBCA_Spec.md) cell grid,
carried **beside** the map rather than inside it. It defines four things:

1. **The sample lattice** (§1) — a fixed, global microdegree lattice sharing OBCA's origin, with
   `int16` metre heights and one reserved "no data" value.
2. **The tile and the terrain cell** (§2, §3) — a 512-byte tile of 16 × 16 samples, and the grid
   cell that is a square block of them.
3. **The container** (§4) — one file format for both published artifacts: a **cell** is a
   container whose cell rectangle is 1 × 1, an **assembly raster** one covering a whole selection. A fixed header, a row-major offset directory over that rectangle,
   then the cell blocks. Every lookup is grid arithmetic; nothing is searched.
4. **Sampling** (§5) — the normative bilinear rules, including what happens at a cell seam, at a
   coverage edge and around a `NODATA` sample. This is the raster analogue of OBCA §3.4's seam rule:
   **two independent implementations MUST produce bit-identical heights for the same `(lat, lon)`.**

OBCT introduces **no new OBCM version and changes no OBCM semantics**. `obc-reader`, `obcm_diff`
and the assembler's graft path are untouched; terrain is a new artifact class with its own revision
track, which is the whole reason it is not an OBCM section (§0.1).

> **Where the bytes live changed in OBCM v14 (#1420); what they are did not.** An assembly's raster
> is now spliced into the map file's terrain region ([`OBCM_Spec.md` §1.3](OBCM_Spec.md)) instead of
> riding beside it as a volume-set role or a sidecar. The container is embedded **verbatim** and the
> map reader hands it over as a window rather than parsing it, so every argument in §0.1 for keeping
> terrain out of OBCM's *parse* survives intact — an OBCM consumer still learns no terrain section,
> and a terrain re-bake still touches no map semantics. One file, two formats, one of them opaque to
> the other.

This document is normative. The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are to be
interpreted as in RFC 2119.

Its code authority for magic, version, field offsets, fixed lengths, sentinels and the layout
arithmetic is [`firmware/obc-formats/src/obct.rs`](../firmware/obc-formats/src/obct.rs); producers
and consumers import those facts directly rather than transcribing them. The reference reader,
sampler and tile cache are [`firmware/obc-elevation`](../firmware/obc-elevation).

Related contracts: [`OBCA_Spec.md`](OBCA_Spec.md) §1 defines the grid this raster sits on (its §5
volume sets are superseded — an assembly raster now ships inside the map); [`OBCC_Spec.md`](OBCC_Spec.md) §13 publishes terrain cells as a
catalog artifact class with its own revision track; [`OBCM_Spec.md`](OBCM_Spec.md) is the map beside which terrain is carried,
and whose §8 nav graph stores the ascent integrated *from* these samples.

## Design principles

1. **Terrain is static; OSM churns.** A cell store re-bakes on any OBCM or schema bump (OBCA §6.3).
   Terrain inside OBCM would re-publish hundreds of MiB of unchanged raster on every bump. As its
   own artifact class with its own revision, terrain is baked once per **dataset** version and
   survives every map-format change.
2. **One sampling truth.** The packer samples baked OBCT tiles when it integrates per-edge ascent;
   the device samples the same tiles when it fills a planned route's elevation and when it draws the
   profile. The router's numbers, the drawn profile and the altimeter reference agree **by
   construction** — one implementation of §5, over one artifact — rather than by luck. It is also
   why the packer needs no DEM decoder: it reads what was baked, like everyone else.
3. **Addressing is arithmetic, never search.** Cell, tile and sample are all reached by shifts and
   masks on the query coordinate, because the lattice, the tile and the cell are all powers of two
   on one origin. A reader holds a 32-byte header and reads one `uint32` to place a cell. There is
   no index to walk, no bbox to compare, and nothing to sort.
4. **Sizes are data; shape is format.** The posting `P` and the cell side are **header fields**
   (§1.3), so retuning either is a terrain re-bake, not a format bump — the OBCA §1.5 idiom. The
   *tile* is not: 16 × 16 samples = 512 B is the device's I/O quantum (one SD block, the size of an
   OBCM §8 nav chunk), and changing it would change the fetch unit every consumer is budgeted
   around.
5. **One container for both artifacts.** A published cell and an assembly's raster differ only in
   how many cells they carry, so they are one format (§4). An assembler that concatenates cells
   writes the same bytes a baker writes; a reader that samples an assembly raster samples a cell
   with no branch.
6. **A hole is silence, never a guess.** Terrain is `None` outside coverage and `None` wherever the
   source DEM had no data — and one missing corner voids the whole sample (§5.4). Elevation degrades
   to "not known here"; it never degrades to a plausible-looking number, because every consumer of
   it (routing cost, profile, altimeter reference) would rather have nothing than a fabrication.
7. **Removable.** Delete the terrain file and the map still renders, routing still works and
   profiles go flat. Nothing else in the system parses these bytes, and the seam every consumer
   wires through has a null implementation that is bit-for-bit the behaviour of not having terrain
   at all.

### 0.1 Why not an OBCM section

The alternative — a raster section inside OBCM, like POIs (§7) or the nav graph (§8) — was rejected
on four counts, all of which are properties of *terrain*, not preferences:

- **Revision lockstep.** OBCA principle 5 makes every cell in an assembly share one OBCM version and
  schema revision. Terrain would inherit that and be re-published on every unrelated bump.
- **Blast radius.** Every existing consumer of OBCM would have to learn a section it never reads.
  As a separate artifact, `obc-reader`, `obcm_diff` and `obcm-testkit` are untouched.
- **Splittability.** A raster splits by bbox trivially, so it never spent the core file's headroom
  when that was the scarcest resource in a set (OBCA principle 7, superseded with OBCA §5). The
  property still holds and is still worth having; it is simply no longer paying for anything, since
  OBCM v14's interior has room for the raster and the map together.
- **Independence.** A rider who does not want terrain does not download it, and a terrain re-bake at
  a new posting does not touch the map.

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

with `0 ≤ i, j < WORLD_SIDE / P`. The same expression on **both** axes: the lattice is square in
µdeg, not in metres, exactly as OBCA cells are and for the same reason — a lattice that "corrected"
for latitude would not nest with the grid, and nesting is what makes §3.3's addressing arithmetic.

> **Consequence, stated once.** A `2^9` posting is ≈ 57 m in latitude everywhere and ≈ 39 m in
> longitude at 47°N, narrowing towards the poles. Samples are therefore denser on the ground the
> further north a rider is. This is not corrected anywhere and MUST NOT be: the whole contract rests
> on the lattice being derivable from the coordinate by a shift.

The lattice does not wrap, and it spans the world box rather than the geographic domain — the
antimeridian and pole rules of [`OBCA_Spec.md` §1.4](OBCA_Spec.md) apply unchanged.

### 1.2 Values

A sample is a signed 16-bit little-endian integer:

| Value | Meaning |
| :-- | :-- |
| `-32767 … 32767` | height in **whole metres**, orthometric (EGM2008) |
| `-32768` (`i16::MIN`) | **`NODATA`** — no height is known at this sample |

Heights are orthometric — height above the geoid, what the source DEM ships and what a rider reads
off a signpost — **not** ellipsoidal. A producer MUST NOT write `-32768` as a real height; the
range it gives up is 1 m at the bottom of the Mariana Trench, and what it buys is a sentinel that
needs no separate mask plane.

Whole metres, not decimetres: the raster's own vertical error at a bikepacking posting is metres,
and a finer unit would be false precision at twice the bytes.

### 1.3 Posting and cell size are data

`P` and the cell side are **header fields** (§4.2), not constants of this document. The v1 baked
values are

| Quantity | v1 value | ≈ at 47°N |
| :-- | :-- | :-- |
| Posting `P` | `2^9` µdeg | 57 × 39 m |
| Terrain cell | `2^19` µdeg | 58 × 40 km — 1024 × 1024 samples, 2 MiB raw |

and they are the packer/bakery's choice, published in the catalog. Retuning them is a terrain
re-bake, **not** a version bump of this format (the [`OBCA_Spec.md` §1.5](OBCA_Spec.md) idiom).
A finer `2^8` posting (≈ 28 m) was measured and rejected: cycling-grade gradient signal lives at
≥ 100 m scales, and it costs +17–26 % of a whole map for detail no rider can act on.

A reader MUST accept any pairing this document permits (§4.5), not only the v1 one. Test fixtures
in particular use a small cell so that a whole multi-cell rectangle fits in a few KB.

---

## 2. Tiles

A **tile** is `16 × 16` samples = **512 bytes**, the unit of every read.

```
tile bytes[ (row · 16 + col) · 2 ] … +2      row, col ∈ 0..16, little-endian int16
```

Row-major, and **rows advance latitude**: `row` steps north by one posting, `col` steps east by one
posting. So the 32 bytes of one row are 16 consecutive longitudes at one latitude, and the tile's
first sample is its **minimum** corner in both axes.

> **Why this order, and why it is worth stating.** It is the opposite of the north-up scanline a
> GeoTIFF ships (row 0 = the *northernmost* line). A baker therefore flips rows on the way in, once,
> deliberately; a consumer never flips anything. Choosing the format's own axis direction over the
> source's keeps every on-device index a plain addition — and this is precisely the sort of
> convention two implementations would otherwise each guess at, which is why §5's determinism
> requirement makes it normative rather than advisory.

One tile spans `16·P` µdeg on each axis — at the v1 posting, `2^13` µdeg ≈ 910 m of latitude.

512 bytes is not a coincidence: it is one SD block and one OBCM §8 nav chunk, so a tile fetch is a
single aligned read on the device's slowest path and the tile cache's slot size is the same number
the rest of the firmware already budgets in.

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
like the square itself. The sample lying exactly on the boundary between two cells belongs to the
upper one, once, in the whole world. This is the raster's version of OBCA principle 3: a seam is
resolved by definition rather than by tolerance, so no sample is ever stored twice and no
consumer has to decide which copy is authoritative.

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

At the v1 pairing, `T = 64` and a cell block is 2 MiB. The `T ≤ 2^11` bound is arithmetic, not
taste: one more doubling would put a cell block past the `uint32` offsets the directory is made of.

A cell block is **complete**: every tile is present, including tiles that are entirely `NODATA`.
There is no per-tile presence bit and no sparse encoding in v1 — the cell is a fixed-size array, so
addressing stays a shift, and a `flags` field (§4.2) is reserved for a future per-tile encoding that
could change that without renaming anything.

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

There is no separate cell format, and a consumer never branches on which it holds.

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

The header carries **no bounding box**: the cell rectangle *is* the bounding box, exactly as a
catalog cell entry carries none because its square follows from its id
([`OBCC_Spec.md` §8](OBCC_Spec.md), and §13.1 for a terrain cell's own entry). Two ways to say the
same thing is one way for them to disagree.

`Flags` is the extension point this format deliberately reserves: a future per-tile packed encoding
(§3.2) sets a bit here rather than needing a new magic. A v1 reader MUST refuse a file with any bit
set — an unknown encoding is not something to guess at.

`Directory Offset` is explicit even though v1 fixes it at 32, so a reader follows the field rather
than an assumption and a later version may prepend something without breaking the follow.

### 4.3 The offset directory

`CellRows · CellCols` `uint32` entries, row-major over the cell rectangle with the **latitude**
index as the row:

```
slot(ci, cj)  = (ci − CellMinI) · CellCols + (cj − CellMinJ)
entry         = absolute byte offset of that cell's block, or 0
```

`0` means **the cell is not in this file**. It needs no bit carved out of the offset space because
no block can start at 0 — the header does.

An entry that is not `0` MUST be even, MUST be at or after the end of the directory, and its whole
`T² · 512` bytes MUST lie inside the file. Present cells MAY appear in any order in the file; the
directory is the only thing that places them.

> **Why a dense rectangle rather than a list of `(i, j, offset)`.** A rectangle is O(1) with two
> subtractions and a multiply — no search, no sort, no resident index. It costs 4 bytes per covered
> *or uncovered* cell in the bbox: a DACH-shaped selection is ~2 KB of directory, against ~430 MiB
> of raster. A sparse list would save a kilobyte and cost every query a binary search, which is
> exactly the trade OBCM §4 refuses everywhere else.

### 4.4 Cell blocks

Each present cell's block is `T² · 512` bytes laid out per §3.2. Blocks are contiguous in v1 and a
producer SHOULD write them in directory order, so a file is a directory followed by a raster in
reading order; a reader MUST NOT rely on either, and MUST use the directory.

### 4.5 What a reader MUST reject

A reader MUST refuse the file — not the individual query — when any of the following holds. All of
them are properties of the bytes, so checking them once at parse is what lets §5 be free of bounds
tests on the hot path.

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
([`obc-formats/src/track.rs`](../firmware/obc-formats/src/track.rs)), and two unrelated things with
one extension on one card is a bug waiting for a directory scan. The magic stays `OBCT` — it names
the format, and it is never ambiguous, because a ride log has no header at all.

How an assembly's raster reaches a rider is not this document's business: since OBCM v14 it is
spliced into the map file's terrain region and is not a file on the card at all
([`OBCM_Spec.md` §1.3](OBCM_Spec.md)). The naming rules OBCA §5 used to state for a terrain shard —
its manifest role, its 8.3 short name, its sidecar convention — are superseded with that section.
A **published cell** is still an object of its own, named by the catalog.

---

## 5. Sampling

This section is normative and exhaustive. **Two independent implementations MUST produce
bit-identical results for the same `(lat, lon)` against the same file.** Every step below is
integer arithmetic for that reason: a float in the interpolation would make the last metre a
property of the FPU, and the packer's routing cost, the device's drawn profile and a host-side
cross-check all have to agree on it.

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

Note what step 2 gives for free: a query exactly on a lattice point has `a = b = 0`, so the
interpolation collapses to `v00` and returns that sample unchanged.

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

Half away from zero rather than `floor`: elevation is signed, and a rider crossing sea level should
not see the rounding bias flip sign with the terrain. It is also the one rule that needs no
`div_euclid` — a truncating divide plus a sign test reproduces it in any language, which matters
because a packer, a device and a browser all evaluate this expression.

Since no corner is `NODATA` at this point, `num / P²` is a weighted mean of values in
`−32767 … 32767`, so the result always fits `int16`.

### 5.3 Cell seams and coverage edges

A corner may lie outside the query's containing cell — at most one sample beyond it on each axis,
by construction. Resolve each corner as follows:

1. Let `(ci', cj')` be the cell owning that corner (§3.1). If `(ci', cj') = (ci, cj)`, read the
   sample from the containing cell's block.
2. Otherwise, if `(ci', cj')` is inside the rectangle **and** present, read the sample from *its*
   block. This is the **cross-cell fetch**, and it is what makes the surface continuous across a
   seam: a query in the last posting of a cell interpolates towards the first sample of the next
   cell, which is the same sample the next cell would use.
3. Otherwise — the corner's cell is absent or outside the rectangle — **clamp**: replace the corner
   with the nearest sample of the **containing** cell, i.e. clamp each out-of-cell axis index to
   that cell's maximum sample index on that axis. The weights `a`, `b` are **not** changed.

Step 3 applies to **absence only**. A failed read — of a directory entry, of a tile — is not
absence, and an implementation MUST NOT let one fall into the clamp: it makes the whole sample
`None` per §5.1. Absence is a fact about the file and has a defined answer; a read error is a fact
about the medium, and answering it with a neighbouring height would be the one thing principle 6
forbids — a guess that looks exactly like data.

Clamping is the coverage-edge rule. It makes the surface flatten over the last half posting at the
outer boundary of coverage instead of jumping to `None`, so a query one micro-degree past the last
sample answers the last sample rather than nothing. Step 3 fires for a hole *inside* the rectangle
too: the visible effect is that terrain plateaus for at most one posting (≈ 57 m at v1) as it
approaches a hole, and then step 3 of §5.1 makes the hole itself `None`.

> **Why clamping and voiding coexist.** They answer different questions. Step 3 of §5.1 asks "is
> this point covered?" — and an uncovered point must never be invented. §5.3's clamp asks "how does
> a covered point behave next to an edge?" — and there, the honest answer is the edge sample itself,
> which is what every texture sampler on earth does and what keeps a route's profile from
> developing a one-posting notch every time it grazes the coverage boundary.

### 5.4 `NODATA` propagation

If **any** of the four resolved corners is `NODATA`, the sample is `None`. There is no partial
interpolation over the remaining corners and no nearest-neighbour substitution.

The alternative — interpolating over whichever corners survive — was rejected because it invents a
height whose error is unbounded and undetectable: a `NODATA` region is typically water or radar
shadow, exactly where a fabricated value would be most confidently wrong. A `None` is one posting
wider than the void it guards, and that is the correct trade.

Consumers MUST treat `None` as "no height here" and MUST NOT substitute `0`. `0` metres is a real
elevation.

### 5.5 What a consumer may assume

- **Determinism.** Two calls with the same coordinate against the same file return the same value,
  always. There is no caching-dependent behaviour: a tile cache changes how many bytes are read,
  never what is returned.
- **Continuity.** Within a connected covered region, the sampled surface is continuous — including
  across cell and tile seams, which is the point of §5.3 step 2.
- **Exactness on a plane.** If the sampled region's heights are an affine function of the lattice
  indices, the interpolated value equals that function evaluated at the query point, rounded per
  §5.2. (This is what makes a synthetic plane an *oracle* for a second implementation, rather than
  a copy of the reference one.)

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

## 6. Budget (informative)

| | |
| :-- | :-- |
| Terrain at v1 posting | ≈ 0.90 MiB per 1000 km² — **+4.4–6.7 %** of a whole map, in its own file |
| DACH (~482 000 km²) | ≈ 430 MiB of terrain objects, baked once per dataset version |
| Device RAM | < 4 KB resident: a 32-byte header + a 4-slot tile cache (2 KB) + one memoized directory entry |
| Emit I/O | ~120–150 tile reads per 100 km of route, with strong locality |

The tile cache is 4 slots because a single bilinear query can straddle a tile corner and touch
exactly four tiles; fewer would thrash on the one access pattern the sampler is guaranteed to make.

---

## 7. Version history

**Version 1** (epic #1068 / #1069) — the initial format: global lattice on the OBCA origin, `int16`
metre samples with `i16::MIN` as `NODATA`, 512-byte 16 × 16 tiles, power-of-two terrain cells, one
container for cells and shards with a row-major `uint32` offset directory, and the bilinear sampling
rules of §5. Posting and cell size are header data; compression is not defined and is what the
reserved `Flags` byte exists for.
