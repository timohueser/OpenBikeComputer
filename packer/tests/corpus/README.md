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
- `firmware/obc-pack` `obcm_diff` binary — the escalating comparator (structural
  + feature-multiset) for the later stages where byte-identity is not the gate.

Current status: **Stage 1 (serializer) passes byte-identical across the whole
corpus**, and the harness reference matches `pack.py`'s own output exactly
(verified with land on monaco).

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
