# TrailView

TrailView is the 3D view of the planner: TypeScript, raw WebGPU and WGSL, no framework. The page is `builder/web/trailview.html`. The terrain streams with LOD from
the master terrain tiles: zooms 0 to 12 from the tile service, zoom 13 from its packs. The ground
look comes from the TrailView layers of the release ([specs/trailview-tiles.md](../../../../../specs/trailview-tiles.md))
and the planner's snow tiles.

## Build and run

From `builder/web`, against a local release:

```sh
obc data serve local                                    # prints VITE_PLANNER_CATALOG
VITE_PLANNER_CATALOG=URL npm run dev -- --mode trailview   # open /trailview.html
npm run build:trailview                                 # dist/trailview
```

The page reads the `active` entry of the planner catalogue, or `VITE_PLANNER_CONFIG`. The ground needs
`trailview_rules` and the `trailview-*` archives in `layers` of the entry; without them the ground
is the palette of the `world` region.

## Layout

| Path | Holds |
| --- | --- |
| `engine.ts` | The frame loop, the view, `prepare()`, `waitForIdle()`, the recovery from a lost device |
| `frame/` | The local metric frame, the walk and fly camera, the input |
| `gpu/` | The device, its pools and shared layouts, the GPU time per pass, the WGSL `// #include` |
| `stream/` | The stored terrain and its decode worker, the feature codec, the ground layers and the worker that builds their pages |
| `terrain/` | The LOD rules and the node selection, the page pools, `drawn-height.wgsl`, the mesh (`drawn-mesh.wgsl`), the page compute, the probe; the stamps (`stamp-core.wgsl`, `stamp.wgsl`), their worker that bins the feature tiles (`bins.ts`), the feature mask (`features.wgsl`), the water meshes (`water-mesh.ts`, `water.wgsl`), the invariant audit (`audit.wgsl`) |
| `structures/` | The bridges (`bridges.ts`), tunnels and their portals (`tunnels.ts`), galleries (`galleries.ts`) and the masts, contact wires and platforms of the tracks (`rails.ts`) as flat faces in vertex colours: one mesh per node from its feature tile, made in the stamp worker (`node.ts`, `mesh.ts`), the anchored meshes of the kinds, whose vertices hang from anchors on the drawn surface (`anchored.ts`); their pools, the heights of the anchors and the draw (`structures.ts`, `structures.wgsl`) |
| `kinds/` | One module per kind of `data/trailview/kinds.json`, found by its file name (`index.ts`), with the models that it stands on the surface; the shared handlers (`kind.ts`): a line structure on the surface, models at points. The instances draw the models of the kinds' points (`structures/kind-points.ts`). A kind's `near` meshes and points draw only within about 2 km, where a node of level 17 draws; a kind names its models, never their numbers |
| `ground/` | The ground pages, `look.wgsl` (the ground shading of the prototype), the road paint (`roads.wgsl`), the rules packing and the per-page compute |
| `structures/` | The houses: a worker makes a record per building of each zoom 14 quarter of the zoom 13 feature tiles (`house-records.ts`); `houses.wgsl` fits each to the drawn surface with a basement, culls it, picks its level of detail and draws it from a template, and lights its windows at night |
| `models/` | The house templates: the prototype's buildings as faces whose coordinates name slots that `houses.wgsl` fills per house; the prototype's trees and plants without three.js (`shape.ts`, `trees.ts`, `trees/`, `plant-shape.ts`, `plants/`); the models of flat faces of the kinds and the dressing (`flat.ts`, lifts, landmarks, rocks, animals, crosses) |
| `instances/` | Instances on the drawn surface: producers emit them (`emit.wgsl`), one indirect draw per model and tier (`sort.wgsl`, `draw.wgsl`), a tree's phase models and crown colours, impostors, culling by the last frame's depth (`hiz.wgsl`), point pools of kinds (`points.ts`) |
| `vegetation/` | Each tree's phase on the date (`phenology.ts`); the tree models of the phases, shared by the forest, the shrubs and the orchards (`tree-models.ts`); the canopy tiles about the eye (`canopy.ts`) and the forest placed from them each frame (`forest.wgsl`); the ground plants and shrubs about the eye (`plants.wgsl`); the orchards' rows from the feature tiles (`orchards.ts`) |
| `dressing/` | The herds of the packs on the materials that they graze, and rocks on open and rocky ground, placed each frame (`dressing.wgsl`) |
| `light/` | The light keys and weather presets, the look at the eye, `shade.wgsl` (toon bands, haze, `finish()`), the sky, the object shadow maps near and far from the eye (`shadow.ts`) |
| `resolve/` | The render targets, the jitter, TAA and `finish()` |
| `rules/` | The regions of `rules.json`, resolved |
| `labs.ts` | The variant registry |
| `view.ts` | The view in the address |
| `hud.ts` | The HUD |

## The address

| Parameter | Value |
| --- | --- |
| `lat`, `lon` | The eye, in degrees |
| `heading`, `pitch` | Degrees: heading clockwise from true north, pitch up |
| `eye` | Walk: metres above the ground, 1.6 by default |
| `alt` | Fly: metres above sea level, at most 100 000 |
| `date`, `time` | `YYYY-MM-DD` and `HH:MM`, the wall clock in Europe/Zurich |
| `weather` | `sunny`, `cloudy` or `rainy` |
| `debug` | `normals`, `lod`, `cracks` (red clear colour), `surface` (red where drawnHeight leaves the mesh by 5 cm), the ground codes `material`, `lithology`, `region`, `snow` (white where snow lies on the date), `albedo` (the ground colour without light), or `mask` (the features of the mask, flat, as `mask_view` in `terrain.wgsl` colours them) |
| `scale` | The render scale per axis, 0.25 to 0.75 (default 0.7); debug views draw at 1, with the nodes of the scale |
| `lab` | `name:variant`, a list with commas |

The HUD lists the keys; Ctrl+C and Ctrl+V copy and paste the view as an address.

## Labs

A lab is an entry in `LABS` of `labs.ts`, with its variants and the default first; `?lab=` selects
one. The HUD has a menu per lab, which loads the page again at the same view. The terrain has no lab: CDLOD by distance, bicubic synthesized levels, and 16-bit
pages with an offset and a scale per page.

## Shots and flights

From `builder/web/tests/browser`:

```sh
npm run pretest:trailview                  # the page against the fixture
node trailview/shoot.mjs [--views FILE] [--out DIR] [--tiles FILE] [--ground DIR] [--colour V] [--features DIR] [--lab LAB] [--hud] [--gpu] [--audit]
node trailview/flight.mjs --gpu [--flights FILE] [--tiles FILE] [--ground DIR] [--colour V] [--features DIR] [--lab LAB] [--cracks N] [--audit N]
node trailview/skyline.mjs [--tiles FILE] [--lab LAB] [--gpu]
trailview/box.sh [view | flight ARGS]       # bake the Engelberg box without obc data; open it; time it
node trailview/network.mjs [--view V] [--mbit 25] [--rtt 40] [--web DIR]  # a cold view of the bake on a shaped link
npm run test:trailview                     # every browser test; CI adds -- --grep-invert @gpu
```

`shoot.mjs` writes one PNG per view of `views.json` after `waitForIdle()`. Two runs can differ
in a few pixels by a few levels: compare shots by their difference. `flight.mjs` prints the GPU
time per pass, the CPU selection time, the nodes, triangles and pages of each flight of `flights.json`. The `ground` pass runs only when ground pages arrive:
read its `amortized` time. Run it on the M1 Pro with `--gpu`: Chrome then draws
2560 × 1600 pixels and reports exact timestamps. `--tiles` serves a `planner-dem` MBTiles instead
of the fixture, and `--features` a folder `z/x/y.bin`; build the page with a config
whose `bounds` are its box. `--lab NAME:VARIANT` adds the lab to each view. `--cracks 10` counts the red pixels of every tenth frame with
`debug=cracks`; such a flight looks down on known terrain only, as `cracks.json` does over the box.

`--ground DIR` serves the bakes of `tools/trailview` and `tools/planner_snow.py` as the tile service
does: `DIR/rules.json`, `DIR/{ground,trailview-canopy}.pmtiles`, `DIR/snow-days/<band>.pmtiles` and
`DIR/colour/<variant>/{q1,q2,q3,q4}.pmtiles`; `--colour` picks the variant (`webp-12`). Build
the page with a config whose `layers` name `https://tiles.test/<name>.json` and whose
`trailview_rules` is `https://tiles.test/trailview/rules.json`.

`skyline.mjs` draws each photograph of `peak-view-photos` from 1.6 m above its `ground_m` and
prints the RMS of the drawn skyline against the photograph's, per quarter-degree column. It exits 1
past an `rms_limit_deg`.

## Checks

```sh
npx vitest run src/lib/trailview
npm run check
npm run check:wgsl          # cargo install naga-cli --version 30.0.1 --locked
```

`check:wgsl` composes each file with its `// #include NAME` lines, as `gpu/wgsl.ts` does.

## The fixture

`tests/browser/trailview/fixture/` holds the planner terrain of a box at Engelberg, from
Copernicus GLO-30 square N46 E008, its feature tiles and its config. To make it again from OSM.pbf:

```sh
cargo run --release --locked -p obc-dem --features terrarium --bin planner-dem -- \
  --dem DIR --bounds 8.37,46.765,8.46,46.83 --output terrain.mbtiles
uv run --locked --group trailview python -m tools.planner_trailview_features --osm OSM.pbf \
  --terrain terrain.mbtiles --bounds 8.37,46.765,8.46,46.83 --output features.pmtiles
uv run --locked --group planner-maps python builder/web/tests/browser/trailview/make-fixture.py terrain.mbtiles features.pmtiles
```

## What bites

- SwiftShader times are not GPU times. Measure on a GPU.
- macOS lowers the GPU clock when a frame has time to spare, so a pass measures long;
  `flight.mjs --gpu` keeps the clock high.
- A browser test that draws for minutes on SwiftShader takes `{ tag: '@gpu' }`: pull requests skip
  it, `test-weekly.yml` runs it each Monday.
- Headless Chromium on Linux with no GPU shows a WebGPU canvas only with
  `--use-vulkan=swiftshader` (`SWIFTSHADER` in `harness.js`). Else the device is lost: "A valid
  external Instance reference no longer exists".
- Chrome rounds timestamps to 100 µs unless it runs with `--enable-webgpu-developer-features`.
- `OBC_CHROMIUM` names a Chromium for the scripts and the smoke when Playwright's own is absent.
- `OBC_BROWSER_WS` names a Playwright browser server for the scripts, so `--gpu` runs share one window.
- Vitest needs the generated WASM: `npm run build:wasm`.
- No shader uses NaN: compilers may assume that there is none. An unknown height is `NONE`.
- Surfaces write linear light; the resolve calls `finish()`. Pixel sizes (`dpdx`, minimum widths)
  multiply by `look.screen.z`.
- A still view shows the mean of 16 jittered frames; only new pages, nodes or extras restart them.
- Debug colours lose bits in the `rg11b10ufloat` frame (2–4/255 near 1).
- A finer node meets a coarser one only on the LOD plateau: keep `RANGE.least` of `lod.ts`
  (`lod.test.ts`).
- The page pools have a fixed size. A view that wants more pages lowers `quality` in the HUD.
- Never let a synthesis read a stamped page: a stored page keeps its raw layer and draws a copy.
- The ground pool has a fixed size too: the farthest pages past it draw from their cover, two levels
  coarser (`short` in the HUD).
- A new date builds every ground page again (the rules, the Sentinel-2 quarters and the snow are
  part of a page) and loads most tile bodies again: a view's bodies exceed the 96 MiB cache.
- Mountain shadows need the `trailview-ground` layer.
- The ground noise is in frame metres about the first view: far from it, float32 coarsens its
  finest grains, and a view that starts elsewhere draws other grains, so compare shots of one first view.
