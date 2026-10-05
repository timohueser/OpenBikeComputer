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

`attribution` is the one copy of a credit. Every product that carries a credit takes it from
here:

| Product | Credit |
| --- | --- |
| Device-map catalog `source` and `LICENSE.txt` | `attribution`, `licence` and `licence_url` of `osm-planet` |
| Device-map catalog `terrain.attribution` | The source whose id is the terrain `dataset_id` |
| Device-map catalog `landmarks.attribution` | `wikipedia`, then `commons` |
| Planner release `attribution`, routing package, search, route overlays | `osm-planet` |
| Web planner and map builder basemap | The release `attribution`, then `daylight-landcover`, which the web build embeds |
| Planner `terrain_attribution` | `copernicus-glo-30`, after the reference models |
| Planner climate and snow layers | `era5-land`; `modis-snow` and `hansen-gfc`, or `hr-wsi` |
| Reference archive manifests | `dtm-<key>` for the national model with that key |

A credit that a rider reads is the `attribution` text. An SPDX `licence` id goes only into a
field for programs, such as the catalog `license`.

Rust code reads the file that the build embeds (`obc_data::sources::attribution`). Python
reads it through `tools/data_registry.py`, and shell through
`tools/data_registry.py attribution ID`. The web build reads it through
`builder/app/vite/registry.ts`. The web planner, the map builder and the iOS planner show the
planner release `attribution`. Text that no step can generate keeps a copy, and a test
compares the copy with this file: the device About page, and the footers of the site, the
docs and the map builder.

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

The bakes read this directory:

| Reader | Regions |
| --- | --- |
| `obc-bake` (device maps) | Every `geofabrik` region. `--regions DIR` reads another directory with this layout; `obc bake` passes the checkout's directory |
| Planner bake | The `box` region with the id of the recipe in `tools/planner-regions/`: its `name` and its box |
| `fixtures/build-map-package.sh` | The `box` regions of the fixtures |

`tools/data_registry.py box ID [--lat-first]` prints the box of a `box` region and refuses
every other kind, so Python and shell never resolve a union or a Geofabrik area.

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
