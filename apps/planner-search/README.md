# Planner search

Run place search and smart requests locally or on the VPS. The service binds to loopback.
Caddy serves hosted requests with the configured site origin. SQLite and model inference
need no external search service. Map tiles and routing are separate services.

## Setup and run

From the repository root, run `obc planner setup` and `obc planner` for maps,
search and routing. See the
[planner README](../../builder/app/src/components/planner/README.md).
Run search separately below.

Install Node 24 or later, Python 3.12 or later, `uv`, and the GitHub CLI. Run from the
repository root. Allow 12 GB for sources and packages.

```sh
python3 apps/planner-search/setup.py --build-data
npm run dev --prefix apps/planner-search
```

Setup verifies the source and model hashes before use. It builds both SQLite packages
from the prepared [Photon Germany dump](https://download1.graphhopper.com/public/europe/germany/).
This preview does not install Photon or OpenSearch. The common-source release pipeline
builds a private Nominatim database and exports it with Photon. See the planner README.
Neither build tool runs as a public service.
To use existing packages, omit `--build-data`. An interrupted build has no completion
metadata. Build into a fresh directory with `build.py SOURCE --output DIRECTORY`.

Pass `--data-dir DIRECTORY --region baden-wuerttemberg` to setup to build only BW
in another directory. Set `OBC_SEARCH_DATA` to that directory when starting the
search service. A regional build becomes visible only after it completes.

Build independent components from one verified enriched dump:

```sh
uv run --with-requirements apps/planner-search/requirements-build.txt python apps/planner-search/split.py SOURCE.jsonl.zst /tmp/search-records
uv run --with-requirements apps/planner-search/requirements-build.txt python apps/planner-search/build.py /tmp/search-records/pois.jsonl.zst --component pois --output DATA/pois --region REGION --bounds=WEST,SOUTH,EAST,NORTH --countries=de,ch --osm-sha256=SHA256
```

Use `addresses.jsonl.zst`, `--component addresses`, and `DATA/addresses` for
addresses. The service opens both directories from `OBC_SEARCH_DATA=DATA`.
The split runs once per source identity. A POI transform update reads only
its filtered records. Addresses own streets and houses; POIs own places and
localities. Both retain their indexes and source context.

For local map tiles, place `basemap.pmtiles` and `places.pmtiles` in `builder/app/public/data/planner/`:

```sh
VITE_PLANNER_PMTILES_URL=/data/planner/basemap.pmtiles npm run dev --prefix apps/planner-search
```

Terrain uses `VITE_PLANNER_DEM_URL`, a Terrarium WebP tile URL template. The default map
URLs point to local files and services. Search still works if
those sources are unavailable.

| Setting | Default |
| --- | --- |
| `OBC_SEARCH_DATA` | This folder's `data/` |
| `OBC_SEARCH_PYTHON` | This folder's `.venv/bin/python` |
| `OBC_SEARCH_PORT` | `8780` |
| `OBC_PLANNER_PORT` | `4184` |
| `OBC_QUERY_ROUTER` | `http://127.0.0.1:8788` |
| `OBC_SEARCH_REGIONS` | `germany,baden-wuerttemberg` |
| `OBC_SEARCH_ORIGINS` | Loopback origins only when unset |
| `OBC_SEARCH_SAMPLE` | Repository Black Forest GPX |

The combined development command passes `OBC_SEARCH_PORT` to the Vite proxy.

## Boundaries

- `query/runtime.py` runs the pinned int8 mmBERT model. `query/decode.py` validates its
  word labels. The model receives only the sentence and never creates place results.
- `web/engine.mjs` retrieves names and addresses with SQLite name, FTS5, and trigram
  indexes. Business matches use text and proximity. Geographic prominence is bounded.
- `resolver.mjs` applies the decoded request to the current view, route, days, and places.
  Explicit words override pointing. Pointing overrides the map view.
- `runtime.mjs` supplies local database, parser, and calendar adapters. `server.mjs`
  serves JSON. The [native provider](native/README.md) uses the same runtime.
  Edited requests bypass inference. The client discards stale responses.
- `routing.mjs` sends the `/v1/route` answer on unchanged. Routing commands fail visibly
  if the routing engine is absent. No route is committed after a failed request.

The request context accepts cumulative `plan.km` and `plan.hours` arrays aligned with
coordinates, and `plan.segments` with kilometre bounds and verified route attributes.
Without `plan.km`, search measures the line. The UI supplies riding time only when every
leg is routed.
Surface, gradient, access, and closure queries report missing segment data.
The sample line preserves imported coordinates and has no terrain data. Split and join keep the line. They require unpinned nights and no rest days.

Opening filters use mapped `opening_hours`. Unknown hours are excluded and counted.
`calendar-bundle.mjs` builds regional calendar adapters without changing the host zone.
Trip-day filters need a start date. Weekday filters need no date; date-dependent rules
remain unknown. No filter predicts arrival time. Distances from the route are geometric,
not routed detours. Place gaps depend on map completeness. Search does not interpolate
house numbers. A missing number returns a clearly labelled street location.

`POST /api/planner-search/reverse` accepts `region` and `[longitude, latitude]` in
`coordinate`. It returns `label` for the nearest mapped house within 100 metres,
or `null`. Search packages use schema 4. Rebuild with `build.py` after a schema change.

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

Search data is © OpenStreetMap contributors, [ODbL 1.0](https://www.openstreetmap.org/copyright).
The category vocabulary derives from the iD tagging schema (ISC). The model derives from
[mmBERT-small](https://huggingface.co/jhu-clsp/mmBERT-small) (MIT). German address terms derive from libpostal (MIT). See the adjacent
`LICENSE.*` files. `opening_hours` is an npm dependency under LGPL-3.0.
