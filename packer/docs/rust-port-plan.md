# Full Rust port of the OBCM packer — implementation plan

> **Status:** proposed. Try this first. If area assembly proves intractable,
> fall back to [`hybrid-pyo3-plan.md`](hybrid-pyo3-plan.md) and revisit the full
> port later.
>
> **Prime directive: no regressions in the ingest pipeline.** The current
> Python + libosmium pipeline is stable and trusted. This port is only worth
> shipping if its output is provably equivalent to the Python output. The Python
> pipeline is **never deleted and never modified** during this work — it remains
> the reference *oracle* that every Rust build is validated against, and the
> web builder keeps an instant fall-back to it until the Rust path is proven on a
> whole corpus of regions.

## Amendments (decided during implementation — 2026-06-19)

These supersede the relevant parts of §4/§5/§9 below; the rest of the plan stands.

1. **Validation gate is render-diff + feature-multiset, NOT byte-identical.**
   Two unavoidable blockers: (a) shapely links **GEOS 3.13.1** but the system
   `geos` the Rust crate binds is **3.14.1** → simplify/intersection diverge in
   the last digits; (b) feature insertion order and osmium ring start-points are
   not reproduced exactly. So the switchover gate (§5) becomes: **feature
   multiset matches per LOD, renders are pixel-identical (or every diff
   explained).** Byte-identity is kept only as a *serializer-in-isolation*
   diagnostic — given the same feature list + tree, serialization is
   deterministic and does match. The GEOS-version bail signal in §9 is therefore
   void: 3.14.1 is fine.

2. **We fix a real oracle bug instead of replicating it (Rust-only).** The Python
   pipeline double-emits **closed line-ways**: osmium's `AreaManager` builds a
   polygon for every closed way except `area=no`/MP-members, and `ingest.py`'s
   `area()` emits it for any tag match — including line styles like
   `highway=residential` — while `way()` still emits the line. The Rust port
   classifies a closed way as a polygon **iff** `area=yes`, or it has an area tag
   (`building|landuse|amenity|leisure|natural|waterway`) and not `area=no`;
   relations are always areas. Python is left untouched; the harness reports the
   oracle's surplus polygons as an *intended improvement*. This also simplifies
   Rust area assembly (it never builds polygons for line-ways).

3. **Corpus is chosen and built** — see `packer/tests/corpus/` (manifest +
   `build_corpus.sh`): `tiny` (hand-authored, every ingest branch), `monaco`
   (small coastal), `malta` (coastal + multipolygon-heavy), `freiburg-forest`
   (inland MP-heavy), `freiburg-town` (mid), `freiburg` (target). Coastal and
   multipolygon coverage are present from day one as the planner required.

### Progress

- **Stage 1 (scaffold + serializer + harness): DONE.** New crate
  `firmware/obc-pack` (workspace member, depends on `obc-reader`):
  `serialize.rs` ports `serialize.py` faithfully (banker's rounding, densify,
  delta/chunk/index/header layout); `obcm_diff` is the reusable structural +
  feature-multiset comparator; `serialize_from_dump` + the Python
  `tests/harness/dump_tree.py` form the byte-parity gate (`run_stage1.sh`).
  **Byte-identical to `pack.py` across the entire corpus** (tiny → 10 MB malta,
  with holes/16-bit deltas/densification/land). 7 unit+integration tests green.
- **Stage 2 (quadtree): DONE.** `quadtree.rs` ports `quadtree.py` (size-based
  split, NW/NE/SW/SE floor-div midpoints via `div_euclid`, containment fast-path
  vs GEOS clip, multi-geom flatten, re-insert on split); `geom.rs` is the GEOS
  bridge (`geos` crate v11 → system GEOS 3.14.1, `intersection` like shapely, not
  `clip_by_rect`). Gate `run_stage2.sh` (Python `dump_features.py` →
  `build_from_features`): **byte-identical to the Python quadtree across the
  whole corpus** — GEOS 3.14 and shapely's 3.13 happen to clip identically on
  this data, so it beat the render+multiset bar. 6 quadtree unit tests
  (`test_quadtree.py` cases) green.
- **Stage 3 (ingest, common case): DONE.** First end-to-end Rust pipeline
  (`.osm.pbf` → `.obcm`), branch `rust-packer-port`. New modules: `config.rs`
  (style-IDs 1-based in **document order** via `serde_json` `preserve_order`),
  `ingest.rs` (`osmpbf` 0.3.8 two-pass — node-location `HashMap`, then ways —
  with the closed-way classification fix of Amendment 2), `main.rs` (the real
  `pack.py` single-PBF pipeline minus relations + land/merge), plus
  `geom::topology_preserve_simplify` and `geom::polygon_is_valid`.
  - **Node-coordinate parity (the gating risk) is solved:** osmium derives
    lon/lat as `decimicro / 1e7`; `osmpbf`'s own `.lon()` is `1e-9*nano_lon()`
    and differs in the last bit. Using `node.decimicro_lon() as f64 / 1e7`
    (division by the exact integer, not `*1e-7`) is **bit-identical** to osmium
    across tiny + monaco + malta (`node_probe` bin / `node_probe.py`).
  - **Closed-way fix + invalid geometry:** a closed way is a polygon iff
    `is_area` (and not `admin_level`), else a line — never both. We additionally
    **skip closed-way polygons whose ring is invalid** (GEOS `is_valid`), because
    osmium's assembler yields no ring for them (e.g. malta's self-intersecting
    "Red House" way 368715930) — this made the ingest multiset exact.
  - **Ingest gate (`run_stage3_ingest.sh`): multiset-identical to the oracle's
    Stage-3-expected set across the whole corpus** (lines exact; closed-way
    polygons up to ring rotation/winding). `dump_ingest.py` observes the real
    `OSMHandler` (via a provenance subclass) and drops relation- and
    closed-line-way-sourced polygons; `compare_ingest.py` does the multiset diff.
  - **End-to-end gate (`run_stage3.sh`, `dump_stage3_ref.py`,
    `obcm_diff --canonical-polys`): PASS.** Structural identical; **lines
    byte-exact at every LOD**; **no-simplify LODs (0 m, 50 m) match exactly**
    (polygons up to ring winding). The **only** divergence is the predicted
    GEOS 3.14-vs-3.13 `TopologyPreservingSimplifier` skew, and it appears
    **only at the 12 m LOD on polygons** (~5–35 % of that LOD's polys, vertex
    level) — the answer to §5.4's "does simplify diverge?": yes, narrowly and
    benignly, lines unaffected. This is exactly the multiset gate's reason to
    exist (Amendment 1).
  - **Freiburg smoke test:** 150 MB / ~14 M nodes runs in **33 s** (release,
    single-thread) at **1.48 GB** peak RSS — vs the Python pipeline's ~227 s, so
    ~7× even before relations/land and before Stage-6 threading. Node-store
    memory (risk §8) is fine; flag for Stage 6.
- **Next — Stage 4 (multipolygon relation area assembly):** the hard sub-project
  (§8.4). Stage 3 intentionally omits relations; `obcm_diff --canonical-polys`
  and the provenance harness are ready to validate the added relation polygons.
  **→ Full briefing: [`stage4-handover.md`](stage4-handover.md)** (start here):
  recommends GEOS `polygonize`/`build_area` for ring assembly, flips the harness
  filter to keep relation polygons, and adds an `obc-sim --png` render-diff.

## 0. Context (measured)

Profiling (see `[[obcm-converter-rust-rewrite]]` memory) on Freiburg (157 MB pbf,
1.54 M features): total ~227 s single-thread, of which **~75 % is CPython +
shapely binding overhead, not real computation**. Per stage: ingest 67 s
(46 s libosmium + 21 s pure shapely construction), quadtree 108 s (only ~15 %
GEOS), serialize 49 s (~0 % GEOS). Projected Rust: **~8× single-thread,
~15× threaded**.

The *only* genuinely hard part is OSM multipolygon-relation **area assembly**
(libosmium's `AreaManager` has no mature Rust equivalent). Everything else is
easy-to-moderate and largely already specified.

## 1. Current layout this plan targets

```
packer/
  pack.py                 # CLI: pack.py <pbf...> <config.json> <out.obcm> [--chunk-size N]
  config.json             # feature selection + styling + LOD tiers
  obcm/                   # the Python packing library (the ORACLE)
    config.py             #   style-ID assignment (1-based, document order)
    ingest.py             #   osmium 2-pass -> shapely features (+ coastlines)
    quadtree.py           #   per-LOD quadtree build + clip
    serialize.py          #   features -> .obcm bytes
    land_ingest.py        #   coastline/land polygons (fiona shapefile + reproject)
  tests/                  # pytest: test_{config,ingest,quadtree,serialize}.py
  web_builder/
    jobs.py               # shells out to pack.py, scrapes stdout stage strings
firmware/                 # Rust workspace (no_std reader/renderer + std sim)
  obc-reader/             # OBCM v5 READER — the format source of truth + oracle
    tests/format.rs       #   already contains a hand-written encoder mirroring serialize.py
  obc-sim/                # std host binary, renders .obcm (obc-sim --png for render-diff)
OBCM_Spec.md              # the binary format spec (v5)
```

## 2. Where the new code lives

Add one crate: **`firmware/obc-pack/`** (a std `lib` + `bin`), a member of the
`firmware/` workspace.

- It depends on **`obc-reader`** (no_std, but compiles fine under std) so the
  writer and reader share one definition of the format and are tested together.
- Yes, "firmware" holding a desktop converter is a slight misnomer, but the
  workspace already hosts a std host binary (`obc-sim`) and benches, and the
  `obc-reader` dependency edge is worth it.
- *Alternative if you'd rather keep `firmware/` embedded-only:* a sibling
  `packer-rs/` workspace that path-depends on `firmware/obc-reader`. Either is
  fine; pick one and note it. (Recommendation: `firmware/obc-pack`.)

Binary name `obc-pack`, same CLI contract as `pack.py` (§7).

## 3. Dependencies (chosen for correctness first, speed second)

| Need | Crate | Why / correctness note |
| :-- | :-- | :-- |
| Read .osm.pbf | `osmpbf` (b-r-u) | Mature, fast, lazy-decode, parallel (`par_map_reduce`). Read-only — node/way/relation iteration + a node-location store. |
| Geometry ops (simplify, clip/intersection) | **`geos`** (georust) | Binds the **same libGEOS shapely uses**. This is the single most important correctness lever — it makes simplify and intersection results match shapely. **Do not** use the pure-Rust `geo` crate for these: its Douglas–Peucker tie-breaking differs and will diverge the geometry. |
| (already) format structs | `obc-reader` | Round-trip oracle for tests; shared constants. |

**GEOS version pin.** Confirm the system libGEOS that the `geos` crate links is
the same version shapely links (`python -c "import shapely; print(shapely.geos_version)"`).
If they differ, simplify/clip can diverge in the last digits. Document the
required GEOS version; on macOS pin via Homebrew, in CI install the matching one.

## 4. The reproduction checklist — every Python behavior the Rust MUST match

This is the heart of "no regressions." Treat each as a test case. (Spec
references: `OBCM_Spec.md`; oracle: `packer/obcm/*`.)

1. **Style-ID assignment** — `config.py::assign_style_ids`: ignore any `id` in
   config; number every feature type **1-based, in document order**. Style table
   is sorted by id (`serialize.py::pack_style_dict`), packed `<BbHBB>` with
   `flags = (priority-1) & 0x03`, priority clamped to 1..4. Marker color from
   `config["marker"]["color"]` (default `0xF800`), accepts int or `"0x…"`.

2. **Feature selection + way/area disambiguation** — `ingest.py`:
   - `_get_style`: first matching `(tag_key, value)` in config document order.
   - Coastlines: `natural=coastline` ways captured as `LineString` separately
     (used for sea generation), **always**, even if also styled.
   - Closed-way-is-area heuristic: a closed way is treated as an **area** (and
     therefore skipped in `way()` so it isn't double-counted) iff
     `area=yes`, **or** (`area!=no` **and** it has any of
     `building|landuse|amenity|leisure|natural|waterway`). Otherwise it is a
     line (circular road).
   - `area()`: skip anything with `admin_level` (those are handled as lines
     only). Polygons get closed (first==last), interiors with ≥3 pts kept &
     closed.

3. **Coordinate integerization — TWO DIFFERENT RULES (classic regression trap):**
   - **Feature coordinates** (`serialize.py::pack_feature`, `_densify`):
     `int(round(v * 1e6))` → Python `round()` is **banker's rounding
     (round-half-to-even)**. Rust `f64::round()` is round-half-**away**-from-zero.
     You must implement round-half-to-even, or coordinates land 1 µdeg off on
     `.5` boundaries → different deltas → different bytes.
   - **Global bbox** (`pack.py`): `int(min_lon * 1e6)` → **truncation toward
     zero** (`int()`), *not* rounding. Replicate truncation here, banker's-round
     for coords. They are deliberately different.

4. **Simplify** — `quadtree.py`: `geom.simplify(simplify_m / 111320.0)`.
   shapely's `.simplify()` defaults to **`preserve_topology=True`** ⇒ GEOS
   `TopologyPreservingSimplifier`, **not** plain Douglas–Peucker. Use the geos
   crate's **topology-preserving** simplify (`Geom::topology_preserve_simplify`),
   not `simplify`. Easy to get wrong; different algorithm entirely.

5. **Long-segment densification** — `serialize.py::_densify`: any segment with
   `max(|dx|,|dy|) > 30000` µdeg gets intermediate vertices: `steps =
   max_dist // 30000 + 1`, then for `step in 1..steps`, point at
   `int(round(p1 + delta * step/steps))`. Exact integer stepping changes vertex
   counts and bytes.

6. **Quadtree** — `quadtree.py`, cross-checked by `obc-reader/src/reader.rs`
   (`walk_leaves`):
   - Split when `current_size > chunk_size`, where size accrues
     `12 + pt_count * 4` per inserted feature (header + ~4 B/pt).
   - Midpoints by **floor division** `(min+max)//2` on both axes (`div_euclid(2)`
     in Rust to match Python `//` for negatives).
   - Child order **NW, NE, SW, SE**; recursion guard: don't split if
     width or height < 10 µdeg.
   - On split, existing features are re-inserted into children (replicate; it
     affects which leaf a straddling feature lands in).
   - Containment fast-path: if a feature's bounds are fully inside the node,
     insert without clipping; else clip via GEOS `intersection` with the node
     box and recurse on the result. Match this decision exactly.
   - Multi-geometry flattening (`_flatten_and_process`): `LineString/LinearRing/
     Polygon` go straight in; `MultiLineString/MultiPolygon/GeometryCollection`
     are split into parts (non-empty only). Points/other are dropped.
   - **Feature order within a leaf = insertion order**; chunk order = **BFS**
     over the tree (`serialize.py::serialize_tree`). Both determine byte output.

7. **Serialization** — `serialize.py` + `OBCM_Spec.md`, already mirrored by
   `obc-reader`: anchor = exterior first point relative to leaf node min corner;
   8-bit deltas iff `max_delta ≤ 127` else 16-bit (flag 0x01); polygon flag
   0x02, holes flag 0x04 + hole count + per-hole pt-count; `0xFF` end-of-features
   sentinel; chunk padded to `chunk_size` with `0xFF`; per-LOD index uses
   `0x7FFFFFFF` (empty leaf) and `0x80000000 | first_child` (branch). Header
   `<4sBiiiiIBIH>`, **bbox field order lat,lon,lat,lon**, LOD table `<fIIHI>`
   with coarsest `max_mpp = +inf`.

8. **Land polygons** — `land_ingest.py`: read `land_polygons.shp` (EPSG:3857),
   filter by bbox, clip to bbox, reproject 3857→4326 (pyproj/PROJ). See §6 — keep
   this in Python initially to avoid a PROJ-parity rabbit hole for a 2.5 s stage.

9. **Multi-PBF merge** — `pack.py`: `osmium merge` + `osmium sort` via
   subprocess before ingest. Keep shelling out to the `osmium` CLI (cheap,
   battle-tested) rather than reimplementing.

## 5. Validation harness — build this FIRST, before optimizing

Test-driven port. The harness is the real deliverable; the speed falls out.

**Corpus.** A handful of `.osm.pbf` of increasing size/complexity, checked into a
fixtures dir or a documented download list:
- a tiny hand-authored extract (instant, unit-level);
- a small town (fast iterate);
- a mid region;
- **Freiburg** (the real 157 MB target);
- at least one **coastal** extract (exercises coastline/land) and one
  **multipolygon-heavy** extract (forests/lakes with holes, relation areas).

**Oracle.** `packer/pack.py <pbf> packer/config.json ref.obcm` for each.

**Escalating comparisons** (a `cargo test` + a driver script):
1. **Structural** — parse both with `obc-reader`: identical header (bbox,
   marker, version), identical style table, and per-LOD identical `node_count`,
   `chunk_count`, `chunk_size`, `max_mpp`.
2. **Feature multiset** — decode every chunk of every LOD in both; assert the
   set of `(style_id, vertex-list)` matches per LOD. Prefer exact order;
   investigate any ordering difference (it usually reveals a real divergence).
3. **Byte-identical** — `a.obcm == b.obcm`. The gold standard, achievable if the
   §4 checklist is fully matched. **Any byte diff is a bug**: bisect it to the
   first differing offset, map it back to a stage, fix the behavior. Don't
   "tolerate" diffs you can't explain.
4. **Render-diff** — `obc-sim --png` both files at several viewports/zooms
   (one per LOD), assert pixel-identical (or within a tiny, understood
   threshold). This is the semantic backstop and the device-truth check: if
   bytes differ but renders are identical, you at least have no visible
   regression while you root-cause.

**Switchover gate:** byte-identical across the *entire* corpus — or
render-identical with every byte diff explained and judged benign. Nothing
ships to the web builder before this.

## 6. Scope decision: keep land + merge in Python initially

Land generation (2.5 s, fiona + PROJ reproject) and multi-PBF merge
(`osmium` CLI) are a tiny fraction of runtime and a large fraction of
fiddly-parity risk (PROJ last-digit reproduction, shapefile clipping). Recommend
the Rust binary **shells out** to the existing Python `land_ingest`/`osmium` for
these at first (or consumes a precomputed land file), and focuses Rust effort on
the ~96 % that is ingest + quadtree + serialize. Port them later only if you
want a zero-Python binary.

## 7. Integration contract (must stay drop-in)

`web_builder/jobs.py` builds:
`[python, -u, pack.py, *pbfs, config_json, out_obcm, --chunk-size, N]`, runs it
with `cwd=packer/`, captures stdout, and maps **stage strings** to UI phases via
`_STAGE_MARKERS` (`"Merging"`, `"Pass 1"/"Pass 2"`, `"Calculating BBox"`,
`"Generating land"`, `"Building Quadtree"`, `"Serializing"`, `"Writing"`); it
splits on `\n` and `\r` so `tqdm` bars surface as transient lines.

The Rust binary must therefore:
- accept the **same positional CLI** + `--chunk-size`;
- **emit the same stage strings** on stdout (cheapest: print the existing marker
  phrases at each stage; progress bars optional) — or update `_STAGE_MARKERS` to
  new strings in the same PR;
- exit `0` and write the output file on success.

Switch the backend behind an env flag, e.g. `OBC_PACK_BACKEND=python|rust`
(default `python` until proven), so the web builder can fall back instantly.

## 8. Staged execution (each stage independently validated)

1. **Scaffold + validation harness + serializer.** Implement serialize first
   (trivial; crib the encoder already in `obc-reader/tests/format.rs`).
   Byte-match against Python `serialize_lods` on captured feature sets and the
   cases in `packer/tests/test_serialize.py`.
2. **Quadtree.** Port the algorithm; validate index + chunk layout against
   Python on the `test_quadtree.py` synthetic cases, then on real features.
3. **Ingest — lines + closed ways only** (skip multipolygon *relations*).
   Now an end-to-end Rust pipeline that should byte-match Python on
   relation-sparse regions. Bank this win; you always have a working subset.
4. **Ingest — multipolygon relation area assembly.** The hard sub-project,
   budget the most time. Start from closed-way rings; add relation ring
   stitching (group member ways, join on shared endpoints, classify outer/inner
   by containment/area). If exact libosmium parity is infeasible, two escape
   hatches: (a) use GEOS `polygonize` on noded ways to build rings; (b) accept
   **render-equivalence** (not byte) for relation areas, documented. Validate on
   the multipolygon-heavy corpus with the render-diff backstop.
5. **Land + merge.** Keep Python/`osmium` (§6) or port; validate coastal corpus.
6. **Parallelize — only after correctness is green.** Parallel pbf decode;
   per-LOD quadtree + serialize across threads. Threading can reorder features →
   byte diffs: enforce determinism (stable sort, deterministic feature order)
   and **re-run the full corpus** after each change.
7. **Integration + gated switchover.** Wire `obc-pack` into `jobs.py` behind the
   env flag; keep Python default; flip to Rust only after the corpus is green.

## 9. When to abandon to the hybrid

Bail signals (→ `hybrid-pyo3-plan.md`, keeping Python ingest untouched):
- Multipolygon area assembly can't reach byte- or render-parity in a reasonable
  time (broken/edge-case geometries are the usual culprit).
- System GEOS can't be matched to shapely's, so simplify/clip diverge
  irreducibly.
- The schedule slips: the hybrid already captures ~3× for ~⅓ the effort with
  the ingest you trust left fully intact.

## 10. Effort

~2–3 weeks. Harness + serialize + quadtree: a few days. Ingest common-case: a
few days. **Area assembly: the bulk and the risk.** Parallelization +
integration: a few days. Correctness work (the §4/§5 traps) is the real cost,
not the line count.
