# obc data

`obc data` reads the data registry: the external sources that a bake uses, the regions, and
the pins of an environment. It fetches sources into the store. The crate is `host/obc-data`.
All files under `data/` are TOML. A file with an unknown key is refused.

## Files

### `data/sources.toml`

One `[[source]]` table per source.

| Key | Type | Required | Meaning |
| --- | --- | --- | --- |
| `id` | string | yes | Unique, lowercase kebab-case |
| `kind` | string | yes | `data` (steps read it), `asset` (ships to users as it is) or `tool` (steps run it; nothing of it ships) |
| `licence` | string | no | SPDX licence expression: ids or `LicenseRef-…`, joined by `AND`, `OR`, `WITH` and parentheses |
| `licence_url` | string | no | Where the licence text is |
| `attribution` | string | no | The credit text, as the product must show it |
| `obligations` | string | no | What the licence asks for, in words; `none` when it asks for nothing |
| `fetch` | table | yes | `kind` and `url`, see below |
| `hosts` | array of strings | no | Hosts the fetch reaches besides the host of `fetch.url`: lowercase letters, digits, `.` and `-`. `*.domain` is any subdomain |
| `version` | string | yes | How upstream names a version: `date`, `release`, `commit` or `digest` |
| `refresh` | integer or string | yes | `7`, `30`, `90` or `365` days, or `"manual"` |
| `redistribute` | boolean | yes | The licence lets us give the upstream bytes to others |
| `r2_copy` | boolean | no, `false` | R2 keeps a copy of the pinned version, because upstream cannot give it again |
| `credential` | table | no | `env`: the environment variables a fetch needs; or `file`: the file that holds them. `~/` is the home directory |

`fetch.kind` is one of:

| Kind | Fetch |
| --- | --- |
| `http` | A file, or one file per tile |
| `osm` | The OSM planet and its replication diffs |
| `geofabrik` | A Geofabrik extract or `.poly` of an area |
| `glo30` | Copernicus GLO-30 tiles |
| `dtm` | A national terrain model service |
| `capture` | Requests to a query service or an API |
| `github` | A GitHub release asset or source archive |
| `by-hand` | A person orders or downloads the files at `url` |
| `installed` | A person installs it, or another source's build brings it. It has no `url` |

`fetch.url` is an `https://` URL template. `{name}` stands for a value the fetcher fills:
`{version}` is the pin, `{area}` a Geofabrik area, `{tile}` a tile name. In `attribution`,
`{year}` and `{month}` are the year and month of the data, which the step that writes the
credit fills.

Rules:

- An id is listed once.
- Each kind of fetch but `installed` has a `url`, and the `url` starts with `https://`.
- `refresh` in days needs `version = "date"`, because only a date pin has an age.
- `r2_copy = true` needs `redistribute = true`, because R2 is public.
- A credential has `env` or `file`, not both.

`attribution` is the one copy of a credit. Every product that carries a credit takes it from
here:

| Product | Credit |
| --- | --- |
| Device-map catalog `source` and `LICENSE.txt` | `attribution`, `licence` and `licence_url` of `osm-planet` |
| Device-map catalog `terrain.attribution` | The source whose id is the terrain `dataset_id` |
| Device-map catalog `landmarks.attribution` | `wikipedia`, then `commons` |
| Planner release `attribution`, routing package, search, route overlays | `osm-planet` |
| Planner release `landcover_attribution` | `daylight-landcover`; the basemaps show it after `attribution` |
| Planner `terrain_attribution` | `copernicus-glo-30`, after the reference models |
| Planner climate and snow layers | `era5-land`; `modis-snow` and `hansen-gfc`, or `hr-wsi` |
| Reference archive manifests | `dtm-<key>` for the national model with that key |

A credit that a rider reads is the `attribution` text. An SPDX `licence` id goes only into a
field for programs, such as the catalog `license`.

Rust code reads the file that the build embeds (`obc_data::sources::attribution`). Python
reads it through `tools/data_registry.py`, and shell through
`tools/data_registry.py attribution ID`. The web bundle notice reads it at build time through
`builder/app/vite/third-party-licenses.ts`. The web planner, the map builder and the iOS planner show the
planner release `attribution`. Text that no step can generate keeps a copy, and a test
compares the copy with this file: the device About page, and the footers of the site, the
docs and the map builder.

### `data/env/<environment>.toml`

`[pins]` maps a source id to the version that the environment is built from. Each pin names
a source of `data/sources.toml`. A pin of a source with `version = "date"` is a `YYYY-MM-DD`
date. `obc data sources` reads `data/env/live.toml`, and the file must exist.

### `data/regions/<id>.toml`

One file per region. The region id is the file path below `data/regions/` without `.toml`.
Each part of the id is lowercase kebab-case.

| Key | Type | Meaning |
| --- | --- | --- |
| `name` | string | The name a person reads; not empty |
| `kind` | string | `geofabrik`, `box`, `polygon` or `union` |
| `box` | array of 4 numbers | Only for `box`: west, south, east, north in degrees, longitude first |
| `polygon` | string | Only for `polygon`: an Osmosis `.poly` file, relative to the region file. The file must exist |
| `union` | array of strings | Only for `union`: two or more region ids |

A box has longitude in −180…180 and latitude in −90…90, with west < east and south < north.
A box that crosses the antimeridian is refused. The order of the numbers is checked only
through these ranges: a latitude-first box is refused only when one of its longitudes is
outside −90…90.

A `geofabrik` region is the Geofabrik area whose path is the region id, for example
`europe/germany/baden-wuerttemberg`. A union resolves to the regions in it that are not
unions. A union that contains itself, or names a region that does not exist, is refused.

The bakes read this directory:

| Reader | Regions |
| --- | --- |
| `obc-bake` (device maps) | Every `geofabrik` region. `--regions DIR` reads another directory with this layout; `obc bake` passes the checkout's directory |
| Planner bake | The `box` region with the id of the recipe in `tools/planner-regions/`: its `name` and its box |
| `fixtures/build-map-package.sh` | The `box` regions of the fixtures |

`tools/data_registry.py box ID [--lat-first]` prints the box of a `box` region and refuses
every other kind, so Python and shell never resolve a union or a Geofabrik area.

## State of a source

`obc data sources` computes the state of each source when it runs. It stores nothing.

| State | When |
| --- | --- |
| `blocked` | A `data` or `asset` source has no `licence`, or its credential is not on this machine |
| `stale` | The pin is a date, `refresh` is in days, the pin is older than `refresh`, and the newest upstream version is later than the pin |
| `ok` | Otherwise. A source with no pin, or with `refresh = "manual"`, is never stale |

The age of a pin is the number of days from its date to today (UTC). When a pin is older than
`refresh` and the newest upstream version is not known, the state is `ok` and the reason says
`upstream unknown`, and whether the source cannot be checked or the check failed.

## Store

The store is the directory in `OBC_DATA_STORE`, or else `~/.cache/openbikecomputer/store/`.

| Path | Holds |
| --- | --- |
| `objects/<ab>/<sha256>` | One file, named by the lowercase hex SHA-256 of its bytes; `<ab>` is its first two characters. Read-only |
| `snapshots/<source>/<version>.json` | The snapshot record of one source version |
| `layers/<key>.json` | The receipt of the layer with that key, see [Layers](#layers) |
| `upstream/<source>.json` | The last upstream check of a source: `checked` (seconds since 1970-01-01 UTC), `version` (a string, or `null` when the check failed) and `error` (only when it failed) |
| `partial/` | Downloads that are not complete, the validators that resume them, and the layers that steps write |
| `locks/` | One lock file per key |

Rules:

- An object is complete. A download goes to `partial/` and moves into `objects/` with one
  rename after the digest check.
- A record goes to a temporary file in its directory and replaces the old record with one
  rename.
- One process at a time downloads a URL, one process at a time writes a snapshot record, and one
  process at a time builds a layer key.

A snapshot record is a JSON object:

| Key | Meaning |
| --- | --- |
| `source` | The source id |
| `version` | The version, as a pin names it |
| `files` | One item per file: `name` (the last segment of the URL), `url`, `size` in bytes, `sha256` and `retrieved` (`YYYY-MM-DDTHH:MM:SSZ`) |

A version is one or more segments joined by `/`. A segment has letters, digits, `.`, `_`, `+`
and `-`, and does not start with `.`. A `date` version is also a `YYYY-MM-DD` date, and a
`digest` version is 64 lowercase hex digits.

## Fetch

A fetch fills each `{name}` of `fetch.url`: `{version}` from the version, and each other name
from a `NAME=VALUE` argument. A name can have more than one value; then the fetch gets one file
for each value. A file that the snapshot record of the version has, and whose object exists,
comes from the store with no request. The fetch records each file when its download is complete,
so a fetch that fails keeps the files before the failure. A record that has the URL with another
SHA-256 fails the fetch.

Which version a fetch gets:

- A URL with `{version}` gives the version that it names.
- A URL without `{version}` gives only the newest file upstream. For a `date` source without a
  version, a `HEAD` request for each URL, with one retry, gives the latest `Last-Modified` day,
  and that day is the version. A date version accepts a file that changed on or before that day; a later file
  fails the fetch before its body is read. A response without `Last-Modified` counts as changed
  today.
- A `release` or `commit` version of a URL without `{version}` is only a name. The fetch accepts
  it only when the record of the version has the URL, and so pins its bytes.
- For a `digest` source, the version is the SHA-256 of its one file.

A download stops after four failed tries in a row, and waits 2, 4 and 8 seconds between them. A
try that adds bytes to a resumed part resets the count; a try that starts the file again does
not. A try receives its body for at most 15 minutes, so a
stalled transfer becomes a retry. It retries a connection error, a cut-off or stalled body, a
refused resume and HTTP 408, 429 and 5xx. It keeps the bytes it has in `partial/`. The next
try, or the next fetch, asks for the rest with `Range` and `If-Range`, with the strong `ETag`
or else the `Last-Modified` of the first response. A `206` answer with another validator, or a
whole file, restarts the file. A `416` answer whose `Content-Range` is the size of the part
means the part is complete. A file with no validator resumes only when its digest is known.
The digest check compares the SHA-256 with the `digest` version, or with the record of the
version. A file that fails it is deleted.

| `fetch.kind` | Fetcher |
| --- | --- |
| `http`, `geofabrik`, `glo30`, `github` | One file per URL, as above |
| `osm`, `dtm`, `capture` | None yet; the fetch fails |
| `by-hand`, `installed` | None; the fetch fails |

The upstream check finds the newest version of a source with one request, which has 15 seconds.
The store keeps its answer, or its failure, for one hour.

| Source | Check |
| --- | --- |
| `osm` | `HEAD` of the URL, not following the redirect; the day in the `planet-YYMMDD` file name of its `Location` |
| `capture` | Today, with no request: a query service answers with current data |
| `github`, `commit` | The GitHub API: the newest commit of the default branch |
| `github`, `release` | The GitHub API: the tag of the newest release that has the asset of the URL |
| `http`, `geofabrik` or `glo30`, `date`, and a URL without `{name}` | `HEAD` of the URL; the `Last-Modified` day |
| Every other source | None; the source cannot be checked |

## Layers

A step makes one layer from snapshots, the layers of other steps and options. The engine builds
the steps in dependency order. It builds a step only when the store has no receipt for the key
of the step, or when an object of that receipt is missing.

A step declares:

| Field | Meaning |
| --- | --- |
| `name` | The layer name: lowercase kebab-case segments joined by `/` |
| `inputs` | Snapshots, as `source`, `version` and `files`: the names of the files that the step reads, or none for every file. The layers of other steps, by name |
| `options` | A JSON object |
| `code` | `paths`: files and directories, relative to the repository root. `crates`: workspace crates. A Rust step declares the crate of its function |
| `outputs` | Paths in the output directory. A path is a file, or a directory whose files are all part of the layer. The step must write each path and no other file. A symbolic link fails the step |
| `run` | A Rust function in the process, or a command: a program and its arguments. No argument names a path outside the repository root: no argument is an absolute path, contains `=/` or has a `..` segment between `/` and `=` |

### Keys

The digest of a list of files is the SHA-256 of the text that `sha256sum` writes for them: one
line `<sha256>  <name>` with a final newline per file, in byte order of the names.

- The digest of a snapshot input lists its selected files by `name`.
- The digest of a layer lists its files by `path`.
- The code hash lists the code files by their path relative to the repository root, with `/`.
  A path adds the files that `git ls-files --cached --others --exclude-standard` lists for
  it: the files that git tracks or does not ignore. A path that lists no file fails the step,
  and so does a repository root that is not a git checkout. A crate adds its `Cargo.toml`, `build.rs`
  and `src/` the same way. Each path dependency that is not a dev-dependency adds the same,
  and so do its own path dependencies, as `cargo metadata --no-deps` lists them. A path
  dependency must be a workspace member. A `.rs` file of a crate also adds each file that it
  names in `include_str!`, `include_bytes!`, `include!` or `#[path = "…"]`, and an added `.rs`
  file adds its own. The name is a string literal, normal or raw, relative to the file, or a
  `concat!` of string literals, relative to the file or after `env!("CARGO_MANIFEST_DIR")`.
  These are not code unless the step declares them: a name that a literal with an escape, a
  constant or another macro gives; `#[path]` in an inline module; a file that `build.rs` reads;
  and `Cargo.lock`.

The key is the SHA-256 of this JSON object, as the compact output of `serde_json` with the keys
of each object in byte order:

| Key | Value |
| --- | --- |
| `step` | The layer name |
| `command` | The program and its arguments, or `null` for a Rust step |
| `inputs` | One `{"kind", "name", "digest"}` per input, sorted by `kind`, then `name`. `kind` is `snapshot` or `layer`; `name` is the source id or the layer name |
| `options` | The options |
| `code` | The code hash |
| `outputs` | The declared outputs, sorted |

An input layer enters a key with its digest, not with its key. A rebuild that gives the same
files gives the same digest, so the keys of the layers that read it do not change, and the
engine reuses them. A snapshot version enters a key the same way, by the digest of its files.

A key holds no version of an installed tool, such as the Python interpreter or Java. The first
Python step that ships adds a `uv.lock`; from then on, a Python step runs with
`uv run --locked` and declares `uv.lock` as code. A Rust step runs the code of
the running binary, but its code hash comes from the files in the repository root. `obc data`
runs with `cargo run` in the checkout that it reads, so the two are the same sources.

### The step contract

A step gets a request. A command reads it as JSON on standard input; a Rust function gets the
same fields.

| Key | Value |
| --- | --- |
| `step` | The layer name |
| `snapshots` | `{source: {file name: object path}}` |
| `layers` | `{layer name: {path in the layer: object path}}` |
| `options` | The options |
| `output` | An empty directory. The layer is the files that the step writes in it |
| `metrics` | A path. The step can write a JSON object there, for example the size of each section |

A command starts in the repository root. Its standard output and standard error go to the
standard error of the engine. Exit status 0 is success. The objects are read-only. While a step
runs, `output` and `metrics` are in `partial/layer-<key>/`; the engine removes that directory
when the step ends, also when it fails. A failed step writes no receipt.

### Offline

A step reads only its inputs: the snapshots, the layers and the options in its request. A step
does not use the network; fetchers are the only network users. The engine does not enforce this.
A step that needs a package or a tool finds it installed, or reads it as a snapshot.

### Receipt

`layers/<key>.json` is the receipt of one layer, a JSON object:

| Key | Meaning |
| --- | --- |
| `step`, `key`, `options`, `code`, `command`, `outputs` | As in the key |
| `inputs` | As in the key |
| `digest` | The digest of `files` |
| `files` | One item per file: `path` in the layer, `size` in bytes and `sha256`, sorted by `path` |
| `built` | `YYYY-MM-DDTHH:MM:SSZ` |
| `wall_ms` | The time from start to end, in milliseconds |
| `cpu_ms` | User and system time in milliseconds of the command and the children it waited for (`wait4`). For a Rust step, of the whole process, so it is exact only while no other step runs. `null` when the system does not report it |
| `peak_rss_bytes` | The peak resident set of the command or of a child it waited for. For a Rust step, the peak of the process when the step raised it, else `null` |
| `bytes_in` | The size of the input files |
| `bytes_out` | The size of `files` |
| `metrics` | The JSON object the step wrote, or `{}` |

## Commands

| Command | Output |
| --- | --- |
| `obc data sources [--json]` | Every source with licence, R2 copy, live pin, newest upstream version, age, policy and state. Rows are in kind order: data, then assets, then tools |
| `obc data fetch SOURCE[@VERSION] [NAME=VALUE…] [--json]` | Fetches the version, or else the live pin, or else the newest file upstream. Writes the store path of each file |
| `obc data refresh SOURCE [NAME=VALUE…] [--env ENV] [--json]` | Fetches the newest upstream version, checked now, and writes it to `[pins]` of `data/env/ENV.toml` (default `live`). `ENV` is lowercase kebab-case. The edit keeps comments, line order and CRLF line ends. Writes the store path of each file |
| `obc data region [list] [--json]` | Every region with its name and definition |
| `obc data region show ID [--json]` | One region, the regions it resolves to, and its box when every part is a box |

`--json` writes one JSON document to standard output:

- `sources`: `{"sources": [...]}`. Each item has the keys of its `[[source]]` table, and
  `pin`, `upstream` (the newest upstream version, or `null`), `age_days`, `state` and `reason`
  (`null` when there is nothing to say).
- `fetch` and `refresh`: the snapshot, `{"source": ..., "version": ..., "files": [...]}`, with
  the requested files only. Each file has the keys of the record and `path`, its object.
- `region list`: `{"regions": [...]}`. Each item has `id`, `name`, `kind` and the key its
  kind names. A `box` is an object with `west`, `south`, `east` and `north`.
- `region show`: the region item, and `leaves` (the region ids it resolves to) and
  `bounds` (a box or `null`).

A command that fails writes the reason to standard error. The exit status is 0 when the command
succeeds, 1 when a file under `data/` is not valid or a fetch fails, and 2 for a usage error,
which includes an unknown region id, an unknown source id and a missing or invalid environment
name.

## R2 client

The R2 client in `host/obc-data` reaches the bucket for `obc bake publish --target r2`,
`obc bake clean-r2`, `obc r2 rm`, `obc fixtures publish` and the firmware publish workflow.
rclone moves the bytes. The planner publish, deploy and finalize, and the reference archive
ingest, still use the remote of `tools/r2.py`. `obc data r2` is plumbing for scripts: those
commands call it. It does not change the state of a release.

### Credentials

| Variable | Meaning |
| --- | --- |
| `OBC_R2_BUCKET`, `OBC_R2_ACCESS_KEY_ID`, `OBC_R2_SECRET_ACCESS_KEY` | The bucket and its key |
| `OBC_R2_ACCOUNT_ID` | The Cloudflare account; the endpoint is `https://<id>.r2.cloudflarestorage.com` |
| `OBC_R2_ENDPOINT` | Another endpoint; replaces the one from `OBC_R2_ACCOUNT_ID` |
| `OBC_R2_LOCAL_DIR` | A local directory that replaces the bucket. Tests use it. When it is set, it must not be empty, and `OBC_R2_BUCKET` must not be set |

`--fixtures` reads the same variables with the prefix `OBC_FIXTURE_R2_` instead. A credential
goes to rclone only in its environment, never in an argument or a file.

### Commands

| Command | Does |
| --- | --- |
| `obc data r2 list PREFIX [--json]` | Lists every object under the folder `PREFIX`. A `PREFIX` that names an object is refused |
| `obc data r2 stat KEY... [--json]` | Lists the objects of the keys that the bucket holds; a missing key is not an error |
| `obc data r2 get KEY FILE` | Downloads one object |
| `obc data r2 put FILE KEY [--cache-control V] [--content-type V] [--immutable]` | Uploads with `--checksum` and the headers given, then verifies |
| `obc data r2 delete (KEY... \| --prefix P) --reason TEXT [--yes]` | Deletes, see below |

`--json` writes `{"bucket": TEXT, "objects": [{"key", "bytes", "modified"}]}`. `bucket` names
the bucket or the local directory, never a credential. `modified` is the upload time.

Rules:

- A key or a prefix is never empty, has no empty, `.` or `..` part, and has no control
  character. The bucket root is never a target.
- Verify passes when the object has the size of the file, and the same MD5 when both sides
  report one.
- `--immutable`: when the key holds an object, nothing is uploaded and the object is never
  replaced. The command passes only when the object has the size and the MD5 of the file. It
  fails when they differ, and when the bucket reports no MD5 for the object.
- `delete --prefix P` resolves `P` to its keys first. Then explicit keys and a prefix have the
  same rules: a key that the bucket does not hold is refused, `removed.jsonl` is refused, and
  then nothing is deleted.
- `delete` prints the plan: the bucket, and each object with its size and upload time. Then it
  asks in the terminal. Without a terminal it needs `--yes`; without `--yes` it exits 2 and
  changes nothing.
- `delete` appends one line per object to `removed.jsonl` at the bucket root before it deletes:
  `{"by": USER, "bytes": N, "key": KEY, "reason": TEXT, "removed": "YYYY-MM-DDTHH:MM:SSZ"}`,
  with the escapes of Python's `json.dumps`: `\uXXXX` for DEL and for each character outside
  ASCII. When the delete fails, the error names the keys that the bucket still holds.

The exit status is 0 on success, 1 when R2 or rclone fails or the person answers no, and 2 for a
usage error or a delete without consent.
