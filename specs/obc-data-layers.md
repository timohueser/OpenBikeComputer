# obc data layers

The layers that a bake makes: their declarations, keys, receipts, plans, runs and versions, and
the state of a layer.

- Writers: `host/obc-data` (engine, receipts, runs).
- Readers: `host/obc-data`, the steps of `host/obc-data-steps`.
- Code authority: `host/obc-data` (`engine.rs`, `product.rs`).
- Related: [products](obc-data-products.md), [store](obc-data-store.md),
  [registry](obc-data-registry.md), [targets](obc-data-targets.md).

## Layers

A layer is data of the store with a version. An operator names an operator layer, such as
`device/map`. An internal layer, such as `osm`, has no readers: only the steps of other layers
read it. One or more steps make a layer, and the version of the layer applies to each of them. A
[plan](#plan) lists the steps to build, and a [run](#runs) builds them in dependency order. The
engine builds a step only when the store has no receipt for the key of the step, or when an
object of that receipt is missing.

The `obc data` binary declares each layer beside the steps that make it:

| Field | Meaning |
| --- | --- |
| `name` | Lowercase kebab-case segments joined by `/` |
| `shape` | `tiled`: built per grid leaf. `whole`: built once over the full coverage. `whole` with named partitions: the last part of the name of each step names a declared partition, else the plan fails. A partition stales, bakes and is published on its own, as a step is |
| `format` | The number that the readers of its files check. Where a reader has a constant, the layer uses it: `obct::SURFACE_VERSION` for `device/terrain`, `obcm::VERSION` for `device/map`, the router `FORMAT` and `CATALOG_SCHEMA_VERSION`. `device/landmarks` has the sum of `obcm::landmarks::SECTION_VERSION` and `obcm::peaks::VERSION`: each only grows. Another layer has the format that its spec states, or 1 |
| `revision` | The number that a person changes in the change that improves the output. It starts at 1 |
| `readers` | The programs or layers that read its files. An internal layer has none |
| `cache` | `true` for a work layer that a later step reads: a [collection](obc-data-store.md#clean) keeps only the layer of each of its steps and options that a run used last, and a release does not keep it. `false` for each other layer |

| Layer | Steps |
| --- | --- |
| `osm` | `device/source/<area>`, `device/region-osm`, `device/osm`, `planner/source/<area>`, `planner/osm`, `planner/osm/bounds`, `planner/terrain-osm/<area>`, `planner/terrain-osm` |
| `device/terrain` | `device/reference/<i>-<j>`, `device/terrain/<i>-<j>` |
| `device/map` | `device/map/<i>-<j>` |
| `device/landmarks` | `device/landmark-content…`, `device/peak-content…`, `device/landmarks/<i>-<j>`, `device/peaks/<i>-<j>` |
| `device/catalog` | `device/catalog` |
| `planner/basemap` | `planner/basemap`, `planner/assets` and their grids |
| `planner/terrain` | `planner/reference`, `planner/terrain` and its grid |
| `planner/places`, `planner/route-facts`, `planner/climate`, `planner/snow`, `planner/trailview-rules`, `planner/trailview-colour`, `planner/ways` | The step of the same name and its grids |
| `planner/trailview-features` | `planner/trailview-features`, `planner/trailview-stamps` and their grids |
| `planner/trailview-ground` | `planner/trailview-material`, `planner/trailview-lithology`, `planner/trailview-region`, `planner/trailview-ground` and its grid |
| `planner/routing-source`, `planner/routing-roads`, `planner/routing-costs` | The step of the same name. Internal, `cache`. Their files hold router types, so their format is the router `FORMAT` plus 1 |
| `planner/routing-ways` | The step of the same name. Internal, `cache`. Its format is the `user_version` of `ways.sqlite` |
| `planner/routing` | `planner/route-sources`, `planner/routing`, `planner/overlays` and their grids |
| `planner/search` | `planner/search/…`, `planner/model` and their grids |
| `planner/index` | `planner/fonts/grid`, `planner/offline/grid`, `planner/index`, `planner/catalog` |

A step declares:

| Field | Meaning |
| --- | --- |
| `layer` | The layer that it makes part of |
| `name` | Lowercase kebab-case segments joined by `/`. A step that two products list has the same declaration in both |
| `inputs` | Snapshots, as `source`, `version`, `params` and `files`. With `params` (the `NAME=VALUE` of the fetch), the step reads the files that a fetch with them gives, and `files` is empty. Without, `files` names the files that the step reads, or is empty for every file. The files of other steps, as `name` and `files`: the paths that the step reads, or none for every file. A selected path that the other step does not have fails the step |
| `options` | A JSON object |
| `outputs` | Paths in the output directory. A path is a file, or a directory whose files are all part of the layer. The step must write each path and no other file. A symbolic link fails the step |
| `client` | `"none"`, `"all"` or `{"paths": [<output>]}`. Selected paths name declared outputs: a file, or a directory and its files. Paths are sorted and unique. An empty selection or a path outside `outputs` fails the plan |
| `run` | A Rust function of the binary, a command (a program and its arguments), or a Python command. No argument names a path outside the repository root: no argument is an absolute path, contains `=/` or has a `..` segment between `/` and `=` |

Each step runs in a child process. A Rust step runs the binary again as `<binary> obc_data_step
<function>`, and that process runs the function that the binary registered with that name
(`obc_data::engine::child`). A function name is `<crate>/<function>` in kebab-case. One binary
links the Rust steps of every product, so Cargo unifies their features. A step crate enables
every feature its bytes depend on itself, or makes its bytes independent of it (structs, or
sorted keys for JSON objects).

A Python command runs `uv run`. The engine sets `UV_PYTHON` to the interpreter that `uv python
find --system --offline --no-python-downloads` selects for the project, with an explicit
`UV_PYTHON` request when set, and sets `UV_NO_SYNC=0`.

## Keys

The digest of a list of files is the SHA-256 of the text that `sha256sum` writes for them: one
line `<sha256>  <name>` with a final newline per file, in byte order of the names.

- The digest of a snapshot input lists its selected files by `name`.
- The digest of the files of a step lists them by `path`. The digest of a step input lists the
  files that it selects.

The key is the SHA-256 of this JSON object, as the compact output of `serde_json` with the keys
of each object in byte order:

| Key | Value |
| --- | --- |
| `layer` | The name of the layer of the step |
| `step` | The name of the step |
| `format`, `revision` | The version of the layer |
| `inputs` | One `{"kind", "name", "digest"}` per input, sorted by `kind`, then `name`. `kind` is `snapshot` or `step`; `name` is the source id or the step name |
| `options` | The options |

Code is not in the key. A change of code with the same version reuses the files of the key. A
change that gives other output changes the `revision` of its layer; a change of the readers
changes its `format`.

A step input enters a key with its digest, not with its key. A snapshot version enters a key the same way, by the digest of its files.

The recipe of a step is the SHA-256 of the same object without digests and with `client`: each
input as `{"kind", "name", "files"}` for a step and `{"kind", "name", "version", "params",
"files"}` for a snapshot, `params` and `files` sorted, and the inputs sorted by their compact
JSON. It holds no digest, so a plan has it also for a step whose key waits for a fetch or
another build.

## The step contract

A step gets a request as JSON on standard input. The child process of a Rust step reads it and
gives the same fields to its function.

| Key | Value |
| --- | --- |
| `step` | The name of the step |
| `snapshots` | `{source: {file name: object path}}` |
| `layers` | `{step name: {path: object path}}`, with the files that the input selects |
| `layer_files` | `{step name: [{path, size, sha256}, …]}`, from receipts for exactly the same selected files |
| `options` | The options |
| `output` | An empty directory. The layer is the files that the step writes in it |
| `metrics` | A path. The step can write a JSON object there, for example the size of each section |

A step starts in the repository root, in a process group of its own. On Linux it gets SIGKILL
when the engine ends; the programs that it started can stay. Its standard output and
standard error go to the standard error of the engine. Exit status 0 is success. When a Rust
function fails, its child process writes the error as the last line of standard error and exits
with 1. The objects are read-only. While a step runs, `output` and `metrics` are in
`partial/layer-<key>/`; the engine removes that directory when the step ends, also when it fails.
`TMPDIR` of a step is the empty directory `tmp` there, beside `output`; a step makes no other file
with that name. A program puts no socket in `TMPDIR`: its path can be longer than the 108 bytes of
a socket path. A failed step writes no receipt. A declared directory may be empty when the step
creates it. A declared output that does not exist is an error.

## Offline

A step reads the data inputs of its request. A step does not use the network; fetchers are the
only network users. A step must not write to its inputs: their paths, and the links of a view, are objects of the store, and a step that runs as
root can write to a read-only object. The engine does not enforce this.
A step that needs a package or a tool finds it installed, or reads it as a snapshot.

The `protomaps-basemaps` GitHub fetch needs a full commit SHA and takes no parameters. It
stores the source archive and `basemap-<recipe SHA-256>.jar`, built by Maven under Java 21.
The recipe hash identifies `tools/basemap_tool.py`. A step selects that jar only; other
preparation recipes in the source snapshot do not change its inputs. The source POM
must pin a Planetiler release. Tool preparation may download Maven packages. The jar holds
the resolved dependencies, so their bytes are part of the basemap input identity. The bake
reads only the prepared jar and the data snapshots. It validates every input file before
Java starts, and passes `--download=false`.

For a tool that reads a directory, `engine::view` in Rust and `view` of `tools/step_request.py`
in Python make a new directory with one symbolic link per file of an input, to its object. A
step makes it beside `output`, in `partial/layer-<key>/`, which the engine removes when the step
ends.

## Receipt

`layers/<key>.json` is the receipt of the files of one step, a JSON object:

| Key | Meaning |
| --- | --- |
| `layer`, `step`, `format`, `revision`, `key`, `options` | As in the key |
| `inputs` | As in the key, and `files` for a step input that selects files: the selected paths, sorted |
| `digest` | The digest of `files` |
| `files` | One item per file: `path` in the layer, `size` in bytes and `sha256`, sorted by `path` |
| `built` | `YYYY-MM-DDTHH:MM:SSZ` |
| `wall_ms` | The time from start to end, in milliseconds |
| `cpu_ms` | User and system time in milliseconds of the process of the step and of the children that it waited for, such as Osmium (`wait4`). `null` when the system does not report it |
| `peak_rss_bytes` | The peak resident set of the process of the step, or of the largest child that it waited for. It is not the sum of processes that run at the same time. `null` when the system does not report it |
| `cache` | The `cache` of the layer (`false` when absent) |
| `used` | For a `cache` layer, `YYYY-MM-DDTHH:MM:SSZ` when the last run that built or read it ended. Absent before that |
| `own_process` | Whether the step ran in a process of its own. Without it (`false` when absent), `cpu_ms` and `peak_rss_bytes` can be those of the engine, and no estimate reads the peak |
| `bytes_in` | The size of the input files |
| `bytes_out` | The size of `files` |
| `metrics` | The JSON object the step wrote, or `{}` |

## Plan

A plan lists what a run fetches and builds. It is a JSON object, `{"groups": [...]}`. The engine
looks at the steps in dependency order:

- A snapshot input needs a fetch when the store cannot give the files that it reads: no record
  of a fetch with its `params`, no record of the version, no selected file in it, or no object of
  a selected file.
- A step needs a build when a snapshot that it reads needs a fetch, when a layer that it reads
  needs a build, or when the store has no layer for its key.
- The plan does not list the other steps: the store has their layers.

A group is one change. Two builds are in the same group when one reads the layer of the other.
Two groups can need the same
fetch. An `id` names a group only in the plan that it comes from.

| Key | Value |
| --- | --- |
| `id` | The step of the first build of the group, in dependency order. For an update: the layer, or the layers that `--only` names, joined by `,` |
| `cause` | `null`. For an update: `{"kind": "stale", "cause"}` with the [cause of the layer](#state-of-a-layer), `{"kind": "off"}` for a layer that no step makes now, or `{"kind": "named"}` for `--only` |
| `layers`, `drops` | `[]`. For an update: the steps that the group changes, as `builds` gives them, and the steps of the serving release that the new release no longer has |
| `fetches` | One per version and `params`: `source`, `version`, `params` (`[[NAME, VALUE], …]`), `files` and `bytes`. `files` are the names of the files that the store lacks. `[]` means that the store cannot name them, and the fetch gets every file that it gives |
| `builds` | In dependency order: `step`, `recipe` (see [Keys](#keys)), `key` (`null` while the step waits for a fetch or another build), `estimate`, `again`: whether the step builds also when the store has its key, and `built`: when the store has the layer of its key, the time of that build, else `null` |

The estimate of a build is `wall_ms`, `bytes_out`, `client_bytes` (the size of the files that a
client reads) and `peak_rss_bytes` of the newest receipt, by `built`, of the same step with the same
options. Without one, it is the `cost` of the output of the step in the serving release with the
same options, else the newest receipt of the step, else that `cost` with other options, or
`null`. Its `peak_rss_bytes` is `null` unless it comes from a receipt with `own_process`.
An estimate from a receipt or a cost with other options than the step, such as another region, is
a guess (`guess`). It scales `wall_ms`, `bytes_out` and `client_bytes` by the input size of the
step against theirs: the input bytes (`bytes_in`) when the store has every input of the step and
the receipt read some, else the area of the option `bounds` (`[west, south, east, north]` in
degrees, on a sphere) when both have it, else not at all. Its `peak_rss_bytes` does not scale.

The `bytes` of a fetch is the size of its files in the record of the version. When that record does not list them all, it is the size in the record of
another version that lists them all, the last in byte order. Without `files`, the files are those
that a fetch with the same `params` gave, or else every file. Otherwise `bytes` is `null`. A run
fetches a version with the same `params` once, with the files of every group that needs it.

## Runs

An update and a rollback own one run each. It starts after the arguments are checked, and
finishes after the switch, and for an update after the collection that follows it.
`update --dry-run` and a read-only command create none. A failure after the run starts returns its id in `error.run`.

A run fetches the fetches of a plan, one after another, with the fetchers of [Fetch](obc-data-store.md#fetch).
Then it builds the builds of the plan. A layer that a planned step reads, and that the plan does
not build, must be in the store or kept by reference. A planned key that is not the key of the
step now fails the run.

A planned step whose key, after the steps that it reads are built, is the key of its output in
the serving release is unchanged: the step does not bake, also when the store lacks its layer. The
new release keeps the files, the digest and the cost of that output by reference, with the
versions and the client of the step now. This needs the store to have each file of that output
that no client of the serving release reads. `--only` bakes the steps of the named layers also
when they are unchanged.

- A step starts when the layers that it reads are built.
- At most one step per core runs at a time.
- Independent steps run in parallel within the job and memory limits.
- Of the steps that can start, the step with the longest chain of planned steps from it to the
  end of the run starts first: its estimated `wall_ms` and that of the longest chain of the steps
  that read it. A step without an estimate counts as the longest.
- The memory that the steps that run at the same time reserve is not more than three quarters of
  the physical memory of the machine, or of the least `memory.max` of the cgroup v2 of the process
  and of its parents when that is less. A step reserves its estimated peak (`peak_rss_bytes` of the estimate).
  A step with a guess, or without a peak, reserves from its measured runs too, the larger of the
  two: the receipts of the step, also with other options or of another leaf of its tiled step,
  with `own_process`, a peak and input bytes, in the store or of this run. They give their
  smallest peak, or, when that is more, its input bytes (`bytes_in`) times the peak per input byte
  of the run that read the most. Another step of the layer does not count. A step without a peak and without a measured run reserves
  512 MiB and 16 times its input bytes. A step that reserves more than the memory runs alone.
- A step with worker processes reserves its largest process, not their sum. It gets
  `OBC_DATA_MEMORY_BYTES`: the part of the budget that the steps that run do not reserve when it
  starts. A Python step pool forks no more workers than fit in it. This does not keep the budget:
  the steps that start after a pool step do not see its workers, and can take the memory that
  they use.
- A step that does not fit in the free memory holds back the steps after it, so that it starts
  first.
- After a fetch fails, no step starts. After a step fails, no other step starts, and the steps
  that run finish. The layers that the run built stay in the store; the next update reuses them.

The id of a run is its start time in UTC, `YYYY-MM-DD-HHMMSS`, with `-2`, `-3` and so on when
another run has that id. The folder `runs/<id>/` and a status `starting` reserve the id. The
process that holds the lock `run-<id>` is the process of the run. A run whose status says
`running` and whose lock is free has failed: its process ended before it finished. A run whose
status says `starting`, whose process does not exist and whose lock is free has failed too: its
process never started.

- `status.json` is the status of the run (`runs RUN --follow --json` in
  [JSON schemas](obc-data-cli.md#json-schemas)): its arguments after `obc data`, its process (while it starts,
  the process that reserved it, then the process that it starts), its state (`starting`,
  `running`, `ok`, `failed` or `stopped`), each phase that it entered with its state and why it
  waits (`waiting`, see [Router ready](obc-data-targets.md#router-ready)), the progress of each layer that it bakes
  (when its first step started, steps done of the total, and for a tiled layer the leaves done,
  the leaves that bake now and the leaves whose steps are all unchanged), its fetches and steps
  (each `unchanged` or not, with when it started and ended, in milliseconds after the start of
  the run), the releases that it switched the target to, and its error with the code, the fix and the steps
  whose layers the store keeps. The run replaces it with one rename, at most each second while
  events come, at each phase and at the start of each fetch. A reader ignores keys that it does not
  know.
- `log` has one line per event, with the time, and the output of the process of a detached run:
  any bytes, which a reader reads as UTF-8 with replacements. After the builds, one line per
  tiled layer gives its leaves: `<layer>: <baked> of <total> leaves baked, <unchanged> unchanged`.
  A leaf is baked when one of its steps baked, and unchanged when all of its steps are.
- `stop`: when it is present, the run stops before its next fetch, step or phase.
  `obc data runs RUN --stop` writes it, and sends SIGTERM to the process group of a `running`
  run, once. When a run stops, the process group of each step that runs gets SIGTERM: the step
  ends with the programs that it started, and fails. A detached run whose process finds it at its
  start does nothing more. A run that stops frees the lock of its target, keeps what it baked, and ends as `stopped`; so does a
  detached `serve`.

`obc data update` runs detached. It makes the review in its own process, refuses as the review
says, and then starts its own program in a new session, without a terminal, with the arguments of
the review (with `--expect ID`) and `OBC_DATA_RUN=<id>`. That process is the run: it takes the
lock and makes the review again, so an update whose target changed refuses. Closing the terminal
does not stop it. The first process follows the log of the run until it ends, with the exit
status of the run, and after a failure writes the fix; Ctrl-C ends only the following. With
`--json`, it writes `{run, review}` and ends. A rollback runs in the process that starts it.

## Versions

A step list gets the version of each fetch that it reads from one function
(`obc_data::product::version`), by three rules:

1. A due fetch reads the newest version upstream, and the update uses that version until it ends.
   When the upstream of an HTTP fetch does not serve (no connection, a timeout, HTTP 404, 408, 429
   or 5xx) or the credential is missing, the fetch reads rule 2, and a note of the
   [review](obc-data-targets.md#review) names the fetch, the error and the version that it reads. Another failure,
   such as a failed capture program, stays an error.
2. Otherwise the fetch reads the newest version that the machine knows: the newest of the serving
   version, the versions of the fetch in the store and the `start` of the source. The serving
   version of any fetch of a source with a `start` is the newest serving version of the source:
   one version names all its fetches, such as the GLO-30 tiles of every region. A version of the
   store serves a fetch without `NAME=VALUE`s only when no record of a fetch with `NAME=VALUE`s
   names it.
3. Otherwise it reads the newest version upstream: the product names the fetch, and the update
   fetches it. A source whose URL needs a `NAME=VALUE`, such as the GLO-30 tiles, has no one newest
   version: a fetch of it without that value fails with `failed`, and the fix names the code of the
   product.

A `date` or a `release` version is newer by its parts: a run of digits by its number, any other
run as text, so `v1.9` comes before `v1.11`; two versions with equal numbers, such as `v1.0`
and `v1.00`, are in the order of their text. A `commit` or a `digest` has no order: rule 2 gives
its `start`; else the version that `obc data source refresh` took last on this machine, when the
target serves a version that the targets served then, or none: a refresh never moves a target
that another machine moved since; else the serving version; else the one version in the store,
when the store has exactly one. A `prepared` source has the rule of
[Prepared sources](obc-data-prepared.md#prepared-sources).

Before it lists the steps, an update finds the fetches that the step lists read, with only small
metadata fetched (see [Products](obc-data-products.md#products)). It judges each of them, as
[State of a source](obc-data-registry.md#state-of-a-source) says, with the policies of the target. A stale fetch is
due. A `manual` source is never due: a step list reads a newer version of it only when the store
has one. In one update, a fetch of a `manual` source without a version takes the version of the
first fetch of that source.

## State of a layer

The engine judges each layer when it is asked, and stores nothing. It compares each step with its
output in the serving release of the target. A layer has one state:

| State | When |
| --- | --- |
| `ok` | The release has each step of the layer as the steps make it now |
| `off` | No step makes the layer: an optional layer that the settings switch off, or a layer that only the release has |
| `stale` | A build changes the layer |

A stale layer has exactly one of three causes. A change of code, a commit or a compiler is no
cause. When more than one row applies, the first row gives the cause. Within one row, `format`
comes before `revision`, then the lower `from`, with `null` first; and the `source` or `name`
that is first in byte order:

| `cause.kind` | Fields | `text` | When |
| --- | --- | --- | --- |
| `version` | `field` (`format` or `revision`), `from`, `to` | `format 19 → 20: readers need new data`, `revision 2 → 3: improved output`, or `new` | The format or the revision of the layer is not that of a release output of its steps. `from` is `null`, and the text `new`, when the release has no step of the layer; `field` is then `format` |
| `source` | `source`, `from`, `to` | `wiki 2026-06-01 → 2026-09-01` | A step reads another version of the source than its release output: a newer version that the store has, or a due fetch (see [Versions](#versions)). `to` is the newer version upstream, or `null` when it has no name, and the text then says `newer` |
| `input` | `name` | `follows device/map`, or `options changed` | A step reads a step of another operator layer that is stale, or whose files in the release are not the files that its release output read; or the store has other files of a snapshot that it reads, at the same version and `params`. `name` is that layer or source. `name` is `options` when the release has other steps of the layer, or a step with other options, inputs (kind and name), snapshot `params` or `client` |

A step that reads a step of its own layer, or of an internal layer, gets the cause of that step.

Each layer also gives what a picture of the layers needs: `shape` (`tiled` or `whole`), the
`format` and the `revision` of its steps, its `readers`, and `reads`, the layers and the sources
that its steps read, sorted. A layer with partitions gives `partitions`, those of its steps, and
`stale_partitions`, those whose steps a build changes. In place of an internal layer, `reads`
names what that layer reads.
A layer that only the release has gives `null` and empty lists.
`obc data` does not list an internal layer: the layers that read it show its cause. In JSON, a
layer is `{"layer", "state", "cause", "text"}`; `cause` and `text` are `null` unless it is stale.
A step that cannot be planned is not judged: `blocked` lists it with its layer and the reason.
