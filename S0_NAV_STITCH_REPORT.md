# S0 — cross-file nav stitch spike (epic #1016)

**Question.** Can two *independently packed* adjacent `.obcm` maps have their §8 navigation
graphs merged into one routable graph by **coordinate-based node unification**, such that a
shortest-path search routes across the shared border?

**Verdict: yes — exact-coordinate unification is sufficient.** Nothing needed an epsilon snap,
and connector edges would have had nothing to connect: across two separately packed extracts,
a junction that exists in both files has **bit-identical microdegree coordinates**, and a
junction that does not match exactly has *no* counterpart within 100 m at all. Dijkstra over the
union graph routes Freiburg im Breisgau → Zürich in **103.5 km** across the Rhine at
Waldshut–Koblenz; both single-file control runs are unreachable. Two short cross-border hops
behave the same, as does a pair of **adjacent 2²⁰ µdeg cells** cut from one extract (§6 — the
epic's own scenario). Zero merge steps joined nodes that share no source file, so the merge
invents no connectivity, and the §8.3 wire limits (degree cap, `int16` deltas, `uint16` cost)
survive with room to spare.

Two findings shape the design work that follows:

- **An epsilon snap must NOT be added.** In the adjacent-cell pair, unmatched junctions *do* have
  counterparts a few metres away (closest 3.89 m) — and they are genuinely *different* nodes,
  because junction detection and edge splitting are relative to the ways a pack run sees. Exact
  equality is not merely sufficient, it is the only safe rule (§6).
- **Island pruning should move to assembly time** — recommended on the mechanism, not on a
  measured failure: in every configuration measurable with today's tooling, bake-time pruning
  destroyed nothing unification wanted (§5).

---

## 1. What was built and measured

Two adjacent Geofabrik extracts, packed separately with the shipped `builder/presets/default.json`
(unmodified — `routing.min_component_edges = 50`, the 7-LOD bikepacking ladder):

| | `freiburg-regbez-latest.osm.pbf` | `switzerland-latest.osm.pbf` |
| :-- | --: | --: |
| source PBF | 158 MB | 541 MB |
| `.obcm` | 199 050 552 B | 724 276 015 B |
| pack wall time | ~2 min | ~5 min |
| header bbox (lat) | 47.40585 … 48.76460 | 45.72243 … 47.83507 |
| header bbox (lon) | 7.50235 … 9.75384 | 5.80633 … 10.56695 |
| §8 junctions (read back) | 720 298 | 2 693 325 |
| §8 adjacency entries | 1 836 323 (≈ 918 161 edges) | 6 767 001 (≈ 3 383 500 edges) |
| coords carrying >1 junction | 9 | 28 |

The pair genuinely overlaps: the extracts share the Hochrhein/Basel border, and Freiburg-Regbez
wraps around the Schaffhausen salient, so there are several separate stretches of shared ground.

Analysis code: `host/obc-pack/examples/nav_stitch_spike.rs` (an example — no shipped crate
behaviour is touched). It reads both files through the real `obc-reader` (`for_each_nav_node`
over the full header bbox, so every leaf and therefore every junction record is visited; the walk
is made idempotent by keying on `Node Id`, per spec §8.2's shared-chunk warning), builds an
in-memory merged graph, and runs plain Dijkstra on raw `Cost M` (profiles deliberately ignored).

> Note in passing: the reader sees **4 489 more junctions and 2 957 more edges** in
> `switzerland.obcm` than `nav.rs` reported building (`nav graph: 2 688 836 nodes, 3 380 543
> edges`). That is the serializer's own §8.4 splitting (chunk-fit / post-densification `int16`
> span), which mints further synthetic degree-2 junctions after `nav.rs` is done. Worth knowing
> for the assembler: the node set it must renumber is the *serialized* one, not the builder's.

## 2. Border analysis — do the two extracts agree on coordinates?

**Coordinate agreement is exact or absent. There is no near-miss regime.**

Restricting to the region both files actually describe (a 0.01° occupancy grid intersected —
**632 cells, ≈ 528 km²** of double coverage, *not* the much larger bbox intersection):

| | junctions in region | exact coord matches |
| :-- | --: | --: |
| double-covered band, freiburg | 42 422 | **21 144** (49.8 %) |
| double-covered band, switzerland | 41 888 | **21 144** (50.5 %) |
| band interior only (all 8 neighbour cells also double-covered), freiburg | 8 115 | **5 878** (72.4 %) |
| band interior only, switzerland | 6 256 | **5 878** (94.0 %) |

Near-miss buckets, for every band junction of the Freiburg file that has **no** exact match,
measured against the switzerland file's unmatched junctions:

| ≤ 1 m | ≤ 10 m | ≤ 100 m | ≤ 1000 m | farther / none |
| --: | --: | --: | --: | --: |
| **0** | **0** | **0** | 10 | 21 268 |

The closest unmatched cross-file pair anywhere in the band is **110.63 m**
(freiburg 47.661356,8.876841 vs switzerland 47.661999,8.875716) — two different junctions on the
same road, not two copies of one junction. Concrete exact matches, for eyeballing:
47.529407,7.691591 · 47.564117,7.620798 · 47.572084,7.621733 · 47.585619,7.672464 ·
47.635963,8.606616 · 47.696393,8.660893.

Why this is the expected shape, and why it is the *load-bearing* result: both extracts carry the
same OSM node coordinates, and the whole §8 path is integer and deterministic — one
`(deg * 1e6).round()` from the source degrees (`poi::to_udeg`), no reprojection, no
simplification (§8 geometry is never simplified, unlike §5), and the packer's synthetic junctions
(`nav.rs::split_edge`, plus the serializer's own splits) cut at **existing polyline vertices**
chosen by a deterministic midpoint index. Identical input geometry therefore produces identical
coordinates, synthetic ones included. A junction cannot drift by a metre between two pack runs;
it is either the same integer pair or absent. (The load-bearing word is *identical* — §6 shows
what happens when two runs see the same road but a different **set of ways** around it.)

The unmatched half of the band is not a failure: it is where one extract has a junction the
other cannot have, because the side road that *creates* the junction is outside its own cut.
Unification handles it correctly by construction — the merged node simply keeps the union of
both files' adjacency.

### Geofabrik cut behaviour

Geofabrik does **not** hard-cut border-crossing ways at the political border; it keeps ways
complete (the `osmium extract --complete-ways`/`smart` behaviour), so each extract carries a
**thin overhang** of the neighbour's road network:

- Double-covered band depth, per 0.01° longitude column (139 columns):
  **min 1.1 km, p50 3.3 km, p90 21.2 km, max 25.6 km.** The p50 is the ordinary
  complete-ways overhang; the fat tail is the Schaffhausen salient, where the border itself is
  convoluted and both extracts legitimately cover the same ground for tens of km.
- Transects (each file's latitude extent in a 0.01° lon column, plus the shared band):

  ```
  lon 7.75: freiburg 47.5430..48.4371 | switzerland 45.9388..47.5515 | shared 47.5430..47.5515 (0.9 km)
  lon 8.00: freiburg 47.5498..48.6962 | switzerland 45.9976..47.5578 | shared 47.5498..47.5578 (0.9 km)
  lon 8.25: freiburg 47.6099..48.6163 | switzerland 46.3417..47.6184 | shared 47.6099..47.6184 (1.0 km)
  lon 8.50: freiburg 47.5749..48.4010 | switzerland 46.1744..47.7772 | shared 47.5749..47.7772 (22.5 km)
  lon 8.75: freiburg 47.6868..48.3732 | switzerland 46.0922..47.7044 | shared 47.6868..47.7044 (2.0 km)
  ```

- Dead-end (degree-1) rate **inside** the double-covered band: 9 911 / 63 166 = **15.69 %**
  (7 094 of them known to only one file). Outside the band, as a control: 652 112 / 3 329 274 =
  **19.59 %**. A hard cut would leave the band *denser* in dead ends than the interior; it is
  in fact slightly sparser. That is the strongest single piece of evidence that ways survive
  the extract boundary intact.

So today's extract pair gives unification a **generous** seam: for ~1–3 km either side of the
border, both files describe the same roads, and unification has thousands of coincident
junctions to work with. A cell cut will give it a **zero-width** seam — see §5.

## 3. Unified routing

Merged graph (union of both node sets keyed by exact `(lat, lon)`; edges from both files' §8.3
adjacency, deduped on endpoint pair + `Cost M`):

```
nodes 3 392 440   (unified/shared by both files: 21 144)
edges 4 273 566   (described by both files:     25 637)
max merged degree 10                (nodes over the §8.3 cap of 24: 0)
max neighbour delta 31 967 µdeg     (i16 bound 32 767; nav.rs splits at 32 000)
nodes whose adjacency is a genuine union of both files' entries: 1 626
```

The wire limits therefore survive unification with room to spare: unifying two junctions unions
their adjacency, and the merged maximum degree is **10** against §8.3's cap of 24. Neighbour
deltas cannot grow at all (every merged neighbour was already some file's neighbour), and the
measured maximum, 31 967 µdeg, is the packer's own 32 000 split bound showing through.

Dijkstra on raw `Cost M`, snapping start/goal to the nearest merged junction. "control" runs are
the same query with relaxation restricted to one file's edges.

| query | control: freiburg only | control: switzerland only | **merged** | handovers | invalid steps |
| :-- | :-- | :-- | :-- | --: | --: |
| Freiburg im Breisgau 47.9990,7.8421 → Zürich 47.3769,8.5417 | unreachable | unreachable | **103.47 km**, 1 311 nodes (23 shared) | 1 | 0 |
| Lörrach 47.6150,7.6600 → Liestal 47.4840,7.7350 | unreachable | unreachable | **19.26 km**, 293 nodes (44 shared) | 1 | 0 |
| Waldshut 47.6230,8.2140 → Brugg 47.4810,8.2080 | unreachable | unreachable | **18.74 km**, 322 nodes (25 shared) | 1 | 0 |

- **Freiburg → Zürich: 103.47 km.** Sanity: road distance is ~85–130 km; straight line is 86 km.
  Route provenance by length: **63.6 km freiburg-only, 39.5 km switzerland-only, 0.4 km
  described by both** — i.e. the route really is built out of two files' edges, handing over
  once.
- **Crossing used:** the Rhine bridge at **Waldshut–Koblenz AG**, handover at
  **47.6086,8.2363** (the route walks a run of shared junctions ~47.6093,8.2319 →
  47.6089,8.2325 and continues on switzerland-only nodes). The Waldshut→Brugg query crosses at
  the same bridge (47.6037,8.2292); Lörrach→Liestal crosses at **47.5357,7.7114**
  (Grenzach-Wyhlen ↔ Riehen), so at least two independent crossings work.
- **`invalid steps: 0`** on every route: no relaxation ever stepped between two nodes that share
  no source file. Since an edge always comes from exactly one file, this is the integrity check
  that unification adds no phantom connectivity — the merge only ever joins through genuinely
  coincident junctions.
- The short hops were chosen with both endpoints **clear of the overhang band** (≥ 10 km from
  the border), which is why both single-file controls fail. That matters: a naive "short hop
  across the Rhine at Basel" is *not* a test, because the switzerland extract's overhang already
  contains both endpoints and routes it alone.

## 4. Cost of the unification key

Coordinate keying also fuses junctions **within** one file when two junctions share a coordinate:
9 such coordinates in freiburg, 28 in switzerland (out of 0.72 M / 2.69 M). These are
vertically-stacked junctions (bridge/tunnel decks meeting in plan view). Fusing them is a
routing bug in principle (it can invent a turn between a bridge and the road under it); at
1-in-100 000 it is not a blocker, but the assembler should be aware that exact-coordinate
unification is not *quite* injective. A cell assembler can restrict unification to the seam
(cell-boundary nodes only), which removes the interior cases entirely.

## 5. Island pruning

**What the code does today** (`host/obc-pack/src/nav.rs`):

- `DEFAULT_MIN_COMPONENT_EDGES = 50` (nav.rs:49), and `builder/presets/default.json` sets
  `routing.min_component_edges: 50` explicitly, so the shipped bakes use exactly this.
- `prune_islands` (nav.rs:492) runs as **pass C of `build_graph_with`**, i.e. *before* the
  `int16`/`uint16` edge splits (pass D). It union-finds over edge endpoints and keeps the
  **largest component by node count** plus **every component with ≥ 50 edges**; everything else
  is dropped and surviving nodes are re-densified.
- Measured on the switzerland bake: `nav components: 14 170 found, 27 kept, 28 995 edges
  dropped` — 0.86 % of edges, 14 143 components.

**Did pruning damage the seam in today's artifacts? No — measured against an unpruned control.**

Both extracts were packed a **second** time with `routing.min_component_edges: 1` (pruning
effectively off) and the whole analysis re-run, so the question is answered against ground truth
rather than by inspection:

| | shipped (threshold 50) | unpruned (threshold 1) | dropped by pruning |
| :-- | --: | --: | --: |
| freiburg junctions | 720 298 | 730 583 | 10 285 (1.4 %) |
| switzerland junctions | 2 693 325 | 2 734 352 | 41 027 (1.5 %) |
| switzerland `nav components` | 14 170 found, 27 kept, 28 995 edges dropped | 14 170 found, 14 170 kept, 0 dropped | 0.86 % of edges |
| exact cross-file coord matches | 21 144 | 22 937 | 1 793 |

Connected components of the merged graph:

| | shipped pair | unpruned pair |
| :-- | :-- | :-- |
| components | 33 | 17 090 |
| largest | 3 388 893 nodes / 4 267 055 edges (99.90 %) | 3 388 901 nodes / 4 267 189 edges (98.46 %) |
| below the 50-edge threshold | 0 | 17 056 (49 457 nodes, 34 381 edges) |
| … with a node in the double-covered band | 0 | 505 |
| **merge-rescued** (≥ threshold merged, < threshold in **each** file alone) | **0** | **0** |
| components described by both files at all | 1 | 230 |

The decisive cell is **merge-rescued = 0 on the unpruned pair**: not one component in the whole
border region is small in each extract yet big enough to keep once unified. So the shipped
threshold destroyed nothing that unification wanted — the 505 small components sitting in the
band are genuinely tiny islands (private yards, disconnected footpath scraps) on both sides.

The reason is exactly the Geofabrik overhang measured in §2: a stub in the band is 1–3 km deep
and is still attached to its own country's network *inside its own extract*, so it was never an
island to begin with. Note also the direction of the effect on the seam itself: pruning removed
1 793 of the coincident junctions, i.e. it *thins* the seam without breaking it.

**Pruning at cell scale, measured.** Because "is this component small" is asked *per pack run*,
the same roads get a different verdict at a different cut. One 2²⁰ µdeg cell
(1.048576°, the epic's strawman fine-band size, grid-aligned to origin 0 — lat 47.18592…48.234496,
lon 7.340032…8.388608, straddling the border from Basel to Waldshut) baked out of each extract:

| bake | components found | kept | edges dropped |
| :-- | --: | --: | --: |
| all of switzerland | 14 170 | **27** | 28 995 / 3 380 543 = 0.86 % |
| that one cell, from the switzerland extract | 1 497 | **5** | 3 484 / 502 512 = 0.69 % |
| that one cell, from the freiburg extract | 1 643 | **10** | 3 449 / 450 069 = 0.77 % |

So the *share* of edges pruning eats does not blow up at cell scale — but note how the notion of
"the map's road network" collapses (27 kept components for a country, 5 for a cell): what counts
as an island is entirely relative to the cut. And the honest limit of this measurement: `obc-pack
--bbox` is `osmium extract`'s **`complete_ways`** (`ingest.rs`, `Crop`), so a cropped bake keeps
every boundary-crossing way *whole* — it is not the hard cut at the cell edge the epic plans.
Today's tooling therefore **cannot** measure the hard-cut case, and the measurable cases all come
out clean (merge-rescued = 0 everywhere, including the adjacent-cell pair in §6).

That leaves prune-at-assembly as a **risk to be designed out, not a measured failure**. The
mechanism is real and easy to state: a hard cut severs the network at a line, so a fragment can
hold fewer than 50 edges *in each of two cells* while being a perfectly good road once assembled;
both neighbours drop their half, and no assembly-time unification can recover bytes that were
never written. The cheap, safe rule:

> At bake time prune only components **strictly interior** to the cell (no node on the cell
> boundary); let the **assembler** run the real pruning pass over the merged graph, where
> component sizes are finally true.

The assembler already renumbers nodes and rewrites the edge pool, so a union-find on top is
nearly free, and `min_component_edges` then means what it says — an island in the *map*, not in
the *cell*. Recommended for P1/P2/P3 on the strength of the mechanism; the numbers above say the
cost of getting it wrong is small-but-not-zero, not catastrophic.

Also worth recording for P2/P3: the pruning threshold must be part of the **schema revision**,
not the skin. Two cells pruned at different thresholds are not assemblable into a graph with
consistent semantics.

## 6. Bonus: two *adjacent cells* from one extract (the epic's real scenario)

The cross-border pair is the hardest seam; the epic's actual seam is two adjacent grid cells cut
from the same source. Both 2²⁰ µdeg cells at lon 7.340032…8.388608 were baked from the
switzerland extract — north lat 47.18592…48.234496, south lat 46.137344…47.18592 — and assembled
by the same spike:

```
cells: 397 184 + 480 901 junctions  →  merged 877 250 nodes (823 unified), 1 097 010 edges
Olten 47.3500,7.9040 → Bern 46.9480,7.4474 :  north alone unreachable, south alone unreachable,
                                              merged REACHABLE 61.17 km, 1 handover, 0 invalid steps
Solothurn 47.2080,7.5370 → Burgdorf 47.0590,7.6270 : merged REACHABLE 20.46 km, 1 handover, 0 invalid
components: 12, none below threshold, merge-rescued 0
```

Two things this adds that the border pair could not:

1. **A cell seam is thin.** Only **823** junctions coincide along the whole ~78 km seam (versus
   21 144 across the border pair's 1–3 km-deep band), because a `complete_ways` crop only
   overhangs by one way's length. Unification still carries the routing — but the epic's plan to
   put **deterministic synthetic junctions on the boundary line** is what turns "thin but lucky"
   into "correct by construction", and this measurement is the argument for it.
2. **A near-miss regime does exist — and it is a trap.** Here, unlike the border pair, unmatched
   junctions *do* have close counterparts: 3 pairs within 10 m, 366 within 100 m, the closest at
   **3.89 m** (47.185337,7.400213 vs 47.185304,7.400230). The cause is not coordinate drift: a
   node is a junction only if ≥ 2 of the ways *present in that pack run* touch it, and
   `nav.rs`'s edge splits cut at the **midpoint index of the edge as that run sees it**. Two
   runs with different way sets over the same road therefore place *different* synthetic
   degree-2 nodes a few metres apart. They are genuinely different nodes, so
   **an epsilon snap at 10 m or 100 m would fuse things that must not be fused** — exact-only
   unification is not just sufficient, it is the *safe* rule. The corollary for the cutter:
   only nodes derived from the boundary line itself (plus real OSM junctions) may be relied on
   to unify; midpoint-derived synthetic junctions must never be load-bearing at a seam.

## 7. What this settles for the epic

- **Verdict: exact-coordinate unification is sufficient.** No epsilon snap, no connector edges.
  It holds in the *hardest* case measured — two files packed in separate runs from separately cut
  sources, with independent dense id spaces and independent edge pools — and in the epic's own
  case (two adjacent cells). The optional `--connect-m` epsilon path in the spike had nothing to
  do.
- **An epsilon snap would be actively wrong.** §6's near-miss regime (3 pairs ≤ 10 m, 366
  ≤ 100 m, closest 3.89 m) is made of genuinely *different* junctions, produced by
  run-dependent junction detection and midpoint edge splits. Snapping would fuse them.
- **Only boundary-derived nodes may be load-bearing at a seam.** `nav.rs::split_edge` and the
  serializer's splits cut at existing polyline vertices — deterministic given identical input
  geometry, but the *input geometry* differs between runs that see different way sets. So the
  cutter must derive its boundary junctions from the cell-edge line itself (an intersection
  coordinate rounded identically in both cells), never rely on interior synthetic nodes lining up.
- **The assembler must renumber the *serialized* node set**, not `nav.rs`'s — the serializer mints
  extra synthetic junctions (§8.4 chunk-fit / span splits) after the builder finishes (+4 489
  nodes, +2 957 edges on the switzerland bake).
- **Degree caps, `int16` neighbour deltas and `uint16` costs survive unification** — measured, not
  assumed: max merged degree **10** against the cap of 24 (0 nodes over it) across 3.39 M merged
  nodes, max neighbour delta **31 967 µdeg** (the packer's own 32 000 split bound), and every
  merged neighbour was already some file's neighbour so no cost or delta is ever recomputed.
- **Island pruning: move it to assembly time** — as a design decision, on the strength of the
  mechanism rather than a measured failure (§5: merge-rescued = 0 in every configuration
  measurable with today's `complete_ways` cropping).
- **Provenance (D3) is visible in the data**: in the double-covered band, only ~50 % of each
  file's junctions exist in the other, because each extract lacks the side roads that create the
  neighbour's junctions. A border cell baked from one extract is therefore *demonstrably* not
  the cell a covering source would produce — the guard in D3 is required, not paranoia.
- **One residual wart:** coordinate keying fuses the handful of coordinate-colliding junctions
  within a file too (§4). Restricting unification to seam nodes removes it.

## 8. Reproduction

```sh
# 1. tools
cargo build --release                       # from the repo root

# 2. sources (outside the repo — do not commit .pbf/.obcm)
mkdir -p /tmp/s0 && cd /tmp/s0
curl -fLO https://download.geofabrik.de/europe/germany/baden-wuerttemberg/freiburg-regbez-latest.osm.pbf
curl -fLO https://download.geofabrik.de/europe/switzerland-latest.osm.pbf

# 3. pack both, separately, with the shipped preset
OBC=<repo>
$OBC/target/release/obc-pack freiburg-regbez-latest.osm.pbf $OBC/builder/presets/default.json freiburg.obcm
$OBC/target/release/obc-pack switzerland-latest.osm.pbf     $OBC/builder/presets/default.json switzerland.obcm

# 4. the spike (default routes are the three in §3)
cargo run --release --example nav_stitch_spike -- /tmp/s0/freiburg.obcm /tmp/s0/switzerland.obcm

# optional: extra routes, and the epsilon experiment that turned out to be unnecessary
cargo run --release --example nav_stitch_spike -- /tmp/s0/freiburg.obcm /tmp/s0/switzerland.obcm \
    --route 47.9990,7.8421:47.3769,8.5417 --connect-m 25

# 5. §5's unpruned control: same packs with routing.min_component_edges = 1
python3 - <<'EOF'
import json; c = json.load(open('builder/presets/default.json'))
c['routing']['min_component_edges'] = 1
json.dump(c, open('/tmp/s0/default-noprune.json', 'w'), indent=2)
EOF
$OBC/target/release/obc-pack freiburg-regbez-latest.osm.pbf /tmp/s0/default-noprune.json freiburg_p1.obcm
$OBC/target/release/obc-pack switzerland-latest.osm.pbf     /tmp/s0/default-noprune.json switzerland_p1.obcm
cargo run --release --example nav_stitch_spike -- /tmp/s0/freiburg_p1.obcm /tmp/s0/switzerland_p1.obcm

# 6. §6's adjacent-cell pair: two 2^20 µdeg cells cut from ONE extract, then assembled
P=$OBC/builder/presets/default.json
$OBC/target/release/obc-pack switzerland-latest.osm.pbf $P ch_cell.obcm   --bbox 7.340032,47.18592,8.388608,48.234496
$OBC/target/release/obc-pack switzerland-latest.osm.pbf $P ch_cell_s.obcm --bbox 7.340032,46.137344,8.388608,47.18592
cargo run --release --example nav_stitch_spike -- /tmp/s0/ch_cell.obcm /tmp/s0/ch_cell_s.obcm \
    --route 47.3500,7.9040:46.9480,7.4474 --route 47.2080,7.5370:47.0590,7.6270
```

Timings on an M-series laptop with 16 GB: freiburg pack ~2 min, switzerland ~5 min, a cell crop
~1 min. The full-country analysis run needs ~5 GB of RAM (3.4 M junctions resident with
adjacency) and ~2 min; **don't run it while a country pack is running** — 16 GB is not enough for
both, and the first attempt at exactly that had the packer killed mid-write.
