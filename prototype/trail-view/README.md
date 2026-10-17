# Trail View: painted panoramas from map data

A throwaway browser prototype. It draws a 360° view at a fixed viewpoint from elevation, OSM, land
cover and Sentinel-2 colour. This branch is an archive. It never merges. Findings go
in a draft pull request from this branch.

## What is here

| Path | What it is |
|---|---|
| `bake.py` | Bakes one preset into `~/.cache/obc/trail-view/<preset>/` |
| `presets.json` | The viewpoints. A `photo` name takes position and camera from the `peak-view-photos` fixture; a `photo` object names a third-party photograph by URL, with its credit and the camera aligned in the page |
| `index.html`, `src/` | The Vite + TypeScript + three.js page |

## Bake

Needs GDAL 3.13 on `PATH` and `uv`. Run from the repository root:

```sh
uv run prototype/trail-view/bake.py hahnen
```

- GLO-30 tiles come from `~/.cache/obcm/dem/` where they exist, else from the public COGs.
- The national DTM (`dtm`) is fetched with the `host/obc-dem/reference` adapters into
  `~/.cache/obc/trail-view/_sources/<dtm>/`: `ch` swissALTI3D 2 m, `de-bw` LGL DGM1 (each 2 km
  zip is converted to GeoTIFF and its XYZ text deleted), `us` 3DEP at 2 m in 3 km requests.
- Third-party photographs download to `_sources/photos/`. They are never committed.
- The near field comes from one Overpass query per position, cached in `_sources/osm/`: highways
  and forests with `leaf_type` over ±5 km, buildings over ±2.5 km. Road levels hold the distance
  to the nearest road edge. Trees stand on tree pixels within 1 km, as tall as the canopy (GLO-30
  minus the DTM). A GLO-30-only place cannot measure a canopy, so its trees stand 5 to 9 m.
- Water comes from a second cached Overpass query over ±10 km: rivers, canals and streams by
  their `width` tag or a default, and water areas. The water levels hold the share of each pixel
  under water, from a 4× finer grid. Streams count only off tree pixels.
- The DTM covers ±10 km, about 1 GB of tiles per place. `--dtm-levels 3` stops it at ±5 km.
- `--skip-peaks` keeps an earlier `peaks.json`. The Overpass query covers 320 km and takes minutes.
- The photographs come from the `peak-view-photos` fixture package in
  `~/.cache/openbikecomputer/fixtures/`.
- WorldCover is read from its public COGs. The Sentinel-2 quarterly mosaics are read from CDSE
  S3 with the keys in `~/.config/openbikecomputer/cdse-s3.env`, as `tools/planner_snow.py` reads
  them.
- A place takes 5 to 12 minutes and about 100 MB, most of it the four Sentinel-2 quarters. A first
  bake in Baden-Württemberg or the US spends most of its time on the DTM.

## Open

```sh
cd prototype/trail-view && npm ci && npm run dev
```

Then open `http://localhost:5180/`. The `trail-view` entry in `.claude/launch.json` runs the same.
`http://localhost:5180/models.html` shows every model of a kit at each level of detail.

- "spot" picks a place by region, "season" one day of the year, "time of day" the local hour.
- Drag to look, scroll to change the field of view. Either one leaves the photo camera.
- "Photo": "photo camera" frames the view like the photograph; "wipe" moves the border between
  the two. "align camera" moves a photograph's camera; "copy camera JSON" copies it for
  `presets.json`. Red is the skyline read off the photograph, cyan the skyline cast through the
  active surface.
- The season (one day each, 2025) also picks the Sentinel-2 quarter and the palette.
- The near field comes from the spot's kit in `src/models/kits.ts`: trees by leaf type and
  elevation, ground plants by land cover within 60 m, and houses fitted to OSM footprints by
  their `building` value and size. Ground plants take their hue from the ground under them.
- "Look" tunes palette snap (0: Sentinel-2 chroma, 1: class palette alone), haze, mist and the
  light bands; "copy look JSON" copies the values.
- "Tools": "save side by side" and "save view" write PNGs to `~/.cache/obc/trail-view/shots/`, named
  with the preset, the surface, the quarter and the clock time.

## What will bite you

- The eye stands 1.6 m above the active surface at the viewpoint. GLO-30 is a surface model, so its
  eye can be metres above the DTM's.
- A level blends into the next over its outer tenth. A skyline in that band is part GLO-30 even
  with the DTM on.
- The dev server sends `*.bin.gz` with `Content-Encoding: gzip`, so the browser unpacks them.
- OSM tags volcanoes `natural=volcano`, not `natural=peak`; the peak query asks for both.
- A photograph's own coordinates can be a place tag rather than where the camera stood. Check
  that the cast skyline at the photo's point shows what the photograph shows.
- gdalwarp prints `ERROR 6: ... SetColorTable()` for every WorldCover level, because it copies the
  colour table into a float32 file. The values are correct.
- Colours are linear once read: three's colour management converts every hex value, and the
  shaders tone-map, grade and encode to sRGB in `finish()`. A colour that reaches the screen
  without `finish()` is too dark.
- The sun model is the planner's `sun.ts`, imported as `planner-sun`. `tsc` reads
  `src/planner-sun.d.ts` instead, because the planner's own imports need builder/web's packages.
