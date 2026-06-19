# Hybrid packer (Python ingest + Rust back-half via PyO3) — fallback plan

> **Status:** the fallback. Use this if the full port
> ([`rust-port-plan.md`](rust-port-plan.md)) stalls on OSM area assembly. You can
> still finish the full port later — this hybrid is a strict subset of it.
>
> **Why this is the safe option for your stability concern:** the part you are
> most worried about regressing — **ingest + libosmium area assembly — is not
> touched at all.** It stays exactly as the stable Python code it is today. Only
> the deterministic, GEOS-light **back half** (quadtree + serialize, the ~69 % /
> ~157 s of pure-Python overhead) moves to Rust.

## 0. The idea

Keep `packer/obcm/ingest.py` (osmium 2-pass → shapely features) verbatim.
Replace only the Python quadtree-build + serialize loop in `pack.py` with a
single Rust call. Expected ~227 s → ~80 s (**~3×**) with **near-zero ingest
regression risk**, because ingest output is unchanged by construction.

```
pack.py:  ingest_osm()  ──►  [features]  ──►  build_obcm(...)  ──►  bytes ──► write
          (unchanged Python)   handoff       (NEW: Rust/PyO3)
```

## 1. Where the new code lives

A **PyO3 extension module**, built with `maturin`, e.g. crate
`firmware/obc-pack/` with a `cdylib` target (or `packer/_native/`). It exposes
one function to Python:

```
build_obcm(
    coords:       np.ndarray[float64, (N,2)],   # all vertices, concatenated
    ring_lens:    np.ndarray[int32],            # vertices per ring
    ring_offsets: structure linking rings -> features (see §3)
    style_ids:    np.ndarray[uint8],
    min_lods:     np.ndarray[uint8],
    geom_kinds:   np.ndarray[uint8],            # line / polygon / multipart
    lods_config, global_bbox, chunk_size,
    style_table_bytes, marker_color,
) -> bytes
```

`pack.py` changes minimally: after `features, coastlines = ingest_osm(...)` and
the land/bbox steps, build the flat arrays (§3) and call `build_obcm(...)`,
then write the returned bytes. Keep the **existing Python quadtree+serialize path
behind a flag** (`OBC_PACK_BACKEND=python|rust`) for A/B validation and instant
fallback.

## 2. Reduced correctness surface

Because ingest is untouched, the §4 reproduction checklist from the full-port
plan **drops items 1–2 (style selection, way/area disambiguation) and all of
area assembly** — those stay in proven Python. The Rust must still match, exactly:

- **Coordinate rounding** — banker's rounding (round-half-to-even) for feature
  coords (`int(round(v*1e6))`); truncation for the global bbox. (Full-port §4.3.)
- **Topology-preserving simplify** — shapely `.simplify()` defaults to
  `preserve_topology=True`; use the geos crate's `topology_preserve_simplify`,
  not plain DP. (Full-port §4.4.)
- **Densification** at 30000 µdeg, exact integer stepping. (§4.5.)
- **Quadtree** rule, floor-division midpoints, NW/NE/SW/SE order, 10-µdeg
  recursion guard, containment-vs-clip decision, multi-geometry flattening,
  BFS chunk order, insertion order within a leaf. (§4.6.)
- **Serialize** byte layout — already mirrored by `obc-reader`. (§4.7.)

Same **GEOS-version pin** caveat as the full port (quadtree clip + simplify call
libGEOS): match the `geos` crate's libGEOS to shapely's `geos_version`.

Dependencies: `pyo3`, `numpy` (zero-copy array views), `geos` (same libGEOS as
shapely — the correctness lever), and `obc-reader` (shared format + round-trip
oracle in tests).

## 3. The Python → Rust boundary (perf- and correctness-critical)

Do **not** pass shapely objects across the boundary (slow + opaque). Extract all
geometry vectorized, at C speed:

- `shapely.get_coordinates(geoms, return_index=True)` → one `(N,2)` float64
  array of every vertex plus an index array mapping each vertex to its source
  geometry. This is the same fast path that makes shapely 2.x vectorized ops
  quick; it avoids the per-object Python loop that dominates the current
  serialize stage.
- For polygons/multipart geoms you also need **ring structure** (exterior +
  interior ring lengths, and which rings belong to which feature). Build these
  offset arrays in Python (vectorized where possible: `shapely.get_parts`,
  `get_rings`, `get_num_interior_rings`).
- Hand the flat arrays to Rust as numpy views (zero-copy via the `numpy` crate).
  Rust reconstructs lightweight `Vec<Ring>` per feature — no GEOS objects needed
  until the clip step, where it wraps coordinates into `geos` geometries only for
  features that actually straddle a quadtree node boundary.

Budget a few seconds for materializing ~13.4 M points into numpy + Rust; it is
dwarfed by the ~157 s removed. If this handoff is slower than hoped, the
fallback-within-fallback is to also move shapely geometry *construction* into
Rust — which is exactly the step toward the full port.

## 4. Validation — easier than the full port

Use the same harness (corpus, `obc-reader` structural compare, byte compare,
`obc-sim --png` render-diff) from the full-port plan §5. **Plus** a sharper
unit test that isolates the ported code:

- Capture the **exact features** the Python ingest produces for a region (pickle
  or a dumped intermediate), once.
- Feed those identical features to **both** the Python `serialize_lods(...)`/
  quadtree path **and** `build_obcm(...)`.
- Assert **byte-identical** output. Because the *input is identical by
  construction* (same ingest), any diff is purely in the ported back-half — far
  easier to bisect than a full-pipeline diff.

Switchover gate: byte-identical on the corpus via the flag, then flip
`OBC_PACK_BACKEND` default to `rust` (keep Python fallback wired).

## 5. Staged execution

1. `maturin` scaffold; expose a stub `build_obcm` returning the header only;
   wire the flag into `pack.py`; prove the build + import + call path.
2. **Serialize in Rust**, fed the flat arrays; byte-match against Python
   `serialize_lods` on captured features (zero GEOS here — trivial to verify).
3. **Quadtree in Rust** (clip via `geos`); byte-match the full back-half against
   Python on the `test_quadtree.py` cases and real features.
4. Validate on the full corpus (structural + byte + render-diff).
5. Parallelize per-LOD build/serialize across threads; enforce determinism and
   re-run the corpus.

## 6. Effort

~1 week. No area-assembly risk; the back half is deterministic and the §4
isolated test makes regressions obvious. This is the low-risk way to bank the
majority of the speedup while leaving the ingest you trust completely untouched.
