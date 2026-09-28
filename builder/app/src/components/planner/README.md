# Planner previews

These entry points use the builder's Svelte runtime. The map renders vector
tiles and terrain. Routing, elevation profiles and place search use fixtures.
They are not inputs to the default production build.

From `builder/app`:

```sh
npm ci
npm run dev -- --mode web
```

Open `/map-study.html` for map styling or `/planner.html` for the desktop editor.
The editor saves its example trip in browser storage. Undo and Redo apply to
changes made in the current session.

## Tile sources

The defaults use public evaluation sources. Set these variables before starting
Vite to use owned sources:

| Variable | Value |
| --- | --- |
| `VITE_PLANNER_PMTILES_URL` | Basemap PMTiles URL; absolute or relative to this host |
| `VITE_PLANNER_DEM_URL` | Terrarium WebP XYZ template with `{z}`, `{x}` and `{y}` |

The style expects the Protomaps basemap schema. Terrain is capped at zoom 12.
Glyphs and sprites still use public Protomaps assets.

Use the PMTiles CLI to extract a region from a compatible archive. Put local
extracts in `builder/app/public/data/planner/`, which is ignored by git.
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
npx vitest run src/lib/planner/editor.test.ts
npx svelte-check --tsconfig tsconfig.planner.json --fail-on-warnings
```

`npm run check` checks the full app. The full type check needs the generated WASM packages described in the builder
README. Native iOS rendering and mobile performance need separate validation.
