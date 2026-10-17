# Trail View: painted panoramas from map data

A throwaway browser prototype. It draws a 360° view at a fixed viewpoint from elevation, OSM and
(later) land cover and Sentinel-2 colour. This branch is an archive. It never merges. Findings go
in a draft pull request from this branch.

## What is here

| Path | What it is |
|---|---|
| `bake.py` | Bakes one preset into `~/.cache/obc/trail-view/<preset>/` |
| `presets.json` | The viewpoints. A `photo` entry takes position and camera from the `peak-view-photos` fixture |
| `index.html`, `src/` | The Vite + TypeScript + three.js page |

## Bake

Needs GDAL 3.13 on `PATH` and `uv`. Run from the repository root:

```sh
uv run prototype/trail-view/bake.py hahnen
```

- GLO-30 tiles come from `~/.cache/obcm/dem/` where they exist, else from the public COGs.
- swissALTI3D tiles are fetched with `host/obc-dem/reference` into `~/.cache/obc/trail-view/_sources/ch/`.
- The DTM covers ±10 km, about 1 GB of tiles per place. `--dtm-levels 3` stops it at ±5 km.
- `--skip-peaks` keeps an earlier `peaks.json`. The Overpass query covers 320 km and takes minutes.
- The photographs come from the `peak-view-photos` fixture package in
  `~/.cache/openbikecomputer/fixtures/`.

## Open

```sh
cd prototype/trail-view && npm ci && npm run dev
```

Then open `http://localhost:5180/`. The `trail-view` entry in `.claude/launch.json` runs the same.
For an iPhone on the same Wi-Fi, run `npm run dev -- --host` and open the printed network address.

- Drag to look, scroll to change the field of view. Either one leaves the photo camera.
- "photo camera" frames the view like the photograph; "wipe" moves the border between the two.
- Red is the skyline read off the photograph, cyan is the skyline cast through the active surface.
- "Light" sets the season (one day each, 2025) and the hour in the preset's time zone.
- "Look" tunes haze, mist and the light bands; "copy look JSON" copies the values.
- "save side by side" and "save view" write PNGs to `~/.cache/obc/trail-view/shots/`, named
  with the preset, the surface and the clock time.

## What will bite you

- The eye stands 1.6 m above the active surface at the viewpoint. GLO-30 is a surface model, so its
  eye can be metres above the DTM's.
- A level blends into the next over its outer tenth. A skyline in that band is part GLO-30 even
  with the DTM on.
- The dev server sends `*.bin.gz` with `Content-Encoding: gzip`, so the browser unpacks them.
- The sun model is the planner's `sun.ts`, imported as `planner-sun`. `tsc` reads
  `src/planner-sun.d.ts` instead, because the planner's own imports need builder/web's packages.
