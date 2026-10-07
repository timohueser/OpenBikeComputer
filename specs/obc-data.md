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
| `start` | string | no | The version that a plan reads while no live release reads the source. It has the form of `version` |
| `refresh` | integer or string | yes | `1` through `65535` whole days, or `"manual"` |
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
`builder/web/vite/third-party-licenses.ts`. The web planner, the map builder and the iOS planner show the
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
| `kind` | string | `geofabrik`, `box` or `union` |
| `box` | array of 4 numbers | Only for `box`: west, south, east, north in degrees, longitude first |
| `areas` | array of strings | Only for `geofabrik`: one or more source paths. Paths are sorted and duplicates removed |
| `union` | array of strings | Only for `union`: two or more region ids |
| `countries` | array of strings | The ISO 3166-1 alpha-2 codes of the countries in the region, such as `DE`. The first is the country of a place outside each country polygon |
| `time_zone` | string | The IANA time zone of the region, such as `Europe/Berlin` |

The planner needs `countries` and `time_zone`. An edit that selects a region without them, and
`dev` with such a region, fail before a fetch.

A box has longitude in −180…180 and latitude in −90…90, with west < east and south < north.
A box that crosses the antimeridian is refused. The order of the numbers is checked only
through these ranges: a latitude-first box is refused only when one of its longitudes is
outside −90…90.

A `geofabrik` region selects source paths such as `europe/germany/baden-wuerttemberg`.
Its saved id is independent of those paths. Creation writes one definition and no child files.
Source paths use the same id syntax. An empty selection is refused. Polygon-file regions are refused.
A union resolves to the regions in it that are not unions. A union that contains itself,
or names a region that does not exist, is refused. Unions can be read but not created through the API.

Creation requires countries and an explicit IANA time zone. Selected source areas derive country
codes from the cached index or its parents. `--country` supplies missing metadata; boxes require
it. The offline Python runtime validates the time zone with `ZoneInfo`. Listing does not need Python.
Area search reads the newest cached public index and verifies its object hash and size. It does
not fetch the index. Suggestions expose names, paths, countries and bounds, without polygon data.

Deletion previews the definition SHA-256 and its references. An environment, fixture, planner
recipe or another region that names it prevents deletion. `--apply --expected SHA` must match
the preview. References and the definition are checked again after confirmation. Deletion removes
only the definition; source snapshots, releases and baked files stay. Creation and deletion never commit.

The bakes read this directory:

| Reader | Regions |
| --- | --- |
| `obc-bake` (device maps) | Single-area definitions whose saved id equals the source path. `--regions DIR` reads another directory with this layout; `obc bake` passes the checkout's directory |
| Planner bake | The `box` region with the id of the recipe in `tools/planner-regions/`: its `name` and its box |
| `obc data`, product `planner` | A single-area definition, its `countries` and its `time_zone` |
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

`obc data sources` discovers the active acquisition requests from the current product step lists.
Discovery can read small region metadata. It does not fetch bulk inputs. A held capture input is
provenance, not an active acquisition request. Request identity is the source id and sorted
`NAME=VALUE` pairs. The source row reports the least request state in the state order.

| State | When |
| --- | --- |
| `blocked` | A `data` or `asset` source has no `licence`, an active request has conflicting live versions, or a due request has no successful upstream result |
| `stale` | An active date request is older than its maximum data age and upstream names a later version, or the request is an on-demand capture. Or its live version is before the live version of the source that `fetch.from` names |
| `unused` | Live is known, and no live layer and no active request reads the source |
| `ok` | Otherwise. A request without a live version is never stale. A `manual` source is never stale by age |

Data age is the number of days from the live version date to today (UTC). A request is due only
when its age is greater than `refresh`. An unchanged successful check does not change data age:
the request stays due, but its state is `ok` until a later check finds a newer version. Probe
cache age is separate from data age.

Each request reports its params, live version, due flag, state, age and upstream observation.
The observation reports the result, the latest probe time and the last successful probe time
and version. A failed probe keeps the last success. A capture result has no probe time: current
capture policy is not evidence of a network check. The source row reports missing credentials,
but credentials block only a selected new fetch. Verified local bytes or retained copies need
no upstream credential. The live column also lists held versions for provenance.

`versions SOURCE` reports the active request params, Live pin, stored request record versions
and current upstream result. Its common versions are the intersection of these known versions
across all active requests. Stored records do not prove that their object bytes remain present.
The newest-per-request choice needs a successful current version observation for every request.
Unavailable observations remain in the report. Held provenance is not an active move target.
Terminal version choices retain source-wide `--move` intent until Plan resolves the exact reads;
only the reviewed plan admits a build or apply.

## Store

The store is the directory in `OBC_DATA_STORE`, or else `~/.cache/openbikecomputer/store/`.

| Path | Holds |
| --- | --- |
| `objects/<ab>/<sha256>` | One file, named by the lowercase hex SHA-256 of its bytes; `<ab>` is its first two characters. Read-only |
| `snapshots/<source>/<version>.json` | The snapshot record of one source version |
| `layers/<key>.json` | The receipt of the layer with that key, see [Layers](#layers) |
| `releases/<product>/<id>.json` | The manifest of a release, see [Releases](#releases) |
| `code/<hash>.json` | The code files of a code hash: `{path: sha256}`. A run writes it for each step that it reads or builds |
| `producers/<hash>.json` | The full code fingerprints, source/config fingerprints and resolved Rust target/profile from the same traversal. The full fingerprints must hash to `<hash>` |
| `local/<product>.json` | One saved portable adoption: its original release and exact selected files. It remains a collection root while apps are stopped |
| `requests/<source>/<sha256>.json` | The files that a fetch with `NAME=VALUE` gave: `version`, `params` and `files` (names). The name is the SHA-256 of the compact JSON `[version, params]`, with `params` sorted. A record with no files selects no file |
| `runs/<id>.jsonl` | The events of one run, see [Runs](#runs) |
| `upstream/<source>/<sha256>.json` | The acquisition check descriptors and observation of one normalized request. The name is the SHA-256 of compact JSON sorted params. The observation has `checked_at` (UTC seconds or `null`), `result` and `last_success` (UTC seconds and version, or `null`). The result has `state`: `newest` with `value`, `failed` with `value`, `capture`, or `cannot_check` |
| `partial/` | Downloads that are not complete, the validators that resume them, and the layers that steps write. A collection empties it |
| `locks/` | One lock file per key and per run |

Rules:

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

The collection deletes what no live release, saved Local adoption or fixture reaches. Its roots are the live
releases (see [Live](#live)), the files of the checkout that it runs in, and the store:

- A snapshot record is reached when a layer of a live release read its source and version. The
  newest record of each source, by the latest `retrieved` of its files, is also reached: a plan
  reads it when nothing else names a version (see [Versions](#versions)), and a source whose
  upstream gives only its newest file cannot give it again. So is the newest version of each request record (`requests/`), by the
  latest `retrieved` of its files, such as the extract of each Geofabrik area.
- An object is reached when a reached snapshot record or a reached layer has it, or when its
  SHA-256 is a file of a live layer, or is in `fixtures/catalog.toml`, a JSON or TOML file below `fixtures/sources/` or a
  planner region recipe in `tools/planner-regions/`.
- A saved Local adoption roots its selected file SHA-256 values and every source version in
  their transitive original layer provenance. A malformed saved anchor stops collection.
- A layer is reached when each of its inputs is reached: a snapshot input whose digest is the
  digest of the files that it names (`files`) of a reached record of its source, and a layer input
  whose digest is the digest of the files that it selects of a reached layer of its step: the
  `files` of the input, or all files when it names none.

The plan lists what the collection deletes and what stays. What stays is one entry for each
reached snapshot record, with the reasons: `live PRODUCT, …`, `newest of the source`,
`newest of a request` and `local PRODUCT, …`. Then one entry for the reached layers of each step (`inputs kept`). Then
one entry for each kind of root that names objects that no reached record or layer has:
`live release`, `local release`, `fixture` or `planner recipe`. The size of
an entry is the size of its files, and the plan gives the size of `partial/`. The collection
takes the store lock alone, or refuses to start while a mutating run or fetch holds it. A writer run holds it through verification and
publication preparation, until finish or drop. An admitted collection deletes each snapshot record and each object
that is not reached, and empties `partial/`. Receipts, release manifests and upstream checks stay. A
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

A `dtm` or `capture` fetch runs in the explicit repository root. The offline system selector
of `uv python find` selects its Python and honors `UV_PYTHON`. A standard-library capture
runs that interpreter directly. A package capture uses `uv run --locked --offline
--no-default-groups --no-python-downloads --group GROUP python`, bound to that interpreter.
It prepares the locked group when the capture runs. Cached captures need no Python runtime.
The capture checks its code and interpreter before execution and before accepting output.
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
| `code` | `paths`: files and directories, relative to the repository root. `crates`: workspace crates. `target`: a Rust target triple, or `null` for the host. `rust`: native or prepared compiler binding and dev/release profile, or `null` for native dev. `sources`: source content settings. `python`: the selected locked package `group`, or `null`. `python_packages`: a separate locked group without an interpreter binding, or `null`. `libraries`: named native files with absolute paths and expected SHA-256 digests. A Rust step declares the crate of its function |
| `outputs` | Paths in the output directory. A path is a file, or a directory whose files are all part of the layer. The step must write each path and no other file. A symbolic link fails the step |
| `client` | `"none"`, `"all"` or `{"paths": [<output>]}`. Selected paths name declared outputs: a file, or a directory and its files. Paths are sorted and unique. An empty selection or a path outside `outputs` fails the plan |
| `run` | A Rust function in the process, or a command: a program and its arguments. No argument names a path outside the repository root: no argument is an absolute path, contains `=/` or has a `..` segment between `/` and `=` |

One binary links the Rust steps of every product, so Cargo unifies their features. A step crate
enables every feature its bytes depend on itself, or makes its bytes independent of it (structs,
or sorted keys for JSON objects).
A Rust function uses the native dev worker. Prepared and release declarations require a command.

### Keys

The digest of a list of files is the SHA-256 of the text that `sha256sum` writes for them: one
line `<sha256>  <name>` with a final newline per file, in byte order of the names.

- The digest of a snapshot input lists its selected files by `name`.
- The digest of a layer lists its files by `path`. The digest of a layer input lists the files
  that it selects.
- The code hash lists file paths and named dependency fingerprints, each to its SHA-256.
  A path adds the files that `git ls-files --cached --others --exclude-standard` lists for
  it: the files that git tracks or does not ignore. A path that lists no file fails the step,
  and so does a repository root that is not a git checkout. Explicit paths hash the full file.
- A crate adds its `Cargo.toml`, `build.rs` and `src/` the same way. Automatic manifest hashes
  omit top-level and target-specific dev-dependency tables. The resolved normal and build
  dependencies add local crate files, registry checksums or resolved Git revisions. Local
  crates must be inside the checkout. Each selected package adds its name, version, edition,
  resolved package settings and features under `cargo/<name>@<version>#<source>`. Cargo metadata runs locked,
  offline and for `target`, or the native host when unset. The target enters
  `rust/target`. Features use Cargo's workspace resolution. This
  conservative union can rebuild a step when another producer enables a shared feature.
  Dev-only and unrelated packages add no records. The walk stops at the engine crate
  `obc-data`: its selected inputs enter the input digests.
  A prepared compiler binding requires an explicit `target` and a checked builder in the step
  options. It does not use the host compiler as the identity of a target executable.
- Native Rust adds the selected compiler and Cargo executable digests, their verbose versions,
  its compiler and selected-host sysroot libraries, native link driver and linker digests,
  and ordered compiler flags.
  The C compiler enters the identity when the selected closure uses `cc`. Rustup proxies
  resolve to the actual selected executables in the checkout. A native target must match the
  selected compiler host. Both compiler bindings add the selected dev/release profile settings,
  its build override and applicable package overrides. Named package overrides use the selected
  closure; `*` applies only when that closure contains a non-workspace package. Unselected
  profiles and package overrides add no records. Profile inheritance and package-ID overrides
  are refused. Tool discovery and bytes are cached within a checking context. Each execution
  boundary checks tool, library, environment and selection/config witnesses. A change runs
  discovery again and invalidates changed file digests.
- Acquisition and planning declare an owner crate and its source paths. The same resolver
  selects its normal and build dependencies, profiles and execution tools. Owner source uses the
  declared paths. Engine backend source is included, apart from its TUI module.
  Backend CLI edits can change owner identity. Its dependency metadata stays conservative:
  an unused UI dependency edit can change owner identity. Ordinary TUI source edits do not.
- Source/config evidence is separate from full execution identity. It uses the same traversal,
  selected source and package projections, repository profile and toolchain configuration.
  It resolves at the recorded producer target and profile without selecting native tools,
  libraries or a Python interpreter. This evidence does not prove equal recomputation on another
  host. Full execution identity includes a reserved `identity/source-config` digest of this
  projection and the recorded Rust target/profile. A published witness must match that digest.
  Real execution still requires its complete native identity.
- A declared native library or executable enters code identity by its name and complete file digest. Its
  installation path does not enter that identity. Each execution boundary checks the current
  file against the declared digest. Osmium extraction and merge bind the exact canonical executable
  in this identity, outside semantic options. Their requests carry the binding; both callbacks
  check it before and after work. Missing bindings refuse execution. GEOS producers bind the loaded shared C and C++ libraries
  at worker startup. A missing or changed file blocks these producers until a fresh worker
  starts. New captures bind the same two digests. Selector children check the requested capture
  code before selection and check the provider again before success. Held captures retain their
  original inputs and must pass their content checks before reconstruction writes output.
- Native discovery checks Cargo config in the checkout, its ancestors and Cargo home. It
  refuses build overrides except jobs and target directories. Network, registry, terminal and
  alias settings add no byte identity. The supported flags are ordered `--cfg` and explicit
  codegen and lint settings from `CARGO_ENCODED_RUSTFLAGS`, or `RUSTFLAGS` when the former is absent.
  An empty encoded value suppresses `RUSTFLAGS`. Host-dependent `target-cpu=native`, external
  codegen inputs, compiler wrappers and undeclared native build overrides are refused.
  Supported `CARGO_PROFILE_DEV_*` and `CARGO_PROFILE_RELEASE_*` settings, including build
  overrides, enter only their selected profile identity. `CARGO_INCREMENTAL` also enters it.
  Cargo validates these raw values and applies its override precedence.
  Errors identify the setting, not its value. Discovery does not install a toolchain.
- `LD_LIBRARY_PATH` and `DYLD_FALLBACK_LIBRARY_PATH` stay in the worker runtime environment.
  Native tool discovery, Cargo metadata and worker builds clear loader search paths in their
  child environment, including the refused `DYLD_LIBRARY_PATH` priority override. A declared native Rust
  `cargo` or `rustc` command uses the same policy. Ordinary runtime commands, Python commands
  and prepared builders retain their environment. These search paths add no byte identity.
  Preload, injection and link-time library search overrides are refused. A compiler installation
  that needs custom runtime loader paths is unsupported.
- A `.rs` file of a selected crate adds each file that it names in `include_str!`,
  `include_bytes!`, `include!` or `#[path = "…"]`, and an added `.rs` file adds its own.
  The name is a string literal, normal or raw, relative to the file, or a `concat!` of string
  literals, relative to the file or after `env!("CARGO_MANIFEST_DIR")`. These are not code
  unless the step declares them: a name that a literal with an escape, a constant or another
  macro gives; `#[path]` in an inline module; and a file that `build.rs` reads.
- `sources` adds each named source's content settings under `data/sources.toml#<id>`.
  Acquisition, coverage, version scheme and licence settings enter this projection.
  Refresh policy, credentials, host permissions and input-copy controls do not. Unnamed
  sources add no records.
- `python` adds the locked base packages and selected group under `python/packages/<group>`.
  A null group selects only the base packages. `uv export --locked --offline` selects the
  dependency closure without default groups. The sorted requirement records retain package
  sources, versions, hashes and markers. `python/runtime` holds the implementation, full
  version and ABI of the interpreter that `uv python find --system --offline --no-python-downloads`
  selects for the project, with an explicit `UV_PYTHON` request when set. Before execution, the
  engine checks this code identity again and sets the child `UV_PYTHON` to that interpreter.
  The project environment can change the environment location, but not the base interpreter.
  A no-sync override cannot retain an incompatible interpreter. Discovery does not download,
  install or sync. A missing interpreter
  fails with a request to prepare the runtime.
- `python_packages` adds the same locked package projection for a separately packaged group,
  without selecting a host interpreter or preparing its environment.

The key is the SHA-256 of this JSON object, as the compact output of `serde_json` with the keys
of each object in byte order:

| Key | Value |
| --- | --- |
| `step` | The layer name |
| `command` | The program and its arguments, or `null` for a Rust step |
| `inputs` | One `{"kind", "name", "digest"}` per input, sorted by `kind`, then `name`. `kind` is `snapshot` or `layer`; `name` is the source id or the layer name |
| `options` | The semantic options |
| `code` | The code hash |
| `outputs` | The declared outputs, sorted |

An input layer enters a key with its digest, not with its key. A rebuild that gives the same
files gives the same digest, so the keys of the layers that read it do not change, and the
engine reuses them. A snapshot version enters a key the same way, by the digest of its files.

`obc data` builds its private producer worker with locked, offline Cargo. Cargo's JSON artifact
record selects the executable. The launcher checks the compiled Rust closure before and after
the build. Cargo tracks the checkout and code hashes that `option_env!` embeds in the worker.
The launcher copies the executable to a temporary directory, and gives the child its checkout,
code and executable hashes. The worker checks the compiled stamps and these hashes before CLI
or capture selection dispatch. An unstamped private worker refuses to run.
It derives the plan from its fresh product code. Its compiled closure includes the engine and
uses the source content projection. A source refresh policy change does not require a restart.
Credential descriptors remain in the compiled guard because product preflight reads them.
An action in a long-lived worker rejects a persistent Rust source change with a restart message.

The engine checks the whole declared step code before execution and after execution, before it
accepts output objects or a receipt. These checks detect persistent concurrent edits. They do
not run Cargo again while its lock and workspace or local manifests remain unchanged. The
manifest paths come from the resolved metadata. Each check reads current declared source files
and Python identities. The compiled worker is checked around complete plan and run operations.
They do not detect an edit that is restored between checks or provide an immutable source snapshot.
The launcher removes the copied worker after normal child exit, including on Windows. Abrupt
launcher termination does not cancel the worker and can leave its copy in the operating
system's temporary directory. The launch hashes guard accidental stale invocation; they are
not an authentication protocol. External prepared
tools enter through their selected snapshot bytes.

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
| `options` | The semantic options |
| `libraries` | Named native library or executable bindings: `{name, path, sha256}`. They come from the checked Code declaration |
| `output` | An empty directory. The layer is the files that the step writes in it |
| `metrics` | A path. The step can write a JSON object there, for example the size of each section |

A command starts in the repository root. Its standard output and standard error go to the
standard error of the engine. Exit status 0 is success. The objects are read-only. While a step
runs, `output` and `metrics` are in `partial/layer-<key>/`; the engine removes that directory
when the step ends, also when it fails. A failed step writes no receipt. A declared directory may be empty when the step creates it.
A declared output that does not exist is an error.

### Offline

A step reads its data inputs and the checked native providers in its request. A step
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
list. They MUST NOT prepare bulk inputs, including with explicit `--move`. An unresolved graph
sets `needs_prepare`. Explicit `prepare` owns downloads and returns a reviewable `.plan`. A build
uses the normal fetcher.

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
| `pointer` | `pointer:PRODUCT` | None. The desired client document or release id differs without a layer change. `release` is its desired id; `document` is the SHA-256 of its compact JSON with sorted keys |

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
A saved Box or multi-area definition has its own complete catalog pick. Source polygons determine
which edge cells are partial. Missing required map, terrain or article layers block the catalog;
a terrain-only release cannot publish.
Before apply, the product compares picks with the previous release and assembles changed picks
with the real assembler. The production reader verifies each result. Missing inputs and invalid
artifacts fail verification before upload. An unchanged pick can reuse prior verification.

### Runs

A mutating preparation, build or apply owns one run. It starts after argument and consent
preconditions, before authorized input preparation. The caller finishes it once, after its
last phase. An apply keeps the run through release verification, upload, pointer switches,
the retention wait and cleanup. A repair with no layer builds still has a run. A read-only
plan creates none. A failure after the run starts returns its id in `error.run`.

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
| `phase` | `phase`: `prepare`, `build`, `verify`, `upload`, `switch`, `wait` or `cleanup` |
| `published` | `mutation`: an acknowledged upload key, product and release switch, or removed key and size |
| `fetch_started` | `source`, `version` and `params` |
| `fetch_finished` | `source`, `version`, `params`, `resolved` (the acquired version), `bytes` (the size of the files that the fetch gave, downloaded or found in the store) and `wall_ms` |
| `fetch_failed` | `source`, `version`, `params` and `error` |
| `step_started` | `step` |
| `step_finished` | `step`, `reused` and `receipt` |
| `step_failed` | `step` and `error` |
| `finished` | `ok`, `error` (`null` when `ok`) and `wall_ms` |

A discovery fetch without a pin uses `newest` as its request version. Its finished event
records the acquired version separately. `runs RUN` gives the last phase and acknowledged
remote writes, also after a later phase fails. An acknowledged write is not proof that later
verification passes. An interrupted write can have an unknown remote outcome. The ordinary
run journal does not make publication atomic or recover an interrupted commit.

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
3. The `start` of the source, when live reads no version of the fetch.
4. The newest version of the fetch in the store.
5. The newest version upstream: the product names the fetch, and `plan` or `build` fetches it. A
   source whose URL needs a `NAME=VALUE`, such as the GLO-30 tiles, has no one newest version: a
   fetch of it without that value fails with `usage` and the fix `Plan with --move
   SOURCE@VERSION`.

A plan or a build of `live` without `--plan` first discovers active requests with metadata-only
preparation. It selects each stale active request (see [State of a source](#state-of-a-source))
using its own upstream observation of the last hour. Other requests keep their live versions.
Each source remains one group, `move:SOURCE`. A held input, an unused source and a `manual`
source do not move this way. A stale capture requires an explicit move for its collection.

A source with `refresh = "manual"` is never stale: after its first version it moves only with
`--move`. Before the first apply, nothing is live, so a plan takes the `start` of each source, and
else the versions of the store and of upstream. The fetch of a
`--move SOURCE` resolves each normalized request independently. `--move SOURCE@VERSION` fixes
that version for all requests of the source. Fetches never mutate source move intent. A saved
plan records both the intent and each exact resolved request version. Upstream can
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
live release stays. Maps gives a catalog and checks changed selections with the production
assembler and reader. Planner verifies stored grid and runtime artifacts. Its service activation
remains blocked. A product checks a release before an apply makes it live.

Each step selects the files that clients read: the device, the web planner or a service on the
VPS. Other files are intermediate: only other layers read them. R2 holds only selected files
(see [Releases](#releases)). The selection changes the recipe and the release, but not the layer
key: a change in selection reuses the same bytes.

`plan ENV` plans the steps of every product together. It prepares no bulk inputs, also with
`--move`. Small discovery metadata uses the status allowlist. `needs_prepare` is true when
input discovery cannot resolve the graph. Blocked readiness alone does not set it. The
blocked reason names the unavailable input or credential.

`prepare ENV` explicitly acquires inputs that discovery needs. It builds no layers and uploads
nothing. Its JSON is `{run, plan}`. Review and save the nested `plan` object for replay.
Missing credentials or an upstream failure keep their actionable error and the run id.

`--json` writes the plan with `env`,
`region` and `layers` of the environment, `moves`, the version of each source that the plan moves
(a `--move SOURCE` has the version that its fetch gave), `versions`, the version of each fetch that
the step lists read, and `only`, the groups that `--only` selected, `[]` for every group or
`["none"]` for no group. A plan
of `live` also has:

- `live`: per product, the id of its live release, or `null` when nothing is live, and `pointer`,
  the SHA-256 of its actual client document with sorted keys. The comparison excludes only
  `release` and `applied`. `observed` is the SHA-256 of the exact original pointer bytes,
  including publication fields, or `null` after a successful absent read. Read failures give no plan.
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
  `repair` group, `remove` lacks the leftovers, and `bytes` is `null` for a record.

Another environment has `[]` for `live`, `edits` and `remove`, and `false` for `listed`.

`build ENV --plan FILE` builds the groups of that file; it takes no `--only` and no `--move`. Its
step lists read the `versions` of the file and no other version, and it moves the sources of
`moves`. A version that the store lacks is fetched; when that
fetch fails, the command fails with its code, and a fix that says to plan again when the fetch
gives none. It refuses the file, with exit status 3, before it builds:

- when `needs_prepare` is true, before any preparation fetch; prepare and review a new plan;
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

The merge and map crop code binds the prepared Osmium executable SHA-256. Their requests name
its canonical path. They check the binding before and after execution, before accepting output. A persistent tool
replacement refuses the step. A missing tool asks to prepare Osmium or set `OBC_OSMIUM`.
Plans probe local tooling only; they do not download or install it.

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
| `maps/region-osm` | The selected PBF of each `maps/source/<area>`; only for multiple areas | None | `osm.pbf`: the available-snapshot union |
| `maps/osm` | The PBF of one `maps/source/<area>`, or `maps/region-osm` | `leaves`: `[i, j]` of each leaf | `osm/<i>-<j>.osm.pbf`: the `osmium extract --strategy smart --set-bounds` of the square of the leaf and one µdeg around it. The metrics name the version (`osmium`) |
| `maps/<band>/<i>-<j>` | `maps/osm`, the file of the leaf; `land-polygons`; `maps/terrain/<i>-<j>` when the cells of the band read heights: contours in their levels, or a nav graph or POIs | `band`: `coarse`, `mid`, `fine` or `network` of the recommended band table (`OBCA_Spec.md`); `leaf`: `[23, i, j]`; `cells`: `[ci, cj]` of each cell of the band in the leaf that the outline touches | `cells/<band>/<ci>/<cj>.obcm` for each cell with content; `cells/<band>/empty.json`: the ids of the other cells. A cell has the bytes that one cut of the whole leaf with all bands writes, with `builder/presets/schema.json` and without landmarks or peaks |
| `maps/reference/<i>-<j>` | Each `dtm-*` source of the leaf with data, `bbox=<box>` | `models`: `source`, `version` and `credit` (its `attribution`) of each model; `tiles`: the ids `<ti:04>/<tj:04>` of the archive tiles that the terrain cells of the leaf read | `reference/`: the reference archive (`host/obc-dem/reference/README.md`) of the models, which `ingest.py ingest` of each model writes into an empty archive, best first by `PRIORITY`, cut to `tiles`. The `fetched` day of a model is its version. A Python step with the group `terrain-reference` |
| `maps/terrain/<i>-<j>` | `copernicus-glo-30`, `tile=` of each tile that the square of a cell reaches and that `copernicus-glo-30-tiles` names. A square without a tile is sea. A leaf without a tile reads no snapshot. `maps/reference/<i>-<j>` when the leaf has one | `posting_log2` and `cell_log2` of OBCT v1; `cells`: `[ci, cj]` of each terrain cell in the leaf that the outline touches | `terrain/<ci>/<cj>.obcd` for each cell with a height (`OBCC_Spec.md` §13), the bytes that `obc-bake terrain --reference` writes from the same tiles and archive; `terrain/empty.json`: the ids of the cells without a height; `terrain/credits.json`, when a cell reads a national model: `key`, `product`, `attribution` and `licence` of each model that a cell reads, as the reference archive states them |
| `maps/landmark-content[/<area>]`, `maps/peak-content[/<area>]` | `wikidata`, `wikipedia` and `commons`, `collection=landmarks` or `collection=peaks`, `area=<source path>`, `osm=`, `poly=` and `code=`; the file of `geofabrik-extracts` and of `geofabrik-poly` that `osm=` and `poly=` name, by its name and without params | None | `landmarks/content.json` or `peaks/peaks.json`, and the photos: the compile of the capture. The step makes the boundary, and the candidates or the summits, again from the `.poly` and the extract. When they differ from those that the recipe of the capture pinned, the code that makes them changed: the step fails, and the fix is `--move wikidata` |
| `maps/landmarks/<i>-<j>`, `maps/peaks/<i>-<j>` | Every area's landmark content and `maps/osm`, the file of the leaf; or every area's peak content | `cell_log2`: 18; `cells`: `[ci, cj]` of each network cell of the leaf, as for `maps/network/<i>-<j>` | `landmarks/<ci>/<cj>.bin` or `peaks/<ci>/<cj>.bin` for each cell that owns content (`OBCC_Spec.md` §14.3). A landmark joins the OSM objects of the leaf that name it |

`<i>`, `<j>`, `<ci>` and `<cj>` have four digits or more, as in a cell id.

A capture keeps its params while no source of the capture moves: the step list reads the capture
of the region that the saved plan or live reads, or else the newest capture of the region in the
store. A new extract alone therefore asks for no new capture. A missing capture, missing capture inputs or an automatic stale-source move blocks only its
content and artifacts. The reason asks for `--move wikidata`. Status and
plans do not start bulk captures without an explicit move. A capture that moves explicitly
reads the extract and the `.poly` of now. `code=` is the digest of the code that makes the boundary and the
candidates or the summits, so `--move wikidata` after a change of that code asks for a new capture.
A held request keeps its original `code=` even when source text changes. Its selection is
provisional until the offline content step reconstructs the boundary and candidates or summits
and checks their full SHA-256 pins. A mismatch fails before content output or a receipt.

A single-area definition keeps the unsuffixed content layer names. Other definitions have one
content layer per collection and source area. A selection edit can retain a capture when its
collection, exact area and pinned inputs match unambiguous live request metadata. It never
chooses held inputs by a layer name suffix alone. Article inputs keep separate file views;
canonical content merge rules deduplicate overlaps without losing unique articles.

`maps/source/<area>`, `maps/region-osm`, `maps/osm`, `maps/reference/<i>-<j>` and all content
layers are private intermediates. Their complete records stay in release provenance.

#### `planner`

The planner accepts a definition that names its `countries` and `time_zone`. It uses the shared
source selection above. Its bounds enclose the requested Box or union of selected outlines.
Each snapshot, such as `copernicus-glo-30`, the extract, the `.poly` and the GLO-30 tile list, is
at its version (see [Versions](#versions)), as for `maps`. The other options come from
[`data/planner.toml`](#dataplannertoml). A GLO-30 input reads the tile
of each 1° square that its box touches and that `copernicus-glo-30-tiles` names; a box at sea
reads no snapshot. No layer reads a national terrain model yet.

| Layer | Reads | Options | Files |
| --- | --- | --- | --- |
| `planner/osm` | The selected PBF of each `planner/source/<area>` | `path`: `osm.pbf` for one area; none for multiple areas | `osm.pbf`: one extract as it is, or the available-snapshot union |
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
| `planner/sun` | `planner/terrain/grid` | `bounds`, `time_zone` of the region, `distance_m` (`terrain.margin_m`), `horizon_samples` and `horizon_directions` | `sun.pmtiles`: [the sun archive](planner-sun-tiles.md). `terrain_grid_sha256` binds the canonical terrain grid index. The step verifies and reconstructs the portable terrain tiles |

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
names it. The search and the sun layer use the `time_zone` of the region. All producers are
intermediate layers. Grid steps select only their `objects/`; their `index.json` stays local.

| Layer | Reads | Files |
| --- | --- | --- |
| `planner/routing/grid` | `planner/routing` | Packed `routing/blocks.json`, `routing/packs/<sha256>/pages.bin` and `pages.idx`, `offline/routing-cells/<cell>.json`, and `routes/tiles/<cell>.json`. It does not build the routing blocks again. Its index records the source graph and cell descriptors |
| `planner/search/pois/grid`, `planner/search/addresses/grid` | The database of its search component | `search/tiles/<component>/<cell>.sqlite`, packed. A cell retains intersecting places, addresses, their streets and contexts. The index records regional metadata and clipped cell bounds |
| `planner/assets/grid`, `planner/model/grid` | `planner/assets` or `planner/model` | The input files, packed as `maps/assets/` or `search/model/`. The index maps logical paths to file and transport identities |
| `planner/fonts/grid` | `planner/basemap`, `basemap.pmtiles`; `planner/places`, `places.pmtiles`; `planner/routing`, `routing/overlays.sqlite`; `planner/assets` | Packed `offline/fonts/<stack>.pbf`: the joined used glyph ranges of [the offline contract](planner-offline.md). The index maps each original glyph range to its joined stack |
| `planner/index` | Only `index.json` of each grid step | Packed `search/<region>.grid.json` and `offline/catalog.json` in `objects/`; `release.json` and the small tile pointers in `public/`. It selects those three outputs. Its own `index.json` stays local |

The overlay grid reads only `index.json` of the routing grid too. It checks the source routing
package and changes its TileJSON `routing_package` to the grid package hash. The final index
checks the OSM source, region, coverage, search schema and time zone, routing cell bounds, terrain
inputs and sun terrain identity. It reads no payload to compose metadata. A climate change builds
only its producer, its grid and the final index. The engine release manifest holds source versions
and preparation provenance; the client manifest holds the [planner release](planner-release.md)
data. It contains no deployment probe, device catalogue snapshot or source mirror.

A Python step runs `env PYTHONHASHSEED=0 uv run --locked --offline --no-default-groups
--no-python-downloads --group <group> python <entry> --step` in the repository root.
`uv sync --all-groups` prepares the packages on a machine. The fixed hash seed keeps the order
of a set out of the bytes. Its code names the selected Python group, each Python file that it
imports, each file that it reads from the repository and `tools/step_request.py`. The selected
locked package closure and interpreter enter its code identity. A credit that it writes comes
in its options, so it needs no source content projection.

### Service runtimes

`data/planner-runtime.toml` has one optional `[target]` table: `triple`, `glibc`, `node`
and `python`. Supported triples are `x86_64-unknown-linux-gnu` and
`aarch64-unknown-linux-gnu`. The glibc baseline is `major.minor`; Node and CPython
versions are exact `major.minor.patch`. Without this table, all runtime producers are blocked.
The optional `[publication]` table names HTTPS `site_origin`, `api_origin` and `objects_origin`.
Without it, planner publication is blocked. Origins affect endpoint bindings, not runtime bytes.
The shared pool is `<objects_origin>/planner/objects`. Tile URLs keep the tile Worker origin.

`planner/runtime/routing`, `planner/runtime/search` and `planner/runtime/downloads` use
prepared native Linux tools or a local pinned container. Probes read tool and image metadata.
Builds use locked offline dependencies. The receipt binds the target and actual builder.
Routing also binds the selected release profile. Search source selection uses Git's
declared file list on the initiating host; the container receives that same list.
Each step writes `<service>.tar.gz` and `runtime.json`; only the archive is a client file.
The descriptor is a named release reference under `runtime/<service>.json`. It binds the
service, target, archive hash and size, required host libraries and entry point.
Libraries supplied by archive ELF files are not host requirements.

Archives retain dependency metadata and licences. They contain no checkout, virtual
environment, builder paths or recipe key. Search keeps its model in the separate data input.
Routing carries the selected Linux executable notices from `THIRD-PARTY.md`.
Downloads contains its standard-library helper closure. File names are sorted, timestamps
are zero, and symbolic links are refused. ELF files must match the target architecture
and require no newer glibc than the baseline.

The pointer's `services` identities bind runtime content to routing grid content, search
grid and model content, or the offline index for downloads. They do not bind receipt keys
or unrelated optional layers. The owner verifies installation and actual service readiness before
publishing the planner pointer. Data producers can build independently.

Staging uses two slots per service. The current publication identifies the active slot;
running units do not establish traffic ownership. Unknown or conflicting ownership blocks
staging. A healthy slot with the same content and endpoint binding is reused. A changed service
or approved origin configuration uses the other slot. The full desired pointer holds those
choices; preview and the owner derive them without unit discovery during preview.

Installation verifies the runtime archive and every materialized service file against their
immutable identities. Archives contain only unique regular files with normalized relative
paths. Host architecture, glibc, shared libraries and interpreter versions must satisfy the
runtime descriptor. Search uses packaged Python dependencies with site packages disabled.
Readiness reports the opened routing package, search grid and model file hashes, or downloads
catalogue hash. An expected identity in an environment variable is not readiness evidence.
Probes bind these results to the running process, its runtime files, and the requested site
origin or shared object pool. Reloaded unit configuration alone cannot prove runtime readiness.

The owner uploads artifacts, stages and probes slots, then reloads routes for old and new
bindings. Public endpoint probes must pass before its planner pointer switches. Each unit and
route mutation uses the same durable intent as R2 writes. Immutable release metadata contains
no mutable slot choice. Old slots and pinned routes stay for the reader window, even when
object cleanup has no leftovers. Retirement checks exact slot ownership again.

An acknowledged partial activation can occupy the next inactive slot. Before replacing it,
the owner restores entry routes to the exact current published bindings and probes them.
It keeps the conflicting pinned binding through a full reader window, then retires and stages.
An unknown mutation outcome remains blocked; this wait never clears that barrier.

Downloads jobs return a ready quote with an absolute pinned `source` URL. The client reads the
bundle, release and objects through that source, so a stable entry switch cannot mix releases.
The private selection key binds the object pool as well as the catalog and bounds. Old quotes
remain available while their service slot is retained. After retirement, a missing bundle
requires a new quote. Job creation completes synchronously; there is no mutable polling path.

A grid cell is a zoom 9 Web Mercator tile that the bounds of the region overlap, clipped to the
bounds, with the id `9-<x>-<y>`. The JSON objects that `planner/routing` writes have their keys in
byte order.

### Releases

`releases/<product>/<id>.json` is the manifest of a release: `{"product", "region", "optional",
"layers", "named", "producers"}`, as the compact output of `serde_json` with the keys of each object in byte order.
`region` is the region of the environment that it was built for, and `optional` the optional layers
of the product that the environment switched on, sorted. The id is the SHA-256 of these bytes. A manifest holds no time or cost of a build, so two machines that build
the same layers make the same release. `layers` is sorted by `step`, and each layer has:

| Key | Meaning |
| --- | --- |
| `step`, `key`, `inputs`, `options`, `code`, `command`, `outputs`, `digest`, `files` | As in the [receipt](#receipt) |
| `snapshots` | `{source: {"version", "params"}}`: the version and the sorted `NAME=VALUE` of each snapshot that the layer read |
| `client` | The selected client outputs, as in the step (see [Products](#products)) |

`named` holds the exact files under `<prefix>/releases/<id>/`. Each entry has `path`, `size` and
`sha256` from a layer receipt. Entries are sorted by path. Paths are relative, use `/`, and have
no empty, `.` or `..` segment. Duplicate paths are refused. Named file identity is part of the
release id. The root document is generated after these entries finalize the release.

Live ownership, drift checks, uploads and cleanup use these exact paths. An undeclared file in
the active release folder is a leftover. A missing named file with local receipt bytes needs no
build or pointer switch. Missing local named objects are restored from their immutable remote
keys with size and SHA-256 checks. If both copies are absent, the repair builds the owning layer
and the inputs that it lacks. Existing named metadata with another digest is refused before any
publication write. A root-only change is actionable without a layer build and still passes the
product's verification.

The objects of a release are the selected client files, with one object per distinct SHA-256.
The manifest records every file of every layer, so a plan compares it with live and live names
the versions that it read. Uploads, live ownership and cleanup use the same selection.

`producers` deduplicates witnesses by the original full `code` digest. Each witness has `files`,
`source_config` and resolved `rust` target/build profile (`null` without Rust). The full fingerprints must
produce that digest and bind the exact source/config projection and recorded Rust metadata.
The projection comes from the same resolver traversal.
A missing witness does not permit portable adoption. Witness metadata changes the release id;
it does not change an existing layer key or receipt.

### Automatic work

`auto ENV` starts the existing detached operation. Fixture environments refuse it. Non-live
environments build and verify with normal working-tree rules. They do not publish or create an
approval record. A known busy reservation returns a successful structured skip; it has no waiting queue. Other admission failures remain errors.

Live admission observes the configured owner's original approval before acquisition or build.
The current native target/profile needs an exact checked execution entry. Planning, acquisition,
producer and runtime declarations must match that entry and the current publication configuration.
A prospective manual review cannot approve replacement tools. Missing retained planning inputs
require explicit preparation. Changed code or settings require reviewed manual apply.

Freshness discovery checks only requests used by the complete selected product declarations.
Stale automatic requests refresh; manual sources retain their selected versions. A failed required
upstream check blocks the run. Compatible portable layers keep the original full identity and
producer provenance. Their verified files can feed a new native consumer. Private metadata does
not imply that unpublished intermediates or native executables are portable. Reuse does not change
a saved Local adoption or create a foreign build receipt.

The run verifies complete desired products, then rechecks the reviewed declarations, pointers and
original approval. Any final automatic publication reuses the existing owner mutation barrier and
original approval CAS; it does not establish or replace manual approval. Publication requires the
checked enabled live timer at the first owner handoff. A disabled timer leaves the active run
building and verifying; its result is verified, not applied. Disable and the first handoff share
one short admission lock. An admitted irreversible owner completes normally. `auto` installs no timer.

`schedule live` reads actual operator systemd unit state, calendar, time zone, next trigger and
last run. `--setup-budget` configures only the bake slice before manual approval; it leaves the timer unchanged. Installed enablement and activity are separate from runnable setup. Missing or changed
setup reports a blocked reason without hiding enablement. `--calendar` and `--time-zone` validate
calendar and host setup before replacing the known units. `--disable` stops only the timer;
it never stops retained work, serving or final publication. Non-live schedules are unsupported.

The timer is persistent. One missed occurrence causes one catch-up admission, without a missed-run
backlog. Its locked offline Cargo entry uses the existing fresh producer launcher. Entry compilation
and retained bake workers share the operator's CPU and memory slice. Serving and installed commit
owners stay outside that slice. Effective cgroup-v2 limits are checked before bake admission. Step scheduling clamps its existing
memory ceiling to the configured host budget, preserving any lower caller limit.
An actual Docker runtime build is refused on a budgeted Linux worker because the daemon does not
inherit those limits. Verified stored runtimes and laptop container publication remain supported.

Host setup supplies CPU, memory, minimum free disk and an alert service. The installed MIT plumbing
binary checks disk reserve before entry compilation. Retained workers check reserve before work,
and known output/fetch estimates before a build. These checks are disk preflights, not a filesystem
quota. Failure notification attaches to actual retained workers as well as the timer entry.
The next calendar occurrence retries failed or busy work; there is no restart or waiting queue.

### Local portable data

`local::plan` compares supplied producer declarations at the original Rust target/profile.
It selects no original native compiler, library or Python interpreter. The owner must declare
the layer portable. Native service archives are excluded. Options, commands, declared outputs,
exact snapshot versions/parameters and selected dependency file digests must match. Dependency
comparison uses original byte provenance, not the Local host's native producer key.

A selection includes the original release's client files and the extra exact paths the caller
needs. Current visibility does not infer new application file requirements. A file can come
from its immutable client object key, an exact named release key, or verified local bytes.
An unpublished missing intermediate blocks adoption; it needs a Local build or materializer.
Planning performs no transfer. It reports each blocked selection with its reason.

`local::adopt` requires the exact reviewed complete selection. It checks source/config before
and after transfer, and checks every selected file's SHA-256 and size. It saves the original
release and selection in a separate adoption record. It creates no Local receipt or replacement
producer identity. A failure leaves verified cache bytes and keeps the previous anchor.
Local rebuilds keep their full execution identity. These APIs do not start apps or services.

### Local app services

`dev` prepares current working-tree data. `data/env/local.toml` holds its region and
optional layers. Local has no region until `dev REGION` or `region local REGION` writes the
file; it never takes the region of Live. Each saved Local product release pins source versions. Only `--refresh-live` replaces those
pins with the current published versions. A failed pointer observation is not absence.
Current declarations derive region geometry, source requests, inputs and semantic options.
Compatible portable layers retain original provenance through the shared verified reuse
path. Only missing or changed layers execute with their full current native identity.
Unused original execution tools are not required for reuse.
Matching-host native routing and Simulator execution keep full compiler/profile identity and compiled
root/code stamp. Startup rejects a changed stamp or prepared descriptor. It requires prepared
Python, Node dependency trees and builder bridge/Wasm; it installs nothing.

Preparation is a retained finite `dev_prepare` operation in environment `local`. It shares
normal Run fetch/build/verify events. `dev --prepare` returns its handle. Preparation never
starts a stopped app owner. It replaces only affected children of an already running owner.
Serving owns a distinct
stable store lock; it does not retain the environment operation lock. The known supervisor
admits only the selected app's children. Web planner uses routing, search, tiles and Vite;
Map builder shares tiles and Vite; Simulator uses its retained executable and assembled map.
A shared child stops only after its last app stops. A failed app stays failed until an explicit start.
Each app admits only its own providers. Start rejects a saved view with changed Local region geometry or layers. It compares child-specific code/config/data
fingerprints before replacement. A token-bound stop drains owned process groups. It does not
signal an arbitrary PID or restart failed children.

Views are immutable and contain verified objects, native execution and service metadata.
Readiness binds the opened routing package, search region/grid and query model to that view.
Frontend asset checks and HTTP availability are separate from browser workflow verification.
`dev --status` and `dev --logs` are observations. Start, stop and open are explicit actions.
After proven drain, stop removes obsolete known views and completed preparation scratch
views. It retains the current prepared view and saved portable-data roots. Active or
uncertain app and preparation ownership prevent cleanup.

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
| `inputs/records/<source>/<version>/<digest>.json` | The immutable record of one exact snapshot read. `digest` is the input digest in the live layer receipt. `files` lists only its selected names, URLs, sizes and SHA-256 values, sorted by name. An empty read has an empty list. Retrieval time stays local. New copies require `redistribute` and `r2_copy`; an existing record of a live read stays without `r2_copy` |
| `inputs/objects/<sha256>` | A file of an input copy. Immutable |

When `OBC_R2_BUCKET` or `OBC_R2_LOCAL_DIR` is set, `obc data` reads that bucket. Otherwise it
reads the pointers, the manifests and the records at `https://maps.openbikecomputer.com/<key>`;
a listing needs the bucket. A release id is 64 lowercase hex digits. The store keeps each manifest
that it reads, and uses its copy only while the SHA-256 of the copy is the id and the copy is of
the product.

Planning reads only copy metadata named by the live manifests, once per immutable key. A build
restores missing files only for matching source, version and request parameters, or the exact
file names of its selected step inputs. It checks file digests and sizes before recording a
snapshot or a request. A complete verified local selection serves even if R2 lacks its record.
A missing or corrupt named retained copy never starts an upstream capture. Status keeps its
small-source discovery allowlist: it does not download input objects for other sources.

`status --check` lists the owned prefixes, and compares them with live:

- drift: a key of a live release or of its input copies that R2 does not have, or has with
  another size. Pointers have no expected size. An available input record gives its canonical size.
- leftovers: a key under the owned prefixes that no live release uses. The files of
  `<prefix>/releases/<id>/` of a live release are never leftovers.

Exit status 1 of `status --check` is drift or leftovers, or a failure of R2. With `--json`, the
first writes the status, and the second writes an error.

Layer status compares current semantic declarations with the published producer's source/config
witness at its recorded target/profile. New execution retains the full native identity. The
runtime owner excludes builder execution bindings only from this read comparison; current
target, files, command, outputs and configuration remain exact. Missing witness evidence is
blocked. Known option, producer and input changes retain their existing state causes.

With `--check`, `vps` is the independent installed runtime/data observation. An unavailable
observer has `host: null`, no service results and an explicit `unavailable` reason. A complete
observation has host inventory and one readiness result for each of routing, search and downloads.
Each result compares the published slot and binding, actual process, verified installed files
and opened data. The fixed `live-status` plumbing command admits only these reads. The
`OBC_STATUS_HOST` credential is a separate forced read-only SSH authorization.

The weekly report keeps one marked issue. A successful command exit alone cannot close it.
Every current layer must be `ok`, R2 must have no drift or leftovers, attention must be empty,
and all three service results must be ready. Missing setup or incomplete evidence keeps the
issue open. An unchanged body emits no comment.

### Apply

`apply live` makes the plan of live live. It needs the bucket, and it changes R2 in this order:

1. It refuses when `data/` has edits that git does not have, apart from
   `data/env/local.toml`: live builds from a committed `data/`. The refusal names the files and
   says `git add data && git commit`; `status` lists them as `uncommitted`. The steps run the code of the
   working tree. Used acquisition, planning and producer code must also be committed.
   Commitment checks the physical inputs of the same code resolver. Selected manifests and
   lockfiles must be clean even when their content projection excludes an edit. UI source files
   outside an owner's declaration do not block apply. Preparation and Local builds may use
   working-tree code. Apply does not commit or push. Build preparation
   can run on the laptop or VPS. All final R2 writes run in one VPS owner under an exclusive OS lock.
2. It asks once in a terminal: "Apply M changes to live? removes X GB from R2", with the groups
   and `remove` of the plan. `--yes` does not ask. `--plan FILE` applies that plan; the plan must
   be the plan of now, as for `build --plan`. Without a terminal, `--yes` or `--plan` is the
   consent. When live has every change and nothing is to be removed, it applies nothing.
   An unresolved preview is refused before preparation. Use `prepare live`, review the
   resulting plan and save it before applying. A no-op still returns a finished run.
3. It builds the plan, as `build live --plan` does.
4. It checks each release that changes: the check of its product, and its pointer. Each file
   that it uploads must have its SHA-256 in the store. A failed check changes nothing on R2.
5. It transfers a bundle of existing release manifests, input-copy records, desired pointers and
   the original run journal to the owner. Only missing payload bytes move. Under the lock, the
   owner checks each exact `observed` pointer again before it uploads each key of the releases after the apply and of their input copies that R2 lacks,
   or holds with another size; a key with another size goes first. Then it checks each key.
6. For planner changes, it activates and publicly probes the approved service endpoints first.
   It writes the pointer of each product whose full desired document changes: the document with
   `"release": "<id>"` and `"applied"`, the time of the switch (`YYYY-MM-DDTHH:MM:SSZ`), and
   `Cache-Control: public, max-age=60, must-revalidate`.
7. It reads live again and lists its prefixes, and `reference/v1` once live reads a `dtm-*`
   source. With drift, it removes nothing. The leftovers are the keys that no live release uses
   and that R2 had 5 minutes before final publication started. The owner keeps its lock through cleanup.
8. A client that read an old pointer finishes its downloads first. So while there are leftovers,
   the apply waits until 12 minutes after the time of the newest pointer on R2 (10 minutes, and 2
   for a clock that differs), and 10 minutes after its own switch. When no pointer time reads, it
   waits 10 minutes. Then it reads and lists again, as in step 7, and removes the leftovers of
   that read only, with a line in `removed.jsonl`. An apply that stopped in the wait waits again.

An apply removes the leftovers of step 7, not the `remove` list of the plan, which is an estimate.
A record on R2 of a version that live reads stays, also when `r2_copy` of its source is off now.

An acknowledged failure before step 6 leaves live as it was. A new plan uploads only what R2
still lacks. An unknown mutation outcome blocks all later commits. The objects, manifests and named files are immutable, with
`Cache-Control: public, max-age=31536000, immutable`.

The commit owner binds the original run id to the bundle SHA-256. Reusing that id with another
bundle is refused. Before each remote mutation, it fsyncs one intent and its directory. After
an acknowledgement, it fsyncs the run event before it clears the intent. A pending intent survives
owner or machine failure. A later read that looks correct does not clear it. There is no timeout
or lock takeover. Mutating children inherit the lock; a surviving child still excludes a new owner.
The owner runs as an admitted system service outside the initiating SSH session. Its durable state
and original run remain on the VPS. Successful replies copy the owner journal to the laptop.
Planner service activation and retirement run under this same owner and mutation barrier.


### Detached operations

Manual `prepare`, `build` and `apply` return `{run, request}`. `request` is the SHA-256 of the
immutable request. A retained fresh worker owns the run after the caller leaves. One environment
on one host/store admits one operation, with no waiting queue. Separate machines can prepare;
only the fixed VPS owner publishes. macOS uses a separate session and private logs. Linux uses
a transient user service, an active user manager, enabled linger and the operator's private
`OBC_RUN_ENV_FILE`. Credential values are not copied into arguments, events or control records.

`operations/<run>/control.json` binds kind, environment, selection, moves, any saved plan's exact
bytes, root and retained worker identity. State is reserved, running, stopping, stopped, owner,
finished or resolved. Stop invalidates a reserved child. During preparation, it drains admitted
work, starts no next fetch or step, and prevents publication handoff. Owner handoff records the
fixed host and bundle SHA before admission; stop is then refused. Terminal resolution retains
that binding and pins `owner-result.json` by size and SHA. Local completion durably pins
`result.json` by size and SHA before recording finished. Finished, drained workers release
their private executable. An unresolved owner retains it.

`runs` reads computed observations. A lost local transport does not prove failure or no writes.
Owner evidence binds run and bundle to both its inherited per-run lock and the fixed writer lock.
A held mutation awaits acknowledgement; abandoned pending intent stays unknown. Owner observations
are noninteractive and bound command execution and output collection to 30 seconds. Neither reads
nor missing remote state clear a barrier. `runs RUN --reconcile` is an explicit mutation: it checks
a final bound reply and journal prefix, copies acknowledged events, and resolves local control.
It refuses while the local worker drains. Result reads verify their sealed bytes.
The shared run view includes `logs`, the bounded tail of its worker stderr. A missing log yields
an empty list. A read failure appears in `observation_error` and preserves the run state.

## Manual automatic approval

A manual Live apply reviews automatic approval even when no layer changes. It requires committed
data and used code. It verifies every available complete product without unchanged-byte shortcuts.
The parent confirmation retains the exact plan, including the approval review. Preparation and
Local builds do not establish approval.

The configured publication owner has one `commits/current.approval`. Its observation distinguishes
absence from failed reads. The plan pins the owner's fingerprint and the exact prior record SHA.
The fingerprint uses the Linux machine identity and canonical fixed owner root. It contains no
raw machine identity or credential. The owner checks it, the prior record and original pointers
under its existing writer lock before mutation. An SSH alias is not an owner identity.

The record binds the effective region, layers, publication target, selected-source settings and
producer declarations. Actual source versions and freshness cadence are excluded. Selected-source
access and publication settings remain included. Manual data commitment still includes them all.
The same code resolver supplies acquisition, planning and producer source/config witnesses.
Implicit embedded registry bytes use declared source projections; explicit raw registry inputs
remain raw. Operator UI source stays outside scoped owner code.

Each native target/profile has its latest checked execution entry. A same-context apply replaces
its old tools. A different context survives only when current declarations reproduce each source
witness at its recorded Rust target/profile and the common configuration is unchanged. This
comparison selects no foreign execution tools. It retains the original checked execution identity;
it does not approve recomputation by another host. Changed acquisition or planning code can remove
an old entry without rebuilding a layer.

Runtime execution retains the common prepared target/profile declaration. Containers bind the
exact local image. Native builders bind selected executable bytes and fixed execution settings.
Routing uses the existing native release resolver, with source/work remaps and the selected C
link driver. Search also binds Node and the self-contained npm implementation. npm has empty
user/global config and no global module paths. uv ignores external config and uses the committed
project, group and lock. All native builders bind readelf and the selected CPython executable,
already loaded standard modules, and unambiguous loaded libpython/libz when required.

The adapter uses isolated, no-site CPython with UTF-8. Standard bytecode caches must match source;
this binding does not attest every loaded instruction or system dependency. Script tool wrappers,
external npm implementation files and ambiguous Python libraries are unsupported. Provider paths
stay local. Public native builder options contain only the execution digest. The checked worker
resolves routing providers without another build. Execution recomputes the complete binding before
work and after packaging; changed providers refuse acceptance. Missing unused acquisition tools can
leave current execution unavailable. Retained-input manual publication remains supported; the
approval outcome states what is unavailable. Only comparable prior entries can survive.

Complete publication seals its result and finishes owner state before writing approval. The
approval record binds that publication SHA, run and bundle. The sealed publication does not
contain an approval digest. Owner results report publication and approval separately: recorded,
unavailable or unresolved. A partial or unknown publication writes no approval. An approval write
failure does not change a checked publication into a failed publication. Observation and explicit
reconciliation preserve this distinction. No automatic schedule is enabled by this record alone.

## Commands

| Command | Output |
| --- | --- |
| `obc data [--json]` | In a terminal, and without `--json`: the TUI. Otherwise the output of `status` |
| `obc data status [--check] [--json]` | Where live was read; per product, the live release (or nothing live), `applied` of its pointer, the size of its objects, the optional layers that `layer` switches, and the state of each layer of the environment `live`; what needs attention: stale and blocked sources, and with `--check` drift and leftovers. When a fetch that the step list of a product needs fails, the layer states of that product are unknown (`layers` is `null`), and attention gives the error. `--check` adds the listing of [Live](#live) and the installed VPS runtime/data observation and exits with 1 when it finds drift or leftovers. Without the bucket, `--check` exits with 4 before it reads anything |
| `obc data sources [--check-now] [--json]` | Every source with licence, R2 copy, live versions (`—` when live does not read the source; `?` with one warning when R2 cannot be read, and then `live` is `null` and `live_unknown` is `true` in the JSON), newest upstream version, age, policy, state and the versions in the local store. Rows are in kind order: data, then assets, then tools. An upstream check of the last hour serves, except with `--check-now` |
| `obc data fetch SOURCE[@VERSION] [NAME=VALUE…] [--json]` | Fetches the version, or else the newest file upstream. Writes the store path of each file |
| `obc data policy SOURCE DAYS\|manual [--json]` | Writes `refresh` of the source in `data/sources.toml`. The edit keeps comments and the other lines. A policy in days for a source without `version = "date"` is refused. Writes the source |
| `obc data region ENV ID [--json]` | Writes `region` of `data/env/ENV.toml`. Writes the environment |
| `obc data layer ENV NAME on\|off [--json]` | Adds the optional layer to `layers` of `data/env/ENV.toml`, or removes it. A layer that no product has is refused. Writes the environment |
| `obc data undo ENV [--json]` | Writes `data/env/ENV.toml` as git has it in `HEAD`: the edits that are not applied go. Writes the environment |
| `obc data clean [--apply [--yes]] [--json]` | The plan of [Clean](#clean): the snapshot records and the objects that nothing reaches, what stays and why, and the size of `partial/`. `--apply` asks, then cleans. With `--json` and `--apply`, the plan goes to standard error, and the output is what it did |
| `obc data region [list] [--json]` | Every region with its name and definition |
| `obc data region show ID [--json]` | One region, the regions it resolves to, and its box when every part is a box |
| `obc data region areas [QUERY] [--json]` | Search cached Geofabrik names and paths. Does not fetch |
| `obc data region create ID --name NAME (--area PATH… \| --box W,S,E,N) [--country CODE]… --time-zone ZONE [--json]` | Save one definition. Repeat `--area` for each path. Countries derive from the cached index where possible |
| `obc data region delete ID [--apply --expected SHA [--yes]] [--json]` | Preview references and definition hash, or delete that reviewed, unused definition after confirmation |
| `obc data plan ENV [--only GROUP,…] [--move SOURCE[@VERSION]]… [--json]` | What a build of the environment fetches and builds, in groups, with estimates. It prepares no bulk input. An unresolved graph sets `needs_prepare`. `--move` is in [Versions](#versions). For `live`: the groups of [Changes of live](#changes-of-live), the edits, and what an apply removes from R2 |
| `obc data prepare ENV [--only GROUP,…] [--move SOURCE[@VERSION]]… [--json]` | Starts durable input preparation. Returns `{run, request}`. Its completed result holds `{run, plan}`; review and save `.plan`. Builds and uploads nothing |
| `obc data build ENV [--plan FILE \| [--only GROUP,…] [--move SOURCE[@VERSION]]…] [--json]` | Starts a durable build of the groups into the store. Returns `{run, request}`. Complete products get release manifests; incomplete products stay blocked. It uploads nothing |
| `obc data apply live [--plan FILE] [--yes] [--json]` | Reviews and starts durable publication, as [Apply](#apply) says. Returns `{run, request}`. The completed owner result lists uploaded, switched and removed keys |
| `obc data runs [--json]` | Every run in the store, newest first: id, command, outcome, time, and the size of its fetches and of the layers that it built |
| `obc data runs RUN [--json]` | One run and its computed operation state, observed owner error and result; fetches and steps retain time, peak RAM, outputs, input and code identities. Reads change no local history |
| `obc data runs RUN --follow [--json]` | For detached operations, changed observations until a final result or actionable unresolved outcome. Other journals stream their events |
| `obc data runs RUN --stop [--json]` | Stops after current preparation; refuses after owner handoff |
| `obc data runs RUN --reconcile [--json]` | Explicitly accepts a verified final bound owner result and mirrors its journal |
| `obc data runs RUN --result` | One completed preparation/build output, or sealed owner result. Unresolved runs have no result |

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
prefix has `release`, and under `inputs/` once any pointer has. They and the reference archive
publisher refuse writes under `reference/v1/` once a verified current manifest reads `dtm-*`.
Missing or malformed current metadata refuses those reference writes.

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
| `busy` | 2 | Another admitted operation owns the environment. No work waits for it. | Observe the current run or retry after it drains. No work is queued. |
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
| `versions SOURCE` | `Versions` |
| `fetch` | `Fetched` |
| `policy` | `Source` |
| `region`, `region list` | `RegionList` |
| `region show` | `RegionDetail` |
| `region areas` | `Suggestions` |
| `region create` | `Region` |
| `region delete` | `Deletion` |
| `region ENV ID`, `layer`, `undo` | `Edited` |
| `status`, and `obc data` without a terminal | `Status` |
| `clean`, `clean --apply` | `CleanPlan` |
| `plan`, `dev --check` | `EnvPlan` |
| `prepare`, `build`, `apply`, `dev --prepare` | `Handle` |
| `dev --start`, `dev --stop`, `dev --status` | `Observed` |
| `dev --logs` | `Logs` |
| `dev`, completed dev preparation | `Prepared` |
| Completed prepare output, `dev --inputs` | `Prepared2` |
| Completed build output | `Built` |
| Completed apply output | `Applied` |
| `auto` admission | `Started` |
| Completed auto output | `Result` |
| Live timer state | `State4` |
| `schedule live --setup-budget` | `Budget` |
| `runs` | `RunList` |
| `runs RUN` for a detached operation | `View` |
| `runs RUN` for other journals | `Details` |
| `runs RUN --follow`, one per line | `Event` |
| `r2 list`, `r2 stat`, `r2 delete` | `Objects` |
| `r2 get` | `Downloaded` |
| `r2 put` | `Uploaded` |
| Every command that fails | `Failure` |

```json
{
  "$defs": {
    "App": {
      "enum": [
        "web-planner",
        "map-builder",
        "simulator"
      ],
      "type": "string"
    },
    "AppState": {
      "additionalProperties": false,
      "properties": {
        "message": {
          "type": [
            "string",
            "null"
          ]
        },
        "status": {
          "type": "string"
        }
      },
      "required": [
        "status",
        "message"
      ],
      "type": "object"
    },
    "Applied": {
      "description": "What an apply did.",
      "properties": {
        "approval": {
          "$ref": "#/$defs/Outcome"
        },
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
        "run": {
          "type": "string"
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
        "run",
        "built",
        "uploaded",
        "switched",
        "removed",
        "approval"
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
          "const": "uncommitted",
          "description": "Edits in `data/` that git does not have; an apply refuses them.",
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
    "Binding": {
      "additionalProperties": false,
      "properties": {
        "code": {
          "$ref": "#/$defs/Code"
        },
        "files": {
          "additionalProperties": {
            "type": "string"
          },
          "type": "object"
        }
      },
      "required": [
        "code",
        "files"
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
    "Budget": {
      "properties": {
        "alert": {
          "type": "string"
        },
        "cpu_percent": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "memory_bytes": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "minimum_free": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "cpu_percent",
        "memory_bytes",
        "minimum_free",
        "alert"
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
          "description": "The operation journal, including preparation and release creation.",
          "type": "string"
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
      "additionalProperties": false,
      "description": "The code that makes a layer. When in doubt, declare more: too much costs a rebuild, too little\ngives stale data.",
      "properties": {
        "crates": {
          "description": "Workspace crates and their resolved normal and build dependencies.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "libraries": {
          "description": "Native library or executable files bound by the provider before its code runs.",
          "items": {
            "$ref": "#/$defs/Library"
          },
          "type": "array"
        },
        "paths": {
          "description": "Files and directories, relative to the repository root.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "python": {
          "anyOf": [
            {
              "$ref": "#/$defs/Python"
            },
            {
              "type": "null"
            }
          ],
          "description": "The selected Python runtime and its locked package group."
        },
        "python_packages": {
          "description": "A locked Python group packaged for another runtime, without selecting its interpreter.",
          "type": [
            "string",
            "null"
          ]
        },
        "rust": {
          "anyOf": [
            {
              "$ref": "#/$defs/Rust"
            },
            {
              "type": "null"
            }
          ],
          "description": "None binds the native dev build. Prepared builds bind their toolchain in step options."
        },
        "sources": {
          "description": "The content settings of these sources. Freshness and access controls are excluded.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "target": {
          "description": "Resolve Rust dependencies for this target; None selects the producer host.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "paths",
        "crates"
      ],
      "type": "object"
    },
    "Code2": {
      "description": "The kind of an error. It sets the exit status and the fix.",
      "oneOf": [
        {
          "const": "busy",
          "description": "Another admitted operation owns the environment. No work waits for it.",
          "type": "string"
        },
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
    "Deletion": {
      "properties": {
        "region": {
          "type": "string"
        },
        "sha256": {
          "type": "string"
        },
        "used_by": {
          "items": {
            "type": "string"
          },
          "type": "array"
        }
      },
      "required": [
        "region",
        "sha256",
        "used_by"
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
          "$ref": "#/$defs/Outcome2"
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
        "approval": {
          "anyOf": [
            {
              "$ref": "#/$defs/Review"
            },
            {
              "type": "null"
            }
          ],
          "description": "Complete manual publication reviews this owner record separately from source refreshes."
        },
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
            "type": [
              "string",
              "null"
            ]
          },
          "description": "Source move intent: an explicit version, or each request's newest version. Exact resolved\nversions are in `versions`; fetching never changes this intent.",
          "type": "object"
        },
        "needs_prepare": {
          "description": "Discovery could not resolve the step graph. Prepare and review a new plan before replay.",
          "type": "boolean"
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
        "listed",
        "needs_prepare",
        "approval"
      ],
      "type": "object"
    },
    "Error": {
      "description": "Why a command failed, and what to do about it.",
      "properties": {
        "code": {
          "$ref": "#/$defs/Code2"
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
    "Execution": {
      "additionalProperties": false,
      "properties": {
        "profile": {
          "$ref": "#/$defs/Profile"
        },
        "roles": {
          "additionalProperties": {
            "$ref": "#/$defs/Role"
          },
          "type": "object"
        },
        "target": {
          "type": "string"
        }
      },
      "required": [
        "target",
        "profile",
        "roles"
      ],
      "type": "object"
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
        "product": {
          "description": "Local product whose saved release supplies this pin; absent for the shared Live plan.",
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
        "partial_bytes": {
          "description": "The size of `partial/`: unfinished downloads and the work of steps that stopped. A\ncollection empties it.",
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
    "Handle": {
      "additionalProperties": false,
      "properties": {
        "request": {
          "type": "string"
        },
        "run": {
          "type": "string"
        }
      },
      "required": [
        "run",
        "request"
      ],
      "type": "object"
    },
    "Host": {
      "additionalProperties": false,
      "description": "Actual host prerequisites. Absent interpreters cannot satisfy a runtime target.",
      "properties": {
        "glibc": {
          "type": "string"
        },
        "node": {
          "type": [
            "string",
            "null"
          ]
        },
        "python": {
          "type": [
            "string",
            "null"
          ]
        },
        "triple": {
          "type": "string"
        }
      },
      "required": [
        "triple",
        "glibc",
        "node",
        "python"
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
          "default": [],
          "description": "Exact resolved snapshot file names, including an empty read. For a layer input, the\nselected paths, or none for every file. The digest names the bytes in the key.",
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
        "digest",
        "files"
      ],
      "type": "object"
    },
    "Installed": {
      "additionalProperties": false,
      "properties": {
        "binding": {
          "type": "string"
        },
        "id": {
          "type": "string"
        },
        "service": {
          "$ref": "#/$defs/Service"
        },
        "slot": {
          "format": "uint8",
          "maximum": 255,
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "service",
        "id",
        "slot",
        "binding"
      ],
      "type": "object"
    },
    "Kept": {
      "description": "A snapshot record, the layers of one step, or the objects that one kind of root names and no\nkept record or layer has.",
      "properties": {
        "because": {
          "description": "`live PRODUCT, …`, `newest of the source`, `newest of a request`, `inputs kept`,\n`live release`, `fixture` or `planner recipe`.",
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
    "Library": {
      "additionalProperties": false,
      "properties": {
        "name": {
          "type": "string"
        },
        "path": {
          "type": "string"
        },
        "sha256": {
          "type": "string"
        }
      },
      "required": [
        "name",
        "path",
        "sha256"
      ],
      "type": "object"
    },
    "LiveRelease": {
      "additionalProperties": false,
      "properties": {
        "observed": {
          "description": "SHA-256 of the exact pointer bytes that consent observed; None means absent.",
          "type": [
            "string",
            "null"
          ]
        },
        "pointer": {
          "description": "SHA-256 of the actual client document, excluding only `release` and `applied`.",
          "type": [
            "string",
            "null"
          ]
        },
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
        "release",
        "pointer",
        "observed"
      ],
      "type": "object"
    },
    "Logs": {
      "properties": {
        "logs": {
          "items": {
            "type": "string"
          },
          "type": "array"
        }
      },
      "required": [
        "logs"
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
    "Observation2": {
      "additionalProperties": false,
      "properties": {
        "host": {
          "anyOf": [
            {
              "$ref": "#/$defs/State2"
            },
            {
              "type": "null"
            }
          ]
        },
        "services": {
          "items": {
            "$ref": "#/$defs/ServiceStatus"
          },
          "type": "array"
        },
        "unavailable": {
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "host",
        "services",
        "unavailable"
      ],
      "type": "object"
    },
    "Observed": {
      "properties": {
        "state": {
          "anyOf": [
            {
              "$ref": "#/$defs/State3"
            },
            {
              "type": "null"
            }
          ]
        }
      },
      "required": [
        "state"
      ],
      "type": "object"
    },
    "Outcome": {
      "oneOf": [
        {
          "additionalProperties": false,
          "properties": {
            "status": {
              "const": "not_requested",
              "type": "string"
            }
          },
          "required": [
            "status"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "properties": {
            "sha256": {
              "type": "string"
            },
            "status": {
              "const": "recorded",
              "type": "string"
            },
            "unavailable": {
              "type": [
                "string",
                "null"
              ]
            }
          },
          "required": [
            "status",
            "sha256",
            "unavailable"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "properties": {
            "reason": {
              "type": "string"
            },
            "status": {
              "const": "unavailable",
              "type": "string"
            }
          },
          "required": [
            "status",
            "reason"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "description": "Publication succeeds independently of this owner-local durable write.",
          "properties": {
            "reason": {
              "type": "string"
            },
            "status": {
              "const": "unresolved",
              "type": "string"
            }
          },
          "required": [
            "status",
            "reason"
          ],
          "type": "object"
        }
      ]
    },
    "Outcome2": {
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
    "Phase": {
      "enum": [
        "prepare",
        "build",
        "verify",
        "upload",
        "switch",
        "wait",
        "cleanup"
      ],
      "type": "string"
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
        },
        {
          "additionalProperties": false,
          "description": "The desired client document or release identity differs, without a layer change.",
          "properties": {
            "document": {
              "type": "string"
            },
            "kind": {
              "const": "pointer",
              "type": "string"
            },
            "product": {
              "type": "string"
            },
            "release": {
              "type": "string"
            }
          },
          "required": [
            "kind",
            "product",
            "release",
            "document"
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
    "Prepared": {
      "additionalProperties": false,
      "properties": {
        "apps": {
          "items": {
            "$ref": "#/$defs/App"
          },
          "type": "array",
          "uniqueItems": true
        },
        "children": {
          "additionalProperties": {
            "$ref": "#/$defs/Binding"
          },
          "type": "object"
        },
        "descriptor": {
          "type": "string"
        },
        "supervisor": {
          "$ref": "#/$defs/Binding"
        },
        "view": {
          "type": "string"
        }
      },
      "required": [
        "view",
        "descriptor",
        "supervisor",
        "children",
        "apps"
      ],
      "type": "object"
    },
    "Prepared2": {
      "description": "Explicit preparation resolves inputs. Save `plan` after reviewing it, not this envelope.",
      "properties": {
        "plan": {
          "$ref": "#/$defs/EnvPlan"
        },
        "run": {
          "type": "string"
        }
      },
      "required": [
        "run",
        "plan"
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
    "Profile": {
      "enum": [
        "dev",
        "release"
      ],
      "type": "string"
    },
    "Publication": {
      "description": "A remote write that acknowledged success. Verification can still fail afterward.",
      "oneOf": [
        {
          "additionalProperties": false,
          "properties": {
            "key": {
              "type": "string"
            },
            "kind": {
              "const": "uploaded",
              "type": "string"
            }
          },
          "required": [
            "kind",
            "key"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "properties": {
            "kind": {
              "const": "switched",
              "type": "string"
            },
            "product": {
              "type": "string"
            },
            "release": {
              "type": "string"
            }
          },
          "required": [
            "kind",
            "product",
            "release"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "properties": {
            "bytes": {
              "format": "uint64",
              "minimum": 0,
              "type": "integer"
            },
            "key": {
              "type": "string"
            },
            "kind": {
              "const": "removed",
              "type": "string"
            }
          },
          "required": [
            "kind",
            "key",
            "bytes"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "properties": {
            "binding": {
              "type": "string"
            },
            "kind": {
              "const": "service_staged",
              "type": "string"
            },
            "service": {
              "type": "string"
            },
            "slot": {
              "format": "uint8",
              "maximum": 255,
              "minimum": 0,
              "type": "integer"
            }
          },
          "required": [
            "kind",
            "service",
            "slot",
            "binding"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "properties": {
            "bindings": {
              "items": {
                "type": "string"
              },
              "type": "array"
            },
            "kind": {
              "const": "services_activated",
              "type": "string"
            }
          },
          "required": [
            "kind",
            "bindings"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "properties": {
            "bindings": {
              "items": {
                "type": "string"
              },
              "type": "array"
            },
            "kind": {
              "const": "services_retired",
              "type": "string"
            }
          },
          "required": [
            "kind",
            "bindings"
          ],
          "type": "object"
        }
      ]
    },
    "Python": {
      "additionalProperties": false,
      "properties": {
        "group": {
          "description": "None selects the project's base packages, without default groups.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "group"
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
      "description": "How old the live version may get, in days, before the source is stale; `manual` is never stale."
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
    "RegionDetail": {
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
    "Request": {
      "properties": {
        "live": {
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
        "stored": {
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "unavailable": {
          "type": [
            "string",
            "null"
          ]
        },
        "upstream": {
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "params",
        "live",
        "stored",
        "upstream",
        "unavailable"
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
        "due": {
          "type": "boolean"
        },
        "live": {
          "type": [
            "string",
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
        "state": {
          "$ref": "#/$defs/State"
        }
      },
      "required": [
        "params",
        "live",
        "observation",
        "due",
        "state",
        "reason",
        "age_days"
      ],
      "type": "object"
    },
    "ResolvedRust": {
      "additionalProperties": false,
      "properties": {
        "build": {
          "$ref": "#/$defs/Rust"
        },
        "target": {
          "type": "string"
        }
      },
      "required": [
        "target",
        "build"
      ],
      "type": "object"
    },
    "Result": {
      "properties": {
        "applied": {
          "anyOf": [
            {
              "$ref": "#/$defs/Applied"
            },
            {
              "type": "null"
            }
          ]
        },
        "approval": {
          "type": [
            "string",
            "null"
          ]
        },
        "built": {
          "$ref": "#/$defs/Built"
        },
        "publication": {
          "description": "Publication requires checked enabled-timer admission.",
          "type": "string"
        }
      },
      "required": [
        "built",
        "approval",
        "publication",
        "applied"
      ],
      "type": "object"
    },
    "Review": {
      "oneOf": [
        {
          "additionalProperties": false,
          "properties": {
            "config": {
              "type": "string"
            },
            "executions": {
              "items": {
                "$ref": "#/$defs/Execution"
              },
              "type": "array"
            },
            "expected": {
              "type": [
                "string",
                "null"
              ]
            },
            "owner": {
              "type": "string"
            },
            "status": {
              "const": "ready",
              "type": "string"
            },
            "unavailable": {
              "description": "Retained inputs can be applied when unused acquisition tools are absent.",
              "type": [
                "string",
                "null"
              ]
            }
          },
          "required": [
            "status",
            "owner",
            "expected",
            "config",
            "executions",
            "unavailable"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "properties": {
            "owner": {
              "type": [
                "string",
                "null"
              ]
            },
            "reason": {
              "type": "string"
            },
            "status": {
              "const": "unavailable",
              "type": "string"
            }
          },
          "required": [
            "status",
            "reason",
            "owner"
          ],
          "type": "object"
        }
      ]
    },
    "Role": {
      "additionalProperties": false,
      "properties": {
        "execution": {
          "type": "string"
        },
        "rust": {
          "anyOf": [
            {
              "$ref": "#/$defs/ResolvedRust"
            },
            {
              "type": "null"
            }
          ]
        },
        "source_config": {
          "type": "string"
        }
      },
      "required": [
        "rust",
        "source_config",
        "execution"
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
        "observation_errors": {
          "additionalProperties": {
            "type": "string"
          },
          "type": "object"
        },
        "operations": {
          "additionalProperties": {
            "$ref": "#/$defs/Status2"
          },
          "type": "object"
        },
        "runs": {
          "items": {
            "$ref": "#/$defs/Summary"
          },
          "type": "array"
        }
      },
      "required": [
        "runs",
        "operations",
        "observation_errors"
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
    "Rust": {
      "oneOf": [
        {
          "additionalProperties": false,
          "properties": {
            "kind": {
              "const": "native",
              "type": "string"
            },
            "profile": {
              "$ref": "#/$defs/Profile"
            }
          },
          "required": [
            "kind",
            "profile"
          ],
          "type": "object"
        },
        {
          "additionalProperties": false,
          "properties": {
            "kind": {
              "const": "prepared",
              "type": "string"
            },
            "profile": {
              "$ref": "#/$defs/Profile"
            }
          },
          "required": [
            "kind",
            "profile"
          ],
          "type": "object"
        }
      ]
    },
    "Service": {
      "enum": [
        "routing",
        "search",
        "downloads"
      ],
      "type": "string"
    },
    "ServiceStatus": {
      "additionalProperties": false,
      "properties": {
        "ready": {
          "type": "boolean"
        },
        "reason": {
          "type": [
            "string",
            "null"
          ]
        },
        "service": {
          "$ref": "#/$defs/Service"
        }
      },
      "required": [
        "service",
        "ready",
        "reason"
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
        "start": {
          "description": "The version that a plan reads while no live release reads the source. `--move` overrides\nit. Without it, the first fetch takes the newest version upstream.",
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
        "requests": {
          "items": {
            "$ref": "#/$defs/RequestStatus"
          },
          "type": "array"
        },
        "snapshots": {
          "description": "The versions in the local store, the one fetched last first.",
          "items": {
            "$ref": "#/$defs/Stored"
          },
          "type": "array"
        },
        "start": {
          "description": "The version that a plan reads while no live release reads the source. `--move` overrides\nit. Without it, the first fetch takes the newest version upstream.",
          "type": [
            "string",
            "null"
          ]
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
        "snapshots",
        "requests",
        "credential_missing"
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
    "Started": {
      "anyOf": [
        {
          "$ref": "#/$defs/Handle"
        },
        {
          "properties": {
            "env": {
              "type": "string"
            },
            "reason": {
              "type": "string"
            },
            "run": {
              "type": [
                "string",
                "null"
              ]
            },
            "skipped": {
              "type": "boolean"
            }
          },
          "required": [
            "skipped",
            "env",
            "reason",
            "run"
          ],
          "type": "object"
        }
      ]
    },
    "State": {
      "description": "The state of a source or a layer. A source is only ok, stale or blocked. When more than one\nstate applies to a layer, the first in this order is its state, so the least of several states\nis the one to show for all of them.",
      "oneOf": [
        {
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
        {
          "const": "unused",
          "description": "A source that no live layer and no active request reads. Only sources have it.",
          "type": "string"
        }
      ]
    },
    "State2": {
      "additionalProperties": false,
      "properties": {
        "host": {
          "$ref": "#/$defs/Host"
        },
        "installed": {
          "items": {
            "$ref": "#/$defs/Installed"
          },
          "type": "array"
        }
      },
      "required": [
        "host",
        "installed"
      ],
      "type": "object"
    },
    "State3": {
      "additionalProperties": false,
      "properties": {
        "apps": {
          "additionalProperties": false,
          "properties": {
            "map-builder": {
              "$ref": "#/$defs/AppState"
            },
            "simulator": {
              "$ref": "#/$defs/AppState"
            },
            "web-planner": {
              "$ref": "#/$defs/AppState"
            }
          },
          "type": "object"
        },
        "code": {
          "type": [
            "string",
            "null"
          ]
        },
        "layers": {
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "message": {
          "type": [
            "string",
            "null"
          ]
        },
        "region": {
          "type": [
            "string",
            "null"
          ]
        },
        "status": {
          "type": "string"
        },
        "token": {
          "type": "string"
        },
        "url": {
          "type": [
            "string",
            "null"
          ]
        },
        "view": {
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "token",
        "status",
        "apps",
        "region",
        "layers"
      ],
      "type": "object"
    },
    "State4": {
      "properties": {
        "active": {
          "type": "boolean"
        },
        "blocked": {
          "type": [
            "string",
            "null"
          ]
        },
        "calendar": {
          "type": [
            "string",
            "null"
          ]
        },
        "enabled": {
          "type": "boolean"
        },
        "last_run": {
          "anyOf": [
            {
              "$ref": "#/$defs/View"
            },
            {
              "type": "null"
            }
          ]
        },
        "last_trigger": {
          "type": [
            "string",
            "null"
          ]
        },
        "next": {
          "type": [
            "string",
            "null"
          ]
        },
        "runnable": {
          "type": "boolean"
        },
        "time_zone": {
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "enabled",
        "active",
        "runnable",
        "blocked",
        "calendar",
        "time_zone",
        "next",
        "last_trigger",
        "last_run"
      ],
      "type": "object"
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
        },
        "vps": {
          "anyOf": [
            {
              "$ref": "#/$defs/Observation2"
            },
            {
              "type": "null"
            }
          ],
          "description": "Installed runtime and opened data, independently from recorded-target source comparison."
        }
      },
      "required": [
        "from",
        "products",
        "attention",
        "check",
        "vps"
      ],
      "type": "object"
    },
    "Status2": {
      "oneOf": [
        {
          "properties": {
            "status": {
              "const": "starting",
              "type": "string"
            }
          },
          "required": [
            "status"
          ],
          "type": "object"
        },
        {
          "properties": {
            "status": {
              "const": "running",
              "type": "string"
            }
          },
          "required": [
            "status"
          ],
          "type": "object"
        },
        {
          "properties": {
            "status": {
              "const": "stopping",
              "type": "string"
            }
          },
          "required": [
            "status"
          ],
          "type": "object"
        },
        {
          "properties": {
            "status": {
              "const": "stopped",
              "type": "string"
            }
          },
          "required": [
            "status"
          ],
          "type": "object"
        },
        {
          "properties": {
            "status": {
              "const": "interrupted",
              "type": "string"
            }
          },
          "required": [
            "status"
          ],
          "type": "object"
        },
        {
          "description": "Remote absence or a transport error does not resolve a dispatched operation.",
          "properties": {
            "bundle": {
              "type": "string"
            },
            "host": {
              "type": "string"
            },
            "status": {
              "const": "awaiting_owner",
              "type": "string"
            }
          },
          "required": [
            "status",
            "host",
            "bundle"
          ],
          "type": "object"
        },
        {
          "properties": {
            "ok": {
              "type": "boolean"
            },
            "status": {
              "const": "finished",
              "type": "string"
            }
          },
          "required": [
            "status",
            "ok"
          ],
          "type": "object"
        },
        {
          "properties": {
            "bundle": {
              "type": "string"
            },
            "host": {
              "type": "string"
            },
            "reason": {
              "type": "string"
            },
            "status": {
              "const": "unknown_owner",
              "type": "string"
            }
          },
          "required": [
            "status",
            "host",
            "bundle",
            "reason"
          ],
          "type": "object"
        }
      ]
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
    "Suggestion": {
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
    "Suggestions": {
      "properties": {
        "areas": {
          "items": {
            "$ref": "#/$defs/Suggestion"
          },
          "type": "array"
        },
        "version": {
          "type": "string"
        }
      },
      "required": [
        "version",
        "areas"
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
          "$ref": "#/$defs/Outcome2"
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
    },
    "Versions": {
      "properties": {
        "common": {
          "description": "Exact versions known for every active request.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "newest": {
          "type": "boolean"
        },
        "requests": {
          "items": {
            "$ref": "#/$defs/Request"
          },
          "type": "array"
        },
        "source": {
          "type": "string"
        }
      },
      "required": [
        "source",
        "requests",
        "common",
        "newest"
      ],
      "type": "object"
    },
    "View": {
      "properties": {
        "logs": {
          "description": "Recent worker stderr. Reading it does not change the run or owner state.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "observation_error": {
          "type": [
            "string",
            "null"
          ]
        },
        "operation": {
          "anyOf": [
            {
              "$ref": "#/$defs/Status2"
            },
            {
              "type": "null"
            }
          ]
        },
        "result": true,
        "run": {
          "$ref": "#/$defs/Details"
        }
      },
      "required": [
        "run",
        "operation",
        "observation_error",
        "result",
        "logs"
      ],
      "type": "object"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema"
}
```
