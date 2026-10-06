# obc data

`obc data` reads the data registry: the external sources that a bake uses, the regions, and
the environments. It fetches sources into the store. The crate is `host/obc-data`.
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
| `fetch` | table | yes | `kind`, `url`, and for `osm` only `from`: the id of the source whose version is the base day. See below |
| `hosts` | array of strings | no | Hosts the fetch reaches besides the host of `fetch.url`: lowercase letters, digits, `.` and `-`. `*.domain` is any subdomain |
| `version` | string | yes | How upstream names a version: `date`, `release`, `commit` or `digest` |
| `refresh` | integer or string | yes | `7`, `30`, `90` or `365` days, or `"manual"` |
| `redistribute` | boolean | yes | The licence lets us give the upstream bytes to others |
| `r2_copy` | boolean | no, `false` | R2 keeps a copy of the version that live reads, because upstream cannot give it again |
| `credential` | table | no | `env`: the environment variables a fetch needs; or `file`: the file that holds them. `~/` is the home directory |
| `extent` | array of 4 numbers | no | The box outside which the source has no data: west, south, east and north in degrees, west < east, south < north. A box meets it when the two overlap; boxes that only touch do not meet |

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
`{version}` is the version, `{yymmdd}` a date version as `YYMMDD`, `{area}` a Geofabrik area, `{tile}` a
tile name. In `attribution`,
`{year}` and `{month}` are the year and month of the data, which the step that writes the
credit fills.

Rules:

- An id is listed once.
- Each kind of fetch but `installed` has a `url`, and the `url` starts with `https://`.
- `refresh` in days needs `version = "date"`, because only a date version has an age.
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
| Planner climate and snow layers | `era5-land`; `hr-wsi` when the snow layer reads it, `modis-snow` and `hansen-gfc` |
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

An environment names what it contains, not the versions of its sources: a file with `[pins]` is
refused. The live release manifests record the version of each source that live reads, see
[Versions](#versions). `data/env/live.toml` must exist.

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
| `snow.seasons` | array of 2 integers | The first and the last season of HR-WSI and MODIS that the snow layer reads |
| `sun.horizon_samples`, `sun.horizon_directions` | integer | The horizon profiles of the sun layer: [the sun archive](planner-sun-tiles.md) |

## State of a source

`obc data sources` computes the state of each source when it runs. It stores nothing.

| State | When |
| --- | --- |
| `blocked` | A `data` or `asset` source has no `licence`, or its credential is not on this machine |
| `stale` | The live version, the first in order when live reads more (for a date, the oldest), is a date, `refresh` is in days, the live version is older than `refresh`, and the newest upstream version is later than the live version. Or the live version is before the live version of the source that `fetch.from` names |
| `ok` | Otherwise. A source that live does not read is never stale, and a source with `refresh = "manual"` is never stale by age |

The live version of a source is the version that the live releases read, see [Live](#live). Its
age is the number of days from its date to today (UTC). When the live version is older than
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
| `version` | The version, as `SOURCE@VERSION` names it |
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

The collection deletes what no live release or fixture reaches. Its roots are the live
releases (see [Live](#live)), the files of the checkout that it runs in, and the store:

- A snapshot record is reached when a layer of a live release read its source and version. The
  newest record of each source, by the latest `retrieved` of its files, is also reached: a plan
  reads it when nothing else names a version (see [Versions](#versions)), and a source whose
  upstream gives only its newest file cannot give it again. So is the newest version of each request record (`requests/`), by the
  latest `retrieved` of its files, such as the extract of each Geofabrik area.
- An object is reached when a reached snapshot record or a reached layer has it, or when its
  SHA-256 is a file of a live layer, or is in `fixtures/catalog.toml`, a JSON or TOML file below `fixtures/sources/`, a
  planner region recipe in `tools/planner-regions/`, or an import record. Deleting an import
  record releases its objects.
- A layer is reached when each of its inputs is reached: a snapshot input whose digest is the
  digest of all the files, or of one file, of a reached record of its source, and a layer input
  whose digest is the digest of the files that it selects of a reached layer of its step: the
  `files` of the input, or all files when it names none.

The plan lists what the collection deletes and what stays. What stays is one entry for each
reached snapshot record, with the reasons: `live PRODUCT, …`, `newest of the source`,
`newest of a request`. Then one entry for the reached layers of each step (`inputs kept`). Then
one entry for each kind of root that names objects that no reached record or layer has:
`live release`, `fixture`, `planner recipe` or `import record`. The size of
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
| `by-hand` of a `dtm-*` source | A program takes the files from the delivery, see below |
| `by-hand`, `installed` | None; the fetch fails |

A `geofabrik` URL with `{yymmdd}` names the day of the data of the extract. Without a version, the
fetch reads the day from the `timestamp` of `<area>-updates/state.txt`; for more than one area,
it takes the earliest day.

The OSM planet is two sources with date versions. `osm-planet` is the weekly planet file of one
day, an `http` URL with `{yymmdd}`. `osm-replication` is the daily diffs. Their two versions
together fix the bytes of the OSM data.

An `osm` URL is an Osmosis replication directory that ends with `/`, such as
`<server>/replication/day/`. A fetch of version `E` takes `from=B`, a version of the
`fetch.from` source, and no other `NAME=VALUE`. `B` is on or before `E`. The sequence
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
is a version of `osm-planet`. The fetch does not apply the diffs. No product reads the two
sources: `obc data fetch` gets them, and `osm-planet` gives the OSM credit.

A `dtm` or `capture` fetch runs a program in the repository root, with the Python of `uv run
<packages> python`: `--locked --group <group>` for a dependency group of `pyproject.toml`
(`OBC_PYTHON` replaces that Python).
The program writes each file of the request to a directory under `partial/`, and its progress to
standard error. The store takes every file in that directory but hidden, `.part` and `.tmp` files.
A failed run keeps the directory and writes no record. A directory of a `manual` source, or one
older than the `refresh` of the source, is deleted before the run. The next run of the same
request resumes from any other directory, also on a later day. In the record, the `url` of a file
is `<fetch.url>#<query>/<path in the directory>`, with the `fetch.url` of the source whose record
takes the file. The fetch checks every record that it adds to before it writes one. Each fetch
takes the `NAME=VALUE` of its row, each once, and no other. A `capture` source without a row has
no fetcher yet; the fetch fails.

| Source | `NAME=VALUE` | Program | Packages | Query |
| --- | --- | --- | --- | --- |
| `dtm-*` | `bbox` | `host/obc-dem/reference/ingest.py fetch` | group `terrain-reference` | `bbox=W,S,E,N` |
| `modis-snow`, `hr-wsi` | `bbox`, `seasons=FIRST-LAST` | `tools/planner_snow.py --fetch` | group `planner-snow` | `bbox=W,S,E,N&seasons=FIRST-LAST` |
| `osm-trails` | `bbox` | `tools/planner_snow.py --fetch-trails` | group `planner-snow` | `bbox=W,S,E,N` |
| `era5-land` | `bbox`, `first-year` | `tools/planner_climate.py --fetch` | group `planner-climate` | `bbox=W,S,E,N&first-year=YEAR` |
| `wikidata`, `wikipedia`, `commons` | `collection`, `area`, `osm`, `poly` | `tools/landmark_capture.py --retry-failed` | none (`python3`) | `<collection>=` and 16 hex digits of the SHA-256 of `<collection> <area> <osm> <poly> ` and the joined hex SHA-256 of `host/obc-pack/src/landmarks/policy.json`, `specs/content-languages.json`, `tools/landmark_capture.py` and `tools/peak_capture.py` |

- `bbox` is `WEST,SOUTH,EAST,NORTH` in degrees.
- A `dtm-*` file is a raster of the model that covers `bbox`, with its `.prj` when it has one. A
  service is asked only for the part of `bbox` inside the `extent`, and a raster without a height
  is no file. A box where the model has no data gives a fetch without files: its record of the
  request names no file. A `by-hand` model takes the rasters of its delivery:
  `OBC_REFERENCE_<KEY>_INPUT` names the directory, as an absolute path, and
  `OBC_REFERENCE_<KEY>_DATUM` the vertical datum that the metadata of the order states. The two
  are the `credential` of the source.
- A snow file is the window of one source raster that covers `bbox`, one pixel wider on each
  side, in the grid of the source. A season starts on 1 September. An `hr-wsi` box without a
  product gives a fetch without files.
- The `osm-trails` file is `trails.json`: the JSON answer of Overpass, as it is, to the query of
  the ways with `highway=path` or `highway=track` that `bbox` touches.
- An `era5-land` file is a source chunk of the ten years from `first-year`, or the orography.
- `collection` is `landmarks` or `peaks`, and `area` is the region id. `osm` and `poly` are
  `sha256:<hex>`: the extract and the `.poly` of the region, files of the store. The program makes the boundary from the `.poly`
  and finds the landmark candidates or the summits in the extract. It selects the places with
  the `obc data` binary that runs the fetch, which answers the commands of
  `obc_pack::landmarks::select`; `obc-data-plumbing` cannot run this fetch. One run captures the
  three sources, and each record takes the files of its licence: `wikipedia` takes `articles/`
  and `links/wikipedia-*`, `commons` takes `images/` and `categories/`, and `wikidata` takes the
  other files. Every record takes `recipe.json`, which links the three. No record takes the
  copies of the inputs, which are OSM data or the policy (`boundary.geojson`, `candidates.json`,
  `summits.json`, `policy.json`), or `attempts/`. The program exits with status 2 when the
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
| `client` | `"none"`, `"all"` or `{"paths": [<output>]}`. Selected paths name declared outputs: a file, or a directory and its files. Paths are sorted and unique. An empty selection or a path outside `outputs` fails the plan |
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
| `layer_files` | `{layer name: [{path, size, sha256}, …]}`, from receipts for exactly the same selected files |
| `options` | The options |
| `output` | An empty directory. The layer is the files that the step writes in it |
| `metrics` | A path. The step can write a JSON object there, for example the size of each section |

A command starts in the repository root. Its standard output and standard error go to the
standard error of the engine. Exit status 0 is success. The objects are read-only. While a step
runs, `output` and `metrics` are in `partial/layer-<key>/`; the engine removes that directory
when the step ends, also when it fails. A failed step writes no receipt. A declared directory may be empty when the step creates it.
A declared output that does not exist is an error.

### Offline

A step reads only its inputs: the snapshots, the layers and the options in its request. A step
does not use the network; fetchers are the only network users. A step must not write to its
inputs: their paths, and the links of a view, are objects of the store, and a step that runs as
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
fetch. `--only GROUP,…` selects groups by their `id`, and `--only none` selects no group. An `id`
names a group only in the plan that it comes from. A plan of `live` has one group per cause instead: see
[Changes of live](#changes-of-live).

| Key | Value |
| --- | --- |
| `id` | The step of the first build of the group, in dependency order |
| `cause` | `null`. A plan of `live` gives the cause |
| `layers`, `drops` | `[]`. A plan of `live` gives the layers that the cause changes, as `builds` gives them |
| `fetches` | One per version and `params`: `source`, `version`, `params` (`[[NAME, VALUE], …]`), `files` and `bytes`. `files` are the names of the files that the store lacks. `[]` means that the store cannot name them, and the fetch gets every file that it gives |
| `builds` | In dependency order: `step`, `recipe` (see [Keys](#keys)), `key` (`null` while the step waits for a fetch or another build) and `estimate` |

The estimate of a build is `wall_ms`, `bytes_out` and `peak_rss_bytes` of the newest receipt, by
`built`, of the same step with the same options. Without one, it is the newest receipt of the
step, or `null` when the store has none. The `bytes` of a fetch is the size of its files in the
record of the version. When that record does not list them all, it is the size in the record of
another version that lists them all, the last in byte order. Without `files`, the files are those
that a fetch with the same `params` gave, or else every file. Otherwise `bytes` is `null`. A run
fetches a version with the same `params` once, with the files of every group that needs it.

Ordinary `plan` and `status` may fetch Geofabrik polygons, the Geofabrik index and the GLO-30 tile
list. They MUST NOT prepare bulk inputs. A missing bulk prerequisite gives a blocked error.
Automatically selected stale moves do not authorize bulk preparation. An explicit `--move`
permits preparation during plan discovery; a build uses the normal fetcher.

### Changes of live

A plan of `live` compares the steps with the layers of the live releases (see [Live](#live)). A
layer changes when it differs from its live layer as [State of a layer](#state-of-a-layer) says,
or when live does not have it. The plan has one group per cause. A layer can have more causes:

| `cause.kind` | `id` | The layers that it changes |
| --- | --- | --- |
| `region` | `region` | `region` of the environment is not the `region` of the live release of a product, or the product has nothing live. Each layer of the product that live does not have, or whose options or snapshot reads differ |
| `layers` | `layers` | `layers` of the environment are not the `optional` layers of the live release of a product. Each layer of the product that live does not have |
| `move` | `move:SOURCE` | The plan moves the source (see [Versions](#versions)). Each layer that reads it at another version than its live layer. `from` is each version that live reads, and `to` is the version of the plan |
| `code` | `code:LAYER` | The code that the steps declare (`paths` and `crates`) is not the code of their live layers: other inputs, command, outputs, `client` or code hash. Without an edit of the region, also other options or snapshot reads. `LAYER` is the first layer in dependency order. A live layer that no step makes, without an edit, has empty code |
| `repair` | `repair` | None. `keys` are the keys of live that R2 lacks or holds with another size, as `status --check` finds them |

A layer that reads a changed layer has its causes too. `layers` of a group are the layers that its
cause changes, in dependency order, each with its `recipe` and `key` as in `builds`. `drops` are
the live layers of a product that no step makes now. Their cause is the edit of the product, or
else `code`. The `fetches` and `builds` of a group make its `layers` and the layers that they
read, when the store lacks them. Those of `repair` make the unchanged layers of its keys that the
store lacks. Groups come in the order of the table. Two groups can need the same build.

A plan of `live` takes every group: git holds what live contains, so an edit or a change of the
code cannot stay out. `--only move:SOURCE,…` selects the moves: a move that it does not select
does not move, and the steps read the version of live. `--only none` selects no move. An explicit
`--move` that `--only` leaves out gives a warning. Another `id` in `--only` is a usage error.

The release of a product after a plan is its live release with the `layers` of the groups in place
of its live layers, without the `drops`, and with the `region` and the `optional` layers of the
environment (`Release::compose`). A product that no group or edit changes keeps its release.

### Device-map publication

The maps catalog step consumes selected layer paths and their receipt metadata. Map and terrain
layers keep empty-coverage records and decoded reference credits in `metadata/`. Only `cells/`
and `terrain/` payloads are client files. The catalog layer publishes pinned satellites from
`objects/`. Named schema, terrain, licence and region files remain release metadata.

The catalog selects only Geofabrik picks whose complete band and terrain coverage is available.
A box or multi-area maps source is blocked until source coverage preparation supports it.
Before apply, the product compares picks with the previous release and assembles changed picks
with the real assembler. The production reader verifies each result. Missing inputs and invalid
artifacts fail verification before upload. An unchanged pick can reuse prior verification.

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

### Versions

A step list gets the version of each fetch that it reads, a source with its `NAME=VALUE`s, from
one function (`obc_data::product::version`), in this order:

1. `--move SOURCE@VERSION` of the plan or the build. `--move SOURCE` without a version is the
   newest version upstream. A `--move` of a source that no step list reads is refused.
2. For the environment `live`, the version that the live releases read for the fetch. A fetch
   without `NAME=VALUE`s of a source that live reads only with them, such as the GLO-30 tiles that
   one version names, reads the version of all of them. A fetch with `NAME=VALUE`s that live does
   not read, such as the extract of a new region, goes on to step 3. When the live layers read one
   fetch at two versions, no order of versions chooses: a step list that reads it without a
   `--move` fails the command with `blocked` and the fix `Plan with --move SOURCE@VERSION`, and
   `status` shows the source as blocked. Another environment has no live release.
3. The newest version of the fetch in the store.
4. The newest version upstream: the product names the fetch, and `plan` or `build` fetches it. A
   source whose URL needs a `NAME=VALUE`, such as the GLO-30 tiles, has no one newest version: a
   fetch of it without that value fails with `usage` and the fix `Plan with --move
   SOURCE@VERSION`.

A plan or a build of `live` without `--plan` also moves each stale source that live reads (see
[State of a source](#state-of-a-source)) to the newest upstream version of the check of the last
hour, as `--move SOURCE@VERSION` does. Each source is one group, `move:SOURCE`. A stale source that
no step list reads, and a `manual` source, do not move this way.

A source with `refresh = "manual"` moves only with `--move`: step 4 does not fetch it, and the
command fails with `blocked` and the fix `Plan with --move SOURCE@VERSION`. Before the first apply,
nothing is live, so a plan takes the versions of the store and of upstream. The fetch of a
`--move SOURCE` names its version, and every product of the plan reads that version. Upstream can
stop serving an old version, such as a Geofabrik extract of an earlier day: when the fetch of a
version that live reads fails, the fix names `--move SOURCE`.

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

A product returns usable steps and blocked layers together. Each blocked layer has `layer`
and `reason`. A plan reports an incomplete product in `blocked`, with these rows in `layers`.
An empty `layers` list means that the whole product is blocked. Independent steps still plan
and build. A build with required blocked layers exits with `blocked` after usable work. It
writes no complete release for that product. An apply with these blockers fails before it
uploads, switches, or removes objects. A failed explicit capture fetch blocks its content and
artifacts; other map layers keep their steps. Status shows these layers as `blocked`, with the
reason in attention. Status fetches only small discovery files: Geofabrik outlines and the
GLO-30 tile list. An absent bulk input gives a not-prepared reason and unknown product layers;
opening the TUI starts no bulk download.

A product also gives what clients read of a release: its pointer document, and the files that a
client finds by name under `<prefix>/releases/<id>/`. A plan, a build and an apply of `live` leave
out a product without them: it is in `blocked` with the reason "no client document yet", and its
live release stays. Neither `maps` nor `planner` gives them yet. A product can check a release
before an apply makes it live; neither `maps` nor `planner` has a check yet.

Each step selects the files that clients read: the device, the web planner or a service on the
VPS. Other files are intermediate: only other layers read them. R2 holds only selected files
(see [Releases](#releases)). The selection changes the recipe and the release, but not the layer
key: a change in selection reuses the same bytes.

`plan ENV` plans the steps of every product together. `--json` writes the plan with `env`,
`region` and `layers` of the environment, `moves`, the version of each source that the plan moves
(a `--move SOURCE` has the version that its fetch gave), `versions`, the version of each fetch that
the step lists read, and `only`, the groups that `--only` selected, `[]` for every group or
`["none"]` for no group. A plan
of `live` also has:

- `live`: per product, the id of its live release, or `null` when nothing is live.
- `edits`: per product, `region` with `from` (the region of the live release, or `null` when
  nothing is live) and `to`, and `layers` with the optional layers that the environment switches
  `on` and `off`.
- `remove`: the keys, with `bytes`, that an apply of the plan removes from R2: each key of the
  listing, or without a listing each key that live uses, that the releases after the plan do not
  use. The listing has the prefixes that live owns after the apply, and `reference/v1` once live
  reads a `dtm-*` source. Those are the keys of their layers and input copies, the pointers and
  the files under `<prefix>/releases/<id>/`. A layer that the store lacks counts with all the
  objects of its live layer, because its new objects are not known yet; an apply keeps an object
  that a new release uses.
- `listed`: whether the plan listed R2. A listing needs the bucket. Without it, the plan has no
  `repair` group, `remove` lacks the leftovers and the files of `<prefix>/releases/<id>/`, and
  `bytes` is `null` for a record.

Another environment has `[]` for `live`, `edits` and `remove`, and `false` for `listed`.

`build ENV --plan FILE` builds the groups of that file; it takes no `--only` and no `--move`. Its
step lists read the `versions` of the file and no other version, and it moves the sources of
`moves`. A version that the store lacks is fetched; when that
fetch fails, the command fails with its code, and a fix that says to plan again when the fetch
gives none. It refuses the file, with exit status 3, before it builds:

- when `env`, `region`, `layers`, `blocked`, `live` or `edits` differ from the environment, its
  products and live now;
- when a step list reads a fetch that `versions` does not name;
- when the groups that `only` selects in the plan of now differ from the groups of the file,
  apart from `estimate` and `bytes`. Against live, the groups must change the same layers, by
  recipe, with the same causes and drops, whatever the store has: a second build of one plan does
  the same work.

When the store has the layer of every step of a product after the run, `build` writes the
release of that product. A build of `live` writes the release of each product that its groups or
edits change, as [Changes of live](#changes-of-live) composes it.

#### `maps`

The device maps have layers per leaf: a cell of size `2^23` µdeg of the OBCA grid that the
outline of the region touches. The outline of a `box` region is its box; the outline of a
`geofabrik` region is its `.poly` from `geofabrik-poly`, `area=<region id>`. The product has no
steps for a `polygon` region yet. Only a `geofabrik` region has map cells, landmarks and peaks:
they read the OSM of its area. Another region has terrain only.

The steps read `copernicus-glo-30` and the national terrain models (`dtm-*`), and the step list
reads files such as a `.poly` and the GLO-30 tile list, each at its version (see
[Versions](#versions)). `copernicus-glo-30`, its tile list and the `dtm-*` sources are `manual`.

A leaf reads each `dtm-*` source whose `extent` meets the box of its terrain cells: the windows
that the crest rule of `OBCT_Spec.md` §9 reads, which have a halo of two postings. The fetch of a
model is `bbox=<that box>`. A fetch without files adds nothing, and a leaf where no model has
data has no `maps/reference` layer. While the store lacks the fetch of a model whose `credential`
is not on this machine, its reference and terrain layers are blocked. Bands that read that
terrain are blocked too. Other leaves and bands keep their steps.

| Layer | Reads | Options | Files |
| --- | --- | --- | --- |
| `maps/osm` | `geofabrik-extracts`, `area=<region id>` | `leaves`: `[i, j]` of each leaf | `osm/<i>-<j>.osm.pbf`: the `osmium extract --strategy smart --set-bounds` of the square of the leaf and one µdeg around it. The key holds no Osmium version: another Osmium can give other bytes. The metrics name the version (`osmium`) |
| `maps/<band>/<i>-<j>` | `maps/osm`, the file of the leaf; `land-polygons`; `maps/terrain/<i>-<j>` when the cells of the band read heights: contours in their levels, or a nav graph or POIs | `band`: `coarse`, `mid`, `fine` or `network` of the recommended band table (`OBCA_Spec.md`); `leaf`: `[23, i, j]`; `cells`: `[ci, cj]` of each cell of the band in the leaf that the outline touches | `cells/<band>/<ci>/<cj>.obcm` for each cell with content; `cells/<band>/empty.json`: the ids of the other cells. A cell has the bytes that one cut of the whole leaf with all bands writes, with `builder/presets/schema.json` and without landmarks or peaks |
| `maps/reference/<i>-<j>` | Each `dtm-*` source of the leaf with data, `bbox=<box>` | `models`: `source`, `version` and `credit` (its `attribution`) of each model; `tiles`: the ids `<ti:04>/<tj:04>` of the archive tiles that the terrain cells of the leaf read | `reference/`: the reference archive (`host/obc-dem/reference/README.md`) of the models, which `ingest.py ingest` of each model writes into an empty archive, best first by `PRIORITY`, cut to `tiles`. The `fetched` day of a model is its version. A Python step with the group `terrain-reference` |
| `maps/terrain/<i>-<j>` | `copernicus-glo-30`, `tile=` of each tile that the square of a cell reaches and that `copernicus-glo-30-tiles` names. A square without a tile is sea. A leaf without a tile reads no snapshot. `maps/reference/<i>-<j>` when the leaf has one | `posting_log2` and `cell_log2` of OBCT v1; `cells`: `[ci, cj]` of each terrain cell in the leaf that the outline touches | `terrain/<ci>/<cj>.obcd` for each cell with a height (`OBCC_Spec.md` §13), the bytes that `obc-bake terrain --reference` writes from the same tiles and archive; `terrain/empty.json`: the ids of the cells without a height; `terrain/credits.json`, when a cell reads a national model: `key`, `product`, `attribution` and `licence` of each model that a cell reads, as the reference archive states them |
| `maps/landmark-content`, `maps/peak-content` | `wikidata`, `wikipedia` and `commons`, `collection=landmarks` or `collection=peaks`, `area=<region id>`, `osm=`, `poly=` and `code=`; the file of `geofabrik-extracts` and of `geofabrik-poly` that `osm=` and `poly=` name, by its name and without params | None | `landmarks/content.json` or `peaks/peaks.json`, and the photos: the compile of the capture. The step makes the boundary, and the candidates or the summits, again from the `.poly` and the extract. When they differ from those that the recipe of the capture pinned, the code that makes them changed: the step fails, and the fix is `--move wikidata` |
| `maps/landmarks/<i>-<j>`, `maps/peaks/<i>-<j>` | `maps/landmark-content` and `maps/osm`, the file of the leaf; or `maps/peak-content` | `cell_log2`: 18; `cells`: `[ci, cj]` of each network cell of the leaf, as for `maps/network/<i>-<j>` | `landmarks/<ci>/<cj>.bin` or `peaks/<ci>/<cj>.bin` for each cell that owns content (`OBCC_Spec.md` §14.3). A landmark joins the OSM objects of the leaf that name it |

`<i>`, `<j>`, `<ci>` and `<cj>` have four digits or more, as in a cell id.

A capture keeps its params while no source of the capture moves: the step list reads the capture
of the region that the saved plan or live reads, or else the newest capture of the region in the
store. A new extract alone therefore asks for no new capture. A missing capture, missing capture inputs, stale capture code, or an automatic stale-source
move blocks only its content and artifacts. The reason asks for `--move wikidata`. Status and
plans do not start bulk captures without an explicit move. A capture that moves explicitly
reads the extract and the `.poly` of now. `code=` is the digest of the code that makes the boundary and the
candidates or the summits, so `--move wikidata` after a change of that code asks for a new capture.

`maps/osm`, `maps/reference/<i>-<j>`, `maps/landmark-content` and `maps/peak-content` are
intermediate layers; the other layers are client layers.

#### `planner`

The planner has steps for a `geofabrik` region that names its `countries` and its `time_zone`:
its OSM is the extract of that one area. The bounds of the region are the box around its `.poly`.
Each snapshot, such as `copernicus-glo-30`, the extract, the `.poly` and the GLO-30 tile list, is
at its version (see [Versions](#versions)), as for `maps`. The other options come from
[`data/planner.toml`](#dataplannertoml). A GLO-30 input reads the tile
of each 1° square that its box touches and that `copernicus-glo-30-tiles` names; a box at sea
reads no snapshot. No layer reads a national terrain model yet.

| Layer | Reads | Options | Files |
| --- | --- | --- | --- |
| `planner/osm` | `geofabrik-extracts`, `area=<region id>` | `path`: `osm.pbf` | `osm.pbf`: the extract as it is. The engine step `pass` writes it, so its code is no file |
| `planner/basemap` | `planner/osm`; The selected jar of `protomaps-basemaps`; `natural-earth`, `water-polygons`, `land-polygons`, `daylight-landcover`, `qrank`, `pgf-encoding` | `bounds` of the region; `attribution` of `osm-planet`, `natural-earth` and `daylight-landcover` | `basemap.pmtiles`: the Protomaps map at zooms 0 to 14 |
| `planner/terrain` | The GLO-30 tiles of `bounds` | `bounds`: west, south, east and north of the zoom 10 tiles that the bounds of the region touch and of their neighbours, widened to `terrain.margin_m` around the bounds | `terrain.mbtiles`: lossless Terrarium WebP tiles of zooms 0 to 12, the bytes that `planner-dem` writes from the same tiles |
| `planner/routing` | `planner/osm`, and the GLO-30 tiles of the bounds of the region | `region` (the last part of the region id), `bounds`, `profiles` and `countries`. The import applies the German access defaults | `routing/`: the package of [the route package contract](route-package.md) with `overlays.sqlite` and `route-catalog.json`; `blocks/`: the routing blocks of the grid cells, as `route-blocks` writes them; `routes/<cell>.json`: the records of `route-catalog.json` that name the cell, with a final newline |
| `planner/overlays` | `planner/routing` | `attribution` of `osm-planet` | `overlays.pmtiles`: the route networks and the access of `routing/overlays.sqlite`, as [the planner release](planner-release.md) defines it |
| `planner/assets` | `protomaps-assets`, `tangrams-icons` | None | `assets/fonts/` and `assets/sprites/` of the assets archive; `assets/sprites/LICENSE.txt`: the MIT notice of `tangrams-icons` |
| `planner/model` | `query-model` | None | `model/`: `model.int8.onnx`, `tokenizer.json` and `tokenizer_config.json` of the archive, and `labels.json`, the labels of the query schema |
| `planner/search/policy` | `nominatim-country-data` | None | `policy.json`: the country names, postcode formats and address levels of the archive; `country_osm_grid.sql.gz`: the country grid of the archive, as it is |
| `planner/search/dump` | `planner/osm`, `planner/search/policy` | `country`: the first of `countries` in lowercase, the country of a place outside each country polygon | `search.jsonl.zst`: the search records that `obc-search-bake` writes, one JSON object per line, with the keys of each object in byte order |
| `planner/search/records` | `planner/search/dump` | None | `pois.jsonl.zst` and `addresses.jsonl.zst`: the records of the dump with a POI and with an address |
| `planner/search/pois`, `planner/search/addresses` | `planner/search/records` | `component` (`pois` or `addresses`); `region`, `bounds` and `countries` as for `planner/routing`; `time_zone` of the region; `attribution` of `osm-planet` | `<component>/<region>.sqlite`: the search database of the component, schema 5. The key holds no SQLite version: another Python patch release can give other database bytes. The metrics name the version as `sqlite` |
| `planner/places` | `planner/search/pois` | None | `places.pmtiles`: the rider places of the POI database, at zoom 11 |
| `planner/climate` | `era5-land`, `bbox=<bounds>`, `first-year=<climate.first_year>` | `bounds`, `first_year`, and `attribution` of `era5-land`, whose `{year}` is the year after the ten years, `first_year` + 10 | `climate.pmtiles`: [the climate archive](planner-climate-tiles.md) |
| `planner/snow` | `hr-wsi` when its `extent` meets the bounds and its fetch has files, and `modis-snow`, each `bbox=<bounds>`, `seasons=<snow.seasons>`; `hansen-gfc`, `tile=` of each 10° tile that the bounds touch | `bounds`, `seasons`; `attribution`: the credits of `hr-wsi` when it reads it, `modis-snow` and `hansen-gfc`; `year`: the year of the `hr-wsi` version, which fills its `{year}`, or null | `snow.pmtiles`: [the snow archive](planner-snow-tiles.md): HR-WSI where it has data, and MODIS elsewhere |
| `planner/sun` | `planner/terrain` | `bounds`, `time_zone` of the region, `distance_m` (`terrain.margin_m`), `horizon_samples` and `horizon_directions` | `sun.pmtiles`: [the sun archive](planner-sun-tiles.md). `terrain_sha256` is the SHA-256 of the PMTiles archive that the step converts from `terrain.mbtiles` |

Each map producer has a `planner/<kind>/grid` step, where `<kind>` is `basemap`, `places`,
`terrain`, `overlays`, `climate`, `snow` or `sun`. It reads only the archive of its producer.
It converts terrain MBTiles with the pinned Python PMTiles library. Each tile goes into the
archive of its zoom 11 parent, or its own tile when its zoom is less than 11. The tile bytes
stay the same. Each archive is packed as `maps/tiles/<kind>/<z>-<x>-<y>.pmtiles`, and its TileJSON
as `maps/<kind>.json`, with the transport rules of [the offline contract](planner-offline.md).
The step writes the packed bytes in `objects/<transport sha256>` and selects only `objects`
for clients. Its local `index.json` has `format: 1`, `kind`, `map_zoom: 11`, `files`, `source`
and `metadata`. `files` maps logical names to source and transport hashes, sizes and encoding.
`source` names the converted archive bytes, or the raw MBTiles when it has no tiles. `metadata`
is the TileJSON. Empty terrain has TileJSON and no tile archives.

`climate`, `snow` and `sun` are optional layers: a step only when `layers` of the environment
names it. The search and the sun layer use the `time_zone` of the region. The map producers,
`planner/osm`, `planner/search/policy`, `planner/search/dump` and `planner/search/records` are
intermediate layers. Other layers select all their files for clients.

A Python step runs `env PYTHONHASHSEED=0 uv run --locked --offline --group <group> python
<entry> --step` in the repository root, with the packages of a dependency group of
`pyproject.toml`; `uv sync --all-groups` installs them on a machine. The fixed hash seed keeps
the order of a set out of the bytes. Its code is each Python file that it imports, each file
that it reads from the repository, `tools/step_request.py`, `.python-version`, `pyproject.toml`
and `uv.lock`. A credit that it writes comes in its options, so `data/sources.toml` is no code
of it.

A grid cell is a zoom 9 Web Mercator tile that the bounds of the region overlap, clipped to the
bounds, with the id `9-<x>-<y>`. The JSON objects that `planner/routing` writes have their keys in
byte order.

### Releases

`releases/<product>/<id>.json` is the manifest of a release: `{"product", "region", "optional",
"layers"}`, as the compact output of `serde_json` with the keys of each object in byte order.
`region` is the region of the environment that it was built for, and `optional` the optional layers
of the product that the environment switched on, sorted. The id is the SHA-256 of these bytes. A manifest holds no time or cost of a build, so two machines that build
the same layers make the same release. `layers` is sorted by `step`, and each layer has:

| Key | Meaning |
| --- | --- |
| `step`, `key`, `inputs`, `options`, `code`, `command`, `outputs`, `digest`, `files` | As in the [receipt](#receipt) |
| `snapshots` | `{source: {"version", "params"}}`: the version and the sorted `NAME=VALUE` of each snapshot that the layer read |
| `client` | The selected client outputs, as in the step (see [Products](#products)) |

The objects of a release are the selected client files, with one object per distinct SHA-256.
The manifest records every file of every layer, so a plan compares it with live and live names
the versions that it read. Uploads, live ownership and cleanup use the same selection.

### State of a layer

The engine computes the state of each layer when it is asked, and stores nothing. It compares
the step with the layer that live has: a layer of a live release manifest. The state of a source
is as in [State of a source](#state-of-a-source).

| State | When | Reason |
| --- | --- | --- |
| `not applied` | Live has no layer of the step | `missing in live` |
| `not applied` | The options are not the options of the live layer | `options` |
| `not applied` | The step reads a source that the live layer read, with another version or other `params` (in any order), or the store has the files that it reads and their digest is not the one that the live layer read | `SOURCE@VERSION not in live`, and ` (not fetched)` when the store does not have the files |
| `code changed` | The inputs (kind and name), the command, the outputs or whether a client reads the layer are not those of the live layer | `inputs`, `command`, `outputs` or `client` |
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
| `<prefix>/catalog.json` | The pointer: the document that clients read, with `"release": "<id>"` and `"applied"`, the time of the switch. A pointer without `release` names no release: nothing is live |
| `<prefix>/releases/<id>.json` | The manifest of the release, as in the store. Immutable |
| `<prefix>/releases/<id>/<path>` | A file of the release that a client finds by name. Immutable |
| `<prefix>/objects/<sha256>` | A file of a client layer of a release. Immutable |
| `inputs/records/<source>/<version>.json` | The snapshot record of an input copy: a version of a source with `r2_copy` that a live layer read. A record that R2 holds of a version that a live layer read counts, also without `r2_copy` |
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

### Apply

`apply live` makes the plan of live live. It needs the bucket, and it changes R2 in this order:

1. It refuses when `data/` has changes that are not committed, apart from
   `data/env/local.toml`: live builds from a committed `data/`. The steps run the code of the
   working tree, also code that is not committed. It does not commit or push. One apply of live
   runs at a time on a machine.
2. It asks once in a terminal: "Apply M changes to live? removes X GB from R2", with the groups
   and `remove` of the plan. `--yes` does not ask. `--plan FILE` applies that plan; the plan must
   be the plan of now, as for `build --plan`. Without a terminal, `--yes` or `--plan` is the
   consent. When live has every change and nothing is to be removed, it applies nothing.
3. It builds the plan, as `build live --plan` does.
4. It checks each release that changes: the check of its product, and its pointer. Each file
   that it uploads must have its SHA-256 in the store. A failed check changes nothing on R2.
5. It uploads each key of the releases after the apply and of their input copies that R2 lacks,
   or holds with another size; a key with another size goes first. Then it checks each key.
6. It writes the pointer of each product whose release changes: the document of the product with
   `"release": "<id>"` and `"applied"`, the time of the switch (`YYYY-MM-DDTHH:MM:SSZ`), and
   `Cache-Control: public, max-age=60, must-revalidate`.
7. It reads live again and lists its prefixes, and `reference/v1` once live reads a `dtm-*`
   source. With drift, it removes nothing. The leftovers are the keys that no live release uses
   and that R2 had 5 minutes before the apply started: a key that another apply uploads and has
   not switched to yet stays.
8. A client that read an old pointer finishes its downloads first. So while there are leftovers,
   the apply waits until 12 minutes after the time of the newest pointer on R2 (10 minutes, and 2
   for a clock that differs), and 10 minutes after its own switch. When no pointer time reads, it
   waits 10 minutes. Then it reads and lists again, as in step 7, and removes the leftovers of
   that read only, with a line in `removed.jsonl`: another apply can switch during the wait. An
   apply that stopped in the wait waits again.

An apply removes the leftovers of step 7, not the `remove` list of the plan, which is an estimate.
A record on R2 of a version that live reads stays, also when `r2_copy` of its source is off now.

An apply that stops before step 6 leaves live as it was, and the same plan applies again: it
uploads only what R2 still lacks. The objects, manifests and named files are immutable, with
`Cache-Control: public, max-age=31536000, immutable`.

## Commands

| Command | Output |
| --- | --- |
| `obc data [--json]` | In a terminal, and without `--json`: the TUI. Otherwise the output of `status` |
| `obc data status [--check] [--json]` | Where live was read; per product, the live release (or nothing live), `applied` of its pointer, the size of its objects, the optional layers that `layer` switches, and the state of each layer of the environment `live`; what needs attention: stale and blocked sources, old cache directories that `clean` imports, and with `--check` drift and leftovers. When a fetch that the step list of a product needs fails, the layer states of that product are unknown (`layers` is `null`), and attention gives the error. `--check` adds the listing of [Live](#live) and exits with 1 when it finds drift or leftovers. Without the bucket, `--check` exits with 4 before it reads anything |
| `obc data sources [--check-now] [--json]` | Every source with licence, R2 copy, live versions (`—` when live does not read the source; `?` with one warning when R2 cannot be read, and then `live` is `null` and `live_unknown` is `true` in the JSON), newest upstream version, age, policy, state and the versions in the local store. Rows are in kind order: data, then assets, then tools. An upstream check of the last hour serves, except with `--check-now` |
| `obc data fetch SOURCE[@VERSION] [NAME=VALUE…] [--json]` | Fetches the version, or else the newest file upstream. Writes the store path of each file |
| `obc data policy SOURCE 7\|30\|90\|365\|manual [--json]` | Writes `refresh` of the source in `data/sources.toml`. The edit keeps comments and the other lines. A policy in days for a source without `version = "date"` is refused. Writes the source |
| `obc data region ENV ID [--json]` | Writes `region` of `data/env/ENV.toml`. Writes the environment |
| `obc data layer ENV NAME on\|off [--json]` | Adds the optional layer to `layers` of `data/env/ENV.toml`, or removes it. A layer that no product has is refused. Writes the environment |
| `obc data undo ENV [--json]` | Writes `data/env/ENV.toml` as git has it in `HEAD`: the edits that are not applied go. Writes the environment |
| `obc data clean [--apply [--yes]] [--json]` | The plan of [Clean](#clean): the snapshot records and the objects that nothing reaches, what stays and why, and the old cache directories with their files and sizes. `--apply` asks, then cleans. With `--json` and `--apply`, the plan goes to standard error, and the output is what it did |
| `obc data region [list] [--json]` | Every region with its name and definition |
| `obc data region show ID [--json]` | One region, the regions it resolves to, and its box when every part is a box |
| `obc data plan ENV [--only GROUP,…] [--move SOURCE[@VERSION]]… [--json]` | What a build of the environment fetches and builds, in groups, with estimates. It fetches what a step list depends on, see [Products](#products). `--move` is in [Versions](#versions). For `live`: the groups of [Changes of live](#changes-of-live), the edits, and what an apply removes from R2 |
| `obc data build ENV [--plan FILE \| [--only GROUP,…] [--move SOURCE[@VERSION]]…] [--json]` | Fetches and builds the groups into the store, and writes the release of each product whose every layer is built; for `live`, of each product that the groups or edits change. It uploads nothing |
| `obc data apply live [--plan FILE] [--yes] [--json]` | Builds the plan of live, uploads what R2 lacks, switches the pointers and removes what no live release uses, as [Apply](#apply) says. Writes what it uploaded, switched and removed |
| `obc data runs [--json]` | Every run in the store, newest first: id, command, outcome, time, and the size of its fetches and of the layers that it built |
| `obc data runs RUN [--json]` | One run, its fetches, and its steps: time, change since the last run that built the step, peak RAM, output, inputs, code hash and users |
| `obc data runs RUN --follow [--json]` | The events of the run, and each new event until the run ends |

`--json` writes one JSON document to standard output. [JSON schemas](#json-schemas) has the
schema of each output, and [Errors](#errors) has the error codes and the exit statuses. In
addition:

- `fetch` lists only the requested files.
- `region ENV ID`, `layer` and `undo` write `data/env/ENV.toml` and nothing else, and they never
  commit. `region` and `layer` keep its comments and its other lines, write `region` and `layers`
  on one line each, and end each line with `\r\n` when the file has one, or else `\n`. A refused
  edit changes nothing.
- `runs` lists a run file that cannot be read as `failed`, or as `running` while its lock is
  held.
- `runs RUN --follow` writes one event per line, as in `runs/<id>.jsonl`. When the run failed,
  the error is the last line.

## R2 client

The R2 client in `host/obc-data` reaches the bucket for `obc bake publish --target r2`,
`obc bake clean-r2`, `obc r2 rm`, `obc fixtures publish` and the firmware publish workflow.
rclone moves the bytes. The planner publish, deploy and finalize, and the reference archive
ingest, still use the remote of `tools/r2.py`. `obc data r2` is plumbing for scripts: those
commands call it. It does not change the state of a release. `obc bake publish --target r2`, `obc
bake clean-r2` and the planner publish, deploy and finalize refuse to run when the pointer of
their prefix has `release`, or is not JSON: after an apply, only an apply changes live. The
planner deploy reads the pointer from the bucket again just before it writes it. `obc data r2
put` and `delete` refuse a key under `cell-catalog/` or `planner/` once the pointer of that
prefix has `release`, and under `inputs/` once any pointer has.

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
| `verify_failed` | 5 | A release failed its check before an apply, or after an upload the object in the bucket is
not the file. | Upload the file again. |
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
| `fetch` | `Fetched` |
| `policy` | `Source` |
| `region`, `region list` | `RegionList` |
| `region show` | `RegionDetail` |
| `region ENV ID`, `layer`, `undo` | `Edited` |
| `status`, and `obc data` without a terminal | `Status` |
| `clean`, `clean --apply` | `CleanPlan` |
| `plan` | `EnvPlan` |
| `build` | `Built` |
| `apply` | `Applied` |
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
    "Applied": {
      "description": "What an apply did.",
      "properties": {
        "built": {
          "anyOf": [
            {
              "$ref": "#/$defs/Built"
            },
            {
              "type": "null"
            }
          ],
          "description": "The build of the plan; `null` when live had every change."
        },
        "removed": {
          "description": "The keys that it removed: no live release used them.",
          "items": {
            "$ref": "#/$defs/Object"
          },
          "type": "array"
        },
        "switched": {
          "description": "The release of each product whose pointer it switched.",
          "items": {
            "$ref": "#/$defs/BuiltRelease"
          },
          "type": "array"
        },
        "uploaded": {
          "description": "The keys that it uploaded.",
          "items": {
            "type": "string"
          },
          "type": "array"
        }
      },
      "required": [
        "built",
        "uploaded",
        "switched",
        "removed"
      ],
      "type": "object"
    },
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
    "BlockedLayer": {
      "additionalProperties": false,
      "properties": {
        "layer": {
          "type": "string"
        },
        "reason": {
          "type": "string"
        }
      },
      "required": [
        "layer",
        "reason"
      ],
      "type": "object"
    },
    "BlockedProduct": {
      "additionalProperties": false,
      "properties": {
        "layers": {
          "items": {
            "$ref": "#/$defs/BlockedLayer"
          },
          "type": "array"
        },
        "product": {
          "type": "string"
        },
        "reason": {
          "type": "string"
        }
      },
      "required": [
        "product",
        "reason",
        "layers"
      ],
      "type": "object"
    },
    "Built": {
      "description": "What a build did.",
      "properties": {
        "blocked": {
          "description": "Incomplete products. Usable layers can build, but these products get no complete release.",
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
          "description": "The release of each product whose every layer is built. For `live`, the release of each\nproduct that the groups change: its live layers with the layers of the groups.",
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
          "description": "A release failed its check before an apply, or after an upload the object in the bucket is\nnot the file.",
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
    "Edit": {
      "description": "What the environment file changes against the live release of a product.",
      "oneOf": [
        {
          "additionalProperties": false,
          "description": "`from` is `None` when the product has nothing live.",
          "properties": {
            "from": {
              "type": [
                "string",
                "null"
              ]
            },
            "kind": {
              "const": "region",
              "type": "string"
            },
            "product": {
              "type": "string"
            },
            "to": {
              "type": "string"
            }
          },
          "required": [
            "kind",
            "product",
            "from",
            "to"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "description": "The optional layers that the environment switches on and off.",
          "properties": {
            "kind": {
              "const": "layers",
              "type": "string"
            },
            "off": {
              "items": {
                "type": "string"
              },
              "type": "array"
            },
            "on": {
              "items": {
                "type": "string"
              },
              "type": "array"
            },
            "product": {
              "type": "string"
            }
          },
          "required": [
            "kind",
            "product",
            "on",
            "off"
          ],
          "type": "object"
        }
      ]
    },
    "Edited": {
      "description": "An environment file after an edit.",
      "properties": {
        "env": {
          "type": "string"
        },
        "layers": {
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
        "layers"
      ],
      "type": "object"
    },
    "EnvPlan": {
      "additionalProperties": false,
      "description": "What a build of an environment would fetch and build.",
      "properties": {
        "blocked": {
          "description": "Incomplete products, with unavailable layers, or an empty layer list when the whole product is blocked.",
          "items": {
            "$ref": "#/$defs/BlockedProduct"
          },
          "type": "array"
        },
        "edits": {
          "description": "For `live`: what the environment file changes against the live releases.",
          "items": {
            "$ref": "#/$defs/Edit"
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
        "listed": {
          "description": "For `live`: whether the plan listed R2, which needs the bucket. Without a listing, the plan\nhas no `repair` group, and `remove` lacks the leftovers and the files that a client finds\nby name.",
          "type": "boolean"
        },
        "live": {
          "description": "For `live`: the release that each product has live now. Empty for another environment.",
          "items": {
            "$ref": "#/$defs/LiveRelease"
          },
          "type": "array"
        },
        "moves": {
          "additionalProperties": {
            "type": "string"
          },
          "description": "The version of each source that the plan moves: each `--move`, and for `live` each stale\nsource that the step lists read. A move without a version has the version that its fetch\ngave.",
          "type": "object"
        },
        "only": {
          "description": "The groups that `--only` selected: none for every group, `[\"none\"]` for no group.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "region": {
          "type": "string"
        },
        "remove": {
          "description": "For `live`: the keys that an apply of the plan removes from R2.",
          "items": {
            "$ref": "#/$defs/Removal"
          },
          "type": "array"
        },
        "versions": {
          "description": "The version of each fetch that the step lists read. `build --plan` reads exactly these.",
          "items": {
            "$ref": "#/$defs/FetchVersion"
          },
          "type": "array"
        }
      },
      "required": [
        "env",
        "region",
        "layers",
        "moves",
        "versions",
        "live",
        "edits",
        "only",
        "groups",
        "blocked",
        "remove",
        "listed"
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
          "description": "Only for `osm`: the source whose version is the base day, `from=`.",
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
    "FetchVersion": {
      "additionalProperties": false,
      "properties": {
        "params": {
          "description": "The `NAME=VALUE`s of the fetch, sorted.",
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
        "params",
        "version"
      ],
      "type": "object"
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
          "description": "`live PRODUCT, …`, `newest of the source`, `newest of a request`, `inputs kept`,\n`live release`, `fixture`, `planner recipe` or `import record`.",
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
    "LiveRelease": {
      "additionalProperties": false,
      "properties": {
        "product": {
          "type": "string"
        },
        "release": {
          "description": "`None` when nothing is live.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "product",
        "release"
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
    "PlanCause": {
      "description": "Why a group changes live.",
      "oneOf": [
        {
          "additionalProperties": false,
          "description": "The environment names another region than a live release, or a product has nothing live.",
          "properties": {
            "kind": {
              "const": "region",
              "type": "string"
            }
          },
          "required": [
            "kind"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "description": "The environment switches on other optional layers than a live release has.",
          "properties": {
            "kind": {
              "const": "layers",
              "type": "string"
            }
          },
          "required": [
            "kind"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "description": "A `--move`, or a stale source: the layers read `to`, not the versions that live reads.",
          "properties": {
            "from": {
              "items": {
                "type": "string"
              },
              "type": "array"
            },
            "kind": {
              "const": "move",
              "type": "string"
            },
            "source": {
              "type": "string"
            },
            "to": {
              "type": "string"
            }
          },
          "required": [
            "kind",
            "source",
            "from",
            "to"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "description": "The code that the steps declare is not the code of their live layers, or it gives other\noptions or inputs. A layer that live has and the steps do not make has no code.",
          "properties": {
            "crates": {
              "items": {
                "type": "string"
              },
              "type": "array"
            },
            "kind": {
              "const": "code",
              "type": "string"
            },
            "paths": {
              "items": {
                "type": "string"
              },
              "type": "array"
            }
          },
          "required": [
            "kind",
            "paths",
            "crates"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "description": "The keys of live that R2 lacks, or holds with another size. Its builds make the unchanged\nlayers of those keys that the store lacks.",
          "properties": {
            "keys": {
              "items": {
                "type": "string"
              },
              "type": "array"
            },
            "kind": {
              "const": "repair",
              "type": "string"
            }
          },
          "required": [
            "kind",
            "keys"
          ],
          "type": "object"
        }
      ]
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
      "description": "One change, and the fetches and builds that it needs. Without live, a group is builds that read\neach other's layers: it never needs a build of another group, so each can be selected alone.\nAgainst live, a group is one cause, and two groups can need the same build. Two groups can need\nthe same fetch.",
      "properties": {
        "builds": {
          "description": "In dependency order.",
          "items": {
            "$ref": "#/$defs/PlanBuild"
          },
          "type": "array"
        },
        "cause": {
          "anyOf": [
            {
              "$ref": "#/$defs/PlanCause"
            },
            {
              "type": "null"
            }
          ],
          "description": "Why live changes; `None` without live."
        },
        "drops": {
          "description": "The live layers that the release no longer has.",
          "items": {
            "type": "string"
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
          "description": "It names the group only in the plan that it comes from.",
          "type": "string"
        },
        "layers": {
          "description": "The layers that the cause changes, in dependency order, each with its recipe and key as in\n`builds`.",
          "items": {
            "$ref": "#/$defs/PlanBuild"
          },
          "type": "array"
        }
      },
      "required": [
        "id",
        "cause",
        "layers",
        "drops",
        "fetches",
        "builds"
      ],
      "type": "object"
    },
    "ProductStatus": {
      "properties": {
        "applied": {
          "description": "When an apply made the release live, `YYYY-MM-DDTHH:MM:SSZ`; `None` when nothing is live or\nthe pointer has no time.",
          "type": [
            "string",
            "null"
          ]
        },
        "bytes": {
          "description": "The size of the objects of the live release.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
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
        "optional": {
          "description": "The optional layers of the product, which `layer live NAME on|off` switches.",
          "items": {
            "type": "string"
          },
          "type": "array"
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
        "applied",
        "bytes",
        "optional",
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
      "description": "How old the live version may get, in days, before the source is stale; `manual` is never stale.",
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
    "Removal": {
      "additionalProperties": false,
      "properties": {
        "bytes": {
          "description": "`None` for the record of an input copy, whose size is not known before a listing.",
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
        "bytes"
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
        "extent": {
          "description": "The box outside which the source has no data: west, south, east and north in degrees.",
          "items": {
            "format": "double",
            "type": "number"
          },
          "maxItems": 4,
          "minItems": 4,
          "type": [
            "array",
            "null"
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
      "description": "A source of `data/sources.toml` with its live version, its snapshots and its state.",
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
        "extent": {
          "description": "The box outside which the source has no data: west, south, east and north in degrees.",
          "items": {
            "format": "double",
            "type": "number"
          },
          "maxItems": 4,
          "minItems": 4,
          "type": [
            "array",
            "null"
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
        "live": {
          "description": "The versions that the live releases read, in order; `null` when R2 could not be read.",
          "items": {
            "type": "string"
          },
          "type": [
            "array",
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
        "live",
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
        "live_unknown": {
          "description": "R2 could not be read: `live` of each source is `null`.",
          "type": "boolean"
        },
        "sources": {
          "items": {
            "$ref": "#/$defs/SourceRow"
          },
          "type": "array"
        }
      },
      "required": [
        "live_unknown",
        "sources"
      ],
      "type": "object"
    },
    "State": {
      "description": "The state of a source or a layer. A source is only ok, stale or blocked. When more than one\nstate applies to a layer, the first in this order is its state, so the least of several states\nis the one to show for all of them.",
      "enum": [
        "not_applied",
        "code_changed",
        "input_changed",
        "stale",
        "blocked",
        "ok"
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
      "description": "How upstream names a version, and so what a version of the source looks like.",
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
          "description": "`YYYY-MM-DD`: the only scheme that gives a version an age.",
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
