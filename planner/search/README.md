# Planner search

Run place search and smart requests locally or on the VPS. The service binds to loopback.
Caddy serves hosted requests with the configured site origin. SQLite and model inference
need no external search service. Map tiles and routing are separate services.

## Setup and run

From the repository root, run `obc planner setup` and `obc planner` for maps,
search and routing. See the
[planner README](../../builder/web/src/components/planner/README.md).
Run search separately below.

To prepare dependencies and the query model separately, install Node 24 or later,
Python 3.12 or later, `uv`, and the GitHub CLI. Run from the repository root:

```sh
python3 planner/search/setup.py
OBC_SEARCH_DATA=RELEASE/search npm run dev --prefix planner/search
```

Use the search directory from `obc planner prepare`. The
[Rust baker](../../host/obc-search-bake/README.md) produces addresses, POIs, and
localities from the release's OSM snapshot. POI tiles are a projection of this
search database. Device and planner POIs share classification and coordinates.
Setup verifies the query model hash. Pass `--data-dir DIRECTORY` to store it elsewhere.
Build packages in a fresh directory. An interrupted build has no completion metadata.

Build independent components from one verified enriched dump:

```sh
uv run --locked --group planner-search python planner/search/split.py SOURCE.jsonl.zst /tmp/search-records
uv run --locked --group planner-search python planner/search/build.py /tmp/search-records/pois.jsonl.zst --component pois --output DATA/pois --region REGION --bounds=WEST,SOUTH,EAST,NORTH --countries=de,ch --osm-sha256=SHA256 --time-zone=Europe/Berlin
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

Live apply builds services on the VPS from the pushed commit. Configure the Python and
Node `major.minor` versions in `data/planner-runtime.toml` and set `OBC_PLANNER_HOST`.
The [runtime contract](../../specs/obc-data.md#service-runtimes) defines installation
and readiness. Allow a short outage while the services stop and receive the new data.

## Runtime and data limits

[`query/`](query/) parses sentences with the pinned int8 model.
[`query/contract.json`](query/contract.json) defines the decoded request.
[`validation.mjs`](validation.mjs) checks its plan context. The resolver applies it to the view, route, days, and places. Explicit words override
pointing; pointing overrides the view. Edited requests bypass inference.

[`runtime.mjs`](runtime.mjs) is shared by the server and the
[native provider](../../companion-ios/PlannerSearch/README.md). It retrieves mapped places from SQLite and merges
results across search cells. Search returns plan edits; the client applies them and
calls its routing service. Search does not call routing.

The server exits with status 1 if inference stops or hangs. Run it under a supervisor
such as systemd. Clients must discard stale responses.

Supply riding time only when every leg is routed. Stretch queries need routed segments.
Without cumulative distances, search measures the line. Split and join require
unpinned nights and no rest days; they preserve the line.

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
npm test --prefix planner/search
npm run test:query --prefix planner/search
npm run test:data --prefix planner/search
npm run test:model --prefix planner/search
node planner/search/benchmark.mjs PACKAGE.sqlite REFERENCE.sqlite
```

The first two suites run in CI without large downloads. The last two use local packages
and model weights. Model evaluation uses the hand-written EN, DE, FR, and IT testsets.
For the shared BW cache, pass `OBC_SEARCH_DATA` and
`OBC_SEARCH_REGIONS=baden-wuerttemberg` to the data suite.
The benchmark checks exact stored records and search results against a reference package.
It reports host query times and combined process memory. It does not measure phone performance.

## Model development

The generator, templates, decoder, training, and ONNX export code live in `query/`.
Use [the training CLI](query/train.py) to retrain. Keep the held-out
sentences separate from template and lexicon changes.

Search data carries the `osm-planet` credit of [`data/sources.toml`](../../data/sources.toml).
The category vocabulary derives from the iD tagging schema (ISC). The model derives from
[mmBERT-small](https://huggingface.co/jhu-clsp/mmBERT-small) (MIT). German address terms derive from libpostal (MIT). See the adjacent
`LICENSE.*` files. `opening_hours` is an npm dependency under LGPL-3.0.
