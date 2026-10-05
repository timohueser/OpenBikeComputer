# obc data

`obc data` reads the data registry: the external sources that a bake uses, the regions, and
the pins of an environment. The crate is `host/obc-data`. All files are TOML. A file with an
unknown key is refused.

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
| `stale` | The pin is a date, `refresh` is in days, and the pin is older than `refresh` |
| `ok` | Otherwise. A source with no pin, or with `refresh = "manual"`, is never stale |

The age of a pin is the number of days from its date to today (UTC).

## Commands

| Command | Output |
| --- | --- |
| `obc data sources [--json]` | Every source with licence, R2 copy, live pin, age, policy and state. Rows are in kind order: data, then assets, then tools |
| `obc data region [list] [--json]` | Every region with its name and definition |
| `obc data region show ID [--json]` | One region, the regions it resolves to, and its box when every part is a box |

`--json` writes one JSON document to standard output:

- `sources`: `{"sources": [...]}`. Each item has the keys of its `[[source]]` table, and
  `pin`, `age_days`, `state` and `reason` (`null` when the state is `ok`).
- `region list`: `{"regions": [...]}`. Each item has `id`, `name`, `kind` and the key its
  kind names. A `box` is an object with `west`, `south`, `east` and `north`.
- `region show`: the region item, and `leaves` (the region ids it resolves to) and
  `bounds` (a box or `null`).

A command that fails writes the reason to standard error. The exit status is 0 when the command
succeeds, 1 when a file under `data/` is not valid, and 2 for a usage error, which includes an
unknown region id.

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
  with non-ASCII characters as `\uXXXX` escapes. When the delete fails, the error names the
  keys that the bucket still holds.

The exit status is 0 on success, 1 when R2 or rclone fails or the person answers no, and 2 for a
usage error or a delete without consent.
