# obc data targets and releases

The targets that serve data, the releases that they serve, and the update, the rollback and the
collection that change them.

- Version: release manifest format 2. A follower reads its format and the one before, see
  [Release manifest](#release-manifest).
- Writers: `host/obc-data` (targets, releases, locks).
- Readers: `host/obc-data`, the device, the web and iOS planners, the planner services.
- Code authority: `host/obc-data` (`target.rs`, `release.rs`, `lock.rs`, `ready.rs`,
  `collection.rs`).
- Related: [serve and deploy](obc-data-serve.md), [layers](obc-data-layers.md),
  [products](obc-data-products.md), [commands](obc-data-cli.md),
  [planner release](planner-release.md).

## Targets

A target is a place that serves data. `data/targets.toml` says where each target is. What a
target holds is in the release that it serves.

## `data/targets.toml`

One table per target. The name of the table is the name of the target, in lowercase kebab-case.

| Key | Type | Meaning |
| --- | --- | --- |
| `kind` | string | `r2` or `dir` |
| `public` | string | `r2` only: the `https://` origin that serves the keys of the bucket, without a path |
| `bucket` | string | `r2` only: the name of the bucket. `OBC_R2_BUCKET` must name it to write the target |
| `prefix` | string | `r2` only, optional: the folder under `test/` of the bucket that holds the target, such as `test/run-1`, without `/` at the end. Without it, the root of the bucket. A collection deletes keys of the target, and a folder outside `test/` can hold live keys |
| `dir` | string | `dir` only, optional: an absolute path, or a path that starts with `~/` for the home directory. Without it, the folder with the name of the target beside the store |

The file has `prod`, the public bucket, and `local`, a folder on each machine. A `dir` target and
the store do not overlap, also through a link.

`obc data` reads an R2 target through the S3 API when it has the credentials, because the public
origin can serve a cached pointer. Else, and always in [serve](obc-data-serve.md#serve), it reads through the public
URL, with each part of a key percent-encoded, so a `#` in a key under `sources/` is no fragment.

`obc data` writes an R2 target with the credentials of `OBC_R2_*` (see [Credentials](obc-data-cli.md#credentials)):
a small document through the S3 API, signed with AWS Signature Version 4, and the files of a
release with rclone. `OBC_R2_BUCKET` must be the `bucket` of the target. A command that writes an
R2 target without these variables refuses (`blocked`); so do an update and a rollback without
rclone, which lists and checks the files of a release. A tagged read and a review need the
credentials too: the review id holds the tag of `release.json`, the ETag of the S3 API, and the
public origin can give another form of it.

## Target layout

A key is a path with `/` between its parts. No part is empty, `.` or `..`.

| Key | Holds |
| --- | --- |
| `release.json` | The release that the target serves, and its rollback. A switch writes it first |
| `lock.json` | The [lock](#lock): the run that changes the target, or free |
| `planner/next.json` | Optional: a release whose planner services [serve](obc-data-serve.md#serve) loads before a switch to it, see [Router ready](#router-ready) |
| `releases/<id>.json` | The [manifest](#release-manifest) of a release. Immutable |
| `releases/<folder>/<path>` | A file that a client finds by name. `<folder>` is the [digest](obc-data-layers.md#keys) of the files of the folder, listed by `<path>`. Immutable |
| `objects/<sha256>` | A client file of an output, or a list of a release. Immutable |
| `cell-catalog/catalog.json`, `planner/catalog.json` | The pointers: a copy of the object that the serving release names for the key |

Only client files of outputs go to a target: an output that no client reads can derive from a
source that we may not give to others, and a bucket is public. No target holds a version of a
[prepared source](obc-data-prepared.md#prepared-sources). A step that reads such an output of a kept layer reads it
from the store of the machine.

## Release manifest

`releases/<id>.json` is compact JSON with the keys of each object in byte order. The id is the
SHA-256 of these bytes. The manifest has format 2. A change that a follower of the format before
cannot read raises the format. A follower reads its own format and the one before. It says its format at `/planner-api/version`, and the review
of an update of prod names a follower that cannot read the release.

A list is an object of its own at `objects/<sha256>` of the target: compact JSON with the keys of
each object in byte order. The manifest names each list as `{size, sha256}`, so the id proves each
list, and the manifest grows with the number of steps, not with the number of files.

| Key | Meaning |
| --- | --- |
| `settings` | The [settings of the target](obc-data-registry.md#settings-of-a-target) |
| `outputs` | One output per step, sorted by `step` |
| `pointers` | `[{key, size, sha256}]`, sorted by `key`: both pointer keys, each with the client file of an output that a client reads there |
| `named` | The list of the files under `releases/<folder>/`: `[{key, size, sha256}]`, sorted by `key`. Each is a client file of an output |
| `sources` | `[{source, version, params, list}]`, sorted by `source`, `params` and `version`: each fetch that an output read, with the sorted `NAME=VALUE`s. Its list holds the files that the outputs read: `[{name, url, size, sha256, retrieved}]`, sorted by `name`; `url` is where upstream gives the file, and `retrieved` when a machine retrieved it first. The target holds no copy of these files |
| `machine` | The machine that made the release |
| `time` | When the machine made it, `YYYY-MM-DDTHH:MM:SSZ` |

An output has `layer`, `step`, `format`, `revision`, `key`, `options` and `digest` as in the
[receipt](obc-data-layers.md#receipt), and:

| Key | Meaning |
| --- | --- |
| `list` | The list `{"files", "inputs"}` of the output, with `files` and `inputs` as in the receipt |
| `snapshots` | `{source: {"version", "params"}}`: the version and the sorted `NAME=VALUE`s of each snapshot that the step read |
| `client` | The client outputs that the step selects (see [Layers](obc-data-layers.md#layers)) |
| `cost` | The `wall_ms`, `peak_rss_bytes` (`null` when the system does not report it) and `bytes_out` of the build that made the receipt |

A manifest of format 1 holds its lists inline: an output has `files` and `inputs` instead of
`list`, a fetch has `files`, and `named` is the list itself. A reader ignores other fields of a
fetch of format 1, such as `copy` in the manifest of an older publisher.

The objects of a release are the client files of its outputs, one object per SHA-256. The keys of
a release are its manifest, its lists, its objects and its named files. A release names its files
exactly: a file in a folder of `releases/` that the manifest does not name is not a file of the
release.

## `release.json`

```json
{
  "serving": {
    "id": "<id>",
    "since": "2026-10-12T08:00:00Z"
  },
  "rollback": {
    "id": "<id>",
    "since": "2026-10-12T08:00:00Z"
  }
}
```

`since` is the time, by the clock of the target, at which the release took its role, or at which
a switch to the serving release wrote its pointers again. `rollback` is `null` until the second
switch.

`planner/next.json` names one more release with the same fields as `serving`: a published release
that the target does not serve yet. [Serve](obc-data-serve.md#serve) starts the services of its planner release
beside the others, and gives it to no client.

## Publish and switch

1. The store gets the lists of the release. The target gets each key of the release that it does
   not have, the lists with the objects, and the manifest last. Before the target changes, each
   of these files in the store must have the SHA-256 that the release gives. A key is immutable: a key with another size is refused, and nothing replaces it.
2. The switch reads the manifest from the target. A conditional write replaces `release.json`
   only when it is still the file that was read, or when it is still absent. The release that
   served becomes the rollback. When the condition fails, nothing changes.
3. Then each pointer key of the release gets the bytes of its object.

Before it writes `release.json`, a switch to another release needs each pointer to hold nothing,
the bytes of the serving release, or the bytes of the release that it switches to. A switch to the
serving release writes its pointers again, and sets `since` of the serving release. Drift is a pointer whose bytes are not those of the serving release. Nothing deletes a
pointer key: clients of older publishers read these keys.

A `dir` target must be on a local file system. A conditional write holds `<dir>/.lock` while it
compares and writes, and writes through a rename. The tag that it compares is the SHA-256 of the
bytes. A key can be a hard link to an object of the store: nothing writes a key or an object in
place.

On an R2 target the tag is the ETag. A conditional write is a PUT with `If-None-Match: *` for an
absent key, or `If-Match: <tag>`. Only HTTP 412 says that the condition fails; another answer
that is not 2xx is an error. `release.json`, `planner/next.json` and `lock.json` get
`Cache-Control: no-store`, a pointer `public, max-age=60, must-revalidate`, and every other key
`public, max-age=31536000, immutable`. A document is `application/json`. After the upload and
before the switch, each key of the release must be on the target with its size, else nothing
switches.

The `prod` bucket also holds keys of older publishers, such as `cell-catalog/objects/`,
`planner/releases/` and `reference/v1/`. An update and a rollback never write or delete them.

## Lock

`lock.json` names the run that changes a target: `{"state": "free"}`, or the holder.

```json
{
  "state": "held",
  "machine": "<machine>",
  "run": "<run id>",
  "review": "<review id>",
  "phase": "build",
  "heartbeat": "2026-10-12T08:00:00Z",
  "progress": [{"layer": "device/map", "done": 12, "total": 40}]
}
```

An update and a rollback hold the lock from their start to their end. `--dry-run` does not take
it.

- A run takes the lock with a conditional write: when `lock.json` is absent, or free with the tag
  that the run read. When another run holds it, the run refuses (`blocked`) and names the holder.
- The run writes its heartbeat each minute (`BEAT` in `lock.rs`), and with its phase at the start
  of each phase, on the condition of the tag of its last write. `review` is `null` until the run
  has its review. The run makes its review after it takes the lock, so `--expect` compares the
  id of the target that the lock keeps.
- The server can apply a write whose answer does not come, so a later write on the old tag fails.
  After a write that fails or gets no answer, the run reads `lock.json`: when it is what the run
  wrote, the write is done; when it names this run on this machine, the run writes again on its
  tag. Else another run took the lock: the run stops and changes nothing more. The write at the
  start of the phase `switch` checks this immediately before the switch. After the switch, a lost
  lock stops only the collection.
- At its end, the run frees the lock with a conditional write of `{"state": "free"}`: also when
  it fails or panics. When another run took the lock meanwhile, the run reports that. SIGINT and
  SIGTERM stop the run before its next fetch, step or phase, and end the steps that run, so it
  frees the lock too. A run that a signal stopped before the switch does not switch. A second
  signal ends the process at once.
- A process that ends without that, such as on SIGKILL, a crash or an out-of-memory kill, keeps
  the lock. On its machine, on Linux, the next run of the same store takes the lock over at once,
  with a conditional write, and writes that to its log: the store shows that the process of the
  run ended (see [Runs](obc-data-layers.md#runs)), and no process of its session runs, such as a program of a step.
  On another machine, `obc data unlock TARGET --from MACHINE` frees the lock of a run of MACHINE
  whose heartbeat is 10 minutes old or older (`STALE` in `lock.rs`). It asks first. When that run continues, its next write fails,
  and it does not switch.
- The heartbeat is the time of the clock of the holder. A `dir` target is on a local file system,
  so the holder and `unlock` read the clock of one machine. On an R2 target, `unlock` measures the
  age of the heartbeat with the clock of the server: its `Date` against the `Last-Modified` of
  `lock.json`.
- `progress` gives the steps done of the total of each layer that the run bakes, at the last
  heartbeat. `status.json` of the run has the detail, see [Runs](obc-data-layers.md#runs). In the phase
  `router-ready`, `waiting` gives the last answer of the follower of the target.

## Update

`obc data update TARGET` makes a target serve its settings as the steps make them now. An R2
target serves riders: its update starts only with `--expect ID`, the id of a review that the owner
approved, and asks first, or needs `--yes` without a terminal. The first process checks the
arguments of the person, before the run. One run does each phase in this order, while it holds the
[lock](#lock):

1. Prepare. It reads the serving release, applies the flags to its settings, and refuses a target
   without a region. It finds the fetches of the step lists, judges which are due (see
   [Versions](obc-data-layers.md#versions)), and lists the steps, with the fetches that the step lists need. It
   judges each layer against the serving release, as [State of a layer](obc-data-layers.md#state-of-a-layer) says:
   one group per stale layer bakes. Each other step keeps its output of the serving release by
   reference: a step that reads it reads the files of the release, never another layer of the same
   key in the store. `--only LAYER,…` bakes the steps of the named layers again, also when they are
   not stale, with the stale steps that they read at any depth and the steps that the release
   lacks. A step that reads one of them bakes when its key changes. A stale step that the named
   layers do not need keeps its output.
2. Review, as [Review](#review) says. A refused review changes nothing.
3. Build. It fetches and bakes the groups.
4. Verify. The new release has the output of each step, the pointer and the named files of each
   product. When it serves as the serving release, with the same settings, outputs, pointers,
   named files and source files, the update changes nothing but a pointer that drifts. A source
   file that the serving release read with the same name, size and SHA-256 keeps its entry, so
   where and when a machine retrieved it changes no list. Else each product checks it, and a
   failed check changes nothing.
5. Upload and switch, as [Publish and switch](#publish-and-switch) says. On an R2 target, the
   phase `router-ready` comes between them, as [Router ready](#router-ready) says.
6. Collect, as [Collection](#collection) says, when it switched to another release.

An update that stops keeps what it baked in the store, and the next update reuses it.

## Router ready

The follower of an R2 target, [serve](obc-data-serve.md#serve) on the machine that serves its API, must run the
planner release of a release before a client gets the release. So between the upload and the
switch to a new release, an update of an R2 target does this; a `dir` target skips it.

1. `planner/next.json` gets `{"id": "<release>", "since": "<time>"}`, with a conditional write
   on what the run read there.
2. Every 10 seconds (`EVERY` in `ready.rs`), the run asks `GET
   <api_origin>/planner-api/releases/<planner>/ready`, with `api_origin` and `active.id` of the
   planner pointer of the new release. Only 200 with `{"state": "ready"}` ends the wait. Any
   other answer, also 404, another 200 and no answer, is a reason to wait: the reason of the
   answer, or its status. Each new reason goes to the log and the status of the run and to the
   next heartbeat of the lock. The next phase clears it.
3. After the switch, the run deletes `planner/next.json`.

While `planner/next.json` names a release, the follower runs the services of the serving release
and of that release. Once that release has an install, the services of the rollback stop, and its
`ready` answers 503 with `{"state": "installed"}` until the file goes.

The run waits at most 24 hours (`GIVE_UP` in `ready.rs`). After that, or when the run stops or
another run takes the lock, it fails with `<target> unchanged: <last reason>`. `release.json` and
the pointers stay as they were; the next collection deletes the release that it uploaded. The run
deletes `planner/next.json` at each end, but only while the file names its release, and never
after it lost the lock: then the file belongs to the run that took it.

## Any machine

An update needs no file of the serving release in the store of the machine. It takes only what
its bakes and the checks of the products read:

- A fetch comes from the store, else from upstream. The target holds no copy of a fetch.
- A version of a prepared source comes from the store, else from the archive. A machine without
  the archive bakes only what its store holds, as [Prepared sources](obc-data-prepared.md#prepared-sources) says.
- A step that bakes and reads a file of a fetch that the serving release lists must find the same
  SHA-256 in the record of the store: before the review, and again after the fetches of the
  update, before any bake. Else nothing changes (`verify_failed`).
- A fetch that the serving release read, of a source whose upstream gives only its newest version
  (a capture), refuses the update (`blocked`) before the fetch, also while the update lists the
  steps, when the store lacks it and a capture cannot give it. A capture gives the version of today, and a version of a manual source that the store started (see [Fetch](obc-data-store.md#fetch)).
  The fix names the machine that made the serving release. A refresh that moves the source to a new
  version is the other way.
- When the store and the serving release give a fetched file the same name, size and SHA-256, the
  new release keeps the entry of the serving release.
- A file of a kept step that a step that bakes reads, or that the check of a product reads, comes
  from `objects/` of the target when the store lacks it.
- A kept step whose file a step that bakes reads, when no client reads that file and the store
  lacks it, bakes again on this machine: the target does not hold it. Its cause is `unheld`. The
  review lists such a layer, also one that only other layers read.

## Review

The review of an update says what it does before it changes a target. `--dry-run` writes the
review and creates no run. `--plan` makes the review in a detached run, as [Runs](obc-data-layers.md#runs) says: the
run logs each fetch with its progress and each product that it plans, writes the review to
`runs/<id>/review.json`, and bakes nothing. It takes no lock: it writes nothing to the target.
`runs RUN` gives that review. To list the steps, an update fetches what the step lists need, which can be bulk data. It fetches into
the store only, and it never writes to the target.

- `layers`: each layer that bakes or goes, with its cause (`stale` with the cause of
  [State of a layer](obc-data-layers.md#state-of-a-layer), `named` by `--only`, `off`, or `unheld` as
  [Any machine](#any-machine) says), the steps that bake, the grid leaves (the last part `<i>-<j>`
  of the names of the steps of a tiled layer), the partitions whose steps bake, the steps of the
  release that no step makes now and their grid leaves, the readers, and a format change with what
  the new format means for each reader. With `--only`, a layer that bakes because it reads a named
  layer follows that layer. A layer that only other layers read is not listed. A planned step can
  end unchanged (see [Runs](obc-data-layers.md#runs)), so the text gives the bakes and the leaves as upper bounds ("at
  most").
- `sources`: each fetch that a baked step reads at another version than the serving release.
- `kept`, `fetches`, `downloads`, `bakes` and `reused`: the outputs that the release keeps by
  reference, what this machine fetches upstream and downloads from the target, the steps that it
  builds, and the steps whose output the store has from an earlier build, each with `built`. A
  reused step does not build; its client files go to the target.
- `cost`: the estimate of each bake, as [Plan](obc-data-layers.md#plan) says. `upload_bytes` is the size of the
  files of the bakes and of the reused steps that a client reads. `unknown` lists each bake
  without an estimate, with a guess or without a peak, and each fetch whose size is not known.
- `machine`: the memory of the machine and the free bytes of the disk of the store after the
  listing. A bake whose estimated peak, also a guess, is more than the memory, or fetches,
  downloads and bakes that leave less than 4 GiB free on the disk, refuse the update.
- `after`: the release that becomes the rollback, and the rollback of now, which the collection
  drops, with the keys that the serving release does not name: what it deletes at most.
- `args`: the arguments after `obc data` that start the update of this review.
- `notes`: what a layer bakes without, such as a national terrain model without a version, with
  the command that gives it. A note does not stop the update.
- `services`: for an R2 target at the root of its bucket whose steps include a `planner/` layer,
  why its follower or its tile Worker cannot serve the release, each with its fix, as
  [Deploy](obc-data-serve.md#deploy) says: no follower answers `version`, the follower reads a layer of the
  release in another format, or the Worker does not answer `version`. It does not stop the
  update: the run would wait at `router-ready`.

The id is the first 12 hex digits of the SHA-256 of the canonical JSON, as for the release id, of
an object: the name of the target, the tag of `release.json` (`null` while the target serves
nothing), the settings, `layers`, the steps that bake, and the source, version and params of each
snapshot that they read. It holds no cost and no time. Two machines that read the same target give
the same id when their stores resolve the same versions and files. `--expect ID` starts the update
only when its review, computed after the listing, has this id.

A format change is a layer that bakes, and whose outputs in the serving release have another
`format` than the layer now. An update with a format change starts only with
`--accept-format-change`. A new layer is no format change.

## Rollback

`obc data rollback TARGET` makes a target serve its rollback release again; for an R2 target, it
asks first, or needs `--yes` without a terminal. With `--expect ID`, where ID is the id of the
rollback release or its first 12 or more hex digits, it refuses with `blocked` and changes nothing
when the rollback release is another one. The release that served becomes the rollback, so a
second rollback undoes the first. A target without a rollback refuses. The target must have each
key of the rollback release, else nothing changes. On an R2 target, the follower must also serve
the planner release of the rollback: the rollback asks its `ready` once, as [Router
ready](#router-ready) says, and any answer but 200 refuses (`blocked`) with its reason. The
follower keeps that planner release running. Then it switches as
[Publish and switch](#publish-and-switch) says. It holds the [lock](#lock) as an update does. A rollback bakes, uploads and deletes nothing: the next update collects.

## Collection

A target keeps the files of the serving release and of the rollback. After an update switches to
another release, it collects the target. The grace of a collection is 10 minutes (`GRACE` in
`collection.rs`): longer than a client needs to fetch the files that a pointer it just read names.

- When a pointer does not hold the bytes of the serving release, it deletes nothing.
- It deletes the keys under `objects/` and `releases/` that no kept release names, and the
  temporary files of a write that stopped: a hidden `.part` or `.tmp` file in the root, or beside a
  pointer with the name of the pointer. The pointers, `release.json` and the keys of older
  publishers stay.
- A release that `release.json` does not name stopped serving at or before the time when the
  release that served before the switch began to serve. The first collection the grace or more
  after that time deletes it. Until then it stays, and its files too.
- It runs while the update holds the [lock](#lock): no other run writes the target. It deletes in
  batches of up to 1,000 keys (`BATCH` in `collection.rs`), the most that one S3 `DeleteObjects`
  request takes. On an R2 target, one such request deletes a batch. Before each batch, a heartbeat
  checks that the run still holds the lock. A collection writes no removal log: its output gives
  what it deleted.
- On an R2 target, it lists the keys under `objects/` and `releases/` only, and the
  grace counts by the clock of the server: the `Date` of its answers, as `since` does.
- It deletes each other file that no remaining manifest names, then the manifests of the releases
  that go, then their lists that no remaining manifest names. A collection that stops resumes at
  the next update that switches, from the manifests that remain, and their lists are there.
- When it cannot read a manifest, or a list of a release that stays, it deletes nothing. It reads
  a list through the store of the machine, which keeps it: a list is immutable.

A collection that stops does not undo the switch: the output gives what it deleted and why it
stopped.
