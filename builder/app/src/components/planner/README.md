# Planner previews

These entry points use the builder's Svelte runtime. The map renders vector
tiles and terrain. The editor uses the Rust route service for routing, terrain profiles,
and moving time. Place search uses the [local search service](../../../../../apps/planner-search/README.md).
They are not inputs to the default production build.

## Local Baden-Württemberg planner

Install Node 24+, Python 3.12+, Rust, `uv`, `gh`, `curl`, and the
[PMTiles CLI](https://docs.protomaps.com/pmtiles/cli). Authenticate `gh` for the
query model release. From this checkout:

```sh
obc planner setup
obc planner
```

Open `http://127.0.0.1:4175/planner.html`. Setup downloads prepared search records,
the query model, Protomaps vector tiles, Mapterhorn terrain, fonts and sprites.
It builds the BW search database and all routing profiles from a regional OSM
extract. Routing uses the local curated elevation archive when configured,
with downloaded Copernicus DEM tiles as fallback. Map hillshade and contours
use Mapterhorn. The inputs do not share an OSM snapshot.

Setup needs network access and can take a long time. It reuses complete packages.
Normal launch performs no downloads or builds. Maps, search, model inference and
routing run locally. Ctrl-C stops all four services. Each service binds to loopback.
An occupied port or an incomplete package stops launch with an error.

| Setting | Default |
| --- | --- |
| `OBC_PLANNER_DATA` or `--data-dir` | `~/.cache/obc/planner/baden-wuerttemberg` |
| `--port` | Planner `4175` |
| `--tile-port` | Terrain tiles `8789` |
| `--route-port` | Routing `8787` |
| `--search-port` | Search `8786` |
| `--reference` or `OBC_REFERENCE_ARCHIVE` | `~/obc-reference` when its index exists |
| `--dem-dir` | `~/.cache/obcm/dem` |
| `setup --osm PATH` | Use an existing BW OSM PBF |
| `setup --basemap URL` | Use this Protomaps archive for a new map bundle |

The data directory contains `maps/`, `search/` and `routing/`. Search preparation
reads the Germany Photon dump but builds only the BW package. Route preparation
uses the existing Geofabrik cache. Bounds are `7.45,47.5,10.5,49.85`; routing stays
inside the OSM extract. The map stores vector zooms 0–14 and terrain zooms 0–12.
Terrain includes neighbouring tiles for contour calculation at the region edge.

Run `obc planner verify` for map checksums, SQLite integrity and routing object
verification. To replace a package, stop the planner, move its directory aside,
then repeat setup. A new map preparation selects an available Protomaps v4 build
and records the chosen URL. Existing maps keep their source. `maps/manifest.json`
records bounds, sizes, hashes and source URLs. Routing records source hashes and
terrain credits. Setup does not upload data or change hosting.

For map styling alone, `tools/planner_maps.py` provides `prepare` and `serve`.
Its separate default directory is `builder/app/public/data/planner/`. Open
`map-study.html` on the preview host. Use `--help` for archive and port options.

The editor saves its trip in browser storage. Undo and Redo apply to
changes made in the current session. Hold a dragged point still to preview its
route. Release it to save one change. Open Route options to load alternatives. The surface
strip follows the profile range; hover or use arrow keys to inspect each section.
Grade colors start enabled in single-route mode. Use Grade to switch between
grade and day colors. Grades use a 100 m terrain window, shortened at route ends
and terrain gaps. Missing terrain and fragments below 20 m have no grade estimate.

## Tile sources

The defaults use this host. `serve` sets URLs and bounds for the local bundle.
For another host, set these variables before starting Vite or building the planner:

| Variable | Value |
| --- | --- |
| `VITE_PLANNER_DATA_URL` | Optional downloadable regional package for a public preview |
| `VITE_PLANNER_ROUTING_URL` | Route API prefix; defaults to the local `/routing` proxy |
| `VITE_PLANNER_PMTILES_URL` | Basemap PMTiles URL; absolute or relative to this host |
| `VITE_PLANNER_DEM_URL` | Terrarium WebP XYZ template with `{z}`, `{x}` and `{y}` |
| `VITE_PLANNER_GLYPHS_URL` | Font template with `{fontstack}` and `{range}` |
| `VITE_PLANNER_SPRITES_URL` | Sprite directory; the style appends `/light` or `/dark` |
| `VITE_PLANNER_MAP_BOUNDS` | Optional `west,south,east,north`; limits panning and terrain requests |

The style expects the Protomaps basemap schema. Terrain is capped at zoom 12.
Most rider places (shops,
lodging, food) exist only in the archive's zoom 14 tiles, so the map shows them
from zoom 14. A highlighted place category loads those tiles along the route
once per session, so it shows at every zoom.

Vite serves `basemap.pmtiles` with HTTP range requests. Its `/tiles` proxy sends
terrain requests to `pmtiles serve` on port 8789. The browser makes contours from
those terrain tiles. The bundle contains two archives and an `assets/` directory;
it needs no database or tile build server at runtime.

The planner build excludes generated data. Publication and common-source
preparation are separate work.

## Checks

From `builder/app`:

```sh
npx vitest run src/lib/planner/ src/components/planner/
npx svelte-check --tsconfig tsconfig.planner.json --fail-on-warnings
```

`npm run check` checks the full app. The full type check needs the generated WASM packages described in the builder
README. Native iOS rendering and mobile performance need separate validation.
