# Stage 5 handover — OBCM packer Rust port (land generation + multi-PBF merge)

> **STATUS: DONE** (2026-06-19, branch `rust-packer-port`). `obc-pack` now runs the
> **full** `pack.py` pipeline natively — multi-PBF merge → ingest → relations →
> **land** → quadtree → serialize — and is a **zero-Python binary** (it shells out
> only to the `osmium` CLI for merge, as the plan always intended). The whole
> coastal/inland corpus passes the Stage-5 gate (`run_stage5.sh`). Outcome is
> recorded in [`rust-port-plan.md`](rust-port-plan.md) (Progress → Stage 5). The
> notes below are the record + the Stage-6 briefing.

## 0. What shipped

Two pieces of `pack.py` that Stages 3–4 deferred:

1. **Land generation** — `src/land.rs`, a native port of `obcm/land_ingest.py`
   (replaces `fiona` + `pyproj` + `shapely`). Clips the global
   [`land-polygons-split-3857`](https://osmdata.openstreetmap.de/data/land-polygons.html)
   shapefile to the map bbox and emits the faces as `natural.land`-styled polygons.
2. **Multi-PBF merge** — `src/main.rs`, `osmium merge` + `osmium sort` via
   `std::process::Command` (plan §4.9/§6 — keep the battle-tested CLI), with
   self-deleting temp files, exactly mirroring `pack.py`.

The §6 "keep land in Python initially" default was **overridden by the user** in
favour of the full native port. It turned out to be low-risk (see §2).

## 1. The land port, precisely (`src/land.rs`)

`get_land_polygons(bbox_deg) -> Vec<Geom>` (one `Geom::Polygon` per face):

- **Shapefile read** — the `.shp` polygon-record format is parsed directly with a
  **per-record MBR skip**: read each record's 8-byte header + shape-type + 32-byte
  bounding box, and `seek_relative` past the body unless the MBR meets the query
  box. A 1 MB `BufReader` keeps the scan ~one sequential pass. This matches GDAL's
  bbox filter (the dataset has **no** `.qix`/`.sbn` spatial index, so GDAL also
  scans record MBRs). Single-ring records take a no-GEOS fast path; multi-ring
  records (holes / disjoint outers) go through GEOS `build_area` (even-odd nesting,
  the Stage-4 primitive) — robust to ring winding.
- **Clip** — GEOS `intersection` against the box built from the forward-projected
  bbox corners (shapely `box(...)` ring order), in **3857, before reprojecting** —
  the exact oracle order. A record fully inside the box skips the clip (the oracle's
  `intersection` of a contained polygon returns it unchanged).
- **Reproject** — closed-form spherical Web Mercator (3857 here is the **auxiliary
  sphere**, `R = 6378137`, per the `.prj`). `merc_inverse`/`merc_forward` match
  `pyproj`'s EPSG:3857 to **2.1e-14°** (verified) — far below the µdeg quantum, so
  no PROJ. This killed the "last-digit reproduction" risk the plan flagged.
- **Dataset cache** — `~/.cache/obcm/land/land-polygons-split-3857/` (same as the
  oracle). Missing ⇒ download via `curl` + `unzip`. The oracle's `Last-Modified`
  freshness check is **intentionally dropped** (a HEAD-request optimization the
  oracle itself skips on any network error); delete the cache dir to force a
  refresh.

Exposed from `geom.rs` for reuse: `ring_to_coordseq`, `geom_from_geos`,
`collect_polygons` (all `pub(crate)`). `config.rs` gained `Config::land_style()`
(`config["features"]["natural"]["land"]`). `main.rs` appends the faces to
`ingested.features` after the bbox step and prints the `Generating land` /
`Merging` stage markers the web builder scrapes.

## 2. Why the port was safe (the validation story)

Gate is render + multiset, **not** byte-identity (the established philosophy).
`run_stage5.sh` per item (monaco, malta, freiburg-town) does three things:

1. **Land parity, isolated from the quadtree** — `land_probe` (Rust bin) vs
   `dump_land.py` (oracle), compared by `compare_land.py` on **total area**. Result:
   area matches to **1.3e-12 … 5.2e-11** relative with **identical vertex counts**
   (monaco 9131, malta 13430). The land geometry is essentially **bit-exact** to the
   oracle — the closed-form reproject + GEOS-3857 clip reproduce pyproj/shapely.
2. **Header identity** — `obcm_diff` version/bbox/marker/style must match (a diff
   there is a real bug; the script greps `^DIFF` lines that aren't `node_count` /
   `chunk_count`).
3. **Render-equivalence** — whole-map overview + coastal fine-LOD tiles, all
   < 0.01 % differing pixels (monaco/malta **0 %**). Plus a **merge** check:
   `obc-pack a a` (osmium dedupes the file with itself) render-matches the single
   build.

**The one documented residual:** on dense items the end-to-end `obcm_diff` reports
`structural_ok=0` — node counts differ ~0.6–1 % (malta) with nonzero line/poly
diffs. This is **not** a land bug: the land geometry is bit-exact (point 1). Land
*adds density*, so the **pre-existing GEOS 3.14-vs-3.13 simplify + relation-assembly
skew** (Stage 3/4 residual) tips a few near-threshold quadtree splits, re-clipping
features at different leaf boundaries. LOD0 feature counts stay **equal** with
**balanced** only-in-A/only-in-B (zero net loss), and every render is pixel-
identical. So Stage 5's gate is header identity + land-area parity + render-
equivalence, **not** `structural_ok=1` (which Stage 4 could afford because it added
no metric-space clip). This is the pre-authorized render+multiset outcome.

## 3. Files

| file | what |
| :-- | :-- |
| `src/land.rs` | the land port (shapefile read + MBR skip + clip + reproject + cache) |
| `src/bin/land_probe.rs` | dump `get_land_polygons(bbox)` as JSON for the parity gate |
| `src/main.rs` | merge (osmium subprocess + `TempPath`) + land wired into the pipeline |
| `src/config.rs` | `Config::land_style()` |
| `src/geom.rs` | `pub(crate)` exposure of `ring_to_coordseq` / `geom_from_geos` / `collect_polygons` |
| `tests/harness/dump_land.py` | oracle land dump (flattens MultiPolygons to faces) |
| `tests/harness/compare_land.py` | land area/count/vertex comparator |
| `tests/harness/run_stage5.sh` | the Stage-5 gate (land parity + header + render + merge) |

Reproduce: `packer/tests/harness/run_stage5.sh` (PASS across monaco/malta/
freiburg-town + merge). Stage 4 still green (`run_stage4.sh`, `--no-land`).

## 4. Next — Stage 6 (parallelize + node-store memory)

The remaining gap is **performance**, explicitly deferred to here:

- **Node store** dominates RSS: freiburg is 2.2 GB (a `HashMap<i64,(i32,i32)>` over
  ~14 M nodes). Shrink it (sorted `Vec` + binary search, or a compact id→coord
  index); this is risk §8 in the plan.
- **Parallelize** the per-LOD quadtree build (the LODs are independent) and/or the
  ways pass. Profiling baseline: freiburg **37 s** (ingest-bound), single-thread.
- **Land scan** is ~4 s (one sequential pass over the 1.3 GB `.shp`). A cheap win:
  pre-filter by reading the `.shx` + record MBRs in a tighter loop, or memory-map.
  Low priority vs the node store.
- **Do not** chase `structural_ok=1` end-to-end — it's the documented GEOS-version
  skew (§2), render-equivalent. Keep the render gate as truth.

**Integration (plan §7) is now DONE** (ahead of Stage 6, by request):
`web_builder/jobs.py` runs the native `obc-pack` by default
(`OBC_PACK_BACKEND=rust`, `python` to fall back; auto-fallback if unbuilt), and
python-vs-rust is render-identical on the user's 5-LOD `user_config.json`. See the
plan's Progress → "Integration (§7)". So Stage 6 is the only remaining work.
