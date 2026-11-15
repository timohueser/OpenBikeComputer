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
| `graph` | `graph::build`: the way graph of a cell (parts, ways, samples, junctions, joints, ends, crossings, decks); `graph::check`; `graph::levels`, which the feature bake runs, and `graph::level_tags`, the tags that carry the levels in the tiles and their reader |
| `cell` | `build_cell`: a cell from its request bytes, stage by stage (`STAGES`), with the time of each; `build_storeys`: the same with the storeys over the ground (decks); `far_request`: a cell's far block alone |
| `transport` | `Request` (the canonical request bytes), `Flat` (the plan as flat integer arrays, its stats and digest), `vocabulary()` |
| `debug` | The palette that the debug tool and the viewer's debug draw share |
| `probe` | Per thread, the caps that a build and the CPU height function hit, for the census; the WASM build counts nothing |

The census of every cell of a release is the binary `census` of
[`obc-trailview-features`](../obc-trailview-features/README.md#census), beside the release readers.

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
  [--size-m 128] [--px 1600] [--layers terrain,inputs,cells] [--gltf] [--probe N] \
  [--census] [--bench] [--faces-bench [--runs N]]
obc exec cargo run --release -p obc-ground-plan --example debug -- --sheet COLUMNS,out.png,a.png,...
obc exec cargo run --release -p obc-ground-plan --example debug -- \
  requests --out DIR (--places NAMES | --all) [--runs 5]   # requests for check 7 and the timing page
```

`requests` writes per cell its request bytes and an entry of `index.json`: the plan digest, the
far block's digest (`far`), bytes (`far_bytes`) and the counts of its check against the near block
(`far_check`), and the native median times per stage, of the whole build (`total`) and of a
far-only build (`far`). A far block that the crate refuses is `far: "refused: …"`; the run goes on.
A far-only build that differs, or a fault of the check, stops the run.

A place is a view of `--views` named `<place>-top`, at the `lat` and `lon` of its query.
`--places A` takes `A` and every place whose name starts with `A-`. Per place, `--out` gets
`<place>.png` (north up), `<place>.json` (cells, tiles, counts, unknown pixels, height range, widths
capped at 15 m, times), with `--gltf` `<place>.glb` (x east, y up, z south, metres about the
centre), and with `--probe N` `<place>.probe.json` (`[lon, lat, height]` away from every stamp).
Each layer draws each cell under a mask of its square: a step at a cell line is a fault.
With the layer `graph`, `<place>.json` also lists the owned points and crossings in the picture.
With the layer `faces`, it lists the rounds in the picture (centre, radius, owner).
`--census` writes `junctions.json` over every fixture cell: the owned junctions by host and guest
class, the host rules, close junction pairs and the summed `GraphStats`. `--bench` writes
`graph-bench.json`: the build time of the graph per fixture cell (the median of 5, and the first run), and its heap.
`--faces-bench` writes `faces-bench.json`: per fixture cell the face build time (the best of `--runs`),
checks 1 and 2, kinks, short features and crashes, with every kink, short feature and fragment place. It builds on 4 threads and prints one line per cell as it ends.
Run it under a memory cap:
`systemd-run --user --scope -p MemoryMax=3G -p MemorySwapMax=0 timeout 600 <debug> --faces-bench`.
`--sheet` puts the pictures in rows of `COLUMNS` into `out.png`.
`--storeys` writes per place the storeys over the ground from above (`<place>-storeys.png`), seen obliquely with the
crate's heights (`<place>-oblique.png`, `--oblique HEADING,PITCH`, default 30,35), their counts and checks
(`<place>-storeys.json`) and with `--gltf` `<place>-storeys.glb`.

The heights (run each under the memory cap above):

```sh
D="obc exec cargo run --release -p obc-ground-plan --example debug -- heights"
$D --out DIR --places A,B [--at NAME=LAT,LON] [--size-m M] [--px N] [--sections H,...]
$D --out DIR --fixture-steps [--grid M] [--cells X,Y;...]
$D --why LAT,LON[;...]     # or --why-g X,Y[;...] (global frame units)
$D --cost DIR
$D --out DIR --gpu-check --places A,B [--at NAME=LAT,LON]
```

| Option | Output |
| --- | --- |
| `--places`, `--at` | `NAME-steps.png` (contours every 0.1 m, face edges, steps by class), `NAME-ripple.png`, `NAME-section-H.png` per heading, `heights-view.json` |
| `--fixture-steps` | `fixture-steps.json` (steps by rule, borders, ripple, steps at label edges with no rule, slopes steeper than a rock face), `inside.json`, `unnamed.json`, `slots-full.json`, `host-missing.json`, `border-differ.json`, `edge-steps.json` |
| `--why` | Prints the records and both sides' causes at the point and 2 units about it |
| `--cost` | `cost.json`: the heights per fixture cell on one thread |
| `--gpu-check` | `blocks.bin` and `points.json` for the GPU probe |

The GPU probe compares the viewer's `heights.wgsl` with `points.json`. Run it from `builder/web`,
one GPU user at a time:

```sh
flock /tmp/trailview-gpu.lock node tests/browser/trailview/model-probe.mjs DIR
```

| Layer | Colour |
| --- | --- |
| `terrain` | Hillshade from the north-west; thin contours every 1 m, dark every 5 m; magenta: no height |
| `inputs` | Bands: grey carriageway over a pale shoulder, blue water. Chords: orange main road, brown road, red path, black track, purple tunnel or gallery, blue waterway, grey other line; dashed on a bridge or in a tunnel. A dot per vertex, a square where `side` is set. Pink: the outer edge of a sidewalk. Grey fill: building; blue fill: lake, river area, sea; green outline: other area. Black dot: point |
| `cells` | Black: cell lines; faint: level-18 node lines; dashed red: the centre cell grown by its margin; cross: the centre; bar: 10 m |
| `graph` | Samples: brown road, red path, black rail, blue water; dashed on a deck, dotted in a tunnel; a tick at a cell line. Red: link; orange: synthetic deck. Green dot: junction, with its host arms thick green and its guest arms thin purple for 8 m. Black dot: joint; ring: end. A crossing's letter: D deck, C culvert, F ford, P pipe in stones, S stepping stones, B footbridge, L level crossing, X stacked |

## Pitfalls

- Every transcendental goes through `libm`, and no `HashMap` or `HashSet`: `clippy.toml` forbids
  the std ones in the crate, its tests and the tool. Rust fuses no multiply and add unless the code
  calls `mul_add`; do not call it.
- The feature bake links `sections`: a change to the width rules changes the feature tiles, and the
  layer bakes again.
- `image` and `tiny-skia` are dev-dependencies: the tool is an example, so they never reach WASM.
