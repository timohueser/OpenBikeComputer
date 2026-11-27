# obc data archive

The archive of whole datasets on drives of their own: its roots, the definition and the manifest
of a dataset, and the commands that get, verify and read it.

- Writers: people (`data/archive/*.toml`, `kept/`); `host/obc-data` (`obc data archive`).
- Readers: `host/obc-data` (a fetch, the source overview).
- Code authority: `host/obc-data` (`datasets.rs`).
- Related: [store](obc-data-store.md), [registry](obc-data-registry.md),
  [prepared sources](obc-data-prepared.md).

## Archive

The archive keeps whole datasets and the versions of the prepared sources on drives of their
own. `OBC_DATA_ARCHIVE` names its roots, separated by `:`. Without the variable, or with an empty
value, there is no archive. Each root is a complete archive. A dataset is wholly in one root, and
so are the versions of a prepared source. A prepared version has no other copy. Only `obc data archive` writes
`mirror/`, `manifest/` and `.partial/`, and only `obc data source prepare`,
`obc data source copy` and `obc data unlock --source` write `prepared/`.

| Path under a root | Holds |
| --- | --- |
| `mirror/<host>/<path>` | The file of the URL `https://<host>/<path>` |
| `mirror/<host>/<path>@<digest>` | The file of the URL `https://<host>/<path>?<query>`: `<digest>` is the first 16 hex digits of the SHA-256 of the query |
| `kept/<dataset>/<path>` | A file that has no upstream URL. A person puts it there |
| `manifest/<dataset>.json` | The [manifest](#manifest-of-a-dataset) of the dataset |
| `prepared/<id>/` | The versions of the [prepared source](obc-data-prepared.md#prepared-sources) `<id>` |
| `.partial/<dataset>/` | The unfinished downloads of `archive get`. You can delete it when no `archive get` runs |

A file goes into `mirror/` only after its check: the size, the `ETag` and the MD5 that the listing
gives, and a `Last-Modified` day that is not after the one of the listing. The modification time of
a file in `mirror/` is the `Last-Modified` time of upstream.

## `data/archive/<dataset>.toml`

One file for each dataset. The name of the file is the id of the dataset.

| Key | Type | Required | Meaning |
| --- | --- | --- | --- |
| `name` | string | yes | What the dataset holds, in one line |
| `sources` | array of strings | yes | The ids of the sources in `data/sources.toml` that read the dataset |
| `coverage` | string | yes | Where the dataset has data, in words, such as `World` or `Europe` |
| `version` | string | no | The version of the dataset, in the form of the `version` of its sources |
| `urls` | array of strings | no | One file for each `https://` URL. A URL can have a query, such as a request to a web service |
| `s3` | array of tables | no | `bucket`, `prefix` and `match`. See below |
| `stac` | array of tables | no | `search`, `sas`, `collection`, `asset`, `bbox`, `days` and `names`. See below |
| `cmr` | array of tables | no | `provider`, `short_names`, `version`, `origin`, `bbox`, `days` and `names`. See below |
| `kept` | boolean | no | `true`: the files of the dataset are the files under `kept/<dataset>/`, and nothing downloads |
| `clms` | table | no | `dataset` and `file`: the prepackaged CLMS file of which the dataset holds the zip |
| `bytes_estimate` | integer | no | The bytes of the files of the dataset, from a listing of upstream, for the [source overview](obc-data-registry.md#source-overview) while no manifest lists them, and the bytes that a get needs on its root when a listing gives fewer, such as a listing without sizes |
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
`msft:expiry` of its answer. No log line, manifest or error holds a query of a URL: they name a URL
with a query as its mirror path does, by its path, `@` and the digest of the query. A download under
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

## Manifest of a dataset

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

## Get

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
  `LastModified` of each object, each page with the retries of [Fetch](obc-data-store.md#fetch). A page that fails
  fails the get, and the manifest stays as it was. A key that names no plain path is left out with
  a warning. For a kept dataset, each file under `kept/<dataset>/`; a folder there that cannot be
  read fails the get. The listing is the list of the files of the dataset: a file that upstream
  removes leaves the manifest.
- A file of the manifest that is in place with its `bytes` stays without a download when upstream
  gives its size, its `ETag` and its `Last-Modified` day, as far as upstream gives them. Only a
  bucket listing gives the `ETag`: the `ETag` of a `HEAD` is not stable.
- A URL whose `HEAD` gives no `ETag` and no day, such as the answer that a web service makes for
  each request, is bare: its size checks nothing, and the get downloads it each time. The same
  bytes keep the entry and its day. Other bytes replace the file in place and have the day of the
  get as `modified`. Other bytes on the day of the entry fail the file and leave the file in place:
  one day names one set of bytes.
- A file on disk with the `Content-Length` and the `Last-Modified` day of upstream as its size and
  modification day is read in place once.
- Each other file downloads into `.partial/<dataset>/` with the retries and the resume of
  [Fetch](obc-data-store.md#fetch), four files at a time, and goes into `mirror/` after its check. An `ETag` of 32
  hex digits in a bucket listing, which S3 and GCS give for a file of one part, is its MD5.
- A get never gives a kept file other bytes than the `sha256` that the manifest records in
  `files` or `missing`. A new kept file is read in place for its SHA-256. To
  replace a kept file, a person puts the new file in place and removes its entry from the
  manifest.
- A file that the get cannot take keeps its entry of the manifest before while that file is in
  place; else it is in `missing`. The get fails when a file failed.
- The get writes the manifest each minute and at the end. The next get resumes. Only one get or verify of a dataset runs at a time.

## Verify

`obc data archive verify DATASET` reads each file of the manifest and compares its size and
SHA-256 with the manifest. A file that differs, or is not in place, moves from `files` to
`missing`, and then does not serve. A mirrored file that differs moves to
`.partial/<dataset>/rejected/`, and the next `archive get` downloads it again. A kept file stays in
place. The verify fails when a file differs or is not in place.

## Status

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

## Read

- Only a source with `keep = "archive"` reads a dataset. A prepared source reads its versions as
  [Prepared sources](obc-data-prepared.md#prepared-sources) says.
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
the archive, see [Fetch](obc-data-store.md#fetch).
