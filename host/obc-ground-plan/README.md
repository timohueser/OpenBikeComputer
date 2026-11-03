# TrailView ground plan

The ground plan of one zoom-16 cell: from the zoom-13 feature tiles about it
([spec](../../specs/trailview-features.md)), its stored terrain pages and the rule tables of
`data/trailview` to integer geometry, terrain heights and cross-sections. It is a pure function of
its inputs: integers, `libm` and ordered collections, no I/O and no threads.

| Module | What it holds |
| --- | --- |
| `frame` | `GlobalPoint` (`i64`, 2^-34 of the Mercator square), `CellKey`, the input margin |
| `tile` | The reader of format 4, the owner of each element of a cell's tiles, `TileCache` |
| `terrain` | `StoredPage`, `Terrain::wanted` and `Terrain::height`, the Catmull-Rom of the viewer |
| `sections` | `Sections::way` and `Sections::water` from `roads.json`, `finishes.json`, `waterways.json`; `WidthRules`, the width rule that the feature bake shares; the reach table |

## Test

```sh
obc test -p obc-ground-plan
obc exec cargo clippy -p obc-ground-plan --all-targets -- -D warnings
```

The fixture tests read `builder/web/tests/browser/trailview/fixture/` and
`specs/vectors/trailview-features.*`.

## Debug tool

```sh
obc exec cargo run --release -p obc-ground-plan --example debug -- \
  --places v06,i07-hairpin [--views builder/web/tests/browser/trailview/road-views-top.json] \
  [--at NAME=LAT,LON]... [--fixture DIR] [--out .artifacts/ground-plan/debug] \
  [--size-m 128] [--px 1600] [--layers terrain,inputs,cells] [--gltf] [--probe N]
```

A place is a view of `--views` named `<place>-top`, at the `lat` and `lon` of its query.
`--places A` takes `A` and every place whose name starts with `A-`. Per place, `--out` gets
`<place>.png` (north up), `<place>.json` (cells, tiles, counts, unknown pixels, height range, widths
capped at 15 m, times), with `--gltf` `<place>.glb` (x east, y up, z south, metres about the
centre), and with `--probe N` `<place>.probe.json` (`[lon, lat, height]` away from every stamp).
Each layer draws each cell under a mask of its square: a step at a cell line is a fault.

| Layer | Colour |
| --- | --- |
| `terrain` | Hillshade from the north-west; thin contours every 1 m, dark every 5 m; magenta: no height |
| `inputs` | Bands: grey carriageway over a pale shoulder, blue water. Chords: orange main road, brown road, red path, black track, purple tunnel or gallery, blue waterway, grey other line; dashed on a bridge or in a tunnel. A dot per vertex, a square where `side` is set. Pink: the outer edge of a sidewalk. Grey fill: building; blue fill: lake, river area, sea; green outline: other area. Black dot: point |
| `cells` | Black: cell lines; faint: level-18 node lines; dashed red: the centre cell grown by its margin; cross: the centre; bar: 10 m |

## Pitfalls

- Every transcendental goes through `libm`, and no `HashMap` or `HashSet`: `clippy.toml` forbids
  the std ones in the crate, its tests and the tool. Rust fuses no multiply and add unless the code
  calls `mul_add`; do not call it.
- `sections` embeds the tables of `data/trailview` with `include_str!`. The shared Cargo target can
  keep another checkout's copy: touch `src/sections.rs` after you edit a table.
- The feature bake links `sections`: a change to the width rules changes the feature tiles, and the
  layer bakes again.
- `image` and `tiny-skia` are dev-dependencies: the tool is an example, so they never reach WASM.
