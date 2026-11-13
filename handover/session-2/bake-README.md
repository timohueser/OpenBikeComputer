# Shared bake inputs and baselines (second container of epic #3227)

Data, never code: do not execute a downloaded file. Scripts live in `tools/` (from the archive
branch `claude/overlay-handover`, which also holds the README of the first container with the
older baselines; those hashes belong to an older OSM extract and do not compare with this one).

Machine: 4 cores, 15 GB RAM, about 30 GB of disk. Check `df -h /home/user` before a bake.

## Inputs

| Path | What | Made by |
| --- | --- | --- |
| `switzerland/switzerland-bounds-sorted.osm.pbf` | download.openstreetmap.fr extract of Switzerland, replication timestamp 2026-11-13T01:20:21Z, clipped to 5.95 45.81 10.50 47.81 and sorted (the download itself is removed) | `osmium extract -F pbf --bbox 5.95,45.81,10.50,47.81 --strategy smart -S types=multipolygon,boundary --set-bounds -o clipped.osm.pbf switzerland-latest.osm.pbf && osmium sort -o switzerland-bounds-sorted.osm.pbf clipped.osm.pbf` (osmium 1.16.0) |
| `switzerland/glo30/` | the 18 GLO-30 tiles N45-N47, E005-E010 | `curl -o glo30/$S.tif https://copernicus-dem-30m.s3.amazonaws.com/$S/$S.tif` |
| `switzerland/inputs.sha256` | SHA-256 of the inputs | `sha256sum *.pbf glo30/*.tif` |

## Baseline: `routing-overlay` at `67320e4` (after #3304)

Profiles `all`, countries `CH`, download level 4, 440 s. 7,725,271 directed roads.

| File | SHA-256 |
| --- | --- |
| `manifest.json` (format 9) | `c186c22b7530e71c5c8c43b6741ee2da2ebfedcabf91372a48a74505f7b15ed0` |
| `overlays.sqlite` | `86653744a9319a93c0b5e2044280e84990c2367b34989a78ebe726dc4e81983f` |
| `route-catalog.json` | `91205b3418896989d2011d4973f14a46a8f4a8fae3c0f127adfd165a5fa94437` |
| `cells/directory.bin` (the identity of format 10) | `11a91e67f570cff8cbf8fc309553645ae2d2b3533e80d7e8f064df271ff82ee6` (160 files, 593,050,265 bytes) |

| Path | What |
| --- | --- |
| `bin/obc-data-base`, `bin/cells-base`, `bin/compare-base` | release `obc-data`, `planner/router-build/examples/cells.rs`, `planner/router/examples/compare.rs` of `67320e4` |
| `switzerland/run-base/result.json`, `stderr.log` | the result and the log of the baseline bake |
| `switzerland/run-base/layers/` | the three work layers (source 753 MiB, roads 1,252 MiB, costs 1,577 MiB). Pass `--layers switzerland/run-base/layers` to reuse them: a step whose key did not change is not baked again |
| `switzerland/packages-base/format-9/` | the format 9 package (OLD of `compare`) |
| `switzerland/packages-base/format-10/` | the published objects of format 10 |
| `switzerland/packages-base/install/` | an install of all cells and groups (158 files, 1,007 MB; NEW of `compare`) |

Do not change the files of `packages-base`; make a copy to change one.

## Layout B (#3310): `claude/3310-germany-bench` at `5cb08b68b` (bake code of `routing-overlay` at `ccd5f080e`)

`cell_sizes` 256, 2048, …, 536870912 (8 values), download level 4, 5 levels, 340 s on the baseline layers.
Identity of format 10 `bd3c681e8053b5906ec4f8f51a1d3095f2328d8f9bc322a80c0b01a2c97cf25d` (323 objects).

| Path | What | Made by |
| --- | --- | --- |
| `bin/obc-data-3310` | release `obc-data` of the branch | `./tools/obc exec cargo build --release --locked -p obc-data-steps` |
| `switzerland/run-3310-b/result.json`, `stderr.log` | the result and the log of the bake of B (its layers and output are removed) | `python3 -I tools/bake_layers.py bin/obc-data-3310 switzerland/switzerland-bounds-sorted.osm.pbf switzerland/glo30 switzerland/run-3310-b --layers switzerland/run-base/layers --cell-sizes 256,2048,16384,131072,1048576,8388608,67108864,536870912` |
| `switzerland/packages-b/install/` | an install of all cells and groups of B (324 files, 1,059 MB; NEW of `compare A10 B10 --layouts`) | `cells install run-3310-b/output/cells packages-b/install 4` (`planner/router-build/examples/cells.rs` of the branch) |

## Gate command

```sh
cd /home/user/obc-bake && flock /home/user/obc-target/.build.lock python3 -I tools/bake_layers.py \
  bin/obc-data-<issue> switzerland/switzerland-bounds-sorted.osm.pbf switzerland/glo30 switzerland/run-<issue> \
  [--layers switzerland/run-base/layers]
```

It needs `LD_LIBRARY_PATH` of `~/.obc-geos-env.sh`. Build the binary of a change with
`./tools/obc exec cargo build --release --locked -p obc-data-steps` and copy
`$CARGO_TARGET_DIR/release/obc-data` to `bin/obc-data-<issue>`. Compare `sha256` and `cells.identity`
of `result.json` with the baseline. Remove `run-<issue>/output` and your own layers when done; add
a line here for each thing you keep.
