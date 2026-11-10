# TrailView features

The step `planner/trailview-features` of `obc data`: the TrailView feature tiles of a region, with
the heights of roads, paths, tracks, bridges, tunnels, rivers and lakes, and the sea. The
[spec](../../specs/trailview-features.md) defines the archive and what the heights promise.

`obc-data-steps` runs the step as the function `obc-trailview-features/features`. The step needs
Osmium (`osmium-tool`) on the path.

## Run

The binary bakes the archive of a box without `obc data`, with the kind table and the road classes
of the checkout, and the terrain of a `planner-dem` MBTiles. Without `--sea`, the archive has no sea:

```sh
cargo build --locked --release -p obc-trailview-features --bin trailview-features
target/release/trailview-features --osm OSM.pbf --terrain TERRAIN.mbtiles --bounds W,S,E,N --output OUT.pmtiles \
  --sea water-polygons-split-3857.zip
```

`builder/web/tests/browser/trailview/box.sh` and the engine fixture recipe use it.

| Variable | Effect |
| --- | --- |
| `OBC_OSMIUM` | The Osmium executable. The default is `osmium` on the path. |
| `OBC_DATA_MEMORY_BYTES` | The road jobs that run at once fit in this memory. `obc data` sets it. |

## Census

`census` builds each zoom-16 cell of a box as the TrailView viewer does: the ground plan of the cell
(`obc-ground-plan`), then its mesh with the rims of its eight neighbours. It reads a local release
of `obc data` (its feature and terrain map packs) or a fixture folder, and it writes nothing into
them. Run it under a memory cap. Each worker process gets its own cap (`--memory`, a user scope of
`systemd-run`):

```sh
cargo build --locked --release -p obc-trailview-features --bin census
systemd-run --user --scope -p MemoryMax=1G -p MemorySwapMax=0 nice -n 19 target/release/census \
  (--release STORE [--id ID] | --fixture DIR) --out OUT [--workers 3] [--memory 2G|none] [--timeout-s 60] \
  [--grid-m 4] [--strip 10] [--rows 48] [--limit N] [--cells X/Y,...] [--bounds W,S,E,N] [--min-available-gb 2]
```

| Output | Holds |
| --- | --- |
| `OUT/census.jsonl` | One line per cell: the state (`ok`, `empty`, `error`, `mesh_error`, `panic`, `timeout`, `memory`, `crash`), the stage times, the heap, the counts, the caps of `obc_ground_plan::probe`, the crate's checks, and the checks with the west and north neighbours |
| `OUT/summary.json` | The counts per state and per cap, the checks, the border faults, each failed cell with its latitude and longitude, the slowest and largest cells |
| `OUT/workers.log` | The stderr of the workers |

A release is the store folder of a target (`release.json`, `releases/`, `objects/`). Without `--id`,
the census reads the release that the target serves. A fixture has `features/13/C/R.bin` (gzip),
`terrain/Z/X/Y.webp`, `config.json` and an optional `cells.json`. A worker that panics, passes
`--timeout-s` in one stage of a cell or passes its memory cap ends. The census records that cell and
gives the rest of the job to a new worker. A run continues from its `census.jsonl`. It stops when
the PC has less memory available than `--min-available-gb`. The mesh check of a shared line needs
both meshes in one worker, so it leaves out the lines between two strips of `--strip` columns and
between two runs of `--rows` rows (`"mesh": null`).

## Memory

The water networks run on all cores. The road jobs run on a core each, the largest first, as many
at once as fit in the memory that is free past a quarter of RAM, less what the process holds. With
no memory free, one job runs at a time.

## Test

`obc test -p obc-trailview-features`. Without Osmium, the tests of the cut and of the bake of a small
region return at once, except in CI (`CI` set), where they fail.
