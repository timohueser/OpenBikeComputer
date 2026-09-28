# Planner previews

These entry points use the builder's Svelte runtime. The map renders vector
tiles and terrain. The editor uses the Rust route service for routing, terrain profiles and
moving time. Place search still uses examples.
They are not inputs to the default production build.

Install the [PMTiles CLI](https://docs.protomaps.com/pmtiles/cli) on `PATH`.
Use Python 3.11 or later and Node.js. From the repository root:

```sh
npm ci --prefix builder/app
python3 tools/planner_maps.py prepare --basemap <Protomaps-PMTiles-URL>
python3 tools/planner_maps.py serve
```

Choose a compatible source from [Protomaps builds](https://maps.protomaps.com/builds/).
Preparation extracts the Baden-Württemberg bounding box, with vector zooms 0–14
and terrain zooms 0–12. Terrain includes a border of neighbouring tiles for
contour calculation at the region edge. It copies fonts, sprites and their licence notices.
The output goes to `builder/app/public/data/planner/`, which git ignores.
Archive checks must pass before the bundle becomes visible. `manifest.json`
records source URLs, bounds, file sizes and SHA-256 hashes. Move an existing
bundle aside before preparing a replacement. Use `prepare --bbox west,south,east,north`
for another region. `--pmtiles /path/to/pmtiles` selects a CLI outside `PATH`.

Open `http://127.0.0.1:4175/planner.html` for the editor or
`http://127.0.0.1:4175/map-study.html` for map styling.
Start the [route service](../../../../../apps/route-server/README.md) first.
Map coverage and route-service coverage are separate. `serve --routing URL`
selects the route service; `--port` and `--tile-port` change the local ports.
Ctrl-C stops both preview servers. All map requests stay local.
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

For R2, upload the bundle under a versioned prefix. Serve the basemap with range
support, expose terrain XYZ requests through a Worker, and serve assets as static
files. Set the four map URLs above. Cross-origin endpoints need CORS.
The planner build excludes the bundle; distribute it separately. Local serving
tests the archive and HTTP interfaces, but does not emulate Worker caching or billing.
The supplied archives do not share the route package's OSM snapshot. A release
pipeline must align vector, routing and search snapshots.

## Checks

From `builder/app`:

```sh
npx vitest run src/lib/planner/ src/components/planner/
npx svelte-check --tsconfig tsconfig.planner.json --fail-on-warnings
```

`npm run check` checks the full app. The full type check needs the generated WASM packages described in the builder
README. Native iOS rendering and mobile performance need separate validation.
