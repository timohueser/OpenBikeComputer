# Planner previews

These entry points use the builder's Svelte runtime. The map renders vector
tiles and terrain. The editor uses the Rust route service for routing, terrain profiles,
and moving time. Place search uses the [local search service](../../../../../apps/planner-search/README.md).
They are not inputs to the default production build.

From `builder/app`:

```sh
npm ci
npm run dev -- --mode web
```

Open `/map-study.html` for map styling or `/planner.html` for the desktop editor.
Start the [route service](../../../../../apps/route-server/README.md) first.
The editor saves its trip in browser storage. Undo and Redo apply to
changes made in the current session. Hold a dragged point still to preview its
route. Release it to save one change. Open Route options to load alternatives. The surface
strip follows the profile range; hover or use arrow keys to inspect each section.
Grade colors start enabled in single-route mode. Use Grade to switch between
grade and day colors. Grades use a 100 m terrain window, shortened at route ends
and terrain gaps. Missing terrain and fragments below 20 m have no grade estimate.

## Tile sources

The basemap default points at a public Protomaps demo archive that no longer
exists, so the map needs a local extract. Terrain defaults to public Mapterhorn
tiles, which still work. Set these variables before starting Vite (an
uncommitted `builder/app/.env.local` is the easiest place):

| Variable | Value |
| --- | --- |
| `VITE_PLANNER_DATA_URL` | Optional downloadable regional package for a public preview |
| `VITE_PLANNER_ROUTING_URL` | Route API prefix; defaults to the local `/routing` proxy |
| `VITE_PLANNER_PMTILES_URL` | Basemap PMTiles URL; absolute or relative to this host |
| `VITE_PLANNER_DEM_URL` | Terrarium WebP XYZ template with `{z}`, `{x}` and `{y}` |

The style expects the Protomaps basemap schema. Terrain is capped at zoom 12.
Glyphs and sprites still use public Protomaps assets. Most rider places (shops,
lodging, food) exist only in the archive's zoom 14 tiles, so the map shows them
from zoom 14. A highlighted place category loads those tiles along the route
once per session, so it shows at every zoom.

Use the PMTiles CLI to extract a region from a compatible archive. Put local
extracts in `builder/app/public/data/planner/`, which is ignored by git. A
symlink to an extract in another checkout works. Vite serves the basemap with
range requests, so `VITE_PLANNER_PMTILES_URL=/data/planner/basemap.pmtiles`
is enough for the basemap.
Serve the terrain archive with `pmtiles serve` and enable CORS for the Vite origin.

```sh
pmtiles serve public/data/planner --interface=127.0.0.1 --port=8787 \
  --cors=http://127.0.0.1:4174 --public-url=http://127.0.0.1:8787
```

In a second terminal, from `builder/app`:

```sh
VITE_PLANNER_PMTILES_URL=/data/planner/basemap.pmtiles \
VITE_PLANNER_DEM_URL='http://127.0.0.1:8787/terrain/{z}/{x}/{y}.webp' \
npm run dev -- --mode web --host 127.0.0.1 --port 4174
```

## Checks

```sh
npx vitest run src/lib/planner/ src/components/planner/
npx svelte-check --tsconfig tsconfig.planner.json --fail-on-warnings
```

`npm run check` checks the full app. The full type check needs the generated WASM packages described in the builder
README. Native iOS rendering and mobile performance need separate validation.
