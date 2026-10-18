# Trail View: painted panoramas from map data

A throwaway browser prototype. It draws a 360° view at a fixed viewpoint from elevation, OSM, land
cover and Sentinel-2 colour. This branch is an archive. It never merges. Findings go
in a draft pull request from this branch.

## What is here

| Path | What it is |
|---|---|
| `bake.py` | Bakes one spot into `~/.cache/obc/trail-view/<spot>/` |
| `spots/<short>.json` | The viewpoints of one region, with the data each one bakes from |
| `regions/<short>.json`, `regions/<short>.<sub>.json` | Rulesets: an ecoregion and its subregions, their terrain rules and their look |
| `packs/`, `styles/` | Asset packs (the trees, plants, herds and house style a region draws) and the shared house styles |
| `src/models/` | One file per tree, plant and animal model, and the building kinds |
| `index.html`, `models.html`, `review.html`, `src/` | The view, the model sheet and the review page: Vite, TypeScript and three.js |
| `tune/` | The Europe tune: its instructions ([tune/README.md](tune/README.md)), the baseline ecoregions and country borders with the scripts that make them, and the commands below |

## Linux setup

The machine for the Europe tune is set up as [tune/README.md](tune/README.md#linux-setup) says.

## Spots, regions and packs

`spots/<short>.json` lists the spots of one region. A spot's walk starts at `regions/<short>.json`.

| Spot key | What it is |
|---|---|
| `id`, `name` | A lower-case slug, unique across the files, and the name in the page |
| `lat`, `lon`, `heading`, `pitch` | The eye and the first view |
| `photo` | A `peak-view-photos` fixture name: it gives position, ground and camera. Or `{url, credit, heading_deg, pitch_deg, vfov_deg, roll_deg}` for a third-party photograph |
| `eye_above_ground_m`, `ground_m` | The eye above the ground (1.6 m), and the ground height where the DTM is wrong |
| `dtm` | A national DTM: `ch`, `de-bw` and the other `de-*`, `at`, `uk`, `no`, `fr`, `es`, `it-bz`, `us`, `scot`. Without one, the spot is GLO-30 only |
| `landcover` | `nlcm` (Ireland) or `habmos` (Scotland): habitats over ±5 km |
| `geology` | `igme` (Spain): rock colour and hardness from the 1:50 000 map |

A region inherits all it does not say from the one it `extends`: objects merge, anything else
replaces. `regions/<short>.json` is the ecoregion `short` of `tune/ecoregions.json` and extends
`world`. `regions/<short>.<sub>.json` is a subregion that covers `where`: boxes `[west, south, east,
north]` or rings of `[lon, lat]`. The walk steps into the subregion that holds the eye, the smallest
on a tie, until none does: `world` › `celtic` › `celtic.irish-coast`.

bake.py writes the walk to `meta.json`: `region` (the last id) and `chain` (root first), `country`
(lower-case ISO 3166-1 code from `tune/countries.json`, else the nearest country within 25 km) and
`tz` (from timezonefinder). The page draws `meta.region` for `meta.country` at the clock of
`meta.tz`. An older bake has none of them: it takes its spots file's region and UTC.

| Region key | What it sets |
|---|---|
| `ecoregion` | The RESOLVE id of the ecoregion, on `<short>.json` only |
| `leaf`, `pack` | Default leaf type, the asset pack |
| `cliffs`, `summits`, `relief` | GLO-30 shaping: sheer sea cliffs, mapped summits as cones, ridges sharpened |
| `look` | Ramps per land cover class, moor, speckle (scattered bushes), habitat colours, outcrops, rock, cliff, road surfaces and edge lines, snowline, sea, swell, haze, grade, weather |
| `look.ramps.<class>` | One ramp `[low, mid, high]` by NDVI for the year, or one per season: `{"winter": […], "summer": […]}`. A season it leaves out keeps `palette.ts`'s. A child's value replaces its parent's |

A pack in `packs/<short>[.<sub>].json` extends `base` or a pack of its short. It keys its trees,
ground plants and herds by species, so a child adds or changes one and `null` drops one. An entry
whose key is not a model id names its `model`; a stand-in also gives its own species' `latin`.
Trees go by leaf type and elevation; a plant model with `cover: "shrub"` stands as a shrub. Herds go
by habitat, or by the WorldCover classes a habitat stands for. An ecoregion owns its regions and
packs; two that share a look keep a copy each. Models and house styles are shared. A pack's `style`
and a look's `roadLine` may be a map by country, `{"ie": …, "*": …}`.

## Bake

Needs `uv` and GDAL 3.13 on `PATH`: `gdalwarp`, `gdal_translate`, `gdalbuildvrt`, and
`gdal_proximity` or `gdal_proximity.py`. A bake checks for them, and for the CDSE keys when it reads
colour, before it starts. Run from the repository root:

```sh
uv run prototype/trail-view/bake.py <spot> [--colour] [--dtm-levels 3]
```

- A rebake keeps the Sentinel-2 levels, most of a bake's time, while the spot stays at the
  position in its `meta.json`. `--colour` reads them again. `--dtm-levels 3` stops the DTM at
  ±5 km instead of ±10 km, about 1 GB of tiles.
- Bakes can run side by side. Each shared source is a lock in `_sources/locks/`: `overpass`,
  `cdse`, `dtm-<key>`, `glo30`, `landcover-<key>` and `worldcover`. A bake that waits prints
  `waiting for <name>`. Cached answers are written whole or not at all.
- GLO-30 comes from `~/.cache/obcm/dem/` where it exists, else from the public COGs. WorldCover
  comes from its public COGs. The Sentinel-2 quarterly mosaics come from CDSE S3 with the keys in
  `~/.config/openbikecomputer/cdse-s3.env`, as `tools/planner_snow.py` reads them.
- The DTM comes through the `host/obc-dem/reference` adapters into `_sources/<dtm>/`. `us`, `fr`
  and `es` go in 3 km requests. A `de-bw` zip becomes GeoTIFF and its XYZ text is deleted. `scot`
  is Scotland's LiDAR Phase 1 in 10 km squares; where it was never flown the spot is GLO-30 only.
- Overpass queries, each cached per position in `_sources/osm/`: highways and forests (±5 km)
  with buildings (±2.5 km); water and multipolygon water (±10 km); walls, hedges, orchards, aretes
  and ridges (±5 km); lifts (±10 km); the coastline (±10 km); and peaks over 320 km, which takes
  minutes.
- Roads are graded into the heights: level across, smoothed along, with banks of at most 40°.
  Trees stand on tree pixels within 1 km, as tall as the canopy (GLO-30 minus the DTM), or 5 to
  9 m without a DTM.
- WorldCover water that OSM does not map stays water only where it is flat. With an OSM coastline,
  the sea lies where it says. Where GLO-30 is the only elevation, cliff coasts are made sheer.
- `nlcm` returns 2000 polygons in about 75 s; `habmos` is one WFS request. Third-party photographs
  download to `_sources/photos/`. Never commit them.
- A spot takes 5 to 12 minutes and about 100 MB. A first bake in Baden-Württemberg or the US
  spends most of its time on the DTM.

## Commands

Run these in `prototype/trail-view`.

```sh
npm run dev                                   # the view at http://localhost:5180/
npm run check                                 # tsc, then tune/check.mjs
npm run check -- --assets                     # also every model and the stand-in census
npm run shoot -- <spot>[,<spot>…] [options]   # headless shots
node tune/comment.mjs <short> [--spot <id>] [--round <n>] <text…>
```

- Port 5180 is strict: a second `npm run dev` fails. Serve another worktree with
  `npx vite --port 518N`. `.claude/launch.json` has a `trail-view` entry for 5180.
- `npm run check` reports one line per problem, with the file and the key path: unknown keys,
  wrong types, missing references, areas out of order or outside Europe, duplicate spot ids,
  spots more than 10 km outside their ecoregion, a region or pack of another short, and two models
  with one Latin name. The page runs the same check at load and shows the errors in its status line.
- `models.html` shows every model of a pack at each level of detail.

| `npm run shoot` option | What it does |
|---|---|
| `--yaw a,b`, `--pitch`, `--vfov` | The views; the spot's heading by default |
| `--hour`, `--season`, `--weather` | 15, `summer` and `sunny` by default |
| `--no-photo` | No photograph side by side |
| `--time` | Also the median frame time |
| `--out <dir>` | Copies the shots there too |
| `--swiftshader` | WebGL on the CPU, for a machine without a GPU |
| `--round <n>` | Copies the shots into review round n of the spot's region; `--region <short>` names it for a spot outside `spots/` |

Shots land in `~/.cache/obc/trail-view/shots/` as
`<spot>-<surface>-q<quarter>[-<weather>][-<yaw>]-<hhmm>.png`; a side by side has no yaw. Each spot
prints one JSON line with its files, the renderer, the triangles and the page errors.

`review.html` is the owner's review page. It lists the 45 baseline regions and the Cascades test
region by status: no shots, in review, approved. A region shows each spot's reference photo, each
round beside the previous round, a link to the live view, and comment boxes. A comment or an
approval belongs to the round on the screen; approve works on the newest round only. A newer round
puts a region back in review. Rounds live in `~/.cache/obc/trail-view/review/<short>/<n>/`, comments
in `review/<short>/comments.jsonl`.

## The view

- "spot" picks a place by region, "season" one day of the year, "time of day" the local hour.
  The season also picks the Sentinel-2 quarter and the palette.
- "weather" is sunny, cloudy or rainy: one cloud layer, its shadows, wet ground and rain. "play
  day" sweeps 4:30 to 22:30 in 36 s.
- Drag to look, scroll to change the field of view. "free movement": W A S D or the arrows walk,
  Q and E lower and raise the eye, Shift runs. Plants stand within 60 m of the spot, trees within
  1 km.
- "Photo": "photo camera" frames the view like the photograph; "wipe" moves the border.
  "align camera" moves a photograph's camera, and "copy camera JSON" copies it for `spots/`. Red
  is the skyline read off the photograph, cyan the cast skyline.
- "Readout" › "region" shows the chain of regions, root first.
- "Look" tunes palette snap, haze, mist and the light bands; "copy look JSON" copies them.
- "Tools": "raise eye" lifts the eye. "save side by side" and "save view" write shots.

## What will bite you

- The eye stands 1.6 m above the active surface at the viewpoint. GLO-30 is a surface model, so its
  eye can be metres above the DTM's.
- A level blends into the next over its outer tenth. A skyline in that band is part GLO-30 even
  with the DTM on.
- The dev server sends `*.bin.gz` with `Content-Encoding: gzip`, so the browser unpacks them.
- OSM tags volcanoes `natural=volcano`, not `natural=peak`; the peak query asks for both.
- A photograph's own coordinates can be a place tag rather than where the camera stood. Check
  that the cast skyline at the photo's point shows what the photograph shows.
- Python checks certificates against the system trust store (`truststore`): Spain's IGN serves a
  chain whose root Python's own bundle lacks.
- A spot is only as good as what its eye sees. A point in forest, beside a house, just below a
  crest or on a broad dome hides the view; check the land cover and the line of sight first.
- gdalwarp prints `ERROR 6: ... SetColorTable()` for every WorldCover level, because it copies the
  colour table into a float32 file. The values are correct.
- Colours are linear once read: three's colour management converts every hex value, and the
  shaders tone-map, grade and encode to sRGB in `finish()`. A colour that reaches the screen
  without `finish()` is too dark.
- The sun model is the planner's `sun.ts`, imported as `planner-sun`. `tsc` reads
  `src/planner-sun.d.ts` instead, because the planner's own imports need builder/web's packages.
- A photo block without `heading_deg` and `pitch_deg` stops bake.py. Estimate them, bake, then
  align the camera in the page.
