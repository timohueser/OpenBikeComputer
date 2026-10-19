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
| `index.html`, `models.html`, `src/` | The view and the model sheet: Vite, TypeScript and three.js |
| `tune/` | The Europe tune: its instructions ([tune/README.md](tune/README.md)), the baseline ecoregions and the borders of countries and their subdivisions, with the scripts that make them, and the commands below |

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
| `landcover` | `nlcm` (Ireland) or `habmos` (Scotland): materials over ±5 km |
| `geology` | `igme` (Spain): the lithology over ±5 km from the 1:50 000 map |

A region inherits all it does not say from the one it `extends`: objects merge, anything else
replaces. `regions/<short>.json` is the ecoregion `short` of `tune/ecoregions.json` and extends
`world`. `regions/<short>.<sub>.json` is a subregion that covers `where`: boxes `[west, south, east,
north]` or rings of `[lon, lat]`. The walk steps into the subregion that holds the eye, the smallest
on a tie, until none does: `world` › `celtic` › `celtic.irish-coast`.

bake.py writes the walk to `meta.json`: `region` (the last id) and `chain` (root first), `country`
(lower-case ISO 3166-1 code from `tune/countries.json`, else the nearest country within 25 km),
`subdivision` (the country's first level from the same file, such as `es-ga` or `gr-l`) and `tz`
(from timezonefinder). The page draws `meta.region`'s sky at the clock of `meta.tz`, and each
texel's ground, water and roads by its region in `meta.regions`.

| Region key | What it sets |
|---|---|
| `ecoregion` | The RESOLVE id of the ecoregion, on `<short>.json` only |
| `leaf`, `pack` | Default leaf type, the asset pack |
| `cliffs`, `summits`, `relief` | GLO-30 shaping: sheer sea cliffs, mapped summits as cones, ridges sharpened |
| `look` | Ramps per cover, material and lithology colours, moor heather, speckle (scattered bushes), outcrops, rock, cliff, bare_luma, road surfaces and edge lines, snow_share, sea, swell, haze, grade, weather |
| `look.ramps.<cover>` | One ramp `[low, mid, high]` by NDVI for the year, or one per season: `{"winter": […], "summer": […]}`. A season it leaves out keeps `palette.ts`'s. A child's value replaces its parent's |
| `look.materials.<id>` | `{"colour"}` paints a material of `materials.json` over its cover's ramp, for the year or per season (`{"autumn": …}`) |
| `look.lithology.<id>` | `{"colour", "banded"}` for a lithology's slabs, crags and cliffs; a lithology without one takes `rock` |
| `look.cliff` | Cliff walls, whatever their lithology |
| `look.bare_luma` | The barren ground's luma, one or one per quarter, against which the satellite lightens or darkens bare ground |
| Applies where the eye is | haze, mist, light, `grade`, `weather` |

A pack in `packs/<short>[.<sub>].json` extends `base` or a pack of its short. It keys its trees,
ground plants and herds by species, so a child adds or changes one and `null` drops one. An entry
whose key is not a model id names its `model`; a stand-in also gives its own species' `latin`.
Trees go by leaf type and elevation; a plant model with `cover: "shrub"` stands as a shrub. Herds go
by material. An ecoregion owns its regions and
packs; two that share a look keep a copy each. Models and house styles are shared. A pack's `style`
and a look's `roadLine` may be a map by place, `{"es-ga": …, "es": …, "*": …}`: the subdivision's
entry, else the country's, else `"*"`. Spain, France and Italy are by region (`es-ga`, `fr-occ`,
`it-42`); the other countries by Natural Earth's first level. `tune/countries.json` lists the codes.

## Bake

Needs `uv` and the GDAL command-line tools on `PATH` (the bakes so far used 3.13): `gdalwarp`,
`gdal_translate`, `gdalbuildvrt`, and `gdal_proximity` or `gdal_proximity.py`. A bake checks for
them, and for the CDSE keys when it reads colour, before it starts. Run from the repository root:

```sh
uv run prototype/trail-view/bake.py <spot> [--colour] [--snow] [--dtm-levels 3] [--baseline]
# --baseline: without the spot's landcover and geology maps, as a place that has none
```

| Layer | Per texel and level |
|---|---|
| `material-k` | A material of `materials.json` |
| `litho-k` | A lithology of `materials.json` on land, 0 unknown |
| `region-k` | An index into `meta.regions`: the deepest subregion whose `where` holds the texel, under its ecoregion in `tune/ecoregions.json` |
| `s2-q1…4-k` | k ≤ 6: a quarterly mosaic's colour and NDVI. Where it sees snow, the colour of the nearest quarter that does not (black where none) and alpha 1, so that only the snow data says where snow lies |
| `snow-k` | k ≥ 3: per season of the planner's last three (`data/planner.toml`), the onset and melt-out day step of the longest snow period, as [the planner's snow tiles](../../specs/planner-snow-tiles.md) store them. HR-WSI on levels 3 to 6 where it has data, else MODIS (none under dense canopy). Level 3's texels are HR-WSI's 20 m; the finer levels read it |

| Step | Material and lithology from | Levels | Names |
|---|---|---|---|
| 1 | WorldCover's class; sea and inland water as they settle | all | every texel |
| 2 | Heath: open ground that Sentinel-2 sees as red as green while it grows | k ≤ 6 | where it fits (`REFINES`) |
| 3 | OSM `leaf_type` forests | ±5 km | where it fits |
| 4 | CORINE Land Cover 2018, one image per spot and level in `_sources/clc/` | all | where it fits |
| 5 | OSM natural and landuse areas | ±10 km | where it fits |
| 6 | The spot's `landcover` map | ±5 km | every land texel |
| 7 | Lithology: EGDI 1:1 000 000 (676 MB), IGME 5000 (48 MB) where EGDI names none, both in `_sources/lithology/`; then the spot's `geology` map on ±5 km | all | every land texel |

- A rebake keeps the Sentinel-2 and snow levels while the spot stays put;
  `--colour` and `--snow` read them again. `--dtm-levels 3` stops the DTM at ±5 km instead of
  ±10 km, about 1 GB of tiles.
- Bakes can run side by side. Each shared source is a lock in `_sources/locks/`: `overpass`,
  `cdse`, `modis`, `dtm-<key>`, `glo30`, `landcover-<key>`, `clc`, `lithology` and `worldcover`. A bake that
  waits prints `waiting for <name>`. Cached answers are written whole or not at all.
- GLO-30 comes from `~/.cache/obcm/dem/` where it exists, else from the public COGs. WorldCover
  comes from its public COGs. The Sentinel-2 quarterly mosaics come from CDSE S3 with the keys in
  `~/.config/openbikecomputer/cdse-s3.env`, as `tools/planner_snow.py` reads them.
- The DTM comes through the `host/obc-dem/reference` adapters into `_sources/<dtm>/`. Every DTM
  level reads one cubic 10 m grid of it. `us`, `fr` and `es` go in 3 km requests. A `de-bw` zip
  becomes GeoTIFF and its XYZ text is deleted. `scot` is Scotland's LiDAR Phase 1 in 10 km
  squares; where it was never flown the spot is GLO-30 only.
- Overpass queries, each cached per position in `_sources/osm/`: highways and forests (±5 km)
  with buildings (±2.5 km); water and multipolygon water (±10 km); walls, hedges, orchards, aretes
  and ridges (±5 km); natural and landuse areas (±10 km); lifts (±10 km); and the coastline (±10
  km). Named summits with a height, for `summits`, come in 5° tiles in `_sources/osm/summits/` that
  every spot shares.
- Roads are graded into the heights: level across, smoothed along, with banks of at most 40°.
  Trees stand on tree pixels within 1 km, as tall as the canopy (GLO-30 minus the DTM), or 5 to
  9 m without a DTM.
- WorldCover water that OSM does not map stays water only where it is flat. With an OSM coastline,
  the sea lies where it says. Where GLO-30 is the only elevation, cliff coasts are made sheer.
- `nlcm` returns 2000 polygons in about 75 s; `habmos` is one WFS request. Third-party photographs
  download to `_sources/photos/`. Never commit them.
- A first bake in Baden-Württemberg or the US spends most of its time on the DTM.

### Local Overpass

Where `_sources/overpass` exists, bake.py runs every Overpass query with its `bin/osm3s_query` on
its `db/`, without a server, and not on the public instances. Set it up once per machine, in a
folder `$OVERPASS` on a large disk:

```sh
sudo apt install -y libexpat1-dev zlib1g-dev liblz4-dev osmium-tool
curl -LO https://dev.overpass-api.de/releases/osm-3s_latest.tar.gz && tar xzf osm-3s_latest.tar.gz
(cd osm-3s_v*/ && ./configure --enable-lz4 --prefix=$OVERPASS && make -j install)
curl -LO https://download.geofabrik.de/europe-latest.osm.pbf
osmium tags-filter europe-latest.osm.pbf w/highway w/landuse=forest,orchard,reservoir,vineyard \
  w/natural=wood,water,coastline,arete,ridge,bare_rock,scree,shingle,sand,beach,heath,wetland,grassland \
  w/building w/waterway=river,stream,canal,riverbank w/barrier=wall,retaining_wall,hedge w/aerialway \
  r/natural=water,bare_rock,scree,shingle,sand,beach,heath,wetland,grassland r/waterway=riverbank \
  r/landuse=reservoir,vineyard,orchard n/natural=peak,volcano -o trail-view.osm.pbf
mkdir $OVERPASS/db && osmium cat trail-view.osm.pbf -f osm -o - |
  $OVERPASS/bin/update_database --db-dir=$OVERPASS/db --compression-method=lz4 --map-compression-method=lz4
ln -s $OVERPASS ~/.cache/obc/trail-view/_sources/overpass
```

## Commands

Run these in `prototype/trail-view`.

```sh
npm run dev                                   # the view at http://localhost:5180/
npm run check                                 # tsc, then tune/check.mjs
npm run check -- --assets                     # also every model and the stand-in census
npm run shoot -- <spot>[,<spot>…] [options]   # headless shots
```

- Port 5180 is strict: a second `npm run dev` fails. Serve another worktree with
  `npx vite --port 518N`. `.claude/launch.json` has a `trail-view` entry for 5180.
- `npm run check` reports one line per problem, with the file and the key path: unknown keys,
  wrong types, missing references, areas out of order or outside Europe, duplicate spot ids,
  spots more than 10 km outside their ecoregion, a region or pack of another short, and two models
  with one Latin name. The page runs the same check at load and shows the errors in its status line.
- `models.html` shows every model with the regions that draw it, the census of species that only a
  stand-in draws, and every model of a pack at each level of detail.

| `npm run shoot` option | What it does |
|---|---|
| `--yaw a,b`, `--pitch`, `--vfov` | The views; the spot's heading by default |
| `--hour`, `--date`, `--weather` | 15, `08-15` and `sunny` by default; `--date` takes a list of `MM-DD` |
| `--no-photo` | No photograph side by side |
| `--time` | Also the median frame time |
| `--out <dir>` | Copies the shots there too |
| `--swiftshader` | WebGL on the CPU, for a machine without a GPU |

Shots land in `~/.cache/obc/trail-view/shots/` as
`<spot>-<surface>-<mmdd>[-<weather>][-<yaw>]-<hhmm>.png`; a side by side has no yaw. Each spot
prints one JSON line with its files, the renderer, the triangles and the page errors.

## The view

- "region" picks a spots file and shows its tune status from `~/.cache/obc/trail-view/status.json`,
  "spot" one of its spots, "date" the day in weekly steps, "time of day" the local hour. The date
  sets the sun, the snow, the ground's two mosaics and the trees' season.
- "reference photo" shows the spot's photograph in a corner. A click enlarges it to the view; with
  "photo camera" on, the two then cover each other exactly.
- "weather" is sunny, cloudy or rainy: one cloud layer, its shadows, wet ground and rain. "play
  day" sweeps 4:30 to 22:30 in 36 s.
- Drag to look, scroll to change the field of view. "free movement": W A S D or the arrows walk,
  Q and E lower and raise the eye, Shift runs. Plants stand within 60 m of the spot, trees within
  1 km.
- "Photo": "photo camera" frames the view like the photograph. "align camera" moves a
  photograph's camera, and "copy camera JSON" copies it for `spots/`.
- "models and missing species" opens `models.html` in a new tab.
- "Readout" › "region" shows the chain of regions, root first.
- "Look" tunes haze, mist and the light bands; "copy look JSON" copies them.
- "Tools": "raise eye" lifts the eye. "save side by side" and "save view" write shots.

## What will bite you

- The eye stands 1.6 m above the active surface at the viewpoint. GLO-30 is a surface model, so its
  eye can be metres above the DTM's.
- A level blends into the next over its outer tenth. A skyline in that band is part GLO-30 even
  with the DTM on.
- OSM tags volcanoes `natural=volcano`, not `natural=peak`; the summit query asks for both.
- The local Overpass holds only the tags of its filter. A new Overpass query in bake.py finds
  nothing there until its tags are in the filter and the database is imported again.
- A photograph's own coordinates can be a place tag rather than where the camera stood. Check
  that the view at the photo's point shows what the photograph shows.
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
