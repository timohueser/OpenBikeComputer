# obc data registry

The files under `data/` that people write for `obc data`: sources, regions, planner options,
machines and route descriptions. Also the settings of a target, and the state of each source that
`obc data sources` gives.

- Writers: people (`data/*.toml`).
- Readers: `host/obc-data`, `tools/data_registry.py`, `tools/check_data_sources.py`, `obc-bake`
  (`data/regions/`) and the steps of `host/obc-data-steps`.
- Code authority: `host/obc-data` (`sources.rs`, `regions.rs`, `settings.rs`, `machines.rs`).
- Related: [store](obc-data-store.md), [archive](obc-data-archive.md),
  [products](obc-data-products.md), [targets](obc-data-targets.md),
  [prepared sources](obc-data-prepared.md), [commands](obc-data-cli.md).

## Conventions

- All files under `data/` are TOML. A file with an unknown key is refused.

## Files

[`data/archive/<dataset>.toml`](obc-data-archive.md#dataarchivedatasettoml) and
[`data/targets.toml`](obc-data-targets.md#datatargetstoml) are in the specs of their subjects.

### `data/sources.toml`

One `[[source]]` table per source.

| Key | Type | Required | Meaning |
| --- | --- | --- | --- |
| `id` | string | yes | Unique, lowercase kebab-case |
| `kind` | string | yes | `data` (steps read it), `asset` (ships to users as it is) or `tool` (steps run it; nothing of it ships) |
| `group` | string | no | The heading under which the operator page lists a source that is not prepared |
| `used_by` | array of strings | yes | What reads the source, one or more of the [products](obc-data-products.md#products) `device` and `planner`, and of `search` and `trailview`, the search and TrailView layers of `planner`. `tools/check_data_sources.py` refuses a source without one, or with another name |
| `use` | string | yes | What `used_by` reads the source for, in one line. `tools/check_data_sources.py` refuses a source without it |
| `licence` | string | no | SPDX licence expression: ids or `LicenseRef-…`, joined by `AND`, `OR`, `WITH` and parentheses |
| `licence_url` | string | no | Where the licence text is |
| `attribution` | string | no | The credit text, as the product must show it |
| `obligations` | string | no | What the licence asks for, in words; `none` when it asks for nothing |
| `fetch` | table | yes | `kind`, `url` and `at_once`. See below |
| `hosts` | array of strings | no | Hosts the fetch reaches besides the host of `fetch.url`: lowercase letters, digits, `.` and `-`. `*.domain` is any subdomain |
| `version` | string | yes | How upstream names a version: `date`, `release`, `commit` or `digest` |
| `start` | string | no | The version that a step list reads while no serving release reads the source. It has the form of `version` |
| `refresh` | integer or string | no, `"manual"` | The policy, which the settings of a target can replace: `1` through `65535` whole days, or `"manual"` |
| `keep` | string | yes | Where the copy that a bake reads stays: `archive`, the [archive](obc-data-archive.md#archive) holds the whole dataset; `store`, the store keeps what each fetch took; `prepared`, we keep what we make: the [archive](obc-data-prepared.md#prepared-sources) keeps a prepared source and a source that only a prepare reads, and R2 keeps our own asset. `tools/check_data_sources.py` refuses a source without it |
| `redistribute` | boolean | yes | The licence lets us give the upstream bytes to others |
| `credential` | table | no | `env`: the environment variables a fetch needs; or `file`: the file that holds them. `~/` is the home directory. `register`: the `https://` page where a person gets it, and `hint`: what to copy there, in one line. See [Credentials of sources](obc-data-store.md#credentials-of-sources) |
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
| `prepared` | `obc data source prepare` makes its versions in the archive, see [Prepared sources](obc-data-prepared.md#prepared-sources). It has no `url` |

`fetch.url` is an `https://` URL template. `{name}` stands for a value the fetcher fills:
`{version}` is the version, `{yymmdd}` a date version as `YYMMDD`, `{area}` a Geofabrik area, `{tile}` a
tile name. `{name:last}` is the last `/` segment of the value of `{name}`. In `attribution`,
`{year}` and `{month}` are the year and month of the data, which the step that writes the
credit fills.

Rules:

- An id is listed once.
- Each kind of fetch but `installed` and `prepared` has a `url`, and the `url` starts with `https://`.
- `fetch.at_once` is only on a `dtm` fetch, and is an integer of 1 or more. The `terrain` prepare runs
  at most that many block programs that read the source at the same time.
- A `prepared` source has `version = "date"`, `redistribute = true` and `keep = "prepared"`.
- Only the OSM sources and the prepared sources have a `refresh` in days. A prepared source
  changes when a person prepares a version.
- `refresh` in days needs `version = "date"`, because only a date version has an age.
- A credential has `env` or `file`, not both. A `file` is `~/.cdsapirc`,
  `~/.config/openbikecomputer/cdse-s3.env` or `~/.config/openbikecomputer/clms-key.json`.

No step reads a `dtm-*` source, a national terrain model: only the producer of the prepared
source `terrain` does, see [Prepared sources](obc-data-prepared.md#prepared-sources). It reads each `dtm-*` row with
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
| Planner TrailView layers | The sources that each step reads, see [`planner`](obc-data-products.md#planner) |
| Reference archive manifests | `dtm-<key>` for the national model with that key |

A credit that a rider reads is the `attribution` text. An SPDX `licence` id goes only into a
field for programs, such as the catalog `license`.

Rust code reads the file that the build embeds (`obc_data::sources::attribution`). Python
reads it through `tools/data_registry.py`, and shell through
`tools/data_registry.py attribution ID`. The web bundle notice reads it at build time through
`builder/web/vite/third-party-licenses.ts`. The web app and the iOS planner show the
planner release `attribution`. Text that no step can generate keeps a copy, and a test
compares the copy with this file: the device About page, and the footers of the site and the
docs.

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

The planner reads `countries` and `time_zone` (see [`planner`](obc-data-products.md#planner)).

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

The options of the [planner layers](obc-data-products.md#planner) that are the same for each region.

| Key | Type | Meaning |
| --- | --- | --- |
| `osm.margin_m` | number | `planner/osm/bounds` of a region with a box reaches at least this far around the region, in metres: the search puts a place in the town of a place node up to 15 km away |
| `routing.profiles` | array of strings | The profiles of the routing package. The route catalog needs `touring`, `road`, `gravel`, `mtb/technical` and `hiking/alpine` |
| `climate.first_year` | integer | The first of the ten years of ERA5-Land that the climate layer reads |
| `snow.seasons` | array of 2 integers | The first and the last season of the snow layer, and of a version of [`snow`](obc-data-producers.md#snow) |

### `data/machines.toml`

One table per machine that runs `obc data ui`. The name of the table is the name of the machine:
`OBC_DATA_MACHINE`, or else the short host name. A machine that the file does not name has the
header `#262624`, and `obc data open` tunnels to the SSH host of its name.

| Key | Type | Meaning |
| --- | --- | --- |
| `colour` | string | `#rrggbb`: the header of the page of the machine. White text on it has a contrast of at least 4.5:1, and it is not `#6e6c66`, the header of a page that loads |
| `ssh` | string | Optional: the SSH host or alias that `obc data open` tunnels to |

### `data/routes/<id>.toml`

The description of a route source: which layers and fields of its files make a route and its
stages. `<id>` is a source of `data/sources.toml` with `used_by` `planner` and `keep = "archive"`.
[Route sources](obc-data-products.md#route-sources) reads it. An unknown key is refused, and so are an `origin` that
another file has, empty `countries`, a `keep` or `drop` value that is no list, and a `rank` level
outside 0 to 4 (`tools/check_data_sources.py`).

| Key | Type | Meaning |
| --- | --- | --- |
| `origin` | integer, 1 to 9006 | The number of the source in the `id` of its records. Unique over `data/routes/`; a number is never used again |
| `countries` | array of strings | The ISO 3166-1 alpha-2 codes of the countries whose routes the source holds, such as `CH`. A region reads the source only when its `countries` name one of them and its bounds meet the `extent` of the row |
| `kind` | string, or table | `hiking`, `bicycle` or `mtb` for every route; or `{ field, values }`: the value of `field` maps to a kind, and a feature with another value is left out |
| `route` | table | How the routes are read, see below |
| `stage` | table | Optional: how the stages of a route are read |

Keys of `route` and `stage`:

| Key | In | Meaning |
| --- | --- | --- |
| `layer` | both | The layer of the file. A file of one layer needs none: a GeoJSON without a `name` has the name of its file as its layer |
| `file` | both | Optional: the path of the file in a zip, for a source with more than one |
| `key` | `route` | The field, or a list of fields, that gives the route of a feature. The features with one key are one route; a feature with a value in two fields of the list is a member of both routes. A text with `{`, such as `"{A}: {B}"`, is a template: the filled text, with outer spaces removed, is the key text, and a feature with no value in one of its fields is no route. A value whose text is a decimal integer from 0 to 10^10 − 1 with no leading zero is the key as it is; another text, such as `007` or `-5`, has as key its SHA-256, read as an integer, modulo 10^10 |
| `of` | `stage` | The field that holds the key of the stage's route |
| `number` | `stage` | The field of the stage number |
| `ref`, `name`, `description`, `operator`, `website` | both | A template of field values, such as `"{NameR}"`. `{key}` is the text of the key field that gives the route; in a stage, `{route}` is the name of its route |
| `name_strip` | both | Optional: a regular expression (Python `re`) that removes a prefix or junk from the filled `name`. Each match is removed, each run of spaces becomes one space, and outer spaces are removed. When nothing is left, the name stays as filled |
| `rank` | `route` | `{ field, values }`: the value maps to a level 0 to 4 as the catalog's `rank`; another value or none: 0 |
| `ascent` | both | The field of the source's own climb in metres. The catalog does not read it: it is there to compare the climb of the catalog with that of the source |
| `keep` | both | `{ FIELD = [values] }`: a feature is read only when each field has one of its values |
| `drop` | both | `{ FIELD = [values] }`: a feature with one of these values is left out |

A value is compared as its text. A whole number reads as an integer, also in an integer field
that holds a null, which the file gives as a real number.

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
It reads only the [status of the archive](obc-data-archive.md#status) and its manifests, the versions and the
manifests in the homes of the [prepared sources](obc-data-prepared.md#prepared-sources), the records of the store and
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
that the cached index lacks refuses. The output lists the step lists of the products for the region, as the [overview](obc-data-cli.md#overview) does, but with no fetch: the sources are
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
