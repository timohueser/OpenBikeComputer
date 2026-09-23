# OBCA — OpenBikeComputer Map Assemblies (v1)

OBCA is the contract that turns a **catalog of pre-baked grid cells** into the map a rider
carries. It defines the cell grid (§1), the alignment theorem that lets an assembler copy a cell's
chunk bytes verbatim (§2), what a baked **cell** is (§3), and what an **assembly** built from cells
must do (§4).

OBCA introduces **no new OBCM version and changes no OBCM semantics**. Every cell is an ordinary
[OBCM](OBCM_Spec.md) file that today's reader parses unchanged. What OBCA adds is a set of
*constraints* on how those files are produced.

This document is normative. The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are to be
interpreted as in RFC 2119.

Related contracts: [`OBCM_Spec.md`](OBCM_Spec.md) is the byte format of every cell and
assembly; [`OBCC_Spec.md`](OBCC_Spec.md) is the catalog manifest that publishes cells and names
the selectable regions; [`OBCT_Spec.md`](OBCT_Spec.md) is the terrain raster, a second artifact
class on **this same grid** (§1) with its own revision track, carried **inside** the assembled map
(`OBCM_Spec.md` §1.3).

---

## 1. The cell grid

### 1.1 Origin, world box, and cell sizes

All coordinates in this document are integer **microdegrees** (µdeg, 1e-6 degrees), the unit of
every OBCM coordinate.

```
GRID_ORIGIN   = −268435456 µdeg   (= −2^28, on BOTH axes)
WORLD_SIDE    =  536870912 µdeg   (= 2^29)
WORLD_BOX     = [GRID_ORIGIN, GRID_ORIGIN + WORLD_SIDE)  on both axes  (≈ ±268.435456°)
```

A **cell size** `S` MUST be a power of two in µdeg with `2^10 ≤ S ≤ 2^28`. A **cell** of size `S`
is the half-open square

```
cell(S, i, j) = [ GRID_ORIGIN + i·S , GRID_ORIGIN + (i+1)·S )   in latitude
              × [ GRID_ORIGIN + j·S , GRID_ORIGIN + (j+1)·S )   in longitude
```

with `0 ≤ i, j < WORLD_SIDE / S`. Cells of one size tile the world box exactly and without
overlap; cells of different sizes nest, because every permitted `S` divides `2^28` and therefore
divides `GRID_ORIGIN` and `WORLD_SIDE`.

Three properties of this definition are load-bearing:

- **Square in µdeg, not in metres.** An OBCM quadtree halves latitude and longitude *together*
  (`OBCM_Spec.md` §4), so a node can only ever coincide with a cell if the cell is square in the
  quadtree's own units. Cells are therefore taller than wide on the ground — at 47°N a `2^18`
  cell is ≈ 29 km × 20 km. Producers MUST NOT "correct" this with per-latitude sizes; it would
  break §2.
- **One origin for every band and every size.** Two bands whose cells did not nest would make the
  assembly bbox satisfy two incompatible alignment conditions.
- **A power-of-two origin, not −90/−180.** `−90 000 000` is not a multiple of any candidate cell
  size, so a grid anchored there would have no size at which cell boundaries and quadtree
  midpoints agree. `−2^28` is divisible by every permitted `S`, and the world box it spans
  (≈ ±268°) contains the whole geographic domain.

### 1.2 Bands

A **band** is a named class of cell content with one cell size. A band's cells carry a stated
subset of the schema's LOD ladder and, optionally, the non-geometry sections. The band table is a
property of the **schema revision** (§6), not of this format; the v1 values are §1.5.

Two rules are normative here:

- **Coverage.** An assembly of a selection MUST include, per band, exactly those cells of that
  band whose square intersects the selection, and MUST NOT include any other. Because coarse bands
  use larger cells, the same rule yields precise coverage at fine bands and whole covering cells at
  coarse ones.
- **Partition.** Every ladder LOD MUST belong to exactly one band, and the nav/POI sections to
  exactly one band.

### 1.3 Cell identity

The canonical textual id of a cell is

```
<log2(S)>/<i>/<j>          e.g.  18/1204/1052
```

with `i` and `j` zero-padded to `max(4, decimal_width(WORLD_SIDE / S − 1))` digits — four for every
size at or above `2^16`, which is every band the v1 table uses, and wider for the smaller sizes
§1.1 still permits. Producers MUST widen rather than truncate. Catalog entries (§6) and object paths
use this id, so a cell's URL is derivable from its **band** and its id plus a base URL — the band,
because two bands may share a cell size ([`OBCC_Spec.md` §2](OBCC_Spec.md)) — while its
**square is derivable from its id alone**, which is why a catalog cell entry carries no bbox
([`OBCC_Spec.md` §8](OBCC_Spec.md)).

Implementations MAY use the packed convenience key `(log2(S) << 24) | (i << 12) | j`, valid for
`S ≥ 2^18`. It is not normative and MUST NOT appear in any published artifact.

### 1.4 Domain edges: poles and the antimeridian

The grid is defined over the world box, which is strictly larger than the geographic domain, and
it **does not wrap**. Three consequences are normative:

- **Cells MAY extend past ±90° / ±180°.** A cell's bbox is exactly its grid square, and producers
  MUST NOT clamp it: clamping would destroy the power-of-two span that §2 depends on. Coordinates
  outside the geographic domain simply carry no features, so the overhang costs empty leaves and
  nothing else. Every field involved is an OBCM `int32` µdeg, which holds the whole world box
  (`|2^28| < 2^31`), so nothing overflows. **Consumers MUST NOT assume an OBCM header bbox lies
  within ±90 000 000 / ±180 000 000** — the reader only ever does bbox intersection on these
  values, which is well-defined outside the domain.
- **No cell straddles the antimeridian**, and the columns either side of ±180° are not adjacent on
  the grid. An assembly therefore MUST NOT combine cells from both sides: its bbox would have to
  span the globe, and no seam unification (§4.6) can ever join a node at `+179.999…°` to one at
  `−179.999…°`. A selection crossing the antimeridian is two assemblies.
- **The poles are not special.** The grid rows nearest ±90° overhang the pole; no routable way
  reaches the overhang, so no boundary junction (§3.4) can be produced there.

### 1.5 The v1 band table (schema `bikepacking`, revision 1)

These values are **schema data**: a catalog states them (§6), a producer reads them from the
catalog, and retuning them is a re-bake rather than a change to this document.

| Band | Cell size | ≈ at 47°N | Carries |
| :-- | :-- | :-- | :-- |
| `coarse` | `2^20` µdeg (1.048576°) | 117 × 80 km | ladder LOD 0, 1, 2, 3, 4 |
| `mid` | `2^19` µdeg (0.524288°) | 58 × 40 km | ladder LOD 5, 6, 7, 8 |
| `fine` | `2^18` µdeg (0.262144°) | 29 × 20 km | ladder LOD 9, 10, 11, 12, 13 |
| `network` | `2^18` µdeg (0.262144°) | 29 × 20 km | nav graph (OBCM §8), POIs (OBCM §7), hours pool (OBCM §7.5) |

The largest cell size in the table, `S_MAX = 2^20`, is the assembly bbox's alignment modulus
(§2.1).

---

## 2. The alignment theorem

### 2.1 Statement

Let `S_MAX` be the largest cell size in the schema's band table. An **assembly bbox** is a box

```
[A_lat, A_lat + 2^n)  ×  [A_lon, A_lon + 2^n)
```

that is **grid-aligned**: `A_lat ≡ A_lon ≡ GRID_ORIGIN (mod S_MAX)`, with `2^n ≥ S_MAX` and
`n ≤ 29`. (The span is a power of two and identical on both axes; the *position* need only be
`S_MAX`-aligned, so the box stays tight to the selection.)

> **Theorem.** For a quadtree built over a grid-aligned assembly bbox per
> [`OBCM_Spec.md` §4](OBCM_Spec.md), and for every band size `S = 2^s` in the table, the nodes at
> depth `d = n − s` are **exactly** the cells of size `S` that tile the assembly bbox — same
> minimum corner, same span, to the microdegree.

Two corollaries that implementations lean on:

- `n ≥ s` for every band, so the cell depth is never negative — guaranteed by `2^n ≥ S_MAX`.
- The four children are emitted **NW, NE, SW, SE**, and the assembler writes its own upper tree
  (depths `0 .. d`) breadth-first, so the cell → depth-`d` slot mapping is a pure function of
  `(i, j)` and needs no search. Below the cell depth the layout is per-cell blocks rather than
  global breadth-first order, which is legal — see §7.

A cell artifact's own header bbox **is** its cell (§3.1), so the cell file's root node is that cell
and its subtree is the same subdivision the assembly performs at that position — identical node
bboxes at every level, to the microdegree. A feature's anchor is stored relative to its **leaf's**
minimum corner (`OBCM_Spec.md` §5.2) and its deltas are relative to the previous vertex, so every
byte of a chunk decodes to the same absolute geometry in the cell file and in the assembly. §4.3
states what is therefore copied and what must be rebuilt.

Two cosmetic costs are inherent to cutting at cell boundaries and are accepted: dash phase restarts
at a cell edge, and the packer's `merge_fills` / `merge_lines` unions cannot cross a cell boundary,
so an assembly carries slightly more features than a single-shot bake of the same area.

---

## 3. Cell artifacts

### 3.1 A cell is an ordinary OBCM file

A baked cell MUST be a complete, valid OBCM file of the catalog's OBCM version, and:

- its **header bbox MUST be exactly its grid cell** (§1.1), not the content-derived box a normal
  pack computes, because §2 needs the box to be the grid square;
- it MUST write the **complete ladder** in its LOD table — one entry per schema LOD, in ladder
  order, with each entry's `Max Meters/Pixel` taken from the schema (so LOD 0 is `+inf` and the
  sequence is strictly decreasing, exactly as `OBCM_Spec.md` §3 requires). LODs outside the cell's
  band are written **empty**: `Index Node Count = 0`, `Chunk Count = 0`, and the single-`0`-entry
  offset table `OBCM_Spec.md` §5.1 mandates for an empty region. A reader walks an empty LOD and
  finds nothing, so a cell of any band stays a legal, openable map;
- the POI section and the nav section MUST be present, per `OBCM_Spec.md` §7/§8, and MUST be
  **empty** unless the cell's band carries them;
- both style tables MUST be the schema revision's **canonical table** (§6.2) — right ids, right
  count, right order, placeholder values — and both marker colors the schema's placeholder.

Because the full ladder is written, **geometry-band membership is not recorded in the cell's
bytes**. It is a property of the schema revision, read from the catalog (§6). A producer MUST NOT
infer a cell's band from which of its LODs happen to be non-empty: a legitimately empty cell — open
sea — is indistinguishable that way.

### 3.2 Determinism

> **Same source snapshot + same schema revision + same cell ⇒ byte-identical file.**

This is what lets the catalog content-address cells, lets a re-bake be a no-op, and lets two
independently baked neighbours agree on a seam coordinate (§3.4). Producers MUST ensure that:

- no wall clock, hostname, path, thread count, or map/set iteration order reaches the bytes;
- every list written is ordered by a **content-derived** key;
- floating-point geometry work is either avoided or performed so that the result is
  reproducible on the producing toolchain, and every coordinate that survives into the file is the
  result of the same integer rounding (`(deg * 1e6).round()`) the packer already uses;
- the schema revision, not the machine, fixes every threshold — simplification tolerances, cull
  areas, merge passes, and the island-prune threshold of §3.5.

Read "source snapshot" as the whole **source set**: a cell's bytes are a function of (source
snapshot set, schema revision, crop), where the source set is every co-baked extract whose coverage
intersects the cell — cut once from an ingest of exactly that set (§3.7) — and the crop is whatever
box that ingest was reduced to, which drops edge-crossing relations exactly as an extract's own
boundary does. Both belong to the determinism key, and neither may depend on the order the extracts
were named: a producer MUST key its cut plans by the **sorted** source set, so that permuting the
extracts on a command line cannot change a single output byte.

A bakery MUST record the source extract identity and snapshot date per cell and MUST NOT publish
two different byte sequences under one (cell, schema revision, snapshot) triple.

### 3.3 Cutting geometry at the cell edge

Geometry features are clipped to the cell at its **exact** boundary coordinates. A cell owns the
half-open square (§1.1), and clipping is a geometric operation, so the rules are:

- a feature wholly inside the cell is written unchanged;
- a feature crossing the boundary is **clipped at the edge line**; the clip vertices lie exactly on
  the edge (integer µdeg), so the two neighbours' clipped pieces meet with no gap and no overlap;
- a polygon clipped by an edge is closed along that edge. The resulting seam is invisible when the
  neighbour is present and is a straight edge at the coverage boundary when it is not — which is
  the honest rendering of a coverage hole;
- a feature reduced to nothing (zero-length line, zero-area polygon) by the clip is dropped;
- the per-LOD sub-pixel area cull (`min_area_px`) is applied to the **clipped** geometry, so a
  polygon may survive in one cell and be culled in its neighbour. The packer never culls lines.

Producers MUST NOT extend a cell's geometry beyond its square "for continuity": the overlap would
be written twice and drawn twice.

### 3.4 Cutting the navigation graph: deterministic boundary junctions

Every cell edge is a border, so naturally coincident junctions are not enough and proximity is
worse than nothing.

**Boundary junctions.** For every routable way that crosses or touches the cell boundary, a cell
MUST materialise a junction record at the crossing coordinate. The coordinate is computed as
follows, and the computation MUST be used verbatim by both neighbours:

1. Take the way's polyline as it exists in the **source snapshot**, in source vertex order.
2. For each segment and each cell-edge line it crosses (a constant latitude or longitude `c`):
   - if a segment endpoint already lies exactly on the line, **that vertex is the boundary
     junction** — no interpolation;
   - otherwise order the two endpoints `P`, `Q` canonically by `(lat, lon)` lexicographically
     (so the result cannot depend on the way's direction), and for a line at longitude `c`
     compute
     ```
     lat = P.lat + round_half_even( (Q.lat − P.lat) · (c − P.lon) / (Q.lon − P.lon) )
     lon = c
     ```
     in exact `i64` arithmetic with banker's rounding, and symmetrically for a line at latitude
     `c`. Both neighbours see the same `P`, `Q`, `c` and therefore produce the same integer pair.
3. A segment exactly **collinear** with an edge line belongs to the cell on the lower side of that
   line (the cell for which the line is a `min` edge, per the half-open convention), so it is
   written exactly once.
4. The boundary junction is materialised in **both** adjacent cells, each carrying its own stub
   edge inward. This is deliberate duplication: the pair is what §4.6 unifies.

**Only boundary-derived and real OSM junction nodes are load-bearing at a seam.** A cell MUST
classify junction-ness from the **source snapshot's** way set, not from the ways that survive
inside the cell, and MUST NOT rely on any *interior* synthetic node coinciding with anything.
Interior synthetic nodes are minted by the packer's own edge splits at a midpoint index of the edge
*as that run sees it* (`nav.rs::split_edge`, plus the serializer's `OBCM_Spec.md` §8.4 chunk-fit and
span splits), so two runs over a different set of ways place them metres apart.

**Exact-coordinate unification only — an epsilon snap is forbidden.** At a cell seam, genuinely
*different* junctions sit as close as **3.9 m**, so a tolerance large enough to be useful would fuse
distinct nodes and invent turns. Producers and assemblers MUST use exact integer equality and MUST
NOT offer a tolerance knob.

**Wire limits survive.** Unifying two junctions unions their adjacency and recomputes nothing, so
`OBCM_Spec.md` §8.3's degree cap of 24, its `int16` neighbour deltas, and its `uint16` `Cost M` all
hold. Boundary junctions are ordinary degree-2 nodes. An assembler MUST nevertheless re-check the
cap (§4.8) rather than assume it.

### 3.5 Island pruning at bake time: strictly interior only

A hard cut severs the road network at a line, so a fragment can hold fewer than the schema's
`min_component_edges` in *each* of two cells while being a good road once assembled. If both
neighbours drop their half, no assembly-time work can recover bytes that were never written.
Therefore:

- A cell bake MUST prune only components that are **strictly interior** to the cell — no node of
  the component lies on the cell boundary — and MUST NOT prune any component touching the
  boundary, however small.
- The real pruning pass runs at **assembly** time (§4.6), over the merged graph, where component
  sizes are finally true.
- `min_component_edges` is a property of the **schema revision**, never of the skin. Two cells
  pruned at different thresholds do not assemble into a graph with consistent semantics.

### 3.6 POIs and hours

POIs are points, so cell assignment is unambiguous: a POI belongs to the **one** cell whose
half-open square contains its coordinate. A `network`-band cell MUST write every POI in its square
and no other, and MUST carry its own deduplicated hours pool with `HoursRef` values local to the
cell. Both are rebuilt at assembly (§4.5), so a per-cell pool may be locally optimal and globally
redundant.

### 3.7 Provenance and partial cells (D3)

A cell baked from a regional extract is **not** the cell a covering source would produce, because
each extract lacks the side roads that create the neighbour's junctions. So:

- Every cell MUST record its **source extent**: the identifier of each source extract it was baked
  from, and that extract's snapshot date.
- A cell whose sources do **not** fully cover its square is **`partial`**. Coverage is decided
  against the sources' own coverage geometry (for Geofabrik-style extracts, the region polygon
  plus its complete-ways overhang), not against the packed content's bbox — content can be
  legitimately empty.
- A catalog MUST mark a partial cell as such (§6.1), and a consumer MUST NOT present a partial
  cell as canonical coverage.
- A bakery MUST replace a partial cell when a covering source becomes available, and MUST NOT
  publish a canonical cell and a partial cell for the same (cell, schema revision) pair. Co-baking
  a border cell from every extract that touches it is the sanctioned way to make it canonical
  without a planet source.

### 3.8 Known-empty coverage

A covering source can prove that a band's canonical payload for a cell is empty. OBCC may
represent those cells as compact **known-empty** row ranges rather than as complete empty OBCM
objects.

- The assertion is per `(schema revision, band, cell)` and carries the same
  source-set identities, snapshot dates, and bake timestamp as an artifact.
- It is canonical coverage: a partial source MUST NOT produce a known-empty
  assertion. Absence from both the artifact list and the known-empty ranges
  remains a coverage hole.
- A catalog MUST NOT publish an artifact and a known-empty assertion for the
  same `(band, cell)`. The assertion contributes zero bytes and no partial flag.
- A consumer includes selected known-empty identities in coverage, hole, and
  assembly-bbox arithmetic, but downloads and grafts no bytes for them. At the
  cell depth the assembler emits the same empty leaf the corresponding empty
  artifact would have contributed.

An assembly containing no artifact at all has no cell from which to verify the
schema revision's binary style and routing-profile tables. An assembler MUST
refuse that all-known-empty input rather than borrow an unselected artifact as
an implicit metadata source.

---

## 4. The assembly contract

An **assembly** is **one** OBCM file built from catalog cells for one selection and two authored skins,
carrying its terrain raster in `OBCM_Spec.md` §1.3's region when the selection has elevation. This
section defines what an assembler does.

### 4.1 Inputs and preconditions

An assembler takes a selection (any set of areas, holes allowed), a schema revision with its band
table, a light skin, a dark skin, the artifacts the coverage rule (§1.2) selects, and any selected
known-empty identities (§3.8). It MUST refuse to proceed if:

- the cells do not all carry the same OBCM version, or that version is not the one it writes; or
- the cells do not all belong to the same schema revision; or
- two cells disagree on the style table's ordered ids, or on the
  `OBCM_Spec.md` §8.6 profile table; or
- any selected cell is missing, and the caller has not accepted the resulting hole; or
- any selected cell is `partial`, and the caller has not accepted the reduced coverage.

Missing cells are legal and produce **empty leaves**; the renderer already paints backdrop there,
so a selection with holes is well-formed by construction. Known-empty cells also produce empty
leaves but are explicit canonical coverage, not holes. What is not legal is a *silent* hole.

**The assembly's size is known before anything is downloaded.** A cell's `bytes` is published per
cell ([`OBCC_Spec.md` §8](OBCC_Spec.md)) and, for a named region, per band in the catalog root
(`bytes_by_band`, [`OBCC_Spec.md` §6](OBCC_Spec.md)), and the remaining overheads do not depend on
content: the header, the schema's style table, the LOD table, the POI and nav directories, and the
per-LOD offset tables, whose sizes follow from the cell count. Therefore:

- A consumer **MUST** project the file before the download and **MUST refuse the selection** if the
  projection exceeds the per-file wall, which is **whichever is smaller**: the interior the file's
  `Offset Scale` covers ([`OBCM_Spec.md` §1.1](OBCM_Spec.md)), and what the consumer's read seam
  addresses. It MUST NOT begin fetching an assembly it cannot legally write **or read**. Both walls
  are stated by reference rather than as literals, because a number copied into this section would
  go stale against either.
- A producer SHOULD warn above **seven eighths** of that wall, and the refusal MUST name the
  coverage as the thing to reduce. One file has no split, so coverage is the only lever.
- An assembler **MUST** fail rather than emit an over-size file, and MUST NOT "solve" one by
  splitting the nav graph, dropping POIs, or degrading coverage silently.
- §4.8's verify re-checks the file's actual size, so the projection is bounded on both ends:
  refused before the fetch, and re-asserted before the write.

The terrain raster is projected the same way. Its size is
`32 + 4 · rows · cols + present · T² · 512` — a header, a directory over the assembly rectangle, and
one fixed-size block per square that has a published object — and every term is known from the
catalog: the rectangle from the assembly bbox and the terrain block's `cell_log2`, the block length
from the `posting_log2`/`cell_log2` pairing (`OBCT_Spec.md` §3.2), and which squares are present
from the pinned terrain index. A consumer MUST show the raster's bytes as their own figure rather
than folding them into the map's ([`OBCC_Spec.md` §13.3](OBCC_Spec.md)).

### 4.2 Choosing the assembly bbox

The assembler computes the minimal grid-aligned box (§2.1) containing every selected cell:

1. Let `B` be the union of every selected artifact and known-empty cell square.
2. Snap `A_lat = GRID_ORIGIN + floor((B.min_lat − GRID_ORIGIN) / S_MAX) · S_MAX`, and likewise
   `A_lon`.
3. Choose the smallest `n` with `2^n ≥ S_MAX` and `A_lat + 2^n ≥ B.max_lat` and
   `A_lon + 2^n ≥ B.max_lon`.
4. The assembly bbox is `[A_lat, A_lat + 2^n) × [A_lon, A_lon + 2^n)`, written into the header.

The box is square, so a selection that is much wider than tall (or vice versa) is padded with
empty leaves; that costs one `uint32` per empty node and nothing else. The assembler MUST NOT
shrink the box to the content afterwards — that would destroy the alignment the whole scheme rests
on.

The box MAY extend past the far edge of the world box, which is treated exactly like §1.4's domain
overhang: the cells out there do not exist, so their leaves are empty. It MUST NOT extend past the
`int32` µdeg range, which `n ≤ 29` already guarantees.

### 4.3 Copied verbatim vs rebuilt

| Part | Treatment |
| :-- | :-- |
| Geometry **chunk payload bytes** | **Verbatim.** Copied byte-for-byte (§2.1). |
| Geometry **quadtree subtrees** at and below the cell depth | Copied with **two constants per cell**: leaf values `+ chunk_id_base`, branch child bases `+ (block_base − 1)` where the cell's nodes `1..` land at assembly index `block_base` (§7). Empty-leaf sentinels and the branch bit are preserved. |
| Geometry **offset tables** | Copied with `+ chunk_byte_base` per cell; the assembler writes the `Chunk Count + 1` entries for the concatenated region. |
| Geometry **index nodes above the cell depth** | Rebuilt: a fresh tree over the assembly bbox down to the cell depth, with a branch wherever any descendant cell is present and an empty leaf where none is. |
| **LOD table** | Rebuilt (new offsets and counts; `Max Meters/Pixel` and `Chunk Size` from the schema). |
| **Header** | Rebuilt (bbox, section offsets, and both marker colors from the skins). |
| **Style tables** | Rebuilt from the skins (§4.7) — same ids, same count, same order; values replaced. |
| **POI section + hours pool** | Rebuilt (§4.5). |
| **Nav section** (directory, node tree, node chunks, edge pool) | Rebuilt (§4.6). |
| **Profile table** (`OBCM_Spec.md` §8.6) | Copied from the cells after checking every cell agrees; it is schema data. |
| **Terrain cell blocks** ([`OBCT_Spec.md`](OBCT_Spec.md) §3.2) | **Verbatim**, into a fresh directory over the assembly rectangle. Terrain assembly is *placement*, not grafting: the lattice is global and half-open, so two neighbouring cells already agree about every sample and there is nothing to relocate, re-index or unify. |

An assembler MUST NOT decode a geometry chunk in the normal path. Decoding is for verification
(§4.8).

The raster's cell rectangle **is** the assembly bbox: a terrain cell no larger than the assembly
square tiles it exactly, because the assembly corner is congruent to `GRID_ORIGIN` modulo `S_MAX`
and therefore modulo the terrain cell too. An assembler MUST refuse a terrain `cell_log2` larger
than the assembly's `span_log2` rather than overhang the box or grow it (§4.2 forbids growing it).
Squares the selection covers but the catalog does not publish an object for — canonically void
ocean ([`OBCC_Spec.md` §13.6](OBCC_Spec.md)), or ground outside the dataset — are directory `0`.

### 4.4 Grafting geometry, per LOD

For each ladder LOD `L`, with band size `S = 2^s` and cell depth `d = n − s`:

1. Order the band's present cells by their depth-`d` node index (the BFS order of
   `OBCM_Spec.md` §4), so output order is deterministic and independent of fetch order.
2. Emit the fresh upper tree for depths `0..d`, reserving a slot for every depth-`d` position and
   writing `0x7FFFFFFF` (empty leaf) where the cell is absent. A depth-`d` position whose cell is
   present takes that cell's **root node** — which may itself be a leaf or a branch, relocated as
   in §4.3.
3. Append each present cell's relocated subtree, then its offset-table entries, then its chunk
   bytes.
4. `Chunk Size` for the LOD is the schema's value; the assembler MUST verify every copied offset
   pair still spans at most that (`OBCM_Spec.md` §5.1) — a cell that violated it would poison the
   assembly.

A cell whose LOD `L` region is empty contributes an empty leaf, exactly like an absent cell.

### 4.5 Merging POIs

1. Collect every POI record from every `network`-band cell. Records are 64 bytes with **absolute**
   coordinates, so they need no relocation.
2. Deduplicate by source identity (`OBCM_Spec.md` §7.3). §3.6 gives each POI exactly one cell, so a
   duplicate is dropped and SHOULD be reported.
3. Rebuild the hours pool: collect each source blob, deduplicate the 29-byte blobs, and remap every
   service record's `HoursRef` to the new index. `0xFFFF` stays `0xFFFF`. Summit records
   (subtype 19) keep their signed elevation trailer; they do not reference the hours pool.
4. Re-bin each category into a fresh quadtree over the **assembly** bbox and re-chunk at the
   directory's shared `Chunk Size`, per `OBCM_Spec.md` §7.1–§7.3.
5. Order records within a chunk by source identity so the output is deterministic.
   Preserve the explicit approach metadata. If a spatial leaf exceeds capacity, fail the assembly
   instead of omitting service identities.

The pool count MUST not exceed `0xFFFE` distinct blobs, because `HoursRef` is a `uint16` with
`0xFFFF` reserved. An assembler MUST fail loudly rather than wrap.

### 4.5.1 Peak article associations

Core cells carry the separate `OBCM_Spec.md` §10 collection. Select associations by the summit
SourceIds retained in the merged POI set. Keep each selected summit's article without a geographic
content filter. Union summit associations, reject conflicting identities, deduplicate article
records, and relocate all indexes and guarded content references. Apply §10.4's deterministic
content precedence and bounded source-copy checks. Landmark queries remain separate.

### 4.6 Merging the navigation graph

This is the most involved rebuild, and its order matters.

1. **Read the serialized node set.** Walk each `network` cell's `OBCM_Spec.md` §8 node quadtree through a real
   reader and collect junction records, keyed by `Node Id` (`OBCM_Spec.md` §8.2's bin-packing
   means one leaf walk can yield a record more than once, so the collection MUST be idempotent).
   The set to renumber is the **serialized** one, not the one a graph builder would produce: the
   serializer mints further synthetic degree-2 junctions after the builder finishes, and they are
   in the bytes.
2. **Unify seam nodes, and only seam nodes.** Two records unify iff their coordinates are
   **exactly** equal *and* the coordinate lies on a boundary line of the `network` band's grid
   (a latitude or longitude congruent to `GRID_ORIGIN` modulo that band's cell size). Unification
   unions their adjacency. Restricting to boundary lines is not an optimisation: whole-map
   coordinate keying would also fuse the *interior* coordinate collisions that exist in a single
   file — vertically stacked bridge/tunnel junctions — inventing a turn between a bridge and the
   road beneath it.
3. **Deduplicate adjacency** keyed on `(unified endpoint pair, Cost M, Way Kind, edge polyline)`.
   The distinction that matters at a unified boundary junction: the two stubs meeting there run in
   *opposite* directions and are different edges, so both MUST survive; only an edge two cells both
   wrote in full — which the half-open ownership rules of §3.3 and §3.4(3) should already prevent —
   collapses to one.
4. **Prune islands** over the merged graph with the schema's `min_component_edges`, keeping the
   largest component plus every component at or above the threshold — the pass §3.5 deferred from
   bake time. This is the only place where the threshold means what it says: an island in the
   *map*, not in a *cell*. "Largest" is by node count, then by edge count, and — because two
   components can tie on both while only one of them can be kept — then by the component holding
   the **lowest-numbered node** of the collection order §4.6.1 read the cells in. The tie-break MUST
   be a property of the graph rather than of the search that found the components, or two
   assemblers of the same cells disagree about which islet reached the map.
5. **Renumber** the surviving nodes densely from 0, in a deterministic order (`(lat, lon)`
   ascending is sufficient and content-derived).
6. **Rebuild the edge pool.** `Edge Id` names a record by its chunk and its position within that
   chunk (`OBCM_Spec.md` §8.4 — it was a pool byte offset before OBCM v14, and either way it is a
   property of *placement*), so every edge record is re-emitted and every `Edge Id` re-derived. Edge polyline bytes MAY be copied from the
   source record (they are self-contained: absolute anchor plus deltas), but their *placement* is
   new, and the no-straddle rule must be re-applied at the 512-byte chunk granularity.
7. **Re-check the wire limits** (§4.8) and rebuild the node quadtree over the assembly bbox, with
   `OBCM_Spec.md` §8.2 bin-packed 512-byte node chunks.
8. **Copy the profile table** after confirming every cell's is identical.

An assembler MUST NOT create an edge between two nodes that no single cell joined. Unification only
ever joins *through* a coincident junction, so a merged route that steps between two nodes sharing
no source cell is a bug.

### 4.7 Stamping the skins

A **skin** is one authored presentation: per feature type a color, weight, dash bit, `color2`,
z-index and priority, plus a marker color. Each assembly stamps one light and one dark skin:

- resolve each feature type's style **id** from the schema revision's canonical assignment (§6.2)
  — neither skin MUST introduce, remove, reorder, or renumber ids;
- before any output write, compare the resolved ids against the cells' canonical table. This check
  also applies when a local schema omits the style assignment and the skins supply explicit ids;
- write both style tables with the schema's ids in the schema's order and each skin's values in the
  other seven bytes of each 8-byte record (`OBCM_Spec.md` §2);
- write both marker colors into the header.

A paired restyle changes only the two small tables and marker fields. Styles may change drawing order freely. An
assembler MUST reject a skin that does not cover every id in the schema's table, and MUST reject one
that names a feature type the schema does not have: silently defaulting a missing style would ship a
map with an invisible layer.

### 4.8 Verify obligations

An assembly is self-made and outside the catalog's guarantees, so it MUST be verified before it is
written to a device. The verify runs through the **real reader** — the same crate the firmware
uses — and MUST cover:

1. **Parse.** Header (magic, version, bbox), both style tables, LOD table, POI directory, nav directory
   and profile table all parse and validate.
2. **Every chunk, every feature.** Walk each non-empty LOD's quadtree and decode every feature of
   every chunk. Any malformed, truncated, or capacity-exceeded outcome fails the assembly. This is
   the gate that catches a mis-relocated index or a bad offset base, because a wrong `node_bbox`
   produces geometry in the wrong place *and* an anchor that no longer fits, and a wrong chunk
   base produces a stream that never meets its `0xFF` sentinel.
3. **Offset-table invariants** of `OBCM_Spec.md` §5.1 for every chunk: monotone offsets, in-region
   end, and span ≤ `Chunk Size`.
4. **Nav integrity.** Every neighbour entry's `Neighbor Id` resolves to a record in the same file;
   `Degree ≤ 24`; every `Edge Id` decodes to a record whose first and last vertices equal the two
   endpoints' coordinates; both directions of an edge agree on `Edge Id`, `Cost M`, and
   `Way Kind`; every `int16` neighbour delta reconstructs the neighbour's stored coordinate.
5. **Nav reachability, as a report.** Emit the merged component histogram. An assembler SHOULD
   surface a selection whose largest component covers an implausibly small share of the graph,
   because that is what a broken seam looks like; it MUST NOT silently repair it.
6. **The file answers for itself.** Two properties of the file alone: its header's `Offset Scale`
   MUST cover its length (`OBCM_Spec.md` §1.1), and its length MUST equal what the pre-download
   projection said it would be (§4.1). An assembler MUST refuse rather than emit a file past the
   interior that scale addresses, and MUST apply that refusal **at plan time**, because every byte
   of an assembly is computable before any of it is written.
7. **Digests.** SHA-256 of the assembled file, and of nothing inside it. In particular the
   **terrain region has no digest of its own**: it is a run of bytes inside the map, and the map has
   one identity. Step 8's read-back is what proves the raster is the bytes the catalog served.
8. **The terrain region** — the raster, spliced into `OBCM_Spec.md` §1.3. Every input is checked
   **before its bytes are copied**, because a bad cell must not reach the map even to be caught on
   the way out: each downloaded object parses as an OBCT container through the real reader
   (`OBCT_Spec.md` §4.5 — magic, version, flags, the posting/cell pairing, the rectangle against the
   world grid, and every directory entry against the file's own length, which is what rejects a
   truncated download and an out-of-bounds offset); its header's `Posting Log2` and `Cell Log2` equal
   the catalog's terrain block ([`OBCC_Spec.md` §13.1](OBCC_Spec.md)); it is the `1 × 1` container at
   exactly its own id that §13.1 requires of a *published cell*; and its SHA-256 equals the one the
   pinned terrain index published. The written region is then read back through the same reader —
   through `OBCM_Spec.md` §1.3's window, once the raster lives inside the map — and
   every present cell's block MUST equal the block of the object it came from, with every square the
   assembly did not receive at directory `0`.

A failure at any step MUST abort the whole assembly. A partially written map is not a degraded map;
it is an unmountable one, which is the correct outcome. The atomicity is the flat store's commit
(`FLAT_Store_Format.md` §5).

---

## 6. Catalog, schema, and skins

The catalog contract is [`OBCC_Spec.md`](OBCC_Spec.md); `schema_version 3` there carries cells,
bands, schemas, skins, and region cell-sets. This section states only the parts that are OBCA's to
define.

### 6.1 What the catalog must say about a cell

For each published cell: its id (§1.3), its band, its schema revision, its OBCM version read from
its own header, its size, its SHA-256, its URL, its source extents and snapshot dates, and whether
it is **`partial`** (§3.7). A consumer must be able to price an assembly — cell count and total
bytes, **per band** — from the manifest alone, before fetching anything, which is what makes §4.1's
projection arithmetic rather than estimation.

### 6.2 Schema owns ids; skin owns values

A schema revision fixes: the feature types and their `min_lod`, the LOD ladder (`Max Meters/Pixel`
per level), simplification tolerances, cull thresholds, the merge passes, `Chunk Size`, the
routing profile table, `min_component_edges`, and the **band table** (§1.2). All of it is baked
into chunk bytes, so it is the identity of a cell store.

Critically, the schema also fixes the **style-id assignment**: `obc-pack` numbers feature types
`1`-based in config document order (`OBCM_Spec.md` §2), and those ids are referenced by every
feature header in every chunk. A schema revision therefore has one **canonical style assignment** —
one id per feature type, in one order. The two skins may change the other seven bytes of each record
and their marker colors, and nothing else (§4.7).

The hosted catalog has **exactly one** schema: the 14-LOD bikepacking ladder. Hosted "presets" are
therefore skins. Custom schemas remain a local-bake affair for the desktop app.

### 6.3 Lockstep and the bake guard

An OBCM version bump or a schema-revision bump invalidates **every** cell, because assembly copies
chunk bytes between files and that is only meaningful within one revision. The bakery's guard MUST
refuse to publish a catalog that mixes OBCM versions or schema revisions across cells, exactly as
`OBCC_Spec.md` §10 already refuses a mixed-version artifact catalog, and an assembler MUST refuse a
mixed input set (§4.1).

---

## 7. Worked example

A two-cell assembly at a toy scale: one band, `S_MAX = S_fine = 2^18 = 262144`, and a two-LOD
ladder both of whose levels sit in that band. The two cells are neighbours in longitude, on the
Rhine above Basel:

```
cell A = 18/1204/1052 :  lat [47185920, 47448064)   lon [7340032, 7602176)
cell B = 18/1204/1053 :  lat [47185920, 47448064)   lon [7602176, 7864320)
```

Every minimum is `GRID_ORIGIN + k · 2^18`; in degrees the pair spans 47.185920…47.448064 °N and
7.340032…7.864320 °E.

**Assembly bbox.** The union of the two squares runs `lat 47185920 … 47448064`,
`lon 7340032 … 7864320`. Both minima are already `2^18`-aligned, so `A = (47185920, 7340032)`. The
spans are `262144` (lat) and `524288` (lon), so the smallest square power-of-two side covering both
is `2^19`, giving `n = 19`:

```
assembly bbox = lat [47185920, 47710208)  ×  lon [7340032, 7864320)
```

**Cell depth.** `d = n − s = 19 − 18 = 1`, so the depth-1 nodes are the cells. The root splits at
`mid_lat = (47185920 + 47710208) / 2 = 47448064` and `mid_lon = (7340032 + 7864320) / 2 = 7602176`
— exactly cell A's northern edge and exactly the two cells' shared edge. The four children, in
NW, NE, SW, SE order:

```
node 1  NW = lat [47448064, 47710208)  lon [7340032, 7602176)   → no cell   → empty leaf
node 2  NE = lat [47448064, 47710208)  lon [7602176, 7864320)   → no cell   → empty leaf
node 3  SW = lat [47185920, 47448064)  lon [7340032, 7602176)   → cell A    ✓
node 4  SE = lat [47185920, 47448064)  lon [7602176, 7864320)   → cell B    ✓
```

Nodes 3 and 4 *are* cells A and B, to the microdegree.

**Grafting LOD 1.** Say cell A's LOD 1 index is five nodes — a root branch plus its four children,
two of which hold chunks —

```
A: [0x80000001, 0x00000000, 0x00000001, 0x7FFFFFFF, 0x7FFFFFFF]   chunks: 1200 B, 800 B
                                                                  offsets: [0, 1200, 2000]
```

and cell B's is a single leaf:

```
B: [0x00000000]                                                   chunks: 1500 B
                                                                  offsets: [0, 1500]
```

The assembler writes the fresh upper tree (root + four children) at indices `0..4`, inlines each
cell's **root value** into its depth-1 slot, and appends each cell's remaining nodes as a
contiguous block:

```
idx 0 : 0x80000001    fresh root branch, children at 1..4
idx 1 : 0x7FFFFFFF    NW empty
idx 2 : 0x7FFFFFFF    NE empty
idx 3 : 0x80000005    SW = A's root, child base 1 relocated by +4  →  A's children at 5..8
idx 4 : 0x00000002    SE = B's root leaf, chunk id 0 relocated by +2 (A owns chunks 0 and 1)
idx 5 : 0x00000000    A's NW, chunk id 0 relocated by +0
idx 6 : 0x00000001    A's NE, chunk id 1 relocated by +0
idx 7 : 0x7FFFFFFF    A's SW
idx 8 : 0x7FFFFFFF    A's SE
```

B contributes no further nodes, so the index is nine `uint32`s. The relocation is **two constants
per cell**: `+0` and `+2` for chunk ids, `+4` and (unused) for branch child bases — where a cell's
branch delta is `block_base − 1` for a block starting at `block_base` (A's nodes `1..5` land at
`5..9`, so `1 − 1 + 5 = 5` ✓, and any deeper branch in A relocates by the same `+4`).

The offset table is A's entries verbatim followed by B's shifted by A's total:
`[0, 1200, 2000, 3500]` for three chunks. The 3 500 bytes of chunk payload are one `memcpy` per
cell. **No feature was decoded**, because every anchor is relative to a leaf whose bbox is
bit-identical in the cell file and in the assembly.

Note the layout is breadth-first only down to the cell depth; below it, each cell's subtree is a
contiguous block. That is legal: `OBCM_Spec.md` §4's reader contract requires only that a branch's
four children be contiguous, in NW/NE/SW/SE order, and at a **higher** index than the branch, all
of which a per-cell block satisfies. An assembler MUST NOT be required to re-interleave cell
subtrees into global breadth-first order, and a consumer MUST NOT assume global breadth-first
order.

**The seam.** A road crosses the shared edge at `lon = 7602176`. Both cells cut it there and both
materialise a junction at the same integer pair (§3.4) — cell A ends its stub there, cell B begins
its stub there. At assembly the two records unify by exact coordinate, which they are eligible for
because the coordinate lies on a fine-band boundary line; their adjacency unions into one ordinary
degree-2 junction and the road is continuous. A junction 3.9 m away on either side is a
*different* junction and stays separate — which is exactly why there is no tolerance knob.

---

## 8. Where this lives

- The map byte format every cell and assembly is an instance of:
  [`OBCM_Spec.md`](OBCM_Spec.md); its code authority
  [`firmware/obc-formats/src/obcm.rs`](../firmware/obc-formats/src/obcm.rs).
- The catalog that publishes cells, bands, schemas, skins, and region cell-sets:
  [`OBCC_Spec.md`](OBCC_Spec.md).
- The packer whose quadtree, anchor, and nav conventions this specification constrains:
  [`host/obc-pack`](../host/obc-pack) — the quadtree in `quadtree.rs`, the byte layout in
  `serialize.rs`, the routable graph and its class tables in `nav.rs`.
- The reader every verify pass (§4.8) and the device itself run:
  [`firmware/obc-reader`](../firmware/obc-reader).
- The bakery that cuts and publishes cells, and the curated region list that names the
  selections: [`host/obc-bake`](../host/obc-bake).
- The conceptual tour, with diagrams: the docs site's
  [data formats](../docs/content/software/formats.md) page.
