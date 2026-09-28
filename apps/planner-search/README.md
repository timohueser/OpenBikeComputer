# Local planner search

Run place search and smart requests on your computer. The service binds to loopback.
Search and query inference need no network after setup. The browser must keep access to
this local server. Map tiles and the routing engine are separate services.

## Setup and run

For the complete BW planner, use `obc planner setup` and `obc planner` from the
repository root. This starts local maps, search and routing together. See the
[planner README](../../builder/app/src/components/planner/README.md).
The commands below run search and its UI separately.

Install Node 24 or later, Python 3.12 or later, `uv`, and the GitHub CLI. Run from the
repository root. Allow 12 GB of free disk space for the source and generated packages.

```sh
python3 apps/planner-search/setup.py --build-data
npm run dev --prefix apps/planner-search
```

Open <http://127.0.0.1:4184/planner.html>. Expand **Baden-Württemberg · local data** and
select **Load Black Forest test route**. This replaces the current draft; Undo restores it.
Select Germany for searches beyond Baden-Württemberg. Both packages stay local.

Try `Kandel`, `Habsburgerstr. 10 Freiburg`, `pizza`, `hotels end of day 1`,
`Bäckereien entlang der Route`, `split into 4 days`, and `reverse the route`.
Edit a chip to correct the interpretation. Changes wait for **Apply change**.
The normal Undo button restores the previous plan.

Setup verifies the source and model hashes before use. It builds both SQLite packages
from the prepared [Photon Germany dump](https://download1.graphhopper.com/public/europe/germany/).
It does not install Photon or OpenSearch. A build from raw OSM is not included.
To use existing packages, omit `--build-data`. An interrupted build has no completion
metadata. Build into a fresh directory with `build.py SOURCE --output DIRECTORY`.

Pass `--data-dir DIRECTORY --region baden-wuerttemberg` to setup to build only BW
in another directory. Set `OBC_SEARCH_DATA` to that directory when starting the
search service. A regional build becomes visible only after it completes.

For local map tiles, place a basemap at `builder/app/public/data/planner/basemap.pmtiles`:

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

The combined development command passes `OBC_SEARCH_PORT` to the Vite proxy.

## Boundaries

- `query/runtime.py` runs the pinned int8 mmBERT model. `query/decode.py` validates its
  word labels. The model receives only the sentence and never creates place results.
- `web/engine.mjs` retrieves names and addresses with SQLite name, FTS5, and trigram
  indexes. Business matches use text and proximity. Geographic prominence is bounded.
- `resolver.mjs` applies the decoded request to the current view, route, days, and places.
  Explicit words override pointing. Pointing overrides the map view.
- `server.mjs` serves JSON. `query.mjs` chooses classic search or model inference.
  Edited requests bypass inference. The client discards stale responses.
- `routing.mjs` calls `/v1/route` on the separate local routing engine. Routing commands
  fail visibly if it is absent. No route is committed after a failed request.

The request context accepts a cumulative `plan.hours` array aligned with coordinates,
and `plan.segments` with kilometre bounds and verified route attributes. The UI supplies
riding time when all legs use the routing engine. Manual legs have no verified time.
Surface, gradient, access, and closure queries report missing segment data.
The sample line preserves imported coordinates and has no terrain data. Split and join keep the line. They require unpinned nights and no rest days.

Opening filters use mapped `opening_hours`. Unknown hours are excluded and counted.
Trip-day filters need a start date. Weekday filters need no date; date-dependent rules
remain unknown. No filter predicts arrival time. Distances from the route are geometric,
not routed detours. Place gaps depend on map completeness. Search does not interpolate
house numbers. A missing number returns a clearly labelled street location.

## Checks

```sh
npm test --prefix apps/planner-search
npm run test:query --prefix apps/planner-search
npm run test:data --prefix apps/planner-search
npm run test:model --prefix apps/planner-search
```

The first two suites run in CI without large downloads. The last two use local packages
and model weights. Model evaluation uses the hand-written EN, DE, FR, and IT testsets.
For the shared BW cache, pass `OBC_SEARCH_DATA` and
`OBC_SEARCH_REGIONS=baden-wuerttemberg` to the data suite.

## Model development

The generator, templates, decoder, training, and ONNX export code live in `query/`.
Use [its README](query/README.md) to restore training data and retrain. Keep the held-out
sentences separate from template and lexicon changes.

Search data is © OpenStreetMap contributors, [ODbL 1.0](https://www.openstreetmap.org/copyright).
The category vocabulary derives from the iD tagging schema (ISC). The model derives from
[mmBERT-small](https://huggingface.co/jhu-clsp/mmBERT-small) (MIT). German address terms derive from libpostal (MIT). See the adjacent
`LICENSE.*` files. `opening_hours` is an npm dependency under LGPL-3.0.
