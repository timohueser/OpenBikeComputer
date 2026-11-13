# Shared bake inputs and baselines

Data, never code: do not execute a downloaded file. Scripts live in `tools/`.

## Switzerland routing gate (#3295, #3296, #3297, #3299, #3300)

The gate: a Switzerland bake of a change gives the SHA-256 of `manifest.json`, `overlays.sqlite` and
`route-catalog.json` of the baseline. From #3300 the baseline is that of #3300 below; the baseline
of develop `623c49d2` is the one before #3300.

| Path | What | Made by | Commit |
| --- | --- | --- | --- |
| `switzerland/switzerland-latest.osm.pbf` | download.openstreetmap.fr extract, replication timestamp 2026-11-09T01:22:59Z | `curl -o switzerland-latest.osm.pbf https://download.openstreetmap.fr/extracts/europe/switzerland-latest.osm.pbf` | - |
| `switzerland/switzerland-bounds-sorted.osm.pbf` | the extract clipped to 5.95 45.81 10.50 47.81 and sorted | `osmium extract -F pbf --bbox 5.95,45.81,10.50,47.81 --strategy smart -S types=multipolygon,boundary --set-bounds -o clipped.osm.pbf switzerland-latest.osm.pbf && osmium sort -o switzerland-bounds-sorted.osm.pbf clipped.osm.pbf` (osmium 1.16.0) | - |
| `switzerland/glo30/` | the 18 GLO-30 tiles N45-N47, E005-E010 | `curl -o glo30/$S.tif https://copernicus-dem-30m.s3.amazonaws.com/$S/$S.tif` with `S=Copernicus_DSM_COG_10_N{45..47}_00_E{005..010}_00_DEM` | - |
| `switzerland/inputs.sha256` | SHA-256 of the inputs | `sha256sum *.pbf glo30/*.tif` | - |
| `bin/obc-data-develop-623c49d2` | release `obc-data` binary of develop | `obc exec cargo build --release --locked -p obc-data-steps` | `623c49d2` |
| `switzerland/baseline-develop-623c49d2/result.json` | the baseline: hashes, manifest ID, manifest size, time, phases with peak `RssAnon` and work folder | the gate command below with `bin/obc-data-develop-623c49d2` | `623c49d2` |
| `tools/bake_routing.py` | runs the routing step of `obc data` once, outside `obc data`, and measures it | - | - |

Baseline before #3300 (develop `623c49d2`, 4 cores, 15 GB RAM, profiles `all`, countries `CH`):

| File | SHA-256 |
| --- | --- |
| `manifest.json` (the manifest ID) | `466b2ca133aa614ee11ea25bcb1eb515e48fbb61b3572141cf38cf9a07d81ab8` |
| `overlays.sqlite` | `c22e59838e0ad0276bec09df7fcdd8d18c225297d55ca7bfaecb6ae9355e2eef` |
| `route-catalog.json` | `5d4a242598b8871e214688d4413e9455aa782db2f095e6ade580dc0bb77e6fb9` |

`manifest.json` has 9,918,794 bytes; 7,719,364 directed roads. The bake took 646 s. Peak `RssAnon`:
import 17 MiB, terrain 649 MiB, preparation 160 MiB, route catalog 1,345 MiB.

`switzerland/run-3295/result.json` (`b6487377`) and `switzerland/run-3295-fix/result.json`
(`f5f1476c`, the binary `bin/obc-data-3295`): the bakes of #3295, the same three hashes.

`switzerland/run-3296/result.json` (`77579753`, the binary `bin/obc-data-3296`, made by the gate
command): the bake of #3296, the same three hashes.

`switzerland/run-3297/result.json` and `switzerland/run-3297-1thread/result.json` (`8d4d2c73`, the
binary `bin/obc-data-3297`, made by the gate command; the second with `RAYON_NUM_THREADS=1`): the
bakes of #3297, the same three hashes. Their `stderr.log` has the time and the own peak of each
stage (`Stage ...` lines).
`switzerland/run-3297-review/result.json` (`d2c3bdff`, after the review fixes, the gate command, 4
threads): the same three hashes.

`switzerland/run-3299/result.json` (`d85613f6`, the binary `bin/obc-data-3299`, the gate command of
#3299 below): the bake of #3299 in its four steps, the same three hashes. 384 s: source 70 s, roads
74 s, costs 35 s, package 205 s. Peak `RssAnon` 22, 644, 6 and 1,288 MiB; peak work folder 1.6,
2.5, 1.7 and 1.0 GB. Stored layers: source 752 MiB, roads 1,243 MiB, costs 1,434 MiB.
`switzerland/run-3299-killed/result.json`: the same layers with `--profiles
touring,road,gravel,mtb/technical,hiking/alpine --kill-at "Stage road-pass"`: source and roads
reused, the costs step killed after 12 s. `switzerland/run-3299-profiles/result.json`: the same
command without `--kill-at`: source and roads reused, costs 15 s, package 192 s, 207 s in all.
`switzerland/run-3299-review/result.json` (`4eaa5fb9`, `bin/obc-data-3299-review`, the gate command of
#3299, after the review fixes): the same three hashes, 416 s.

## North Rhine-Westphalia (#3295)

Germany (5.9 GB PBF and about 18 GB of work files) does not fit the 30 GB disk of this container.

| Path | What | Made by | Commit |
| --- | --- | --- | --- |
| `nrw/nordrhein_westfalen-latest.osm.pbf` | download.openstreetmap.fr extract, replication timestamp 2026-11-09T00:52:38Z | `curl -o nordrhein_westfalen-latest.osm.pbf https://download.openstreetmap.fr/extracts/europe/germany/nordrhein_westfalen-latest.osm.pbf` | - |
| `nrw/nrw-sorted.osm.pbf` | the extract, sorted | `osmium sort -o nrw-sorted.osm.pbf nordrhein_westfalen-latest.osm.pbf` | - |
| `nrw/glo30/` | the 15 GLO-30 tiles N50-N52, E005-E009 | as for Switzerland | - |
| `nrw/run-3295/result.json` | bake up to the overlay index, bounds 5.86 50.32 9.46 52.53, countries `DE` | `python3 -I tools/bake_routing.py bin/obc-data-3295 nrw/nrw-sorted.osm.pbf nrw/glo30 nrw/run-3295 --bounds 5.86 50.32 9.46 52.53 --countries DE --region nordrhein-westfalen --stop-at "Writing the overlay index"` | `b6487377` |

NRW runs of #3296 (same inputs, bounds and options as `nrw/run-3295`; `B` = `bin/obc-data-3295`, `H` = `bin/obc-data-3296`):

| Path | Binary | Stop at | Other |
| --- | --- | --- | --- |
| `nrw/run-3296-base-profile/result.json` | `B` | `Preparing profile` | - |
| `nrw/run-3296-profile/result.json` | `H` | `Preparing profile` | - |
| `nrw/run-3296/result.json` | `H` | `Writing the overlay index` | - |
| `nrw/run-3296-capped/result.json` | `H` | `Writing the overlay index` | in a child memory cgroup (v1) of the bash cgroup with `memory.limit_in_bytes` 1421869056, and `OBC_DATA_MEMORY_BYTES=238616576` |
| `nrw/run-3297/result.json` | `bin/obc-data-3297` | `Writing the overlay index` | - |
| `nrw/run-3297-capped/result.json` | `bin/obc-data-3297` | `Writing the overlay index` | as `nrw/run-3296-capped` |
| `nrw/run-3299/result.json` | `bin/obc-data-3299` | `--kill-at "Writing the package"` of `tools/bake_layers.py` | the three work layers: source 102 s / 825 MiB, roads 61 s / 1,449 MiB, costs 49 s / 1,902 MiB |

## Gate command

From #3299, the routing bake is four steps, and `tools/bake_layers.py` replaces `tools/bake_routing.py`
for a binary of #3299 or later (the old script stays for older binaries). It caches the work
layers in `OUT/layers`, or in `--layers DIR` of an earlier run, by their keys, as the engine does;
`--profiles` and `--kill-at TEXT` change a late layer or stop a bake. Remove `OUT/layers` and
`OUT/output` when done:

```sh
cd /home/user/obc-bake && flock /home/user/obc-target/.build.lock python3 -I tools/bake_layers.py \
  bin/obc-data-<issue> switzerland/switzerland-bounds-sorted.osm.pbf switzerland/glo30 switzerland/run-<issue>
```

Before #3299:

Build the release binary of the change, copy it out of the shared target, and run the bake under
the build lock (about 11 minutes):

```sh
cd <worktree> && export CARGO_TARGET_DIR=/home/user/obc-target/<issue>; source ~/.obc-geos-env.sh
flock /home/user/obc-target/.build.lock ./tools/obc exec cargo build --release --locked -p obc-data-steps
cp $CARGO_TARGET_DIR/release/obc-data /home/user/obc-bake/bin/obc-data-<issue>
cd /home/user/obc-bake && flock /home/user/obc-target/.build.lock python3 -I tools/bake_routing.py \
  bin/obc-data-<issue> switzerland/switzerland-bounds-sorted.osm.pbf switzerland/glo30 switzerland/run-<issue>
```

The script prints `result.json`: compare `sha256` with the baseline. It needs `LD_LIBRARY_PATH` of
`~/.obc-geos-env.sh`. `--stop-at "Writing the overlay index"` stops a bake before the overlay index.
Remove `switzerland/run-<issue>/output` (1.5 GB) when done.

## #3300: cells change the road order (new baseline)

#3300 numbers the junctions and the roads in cell order, so the package bytes change once. Its
gate: `compare` reports 0 differences between the package of the base and the new package, and
a second bake with `RAYON_NUM_THREADS=1` gives the same partition SHA-256 and the same three
hashes. `tools/bake_layers.py` passes the option `cell_sizes` of the roads step (`--cell-sizes`,
default the sizes of `data/planner.toml`); a binary before #3300 ignores it.

New baseline (`claude/3300-cell-partition` at `b404c215`, code equal to the pull request head,
4 cores, 15 GB RAM, profiles `all`, countries `CH`):

| File | SHA-256 |
| --- | --- |
| `manifest.json` (the manifest ID) | `979675e1d72f6bfe6ce67bb67ed6ee1fb0de81a204e7f98d0d9313dce4af91a2` |
| `overlays.sqlite` | `2231583903d286232ccdba3c281ebdbc64d87550fdef14d4ce31375e0af1c952` |
| `route-catalog.json` | `a9ae51f0702839038e92fe0fd649b435e7feac059a2cb104e5ed11093a4f08fb` |

Partition SHA-256 (the `Partition:` line): `612ae937855762e0e8674f147486ec1b6c2ad381663f5de20340d17f6c70b273`.

| Path | What | Made by | Commit |
| --- | --- | --- | --- |
| `bin/obc-data-3300`, `bin/compare-3300` | release `obc-data` and `planner/router/examples/compare.rs` | `obc exec cargo build --release --locked -p obc-data-steps`; `obc exec cargo build --release -p planner-router --example compare` | `b404c215` |
| `switzerland/run-3300-base/` | the package of the base (`bin/obc-data-3299-review`), the hashes of the old baseline; only `output/routing` kept, the old package for `compare` | the gate command with `bin/obc-data-3299-review` | `4eaa5fb9` |
| `switzerland/run-3300/` | the bake of #3300, 531 s; `compare.txt` (`output` removed: `run-3300-fix/output` has the same bytes) | the gate command with `bin/obc-data-3300` | `b404c215` |
| `switzerland/run-3300-1thread/result.json` | the same with `RAYON_NUM_THREADS=1` and the source layer of `run-3300`: same partition SHA-256, same three hashes, 725 s | as `run-3300`, with `--layers` holding a link to its source layer | `b404c215` |
| `nrw/run-3300/result.json` | NRW up to the end of the roads step (bounds and options as `nrw/run-3295`) | `--kill-at "Manifest estimate"` | `b404c215` |

The gate command of #3300 (about 9 minutes for the bake, 20 minutes for `compare`, one thread):

```sh
cd /home/user/obc-bake && flock /home/user/obc-target/.build.lock python3 -I tools/bake_layers.py \
  bin/obc-data-<issue> switzerland/switzerland-bounds-sorted.osm.pbf switzerland/glo30 switzerland/run-<issue>
flock /home/user/obc-target/.build.lock bin/compare-3300 switzerland/run-3300-fix/output/routing \
  switzerland/run-<issue>/output/routing 200 1
```

`compare` of `run-3300-base` and `run-3300`: `2000 requests, 420 skipped, 0 differences`.
Partition: Switzerland 3,092,540 junctions, 7,680,912 arcs, 137.6 s (1 thread 298.6 s), own peak
140 MiB; NRW 3,991,797 junctions, 10,091,514 arcs, 273.0 s, own peak 182 MiB.

### #3300 review fixes (head `197e4f71`; the bake binary `bin/obc-data-3300-fix` at `ac16ba71`, the same code)

The flow of a phase works only on the junctions it reaches, and the RAM graph of the partition is
numbered in Morton order. The cut does not change: the same partition SHA-256 and the same three
hashes as the baseline of #3300 above, with 4 threads and with 1. `bin/compare-3300` now compares
the truncated requests too and counts them apart. It fails on a difference that is not at a tie
of the 8th and the 9th snap candidate.

| Path | What | Made by |
| --- | --- | --- |
| `switzerland/run-3300-fix/` | 4 threads, 473 s, partition 73.8 s, own peak 152 MiB; `output/routing` is the new package for `compare` (it replaces `run-3300/output`, the same bytes); `compare.txt` | the gate command with `bin/obc-data-3300-fix` |
| `switzerland/run-3300-fix-1thread/result.json` | `RAYON_NUM_THREADS=1`, source layer of `run-3300-fix`, 578 s, partition 160.9 s | as `run-3300-1thread` |
| `nrw/run-3300-fix/result.json` | NRW roads step, partition 116.1 s, own peak 197 MiB, SHA-256 as `nrw/run-3300` | as `nrw/run-3300` |

`compare 200 1` of `run-3300-base` and `run-3300-fix`: `2000 requests, 0 differences; 420
truncated, 14 differences, 14 of them at a tie` (17 minutes, one thread). In the gate command,
use `bin/compare-3300` with `switzerland/run-3300-fix/output/routing` as OLD.

## #3301: route package format 10 (branch `claude/3301-format-10`)

The bake writes format 10 beside format 9 in `cells/` of the package step. `tools/bake_layers.py`
now passes `cell_sizes` and `--download-level` (default 4) to the package step, and `result.json`
has `cells`: the SHA-256 of `cells/directory.bin` (the identity of format 10), the file count and
the bytes. The format 9 gate is unchanged: the three hashes of the #3300 baseline.

| Path | What | Made by | Commit |
| --- | --- | --- | --- |
| `bin/obc-data-3301`, `bin/cells-3301` | release `obc-data` (rebuilt at `e798e41f`) and `planner/router-build/examples/cells.rs`, zstd level 12 | `obc exec cargo build --release --locked -p obc-data-steps`; `obc exec cargo build --release -p planner-router-build --example cells` | `e9f83794` |
| `switzerland/run-3301/result.json` | the gate bake with zstd level 19 (`84bec105`..`09b38647`): the three hashes of the #3300 baseline, 551 s; format 10 identity `742b5a49…`, 460,559,288 bytes. Layers and output removed | the gate command with the binary of that commit | `09b38647` |
| `switzerland/run-3301-level12/result.json` | the package step again with zstd level 12 (source rebuilt, roads and costs reused): the three hashes of the #3300 baseline; format 10 identity `bbfb0484a5278cdb4052c49ced217e29a197fdbeb1bcd0762482bf282c286b16`, 148 files, 484,705,107 bytes. Output removed | the gate command with `--layers switzerland/run-3301/layers` | `e9f83794` |
| `switzerland/run-3301-level12/measure/` | `write-1thread.txt`, `write-4threads.txt` (level 19: the same directory hash with 1 and 4 threads), `write-level12.txt`, `install-1worker.txt`, `install-4workers.txt` | `bin/cells-3301 write ROADS COSTS OUT [12]`, `RAYON_NUM_THREADS=1` for the first; `bin/cells-3301 install OUT TARGET 1` and `4` | `09b38647`, `e9f83794` |

Format 10 of Switzerland (7,719,364 roads, 4 cores, 15 GB RAM, download level 4): 146 objects (18
download cells × 8 parts, 2 upper cells), 404 B to 35.5 MB, median 1.14 MB. Installed / published
bytes for each road: topology 8.05 / 2.54, geometry 54.07 / 41.06, 6 profile groups 55.41 /
19.19 (turn costs 23.58, cost classes 9.85, road costs 20.28, snap bits 1.26 installed), in all
117.53 / 62.79. Install of all cells: 1 worker 9.1 s, peak RssAnon 17 MiB; 4 workers 2.9 s, 88
MiB; check 8.1 s and 2.2 s. ruzstd alone decodes 145 MB/s on one core.

To repeat the gate: the gate command of #3300 with `bin/obc-data-3301`, then
`bin/cells-3301 write LAYERS/roads-*/output/roads LAYERS/costs-*/output/costs OUT` with and without
`RAYON_NUM_THREADS=1`, and compare `sha256sum OUT/directory.bin`.

## #3381: the time of the partition

`3381-results.log` has each partition run of #3381 (4 cores, 15 GB RAM): the `Partition:` line and,
for the binaries with the step-1 counters, one line for each level of the recursion and for each
direction of the first cut. The run folders `{switzerland,nrw}/run-3381-<label>-t<threads>/` keep
`stderr.log` and `result.json`; their layers are removed. A run reuses a source layer and stops
the roads step after the partition:

```sh
cd /home/user/obc-bake && RAYON_NUM_THREADS=<1|4> flock /home/user/obc-target/.build.lock python3 -I tools/bake_layers.py \
  bin/obc-data-3381 switzerland/switzerland-bounds-sorted.osm.pbf switzerland/glo30 switzerland/run-3381-<label>-t<threads> \
  --layers switzerland/layers-3381 --kill-at "Stage partition"
```

| Path | What | Commit |
| --- | --- | --- |
| `bin/obc-data-3381` | release `obc-data` of the pull request head (`obc exec cargo build --release --locked -p obc-data-steps`) | `fdd3f31d` |
| `*/run-3381-final-t4`, `*/run-3381-final-t1` | the head: Switzerland 59.7 s (1 thread 121.3 s), NRW 98.2 s (1 thread 195.5 s), partition SHA-256 of #3300, own peak 152 and 197 MiB | `fdd3f31d` |

No third region: the disk had 6.6 GB free, under the 4 GB floor plus a larger extract and its layers.

`switzerland/run-3301-review/result.json` (`e798e41f`, after the review fixes of #3386, the gate
command with `bin/obc-data-3301` rebuilt at that commit, 517 s): the three hashes of the #3300
baseline and the same format 10 identity `bbfb0484…`, 148 files, 484,705,107 bytes. Layers and
output removed.

`switzerland/run-3301-ruzstd/result.json` (`8cb6daae`, ruzstd 0.9.1 for RUSTSEC-2026-0344; the gate
command): the three hashes of the #3300 baseline and the identity `bbfb0484…`. On its objects,
ruzstd 0.9.1 alone decodes 141–142 MB/s on one core; `bin/cells-3301 install` 1 worker 10.0 s
(peak RssAnon 9 MiB), 4 workers 2.6 s (41 MiB); check 7.6 s and 2.1 s. Layers, output and
installs removed.

## Shared packages of Switzerland (#3302): `switzerland/packages-3301/`

One bake of the merged #3301 for the next sub-issues. Do not change these files; make a copy to
change one.

| Path | What | Made by | Commit |
| --- | --- | --- | --- |
| `switzerland/packages-3301/format-9/` | the format 9 package (`manifest.json`, `pages.*`, `overlays.sqlite`, `route-catalog.json`): the three hashes of the #3300 baseline | the gate command with `bin/obc-data-3301` (OUT `run-3302-bake`, 560 s), then `output/routing` moved here | `e798e41f` |
| `switzerland/packages-3301/format-10/` | the 146 objects, `package.json` and `directory.bin` of format 10: identity `bbfb0484…` | the same bake, `output/cells` moved here | `e798e41f` |
| `switzerland/packages-3301/install/` | an install of all cells and all groups (866 MB) | `bin/cells-3301 install format-10 install 4` (2.8 s) | `e798e41f` |
| `switzerland/packages-3301/bake-result.json`, `bake.log` | `result.json` and the log of that bake | - | - |

The layers, `blocks`, `routes` and the work folder of that bake are removed.

`switzerland/run-3302/` (#3302, `claude/3302-region-data-layer` at `59d3c8ef`, 4 cores, 15 GB RAM):
`compare.txt` (`Region` on `install` against `Selection` on `format-9`: each road, each 7th road for
each profile, 40,000 snaps; all equal), `first.txt` (open and the first request point, cold and
warm), `routes.txt` (the route IDs of the 20 requests in `requests/` with `alternatives: true`,
before and after #3302: all equal), and `region_gate.rs`, the uncommitted program of the first two
(copy it to `planner/router/examples/` and build it with `--release --example region_gate`).
`switzerland/run-3302/compare-review.txt` (`34f469ea`, after the review of #3401): the same comparison,
all equal; endpoints format 9 102.7 s / format 10 87.5 s, snaps 387.0 s / 239.0 s.
`switzerland/run-3302/compare-delta.txt` (`d20bab38`, after the delta review): endpoints only, all
equal; format 9 88.3 s / format 10 89.3 s.

## #3304: the cost tables (branch `claude/3304-cost-tables`, PR #3424, head `20567d67`)

The costs step now also writes the cost tables; format 10 holds them in the group parts and each
upper cell has a group part for each group. Format 9 gate unchanged (the three hashes of #3300).
New format 10 identity: `004a933baa66d7c2a2dcd9eb5c1f291ac989e1855e087273286470c8bfb86aae`, 160
files, 592,716,349 bytes (4 cores, 15 GB RAM, profiles `all`, countries `CH`, 441 s).

| Path | What | Made by | Commit |
| --- | --- | --- | --- |
| `bin/obc-data-3304`, `bin/cells-3304`, `bin/tables-gate-3304` | release `obc-data`, `examples/cells.rs` (with `cells tables ROADS COSTS`), and the uncommitted value check | `obc exec cargo build --release --locked -p obc-data-steps`; `--release -p planner-router-build --example cells`; the third from `switzerland/run-3304/tables_gate.rs` copied to `planner/router-build/examples/` | `87c5d06b` |
| `switzerland/run-3304/` | `result.json`, `stderr.log`, `figures.txt` (the `Tables:` line of each level), `measure/` (`tables-1thread.txt`, `tables-4threads.txt`, `write-*.txt`, `exactness.txt` sampled, `exactness-all.txt` every entry: 50,270,700 checked, 0 differ, `install-4workers.txt`), `tables_gate.rs`. Layers and output removed | the gate command with `bin/obc-data-3304`; `bin/tables-gate-3304 ROADS COSTS` (`ALL=1` for every entry) | `87c5d06b` |
| `switzerland/packages-3304/` | the package with tables for #3305: `format-10/` (identity `004a933b…`), `install/` (all cells and groups, 1,006 MB), `bake-result.json`, `bake.log`. Format 9 is unchanged: use `packages-3301/format-9` | the bake above, `output/cells` moved here; `bin/cells-3304 install format-10 install 4` | `87c5d06b` |
| `nrw/run-3304/` | NRW up to the end of the costs step, `figures.txt`, `measure/` (1 thread, sampled value check: 798,370 entries, 0 differ). Layers removed | `--kill-at "Writing the package"`, bounds and options as `nrw/run-3295` | `87c5d06b` |

## Removed to free disk

The disk of the container is small. These were removed; their `result.json` files and hashes stay:

- `switzerland/switzerland-latest.osm.pbf` and `nrw/nordrhein_westfalen-latest.osm.pbf` (the unsorted
  downloads; the bakes read only `switzerland-bounds-sorted.osm.pbf` and `nrw-sorted.osm.pbf`, which
  stay; a new download has a newer replication timestamp and gives other bytes).
- The package output of `switzerland/run-3300-base` (spatial order) and `switzerland/run-3300-fix`
  (the format-9 package in cell order is in `switzerland/packages-3301/format-9`).
- The binaries of #3295 to #3300 and of develop `623c49d2` in `bin/`.
`switzerland/run-3304-merged/` (review fixes of #3424, head `8f774d2e`, merged with `routing-overlay` `f3f200e3`; `bin/obc-data-3304-fix`, `bin/cells-3304-fix`): the three hashes of #3300, identity `004a933b…`, the same table hashes for each level; `measure/` has the times of each level before (`bin/cells-3304`) and after, 1 and 4 threads, and a sampled value check (592,010 entries, 0 differ). Layers and output removed. `switzerland/run-3304-fix/measure/` holds the runs of the intermediate head.
