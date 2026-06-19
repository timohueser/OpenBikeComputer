# Stage 3 handover — OBCM packer Rust port (ingest)

> Read this top-to-bottom before writing code. It is self-contained: a fresh
> session should be able to start Stage 3 from here. Authoritative sources are
> linked throughout — when in doubt, the **Python oracle** (`packer/obcm/*`) and
> [`OBCM_Spec.md`](../../OBCM_Spec.md) win.

## 0. TL;DR

We are porting the Python OBCM packer (`packer/obcm/*`, driven by `packer/pack.py`)
to Rust, crate **`firmware/obc-pack`**. **Stages 1 (serializer) and 2 (quadtree)
are done and byte-identical to the Python pipeline across the whole corpus** (branch
`rust-packer-port`). Your job is **Stage 3: ingest — lines + closed ways only,
skipping multipolygon relations** (those are Stage 4). This is the first
*end-to-end* Rust pipeline (`.osm.pbf` → `.obcm`).

Verify the current state first:
```sh
cd firmware && cargo test -p obc-pack          # 13 tests, all green
packer/tests/harness/run_stage1.sh             # serializer: byte-identical x5
packer/tests/harness/run_stage2.sh             # quadtree:   byte-identical x5
```

## 1. The mission and the one hard constraint

Port the packer for speed (~8–15× projected) **without regressions**. The Python
pipeline is the trusted **oracle**; it is **never modified** — only read and
compared against. The full plan is [`rust-port-plan.md`](rust-port-plan.md);
**read its "Amendments" + "Progress" sections first** — they override the older
body where they conflict.

**Validation gate = feature-multiset + render-diff, NOT byte-identity** (see plan
Amendment 1). Byte-identity is impractical end-to-end (shapely links GEOS 3.13.1;
the Rust `geos` crate binds system GEOS 3.14.1; plus ordering/ring-winding). It
*happened* to hold for Stages 1–2 because clipping is stable across that GEOS bump
and the trees were fed Python-simplified geometry. **Stage 3 is where that luck
likely runs out** — simplify moves into Rust here (see §5.4).

## 2. What's already built (and proven)

Crate `firmware/obc-pack` (std, workspace member, depends on `obc-reader` + `geos`
+ `serde`/`serde_json`):

| file | what | status |
| :-- | :-- | :-- |
| `src/serialize.rs` | port of `serialize.py` — rounding, densify, delta/chunk/index/header | DONE, byte-exact |
| `src/quadtree.rs` | port of `quadtree.py` — split/insert/flatten/clip | DONE, byte-exact |
| `src/geom.rs` | `Geom` type + bounds + **GEOS bridge** (clip via `intersection`) | DONE |
| `src/dump.rs`, `src/feature_dump.rs` | serde bridges for the Stage 1/2 harness dumps | DONE |
| `src/bin/serialize_from_dump.rs` | Stage-1 harness binary | DONE |
| `src/bin/build_from_features.rs` | Stage-2 harness binary | DONE |
| `src/bin/obcm_diff.rs` | **reusable comparator**: structural + feature-multiset | DONE (may need extending, §6) |
| `src/main.rs` | the `obc-pack` CLI — **currently a stub**; you wire the real pipeline here | TODO |

Harness (Python side, `packer/tests/harness/`): `dump_tree.py` (Stage 1),
`dump_features.py` (Stage 2), `run_stage1.sh`, `run_stage2.sh`. Corpus +
manifest: `packer/tests/corpus/` ([README](../tests/corpus/README.md)) —
`tiny` (hand-authored, every ingest branch), `monaco`/`malta` (coastal +
MP-heavy), `freiburg-forest`/`freiburg-town` (inland), `freiburg` (target).
Rebuild with `packer/tests/corpus/build_corpus.sh` (offline).

## 3. The landmines (respect these — each cost real debugging)

1. **Coordinate f64 parity is everything, and it is fragile.** Two stored rules:
   - **Feature coords** round `int(round(v*1e6))` with **banker's rounding**
     (round-half-to-even). Rust: `f64::round_ties_even`, NOT `.round()`. Done in
     `serialize.rs`.
   - **Global bbox** uses `int(min*1e6)` — **truncation toward zero**, not
     rounding. You implement this in the new pipeline (`(v*1e6) as i64`).
   - **NEW for Stage 3 — node lon/lat from PBF.** osmium derives lon/lat as
     `int32_at_1e-7 / 1e7` (a single divide by 1e7). `osmpbf` may instead expose
     nanodegrees and tempt you into `(100*raw)*1e-9`, which differs in the last
     bit → different microdegree → different bytes. **This is the same class of
     bug as the Stage-1 "dump f64 as bits" issue.** *Before* trusting anything:
     dump `(node_id, lon.to_bits(), lat.to_bits())` for a sample from Python
     osmium and from Rust `osmpbf`, and assert equal. Match osmium's formula
     (likely `(node.nano_lon()/100) as f64 / 1e7`) until the bits agree.
2. **serde_json decimal round-trip is lossy** (1 ULP). The harness dumps coords as
   **u64 bit patterns**, never decimal text. Keep doing that.
3. **GEOS version (3.14 vs 3.13).** Clipping matched; **simplify may not.** Don't
   assume — measure (§5.4). When it diverges, that's expected → multiset gate.
4. **Ring winding/start.** osmium normalizes assembled-area ring orientation and
   may start a closed-way polygon at a different vertex than the raw way order.
   Lines are emitted in node order (so byte-exact), but **closed-way polygons may
   differ in vertex start/direction** → the multiset comparator (which keys on the
   exact vertex list) will flag geometrically-identical polygons. See §6.

## 4. The closed-way fix (decided; you implement it here)

The oracle has a **bug we do NOT replicate**: osmium's `AreaManager` builds a
polygon for *every* closed way except `area=no`/MP-members, and `ingest.py::area()`
emits it for any tag match — including line styles like `highway=residential` —
while `way()` still emits the line. So a closed residential loop becomes **line +
filled-blob polygon**. (`packer/tests/corpus/tiny.osm` way 106 pins this; its
header comment documents the whole truth table.)

In Rust there is no `AreaManager`, so you classify directly and the bug can't
arise: for each styled way, **emit a polygon iff it's an area, else a line** —
never both. Area test (mirror `ingest.py::way`'s `is_area`):
`area=yes`, OR (`area != no` AND has any of
`building|landuse|amenity|leisure|natural|waterway`). Open ways are always lines.

**Edge case to get right:** a closed way with both `admin_level` and an area-tag.
Python drops it entirely (`way()` skips the line as `is_area`; `area()` skips the
polygon on `admin_level`). So in Rust: if classified as area **and** `admin_level`
present → emit nothing.

## 5. Stage 3 scope, precisely

Build the real `obc-pack` CLI (replace the `main.rs` stub) reproducing
`pack.py`'s single-PBF path, MINUS multipolygon relations and MINUS land/merge
(deferred). Pieces, in order:

### 5.1 `config.rs` (deferred from Stage 1 — do this first)
Parse `config.json` with `serde_json` **`features = ["preserve_order"]`** (document
order is load-bearing). Mirror `config.py::assign_style_ids`: ignore any `id`,
number every feature type **1-based in document order**. Expose: the ordered
`(tag_key, {value → style})` map for first-match `_get_style`; the style list (id,
z_index default 0, color int-or-`"0x.."`, weight default 1, priority default 3,
min_lod default 0) for `pack_style_dict`; `lods` (`max_mpp`, `simplify`); `marker`
color; `chunk_size` (`config.get("chunk_size", 4096)`).

### 5.2 Ingest (`ingest.rs`) — the new work
`osmpbf` (0.3.8) + a **node-location store**. PBF is node-sorted, but be safe:
pass 1 collect `node_id → (lon,lat)` (start with a `HashMap<i64,(i32_1e7,i32_1e7)>`
or store the f64-bits; ~14 M nodes for Freiburg ≈ a few hundred MB — fine for now,
optimize in Stage 6); pass 2 resolve ways. **Skip relations.** Per styled way,
apply §4 classification. Capture `natural=coastline` ways into a separate
coastlines list **always** (even if closed/styled) — they feed bbox (and land,
later). Mirror `_get_style` (first tag_key in doc order whose value matches) and
the `>=2` / `>=3` node-count guards from `ingest.py` exactly.

### 5.3 Pipeline orchestration (in `main.rs`)
Mirror `pack.py`: ingest → bbox (`shapely.total_bounds` over features+coastlines,
then **truncate**) → (land: skip for now / `--no-land`) → per LOD i: keep features
with `min_lod ≤ i`, simplify to `simplify_m/111320.0`, `build_lod(...)` (Stage 2)
→ `serialize_lods` (Stage 1). Emit the stage strings the web builder scrapes
(plan §7) and honor `--chunk-size`. Keep the `OBC_PACK_BACKEND` flag idea (plan
§7): Python stays the default until a whole-corpus green.

### 5.4 Simplify (the first real divergence risk)
`geom.simplify(tol)` in shapely defaults to `preserve_topology=True` ⇒ GEOS
`TopologyPreservingSimplifier`. Use the geos crate's
**`Geometry::topology_preserve_simplify(tol)`** (NOT `simplify`, which is plain
Douglas–Peucker — different algorithm). **First task once you can simplify:
measure divergence** — simplify the same corpus geometry in shapely (3.13) and via
geos (3.14) at the real tolerances (`50/111320`, `12/111320`) and diff. If it
matches like clipping did, Stage 3 can stay byte-exact on relation-sparse regions;
if not, the multiset/render gate is why it exists.

## 6. Stage 3 validation plan

You can't compare end-to-end against `pack.py` directly: Rust intentionally omits
relation polygons (Stage 4) and the closed-line-way blobs (§4). Two layers:

1. **Ingest-only (isolates the new code, should be near-exact).** Add
   `dump_ingest.py`: run `ingest_osm`, dump each feature with **source metadata**
   — `way`/`relation` (osmium `area.from_way()`/`orig_id()`), and for way-areas
   the tags needed to classify. Compute the "Stage-3 expected set" = lines +
   genuine closed-way-area polygons (drop relation-sourced, drop line-type
   closed-way polygons, drop `admin_level`+area edge). Compare to Rust ingest as a
   multiset of `(style_id, kind, microdeg-rounded vertices)`. Lines should be
   **exact**; closed-way polygons may differ only in ring start/winding (see §3.4).
2. **End-to-end (`.obcm`).** Build a Python reference restricted to the same
   Stage-3 set (reuse the harness: filter features, then quadtree+serialize) and
   compare to Rust's `obc-pack` output with `obcm_diff`. Expect: structural match;
   multiset match modulo (a) simplify last-digits (§5.4) and (b) closed-way ring
   start/winding.

**Likely harness work:** extend `obcm_diff` to optionally compare polygons
**up to ring rotation + reflection** (canonicalize: rotate exterior to its
lexicographically-min vertex, and/or compare as sets) so winding-only differences
don't drown the signal. Keep the strict mode too.

## 7. Concrete first steps (ordered)

1. `config.rs` + a unit test that the corpus `config.json` assigns the expected
   style ids (cross-check against `python -c "from obcm.config import load_config; ..."`).
2. **Node-coordinate parity probe** (§3.1) — a throwaway: Rust `osmpbf` lon/lat
   bits vs Python osmium lon/lat bits on `tiny` + a sample of `monaco`. Do NOT
   proceed until they match.
3. `ingest.rs`: lines + closed-way classification (§4), coastlines, `_get_style`.
   Unit-test against `tiny.osm` (known truth table in its header comment).
4. `dump_ingest.py` + ingest-only multiset comparison (§6.1) on tiny/monaco/malta.
5. Wire `main.rs` pipeline (§5.3) with simplify (§5.4); run the simplify-divergence
   probe; then end-to-end compare (§6.2). Add `run_stage3.sh`.
6. Update plan Progress + corpus README + the `rust-port-progress` memory; commit.

## 8. Risks & open questions

- **Node store memory** for Freiburg (14 M nodes). HashMap is fine for correctness;
  flag for Stage 6 if it bites.
- **osmium vs osmpbf coordinate formula** (§3.1) — the gating risk; probe first.
- **Closed-way polygon winding** (§3.4) — may force the §6 comparator extension.
- **Invalid geometries.** shapely/GEOS may handle a self-touching closed way that
  `geos` errors on (or vice-versa). `geom.rs` currently `expect()`s GEOS calls;
  watch for panics on real data and decide whether to skip-and-warn (the oracle
  would raise — but it apparently doesn't on the corpus).
- **Does simplify diverge?** Unknown until measured (§5.4). This determines whether
  Stage 3 is "byte-exact on relation-sparse" or "multiset-equivalent".

## 9. Reference index

- Oracle (read these): `packer/obcm/ingest.py` (the spec for §4/§5.2),
  `config.py`, `quadtree.py`, `serialize.py`, `pack.py` (orchestration), and
  `tests/test_ingest.py`.
- Format: [`OBCM_Spec.md`](../../OBCM_Spec.md). Plan: [`rust-port-plan.md`](rust-port-plan.md)
  (esp. §4 reproduction checklist, §6 land/merge scope, §7 integration contract).
- Reader (the format truth, reads what you write): `firmware/obc-reader/src/reader.rs`.
- Corpus + validation strategy: `packer/tests/corpus/README.md`.
- Fixture truth table: `packer/tests/corpus/tiny/tiny.osm` (header comment).
