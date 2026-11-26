# Data registry

What `obc data` reads: declarations only, in TOML and JSON, with no code. The external sources
with their licences and credits, the regions, the targets, the archive datasets, the route
sources, the planner options and the TrailView rules.

The engine that reads these files is `host/obc-data`; the products and steps are in
`host/obc-data-steps`; Python steps read them through `tools/data_registry.py`. How to run
`obc data` is in [`host/obc-data/README.md`](../host/obc-data/README.md).

## Where to start

| Path | What | Contract |
| --- | --- | --- |
| `sources.toml` | One row for each external dataset, asset or tool: licence, credit, fetch, version, refresh, `keep`. | [obc-data-registry](../specs/obc-data-registry.md) |
| `regions/` | Presets; the id is the path without `.toml`. Named regions at the top, Geofabrik paths under `europe/` and `north-america/`, TrailView tune boxes under `trailview/`. | obc-data-registry |
| `targets.toml` | Where releases go: `prod` (the public bucket) and `local` (a folder). | [obc-data-targets](../specs/obc-data-targets.md) |
| `archive/` | The datasets of the archive of whole datasets. | [obc-data-archive](../specs/obc-data-archive.md) |
| `routes/` | Which layers and fields of a route source make a route. | obc-data-registry |
| `planner.toml` | Planner options that are the same for each region. | obc-data-registry |
| `planner-runtime.toml` | The runtime of the planner services, and the origins of a publication. | [obc-data-products](../specs/obc-data-products.md) |
| `machines.toml` | The operator machines for the operator page. | obc-data-registry |
| [`trailview/`](trailview/README.md) | The TrailView rules: materials, kinds, parts, regions. | its README |

The guide pages are `docs/content/software/data.md` and `data-sources.md`; the second renders
its tables from `sources.toml`.

## Run and test

```sh
obc data sources --region ID            # the state and holding of each source; fetches nothing
obc data areas QUERY                    # search the regions and the Geofabrik index
obc data update local --region ID --dry-run   # the review of an update, with its download size
obc test -p obc-data                    # loads the checked-in sources, regions, targets and machines
obc test -p obc-data-steps              # loads planner.toml and the route sources
python3 tools/check_data_sources.py     # every fetched host is a declared source
```

`obc help data` lists the subcommands. A change under `data/` selects the tests of `obc-data` and
`obc-data-steps`; `sources.toml` also selects `obc-app` (the About page credits), the site
footers and the licence checks.

## Production

`obc data update prod` publishes to the public bucket. It starts only with `--expect ID`, where
ID is a review that the owner approved: `obc data update prod --yes --expect ID`. Never run two
publications, or a publication and a deployment, at the same time. After a production update,
run the route pictures steps of
[`builder/web/tests/browser/trailview/README.md`](../builder/web/tests/browser/trailview/README.md).

An update can download a lot: the planet is about 84 GB, the Copernicus terrain archive about
590 GB. Read the review of a `--dry-run` first.

## Gotchas

- The build embeds the registry. `sources.toml` is compiled into `obc-data`, `planner.toml` into
  `obc-data-steps`, `trailview/kinds.json` into `obc-trailview-features`. `obc data` rebuilds
  first; an old binary keeps the old values.
- Every file refuses an unknown key. A new key needs a change of its reader.
- Every host that code fetches from must be in the registry: the host of a `fetch.url`, one of
  the `hosts` of a source, or a `NOT_SOURCES` entry of `tools/check_data_sources.py`. The guard
  also refuses a source with no `used_by`, `use` or `keep`.
- A `data` or `asset` source with no `licence` is blocked. The licence is an SPDX expression.
  The text a rider sees is `attribution`, never the SPDX id.
- `attribution` is the one home of a credit. Some places cannot generate their text and keep a
  copy that a test compares with the registry: the About page of the device, the site footers,
  the third-party notice of the web app. A changed credit fails those tests until the copies
  follow.
- Only the OSM sources and the prepared sources refresh by age. A prepared source has
  `redistribute = true`, `version = "date"` and `keep = "prepared"`.
- A region whose single `areas` entry equals its own id is a Geofabrik-path region, and
  `obc bake` bakes every one of them. For a preset only, use an id that differs from the path,
  as `dach.toml` does.
- A box is longitude first. The first `countries` entry is the fallback country.
- A target keeps a copy of its region in the release it serves. An edit of a region file
  changes nothing for a target until an update names `--region` again.
- `regions/trailview/all.toml` is the union of the tune boxes and is kept by hand.
- A file in `routes/` needs a `sources.toml` row with `used_by` planner and `keep = "archive"`.
  Its `origin` number is unique, and a deleted number is never used again.
- The public bucket gets client files only. No prepared source goes to a target, because an
  output that no client reads can derive from a source that may not be given to others.
- A target `prefix` must be under `test/`, because a collection deletes keys.
- `obc data deploy services` refuses an uncommitted change of `planner-runtime.toml`.
- Source credentials live in `~/.config/openbikecomputer/`. Never print them. Set one with
  `obc data source credential ID`, which reads standard input.
- "Cairngorms" in the TrailView regions is a mountain range.

## Decisions

- Data changes only on purpose. The OSM sources refresh by age so that the roads stay current;
  a prepared source changes only when a person prepares a newer version.
- The registry is the one list of external data, which is why a guard checks every fetched host.
- One string is the region id and the Geofabrik path, so the two cannot disagree.
- `host/obc-data` is MIT OR Apache-2.0 and links no GPL crate
  (`tools/check_obc_data_licence.py`); the steps that link GPL code are in `obc-data-steps`.
