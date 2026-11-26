# obc data prepared sources

The versions of the prepared sources in the archive, the commands that make and copy them, and
the producer `terrain`, whose rules the other [producers](obc-data-producers.md) share.

- Writers: `host/obc-data` (`source prepare`, `source copy`, `unlock --source`); the producers of
  the `obc data` binary of `host/obc-data-steps`.
- Readers: `host/obc-data` (the fetch of a prepared source).
- Code authority: `host/obc-data` (`prepared.rs`); `host/obc-data-steps` (the producers).
- Related: [archive](obc-data-archive.md), [products](obc-data-products.md),
  [OBCT](obct-terrain.md).

## Prepared sources

A prepared source is a source whose versions `obc data source prepare` makes in a long job, such
as the processing of large dumps. A version is a fixed set of files in the [archive](obc-data-archive.md#archive),
and the archive is the only place that keeps it: no target holds a version. A bake reads a version
as it reads any other source.

A prepared source is a row of `data/sources.toml` with `fetch.kind = "prepared"`. The `obc data`
binary links a producer for each name: the producer of `<name>` makes the source `<name>`. Only a
source whose licence lets us give its bytes to others is prepared.

The name of a version is the UTC date when its first run starts, `YYYY-MM-DD`. A later version of
the same day adds `.2`, `.3` and so on: `2026-10-28`, `2026-10-28.2`, `2026-10-28.10`. Versions
are in the order of the date, then of the number; a plain date is number 1.

The home of a prepared source is the folder `prepared/<id>/` of the first root of
`OBC_DATA_ARCHIVE` that has it. Each version of the source is in its home, because a version can
keep the files of an earlier one (`reused`). When a later root also has files in
`prepared/<id>/`, each command that reads or writes the source warns: only the home serves. The
home holds:

| Path | Holds |
| --- | --- |
| `prepared/<id>/lock.json` | The lock of the source: one prepare or copy at a time. It has the format of the [lock](obc-data-targets.md#lock) of a target |
| `prepared/<id>/versions.json` | The complete versions, oldest first |
| `prepared/<id>/<version>/<path>` | A file of the version. Immutable |
| `prepared/<id>/<version>/manifest.json` | The manifest of the version, written last. Immutable |

A version is complete when it has its manifest.

```json
[
  {
    "version": "2026-10-12",
    "manifest_sha256": "<sha256 of manifest.json>",
    "format": 1,
    "bytes": 1048576,
    "time": "2026-10-19T08:00:00Z"
  }
]
```

`format` is the `format` of the manifest, `bytes` is the size of the files of the version, and
`time` is when it was complete. The manifest
is compact JSON with the keys of each object in byte order:

```json
{
  "attribution": "<credit>",
  "cost": {"peak_rss_bytes": 8589934592, "wall_ms": 3600000},
  "files": [{"path": "common.tar.zst", "sha256": "<sha256>", "size": 1048576}],
  "format": 1,
  "inputs": [{"digest": "<digest>", "source": "wikidata", "version": "2026-10-01"}],
  "languages": ["en", "de"],
  "licence": "<SPDX expression>",
  "machine": "<machine>",
  "revision": 1,
  "scope": null,
  "source": "wiki",
  "time": "2026-10-19T08:00:00Z",
  "version": "2026-10-12"
}
```

| Key | Meaning |
| --- | --- |
| `source`, `version` | The id of the source and the version |
| `format` | The number that readers check |
| `revision` | A person changes it in the change that improves the output |
| `inputs` | Each source version that the prepare read, sorted by source and version, with the [digest](obc-data-layers.md#keys) of the files that it read |
| `files` | Sorted by `path`. A path is relative, and is not `manifest.json` |
| `reused` | Absent, or each path of `files` that an earlier version of the same source holds, with that version: the file is at `<that version>/<path>`. A version that extends the one before keeps its files without a copy |
| `scope` | `null` for every place that the source covers, or `{"region", "definitions"}` as in the [settings of a target](obc-data-registry.md#settings-of-a-target) |
| `extends` | Absent, or the scopes of the earlier versions that this version extends, oldest first: the version keeps their content, as its producer says. A tile producer keeps their files outside its own `scope`; [`wiki`](obc-data-producers.md#wiki) rewrites their shards with one form of each fact |
| `languages` | Article language codes in supported order, each once; empty for a source without articles |
| `publish_text` | Absent, or the `publish_text` of the row in `data/sources.toml` |
| `licence`, `attribution` | The licence and the credit of the row in `data/sources.toml` |
| `machine`, `time` | The machine that made the version, and when the version was complete |
| `cost` | The wall time of the last run of the version, and the peak memory of its process or, when that is more, of the largest program that it ran times the most programs that ran at once (`null` when the system does not report it) |

## Prepare

`obc data source prepare ID` makes a version in the home of the source:

1. The prepare refuses while `OBC_DATA_ARCHIVE` names no root, and while a root of it does not
   exist, such as a drive that is not mounted: that root can be the home. A source without a home
   gets one in the first root with 4 GiB free. To put it in another root, a person makes the empty
   folder `prepared/<id>/` there first. A run that made the home and then refuses or fails
   removes the home again while it holds only the lock. While
   a target of `data/targets.toml` lists a version in its legacy folder `sources/<id>/` that the
   home lacks, with its manifest SHA-256 and that manifest, or cannot be read, the prepare
   refuses: [copy](#copy) it first.
2. A new version takes the first name of the day that is after each version of that day in
   `versions.json` and has no manifest in the home. A run keeps its work in
   `prepared/<id>/<version>/` of the [store](obc-data-store.md#store) of its machine. The same command
   resumes the version: the producer does not make a file of the journal again, unless the home
   lacks it. A resume with another archive root, region or language selection refuses. When the
   manifest of a stopped version is in the home and names the files of its journal, its run stopped after the manifest: the run
   only adds the version to `versions.json`, and the review says `listing`. Another stopped version
   that is not newer than the newest complete version in the home, or whose manifest is in the
   home, is not resumed: the run starts a new version, deletes the work of the old one, and the
   review names it in `abandoned`. So a run never writes a file of a complete version.
3. The review names the root of the home, the version, the inputs with the size of their files in
   the store or on this machine (`unknown` lists an input whose size neither knows), the files of
   the version at most (`files`, `null` when the producer cannot tell before the run), the files
   that earlier runs moved into the home, the cost from the newest version in the home, only when
   its scope and languages match, the bytes that go into the store, with the disk that the
   producer says its programs take, and into the archive (unknown without such a version), the
   check of this machine as in the [review](obc-data-targets.md#review) of an update, which also refuses when the source or an input has a
   `credential` that is not on the machine, or when the root lacks the free space for the bytes of
   the version and 4 GiB more, and for each target of `data/targets.toml` the
   layers of its serving release that read an older version. `--dry-run` writes it. Its id is the
   first 12 hex digits of the SHA-256 of the canonical JSON of the source, the root, the
   version, the format, the revision, the scope, the languages, `publish_text` and each planned
   input, including its archive path, URL, size and object identity. With `--expect ID`, the
   prepare starts only when the review has that id.
4. A detached run, as [Runs](obc-data-layers.md#runs) says, takes the lock of the source as an update takes the lock
   of a target. It makes the review again under the lock, and refuses a review that is not
   `--expect`.
5. A run of a new version deletes the folders of older versions of the source in the home that
   have no manifest: no run resumes them. Before each 100 deletes, it checks that it still holds
   the lock. A version that `versions.json` lists and that has no manifest refuses the run: its
   manifest is lost, and the fix restores it from the backup.
6. The producer writes each file. `terrain`, `forest`, `climate` and `snow` run the programs of
   eight tiles at a time, `colour` of four. The run moves each file into the home at once, records its SHA-256 in the
   journal and deletes it from the work. No reader reads a version before its manifest, so the
   move replaces a file that a stopped run left with other bytes. The run shows each tile that a
   program makes as a step `<id>/<tile>` of the layer `<id>`, with its time, CPU, peak memory and
   the bytes of its files.
7. The run checks that it still holds the lock, and that the home has each file of the journal
   with its size. It writes the manifest only when the file is absent: a manifest that an earlier
   run of the version wrote stays when it names the same files. Then a conditional write adds the
   version to `versions.json`, with the time of the manifest, and the work of the version goes.

`--region` and `--box` make a version of that region only. `obc data runs RUN --stop` stops a run,
and the same command resumes it. `obc data unlock --source ID --from MACHINE` frees a lock that a stopped run left.

A test of a producer uses a root of its own: name it first in `OBC_DATA_ARCHIVE`, and make the
empty folder `prepared/<id>/` in it. Then the prepare and each update with that variable use the
versions of the test root, and the other roots still serve their datasets.

## Read

The archive is the reference: each target reads the versions that the home lists. Before an
update or the [overview](obc-data-cli.md#overview) lists the steps, each prepared source (an update) or each
that the steps read (the overview) gets a version in the `format` of the producer of this
`obc data`: the serving version of a `manual` source, else the newest version of the home. So a
new version makes each layer that reads an older one stale, with the cause `source`, in each
target. When no root has the home, such as on a machine without the
archive, a target keeps the version that it serves, and a bake from the store needs no archive.
When the home lists no version in that format and the target serves none, the listing blocks the
layers that read the source (`blocked`), and the fix names `OBC_DATA_ARCHIVE` and the command that
prepares one. A product can bake a layer without the source instead, as `device/terrain` does without a national terrain model: then a note
of the [review](obc-data-targets.md#review) names the source and the command.

The manifest is the identity of a version: a step reads `<version>/<manifest sha256>`, and the
record in the store, the `sources` entry of a release
and the fetch name it. Text output shows it as `<version> (<first 12 hex digits
of the SHA-256>)`; with `--json`, a command gives the whole version.

A step names the files of a version that it reads; an update refuses a step that names none. The
fetch reads the manifest of the version in the home, and refuses one with another SHA-256. It
copies only the named files that the store lacks, never as a link, and checks the SHA-256 of each
against the manifest: a file with other bytes fails the fetch. The record of the version in the
store has the manifest and these files, and the `url` of each is its path under the root. When no
root has the manifest, the fetch refuses (`blocked`), and the fix names `OBC_DATA_ARCHIVE`.

## Delete

No command deletes a complete version: the archive is its only copy, and a release that a target
serves or keeps for its rollback can read any version. A [collection](obc-data-targets.md#collection) of a target and
a [clean](obc-data-store.md#clean) of the store never change the archive. The store of a machine keeps the files of
a version that a release of one of its targets reads, as [Clean](obc-data-store.md#clean) says.

## Copy

`obc data source copy ID --from TARGET` copies the versions of a prepared source from the legacy
folder `sources/<id>/` of a target into its home. That folder has the layout of the home. The
copy only reads the target.

- The home is that of a prepare, and so are its refusals. The copy is a detached [run](obc-data-layers.md#runs) that
  holds the lock of the source.
- It takes the versions of `sources/<id>/versions.json`, oldest first. A version that the home
  lists with the same manifest SHA-256 is copied. A version that the home lists with another one,
  or a manifest whose SHA-256 is not that of its entry, refuses the copy before it writes a file.
- Each file of a version that the home lacks downloads into the store, with the check of its
  SHA-256 against the manifest, and then goes into the home as a copy. A file that the home holds
  with another size refuses the copy; the fix deletes it. Of a version that the home does not
  list, the run reads each file that the home held before and refuses one with another SHA-256.
  Each file that the version reuses must be in the home. Then the copy writes the manifest, adds
  the version to `versions.json`, and deletes the `.part` files that a stopped copy left in the
  folder of the version. A copy that stops resumes.
- `--dry-run` writes the root of the home and, for each version, the files and the bytes to copy
  and those in place, and changes nothing. The run refuses when the root or the store lacks the
  free space for the bytes to copy and 4 GiB more; when they share a file system, it needs the
  space twice there.
- The files stay in the store; a [clean](obc-data-store.md#clean) deletes those that no release reads.

## `terrain`

The producer `terrain`, format 4, makes the blocks of the reference archive
([`obct-terrain.md` §10](obct-terrain.md#10-the-reference-archive)) of the national terrain models `dtm-<key>`, in two pools: the
maximum, which `device/terrain` reads, and under `mean/` the mean, which `planner/terrain` reads.
A version adds a region to the newest complete version of `terrain` in the home, in the same
format.

- A block is 4 × 4 archive tiles, `2^18` µdeg on a side, about 29 km. Block `(bi, bj)` holds the
  tiles `(ti, tj)` with `ti >> 2 = bi` and `tj >> 2 = bj`.
- The models are the `dtm-*` rows with `redistribute = true`. The tiles of a model are those that
  meet its `extent` and the `geofabrik-poly` polygon of one of its `areas`: the boundary of the
  polygon crosses the tile, or the polygon holds the centre of the tile.
- The blocks of a region are the blocks with a tile of a model that a box or a `geofabrik-poly`
  polygon of a leaf of the region meets; for a version of everything, each block with a tile of a
  model. A run keeps the version of each polygon that the first run of the version read. A
  version makes each block of its region from each model with a tile in it, also from a model of
  a country that the region does not name. So the pixels of a block do not depend on the region.
  Each other block of the version before stays in `reused`; the manifest names the scopes of the
  versions before in `extends`. The review counts the two pools of the blocks that the boxes of the
  region meet, and `index.json`; for a region with a `geofabrik` leaf, `files` is `null`. The review
  names the models with a tile in a block that the boxes meet; for a region without boxes, the
  models of its countries and each model whose `extent` meets the `extent` of one of them, or every
  model when the region names no countries.
- For each block, `host/obc-dem/reference/ingest.py fetch --block <bi:04>-<bj:04>
  <ti:04>-<tj:04>=<keys> …` pools the rasters of each model of each tile of the block onto the
  lattice in one pass, and folds the models of the tile worst first by `PRIORITY`: a model keeps
  each pixel where it has a height. Before it
  uses a model whose row has a `summit`, the program pools the tiles near that summit and refuses
  the model when the highest height within about 1 km of it is more than 10 m from the surveyed
  height, and names a check that finds no height. `--shared`, one directory for the blocks of a
  prepare, keeps the passed checks. Each lattice pixel that no
  centre of a model reaches and that lies between four of its centres with a height takes their
  bilinear height. A model whose step is longer than the east side of a lattice pixel in the tile
  asks for the tile with two of its steps of margin. A raster of one row or one column fills
  nothing. The program writes `18/<bi:04>/<bj:04>.tif` and `mean/18/<bi:04>/<bj:04>.tif`, then
  `<bi:04>-<bj:04>.tiles`, a JSON object of each tile `<ti:04>/<tj:04>` of the block with a height:
  `sources`, its sources best first, and `samples`, the bias samples of its mean: the decimetres of
  the 16 × 16 pixels whose row and column are 32 modulo 64, row by row from the south, `-` where the
  tile has no height. Then it writes `<bi:04>-<bj:04>.mean.sha256` and `<bi:04>-<bj:04>.sha256`,
  the SHA-256 of the pixels of the two blocks. Where no model has a height in the block, it writes
  the empty `<bi:04>-<bj:04>.none`. The catalogues of `se`, `de-ni`, `pt` and `pt-20` can name a
  file that the server does not hold. A 404 of such a file gets the retries of a fault; then the
  program goes on without the file and states `<key> <file>` in `missing` of the tile in the mark,
  with no `sources` where the tile has no height. Two such files in one fetch stop the run, and so
  does such a file at the `summit` of the model. It asks a service only for the part of a tile in the `extent`,
  and it removes the raw rasters of a tile after it pools the tile. The run keeps the marks of each
  block until the version is complete, so a resume fetches no block again that it has.
- A block is a GeoTIFF of 4096 × 4096 lattice pixels with the encoding of an archive tile and
  256 × 256 internal tiles. Tile `(ti, tj)` is the window of 1024 × 1024 pixels that starts
  `(4 bi + 3 − ti) × 1024` rows below the north edge and `(tj − 4 bj) × 1024` columns east of the
  west edge. Each pixel of a tile without a height is nodata. The SHA-256 of the pixels of a block
  is over its rows north first, little endian.
- The run moves the files into the home in batches of 64.
- Then the run measures the GLO-30 bias of each model that is the best source of a tile and that
  the index of the version before does not state: the median of model − GLO-30, to the
  centimetre, over the samples of each tile whose best source it is. The samples of a tile that
  the run made are in the mark of its block; those of a tile of the version before come from its
  mean block. GLO-30 is bilinear at the sample, from the squares of `copernicus-glo-30` that
  `copernicus-glo-30-tiles` names; a land square that the fetch does not give is an error. A
  sample in a square that the list does not name is at sea and does not count, and the run logs
  how many. A model with fewer than 1024 samples with GLO-30 has no bias, and a later version
  measures it again. A model keeps the bias of the version before. A version that extends no
  version, such as the first version of a format, measures every model over all its tiles.
- The plan of `planner/reference` reads `index.json` (a fetch of a prepared version with the param
  `file` gives that file too). It refuses a version that states no bias of the best model of a
  tile that it reads, and names the fix: a prepare of a box of about four tiles in the middle of
  the `extent` of that model extends the version and measures it, such as
  `obc data source prepare terrain --box 8.10,46.75,8.30,46.95 --dry-run` for `ch`.
- Last, `ingest.py index-terrain` writes `index.json`, one for the two pools: the index of the
  version before, where the tiles of each block that the run made replace the tiles of that block,
  and the facts of each model that the run read with `fetched`, the date of the version. The facts
  of a model hold `glo30_bias_m`, its bias in metres: the bias of the version before, else the one
  that the run measured (`--biases`). The index is compact JSON with the keys in byte order:

  ```json
  {"block_log2":18,"contributors":{"3410/2882":["ch","es"]},
   "sources":{"ch":{"attribution":"© swisstopo","fetched":"2026-09-21","glo30_bias_m":-1.42,
   "licence":"…","product":"swissALTI3D 2 m","vertical_datum":"LN02/LHN95"}},
   "step_log2":6,"tile_log2":16}
  ```

  `contributors` names each tile with a height and its sources, best first; `sources` names each
  source of a tile. `missing`, left out when empty, is the ledger: each tile and the files that its
  models lack. A version that makes the block of a tile again replaces its entry, so a prepare of a
  box in the tile, when the file is back, removes it. The run logs the ledger when it ends. The manifest names the input `dtm-<key>` of each model at that date, with the
  [digest](obc-data-layers.md#keys) of the pixel SHA-256 of the blocks that it made.
- Only the machine that prepares needs the `credential` of each model. A `by-hand` model reads
  its delivery: `OBC_REFERENCE_<KEY>_INPUT` names its directory, as an absolute path, and
  `OBC_REFERENCE_<KEY>_DATUM` the vertical datum that the metadata of the order states.
- No run writes `reference/v1/`.
