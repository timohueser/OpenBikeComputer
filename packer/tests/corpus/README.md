# OBCM packer validation corpus

The fixed set of `.osm.pbf` inputs the Rust port (`firmware/obc-pack`, see
[`../../docs/rust-port-plan.md`](../../docs/rust-port-plan.md)) is validated
against. Chosen so **area assembly and coastline ingest are exercised from day
one**: every item has a known purpose, increasing in size and messiness.

The `.pbf`/`.osm.pbf` files are **git-ignored** (`*.osm.pbf`). Only the
hand-authored XML source [`tiny/tiny.osm`](tiny/tiny.osm) and this manifest are
checked in. Regenerate the binary corpus with [`build_corpus.sh`](build_corpus.sh).

## Items

| name | size | features | polys w/ holes | coastlines | role |
| :-- | --: | --: | --: | --: | :-- |
| `tiny`            | 735 B | 11    | 1   | 1  | unit-level; every ingest branch, known by construction |
| `monaco`          | 0.7 MB | 4.2 K | 9   | 24 | small **coastal**, fast iterate |
| `malta`           | 8.4 MB | 79 K  | 338 | 93 | **coastal + multipolygon-heavy** (the primary area-assembly stressor) |
| `freiburg-forest` | 4.5 MB | 37 K  | 91  | 0  | inland **multipolygon-heavy** (Black-Forest forests/lakes with holes) |
| `freiburg-town`   | 8.2 MB | 57 K  | 142 | 0  | mid inland region, dense city |
| `freiburg`        | 157 MB | ~1.5 M | —  | 0  | the real target (repo root, `freiburg-regbez-260618.osm.pbf`) |

`features`/`polys w/ holes`/`coastlines` are from the Python oracle
(`obcm.ingest.ingest_osm` with `packer/config.json`); "polys w/ holes" =
polygons carrying interior rings = relation/area assembly producing holes.

## Provenance

- **tiny** — hand-authored [`tiny/tiny.osm`](tiny/tiny.osm), converted with
  `osmium cat`. Its header comment documents the expected per-element ingest
  result; it deliberately includes the closed-way area-assembly traps (see below).
- **monaco / malta** — Geofabrik extracts
  (`download.geofabrik.de/europe/{monaco,malta}-latest.osm.pbf`), reused from the
  web builder's PBF cache (`~/.cache/obcm/pbf`).
- **freiburg-forest / freiburg-town** — carved offline from the repo-root
  Freiburg pbf with `osmium extract` (bboxes in `build_corpus.sh`); no download.
- **freiburg** — the existing `freiburg-regbez-260618.osm.pbf` (157 MB target).

## Validation strategy (why not byte-identical)

The plan's original gold standard was byte-identical output vs. the Python
oracle. In practice that is **not** the gate, for two unavoidable reasons:

1. **GEOS version skew.** shapely links **GEOS 3.13.1**; the system/Homebrew
   `geos` the Rust `geos` crate binds is **3.14.1**. `simplify`/`intersection`
   differ in the last digits ⇒ different vertices ⇒ different bytes. Matching
   would mean building GEOS 3.13.1 from source — not worth it.
2. **Ordering & ring stitching.** Feature insertion order within a leaf and
   osmium's multipolygon ring start-points are not something the Rust path
   reproduces exactly.

So the **primary gate is feature-multiset equivalence + render-diff** (pixel
comparison via `obc-sim --png`), with every remaining difference *explained*.
Byte-identity is retained only as a **serializer-in-isolation** check: given the
exact same feature list + quadtree, serialization is deterministic integer work
and does match — a sharp test of `pack_feature`/`pack_chunk`/`serialize_tree`.

## Harness

`packer/tests/harness/` drives validation against this corpus:

- `dump_tree.py` — replicates `pack.py`'s pipeline, writing both a reference
  `.obcm` (via the oracle's `serialize_lods`) and a JSON dump of the exact
  quadtrees. Coordinates are dumped as **exact f64 bit patterns**, not decimal
  text, because decimal round-trip is lossy (serde_json can land 1 ULP off
  Python, flipping a `*1e6` halfway case).
- `run_stage1.sh` — the Stage-1 gate: re-serializes each dump with the Rust
  `serialize_from_dump` binary and asserts byte-identical output. Run it from
  anywhere; `WITH_LAND=1` also exercises the land path, `ITEMS="tiny monaco"`
  narrows the set.
- `dump_features.py` + `run_stage2.sh` — the Stage-2 gate: dumps each LOD's
  *pre-quadtree* simplified features, builds the quadtree in Rust
  (`build_from_features`), and compares to the Python quadtree's `.obcm`.
- `dump_ingest.py` + `compare_ingest.py` + `run_stage3_ingest.sh` — the Stage-3
  **ingest** gate: dumps the oracle's Stage-3-expected feature set (relations and
  closed-line-way blobs removed) and the Rust ingest (`ingest_dump`), then
  compares as a multiset (lines exact; polygons up to ring rotation/winding).
  *Superseded by the Stage-4 ingest gate now that the pipeline emits relations.*
- `dump_stage3_ref.py` + `run_stage3.sh` — the Stage-3 **end-to-end** gate:
  builds a Python reference restricted to the same Stage-3 set and compares to
  Rust `obc-pack` with `obcm_diff --canonical-polys`. *Superseded by Stage 4.*
- `run_stage4_ingest.sh` — the Stage-4 **ingest** gate: `dump_ingest.py
  --with-relations` keeps the relation-assembled polygons, so this validates the
  multipolygon assembly. Lines + coastlines must stay exact; a small **balanced**
  polygon residual (broken relations osmium and GEOS `build_area`/`node` repair
  into different vertex sets) is accepted and render-verified by `run_stage4.sh`.
- `find_divergences.py` + `compare_png.py` + `run_stage4.sh` — the Stage-4
  **end-to-end + render** gate: builds the `.obcm` both ways (Rust `obc-pack` and
  `dump_stage3_ref.py --with-relations`), hard-guards structural identity + zero
  line diffs via `obcm_diff`, then render-diffs each assembly divergence at the
  **finest (no-simplify) LOD** (`find_divergences.py` aims the camera at a boundary
  tip, forced to the finest LOD so it measures assembly, not the coarse-LOD
  simplify skew; `compare_png.py` pixel-diffs with PIL). PASS = divergences
  render-equivalent.
- `node_probe.py` + `node_probe` bin — throwaway coordinate-parity probe (kept as
  a regression guard for the `decimicro/1e7` lon/lat formula).
- `firmware/obc-pack` `obcm_diff` binary — the escalating comparator (structural
  + feature-multiset; `--canonical-polys` compares polygons up to ring
  rotation/winding) for the stages where byte-identity is not the gate.

Current status: **Stages 1 (serializer) and 2 (quadtree) pass byte-identical
across the whole corpus** (the Stage-1 reference also matches `pack.py` exactly,
verified with land on monaco). **Stage 3 (ingest: lines + closed ways) passes**:
ingest is multiset-identical to the oracle's Stage-3 set, and end-to-end the
output is structurally identical with **lines byte-exact and no-simplify LODs
exact** — the only divergence is the expected GEOS 3.14-vs-3.13 simplify skew on
polygons at the 12 m LOD. **Stage 4 (multipolygon relation areas) passes** the
whole corpus: ingest multiset-identical (lines + coastlines exact, relation
polygons up to ring winding with a small balanced re-tessellation residual), and
end-to-end structurally identical with zero line diffs and **every assembly
divergence render-equivalent at the finest LOD** (worst tile <0.8 %); the residual
again lives only at the simplify LODs.

## Known intentional divergence (a bug we do NOT replicate)

The Python oracle double-emits **closed line-ways**: osmium's `AreaManager`
builds a polygon for *every* closed way except `area=no`/multipolygon members,
and `ingest.py`'s `area()` emits it whenever the tags match a configured style —
even line styles like `highway=residential`. Meanwhile `way()` still emits the
line. So a closed residential loop becomes **both** a line and a filled polygon
(visible as a grey blob). The Rust port classifies closed line-ways as lines
only (polygon iff `area=yes` or an area tag, and not `area=no`; relations always
areas). The harness flags the oracle's surplus polygons as an intended
improvement, not a regression. `tiny.osm` pins this case (way 106).
