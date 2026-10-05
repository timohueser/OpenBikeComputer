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

## State of a source

`obc data sources` computes the state of each source when it runs. It stores nothing.

| State | When |
| --- | --- |
| `blocked` | A `data` or `asset` source has no `licence`, or its credential is not on this machine |
| `stale` | The pin is a date, `refresh` is in days, the pin is older than `refresh`, and the newest upstream version is later than the pin or is not known |
| `ok` | Otherwise. A source with no pin, or with `refresh = "manual"`, is never stale |

The age of a pin is the number of days from its date to today (UTC).

## Store

The store is the directory in `OBC_DATA_STORE`, or else `~/.cache/openbikecomputer/store/`.

| Path | Holds |
| --- | --- |
| `objects/<ab>/<sha256>` | One file, named by the lowercase hex SHA-256 of its bytes; `<ab>` is its first two characters. Read-only |
| `snapshots/<source>/<version>.json` | The snapshot record of one source version |
| `upstream/<source>.json` | The last upstream check of a source: `checked` (seconds since 1970-01-01 UTC) and `version` (a string or `null`) |
| `partial/` | Downloads that are not complete, and the validators that resume them |
| `locks/` | One lock file per key |

Rules:

- An object is complete. A download goes to `partial/` and moves into `objects/` with one
  rename after the digest check.
- A record goes to a temporary file in its directory and replaces the old record with one
  rename.
- One process at a time downloads a URL, and one process at a time writes a snapshot record.

A snapshot record is a JSON object:

| Key | Meaning |
| --- | --- |
| `source` | The source id |
| `version` | The version, as a pin names it |
| `files` | One item per file: `name` (the last segment of the URL), `url`, `size` in bytes, `sha256` and `retrieved` (`YYYY-MM-DDTHH:MM:SSZ`) |

A version is one or more segments joined by `/`. A segment has letters, digits, `.`, `_`, `+`
and `-`, and does not start with `.`.

## Fetch

A fetch fills each `{name}` of `fetch.url`: `{version}` from the version, and each other name
from a `NAME=VALUE` argument. A name can have more than one value; then the fetch gets one file
for each value. A file that the snapshot record of the version has, and whose object exists,
comes from the store with no request.

Which version a fetch gets:

- A URL with `{version}` gives the version that it names. A `release` or `commit` source whose
  URL has no `{version}` needs a version, and the record keeps the version as given.
- A URL without `{version}` gives only the newest file upstream. For a `date` source, the
  version is the latest `Last-Modified` day of the files. A given date version accepts a file
  that changed on or before that day; a later file fails the fetch before its body is read.
  A response without `Last-Modified` counts as changed today.
- For a `digest` source, the version is the SHA-256 of its one file.

A download tries four times, and waits 2, 4 and 8 seconds between the tries. It retries a
connection error, a cut-off body, a refused resume and HTTP 408, 429 and 5xx. It keeps the bytes it has in
`partial/`. The next try, or the next fetch, asks for the rest with `Range` and `If-Range`,
with the `ETag` or else the `Last-Modified` of the first response. A server that sends the
whole file again restarts the file. A file with no validator resumes only when its digest is
known. The digest check compares the SHA-256 with the `digest` version, or with the record of
the version. A file that fails it is deleted.

| `fetch.kind` | Fetcher |
| --- | --- |
| `http`, `geofabrik`, `glo30`, `github` | One file per URL, as above |
| `osm`, `dtm`, `capture` | None yet; the fetch fails |
| `by-hand`, `installed` | None; the fetch fails |

The upstream check finds the newest version of a source with at most one request. Its result
stays valid for one hour.

| Source | Check |
| --- | --- |
| `osm` | `HEAD` of the URL; the day in the `planet-YYMMDD` file name that it redirects to |
| `capture` | Today, with no request: a query service answers with current data |
| `github`, `commit` | The GitHub API: the newest commit of the default branch |
| `github`, `release` | The GitHub API: the tag of the newest release that has the asset of the URL |
| `http`, `geofabrik` or `glo30`, `date`, and a URL without `{name}` | `HEAD` of the URL; the `Last-Modified` day |
| Every other source | None; the newest version is not known |

## Commands

| Command | Output |
| --- | --- |
| `obc data sources [--json]` | Every source with licence, R2 copy, live pin, newest upstream version, age, policy and state. Rows are in kind order: data, then assets, then tools |
| `obc data fetch SOURCE[@VERSION] [NAME=VALUE…] [--json]` | Fetches the version, or else the live pin, or else the newest file upstream. Writes the store path of each file |
| `obc data refresh SOURCE [NAME=VALUE…] [--env ENV] [--json]` | Fetches the newest upstream version, checked now, and writes it to `[pins]` of `data/env/ENV.toml` (default `live`). Writes the store path of each file |
| `obc data region [list] [--json]` | Every region with its name and definition |
| `obc data region show ID [--json]` | One region, the regions it resolves to, and its box when every part is a box |

`--json` writes one JSON document to standard output:

- `sources`: `{"sources": [...]}`. Each item has the keys of its `[[source]]` table, and
  `pin`, `upstream` (the newest upstream version, or `null`), `age_days`, `state` and `reason`
  (`null` when the state is `ok`).
- `fetch` and `refresh`: the snapshot, `{"source": ..., "version": ..., "files": [...]}`, with
  the requested files only. Each file has the keys of the record and `path`, its object.
- `region list`: `{"regions": [...]}`. Each item has `id`, `name`, `kind` and the key its
  kind names. A `box` is an object with `west`, `south`, `east` and `north`.
- `region show`: the region item, and `leaves` (the region ids it resolves to) and
  `bounds` (a box or `null`).

A command that fails writes the reason to standard error. The exit status is 0 when the command
succeeds, 1 when a file under `data/` is not valid or a fetch fails, and 2 for a usage error,
which includes an unknown region id, an unknown source id and a missing environment file.
