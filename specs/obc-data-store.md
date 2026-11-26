# obc data store

The store of a machine, its collection, the fetchers that fill it and the credentials of the
sources.

- Version: store `layout` 3. `obc data` refuses another layout, see [Store](#store).
- Writers: `host/obc-data`; the capture programs of `tools/` (the files of a capture).
- Readers: `host/obc-data`.
- Code authority: `host/obc-data` (`store.rs`, `fetch.rs`, `credentials.rs`).
- Related: [registry](obc-data-registry.md), [archive](obc-data-archive.md),
  [layers](obc-data-layers.md), [prepared sources](obc-data-prepared.md).

## Store

The store is the directory in `OBC_DATA_STORE`, or else `~/.cache/openbikecomputer/store/`.

| Path | Holds |
| --- | --- |
| `layout` | The layout number of the store, `3`, and a line end |
| `objects/<ab>/<sha256>` | One file, named by the lowercase hex SHA-256 of its bytes; `<ab>` is its first two characters. Read-only |
| `snapshots/<source>/<version>.json` | The snapshot record of one source version |
| `layers/<key>.json` | The receipt of the layer with that key, see [Layers](obc-data-layers.md#layers) |
| `requests/<source>/<sha256>.json` | The files that a fetch with `NAME=VALUE` gave: `version`, `params` and `files` (names). The name is the SHA-256 of the compact JSON `[version, params]`, with `params` sorted. A record with no files selects no file |
| `runs/<id>/` | `status.json`, `log` and `stop` of one run, see [Runs](obc-data-layers.md#runs) |
| `refreshed/<source>.json` | `{source, from, to}`: the version that `obc data source refresh` took last (`to`), and the versions that the targets served then (`from`) |
| `upstream/<source>/<sha256>.json` | The acquisition check descriptors and observation of one normalized request. The name is the SHA-256 of compact JSON sorted params. The observation has `checked_at` (UTC seconds or `null`), `result` and `last_success` (UTC seconds and version, or `null`). The result has `state`: `newest` with `value`, `failed` with `value`, `capture`, or `cannot_check` |
| `partial/` | Downloads that are not complete, the validators that resume them, and the layers that steps write. A collection empties it |
| `serve/<target>/<planner id>/` | The files that the services of a planner release read while [`obc data serve`](obc-data-serve.md#serve) runs them |
| `follower/<target>/` | A store of its own with the objects of the releases that [`obc data serve`](obc-data-serve.md#serve) follows, and nothing else |
| `unpacked/<sha256>/` | The one GeoPackage or FileGDB of the archived zip with that SHA-256, as an [`archive-features`](#fetch) capture unpacks it. `.complete` marks a whole copy. A bake from the store never reads it |
| `prepared/<id>/<version>/` | The work of a [prepare](obc-data-prepared.md#prepare) that is not complete: `selection.json`, `[scope, languages]` of the run; `journal.jsonl`, the files that it moved into the archive and the source versions that it read; and `work/`, the files that it did not move yet |
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
| `files` | One item per file: `name`, `url`, `size` in bytes, `sha256`, `retrieved` (`YYYY-MM-DDTHH:MM:SSZ`), and `archive`, the path in the [archive](obc-data-archive.md#archive) of the file that gave the bytes, when the archive served them |

The `name` of a file is the part of its URL that identifies it in the source. For a `dtm`
or `capture` fetch, it is the URL after `fetch.url`, such as `#bbox=W,S,E,N/sub/a.tif`. For a URL template, it is the URL from the segment of the first
`{name}` other than `{version}` and `{yymmdd}`, such as `europe/monaco-261003.osm.pbf`; without
such a `{name}`, it is the last segment. A name is unique in a record: a file whose name the
record has for another URL fails the fetch.

A version is one or more segments joined by `/`. A segment has letters, digits, `.`, `_`, `+`
and `-`, and does not start with `.`. A `date` version is also a `YYYY-MM-DD` date, and a
`digest` version is 64 lowercase hex digits. The version of a `prepared` source that a step reads
is `<version>/<manifest sha256>`, see [Prepared sources](obc-data-prepared.md#prepared-sources).

### Clean

`obc data clean` shows one plan. With `--apply`, it asks once, as [Errors](obc-data-cli.md#errors) says, and
then collects. The collection deletes only the plan that it showed: when the plan of now differs,
also in the size of `partial/`, it deletes nothing. A receipt that the command cannot read, such
as one that a newer `obc data` wrote, stops the collection. For reuse, such a receipt is absent and
its step builds again.

The collection deletes what no release of a target or fixture reaches. Its roots are the serving
release and the rollback of each target (see [Targets](obc-data-targets.md#targets)), the files of the checkout that
it runs in, and the store:

- A snapshot record is reached when an output of a root release read its source and version. The
  newest record of each source, by the latest `retrieved` of its files, is also reached: a plan
  step list reads it when nothing else names a version (see [Versions](obc-data-layers.md#versions)). So is the
  newest version of each request record (`requests/`), by the latest `retrieved` of its files, such as the extract of each Geofabrik area, and each version
  that `obc data source refresh` took (`refreshed/`).
- An object is reached when a reached snapshot record or a reached layer has it, or when its
  SHA-256 is a file of an output of a root release (for an output of a `cache` layer, only a file
  that a client reads), or is in `fixtures/catalog.toml` or a JSON or
  TOML file below `fixtures/sources/`.
- An unpacked copy (`unpacked/<sha256>/`) is reached while a manifest of the
  [archive](obc-data-archive.md#archive) lists a file with that SHA-256. While `OBC_DATA_ARCHIVE` names no root, a
  root or its `manifest/` does not exist, `manifest/` holds no manifest, or `manifest/` or a
  manifest in it cannot be read, each copy is reached, and `obc data` warns and names the root.
- A layer is reached when each of its inputs is reached: a snapshot input whose digest is the
  digest of the files that it names (`files`) of a reached record of its source, and a step input
  whose digest is the digest of the files that it selects of a reached layer of its step: the
  `files` of the input, or all files when it names none.
- Of the receipts with `cache` of one step and the same options, only the one with the latest
  `used`, else `built` (then by key), can be reached, and it is reached when its inputs are. So a
  new OSM cut or a new version of a work layer deletes the older work layers of that region at the
  next collection, also those that a release read, and the work layers of other regions stay. An
  update that ends without an error marks the work layers that it built or read, so an update back
  to an older set keeps that set. After a failed or stopped update, a collection can delete a set
  that the update read, and the next update makes it again. A collection between a failed
  bake and the next bake can delete the last complete set of work layers while the new set is not
  complete. A region that no target lists any more keeps its work layers until its extract
  request goes, as its `osm` layers do. When the work layers of a target are gone, a later update
  of that target bakes them again, also when its package is still in the store.

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
  today. A `HEAD` that the server refuses with 405 is an answer without a size, an `ETag` and a
  day. A file of a `keep = "archive"` source whose `HEAD` gives no `ETag` and no day comes only
  from the [archive](obc-data-archive.md#archive), which dates its bytes: without the archive file, and with
  `--refresh`, the fetch refuses and names `obc data archive get` with the dataset that lists
  the source.
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
| `prepared` | The manifest and the files that a step names, from the archive, see [Prepared sources](obc-data-prepared.md#prepared-sources) |

A `geofabrik` URL with `{yymmdd}` names the day of the data of the extract. Without a version, the
fetch reads the day from the `timestamp` of `updates/state.txt` in the folder of the extract; for
more than one area, it takes the earliest day.

`osm-planet` is the weekly planet file of one day, an `http` URL with `{yymmdd}`. No step reads
it: the [`wiki`](obc-data-producers.md#wiki) preparation reads a planet file of it, and it gives the OSM credit.

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
producer of [`wiki`](obc-data-producers.md#wiki) reads them. `era5-land` has none: only the producer of
[`climate`](obc-data-producers.md#climate) reads it. `sentinel-2-mosaics` has none: only the producer of
[`colour`](obc-data-producers.md#colour) reads it. `modis-snow` and `hr-wsi` have none: only the producer of
[`snow`](obc-data-producers.md#snow) reads them.

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
- `archive-features` reads the zip of the `fetch.url` of the source in the [archive](obc-data-archive.md#archive),
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
  the error names the variables or the file. A capture that the [archive](obc-data-archive.md#archive) serves needs
  no credential.

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
