# Stage 4 handover — OBCM packer Rust port (multipolygon relations)

> **STATUS: DONE** (2026-06-19, branch `rust-packer-port`). Relations are assembled
> via GEOS `build_area` (even-odd holes) with a `node()` repair fallback and an
> osmium-matching completeness rule; whole corpus passes the ingest multiset gate
> (`run_stage4_ingest.sh`) and the end-to-end + render gate (`run_stage4.sh`).
> Outcome + the few divergences are recorded in
> [`rust-port-plan.md`](rust-port-plan.md) (Progress → Stage 4). The notes below
> are the original briefing, kept for context.

> Read this top-to-bottom before writing code. It is self-contained: a fresh
> session should be able to start Stage 4 from here. Authoritative sources are
> linked throughout — when in doubt, the **Python oracle** (`packer/obcm/*`) and
> [`OBCM_Spec.md`](../../OBCM_Spec.md) win. Stage 3's record is
> [`stage3-handover.md`](stage3-handover.md); the running plan + outcomes are in
> [`rust-port-plan.md`](rust-port-plan.md) (read its **Amendments + Progress**).

## 0. TL;DR

Stages 1–3 are **done** on branch `rust-packer-port`: the Rust `obc-pack` crate
ingests `.osm.pbf` → `.obcm` end-to-end for **lines + closed ways**, validated
against the Python oracle (serializer + quadtree byte-identical; ingest
multiset-identical; end-to-end lines byte-exact + no-simplify LODs exact, only a
benign GEOS-version simplify skew on 12 m-LOD polygons). **Stage 3 deliberately
skips multipolygon *relations*.**

**Your job — Stage 4: assemble area polygons from `type=multipolygon` /
`type=boundary` relations** (lakes-with-islands, multi-part forests, etc.) and
emit them alongside the Stage-3 features. This is **the hard sub-project** the
plan flagged (§8.4, §0): there is no mature Rust equivalent of libosmium's
`AreaManager`. Budget the most time here; lean on the validation harness (built)
and the GEOS `polygonize` escape hatch.

Verify current state first:
```sh
cd firmware && cargo test -p obc-pack                 # 13 lib + 5 integ, all green
packer/tests/harness/run_stage1.sh                    # serializer byte-identical
packer/tests/harness/run_stage2.sh                    # quadtree   byte-identical
packer/tests/harness/run_stage3_ingest.sh             # ingest multiset-identical
packer/tests/harness/run_stage3.sh                    # end-to-end gate PASS
```

## 1. The mission and the one hard constraint

Add relation-sourced area polygons to ingest **without regressing** the Stage-3
lines + closed-way polygons. Python stays the untouched **oracle**. The
validation gate is **feature-multiset + render-diff, NOT byte-identity**
(Amendment 1): relation ring assembly will not reproduce osmium's exact vertex
order/winding, so polygons are compared **up to ring rotation/winding**
(`obcm_diff --canonical-polys`, already built) and, where vertex sets genuinely
differ, by **render-diff** (`obc-sim --png`, the semantic backstop).

Stage 4 is **mostly additive**: Stage 3 already emits the right *lines* and the
right *closed-way (`from_way`) polygons*. You are adding osmium's **non-`from_way`
areas** — the polygons it builds from relations. (See §3.3 for the one
interaction to verify.)

## 2. What's already built (lean on all of it)

Crate `firmware/obc-pack` (std; deps: `obc-reader`, `geos` v11→system GEOS 3.14.1,
`osmpbf` 0.3.8, `serde`/`serde_json` w/ `preserve_order`):

| file | what | reuse for Stage 4 |
| :-- | :-- | :-- |
| `src/config.rs` | config + style-IDs (doc order); `get_style(&tags)` first-match | style relations by **relation tags** |
| `src/ingest.rs` | `osmpbf` 2-pass (node store → ways); closed-way classify; coastlines; `polygon_is_valid` | add a relation pass + member-way geometry capture + assembly |
| `src/geom.rs` | `Geom` (Line/Polygon/Multi), bounds, GEOS bridge (`to_geos`/`from_geos`), clip, `topology_preserve_simplify`, `polygon_is_valid` | **`Geometry::polygonize`/`build_area`** live here too (add) |
| `src/main.rs` | pipeline: ingest → bbox → per-LOD simplify+quadtree → serialize | relation polygons flow through unchanged once ingest emits them |
| `src/bin/obcm_diff.rs` | structural + multiset comparator; **`--canonical-polys`** (rings up to rotation/winding) + per-LOD `SUMMARY` | the Stage-4 multiset gate |
| `src/bin/ingest_dump.rs` | dump Rust ingest features (microdeg) as JSON | unchanged — now also carries relation polygons |

Harness (`packer/tests/harness/`):
- `dump_ingest.py` — **already has a provenance handler** that tags each oracle
  feature `way`/`area` + `from_way` + tags. Stage 3 *drops* `from_way==False`
  (relation) polygons; **Stage 4 keeps them** (one-line filter flip, §6).
- `compare_ingest.py` — multiset diff, polygons canonicalized up to
  rotation+reflection (`canon_ring`). Reuse as-is.
- `dump_stage3_ref.py` — Python reference restricted to the Stage-3 set; make a
  Stage-4 variant that keeps relation polygons (§6).
- `run_stage3_ingest.sh` / `run_stage3.sh` — copy to `run_stage4_*.sh`.
- `node_probe.{rs,py}` — coordinate parity (already proven; no Stage-4 work).

Corpus (`packer/tests/corpus/`, pbfs gitignored, rebuild `build_corpus.sh`):
`tiny` (R1 + R2 below), `monaco`, **`malta` (MP-heavy, the stressor)**,
**`freiburg-forest` (inland MP-heavy)**, `freiburg-town`, `freiburg` (target).

**`tiny.osm` pins the relation cases** (header comment is the truth table):
- **R1 (rel 201)** `natural=water`, members W1 outer + W2 inner → **1 polygon WITH
  one hole** (lake + island).
- **R2 (rel 202)** `landuse=forest`, members W3 + W4 both outer → **2 separate
  polygons** (one relation → multiple outer rings → multiple polygons).
- Untagged member ways 101–104 produce **no standalone feature** (no style match —
  already dropped by Stage 3's styling filter, so no double-emit risk for them).

## 3. The landmines (Stage-4-specific)

1. **Roles are unreliable; classify outer/inner by geometry.** Don't trust the
   member `role` ("outer"/"inner") tag — lots of data is mistagged or blank.
   osmium classifies by **containment / nesting parity**: a ring at even nesting
   depth is an outer, odd is an inner (hole) of the outer that contains it. Use
   GEOS `contains` / area nesting, not roles. (Roles are a fine *tie-break/ hint*
   only.)
2. **One relation → many polygons.** R2 shows it: two disjoint outer rings → two
   `Geom::Polygon`s, same style. Inner rings attach to the outer that contains
   them. Emit one polygon per outer ring (+ its directly-nested holes).
3. **Ways are fragments to be stitched.** Multipolygon member ways are often
   *open* fragments that must be joined end-to-end (shared endpoint nodes) into
   closed rings before you have a polygon. This is the assembly step osmium's
   `Assembler` does and the `geo`/hand-rolled path must too. **GEOS
   `Geometry::polygonize(&lines)` does this for you** (§4).
4. **`admin_level` relations are line-only.** `ingest.py::area()` returns early on
   `admin_level` (Stage 3 mirrors this for closed ways). For relations: a
   `type=boundary` / `admin_level` relation produces **no polygon**. Match.
5. **Old-style (tags-on-outer-way) multipolygons** exist but are deprecated;
   modern data tags the *relation*. Oracle uses `a.tags` (the assembled area's
   tags, which osmium fills from the relation). Style via **relation tags**;
   defer old-style unless the corpus shows it (grep the diff).
6. **Broken relations** (unclosed rings, missing members, self-intersections) —
   osmium silently drops or repairs them (you saw the analogous closed-way case:
   the self-intersecting "Red House" yielded zero rings). Expect to **skip-and-
   warn** on geometry GEOS won't polygonize; verify the oracle also drops them
   (it usually emits nothing → no diff).
7. **Coordinate parity is already solved** — reuse `to_deg(decimicro)` =
   `decimicro as f64 / 1e7` (NOT osmpbf `.lon()`); see Stage-3 `node_probe`.

## 4. The hard part: ring assembly — recommended approach

**Recommendation: use GEOS `polygonize` (plan §8.4 escape hatch a) before hand-
rolling osmium's stitcher.** The `geos` crate (already linked) exposes exactly
the right tools in `geometry.rs`:

- `Geometry::polygonize<T: Borrow<Geometry>>(&[T]) -> GResult<Geometry>` — takes a
  slice of (noded) `LineString`s, returns a `GeometryCollection` of the polygonal
  faces. This does the fragment-stitching for you.
- `Geometry::build_area(&self) -> GResult<Geometry>` — builds areas from a
  (multi)linestring, handling holes; an alternative worth A/B-testing against
  polygonize for hole attachment.
- Also available: `node()` (node a set of lines at intersections — call before
  polygonize if members cross), `line_merge()`, `unary_union()`.

Sketch:
1. **Relation pass** (osmpbf has relations in `Element::Relation`): for each
   `type in {multipolygon, boundary}` relation **without `admin_level`**, record
   its member way-ids (+ roles as hints) and its tags; `get_style(rel_tags)` —
   skip if no style.
2. **Member geometry**: collect the geometry of member ways (reuse the node
   store; you already resolve way coords in `process_way`). Build `Geom::Line`s.
3. **Assemble**: feed the member LineStrings to `Geometry::polygonize` (node first
   if needed). You get polygonal faces.
4. **Outer/inner + holes**: GEOS `build_area`/`polygonize` give faces; determine
   nesting by containment to attach holes to the right outer (or test whether
   `build_area` already returns proper polygons-with-holes — measure on tiny R1).
5. **Emit** one `IngestFeature { style_id, min_lod, Geom::Polygon }` per outer
   (+ holes), styled by relation tags. Run each through `polygon_is_valid` (skip
   invalid, matching osmium).

If polygonize parity is poor (vertex sets far from osmium), the documented
fallback is **render-equivalence** (§6) — accept it, don't chase byte parity.

A pure hand-rolled stitcher (node→ways adjacency graph walk, ring closure,
containment classification) is the faithful-but-expensive path; only reach for it
if GEOS polygonize can't match render-wise.

## 5. Scope, precisely

- **In:** `type=multipolygon` and `type=boundary` area relations → polygons
  (with holes), styled by relation tags, skipping `admin_level`. Skip-and-warn on
  un-assemblable geometry.
- **Out (later stages):** land/sea generation + multi-PBF merge (Stage 5);
  parallelization + node-store shrink (Stage 6); `jobs.py` integration behind
  `OBC_PACK_BACKEND` (Stage 7).
- **Unchanged:** lines + closed-way polygons (Stage 3) and the
  bbox/simplify/quadtree/serialize pipeline. Relation polygons just join
  `Ingested.features` and flow through `main.rs` untouched.

## 6. Validation plan (the harness is 90% there)

1. **Ingest-only multiset (primary).** Flip `dump_ingest.py`: **keep**
   `from_way==False` (relation) polygons in the expected set (Stage 3 dropped
   them). Make `dump_ingest_stage4.py` (or a `--with-relations` flag). Rust
   `ingest_dump` already emits relation polygons once `ingest.rs` builds them.
   Compare with `compare_ingest.py` (polygons already canonicalized up to
   rotation/winding). **Target: multiset-identical** modulo ring winding; any
   residual is an assembly divergence to investigate.
2. **End-to-end `.obcm`.** Stage-4 variant of `dump_stage3_ref.py` that keeps
   relation polygons; compare with `obcm_diff --canonical-polys`. Expect:
   structural match; lines byte-exact; closed-way + relation polygons up to ring
   winding; the same 12 m-LOD simplify skew as Stage 3.
3. **Render-diff (the backstop for assembly divergence).** Where vertex *sets*
   differ (osmium stitched/wound a ring differently than GEOS polygonize),
   canonicalization won't reconcile them — confirm visual equivalence:
   ```sh
   obc-sim ref.obcm  --png ref.png  --center <lon>,<lat> --zoom <m> --size 480x480
   obc-sim rust.obcm --png rust.png --center <lon>,<lat> --zoom <m> --size 480x480
   # then pixel-diff ref.png vs rust.png (add a tiny compare_png.py; assert ==,
   # or within a small threshold). Pick centers/zooms over MP-dense areas.
   ```
   Wire a `run_stage4.sh` that does multiset + a couple of render-diffs per item.
4. **Corpus order:** `tiny` (R1 hole + R2 two-outer — must be exact), then
   `freiburg-forest` + `malta` (the MP stressors), then the rest.

## 7. Concrete first steps (ordered)

1. Re-read `ingest.py::area()` (the oracle for tagging + hole/`admin_level`
   rules) and this doc's §3–§4. Skim `osmpbf`'s `Relation`/`RelMemberIter` API.
2. **Probe GEOS assembly on tiny R1/R2** before wiring anything: a throwaway that
   feeds R1's two member-way rings to `Geometry::polygonize` (and `build_area`)
   and prints whether you get 1 polygon-with-hole (R1) and 2 polygons (R2). Pick
   polygonize vs build_area based on which attaches the hole correctly.
3. `ingest.rs`: add a relation collector (pass over `Element::Relation`) +
   member-geometry capture in pass 2 + an `assemble_relation` using the chosen
   GEOS path; emit polygons styled by relation tags; `admin_level` → skip;
   `polygon_is_valid` filter. Unit-test against `tiny.osm` (R1 → 1 poly+hole,
   R2 → 2 polys).
4. Harness: `dump_ingest` Stage-4 variant (keep relations) + `compare_ingest`
   (reuse) + `run_stage4_ingest.sh`. Get tiny → multiset-exact, then
   freiburg-forest/malta.
5. End-to-end: Stage-4 reference + `run_stage4.sh` (`obcm_diff --canonical-polys`)
   + a small `compare_png.py` render-diff. Explain every residual.
6. Update `rust-port-plan.md` Progress + corpus README + the
   `[[rust-port-progress]]` memory; commit.

## 8. Risks & open questions

- **Does GEOS `polygonize`/`build_area` reach render-parity with osmium's
  `Assembler`?** Unknown until measured (step 2). This is the gating risk and
  decides hand-roll vs GEOS. The plan pre-authorizes render-equivalence if not.
- **Hole attachment / nesting depth** for relations with nested rings (island in
  a lake in a … ). Test on real data (malta lakes).
- **Double-emit interaction (verify early).** Stage 3 found malta has ~48 closed
  `building=yes` ways that are *also* relation members, and the oracle emitted
  `from_way` polygons for **47 of them** (the 48th was the invalid Red House) —
  i.e. a *tagged* member way legitimately yields **both** its own `from_way`
  polygon (Stage 3, already emitted) **and** contributes to the relation polygon
  (Stage 4). So Stage 4 is additive and untagged members are already dropped by
  styling. **Confirm** no case where Stage 3 + Stage 4 together emit a *duplicate*
  of the same polygon (watch `only-in-rust` polygon counts climb).
- **Performance**: per-relation GEOS polygonize on freiburg (many relations) —
  keep an eye on the 33 s / 1.48 GB Stage-3 baseline; defer optimization to
  Stage 6 but don't regress wildly.

## 9. Reference index

- Oracle: `packer/obcm/ingest.py` (`area()` is the spec — tags, holes,
  `admin_level`, `>=3` ring guard), `config.py`, `pack.py`.
- Plan: [`rust-port-plan.md`](rust-port-plan.md) §4.2 (way/area disambiguation),
  **§8.4 (relation assembly + escape hatches)**, §9 (when to bail to hybrid).
- Format: [`OBCM_Spec.md`](../../OBCM_Spec.md); reader
  `firmware/obc-reader/src/reader.rs`.
- GEOS crate assembly APIs: `geos-11.1.1/src/geometry.rs` — `polygonize`,
  `build_area`, `node`, `line_merge`, `unary_union`.
- Fixture truth: `packer/tests/corpus/tiny/tiny.osm` (R1 + R2 header comment).
- Stage-3 record + the coordinate-parity / invalid-geometry findings:
  [`stage3-handover.md`](stage3-handover.md).
