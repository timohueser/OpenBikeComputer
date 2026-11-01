# obc data registry and releases

The data registry under `data/` (sources, regions, targets, archive datasets, machines), the
store, the archive, the layers that a bake makes, the releases that a target serves, and the
`obc data` commands with their errors and JSON output.

- Version: store `layout` 3; release manifest format 1. Each section gives its refusal rule.
- Writers: people (`data/*.toml`); `host/obc-data` (store, archive, targets, releases); the
  `obc data` binary of `host/obc-data-steps` (the steps of the products and the producers of the
  prepared sources).
- Readers: `host/obc-data`, `tools/data_registry.py`, `tools/check_data_sources.py`, `obc-bake`
  (`data/regions/`), the planner services and the device.
- Code authority: `host/obc-data`. Its Rust types generate [Error codes](#error-codes) and
  [JSON schemas](#json-schemas), and a test compares them with this file.
- Related: [Shared Wikimedia content](wikimedia-content.md#the-wiki-source) (the prepared source
  `wiki`), [planner release](planner-release.md), [offline planner bundle](planner-offline.md),
  [TrailView tiles](trailview-tiles.md), [OBCC catalog](obcc-catalog.md).

## Conventions

- All files under `data/` are TOML. A file with an unknown key is refused.

## Files

### `data/sources.toml`

One `[[source]]` table per source.

| Key | Type | Required | Meaning |
| --- | --- | --- | --- |
| `id` | string | yes | Unique, lowercase kebab-case |
| `kind` | string | yes | `data` (steps read it), `asset` (ships to users as it is) or `tool` (steps run it; nothing of it ships) |
| `group` | string | no | The heading under which the operator page lists a source that is not prepared |
| `used_by` | array of strings | yes | What reads the source, one or more of the [products](#products) `device` and `planner`, and of `search` and `trailview`, the search and TrailView layers of `planner`. `tools/check_data_sources.py` refuses a source without one, or with another name |
| `use` | string | yes | What `used_by` reads the source for, in one line. `tools/check_data_sources.py` refuses a source without it |
| `licence` | string | no | SPDX licence expression: ids or `LicenseRef-…`, joined by `AND`, `OR`, `WITH` and parentheses |
| `licence_url` | string | no | Where the licence text is |
| `attribution` | string | no | The credit text, as the product must show it |
| `obligations` | string | no | What the licence asks for, in words; `none` when it asks for nothing |
| `fetch` | table | yes | `kind` and `url`. See below |
| `hosts` | array of strings | no | Hosts the fetch reaches besides the host of `fetch.url`: lowercase letters, digits, `.` and `-`. `*.domain` is any subdomain |
| `version` | string | yes | How upstream names a version: `date`, `release`, `commit` or `digest` |
| `start` | string | no | The version that a step list reads while no serving release reads the source. It has the form of `version` |
| `refresh` | integer or string | no, `"manual"` | The policy, which the settings of a target can replace: `1` through `65535` whole days, or `"manual"` |
| `keep` | string | yes | Where the copy that a bake reads stays: `archive`, the [archive](#archive) holds the whole dataset; `store`, the store keeps what each fetch took; `prepared`, we keep what we make: the [archive](#prepared-sources) keeps a prepared source and a source that only a prepare reads, and R2 keeps our own asset. `tools/check_data_sources.py` refuses a source without it |
| `redistribute` | boolean | yes | The licence lets us give the upstream bytes to others |
| `credential` | table | no | `env`: the environment variables a fetch needs; or `file`: the file that holds them. `~/` is the home directory. `register`: the `https://` page where a person gets it, and `hint`: what to copy there, in one line. See [Credentials of sources](#credentials-of-sources) |
| `publish_text` | boolean | no, `true` | Only for a `prepared` source of article text: `false` keeps the text out of its versions |
| `extent` | array of 4 numbers | no | The box outside which the source has no data: west, south, east and north in degrees, west < east, south < north. A box meets it when the two overlap; boxes that only touch do not meet |
| `areas` | array of strings | no | Geofabrik areas whose `geofabrik-poly` polygons hold the data inside `extent`. Needs `extent`. None means all of `extent` |

`fetch.kind` is one of:

| Kind | Fetch |
| --- | --- |
| `http` | A file, or one file per tile |
| `geofabrik` | A Geofabrik extract or `.poly` of an area |
| `glo30` | Copernicus GLO-30 tiles |
| `dtm` | A national terrain model service |
| `capture` | Requests to a query service or an API |
| `github` | A GitHub release asset or source archive |
| `by-hand` | A person orders or downloads the files at `url` |
| `installed` | A person installs it, or another source's build brings it. It has no `url` |
| `prepared` | `obc data source prepare` makes its versions in the archive, see [Prepared sources](#prepared-sources). It has no `url` |

`fetch.url` is an `https://` URL template. `{name}` stands for a value the fetcher fills:
`{version}` is the version, `{yymmdd}` a date version as `YYMMDD`, `{area}` a Geofabrik area, `{tile}` a
tile name. In `attribution`,
`{year}` and `{month}` are the year and month of the data, which the step that writes the
credit fills.

Rules:

- An id is listed once.
- Each kind of fetch but `installed` and `prepared` has a `url`, and the `url` starts with `https://`.
- A `prepared` source has `version = "date"`, `redistribute = true` and `keep = "prepared"`.
- Only the OSM sources and the prepared sources have a `refresh` in days. A prepared source
  changes when a person prepares a version.
- `refresh` in days needs `version = "date"`, because only a date version has an age.
- A credential has `env` or `file`, not both. A `file` is `~/.cdsapirc`,
  `~/.config/openbikecomputer/cdse-s3.env` or `~/.config/openbikecomputer/clms-key.json`.

No step reads a `dtm-*` source, a national terrain model: only the producer of the prepared
source `terrain` does, see [Prepared sources](#prepared-sources). It reads each `dtm-*` row with
`redistribute = true`.

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
| Planner climate and snow layers | `era5-land`; `hr-wsi` when its `extent` meets the bounds, `modis-snow` and `hansen-gfc` |
| Planner TrailView layers | The sources that each step reads, see [`planner`](#planner) |
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
| `layers` | array of strings | The parts that are on, sorted, each once. A product with parts bakes only while one of its parts is on; an update with no part on refuses |
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
| `obc data`, product `planner` | A region, its `countries` and its `time_zone` |
| `fixtures/build-map-package.sh` | The `box` regions of the fixtures |

`tools/data_registry.py box ID [--lat-first]` prints the box of a `box` region and refuses
every other kind.

### `data/planner.toml`

The options of the [planner layers](#planner) that are the same for each region.

| Key | Type | Meaning |
| --- | --- | --- |
| `osm.margin_m` | number | `planner/osm/bounds` of a region with a box reaches at least this far around the region, in metres: the search puts a place in the town of a place node up to 15 km away |
| `routing.profiles` | array of strings | The profiles of the routing package. The route catalog needs `touring`, `road`, `gravel`, `mtb` and `hiking` |
| `climate.first_year` | integer | The first of the ten years of ERA5-Land that the climate layer reads |
| `snow.seasons` | array of 2 integers | The first and the last season of the snow layer, and of a version of [`snow`](#snow) |

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
| `blocked` | A `data` or `asset` source has no `licence` |
| `stale` | A date fetch is older than its `refresh`, and upstream names a later version, the fetch is a capture, or the upstream check fails |
| `unused` | The serving release reads no fetch of the source |
| `ok` | Otherwise. A fetch without a serving version is never stale. A `manual` source and a `prepared` source are never stale by age |

Data age is the number of days from the serving version to today (UTC). A fetch is due only when
its age is greater than `refresh`. An unchanged successful check does not change data age: the
fetch stays due, but its state is `ok` until a later check finds a newer version. Probe cache
age is separate from data age.

Each fetch gives its params, serving version, state, reason, age and upstream observation. The
observation gives the result, the latest probe time and the last successful probe time and
version. A failed probe keeps the last success. A capture result has no probe time. The source row gives missing credentials, but a
credential blocks only a new fetch.

### Source overview

`obc data sources` also tells how this machine holds the copy of each source that a bake reads.
It reads only the [status of the archive](#status) and its manifests, the versions and the
manifests in the homes of the [prepared sources](#prepared-sources), the records of the store and
the cached Geofabrik index. It fetches nothing. The state of the table above stays the judgement
of the targets.

A source that only a prepare reads is a `data` source with `keep = "prepared"` that is not
prepared, such as a `dtm-*` row. The prepared source that reads it stands for it in a bake. The
producer of each prepared source names the sources that it can read.

| Field of `held` | Meaning |
| --- | --- |
| `rule` | `keep` for `keep = "archive"`, `prepare` for `keep = "prepared"`, `live` for `keep = "store"` with a `refresh` in days (the OSM sources), else `store` |
| `covers` | Where the source has data, in words: the region of each scope of the newest version of a prepared source, or `World`; else the `coverage` of its dataset; else the countries of its `areas` in the cached Geofabrik index, or the paths without an index; else the box of its `extent`, or `World` |
| `status` | The first that applies of the table below |
| `where` | The archive root of its dataset, the home of the prepared source that holds it, `store`, or the host of an asset that R2 keeps; `null` when nothing is held |
| `bytes` | The bytes held: the verified files of the dataset, the newest version of a prepared source, or the newest record of each request or the newest version in the store |
| `download_bytes` | For `partial` and `missing`: the bytes that an `archive get` still takes, from the listed files of the manifest or else the `bytes_estimate` of the dataset; `null` when unknown |
| `date` | The day of the last `archive get`, of the newest version that holds it, or of the newest file in the store |
| `archive_files` | The verified and the listed files of its dataset; `null` while no manifest lists a file |
| `store_versions`, `store_requests` | The versions of the source in the store, and its sets of params |
| `gap` | A `dtm-*` row that nothing holds: the terrain there is GLO-30 only |
| `no_dataset` | `keep = "archive"`, and no dataset of `data/archive/` has the source |
| `error` | Why its dataset or its prepared versions cannot be read |

| Status | When |
| --- | --- |
| `archived` | The dataset of the source is `complete` |
| `prepared` | A prepared source whose home has a version in the format of its producer; a source that only a prepare reads, which the newest such version, or a version whose region and scope its `extends` names, reads (its `inputs`; a manifest of such a version that cannot be read gives none); or an asset with `keep = "prepared"`, which R2 keeps |
| `live` | The `rule` is `live`: a bake reads it upstream when it is due |
| `held` | The store has a version of a source that a fetch takes whole: no params, and no `capture`, `glo30`, `dtm`, `installed` or `prepared` fetch |
| `partial` | The archive holds some files of its dataset, or the store holds requests or versions of another source |
| `missing` | Nothing is held |

The text output has one row per source in kind order, the tools under a heading of their own,
with the columns SOURCE, LICENCE, RULE, COVERS, STATUS, WHERE, SIZE and DATE, and then the state of
the source for each target.

`--layer [LAYER]` gives each name of `used_by`, or LAYER only, with the `data` sources and assets
whose `used_by` has it, but no source that only a prepare reads. It does not tell what a bake
still downloads; `--region` does.

`--region ID|PATH[,PATH…]` and `--box W,S,E,N` name a region as `update` does, with each optional
part on. They need the products of `obc data`: `obc-data-plumbing` refuses. A Geofabrik area
that the cached index lacks refuses. The output lists the step lists of the products for the region, as the [overview](#overview) does, but with no fetch: the sources are
the fetches that the step lists read, and the sources that only a prepare reads of the prepared
sources among them, by their `extent`, the bounds of their `areas` in the index, and for a national
model (`dtm-<cc>…`, `canopy-<cc>`) of a region that names countries, its country `<CC>`. A fetch
without params of a source that a fetch takes with params only picks a version, and is not listed.
A region that names no countries, such as a box, takes the countries of the areas of one country in
the cached index whose polygons hold a corner or the centre of one of its boxes. Only the overview
reads these countries; the step lists read the region as it is.

A product whose step list stops, because it needs a snapshot that the store lacks or does not suit
the region, is `incomplete`, with the reason. Its layer has the need `unknown`. Each source that
the product can read (`Product::reads`), that the listing did not reach and that meets the region
as above, is listed with `after_stop`: its need is `unknown`, but a prepared source keeps the need
of its coverage.

`for_region` gives what a bake of the region still needs of each source (`need`):

| `need` | When |
| --- | --- |
| `held` | The store has each fetch at the version that the step list reads, or the archive lists each of its files as verified; for a prepared source, a version in the format of its producer covers the region; for a source that only a prepare reads, the prepared source that reads it is `held` |
| `download` | A fetch is not held. Its bytes are known when the manifest of an archive dataset lists each of its files with bytes |
| `prepare` | No version of the prepared source covers the region, or the prepared source that reads it needs a prepare |
| `unknown` | A scope of the newest version has a Geofabrik area that the cached index lacks, or the step list stopped before it read the source (`after_stop`) |

A version covers the region when one of its scopes is `null`, or each box of the region is inside
one box of a scope that names no countries or each country of the region. `for_region` also counts
the fetches, the held fetches and their known bytes, the known bytes to download and the fetches
of unknown size. Each layer has the most of the needs of the sources that its steps read. The
output sums the known bytes to download, and names the sources of unknown size or need and the sources to prepare.

## Store

The store is the directory in `OBC_DATA_STORE`, or else `~/.cache/openbikecomputer/store/`.

| Path | Holds |
| --- | --- |
| `layout` | The layout number of the store, `3`, and a line end |
| `objects/<ab>/<sha256>` | One file, named by the lowercase hex SHA-256 of its bytes; `<ab>` is its first two characters. Read-only |
| `snapshots/<source>/<version>.json` | The snapshot record of one source version |
| `layers/<key>.json` | The receipt of the layer with that key, see [Layers](#layers) |
| `requests/<source>/<sha256>.json` | The files that a fetch with `NAME=VALUE` gave: `version`, `params` and `files` (names). The name is the SHA-256 of the compact JSON `[version, params]`, with `params` sorted. A record with no files selects no file |
| `runs/<id>/` | `status.json`, `log` and `stop` of one run, see [Runs](#runs) |
| `refreshed/<source>.json` | `{source, from, to}`: the version that `obc data source refresh` took last (`to`), and the versions that the targets served then (`from`) |
| `upstream/<source>/<sha256>.json` | The acquisition check descriptors and observation of one normalized request. The name is the SHA-256 of compact JSON sorted params. The observation has `checked_at` (UTC seconds or `null`), `result` and `last_success` (UTC seconds and version, or `null`). The result has `state`: `newest` with `value`, `failed` with `value`, `capture`, or `cannot_check` |
| `partial/` | Downloads that are not complete, the validators that resume them, and the layers that steps write. A collection empties it |
| `serve/<target>/<planner id>/` | The files that the services of a planner release read while [`obc data serve`](#serve) runs them |
| `follower/<target>/` | A store of its own with the objects of the releases that [`obc data serve`](#serve) follows, and nothing else |
| `unpacked/<sha256>/` | The one GeoPackage or FileGDB of the archived zip with that SHA-256, as an [`archive-features`](#fetch) capture unpacks it. `.complete` marks a whole copy. A bake from the store never reads it |
| `prepared/<id>/<version>/` | The work of a [prepare](#prepare) that is not complete: `selection.json`, `[scope, languages]` of the run; `journal.jsonl`, the files that it moved into the archive and the source versions that it read; and `work/`, the files that it did not move yet |
| `locks/` | One lock file per key and per run |

Rules:

- `obc data` refuses a store whose `layout` is missing or holds another number. An empty or new
  store directory gets `layout`. For a store without it or with `1`, the fix removes `layers/`,
  `releases/`, `runs/`, `operations/`, `settings/`, `regions/`, `code/`, `producers/` and
  `local/`, which hold records of an older format, and writes `layout`. A receipt of layout 1
  names the input kind `layer`: a clean cannot read it. Snapshot records and objects stay. For a
  store with `2`, the fix removes `runs/`, whose runs are event journals.
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
| `files` | One item per file: `name`, `url`, `size` in bytes, `sha256`, `retrieved` (`YYYY-MM-DDTHH:MM:SSZ`), and `archive`, the path in the [archive](#archive) of the file that gave the bytes, when the archive served them |

The `name` of a file is the part of its URL that identifies it in the source. For a `dtm`
or `capture` fetch, it is the URL after `fetch.url`, such as `#bbox=W,S,E,N/sub/a.tif`. For a URL template, it is the URL from the segment of the first
`{name}` other than `{version}` and `{yymmdd}`, such as `europe/monaco-261003.osm.pbf`; without
such a `{name}`, it is the last segment. A name is unique in a record: a file whose name the
record has for another URL fails the fetch.

A version is one or more segments joined by `/`. A segment has letters, digits, `.`, `_`, `+`
and `-`, and does not start with `.`. A `date` version is also a `YYYY-MM-DD` date, and a
`digest` version is 64 lowercase hex digits. The version of a `prepared` source that a step reads
is `<version>/<manifest sha256>`, see [Prepared sources](#prepared-sources).

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
  step list reads it when nothing else names a version (see [Versions](#versions)). So is the
  newest version of each request record (`requests/`), by the latest `retrieved` of its files, such as the extract of each Geofabrik area, and each version
  that `obc data source refresh` took (`refreshed/`).
- An object is reached when a reached snapshot record or a reached layer has it, or when its
  SHA-256 is a file of an output of a root release, or is in `fixtures/catalog.toml` or a JSON or
  TOML file below `fixtures/sources/`.
- An unpacked copy (`unpacked/<sha256>/`) is reached while a manifest of the
  [archive](#archive) lists a file with that SHA-256. While `OBC_DATA_ARCHIVE` names no root, a
  root or its `manifest/` does not exist, `manifest/` holds no manifest, or `manifest/` or a
  manifest in it cannot be read, each copy is reached, and `obc data` warns and names the root.
- A layer is reached when each of its inputs is reached: a snapshot input whose digest is the
  digest of the files that it names (`files`) of a reached record of its source, and a step input
  whose digest is the digest of the files that it selects of a reached layer of its step: the
  `files` of the input, or all files when it names none.

The plan lists what the collection deletes and what stays. What stays is one entry for each
reached snapshot record, with the reasons: `served by TARGET, …`, `newest of the source`,
`newest of a request` and `refreshed`. Then one entry for the reached layers of each step (`inputs kept`).
Then one entry for each kind of root that names objects that no reached record or layer has:
`release` or `fixture`. Then one entry for the unpacked copies that stay: `in the archive` or
`archive not read`. The size of an entry is the size of its files, and the plan gives the size
of `partial/`. The collection takes the store lock alone, or refuses to start while a run or a
fetch holds it. A run holds it until it finishes; a run of `serve` does not hold it. A collection
deletes each snapshot record, each object and each unpacked copy that is not reached, and empties
`partial/`. It deletes the `.complete` of a copy first, so a stopped collection leaves no damaged
copy that is marked whole. Receipts and upstream checks stay. A clean that cannot read a target
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
not. A try receives its body for at most 15 minutes; a stalled transfer is then a retry. It
retries a connection error, a cut-off or stalled body, a
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
| `capture` | A program writes the files, see below |
| `dtm`, `by-hand`, `installed` | None; the fetch fails. A `dtm-*` source is read through `terrain` |
| `prepared` | The manifest and the files that a step names, from the archive, see [Prepared sources](#prepared-sources) |

A `geofabrik` URL with `{yymmdd}` names the day of the data of the extract. Without a version, the
fetch reads the day from the `timestamp` of `<area>-updates/state.txt`; for more than one area,
it takes the earliest day.

`osm-planet` is the weekly planet file of one day, an `http` URL with `{yymmdd}`. No step reads
it: the [`wiki`](#wiki) preparation reads a planet file of it, and it gives the OSM credit.

A `capture` fetch runs in the explicit repository root. The offline system selector
of `uv python find` selects its Python and honors `UV_PYTHON`. A standard-library capture
runs that interpreter directly. A package capture uses `uv run --locked --offline
--no-default-groups --no-python-downloads --group GROUP python`, bound to that interpreter.
It prepares the locked group when the capture runs. Cached captures need no Python runtime.
The program writes each file of the request to a directory under `partial/`, and its progress to
standard error. The store takes every file in that directory but hidden, `.part` and `.tmp` files.
A program writes each file whole, so a failed run leaves no half file. A failed run keeps the
directory and writes no record. A directory older than the `refresh` in days of the source is
deleted before the run; a `manual` source keeps one version, so its directory stays. The next run
of the same request resumes from the directory, also on a later day. In the record, the `url` of a file
is `<fetch.url>#<query>/<path in the directory>`, with the `fetch.url` of the source whose record
takes the file. The fetch checks every record that it adds to before it writes one. Each fetch
takes the `NAME=VALUE` of its row, each once, and no other. A `capture` source without a row has
no fetcher yet; the fetch fails. `wikidata`, `wikipedia` and `commons` have no fetcher: only the
producer of [`wiki`](#wiki) reads them. `era5-land` has none: only the producer of
[`climate`](#climate) reads it. `sentinel-2-mosaics` has none: only the producer of
[`colour`](#colour) reads it. `modis-snow` and `hr-wsi` have none: only the producer of
[`snow`](#snow) reads them.

| Source | `NAME=VALUE` | Program | Packages | Query |
| --- | --- | --- | --- | --- |
| Each source of `data/trailview/ground.json` with the `protocol` `arcgis-features` or `archive-features` | `bbox`, `ground` | `tools/trailview/ground.py --source ID --fetch` | group `trailview` | `bbox=W,S,E,N&ground=DIGEST` |
| `corine-land-cover` | None | `tools/clms.py --fetch` | None | `dataset=0407d497d3c44bcd93ce8fd5bf78596a&file=3d9e2413-46ca-4f6e-abf8-b6eb429a4e48` |

- `bbox` is `WEST,SOUTH,EAST,NORTH` in degrees.
- The `corine-land-cover` file is `U2018_CLC2018_V2020_20u1.tif`, the CORINE Land Cover 2018
  raster of Europe. The capture orders the prepackaged `file` of the CLMS `dataset` through the
  [CLMS download API](https://eea.github.io/clms-api-docs/download.html), with an access token
  that the service key of the credential signs for. An API call that answers with a redirect
  fails. It polls the order every 30 seconds for at most 6 hours, downloads the zip archive of the
  finished order from its `https://` link without the token, and takes the file from that archive
  or from a zip archive in it. A download shorter than its `Content-Length` is tried again. A
  complete archive stays in the work directory until the run succeeds, so a run that fails after
  the download resumes without a new order. An archive that is not a zip is deleted, and the next
  run orders again.
- `ground` is the first 12 hex digits of the SHA-256 of the canonical JSON of the keys `protocol`,
  `layer` and `field` of the source's entry, and of what of its `legend` decides the features
  that a capture keeps: `match`, the sorted native classes and the `word` of `qualifier`. A new
  name of a class keeps the digest. The fetch refuses a digest that the entry does not have.
- A ground file is `features-NNNNN.geojson`: a page of the features that meet `bbox` and whose
  `field` the legend may name, in EPSG:4326 with that field and 6 decimals. Each feature is
  simplified by a quarter of a zoom 13 texel at the equator. A box without a feature gives a
  fetch without files.
- `arcgis-features` asks the service, pages by object id, and asks again for the ids that an
  answer lacks. It leaves out an id that the service no longer has, up to 0.1 % of the ids, and
  fails above that.
- `archive-features` reads the zip of the `fetch.url` of the source in the [archive](#archive),
  never upstream. Without that file in the archive, the fetch fails and names the fix. The
  capture reads the `layer` of the one GeoPackage or FileGDB of the zip, 5000 features to a page,
  in the order of their feature ids. It reads that file from its copy in `unpacked/<sha256>/` of
  the [store](#store). While the copy has no `.complete`, one capture at a time makes it: it checks the SHA-256 of
  the zip against its manifest, refuses while the disk of the store cannot take the members that
  the copy lacks and keep 4 GiB free, unpacks them, flushes them to disk and then writes
  `.complete`. A stopped unpack resumes with the members in place. Each later box reads the
  copy.
- An `arcgis-features` capture tries a request again after a failed or cut transfer, a 5xx or
  429 answer, an ArcGIS error without a 4xx code, 6 times in all. Another 4xx answer or ArcGIS
  error fails at once. The hidden file `.positions` records the object ids that the page numbers
  stand for; a resumed run that finds others deletes the pages.
- The version is the day of the fetch. Another day comes only from the store. A request that a started version of a `manual` source lacks joins
  that version, so requests over several days have one version.
- A source with a `credential` that is not on this machine fails before the program runs, and
  the error names the variables or the file. A capture that the [archive](#archive) serves needs
  no credential.

### Archive

The archive keeps whole datasets and the versions of the prepared sources on drives of their
own. `OBC_DATA_ARCHIVE` names its roots, separated by `:`. Without the variable, or with an empty
value, there is no archive. Each root is a complete archive. A dataset is wholly in one root, and
so are the versions of a prepared source. A prepared version has no other copy. Only `obc data archive` writes
`mirror/`, `manifest/` and `.partial/`, and only `obc data source prepare`,
`obc data source copy` and `obc data unlock --source` write `prepared/`.

| Path under a root | Holds |
| --- | --- |
| `mirror/<host>/<path>` | The file of the URL `https://<host>/<path>` |
| `kept/<dataset>/<path>` | A file that has no upstream URL. A person puts it there |
| `manifest/<dataset>.json` | The [manifest](#manifest-of-a-dataset) of the dataset |
| `prepared/<id>/` | The versions of the [prepared source](#prepared-sources) `<id>` |
| `.partial/<dataset>/` | The unfinished downloads of `archive get`. You can delete it when no `archive get` runs |

A file goes into `mirror/` only after its check: the size, the `ETag` and the MD5 that the listing
gives, and a `Last-Modified` day that is not after the one of the listing. The modification time of
a file in `mirror/` is the `Last-Modified` time of upstream.

#### `data/archive/<dataset>.toml`

One file for each dataset. The name of the file is the id of the dataset.

| Key | Type | Required | Meaning |
| --- | --- | --- | --- |
| `name` | string | yes | What the dataset holds, in one line |
| `sources` | array of strings | yes | The ids of the sources in `data/sources.toml` that read the dataset |
| `coverage` | string | yes | Where the dataset has data, in words, such as `World` or `Europe` |
| `version` | string | no | The version of the dataset, in the form of the `version` of its sources |
| `urls` | array of strings | no | One file for each `https://` URL |
| `s3` | array of tables | no | `bucket`, `prefix` and `match`. See below |
| `stac` | array of tables | no | `search`, `sas`, `collection`, `asset`, `bbox`, `days` and `names`. See below |
| `cmr` | array of tables | no | `provider`, `short_names`, `version`, `origin`, `bbox`, `days` and `names`. See below |
| `kept` | boolean | no | `true`: the files of the dataset are the files under `kept/<dataset>/`, and nothing downloads |
| `clms` | table | no | `dataset` and `file`: the prepackaged CLMS file of which the dataset holds the zip |
| `bytes_estimate` | integer | no | The bytes of the files of the dataset, from a listing of upstream, for the [source overview](#source-overview) while no manifest lists them, and the bytes that a get needs on its root when a listing gives fewer, such as a listing without sizes |
| `notes` | string | no | For people |

An `s3` table lists each object of an S3 `ListObjectsV2` of `bucket` under `prefix`. `bucket` is
the `https://` URL of the bucket, in path style or in virtual-host style, and the URL of an object
is `<bucket>/<key>`. `match` is a pattern for the key after `prefix`, in which `*` stands for
characters other than `/`. Without `match`, each object is listed. A request to
`https://eodata.dataspace.copernicus.eu` is signed with AWS Signature Version 4, the region
`default` and the keys of `~/.config/openbikecomputer/cdse-s3.env`.

A `stac` table lists the `asset` of each item of a STAC search: a `POST` to `search` with the
`collection`, the `bbox` `[west, south, east, north]` and the `days` `[first, last]` as RFC 3339
times, 1000 items a page, and only the `id` and the `href` of the asset of each; a page names the
next one with a link of `rel = "next"`, whose `body` the next `POST` sends. A `cmr` table lists the
data file under `origin` of each granule of the NASA CMR granule search of `provider`,
`short_names`, `version`, `bbox` and `days`, 2000 granules a page. A get lists `cmr` before `stac`,
and tries a page again as a bucket listing. Both list only a file whose name has one of `names`, at
its URL without the query. A download of an Azure blob of a `stac` table adds the SAS token of its
storage account and container from `<sas>/<account>/<container>`, renewed 15 minutes before the
`msft:expiry` of its answer. No log line, manifest or error holds a query of a URL. A download under
the `origin` of a `cmr` table sends `EARTHDATA_TOKEN`, an Earthdata Login user token, as a Bearer
token. A file whose name ends with `.hdf` or `.tif` enters `mirror/` only when it starts with the
magic of HDF4 or TIFF, so a login page is no file; `verify` checks the magic too. SIGINT or
SIGTERM stops a get after the files that it takes, and the get fails. A file that a stopped get did
not try is not in `missing`; `listed` keeps the dataset incomplete. A file in place without a size
or a day upstream downloads again.

Each `obc data archive` command and a test of `host/obc-data` refuse a dataset that breaks a rule:

- Each id of `sources` is a source of `data/sources.toml` with `keep = "archive"`. The sources
  of a dataset have the same `version` kind.
- A dataset has `kept = true`, or else `urls`, `s3`, `stac`, `cmr` or more of them.
- A dataset of `release` or `commit` sources, and a kept dataset, has `version`.
- A dataset of `digest` sources has one URL and no `s3`.

The version of a dataset:

| `version` of its sources | Version of the dataset |
| --- | --- |
| `date` | `version`, or else the latest `Last-Modified` day of its files. While no file has one, the version of the manifest before, or else the day of the first `archive get` |
| `release`, `commit` | `version` |
| `digest` | The SHA-256 of its one file |

#### Manifest of a dataset

`manifest/<dataset>.json` is a JSON object:

| Key | Holds |
| --- | --- |
| `dataset` | The id of the dataset |
| `sources`, `name`, `coverage`, `notes` | As the definition has them |
| `version` | The version of the dataset |
| `got` | The day of the last `archive get`, `YYYY-MM-DD` in UTC |
| `files` | Each verified file: `path` under the root, `bytes`, `sha256`, and, when upstream gives them, `modified`, its `Last-Modified` day, and `etag`, the `ETag` of a bucket listing. A file without `sha256` makes the manifest unreadable |
| `missing` | Each listed file that the get tried and the root does not hold: `path`, `bytes` when known, `error`, and for a kept file the `sha256` that the manifest recorded |
| `listed` | The files that the last get listed. The dataset is complete when each is in `files` and in place |
| `clms_dataset_uid`, `clms_file_id` | The `dataset` and `file` of `clms` |

#### Get

`obc data archive get DATASET [--root PATH]` puts the dataset into one root:

- The get refuses while a root of `OBC_DATA_ARCHIVE` does not exist, such as a drive that is not
  mounted: that root can hold the dataset.
- The root is `--root`, which must be a root of `OBC_DATA_ARCHIVE` and refuses while another root
  has the manifest of the dataset. Else it is the root that has the manifest, or else the
  `.partial/<dataset>/` of a get that stopped before it wrote one. Else, for a kept
  dataset, the first root that has `kept/<dataset>/`; for another dataset, the first root that has
  the free space for the listed bytes and 4 GiB more.
- The listing: for each URL, a `HEAD` request for its `Content-Length` and `Last-Modified`;
  for each `s3` table, the pages of the bucket listing, with the size, the `ETag` and the
  `LastModified` of each object, each page with the retries of [Fetch](#fetch). A page that fails
  fails the get, and the manifest stays as it was. A key that names no plain path is left out with
  a warning. For a kept dataset, each file under `kept/<dataset>/`; a folder there that cannot be
  read fails the get. The listing is the list of the files of the dataset: a file that upstream
  removes leaves the manifest.
- A file of the manifest that is in place with its `bytes` stays without a download when upstream
  gives its size, its `ETag` and its `Last-Modified` day, as far as upstream gives them. Only a
  bucket listing gives the `ETag`: the `ETag` of a `HEAD` is not stable.
- A file on disk with the `Content-Length` and the `Last-Modified` day of upstream as its size and
  modification day is read in place once.
- Each other file downloads into `.partial/<dataset>/` with the retries and the resume of
  [Fetch](#fetch), four files at a time, and goes into `mirror/` after its check. An `ETag` of 32
  hex digits in a bucket listing, which S3 and GCS give for a file of one part, is its MD5.
- A get never gives a kept file other bytes than the `sha256` that the manifest records in
  `files` or `missing`. A new kept file is read in place for its SHA-256. To
  replace a kept file, a person puts the new file in place and removes its entry from the
  manifest.
- A file that the get cannot take keeps its entry of the manifest before while that file is in
  place; else it is in `missing`. The get fails when a file failed.
- The get writes the manifest each minute and at the end. The next get resumes. Only one get or verify of a dataset runs at a time.

#### Verify

`obc data archive verify DATASET` reads each file of the manifest and compares its size and
SHA-256 with the manifest. A file that differs, or is not in place, moves from `files` to
`missing`, and then does not serve. A mirrored file that differs moves to
`.partial/<dataset>/rejected/`, and the next `archive get` downloads it again. A kept file stays in
place. The verify fails when a file differs or is not in place.

#### Status

`obc data archive status` gives each root with its free space, and each dataset of
`data/archive/`: its sources, coverage, root, state, version, the day of the last get, the files
that the last get listed and their known bytes, the verified files and their bytes, and the
`bytes_estimate`. It also names each source with `keep = "archive"` that no dataset has. It reads only the manifests and the sizes of the files.

| State | When |
| --- | --- |
| `complete` | Each listed file is verified |
| `incomplete` | A listed file is missing, or the last get listed no file |
| `absent` | No root has the manifest |
| `unreadable` | The manifest cannot be read, has a file without `sha256`, or has no `version`: no file of it serves |

#### Read

- Only a source with `keep = "archive"` reads a dataset. A prepared source reads its versions as
  [Prepared sources](#prepared-sources) says.
- A fetch reads the manifests of each root, in the order of `OBC_DATA_ARCHIVE`. When two roots
  list the same path, the first one serves. A root that does not exist gives a warning.
- Only a file that a manifest lists serves: an unlisted file can still be downloading. A
  `sha256` can be in uppercase or lowercase hex.
- A problem of the archive never stops a fetch that the store or upstream can serve. A manifest
  that cannot be read or has no `version`, or a listed file of another size, gives a warning, and
  the file does not serve. Only a file that the fetch takes, with another SHA-256 than its
  manifest, fails the fetch.
- The store takes a copy of the file, never a link.
- The record of a file keeps the upstream `url`, and its `archive` names the path under the root.

An `http`, `glo30` or `github` fetch looks for the file of each URL that the store does not have,
before it downloads. The day of a file stands for `Last-Modified`: its `modified`, or else the
version of its dataset when that is a day. For a `date` source without a version, the latest day of
its files is the version. Only a URL that the archive has no day for gets a `HEAD` request
upstream, so a fetch offline needs a day in the archive for each of its URLs. The archive file
serves when it is the version: its SHA-256 is that of the record of the version or of the `digest`
version, or else its day is on or before the day of a date version. Otherwise the fetch writes
why and downloads the file. `obc data source refresh` asks upstream, not the archive.

The `corine-land-cover` capture looks for the manifest with its `dataset` and `file`. When the
archive has the zip, the capture checks its SHA-256 against the manifest, and
`tools/clms.py --archive ZIP` takes the file from that zip as from an order. An `archive-features` ground capture reads only
the archive, see [Fetch](#fetch).

### Credentials of sources

`obc data source credential ID` reads the values of the credential of source ID on standard
input, one line each, in the order of `fields` in `obc data setup`. It never takes a value as an
argument. It writes them to a file under the home directory, with mode 0600, and makes a missing
directory with mode 0700. The file keeps its other lines. The output names the file and the
values, never a value.

| Credential | File | Lines |
| --- | --- | --- |
| `env` | `~/.config/openbikecomputer/credentials.env` | `NAME=VALUE`; a leading `export ` and one matching pair of quotes around the value are removed |
| `file = "~/.cdsapirc"` | `~/.cdsapirc` | `key: VALUE` |
| `file = "~/.config/openbikecomputer/cdse-s3.env"` | that file | `CDSE_S3_ACCESS_KEY=VALUE` and `CDSE_S3_SECRET_KEY=VALUE` |
| `file = "~/.config/openbikecomputer/clms-key.json"` | that file | None: a person saves the JSON service key whole, as the `hint` says, and the command refuses it |

Each `obc data` command but `obc data ui`, and `obc bake publish --target r2`, loads
`credentials.env` when it starts, and its steps get the variables. A variable that the
environment already sets wins. `overridden` in the output names such variables: the new value serves only after they are removed from the environment.

Each acquisition check has 15 seconds. The store keeps its answer or failure for one hour.
Equal normalized requests share a check. A changed acquisition URL or check kind invalidates
the cached observation. Refresh policy changes do not invalidate acquisition evidence.

| Source | Check |
| --- | --- |
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
| `shape` | `tiled`: built per grid leaf. `whole`: built once over the full coverage. `whole` with named partitions: the last part of the name of each step names a declared partition, else the plan fails. A partition stales, bakes and is published on its own, as a step is |
| `format` | The number that the readers of its files check. Where a reader has a constant, the layer uses it: `obct::SURFACE_VERSION` for `device/terrain`, `obcm::VERSION` for `device/map`, the router `FORMAT` and `CATALOG_SCHEMA_VERSION`. `device/landmarks` has the sum of `obcm::landmarks::SECTION_VERSION` and `obcm::peaks::VERSION`: each only grows. Another layer has the format that its spec states, or 1 |
| `revision` | The number that a person changes in the change that improves the output. It starts at 1 |
| `readers` | The programs or layers that read its files. An internal layer has none |

| Layer | Steps |
| --- | --- |
| `osm` | `device/source/<area>`, `device/region-osm`, `device/osm`, `planner/source/<area>`, `planner/osm`, `planner/osm/bounds`, `planner/terrain-osm/<area>`, `planner/terrain-osm` |
| `device/terrain` | `device/reference/<i>-<j>`, `device/terrain/<i>-<j>` |
| `device/map` | `device/map/<i>-<j>` |
| `device/landmarks` | `device/landmark-content…`, `device/peak-content…`, `device/landmarks/<i>-<j>`, `device/peaks/<i>-<j>` |
| `device/catalog` | `device/catalog` |
| `planner/basemap` | `planner/basemap`, `planner/assets` and their grids |
| `planner/terrain` | `planner/reference`, `planner/terrain` and its grid |
| `planner/places`, `planner/climate`, `planner/snow`, `planner/trailview-features`, `planner/trailview-rules`, `planner/trailview-colour` | The step of the same name and its grids |
| `planner/trailview-ground` | `planner/trailview-material`, `planner/trailview-lithology`, `planner/trailview-region`, `planner/trailview-ground` and its grid |
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

A step input enters a key with its digest, not with its key. A snapshot version enters a key the same way, by the digest of its files.

The recipe of a step is the SHA-256 of the same object without digests and with `client`: each
input as `{"kind", "name", "files"}` for a step and `{"kind", "name", "version", "params",
"files"}` for a snapshot, `params` and `files` sorted, and the inputs sorted by their compact
JSON. It holds no digest, so a plan has it also for a step whose key waits for a fetch or
another build.

### The step contract

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

### Offline

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
| `cpu_ms` | User and system time in milliseconds of the process of the step and of the children that it waited for, such as Osmium (`wait4`). `null` when the system does not report it |
| `peak_rss_bytes` | The peak resident set of the process of the step, or of the largest child that it waited for. It is not the sum of processes that run at the same time. `null` when the system does not report it |
| `own_process` | Whether the step ran in a process of its own. Without it (`false` when absent), `cpu_ms` and `peak_rss_bytes` can be those of the engine, and no estimate reads the peak |
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

### Device-map publication

The `device/catalog` step consumes selected layer paths and their receipt metadata. Map and terrain
layers list their empty cells beside the cells: a map layer in `cells/<band>/empty.json`, a terrain
layer in `metadata/`, with the decoded reference credits. The `cells/` and `terrain/` payloads
and `metadata/` are client files: a target holds the metadata that the catalog and the map cells
read, so another machine bakes them without the leaf. Every file of the catalog layer is a client
file: the pinned satellites in `objects/`, which it names `/objects/<sha256>`, and the root `catalog.json`
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
`update --dry-run` and a read-only command create none. A failure after the run starts returns its id in `error.run`.

A run fetches the fetches of a plan, one after another, with the fetchers of [Fetch](#fetch).
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
  the physical memory of the machine. A step reserves its estimated peak (`peak_rss_bytes` of the estimate).
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
  [JSON schemas](#json-schemas)): its arguments after `obc data`, its process (while it starts,
  the process that reserved it, then the process that it starts), its state (`starting`,
  `running`, `ok`, `failed` or `stopped`), each phase that it entered with its state and why it
  waits (`waiting`, see [Router ready](#router-ready)), the progress of each layer that it bakes
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

### Versions

A step list gets the version of each fetch that it reads from one function
(`obc_data::product::version`), by three rules:

1. A due fetch reads the newest version upstream, and the update uses that version until it ends.
   When the upstream of an HTTP fetch does not serve (no connection, a timeout, HTTP 404, 408, 429
   or 5xx) or the credential is missing, the fetch reads rule 2, and a note of the
   [review](#review) names the fetch, the error and the version that it reads. Another failure,
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
[Prepared sources](#prepared-sources).

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
For a Box, the cached index selects candidates by polygon coverage: of the areas whose polygons
meet the Box, each area that the others cover drops out, largest first. Full `.poly` files must then cover the
complete Box. A gap tries the parent extract before selecting bulk inputs.
An uncovered Box gives an actionable error. Country metadata does not remove required geography.

Each selected area has a private `<product>/source/<area>` step. It reads the extract and full
`.poly` at their separate exact versions, and writes `source.osm.pbf` and `source.poly`. A Box
also records its selected index version. These receipts retain active request provenance even
when a capture holds older extract or polygon bytes. Each product sorts and deduplicates areas.

A multi-area input uses prepared `osmium merge --with-history`, then `time-filter` at the latest
object timestamp present in the union. For an overlapping object, its highest supplied version
wins. Unique supplied objects remain. Conflicting payloads
at the same object type, id and version are malformed OSM inputs.

The merge, the map crop, the cut and the terrain of the planner run the Osmium of `OBC_OSMIUM`, or
else `osmium` on the `PATH`. Without it, an update blocks the device layers that need it, the plan
of the planner fails for more than one source area, a box or `planner/trailview-features`, and a
build of `planner/terrain` fails. Each asks to install Osmium or set `OBC_OSMIUM`. It does not
download or install a tool.

The steps read `copernicus-glo-30` and the national terrain models of `terrain`,
and the step list reads files such as a `.poly` and the GLO-30 tile list, each at its version (see
[Versions](#versions)). `copernicus-glo-30` and its tile list are `manual`.

A leaf reads the archive tiles of its terrain cells: those of the windows that the crest rule of
`obct-terrain.md` §9 reads, which have a halo of two postings. A model whose `extent` meets one of
those tiles in the version of `terrain` that the target reads (see [Prepared sources](#prepared-sources)):
the step list reads its manifest, and the leaf reads the blocks `18/<bi:04>/<bj:04>.tif` of the
manifest that hold those tiles and its `index.json`. A leaf where the version has no block of its
tiles has no `device/reference` layer. Where a tile whose block the version lacks meets the
outline of the region and is in the `geofabrik-poly` polygon of one of the `areas` of a `dtm-*`
model, a note on `device/terrain` names the model and the command that adds
the region; the version that adds it is a new input of the leaves that read it.

| Layer | Reads | Options | Files |
| --- | --- | --- | --- |
| `device/region-osm` | The selected PBF of each `device/source/<area>`; only for multiple areas | None | `osm.pbf`: the available-snapshot union |
| `device/osm` | The PBF of one `device/source/<area>`, or `device/region-osm` | `leaves`: `[i, j]` of each leaf | `osm/<i>-<j>.osm.pbf`: the `osmium extract --strategy smart --set-bounds --generator obc-data` of the square of the leaf and one µdeg around it, without the header options of the input, such as its replication fields. Each Osmium run writes at most two extracts: the rectangle around the leaves is halved along its longer side, and each half is cut from the extract of its parent, until each part is one leaf. Each leaf has the bytes of the direct extract. The metrics name the version (`osmium`) |
| `device/map/<i>-<j>` | `device/osm`, the file of the leaf; `land-polygons`; `device/terrain/<i>-<j>`, which the contours, the nav graph and the POIs read | `leaf`: `[23, i, j]`; `cells`: for each band of the recommended band table (`obca-assembly.md`), `[ci, cj]` of each cell of the band in the leaf that the outline touches; `partial_cells`: the ids of those cells that the source coverage does not hold | `cells/<band>/<ci>/<cj>.obcm` for each cell with content or with partial coverage; `cells/<band>/empty.json`: the ids of the other cells of the band. A cell has the bytes that one cut of the whole leaf with all bands writes, with `builder/presets/schema.json` and without landmarks or peaks. The step decodes the OSM of the leaf once |
| `device/reference/<i>-<j>` | `terrain`, without params: the blocks of those tiles in its version and `index.json`; only when the version has one of them | `source`: `terrain`; `tiles`: the ids `<ti:04>/<tj:04>` of the archive tiles that the terrain cells of the leaf read | `reference/`: the reference archive (`host/obc-dem/reference/README.md`) of those tiles that the index names, each cut out of its block, and the `index.json` of the version cut to them, with `tiles` and the `sha256` of the pixels of each tile. A Python step with the group `terrain-reference` |
| `device/terrain/<i>-<j>` | `copernicus-glo-30`, `tile=` of each tile that the square of a cell reaches and that `copernicus-glo-30-tiles` names. A square without a tile is sea. A leaf without a tile reads no snapshot. `device/reference/<i>-<j>` when the leaf has one | `posting_log2` and `cell_log2` of OBCT v1; `cells`: `[ci, cj]` of each terrain cell in the leaf that the outline touches | `terrain/<ci>/<cj>.obcd` for each cell with a height (`obcc-catalog.md` §13), the bytes that `obc-bake terrain --reference` writes from the same tiles and archive; `metadata/empty.json`: the ids of the cells without a height; `metadata/credits.json`, when a cell reads a national model: `key`, `product`, `attribution` and `licence` of each model that a cell reads, as the reference archive states them |
| `device/landmark-content[/<area>]`, `device/peak-content[/<area>]` | `wiki`, without params: `common.tar.zst` and the shards of the `2^20` µdeg cells that the `.poly` of the area touches, of its manifest; regional `geofabrik-extracts` and `geofabrik-poly` | None | `landmarks/content.json` or `peaks/peaks.json` and device photos. The metrics count the facts that the version lacks, by kind (`absent`), and the run log names them. No client reads the layer: the device cell layers publish the final assets |
| `device/landmarks/<i>-<j>`, `device/peaks/<i>-<j>` | Every area's landmark content and `device/osm`, the file of the leaf; or every area's peak content | `cell_log2`: 18; `cells`: `[ci, cj]` of each network cell of the leaf: the `network` list of `cells` of `device/map/<i>-<j>` | `landmarks/<ci>/<cj>.bin` or `peaks/<ci>/<cj>.bin` for each cell that owns content (`obcc-catalog.md` §14.3). A landmark joins the OSM objects of the leaf that name it |

`<i>`, `<j>`, `<ci>` and `<cj>` have four digits or more, as in a cell id.

The content of a collection reads the shards of [`wiki`](wikimedia-content.md#the-wiki-source)
at the version that the target reads, and the extract and the `.poly` of its area. The step makes
the boundary and the candidates or the summits again from them, offline. A fact that the version
lacks is no content, as an OSM object newer than the version has none. Without a version of
`wiki`, the content layers block with "Fix: Run `obc data source prepare wiki`.", and the layers
that read them block too. A version whose manifest lists no shard of the area blocks them as well.

A single-area definition keeps the unsuffixed content layer names. Other definitions have one
content layer per collection and source area. Article inputs keep separate file views;
canonical content merge rules deduplicate overlaps without losing unique articles.

`device/source/<area>`, `device/region-osm`, `device/osm`, `device/reference/<i>-<j>` and all
content layers are private intermediates. Their complete records stay in release provenance.

#### `planner`

The planner reads the `countries` of the region, or else the countries of its source areas in
the Geofabrik index, sorted; a region without countries is blocked. It reads the `time_zone` of the
region, or else `UTC`. It uses the shared source selection above. Its bounds enclose the
requested Box or union of selected outlines. Each snapshot, such as `copernicus-glo-30`, the extract, the `.poly` and the GLO-30 tile list, is
at its version (see [Versions](#versions)), as for `device`. The other options come from
[`data/planner.toml`](#dataplannertoml). A GLO-30 input reads the tile
of each 1° square that its box touches and that `copernicus-glo-30-tiles` names; a box at sea
reads no snapshot. `planner/terrain` reads the mean of the national terrain models as `device`
reads their maximum: the blocks `mean/18/<bi:04>/<bj:04>.tif` that hold the archive tiles of its
`bounds` in the version of `terrain` that the target reads, and `index.json`.

| Layer | Reads | Options | Files |
| --- | --- | --- | --- |
| `planner/osm` | The selected PBF of each `planner/source/<area>` | `path`: `osm.pbf` for one area; none for multiple areas | `osm.pbf`: one extract as it is, or the available-snapshot union |
| `planner/osm/bounds` | `planner/osm` | For a region with a box, `bbox`: west, south, east and north of the bounds of the region, widened by `osm.margin_m`, within ±180° and ±90°. For a region without a box, `path`: `osm.pbf` | `osm.pbf`: for a region with a box, the `osmium extract --strategy smart -S types=multipolygon,boundary --set-bounds --generator obc-data` of `bbox`, in which each multipolygon and each boundary that `bbox` touches is complete, and the metrics name the version (`osmium`). For a region without a box, `planner/osm` as it is: its extracts are its whole area |
| `planner/basemap` | `planner/osm/bounds`; The selected jar of `protomaps-basemaps`; `natural-earth`, `water-polygons`, `land-polygons`, `daylight-landcover`, `qrank`, `pgf-encoding` | `bounds` of the region; `attribution` of `osm-planet`, `natural-earth` and `daylight-landcover` | `basemap.pmtiles`: the Protomaps map at zooms 0 to 14 |
| `planner/reference` | `terrain`, without params: the mean blocks of the tiles in `bounds` of `planner/terrain` and in the ring of tiles around them, and `index.json`; only when the version has one of them | `source` and `tiles` as for `device/reference`, of the tiles of `bounds`; `pool`: `mean` | `reference/`: the mean archive of those tiles. A Python step with the group `terrain-reference` |
| `planner/terrain-osm/<area>` | `terrain-osm`, `area=<area>`, for each source area | `path`: `source.osm.pbf` | `source.osm.pbf`: the extract as it is |
| `planner/terrain-osm` | Only for more than one source area: `source.osm.pbf` of each `planner/terrain-osm/<area>` | None | `osm.pbf`: their union, as `planner/osm` merges |
| `planner/terrain` | The GLO-30 tiles of `bounds`; `planner/reference` when there is one; `planner/terrain-osm`, or for one source area its `planner/terrain-osm/<area>` | `bounds`: west, south, east and north of the zoom 10 tiles that the bounds of the region touch and of their neighbours; `attribution`: the credit of GLO-30; `osm_attribution`: the credit of `osm-planet` | `terrain.mbtiles`: the terrain tiles of [the planner release](planner-release.md#terrain-tiles), the bytes that `planner-dem` writes from the same tiles, archive and OSM. `terrain-osm` is the Geofabrik extract with `refresh = "manual"`: the terrain keeps the extract of each area that it read first, and a newer `geofabrik-extracts` does not build the terrain again. `obc data fetch terrain-osm area=<area>` puts a newer extract in the store, which the next update reads. Where GLO-30 has weight, its heights follow the OSM within 300 m of `bounds` (`host/obc-dem/src/shape.rs`): the water side of `natural=coastline` is at 0 m up to 45 m from the line; within 45 m of the top of GLO-30 within 30 m of a `natural=peak` or `natural=volcano` with a numeric `ele`, the cone of its flanks continued from 45 m and 90 m, only rising and never above `ele`. With OSM, the metrics name the Osmium version (`osmium`) |
| `planner/routing` | `planner/osm/bounds`, the GLO-30 tiles of the bounds of the region, and `planner/reference` when there is one. The heights of the roads are those of `planner/terrain` without the OSM corrections | `region` (the last part of the region id), `bounds`, `profiles` and `countries`; `attribution` and `terrain_attribution`: the credits of OSM and GLO-30. The import applies the German access defaults | `routing/`: the package of [the route package contract](route-package.md) with `overlays.sqlite` and `route-catalog.json`; `blocks/`: the routing blocks of the grid cells: shared page packs, one manifest for each cell, `blocks.json` and `catalog.json`; `routes/<cell>.json`: the records of `route-catalog.json` that name the cell, with a final newline |
| `planner/overlays` | `planner/routing` | `attribution` of `osm-planet` | `overlays.pmtiles`: the route networks and the access of `routing/overlays.sqlite`, as [the planner release](planner-release.md) defines it |
| `planner/assets` | `protomaps-assets`, `tangrams-icons` | None | `assets/fonts/` and `assets/sprites/` of the assets archive; `assets/sprites/LICENSE.txt`: the MIT notice of `tangrams-icons` |
| `planner/model` | `query-model` | None | `model/`: `model.int8.onnx`, `tokenizer.json` and `tokenizer_config.json` of the archive, and `labels.json`, the labels of the query schema |
| `planner/search/policy` | `nominatim-country-data` | None | `policy.json`: the country names, postcode formats and address levels of the archive; `country_osm_grid.sql.gz`: the country grid of the archive, as it is |
| `planner/search/dump` | `planner/osm/bounds`, `planner/search/policy` | `country`: the first of `countries` in lowercase, the country of a place outside each country polygon | `search.jsonl.zst`: the search records that `obc-search-bake` writes, one JSON object per line, with the keys of each object in byte order |
| `planner/search/records` | `planner/search/dump` | None | `pois.jsonl.zst` and `addresses.jsonl.zst`: the records of the dump with a POI and with an address |
| `planner/search/pois`, `planner/search/addresses` | `planner/search/records`; POIs also read the selected `device/landmark-content*` and `device/peak-content*` outputs | `component` (`pois` or `addresses`); `region`, `bounds` and `countries` as for `planner/routing`; `time_zone` of the region; `attribution` of `osm-planet` | `<component>/<region>.sqlite`: the search database of the component, schema 6. The key holds no SQLite version: another Python patch release can give other database bytes. The metrics name the version as `sqlite` |
| `planner/places` | `planner/search/pois` | None | `places.pmtiles`: the rider places of the POI database, at zoom 11 |
| `planner/climate` | The files of [`climate`](#climate) of the blocks of the outline of the region, as `climate` defines the blocks of a region: the orography and the ten years from `climate.first_year`. A version that lacks one of them refuses | `bounds`, `first_year`, and `attribution` of `era5-land`, whose `{year}` is the year after the ten years, `first_year` + 10; `climate`: the version that it reads | `climate.pmtiles`: [the climate archive](planner-climate-tiles.md). A cell of a block that the step does not read is missing |
| `planner/snow` | The files of [`snow`](#snow) of the blocks of the outline of the region, as `snow` defines the blocks of a region, for each season of `snow.seasons`. A version that lacks one of them refuses | `bounds`, `seasons`; `attribution`: the credits of `hr-wsi` when its `extent` meets the bounds, `modis-snow` and `hansen-gfc`; `year`: the year of the `snow` version, which fills the `{year}` of `hr-wsi` | `snow.pmtiles`: [the snow archive](planner-snow-tiles.md). Its tiles of zoom 6 to 12 have the bytes of each region whose step reads their block |
| `planner/trailview-features` | `planner/osm`, `planner/terrain/grid`, `water-polygons` | `bounds`; `kinds`: the kind table of `data/trailview/kinds.json`; `roads`: the name, the grade, the smoothing length and the alignment fields of each class of `data/trailview/roads.json`, as an array in its order; `attribution` of `osm-planet` | `trailview-features.pmtiles`: [the feature archive](trailview-features.md) at zooms 8 to 13 |
| `planner/trailview-rules` | `data/trailview/` of the repository | `ruleset`: the digest of its JSON files by their paths in it. The step fails when the files differ | `rules.json`: [the TrailView rules](trailview-tiles.md#rules); `regions.json`, `materials.json` and `ground.json`: the region ids, the region areas, the ecoregions, the places, the vocabulary and the ground sources that the ground steps read |
| `planner/trailview-material` | `materials.json` and `ground.json` of `planner/trailview-rules`; `lithology.pmtiles` of `planner/trailview-lithology`; each [material source](trailview-tiles.md#ground-sources) that meets the bounds: `osm.pbf` of `planner/osm` for `osm-planet`; `esa-worldcover`, `tile=` of each 3° tile that the bounds touch and that `esa-worldcover-grid` lists; the files of [`colour`](#colour) of `trailview.colour_year` of the blocks that the outline of the region meets, for `colour`. A version that lacks one of them refuses; a `capture` source with `bbox=<bounds>`, another without params | `layer`: `material`; `bounds`; `year`: `trailview.colour_year` | `material.pmtiles`: the material bytes and the barren shares of [the ground](trailview-tiles.md#ground) |
| `planner/trailview-lithology` | `materials.json` and `ground.json` of `planner/trailview-rules`; each lithology source that meets the bounds, as for the material | `layer`: `lithology`; `bounds` | `lithology.pmtiles`: the lithology bytes |
| `planner/trailview-region` | `regions.json` of `planner/trailview-rules` | `layer`: `region`; `bounds` | `region.pmtiles`: the region bytes |
| `planner/trailview-ground` | `planner/trailview-material`, `planner/trailview-lithology`, `planner/trailview-region` | `layer`: `ground`; `bounds`; `attribution` of the sources of the three | `ground.pmtiles`: [the ground archive](trailview-tiles.md#ground) |
| `planner/trailview-colour` | The files of `colour`, as for the material | `bounds`, `year`, `variant` (`trailview.colour`); `attribution` of `sentinel-2-mosaics` with the year | `q1.pmtiles` to `q4.pmtiles`: [the colour archives](trailview-tiles.md#colour) |
| `planner/trailview-snow` | `snow.pmtiles` of `planner/snow` | `bounds` | `00.pmtiles` to `45.pmtiles`: [the snow days](trailview-tiles.md#snow-days), one archive per band |
| `planner/trailview-canopy` | The blocks of [`forest`](#forest) that the outline of the region meets, as `forest` defines them. A version that lacks one of those blocks refuses | `bounds`; `attribution`: the credits of the canopy models with a tile in those blocks and of the leaf type | `trailview-canopy.pmtiles`: [the canopy archive](trailview-tiles.md#canopy) |

Each map producer has a `planner/<kind>/grid` step, where `<kind>` is `basemap`, `places`,
`terrain`, `overlays`, `climate`, `snow`, `trailview-features`, `trailview-ground` or
`trailview-canopy`. It reads only the archive of its producer.
`planner/trailview-colour/<archive>/grid` reads one archive of the colour step, as the kind
`trailview-colour-<archive>`. `planner/trailview-snow/<band>/grid` reads one archive of the snow
day step, as the kind `trailview-snow-<band>`. It converts terrain MBTiles with the pinned Python
PMTiles library. It groups the tiles as
[grid publication](planner-offline.md#grid-publication-and-selection) says: each tile goes into
the archive of its zoom 11 parent, or its own tile when its zoom is less than 11, and zoom 13
terrain has archives of its own ([terrain tiles](planner-release.md#terrain-tiles)). Each
archive is packed as `maps/tiles/<kind>/<z>-<x>-<y>.pmtiles`, and its TileJSON
as `maps/<kind>.json`, with the transport rules of [the offline contract](planner-offline.md).
The step writes the packed bytes in `objects/<transport sha256>`, and selects `objects` and
`index.json` for clients. Its `index.json` has `format: 1`, `kind`, `map_zoom: 11`, `files`,
`source` and `metadata`. `files` maps logical names to source and transport hashes, sizes and
encoding. `source` names the converted archive bytes, or the raw MBTiles when it has no tiles. `metadata`
is the TileJSON. Empty terrain has TileJSON and no tile archives.

The parts of the planner are `map`, `routing`, `search`, `climate`, `snow`,
`trailview-features`, `trailview-rules`, `trailview-ground`, `trailview-colour`, `trailview-snow`
and `trailview-canopy`; the device maps are the part `device`. A planner step is in the list when a
part that is on makes it or reads it: `map` makes the basemap, places, assets and terrain grids,
`routing` the routing and overlays grids, `search` the model and the two search grids; the other
parts make their own steps. The fonts, the index and the catalog read only the grids that stay.
`trailview-ground` makes the material, lithology and region steps, and needs `trailview-rules`.
`trailview-snow` needs `snow`. All producers are intermediate layers. Grid steps select their `objects/` and their
`index.json`.

| Layer | Reads | Files |
| --- | --- | --- |
| `planner/routing/grid` | `planner/routing` | Packed `routing/blocks.json`, `routing/packs/<sha256>/pages.bin` and `pages.idx`, `offline/routing-cells/<cell>.json`, and `routes/tiles/<cell>.json`. It does not build the routing blocks again. Its index records the source graph and cell descriptors |
| `planner/search/pois/grid`, `planner/search/addresses/grid` | The database of its search component | `search/tiles/<component>/<cell>.sqlite`, packed. A cell retains intersecting places, addresses, their streets and contexts. The index records regional metadata and clipped cell bounds |
| `planner/assets/grid`, `planner/model/grid`, `planner/trailview-rules/grid` | `planner/assets`, `planner/model` or `rules.json` of `planner/trailview-rules` | The input files, packed as `maps/assets/`, `search/model/` or `trailview/`. The index maps logical paths to file and transport identities |
| `planner/fonts/grid` | `planner/basemap/grid`, `planner/places/grid`, `planner/overlays/grid` and `planner/assets/grid`: client files only. The route references are the `ref` of the overlay tiles | Packed `offline/fonts/<stack>.pbf`: the joined used glyph ranges of [the offline contract](planner-offline.md). The index maps each original glyph range to its joined stack |
| `planner/index` | Only `index.json` of each grid step | Packed `search/<region>.grid.json` and `offline/catalog.json` in `objects/`; `release.json` and the small tile pointers in `public/`. It selects those three outputs. Its own `index.json` stays local |
| `planner/catalog` | `planner/index`, and `index.json` of each grid step | `catalog.json`: the pointer that a planner client reads first. Its `active.id` is the planner release id: the digest of the named files of the planner. `origins` is the `[publication]` table of `data/planner-runtime.toml` |

The overlay grid reads only `index.json` of the routing grid too. It checks the source routing
package and changes its TileJSON `routing_package` to the grid package hash. The final index
checks the OSM source, region, coverage, search schema and time zone, routing cell bounds, terrain
inputs. It reads no payload to compose metadata. A climate change builds
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

Each service prints the formats of the layers that it reads with `--formats`, as one JSON object
of layer and format, and reads no data: `planner-service --formats` gives `planner/routing`,
`node planner/search/server.mjs --formats` gives `planner/search` and `planner/index`, and
`python3 -m tools.planner_downloads --formats` gives `planner/index`. The format of
`planner/index` is that of the two files of it that the services check: the offline catalog and
`search/<region>.grid.json`.

The pointer names routing, search and downloads under
`<api_origin>/planner-api/releases/<id>/SERVICE`, and the release document at
`<objects_origin>/releases/<id>/release.json`. Its `device_catalog` names
`<objects_origin>/cell-catalog/catalog.json`. [`obc data serve`](#serve) runs the services for a
`dir` target. [`obc data deploy`](#deploy) puts the follower of `prod` on its server and the tile
Worker on Cloudflare.

The [download service](planner-offline.md#download-service) returns a ready quote with an
absolute pinned `source` URL. The client reads the bundle, release and objects through that
source. The job ID hashes the URL of the object pool as well as the catalog and the bounds. A
quote stays valid while its bundle exists; a missing bundle requires a new quote. Job creation
completes synchronously; there is no polling path.

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

Each layer also gives what a picture of the layers needs: `shape` (`tiled` or `whole`), the
`format` and the `revision` of its steps, its `readers`, and `reads`, the layers and the sources
that its steps read, sorted. A layer with partitions gives `partitions`, those of its steps, and
`stale_partitions`, those whose steps a build changes. In place of an internal layer, `reads`
names what that layer reads.
A layer that only the release has gives `null` and empty lists.
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
| `bucket` | string | `r2` only: the name of the bucket. `OBC_R2_BUCKET` must name it to write the target |
| `prefix` | string | `r2` only, optional: the folder under `test/` of the bucket that holds the target, such as `test/run-1`, without `/` at the end. Without it, the root of the bucket. A collection deletes keys of the target, and a folder outside `test/` can hold live keys |
| `dir` | string | `dir` only, optional: an absolute path, or a path that starts with `~/` for the home directory. Without it, the folder with the name of the target beside the store |

The file has `prod`, the public bucket, and `local`, a folder on each machine. A `dir` target and
the store do not overlap, also through a link.

`obc data` reads an R2 target through the S3 API when it has the credentials, because the public
origin can serve a cached pointer. Else, and always in [serve](#serve), it reads through the public
URL, with each part of a key percent-encoded, so a `#` in a key under `sources/` is no fragment.

`obc data` writes an R2 target with the credentials of `OBC_R2_*` (see [Credentials](#credentials)):
a small document through the S3 API, signed with AWS Signature Version 4, and the files of a
release with rclone. `OBC_R2_BUCKET` must be the `bucket` of the target. A command that writes an
R2 target without these variables refuses (`blocked`); so do an update and a rollback without
rclone, which lists and checks the files of a release. A tagged read and a review need the
credentials too: the review id holds the tag of `release.json`, the ETag of the S3 API, and the
public origin can give another form of it.

### Target layout

A key is a path with `/` between its parts. No part is empty, `.` or `..`.

| Key | Holds |
| --- | --- |
| `release.json` | The release that the target serves, and its rollback. A switch writes it first |
| `lock.json` | The [lock](#lock): the run that changes the target, or free |
| `planner/next.json` | Optional: a release whose planner services [serve](#serve) loads before a switch to it, see [Router ready](#router-ready) |
| `releases/<id>.json` | The [manifest](#release-manifest) of a release. Immutable |
| `releases/<folder>/<path>` | A file that a client finds by name. `<folder>` is the [digest](#keys) of the files of the folder, listed by `<path>`. Immutable |
| `objects/<sha256>` | A client file of an output. Immutable |
| `cell-catalog/catalog.json`, `planner/catalog.json` | The pointers: a copy of the object that the serving release names for the key |

Only client files of outputs go to a target: an output that no client reads can derive from a
source that we may not give to others, and a bucket is public. No target holds a version of a
[prepared source](#prepared-sources). A step that reads such an output of a kept layer reads it
from the store of the machine.

### Release manifest

`releases/<id>.json` is compact JSON with the keys of each object in byte order. The id is the
SHA-256 of these bytes. The manifest has format 1. A change that a follower of the format before
cannot read raises the format. A follower reads its own format and the one before. It says its format at `/planner-api/version`, and the review
of an update of prod names a follower that cannot read the release.

| Key | Meaning |
| --- | --- |
| `settings` | The [settings of the target](#settings-of-a-target) |
| `outputs` | One output per step, sorted by `step` |
| `pointers` | `[{key, size, sha256}]`, sorted by `key`: both pointer keys, each with the client file of an output that a client reads there |
| `named` | `[{key, size, sha256}]`, sorted by `key`: the files under `releases/<folder>/`. Each is a client file of an output |
| `sources` | `[{source, version, params, files}]`, sorted by `source`, `params` and `version`: each fetch that an output read, with the sorted `NAME=VALUE`s and the files that the outputs read (`[{name, url, size, sha256, retrieved}]`, sorted by `name`; `url` is where upstream gives the file, and `retrieved` when a machine retrieved it first). A reader ignores other fields of an entry, such as `copy` in the manifest of an older publisher. The target holds no copy of these files |
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

`since` is the time, by the clock of the target, at which the release took its role, or at which
a switch to the serving release wrote its pointers again. `rollback` is `null` until the second
switch.

`planner/next.json` names one more release with the same fields as `serving`: a published release
that the target does not serve yet. [Serve](#serve) starts the services of its planner release
beside the others, and gives it to no client.

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

### Lock

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
  run ended (see [Runs](#runs)), and no process of its session runs, such as a program of a step.
  On another machine, `obc data unlock TARGET --from MACHINE` frees the lock of a run of MACHINE
  whose heartbeat is 10 minutes old or older (`STALE` in `lock.rs`). It asks first. When that run continues, its next write fails,
  and it does not switch.
- The heartbeat is the time of the clock of the holder. A `dir` target is on a local file system,
  so the holder and `unlock` read the clock of one machine. On an R2 target, `unlock` measures the
  age of the heartbeat with the clock of the server: its `Date` against the `Last-Modified` of
  `lock.json`.
- `progress` gives the steps done of the total of each layer that the run bakes, at the last
  heartbeat. `status.json` of the run has the detail, see [Runs](#runs). In the phase
  `router-ready`, `waiting` gives the last answer of the follower of the target.

### Update

`obc data update TARGET` makes a target serve its settings as the steps make them now. An R2
target serves riders: its update starts only with `--expect ID`, the id of a review that the owner
approved, and asks first, or needs `--yes` without a terminal. The first process checks the
arguments of the person, before the run. One run does each phase in this order, while it holds the
[lock](#lock):

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
   file is the same when its name, size and SHA-256 are. Else each product checks it, and a
   failed check changes nothing.
5. Upload and switch, as [Publish and switch](#publish-and-switch) says. On an R2 target, the
   phase `router-ready` comes between them, as [Router ready](#router-ready) says.
6. Collect, as [Collection](#collection) says, when it switched to another release.

An update that stops keeps what it baked in the store, and the next update reuses it.

### Router ready

The follower of an R2 target, [serve](#serve) on the machine that serves its API, must run the
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

The run waits at most 24 hours (`GIVE_UP` in `ready.rs`). After that, or when the run stops or
another run takes the lock, it fails with `<target> unchanged: <last reason>`. `release.json` and
the pointers stay as they were; the next collection deletes the release that it uploaded. The run
deletes `planner/next.json` at each end, but only while the file names its release, and never
after it lost the lock: then the file belongs to the run that took it.

### Any machine

An update needs no file of the serving release in the store of the machine. It takes only what
its bakes and the checks of the products read:

- A fetch comes from the store, else from upstream. The target holds no copy of a fetch.
- A version of a prepared source comes from the store, else from the archive. A machine without
  the archive bakes only what its store holds, as [Prepared sources](#prepared-sources) says.
- A step that bakes and reads a file of a fetch that the serving release lists must find the same
  SHA-256 in the record of the store: before the review, and again after the fetches of the
  update, before any bake. Else nothing changes (`verify_failed`).
- A fetch that the serving release read, of a source whose upstream gives only its newest version
  (a capture), refuses the update (`blocked`) before the fetch, also while the update lists the
  steps, when the store lacks it and a capture cannot give it. A capture gives the version of today, and a version of a manual source that the store started (see [Fetch](#fetch)).
  The fix names the machine that made the serving release. A refresh that moves the source to a new
  version is the other way.
- When the store and the serving release give a fetched file the same name, size and SHA-256, the
  new release keeps the entry of the serving release.
- A file of a kept step that a step that bakes reads, or that the check of a product reads, comes
  from `objects/` of the target when the store lacks it.
- A kept step whose file a step that bakes reads, when no client reads that file and the store
  lacks it, bakes again on this machine: the target does not hold it. Its cause is `unheld`. The
  review lists such a layer, also one that only other layers read.

### Review

The review of an update says what it does before it changes a target. `--dry-run` writes the
review and creates no run. `--plan` makes the review in a detached run, as [Runs](#runs) says: the
run logs each fetch with its progress and each product that it plans, writes the review to
`runs/<id>/review.json`, and bakes nothing. It takes no lock: it writes nothing to the target.
`runs RUN` gives that review. To list the steps, an update fetches what the step lists need, which can be bulk data. It fetches into
the store only, and it never writes to the target.

- `layers`: each layer that bakes or goes, with its cause (`stale` with the cause of
  [State of a layer](#state-of-a-layer), `named` by `--only`, `off`, or `unheld` as
  [Any machine](#any-machine) says), the steps that bake, the grid leaves (the last part `<i>-<j>`
  of the names of the steps of a tiled layer), the partitions whose steps bake, the steps of the
  release that no step makes now and their grid leaves, the readers, and a format change with what
  the new format means for each reader. With `--only`, a layer that bakes because it reads a named
  layer follows that layer. A layer that only other layers read is not listed. A planned step can
  end unchanged (see [Runs](#runs)), so the text gives the bakes and the leaves as upper bounds ("at
  most").
- `sources`: each fetch that a baked step reads at another version than the serving release.
- `kept`, `fetches`, `downloads`, `bakes` and `reused`: the outputs that the release keeps by
  reference, what this machine fetches upstream and downloads from the target, the steps that it
  builds, and the steps whose output the store has from an earlier build, each with `built`. A
  reused step does not build; its client files go to the target.
- `cost`: the estimate of each bake, as [Plan](#plan) says. `upload_bytes` is the size of the
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
  [Deploy](#deploy) says: no follower answers `version`, the follower reads a layer of the
  release in another format, or the Worker does not answer `version`. It does not stop the
  update: the run would wait at `router-ready`.

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

### Collection

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
- It runs while the update holds the [lock](#lock): no other run writes the target. Before each
  100 deletes, a heartbeat checks that the run still holds it.
- On an R2 target, it lists the keys under `objects/` and `releases/` only, and the
  grace counts by the clock of the server: the `Date` of its answers, as `since` does.
- It deletes each other file that no remaining manifest names, then the manifests of the releases
  that go. A collection that stops resumes at the next update that switches, from the manifests
  that remain.
- When it cannot read a manifest, it deletes nothing.

A collection that stops does not undo the switch: the output gives what it deleted and why it
stopped.

### Prepared sources

A prepared source is a source whose versions `obc data source prepare` makes in a long job, such
as the processing of large dumps. A version is a fixed set of files in the [archive](#archive),
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
| `prepared/<id>/lock.json` | The lock of the source: one prepare or copy at a time. It has the format of the [lock](#lock) of a target |
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
| `inputs` | Each source version that the prepare read, sorted by source and version, with the [digest](#keys) of the files that it read |
| `files` | Sorted by `path`. A path is relative, and is not `manifest.json` |
| `reused` | Absent, or each path of `files` that an earlier version of the same source holds, with that version: the file is at `<that version>/<path>`. A version that extends the one before keeps its files without a copy |
| `scope` | `null` for every place that the source covers, or `{"region", "definitions"}` as in the [settings of a target](#settings-of-a-target) |
| `extends` | Absent, or the scopes of the earlier versions that this version extends, oldest first: the version keeps their content, as its producer says. A tile producer keeps their files outside its own `scope`; [`wiki`](#wiki) rewrites their shards with one form of each fact |
| `languages` | Article language codes in supported order, each once; empty for a source without articles |
| `publish_text` | Absent, or the `publish_text` of the row in `data/sources.toml` |
| `licence`, `attribution` | The licence and the credit of the row in `data/sources.toml` |
| `machine`, `time` | The machine that made the version, and when the version was complete |
| `cost` | The wall time of the last run of the version, and the peak memory of its process or, when that is more, of the largest program that it ran times the most programs that ran at once (`null` when the system does not report it) |

#### Prepare

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
   `prepared/<id>/<version>/` of the [store](#store) of its machine. The same command
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
   check of this machine as in the [review](#review) of an update, which also refuses when the source or an input has a
   `credential` that is not on the machine, or when the root lacks the free space for the bytes of
   the version and 4 GiB more, and for each target of `data/targets.toml` the
   layers of its serving release that read an older version. `--dry-run` writes it. Its id is the
   first 12 hex digits of the SHA-256 of the canonical JSON of the source, the root, the
   version, the format, the revision, the scope, the languages, `publish_text` and each planned
   input, including its archive path, URL, size and object identity. With `--expect ID`, the
   prepare starts only when the review has that id.
4. A detached run, as [Runs](#runs) says, takes the lock of the source as an update takes the lock
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

#### Read

The archive is the reference: each target reads the versions that the home lists. Before an
update or the [overview](#overview) lists the steps, each prepared source (an update) or each
that the steps read (the overview) gets a version in the `format` of the producer of this
`obc data`: the serving version of a `manual` source, else the newest version of the home. So a
new version makes each layer that reads an older one stale, with the cause `source`, in each
target. When no root has the home, such as on a machine without the
archive, a target keeps the version that it serves, and a bake from the store needs no archive.
When the home lists no version in that format and the target serves none, the listing blocks the
layers that read the source (`blocked`), and the fix names `OBC_DATA_ARCHIVE` and the command that
prepares one. A product can bake a layer without the source instead, as `device/terrain` does without a national terrain model: then a note
of the [review](#review) names the source and the command.

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

#### Delete

No command deletes a complete version: the archive is its only copy, and a release that a target
serves or keeps for its rollback can read any version. A [collection](#collection) of a target and
a [clean](#clean) of the store never change the archive. The store of a machine keeps the files of
a version that a release of one of its targets reads, as [Clean](#clean) says.

#### Copy

`obc data source copy ID --from TARGET` copies the versions of a prepared source from the legacy
folder `sources/<id>/` of a target into its home. That folder has the layout of the home. The
copy only reads the target.

- The home is that of a prepare, and so are its refusals. The copy is a detached [run](#runs) that
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
- The files stay in the store; a [clean](#clean) deletes those that no release reads.

#### `terrain`

The producer `terrain`, format 4, makes the blocks of the reference archive
(`host/obc-dem/reference/README.md`) of the national terrain models `dtm-<key>`, in two pools: the
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
  the empty `<bi:04>-<bj:04>.none`. It asks a service only for the part of a tile in the `extent`,
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
  source of a tile. The manifest names the input `dtm-<key>` of each model at that date, with the
  [digest](#keys) of the pixel SHA-256 of the blocks that it made.
- Only the machine that prepares needs the `credential` of each model. A `by-hand` model reads
  its delivery: `OBC_REFERENCE_<KEY>_INPUT` names its directory, as an absolute path, and
  `OBC_REFERENCE_<KEY>_DATUM` the vertical datum that the metadata of the order states.
- No run writes `reference/v1/`.

#### `forest`

The producer `forest`, format 4, makes the forest of each cell of the zoom 12 tiles at
256 × 256 cells, which [the canopy archive](trailview-tiles.md#canopy) packs, in one file per zoom
10 tile. A version adds a region to the newest complete version of `forest` in the home, in the
same format.

- A block is a zoom 10 tile: block `(x, y)` holds the zoom 12 tiles `(x', y')` with `x' >> 2 = x`
  and `y' >> 2 = y`.
- The canopy models are the `canopy-<key>` rows: one per country, whose key is the lowercase
  country code, and `canopy-global`. The tiles of a model are the zoom 12 tiles whose inside meets
  its `extent`.
- The blocks of a region are the blocks with a tile of a model that a box or a `geofabrik-poly`
  polygon of a leaf of the region meets; for a version of everything, the blocks with a tile of a
  national model. A run keeps the version of each polygon that the first run of the version read, as for
  [`terrain`](#terrain). A version has a file for each block of its region, also for a block
  without a pixel, and makes each zoom 12 tile of the block from each model that has the tile, also
  from a model of a country that the region does not name. So the pixels of a block do not depend
  on the region. Each other block of the version before stays in `reused`; the manifest names the
  scopes of the versions before in `extends`. The review counts the blocks of a region of boxes and
  names the models of their tiles; for a region with a `geofabrik` leaf, `files` is `null`, and the
  review names every canopy model.
- For each block, `python -m tools.forest_reference fetch --block 10-<x>-<y> 12-<x'>-<y'>=<keys> …`
  writes `10/<x>/<y>.tif` and then `10-<x>-<y>.sha256`, the SHA-256 of its pixels. The keys of a
  tile are the models that have it, the national ones in the order of `data/sources.toml` and then
  `global`; a cell takes the first model that has a pixel in it. It keeps no raw raster. The run
  keeps the SHA-256 of each block until the version is complete, so a resume makes no block again
  that it has.
- A block is a GeoTIFF in EPSG:3857 on the 1024 × 1024 cells of its zoom 12 tiles: four bands of
  `uint16`, deflate, predictor 2, 65535 unknown, with 256 × 256 internal tiles. Tile `(x', y')` is
  the internal tile `(x' − 4x, y' − 4y)`; GDAL leaves out an internal tile without a known cell,
  which a reader reads as unknown. The SHA-256 of the pixels of a block is over the bands in
  order, rows north first, little endian.

  | Band | Value of a cell |
  | --- | --- |
  | `height_dm` | The 95th percentile of the canopy pixels in decimetres, at most 800; 0 without a canopy pixel |
  | `cover_pct` | The share of the pixels that are canopy, in % |
  | `stems_per_ha` | The tree tops per hectare of the known pixels |
  | `conifer_pct` | The share of coniferous trees of the leaf type samples with a tree, in %; 65535 without one |

- A canopy pixel is 3 m high or more. A tree top is a pixel of the canopy that a binomial 3 × 3
  mean smooths, 5 m high or more, that no pixel within its crown radius exceeds: 0.5 m plus 8 % of
  its height, from 1 m to 4 m. Tops of one height within the crown radius of each other, directly
  or through other such tops, are one tree, such as a flat crown; its top is the first in raster
  order.
- `global` reads `canopy-global`, Canopy Height Maps v2, through HTTP range requests: a COG per zoom
  10 tile, whose pixels of zoom 17 lie on the cells, 32 × 32 to a cell. Its heights are whole metres,
  where 0 is also no data. A cell takes 0.982 times its height plus 3.73 m, and the stems that fill
  41.7 % of its cover with crowns of that height.
- `ch` reads `canopy-ch` through HTTP range requests, in strips of whole rows of its internal
  tiles, so each internal tile downloads once: at each 0.5 m pixel, the newest acquisition that has
  it.
- The leaf type of a cell with a height is `hrl-dominant-leaf-type` of 2024 at 4 × 4 samples per
  cell, from its tiles on CDSE S3.
- The run moves the blocks into the home in batches of 64. The manifest names each model that the
  version read and `hrl-dominant-leaf-type` at the date of the version, with the [digest](#keys) of
  the pixel SHA-256 of the blocks that it made.

#### `climate`

The producer `climate`, format 1, makes the daily values of `era5-land` for each cell of
[the climate grid](planner-climate-tiles.md#cells), without a calibration. `planner/climate`
bakes the climate archive from them, and applies `wet_day_mm`, `rain_factors` and `wind_factor`.
A version adds blocks and years to the newest complete version of `climate` in the home, in the
same format.

- A block holds 96 × 64 cells, 12 × 16 chunks of the ARCO stores, so no two blocks read one
  chunk. Block x, y holds the cells whose Zarr column, (i − 1) mod 3600 for the cell column i, is
  96x to 96x + 95, and whose Zarr row, 1800 − j for the cell row j, is 64y to 64y + 63. A block
  with x = 37 or y = 28 holds fewer cells. A block has the size of a zoom 6 climate tile, but it is
  not a tile of the archive.
- The blocks of a region are those that a box or a `geofabrik-poly` polygon of a leaf of the
  region meets, after the block grows by 0.2° on each side: the blocks of the cells that overlap
  the outline and of the two cells around each, which the bake of a box reads for the lapse
  rates. A run keeps the version of each polygon that its first run read, as for
  [`terrain`](#terrain). A version of everything has every block. A region within 0.3° of the
  antimeridian refuses: its bake would read cells on both sides.
- The years are the ten years from `climate.first_year` of [`data/planner.toml`](#dataplannertoml).
  For each block of its region, a version holds `orography/<x>/<y>.gz` and `<year>/<x>/<y>.gz` of
  each year, also for a block without land. A file that the version before holds stays in
  `reused`, and the run does not make it again: final data of a year does not change. The manifest
  names the scopes of the version before in `extends`.
- For each block, `python -m tools.planner_climate prepare --block <x>-<y> --years <year>,…`
  writes the files that the version lacks: with `--orography DIR` also the orography, from the
  orography file that the first program downloads into `DIR` of the run. It reads the chunks of
  the block with the `credential` of `era5-land`, final data only, as the
  [climate archive](planner-climate-tiles.md#source) says, at most 2 at once, so the 8 programs
  open at most 16 connections to the stores. It tries a request again after a 429 or 5xx answer
  or a network error, for about 35 minutes with longer waits. Then it writes
  `<x>-<y>.orography.sha256` and `<x>-<y>.<year>.sha256`, the SHA-256 of each file, and deletes
  its chunks; a block that fails keeps them for the next run.
- The review counts into the store the chunks and the files of the programs that run at once, and
  a batch of files that waits for its move into the home.
- A file is gzip, with no name and time 0, of planes in the order of the table. A plane has one
  value per cell for each of its indexes. For each index, from 0 in order, the cells of the block
  follow in row order: the north row first, the west cell first. A value is little-endian. A
  missing value is NaN, or 255 for `sector`. A day is a local solar day of
  [the climate archive](planner-climate-tiles.md#time), from day 0 on 1 January; a day with a
  missing hour is missing.

  | Plane | Type | Indexes | Value of a cell |
  | --- | --- | --- | --- |
  | `tmax` | float32 | day | The maximum of the 24 hourly 2 m temperatures of the day, in K, as the store holds it |
  | `tmin` | float32 | day | The minimum of the same 24 values, in K |
  | `rain` | float64 | day | The sum, in hour order, of the 24 hourly precipitation totals of the day in mm, each at least 0 |
  | `wind` | float64 | day | The mean of the 10 hourly 10 m wind speeds of the day time, in m/s |
  | `sector` | uint8 | 10 × day + hour − 9 | The [sector](planner-climate-tiles.md#values), 0 to 15, of the wind at the local hour 09:00 to 18:00 |

  The orography file has one plane: the geopotential of the cell in m²/s², float64, from the
  orography file of the climate archive. With these types, `planner/climate` computes the same
  values as from the hourly data, so its archive has the same bytes except `inputs`.
- The run moves the files into the home in batches of 64. The manifest names `era5-land` at the
  date of the version, with the [digest](#keys) of the SHA-256 of the files that it made.

#### `colour`

The producer `colour`, format 1, makes the Sentinel-2 measurements of each texel of the zoom 12
tiles, per quarter of a year, from `sentinel-2-mosaics`: the CDSE collection
`sentinel-2-global-mosaics`. `planner/trailview-colour` and `planner/trailview-material` read
them and apply the rules of [the colour](trailview-tiles.md#colour): the look minimum, the NDVI,
the shadow rule and the snow rule. A version adds blocks and years to the newest complete version
of `colour` in the home, in the same format.

- A block is a zoom 10 tile, which holds 4 × 4 zoom 12 tiles. The blocks of a region are those
  whose inside a box or a `geofabrik-poly` polygon of a leaf of the region meets. A run keeps the
  version of each polygon that its first run read, as for [`terrain`](#terrain). A version without
  a region refuses.
- The year is `trailview.colour_year` of [`data/planner.toml`](#dataplannertoml). For each block
  of its region, a version holds `<year>/<x>/<y>.pmtiles`, with x and y of zoom 10. The file of
  a block without data is empty. A file that the version before holds stays in `reused`, and the
  run does not make it again. So a new region or a new year adds files, and a new format makes each
  file again. The manifest names the scopes of the version before in `extends`.
- For each block, `python -m tools.trailview.colour prepare --block <x>-<y> --year <year>`
  writes the file of the block and then `<x>-<y>.sha256`, the SHA-256 of the file. It reads the
  window over the block, one pixel wider on each side, of each band of each mosaic tile of the
  year that the STAC search of the collection lists, from CDSE S3 with the `credential` of
  `sentinel-2-mosaics`, one window at a time. CDSE S3 allows 4 connections per user, so 4
  programs run at once. It deletes its windows when the file is complete; a block that fails
  keeps them for the next run.
- In each quarter, the program puts the bands B02, B03, B04, B08 and `observations` on the
  512 × 512 texels of each zoom 12 tile in EPSG:3857: bilinear where the texel is under 10 m, the
  mean of the pixels otherwise. The mosaic tiles go in the order of their ids, and a later tile
  writes over an earlier one where it has a pixel. `refl` is a band ÷ 10000, clipped to 0 to 1,
  and 0 without a pixel: a negative band value, as over deep water, is 0.

  | Plane | Value of a texel |
  | --- | --- |
  | Red, green, blue, near-infrared | round(255 × refl^(1/2.2)) of B04, B03, B02 and B08 |
  | Look | min(15, ⌊`observations`⌋) where B04 has a pixel; else 0 |

  So a test "look ≥ n" for n of 1 to 15 is the same test as on the resampled `observations`.
- A file is PMTiles v3 at zoom 12 only: tile type `unknown` (0), no tile compression, metadata
  `{}`. A tile is five lossless WebP images of 512 × 512 texels, one after the other: the quarters
  1 to 4 as RGBA, with the near-infrared in alpha; then the looks of the quarters 1 to 4 as red,
  green, blue and alpha. A texel keeps each byte where its alpha is 0 (WebP `exact`). The length
  of an image is 8 + the RIFF size in bytes 4 to 7, little endian. A tile whose look is 0 in each
  quarter is not in the file.
- A file depends only on its block, its year and the mosaics, never on the region. So
  `planner/trailview-colour` makes the same tiles from each version that holds the blocks of its
  bounds.
- The review counts into the store the windows of the 4 programs and a batch of files that waits
  for its move into the home.
- The run moves the files into the home in batches of 64. The manifest names
  `sentinel-2-mosaics` at the date of the version, with the [digest](#keys) of the SHA-256 of the
  files that it made.

#### `snow`

The producer `snow`, format 1, makes the tiles of zoom 6 to 12 of
[the snow archive](planner-snow-tiles.md), one season at a time. `planner/snow` joins the seasons
into the archive of a region, and `planner/trailview-snow` makes the TrailView snow days from that
archive. A version adds blocks and seasons to the newest complete version of `snow` in the home,
in the same format.

- A block is a zoom 6 tile of Web Mercator, and holds its tiles of zoom 6 to 12. The blocks of a
  region are those that a box or a `geofabrik-poly` polygon of a leaf of the region meets. A run
  keeps the version of each polygon that its first run read, as for [`terrain`](#terrain). A
  version of everything has every block.
- The seasons are those of `snow.seasons` of [`data/planner.toml`](#dataplannertoml). For each
  block of its region, a version holds `<season>/<x>/<y>.pmtiles` of each season, also for a
  block without data, whose file is empty: PMTiles has no archive without a tile. A file that
  the version before holds stays in `reused`, and the run does not make it again: the data of a
  past season does not change. So a new region or a new season adds files, and a new format makes
  each file again. The manifest names the scopes of the version before in `extends`.
- A file is PMTiles v3 of the tiles of the block that the season has, as
  [the snow archive](planner-snow-tiles.md#zoom-levels) defines them: tile type `unknown`, gzip
  with no name and time 0, the metadata `{"season": <season>}`. The body of a tile is its season
  block: the onset and the melt-out planes, 131,072 bytes. The rules of the archive read only the
  source pixels near a tile, so a block has the same bytes in each version that makes it.
- For each block, `python -m tools.planner_snow prepare --block <x>-<y> --seasons <season>,…
  --modis <list> --canopy <file>,… --slots <dir>` writes the files of the seasons that the version
  lacks, one season at a time. Of each season, the program reads:
  - The MODIS files of the archive dataset `modis-snow` in place, from the root that holds its
    manifest: `<list>` names each file of a MODIS tile that the block meets with two MODIS pixels
    of margin, the most that the MODIS grid of the program reaches past the block. A day and a
    platform of a tile take the COG of the Planetary Computer, else the HDF4 granule of NSIDC. A
    prepare refuses when no root holds the dataset, and when a MODIS tile of the block lacks a
    file of either platform on more than 7 days in a row of a season: the nearest clear day fills
    such a run as it fills a cloudy week, and a longer one is a gap of the source. The number is
    chosen. A tile that `data/archive/modis-snow.toml` names and the block meets lacks every day
    when the dataset has no file of it; a tile that it does not name is sea or outside the
    dataset. The checks run before the program fetches anything.
  - The `hansen-gfc` tiles at the `start` of its row that the block meets with two MODIS pixels of
    margin, which the run fetches.
  - SCO, SCM and SCD of HR-WSI from CDSE S3, with the `credential` of `hr-wsi`: the products of
    the season that the catalogue lists for the block, in the order of their names, a window of
    each zoom 10 tile at a time. A block fails when its catalogue lists products for one season
    and none for another of the seasons that it makes, as a short listing would make a season MODIS
    only for good, and when those seasons have other tiles. An empty listing is asked once more; a block outside
    HR-WSI or at sea has no product. The programs share the 4 connections that CDSE allows a user
    through lock files in `<dir>`, and GDAL retries a refused request.

  Then it writes `<x>-<y>.<season>.sha256`, the SHA-256 of each file.
- The review counts into the store the files of the programs that run at once, and a batch of
  files that waits for its move into the home.
- The run moves the files into the home in batches of 64. The manifest names `hr-wsi` at the date
  of the version with the [digest](#keys) of the SHA-256 of the files that it made, `modis-snow`
  at the version of its dataset with the digest of the SHA-256 of the files of the lists, and the
  `hansen-gfc` version that it fetched.

#### `wiki`

The producer `wiki`, format 1, makes the Wikimedia facts and the article text of the OSM subjects
in the shards of [Shared Wikimedia content](wikimedia-content.md#the-wiki-source):

- It reads cached archives in place. `OBC_WIKI_DUMPS` overrides the default `wiki-dumps/` in the
  store. Of each kind it reuses the newest cached day: `wikidata-<day>-all.json.bz2`,
  `wikidatawiki-<day>-page.sql.gz` and `-redirect.sql.gz`, and for each selected article language
  `<lang>wiki-<day>-langlinks.sql.gz` and the Wikimedia Enterprise HTML
  `<lang>wiki-NS0-<day>-ENTERPRISE-HTML.json.tar.gz`. A preparation without a region, or with
  a box, reads `planet-<yymmdd>.osm.pbf`. Each Geofabrik area of a region reads
  `<area>-<yymmdd>.osm.pbf` under the dump directory, such as `europe/dach-261009.osm.pbf`.
  The path of the area supplies the subdirectories. These extracts select the regional subjects
  before article selection or Commons acquisition. For a missing kind, the review discovers a
  completed upstream version through metadata only. SQL jobs must be `done`; Wikidata JSON must
  have a checksum entry. Wikipedia HTML uses the Enterprise Snapshot API. The review shows each
  file, date, size and bytes to download. It does not download archives. Start downloads missing
  archives serially, with Range resume and the reviewed size and ETag, then prepares the source.
  A changed remote identity refuses. The archive becomes visible only after the download ends.
  Credentials stay in the local environment and private token cache. No review or manifest holds
  them. The manifest names `wikidata`, `wikipedia` and
  `osm-planet` or `geofabrik-extracts` at the days of their dumps, with the [digest](#keys) of the
  SHA-256 of the files.
- `--languages en,de` selects article languages from `specs/content-languages.json`. Without
  the flag, all supported languages are selected. Unknown or empty codes refuse; duplicates
  are removed and codes have supported order. The review, its id and the manifest keep the
  selection. Only selected languages require HTML and language-link dumps.
  [The `wiki` source](wikimedia-content.md#the-wiki-source) gives the facts of an excluded
  language.
- Start saves the reviewed input plan before it downloads. A resume uses this plan, including
  the URLs, sizes and object identities. The review ID covers these decisions, but not whether
  the download has since completed. After acquisition, the run pins each dump with its path, size,
  modification time and SHA-256, and `publish_text`. A resume reads exactly those dumps, and refuses when one is gone or changed,
  or `publish_text` changed; the fix deletes the work of the version, so the next run starts a
  new one. A dump of the pinned size with another modification time is the pinned dump when its
  SHA-256 is. `tools.wiki` gets each SHA-256 and does not hash a dump again. A row without
  `publish_text` refuses.
- `python -m tools.wiki` runs the steps: `subjects` lists the OSM objects with a `wikidata` or
  `wikipedia` tag, `import` reads the Wikidata dump twice, `candidates` gives the classes, the
  subjects and the peaks, `articles` reads the language links and the HTML, and `shards` writes
  the version. A step whose output the run has does not run again, and `tools.wiki` does not
  repeat an import that finished with the same input.
- The selection is the rule of the compilers: each subject of a landmark category, and each peak
  with a resolved link of a node that the map takes as a summit. Only their articles are read, and
  only the photos that the compilers choose for them come from Commons, as [Shared Wikimedia content](wikimedia-content.md#the-wiki-source)
  says.
- A box keeps the subjects whose box meets it. A region whose leaves are all Geofabrik regions
  reads the extract of each of their areas. An object in two extracts is the object of its
  highest version; extracts with one version give the box around their boxes. Other scopes, such
  as a union of a box and a Geofabrik region, refuse.
- A version of a region extends the newest complete version of `wiki` in the home, in the same
  format, and the manifest names the scopes of that version in `extends`. The version holds each
  shard of the version before, and each shard of its run. A shard of both holds the facts of the
  run and the earlier facts. A fact has one form in every shard of the version: the form of the
  run, unless the run lacks the fact, has an older revision of it, or has it only as an article
  of a language that the run did not prepare. Then the fact keeps its earlier bytes and its asset,
  also its languages. A shard writes its facts in the order of kind and key, and an earlier shard
  whose members do not change stays in `reused`. A version of everything extends no version.
- An extension whose row has another `publish_text` than the version before refuses before its
  first step. The fix is the `publish_text` of the
  version before, or a version of everything.
- The run moves the shards into the home in batches of 64, then `index.json`. A resume names the
  shards in the home to `shards`, which does not write them again.
- With `publish_text = false`, each article is a `missing` fact, as
  [the `wiki` source](wikimedia-content.md#the-wiki-source) says. The review says "text published"
  or "text kept local".

### Serve

`obc data serve [TARGET]` follows the releases that a target names, by default `local`, and runs
the services of their planner releases. It answers on port 8710: for a `dir` target on every IPv4
address of the machine, and for an R2 target on `127.0.0.1` only, behind a proxy. It reads an R2
target through its public origin, without credentials. The current release is the serving
release once its services answer: a `dir` target gives clients its pointers. Every answer allows
each origin.

| Path | Answer |
| --- | --- |
| `/planner-api/version` | `{"commit", "formats", "manifest"}`: the commit that the build of serve names in `OBC_DATA_COMMIT`, or `null`, the format of each layer that its services read, and the format of the release manifest that it reads. Never cached |
| `/planner-api/releases/<id>/ready` | `{"state": "ready"}` while the services of the planner release `<id>` answer. 503 with `{"state": "starting"}` while its objects download and its services start, or `{"state": "failed", "reason": "not ready: <why>"}` after they failed. 404 with `{"state": "unknown", "reason"}` while serve does not know `<id>` yet: a publisher that waits for `<id>` keeps waiting, as on 503. Never cached |
| `/planner-api/releases/<id>/<service>/<path>` | `<path>` of the `routing`, `search` or `downloads` service of the planner release `<id>`. Search gets `/api/planner-search/<path>`. 404 when the services of `<id>` do not answer |
| `/planner-offline/<path>` | `<path>` of the downloads service of the current release, for apps that name no planner release. 503 while it does not answer |
| `/cell-catalog/catalog.json` | `dir` only: the device pointer of the current release |
| `/planner/catalog.json` | `dir` only: the planner pointer of the current release. Each string that starts with one of its `origins`, followed by `/` or nothing, starts with the origin of the request instead: `http://` and the `Host` of the request |
| Another path | `dir` only: the file of the target at that key, with ranges; without a file, the tile handler of `planner/tiles` over the target |

Serve reads `release.json` and `planner/next.json` of a `dir` target each second, and of an R2
target every ten seconds. It reads the manifest and the pointers of a release once: a rollback
reads nothing else. A `planner/next.json` that cannot be read, or whose release cannot be read,
names no release: serve writes why, reads it again at each look, and loads the other releases.
When one of the two files changes:

1. Serve loads the planner release of the serving release, of the rollback and of the release of
   `planner/next.json`, unless it runs. The planner release of a release is `active.id` of its
   planner pointer.
   First it compares the formats of the layers of the release with the formats that the services
   read (see [Service runtimes](#service-runtimes)). A layer in another format fails the planner
   release before anything downloads: `needs a routing service that reads planner/routing format
   9; it reads 8`. Then a thread downloads its objects into the store, and its services start once
   they are there. Serve keeps reading the two files and checking the other services meanwhile.
2. Clients get the serving release once each service of its planner release answers: routing
   `GET /health`, search `GET /api/planner-search/status` with `parser.ready` true, and downloads
   `GET /catalog`. Until then clients keep the current release. No client gets the release of
   `planner/next.json`.
3. A planner release fails when its start fails, when one of its services stops, or when its
   services do not answer within 120 seconds after they start. Its other services stop, and its
   API answers 404. Clients keep the current release, also when the failed planner release is the
   one of the current release. A planner release that failed starts again after 60 seconds, after
   each later failure in a row twice the wait before, up to 30 minutes. It starts again at once
   only when the two files name other releases.
4. The services of a planner release that neither the serving release, the rollback, the release
   of `planner/next.json` nor the current release names stop.

When the serving release and the rollback fail one after the other, clients keep a release that
`release.json` no longer names, and the services of its planner release keep answering. Serve
then writes a warning: an update can delete the files of that release.

The services of a planner release read its files under `routing/`, `search/` and `offline/` in
`<store>/serve/<target>/<planner id>/`. The document `releases/<planner id>/release.json` that the
manifest names lists them. Each file comes from an object of the store of serve, which downloads
an object that it lacks from `objects/` of the target and checks its SHA-256: a link to an `identity`
object, or the decoded bytes of a `gzip` object under a temporary name. No write opens an
existing file. Each file must have the SHA-256 and the size that the planner release gives. When
serve stops, it ends each download at its next read and waits for it within the 10 seconds of its
grace; a wait before a retry, a connection or a hash does not see the stop.
Serve removes `<store>/serve/<target>/` when it starts and when it stops; `obc data clean` does
not touch it. The objects are in a store of serve alone, `<store>/follower/<target>/`. After the
releases that the two files name change, and after downloads, serve deletes there each object and
each unfinished download that the services of none of them read, once no download runs. A
download for a planner release that no release names any more stops at its next object. For a
`dir` target, download bundles name the origin of the machine on its network when serve starts,
else `http://127.0.0.1:8710`. For an R2 target, they name `api_origin` and
`<objects_origin>/objects` of the planner pointer.

For a `dir` target, serve builds the routing service with cargo when it starts. For an R2 target,
the routing service is the `planner-service` program beside the program of serve, and serve
needs no tile handler. When the thread that follows the target ends without a stop, such as on a
panic, serve ends with a `failed` error, so that a supervisor starts it again.

### Deploy

An update of `prod` publishes data. Two programs serve it, and `obc data deploy` puts the commit
of the checkout that runs it in place: the follower, `obc data serve prod` as the systemd unit
`obc-data-serve` on the VPS, and the tile Worker of `planner/tiles` on Cloudflare.

| Part | Deploy | Answers `version` at | Credential |
| --- | --- | --- | --- |
| `services` | `tools/vps/deploy-services.sh "$OBC_VPS_SSH" COMMIT` | `<api_origin>/planner-api/version` | `OBC_VPS_SSH`: an SSH host that logs in as root |
| `tiles` | `npm ci --prefix planner/tiles`, then `npm run deploy --prefix planner/tiles -- --var COMMIT:COMMIT` | `<tiles_origin>/version`, `{"commit"}` | `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` |

The origins are the `[publication]` of `data/planner-runtime.toml`. Only 200 with `commit` is an
answer of this code; `commit` is `null` for a build that names none.

`obc data deploy` asks each part for its `version`, and checks what an action needs from this
machine. Each check has a name, its problem, its fix in words, the commands of the fix, and the
command of the page that applies it, if one does. The checks read and change nothing, except the
fetch of `origin/develop`, which never prompts. Each command that reaches another machine ends
within 15 seconds, and the checks run at once. The check of the follower runs up to three such
commands in a row: the login, the configuration of `ssh` and the host key.

| Action | Checks |
| --- | --- |
| Publish: an update of `prod` and `r2 legacy` | The `OBC_R2_*` variables; rclone on PATH |
| `deploy services` | `OBC_VPS_SSH`, which does not start with `-`; this machine logs in as root without a prompt, with a host key that it knows; the one-time setup on the VPS: the user `obc-data`, `/opt/obc-data`, Rust and uv of that user, a C compiler, Python and Node of the `[target]` of `data/planner-runtime.toml`, and the enabled unit. The Caddy route of the API is a check that the deploy does not wait for. `tools/vps/` and `data/planner-runtime.toml` have no change that the commit does not hold |
| `deploy tiles` | The two variables; the token may list the Workers of the account; `planner/tiles` has no change that the commit does not hold |
| Both | The commit of the checkout is on `origin/develop` after a fetch. An older commit of develop serves to go back |

A refused login names its causes: the VPS does not know the key of this machine, the host does
not log in as root, or the key has a passphrase and no agent holds it. It gives the line of the
first public key of `~/.ssh` to add to `/root/.ssh/authorized_keys` on the VPS, or without a key
the command that makes one. A host key that this machine does not know gives the fingerprint of
the ED25519 host key of the VPS to compare with the console of the hosting provider, and
`deploy trust --expect FINGERPRINT`. A host key that changed gives the command that removes the
old one. A missing piece of the setup gives its command.

`deploy trust --expect FINGERPRINT` reads the ED25519 host key of the VPS again and refuses
another fingerprint. It refuses when the known hosts already hold a key of the host: a key that
changed goes only with `ssh-keygen -R`. Then it asks, or needs `--yes` without a terminal, and
appends the key under the name and to the file that `ssh` reads for the host: its
`HostKeyAlias` and its first `UserKnownHostsFile`. It is the one change of a check.

`deploy services|tiles --dry-run` checks the credential, the commit and the files, and writes the commit, what the deploy changes, its commands and the arguments that start it. A
deploy starts only with `--expect COMMIT`, the commit of the checkout now, and asks first, or needs
`--yes` without a terminal. It is a detached [run](#runs), and the output of each command goes to
its log. One run at a time deploys each part. The commands get only `PATH`, `HOME`, the user, the
locale, `TMPDIR`, the SSH agent and the credential of their part. `deploy credential
services|tiles` reads the values on standard input, as
[Credentials of sources](#credentials-of-sources) says, into `credentials.env`.

### Overview

`obc data` gives the machine that runs it and, for each target, the release that it serves, the
machine that made it and when, its rollback, for a `dir` target how [serve](#serve) gives it to
each app, the run that holds its lock or why `lock.json` cannot be read, the region and the parts
that are on, the state of each layer against the steps of now, the blocked steps, the notes of the
layers as in the [review](#review), the pointers that drift, an error that stopped the judgement,
and the command to run next. While a run holds the lock of a `dir` target,
the next step is to wait for that run, unless its process ended on this machine: then the next
update takes the lock over. It fetches only small metadata, and checks upstream with the checks
of the last hour. A target that serves nothing has the next command
`obc data update TARGET --region ID`. For an R2 target, the next command is the review
(`--dry-run`): its update needs the id that the owner approved.

## Commands

| Command | Output |
| --- | --- |
| `obc data [--json]` | The [overview](#overview): the machine, and for each target what it serves and the command to run next |
| `obc data update TARGET [--region ID\|PATH[,PATH…] \| --box W,S,E,N] [--add LAYER]… [--remove LAYER]… [--refresh SOURCE=DAYS\|manual]… [--only LAYER,…] [--dry-run \| --plan \| --expect ID] [--accept-format-change] [--yes] [--json]` | The [update](#update) of a target. The flags change its settings. `--dry-run` writes the [review](#review) and changes nothing. `--plan` makes the review in a run and changes nothing; with `--json`, it writes the run and a `null` review id. Else it starts a detached run, as [Runs](#runs) says, and writes its log until it ends: the steps that it baked or reused, the release that the target serves, its rollback, the number of kept outputs and what the collection did. With `--json`, it writes the run and the review id |
| `obc data rollback TARGET [--yes] [--json]` | The [rollback](#rollback) of a target. Writes the release that the target serves and its rollback |
| `obc data router TARGET [--json]` | For an R2 target, asks the follower once, as [Router ready](#router-ready) says, for the planner release of the serving release and of the rollback. Writes each answer, and the release that `planner/next.json` names. Changes nothing. Refuses a `dir` target |
| `obc data unlock (TARGET \| --source ID) --from MACHINE [--yes] [--json]` | Frees the [lock](#lock) of a target, or with `--source` the lock of a [prepared source](#prepared-sources), that a run of MACHINE holds with a heartbeat 10 minutes old or older. Asks first. Writes the holder that it removed |
| `obc data sources [--check-now] [--json]` | Every source with licence, and for each target that serves a release its policy, its versions and its [state](#state-of-a-source); the versions in the local store; for a prepared source the root of its home, its versions there and the run that holds its lock; and how this machine holds it, as the [source overview](#source-overview) says. Rows are in kind order: data, then assets, then tools. An upstream check of the last hour serves, except with `--check-now` |
| `obc data sources --layer [LAYER] [--json]` | For each layer, or LAYER, the sources that it reads with their status, as the [source overview](#source-overview) says |
| `obc data sources (--region ID\|PATH[,PATH…] \| --box W,S,E,N) [--json]` | The sources that a bake of the region reads, what each layer and each source still needs, and what a bake still downloads or prepares, as the [source overview](#source-overview) says. Fetches nothing |
| `obc data source refresh ID [--json]` | Fetches upstream's newest version of a `digest` source without `start` or params, and writes `refreshed/<source>.json` of the store with the versions that the targets serve: the next update of a target that serves one of them, or none, reads it, as [Versions](#versions) says. Refuses another source. The fetch is a detached [run](#runs): a terminal follows it, and `--json` writes `{run}` and ends |
| `obc data source prepare ID [--region ID\|PATH[,PATH…] \| --box W,S,E,N] [--languages L,…] [--dry-run \| --expect ID] [--json]` | [Prepares](#prepare) a version of a prepared source in the archive. `--dry-run` writes the review. Else it starts a detached run and writes its log until it ends; with `--json`, it writes the run and the review id |
| `obc data source copy ID --from TARGET [--dry-run] [--json]` | [Copies](#copy) the versions of a prepared source from a target into the archive. `--dry-run` writes the plan. Else it starts a detached run and writes its log until it ends; with `--json`, it writes `{run}` |
| `obc data source credential ID [--json]` | Reads the values of the credential of a source on standard input and writes them, as [Credentials of sources](#credentials-of-sources) says. Writes the file and the names of the values |
| `obc data fetch SOURCE[@VERSION] [NAME=VALUE…] [--json]` | Fetches the version, or else the newest file upstream. Writes the store path of each file |
| `obc data archive status [--json]` | The [status](#status) of the archive. Changes nothing |
| `obc data archive get DATASET [--root PATH] [--json]` | [Gets](#get) the dataset. Writes the status of the dataset |
| `obc data archive verify DATASET [--json]` | [Verifies](#verify) the dataset. Writes the status of the dataset |
| `obc data areas [QUERY] [--json]` | The regions of `data/regions/` and the Geofabrik areas of the cached index whose id or name holds `QUERY`. Fetches nothing |
| `obc data clean [--apply [--yes]] [--json]` | The plan of [Clean](#clean). `--apply` asks, then cleans. With `--json` and `--apply`, the plan goes to standard error, and the output is what it did |
| `obc data runs [--json]` | Every run in the store, newest first: id, arguments, state, time, and the size of its fetches and of the layers that it built |
| `obc data runs RUN [--json]` | One run: its status, with the users of each step and its time in the last run that built it, the last 50 lines of its log with its path, and the review of a run of `update --plan` |
| `obc data runs RUN --follow [--json]` | The log of the run as it comes, until it ends; with `--json`, its status each time it changes. Ctrl-C ends only the following. The exit status is the one of the run |
| `obc data runs RUN --stop [--json]` | Stops a run that runs, as [Runs](#runs) says. With `--json`, writes the run as `runs RUN` does |
| `obc data serve [TARGET] [--detach] [--json]` | [Serves](#serve) a target until Ctrl-C. Writes its origin on the machine, and for a `dir` target its origin on the network and how each app opens it. With `--detach`, it serves in a detached [run](#runs) and writes `{run}`; `runs RUN --stop` stops it. That run does not hold the store for [Clean](#clean) |
| `obc data deploy [--json]` | The commit of this checkout; for each part of [Deploy](#deploy), the commit that it runs or why it does not answer, and the checks of a deploy; and the checks of a publish to `prod`. Changes nothing |
| `obc data deploy services\|tiles [--dry-run \| --expect COMMIT [--yes]] [--json]` | `--dry-run` writes what the deploy does. Else it deploys the commit in a detached run and writes its log until it ends; with `--json`, it writes `{run}` |
| `obc data deploy credential services\|tiles [--json]` | Reads the values of the credential of the deploy on standard input and writes them to `credentials.env`. Writes the file and the names of the values |
| `obc data deploy trust --expect FINGERPRINT [--yes] [--json]` | Adds the host key of the VPS to `~/.ssh/known_hosts`, as [Deploy](#deploy) says. Writes the fingerprint and the file |
| `obc data setup [--json]` | What this machine can do: each job, each tool with its version or its install command (Java needs 21 or newer, Node 22 or newer), each credential (only whether it is set, and how to set it), and the store with its size and free space. It exits 0 also when a tool or a credential is missing |
| `obc data ui` | Builds the page with `npm run build:data` in `builder/web`, then serves it as [The page](#the-page) says, until it is stopped |
| `obc data open MACHINE` | Runs `ssh -N -L 8701:localhost:8700 HOST`, where HOST is the `ssh` of MACHINE in [`data/machines.toml`](#datamachinestoml) or else MACHINE, and opens `http://localhost:8701/` in the browser. It sends nothing to MACHINE |

`--json` writes one JSON document to standard output. [JSON schemas](#json-schemas) has the
schema of each output, and [Errors](#errors) has the error codes and the exit statuses. In
addition:

- `fetch` lists only the requested files.
- `runs` lists a run whose `status.json` cannot be read as `failed`, or as `running` while its
  lock is held.
- `runs RUN --follow --json` writes one status per line. When the run failed, the error is the
  last line.

### The page

`obc data ui` listens on `127.0.0.1:8700` and answers:

- `GET /` with `builder/web/dist/data/data.html`, and `GET /PATH` with the file PATH in that
  folder. A part of PATH that is empty, `.` or `..`, or that holds `\`, names no file.
- `POST /api/cli` with the body `{"args": [ARG, …]}`: it runs its own binary as
  `obc data --json ARG…` in the repository root, with no standard input, and answers
  `{"exit": STATUS, "output": JSON}`, the exit status and the JSON that the command wrote. When
  the command writes no JSON, `output` is a `failed` [error](#errors) and the status is not 0.
  A command runs to its end, and its output is read, also when the request ends first. The page
  runs only `obc data` without a command, `obc data setup`, `obc data areas`, `obc data sources`, `obc data sources --layer`, `obc data sources --region ID`, `obc data runs`, `obc data runs RUN`,
  `obc data runs RUN --stop`, `obc data router TARGET`, `obc data serve TARGET --detach`, `obc data update TARGET` with settings flags and then `--dry-run`,
  `--plan` or `--expect ID` (with `--accept-format-change` and `--yes`), `obc data rollback TARGET --expect ID`
  (with `--yes`), `obc data unlock TARGET --from MACHINE --yes`, `obc data source refresh ID`, and
  `obc data source prepare ID` with `--region`, `--box` and `--languages` and then `--dry-run` or
  `--expect ID`, `obc data clean`, `obc data clean --apply --yes`, `obc data deploy`,
  `obc data deploy services|tiles --dry-run` and `--expect COMMIT --yes`, `obc data deploy trust
  --expect FINGERPRINT --yes`, `obc data r2 legacy
  TARGET`, and `obc data r2 legacy TARGET --delete --expect DIGEST --reason TEXT --yes`.
- `POST /api/credential` with the body `{"args": ARGS, "values": [VALUE, …]}`, where ARGS is
  `["source", "credential", ID]` or `["deploy", "credential", "services"|"tiles"]`: it runs
  `obc data --json ARGS…` the same way, with each value on a line of its own on standard input,
  and answers in the same form. A body that is not of this form, an ID that starts with `-`, or a
  value with a line break gets HTTP 400 and the code `usage`. The page logs no body,
  and no answer holds a value.

Each request has a `Host` of `127.0.0.1`, `localhost` or `[::1]` at any port; an `Origin`, when
it has one, of `http://` and that `Host`; and, for a POST, the `Content-Type` `application/json`.
A request that does not, and a command that the page does not run, get HTTP 403 and
`{"exit": 2, "output": {"error": …}}` with the code `usage`.

## R2 client

The R2 client in `host/obc-data` reaches the bucket for `obc bake publish --target r2`,
`obc bake clean-r2`, `obc fixtures publish` and the firmware publish workflow. rclone moves the
bytes. `obc data r2` is plumbing for scripts: those commands call it, and a person deletes a key with
`obc data r2 delete`. It does not change the state of a release. Once the bucket has the
`release.json` of `obc data`, `obc bake publish --target r2` refuses to run, and `obc data r2 put`
and `delete` refuse `release.json`, `lock.json`, `planner/next.json`, a pointer key, a key
under `objects/` or `releases/`. `obc data r2 delete` refuses a key under `sources/<id>/` of a
prepared source while the [archive](#prepared-sources) lacks a version that
`sources/<id>/versions.json` of the bucket lists, with its manifest SHA-256 and that manifest.
`obc bake clean-r2` refuses to run when the pointer of its prefix has `release`, or is not JSON. `obc data` never writes under `reference/v1/`.

### Credentials

| Variable | Meaning |
| --- | --- |
| `OBC_R2_BUCKET`, `OBC_R2_ACCESS_KEY_ID`, `OBC_R2_SECRET_ACCESS_KEY` | The bucket and its key |
| `OBC_R2_ACCOUNT_ID` | The Cloudflare account; the endpoint is `https://<id>.r2.cloudflarestorage.com` |
| `OBC_R2_ENDPOINT` | Another endpoint; replaces the one from `OBC_R2_ACCOUNT_ID` |
| `OBC_R2_LOCAL_DIR` | A local directory that replaces the bucket. Tests use it. When it is set, it must not be empty, and `OBC_R2_BUCKET` must not be set |

`--fixtures` reads the same variables with the prefix `OBC_FIXTURE_R2_` instead. `legacy` refuses
`--fixtures`: it reads the bucket of its target. A credential goes to rclone only in its
environment, never in an argument or a file. The S3 client of an R2 target signs with the secret
key and never sends it.

### Commands

| Command | Does |
| --- | --- |
| `obc data r2 list PREFIX [--json]` | Lists every object under the folder `PREFIX`. A `PREFIX` that names an object is refused |
| `obc data r2 stat KEY... [--json]` | Lists the objects of the keys that the bucket holds; a missing key is not an error |
| `obc data r2 get KEY FILE [--json]` | Downloads one object |
| `obc data r2 put FILE KEY [--cache-control V] [--content-type V] [--immutable] [--json]` | Uploads with `--checksum` and the headers given, then verifies |
| `obc data r2 delete (KEY... \| --prefix P) --reason TEXT [--legacy] [--yes] [--json]` | Deletes, see below |
| `obc data r2 legacy TARGET [--prefix P] [--delete --expect DIGEST --reason TEXT [--yes]] [--json]` | Lists the legacy keys that the target does not use; with `--delete`, deletes them, see below |

Rules:

- The `r2` commands are plumbing below the rule of [Errors](#errors). Only `delete` and
  `legacy --delete` ask: a delete cannot be undone. `put` and `get` never ask.
- A key or a prefix is never empty, has no empty, `.` or `..` part, and has no control
  character. The bucket root is never a target.
- Verify passes when the object has the size of the file, and the same MD5 when both sides
  report one.
- `--immutable`: when the key holds an object, nothing is uploaded and the object is never
  replaced. The command passes only when the object has the size and the MD5 of the file. It
  fails when they differ, and when the bucket reports no MD5 for the object.
- `delete --prefix P` resolves `P` to its keys first. Then explicit keys and a prefix have the
  same rules: a key that the bucket does not hold is refused, `removed.jsonl` is refused, a key
  under `cell-catalog/`, `planner/` or `reference/` is refused without `--legacy`, and then nothing
  is deleted. Legacy keys go only after the cutover, from the list that the owner approved.
- `legacy` lists each key under `cell-catalog/` and `planner/`, or under the one `--prefix`, that
  `obc data` does not write. It refuses while the target serves no release. It refuses
  `reference/v1` while `reference/v1/index.json` is missing or does not parse, and while the
  `index.json` of the newest version of `terrain` in the archive lacks a key of its `sources`. The
  digest is the SHA-256 of one line `<bytes>  <key>\n` per key, sorted by key. With `--delete`, it lists again,
  refuses when the digest is not `--expect`, asks, and then deletes in a detached [run](#runs): as
  `delete --legacy`, with the counts, the bytes and the digest as its plan. The run lists again
  and refuses another digest too. One run at a time deletes the keys of a target. With no key, the
  run deletes nothing. With `--json`, it writes `{run}`; else it follows the log of the run.
- `delete` prints the plan: the bucket, and each object with its size and
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
| `usage` | 2 | An argument is not valid, an id names nothing, the command runs outside the repository, or another command must run first. | Correct the command. `obc data --help` lists the commands and their arguments. |
| `no_terminal` | 2 | A command that asks has no terminal to ask in, and no `--yes`. | Show the question to a person. When they agree, run the command again with `--yes`. |
| `not_confirmed` | 1 | The person did not answer yes. | Nothing changed. Run the command again when you want the change. |
| `invalid_data` | 1 | A file under `data/` is not valid. | Correct the file that the message names. `specs/obc-data.md` gives its format. |
| `fetch_failed` | 1 | A fetch or an upstream check failed. | Run the command again. A download continues where it stopped. |
| `blocked` | 4 | A credential is missing: a fetch failed without the credential of its source, or the R2 variables are not set. Or a layer cannot be planned, the target cannot take the command, or this machine cannot do the work. | Set the credential that the message or `obc data sources` names, or correct what the message says a product needs, then run again. |
| `r2_failed` | 1 | R2 or rclone failed, or refused a key. | Check the key, the `OBC_R2_*` variables and that rclone is on PATH, then run again. |
| `verify_failed` | 5 | A release failed its check before a target serves it, or after an upload the object in the bucket is not the file. | Upload the file again. |
| `run_failed` | 1 | A run failed: the build, or the run that `runs RUN --follow` shows. | `obc data runs RUN` shows the step that failed and its error. |
| `failed` | 1 | The store or the file system failed. | Correct the file or the directory that the message names, then run again. |

## JSON schemas

`--json` writes one document of the schema in this table to standard output. The schemas
come from the Rust types in `host/obc-data`. A test fails when this section is not the one
that they give; `OBC_UPDATE_DATA_SPEC=1 cargo test -p obc-data` writes it again.

| Command | Schema |
| --- | --- |
| `obc data` | `Overview` |
| `update` | `Started` |
| `update --dry-run` | `Review` |
| `rollback` | `RolledBack` |
| `router` | `Router` |
| `unlock` | `Unlocked` |
| `sources` | `Sources` |
| `sources --layer` | `SourceLayers` |
| `sources --region`, `sources --box` | `RegionSources` |
| `source refresh`, `source copy`, `serve --detach`, `deploy WHAT --expect`, `r2 legacy --delete` | `Detached` |
| `source prepare` | `Started` |
| `source prepare --dry-run` | `PrepareReview` |
| `source copy --dry-run` | `CopyPlan` |
| `fetch` | `Fetched` |
| `archive status` | `ArchiveStatus` |
| `archive get`, `archive verify` | `ArchiveDataset` |
| `areas` | `Areas` |
| `clean`, `clean --apply` | `CleanPlan` |
| `serve`, when it listens | `Listening` |
| `setup` | `Setup` |
| `source credential`, `deploy credential` | `CredentialSet` |
| `deploy` | `Deployed` |
| `deploy WHAT --dry-run` | `DeployPlan` |
| `deploy trust` | `Trusted` |
| `runs` | `RunList` |
| `runs RUN`, `runs RUN --stop` | `View` |
| `runs RUN --follow`, one per line | `Status` |
| `r2 list`, `stat`, `delete` | `Objects` |
| `r2 get` | `Downloaded` |
| `r2 put` | `Uploaded` |
| `r2 legacy` | `Legacy` |
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
    "Answer": {
      "description": "What the follower says about the planner release of a release.",
      "properties": {
        "reason": {
          "description": "Why the follower does not serve it; `null` when it does.",
          "type": [
            "string",
            "null"
          ]
        },
        "release": {
          "type": "string"
        }
      },
      "required": [
        "release",
        "reason"
      ],
      "type": "object"
    },
    "ArchiveDataset": {
      "description": "A dataset of the archive.",
      "properties": {
        "bytes": {
          "description": "The bytes of the verified files.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "bytes_estimate": {
          "description": "The `bytes_estimate` of the definition.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "coverage": {
          "type": "string"
        },
        "dataset": {
          "type": "string"
        },
        "error": {
          "description": "Why the manifest is `unreadable`.",
          "type": [
            "string",
            "null"
          ]
        },
        "got": {
          "description": "The day of the last `archive get`.",
          "type": [
            "string",
            "null"
          ]
        },
        "listed": {
          "description": "The files that the last get listed.",
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        },
        "listed_bytes": {
          "description": "Their bytes, as far as the manifest knows them.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "root": {
          "description": "The root that has its manifest.",
          "type": [
            "string",
            "null"
          ]
        },
        "sources": {
          "description": "Ids of `data/sources.toml`.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "state": {
          "$ref": "#/$defs/DatasetState"
        },
        "verified": {
          "description": "The files of the manifest that are in the root with their size: they serve.",
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        },
        "version": {
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "dataset",
        "sources",
        "coverage",
        "root",
        "state",
        "error",
        "version",
        "got",
        "listed",
        "listed_bytes",
        "verified",
        "bytes",
        "bytes_estimate"
      ],
      "type": "object"
    },
    "ArchiveRoot": {
      "properties": {
        "free_bytes": {
          "description": "`null` when the root does not exist.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "path": {
          "type": "string"
        }
      },
      "required": [
        "path",
        "free_bytes"
      ],
      "type": "object"
    },
    "ArchiveStatus": {
      "description": "The output of `obc data archive status`.",
      "properties": {
        "datasets": {
          "description": "Each dataset of `data/archive/`.",
          "items": {
            "$ref": "#/$defs/ArchiveDataset"
          },
          "type": "array"
        },
        "roots": {
          "description": "The roots of `OBC_DATA_ARCHIVE`, in order.",
          "items": {
            "$ref": "#/$defs/ArchiveRoot"
          },
          "type": "array"
        },
        "unarchived": {
          "description": "The sources with `keep = \"archive\"` that no dataset has.",
          "items": {
            "type": "string"
          },
          "type": "array"
        }
      },
      "required": [
        "roots",
        "datasets",
        "unarchived"
      ],
      "type": "object"
    },
    "AreaBoxes": {
      "description": "An area as boxes.",
      "properties": {
        "boxes": {
          "description": "A box for each part: the box of a box region, and the bounds of each Geofabrik area in the\ncached index.",
          "items": {
            "$ref": "#/$defs/Bbox"
          },
          "type": "array"
        },
        "missing": {
          "description": "Why a part has no box; `null` when each part has one.",
          "type": [
            "string",
            "null"
          ]
        },
        "name": {
          "type": "string"
        }
      },
      "required": [
        "name",
        "boxes",
        "missing"
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
    "Check": {
      "description": "One thing that an action needs.",
      "properties": {
        "action": {
          "description": "The arguments after `obc data` of the command that applies the fix, which the page runs\nafter a confirm; `null` when a person applies it.",
          "items": {
            "type": "string"
          },
          "type": [
            "array",
            "null"
          ]
        },
        "blocks": {
          "description": "Whether the action waits for it.",
          "type": "boolean"
        },
        "commands": {
          "description": "The commands of the fix, one per line.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "fix": {
          "description": "What to do about the problem, and where the commands run.",
          "type": [
            "string",
            "null"
          ]
        },
        "name": {
          "type": "string"
        },
        "problem": {
          "description": "Why it fails; `null` when it passes.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "name",
        "problem",
        "fix",
        "commands",
        "action",
        "blocks"
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
    "CopiedVersion": {
      "properties": {
        "copy": {
          "$ref": "#/$defs/Downloads",
          "description": "The files that the home lacks."
        },
        "held": {
          "$ref": "#/$defs/Downloads",
          "description": "The files that the home holds."
        },
        "manifest_sha256": {
          "type": "string"
        },
        "version": {
          "type": "string"
        }
      },
      "required": [
        "version",
        "manifest_sha256",
        "copy",
        "held"
      ],
      "type": "object"
    },
    "CopyPlan": {
      "description": "What a copy does.",
      "properties": {
        "from": {
          "type": "string"
        },
        "root": {
          "description": "The archive root of the home of the source.",
          "type": "string"
        },
        "source": {
          "type": "string"
        },
        "versions": {
          "description": "Each version of the target, oldest first.",
          "items": {
            "$ref": "#/$defs/CopiedVersion"
          },
          "type": "array"
        }
      },
      "required": [
        "source",
        "from",
        "root",
        "versions"
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
        },
        "hint": {
          "description": "What to copy there, in one line.",
          "type": [
            "string",
            "null"
          ]
        },
        "register": {
          "description": "Where a person gets the credential.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "type": "object"
    },
    "CredentialSet": {
      "additionalProperties": false,
      "description": "What `source credential` wrote. It never holds a value.",
      "properties": {
        "file": {
          "description": "The file that holds the values now.",
          "type": "string"
        },
        "names": {
          "description": "The names of the values that it wrote.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "overridden": {
          "description": "The names that the environment of the command sets, from the shell or `tools/obc.local`:\nthe environment wins over the file, so the new value serves only once they are removed\nthere.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "sources": {
          "description": "The sources that the credential serves; none for a credential of `deploy`.",
          "items": {
            "type": "string"
          },
          "type": "array"
        }
      },
      "required": [
        "file",
        "names",
        "sources",
        "overridden"
      ],
      "type": "object"
    },
    "CredentialState": {
      "additionalProperties": false,
      "properties": {
        "fields": {
          "description": "The values that the command reads, one per line, in this order.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "hint": {
          "description": "What to copy there.",
          "type": [
            "string",
            "null"
          ]
        },
        "name": {
          "type": "string"
        },
        "needed_for": {
          "type": "string"
        },
        "register": {
          "description": "Where a person gets it.",
          "type": [
            "string",
            "null"
          ]
        },
        "set": {
          "description": "The arguments after `obc data` of the command that sets it from standard input, such as\n`source credential SOURCE` for each source that needs it. `null` for the R2 key, which the\npage does not set.",
          "items": {
            "type": "string"
          },
          "type": [
            "array",
            "null"
          ]
        },
        "state": {
          "$ref": "#/$defs/Readiness"
        },
        "uses": {
          "description": "The `use` of each source that it serves.",
          "items": {
            "type": "string"
          },
          "type": "array"
        }
      },
      "required": [
        "name",
        "needed_for",
        "state",
        "set",
        "fields",
        "uses",
        "register",
        "hint"
      ],
      "type": "object"
    },
    "DatasetState": {
      "oneOf": [
        {
          "const": "complete",
          "description": "Each listed file is verified.",
          "type": "string"
        },
        {
          "const": "incomplete",
          "description": "A listed file is missing.",
          "type": "string"
        },
        {
          "const": "absent",
          "description": "No root has the manifest.",
          "type": "string"
        },
        {
          "const": "unreadable",
          "description": "The manifest cannot be read, or has no `version`: no file of it serves.",
          "type": "string"
        }
      ]
    },
    "DeployPlan": {
      "description": "What `deploy WHAT --dry-run` writes.",
      "properties": {
        "args": {
          "description": "The arguments after `obc data` that start it.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "commands": {
          "description": "The commands that it runs in the repository root, in this order.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "commit": {
          "description": "The commit of this checkout that it deploys.",
          "type": "string"
        },
        "text": {
          "description": "What the deploy changes.",
          "type": "string"
        },
        "what": {
          "type": "string"
        }
      },
      "required": [
        "what",
        "commit",
        "text",
        "commands",
        "args"
      ],
      "type": "object"
    },
    "Deployed": {
      "description": "What `deploy` writes.",
      "properties": {
        "checkout": {
          "description": "The commit of this checkout: what a deploy puts in place.",
          "type": "string"
        },
        "publish": {
          "description": "What `update prod` and the delete of its old data need on this machine.",
          "items": {
            "$ref": "#/$defs/Check"
          },
          "type": "array"
        },
        "services": {
          "$ref": "#/$defs/Running"
        },
        "tiles": {
          "$ref": "#/$defs/Running"
        }
      },
      "required": [
        "checkout",
        "publish",
        "services",
        "tiles"
      ],
      "type": "object"
    },
    "Detached": {
      "description": "What a command that starts a detached run writes: the run.",
      "properties": {
        "run": {
          "type": "string"
        }
      },
      "required": [
        "run"
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
        "guess": {
          "description": "It comes from a build of the step with other options, such as another region, scaled to the\ninput size of the step.",
          "type": "boolean"
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
        "peak_rss_bytes",
        "guess"
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
        },
        {
          "const": "prepared",
          "description": "`obc data source prepare` makes its versions in the archive.",
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
        "archive": {
          "description": "The path in the archive (`OBC_DATA_ARCHIVE`) of the file that gave the bytes instead of `url`.",
          "type": [
            "string",
            "null"
          ]
        },
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
    "Files": {
      "properties": {
        "listed": {
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        },
        "verified": {
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "verified",
        "listed"
      ],
      "type": "object"
    },
    "ForRegion": {
      "description": "What a bake of the region reads of a source, and what it still needs.",
      "properties": {
        "after_stop": {
          "description": "The step list of a product that reads it stopped before it read the source: its fetches\nare not known, and `need` is `unknown` but for a prepared source.",
          "type": "boolean"
        },
        "download_bytes": {
          "description": "The known bytes to download.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "held_bytes": {
          "description": "The known bytes of the held ones.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "held_requests": {
          "description": "Of them, those that the store or the archive holds.",
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        },
        "need": {
          "$ref": "#/$defs/Need"
        },
        "read_by": {
          "description": "For a source that only a prepare reads: the prepared source whose need it has.",
          "type": [
            "string",
            "null"
          ]
        },
        "requests": {
          "description": "The fetches of the source that the step lists read: one per set of params.",
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        },
        "unknown_requests": {
          "description": "The fetches to download of unknown size.",
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "need",
        "requests",
        "held_requests",
        "held_bytes",
        "download_bytes",
        "unknown_requests",
        "read_by",
        "after_stop"
      ],
      "type": "object"
    },
    "FormatChange": {
      "properties": {
        "from": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "readers": {
          "description": "What the new format means for each reader of the layer.",
          "items": {
            "$ref": "#/$defs/FormatReader"
          },
          "type": "array"
        },
        "to": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "from",
        "to",
        "readers"
      ],
      "type": "object"
    },
    "FormatReader": {
      "properties": {
        "effect": {
          "type": "string"
        },
        "reader": {
          "type": "string"
        }
      },
      "required": [
        "reader",
        "effect"
      ],
      "type": "object"
    },
    "GcPlan": {
      "description": "What `clean` deletes from the store, or deleted, and what stays.",
      "properties": {
        "keep_bytes": {
          "description": "The size of the objects and the unpacked copies that stay.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "keep_copies": {
          "description": "The unpacked copies that stay.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "keep_objects": {
          "description": "The objects that stay.",
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
          "description": "The size of `partial/`: unfinished downloads and the work of steps that stopped. A\ncollection removes it.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "remove_bytes": {
          "description": "The size of `objects` and `unpacked`.",
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
        },
        "unpacked": {
          "description": "The SHA-256 and size of each unpacked copy of an archive file that the archive no longer lists.",
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
        }
      },
      "required": [
        "kept",
        "snapshots",
        "objects",
        "remove_bytes",
        "keep_objects",
        "keep_copies",
        "keep_bytes",
        "partial_bytes",
        "unpacked"
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
    "Held": {
      "description": "How this machine holds a source.",
      "properties": {
        "archive_files": {
          "anyOf": [
            {
              "$ref": "#/$defs/Files"
            },
            {
              "type": "null"
            }
          ],
          "description": "The verified and the listed files of its dataset; `null` while no manifest lists a file."
        },
        "bytes": {
          "description": "The bytes held; `null` when the source holds none of its own.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "covers": {
          "description": "Where the source has data, in words.",
          "type": "string"
        },
        "dataset": {
          "description": "The dataset of `data/archive/` of the source.",
          "type": [
            "string",
            "null"
          ]
        },
        "date": {
          "description": "`YYYY-MM-DD`: the last `archive get`, the newest version that holds it, or the newest file\nin the store.",
          "type": [
            "string",
            "null"
          ]
        },
        "download_bytes": {
          "description": "What an `archive get` still takes; `null` when unknown or nothing is to get.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "error": {
          "description": "Why its dataset or its prepared versions cannot be read.",
          "type": [
            "string",
            "null"
          ]
        },
        "gap": {
          "description": "A national terrain model that nothing holds: the terrain there is GLO-30 only.",
          "type": "boolean"
        },
        "no_dataset": {
          "description": "`keep = \"archive\"`, and no dataset of `data/archive/` has the source.",
          "type": "boolean"
        },
        "prepared_by": {
          "description": "The prepared source whose versions hold it: itself, or the one that reads it.",
          "type": [
            "string",
            "null"
          ]
        },
        "rule": {
          "$ref": "#/$defs/Rule"
        },
        "status": {
          "$ref": "#/$defs/SourceStatus"
        },
        "store_requests": {
          "description": "The requests of the source with params in the store, the newest of each set of params.",
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        },
        "store_versions": {
          "description": "The versions of the source in the store.",
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        },
        "where": {
          "description": "The archive root of its dataset, the home of the prepared source that holds it, `store`, or\nthe host of an asset that R2 keeps; `null` when nothing is held.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "rule",
        "covers",
        "status",
        "where",
        "dataset",
        "prepared_by",
        "bytes",
        "download_bytes",
        "date",
        "archive_files",
        "store_versions",
        "store_requests",
        "gap",
        "no_dataset",
        "error"
      ],
      "type": "object"
    },
    "HeldSource": {
      "properties": {
        "held": {
          "$ref": "#/$defs/Held"
        },
        "source": {
          "type": "string"
        }
      },
      "required": [
        "source",
        "held"
      ],
      "type": "object"
    },
    "Holder": {
      "description": "The run that holds the lock of a target. A reader ignores keys that it does not know, so builds\nof `obc data` read each other's lock.",
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
        "progress": {
          "default": [],
          "description": "Each layer that the run bakes, at the last heartbeat. The status of the run on its machine\nhas the detail.",
          "items": {
            "$ref": "#/$defs/Progress"
          },
          "type": "array"
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
        },
        "waiting": {
          "description": "Why the run waits, in the phase `router-ready`: the last answer of the follower of the\ntarget.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "machine",
        "run",
        "review",
        "phase",
        "heartbeat",
        "progress"
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
    "Keep": {
      "description": "Where the copy of a source that a bake reads stays, so that no outage stops a bake.",
      "oneOf": [
        {
          "const": "archive",
          "description": "The archive (`OBC_DATA_ARCHIVE`) holds the whole dataset: a fetch reads it there first.",
          "type": "string"
        },
        {
          "const": "store",
          "description": "The store keeps what each fetch took.",
          "type": "string"
        },
        {
          "const": "prepared",
          "description": "We keep what we make: the archive keeps a prepared source and a source that only a prepare\nreads, and R2 keeps our own asset.",
          "type": "string"
        }
      ]
    },
    "Kept": {
      "description": "A snapshot record, the layers of one step, or the objects that one kind of root names and no\nkept record or layer has.",
      "properties": {
        "because": {
          "description": "`served by TARGET, …`, `newest of the source`, `newest of a request`, `refreshed`, `inputs\nkept`, `release`, `fixture`, `in the archive` or `archive not read`.",
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
        "dropped_leaves": {
          "description": "The grid leaves of `drops` of a tiled layer, as `leaves` names them.",
          "items": {
            "type": "string"
          },
          "type": "array"
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
        "partitions": {
          "description": "The partitions of a partitioned layer whose steps bake.",
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
        "dropped_leaves",
        "readers",
        "format"
      ],
      "type": "object"
    },
    "LayerProgress": {
      "properties": {
        "done": {
          "default": 0,
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        },
        "layer": {
          "default": "",
          "type": "string"
        },
        "leaves_baking": {
          "default": [],
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "leaves_done": {
          "default": [],
          "description": "The leaves `<i>-<j>` of a tiled layer whose steps finished, and those whose steps bake now.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "leaves_unchanged": {
          "default": [],
          "description": "The finished leaves whose steps all kept their output of the serving release.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "started": {
          "default": null,
          "description": "When its first step started, `YYYY-MM-DDTHH:MM:SSZ`; `null` before.",
          "type": [
            "string",
            "null"
          ]
        },
        "total": {
          "default": 0,
          "description": "The steps of the layer that the run bakes, and those that finished.",
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "layer",
        "total",
        "done",
        "leaves_done",
        "leaves_baking",
        "leaves_unchanged",
        "started"
      ],
      "type": "object"
    },
    "LayerShape": {
      "description": "How the steps of a layer bake.",
      "oneOf": [
        {
          "const": "tiled",
          "description": "Per grid leaf.",
          "type": "string"
        },
        {
          "const": "whole",
          "description": "Once over the coverage of a target, also in named partitions (`partitions`).",
          "type": "string"
        }
      ]
    },
    "LayerSources": {
      "description": "A name of `used_by` and the sources that it reads. Whether a bake of a region still downloads\nor prepares is in `sources --region`.",
      "properties": {
        "layer": {
          "type": "string"
        },
        "sources": {
          "description": "The `data` sources and assets whose `used_by` has the layer, but no source that only a\nprepare reads.",
          "items": {
            "$ref": "#/$defs/HeldSource"
          },
          "type": "array"
        }
      },
      "required": [
        "layer",
        "sources"
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
        "format": {
          "description": "The format and the revision of its steps; `null` for a layer that only the release has.",
          "format": "uint32",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "layer": {
          "type": "string"
        },
        "leaves": {
          "description": "The grid leaves of a tiled layer, as `<i>-<j>`, sorted; empty for another layer.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "partitions": {
          "description": "The partitions of a partitioned layer that its steps make, sorted; absent for another layer.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "readers": {
          "description": "The programs that read its files.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "reads": {
          "description": "The layers and the sources that its steps read, sorted. In place of a layer that no program\nreads, it names what that layer reads.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "revision": {
          "format": "uint32",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "served_format": {
          "description": "The format and the revision of the layer in the release; `null` while the release has none.",
          "format": "uint32",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "served_revision": {
          "format": "uint32",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "shape": {
          "anyOf": [
            {
              "$ref": "#/$defs/LayerShape"
            },
            {
              "type": "null"
            }
          ],
          "description": "How its steps bake; `null` for a layer that only the release has."
        },
        "stale_leaves": {
          "description": "The leaves whose steps a build changes.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "stale_partitions": {
          "description": "The partitions whose steps a build changes.",
          "items": {
            "type": "string"
          },
          "type": "array"
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
        "text",
        "shape",
        "format",
        "revision",
        "served_format",
        "served_revision",
        "readers",
        "reads",
        "leaves",
        "stale_leaves"
      ],
      "type": "object"
    },
    "Legacy": {
      "description": "The keys of the legacy pipelines that a target does not use.",
      "properties": {
        "digest": {
          "description": "The SHA-256 of the lines `<bytes>  <key>\\n` of the keys of every folder, sorted by key.",
          "type": "string"
        },
        "folders": {
          "additionalProperties": {
            "items": {
              "$ref": "#/$defs/LegacyKey"
            },
            "type": "array"
          },
          "description": "The keys of each folder, sorted.",
          "type": "object"
        },
        "target": {
          "type": "string"
        }
      },
      "required": [
        "target",
        "folders",
        "digest"
      ],
      "type": "object"
    },
    "LegacyKey": {
      "properties": {
        "bytes": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
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
    "Link": {
      "properties": {
        "app": {
          "type": "string"
        },
        "how": {
          "description": "The command, or what to do, with absolute paths.",
          "type": "string"
        }
      },
      "required": [
        "app",
        "how"
      ],
      "type": "object"
    },
    "Listening": {
      "description": "What `serve` writes when it listens.",
      "properties": {
        "links": {
          "description": "How each app of this machine and of its network opens a dir target; empty for an R2 target.",
          "items": {
            "$ref": "#/$defs/Link"
          },
          "type": "array"
        },
        "network": {
          "description": "Its origin on the network, for a phone; `null` without a network and for an R2 target.",
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
        "network",
        "links"
      ],
      "type": "object"
    },
    "Lock": {
      "description": "The run that holds the lock of a target, as its `lock.json` says.",
      "properties": {
        "died": {
          "description": "Whether the run ran on this machine and its process ended: the next run takes the lock\nover.",
          "type": "boolean"
        },
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
        "progress": {
          "default": [],
          "description": "Each layer that the run bakes, at the last heartbeat. The status of the run on its machine\nhas the detail.",
          "items": {
            "$ref": "#/$defs/Progress"
          },
          "type": "array"
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
        },
        "stale": {
          "description": "Whether the last heartbeat is as old as `unlock` needs, by the clock of this machine: the\nrun likely ended without freeing the lock.",
          "type": "boolean"
        },
        "waiting": {
          "description": "Why the run waits, in the phase `router-ready`: the last answer of the follower of the\ntarget.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "machine",
        "run",
        "review",
        "phase",
        "heartbeat",
        "progress",
        "stale",
        "died"
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
        "load": {
          "description": "The load average of the last minute; `null` where the system does not tell.",
          "format": "double",
          "type": [
            "number",
            "null"
          ]
        },
        "memory_available_bytes": {
          "description": "The memory that programs can still take; `null` where the system does not tell.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
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
        "store_free_bytes",
        "load",
        "memory_available_bytes"
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
    "Made": {
      "properties": {
        "machine": {
          "type": "string"
        },
        "time": {
          "description": "`YYYY-MM-DDTHH:MM:SSZ`.",
          "type": "string"
        }
      },
      "required": [
        "machine",
        "time"
      ],
      "type": "object"
    },
    "Need": {
      "description": "What a bake of a region still needs of a source or a layer, from the least to the most.",
      "oneOf": [
        {
          "const": "held",
          "description": "The store, the archive or a prepared version holds what the bake reads.",
          "type": "string"
        },
        {
          "const": "download",
          "description": "A fetch downloads a part, in the bake or in the prepare that reads it.",
          "type": "string"
        },
        {
          "const": "prepare",
          "description": "`obc data source prepare` makes a version that covers the region.",
          "type": "string"
        },
        {
          "const": "unknown",
          "description": "Unknown: a step list needs a snapshot that the store lacks, or the cached index lacks an\narea of a scope.",
          "type": "string"
        }
      ]
    },
    "Note": {
      "additionalProperties": false,
      "description": "A layer that bakes without a source that it reads where the source has a version, such as a\nprepared source that no target lists yet, or that reads an older version of a due source whose\nfetch failed. It does not prevent an update.",
      "properties": {
        "layer": {
          "type": "string"
        },
        "text": {
          "description": "What the layer bakes without and the command that gives it, or the version that it reads.",
          "type": "string"
        }
      },
      "required": [
        "layer",
        "text"
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
        "build",
        "verify",
        "upload",
        "router-ready",
        "switch",
        "collect"
      ],
      "type": "string"
    },
    "PhaseState": {
      "properties": {
        "phase": {
          "$ref": "#/$defs/Phase"
        },
        "started": {
          "description": "When it began, `YYYY-MM-DDTHH:MM:SSZ`.",
          "type": "string"
        },
        "state": {
          "$ref": "#/$defs/State2"
        },
        "waiting": {
          "description": "Why it waits: in `router-ready`, the last answer of the follower of the target.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "phase",
        "state",
        "started"
      ],
      "type": "object"
    },
    "PlanBuild": {
      "additionalProperties": false,
      "properties": {
        "again": {
          "description": "Whether the step builds also when the store has the layer of its key.",
          "type": "boolean"
        },
        "built": {
          "description": "When the store has the layer of `key`: the time of its build, `YYYY-MM-DDTHH:MM:SSZ`. The\nupdate reuses that layer unless `again`.",
          "type": [
            "string",
            "null"
          ]
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
        "again",
        "built"
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
    "PrepareReview": {
      "description": "The review of a prepare. Its id names the decisions, so `--expect ID` starts a prepare only when\nthey are the ones that a person read.",
      "properties": {
        "abandoned": {
          "description": "The version that a stopped run of this machine started and that this run does not resume:\nthe home has a newer complete version. `null` when there is none.",
          "type": [
            "string",
            "null"
          ]
        },
        "args": {
          "description": "The arguments after `obc data` that start the prepare of this review.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "cost": {
          "$ref": "#/$defs/PreparedCost"
        },
        "done": {
          "$ref": "#/$defs/Downloads",
          "description": "The files that earlier runs of the version moved into the home: this run does not make them\nagain."
        },
        "files": {
          "description": "The files of the version, at most; `null` when the producer cannot tell before the run.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "format": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "id": {
          "description": "The first 12 hex digits of the SHA-256 of the canonical JSON of the source, the root, the\nversion, the format, the revision, the scope, the languages, `publish_text` and each planned\ninput, including archive paths, URLs, sizes and object identities.",
          "type": "string"
        },
        "inputs": {
          "items": {
            "$ref": "#/$defs/PreparedInput"
          },
          "type": "array"
        },
        "languages": {
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "listing": {
          "description": "Whether the manifest of the version is in the home and names the files of the journal: the\nrun only lists the version, and makes and writes nothing else.",
          "type": "boolean"
        },
        "machine": {
          "$ref": "#/$defs/MachineCheck"
        },
        "publish_text": {
          "description": "Whether the version publishes its article text: `publish_text` of the source; `null` for a\nsource without text.",
          "type": [
            "boolean",
            "null"
          ]
        },
        "revision": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "root": {
          "description": "The archive root of the home of the source.",
          "type": "string"
        },
        "scope": {
          "anyOf": [
            {
              "$ref": "#/$defs/Scope"
            },
            {
              "type": "null"
            }
          ],
          "description": "`null`: every place that the source covers."
        },
        "source": {
          "type": "string"
        },
        "stale": {
          "description": "Each target whose serving release reads an older version, with those layers: they are stale\nonce the version is complete.",
          "items": {
            "$ref": "#/$defs/StaleReaders"
          },
          "type": "array"
        },
        "version": {
          "description": "The version that the run makes: the UTC date when its first run started, with `.2`, `.3`\nand so on after the first version of that day.",
          "type": "string"
        }
      },
      "required": [
        "id",
        "source",
        "root",
        "version",
        "abandoned",
        "listing",
        "format",
        "revision",
        "scope",
        "languages",
        "inputs",
        "files",
        "publish_text",
        "done",
        "cost",
        "machine",
        "stale",
        "args"
      ],
      "type": "object"
    },
    "PreparedCost": {
      "description": "Estimates from the newest version in the home; `null` without one.",
      "properties": {
        "archive_bytes": {
          "description": "What goes into the archive: the files of the version that earlier runs did not move.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "peak_rss_bytes": {
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "store_bytes": {
          "description": "What goes into the store: the inputs that it lacks.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "unknown": {
          "description": "The inputs whose size the store does not know, as `SOURCE@VERSION`.",
          "items": {
            "type": "string"
          },
          "type": "array"
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
        "wall_ms",
        "peak_rss_bytes",
        "store_bytes",
        "archive_bytes",
        "unknown"
      ],
      "type": "object"
    },
    "PreparedIn": {
      "properties": {
        "error": {
          "description": "Why the versions or the lock of the source cannot be read; `null` when they can.",
          "type": [
            "string",
            "null"
          ]
        },
        "lock": {
          "anyOf": [
            {
              "$ref": "#/$defs/Lock"
            },
            {
              "type": "null"
            }
          ]
        },
        "root": {
          "description": "The archive root of the home; `null` when no root of `OBC_DATA_ARCHIVE` has one.",
          "type": [
            "string",
            "null"
          ]
        },
        "versions": {
          "items": {
            "$ref": "#/$defs/Version"
          },
          "type": "array"
        }
      },
      "required": [
        "root",
        "versions",
        "lock",
        "error"
      ],
      "type": "object"
    },
    "PreparedInput": {
      "properties": {
        "bytes": {
          "description": "Input bytes from local files, upstream metadata or the store; `null` when unknown.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "download_bytes": {
          "description": "Archive bytes absent on this machine; null for other source inputs.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
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
          "description": "`null` for the newest version upstream.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "source",
        "version",
        "params",
        "bytes",
        "download_bytes"
      ],
      "type": "object"
    },
    "Progress": {
      "description": "The steps of a layer that a run bakes, and those that finished.",
      "properties": {
        "done": {
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        },
        "layer": {
          "type": "string"
        },
        "total": {
          "format": "uint",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "layer",
        "done",
        "total"
      ],
      "type": "object"
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
    "RegionLayer": {
      "description": "A layer and what a bake of the region still needs for it: the most of its sources.",
      "properties": {
        "layer": {
          "type": "string"
        },
        "need": {
          "$ref": "#/$defs/Need"
        },
        "reason": {
          "description": "Why its step list cannot be listed: then `need` is `unknown`.",
          "type": [
            "string",
            "null"
          ]
        },
        "sources": {
          "description": "The sources that its steps read.",
          "items": {
            "type": "string"
          },
          "type": "array"
        }
      },
      "required": [
        "layer",
        "need",
        "sources",
        "reason"
      ],
      "type": "object"
    },
    "RegionProduct": {
      "description": "A product of the region.",
      "properties": {
        "incomplete": {
          "description": "Its step list stopped: a snapshot that it needs is not in the store, or it does not suit\nthe region. Its other sources come from the table of what it can read.",
          "type": "boolean"
        },
        "product": {
          "type": "string"
        },
        "reason": {
          "description": "Why it stopped.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "product",
        "incomplete",
        "reason"
      ],
      "type": "object"
    },
    "RegionSource": {
      "description": "A source that a bake of the region reads.",
      "properties": {
        "for_region": {
          "$ref": "#/$defs/ForRegion"
        },
        "held": {
          "$ref": "#/$defs/Held",
          "description": "How this machine holds the source."
        },
        "source": {
          "type": "string"
        }
      },
      "required": [
        "source",
        "held",
        "for_region"
      ],
      "type": "object"
    },
    "RegionSources": {
      "description": "What `sources --region` and `sources --box` write.",
      "properties": {
        "boxes": {
          "description": "Its boxes, and the bounds of its Geofabrik areas in the cached index.",
          "items": {
            "$ref": "#/$defs/Bbox"
          },
          "type": "array"
        },
        "countries": {
          "description": "The countries that select the national models; empty selects each that meets a box.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "download_bytes": {
          "description": "The sum of the known bytes to download.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "layers": {
          "description": "The layers of the products.",
          "items": {
            "$ref": "#/$defs/RegionLayer"
          },
          "type": "array"
        },
        "name": {
          "type": "string"
        },
        "prepare": {
          "description": "The prepared sources to prepare.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "products": {
          "description": "The products with each optional part on, and whether their step lists are complete.",
          "items": {
            "$ref": "#/$defs/RegionProduct"
          },
          "type": "array"
        },
        "region": {
          "type": "string"
        },
        "sources": {
          "description": "The sources that the step lists read, and the sources that only a prepare reads of the\nprepared sources among them.",
          "items": {
            "$ref": "#/$defs/RegionSource"
          },
          "type": "array"
        },
        "unknown": {
          "description": "The sources with a download of unknown size, or whose need is unknown.",
          "items": {
            "type": "string"
          },
          "type": "array"
        }
      },
      "required": [
        "region",
        "name",
        "boxes",
        "countries",
        "products",
        "layers",
        "sources",
        "download_bytes",
        "unknown",
        "prepare"
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
          "description": "The steps that this machine builds. A step whose output is in the store already is not here.",
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
          "description": "What this machine downloads from the target: files of kept steps that a step that bakes reads."
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
        "notes": {
          "description": "What layers bake without, such as a prepared source that no target lists yet.",
          "items": {
            "$ref": "#/$defs/Note"
          },
          "type": "array"
        },
        "optional": {
          "description": "The parts that `--add` and `--remove` name, sorted; `settings.layers` holds those\nthat are on.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "reused": {
          "description": "The steps whose output the store has from an earlier build: the update uses it and does not\nbuild it. Each has `built`.",
          "items": {
            "$ref": "#/$defs/PlanBuild"
          },
          "type": "array"
        },
        "services": {
          "description": "For `prod`: why its follower or its tiles Worker cannot serve the release, each with its\nfix. The update waits at router-ready for a follower, and refuses a tiles Worker that cannot\nread the grid of the release.",
          "items": {
            "type": "string"
          },
          "type": "array"
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
        "reused",
        "downloads",
        "cost",
        "machine",
        "after",
        "args",
        "optional",
        "notes",
        "services"
      ],
      "type": "object"
    },
    "ReviewCost": {
      "properties": {
        "peak_rss_bytes": {
          "description": "The highest estimated peak of one bake.",
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
          "description": "What goes to the target: the files of the bakes and of the reused steps that a client reads.",
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
    "Router": {
      "description": "What `router` writes.",
      "properties": {
        "next": {
          "anyOf": [
            {
              "$ref": "#/$defs/Role"
            },
            {
              "type": "null"
            }
          ],
          "description": "The release that `planner/next.json` names while a run waits for it; `null` without one."
        },
        "rollback": {
          "anyOf": [
            {
              "$ref": "#/$defs/Answer"
            },
            {
              "type": "null"
            }
          ],
          "description": "`null` without a rollback."
        },
        "serving": {
          "anyOf": [
            {
              "$ref": "#/$defs/Answer"
            },
            {
              "type": "null"
            }
          ],
          "description": "`null` while the target serves nothing."
        },
        "target": {
          "type": "string"
        }
      },
      "required": [
        "target",
        "serving",
        "rollback",
        "next"
      ],
      "type": "object"
    },
    "Rule": {
      "oneOf": [
        {
          "const": "keep",
          "description": "`keep = \"archive\"`.",
          "type": "string"
        },
        {
          "const": "prepare",
          "description": "`keep = \"prepared\"`.",
          "type": "string"
        },
        {
          "const": "store",
          "description": "`keep = \"store\"`.",
          "type": "string"
        },
        {
          "const": "live",
          "description": "`keep = \"store\"` with a `refresh` in days: the OSM sources.",
          "type": "string"
        }
      ]
    },
    "RunError": {
      "properties": {
        "code": {
          "description": "The code of the [error](#errors).",
          "type": "string"
        },
        "fix": {
          "type": "string"
        },
        "kept": {
          "default": [],
          "description": "The steps whose layers the run built: the store keeps them, and the next update reuses\nthem.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "message": {
          "type": "string"
        }
      },
      "required": [
        "code",
        "message",
        "fix",
        "kept"
      ],
      "type": "object"
    },
    "RunFetch": {
      "properties": {
        "bytes": {
          "default": null,
          "description": "`None` while it runs, and when it failed.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "end_ms": {
          "default": null,
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "error": {
          "default": null,
          "type": [
            "string",
            "null"
          ]
        },
        "params": {
          "default": [],
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
          "default": null,
          "type": [
            "string",
            "null"
          ]
        },
        "source": {
          "default": "",
          "type": "string"
        },
        "start_ms": {
          "default": null,
          "description": "When it started and when it ended, in milliseconds after the start of the run; `null` before.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "version": {
          "default": "",
          "type": "string"
        },
        "wall_ms": {
          "default": null,
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
        "start_ms",
        "end_ms",
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
      "properties": {
        "bytes_out": {
          "default": null,
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "cpu_ms": {
          "default": null,
          "description": "User plus system CPU time; `null` as `wall_ms` is, and when the system does not report it.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "end_ms": {
          "default": null,
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "error": {
          "default": null,
          "type": [
            "string",
            "null"
          ]
        },
        "last_wall_ms": {
          "default": null,
          "description": "Its wall time in the newest earlier run that built it. `runs RUN` gives it.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "layer": {
          "default": "",
          "description": "Empty until it finishes.",
          "type": "string"
        },
        "peak_rss_bytes": {
          "default": null,
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "reads": {
          "default": [],
          "description": "The inputs that it read.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "reused": {
          "default": false,
          "type": "boolean"
        },
        "start_ms": {
          "default": null,
          "description": "When it started and when it finished or failed, in milliseconds after the start of the run;\n`null` before. An unchanged step does not start.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "step": {
          "default": "",
          "type": "string"
        },
        "unchanged": {
          "default": false,
          "description": "It kept its output of the serving release: its key is the key of that output.",
          "type": "boolean"
        },
        "users": {
          "default": [],
          "description": "The steps of this run that read it. `runs RUN` gives it; the status file has none.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "wall_ms": {
          "default": null,
          "description": "`null` while it runs, when it failed, and when it is unchanged.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        }
      },
      "required": [
        "step",
        "layer",
        "reused",
        "unchanged",
        "start_ms",
        "end_ms",
        "wall_ms",
        "cpu_ms",
        "peak_rss_bytes",
        "bytes_out",
        "reads",
        "error",
        "users",
        "last_wall_ms"
      ],
      "type": "object"
    },
    "Running": {
      "description": "What a deployment says that it runs.",
      "properties": {
        "checks": {
          "description": "What a deploy from this machine needs, each checked now.",
          "items": {
            "$ref": "#/$defs/Check"
          },
          "type": "array"
        },
        "commit": {
          "description": "The commit that it runs; `null` when it does not answer, or when its build named none.",
          "type": [
            "string",
            "null"
          ]
        },
        "credential": {
          "$ref": "#/$defs/CredentialState",
          "description": "The credential that a deploy needs, which the page sets."
        },
        "reason": {
          "description": "Why it cannot serve a release of `obc data`; `null` when it answers.",
          "type": [
            "string",
            "null"
          ]
        },
        "url": {
          "description": "The URL that answers with its commit.",
          "type": "string"
        }
      },
      "required": [
        "url",
        "commit",
        "reason",
        "checks",
        "credential"
      ],
      "type": "object"
    },
    "Scope": {
      "additionalProperties": false,
      "description": "The part of the world that a version covers, as the settings of a target name a region.",
      "properties": {
        "definitions": {
          "additionalProperties": {
            "type": "string"
          },
          "description": "The TOML text of the region and of the members of a union, by id.",
          "type": "object"
        },
        "region": {
          "type": "string"
        }
      },
      "required": [
        "region",
        "definitions"
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
          "description": "The parts that are on, sorted.",
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
    "SourceLayers": {
      "description": "What `sources --layer` writes.",
      "properties": {
        "layers": {
          "items": {
            "$ref": "#/$defs/LayerSources"
          },
          "type": "array"
        }
      },
      "required": [
        "layers"
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
        "group": {
          "description": "The heading under which the operator page lists a source that is not prepared.",
          "type": [
            "string",
            "null"
          ]
        },
        "held": {
          "$ref": "#/$defs/Held",
          "description": "How this machine holds the copy that a bake reads."
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
        "keep": {
          "$ref": "#/$defs/Keep"
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
        "prepared": {
          "anyOf": [
            {
              "$ref": "#/$defs/PreparedIn"
            },
            {
              "type": "null"
            }
          ],
          "description": "For a prepared source: its home in the archive, its complete versions there, and the run\nthat holds its lock. `null` for another source."
        },
        "publish_text": {
          "description": "A prepared source of article text publishes the text in its versions, unless this is\n`false`: then the text stays on the machine that prepares.",
          "type": [
            "boolean",
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
        "refreshable": {
          "description": "Whether `source refresh` moves the source.",
          "type": "boolean"
        },
        "requests": {
          "description": "For a source fetched with params: the newest record in the local store of each set of\nparams, sorted by params.",
          "items": {
            "$ref": "#/$defs/StoredRequest"
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
        "use": {
          "description": "What `used_by` reads the source for, in one line.",
          "type": [
            "string",
            "null"
          ]
        },
        "used_by": {
          "description": "What reads the source: the products `device` and `planner`, or the `search` and `trailview`\nlayers of `planner`. `tools/check_data_sources.py` checks the names.",
          "items": {
            "type": "string"
          },
          "type": "array"
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
        "keep",
        "redistribute",
        "targets",
        "snapshots",
        "requests",
        "credential_missing",
        "refreshable",
        "prepared",
        "held"
      ],
      "type": "object"
    },
    "SourceStatus": {
      "description": "How this machine holds a source.",
      "oneOf": [
        {
          "const": "archived",
          "description": "The dataset of the source is complete in the archive.",
          "type": "string"
        },
        {
          "const": "prepared",
          "description": "A prepared version holds it, or R2 keeps our own asset.",
          "type": "string"
        },
        {
          "const": "live",
          "description": "A bake reads it upstream when it is due.",
          "type": "string"
        },
        {
          "const": "held",
          "description": "The store has a version of a source that a fetch takes whole.",
          "type": "string"
        },
        {
          "const": "partial",
          "description": "The archive holds some files of its dataset, or the store some requests of it.",
          "type": "string"
        },
        {
          "const": "missing",
          "description": "Nothing is held.",
          "type": "string"
        }
      ]
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
    "SourceVersion": {
      "description": "A source version that a release read.",
      "properties": {
        "retrieved": {
          "description": "When a machine first retrieved a file of it, `YYYY-MM-DDTHH:MM:SSZ`; `null` without a file.",
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
        "version",
        "retrieved"
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
    "StaleReaders": {
      "properties": {
        "error": {
          "description": "Why the target cannot be read; `null` when it can.",
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
        "target": {
          "type": "string"
        }
      },
      "required": [
        "target",
        "layers",
        "error"
      ],
      "type": "object"
    },
    "Started": {
      "description": "What `obc data update --json` writes: the run that it started.",
      "properties": {
        "review": {
          "description": "The id of the review that the run does; `null` for `--plan`, whose run makes the review.",
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
        "run",
        "review"
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
    "State2": {
      "oneOf": [
        {
          "enum": [
            "running",
            "ok"
          ],
          "type": "string"
        },
        {
          "const": "starting",
          "description": "Reserved: no process took it yet.",
          "type": "string"
        },
        {
          "const": "failed",
          "description": "It failed, or its process ended before it finished.",
          "type": "string"
        },
        {
          "const": "stopped",
          "description": "A signal or `obc data runs RUN --stop` stopped it.",
          "type": "string"
        }
      ]
    },
    "Status": {
      "description": "`runs/<id>/status.json`: a run as far as it went. The process of the run writes it. A reader\ntakes fields that it does not know, and gives a missing one its default, so a status of another\nbuild of `obc data` reads.",
      "properties": {
        "args": {
          "default": [],
          "description": "The arguments after `obc data` that started the run.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "bytes_built": {
          "default": 0,
          "description": "The size of the layers that it built; a reused layer is not counted.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "bytes_fetched": {
          "default": 0,
          "description": "The size of the files of its fetches.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "error": {
          "anyOf": [
            {
              "$ref": "#/$defs/RunError"
            },
            {
              "type": "null"
            }
          ],
          "default": null,
          "description": "`null` unless it failed or stopped."
        },
        "fetches": {
          "default": [],
          "items": {
            "$ref": "#/$defs/RunFetch"
          },
          "type": "array"
        },
        "id": {
          "default": "",
          "type": "string"
        },
        "layers": {
          "default": [],
          "description": "Each layer whose steps it bakes, in the order of the plan.",
          "items": {
            "$ref": "#/$defs/LayerProgress"
          },
          "type": "array"
        },
        "phases": {
          "default": [],
          "description": "Each phase that it entered, in order.",
          "items": {
            "$ref": "#/$defs/PhaseState"
          },
          "type": "array"
        },
        "pid": {
          "default": 0,
          "description": "The process of the run: while it starts, the process that reserved it, and then the\nprocess that it starts.",
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "started": {
          "default": "",
          "description": "`YYYY-MM-DDTHH:MM:SSZ`",
          "type": "string"
        },
        "state": {
          "$ref": "#/$defs/State2",
          "default": "starting"
        },
        "steps": {
          "default": [],
          "description": "In the order they started.",
          "items": {
            "$ref": "#/$defs/RunStep"
          },
          "type": "array"
        },
        "switched": {
          "default": [],
          "description": "The releases that the run switched the target to.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "wall_ms": {
          "default": null,
          "description": "`null` until it ends.",
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
        "args",
        "pid",
        "state",
        "started",
        "wall_ms",
        "phases",
        "layers",
        "fetches",
        "steps",
        "switched",
        "bytes_fetched",
        "bytes_built",
        "error"
      ],
      "type": "object"
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
    "StoredRequest": {
      "description": "A fetch of a source with params, as the local store records it.",
      "properties": {
        "bytes": {
          "description": "The size of its files.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "fetched": {
          "description": "When its newest file was retrieved, `YYYY-MM-DDTHH:MM:SSZ`; `null` for a fetch without\nfiles.",
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
        "version": {
          "type": "string"
        }
      },
      "required": [
        "params",
        "version",
        "bytes",
        "fetched"
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
      "description": "A run, as `obc data runs` lists it.",
      "properties": {
        "args": {
          "description": "The arguments after `obc data` that started the run.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "bytes_built": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "bytes_fetched": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "id": {
          "type": "string"
        },
        "started": {
          "description": "`YYYY-MM-DDTHH:MM:SSZ`",
          "type": "string"
        },
        "state": {
          "$ref": "#/$defs/State2"
        },
        "wall_ms": {
          "description": "`null` until it ends.",
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
        "args",
        "started",
        "state",
        "wall_ms",
        "bytes_fetched",
        "bytes_built"
      ],
      "type": "object"
    },
    "TargetState": {
      "properties": {
        "area": {
          "anyOf": [
            {
              "$ref": "#/$defs/AreaBoxes"
            },
            {
              "type": "null"
            }
          ],
          "description": "The area of the serving release, as boxes that a map draws; `null` while it serves nothing."
        },
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
              "$ref": "#/$defs/Lock"
            },
            {
              "type": "null"
            }
          ],
          "description": "The run that holds the lock of the target; `null` while it is free."
        },
        "lock_error": {
          "description": "Why `lock.json` of the target cannot be read; `null` when it can.",
          "type": [
            "string",
            "null"
          ]
        },
        "made": {
          "anyOf": [
            {
              "$ref": "#/$defs/Made"
            },
            {
              "type": "null"
            }
          ],
          "description": "The machine that made the serving release, and when; `null` while it serves nothing."
        },
        "next": {
          "description": "The command to run next, or the run to wait for while it holds the lock; `null` when the\ntarget serves its settings as the steps make them now. For an R2 target, the command is the\nreview: its update needs the id that the owner approved.",
          "type": [
            "string",
            "null"
          ]
        },
        "notes": {
          "description": "What layers bake without, such as a prepared source that no target lists yet.",
          "items": {
            "$ref": "#/$defs/Note"
          },
          "type": "array"
        },
        "parts": {
          "description": "The parts that the serving release has on, as `--add` names them.",
          "items": {
            "type": "string"
          },
          "type": "array"
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
        "serve": {
          "anyOf": [
            {
              "$ref": "#/$defs/Listening"
            },
            {
              "type": "null"
            }
          ],
          "description": "Where `obc data serve` serves a dir target, and how each app opens it; `null` for an R2\ntarget."
        },
        "sources": {
          "description": "Each source version that the serving release read.",
          "items": {
            "$ref": "#/$defs/SourceVersion"
          },
          "type": "array"
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
        "made",
        "serve",
        "lock",
        "lock_error",
        "region",
        "parts",
        "area",
        "sources",
        "layers",
        "blocked",
        "notes",
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
    "Trusted": {
      "description": "What `deploy trust` wrote.",
      "properties": {
        "file": {
          "description": "The file that holds the host key now.",
          "type": "string"
        },
        "fingerprint": {
          "type": "string"
        }
      },
      "required": [
        "fingerprint",
        "file"
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
        "guess": {
          "description": "The bakes whose estimate is a guess from a build with other options.",
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
        "guess",
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
        "source": {
          "description": "The prepared source whose lock it freed; `null` for the lock of a target.",
          "type": [
            "string",
            "null"
          ]
        },
        "target": {
          "description": "The target whose lock it freed; `null` for the lock of a prepared source.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "target",
        "source",
        "holder"
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
    "Version": {
      "additionalProperties": false,
      "description": "An entry of `versions.json`, which lists the complete versions oldest first.",
      "properties": {
        "bytes": {
          "description": "The size of its files.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "format": {
          "description": "The format of the manifest: a target reads only a version of the format of its producer.",
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "manifest_sha256": {
          "type": "string"
        },
        "region": {
          "description": "The region of its scope; absent for a version of every place that the source covers.",
          "type": [
            "string",
            "null"
          ]
        },
        "time": {
          "description": "When it was complete, `YYYY-MM-DDTHH:MM:SSZ`.",
          "type": "string"
        },
        "version": {
          "type": "string"
        }
      },
      "required": [
        "version",
        "manifest_sha256",
        "format",
        "bytes",
        "time"
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
    },
    "View": {
      "description": "A run as `obc data runs RUN` gives it: its status, with the users and the earlier times of its\nsteps, and the end of its log.",
      "properties": {
        "args": {
          "default": [],
          "description": "The arguments after `obc data` that started the run.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "bytes_built": {
          "default": 0,
          "description": "The size of the layers that it built; a reused layer is not counted.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "bytes_fetched": {
          "default": 0,
          "description": "The size of the files of its fetches.",
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "error": {
          "anyOf": [
            {
              "$ref": "#/$defs/RunError"
            },
            {
              "type": "null"
            }
          ],
          "default": null,
          "description": "`null` unless it failed or stopped."
        },
        "fetches": {
          "default": [],
          "items": {
            "$ref": "#/$defs/RunFetch"
          },
          "type": "array"
        },
        "id": {
          "default": "",
          "type": "string"
        },
        "layers": {
          "default": [],
          "description": "Each layer whose steps it bakes, in the order of the plan.",
          "items": {
            "$ref": "#/$defs/LayerProgress"
          },
          "type": "array"
        },
        "log": {
          "description": "The last lines of `runs/<id>/log`.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "log_path": {
          "description": "The path of that log on the machine of the run.",
          "type": "string"
        },
        "phases": {
          "default": [],
          "description": "Each phase that it entered, in order.",
          "items": {
            "$ref": "#/$defs/PhaseState"
          },
          "type": "array"
        },
        "pid": {
          "default": 0,
          "description": "The process of the run: while it starts, the process that reserved it, and then the\nprocess that it starts.",
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "review": {
          "description": "The review that a run of `update --plan` wrote, as `update --dry-run` writes it; `null`\nfor another run, and until the plan is done."
        },
        "started": {
          "default": "",
          "description": "`YYYY-MM-DDTHH:MM:SSZ`",
          "type": "string"
        },
        "state": {
          "$ref": "#/$defs/State2",
          "default": "starting"
        },
        "steps": {
          "default": [],
          "description": "In the order they started.",
          "items": {
            "$ref": "#/$defs/RunStep"
          },
          "type": "array"
        },
        "switched": {
          "default": [],
          "description": "The releases that the run switched the target to.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "wall_ms": {
          "default": null,
          "description": "`null` until it ends.",
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
        "args",
        "pid",
        "state",
        "started",
        "wall_ms",
        "phases",
        "layers",
        "fetches",
        "steps",
        "switched",
        "bytes_fetched",
        "bytes_built",
        "error",
        "log",
        "log_path",
        "review"
      ],
      "type": "object"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema"
}
```
