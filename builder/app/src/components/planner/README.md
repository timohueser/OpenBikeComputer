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

The basemap default points at a public Protomaps demo archive that no longer
exists, so the map needs a local extract. Terrain defaults to public Mapterhorn
tiles, which still work. Set these variables before starting Vite (an
uncommitted `builder/app/.env.local` is the easiest place):

| Variable | Value |
| --- | --- |
| `VITE_PLANNER_PMTILES_URL` | Basemap PMTiles URL; absolute or relative to this host |
| `VITE_PLANNER_DEM_URL` | Terrarium WebP XYZ template with `{z}`, `{x}` and `{y}` |

The style expects the Protomaps basemap schema. Terrain is capped at zoom 12.
Glyphs and sprites still use public Protomaps assets.

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
npx vitest run src/lib/planner/editor.test.ts
npx svelte-check --tsconfig tsconfig.planner.json --fail-on-warnings
```

`npm run check` checks the full app. The full type check needs the generated WASM packages described in the builder
README. Native iOS rendering and mobile performance need separate validation.
