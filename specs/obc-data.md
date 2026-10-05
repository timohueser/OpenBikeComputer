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
| `fetch` | table | yes | `kind`, `url`, and for `osm` only `from`: the id of the source whose pin is the base day. See below |
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
| `osm` | The daily OSM replication diffs |
| `geofabrik` | A Geofabrik extract or `.poly` of an area |
| `glo30` | Copernicus GLO-30 tiles |
| `dtm` | A national terrain model service |
| `capture` | Requests to a query service or an API |
| `github` | A GitHub release asset or source archive |
| `by-hand` | A person orders or downloads the files at `url` |
| `installed` | A person installs it, or another source's build brings it. It has no `url` |

`fetch.url` is an `https://` URL template. `{name}` stands for a value the fetcher fills:
`{version}` is the pin, `{yymmdd}` a date pin as `YYMMDD`, `{area}` a Geofabrik area, `{tile}` a
tile name. In `attribution`,
`{year}` and `{month}` are the year and month of the data, which the step that writes the
credit fills.

Rules:

- An id is listed once.
- Each kind of fetch but `installed` has a `url`, and the `url` starts with `https://`.
- `refresh` in days needs `version = "date"`, because only a date pin has an age.
- `r2_copy = true` needs `redistribute = true`, because R2 is public.
- A credential has `env` or `file`, not both.
- An `osm` fetch has `from`, and `from` names a source. No other fetch has `from`.

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

The environment name is lowercase kebab-case.

| Key | Type | Meaning |
| --- | --- | --- |
| `region` | string | The region of both products: a region id of `data/regions/`. `plan` and `build` refuse a file without it |
| `layers` | array of strings | The optional layers that are on, each once. Each is an optional layer of a product |
| `[pins]` | table | A source id to the version that the environment is built from |

Each pin names a source of `data/sources.toml`. A pin of a source with `version = "date"` is a
`YYYY-MM-DD` date. `obc data sources` reads `data/env/live.toml`, and the file must exist.

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
| `countries` | array of strings | Optional: the ISO 3166-1 alpha-2 codes of the countries in the region, such as `DE` |
| `time_zone` | string | Optional: the IANA time zone of the region, such as `Europe/Berlin` |

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
| `obc data`, product `planner` | The `geofabrik` region of the environment, its `countries` and its `time_zone` |
| `fixtures/build-map-package.sh` | The `box` regions of the fixtures |

`tools/data_registry.py box ID [--lat-first]` prints the box of a `box` region and refuses
every other kind, so Python and shell never resolve a union or a Geofabrik area.

### `data/planner.toml`

The options of the [planner layers](#planner) that are the same for each region.

| Key | Type | Meaning |
| --- | --- | --- |
| `terrain.margin_m` | number | The terrain reaches at least this far around the region, in metres: the horizon of the sun layer |
| `routing.profiles` | array of strings | The profiles of the routing package. The route catalog needs `touring`, `road`, `gravel`, `mtb` and `hiking` |
| `climate.first_year` | integer | The first of the ten years of ERA5-Land that the climate layer reads |
| `snow.seasons` | array of 2 integers | The first and the last season of HR-WSI Snow Phenology that the snow layer reads |
| `sun.horizon_samples`, `sun.horizon_directions` | integer | The horizon profiles of the sun layer: [the sun archive](planner-sun-tiles.md) |

## State of a source

`obc data sources` computes the state of each source when it runs. It stores nothing.

| State | When |
| --- | --- |
| `blocked` | A `data` or `asset` source has no `licence`, or its credential is not on this machine |
| `stale` | The pin is a date, `refresh` is in days, the pin is older than `refresh`, and the newest upstream version is later than the pin. Or the pin is before the pin of the source that `fetch.from` names |
| `ok` | Otherwise. A source with no pin is never stale, and a source with `refresh = "manual"` is never stale by age |

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
| `releases/<product>/<id>.json` | The manifest of a release, see [Releases](#releases) |
| `code/<hash>.json` | The code files of a code hash: `{path: sha256}`. A run writes it for each step that it reads or builds |
| `requests/<source>/<sha256>.json` | The files that a fetch with `NAME=VALUE` gave: `version`, `params` and `files` (names). The name is the SHA-256 of the compact JSON `[version, params]`, with `params` sorted. A record with no files selects no file |
| `runs/<id>.jsonl` | The events of one run, see [Runs](#runs) |
| `upstream/<source>.json` | The last upstream check of a source: `checked` (seconds since 1970-01-01 UTC), `version` (a string, or `null` when the check failed) and `error` (only when it failed) |
| `imports/<YYYYMMDDTHHMMSSZ>.jsonl` | The import record of one `obc data clean --apply`, see [Clean](#clean) |
| `partial/` | Downloads that are not complete, the validators that resume them, and the layers that steps write |
| `locks/` | One lock file per key and per run |

Rules:

- An object is complete. A download goes to `partial/` and moves into `objects/` with one
  rename after the digest check.
- A record goes to a temporary file in its directory and replaces the old record with one
  rename.
- One process at a time downloads a URL, one process at a time writes a snapshot record, and one
  process at a time builds a layer key.
- A fetch, a run of the engine and an import hold the store lock (`locks/store.lock`) shared. A
  collection holds it alone.

A snapshot record is a JSON object:

| Key | Meaning |
| --- | --- |
| `source` | The source id |
| `version` | The version, as a pin names it |
| `files` | One item per file: `name`, `url`, `size` in bytes, `sha256` and `retrieved` (`YYYY-MM-DDTHH:MM:SSZ`) |

The `name` of a file is the part of its URL that identifies it in the source. For an `osm`,
`dtm` or `capture` fetch, it is the URL after `fetch.url`, such as `000/005/130.osc.gz` or
`#bbox=W,S,E,N/sub/a.tif`. For a URL template, it is the URL from the segment of the first
`{name}` other than `{version}` and `{yymmdd}`, such as `europe/monaco-261003.osm.pbf`; without
such a `{name}`, it is the last segment. A name is unique in a record: a file whose name the
record has for another URL fails the fetch.

A version is one or more segments joined by `/`. A segment has letters, digits, `.`, `_`, `+`
and `-`, and does not start with `.`. A `date` version is also a `YYYY-MM-DD` date, and a
`digest` version is 64 lowercase hex digits.

### Clean

`obc data clean` shows one plan: the collection, then the import. With `--apply`, it asks once,
as [Errors](#errors) says, and then collects and imports. The collection deletes only the plan
that it showed: when the plan of now differs, it deletes nothing. The import moves the files that
exist when it runs, and records each one.

The import moves the cache directories of the older bake tools into the store:
`~/.cache/obcm`, `~/.cache/obc/planner`, `~/.cache/openbikecomputer`, `~/obc-bake` and
`~/obc-reference`. Each regular file becomes an object, so the same bytes are one object.

- The command resolves symbolic links in the path of the store and of each directory. The store
  and the files below it stay where they are, also when the store is in one of these
  directories. A directory inside the store is refused.
- The plan counts the files and their size, hashes nothing and changes nothing. It does not
  follow a link. What stays is symbolic links and other entries that are not regular files.
- One import runs at a time, and it holds the store lock shared. For each file,
  it checks the size and the modification time before and after it reads the file. On the file
  system of the store, it hashes the file in place and renames it into `objects/`, or deletes it
  when the object exists. On another file system, it copies the file to `partial/import-<pid>`,
  hashes the copy, makes it an object and deletes the file. A file that changed stays where it is.
  When it changed after the rename, it goes back to its path, or, when a new file has that path,
  beside it as `<name>.changed-<pid>`. The import starts by deleting the copies that a stopped
  import left.
- Before a file moves, the import adds its line to the import record and writes it to the disk.
  A line of a file that then stays is only one more root of a collection. After the files, the
  import deletes each directory that is empty, and a symbolic link whose target it deleted. What
  it did not move stays, and the command lists it. An import that stops keeps its record; the next
  import moves the rest.
- A process that writes a file after the last check can change an object. Stop the bakes, the
  planner and every fetch before `--apply`.

The import record has one JSON object per line: `dir` (without symbolic links), `path` (below
`dir`, with `/`), `size` and `sha256`.

The collection deletes what no live release, pin or fixture reaches. Its roots are the live
releases (see [Live](#live)), the files of the checkout that it runs in, and the store:

- A snapshot record is reached when a layer of a live release read its source and version, or
  when `[pins]` of a `data/env/*.toml` file names them. The newest record of each source, by the latest `retrieved` of its files, is also
  reached: a bake without a pin reads it, and a source whose upstream gives only its newest file
  cannot give it again. So is the newest version of each request record (`requests/`), by the
  latest `retrieved` of its files, such as the extract of each Geofabrik area.
- An object is reached when a reached snapshot record or a reached layer has it, or when its
  SHA-256 is a file of a live layer, or is in a pin, `fixtures/catalog.toml`, a JSON or TOML file below `fixtures/sources/`, a
  planner region recipe in `tools/planner-regions/`, or an import record. Deleting an import
  record releases its objects.
- A layer is reached when each of its inputs is reached: a snapshot input whose digest is the
  digest of all the files, or of one file, of a reached record of its source, and a layer input
  whose digest is the digest of the files that it selects of a reached layer of its step: the
  `files` of the input, or all files when it names none.

The plan lists what the collection deletes and what stays. What stays is one entry for each
reached snapshot record, with the reasons: `live PRODUCT, …`, `pin of ENV, …`,
`newest of the source`, `newest of a request`. Then one entry for the reached layers of each step
(`inputs kept`). Then one entry for each kind of root that names objects that no reached record
or layer has: `live release`, `pin`, `fixture`, `planner recipe` or `import record`. The size of
an entry is the size of its files. The collection takes the store lock alone, or refuses to start
while a fetch, a build or an import holds it. Then it deletes each snapshot record and each object
that is not reached. Receipts, release manifests, import records and upstream checks stay. A
clean that cannot read live deletes nothing.

## Fetch

A fetch fills each `{name}` of `fetch.url`: `{version}` and `{yymmdd}` from the version, and each
other name from a `NAME=VALUE` argument. A name can have more than one value; then the fetch gets one file
for each value. A file that the snapshot record of the version has, and whose object exists,
comes from the store with no request. The fetch records each file when its download is complete,
so a fetch that fails keeps the files before the failure. A record that has the URL with another
SHA-256 fails the fetch.

Which version a fetch gets:

- A URL with `{version}` or `{yymmdd}` gives the version that it names.
- A URL without them gives only the newest file upstream. For a `date` source without a
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
| `osm` | The daily diffs from a base day, see below |
| `dtm` | A program writes the files, see below |
| `capture` | A program writes the files, see below |
| `by-hand`, `installed` | None; the fetch fails |

A `geofabrik` URL with `{yymmdd}` names the day of the data of the extract. Without a version, the
fetch reads the day from the `timestamp` of `<area>-updates/state.txt`; for more than one area,
it takes the earliest day.

The OSM planet is two sources with date versions. `osm-planet` is the weekly planet file of one
day, an `http` URL with `{yymmdd}`. `osm-replication` is the daily diffs. Both pins together fix
the bytes of the OSM data.

An `osm` URL is an Osmosis replication directory that ends with `/`, such as
`<server>/replication/day/`. A fetch of version `E` takes `from=B` and no other `NAME=VALUE`.
Without `from=`, `B` is the pin of the `fetch.from` source: the live pin for `fetch`, the
`--env` pin for `refresh`. An explicit `from=` must be that pin, unless there is none. `B` is on or before `E`. The sequence
of a day is the one diff whose `state.txt` has the `timestamp` of that day; a day with two
diffs has no sequence. The snapshot is the
`state.txt` of the sequence of `B`, and the diff and the `state.txt` of each sequence after it up
to the sequence of `E`, in order.

- When the store has the diff and the `state.txt` of each of these sequences, in the record of
  any version, the fetch makes no request. The stored states name the day of each sequence.
- Else the fetch reads the newest `state.txt` first, and fails when `E` is after its day. Then it
  finds the sequence of `B` and of `E`: the newest sequence less the days between, then, when
  the `timestamp` of that sequence is another day, moved by the difference once, and the states
  of the sequences next to it. It fails before a diff downloads when it finds no sequence of `B`
  or `E`. These reads record nothing.
- A diff and its state never change. A file in the record of another version comes from the
  store, so a fetch of a later `E` from the same `B` downloads only the new days.

A late or missing weekly planet does not block a version of `osm-replication`, because its base
is the planet that is pinned. The fetch does not apply the diffs. A step does that with
`osmium apply-changes`: it reads the planet of the `osm-planet` pin and the diffs of the
`osm-replication` pin from that day.

A `dtm` or `capture` fetch runs a program in the repository root, with the Python of `uv run
<packages> python`: `--no-project --python '>=3.12' --with-requirements <file>` for a requirements file, and
`--locked --group <group>` for a dependency group of `pyproject.toml` (`OBC_PYTHON` replaces that
Python).
The program writes each file of the request to a directory under `partial/`, and its progress to
standard error. The store takes every file in that directory but hidden, `.part` and `.tmp`
files. A failed run keeps the directory and writes no record; the next run of the same request,
also on a later day, resumes from it. A directory older than the `refresh` of the source is
deleted before the run. In the record, the `url` of a file is `<fetch.url>#<query>/<path in the
directory>`, with the `fetch.url` of the source whose record takes the file. The fetch checks
every record that it adds to before it writes one. Each fetch takes the `NAME=VALUE` of its row,
each once, and no other. A `capture` source without a row has no fetcher yet; the fetch fails.

| Source | `NAME=VALUE` | Program | Packages | Query |
| --- | --- | --- | --- | --- |
| `dtm-*` | `bbox` | `host/obc-dem/reference/ingest.py fetch` | `tools/requirements-bake.txt` | `bbox=W,S,E,N` |
| `modis-snow`, `hr-wsi` | `bbox`, `seasons=FIRST-LAST` | `tools/planner_snow.py --fetch` | group `planner-snow` | `bbox=W,S,E,N&seasons=FIRST-LAST` |
| `osm-trails` | `bbox` | `tools/planner_snow.py --fetch-trails` | group `planner-snow` | `bbox=W,S,E,N` |
| `era5-land` | `bbox`, `first-year` | `tools/planner_climate.py --fetch` | group `planner-climate` | `bbox=W,S,E,N&first-year=YEAR` |
| `wikidata`, `wikipedia`, `commons` | `boundary`, `candidates`, `select-with` | `tools/landmark_capture.py --retry-failed` | none (`python3`) | `recipe=` and 16 hex digits of the SHA-256 of the joined hex SHA-256 of the boundary, the candidates, `host/obc-pack/src/landmarks/policy.json`, `specs/content-languages.json` and `tools/landmark_capture.py` |

- `bbox` is `WEST,SOUTH,EAST,NORTH` in degrees.
- A snow file is the window of one source raster that covers `bbox`, one pixel wider on each
  side, in the grid of the source. A season starts on 1 September.
- The `osm-trails` file is `trails.json`: the JSON answer of Overpass, as it is, to the query of
  the ways with `highway=path` or `highway=track` that `bbox` touches.
- An `era5-land` file is a source chunk of the ten years from `first-year`, or the orography.
- `boundary` and `candidates` are the files of `landmark_capture.py`, and `select-with` is the
  `obc-bake` that selects the places. One run captures the three sources, and each record takes
  the files of its licence: `wikipedia` takes `articles/`, `commons` takes `images/` and
  `categories/`, and `wikidata` takes the other files. Every record takes `recipe.json`, which
  links the three. No record takes the copies of the inputs (`boundary.geojson`,
  `candidates.json`, `policy.json`) or `attempts/`. The program exits with status 2 when the
  capture is not complete; then the fetch fails and the next run asks again for what failed.
- The version is the day of the fetch, because the service answers with current data. Another
  day comes only from the store.
- A source with a `credential` that is not on this machine fails before the program runs, and
  the error names the variables or the file.

The upstream check finds the newest version of a source with one request, which has 15 seconds.
The store keeps its answer, or its failure, for one hour.

| Source | Check |
| --- | --- |
| `osm` | `GET` of `<fetch.url>state.txt`; the day of its `timestamp` |
| `http`, and a URL whose only `{name}` is `{yymmdd}` | `HEAD` of the URL with `latest` for `{yymmdd}`, not following the redirect; the day in the file name of its `Location` |
| `capture` | Today, with no request: a query service answers with current data |
| `github`, `commit` | The GitHub API: the newest commit of the default branch |
| `github`, `release` | The GitHub API: the tag of the newest release that has the asset of the URL |
| `http`, `geofabrik` or `glo30`, `date`, and a URL without `{name}` | `HEAD` of the URL; the `Last-Modified` day |
| Every other source | None; the source cannot be checked |

## Layers

A step makes one layer from snapshots, the layers of other steps and options. A
[plan](#plan) lists the steps to build, and a [run](#runs) builds them in dependency order. The
engine builds a step only when the store has no receipt for the key of the step, or when an
object of that receipt is missing.

A step declares:

| Field | Meaning |
| --- | --- |
| `name` | The layer name: lowercase kebab-case segments joined by `/` |
| `inputs` | Snapshots, as `source`, `version`, `params` and `files`. With `params` (the `NAME=VALUE` of the fetch), the step reads the files that a fetch with them gives, and `files` is empty. Without, `files` names the files that the step reads, or is empty for every file. The layers of other steps, as `name` and `files`: the paths in the layer that the step reads, or none for every file. A selected path that the layer does not have fails the step |
| `options` | A JSON object |
| `code` | `paths`: files and directories, relative to the repository root. `crates`: workspace crates. A Rust step declares the crate of its function |
| `outputs` | Paths in the output directory. A path is a file, or a directory whose files are all part of the layer. The step must write each path and no other file. A symbolic link fails the step |
| `run` | A Rust function in the process, or a command: a program and its arguments. No argument names a path outside the repository root: no argument is an absolute path, contains `=/` or has a `..` segment between `/` and `=` |

One binary links the Rust steps of every product, so Cargo unifies their features. A step crate
enables every feature its bytes depend on itself, or makes its bytes independent of it (structs,
or sorted keys for JSON objects).

### Keys

The digest of a list of files is the SHA-256 of the text that `sha256sum` writes for them: one
line `<sha256>  <name>` with a final newline per file, in byte order of the names.

- The digest of a snapshot input lists its selected files by `name`.
- The digest of a layer lists its files by `path`. The digest of a layer input lists the files
  that it selects.
- The code hash lists the code files by their path relative to the repository root, with `/`.
  A path adds the files that `git ls-files --cached --others --exclude-standard` lists for
  it: the files that git tracks or does not ignore. A path that lists no file fails the step,
  and so does a repository root that is not a git checkout. A crate adds its `Cargo.toml`, `build.rs`
  and `src/` the same way. Each path dependency that is not a dev-dependency adds the same,
  and so do its own path dependencies, as `cargo metadata --no-deps` lists them. A path
  dependency must be a workspace member. The walk stops at the engine crate `obc-data`: the
  engine only selects inputs, and what it selects is in the input digests. A crate that the
  walk reaches through another crate is still code. A step whose bytes use `obc_data::sources`,
  such as an attribution, declares `data/sources.toml` in its code paths. A `.rs` file of a
  crate also adds each file that it names in `include_str!`, `include_bytes!`, `include!` or `#[path = "…"]`, and an added `.rs`
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
`uv run --locked` and declares `uv.lock` as code. A Rust step runs the code of the running
binary, but its code hash comes from the files in the repository root. `obc data` runs with
`cargo run` in the checkout that it reads, so the two are the same sources.

The recipe of a step is the SHA-256 of the same object, with each input as `{"kind", "name", "files"}`
for a layer and `{"kind", "name", "version", "params", "files"}` for a snapshot, `params` and
`files` sorted, and the inputs sorted by their compact JSON. It holds no digest, so a plan has it
also for a step whose key waits for a fetch or another build.

### The step contract

A step gets a request. A command reads it as JSON on standard input; a Rust function gets the
same fields.

| Key | Value |
| --- | --- |
| `step` | The layer name |
| `snapshots` | `{source: {file name: object path}}` |
| `layers` | `{layer name: {path in the layer: object path}}`, with the files that the input selects |
| `options` | The options |
| `output` | An empty directory. The layer is the files that the step writes in it |
| `metrics` | A path. The step can write a JSON object there, for example the size of each section |

A command starts in the repository root. Its standard output and standard error go to the
standard error of the engine. Exit status 0 is success. The objects are read-only. While a step
runs, `output` and `metrics` are in `partial/layer-<key>/`; the engine removes that directory
when the step ends, also when it fails. A failed step writes no receipt.

### Offline

A step reads only its inputs: the snapshots, the layers and the options in its request. A step
does not use the network; fetchers are the only network users. A step must not write to its
inputs: their paths, and the links of a view, are objects of the store, and a step that runs as
root can write to a read-only object. The engine does not enforce this.
A step that needs a package or a tool finds it installed, or reads it as a snapshot.

For a tool that reads a directory, `engine::view` in Rust and `view` of `tools/step_request.py`
in Python make a new directory with one symbolic link per file of an input, to its object. A
step makes it beside `output`, in `partial/layer-<key>/`, which the engine removes when the step
ends.

### Receipt

`layers/<key>.json` is the receipt of one layer, a JSON object:

| Key | Meaning |
| --- | --- |
| `step`, `key`, `options`, `code`, `command`, `outputs` | As in the key |
| `inputs` | As in the key, and `files` for a layer input that selects files: the selected paths, sorted |
| `digest` | The digest of `files` |
| `files` | One item per file: `path` in the layer, `size` in bytes and `sha256`, sorted by `path` |
| `built` | `YYYY-MM-DDTHH:MM:SSZ` |
| `wall_ms` | The time from start to end, in milliseconds |
| `cpu_ms` | User and system time in milliseconds of the command and the children it waited for (`wait4`). For a Rust step, of the whole process, so it is exact only while no other step runs. `null` when the system does not report it |
| `peak_rss_bytes` | The peak resident set of the command or of a child it waited for. For a Rust step, the peak of the whole process so far |
| `bytes_in` | The size of the input files |
| `bytes_out` | The size of `files` |
| `metrics` | The JSON object the step wrote, or `{}` |

### Plan

A plan lists what a run fetches and builds. It is a JSON object, `{"groups": [...]}`. The engine
looks at the steps in dependency order:

- A snapshot input needs a fetch when the store cannot give the files that it reads: no record
  of a fetch with its `params`, no record of the version, no selected file in it, or no object of
  a selected file.
- A step needs a build when a snapshot that it reads needs a fetch, when a layer that it reads
  needs a build, or when the store has no layer for its key.
- The plan does not list the other steps: the store has their layers.

A group is one change. Two builds are in the same group when one reads the layer of the other.
Thus a run of one group never needs a build of another group. Two groups can need the same
fetch. `--only GROUP,…` selects groups by their `id`. An `id` names a group only in the plan that
it comes from.

| Key | Value |
| --- | --- |
| `id` | The step of the first build of the group, in dependency order |
| `fetches` | One per version and `params`: `source`, `version`, `params` (`[[NAME, VALUE], …]`), `files` and `bytes`. `files` are the names of the files that the store lacks. `[]` means that the store cannot name them, and the fetch gets every file that it gives |
| `builds` | In dependency order: `step`, `recipe` (see [Keys](#keys)), `key` (`null` while the step waits for a fetch or another build) and `estimate` |

The estimate of a build is `wall_ms`, `bytes_out` and `peak_rss_bytes` of the newest receipt, by
`built`, of the same step with the same options. Without one, it is the newest receipt of the
step, or `null` when the store has none. The `bytes` of a fetch is the size of its files in the
record of the version. When that record does not list them all, it is the size in the record of
another version that lists them all, the last in byte order. Without `files`, the files are those
that a fetch with the same `params` gave, or else every file. Otherwise `bytes` is `null`. A run
fetches a version with the same `params` once, with the files of every group that needs it.

### Runs

A run fetches the fetches of a plan, one after another, with the fetchers of [Fetch](#fetch).
Then it builds the builds of the plan. A layer that a planned step reads, and that the plan does
not build, must be in the store. A planned key that is not the key of the step now fails the run
with "the plan is outdated; plan again".

- A step starts when the layers that it reads are built.
- At most one step per core runs at a time.
- At most one Rust step runs at a time: its CPU time and peak are those of the whole process.
  Command steps run in parallel.
- The sum of the estimated peaks (`peak_rss_bytes` of the estimate) of the steps that run at the
  same time is not more than the physical memory of the machine. A step without an estimated
  peak counts as the whole memory. A step whose estimate is more than the memory runs alone.
- After a fetch fails, no step starts. After a step fails, no other step starts, and the steps
  that run finish. The layers that the run built stay in the store, so the next plan does not
  list them: a new run continues after the failed step.

The id of a run is its start time in UTC, `YYYY-MM-DD-HHMMSS`, with `-2`, `-3` and so on when
another run has that id. The process that holds the lock `run-<id>` is the process that runs the
run. A run without a `finished` event whose lock is free has failed. A command that gets
`--detach` must start a child process that creates the run and gives its id back; no command has
`--detach` yet.

`runs/<id>.jsonl` has one JSON object per line. The key `event` gives its kind:

| `event` | Keys |
| --- | --- |
| `started` | `command`, and `at` (`YYYY-MM-DDTHH:MM:SSZ`) |
| `fetch_started` | `source`, `version` and `params` |
| `fetch_finished` | `source`, `version`, `params`, `bytes` (the size of the files that the fetch gave, downloaded or found in the store) and `wall_ms` |
| `fetch_failed` | `source`, `version`, `params` and `error` |
| `step_started` | `step` |
| `step_finished` | `step`, `reused` and `receipt` |
| `step_failed` | `step` and `error` |
| `finished` | `ok`, `error` (`null` when `ok`) and `wall_ms` |

### Products

A product is a set of steps that makes one release, such as `planner` or `maps`. The
`obc data` binary (`host/obc-data-steps`, GPL-3.0-only) gives the list of products to the
commands of `host/obc-data`. The `obc-data-plumbing` binary of `host/obc-data` has the same
commands without products, for scripts that fetch or use R2. Each layer name of a product starts
with `<product>/`.

A product gives its steps for an environment, its regions and the store. When the step list
depends on a snapshot that the store does not have, such as the `.poly` of a region, the product
names those fetches instead. `plan` and `build` fetch them and then ask the product again, as
long as each round names only new fetches: a fetched file can name the next fetch, such as the
`.poly` that gives the box of a capture. A fetch that fails, fails the command with its own code,
`fetch_failed` or `blocked`. A product that names a fetch it named before, that still names
fetches after 8 rounds, or a step name without `<product>/`, fails the command with
`failed` and a fix that points at the code of the product. A product that has no steps for the
environment, such as for a kind of region that it does not read, is blocked: `plan` lists it in
`blocked` with the reason, and the other products plan without it. `build` builds the groups of
the other products and writes no release of a blocked product. When no product suits the
environment, `build` fails with `blocked`. An error of the store, a file or the data of a fetch
in a step list fails the command with `failed`.

`plan ENV` plans the steps of every product together. `--json` writes the plan with `env`,
`region` and `layers` of the environment, and `only`, the groups that `--only` selected or `[]`
for every group. `build ENV --plan FILE` builds the groups of that file, or those of them that
its own `--only` selects. It refuses the file, with exit status 3, before it fetches or builds:

- when `env`, `region`, `layers` or `blocked` differ from the environment and its products;
- when a product names a fetch: `plan` fetched what each step list reads;
- when the groups that `only` selects in the plan of now differ from the groups of the file,
  apart from `estimate` and `bytes`.

When the store has the layer of every step of a product after the run, `build` writes the
release of that product.

#### `maps`

The device maps have layers per leaf: a cell of size `2^23` µdeg of the OBCA grid that the
outline of the region touches. The outline of a `box` region is its box; the outline of a
`geofabrik` region is its `.poly` from `geofabrik-poly`, `area=<region id>`. The product has no
steps for a `polygon` region yet. The environment pins `copernicus-glo-30`. Only a `geofabrik`
region has map cells: they read the OSM of its area. Another region has terrain only.

A file that the step list reads, such as a `.poly` or the GLO-30 tile list, is at the version
of the environment. Without one, the product names a fetch of the newest version upstream, and
then reads the newest version of that fetch in the store.

| Layer | Reads | Options | Files |
| --- | --- | --- | --- |
| `maps/osm` | `geofabrik-extracts`, `area=<region id>` | `leaves`: `[i, j]` of each leaf | `osm/<i>-<j>.osm.pbf`: the `osmium extract --strategy smart --set-bounds` of the square of the leaf and one µdeg around it. The key holds no Osmium version: another Osmium can give other bytes. The metrics name the version (`osmium`) |
| `maps/<band>/<i>-<j>` | `maps/osm`, the file of the leaf; `land-polygons`; `maps/terrain/<i>-<j>` when the cells of the band read heights: contours in their levels, or a nav graph or POIs | `band`: `coarse`, `mid`, `fine` or `network` of the recommended band table (`OBCA_Spec.md`); `leaf`: `[23, i, j]`; `cells`: `[ci, cj]` of each cell of the band in the leaf that the outline touches | `cells/<band>/<ci>/<cj>.obcm` for each cell with content; `cells/<band>/empty.json`: the ids of the other cells. A cell has the bytes that one cut of the whole leaf with all bands writes, with `builder/presets/schema.json` and without landmarks or peaks |
| `maps/terrain/<i>-<j>` | `copernicus-glo-30`, `tile=` of each tile that the square of a cell reaches and that `copernicus-glo-30-tiles` names. A square without a tile is sea. A leaf without a tile reads no snapshot | `posting_log2` and `cell_log2` of OBCT v1; `cells`: `[ci, cj]` of each terrain cell in the leaf that the outline touches | `terrain/<ci>/<cj>.obcd` for each cell with a height (`OBCC_Spec.md` §13); `terrain/empty.json`: the ids of the cells without a height |

`<i>`, `<j>`, `<ci>` and `<cj>` have four digits or more, as in a cell id.

#### `planner`

The planner has steps for a `geofabrik` region that names its `countries`: its OSM is the
extract of that one area. The bounds of the region are the box around its `.poly`. The
environment pins `copernicus-glo-30`. Every other snapshot, such as the extract, the `.poly`
and the GLO-30 tile list, is at the version of the environment, or else the newest version in the
store, as for `maps`. The
other options come from [`data/planner.toml`](#dataplannertoml). A GLO-30 input reads the tile
of each 1° square that its box touches and that `copernicus-glo-30-tiles` names; a box at sea
reads no snapshot. No layer reads a national terrain model yet.

| Layer | Reads | Options | Files |
| --- | --- | --- | --- |
| `planner/osm` | `geofabrik-extracts`, `area=<region id>` | `path`: `osm.pbf` | `osm.pbf`: the extract as it is. The engine step `pass` writes it, so its code is no file |
| `planner/terrain` | The GLO-30 tiles of `bounds` | `bounds`: west, south, east and north of the zoom 10 tiles that the bounds of the region touch and of their neighbours, widened to `terrain.margin_m` around the bounds | `terrain.mbtiles`: lossless Terrarium WebP tiles of zooms 0 to 12, the bytes that `planner-dem` writes from the same tiles |
| `planner/routing` | `planner/osm`, and the GLO-30 tiles of the bounds of the region | `region` (the last part of the region id), `bounds`, `profiles` and `countries`. The import applies the German access defaults | `routing/`: the package of [the route package contract](route-package.md) with `overlays.sqlite` and `route-catalog.json`; `blocks/`: the routing blocks of the grid cells, as `route-blocks` writes them; `routes/<cell>.json`: the records of `route-catalog.json` that name the cell, with a final newline |
| `planner/overlays` | `planner/routing` | `attribution` of `osm-planet` | `overlays.pmtiles`: the route networks and the access of `routing/overlays.sqlite`, as [the planner release](planner-release.md) defines it |
| `planner/assets` | `protomaps-assets`, `tangrams-icons` | None | `assets/fonts/` and `assets/sprites/` of the assets archive; `assets/sprites/LICENSE.txt`: the MIT notice of `tangrams-icons` |
| `planner/model` | `query-model` | None | `model/`: `model.int8.onnx`, `tokenizer.json` and `tokenizer_config.json` of the archive, and `labels.json`, the labels of the query schema |
| `planner/climate` | `era5-land`, `bbox=<bounds>`, `first-year=<climate.first_year>` | `bounds`, `first_year`, and `attribution` of `era5-land`, whose `{year}` is the year after the ten years, `first_year` + 10 | `climate.pmtiles`: [the climate archive](planner-climate-tiles.md) |
| `planner/snow` | `hr-wsi`, `bbox=<bounds>`, `seasons=<snow.seasons>` | `bounds`, `seasons`, `attribution` of `hr-wsi`, and `year`: the year of the `hr-wsi` version, which fills its `{year}` | `snow.pmtiles`: [the snow archive](planner-snow-tiles.md) from HR-WSI |
| `planner/sun` | `planner/terrain` | `bounds`, `time_zone` of the region, `distance_m` (`terrain.margin_m`), `horizon_samples` and `horizon_directions` | `sun.pmtiles`: [the sun archive](planner-sun-tiles.md). `terrain_sha256` is the SHA-256 of the PMTiles archive that the step converts from `terrain.mbtiles` |

`climate`, `snow` and `sun` are optional layers: a step only when `layers` of the environment
names it. The sun layer needs the `time_zone` of the region.

A Python step runs `uv run --locked --offline --group <group> python <entry> --step` in the
repository root, with the packages of a dependency group of `pyproject.toml`; `uv sync
--all-groups` installs them on a machine. Its code is each Python file that it imports,
`tools/step_request.py`, `.python-version`, `pyproject.toml` and `uv.lock`. A credit that it
writes comes in its options, so `data/sources.toml` is no code of it.

A grid cell is a zoom 9 Web Mercator tile that the bounds of the region overlap, clipped to the
bounds, with the id `9-<x>-<y>`. The JSON objects that `planner/routing` writes have their keys in
byte order.

### Releases

`releases/<product>/<id>.json` is the manifest of a release: `{"product", "layers"}`, as the
compact output of `serde_json` with the keys of each object in byte order. The id is the
SHA-256 of these bytes. A manifest holds no time or cost of a build, so two machines that build
the same layers make the same release. `layers` is sorted by `step`, and each layer has:

| Key | Meaning |
| --- | --- |
| `step`, `key`, `inputs`, `options`, `code`, `command`, `outputs`, `digest`, `files` | As in the [receipt](#receipt) |
| `snapshots` | `{source: {"version", "params"}}`: the version and the sorted `NAME=VALUE` of each snapshot that the layer read |

The objects of a release are the `files` of its layers.

### State of a layer

The engine computes the state of each layer when it is asked, and stores nothing. It compares
the step with the layer that live has: a layer of a live release manifest. The state of a source
is as in [State of a source](#state-of-a-source).

| State | When | Reason |
| --- | --- | --- |
| `not applied` | Live has no layer of the step | `missing in live` |
| `not applied` | The options are not the options of the live layer | `options` |
| `not applied` | The step reads a source that the live layer read, with another version or other `params` (in any order), or the store has the files that it reads and their digest is not the one that the live layer read | `SOURCE@VERSION not in live`, and ` (not fetched)` when the store does not have the files |
| `code changed` | The inputs (kind and name), the command or the outputs are not those of the live layer | `inputs`, `command` or `outputs` |
| `code changed` | The code hash is not the one of the live layer | The first code file that changed, and `and N more`. The declared code when the store has no `code/<hash>.json` of the live layer |
| `input changed` | A layer that the step reads is `not applied`, `code changed` or `input changed`, or its live layer is not the one that the live layer of the step read | The name of that layer |
| `stale` | A source that the step reads is `stale` | `SOURCE: ` and the reason of the source |
| `blocked` | A source that the step reads is `blocked` | `SOURCE: ` and the reason of the source |
| `ok` | Otherwise | `null` |

When more than one row applies, the first row gives the state. In JSON, a state is `ok`,
`stale`, `code_changed`, `input_changed`, `not_applied` or `blocked`. Each layer also has `live`
(the key of the live layer, or `null`), `reads` (`kind`, `name`, and `version` for a snapshot),
`code` (`paths` and `crates`), `code_hash` and `users` (the layers that read it).

## Live

Live is the release that the pointer of each product names on R2. Each product has one prefix:
`cell-catalog` for `maps`, `planner` for `planner`. The input copies are under `inputs`. Live
owns the prefix of each product that has a live release, and `inputs` once any release is live;
a product with nothing live owns no prefix, so nothing under it is ever a leftover. The shared
`inputs` is owned once any release is live, so an input copy that only a product with nothing live
would use is a leftover.

| Key | Holds |
| --- | --- |
| `<prefix>/catalog.json` | The pointer: the document that clients read, with `"release": "<id>"`. A pointer without `release` names no release: nothing is live |
| `<prefix>/releases/<id>.json` | The manifest of the release, as in the store. Immutable |
| `<prefix>/releases/<id>/<path>` | A file of the release that a client finds by name. Immutable |
| `<prefix>/objects/<sha256>` | A file of a layer of a release. Immutable |
| `inputs/records/<source>/<version>.json` | The snapshot record of an input copy: a version of a source with `r2_copy` that a live layer read |
| `inputs/objects/<sha256>` | A file of an input copy. Immutable |

When `OBC_R2_BUCKET` or `OBC_R2_LOCAL_DIR` is set, `obc data` reads that bucket. Otherwise it
reads the pointers, the manifests and the records at `https://maps.openbikecomputer.com/<key>`;
a listing needs the bucket. A release id is 64 lowercase hex digits. The store keeps each manifest
that it reads, and uses its copy only while the SHA-256 of the copy is the id and the copy is of
the product.

`status --check` lists the owned prefixes, and compares them with live:

- drift: a key of a live release or of its input copies that R2 does not have, or has with
  another size. Pointers and records of input copies have no expected size.
- leftovers: a key under the owned prefixes that no live release uses. The files of
  `<prefix>/releases/<id>/` of a live release are never leftovers.

Exit status 1 of `status --check` is drift or leftovers, or a failure of R2. With `--json`, the
first writes the status, and the second writes an error.

## Commands

| Command | Output |
| --- | --- |
| `obc data [--json]` | In a terminal, and without `--json`: the TUI. Otherwise the output of `status` |
| `obc data status [--check] [--json]` | Where live was read; per product, the live release (or nothing live) and the state of each layer of the environment `live`; what needs attention: stale and blocked sources, old cache directories that `clean` imports, and with `--check` drift and leftovers. When a fetch that the step list of a product needs fails, the layer states of that product are unknown (`layers` is `null`), and attention gives the error. `--check` adds the listing of [Live](#live) and exits with 1 when it finds drift or leftovers. Without the bucket, `--check` exits with 4 before it reads anything |
| `obc data sources [--check-now] [--json]` | Every source with licence, R2 copy, live pin, newest upstream version, age, policy, state and the versions in the local store. Rows are in kind order: data, then assets, then tools. An upstream check of the last hour serves, except with `--check-now` |
| `obc data fetch SOURCE[@VERSION] [NAME=VALUE…] [--json]` | Fetches the version, or else the live pin, or else the newest file upstream. Writes the store path of each file |
| `obc data refresh SOURCE [NAME=VALUE…] [--env ENV] [--json]` | Fetches the newest upstream version, checked now, and writes it to `[pins]` of `data/env/ENV.toml` (default `live`). `ENV` is lowercase kebab-case. The edit keeps comments, line order and CRLF line ends. Writes the store path of each file. A version after the pin of a source whose `fetch.from` names `SOURCE` is refused before the fetch: refresh that source first |
| `obc data policy SOURCE 7\|30\|90\|365\|manual [--json]` | Writes `refresh` of the source in `data/sources.toml`. The edit keeps comments and the other lines. A policy in days for a source without `version = "date"` is refused. Writes the source |
| `obc data clean [--apply [--yes]] [--json]` | The plan of [Clean](#clean): the snapshot records and the objects that nothing reaches, what stays and why, and the old cache directories with their files and sizes. `--apply` asks, then cleans. With `--json` and `--apply`, the plan goes to standard error, and the output is what it did |
| `obc data region [list] [--json]` | Every region with its name and definition |
| `obc data region show ID [--json]` | One region, the regions it resolves to, and its box when every part is a box |
| `obc data plan ENV [--only GROUP,…] [--json]` | What a build of the environment fetches and builds, in groups, with estimates. It fetches what a step list depends on, see [Products](#products) |
| `obc data build ENV [--only GROUP,…] [--plan FILE] [--json]` | Fetches and builds the groups into the store, and writes the release of each product whose every layer is built. It uploads nothing |
| `obc data runs [--json]` | Every run in the store, newest first: id, command, outcome, time, and the size of its fetches and of the layers that it built |
| `obc data runs RUN [--json]` | One run, its fetches, and its steps: time, change since the last run that built the step, peak RAM, output, inputs, code hash and users |
| `obc data runs RUN --follow [--json]` | The events of the run, and each new event until the run ends |

`--json` writes one JSON document to standard output. [JSON schemas](#json-schemas) has the
schema of each output, and [Errors](#errors) has the error codes and the exit statuses. In
addition:

- `fetch` and `refresh` list only the requested files.
- `runs` lists a run file that cannot be read as `failed`, or as `running` while its lock is
  held.
- `runs RUN --follow` writes one event per line, as in `runs/<id>.jsonl`. When the run failed,
  the error is the last line.

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
| `obc data r2 get KEY FILE [--json]` | Downloads one object |
| `obc data r2 put FILE KEY [--cache-control V] [--content-type V] [--immutable] [--json]` | Uploads with `--checksum` and the headers given, then verifies |
| `obc data r2 delete (KEY... \| --prefix P) --reason TEXT [--yes] [--json]` | Deletes, see below |

Rules:

- The `r2` commands are plumbing below the rule of [Errors](#errors). Only `delete` asks,
  because a delete cannot be undone. `put` and `get` never ask.
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
- `delete` changes live. It prints the plan: the bucket, and each object with its size and
  upload time. With `--json`, the plan goes to standard error, and the output is the objects
  that it deleted. Then it asks as [Errors](#errors) says.
- `delete` appends one line per object to `removed.jsonl` at the bucket root before it deletes:
  `{"by": USER, "bytes": N, "key": KEY, "reason": TEXT, "removed": "YYYY-MM-DDTHH:MM:SSZ"}`,
  with the escapes of Python's `json.dumps`: `\uXXXX` for DEL and for each character outside
  ASCII. When the delete fails, the error names the keys that the bucket still holds.

## Errors

A command that fails writes its error to standard error. With `--json`, it writes
`{"error": {"code", "message", "fix"}}` on one line to standard output. The code is stable and
sets the exit status. The message tells what failed, and the fix tells what to do.

| Exit status | Meaning |
| --- | --- |
| 0 | The command succeeded |
| 1 | A check found problems, or the command failed: a file is not valid; a fetch, R2, a run or the store failed; or the person did not agree |
| 2 | Usage: an argument is not valid, or a command that changes live did not get consent |
| 3 | The plan is outdated: live, the steps or the store changed after the plan was made. Plan again |
| 4 | Blocked: a credential is missing |
| 5 | Verify failed: the bytes in a target are not the bytes that the command wrote |

A command that changes live shows its plan and asks in a terminal. Without a terminal, it needs
`--yes`, or `--plan FILE` when the command takes a plan file. Without them, it exits with 2 and
changes nothing.

### Error codes

| Code | Exit | When | Fix |
| --- | --- | --- | --- |
| `usage` | 2 | An argument is not valid, an id names nothing, the command runs outside the repository, or
another command must run first. | Correct the command. `obc data --help` lists the commands and their arguments. |
| `no_terminal` | 2 | A command that changes live has no terminal to ask in, and no `--yes`. | Show the plan to a person. When they agree, run the command again with `--yes`. |
| `not_confirmed` | 1 | The person did not answer yes. | Nothing changed. Run the command again when you want the change. |
| `invalid_data` | 1 | A file under `data/` is not valid. | Correct the file that the message names. `specs/obc-data.md` gives its format. |
| `fetch_failed` | 1 | A fetch or an upstream check failed. | Run the command again. A download continues where it stopped. |
| `blocked` | 4 | A credential is missing: a fetch failed without the credential of its source, or the R2
variables are not set. Or `build` has no product that suits the environment. | Set the credential that the message or `obc data sources` names, or correct what the message says a product needs, then run again. |
| `r2_failed` | 1 | R2 or rclone failed, or refused a key. | Check the key, the `OBC_R2_*` variables and that rclone is on PATH, then run again. |
| `verify_failed` | 5 | After an upload, the object in the bucket is not the file. | Upload the file again. |
| `run_failed` | 1 | A run failed: the build, or the run that `runs RUN --follow` shows. | `obc data runs RUN` shows the step that failed and its error. |
| `plan_outdated` | 3 | The plan file is not the plan of now: live, the steps or the store changed after it was made. | Make the plan again with `obc data plan ENV --json`, read it, and pass the new file. |
| `failed` | 1 | The store or the file system failed. | Correct the file or the directory that the message names, then run again. |

## JSON schemas

`--json` writes one document of the schema in this table to standard output. The schemas
come from the Rust types in `host/obc-data`. A test fails when this section is not the one
that they give; `OBC_UPDATE_DATA_SPEC=1 cargo test -p obc-data` writes it again.

| Command | Schema |
| --- | --- |
| `sources` | `Sources` |
| `fetch`, `refresh` | `Fetched` |
| `policy` | `Source` |
| `region`, `region list` | `RegionList` |
| `region show` | `RegionDetail` |
| `status`, and `obc data` without a terminal | `Status` |
| `clean`, `clean --apply` | `CleanPlan` |
| `plan` | `EnvPlan` |
| `build` | `Built` |
| `runs` | `RunList` |
| `runs RUN` | `Details` |
| `runs RUN --follow`, one per line | `Event` |
| `r2 list`, `r2 stat`, `r2 delete` | `Objects` |
| `r2 get` | `Downloaded` |
| `r2 put` | `Uploaded` |
| Every command that fails | `Failure` |

```json
{
  "$defs": {
    "Attention": {
      "description": "Something that needs a person.",
      "properties": {
        "about": {
          "description": "The source, the directory, the product, or `R2`.",
          "type": "string"
        },
        "kind": {
          "$ref": "#/$defs/AttentionKind"
        },
        "reason": {
          "type": "string"
        }
      },
      "required": [
        "kind",
        "about",
        "reason"
      ],
      "type": "object"
    },
    "AttentionKind": {
      "oneOf": [
        {
          "const": "stale",
          "description": "A source that is stale.",
          "type": "string"
        },
        {
          "const": "blocked",
          "description": "A source that is blocked.",
          "type": "string"
        },
        {
          "const": "old_cache",
          "description": "A cache directory of the older bake tools that `clean` moves into the store.",
          "type": "string"
        },
        {
          "const": "drift",
          "description": "Keys that live uses and R2 lacks, or holds with another size.",
          "type": "string"
        },
        {
          "const": "leftovers",
          "description": "Keys under the owned prefixes that no live release uses.",
          "type": "string"
        },
        {
          "const": "unreachable",
          "description": "A fetch that the step list of a product needs failed, so its layer states are unknown.",
          "type": "string"
        }
      ]
    },
    "Bbox": {
      "description": "Degrees, longitude first.",
      "properties": {
        "east": {
          "format": "double",
          "type": "number"
        },
        "north": {
          "format": "double",
          "type": "number"
        },
        "south": {
          "format": "double",
          "type": "number"
        },
        "west": {
          "format": "double",
          "type": "number"
        }
      },
      "required": [
        "west",
        "south",
        "east",
        "north"
      ],
      "type": "object"
    },
    "BlockedProduct": {
      "additionalProperties": false,
      "properties": {
        "product": {
          "type": "string"
        },
        "reason": {
          "type": "string"
        }
      },
      "required": [
        "product",
        "reason"
      ],
      "type": "object"
    },
    "Built": {
      "description": "What a build did.",
      "properties": {
        "blocked": {
          "description": "The products that give no steps for the environment; nothing of them is built.",
          "items": {
            "$ref": "#/$defs/BlockedProduct"
          },
          "type": "array"
        },
        "layers": {
          "description": "The layers of the run, in dependency order.",
          "items": {
            "$ref": "#/$defs/BuiltLayer"
          },
          "type": "array"
        },
        "releases": {
          "description": "The release of each product whose every layer is built.",
          "items": {
            "$ref": "#/$defs/BuiltRelease"
          },
          "type": "array"
        },
        "run": {
          "description": "`None` when there was nothing to fetch or build.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "run",
        "layers",
        "releases",
        "blocked"
      ],
      "type": "object"
    },
    "BuiltLayer": {
      "properties": {
        "key": {
          "type": "string"
        },
        "reused": {
          "type": "boolean"
        },
        "step": {
          "type": "string"
        }
      },
      "required": [
        "step",
        "key",
        "reused"
      ],
      "type": "object"
    },
    "BuiltRelease": {
      "properties": {
        "id": {
          "description": "The SHA-256 of `releases/<product>/<id>.json` in the store.",
          "type": "string"
        },
        "product": {
          "type": "string"
        }
      },
      "required": [
        "product",
        "id"
      ],
      "type": "object"
    },
    "Check": {
      "description": "The owned prefixes of R2 against live.",
      "properties": {
        "drift": {
          "description": "The keys that live uses and that R2 lacks, or holds with another size.",
          "items": {
            "$ref": "#/$defs/Drift"
          },
          "type": "array"
        },
        "leftovers": {
          "description": "The keys under the prefixes that no live release uses.",
          "items": {
            "$ref": "#/$defs/Object"
          },
          "type": "array"
        },
        "prefixes": {
          "description": "The prefixes that were listed.",
          "items": {
            "type": "string"
          },
          "type": "array"
        }
      },
      "required": [
        "prefixes",
        "drift",
        "leftovers"
      ],
      "type": "object"
    },
    "CleanPlan": {
      "description": "What `clean` removes from the store and moves into it, or removed and moved.",
      "properties": {
        "import": {
          "$ref": "#/$defs/ImportPlan",
          "description": "The cache directories of the older bake tools."
        },
        "store": {
          "$ref": "#/$defs/GcPlan",
          "description": "The snapshot records and the objects that nothing reaches, and what stays."
        }
      },
      "required": [
        "store",
        "import"
      ],
      "type": "object"
    },
    "Code": {
      "description": "The kind of an error. It sets the exit status and the fix.",
      "oneOf": [
        {
          "const": "usage",
          "description": "An argument is not valid, an id names nothing, the command runs outside the repository, or\nanother command must run first.",
          "type": "string"
        },
        {
          "const": "no_terminal",
          "description": "A command that changes live has no terminal to ask in, and no `--yes`.",
          "type": "string"
        },
        {
          "const": "not_confirmed",
          "description": "The person did not answer yes.",
          "type": "string"
        },
        {
          "const": "invalid_data",
          "description": "A file under `data/` is not valid.",
          "type": "string"
        },
        {
          "const": "fetch_failed",
          "description": "A fetch or an upstream check failed.",
          "type": "string"
        },
        {
          "const": "blocked",
          "description": "A credential is missing: a fetch failed without the credential of its source, or the R2\nvariables are not set. Or `build` has no product that suits the environment.",
          "type": "string"
        },
        {
          "const": "r2_failed",
          "description": "R2 or rclone failed, or refused a key.",
          "type": "string"
        },
        {
          "const": "verify_failed",
          "description": "After an upload, the object in the bucket is not the file.",
          "type": "string"
        },
        {
          "const": "run_failed",
          "description": "A run failed: the build, or the run that `runs RUN --follow` shows.",
          "type": "string"
        },
        {
          "const": "plan_outdated",
          "description": "The plan file is not the plan of now: live, the steps or the store changed after it was made.",
          "type": "string"
        },
        {
          "const": "failed",
          "description": "The store or the file system failed.",
          "type": "string"
        }
      ]
    },
    "Credential": {
      "additionalProperties": false,
      "description": "What a fetch needs before upstream answers: environment variables, or a file.",
      "properties": {
        "env": {
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "file": {
          "type": [
            "string",
            "null"
          ]
        }
      },
      "type": "object"
    },
    "Details": {
      "additionalProperties": false,
      "description": "A run with its fetches and steps, as `obc data runs RUN` shows it.",
      "properties": {
        "bytes_built": {
          "description": "The size of the layers that it built; a reused layer is not counted.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "bytes_fetched": {
          "description": "The size of the files of its fetches.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "command": {
          "type": "string"
        },
        "error": {
          "type": [
            "string",
            "null"
          ]
        },
        "fetches": {
          "items": {
            "$ref": "#/$defs/RunFetch"
          },
          "type": "array"
        },
        "id": {
          "type": "string"
        },
        "outcome": {
          "$ref": "#/$defs/Outcome"
        },
        "started": {
          "description": "`YYYY-MM-DDTHH:MM:SSZ`",
          "type": "string"
        },
        "steps": {
          "description": "In the order they started.",
          "items": {
            "$ref": "#/$defs/RunStep"
          },
          "type": "array"
        },
        "wall_ms": {
          "description": "`None` until it finishes.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        }
      },
      "required": [
        "id",
        "command",
        "started",
        "outcome",
        "wall_ms",
        "bytes_fetched",
        "bytes_built",
        "error",
        "fetches",
        "steps"
      ],
      "type": "object"
    },
    "Downloaded": {
      "properties": {
        "file": {
          "type": "string"
        },
        "key": {
          "type": "string"
        }
      },
      "required": [
        "key",
        "file"
      ],
      "type": "object"
    },
    "Drift": {
      "properties": {
        "expected": {
          "description": "The size that live needs, when it is known.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "found": {
          "description": "The size on R2; `None` when R2 lacks the key.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "key": {
          "type": "string"
        }
      },
      "required": [
        "key",
        "expected",
        "found"
      ],
      "type": "object"
    },
    "EnvPlan": {
      "additionalProperties": false,
      "description": "What a build of an environment would fetch and build.",
      "properties": {
        "blocked": {
          "description": "The products that give no steps for the environment. The others plan without them.",
          "items": {
            "$ref": "#/$defs/BlockedProduct"
          },
          "type": "array"
        },
        "env": {
          "type": "string"
        },
        "groups": {
          "items": {
            "$ref": "#/$defs/PlanGroup"
          },
          "type": "array"
        },
        "layers": {
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "only": {
          "description": "The groups that `--only` selected, or none for every group.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "region": {
          "type": "string"
        }
      },
      "required": [
        "env",
        "region",
        "layers",
        "only",
        "groups",
        "blocked"
      ],
      "type": "object"
    },
    "Error": {
      "description": "Why a command failed, and what to do about it.",
      "properties": {
        "code": {
          "$ref": "#/$defs/Code"
        },
        "fix": {
          "type": "string"
        },
        "message": {
          "type": "string"
        }
      },
      "required": [
        "code",
        "message",
        "fix"
      ],
      "type": "object"
    },
    "Estimate": {
      "additionalProperties": false,
      "properties": {
        "bytes_out": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "peak_rss_bytes": {
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "wall_ms": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "wall_ms",
        "bytes_out",
        "peak_rss_bytes"
      ],
      "type": "object"
    },
    "Event": {
      "description": "One line of `runs/<id>.jsonl`.",
      "oneOf": [
        {
          "additionalProperties": false,
          "properties": {
            "at": {
              "description": "`YYYY-MM-DDTHH:MM:SSZ`",
              "type": "string"
            },
            "command": {
              "type": "string"
            },
            "event": {
              "const": "started",
              "type": "string"
            }
          },
          "required": [
            "event",
            "command",
            "at"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "properties": {
            "event": {
              "const": "fetch_started",
              "type": "string"
            },
            "params": {
              "items": {
                "maxItems": 2,
                "minItems": 2,
                "prefixItems": [
                  {
                    "type": "string"
                  },
                  {
                    "type": "string"
                  }
                ],
                "type": "array"
              },
              "type": "array"
            },
            "source": {
              "type": "string"
            },
            "version": {
              "type": "string"
            }
          },
          "required": [
            "event",
            "source",
            "version",
            "params"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "properties": {
            "bytes": {
              "description": "The size of the files that the fetch gave, downloaded or found in the store.",
              "format": "uint64",
              "minimum": 0,
              "type": "integer"
            },
            "event": {
              "const": "fetch_finished",
              "type": "string"
            },
            "params": {
              "items": {
                "maxItems": 2,
                "minItems": 2,
                "prefixItems": [
                  {
                    "type": "string"
                  },
                  {
                    "type": "string"
                  }
                ],
                "type": "array"
              },
              "type": "array"
            },
            "source": {
              "type": "string"
            },
            "version": {
              "type": "string"
            },
            "wall_ms": {
              "format": "uint64",
              "minimum": 0,
              "type": "integer"
            }
          },
          "required": [
            "event",
            "source",
            "version",
            "params",
            "bytes",
            "wall_ms"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "properties": {
            "error": {
              "type": "string"
            },
            "event": {
              "const": "fetch_failed",
              "type": "string"
            },
            "params": {
              "items": {
                "maxItems": 2,
                "minItems": 2,
                "prefixItems": [
                  {
                    "type": "string"
                  },
                  {
                    "type": "string"
                  }
                ],
                "type": "array"
              },
              "type": "array"
            },
            "source": {
              "type": "string"
            },
            "version": {
              "type": "string"
            }
          },
          "required": [
            "event",
            "source",
            "version",
            "params",
            "error"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "properties": {
            "event": {
              "const": "step_started",
              "type": "string"
            },
            "step": {
              "type": "string"
            }
          },
          "required": [
            "event",
            "step"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "properties": {
            "event": {
              "const": "step_finished",
              "type": "string"
            },
            "receipt": {
              "$ref": "#/$defs/Receipt"
            },
            "reused": {
              "type": "boolean"
            },
            "step": {
              "type": "string"
            }
          },
          "required": [
            "event",
            "step",
            "reused",
            "receipt"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "properties": {
            "error": {
              "type": "string"
            },
            "event": {
              "const": "step_failed",
              "type": "string"
            },
            "step": {
              "type": "string"
            }
          },
          "required": [
            "event",
            "step",
            "error"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "properties": {
            "error": {
              "type": [
                "string",
                "null"
              ]
            },
            "event": {
              "const": "finished",
              "type": "string"
            },
            "ok": {
              "type": "boolean"
            },
            "wall_ms": {
              "format": "uint64",
              "minimum": 0,
              "type": "integer"
            }
          },
          "required": [
            "event",
            "ok",
            "error",
            "wall_ms"
          ],
          "type": "object"
        }
      ]
    },
    "Failure": {
      "description": "What `--json` writes when a command fails.",
      "properties": {
        "error": {
          "$ref": "#/$defs/Error"
        }
      },
      "required": [
        "error"
      ],
      "type": "object"
    },
    "Fetch": {
      "additionalProperties": false,
      "properties": {
        "from": {
          "description": "Only for `osm`: the source whose pin is the base day, `from=`.",
          "type": [
            "string",
            "null"
          ]
        },
        "kind": {
          "$ref": "#/$defs/FetchKind"
        },
        "url": {
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "kind"
      ],
      "type": "object"
    },
    "FetchKind": {
      "oneOf": [
        {
          "enum": [
            "http",
            "osm",
            "geofabrik",
            "glo30",
            "dtm",
            "capture",
            "github"
          ],
          "type": "string"
        },
        {
          "const": "by-hand",
          "description": "A person orders or downloads the files.",
          "type": "string"
        },
        {
          "const": "installed",
          "description": "A person installs it, or another source's build brings it.",
          "type": "string"
        }
      ]
    },
    "Fetched": {
      "description": "The requested files of a snapshot.",
      "properties": {
        "files": {
          "items": {
            "$ref": "#/$defs/FetchedFile"
          },
          "type": "array"
        },
        "source": {
          "type": "string"
        },
        "version": {
          "type": "string"
        }
      },
      "required": [
        "source",
        "version",
        "files"
      ],
      "type": "object"
    },
    "FetchedFile": {
      "properties": {
        "name": {
          "description": "The part of `url` that names the file within its source, unique in a record: the name a\nstep gives the file when it needs one.",
          "type": "string"
        },
        "path": {
          "description": "The object in the store.",
          "type": "string"
        },
        "retrieved": {
          "description": "`YYYY-MM-DDTHH:MM:SSZ`",
          "type": "string"
        },
        "sha256": {
          "type": "string"
        },
        "size": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "url": {
          "type": "string"
        }
      },
      "required": [
        "name",
        "url",
        "size",
        "sha256",
        "retrieved",
        "path"
      ],
      "type": "object"
    },
    "GcPlan": {
      "description": "What `clean` deletes from the store, or deleted, and what stays.",
      "properties": {
        "keep_bytes": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "keep_objects": {
          "description": "The objects that stay, and their size.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "kept": {
          "description": "What stays, and why.",
          "items": {
            "$ref": "#/$defs/Kept"
          },
          "type": "array"
        },
        "objects": {
          "description": "SHA-256 and size of each object that nothing reaches.",
          "items": {
            "maxItems": 2,
            "minItems": 2,
            "prefixItems": [
              {
                "type": "string"
              },
              {
                "format": "uint64",
                "minimum": 0,
                "type": "integer"
              }
            ],
            "type": "array"
          },
          "type": "array"
        },
        "remove_bytes": {
          "description": "The size of `objects`.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "snapshots": {
          "description": "`source@version` of each snapshot record that nothing reaches.",
          "items": {
            "type": "string"
          },
          "type": "array"
        }
      },
      "required": [
        "kept",
        "snapshots",
        "objects",
        "remove_bytes",
        "keep_objects",
        "keep_bytes"
      ],
      "type": "object"
    },
    "ImportDir": {
      "properties": {
        "bytes": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "dir": {
          "description": "The directory, without symbolic links when it is present.",
          "type": "string"
        },
        "files": {
          "description": "The regular files that it moves, or moved, and their size.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "left": {
          "description": "What stays in the directory: symbolic links and other entries that are not regular files,\nand after an import each file that changed while it was read.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "present": {
          "type": "boolean"
        }
      },
      "required": [
        "dir",
        "present",
        "files",
        "bytes",
        "left"
      ],
      "type": "object"
    },
    "ImportPlan": {
      "description": "What `clean` moves into the store, or moved, and what stays.",
      "properties": {
        "bytes": {
          "description": "The size of every file.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "dirs": {
          "items": {
            "$ref": "#/$defs/ImportDir"
          },
          "type": "array"
        }
      },
      "required": [
        "dirs",
        "bytes"
      ],
      "type": "object"
    },
    "InputKind": {
      "enum": [
        "layer",
        "snapshot"
      ],
      "type": "string"
    },
    "InputRecord": {
      "additionalProperties": false,
      "properties": {
        "digest": {
          "type": "string"
        },
        "files": {
          "description": "The paths that a layer input selects, sorted, or none for every file. Not in the key: the\ndigest names the paths.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "kind": {
          "$ref": "#/$defs/InputKind"
        },
        "name": {
          "description": "The source id or the layer name.",
          "type": "string"
        }
      },
      "required": [
        "kind",
        "name",
        "digest"
      ],
      "type": "object"
    },
    "Kept": {
      "description": "A snapshot record, the layers of one step, or the objects that one kind of root names and no\nkept record or layer has.",
      "properties": {
        "because": {
          "description": "`live PRODUCT, …`, `pin of ENV, …`, `newest of the source`, `newest of a request`,\n`inputs kept`, `live release`, `pin`, `fixture`, `planner recipe` or `import record`.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "bytes": {
          "description": "The size of its files.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "entry": {
          "description": "`source@version`, a step, or `N objects`.",
          "type": "string"
        }
      },
      "required": [
        "entry",
        "bytes",
        "because"
      ],
      "type": "object"
    },
    "Kind": {
      "description": "In the order `obc data sources` lists them.",
      "oneOf": [
        {
          "const": "data",
          "description": "An input that steps read.",
          "type": "string"
        },
        {
          "const": "asset",
          "description": "A file that ships to users as it is.",
          "type": "string"
        },
        {
          "const": "tool",
          "description": "Code that steps run; nothing of it ships.",
          "type": "string"
        }
      ]
    },
    "LayerFile": {
      "additionalProperties": false,
      "properties": {
        "path": {
          "type": "string"
        },
        "sha256": {
          "type": "string"
        },
        "size": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "path",
        "size",
        "sha256"
      ],
      "type": "object"
    },
    "LayerStatus": {
      "properties": {
        "layer": {
          "type": "string"
        },
        "reason": {
          "type": [
            "string",
            "null"
          ]
        },
        "state": {
          "$ref": "#/$defs/State"
        }
      },
      "required": [
        "layer",
        "state",
        "reason"
      ],
      "type": "object"
    },
    "Object": {
      "description": "One object in the bucket.",
      "properties": {
        "bytes": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "key": {
          "type": "string"
        },
        "modified": {
          "description": "The upload time that the bucket reports.",
          "type": "string"
        }
      },
      "required": [
        "key",
        "bytes",
        "modified"
      ],
      "type": "object"
    },
    "Objects": {
      "description": "The objects of a bucket that a command lists or deletes.",
      "properties": {
        "bucket": {
          "description": "The bucket or the local directory, never a credential.",
          "type": "string"
        },
        "objects": {
          "items": {
            "$ref": "#/$defs/Object"
          },
          "type": "array"
        }
      },
      "required": [
        "bucket",
        "objects"
      ],
      "type": "object"
    },
    "Outcome": {
      "oneOf": [
        {
          "enum": [
            "running",
            "ok"
          ],
          "type": "string"
        },
        {
          "const": "failed",
          "description": "It failed, or its process ended before it finished.",
          "type": "string"
        }
      ]
    },
    "PlanBuild": {
      "additionalProperties": false,
      "properties": {
        "estimate": {
          "anyOf": [
            {
              "$ref": "#/$defs/Estimate"
            },
            {
              "type": "null"
            }
          ]
        },
        "key": {
          "description": "`None` until the layers and snapshots that it reads are in the store.",
          "type": [
            "string",
            "null"
          ]
        },
        "recipe": {
          "description": "The key of the step without the digests of its inputs.",
          "type": "string"
        },
        "step": {
          "type": "string"
        }
      },
      "required": [
        "step",
        "recipe",
        "key",
        "estimate"
      ],
      "type": "object"
    },
    "PlanFetch": {
      "additionalProperties": false,
      "properties": {
        "bytes": {
          "description": "The size of these files in a snapshot record of the source, or `None`.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "files": {
          "description": "The names of the files that the store lacks, or none when the store cannot name them: then\nthe fetch gets every file that it gives.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "params": {
          "items": {
            "maxItems": 2,
            "minItems": 2,
            "prefixItems": [
              {
                "type": "string"
              },
              {
                "type": "string"
              }
            ],
            "type": "array"
          },
          "type": "array"
        },
        "source": {
          "type": "string"
        },
        "version": {
          "type": "string"
        }
      },
      "required": [
        "source",
        "version",
        "params",
        "files",
        "bytes"
      ],
      "type": "object"
    },
    "PlanGroup": {
      "additionalProperties": false,
      "description": "One change: builds that read each other's layers, and the fetches that they need. A group\nnever needs a build of another group, so each can be selected alone. Two groups can need the\nsame fetch.",
      "properties": {
        "builds": {
          "description": "In dependency order.",
          "items": {
            "$ref": "#/$defs/PlanBuild"
          },
          "type": "array"
        },
        "fetches": {
          "items": {
            "$ref": "#/$defs/PlanFetch"
          },
          "type": "array"
        },
        "id": {
          "description": "The step of its first build. It names the group only in the plan that it comes from.",
          "type": "string"
        }
      },
      "required": [
        "id",
        "fetches",
        "builds"
      ],
      "type": "object"
    },
    "ProductStatus": {
      "properties": {
        "layers": {
          "description": "Each layer of the environment `live`, in dependency order; `None` when a fetch that its\nstep list needs failed, and `attention` says why.",
          "items": {
            "$ref": "#/$defs/LayerStatus"
          },
          "type": [
            "array",
            "null"
          ]
        },
        "product": {
          "type": "string"
        },
        "release": {
          "description": "The id of the live release; `None` when nothing is live.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "product",
        "release",
        "layers"
      ],
      "type": "object"
    },
    "Receipt": {
      "additionalProperties": false,
      "description": "The record of one layer: what made it, its files, and what building it cost.",
      "properties": {
        "built": {
          "description": "`YYYY-MM-DDTHH:MM:SSZ`",
          "type": "string"
        },
        "bytes_in": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "bytes_out": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "code": {
          "type": "string"
        },
        "command": {
          "items": {
            "type": "string"
          },
          "type": [
            "array",
            "null"
          ]
        },
        "cpu_ms": {
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "digest": {
          "description": "The digest of `files`: what a step that reads this layer puts in its key.",
          "type": "string"
        },
        "files": {
          "items": {
            "$ref": "#/$defs/LayerFile"
          },
          "type": "array"
        },
        "inputs": {
          "items": {
            "$ref": "#/$defs/InputRecord"
          },
          "type": "array"
        },
        "key": {
          "type": "string"
        },
        "metrics": {
          "additionalProperties": true,
          "type": "object"
        },
        "options": true,
        "outputs": {
          "description": "The declared outputs, sorted.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "peak_rss_bytes": {
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "step": {
          "type": "string"
        },
        "wall_ms": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "step",
        "key",
        "inputs",
        "options",
        "code",
        "command",
        "outputs",
        "digest",
        "files",
        "built",
        "wall_ms",
        "cpu_ms",
        "peak_rss_bytes",
        "bytes_in",
        "bytes_out",
        "metrics"
      ],
      "type": "object"
    },
    "Refresh": {
      "description": "How old a pin may get, in days, before the source is stale; `manual` is never stale.",
      "enum": [
        7,
        30,
        90,
        365,
        "manual"
      ]
    },
    "Region": {
      "oneOf": [
        {
          "description": "The Geofabrik area whose path is the region id.",
          "properties": {
            "kind": {
              "const": "geofabrik",
              "type": "string"
            }
          },
          "required": [
            "kind"
          ],
          "type": "object"
        },
        {
          "properties": {
            "box": {
              "$ref": "#/$defs/Bbox"
            },
            "kind": {
              "const": "box",
              "type": "string"
            }
          },
          "required": [
            "kind",
            "box"
          ],
          "type": "object"
        },
        {
          "description": "An Osmosis `.poly` file, relative to the region file.",
          "properties": {
            "kind": {
              "const": "polygon",
              "type": "string"
            },
            "polygon": {
              "type": "string"
            }
          },
          "required": [
            "kind",
            "polygon"
          ],
          "type": "object"
        },
        {
          "properties": {
            "kind": {
              "const": "union",
              "type": "string"
            },
            "union": {
              "items": {
                "type": "string"
              },
              "type": "array"
            }
          },
          "required": [
            "kind",
            "union"
          ],
          "type": "object"
        }
      ],
      "properties": {
        "countries": {
          "description": "The ISO 3166-1 alpha-2 codes of its countries, such as `DE`. Empty when the file names none.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "id": {
          "type": "string"
        },
        "name": {
          "type": "string"
        },
        "time_zone": {
          "description": "The IANA time zone of the region, such as `Europe/Berlin`.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "id",
        "name",
        "countries",
        "time_zone"
      ],
      "type": "object"
    },
    "RegionDetail": {
      "oneOf": [
        {
          "description": "The Geofabrik area whose path is the region id.",
          "properties": {
            "kind": {
              "const": "geofabrik",
              "type": "string"
            }
          },
          "required": [
            "kind"
          ],
          "type": "object"
        },
        {
          "properties": {
            "box": {
              "$ref": "#/$defs/Bbox"
            },
            "kind": {
              "const": "box",
              "type": "string"
            }
          },
          "required": [
            "kind",
            "box"
          ],
          "type": "object"
        },
        {
          "description": "An Osmosis `.poly` file, relative to the region file.",
          "properties": {
            "kind": {
              "const": "polygon",
              "type": "string"
            },
            "polygon": {
              "type": "string"
            }
          },
          "required": [
            "kind",
            "polygon"
          ],
          "type": "object"
        },
        {
          "properties": {
            "kind": {
              "const": "union",
              "type": "string"
            },
            "union": {
              "items": {
                "type": "string"
              },
              "type": "array"
            }
          },
          "required": [
            "kind",
            "union"
          ],
          "type": "object"
        }
      ],
      "properties": {
        "bounds": {
          "anyOf": [
            {
              "$ref": "#/$defs/Bbox"
            },
            {
              "type": "null"
            }
          ],
          "description": "Its box, when every part is a box."
        },
        "countries": {
          "description": "The ISO 3166-1 alpha-2 codes of its countries, such as `DE`. Empty when the file names none.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "id": {
          "type": "string"
        },
        "leaves": {
          "description": "The region ids that it resolves to.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "name": {
          "type": "string"
        },
        "time_zone": {
          "description": "The IANA time zone of the region, such as `Europe/Berlin`.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "id",
        "name",
        "countries",
        "time_zone",
        "leaves",
        "bounds"
      ],
      "type": "object"
    },
    "RegionList": {
      "properties": {
        "regions": {
          "items": {
            "$ref": "#/$defs/Region"
          },
          "type": "array"
        }
      },
      "required": [
        "regions"
      ],
      "type": "object"
    },
    "RunFetch": {
      "additionalProperties": false,
      "properties": {
        "bytes": {
          "description": "`None` while it runs, and when it failed.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "error": {
          "type": [
            "string",
            "null"
          ]
        },
        "params": {
          "items": {
            "maxItems": 2,
            "minItems": 2,
            "prefixItems": [
              {
                "type": "string"
              },
              {
                "type": "string"
              }
            ],
            "type": "array"
          },
          "type": "array"
        },
        "source": {
          "type": "string"
        },
        "version": {
          "type": "string"
        },
        "wall_ms": {
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        }
      },
      "required": [
        "source",
        "version",
        "params",
        "bytes",
        "wall_ms",
        "error"
      ],
      "type": "object"
    },
    "RunList": {
      "description": "Every run in the store, newest first.",
      "properties": {
        "runs": {
          "items": {
            "$ref": "#/$defs/Summary"
          },
          "type": "array"
        }
      },
      "required": [
        "runs"
      ],
      "type": "object"
    },
    "RunStep": {
      "additionalProperties": false,
      "properties": {
        "error": {
          "type": [
            "string",
            "null"
          ]
        },
        "last_wall_ms": {
          "description": "Its wall time in the newest earlier run that built it.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "receipt": {
          "anyOf": [
            {
              "$ref": "#/$defs/Receipt"
            },
            {
              "type": "null"
            }
          ],
          "description": "`None` while the step runs, and when it failed."
        },
        "reused": {
          "type": "boolean"
        },
        "step": {
          "type": "string"
        },
        "users": {
          "description": "The layers of this run that read it.",
          "items": {
            "type": "string"
          },
          "type": "array"
        }
      },
      "required": [
        "step",
        "reused",
        "receipt",
        "error",
        "users",
        "last_wall_ms"
      ],
      "type": "object"
    },
    "Source": {
      "additionalProperties": false,
      "properties": {
        "attribution": {
          "type": [
            "string",
            "null"
          ]
        },
        "credential": {
          "anyOf": [
            {
              "$ref": "#/$defs/Credential"
            },
            {
              "type": "null"
            }
          ]
        },
        "fetch": {
          "$ref": "#/$defs/Fetch"
        },
        "hosts": {
          "description": "Hosts the fetch reaches besides the host of `fetch.url`. `*.example.org` is any subdomain.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "id": {
          "type": "string"
        },
        "kind": {
          "$ref": "#/$defs/Kind"
        },
        "licence": {
          "description": "An SPDX id or `LicenseRef-…`. Unset blocks a data source or an asset.",
          "type": [
            "string",
            "null"
          ]
        },
        "licence_url": {
          "type": [
            "string",
            "null"
          ]
        },
        "obligations": {
          "type": [
            "string",
            "null"
          ]
        },
        "r2_copy": {
          "default": false,
          "description": "R2 keeps a copy, because upstream cannot give a version again.",
          "type": "boolean"
        },
        "redistribute": {
          "type": "boolean"
        },
        "refresh": {
          "$ref": "#/$defs/Refresh"
        },
        "version": {
          "$ref": "#/$defs/VersionScheme"
        }
      },
      "required": [
        "id",
        "kind",
        "fetch",
        "version",
        "refresh",
        "redistribute",
        "r2_copy"
      ],
      "type": "object"
    },
    "SourceRow": {
      "description": "A source of `data/sources.toml` with its live pin, its snapshots and its state.",
      "properties": {
        "age_days": {
          "format": "int64",
          "type": [
            "integer",
            "null"
          ]
        },
        "attribution": {
          "type": [
            "string",
            "null"
          ]
        },
        "credential": {
          "anyOf": [
            {
              "$ref": "#/$defs/Credential"
            },
            {
              "type": "null"
            }
          ]
        },
        "fetch": {
          "$ref": "#/$defs/Fetch"
        },
        "hosts": {
          "description": "Hosts the fetch reaches besides the host of `fetch.url`. `*.example.org` is any subdomain.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "id": {
          "type": "string"
        },
        "kind": {
          "$ref": "#/$defs/Kind"
        },
        "licence": {
          "description": "An SPDX id or `LicenseRef-…`. Unset blocks a data source or an asset.",
          "type": [
            "string",
            "null"
          ]
        },
        "licence_url": {
          "type": [
            "string",
            "null"
          ]
        },
        "obligations": {
          "type": [
            "string",
            "null"
          ]
        },
        "pin": {
          "type": [
            "string",
            "null"
          ]
        },
        "r2_copy": {
          "default": false,
          "description": "R2 keeps a copy, because upstream cannot give a version again.",
          "type": "boolean"
        },
        "reason": {
          "type": [
            "string",
            "null"
          ]
        },
        "redistribute": {
          "type": "boolean"
        },
        "refresh": {
          "$ref": "#/$defs/Refresh"
        },
        "snapshots": {
          "description": "The versions in the local store, the one fetched last first.",
          "items": {
            "$ref": "#/$defs/Stored"
          },
          "type": "array"
        },
        "state": {
          "$ref": "#/$defs/State"
        },
        "upstream": {
          "description": "The newest upstream version.",
          "type": [
            "string",
            "null"
          ]
        },
        "version": {
          "$ref": "#/$defs/VersionScheme"
        }
      },
      "required": [
        "id",
        "kind",
        "fetch",
        "version",
        "refresh",
        "redistribute",
        "r2_copy",
        "pin",
        "upstream",
        "age_days",
        "state",
        "reason",
        "snapshots"
      ],
      "type": "object"
    },
    "Sources": {
      "properties": {
        "sources": {
          "items": {
            "$ref": "#/$defs/SourceRow"
          },
          "type": "array"
        }
      },
      "required": [
        "sources"
      ],
      "type": "object"
    },
    "State": {
      "description": "The state of a source or a layer. A source is only ok, stale or blocked.",
      "enum": [
        "ok",
        "stale",
        "code_changed",
        "input_changed",
        "not_applied",
        "blocked"
      ],
      "type": "string"
    },
    "Status": {
      "description": "What `status` writes.",
      "properties": {
        "attention": {
          "items": {
            "$ref": "#/$defs/Attention"
          },
          "type": "array"
        },
        "check": {
          "anyOf": [
            {
              "$ref": "#/$defs/Check"
            },
            {
              "type": "null"
            }
          ],
          "description": "Only with `--check`."
        },
        "from": {
          "description": "Where live was read: the bucket, or its public URL.",
          "type": "string"
        },
        "products": {
          "items": {
            "$ref": "#/$defs/ProductStatus"
          },
          "type": "array"
        }
      },
      "required": [
        "from",
        "products",
        "attention",
        "check"
      ],
      "type": "object"
    },
    "Stored": {
      "description": "A version of a source in the local store.",
      "properties": {
        "bytes": {
          "description": "The size of its files.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "version": {
          "type": "string"
        }
      },
      "required": [
        "version",
        "bytes"
      ],
      "type": "object"
    },
    "Summary": {
      "additionalProperties": false,
      "description": "A run, as `obc data runs` lists it.",
      "properties": {
        "bytes_built": {
          "description": "The size of the layers that it built; a reused layer is not counted.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "bytes_fetched": {
          "description": "The size of the files of its fetches.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "command": {
          "type": "string"
        },
        "id": {
          "type": "string"
        },
        "outcome": {
          "$ref": "#/$defs/Outcome"
        },
        "started": {
          "description": "`YYYY-MM-DDTHH:MM:SSZ`",
          "type": "string"
        },
        "wall_ms": {
          "description": "`None` until it finishes.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        }
      },
      "required": [
        "id",
        "command",
        "started",
        "outcome",
        "wall_ms",
        "bytes_fetched",
        "bytes_built"
      ],
      "type": "object"
    },
    "Uploaded": {
      "properties": {
        "key": {
          "type": "string"
        },
        "uploaded": {
          "description": "`false` when an immutable key already held these bytes.",
          "type": "boolean"
        }
      },
      "required": [
        "key",
        "uploaded"
      ],
      "type": "object"
    },
    "VersionScheme": {
      "description": "How upstream names a version, and so what a pin of the source looks like.",
      "oneOf": [
        {
          "enum": [
            "release",
            "commit"
          ],
          "type": "string"
        },
        {
          "const": "date",
          "description": "`YYYY-MM-DD`: the only scheme that gives a pin an age.",
          "type": "string"
        },
        {
          "const": "digest",
          "description": "The SHA-256 of the file.",
          "type": "string"
        }
      ]
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema"
}
```
