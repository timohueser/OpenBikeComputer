# TrailView

The 3D view of the planner: TypeScript, raw WebGPU and WGSL, no framework. The page is
`builder/web/trailview.html`. The terrain streams with LOD from the master terrain tiles: zooms 0
to 12 from the tile service, zoom 13 from its packs. The ground comes from the TrailView layers of
the release ([specs/trailview-tiles.md](../../../../../specs/trailview-tiles.md)) and the planner's
snow tiles.

## Build and run

From `builder/web`, against a local release:

```sh
obc data serve local                                    # prints VITE_PLANNER_CATALOG
VITE_PLANNER_CATALOG=URL npm run dev -- --mode trailview   # open /trailview.html
npm run build:trailview                                 # dist/trailview
```

The page reads the `active` entry of the planner catalogue, or `VITE_PLANNER_CONFIG`. The ground
needs `trailview_rules` and the `trailview-*` archives in `layers` of the entry; without them it is
the palette of the `world` region.

## Layout

| Path | Holds |
| --- | --- |
| `engine.ts` | The frame loop, the view, `prepare()`, `waitForIdle()`, `nearReady()`, `eyeReady()`, `capture()`, `destroy()`, the recovery from a lost device |
| `still.ts` | `still()`: one view to an image and, for a line, the place of each point in the image and whether it is visible, with an engine of its own that ends after it; `canRenderStills()`: false with no GPU or a software renderer |
| `still-view.ts` | The view of a cover from the route and the terrain tiles (`coverView()`, `chooseView()`): not toward the sun, as low as shows 85 % of the route |
| `budget.ts` | The tiers and the budget of each: the pools, the loads in flight, the frame rate, the byte caps of the caches and the plan workers |
| `pacer.ts` | The display frames that draw at a rate. Headless Chromium runs at 60 Hz, so flights and shots draw each display frame |
| `frame/` | The local metric frame, the walk and fly camera, the input |
| `gpu/` | The device, its pools and shared layouts, the GPU time per pass, the WGSL `// #include` |
| `stream/` | The stored terrain and its decode worker, the feature codec, the ground layers and the worker that builds their pages |
| `terrain/` | The LOD rules and the node selection, the page pools, `drawn-height.wgsl`, the mesh (`drawn-mesh.wgsl`), the page compute, the probe; the stamps (`stamp-core.wgsl`, `stamp.wgsl`), their worker that bins the feature tiles (`bins.ts`), the buildings by place for the town rules (`town.ts`: sidewalks, paved footways, the houses' fronts, the street lamps), the junctions' kerbed corners (`corners.ts`), the feature mask (`features.wgsl`), the water meshes (`water-mesh.ts`, `water.wgsl`), the invariant audit (`audit.wgsl`) |
| `structures/` | The bridges (`bridges.ts`), tunnels and their portals (`tunnels.ts`), galleries (`galleries.ts`) and the masts, contact wires and platforms of the tracks (`rails.ts`) as flat faces in vertex colours: one mesh per node from its feature tile, made in the stamp worker (`node.ts`, `mesh.ts`), the anchored meshes of the kinds, whose vertices hang from anchors on the drawn surface (`anchored.ts`); their pools, the heights of the anchors and the draw (`structures.ts`, `structures.wgsl`) |
| `roads/` | The corridors, the meshes of roads, tracks, paths and railways near the eye; past their range the paint draws them ([guide](../../../../../docs/content/software/trailview-roads.md)). The pools, the culling and the draw (`roads.ts`, `corridor.wgsl`), the mesh of an owner and its pieces (`mesh.ts`, `piece.ts`) from a worker of its own (`road-worker.ts`); the stations, the cross-section and the sweep (`stations.ts`, `section.ts`, `sweep.ts`, `joints.ts`); the junction plan and its faces (`junctions.ts`, `arms.ts`, `corner.ts`, `planes.ts`, `path-ends.ts`, `junction-faces.ts`, `ways.ts`); zebras, give-way lines and lowered kerbs (`marks.ts`); rails and walls, decided per station (`standing.ts`, `ground.ts`, `details.ts`, `details.wgsl`); lamps, sleepers and steps (`lamps.ts`, `tracks.ts`, `parts.ts`, `track-looks.wgsl`); bridge decks (`decks.ts`); where water stops a way (`shore.ts`); the looks (`looks.wgsl`) |
| `kinds/` | One module per kind of `data/trailview/kinds.json`, found by its file name (`index.ts`), with the models that it stands on the surface; the shared handlers (`kind.ts`): a line structure on the surface, models at points. The instances draw the models of the kinds' points (`structures/kind-points.ts`). A kind's `near` meshes and points draw only within about 2 km, where a node of level 17 draws; a kind names its models, never their numbers |
| `ground/` | The ground pages, `look.wgsl` (the ground shading of the prototype), the road paint (`roads.wgsl`), the rows of the material fields (`fields.wgsl`), the rules packing and the per-page compute |
| `structures/` | The houses: a worker makes a record per building of each zoom 14 quarter of the zoom 13 feature tiles (`house-records.ts`); `houses.wgsl` fits each to the drawn surface with a basement, culls it, picks its level of detail and draws it from a template, and lights its windows at night |
| `models/` | The house templates: the prototype's buildings as faces whose coordinates name slots that `houses.wgsl` fills per house; the prototype's trees and plants without three.js (`shape.ts`, `trees.ts`, `trees/`, `plant-shape.ts`, `plants/`); the models of flat faces of the kinds and the dressing (`flat.ts`, lifts, landmarks, rocks, crosses); the modelling kit of the soft low-poly style (`sculpt.ts`) and the herd animals, one file per breed (`animals.ts`, `animals/`). The build writes the fixed models as files (`bake.ts`, [contract](../../../../../specs/trailview-models.md)) |
| `instances/` | Instances on the drawn surface: producers emit them (`emit.wgsl`), one indirect draw per model and tier (`sort.wgsl`, `draw.wgsl`), a tree's phase models and crown colours, impostors, culling by the last frame's depth (`hiz.wgsl`), point pools of kinds (`points.ts`) |
| `vegetation/` | Each tree's phase on the date (`phenology.ts`); the tree models of the phases, shared by the forest, the shrubs and the plantings (`tree-models.ts`); the canopy tiles about the eye (`canopy.ts`) and the forest placed from them each frame (`forest.wgsl`); the ground plants and shrubs about the eye (`plants.wgsl`); the clearing that moves with the eye (`clearing.wgsl`); the plants of the materials planted in rows, on the rows of the material fields (`rows.wgsl`, `ground/fields.wgsl`) |
| `dressing/` | The herds of the packs on the materials that they graze, and rocks on rough and rocky ground, placed each frame (`dressing.wgsl`); the `showroom` address option, which stands a row of animals or tree tiers before the eye for the model review; the rocks' density and placement, which the ground also paints past their instances (`rocks.wgsl`) |
| `light/` | The light keys and weather presets, the look at the eye, `shade.wgsl` (toon bands, haze, `finish()`), the sky with the moon, the stars and the Milky Way, and a solar eclipse where the moon covers the sun (`celestial.ts`, `eclipse.ts`, `night.ts`; `tools/trailview/sky.py` makes `milkyway.webp`, `stars.bin` and `moon.webp`), the object shadow maps near and far from the eye (`shadow.ts`) |
| `resolve/` | The render targets, the jitter, TAA and `finish()` |
| `plan/` | The ground plan with `ground=plan`: the cells about the eye and their inputs (`plan.ts`, `inputs.ts`), the build in the bridge of [`builder/ground-plan-wasm`](../../../../ground-plan-wasm/README.md), one copy per page (`bridge.ts`, `plan-worker.ts`, `pkg/`), the debug draw over the canvas (`debug.ts`, `draw.ts`, `debug.wgsl`) |
| `rules/` | The regions of `rules.json`, resolved |
| `view.ts` | The view in the address |
| `hud.ts` | The HUD |

## The address

| Parameter | Value |
| --- | --- |
| `lat`, `lon` | The eye, in degrees |
| `heading`, `pitch` | Degrees: heading clockwise from true north, pitch up |
| `eye` | Walk: metres above the ground, 1.6 by default |
| `alt` | Fly: metres above sea level, at most 100 000 |
| `date`, `time` | `YYYY-MM-DD` and `HH:MM` or `HH:MM:SS`, the wall clock in Europe/Zurich |
| `weather` | `sunny`, `cloudy` or `rainy` |
| `debug` | `normals`, `lod`, `cracks` (red clear colour), `surface` (red where drawnHeight leaves the mesh by 5 cm), the ground codes `material`, `lithology`, `region`, `snow` (white where snow lies on the date), `albedo` (the ground colour without light), or `mask` (the features of the mask, flat, as `mask_view` in `terrain.wgsl` colours them) |
| `scale` | The render scale per axis, 0.25 to 0.75 (default 0.7); debug views draw at 1, with the nodes of the scale |
| `tier` | `phone`: the budget for the iOS app, with smaller pools and caches, fewer loads in flight, one plan worker and 30 fps (`budget.ts`); else `desktop`. Read once, as `scale` is |
| `ground` | `plan`: a worker builds the ground plan of each cell near the eye; the HUD shows its line. Read once, as `scale` is |
| `plan` | With `ground=plan`: `faces` draws the plan over the view in the debug tool's colours (default `off`) |
| `still` | No engine starts. A headless script calls `window.trailviewStill.still(job, options)`, `coverView(line, when)`, `canRenderStills(options)`, `view(search)`, the view of a query string, and reads `release`, the id of the release; `still.test.js` and `home-views.mjs` show how |

The HUD lists the keys; Ctrl+C and Ctrl+V copy and paste the view as an address.

## Shots and flights

From `builder/web/tests/browser`:

```sh
npm run pretest:trailview                  # the page against the fixture
node trailview/shoot.mjs [--views FILE] [--out DIR] [--width PX] [--height PX] [--tiles FILE] [--ground DIR] [--colour V] [--features DIR] [--hud] [--gpu] [--audit] [--pages N] [--query Q]
node trailview/shoot.mjs --gpu --views trailview/road-views.json --width 1920 --height 960  # these views need this size: the default is 1280 × 800
node trailview/flight.mjs --gpu [--flights FILE] [--tiles FILE] [--ground DIR] [--colour V] [--features DIR] [--cracks N] [--audit N] [--resolve MS] [--query Q]
flock /tmp/trailview-gpu.lock node trailview/shoot.mjs --gpu --views trailview/road-views-top.json --width 1920 --height 960 --query 'ground=plan&plan=faces'  # Q is appended to each view
node trailview/skyline.mjs [--tiles FILE] [--gpu]
trailview/box.sh [view | flight ARGS]       # bake the Engelberg box without obc data; open it; time it
node trailview/network.mjs [--view V] [--mbit 25] [--rtt 40] [--web DIR]  # a cold view of the bake on a shaped link
npm run test:trailview                     # every browser test; CI adds -- --grep-invert @gpu
OBC_TRAILVIEW_GPU=1 flock /tmp/trailview-gpu.lock npm run test:trailview -- --grep @gpu  # @gpu tests on the GPU
(cd ../.. && npx vite-node scripts/road-census.ts .artifacts/census)  # the census's CPU counts and samples
flock /tmp/trailview-gpu.lock node trailview/census.mjs --gpu ../../.artifacts/census --ground DIR  # its GPU counts
```

The fault census counts over the fixture, 70 m inside its bounds. Each count prints
`{"count":…,"samples":…,"places":…}`; a place is a 10 m square with 2 or more samples.

| Count | A sample counts where |
| --- | --- |
| `gaps` | a point of a way's core (its side's outer edge, `sideOf`), on a 0.25 m grid, lies in no near face of a corridor; a band covers nothing |
| `objects-on-ways` | a point kind places a model whose foot stands in a way's core or a junction's corner |
| `short-sidewalks` | a sidewalk is under 50 m in all: one run on one side, on across each vertex into the next way's sidewalk on that side |
| `tears` | a corridor face, at a point of a 0.5 m grid that it covers, lies more than 10 cm under the drawn terrain |
| `lifts` | a corridor face lies more than 60 cm over the drawn terrain |
| `plants-in-water` | `kept_off` lets a plant stand at a point of a stream's or river's ribbon, a lake or a river area, on a 0.5 m grid |
| `torn-folds` | a corridor face turns over 100° from up or a neighbour |
| `torn-slivers` | a face is under 2 cm wide in plan and steps over 3 cm across |
| `torn-twists` | a quad's two faces turn 30° to 100° apart |

Tears and lifts compare with the drawn mesh everywhere, and leave out decks. `road-census.ts` gates the
first three and the torn faces (`census-baseline.json`): it fails past a gate.

`shoot.mjs` writes one PNG per view after `waitForIdle()`, `--pages` views at once (3 with `--gpu`,
else 1); lines come as views finish. Two runs can differ in a few pixels by a few levels: compare
their difference. `flight.mjs` prints the GPU time per pass, the CPU selection time, and
the nodes, triangles and pages of each flight of `flights.json`. The `ground` pass runs only when ground pages
arrive: read its `amortized` time. `--gpu` draws 2560 × 1600 pixels with exact timestamps. With
`--resolve MS`, a flight whose `resolve` median drifts prints `valid: false`. `zoom-out` always reads
invalid: its pass is too small to hold the clock, so only walk and drive count. With `--tiles` or
`--features`, build the page with a config whose `bounds` are its box. `--cracks 10` counts the red
pixels of every tenth frame with `debug=cracks`; such a flight looks down on known terrain only, as
`cracks.json` does.

`--ground DIR` serves the bakes of `tools/trailview` and `tools/planner_snow.py` as the tile service
does: `DIR/rules.json`, `DIR/{ground,trailview-canopy}.pmtiles`, `DIR/snow.pmtiles` and
`DIR/colour/<variant>/{q1,q2,q3,q4}.pmtiles`; `--colour` picks the variant (`webp-12`). Build
the page with a config whose `layers` name `https://tiles.test/<name>.json` and whose
`trailview_rules` is `https://tiles.test/trailview/rules.json`.

`skyline.mjs` draws each photograph of `peak-view-photos` and prints the RMS of the drawn skyline
against the photograph's, per quarter-degree column. It exits 1 past an `rms_limit_deg`.

## Test

```sh
npx vitest run src/lib/trailview
npm run check
npm run check:wgsl          # cargo install naga-cli --version 30.0.1 --locked
```

`check:wgsl` composes each file with its `// #include NAME` lines, as `gpu/wgsl.ts` does.

## The fixture

`tests/browser/trailview/fixture/` holds the planner terrain of a box at Engelberg, its feature
tiles and its config. `box.sh` made the terrain. To bake the features on it, from GLO-30
square N46 E008 in DIR, with `make-fixture.py` as FIX:

```sh
osmium extract -b 8.34961,46.73986,8.48145,46.83013 switzerland.osm.pbf -o osm.pbf
cargo run --release --locked -p obc-dem --features terrarium --bin planner-dem -- \
  --dem DIR --bounds 8.37,46.765,8.46,46.83 --output terrain.mbtiles
uv run --locked --group planner-maps python FIX overlay terrain.mbtiles
cargo run --release --locked -p obc-trailview-features -- --osm osm.pbf \
  --terrain terrain.mbtiles --bounds 8.37,46.765,8.46,46.83 --output features.pmtiles
uv run --locked --group planner-maps python FIX features features.pmtiles
```

## Pitfalls

- SwiftShader times are not GPU times. Measure on a GPU.
- A GPU lowers its clock when a frame has time to spare, so a pass measures long;
  `flight.mjs --gpu` keeps the clock high.
- Tag a browser test that draws for minutes on SwiftShader `{ tag: '@gpu' }`: pull requests skip
  it; `web.trailview-gpu` runs it on a GPU.
- Headless Chromium on Linux with no GPU shows a WebGPU canvas only with
  `--use-vulkan=swiftshader` (`SWIFTSHADER` in `harness.js`). Else the device is lost: "A valid
  external Instance reference no longer exists".
- Chrome rounds timestamps to 100 µs unless it runs with `--enable-webgpu-developer-features`.
- `OBC_CHROMIUM` names a Chromium for the scripts and the smoke when Playwright's own is absent.
- `OBC_BROWSER_WS` names a Playwright browser server for the scripts.
- Vitest needs the generated WASM: `npm run build:wasm`.
- No shader uses NaN: compilers may assume that there is none. An unknown height is `NONE`.
- Surfaces write linear light; the resolve calls `finish()`. Pixel sizes (`dpdx`, minimum widths)
  multiply by `look.screen.z`.
- A still view shows the mean of 16 jittered frames; only new pages, nodes or extras restart them.
- Debug colours lose bits in the `rg11b10ufloat` frame (2–4/255 near 1).
- A finer node meets a coarser one only on the LOD plateau: keep `lod` of `budget.ts`
  (`lod.test.ts`).
- The page pools have a fixed size. A view that wants more pages lowers `quality` in the HUD.
- Never let a synthesis read a stamped page: a stored page keeps its raw layer and draws a copy.
- The ground pool has a fixed size too: the farthest pages past it draw from their cover, two levels
  coarser (`short` in the HUD).
- A new date builds every ground page again (the rules, the Sentinel-2 quarters and the snow are
  part of a page) and loads most tile bodies again: a view's bodies exceed their cache (`caps.groundBodies`).
- Mountain shadows need the `trailview-ground` layer.
- The ground noise is in frame metres about the first view: far from it, float32 coarsens its
  finest grains, and a view that starts elsewhere draws other grains. Compare shots of one first
  view.

## The still function

A page that does not show the 3-D view can still use one image of it: the cover of a route or a ride, or a home view. `still()` in [`still.ts`](still.ts) starts an engine of its own on a canvas that the page does not show. It waits until the view is complete, takes the image, and ends the engine. The image is WebP, or JPEG in a browser that cannot encode WebP. The still also gives the credits of the release and of the layers that the engine opened for the view, so that a page shows them with the image.

The script `home-views.mjs` in `builder/web/tests/browser/trailview/` renders the home views with the still function on a GPU of the owner, and uploads them as one set ([specs/home-views.md](../../../../../specs/home-views.md)).

The script `route-pictures.mjs` beside it renders the cover of each route of the route catalog in the same way, with `coverView()` and the still function. It writes the image and the place of each point of the route's line, and makes a row image of half the size of each image for the list rows. The key of a cover comes from the line, the size and the camera rule, not from the month of the image, so a new planner release needs a render only of the routes whose line changed ([specs/route-pictures.md](../../../../../specs/route-pictures.md)). Its command `check` reads only public files and names each route of the production release that has no cover in the live set, for example a route whose line changed or a new record.

For a line that the caller gives, the still also gives the place of each point in the image, its distance from the eye, and whether it is visible. The engine puts each point on its drawn terrain. A point is hidden only when the drawn terrain rises above the line of sight from the eye to the point: the engine reads the terrain height on that line, in steps that grow with the distance from the eye. Trees, houses, bridges and water do not hide a point. So a page draws the line on the image with no depth image and no camera values.

`coverView()` in [`still-view.ts`](still-view.ts) chooses the view of a cover before the render, from at most four terrain tiles of the release. For each heading that does not look toward the sun, it finds the lowest tilt that shows 85 % of the points of its lines. Of these views, it takes the one that draws the route longest on the screen.

- The caller starts no still while another engine draws: each engine holds hundreds of megabytes of GPU memory.
- A still fails when a tile fails to load, so no still has holes.
- A browser with only a software renderer renders no still (`canRenderStills()`): one still takes minutes there.

## The route lines

The engine draws routes on the drawn terrain: the one route of a plan in the planner, or several routes at once. `showRoutes()` in [`engine.ts`](engine.ts) takes a list of routes. A route has a line, a colour for each edge, and can have a height profile. The number of a route is its place in the list, from 1. A route with fewer than two points draws nothing and keeps its number.

All routes are one triangle strip in one buffer ([`route/route.ts`](route/route.ts)), so the CPU does no work for the line in each frame. Each point of the buffer holds the number of its route. Between two routes, the buffer has two break points with the number 0: a copy of the last point of the one route, and a copy of the first point of the next. The shader ([`route/route.wgsl`](route/route.wgsl)) gives a break point no width, so no line joins two routes. For the direction of the line at a point, the shader uses a neighbour point only when the neighbour has the same number. Each route has round ends: the buffer repeats the first and the last point of a route, and the shader moves each copy half the line width beyond the end and cuts a half circle from the part between.

`boldRoute()` draws one route bold: 2.5 times as wide, and on top of the others where routes share a path. The engine then draws the strip two times in the same pass. The first time it draws all routes but the bold route, and the second time only the bold route. So the bold route draws one time, and its light does not add up. A new list of routes has no bold route.

`routeAt()` gives the number of the route at a point of the canvas, or none. It reads nothing from the GPU, so it answers at once. Each route with a profile has pick points: each vertex of its line, and each entry of its profile between two vertices, at the height that the profile gives there (`profile_m` of [specs/route-catalog.md](../../../../../specs/route-catalog.md)). The engine projects the pick points to the screen with the camera of the last frame, without the jitter (the small shift of each frame that smooths the edges). Of a segment that passes behind the eye, it keeps the part in front of the eye. [`route/pick.ts`](route/pick.ts) takes the route that has a segment between two pick points within 12 pixels of the point. Of several routes, the bold route wins, then the route nearer to the eye.

- `routeAt()` does not test whether the terrain hides a pick point. So a route behind a ridge answers when no nearer route is at the point.
- A route with no profile is never found.

The controls ([`frame/controls.ts`](frame/controls.ts)) tell a click from a look around. A look around starts only after the pointer moves more than 4 pixels from the press. A release of the primary button before that, within 300 ms of the press, is a click. Another button looks around at once and never clicks. The options `onclick` and `onhover` of the start of the engine get each click, and each move with no button at most once in each display frame. The page then asks `routeAt()` for the route there.

`coverView()` also takes several lines. It leaves out a line with a point outside the bounds of the release. Each line gets its share of the 200 points that the search tests, by its length, and at least 3. The length of a view is the sum of the drawn lengths of the lines.
