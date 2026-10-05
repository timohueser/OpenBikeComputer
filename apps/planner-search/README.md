# Planner search

Run place search and smart requests locally or on the VPS. The service binds to loopback.
Caddy serves hosted requests with the configured site origin. SQLite and model inference
need no external search service. Map tiles and routing are separate services.

## Setup and run

From the repository root, run `obc planner setup` and `obc planner` for maps,
search and routing. See the
[planner README](../../builder/app/src/components/planner/README.md).
Run search separately below.

To prepare dependencies and the query model separately, install Node 24 or later,
Python 3.12 or later, `uv`, and the GitHub CLI. Run from the repository root:

```sh
python3 apps/planner-search/setup.py
OBC_SEARCH_DATA=RELEASE/search npm run dev --prefix apps/planner-search
```

Use the search directory from `obc planner prepare`. The
[Rust baker](../../host/obc-search-bake/README.md) produces addresses, POIs, and
localities from the release's OSM snapshot. POI tiles are a projection of this
search database. Device and planner POIs share classification and coordinates.
Setup verifies the query model hash. Pass `--data-dir DIRECTORY` to store it elsewhere.
Build packages in a fresh directory. An interrupted build has no completion metadata.

Build independent components from one verified enriched dump:

```sh
uv run --locked --group planner-search python apps/planner-search/split.py SOURCE.jsonl.zst /tmp/search-records
uv run --locked --group planner-search python apps/planner-search/build.py /tmp/search-records/pois.jsonl.zst --component pois --output DATA/pois --region REGION --bounds=WEST,SOUTH,EAST,NORTH --countries=de,ch --osm-sha256=SHA256 --time-zone=Europe/Berlin
```

Use `addresses.jsonl.zst`, `--component addresses`, and `DATA/addresses` for
addresses. The service opens both directories from `OBC_SEARCH_DATA=DATA`.
The split runs once per source identity. A POI transform update reads only
its filtered records.

| Setting | Default |
| --- | --- |
| `OBC_SEARCH_DATA` | This folder's `data/` |
| `OBC_SEARCH_PYTHON` | This folder's `.venv/bin/python` |
| `OBC_SEARCH_PORT` | `8780` |
| `OBC_SEARCH_REGIONS` | `baden-wuerttemberg` |
| `OBC_SEARCH_ORIGINS` | Loopback origins only when unset |

One process serves the one region that `OBC_SEARCH_REGIONS` names.

## Boundaries

- `query/runtime.py` runs the pinned int8 mmBERT model. `query/decode.py` validates its
  word labels. The model receives only the sentence and never creates place results.
- `query/schema.py` writes `query/contract.json`, the query language that validation, the
  resolver, the web planner and the iOS app read.
- `web/engine.mjs` retrieves names and addresses with SQLite name, FTS5, and trigram
  indexes. Business matches use text and proximity. Geographic prominence is bounded.
- `resolver.mjs` applies the decoded request to the current view, route, days, and places.
  Explicit words override pointing. Pointing overrides the map view.
- `federation.mjs` runs each query in the search cells it touches and merges the rows by
  its declared order and limit. `cells.mjs` and the [native provider](native/README.md)
  open the cells.
- `runtime.mjs` supplies the database, parser, and calendar adapters. `server.mjs`
  serves JSON. The native provider uses the same runtime.
  Edited requests bypass inference. The client discards stale responses.
- `server.mjs` exits with status 1 when the query runtime stops or hangs. Its supervisor,
  such as systemd, restarts it.
- Plan edits, such as a new route or a reversed route, return changes. The planner applies
  them and routes the changed plan with its own routing service. Search never calls it.

The request context accepts cumulative `plan.km` and `plan.hours` arrays aligned with
coordinates, and `plan.segments`: runs of one stretch `kind` from `from` to `to` km, with
`ascent` (m) and `gradient` (%) for height runs.
Without `plan.km`, search measures the line. The UI supplies riding time only when every
leg is routed, and segments from its routed line. Without segments, stretch queries
report missing data.
Split and join keep the line. They require unpinned nights and no rest days.

Opening filters use mapped `opening_hours`, each place's country holidays, and the
region time zone. Unknown hours are excluded and counted.
Trip-day filters need a start date. Weekday filters need no date; date-dependent rules
remain unknown. No filter predicts arrival time. Distances from the route are geometric,
not routed detours. Place gaps depend on map completeness. Search does not interpolate
house numbers. A missing number returns a clearly labelled street location.

`POST /api/planner-search/reverse` accepts `[longitude, latitude]` in `coordinate`.
It returns `label` for the nearest mapped house within 100 metres, or `null`.
Search packages use schema 5. Rebuild with `build.py` after a schema change.

## Checks

```sh
npm test --prefix apps/planner-search
npm run test:query --prefix apps/planner-search
npm run test:data --prefix apps/planner-search
npm run test:model --prefix apps/planner-search
node apps/planner-search/benchmark.mjs PACKAGE.sqlite REFERENCE.sqlite
```

The first two suites run in CI without large downloads. The last two use local packages
and model weights. Model evaluation uses the hand-written EN, DE, FR, and IT testsets.
For the shared BW cache, pass `OBC_SEARCH_DATA` and
`OBC_SEARCH_REGIONS=baden-wuerttemberg` to the data suite.
The benchmark checks exact stored records and search results against a reference package.
It reports host query times and combined process memory. It does not measure phone performance.

## Model development

The generator, templates, decoder, training, and ONNX export code live in `query/`.
Use [its README](query/README.md) to restore training data and retrain. Keep the held-out
sentences separate from template and lexicon changes.

Search data carries the `osm-planet` credit of [`data/sources.toml`](../../data/sources.toml).
The category vocabulary derives from the iD tagging schema (ISC). The model derives from
[mmBERT-small](https://huggingface.co/jhu-clsp/mmBERT-small) (MIT). German address terms derive from libpostal (MIT). See the adjacent
`LICENSE.*` files. `opening_hours` is an npm dependency under LGPL-3.0.
