# OBCM v3 LOD — implementation handoff

Picks up the v3 LOD migration. Format design (locked):
[../specs/2026-06-16-obcm-lod-design.md](../specs/2026-06-16-obcm-lod-design.md).
Decisions: **pyramid layers**, **RGB565 in file + quantize at render**,
**meters-per-pixel LOD selection**. **v2 has been dropped entirely — v3 only.**

## Done (ingest side, working & verified)

- **config.json** → v3 schema: top-level `lods` array (`max_mpp` (null = ∞ for
  coarsest), `simplify` in meters) + per-feature `min_lod`. 3-level default.
- **obcm/ingest.py** → `OSMHandler` attaches `min_lod` to each feature from its
  matched style entry.
- **obcm/serialize.py** → `serialize_lods()` (v3) writes header(30) + style table
  + LOD table (18 B/entry: `max_mpp f32, index_off u32, node_count u32,
  chunk_size u16, chunk_count u32`) + per-LOD index/chunks. Shared
  `serialize_tree()` helper. **`serialize_all` (v2) removed.**
- **obcm_pack.py** → builds one quadtree per LOD (cumulative: features with
  `min_lod <= i`, each `shapely.simplify(simplify_m/111320)`), calls
  `serialize_lods`. Land polygons get `min_lod` from config.
- **obcm/reader.py** → rewritten **v3-only**: parses LOD table, `select_lod_for_mpp`,
  `query_bbox(bbox, mpp=)`, defaults to finest layer. Removed v2 header branch +
  index-size discovery (v3 stores node counts).
- **Verified:** Monaco → v3 with 3 LODs; reader round-trips; m/px selection gives
  167 / 670 / 6231 features at mpp 2000 / 50 / 5. Reference file at
  `/tmp/monaco_v3.obcm` (regenerate with `python obcm_pack.py
  monaco-260614.osm.pbf config.json out.obcm`).

## Remaining — pick up here

1. **Webapp LOD configuration UI** — ✅ **DONE** (2026-06-16). Files:
   `webapp/static/{app.js,index.html,style.css}`.
   - New **"Levels of detail"** card (above features): variable number of LOD
     tiers; LOD 0 = coarsest with `max_mpp` shown as ∞ (forced null), each finer
     row has editable `max_mpp` + `simplify` (m). "+ add finer level" appends a
     finer tier (default max_mpp = prev/2); per-row × removes (with index remap of
     feature start-tiers). Helpers: `renderLodEditor`, `addLod`, `removeLod`,
     `floatInput`.
   - Per-feature start-tier is a **segmented LOD picker** (`buildLodPicker`): N
     little numbered pills; clicking pill `i` sets `min_lod = i` and fills pills
     `i..N-1` (cumulative — feature shows at that tier and every finer one). Lives
     in a "LODs" column placed right after the type name so it stays visible.
   - `loadConfig()` ensures `config.lods` exists, forces `lods[0].max_mpp=null`,
     and defaults/clamps each feature's `min_lod`. `buildConfigForSubmit()` emits
     `{lods:[{max_mpp,simplify}], features:{…,min_lod}}` (clamped). Add/remove LOD
     re-renders the style editor so pickers stay in sync. Colors/z/weight are
     global (unchanged) per request.
   - Backend unchanged. Verified live (preview): pickers reflect config.json,
     click/add/remove all work, coarsest stays ∞, no console errors.

2. **Tests** (currently skipped; `pytest` FAILS until done) — *left as-is per
   request; revisit later:*
   - `tests/test_reader.py` is still the **old v2 version and is broken** (uses
     removed `serialize_all`, builds v2 bytes). Rewrite for v3: build files via
     `serialize_lods` + `QuadtreeNode`, assert header/styles, query+decode a line
     and a polygon-with-hole, and `select_lod_for_mpp` switching. (A v3 draft was
     sketched in the prior session — re-create it.)
   - `tests/test_serialize.py`: already updated (`test_serialize_lods_header`);
     other tests still valid.
   - `tests/test_quadtree.py`, `test_ingest.py`, `test_config.py`: unaffected.

3. **Docs:**
   - `OBCM_Spec.md` still documents v2 — rewrite for v3 (30-byte header, LOD table,
     per-LOD index/chunks, m/px selection).
   - LOD design doc "Migration" section says "v2 stays readable" — now stale;
     update to note v2 dropped.

4. **Then: Rust v3** (next phase, explicitly deferred). `viewer-rs/obcm` is
   **v2-only** and cannot read the new files. Extend it to parse the LOD table and
   add m/px layer selection in `obcm-sim`; quantize stays.

## Watch-outs

- **Stale .obcm files**: repo files (`monaco.obcm`, `andorra.obcm`, `bw.obcm`, …)
  are v2 → unreadable by the v3 reader and the (still-v2) Rust sim. Re-pack as
  needed.
- **`obcm_view.py`** (Python viewer) left as-is per request; runs on v3 (finest
  layer only, no LOD switching), no longer opens v2 files.
- **Style-id collisions** in config.json (id 20 = highway.unclassified *and*
  natural.water, with different colors; id 30 wood/forest; id 32 grass/park).
  `pack_style_dict` emits duplicate records, reader keeps last. Pre-existing —
  decide whether to assign unique ids.
- **Uncommitted**: config.json, obcm/{ingest,serialize,reader}.py, obcm_pack.py,
  tests/test_serialize.py modified; tests/test_reader.py broken. Not yet committed.
