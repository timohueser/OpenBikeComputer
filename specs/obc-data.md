# obc data

`obc data` reads the data registry: the external sources that a bake uses, the regions, and
the targets. It fetches sources into the store, bakes layers and updates the release that a
target serves. The crate is `host/obc-data`.
All files under `data/` are TOML. A file with an unknown key is refused.

The shared acquisition boundary for landmarks and peaks is defined in
[Shared Wikimedia content](wikimedia-content.md). Regional capture paths below
describe the current integration until it consumes those shared source pins.

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
| `start` | string | no | The version that a step list reads while no serving release reads the source. It has the form of `version` |
| `refresh` | integer or string | no, `"manual"` | The policy, which the settings of a target can replace: `1` through `65535` whole days, or `"manual"` |
| `redistribute` | boolean | yes | The licence lets us give the upstream bytes to others. A target then copies the files of the source that its release reads, see [Target layout](#target-layout). The Wikimedia API captures `wikidata`, `wikipedia` and `commons` have no copy. The national terrain models `dtm-*` have no copy either: `obc data` reads them through a capture per archive tile |
| `credential` | table | no | `env`: the environment variables a fetch needs; or `file`: the file that holds them. `~/` is the home directory |
| `extent` | array of 4 numbers | no | The box outside which the source has no data: west, south, east and north in degrees, west < east, south < north. A box meets it when the two overlap; boxes that only touch do not meet |
| `areas` | array of strings | no | Geofabrik areas whose `geofabrik-poly` polygons hold the data inside `extent`. Needs `extent`. None means all of `extent` |

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
`builder/web/vite/third-party-licenses.ts`. The web planner, the map builder and the iOS planner show the
planner release `attribution`. Text that no step can generate keeps a copy, and a test
compares the copy with this file: the device About page, and the footers of the site, the
docs and the map builder.

### Settings of a target

The settings of a target are in the manifest of the release that it serves. `obc data update`
reads them, applies its flags, and keeps them in the new release.

| Key | Type | Meaning |
| --- | --- | --- |
| `region` | string | The id of the region; empty until an update names one |
| `definitions` | object | The TOML text of the region and of the members of a union, by id |
| `layers` | array of strings | The optional layers that are on, sorted, each once |
| `refresh` | object | Source id to `1` through `65535` whole days, or `"manual"`. This policy replaces the `refresh` of `data/sources.toml` for the target. Days need a date version |

`--region ID` copies the definition of a region of `data/regions/`. `--region PATH[,PATH…]`
defines a `geofabrik` region of these Geofabrik paths: its id is the path, or `areas` for more
paths. `--box W,S,E,N` defines a `box` region with the id `box`. Another flag keeps the copy, so a
change of `data/regions/` does not change a target. A change of `refresh` alone changes the
release, and bakes nothing.

### `data/regions/<id>.toml`

The presets are in `data/regions/`. One file holds one region. Its id is the relative file path
without `.toml`. Each part of the id is lowercase kebab-case.

| Key | Type | Meaning |
| --- | --- | --- |
| `name` | string | The name a person reads; not empty |
| `kind` | string | `geofabrik`, `box` or `union` |
| `box` | array of 4 numbers | Only for `box`: west, south, east, north in degrees, longitude first |
| `areas` | array of strings | Only for `geofabrik`: one or more source paths. Paths are sorted and duplicates removed |
| `union` | array of strings | Only for `union`: two or more region ids |
| `countries` | array of strings | The ISO 3166-1 alpha-2 codes of the countries in the region, such as `DE`. The first is the country of a place outside each country polygon |
| `time_zone` | string | The IANA time zone of the region, such as `Europe/Berlin` |

The planner reads `countries` and `time_zone` (see [`planner`](#planner)).

A box has longitude in −180…180 and latitude in −90…90, with west < east and south < north.
A box that crosses the antimeridian is refused. The order of the numbers is checked only
through these ranges: a latitude-first box is refused only when one of its longitudes is
outside −90…90.

A `geofabrik` region selects source paths such as `europe/germany/baden-wuerttemberg`.
Its id is independent of those paths. An empty selection is refused. A union resolves to the
regions in it that are not unions. A union that contains itself, or names a region that does not
exist, is refused.

`obc data areas [QUERY]` searches the regions of `data/regions/` and the names and paths of the
newest Geofabrik index in the store. It verifies the object of the index and fetches nothing.
Its output gives names, paths, countries and bounds, without polygons.

The bakes read this directory:

| Reader | Regions |
| --- | --- |
| `obc-bake` (device maps) | Single-area definitions whose saved id equals the source path. `--regions DIR` reads another directory with this layout; `obc bake` passes the checkout's directory |
| Planner bake | The `box` region with the id of the recipe in `tools/planner-regions/`: its `name` and its box |
| `obc data`, product `planner` | A region, its `countries` and its `time_zone` |
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

### `data/machines.toml`

One table per machine that runs `obc data ui`. The name of the table is the name of the machine:
`OBC_DATA_MACHINE`, or else the short host name. A machine that the file does not name has the
header `#262624`, and `obc data open` tunnels to the SSH host of its name.

| Key | Type | Meaning |
| --- | --- | --- |
| `colour` | string | `#rrggbb`: the header of the page of the machine. White text on it has a contrast of at least 4.5:1, and it is not `#6e6c66`, the header of a page that loads |
| `ssh` | string | Optional: the SSH host or alias that `obc data open` tunnels to |

## State of a source

A fetch is a source id with its sorted `NAME=VALUE`s. `obc data sources` gives, for each target
that serves a release, the policy of each source and the state of each fetch that the release
reads. The source has the least state of its fetches, in the order of this table. `obc data` and
`obc data update` judge the fetches that the step lists read now in the same way.

| State | When |
| --- | --- |
| `blocked` | A `data` or `asset` source has no `licence`, or a due fetch has no successful upstream result |
| `stale` | A date fetch is older than its `refresh`, and upstream names a later version or the fetch is a capture. Or its version is before the serving version of the source that `fetch.from` names |
| `unused` | The serving release reads no fetch of the source |
| `ok` | Otherwise. A fetch without a serving version is never stale. A `manual` source is never stale by age |

Data age is the number of days from the serving version to today (UTC). A fetch is due only when
its age is greater than `refresh`. An unchanged successful check does not change data age: the
fetch stays due, but its state is `ok` until a later check finds a newer version. Probe cache
age is separate from data age.

Each fetch gives its params, serving version, state, reason, age and upstream observation. The
observation gives the result, the latest probe time and the last successful probe time and
version. A failed probe keeps the last success. A capture result has no probe time: the capture
policy is not evidence of a network check. The source row gives missing credentials, but a
credential blocks only a new fetch.

## Store

The store is the directory in `OBC_DATA_STORE`, or else `~/.cache/openbikecomputer/store/`.

| Path | Holds |
| --- | --- |
| `layout` | The layout number of the store, `2`, and a line end |
| `objects/<ab>/<sha256>` | One file, named by the lowercase hex SHA-256 of its bytes; `<ab>` is its first two characters. Read-only |
| `snapshots/<source>/<version>.json` | The snapshot record of one source version |
| `layers/<key>.json` | The receipt of the layer with that key, see [Layers](#layers) |
| `requests/<source>/<sha256>.json` | The files that a fetch with `NAME=VALUE` gave: `version`, `params` and `files` (names). The name is the SHA-256 of the compact JSON `[version, params]`, with `params` sorted. A record with no files selects no file |
| `runs/<id>.jsonl` | The events of one run, see [Runs](#runs) |
| `refreshed/<source>.json` | `{source, from, to}`: the version that `obc data source refresh` took last (`to`), and the versions that the targets served then (`from`) |
| `upstream/<source>/<sha256>.json` | The acquisition check descriptors and observation of one normalized request. The name is the SHA-256 of compact JSON sorted params. The observation has `checked_at` (UTC seconds or `null`), `result` and `last_success` (UTC seconds and version, or `null`). The result has `state`: `newest` with `value`, `failed` with `value`, `capture`, or `cannot_check` |
| `partial/` | Downloads that are not complete, the validators that resume them, and the layers that steps write. A collection empties it |
| `serve/<target>/<planner id>/` | The files that the services of a planner release read while [`obc data serve`](#serve) runs them |
| `locks/` | One lock file per key and per run |

Rules:

- `obc data` refuses a store whose `layout` is missing or holds another number. An empty or new
  store directory gets `layout`. For a store without it or with `1`, the fix removes `layers/`,
  `releases/`, `runs/`, `operations/`, `settings/`, `regions/`, `code/`, `producers/` and
  `local/`, which hold records of an older format, and writes `layout`. A receipt of layout 1
  names the input kind `layer`: a clean cannot read it. Snapshot records and objects stay.
- An object is complete. A download goes to `partial/` and moves into `objects/` with one
  rename after the digest check.
- A record goes to a temporary file in its directory and replaces the old record with one
  rename.
- One process at a time downloads a URL, one process at a time writes a snapshot record, and one
  process at a time builds a layer key.
- A fetch and a run of the engine hold the store lock (`locks/store.lock`) shared. A collection
  holds it alone.

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

`obc data clean` shows one plan. With `--apply`, it asks once, as [Errors](#errors) says, and
then collects. The collection deletes only the plan that it showed: when the plan of now differs,
also in the size of `partial/`, it deletes nothing. A receipt that the command cannot read, such
as one that a newer `obc data` wrote, stops the collection. For reuse, such a receipt is absent and
its step builds again.

The collection deletes what no release of a target or fixture reaches. Its roots are the serving
release and the rollback of each target (see [Targets](#targets)), the files of the checkout that
it runs in, and the store:

- A snapshot record is reached when an output of a root release read its source and version. The
  newest record of each source, by the latest `retrieved` of its files, is also reached: a plan
  step list reads it when nothing else names a version (see [Versions](#versions)), and a source whose
  upstream gives only its newest file cannot give it again. So is the newest version of each request record (`requests/`), by the
  latest `retrieved` of its files, such as the extract of each Geofabrik area, and each version
  that `obc data source refresh` took (`refreshed/`).
- An object is reached when a reached snapshot record or a reached layer has it, or when its
  SHA-256 is a file of an output of a root release, or is in `fixtures/catalog.toml`, a JSON or TOML file below `fixtures/sources/` or a
  planner region recipe in `tools/planner-regions/`.
- A layer is reached when each of its inputs is reached: a snapshot input whose digest is the
  digest of the files that it names (`files`) of a reached record of its source, and a step input
  whose digest is the digest of the files that it selects of a reached layer of its step: the
  `files` of the input, or all files when it names none.

The plan lists what the collection deletes and what stays. What stays is one entry for each
reached snapshot record, with the reasons: `served by TARGET, …`, `newest of the source`,
`newest of a request` and `refreshed`. Then one entry for the reached layers of each step (`inputs kept`).
Then one entry for each kind of root that names objects that no reached record or layer has:
`release`, `fixture` or `planner recipe`. The size of
an entry is the size of its files, and the plan gives the size of `partial/`. The collection
takes the store lock alone, or refuses to start while a run or a fetch holds it. A run holds it
until it finishes. A collection deletes each snapshot record and each object that is not reached,
and empties `partial/`. Receipts and upstream checks stay. A clean that cannot read a target
deletes nothing.

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

A `dtm` or `capture` fetch runs in the explicit repository root. The offline system selector
of `uv python find` selects its Python and honors `UV_PYTHON`. A standard-library capture
runs that interpreter directly. A package capture uses `uv run --locked --offline
--no-default-groups --no-python-downloads --group GROUP python`, bound to that interpreter.
It prepares the locked group when the capture runs. Cached captures need no Python runtime.
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
| `dtm-*` | `tile=<ti:04>-<tj:04>` | `host/obc-dem/reference/ingest.py fetch` | group `terrain-reference` | `tile=<ti:04>-<tj:04>` |
| `modis-snow`, `hr-wsi` | `bbox`, `seasons=FIRST-LAST` | `tools/planner_snow.py --fetch` | group `planner-snow` | `bbox=W,S,E,N&seasons=FIRST-LAST` |
| `osm-trails` | `bbox` | `tools/planner_snow.py --fetch-trails` | group `planner-snow` | `bbox=W,S,E,N` |
| `era5-land` | `bbox`, `first-year` | `tools/planner_climate.py --fetch` | group `planner-climate` | `bbox=W,S,E,N&first-year=YEAR` |
| `wikidata`, `wikipedia`, `commons` | `content=<JSON exact identities>` | `tools/wikimedia_acquire.py` | none (`python3`) | SHA-256 of the identity request |

- `bbox` is `WEST,SOUTH,EAST,NORTH` in degrees.
- A `dtm-*` fetch is one archive tile `(ti, tj)` of the reference archive
  (`host/obc-dem/reference/README.md`). Its one file is `<ti:04>-<tj:04>.tif`: the rasters of the
  model in the tile, pooled onto the lattice as an archive tile. Where the model has no height in
  the tile, the file is the empty `<ti:04>-<tj:04>.none`. The raw rasters stay in the work
  directory, which the program removes; the store never holds them. A service is asked only for
  the part of the tile inside the `extent`. A request that a version of a `manual` source in the
  store lacks joins that version, so a model fetched tile by tile over days has one version; a
  version that the store does not have is refused, as for any capture. In one run, a request of
  a `manual` source without a version takes the version of the first fetch of that source. So one
  version can hold tiles of several upstream releases: its label is the day of the first tile,
  and the credits and `fetched` of the model name that day. A `by-hand` model takes the rasters of its delivery:
  `OBC_REFERENCE_<KEY>_INPUT` names the directory, as an absolute path, and
  `OBC_REFERENCE_<KEY>_DATUM` the vertical datum that the metadata of the order states. The two
  are the `credential` of the source.
- A snow file is the window of one source raster that covers `bbox`, one pixel wider on each
  side, in the grid of the source. A season starts on 1 September. An `hr-wsi` box without a
  product gives a fetch without files.
- The `osm-trails` file is `trails.json`: the JSON answer of Overpass, as it is, to the query of
  the ways with `highway=path` or `highway=track` that `bbox` touches.
- An `era5-land` file is a source chunk of the ten years from `first-year`, or the orography.
- `content` names entities, explicit links, articles, Commons files and bounded categories.
  A request has no region, OSM digest or transform hash. The Store owns compact fact records,
  their provenance manifests and local conversion inputs. Admission verifies each digest and
  size. A failed operation keeps successful work for resume. Complete work is removed after
  admission. Existing regional captures are read-only inputs for adoption.
- The version is the day of the fetch, because the service answers with current data. Another
  day comes only from the store.
- A source with a `credential` that is not on this machine fails before the program runs, and
  the error names the variables or the file.

Each acquisition check has 15 seconds. The store keeps its answer or failure for one hour.
Equal normalized requests share a check. A changed acquisition URL or check kind invalidates
the cached observation. Refresh policy changes do not invalidate acquisition evidence.

| Source | Check |
| --- | --- |
| `osm` | `GET` of `<fetch.url>state.txt`; the day of its `timestamp` |
| `http` or `geofabrik`, with `{yymmdd}` and known request params | `HEAD` of the URL with `latest` for `{yymmdd}`, not following the redirect; the day in the file name of its `Location` |
| `capture` | On-demand capture policy, with no network check or probe time |
| `github`, `commit` | The GitHub API: the newest commit of the default branch |
| `github`, `release` | The GitHub API: the tag of the newest release that has the asset of the URL |
| `http`, `geofabrik` or `glo30`, `date`, and a URL without a version placeholder, with known request params | `HEAD` of the URL; the `Last-Modified` day |
| Every other source | None; the source cannot be checked |

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
| `shape` | `tiled`: built per grid leaf. `whole`: built once over the full coverage. `whole` with named partitions |
| `format` | The number that the readers of its files check. Where a reader has a constant, the layer uses it: `obct::SURFACE_VERSION` for `device/terrain`, `obcm::VERSION` for `device/map`, the router `FORMAT` and `CATALOG_SCHEMA_VERSION`. `device/landmarks` has the sum of `obcm::landmarks::SECTION_VERSION` and `obcm::peaks::VERSION`: each only grows. Another layer has the format that its spec states, or 1 |
| `revision` | The number that a person changes in the change that improves the output. It starts at 1 |
| `readers` | The programs or layers that read its files. An internal layer has none |

| Layer | Steps |
| --- | --- |
| `osm` | `device/source/<area>`, `device/region-osm`, `device/osm`, `planner/source/<area>`, `planner/osm` |
| `device/terrain` | `device/reference/<i>-<j>`, `device/terrain/<i>-<j>` |
| `device/map` | `device/<band>/<i>-<j>` |
| `device/landmarks` | `device/landmark-content…`, `device/peak-content…`, `device/landmarks/<i>-<j>`, `device/peaks/<i>-<j>` |
| `device/catalog` | `device/catalog` |
| `planner/basemap` | `planner/basemap`, `planner/assets` and their grids |
| `planner/terrain`, `planner/places`, `planner/climate`, `planner/snow`, `planner/sun` | The step of the same name and its grid |
| `planner/routing` | `planner/routing`, `planner/overlays` and their grids |
| `planner/search` | `planner/search/…`, `planner/model` and their grids |
| `planner/index` | `planner/fonts/grid`, `planner/index`, `planner/catalog` |

A step declares:

| Field | Meaning |
| --- | --- |
| `layer` | The layer that it makes part of |
| `name` | Lowercase kebab-case segments joined by `/`. A step that two products list has the same declaration in both |
| `inputs` | Snapshots, as `source`, `version`, `params` and `files`. With `params` (the `NAME=VALUE` of the fetch), the step reads the files that a fetch with them gives, and `files` is empty. Without, `files` names the files that the step reads, or is empty for every file. The files of other steps, as `name` and `files`: the paths that the step reads, or none for every file. A selected path that the other step does not have fails the step |
| `options` | A JSON object |
| `outputs` | Paths in the output directory. A path is a file, or a directory whose files are all part of the layer. The step must write each path and no other file. A symbolic link fails the step |
| `client` | `"none"`, `"all"` or `{"paths": [<output>]}`. Selected paths name declared outputs: a file, or a directory and its files. Paths are sorted and unique. An empty selection or a path outside `outputs` fails the plan |
| `run` | A Rust function in the process, a command (a program and its arguments), or a Python command. No argument names a path outside the repository root: no argument is an absolute path, contains `=/` or has a `..` segment between `/` and `=` |

One binary links the Rust steps of every product, so Cargo unifies their features. A step crate
enables every feature its bytes depend on itself, or makes its bytes independent of it (structs,
or sorted keys for JSON objects).

A Python command runs `uv run`. The engine sets `UV_PYTHON` to the interpreter that `uv python
find --system --offline --no-python-downloads` selects for the project, with an explicit
`UV_PYTHON` request when set, and sets `UV_NO_SYNC=0`.

### Keys

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

A step input enters a key with its digest, not with its key. A rebuild that gives the same
files gives the same digest, so the keys of the steps that read it do not change, and the
engine reuses them. A snapshot version enters a key the same way, by the digest of its files.

The recipe of a step is the SHA-256 of the same object without digests and with `client`: each
input as `{"kind", "name", "files"}` for a step and `{"kind", "name", "version", "params",
"files"}` for a snapshot, `params` and `files` sorted, and the inputs sorted by their compact
JSON. It holds no digest, so a plan has it also for a step whose key waits for a fetch or
another build.

### The step contract

A step gets a request. A command reads it as JSON on standard input; a Rust function gets the
same fields.

| Key | Value |
| --- | --- |
| `step` | The name of the step |
| `snapshots` | `{source: {file name: object path}}` |
| `layers` | `{step name: {path: object path}}`, with the files that the input selects |
| `layer_files` | `{step name: [{path, size, sha256}, …]}`, from receipts for exactly the same selected files |
| `options` | The options |
| `output` | An empty directory. The layer is the files that the step writes in it |
| `metrics` | A path. The step can write a JSON object there, for example the size of each section |

A command starts in the repository root. Its standard output and standard error go to the
standard error of the engine. Exit status 0 is success. The objects are read-only. While a step
runs, `output` and `metrics` are in `partial/layer-<key>/`; the engine removes that directory
when the step ends, also when it fails. A failed step writes no receipt. A declared directory may be empty when the step creates it.
A declared output that does not exist is an error.

### Offline

A step reads the data inputs of its request. A step does not use the network; fetchers are the only network users. A step must not write to its
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

`layers/<key>.json` is the receipt of the files of one step, a JSON object:

| Key | Meaning |
| --- | --- |
| `layer`, `step`, `format`, `revision`, `key`, `options` | As in the key |
| `inputs` | As in the key, and `files` for a step input that selects files: the selected paths, sorted |
| `digest` | The digest of `files` |
| `files` | One item per file: `path` in the layer, `size` in bytes and `sha256`, sorted by `path` |
| `built` | `YYYY-MM-DDTHH:MM:SSZ` |
| `wall_ms` | The time from start to end, in milliseconds |
| `cpu_ms` | User and system time in milliseconds of the command and the children it waited for (`wait4`). For a Rust step, of the whole process, so it is exact only while no other step runs. `null` when the system does not report it |
| `peak_rss_bytes` | The peak resident set of the command or of a child it waited for. For a Rust step, the peak of the whole process so far: an upper bound |
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
fetch. An `id` names a group only in the plan that it comes from.

| Key | Value |
| --- | --- |
| `id` | The step of the first build of the group, in dependency order. For an update: the layer, or the layers that `--only` names, joined by `,` |
| `cause` | `null`. For an update: `{"kind": "stale", "cause"}` with the [cause of the layer](#state-of-a-layer), `{"kind": "off"}` for a layer that no step makes now, or `{"kind": "named"}` for `--only` |
| `layers`, `drops` | `[]`. For an update: the steps that the group changes, as `builds` gives them, and the steps of the serving release that the new release no longer has |
| `fetches` | One per version and `params`: `source`, `version`, `params` (`[[NAME, VALUE], …]`), `files` and `bytes`. `files` are the names of the files that the store lacks. `[]` means that the store cannot name them, and the fetch gets every file that it gives |
| `builds` | In dependency order: `step`, `recipe` (see [Keys](#keys)), `key` (`null` while the step waits for a fetch or another build), `estimate`, and `again`: whether the step builds also when the store has its key |

The estimate of a build is `wall_ms`, `bytes_out`, `client_bytes` (the size of the files that a
client reads) and `peak_rss_bytes` of the newest receipt, by `built`, of the same step with the same
options. Without one, it is the `cost` of the output of the step in the serving release, else the
newest receipt of the step, or `null`. The `bytes` of a fetch is the size of its files in the
record of the version. When that record does not list them all, it is the size in the record of
another version that lists them all, the last in byte order. Without `files`, the files are those
that a fetch with the same `params` gave, or else every file. Otherwise `bytes` is `null`. A run
fetches a version with the same `params` once, with the files of every group that needs it.

### Device-map publication

The `device/catalog` step consumes selected layer paths and their receipt metadata. Map and terrain
layers keep empty-coverage records and decoded reference credits in `metadata/`. The `cells/`
and `terrain/` payloads and `metadata/` are client files: a target holds the metadata that the
catalog and the map cells read, so another machine bakes them without the leaf. Every file of the catalog layer is a client file: the
pinned satellites in `objects/`, which it names `/objects/<sha256>`, and the root `catalog.json`
with the schema, terrain, licence and region files, which are the named files of the device. The
option `credits` holds the OSM source entry and the credits of GLO-30 and of the landmark
articles, from `data/sources.toml`, so that a credit edit changes the key.

The catalog selects only Geofabrik picks whose complete band and terrain coverage is available.
A saved Box or multi-area definition has its own complete catalog pick. Source polygons determine
which edge cells are partial. Missing required map, terrain or article layers block the catalog;
a terrain-only release cannot publish.
Before a target serves a release, the product compares picks with the serving release and
assembles changed picks with the real assembler. The production reader verifies each result.
Missing inputs and invalid artifacts fail verification before the target changes. An unchanged
pick can reuse prior verification.

### Runs

An update and a rollback own one run each. It starts after the arguments are checked, and
finishes after the switch, and for an update after the collection that follows it.
`update --dry-run` creates none.
A read-only command creates none. A failure after the run starts returns its id in `error.run`.

A run fetches the fetches of a plan, one after another, with the fetchers of [Fetch](#fetch).
Then it builds the builds of the plan. A layer that a planned step reads, and that the plan does
not build, must be in the store or kept by reference. A planned key that is not the key of the
step now fails the run.

- A step starts when the layers that it reads are built.
- At most one step per core runs at a time.
- Independent Rust and command steps run in parallel within the same job and memory limits.
- The sum of the estimated peaks (`peak_rss_bytes` of the estimate) of the steps that run at the
  same time is not more than the physical memory of the machine. A step without an estimated
  peak counts as the whole memory. A step whose estimate is more than the memory runs alone.
- After a fetch fails, no step starts. After a step fails, no other step starts, and the steps
  that run finish. The layers that the run built stay in the store, so the next update reuses
  them: it continues after the failed step.

The id of a run is its start time in UTC, `YYYY-MM-DD-HHMMSS`, with `-2`, `-3` and so on when
another run has that id. The process that holds the lock `run-<id>` is the process that runs the
run. A run without a `finished` event whose lock is free has failed.

`runs/<id>.jsonl` has one JSON object per line. The key `event` gives its kind:

| `event` | Keys |
| --- | --- |
| `started` | `command`, and `at` (`YYYY-MM-DDTHH:MM:SSZ`) |
| `phase` | `phase`: `prepare`, `download`, `build`, `verify`, `upload`, `switch` or `collect` |
| `published` | `mutation`: `{"kind": "switched", "release"}` after the switch of the target |
| `fetch_started` | `source`, `version` and `params` |
| `fetch_finished` | `source`, `version`, `params`, `resolved` (the acquired version), `bytes` (the size of the files that the fetch gave, downloaded or found in the store) and `wall_ms` |
| `fetch_failed` | `source`, `version`, `params` and `error` |
| `step_started` | `step` |
| `step_finished` | `step`, `reused` and `receipt` |
| `step_failed` | `step` and `error` |
| `finished` | `ok`, `error` (`null` when `ok`) and `wall_ms` |

A fetch without a version uses `newest` as its request version. Its finished event records the
acquired version separately. `runs RUN` gives the last phase, also after a later phase fails.

### Versions

A step list gets the version of each fetch that it reads from one function
(`obc_data::product::version`), by three rules:

1. A due fetch reads the newest version upstream, and the update uses that version until it ends.
2. Otherwise the fetch reads the newest version that the machine knows: the newest of the serving
   version, the versions of the fetch in the store and the `start` of the source. The serving
   version of a fetch without `NAME=VALUE`s, or of any fetch of a source with a `start`, is the
   newest serving version of the source: one version names all its fetches, such as the GLO-30
   tiles of every region, or the extract of a new area.
3. Otherwise it reads the newest version upstream: the product names the fetch, and the update
   fetches it. A source whose URL needs a `NAME=VALUE`, such as the GLO-30 tiles, has no one newest
   version: a fetch of it without that value fails with `failed`, and the fix names the code of the
   product.

A `date` or a `release` version is newer by its parts: a run of digits by its number, any other
run as text, so `v1.9` comes before `v1.11`; two versions with equal numbers, such as `v1.0`
and `v1.00`, are in the order of their text. A `commit` or a `digest` has no order: rule 2 gives
its `start`, so that a change of `data/sources.toml` moves it; else the version that
`obc data source refresh` took last on this machine, when the target serves a version that the
targets served then, or none: a refresh never moves a target that another machine moved since;
else the serving version; else the one
version in the store, when the store has exactly one. So an update after an update that changed
nothing reads the same versions.

Before it lists the steps, an update finds the fetches that the step lists read, with only small
metadata fetched (see [Products](#products)). It judges each of them, as
[State of a source](#state-of-a-source) says, with the policies of the target. A stale fetch is
due. A `manual` source is never due: a step list reads a newer version of it only when the store
has one. In one update, a fetch of a `manual` source without a version takes the version of the
first fetch of that source.

### Products

A product is a set of steps that makes part of a release, such as `planner` or `device`. The
`obc data` binary (`host/obc-data-steps`, GPL-3.0-only) gives the list of products to the
commands of `host/obc-data`. The `obc-data-plumbing` binary of `host/obc-data` has the same
commands without products, for scripts that use R2. Each step name starts with `<product>/`. A
product can list a step of another product, such as the landmark content that the planner search
reads; both products declare it the same.

A product gives its steps for the settings of a target, the versions of [Versions](#versions),
its regions and the store. When the step list depends on a snapshot that the store does not have,
such as the `.poly` of a region, the product names those fetches instead. The command fetches them
and then asks the product again, as long as each round names only new fetches: a fetched file can
name the next fetch. A fetch that fails, fails the command with its own code, `fetch_failed` or
`blocked`. A product that names a fetch it named before, that still names fetches after 8 rounds,
a step name without the name of a product, or a step that differs from the same step of another
product, fails the command with `failed` and a fix that points at the code of the product. A
product that has no steps for the settings, such as for a kind of region that it does not read, is
blocked. An error of the store, a file or the data of a fetch in a step list fails the command with
`failed`.

A product returns usable steps and blocked layers together. Each blocked layer has `step`,
`layer` and `reason`. An update with a blocked layer fails with `blocked` before it bakes. A
failed capture fetch blocks its content and artifacts; other map layers keep their steps. `obc
data` lists blocked layers with the reason. A read-only command fetches only small metadata: the
Geofabrik polygons, the Geofabrik index and the GLO-30 tile list. Another fetch that it needs
blocks the product with the fix `obc data update TARGET`.

A product also gives what clients read of a release: its pointer, which is one of the two pointer
keys with the client file of a step that a client reads there, and its named files, the client
files that a client finds by path. A product checks a release before a target serves it. The
device checks changed selections with the production assembler and reader. The planner verifies
stored grid and runtime artifacts.

Each step selects the files that clients read: the device, the web planner or a service on the
VPS. Other files are intermediate: only other layers read them. A target holds only selected
files (see [Targets](#targets)). A change of the selection makes the layer stale with the cause
`input` `options`, and reuses the bytes of its key.

#### `device`

The device maps have layers per leaf: a cell of size `2^23` µdeg of the OBCA grid that the
outline of the region touches. The outline of a `box` region is its box; the outline of a
`geofabrik` region is the union of its source `.poly` files from `geofabrik-poly`,
`area=<source path>`. Existing union definitions select the union of their leaves.

Both products resolve the same source areas. A Geofabrik definition names its areas directly.
For a Box, the cached index selects candidates by polygon coverage. Selection descends into
children only when their union covers the requested part of the parent. Full `.poly` files
must then cover the complete Box. A gap tries the parent extract before selecting bulk inputs.
An uncovered Box gives an actionable error. Country metadata does not remove required geography.

Each selected area has a private `<product>/source/<area>` step. It reads the extract and full
`.poly` at their separate exact versions, and writes `source.osm.pbf` and `source.poly`. A Box
also records its selected index version. These receipts retain active request provenance even
when a capture holds older extract or polygon bytes. Each product sorts and deduplicates areas.

A multi-area input uses prepared `osmium merge --with-history`, then `time-filter` at the latest
object timestamp present in the union. For an overlapping object, its highest supplied version
wins. Unique supplied objects remain. An extract cannot reveal a deletion absent from its bytes;
this is a union of available snapshots, not a snapshot at one common date. Conflicting payloads
at the same object type, id and version are malformed OSM inputs.

The merge and the map crop run the Osmium of `OBC_OSMIUM`, or else `osmium` on the `PATH`.
Without it, an update blocks their layers and asks to install Osmium or set `OBC_OSMIUM`. It does
not download or install a tool.

The steps read `copernicus-glo-30` and the national terrain models (`dtm-*`), and the step list
reads files such as a `.poly` and the GLO-30 tile list, each at its version (see
[Versions](#versions)). `copernicus-glo-30`, its tile list and the `dtm-*` sources are `manual`.

A leaf reads the archive tiles of its terrain cells: those of the windows that the crest rule of
`OBCT_Spec.md` §9 reads, which have a halo of two postings. A `dtm-*` source reads a tile when
the tile meets its `extent` and the `geofabrik-poly` polygon of one of its `areas`: the boundary
of the polygon crosses the tile, or the polygon holds the centre of the tile. Each such tile is
one fetch `tile=<ti:04>-<tj:04>` at the version of the source, so leaves and regions share
tiles and a stopped fetch resumes at the next tile. A source with only `.none` files in the leaf
adds nothing, and a leaf where no source has a height has no `device/reference` layer. While the
store lacks a tile of a source whose `credential` is not on this machine, its reference and
terrain layers are blocked. Bands that read that
terrain are blocked too. Other leaves and bands keep their steps.

| Layer | Reads | Options | Files |
| --- | --- | --- | --- |
| `device/region-osm` | The selected PBF of each `device/source/<area>`; only for multiple areas | None | `osm.pbf`: the available-snapshot union |
| `device/osm` | The PBF of one `device/source/<area>`, or `device/region-osm` | `leaves`: `[i, j]` of each leaf | `osm/<i>-<j>.osm.pbf`: the `osmium extract --strategy smart --set-bounds` of the square of the leaf and one µdeg around it. The metrics name the version (`osmium`) |
| `device/<band>/<i>-<j>` | `device/osm`, the file of the leaf; `land-polygons`; `device/terrain/<i>-<j>` when the cells of the band read heights: contours in their levels, or a nav graph or POIs | `band`: `coarse`, `mid`, `fine` or `network` of the recommended band table (`OBCA_Spec.md`); `leaf`: `[23, i, j]`; `cells`: `[ci, cj]` of each cell of the band in the leaf that the outline touches | `cells/<band>/<ci>/<cj>.obcm` for each cell with content; `cells/<band>/empty.json`: the ids of the other cells. A cell has the bytes that one cut of the whole leaf with all bands writes, with `builder/presets/schema.json` and without landmarks or peaks |
| `device/reference/<i>-<j>` | Each `dtm-*` source of the leaf with a height, without params, the files of its tiles | `models`: `source`, `version` and `credit` (its `attribution`) of each model; `tiles`: the ids `<ti:04>/<tj:04>` of the archive tiles that the terrain cells of the leaf read | `reference/`: the reference archive (`host/obc-dem/reference/README.md`) of the models, which `ingest.py ingest` of the pooled tiles of each model writes into an empty archive, best first by `PRIORITY`, cut to `tiles`. The `fetched` day of a model is its version. A Python step with the group `terrain-reference` |
| `device/terrain/<i>-<j>` | `copernicus-glo-30`, `tile=` of each tile that the square of a cell reaches and that `copernicus-glo-30-tiles` names. A square without a tile is sea. A leaf without a tile reads no snapshot. `device/reference/<i>-<j>` when the leaf has one | `posting_log2` and `cell_log2` of OBCT v1; `cells`: `[ci, cj]` of each terrain cell in the leaf that the outline touches | `terrain/<ci>/<cj>.obcd` for each cell with a height (`OBCC_Spec.md` §13), the bytes that `obc-bake terrain --reference` writes from the same tiles and archive; `terrain/empty.json`: the ids of the cells without a height; `terrain/credits.json`, when a cell reads a national model: `key`, `product`, `attribution` and `licence` of each model that a cell reads, as the reference archive states them |
| `device/landmark-content[/<area>]`, `device/peak-content[/<area>]` | Shared `wikidata`, `wikipedia` and `commons` facts requested by exact identity; regional `geofabrik-extracts` and `geofabrik-poly` | None | `landmarks/content.json` or `peaks/peaks.json`, device photos, and bounded `shared-content/` fact bundles. Only the bundles are selected publication outputs of this intermediate layer. Final device assets are published by the device cell layers |
| `device/landmarks/<i>-<j>`, `device/peaks/<i>-<j>` | Every area's landmark content and `device/osm`, the file of the leaf; or every area's peak content | `cell_log2`: 18; `cells`: `[ci, cj]` of each network cell of the leaf, as for `device/network/<i>-<j>` | `landmarks/<ci>/<cj>.bin` or `peaks/<ci>/<cj>.bin` for each cell that owns content (`OBCC_Spec.md` §14.3). A landmark joins the OSM objects of the leaf that name it |

`<i>`, `<j>`, `<ci>` and `<cj>` have four digits or more, as in a cell id.

The content of a collection reads the [shared Wikimedia content](wikimedia-content.md) of the
identities that the extract and the `.poly` of its area name, and that extract and `.poly`. The
step makes the boundary and the candidates or the summits again from them, offline.

A single-area definition keeps the unsuffixed content layer names. Other definitions have one
content layer per collection and source area. Article inputs keep separate file views;
canonical content merge rules deduplicate overlaps without losing unique articles.

`device/source/<area>`, `device/region-osm`, `device/osm`, `device/reference/<i>-<j>` and all content
layers are private intermediates. Their complete records stay in release provenance.

#### `planner`

The planner reads the `countries` of the region, or else the countries of its source areas in
the Geofabrik index, sorted; a region without countries is blocked. It reads the `time_zone` of the
region, or else `UTC`. It uses the shared source selection above. Its bounds enclose the requested Box or union of selected outlines.
Each snapshot, such as `copernicus-glo-30`, the extract, the `.poly` and the GLO-30 tile list, is
at its version (see [Versions](#versions)), as for `device`. The other options come from
[`data/planner.toml`](#dataplannertoml). A GLO-30 input reads the tile
of each 1° square that its box touches and that `copernicus-glo-30-tiles` names; a box at sea
reads no snapshot. No layer reads a national terrain model yet.

| Layer | Reads | Options | Files |
| --- | --- | --- | --- |
| `planner/osm` | The selected PBF of each `planner/source/<area>` | `path`: `osm.pbf` for one area; none for multiple areas | `osm.pbf`: one extract as it is, or the available-snapshot union |
| `planner/basemap` | `planner/osm`; The selected jar of `protomaps-basemaps`; `natural-earth`, `water-polygons`, `land-polygons`, `daylight-landcover`, `qrank`, `pgf-encoding` | `bounds` of the region; `attribution` of `osm-planet`, `natural-earth` and `daylight-landcover` | `basemap.pmtiles`: the Protomaps map at zooms 0 to 14 |
| `planner/terrain` | The GLO-30 tiles of `bounds` | `bounds`: west, south, east and north of the zoom 10 tiles that the bounds of the region touch and of their neighbours, widened to `terrain.margin_m` around the bounds; `attribution`: the credit of GLO-30 | `terrain.mbtiles`: lossless Terrarium WebP tiles of zooms 0 to 12, the bytes that `planner-dem` writes from the same tiles |
| `planner/routing` | `planner/osm`, and the GLO-30 tiles of the bounds of the region | `region` (the last part of the region id), `bounds`, `profiles` and `countries`; `attribution` and `terrain_attribution`: the credits of OSM and GLO-30. The import applies the German access defaults | `routing/`: the package of [the route package contract](route-package.md) with `overlays.sqlite` and `route-catalog.json`; `blocks/`: the routing blocks of the grid cells, as `route-blocks` writes them; `routes/<cell>.json`: the records of `route-catalog.json` that name the cell, with a final newline |
| `planner/overlays` | `planner/routing` | `attribution` of `osm-planet` | `overlays.pmtiles`: the route networks and the access of `routing/overlays.sqlite`, as [the planner release](planner-release.md) defines it |
| `planner/assets` | `protomaps-assets`, `tangrams-icons` | None | `assets/fonts/` and `assets/sprites/` of the assets archive; `assets/sprites/LICENSE.txt`: the MIT notice of `tangrams-icons` |
| `planner/model` | `query-model` | None | `model/`: `model.int8.onnx`, `tokenizer.json` and `tokenizer_config.json` of the archive, and `labels.json`, the labels of the query schema |
| `planner/search/policy` | `nominatim-country-data` | None | `policy.json`: the country names, postcode formats and address levels of the archive; `country_osm_grid.sql.gz`: the country grid of the archive, as it is |
| `planner/search/dump` | `planner/osm`, `planner/search/policy` | `country`: the first of `countries` in lowercase, the country of a place outside each country polygon | `search.jsonl.zst`: the search records that `obc-search-bake` writes, one JSON object per line, with the keys of each object in byte order |
| `planner/search/records` | `planner/search/dump` | None | `pois.jsonl.zst` and `addresses.jsonl.zst`: the records of the dump with a POI and with an address |
| `planner/search/pois`, `planner/search/addresses` | `planner/search/records`; POIs also read the selected `device/landmark-content*` and `device/peak-content*` outputs | `component` (`pois` or `addresses`); `region`, `bounds` and `countries` as for `planner/routing`; `time_zone` of the region; `attribution` of `osm-planet` | `<component>/<region>.sqlite`: the search database of the component, schema 6. The key holds no SQLite version: another Python patch release can give other database bytes. The metrics name the version as `sqlite` |
| `planner/places` | `planner/search/pois` | None | `places.pmtiles`: the rider places of the POI database, at zoom 11 |
| `planner/climate` | `era5-land`, `bbox=<bounds>`, `first-year=<climate.first_year>` | `bounds`, `first_year`, and `attribution` of `era5-land`, whose `{year}` is the year after the ten years, `first_year` + 10 | `climate.pmtiles`: [the climate archive](planner-climate-tiles.md) |
| `planner/snow` | `hr-wsi` when its `extent` meets the bounds and its fetch has files, and `modis-snow`, each `bbox=<bounds>`, `seasons=<snow.seasons>`; `hansen-gfc`, `tile=` of each 10° tile that the bounds touch | `bounds`, `seasons`; `attribution`: the credits of `hr-wsi` when it reads it, `modis-snow` and `hansen-gfc`; `year`: the year of the `hr-wsi` version, which fills its `{year}`, or null | `snow.pmtiles`: [the snow archive](planner-snow-tiles.md): HR-WSI where it has data, and MODIS elsewhere |
| `planner/sun` | `planner/terrain/grid` | `bounds`, `time_zone` of the region, `distance_m` (`terrain.margin_m`), `horizon_samples` and `horizon_directions` | `sun.pmtiles`: [the sun archive](planner-sun-tiles.md). `terrain_grid_sha256` binds the canonical terrain grid index. The step verifies and reconstructs the portable terrain tiles |

Each map producer has a `planner/<kind>/grid` step, where `<kind>` is `basemap`, `places`,
`terrain`, `overlays`, `climate`, `snow` or `sun`. It reads only the archive of its producer.
It converts terrain MBTiles with the pinned Python PMTiles library. Each tile goes into the
archive of its zoom 11 parent, or its own tile when its zoom is less than 11. The tile bytes
stay the same. Each archive is packed as `maps/tiles/<kind>/<z>-<x>-<y>.pmtiles`, and its TileJSON
as `maps/<kind>.json`, with the transport rules of [the offline contract](planner-offline.md).
The step writes the packed bytes in `objects/<transport sha256>`, and selects `objects` and
`index.json` for clients. Its `index.json` has `format: 1`, `kind`, `map_zoom: 11`, `files`,
`source` and `metadata`. `files` maps logical names to source and transport hashes, sizes and encoding.
`source` names the converted archive bytes, or the raw MBTiles when it has no tiles. `metadata`
is the TileJSON. Empty terrain has TileJSON and no tile archives.

`climate`, `snow` and `sun` are optional layers: a step only when `layers` of the settings
names it. All producers are intermediate layers. Grid steps select their `objects/` and their
`index.json`.

| Layer | Reads | Files |
| --- | --- | --- |
| `planner/routing/grid` | `planner/routing` | Packed `routing/blocks.json`, `routing/packs/<sha256>/pages.bin` and `pages.idx`, `offline/routing-cells/<cell>.json`, and `routes/tiles/<cell>.json`. It does not build the routing blocks again. Its index records the source graph and cell descriptors |
| `planner/search/pois/grid`, `planner/search/addresses/grid` | The database of its search component | `search/tiles/<component>/<cell>.sqlite`, packed. A cell retains intersecting places, addresses, their streets and contexts. The index records regional metadata and clipped cell bounds |
| `planner/assets/grid`, `planner/model/grid` | `planner/assets` or `planner/model` | The input files, packed as `maps/assets/` or `search/model/`. The index maps logical paths to file and transport identities |
| `planner/fonts/grid` | `planner/basemap`, `basemap.pmtiles`; `planner/places`, `places.pmtiles`; `planner/routing`, `routing/overlays.sqlite`; `planner/assets` | Packed `offline/fonts/<stack>.pbf`: the joined used glyph ranges of [the offline contract](planner-offline.md). The index maps each original glyph range to its joined stack |
| `planner/index` | Only `index.json` of each grid step | Packed `search/<region>.grid.json` and `offline/catalog.json` in `objects/`; `release.json` and the small tile pointers in `public/`. It selects those three outputs. Its own `index.json` stays local |
| `planner/catalog` | `planner/index`, and `index.json` of each grid step | `catalog.json`: the pointer that a planner client reads first. Its `active.id` is the planner release id: the digest of the named files of the planner. `origins` is the `[publication]` table of `data/planner-runtime.toml` |

The overlay grid reads only `index.json` of the routing grid too. It checks the source routing
package and changes its TileJSON `routing_package` to the grid package hash. The final index
checks the OSM source, region, coverage, search schema and time zone, routing cell bounds, terrain
inputs and sun terrain identity. It reads no payload to compose metadata. A climate change builds
only its producer, its grid, the final index and the pointer. The named files of the planner are
`release.json` and `public/` of `planner/index`, and the `index.json` of each grid step at
`indexes/<step>/index.json`. The engine release manifest holds source versions
and preparation provenance; the client manifest holds the [planner release](planner-release.md)
data. It contains no deployment probe, device catalogue snapshot or source mirror.

A Python step runs `env PYTHONHASHSEED=0 uv run --locked --offline --no-default-groups
--no-python-downloads --group <group> python <entry> --step` in the repository root.
`uv sync --all-groups` prepares the packages on a machine. The fixed hash seed keeps the order
of a set out of the bytes. A credit that it writes comes in its options.

### Service runtimes

`data/planner-runtime.toml` names the Linux target triple and the `major.minor` versions
of Python and Node. Its `[publication]` table names HTTPS `site_origin`, `api_origin`,
`objects_origin` and `tiles_origin`. The shared pool is `<objects_origin>/objects`. Tile URLs
are under `<tiles_origin>/releases/<id>/`: the tile Worker reads `releases/<id>/public/` and
`objects/` of the bucket.

The pointer names routing, search and downloads under
`<api_origin>/planner-api/releases/<id>/SERVICE`, and the release document at
`<objects_origin>/releases/<id>/release.json`. Its `device_catalog` names
`<objects_origin>/cell-catalog/catalog.json`. [`obc data serve`](#serve) runs the services for a
`dir` target; `obc data` does not install them on a server.

Downloads jobs return a ready quote with an absolute pinned `source` URL. The client reads the
bundle, release and objects through that source, so a stable entry switch cannot mix releases.
The private selection key binds the object pool as well as the catalog and bounds. A quote
stays valid while its bundle exists; a missing bundle requires a new quote. Job creation completes synchronously; there is no mutable polling path.

A grid cell is a zoom 9 Web Mercator tile that the bounds of the region overlap, clipped to the
bounds, with the id `9-<x>-<y>`. The JSON objects that `planner/routing` writes have their keys in
byte order.

### State of a layer

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
`obc data` does not list an internal layer: the layers that read it show its cause. In JSON, a
layer is `{"layer", "state", "cause", "text"}`; `cause` and `text` are `null` unless it is stale.
A step that cannot be planned is not judged: `blocked` lists it with its layer and the reason.

## Targets

A target is a place that serves data. `data/targets.toml` says where each target is. What a
target holds is in the release that it serves.

### `data/targets.toml`

One table per target. The name of the table is the name of the target, in lowercase kebab-case.

| Key | Type | Meaning |
| --- | --- | --- |
| `kind` | string | `r2` or `dir` |
| `public` | string | `r2` only: the `https://` origin that serves the keys of the bucket, without a path |
| `dir` | string | `dir` only, optional: an absolute path, or a path that starts with `~/` for the home directory. Without it, the folder with the name of the target beside the store |

The file has `prod`, the public bucket, and `local`, a folder on each machine. A `dir` target and
the store do not overlap, also through a link, so a clean of the store never deletes a file of a
target. `obc data` reads an R2 target only through its public URL.

### Target layout

A key is a path with `/` between its parts. No part is empty, `.` or `..`.

| Key | Holds |
| --- | --- |
| `release.json` | The release that the target serves, and its rollback. A switch writes it first |
| `lock.json` | The [lock](#lock): the run that changes the target, or free |
| `releases/<id>.json` | The [manifest](#release-manifest) of a release. Immutable |
| `releases/<folder>/<path>` | A file that a client finds by name. `<folder>` is the [digest](#keys) of the files of the folder, listed by `<path>`. So other files make another folder. Immutable |
| `objects/<sha256>` | A client file of an output. Immutable |
| `sources/<source>/<version>/<name>` | A copy of a file of a source version that an output of a release read, when the source has `redistribute = true`. Immutable |
| `cell-catalog/catalog.json`, `planner/catalog.json` | The pointers: a copy of the object that the serving release names for the key |

Only client files of outputs go to a target: an output that no client reads can derive from a
source that we may not give to others, and a bucket is public. A step that reads such an output
of a kept layer reads it from the store of the machine.

### Release manifest

`releases/<id>.json` is compact JSON with the keys of each object in byte order. The id is the
SHA-256 of these bytes.

| Key | Meaning |
| --- | --- |
| `settings` | The [settings of the target](#settings-of-a-target) |
| `outputs` | One output per step, sorted by `step` |
| `pointers` | `[{key, size, sha256}]`, sorted by `key`: both pointer keys, each with the client file of an output that a client reads there |
| `named` | `[{key, size, sha256}]`, sorted by `key`: the files under `releases/<folder>/`. Each is a client file of an output |
| `sources` | `[{source, version, params, files, copy, licence, attribution}]`, sorted by `source`, `params` and `version`: each fetch that an output read, with the sorted `NAME=VALUE`s, the files that the outputs read (`[{name, url, size, sha256, retrieved}]`, sorted by `name`; `url` is where upstream gives the file, and `retrieved` when a machine retrieved it first), whether the target copies them under `sources/`, and for a copy the `licence` and `attribution` of `data/sources.toml`. Two fetches that name one copy name it with one SHA-256 |
| `machine` | The machine that made the release |
| `time` | When the machine made it, `YYYY-MM-DDTHH:MM:SSZ` |

An output has `layer`, `step`, `format`, `revision`, `key`, `inputs`, `options`, `digest` and
`files` as in the [receipt](#receipt), and:

| Key | Meaning |
| --- | --- |
| `snapshots` | `{source: {"version", "params"}}`: the version and the sorted `NAME=VALUE`s of each snapshot that the step read |
| `client` | The client outputs that the step selects (see [Layers](#layers)) |
| `cost` | The `wall_ms`, `peak_rss_bytes` (`null` when the system does not report it) and `bytes_out` of the build that made the receipt |

The objects of a release are the client files of its outputs, one object per SHA-256. A release
names its files exactly: a file in a folder of `releases/` that the manifest does not name is not
a file of the release.

### `release.json`

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

`since` is the time at which the release took its role, or at which a switch to the serving
release wrote its pointers again. `rollback` is `null` until the second switch.

### Publish and switch

1. The target gets each key of the release that it does not have, the manifest last. Before the
   target changes, each of these files in the store must have the SHA-256 that the release
   gives. A key is immutable: a key with another size is refused, and nothing replaces it.
2. The switch reads the manifest from the target. A conditional write replaces `release.json`
   only when it is still the file that was read, or when it is still absent. The release that
   served becomes the rollback. When the condition fails, nothing changes.
3. Then each pointer key of the release gets the bytes of its object.

Before it writes `release.json`, a switch to another release needs each pointer to hold nothing,
the bytes of the serving release, or the bytes of the release that it switches to. A switch to the
serving release writes its pointers again, and sets `since` of the serving release: a pointer
that drifted served other bytes until then. So each pointer holds the bytes of the serving release
or of the rollback, and a rollback after a stopped switch needs nothing of the release that it
leaves. Drift is a pointer whose bytes are not those of the serving release. Nothing deletes a
pointer key: clients of older publishers read these keys.

A `dir` target must be on a local file system. A conditional write holds `<dir>/.lock` while it
compares and writes, and writes through a rename. The tag that it compares is the SHA-256 of the
bytes. A key can be a hard link to an object of the store: nothing writes a key or an object in
place.

Older publishers wrote other keys of the `prod` bucket, such as `cell-catalog/objects/`,
`planner/releases/` and `reference/v1/`. An update and a rollback never write or delete them.

### Lock

`lock.json` names the run that changes a target: `{"state": "free"}`, or the holder.

```json
{
  "state": "held",
  "machine": "<machine>",
  "run": "<run id>",
  "review": "<review id>",
  "phase": "build",
  "heartbeat": "2026-10-12T08:00:00Z"
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
- When a write fails its condition, another run took the lock. Then the run stops and changes
  nothing more. The write at the start of the phase `switch` checks this immediately before the
  switch.
- At its end, the run frees the lock with a conditional write of `{"state": "free"}`.
- `obc data unlock TARGET --from MACHINE` frees the lock of a run of MACHINE whose heartbeat is
  10 minutes old or older (`STALE` in `lock.rs`): ten heartbeats that did not come. It asks
  first. When that run continues, its next write fails, and it does not switch.

### Update

`obc data update TARGET` makes a target serve its settings as the steps make them now. It writes
only a `dir` target. One run does each phase in this order, while it holds the [lock](#lock):

1. Prepare. It reads the serving release, applies the flags to its settings, and refuses a target
   without a region. It finds the fetches of the step lists, judges which are due (see
   [Versions](#versions)), and lists the steps, with the fetches that the step lists need. It
   judges each layer against the serving release, as [State of a layer](#state-of-a-layer) says:
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
   file is the same when its name, size and SHA-256 are. Whether the target copies a source, and
   its credit, change no file of the release. Else each product checks it, and a failed check
   changes nothing.
5. Upload and switch, as [Publish and switch](#publish-and-switch) says.
6. Collect, as [Collection](#collection) says, when it switched to another release.

An update that stops keeps what it baked in the store, and the next update reuses it.

### Any machine

An update needs no file of the serving release in the store of the machine. It takes only what
its bakes and the checks of the products read:

- A fetch that the serving release lists with `copy` comes from the copies on the target, in the
  phase `download`: the files that the store lacks, with their snapshot record and the time when a
  machine retrieved them first. Another fetch comes from upstream.
- A step that bakes and reads a file of a fetch that the serving release lists must find the same
  SHA-256 in the record of the store: before the review, and again after the fetches of the
  update, before any bake. Else nothing changes (`verify_failed`).
- A fetch that the serving release read, of a source whose upstream gives only its newest version
  (a capture, a national terrain model, a delivery) and that the target does not copy, refuses the
  update (`blocked`) before the fetch, also while the update lists the steps, when the store lacks
  it and a capture cannot give it: this machine cannot get that version again. A capture gives the
  version of today, and a version of a manual source that the store started (see [Fetch](#fetch)). The fix names the machine that made the serving release. A refresh that
  moves the source to a new version is the other way. A national terrain model on a new machine
  is such a case.
- When the store and the serving release give a fetched file the same name, size and SHA-256, the
  new release keeps the entry of the serving release.
- A file of a kept step that a step that bakes reads, or that the check of a product reads, comes
  from `objects/` of the target when the store lacks it.
- A kept step whose file a step that bakes reads, when no client reads that file and the store
  lacks it, bakes again on this machine: the target does not hold it. Its cause is `unheld`. The
  review lists such a layer, also one that only other layers read.

### Review

The review of an update says what it does before it changes a target. `--dry-run` writes the
review and creates no run. To list the steps, an update fetches what the step lists need, which
can be bulk data. It fetches into the store only, and it never writes to the target.

- `layers`: each layer that bakes or goes, with its cause (`stale` with the cause of
  [State of a layer](#state-of-a-layer), `named` by `--only`, `off`, or `unheld` as
  [Any machine](#any-machine) says), the steps that bake, the grid leaves (the last part `<i>-<j>`
  of the names of the steps of a tiled layer), the steps of the release that no step makes now, the
  readers and a format change. With `--only`, a layer that bakes because it reads a named layer
  follows that layer. A layer that only other layers read is not listed.
- `sources`: each fetch that a baked step reads at another version than the serving release.
- `kept`, `fetches`, `downloads` and `bakes`: the outputs that the release keeps by reference, and
  what this machine fetches upstream, downloads from the target and bakes. A step whose output is
  in the store already does not bake.
- `cost`: each bake has the estimate of the newest receipt in the store of the step with the same
  options, else of the `cost` of the step in the serving release, else of the newest receipt of the
  step. `upload_bytes` is the size of the files of the bakes that a client reads, and of the copies
  under `sources/` that the serving release does not name, as the store records them. `unknown`
  lists each bake without an estimate or without a peak, and each fetch whose size is not known,
  also of a new copy that the store lacks yet. `process_peak_rss_bytes` is the highest peak of a
  bake that runs in the `obc` process: the peak of that process, not of the step.
- `machine`: the memory of the machine and the free bytes of the disk of the store after the
  listing. A bake that runs in its own process and whose estimated peak is more than the memory,
  or fetches, downloads and bakes that leave less than 4 GiB free on the disk, refuse the update.
  The memory check covers only the steps that run in their own process, where `wait4` gives the
  peak of the step.
- `after`: the release that becomes the rollback, and the rollback of now, which the collection
  drops, with the keys that the serving release does not name: what it deletes at most.
- `args`: the arguments after `obc data` that start the update of this review.

The id is the first 12 hex digits of the SHA-256 of the canonical JSON, as for the release id, of
an object: the name of the target, the tag of `release.json` (`null` while the target serves
nothing), the settings, `layers`, the steps that bake, and the source, version and params of each
snapshot that they read. It holds no cost and no time. Two machines that read the same target give
the same id when their stores resolve the same versions and files. `--expect ID` starts the update
only when its review, computed after the listing, has this id.

A format change is a layer that bakes, and whose outputs in the serving release have another
`format` than the layer now. Its readers need new data: old readers keep what they have and get no
new data until they update. An update with a format change starts only with
`--accept-format-change`. A new layer is no format change.

### Rollback

`obc data rollback TARGET` makes a `dir` target serve its rollback release again. The release
that served becomes the rollback, so a second rollback undoes the first. A target without a
rollback refuses. The target must have each key of the rollback release, else nothing changes.
Then it switches as [Publish and switch](#publish-and-switch) says. It holds the [lock](#lock)
as an update does. A rollback bakes, uploads and deletes nothing: the next update collects.

### Collection

A target keeps the files of the serving release and of the rollback. After an update switches to
another release, it collects the target. The grace of a collection is 10 minutes (`GRACE` in
`collection.rs`): longer than a client needs to fetch the files that a pointer it just read names.

- When a pointer does not hold the bytes of the serving release, it deletes nothing.
- It deletes the keys under `objects/` and `releases/` that no kept release names, the keys under
  `sources/` that a release that it drops names, and the temporary files of a write that stopped:
  a hidden `.part` or `.tmp` file in the root, or beside a pointer with the name of the pointer.
  The pointers, `release.json` and the keys of older publishers stay.
- A publish that stops can leave copies under `sources/` that no manifest names. They stay, and a
  later publish of the same version uses them again.
- A release that `release.json` does not name stopped serving at or before the time when the
  release that served before the switch began to serve. The first collection the grace or more
  after that time deletes it. Until then it stays, and its files too.
- It runs while the update holds the [lock](#lock): no other run writes the target.
- It deletes each other file that no remaining manifest names, then the manifests of the releases
  that go. A collection that stops resumes at the next update that switches, from the manifests
  that remain.
- When it cannot read a manifest, it deletes nothing.

A collection that stops does not undo the switch: the output gives what it deleted and why it
stopped.

### Serve

`obc data serve [TARGET]` serves a `dir` target, by default `local`, on port 8710 of every IPv4
address of the machine. It refuses an R2 target. The current release is the release whose
pointers clients get. Every answer allows each origin.

| Path | Answer |
| --- | --- |
| `/cell-catalog/catalog.json` | The device pointer of the current release |
| `/planner/catalog.json` | The planner pointer of the current release. Each string that starts with one of its `origins`, followed by `/` or nothing, starts with the origin of the request instead: `http://` and the `Host` of the request |
| `/planner-api/releases/<id>/<service>/<path>` | `<path>` of the `routing`, `search` or `downloads` service of the planner release `<id>`. Search gets `/api/planner-search/<path>`. 404 when the services of `<id>` do not answer |
| Another path | The file of the target at that key, with ranges; without a file, the tile handler of `planner/tiles` over the target |

Serve reads `release.json` each second. When it changes:

1. Serve starts the services of the planner release of the serving release and of the rollback,
   unless they run. The planner release of a release is `active.id` of its planner pointer.
2. Clients get the serving release once each service of its planner release answers: routing
   `GET /health`, search `GET /api/planner-search/status` with `parser.ready` true, and downloads
   `GET /catalog`. Until then clients keep the current release.
3. A planner release fails when one of its services stops, or when its services do not answer
   within 120 seconds after they start. Its other services stop, and its API answers 404. Clients
   keep the current release, also when the failed planner release is the one of the current
   release. A planner release that failed starts again at the next change of `release.json`, and
   at no other time.
4. The services of a planner release that neither the serving release, the rollback nor the
   current release names stop.

When the serving release and the rollback fail one after the other, clients keep a release that
`release.json` no longer names. Serve then writes a warning: an update can delete the files of that
release.

The services of a planner release read its files under `routing/`, `search/` and `offline/` in
`<store>/serve/<target>/<planner id>/`. Each file comes from `objects/` of the target: a link to an
`identity` object, or the decoded bytes of a `gzip` object under a temporary name. No write opens
an existing file. Each file must have the SHA-256 and the size that the planner release gives.
Serve removes `<store>/serve/<target>/` when it starts and when it stops; `obc data clean` does not
touch it. Download bundles name the origin of the machine on its network when serve starts, else
`http://127.0.0.1:8710`.

### Overview

`obc data` gives the machine that runs it and, for each target, the release that it serves and its
rollback, the run that holds its lock, the region, the state of each layer against the steps of now,
the blocked steps, the pointers that drift, an error that stopped the judgement, and the command to
run next. It fetches only small metadata, and checks upstream with the checks of the last hour. A
target that serves nothing has the next command `obc data update TARGET --region ID`. An R2 target
has no next command: `obc data update` writes only a `dir` target.

## Commands

| Command | Output |
| --- | --- |
| `obc data [--json]` | The [overview](#overview): the machine, and for each target what it serves and the command to run next |
| `obc data update TARGET [--region ID\|PATH[,PATH…] \| --box W,S,E,N] [--add LAYER]… [--remove LAYER]… [--refresh SOURCE=DAYS\|manual]… [--only LAYER,…] [--dry-run \| --expect ID] [--accept-format-change] [--json]` | The [update](#update) of a `dir` target. The flags change its settings. `--dry-run` writes the [review](#review) and changes nothing. Writes the steps that it baked or reused, the release that the target serves, its rollback, the number of kept outputs and what the collection did |
| `obc data rollback TARGET [--json]` | The [rollback](#rollback) of a `dir` target. Writes the release that the target serves and its rollback |
| `obc data unlock TARGET --from MACHINE [--yes] [--json]` | Frees the [lock](#lock) of a `dir` target that a run of MACHINE holds with a heartbeat 10 minutes old or older. Asks first. Writes the holder that it removed |
| `obc data sources [--check-now] [--json]` | Every source with licence and whether a target copies it, and for each target that serves a release its policy, its versions and its [state](#state-of-a-source); and the versions in the local store. Rows are in kind order: data, then assets, then tools. An upstream check of the last hour serves, except with `--check-now` |
| `obc data source refresh ID [--json]` | Fetches upstream's newest version of a `digest` source without `start` or params, and writes `refreshed/<source>.json` of the store with the versions that the targets serve: the next update of a target that serves one of them, or none, reads it, as [Versions](#versions) says. Refuses another source |
| `obc data fetch SOURCE[@VERSION] [NAME=VALUE…] [--json]` | Fetches the version, or else the newest file upstream. Writes the store path of each file |
| `obc data areas [QUERY] [--json]` | The regions of `data/regions/` and the Geofabrik areas of the cached index whose id or name holds `QUERY`. Fetches nothing |
| `obc data clean [--apply [--yes]] [--json]` | The plan of [Clean](#clean): the snapshot records and the objects that nothing reaches, what stays and why, and the size of `partial/`. `--apply` asks, then cleans. With `--json` and `--apply`, the plan goes to standard error, and the output is what it did |
| `obc data runs [--json]` | Every run in the store, newest first: id, command, outcome, time, and the size of its fetches and of the layers that it built |
| `obc data runs RUN [--json]` | One run, with its fetches and steps: time, peak RAM, outputs, inputs and layers |
| `obc data runs RUN --follow [--json]` | The events of the run as they come, until it ends |
| `obc data serve [TARGET] [--json]` | [Serves](#serve) a `dir` target until Ctrl-C. Writes its origin on the machine and on its network |
| `obc data setup [--json]` | What this machine can do: each job, each tool with its version or its install command (Java needs 21 or newer, Node 22 or newer), each credential (only whether it is set), and the store with its size and free space. It exits 0 also when a tool or a credential is missing |
| `obc data ui` | Builds the page with `npm run build:data` in `builder/web`, then serves it as [The page](#the-page) says, until it is stopped |
| `obc data open MACHINE` | Runs `ssh -N -L 8701:localhost:8700 HOST`, where HOST is the `ssh` of MACHINE in [`data/machines.toml`](#datamachinestoml) or else MACHINE, and opens `http://localhost:8701/` in the browser. It sends nothing to MACHINE |

`--json` writes one JSON document to standard output. [JSON schemas](#json-schemas) has the
schema of each output, and [Errors](#errors) has the error codes and the exit statuses. In
addition:

- `fetch` lists only the requested files.
- `runs` lists a run file that cannot be read as `failed`, or as `running` while its lock is
  held.
- `runs RUN --follow` writes one event per line, as in `runs/<id>.jsonl`. When the run failed,
  the error is the last line.

### The page

`obc data ui` listens on `127.0.0.1:8700` and answers:

- `GET /` with `builder/web/dist/data/data.html`, and `GET /PATH` with the file PATH in that
  folder. A part of PATH that is empty, `.` or `..`, or that holds `\`, names no file.
- `POST /api/cli` with the body `{"args": [ARG, …]}`: it runs its own binary as
  `obc data --json ARG…` in the repository root, with no standard input, and answers
  `{"exit": STATUS, "output": JSON}`, the exit status and the JSON that the command wrote. When
  the command writes no JSON, `output` is a `failed` [error](#errors) and the status is not 0.
  A command runs to its end, and its output is read, also when the request ends first. The page
  runs only `obc data` without a command and `obc data setup`.

Each request has a `Host` of `127.0.0.1`, `localhost` or `[::1]` at any port; an `Origin`, when
it has one, of `http://` and such a host; and, for a POST, the `Content-Type` `application/json`.
A request that does not, and a command that the page does not run, get HTTP 403 and
`{"exit": 2, "output": {"error": …}}` with the code `usage`.

## R2 client

The R2 client in `host/obc-data` reaches the bucket for `obc bake publish --target r2`,
`obc bake clean-r2`, `obc r2 rm`, `obc fixtures publish` and the firmware publish workflow.
rclone moves the bytes. The planner publish, deploy and finalize, and the reference archive
ingest, still use the remote of `tools/r2.py`. `obc data r2` is plumbing for scripts: those
commands call it. It does not change the state of a release. Once the bucket has the
`release.json` of `obc data`, `obc bake publish --target r2` refuses to run, and `obc data r2 put`
and `delete` refuse `release.json`, `lock.json`, a pointer key, and a key under `objects/`,
`releases/` or `sources/`. The planner publish, deploy and finalize, and `obc bake clean-r2`,
refuse to run when the pointer of their prefix has `release`, or is not JSON. The planner deploy
reads the pointer from the bucket again just before it writes it. `obc data` never writes under
`reference/v1/`.

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
- `delete` cannot be undone. It prints the plan: the bucket, and each object with its size and
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
| 2 | Usage: an argument is not valid, or a command that deletes did not get consent |
| 4 | Blocked: a credential is missing, a layer cannot be planned, the target cannot take the command, or this machine cannot do the work |
| 5 | Verify failed: a release failed its check, or the bytes in a target are not the bytes that the command wrote |

The commands that delete, `clean --apply` and `r2 delete`, and `unlock` show their plan and ask
in a terminal. Without a terminal, they need `--yes`. Without it, they exit with 2 and change
nothing.

### Error codes

| Code | Exit | When | Fix |
| --- | --- | --- | --- |
| `usage` | 2 | An argument is not valid, an id names nothing, the command runs outside the repository, or
another command must run first. | Correct the command. `obc data --help` lists the commands and their arguments. |
| `no_terminal` | 2 | A command that asks has no terminal to ask in, and no `--yes`. | Show the question to a person. When they agree, run the command again with `--yes`. |
| `not_confirmed` | 1 | The person did not answer yes. | Nothing changed. Run the command again when you want the change. |
| `invalid_data` | 1 | A file under `data/` is not valid. | Correct the file that the message names. `specs/obc-data.md` gives its format. |
| `fetch_failed` | 1 | A fetch or an upstream check failed. | Run the command again. A download continues where it stopped. |
| `blocked` | 4 | A credential is missing: a fetch failed without the credential of its source, or the R2
variables are not set. Or a layer cannot be planned, the target cannot take the command, or
this machine cannot do the work. | Set the credential that the message or `obc data sources` names, or correct what the message says a product needs, then run again. |
| `r2_failed` | 1 | R2 or rclone failed, or refused a key. | Check the key, the `OBC_R2_*` variables and that rclone is on PATH, then run again. |
| `verify_failed` | 5 | A release failed its check before a target serves it, or after an upload the object in the
bucket is not the file. | Upload the file again. |
| `run_failed` | 1 | A run failed: the build, or the run that `runs RUN --follow` shows. | `obc data runs RUN` shows the step that failed and its error. |
| `failed` | 1 | The store or the file system failed. | Correct the file or the directory that the message names, then run again. |

## JSON schemas

`--json` writes one document of the schema in this table to standard output. The schemas
come from the Rust types in `host/obc-data`. A test fails when this section is not the one
that they give; `OBC_UPDATE_DATA_SPEC=1 cargo test -p obc-data` writes it again.

| Command | Schema |
| --- | --- |
| `obc data` | `Overview` |
| `update` | `Updated` |
| `update --dry-run` | `Review` |
| `rollback` | `RolledBack` |
| `unlock` | `Unlocked` |
| `sources` | `Sources` |
| `source refresh` | `Refreshed` |
| `fetch` | `Fetched` |
| `areas` | `Areas` |
| `clean`, `clean --apply` | `CleanPlan` |
| `serve`, when it listens | `Listening` |
| `setup` | `Setup` |
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
    "AfterUpdate": {
      "properties": {
        "collects": {
          "anyOf": [
            {
              "$ref": "#/$defs/Collects"
            },
            {
              "type": "null"
            }
          ],
          "description": "The rollback of now, which the collection after the switch drops."
        },
        "rollback": {
          "description": "The release that becomes the rollback; `null` when the update changes no layer or the target\nserves nothing.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "rollback",
        "collects"
      ],
      "type": "object"
    },
    "Areas": {
      "description": "What `areas` writes.",
      "properties": {
        "areas": {
          "description": "Its areas whose path or name holds the query.",
          "items": {
            "$ref": "#/$defs/GeofabrikArea"
          },
          "type": "array"
        },
        "index": {
          "description": "The version of the cached Geofabrik index; `null` while the store has none.",
          "type": [
            "string",
            "null"
          ]
        },
        "regions": {
          "description": "The regions of `data/regions/` whose id or name holds the query.",
          "items": {
            "$ref": "#/$defs/Region"
          },
          "type": "array"
        }
      },
      "required": [
        "regions",
        "index",
        "areas"
      ],
      "type": "object"
    },
    "Baked": {
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
          "description": "The layer of the step.",
          "type": "string"
        },
        "reason": {
          "type": "string"
        },
        "step": {
          "description": "The step that cannot be planned.",
          "type": "string"
        }
      },
      "required": [
        "step",
        "layer",
        "reason"
      ],
      "type": "object"
    },
    "Cause": {
      "description": "Why a layer is stale: exactly one of three causes. When more than one applies, the first in\nthis order is the cause.",
      "oneOf": [
        {
          "additionalProperties": false,
          "description": "The version of the layer is not that of the release. `from` is `null` when the release has\nno step of the layer.",
          "properties": {
            "field": {
              "$ref": "#/$defs/Field"
            },
            "from": {
              "format": "uint32",
              "minimum": 0,
              "type": [
                "integer",
                "null"
              ]
            },
            "kind": {
              "const": "version",
              "type": "string"
            },
            "to": {
              "format": "uint32",
              "minimum": 0,
              "type": "integer"
            }
          },
          "required": [
            "kind",
            "field",
            "from",
            "to"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "description": "A source has a newer version to use. `to` is the version that the steps read, or the newer\nversion upstream; `null` when upstream names none.",
          "properties": {
            "from": {
              "type": "string"
            },
            "kind": {
              "const": "source",
              "type": "string"
            },
            "source": {
              "type": "string"
            },
            "to": {
              "type": [
                "string",
                "null"
              ]
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
          "description": "A layer or a source that the layer reads has other files, or `options`: the release has\nother steps of the layer, or a step with other options, inputs, `params` or `client`.",
          "properties": {
            "kind": {
              "const": "input",
              "type": "string"
            },
            "name": {
              "type": "string"
            }
          },
          "required": [
            "kind",
            "name"
          ],
          "type": "object"
        }
      ]
    },
    "CleanPlan": {
      "description": "What `clean` removes from the store, or removed.",
      "properties": {
        "store": {
          "$ref": "#/$defs/GcPlan",
          "description": "The snapshot records and the objects that nothing reaches, and what stays."
        }
      },
      "required": [
        "store"
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
          "description": "A command that asks has no terminal to ask in, and no `--yes`.",
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
          "description": "A credential is missing: a fetch failed without the credential of its source, or the R2\nvariables are not set. Or a layer cannot be planned, the target cannot take the command, or\nthis machine cannot do the work.",
          "type": "string"
        },
        {
          "const": "r2_failed",
          "description": "R2 or rclone failed, or refused a key.",
          "type": "string"
        },
        {
          "const": "verify_failed",
          "description": "A release failed its check before a target serves it, or after an upload the object in the\nbucket is not the file.",
          "type": "string"
        },
        {
          "const": "run_failed",
          "description": "A run failed: the build, or the run that `runs RUN --follow` shows.",
          "type": "string"
        },
        {
          "const": "failed",
          "description": "The store or the file system failed.",
          "type": "string"
        }
      ]
    },
    "Collected": {
      "description": "What a collection did.",
      "properties": {
        "bytes": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "deferred": {
          "description": "The releases that `release.json` does not name, which stay for now.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "keys": {
          "description": "The keys that it deleted, also of files that no release names, and their size.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "releases": {
          "description": "The releases whose files and manifest it deleted.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "stopped": {
          "description": "Why it stopped before the end: a pointer that is not of the serving release, a manifest that\nit cannot read, or an error. `null` when it finished.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "releases",
        "deferred",
        "keys",
        "bytes",
        "stopped"
      ],
      "type": "object"
    },
    "Collects": {
      "properties": {
        "bytes": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "keys": {
          "description": "The keys of the release that the serving release does not name, and their size: what the\ncollection deletes at most.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "release": {
          "type": "string"
        }
      },
      "required": [
        "release",
        "keys",
        "bytes"
      ],
      "type": "object"
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
    "CredentialState": {
      "additionalProperties": false,
      "properties": {
        "name": {
          "type": "string"
        },
        "needed_for": {
          "type": "string"
        },
        "state": {
          "$ref": "#/$defs/Readiness"
        }
      },
      "required": [
        "name",
        "needed_for",
        "state"
      ],
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
        "phase": {
          "anyOf": [
            {
              "$ref": "#/$defs/Phase"
            },
            {
              "type": "null"
            }
          ]
        },
        "published": {
          "items": {
            "$ref": "#/$defs/Publication"
          },
          "type": "array"
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
        "phase",
        "published",
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
    "Downloads": {
      "properties": {
        "bytes": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "files": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "files",
        "bytes"
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
        },
        "run": {
          "type": [
            "string",
            "null"
          ]
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
        "client_bytes": {
          "description": "The size of the files that a client reads: what a target gets.",
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
        "client_bytes",
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
              "const": "phase",
              "type": "string"
            },
            "phase": {
              "$ref": "#/$defs/Phase"
            }
          },
          "required": [
            "event",
            "phase"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "properties": {
            "event": {
              "const": "published",
              "type": "string"
            },
            "mutation": {
              "$ref": "#/$defs/Publication"
            }
          },
          "required": [
            "event",
            "mutation"
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
            "acquisition": true,
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
            "resolved": {
              "type": "string"
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
            "resolved",
            "bytes",
            "wall_ms"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "properties": {
            "acquisition": true,
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
    "Field": {
      "oneOf": [
        {
          "const": "format",
          "description": "The number that the readers check.",
          "type": "string"
        },
        {
          "const": "revision",
          "description": "The number of the output.",
          "type": "string"
        }
      ]
    },
    "FormatChange": {
      "properties": {
        "from": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "to": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "from",
        "to"
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
        "partial_bytes": {
          "description": "The size of `partial/`: unfinished downloads and the work of steps that stopped. A\ncollection removes work that has no retained acquisition journal.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
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
        "keep_bytes",
        "partial_bytes"
      ],
      "type": "object"
    },
    "GeofabrikArea": {
      "properties": {
        "bounds": {
          "$ref": "#/$defs/Bbox"
        },
        "countries": {
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
        "parent": {
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "id",
        "name",
        "parent",
        "countries",
        "bounds"
      ],
      "type": "object"
    },
    "Holder": {
      "additionalProperties": false,
      "description": "The run that holds the lock of a target.",
      "properties": {
        "heartbeat": {
          "description": "The time of the last heartbeat, `YYYY-MM-DDTHH:MM:SSZ`.",
          "type": "string"
        },
        "machine": {
          "description": "`OBC_DATA_MACHINE`, or else the short host name.",
          "type": "string"
        },
        "phase": {
          "$ref": "#/$defs/Phase"
        },
        "review": {
          "description": "The id of the review that the run does; `null` until the run has it.",
          "type": [
            "string",
            "null"
          ]
        },
        "run": {
          "type": "string"
        }
      },
      "required": [
        "machine",
        "run",
        "review",
        "phase",
        "heartbeat"
      ],
      "type": "object"
    },
    "InputKind": {
      "enum": [
        "snapshot",
        "step"
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
          "default": [],
          "description": "Exact resolved snapshot file names, including an empty read. For a step input, the\nselected paths, or none for every file. The digest names the bytes in the key.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "kind": {
          "$ref": "#/$defs/InputKind"
        },
        "name": {
          "description": "The source id or the step name.",
          "type": "string"
        }
      },
      "required": [
        "kind",
        "name",
        "digest",
        "files"
      ],
      "type": "object"
    },
    "Job": {
      "additionalProperties": false,
      "properties": {
        "name": {
          "type": "string"
        },
        "reason": {
          "type": "string"
        },
        "state": {
          "$ref": "#/$defs/Readiness"
        }
      },
      "required": [
        "name",
        "state",
        "reason"
      ],
      "type": "object"
    },
    "Kept": {
      "description": "A snapshot record, the layers of one step, or the objects that one kind of root names and no\nkept record or layer has.",
      "properties": {
        "because": {
          "description": "`served by TARGET, …`, `newest of the source`, `newest of a request`, `refreshed`, `inputs\nkept`, `release`, `fixture` or `planner recipe`.",
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
    "LayerChange": {
      "description": "A layer that the update changes.",
      "properties": {
        "cause": {
          "$ref": "#/$defs/PlanCause"
        },
        "drops": {
          "description": "The steps of the serving release that no step makes now.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "format": {
          "anyOf": [
            {
              "$ref": "#/$defs/FormatChange"
            },
            {
              "type": "null"
            }
          ],
          "description": "The format of the serving release and of the steps, when it changes. Old readers keep what\nthey have and get no new data until they update."
        },
        "layer": {
          "type": "string"
        },
        "leaves": {
          "description": "The grid leaves whose steps bake, as `<i>-<j>`: the last part of the name of each step of a\ntiled layer that has that form.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "readers": {
          "description": "The programs that read the layer.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "steps": {
          "description": "The steps that bake.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "text": {
          "type": "string"
        }
      },
      "required": [
        "layer",
        "cause",
        "text",
        "steps",
        "leaves",
        "drops",
        "readers",
        "format"
      ],
      "type": "object"
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
    "LayerState": {
      "additionalProperties": false,
      "properties": {
        "cause": {
          "anyOf": [
            {
              "$ref": "#/$defs/Cause"
            },
            {
              "type": "null"
            }
          ],
          "description": "Why a `stale` layer is stale; `null` for another state."
        },
        "layer": {
          "type": "string"
        },
        "state": {
          "$ref": "#/$defs/Verdict"
        },
        "text": {
          "description": "The cause in a few words; `null` without a cause.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "layer",
        "state",
        "cause",
        "text"
      ],
      "type": "object"
    },
    "Listening": {
      "description": "What `serve` writes when it listens.",
      "properties": {
        "network": {
          "description": "Its origin on the network of this machine, for a phone; `null` without a network.",
          "type": [
            "string",
            "null"
          ]
        },
        "target": {
          "type": "string"
        },
        "url": {
          "description": "The origin of `obc data serve` on this machine.",
          "type": "string"
        }
      },
      "required": [
        "target",
        "url",
        "network"
      ],
      "type": "object"
    },
    "Machine": {
      "additionalProperties": false,
      "description": "The machine that runs the command.",
      "properties": {
        "colour": {
          "description": "The header colour of its page: `data/machines.toml`, or else the charcoal of a laptop.",
          "type": "string"
        },
        "cores": {
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        },
        "memory_bytes": {
          "description": "`null` where the system does not tell.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "name": {
          "description": "`OBC_DATA_MACHINE`, or else the short host name.",
          "type": "string"
        },
        "os": {
          "description": "`linux` or `macos`.",
          "type": "string"
        },
        "store_free_bytes": {
          "description": "The free bytes of the disk of the store; `null` where the system does not tell.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        }
      },
      "required": [
        "name",
        "colour",
        "os",
        "cores",
        "memory_bytes",
        "store_free_bytes"
      ],
      "type": "object"
    },
    "MachineCheck": {
      "description": "This machine against the cost.",
      "properties": {
        "memory_bytes": {
          "description": "`null` where the system does not tell.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "refusals": {
          "description": "Why this machine cannot do the update; empty when it can, or cannot tell.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "store_free_bytes": {
          "description": "The free bytes of the disk of the store; `null` where the system does not tell.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        }
      },
      "required": [
        "memory_bytes",
        "store_free_bytes",
        "refusals"
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
    "Observation": {
      "properties": {
        "checked_at": {
          "description": "None for policy-derived capture results and sources without a probe.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "last_success": {
          "anyOf": [
            {
              "$ref": "#/$defs/Success"
            },
            {
              "type": "null"
            }
          ]
        },
        "result": {
          "$ref": "#/$defs/Upstream"
        }
      },
      "required": [
        "checked_at",
        "result",
        "last_success"
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
    "Overview": {
      "description": "What `obc data` writes.",
      "properties": {
        "machine": {
          "$ref": "#/$defs/Machine"
        },
        "targets": {
          "description": "One per target of `data/targets.toml`, by name.",
          "items": {
            "$ref": "#/$defs/TargetState"
          },
          "type": "array"
        }
      },
      "required": [
        "machine",
        "targets"
      ],
      "type": "object"
    },
    "Phase": {
      "enum": [
        "prepare",
        "download",
        "build",
        "verify",
        "upload",
        "switch",
        "collect"
      ],
      "type": "string"
    },
    "PlanBuild": {
      "additionalProperties": false,
      "properties": {
        "again": {
          "description": "Whether the step builds also when the store has the layer of its key.",
          "type": "boolean"
        },
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
        "estimate",
        "again"
      ],
      "type": "object"
    },
    "PlanCause": {
      "description": "Why a group changes a release.",
      "oneOf": [
        {
          "additionalProperties": false,
          "description": "The layer is stale.",
          "properties": {
            "cause": {
              "$ref": "#/$defs/Cause"
            },
            "kind": {
              "const": "stale",
              "type": "string"
            }
          },
          "required": [
            "kind",
            "cause"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "description": "`--only` names the layer: its steps build again.",
          "properties": {
            "kind": {
              "const": "named",
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
          "description": "The release has the layer, and no step makes it now.",
          "properties": {
            "kind": {
              "const": "off",
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
          "description": "A step that bakes reads files of the layer that no client reads, so the target does not hold\nthem, and the store of this machine lacks them.",
          "properties": {
            "kind": {
              "const": "unheld",
              "type": "string"
            }
          },
          "required": [
            "kind"
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
    "Publication": {
      "description": "A change of a target.",
      "oneOf": [
        {
          "additionalProperties": false,
          "properties": {
            "kind": {
              "const": "switched",
              "type": "string"
            },
            "release": {
              "type": "string"
            }
          },
          "required": [
            "kind",
            "release"
          ],
          "type": "object"
        }
      ]
    },
    "Readiness": {
      "oneOf": [
        {
          "enum": [
            "ready",
            "missing"
          ],
          "type": "string"
        },
        {
          "const": "optional",
          "description": "Missing, but no main job needs it.",
          "type": "string"
        }
      ]
    },
    "Receipt": {
      "additionalProperties": false,
      "description": "The record of what one step made: its key, its files, and what building it cost.",
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
        "format": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
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
        "layer": {
          "type": "string"
        },
        "metrics": {
          "additionalProperties": true,
          "type": "object"
        },
        "options": true,
        "peak_rss_bytes": {
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "revision": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
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
        "layer",
        "step",
        "format",
        "revision",
        "key",
        "inputs",
        "options",
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
      "anyOf": [
        {
          "maximum": 65535,
          "minimum": 1,
          "type": "integer"
        },
        {
          "const": "manual"
        }
      ],
      "description": "How old the serving version may get, in days, before the source is stale; `manual` is never stale."
    },
    "Refreshed": {
      "description": "What `source refresh` did.",
      "properties": {
        "bytes": {
          "description": "The size of its files.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "from": {
          "description": "The versions that the targets served: a target that serves one of them, or none, moves.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "source": {
          "type": "string"
        },
        "version": {
          "description": "Upstream's newest version, now in the store.",
          "type": "string"
        }
      },
      "required": [
        "source",
        "from",
        "version",
        "bytes"
      ],
      "type": "object"
    },
    "Region": {
      "oneOf": [
        {
          "description": "The selected Geofabrik source paths, independent of the saved region id.",
          "properties": {
            "areas": {
              "items": {
                "type": "string"
              },
              "type": "array"
            },
            "kind": {
              "const": "geofabrik",
              "type": "string"
            }
          },
          "required": [
            "kind",
            "areas"
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
    "RequestStatus": {
      "properties": {
        "age_days": {
          "format": "int64",
          "type": [
            "integer",
            "null"
          ]
        },
        "observation": {
          "$ref": "#/$defs/Observation"
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
        "reason": {
          "type": [
            "string",
            "null"
          ]
        },
        "serving": {
          "description": "The version that the serving release reads.",
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
        "params",
        "serving",
        "observation",
        "state",
        "reason",
        "age_days"
      ],
      "type": "object"
    },
    "Review": {
      "description": "The review of an update.",
      "properties": {
        "after": {
          "$ref": "#/$defs/AfterUpdate"
        },
        "args": {
          "description": "The arguments after `obc data` that start the update of this review.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "bakes": {
          "description": "The steps that this machine bakes. A step whose output is in the store already is not here.",
          "items": {
            "$ref": "#/$defs/PlanBuild"
          },
          "type": "array"
        },
        "base": {
          "description": "The release that the target serves; `null` while it serves nothing.",
          "type": [
            "string",
            "null"
          ]
        },
        "cost": {
          "$ref": "#/$defs/ReviewCost"
        },
        "downloads": {
          "$ref": "#/$defs/Downloads",
          "description": "What this machine downloads from the target: copies of source files, and files of kept steps\nthat a step that bakes reads."
        },
        "fetches": {
          "description": "What this machine fetches into its store.",
          "items": {
            "$ref": "#/$defs/PlanFetch"
          },
          "type": "array"
        },
        "id": {
          "description": "The first 12 hex digits of the SHA-256 of the canonical JSON of the decisions: the target,\nthe tag of `release.json`, the settings, `layers`, every step that bakes and the source version\nthat each of them reads. Two machines that read the same target give the same id when their\nstores resolve the same versions and files.",
          "type": "string"
        },
        "kept": {
          "description": "The number of outputs that the release keeps by reference.",
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        },
        "layers": {
          "description": "Each layer that the update bakes or drops, in dependency order. A layer that only other\nlayers read is not listed: the layers that read it are.",
          "items": {
            "$ref": "#/$defs/LayerChange"
          },
          "type": "array"
        },
        "machine": {
          "$ref": "#/$defs/MachineCheck"
        },
        "settings": {
          "$ref": "#/$defs/Settings"
        },
        "sources": {
          "description": "Each fetch that a baked step reads at another version than the serving release.",
          "items": {
            "$ref": "#/$defs/SourceMove"
          },
          "type": "array"
        },
        "target": {
          "type": "string"
        }
      },
      "required": [
        "id",
        "target",
        "base",
        "settings",
        "layers",
        "sources",
        "kept",
        "fetches",
        "bakes",
        "downloads",
        "cost",
        "machine",
        "after",
        "args"
      ],
      "type": "object"
    },
    "ReviewCost": {
      "properties": {
        "peak_rss_bytes": {
          "description": "The highest estimated peak of one bake that runs in its own process.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "process_peak_rss_bytes": {
          "description": "The highest peak of the `obc` process in the estimates of the bakes that run in it: an upper\nbound, not the peak of a step. It refuses nothing.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "store_bytes": {
          "description": "What the fetches, the downloads and the bakes write into the store.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "unknown": {
          "$ref": "#/$defs/Unknown",
          "description": "What the figures lack."
        },
        "upload_bytes": {
          "description": "What goes to the target: the files of the bakes that a client reads, and the copies under\n`sources/` that the serving release does not name.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "wall_ms": {
          "description": "The estimated times of the bakes, added as if one ran after another.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "wall_ms",
        "peak_rss_bytes",
        "process_peak_rss_bytes",
        "store_bytes",
        "upload_bytes",
        "unknown"
      ],
      "type": "object"
    },
    "Role": {
      "additionalProperties": false,
      "description": "A release and the time it took its role, `YYYY-MM-DDTHH:MM:SSZ`.",
      "properties": {
        "id": {
          "type": "string"
        },
        "since": {
          "type": "string"
        }
      },
      "required": [
        "id",
        "since"
      ],
      "type": "object"
    },
    "RolledBack": {
      "description": "What a rollback did.",
      "properties": {
        "rollback": {
          "description": "The release that it served before.",
          "type": "string"
        },
        "run": {
          "type": "string"
        },
        "serving": {
          "description": "The release that the target serves now: its rollback before.",
          "type": "string"
        },
        "target": {
          "type": "string"
        }
      },
      "required": [
        "run",
        "target",
        "serving",
        "rollback"
      ],
      "type": "object"
    },
    "RunFetch": {
      "additionalProperties": false,
      "properties": {
        "acquisition": true,
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
        "resolved": {
          "type": [
            "string",
            "null"
          ]
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
        "resolved",
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
    "Serving": {
      "additionalProperties": false,
      "description": "`release.json`: the release that a target serves, and the one that a rollback switches to.",
      "properties": {
        "rollback": {
          "anyOf": [
            {
              "$ref": "#/$defs/Role"
            },
            {
              "type": "null"
            }
          ]
        },
        "serving": {
          "$ref": "#/$defs/Role"
        }
      },
      "required": [
        "serving",
        "rollback"
      ],
      "type": "object"
    },
    "Settings": {
      "additionalProperties": false,
      "properties": {
        "definitions": {
          "additionalProperties": {
            "type": "string"
          },
          "description": "Full TOML definitions of the region and its union members, keyed by region id.",
          "type": "object"
        },
        "layers": {
          "description": "The optional layers that are on, sorted.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "refresh": {
          "additionalProperties": {
            "$ref": "#/$defs/Refresh"
          },
          "description": "Source id to the policy that replaces the policy of `data/sources.toml`.",
          "type": "object"
        },
        "region": {
          "description": "The id of the region; empty until an update names one.",
          "type": "string"
        }
      },
      "required": [
        "region",
        "definitions",
        "layers",
        "refresh"
      ],
      "type": "object"
    },
    "Setup": {
      "additionalProperties": false,
      "description": "What `setup` writes.",
      "properties": {
        "credentials": {
          "description": "Only whether each credential is set; never its value.",
          "items": {
            "$ref": "#/$defs/CredentialState"
          },
          "type": "array"
        },
        "jobs": {
          "description": "What this machine can do, and the reason when it cannot.",
          "items": {
            "$ref": "#/$defs/Job"
          },
          "type": "array"
        },
        "store": {
          "$ref": "#/$defs/StoreUse"
        },
        "tools": {
          "items": {
            "$ref": "#/$defs/Tool"
          },
          "type": "array"
        }
      },
      "required": [
        "store",
        "jobs",
        "tools",
        "credentials"
      ],
      "type": "object"
    },
    "SourceMove": {
      "properties": {
        "from": {
          "description": "`null` when the serving release does not read the fetch.",
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
        "to": {
          "type": "string"
        }
      },
      "required": [
        "source",
        "params",
        "from",
        "to"
      ],
      "type": "object"
    },
    "SourceRow": {
      "description": "A source of `data/sources.toml`, what each target reads of it, and its versions in the store.",
      "properties": {
        "areas": {
          "description": "The Geofabrik areas (`geofabrik-poly`) whose polygons hold the data inside `extent`. None\nmeans all of `extent`.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "attribution": {
          "type": [
            "string",
            "null"
          ]
        },
        "copy": {
          "description": "Whether a target keeps a copy of the files of the source that its release reads.",
          "type": "boolean"
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
        "credential_missing": {
          "type": "boolean"
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
        "redistribute": {
          "type": "boolean"
        },
        "refresh": {
          "$ref": "#/$defs/Refresh",
          "default": "manual"
        },
        "snapshots": {
          "description": "The versions in the local store, the one fetched last first.",
          "items": {
            "$ref": "#/$defs/Stored"
          },
          "type": "array"
        },
        "start": {
          "description": "The version that a step list reads while no serving release reads the source. Without it,\nthe first fetch takes the newest version upstream.",
          "type": [
            "string",
            "null"
          ]
        },
        "targets": {
          "additionalProperties": {
            "$ref": "#/$defs/SourceTarget"
          },
          "description": "By target: its policy, and the state of each request that its serving release reads.",
          "type": "object"
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
        "copy",
        "targets",
        "snapshots",
        "credential_missing"
      ],
      "type": "object"
    },
    "SourceTarget": {
      "properties": {
        "refresh": {
          "$ref": "#/$defs/Refresh"
        },
        "requests": {
          "items": {
            "$ref": "#/$defs/RequestStatus"
          },
          "type": "array"
        },
        "state": {
          "$ref": "#/$defs/State",
          "description": "The least state of its requests; `unused` when the serving release reads none."
        }
      },
      "required": [
        "refresh",
        "state",
        "requests"
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
      "description": "The state of a source. The least of the states of its requests is the one to show for all of\nthem.",
      "oneOf": [
        {
          "enum": [
            "stale",
            "blocked",
            "ok"
          ],
          "type": "string"
        },
        {
          "const": "unused",
          "description": "No step of the target reads the source.",
          "type": "string"
        }
      ]
    },
    "StoreUse": {
      "additionalProperties": false,
      "properties": {
        "free_bytes": {
          "description": "`null` where the system does not tell.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "path": {
          "type": "string"
        },
        "used_bytes": {
          "description": "The size of every file in the store.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "path",
        "used_bytes",
        "free_bytes"
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
    "Success": {
      "properties": {
        "checked_at": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "version": {
          "type": "string"
        }
      },
      "required": [
        "checked_at",
        "version"
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
    "TargetState": {
      "properties": {
        "at": {
          "description": "The folder of a dir target, or the public URL of an R2 bucket.",
          "type": "string"
        },
        "blocked": {
          "description": "The steps that cannot be planned.",
          "items": {
            "$ref": "#/$defs/BlockedLayer"
          },
          "type": "array"
        },
        "drift": {
          "description": "The pointer keys whose bytes are not those of the serving release.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "error": {
          "description": "Why the target or its steps could not be read; `null` when they could.",
          "type": [
            "string",
            "null"
          ]
        },
        "fix": {
          "description": "What to do about `error`.",
          "type": [
            "string",
            "null"
          ]
        },
        "kind": {
          "description": "`dir` or `r2`, as `data/targets.toml` says.",
          "type": "string"
        },
        "layers": {
          "description": "Each layer of the steps and of the serving release; `null` while the target serves nothing\nor its steps cannot be listed.",
          "items": {
            "$ref": "#/$defs/LayerState"
          },
          "type": [
            "array",
            "null"
          ]
        },
        "lock": {
          "anyOf": [
            {
              "$ref": "#/$defs/Holder"
            },
            {
              "type": "null"
            }
          ],
          "description": "The run that holds the lock of the target; `null` while it is free."
        },
        "next": {
          "description": "The command to run next; `null` when the target serves its settings as the steps make them\nnow, or when no command of this `obc data` writes the target: an R2 bucket.",
          "type": [
            "string",
            "null"
          ]
        },
        "region": {
          "description": "The region of the serving release.",
          "type": [
            "string",
            "null"
          ]
        },
        "release": {
          "anyOf": [
            {
              "$ref": "#/$defs/Serving"
            },
            {
              "type": "null"
            }
          ],
          "description": "What `release.json` of the target says; `null` while it serves nothing."
        },
        "target": {
          "type": "string"
        }
      },
      "required": [
        "target",
        "kind",
        "at",
        "release",
        "lock",
        "region",
        "layers",
        "blocked",
        "drift",
        "error",
        "fix",
        "next"
      ],
      "type": "object"
    },
    "Tool": {
      "additionalProperties": false,
      "properties": {
        "detail": {
          "description": "The version that the tool gives, or the command that installs it; both when the version is\ntoo old.",
          "type": "string"
        },
        "name": {
          "type": "string"
        },
        "needed_for": {
          "type": "string"
        },
        "state": {
          "$ref": "#/$defs/Readiness"
        }
      },
      "required": [
        "name",
        "needed_for",
        "state",
        "detail"
      ],
      "type": "object"
    },
    "Unknown": {
      "properties": {
        "estimate": {
          "description": "The bakes without an estimate.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "peak": {
          "description": "The bakes whose estimate has no peak.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "size": {
          "description": "The fetches whose size is not known, as `SOURCE@VERSION`.",
          "items": {
            "type": "string"
          },
          "type": "array"
        }
      },
      "required": [
        "estimate",
        "peak",
        "size"
      ],
      "type": "object"
    },
    "Unlocked": {
      "description": "What `unlock` did.",
      "properties": {
        "holder": {
          "$ref": "#/$defs/Holder",
          "description": "The run that held the lock."
        },
        "target": {
          "type": "string"
        }
      },
      "required": [
        "target",
        "holder"
      ],
      "type": "object"
    },
    "Updated": {
      "description": "What an update did.",
      "properties": {
        "baked": {
          "description": "The steps that the run baked or found in the store, in dependency order.",
          "items": {
            "$ref": "#/$defs/Baked"
          },
          "type": "array"
        },
        "collection": {
          "anyOf": [
            {
              "$ref": "#/$defs/Collected"
            },
            {
              "type": "null"
            }
          ],
          "description": "What the collection after the switch did; `null` without a switch."
        },
        "kept": {
          "description": "The number of outputs that the release keeps by reference.",
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        },
        "review": {
          "description": "The id of the review that the update did.",
          "type": "string"
        },
        "rollback": {
          "description": "The release that it served before; `null` when it served nothing or the update changed\nnothing.",
          "type": [
            "string",
            "null"
          ]
        },
        "run": {
          "type": "string"
        },
        "serving": {
          "description": "The release that the target serves now.",
          "type": "string"
        },
        "target": {
          "type": "string"
        }
      },
      "required": [
        "run",
        "target",
        "review",
        "serving",
        "rollback",
        "baked",
        "kept",
        "collection"
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
    "Upstream": {
      "oneOf": [
        {
          "properties": {
            "state": {
              "const": "newest",
              "type": "string"
            },
            "value": {
              "type": "string"
            }
          },
          "required": [
            "state",
            "value"
          ],
          "type": "object"
        },
        {
          "description": "The service captures current data on demand; no network probe establishes a version.",
          "properties": {
            "state": {
              "const": "capture",
              "type": "string"
            }
          },
          "required": [
            "state"
          ],
          "type": "object"
        },
        {
          "description": "The source has no cheap upstream probe.",
          "properties": {
            "state": {
              "const": "cannot_check",
              "type": "string"
            }
          },
          "required": [
            "state"
          ],
          "type": "object"
        },
        {
          "properties": {
            "state": {
              "const": "failed",
              "type": "string"
            },
            "value": {
              "type": "string"
            }
          },
          "required": [
            "state",
            "value"
          ],
          "type": "object"
        }
      ]
    },
    "Verdict": {
      "oneOf": [
        {
          "const": "ok",
          "description": "The release has the layer as its steps make it now.",
          "type": "string"
        },
        {
          "const": "off",
          "description": "No step makes the layer: an optional layer that is off, or a layer that only the release has.",
          "type": "string"
        },
        {
          "const": "stale",
          "description": "A build changes the layer.",
          "type": "string"
        }
      ]
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
