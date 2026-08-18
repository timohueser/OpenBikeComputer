# OBCA — OpenBikeComputer Map Assemblies (v1)

OBCA is the contract that turns a **catalog of pre-baked grid cells** into the map a rider
carries. It defines four things:

1. **The cell grid** (§1) — a fixed, global, power-of-two microdegree lattice, shared by every
   band and every schema revision.
2. **The alignment theorem** (§2) — why an assembled [`OBCM`](OBCM_Spec.md) file can adopt a
   cell's quadtree subtree with its **chunk bytes copied verbatim**, and exactly what has to be
   true for that to hold.
3. **The artifacts** — what a baked **cell** is (§3) and what an **assembly** built from cells
   must do (§4), including the navigation-graph seam rules that make routing correct across cell
   boundaries by construction.
4. ~~**Volume sets** (§5)~~ — **superseded**, see the marker below and §5.

OBCA introduces **no new OBCM version and changes no OBCM semantics**. Every cell is an ordinary
[OBCM](OBCM_Spec.md) file that today's reader parses unchanged. What OBCA adds is a set of
*constraints* on how those files are produced, plus the discipline that makes assembling them cheap
enough to run in a browser.

> **§5 (volume sets) and the OBCS manifest are superseded** by [OBCM v14](OBCM_Spec.md) / issue
> #1420. A map is one OBCM object: v14's scaled offsets removed the format's 4 GiB ceiling and the
> flat store removed FAT32's, so there is nothing left for a set to work around. Do not extend §5,
> do not implement against it, and do not add a role, a record or a validation rule to it. The
> section's text is kept for history until the code that reads it is deleted; `obc-formats`'
> `obcs.rs`, the assembler's shard emitter, and the wire's `mapShard`/`mapSet`/`terrainShard` object
> types all go in **FS7.5b/c**. §4's assembly contract is **not** superseded — an assembler still
> grafts cells into one file; it simply emits that one file rather than a set of them, and splices
> the terrain raster into `OBCM_Spec.md` §1.3's region rather than shipping it beside the map.
>
> (For history: manifest **v3** was FS7 #1389 — every record carried its member's `ObjectId` so a
> set resolved through object identity rather than derived filenames, growing the record 56 → 64
> bytes. **v2** was EL4 #1072 — it added the `terrain` role and made §5.3's role and tiling rules
> count OBCM shards rather than records. Both were hard cuts under the pre-release rule; v3 shipped
> a week before the re-scope that retired the whole idea.)

This document is normative. The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are to be
interpreted as in RFC 2119.

Related contracts: [`OBCM_Spec.md`](OBCM_Spec.md) is the byte format of every cell, shard, and
assembly; [`OBCC_Spec.md`](OBCC_Spec.md) is the catalog manifest that publishes cells and names
the selectable regions; [`OBCT_Spec.md`](OBCT_Spec.md) is the terrain raster, a second artifact
class on **this same grid** (§1) with its own revision track, carried **inside** the assembled map
since OBCM v14 (`OBCM_Spec.md` §1.3) rather than beside it; [`OBCU_Spec.md`](OBCU_Spec.md) is unrelated (firmware updates) and is cited only for
the fixed-layout header conventions §5.2 follows.

## Design principles

1. **Bake once per cell, not once per selection.** N cells cover a coverage area exactly once.
   Every selection — a country, a hand-drawn box, a corridor buffered around a trip's routes,
   holes and all — is an *assembly* of those cells, so combinations cost download bandwidth and
   never storage. This is the whole reason the format exists.
2. **The grid is the format's own arithmetic.** OBCM subdivides a quadtree by integer
   floor-midpoints and stores feature anchors relative to their *leaf's* min corner
   ([`OBCM_Spec.md` §4, §5.2](OBCM_Spec.md)). Choosing power-of-two cells on a shared origin
   makes subdivision land *exactly* on cell boundaries, which is what lets an assembler copy
   chunk bytes instead of decoding them (§2). The grid is not a convention layered on the format;
   it is the format's arithmetic taken seriously.
3. **Correct at seams by construction, not by tolerance.** Every cell edge is a border, so seam
   correctness cannot rest on proximity. Cells cut routable ways at the **exact** cell edge and
   materialise a junction whose coordinate both neighbours compute identically; unification is
   **exact-coordinate equality only**. An epsilon snap is forbidden (§3.4) — measurement showed
   genuinely distinct junctions 3.9 m apart at a cell seam, so a tolerance would fuse them.
4. **Sizes are data; shape is format.** The *shape* of the grid (origin, square power-of-two
   cells, one origin for all bands) is normative here and never changes. The actual cell sizes and
   which LODs live in which band are properties of the **schema revision**, published in the
   catalog (§6). Retuning them is a re-bake, not a format bump.
5. **A cell store is lockstep.** Every cell in an assembly MUST share OBCM version *and* schema
   revision. Graft-level assembly buys its speed by trusting that chunk bytes from two different
   files mean the same thing; that is only true within one revision. The bake guard enforces it
   (§6.3), and the price is that a format or schema bump re-bakes the whole store.
6. **Nothing self-made reaches a device unverified.** A catalog artifact was verified by the
   bakery; an assembly was made on the rider's own machine, outside the manifest. §4.8 therefore
   makes a full reader-based verify a *precondition* of writing a set, not an optional extra.
7. ~~**The core file's headroom is the scarcest resource, so nothing else may spend it.**~~
   **Superseded** with §5 (OBCM v14, #1420): there is no core file and no `4 GiB − 1` margin to
   spend, because a map is one object whose interior scales to 64 GiB. What survives is the
   corollary, and it survives unchanged: every file's size is computable from the catalog before any
   download, so density growth degrades to a refusal in a builder rather than to a malformed
   artifact — it is now **one** number checked against the card's free space rather than several
   against a ceiling.

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

Three properties of this definition are load-bearing and none of them is aesthetic:

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
  (≈ ±268°) contains the whole geographic domain with room to spare.

### 1.2 Bands

A **band** is a named class of cell content with one cell size. A band's cells carry a stated
subset of the schema's LOD ladder and, optionally, the non-geometry sections. The band table is a
property of the **schema revision** (§6), not of this format; the v1 values are §1.5.

Two rules are normative here:

- **Coverage.** An assembly of a selection MUST include, per band, exactly those cells of that
  band whose square intersects the selection, and MUST NOT include any other. Nothing else is
  needed: because coarse bands use larger cells, the same rule yields *precise* coverage at fine
  bands and *generous* coverage — whole covering cells, i.e. context beyond the selection — at
  coarse ones. Generosity is a consequence of cell size, not a second rule.
- **Partition.** Every ladder LOD MUST belong to exactly one band, and the nav/POI sections to
  exactly one band. A LOD in two bands would be written twice; a LOD in none would be missing.

### 1.3 Cell identity

The canonical textual id of a cell is

```
<log2(S)>/<i>/<j>          e.g.  18/1204/1052
```

with `i` and `j` zero-padded to `max(4, decimal_width(WORLD_SIDE / S − 1))` digits — four for every
size at or above `2^16`, which is every band the v1 table uses, and wider for the smaller sizes
§1.1 still permits. Producers MUST widen rather than truncate. Catalog entries (§6) and object paths
use this id, so a cell's URL is derivable from its **band** and its id plus a base URL — the band,
because two bands may share a cell size ([`OBCC_Spec.md` §11.1](OBCC_Spec.md)) — while its
**square is derivable from its id alone**, which is why a catalog cell entry carries no bbox
([`OBCC_Spec.md` §11.6](OBCC_Spec.md)).

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
  `−179.999…°`. A selection crossing the antimeridian is two assemblies. (DACH, the v1 coverage,
  is nowhere near either edge; the rule exists so the behaviour is defined rather than discovered.)
- **The poles are not special.** The grid rows nearest ±90° overhang the pole; no routable way
  reaches the overhang, so no boundary junction (§3.4) can be produced there.

### 1.5 The v1 band table (schema `bikepacking`, revision 1)

These values are the measured recommendation of epic #1016 D1, taken over whole-extract bakes of
`switzerland`, `austria`, and `freiburg-regbez` with the then-shipped bikepacking ladder
([`builder/presets/schema.json`](../builder/presets/schema.json)). They are **schema data**: a
catalog states them (§6), a producer reads them from the catalog, and retuning them is a re-bake
rather than a change to this document.

| Band | Cell size | ≈ at 47°N | Carries | Assembly role (§5.1 — superseded, see §5) |
| :-- | :-- | :-- | :-- | :-- |
| `coarse` | `2^20` µdeg (1.048576°) | 117 × 80 km | ladder LOD 0, 1, 2, 3, 4 | **coarse shard** |
| `mid` | `2^19` µdeg (0.524288°) | 58 × 40 km | ladder LOD 5, 6, 7, 8 (semantic through 20 m/px) | geometry shard |
| `fine` | `2^18` µdeg (0.262144°) | 29 × 20 km | ladder LOD 9, 10, 11, 12, 13 (ordinary geometry from 16 m/px) | geometry shard |
| `network` | `2^18` µdeg (0.262144°) | 29 × 20 km | nav graph (OBCM §8), POIs (OBCM §7), hours pool (OBCM §7.5) | **core file** |

The largest cell size in the table, `S_MAX = 2^20`, is the assembly bbox's alignment modulus
(§2.1).

The current 14-rung ladder keeps its semantic overview tiers through 20 m/px in `mid`, and puts the
ordinary 16 m/px-and-closer geometry in `fine`. The measured densities below predate that expanded
ladder and semantic generalisation; they remain historical sizing evidence rather than current
byte-exact forecasts.

**Measured density**, in MiB per 1000 km² of covered ground — a latitude-free unit, unlike bytes
per square degree. Three whole-extract bakes at this schema: `switzerland` (41 285 km²),
`austria` (83 879 km²), `freiburg-regbez` (9 357 km², the densest, an upper bound rather than a
German average — the Rhine plain carries more road and more building than the Alps do):

| Band | switzerland | austria | freiburg-regbez | one cell: fully covered / CH p90 / CH max |
| :-- | --: | --: | --: | :-- |
| `coarse` — LOD 0–4 | 0.45 | 0.54 | 0.65 | 4.2 / 2.9 / 3.9 MiB |
| `mid` — LOD 5,6 | 2.32 | 2.34 | 3.05 | 5.4 / 5.9 / 9.5 MiB |
| `fine` — LOD 7,8 | 7.66 | 6.73 | 9.54 | 4.4 / 5.5 / 12.1 MiB |
| `network` — nav + POI | 6.30 | 3.80 | 7.06 | 3.7 / 5.3 / 19.5 MiB |
| **whole map** | **16.7** | **13.4** | **20.3** | |

> **Why these boundaries.** ⚠️ **Half of this justification is superseded** (OBCM v14, #1420): the
> `4 GiB − 1` file ceiling every "must fit one file" clause below appeals to is gone, and with it
> the core-versus-shard split those clauses were protecting. **What survives is fetch-unit
> uniformity**, which is about the size of one *download*, not one file — that is the property the
> band boundaries are actually set by, and it is unaffected. Read the rest as the reasoning that
> produced the v1 table rather than as a live constraint; the table itself stands, because the sizes
> it names are still the sizes.
>
> **No ladder LOD lives in the core file.** The core carries the nav
> graph, the POIs and the style table and nothing else (§5.1), because it is the one file of a set
> that cannot be split by bbox — so every byte that *can* scale horizontally is kept out of it. The
> band boundaries are therefore set by two properties that really are about geometry: the size of
> one fetch, and the size of the single coarse shard.
>
> **Fetch-unit uniformity.** Cell size scales inversely with band density, so a fully covered cell
> of every band is 3.7–5.4 MiB and one fetch costs about the same wherever it comes from. That is
> what keeps the 30 m/px tier (LOD 5) in `mid`: a `2^20` cell carrying it as well measures
> **10.4–11.7 MiB** fully covered, two and a half times every other band's object. The boundary is
> not a knife edge from the other side — moving LOD 4 down into `mid` would add only 1.1–1.4 MiB to
> a `2^19` cell — so `coarse` ending where it does is the cheap end of a shallow optimum, chosen
> rather than forced.
>
> **The coarse shard is one file spanning the whole assembly** (§5.1), which is what keeps a
> zoomed-out viewport a single-file read. Its band's content must therefore stay small enough that
> even a *continental* assembly's coarse layer fits `4 GiB − 1`: at 0.47–0.61 MiB per 1000 km²
> (DACH-weighted, and sparser ground is cheaper) that is **≈ 6.7–8.8 M km²** — EU-27 would be a
> 1.9–2.5 GiB single coarse
> shard, geographic Europe 4.6–6.1 GiB and would have to split. Adding LOD 3 roughly doubles the band
> and pulls that ceiling down to ≈ 2.9–3.6 M km², *below* EU-27, giving up the single-file zoomed-out
> read exactly where it matters most.
>
> **Why the fine band is `2^18` rather than `2^19`:** the worst measured single cell is then ~12 MiB
> of geometry (Zürich) instead of the ~33 MiB a `2^19` fine band produces. Note the p90 and max
> columns: per-cell bytes run 30–45 % above the average in populated bands and 4–5× above it in the
> worst urban cell, so a shard planner MUST size on the distribution, never on the mean. (The
> *fully covered* column is `cell area × density`; the p90 and max columns are the measured per-cell
> distribution over Switzerland, which includes partially covered cells and so sits below it at the
> coarse sizes, where few whole cells exist.)
>
> These densities are measured on **whole-extract bakes**, not on cells, and a producer SHOULD still
> budget **+5–15 %** over the figures above. That margin is deliberate slack rather than an expected
> cost: P2 measured a real scoped bake (`freiburg-regbez + switzerland`, 314 cells) at **0–4 %
> *smaller*** than these figures, band for band. The cutter runs the packer's `merge_fills` /
> `merge_lines` passes once over the whole ingest and cuts the *merged* set, so the cross-cell union
> loss this margin was first attributed to never happens; the ~3 KB of fixed per-cell overhead is
> ~0.1 % at country scale; and the vertices clipping adds at a cell edge are outweighed by the
> sub-pixel cull running on *clipped* geometry. The margin stays because §5.7 requires the
> pre-download projection to be an **upper bound** on real cell bytes, and a budget that is never
> exceeded is doing its job.

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

### 2.2 Proof

By induction on depth, in exact integer arithmetic.

*Span.* The root spans `2^n` on both axes. A node spanning `2^k` from `m` has
`min + max = 2m + 2^k`, an even integer, so the floor-division midpoint
`mid = (min + max).div_euclid(2)` equals `m + 2^(k−1)` **exactly** — `div_euclid` never rounds
here, and the sign of `m` is irrelevant because the dividend is even. Each of the four children
(`OBCM_Spec.md` §4) therefore spans exactly `2^(k−1)` on both axes. So a depth-`d` node spans
`2^(n−d)`.

*Position.* Child minima are drawn from `{m, m + 2^(k−1)}` per axis, so by induction every
depth-`d` node has minimum `A + j·2^(n−d)` for some integer `0 ≤ j < 2^d`, per axis.

*Cells.* At `d = n − s` the span is `2^s = S` and the minima are `A + j·S`. Since `2^s` divides
`S_MAX` (every band size is a power of two no greater than `S_MAX`) and `A ≡ GRID_ORIGIN
(mod S_MAX)`, we get `A ≡ GRID_ORIGIN (mod S)`, hence `A + j·S ≡ GRID_ORIGIN (mod S)`. A square of
side `S` whose minimum is congruent to `GRID_ORIGIN` modulo `S` *is* a grid cell of size `S`
(§1.1). Finally `2^n` is a multiple of `S`, so the depth-`d` nodes tile the assembly bbox exactly,
with no partial row or column. ∎

Two corollaries worth stating because implementations lean on them:

- `n ≥ s` for every band, so the cell depth is never negative — guaranteed by `2^n ≥ S_MAX`.
- The four children are emitted **NW, NE, SW, SE**, and the assembler writes its own upper tree
  (depths `0 .. d`) breadth-first, so the cell → depth-`d` slot mapping is a pure function of
  `(i, j)` and needs no search. Below the cell depth the layout is per-cell blocks rather than
  global breadth-first order, which is legal — see §7.

### 2.3 What the theorem buys

A cell artifact's own header bbox **is** its cell (§3.1), so the cell file's root node is that
cell and its subtree is the same subdivision the assembly performs at that position — identical
node bboxes at every level, to the microdegree. Since a feature's anchor is stored relative to its
**leaf's** minimum corner (`OBCM_Spec.md` §5.2) and its deltas are relative to the previous
vertex, *every byte of a chunk decodes to the same absolute geometry in the cell file and in the
assembly*. Therefore:

- **Chunk payload bytes are copied verbatim.** No decode, no re-encode, no simplification, no
  GEOS. This is what makes assembly a streaming concatenation that runs in wasm.
- **Index nodes are relocated, not rebuilt.** A copied subtree needs exactly two constants added:
  one to its leaf values (the cell's chunk-id base) and one to its branch child bases (§4.3, §7) —
  integer arithmetic on `uint32`s, no geometry involved.
- **Offset-table entries are copied with one constant added** (the cell's base within the LOD's
  concatenated chunk region).

### 2.4 What the theorem does *not* buy

The theorem is about the geometry quadtrees and nothing else. It says nothing about, and the
assembler MUST fully rebuild (§4.4–§4.7):

- the POI section and hours pool — POI coordinates are **absolute** (`OBCM_Spec.md` §7.3), so the
  per-category trees must be re-binned and the pool re-deduplicated with `HoursRef` remapped;
- the navigation graph — node ids are file-local and dense, and `Edge Id` is a **pool byte
  offset** (`OBCM_Spec.md` §8.4), so nothing in §8 survives concatenation;
- the header, the style table, the LOD table, and every section offset.

It also does not make the result *pretty* for free. Two cosmetic costs are inherent to cutting at
cell boundaries and are accepted (epic #1016 §7): dash phase restarts at a cell edge, and the
packer's `merge_fills` / `merge_lines` unions cannot cross a cell boundary, so an assembly carries
slightly more features than a single-shot bake of the same area for the same pixels.

---

## 3. Cell artifacts

### 3.1 A cell is an ordinary OBCM file

A baked cell MUST be a complete, valid OBCM file of the catalog's OBCM version, and:

- its **header bbox MUST be exactly its grid cell** (§1.1) — not the content-derived box a normal
  pack computes. This is the one place where the packer's usual "the bbox is what the content
  covers" rule is deliberately inverted, because §2 needs the box to be the grid square;
- it MUST write the **complete ladder** in its LOD table — one entry per schema LOD, in ladder
  order, with each entry's `Max Meters/Pixel` taken from the schema (so LOD 0 is `+inf` and the
  sequence is strictly decreasing, exactly as `OBCM_Spec.md` §3 requires). LODs outside the cell's
  band are written **empty**: `Index Node Count = 0`, `Chunk Count = 0`, and the single-`0`-entry
  offset table `OBCM_Spec.md` §5.1 mandates for an empty region. A reader walks an empty LOD and
  finds nothing, so a cell of any band stays a legal, openable map;
- the POI section and the nav section MUST be present, per `OBCM_Spec.md` §7/§8, and MUST be
  **empty** unless the cell's band carries them;
- its style table MUST be the schema revision's **canonical table** (§6.2) — right ids, right
  count, right order, placeholder values — and its `Marker Color` the schema's placeholder. Both
  are replaced at assembly by the chosen skin.

The consequence of writing the full ladder is worth stating plainly: **band membership is not
recorded in the cell's bytes**. It is a property of the schema revision, read from the catalog
(§6). A producer MUST NOT infer a cell's band from which of its LODs happen to be non-empty (a
legitimately empty cell — open sea — is indistinguishable that way).

### 3.2 Determinism

> **Same source snapshot + same schema revision + same cell ⇒ byte-identical file.**

This is a hard requirement, not an aspiration: it is what lets the catalog content-address cells,
lets a re-bake be a no-op, and lets two independently baked neighbours agree on a seam coordinate
(§3.4). Producers MUST therefore ensure that:

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
  polygon may survive in one cell and be culled in its neighbour. That is acceptable for fills and
  is why the packer never culls lines.

Producers MUST NOT extend a cell's geometry beyond its square "for continuity". Overlap would be
written twice and drawn twice.

### 3.4 Cutting the navigation graph: deterministic boundary junctions

This is the part that has to be right by construction, because every cell edge is a border and a
cell seam is *thin* — measured, a `2^20` seam of ~78 km carried only 823 naturally coincident
junctions, where a country border band carried 21 144 across its 1–3 km overhang. Naturally
coincident junctions are therefore not enough, and proximity is worse than nothing (see the
epsilon rule below).

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
Interior synthetic nodes are minted by the packer's own edge splits at a midpoint index of the
edge *as that run sees it* (`nav.rs::split_edge`, plus the serializer's `OBCM_Spec.md` §8.4 chunk-fit and span
splits), so two runs over a different set of ways place them metres apart.

**Exact-coordinate unification only — an epsilon snap is forbidden.** At a cell seam, genuinely
*different* junctions sit as close as **3.9 m** (measured: 3 pairs within 10 m and 366 within
100 m across one `2^20` seam). A tolerance of any size large enough to be useful would fuse
distinct nodes and invent turns. Across independently packed sources the matching regime is
all-or-nothing — 21 144 exact matches and **zero** near-misses at 1 m / 10 m / 100 m — because the
whole §8 path is integer and deterministic. Producers and assemblers MUST use exact integer
equality and MUST NOT offer a tolerance knob.

**Wire limits survive.** Unifying two junctions unions their adjacency and recomputes nothing, so
`OBCM_Spec.md` §8.3's degree cap of 24, its `int16` neighbour deltas, and its `uint16` `Cost M`
all hold: a measured 3.39 M-node merge reached a maximum degree of 10 and a maximum neighbour
delta of 31 967 µdeg (the packer's own 32 000 split bound showing through). Boundary junctions are
ordinary degree-2 nodes. An assembler MUST nevertheless re-check the cap (§4.8) rather than assume
it.

### 3.5 Island pruning at bake time: strictly interior only

A hard cut severs the road network at a line, so a fragment can hold fewer than the schema's
`min_component_edges` in *each* of two cells while being a perfectly good road once assembled. If
both neighbours drop their half, no assembly-time work can recover bytes that were never written.

Therefore:

- A cell bake MUST prune only components that are **strictly interior** to the cell — no node of
  the component lies on the cell boundary — and MUST NOT prune any component touching the
  boundary, however small.
- The real pruning pass runs at **assembly** time (§4.6), over the merged graph, where component
  sizes are finally true.
- `min_component_edges` is a property of the **schema revision**, never of the skin. Two cells
  pruned at different thresholds do not assemble into a graph with consistent semantics.

This is a decision taken on the mechanism, not on a measured failure: in every configuration
measurable with today's relation-complete cropping, bake-time pruning destroyed nothing unification
wanted (merge-rescued components = 0 in all runs, including an adjacent-cell pair). The cost of
getting it wrong is small but real, and the assembler already renumbers nodes and rewrites the
edge pool, so a union-find on top is nearly free.

### 3.6 POIs and hours

POIs are points, so cell assignment is unambiguous: a POI belongs to the **one** cell whose
half-open square contains its coordinate. A `network`-band cell MUST write every POI in its square
and no other, and MUST carry its own deduplicated hours pool with `HoursRef` values local to the
cell. Both are rebuilt at assembly (§4.5), so per-cell pools are allowed to be locally optimal and
globally redundant.

### 3.7 Provenance and partial cells (D3)

A cell baked from a regional extract is **not** the cell a covering source would produce. Measured
in the double-covered band of two neighbouring extracts, only ~50 % of each file's junctions exist
in the other, because each extract lacks the side roads that create the neighbour's junctions. So:

- Every cell MUST record its **source extent**: the identifier of each source extract it was baked
  from, and that extract's snapshot date.
- A cell whose sources do **not** fully cover its square is **`partial`**. Coverage is decided
  against the sources' own coverage geometry (for Geofabrik-style extracts, the region polygon
  plus its complete-ways overhang), not against the packed content's bbox — content can be
  legitimately empty.
- A catalog MUST mark a partial cell as such (§6.1), and a consumer MUST NOT present a partial
  cell as canonical coverage. The builder shows the affected area as a warning inside the
  selection rather than as covered ground.
- A bakery MUST replace a partial cell when a covering source becomes available, and MUST NOT
  publish a canonical cell and a partial cell for the same (cell, schema revision) pair. Co-baking
  a border cell from every extract that touches it is the sanctioned way to make it canonical
  without a planet source.

### 3.8 Known-empty coverage

A covering source can prove that a band's canonical payload for a cell is
empty. Publishing a complete empty OBCM for every such square would turn oceans
and other sparse planet coverage into millions of fixed-overhead objects, so
OBCC may represent those cells as compact **known-empty** row ranges instead.

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

An **assembly** is **one** OBCM file built from catalog cells for one selection and one skin,
carrying its terrain raster in `OBCM_Spec.md` §1.3's region when the selection has elevation. This
section defines what an assembler does. (It used to say "or a volume set of them"; §5 is superseded
by OBCM v14, and the emitter that splits an assembly into several files goes in FS7.5b.)

### 4.1 Inputs and preconditions

An assembler takes a selection (any set of areas, holes allowed), a schema revision with its band
table, a skin, the artifacts the coverage rule (§1.2) selects, and any selected
known-empty identities (§3.8). It MUST refuse to proceed if:

- the cells do not all carry the same OBCM version, or that version is not the one it writes; or
- the cells do not all belong to the same schema revision; or
- two cells disagree on the style table's id set or count, or on the `OBCM_Spec.md` §8.6 profile table; or
- any selected cell is missing, and the caller has not accepted the resulting hole; or
- any selected cell is `partial`, and the caller has not accepted the reduced coverage.

Missing cells are legal and produce **empty leaves**; the renderer already paints backdrop there,
so a selection with holes is well-formed by construction. Known-empty cells
also produce empty leaves but are explicit canonical coverage, not holes. What
is not legal is a *silent* hole.

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
| Geometry **chunk payload bytes** | **Verbatim.** Copied byte-for-byte (§2.3). |
| Geometry **quadtree subtrees** at and below the cell depth | Copied with **two constants per cell**: leaf values `+ chunk_id_base`, branch child bases `+ (block_base − 1)` where the cell's nodes `1..` land at assembly index `block_base` (§7). Empty-leaf sentinels and the branch bit are preserved. |
| Geometry **offset tables** | Copied with `+ chunk_byte_base` per cell; the assembler writes the `Chunk Count + 1` entries for the concatenated region. |
| Geometry **index nodes above the cell depth** | Rebuilt: a fresh tree over the assembly bbox down to the cell depth, with a branch wherever any descendant cell is present and an empty leaf where none is. |
| **LOD table** | Rebuilt (new offsets and counts; `Max Meters/Pixel` and `Chunk Size` from the schema). |
| **Header** | Rebuilt (bbox, section offsets, `Marker Color` from the skin). |
| **Style table** | Rebuilt from the skin (§4.7) — same ids, same count, same order; values replaced. |
| **POI section + hours pool** | Rebuilt (§4.5). |
| **Nav section** (directory, node tree, node chunks, edge pool) | Rebuilt (§4.6). |
| **Profile table** (`OBCM_Spec.md` §8.6) | Copied from the cells after checking every cell agrees; it is schema data. |
| **Terrain cell blocks** ([`OBCT_Spec.md`](OBCT_Spec.md) §3.2) | **Verbatim**, into a fresh directory over the assembly rectangle (§5.1's `terrain` role). Terrain assembly is *placement*, not grafting: the lattice is global and half-open, so two neighbouring cells already agree about every sample and there is nothing to relocate, re-index or unify. |

An assembler MUST NOT decode a geometry chunk in the normal path. Decoding is for verification
(§4.8), where it is the point.

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

1. Collect every POI record from every `network`-band cell. Records are 36 bytes with **absolute**
   coordinates, so they need no relocation.
2. Deduplicate by `(lat, lon, subtype)`. Duplicates are possible only through operator error
   (§3.6 gives each POI exactly one cell), so a duplicate is dropped and SHOULD be reported.
3. Rebuild the hours pool: collect each source blob, deduplicate the 29-byte blobs, and remap every
   record's `HoursRef` to the new index. `0xFFFF` stays `0xFFFF`.
4. Re-bin each category into a fresh quadtree over the **assembly** bbox and re-chunk at the
   directory's shared `Chunk Size`, per `OBCM_Spec.md` §7.1–§7.3.
5. Order records within a chunk by `(lat, lon, subtype)` so the output is deterministic.

The pool count MUST not exceed `0xFFFE` distinct blobs, because `HoursRef` is a `uint16` with
`0xFFFF` reserved. Measured, a whole country needs a few thousand; an assembler MUST nevertheless
fail loudly rather than wrap.

### 4.6 Merging the navigation graph

This is the most involved rebuild, and its order matters.

1. **Read the serialized node set.** Walk each `network` cell's `OBCM_Spec.md` §8 node quadtree through a real
   reader and collect junction records, keyed by `Node Id` (`OBCM_Spec.md` §8.2's bin-packing
   means one leaf walk can yield a record more than once, so the collection MUST be idempotent).
   The set to renumber is the **serialized** one, not the one a graph builder would produce: the
   serializer mints further synthetic degree-2 junctions after the builder finishes (measured
   +4 489 nodes and +2 957 edges on a country bake), and they are in the bytes.
2. **Unify seam nodes, and only seam nodes.** Two records unify iff their coordinates are
   **exactly** equal *and* the coordinate lies on a boundary line of the `network` band's grid
   (a latitude or longitude congruent to `GRID_ORIGIN` modulo that band's cell size). Unification
   unions their adjacency. Restricting to boundary lines is not an optimisation: whole-map
   coordinate keying would also fuse the handful of *interior* coordinate collisions that exist in
   a single file — vertically stacked bridge/tunnel junctions, measured at 9 in one regional bake
   and 28 in a country bake — inventing a turn between a bridge and the road beneath it.
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

An assembler MUST NOT create an edge between two nodes that no single cell joined. Unification
only ever joins *through* a coincident junction, so this is a checkable invariant rather than a
hope: a merged route that steps between two nodes sharing no source cell is a bug (measured zero
such steps over three cross-source routes).

### 4.7 Stamping the skin

A **skin** is the presentation half of a preset: per feature type a color, weight, dash bit,
`color2`, z-index and priority, plus the map's `Marker Color`. Stamping is:

- resolve each feature type's style **id** from the schema revision's canonical assignment (§6.2)
  — the skin MUST NOT introduce, remove, reorder, or renumber ids;
- write the style table with the schema's ids in the schema's order and the skin's values in the
  other seven bytes of each 8-byte record (`OBCM_Spec.md` §2);
- write the skin's `Marker Color` into the header.

That is the entire cost of a restyle: ~2 KB of the output. It is why the builder can offer a style
editor with no re-bake and no server. An assembler MUST reject a skin that does not cover every id
in the schema's table, and MUST reject one that names a feature type the schema does not have —
silently defaulting a missing style would ship a map with an invisible layer.

### 4.8 Verify obligations

An assembly is self-made and outside the catalog's guarantees, so it MUST be verified before it is
written to a device. The verify runs through the **real reader** — the same crate the firmware
uses — and MUST cover, per file of the set:

1. **Parse.** Header (magic, version, bbox), style table, LOD table, POI directory, nav directory
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
6. ~~**Set invariants** (§5)~~ — **superseded** with §5 (OBCM v14, #1420). One file has no roles
   to check and no tiling to verify; what replaces this step is the header's own `Offset Scale`
   covering the file's length (`OBCM_Spec.md` §1.1) and the file's size equalling what the
   pre-download projection said it would be. FS7.5b writes the replacement rule; until then this
   item describes an emitter that is on its way out.
7. **Digests.** SHA-256 of the assembled file. (It was per file, recorded in the §5.2 manifest;
   with one file there is one digest and nowhere in-band to record it — the catalog and the
   transfer's own CRC carry it instead.)
8. **The terrain region** — the raster, spliced into `OBCM_Spec.md` §1.3 rather than shipped as
   §5.1's `terrain` shard. Every input is checked
   **before its bytes are copied**, because a bad cell must not reach the shard even to be caught on
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
it is an unmountable one, which is the correct outcome. (That used to rest on §5.4's manifest-last
trick; it now rests on the flat store's commit, which is the atomicity the trick was faking —
`FLAT_Store_Format.md` §5.)

---

## 5. Volume sets

> **Superseded** by [OBCM v14](OBCM_Spec.md) / issue #1420. A map is one OBCM object; there are no
> shards, no roles, no manifest and no set. Nothing in this section is normative any more, and
> nothing in it may be extended. It is kept until the code that reads it is deleted in FS7.5b/c:
> `obc-formats/src/obcs.rs`, `obcm-assemble`'s shard emitter, the board's set mount, the builder's
> `parseSetManifest`, and the `mapShard` / `mapSet` / `terrainShard` object types of the wire
> contract.
>
> **What replaced each part.** The two 4 GiB ceilings §5 opens with are both gone: OBCM v14's scaled
> offsets (`OBCM_Spec.md` §1.1) address 64 GiB of interior, and the flat store
> (`FLAT_Store_Format.md`) is not FAT32 and has no file-size limit of its own. §5.1's roles and
> tiling become one file. §5.2's manifest becomes the OBCM header. §5.3's validation becomes
> OBCM's own header validation. §5.4's manifest-last atomicity becomes the flat store's commit,
> which is the atomicity that trick was faking. §5.5's single-file fast path becomes the only path.
> §5.6's empty-LOD cache is unnecessary when there is one file to dispatch to. §5.7's per-file size
> projection survives in substance — a consumer still computes a selection's bytes from the catalog
> before downloading — but it projects **one** number against the card's free space rather than
> several against a ceiling, and the "refuse a selection whose core exceeds 4 GiB" rule goes with the
> ceiling it names. The `terrain` role becomes `OBCM_Spec.md` §1.3's embedded region.

One *logical* map is a **set**: a small manifest plus 1..N physical OBCM files. This is not an
optimisation — two independent 4 GiB ceilings make it necessary, and the headline selection
(Germany + Austria + Switzerland, projected at **7.6–8.9 GiB**, of which Germany alone is
5.8–7.1 GiB) is past both:

1. **FAT32** caps one file at `4 GiB − 1 B`, and FAT32 is the card format the firmware's FAT stack
   reads.
2. **OBCM itself** is `uint32` everywhere that matters — header section offsets, per-LOD chunk
   offset tables, and `Edge Id` as a pool byte offset — so a single `.obcm` cannot address past
   4 GiB on any filesystem.

Sets are therefore the shape from day one, and a small map is a set of **one** (§5.5), so the
common case costs one extra small file.

### 5.1 Roles

Every shard is an ordinary OBCM file whose header bbox is a grid-aligned power-of-two square
(§2.1) — a node of the assembly quadtree — and whose LOD table lists the **full ladder**, with the
LODs it does not carry written empty (§3.1).

Four roles are defined. Three of them are OBCM shards and obey one ordering principle, stated once
and then obeyed everywhere: **the core file holds only what cannot be split by bbox, and everything
that can be is moved out of it.** The fourth is the raster, which is not an OBCM file at all.

- **The core shard** (exactly one per set) carries the style table, the `Marker Color`, the single
  unified **nav graph**, and the **POIs** with their hours pool — the sections of a band whose
  schema `role` is `core`, the `network` band at the v1 table. It carries **no ladder LOD at all**
  (every LOD region is written empty, §3.1), except in the single-file fast path (§5.5), where the
  one shard is the core and carries everything. Its bbox is the whole assembly bbox.
- **Coarse shards** carry the LODs of the band whose schema `role` is `coarse` — LOD 0–4 at the
  v1 table — and nothing else: empty nav directory, empty POI categories, every other LOD empty.
  There is **exactly one** by default, its bbox the whole assembly bbox, because a zoomed-out
  viewport covers the whole map and should be a single-file read. It MAY be split by bbox in the
  ordinary way (below) if a continental selection ever brings it near the ceiling; §1.5 puts that at
  ≈ 6.7–8.8 M km², about the size of geographic Europe.
- **Geometry shards** carry the `mid`- and `fine`-band LODs and nothing else: empty nav directory,
  empty POI categories. There are as many as the target shard size needs, and none at all only in
  the single-file fast path.
- **The terrain shard** (at most one per set) is an [OBCT](OBCT_Spec.md) container, not an OBCM
  file: the elevation raster for the whole assembly, as a fresh offset directory over the assembly
  rectangle with each selected cell's block copied verbatim (§4.3). Its cell rectangle **is** the
  assembly bbox — a terrain cell no larger than the assembly square tiles it exactly, because the
  assembly corner is congruent to `GRID_ORIGIN` modulo `S_MAX` and therefore modulo the terrain cell
  too. An assembler MUST refuse a terrain `cell_log2` larger than the assembly's `span_log2` rather
  than overhang the box or grow it (§4.2 forbids growing it). Squares the selection covers but the
  catalog does not publish an object for — canonically void ocean (`OBCC_Spec.md` §13.6), or ground
  outside the dataset — are directory `0`, which `OBCT_Spec.md` §4.3 makes indistinguishable from an
  all-`NODATA` block. That is the whole reason terrain costs four bytes per uncovered square rather
  than a cell block.

  **Terrain is always its own file, and in v1 there is exactly one of it.** Not because it could not
  be split — an OBCT container is a rectangle and splits by bbox as easily as geometry does — but
  because it does not need to be at any scale v1 supports: a DACH-shaped raster is ≈ 430 MiB against
  a `4 GiB − 1` ceiling, an order of magnitude of headroom, and a second terrain shard would buy a
  file-count problem in exchange for nothing. An assembler MUST fail rather than emit an over-size
  terrain shard; splitting the raster by bbox is the specific future change that would lift that,
  and it touches only this role.

Coarse and geometry shards are split the same way. The shards of one role **tile** the assembly
bbox: their bboxes are an antichain of assembly-quadtree nodes whose squares are pairwise disjoint
and whose union is the assembly bbox. An assembler produces them by recursive quadtree splitting
wherever a node's bytes exceed the target shard size, so balancing needs no new geometry — only the
theorem (§2). With one shard the antichain is the root, i.e. the assembly bbox itself.

> **Why the coarse band is a shard and not part of the core.** The core is the single component of a
> set that **cannot scale horizontally**: it holds one unified nav graph, and until the router learns
> sharded graphs (below) that graph is one file. Its headroom under `4 GiB − 1` is therefore the
> scarcest resource in the whole design, and it MUST NOT be spent on bytes that have somewhere else
> to go. Coarse geometry has somewhere else to go — it tiles, it splits, it is ordinary cell content
> — so it goes there. At DACH this moves 225–296 MiB out of the core (§5.7). The property that
> motivated putting coarse in the core in the first place survives intact: a zoomed-out viewport
> still touches **exactly one** file, because the coarse shard spans the whole assembly.

At the v1 schema and DACH densities (§1.5), the shape of the largest set v1 supports is:

| DACH (482 760 km²), v1 schema | bytes | files |
| :-- | --: | :-- |
| core — nav + POI + style table | **2.8–3.0 GiB** | 1 |
| coarse shard — LOD 0–4 | 225–296 MiB | 1 |
| geometry shards — LOD 5–8 | 4.6–5.5 GiB | ~6 at a ~1 GiB target |
| **the set** | **7.6–8.9 GiB** | **~8** |

Two consequences shape the firmware work (P3b) and are worth stating as contract:

- **Routing never crosses a file.** The nav graph is whole, in one file, under 4 GiB. A\* logic is
  untouched. Nav and POI queries always go to the core shard.
- **Viewport dispatch needs no role logic.** A viewport query goes to every shard whose bbox
  intersects it; a shard that does not carry the requested LOD has an empty index for it and
  contributes nothing. §5.6 makes that free rather than merely correct.

> **The ceiling this leaves.** The core is now nav plus POIs and nothing else: they run 3.8–7.1 MiB
> per 1000 km² (§1.5), 5.9–6.4 area-weighted over DACH, so a DACH core is **2.8–3.0 GiB** and the
> nav graph alone reaches `4 GiB − 1` at roughly **640–700 thousand km²** — about 1.3–1.45× DACH,
> enough for DACH plus its northern and eastern neighbours and not enough for DACH plus France. One
> logical map is therefore capped at that scale, and the cap is now a statement about the **nav graph
> alone**: no geometry decision can move it. Going
> past it needs a **sharded nav graph** with cross-file boundary nodes — the same unification trick
> applied at query time — which is deliberately out of v1 scope, because it is the one change that
> would touch the router.

### 5.2 The set manifest

The manifest is parsed on the device, so it is fixed-layout, little-endian, and needs no
allocation — the `OBCU_Spec.md` §1.1 conventions. It is `72 + 64 × Shard Count` bytes.

**Header (72 bytes)**

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Magic | 4 | `char[4]` | Must be `b"OBCS"` |
| 4 | Version | 1 | `uint8` | `0x03` (readers reject any other value) |
| 5 | OBCM Version | 1 | `uint8` | The OBCM version of every **OBCM** shard, e.g. `0x0C` |
| 6 | Shard Count | 1 | `uint8` | `1..=32`; readers reject `0` or `> 32`. Counts **every** record, the terrain one included |
| 7 | Core Shard | 1 | `uint8` | Index of the core shard (§5.1); `< Shard Count`, and it MUST name an OBCM record |
| 8 | Schema Revision | 4 | `uint32` | The schema revision every cell was baked at (§6.3) |
| 12 | Flags | 4 | `uint32` | Reserved; written `0`, readers MUST reject a non-zero value |
| 16 | Min Lat | 4 | `int32` | Assembly bbox (§4.2), microdegrees — **lat, lon, lat, lon**, the OBCM header order |
| 20 | Min Lon | 4 | `int32` | |
| 24 | Max Lat | 4 | `int32` | |
| 28 | Max Lon | 4 | `int32` | |
| 32 | Set Id | 16 | `uint8[16]` | First 16 bytes of SHA-256 over the shard digests concatenated in index order |
| 48 | Name | 24 | `char[24]` | Display name; pre-folded printable ASCII, `0xFF`-padded (the `OBCM_Spec.md` §7.3 name convention) |

**Shard record (64 bytes each, `Shard Count` of them, starting at offset 72)**

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Role | 1 | `uint8` | `0` = core, `1` = geometry, `2` = coarse, `3` = terrain (§5.1); readers reject any other value |
| 1 | Flags | 1 | `uint8` | Reserved; `0` |
| 2 | Reserved | 2 | — | `0` |
| 4 | Min Lat | 4 | `int32` | This shard's OBCM header bbox, verbatim |
| 8 | Min Lon | 4 | `int32` | |
| 12 | Max Lat | 4 | `int32` | |
| 16 | Max Lon | 4 | `int32` | |
| 20 | Bytes | 4 | `uint32` | Shard size in bytes. **This field counts bytes, not units**, so it did not widen with v14's offsets and did not widen with the read seam either — it is now the **narrowest** wall a member of a set has to clear, and the only reason a country-scale selection still splits (§5.5, §5.7). A producer MUST refuse a shard it cannot record here rather than truncating: a 5 GiB file written as ≈0.7 GiB is worse than a refusal, because every consumer that sizes a download from the manifest then trusts a number the file contradicts. It is deliberately **not** widened: this whole section is superseded, and a field that dies with the manifest is not worth a format bump — a selection that needs more than `4 GiB − 1` in one file takes §5.5's single-file path, which has no manifest record at all |
| 24 | SHA-256 | 32 | `uint8[32]` | Digest of the shard's bytes |
| 56 | Member Id | 8 | `uint64` | This member's `ObjectId` (`FLAT_Store_Format.md` §3), or `0` while the manifest is **unbound** — see below |

**Members are named by identity.** Every record — the `terrain` one included — carries the
`ObjectId` of the object that holds its bytes. That is what a reader resolves a set through: it opens
`Shard Count` ids, not `Shard Count` filenames it computed from a card id and an ordinal. The v3 field
was **appended** rather than fitted into the three reserved bytes (eight will not fit in three) and
appended rather than inserted, so every field v2 defined keeps the offset it had; the whole byte-level
diff between the two versions is "eight more bytes at the end of each record". 64 over a 72-byte header
also puts every member id on an 8-byte boundary.

**Bound and unbound.** An id is minted by the store on the card a set is sent to, and an assembler has
never spoken to one — it may write a set that is sent to several cards, or to none. So a manifest has
exactly two legal states:

- **Unbound** — every member id is `0`, the reserved id that names no object. This is what an
  assembler MUST write. It is a complete, §5.3-valid manifest; it simply names no objects yet.
- **Bound** — every member id is non-zero, and no two records carry the same one. A client reaches
  this by committing each member, learning the id the store assigned it, and writing that id into
  the manifest bytes it is still assembling — which §5.4 already made the last write of a set.

**Binding MUST complete before the manifest is committed, and a committed manifest MUST NOT be
patched.** Binding is an edit of a *staging* buffer — bytes the client holds, that no reader can
reach — and it MUST stay one. This is the one rule in §5 that cannot be enforced by a validator, and
that is exactly why it is normative: an interrupted 8-byte id write leaves a value that is neither
`0` nor a duplicate, and **no validation rule can distinguish it from a correctly bound id**. Such a
manifest passes §5.3, reads as bound, and resolves a member to an `ObjectId` that names either
nothing or the wrong object. Contrast §5.4's magic-last-write, whose torn shape *is* recognisable —
all zeros, or a strict prefix of `OBCS` — which is what makes that trick safe and this one not.

A **half-bound** manifest — some ids named, some `0` — MUST be rejected (§5.3). It is the shape a
client that died mid-binding leaves, and it is the dangerous one only in the sense that it looks
resolvable: a reader that trusted it would open the members that were patched and silently lose the
rest. **A half-bound manifest means the set never existed.** A reader MUST treat it exactly as
§5.4 treats a failed validation — not a map, no partial acceptance — and a client that finds one MUST
discard the whole set and send it again rather than repair it, which is the same posture
`FLAT_Store_Protocol.md` §1 takes toward every broken transfer. The members it named are then
ordinary objects no manifest references, and §5.4's orphan rule reclaims them.

Member ids are **not** required to ascend. Ids come from a never-reused monotonic cursor, but a set
that shares a member with one already on the card reuses that object rather than storing its bytes
twice, so an older id beside newer ones is deduplication working, not a fault.

Member ids are **not** in the `Set Id` digest chain, and this is load-bearing twice over: `Set Id` is
a *content* identity (two assemblies of the same cells with the same skin produce the same id) while
ids are properties of one card's store, and keeping them out is what makes binding a legal eight-byte
write into the staged bytes rather than a re-serialization.

**The terrain record is the last one.** A manifest with a `Role == 3` record MUST carry exactly one,
and it MUST be at index `Shard Count − 1`. That is not house style: readers take the leading records
as the OBCM shards and a record's index as the `S<kk>` of its derived filename, so a raster anywhere
else would renumber every shard after it. Keeping it last means a consumer that never asks about
terrain — every existing mount, dispatch and transfer path — needs no role filter and cannot hand a
raster to an OBCM parser.

**Filenames are derived, not stored**, and since v3 they are no longer the only way to reach a
member. A reader resolving a set through the flat store opens the member ids above and never forms a
name at all; the derived names below are what a reader of a **FAT card** has, and that path is
unchanged. Shard `k` of the set with card id `id` lives at the card root as

```
MS<id>S<kk>.OBM       e.g.  MS7S00.OBM, MS7S01.OBM     (id: 1..3 digits, kk: 2 digits, 00..31)
```

the terrain shard at

```
MS<id>.OBD            e.g.  MS7.OBD
```

and the manifest itself at

```
MS<id>.OBS            e.g.  MS7.OBS
```

The terrain shard carries **no `S<kk>`** — there is at most one, so an index would be a number that
is always `00` and a second thing to keep in step with the manifest. Its name is therefore exactly
the manifest's own stem with the terrain extension, which makes it the `OBCT_Spec.md` §4.6 sidecar of
`MS<id>.OBS`: a host that resolves terrain by the sidecar convention and one that reads the manifest
role open the same file, and the role lookup adds only the two things a sidecar cannot state — that
the set claims a raster, and how many bytes of one.

`MS999S31.OBM` is eight characters, so every name is an 8.3-safe uppercase short name — required,
because the firmware's FAT layer creates short names only. The `MS` prefix keeps sets clear of the
existing single-map `MP<id>.OBM` convention, whose id parser matches on that exact prefix.

Deriving rather than storing the names is deliberate: a stored name is a second source of truth
that can disagree with the directory, and the device would then have to decide which to believe. It
also keeps the reader free of string handling — it formats one integer. The `.OBM` extension is the
same one a single received map uses, so the existing map scan already recognises a shard as *a map
file*; the `.OBS` manifest is the only thing that says those files are **one** map and not several
(§5.4).

`Set Id` is a content identity, not a random one: two assemblies of the same cells with the same
skin produce the same id. It is what a device registry keys a set on, and what lets an upload notice
the set is already present **and bound** — since v3, matching the id says the same bytes are here,
not that they are reachable, because the ids that make them reachable are deliberately not in the
chain.

### 5.3 Validation

A reader MUST reject a manifest unless all of the following hold. There is no partial acceptance:
a set that does not validate does not mount (§5.4).

Below, an **OBCM shard** is a record whose `Role` is `0`, `1` or `2`, and the *shard count* the rules
speak of is the number of those — `Shard Count` minus the terrain record, if there is one.

- length is exactly `72 + 64 × Shard Count`; magic is `OBCS`; `Version == 3`; `Flags == 0`;
- `1 ≤ Shard Count ≤ 32`; `Core Shard < Shard Count`; the record at `Core Shard` has `Role == 0` and
  is the **only** record with `Role == 0`; every other record has `Role == 1`, `2` or `3`;
- at most one record has `Role == 3`, and if one does it is the **last** record (§5.2);
- the member ids are either **all** `0` (unbound) or **all** non-zero and pairwise distinct (bound);
  a half-bound manifest, or one naming a single object twice, is refused (§5.2);
- if there is exactly one OBCM shard it is the core (§5.5); otherwise there is **at least one**
  shard of each role the schema's bands name — at least one `Role == 2` and at least one
  `Role == 1` at the v1 table — because a role with no shard is a map missing whole zoom levels. The
  terrain record is not counted here: it adds a raster, never a zoom level, so a one-shard map with
  terrain beside it is still the fast path;
- the assembly bbox has `min ≤ max` on both axes; every record's bbox has `min ≤ max` and lies inside
  the assembly bbox; the core shard's bbox equals the assembly bbox, and so does the terrain
  record's; the shards of each non-core OBCM role have pairwise disjoint bboxes whose union is the
  assembly bbox (§5.1), which for a single shard of that role means its bbox equals the assembly
  bbox. The terrain record takes no part in that tiling — it spans the whole assembly, so counting it
  would read as an overlapping shard of some role;
- every **OBCM shard** member exists, has exactly the recorded `Bytes`, opens as OBCM with the
  recorded `OBCM Version`, and has a header bbox equal to its recorded bbox. A reader resolving by
  identity reaches each member by its `Member Id`; one reading a FAT card reaches it by the derived
  §5.2 filename. Which of the two is a property of the reader, not of the manifest.

**Being bound is a mount precondition, not a validity rule.** An unbound manifest is valid — an
assembler writes nothing else — so it MUST parse. But a reader that resolves members **by identity**
MUST refuse to mount one: every id it would open is `0`, which names no object. A reader that resolves
by the derived §5.2 filenames needs no id and is unaffected either way, which is what lets the two
resolution paths coexist on one format while the device's own storage is migrated.

The `terrain` record is deliberately **not** in that list, and this is the one place the two halves
of §5.3 have to be read together rather than as a checklist:

**A missing or unreadable terrain shard does not fail the mount.** It is the one exception, and it
follows from `OBCC_Spec.md` §13: elevation is an enhancement, so a set whose raster will not open is
a map that plans, renders and rides exactly as one baked without terrain, while a set whose *core* is
missing is not a map at all. A reader MUST mount such a set, MUST fall back to no elevation, and MUST
NOT present it as a fault. A reader MUST NOT let the terrain record's agreement with the file — its
presence, its `Bytes`, or whether it parses per `OBCT_Spec.md` §4.5 — decide whether the **set**
mounts or lists.

That clemency is about reading a card that has aged: a rider deleted the raster to reclaim space, a
hand copy was truncated, a read glitched, a later OBCT version arrived. It is **not** a licence for a
*writer* to publish a manifest whose terrain record does not describe the file it ships beside it —
a writer MUST verify the record like any other, and a device receiving a set over the wire MUST
refuse a manifest whose terrain record disagrees with the raster it just took
([`obc-ble-interface-spec.md` §4.1](obc-ble-interface-spec.md) rule 7). The asymmetry is between
*reading an old card* and *accepting a new upload*, not between terrain and everything else.

A device MAY defer the SHA-256 check (hashing gigabytes is minutes of work on a microcontroller,
and the cost is the hash itself rather than the storage it is read from) but
MUST verify `Bytes` and the header bbox at mount, and a **host** writing a set MUST verify every
digest before the manifest is written.

### 5.4 Atomicity: the manifest is written last

> **No manifest, or a manifest whose shards do not all validate ⇒ the set never mounts.**

- A writer MUST transfer every shard first and write the manifest **last**. A half-uploaded set
  therefore has no manifest and is invisible as a map.
- A writer replacing an existing set MUST delete the old manifest **before** overwriting any of its
  shards, so the window in which shards are mixed has no manifest pointing at them.
- A reader MUST treat a manifest whose §5.3 validation fails as *no set at all* — it MUST NOT
  mount the shards that happen to be present, and it MUST NOT mount a shard individually as a
  standalone map, even though each shard is a valid OBCM file. A geometry or coarse shard opened
  alone is a map with no roads and no POIs, and the core opened alone is a map that draws nothing at
  all — exactly the kind of quiet wrongness a rider cannot diagnose.
- Shard files with no manifest referencing them are **orphans** and MAY be deleted to reclaim
  space. A writer SHOULD do so when it replaces a set.
- Every UI — device and builder alike — MUST present a set as **one map**. Shard count is an
  implementation detail; it appears in no list, no picker, and no size figure other than the total.

This composes with the resumable-upload work rather than competing with it: shards are independent
files, so per-shard resume, parallel transfer, and re-uploading only the shards a selection change
touched all become possible later without a manifest change.

> **Transferring a set to a device.** The rules above address a *writer*, and a device cannot hold a
> host to them by reading them. The receiving half — the `mapShard` / `terrainShard` / `mapSet`
> object types, the packed `(shard_count, index)` a shard announces itself with, the **refusal** of a
> manifest sent before every shard it names has committed, a device's own shard ceiling, and the
> cleanup a torn upload gets — is normative in
> [`obc-ble-interface-spec.md` §4.1](obc-ble-interface-spec.md), "Volume sets: several transfers, one
> map". Nothing there changes this section; it is what makes it enforceable.
>
> **§5.2's `Shard Count` reaches across that seam**, and it is worth restating where the count is
> defined rather than only where it is checked (#1044). The field counts every record, so the
> manifest of a set with terrain is `72 + 64 × (N + 1)` bytes for `N` OBCM shards. A device checks
> that length at the manifest's *announce*, against the files this upload actually delivered — which
> is why the terrain shard is transferred under its own object type and before the manifest, rather
> than skipped. A host that omits it and sends the longer manifest anyway loses the whole set at its
> last transfer.

### 5.5 Single-file fast path

When the whole assembly fits one OBCM file — §5.7's wall, the interior its `Offset Scale` covers —
the assembler writes a set of **one**: one OBCM shard with `Core Shard = 0`, `Role = 0` and its
bbox equal to the assembly bbox,
carrying every LOD, the nav graph, and the POIs. It is the one case where geometry lives in the core,
and it is safe by construction: the whole map already fits one file, so there is nothing to move out.
Nothing else in §5 changes; the device's dispatch loop simply runs over one shard, and §5.6's
empty-LOD cache finds nothing empty.

v14 moved the *format's* half of that wall from 4 GiB to 64 GiB, and widening the read seam to
64 bits removed the other half — so this path's threshold is now **64 GiB**, and it covers every
selection the builder offers. The measured figures below are the ones that used to decide it and are
kept as the record of what a country costs; they no longer describe a boundary.

**The split path survives the widening for one reason**, and it is not the file's size: a *member*
of a set is recorded in §5.2's `Bytes`, a `uint32`, so a set cannot describe a shard past
`4 GiB − 1`. A selection that takes this path meets no such field and is bound only by §5.7's two
walls. So the fast path is very nearly every case, the split machinery is the exception it was
always meant to be, and what keeps the exception alive is the manifest rather than the map.

If the selection has terrain, the raster rides beside it as the `terrain` record — `Shard Count = 2`,
`map.obcm`-plus-sidecar in every respect that matters — and the fast path is unaffected, because
terrain is always its own file (§5.1) and is therefore never something the core could have absorbed.

At the v1 schema this covers essentially every selection a rider makes below country-plus scale —
a measured whole-Switzerland bake is 0.67 GiB (690 MiB), Baden-Württemberg projects to ≈ 0.71 GiB,
and a 300 km corridor 10 km wide to ≈ 0.25 GiB — so multi-shard sets are the exception, reached at
roughly Germany scale.

### 5.6 Mount-time LOD presence: why role-free dispatch is also free

§5.1's dispatch rule is deliberately role-blind — a viewport query goes to every shard whose bbox
intersects it, and a shard that does not carry the requested LOD contributes nothing. Two shards of
a set have a bbox that intersects **every** query: the core and (unsplit) the coarse shard both span
the whole assembly. So role-blind dispatch would, naively, walk into the core file at every zoom
level and into every geometry shard at zoomed-out ones, to discover an empty index each time.

That discovery costs nothing if it is made once:

- A reader **SHOULD** cache, per mounted file, the per-LOD `Index Node Count == 0` predicate at
  **mount** time, and skip a file's LOD without any I/O when the predicate says the region is empty.
  The LOD table is already read resident at open (`OBCM_Spec.md` §3), so the cache is derived from
  bytes the reader has in hand — at the v1 ladder it is **7 bits per file**, and a 32-shard set's
  whole table is 32 bytes.
- A reader MUST NOT infer band membership or a role from that cache (§3.1: a legitimately empty cell
  is indistinguishable from an out-of-band one), and MUST NOT use it as a substitute for the
  manifest's `Role`. It is a pure I/O-avoidance predicate over one file's own LOD table.
- With the cache in place, a zoomed-out viewport reads exactly one file (the coarse shard), a
  zoomed-in one reads the one or two geometry shards its box straddles, and the core is opened only
  for nav and POI queries — with no role logic anywhere in the dispatch path.

### 5.7 Robustness: every file's size is known before anything is downloaded

This subsection is the design's safety property, and it is normative.

Every physical file of a set is **exactly computable from the catalog before a single byte is
fetched**. A shard's bytes are the sum of the sizes of the cells it will carry — `bytes` is published
per cell ([`OBCC_Spec.md` §11.6](OBCC_Spec.md)) and, for a named region, per band in the catalog root
(`bytes_by_band`, [`OBCC_Spec.md` §11.5](OBCC_Spec.md)), which is exactly the split the roles need
(§6.1) — plus fixed
overheads that do not depend on content: the 40-byte header, the schema's style table, the LOD table,
the POI and nav directories, and the per-LOD offset tables, whose sizes follow from the cell count.
A consumer computing that sum is not estimating; it is adding up numbers the catalog states.

Therefore:

- A consumer **MUST** project every file of the set — core, coarse shards, geometry shards — before
  the download, and **MUST refuse the selection** if any projected file exceeds the per-file wall,
  which is **whichever is smaller**:

  1. **the interior its `Offset Scale` covers** ([`OBCM_Spec.md` §1.1](OBCM_Spec.md)) — `2^32 × U`,
     64 GiB at the scale every producer in this tree writes; and
  2. **what the consumer's read seam addresses** — because a file the format permits but the reader
     cannot open is not a legal file, it is an unreadable one. In this tree that seam is
     `ByteSource`, whose offsets and length are **64-bit**, so it no longer bounds anything a
     `uint32`-unit offset can name.

  It MUST NOT begin fetching a set it cannot legally write **or read**. Both walls are stated by
  reference rather than as literals: the first is a property of the file's own scale byte, the
  second of the implementation's addressing, and a number copied into this section would go stale
  against either. **The first now binds, at 64 GiB.** It did not until the read seam widened: that
  seam was `uint32` and held the effective wall at 4 GiB — the same number as the pre-v14 one, by
  coincidence rather than inheritance — and widening it was its own slice of work, the prerequisite
  for single files past 4 GiB.

  A **member of a set** has a third wall, and it is narrower than both: §5.2's `Bytes` is a
  `uint32` of bytes, so a manifest cannot record a shard past `4 GiB − 1`. A producer emitting a
  set MUST apply that bound to every member, and MUST refuse rather than record a truncated size.
  The single file of §5.5 carries no manifest record and is bound only by the two walls above.
- For the **core** specifically it SHOULD warn above **seven eighths of that wall**, and both the
  refusal and the warning MUST name the **navigation graph** as the reason and the coverage as the
  thing to reduce — because after this section's split the core is nav plus POIs and nothing else,
  so no other explanation is true. The warning is a *proportion* — "you are close" — rather than a
  size, for the same reason the refusal is a reference. (A set's core answers to §5.2's narrower
  wall, so in practice the warning a *set* produces is ⅞ of `4 GiB − 1`, ≈ 3.5 GiB — the figure
  this section always wrote.)
- A consumer MUST apply the schema's own cell-bake budget (§1.5's +5–15 %, measured headroom rather
  than an expected cost) on the *pessimistic* side of that comparison, so the projection is an upper
  bound rather than a hope.
- An assembler **MUST** fail rather than emit an over-size file, and MUST NOT "solve" an over-size
  core by splitting the nav graph, dropping POIs, or degrading coverage silently. It MAY split a
  coarse or geometry shard further, since that is what those roles are for.
- §4.8's verify then re-checks every file's actual size against the ceiling, so the pre-download
  projection is bounded on both ends: refused before the fetch, and re-asserted before the write.

> **The two figures above were `4 GiB − 1 B` and `≈ 3.5 GiB` before OBCM v14.** They were not
> arbitrary: a byte offset was a bare `uint32`, so a file stopped at 4 GiB, and FAT32 — the card
> format the firmware's FAT stack wrote — capped a file at exactly the same number. Two independent
> walls landing on one value is why sets exist at all (§5.5), and why so much of this section reads
> as though 4 GiB were a law of nature.
>
> Both of those causes are gone. v14 scales offsets (§1.1), moving the format's own wall to
> `2^32 × U`, and the flat store replaced FAT. The number did not move at first, because a third
> wall was behind them the whole time: the read seam. `ByteSource` was `uint32`, so 4 GiB stayed
> where a file stopped — because nothing could read past it rather than because nothing could
> address or store it.
>
> **That third wall is now gone too**, and the number finally moved. `ByteSource` addresses 64 bits,
> so the smaller of the two walls in the rule above is §1.1's interior and a lone file stops at
> 64 GiB. The core warn was stated as the ⅞ proportion rather than as "≈ 3.5 GiB" for exactly this
> day: it followed the wall up without being rewritten.
>
> **What still splits a country-scale selection is §5.2's `Bytes`**, which is a `uint32` of bytes in
> the OBCS manifest and did not widen with anything. It bounds a *member of a set* at `4 GiB − 1`
> and bounds a single file not at all — so §5.5's fast path reaches the format's wall while the
> split path it falls back to is held at the old number. That is a property of the manifest, and it
> dies with the manifest.
>
> Statements elsewhere in this document that name `4 GiB − 1` describe the **pre-v14** design and
> the reasoning that produced the split; they are history, not the current per-file wall, and §5.7
> is the normative statement. The exception is §5.2's `Bytes` field, above.

**The terrain shard is the easiest file of the set to project, and it is projected the same way.**
Its size is `32 + 4 · rows · cols + present · T² · 512` — a header, a directory over the assembly
rectangle, and one fixed-size block per square that has a published object — and every term is known
from the catalog: the rectangle from the assembly bbox and the terrain block's `cell_log2`, the block
length from the `posting_log2`/`cell_log2` pairing (`OBCT_Spec.md` §3.2), and which squares are
present from the pinned terrain index. Nothing is content-dependent, so §1.5's `+5–15 %` budget does
**not** apply to it: that allowance exists for what a *bake* varies by, and a raster's size does not
vary. A consumer MUST show the raster's bytes as their own figure rather than folding them into the
map's — `OBCC_Spec.md` §13.3 makes the two separate prices, because a rider may take one without the
other.

The consequence is the point: **no runtime and no on-device path can ever encounter an over-limit
file.** A device only ever sees a set that a host projected, assembled, verified and wrote, and each
of those three stages independently rejects an over-size file. Density growth — a denser OSM
snapshot, a schema that keeps more detail, a region that urbanises — therefore degrades to a clear
pre-download refusal in a builder UI. It cannot degrade to a truncated `uint32` offset, a wrapped
FAT32 write, or a map that opens and then misroutes.

The honest capability cap follows from the same arithmetic. One logical map is limited to the ground
whose **nav graph alone** approaches the ceiling: ≈ 640–700 thousand km² at v1 densities, or
≈ 550–610 thousand km² carrying the pessimistic +15 % budget (§1.5) — comfortably past DACH, short of
a continent. Lifting that cap is one specific future change, a **sharded nav graph** with cross-file
boundary nodes, and it is deliberately not v1 because it is the only change here that would touch the
router. Nothing else in the design needs to move: geometry already scales horizontally, and after
§5.1's split the core holds nothing else.

---

## 6. Catalog, schema, and skins

The catalog contract is [`OBCC_Spec.md`](OBCC_Spec.md); `schema_version 2` there carries cells,
bands, schemas, skins, and region cell-sets. This section states only the parts that are OBCA's to
define.

### 6.1 What the catalog must say about a cell

For each published cell: its id (§1.3), its band, its schema revision, its OBCM version read from
its own header, its size, its SHA-256, its URL, its source extents and snapshot dates, and whether
it is **`partial`** (§3.7). A consumer must be able to price an assembly — cell count and total
bytes, **per band** — from the manifest alone, before fetching anything; that is OBCC's
knowable-before-the-download guarantee applied to cells, and it is what makes §5.7's per-file
projection arithmetic rather than estimation. Per band matters because the roles partition by band:
the core's size is the `network` band's cell bytes plus fixed overheads, the coarse shard's is the
`coarse` band's, and the geometry shards' is the `mid` and `fine` bands'.

### 6.2 Schema owns ids; skin owns values

A schema revision fixes: the feature types and their `min_lod`, the LOD ladder (`Max Meters/Pixel`
per level), simplification tolerances, cull thresholds, the merge passes, `Chunk Size`, the
routing profile table, `min_component_edges`, and the **band table** (§1.2). All of it is baked
into chunk bytes, so it is the identity of a cell store.

Critically, the schema also fixes the **style-id assignment**: `obc-pack` numbers feature types
`1`-based in config document order (`OBCM_Spec.md` §2), and those ids are referenced by every
feature header in every chunk. A schema revision therefore has one **canonical style table** — one
id per feature type, in one order — and a skin may change only the other seven bytes of each
record plus the header's `Marker Color`. That is the byte-level meaning of the schema/skin split,
and it is what makes a restyle free (§4.7).

The hosted catalog has **exactly one** schema: the 14-LOD bikepacking ladder. It is the ladder
tested to render inside the device's RAM and map-complexity budget; a superset schema was rejected
because it would make every map carry complexity the device cannot honour. Hosted "presets" are
therefore skins. Custom schemas remain a local-bake affair for the desktop app, which packs from
an extract exactly as before.

### 6.3 Lockstep and the bake guard

An OBCM version bump or a schema-revision bump invalidates **every** cell, because assembly copies
chunk bytes between files and that is only meaningful within one revision. The bakery's guard MUST
refuse to publish a catalog that mixes OBCM versions or schema revisions across cells, exactly as
`OBCC_Spec.md` §6 already refuses a mixed-version artifact catalog, and an assembler MUST refuse a
mixed input set (§4.1). The set manifest records the schema revision (§5.2) so a device can say
which revision a card holds.

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
7.340032…7.864320 °E. The numbers are as unromantic as the format is, which is the point — they are
exact.

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

Nodes 3 and 4 *are* cells A and B, to the microdegree — the theorem, in four lines.

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

- The map byte format every cell, shard, and assembly is an instance of:
  [`OBCM_Spec.md`](OBCM_Spec.md); its code authority
  [`firmware/obc-formats/src/obcm.rs`](../firmware/obc-formats/src/obcm.rs).
- The catalog that publishes cells, bands, schemas, skins, and region cell-sets:
  [`OBCC_Spec.md`](OBCC_Spec.md).
- The packer whose quadtree, anchor, and nav conventions this specification constrains:
  [`host/obc-pack`](../host/obc-pack) — the quadtree in `quadtree.rs`, the byte layout in
  `serialize.rs`, the routable graph and its class tables in `nav.rs`.
- The reader every verify pass (§4.8) and the device itself run:
  [`firmware/obc-reader`](../firmware/obc-reader).
- The cell-size measurement behind §1.5:
  [`host/obc-pack/examples/cell_size_survey.rs`](../host/obc-pack/examples/cell_size_survey.rs).
- The bakery that will cut and publish cells, and the curated region list that names the
  selections: [`host/obc-bake`](../host/obc-bake).
- The conceptual tour, with diagrams: the docs site's
  [data formats](../docs/content/software/formats.md) page.
