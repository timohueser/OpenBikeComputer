# TrailView

TrailView is the 3D view of the planner. The engine is TypeScript with raw WebGPU and WGSL, and
it uses no framework. The page is `builder/web/trailview.html`. The terrain streams with LOD from
the master terrain tiles: zooms 0 to 12 from the tile service, zoom 13 from its packs. The feature
tiles of `trailview-features` (specs/trailview-features.md) stamp the roads, paths and lakes into
each page as it loads.

## Build and run

From `builder/web`, against a local release:

```sh
obc data serve local                                    # prints VITE_PLANNER_CATALOG
VITE_PLANNER_CATALOG=URL npm run dev -- --mode trailview   # open /trailview.html
npm run build:trailview                                 # dist/trailview
```

The page reads the `active` entry of the planner catalogue, as the planner does.
`VITE_PLANNER_CONFIG` replaces the entry. A browser without WebGPU shows a notice.

## Layout

| Path | Holds |
| --- | --- |
| `engine.ts` | The frame loop, the view, `waitForIdle()`, the recovery from a lost device |
| `frame/` | The local metric frame, the walk and fly camera, the input |
| `gpu/` | The device and its pools, the GPU time per pass, the WGSL `// #include` |
| `stream/` | The stored terrain and its decode worker, the feature codec |
| `terrain/` | The LOD rules and the node selection, the page pools, `drawn-height.wgsl`, the mesh, the page compute, the probe; the stamps (`stamp-core.wgsl`, `stamp.wgsl`), their worker that bins the feature tiles (`bins.ts`), the feature mask and water height (`features.wgsl`), the invariant audit (`audit.wgsl`) |
| `labs.ts` | The variant registry |
| `view.ts` | The view in the address |
| `hud.ts` | The lab HUD |

## The address

| Parameter | Value |
| --- | --- |
| `lat`, `lon` | The eye, in degrees |
| `heading`, `pitch` | Degrees: heading clockwise from true north, pitch up |
| `eye` | Walk: metres above the ground, 1.6 by default |
| `alt` | Fly: metres above sea level, at most 100 000 |
| `date`, `time` | `YYYY-MM-DD` and `HH:MM`, the wall clock in Europe/Zurich |
| `weather` | `sunny`, `cloudy` or `rainy` |
| `debug` | `normals`, `lod`, `cracks` (red clear colour), `surface` (red where drawnHeight leaves the mesh by 5 cm), or `mask` (the features of the mask flat: roads by surface, water blue, conflicts red, fords cyan) |
| `lab` | `name:variant`, a list with commas |

Ctrl+C copies the view as an address, and Ctrl+V shows a pasted address. W A S D or the arrows
move, Q and E sink and rise in flight, Shift runs, F walks or flies, a drag looks and the wheel
zooms. H hides the HUD.

## Labs

A lab is an entry in `LABS` of `labs.ts`, with its variants and the default first. The engine
builds the variant that `?lab=` selects. The HUD has a menu per lab; a change loads the page again
at the same view.

| Lab | Variants |
| --- | --- |
| `terrain` | `cdlod`, `rings`: the LOD rule of the one quadtree; no geometry clipmap is built |
| `relief` | `bicubic`, `erosion`: the micro-relief of the synthesized levels |
| `pages` | `r32float`, `r16`: 16 bits with an offset and a scale per page |
| `stamps` | `runtime`, `baked`: the stamps on every page as it loads, or conditioning baked into the stored levels, which the synthesized levels upsample |

## Shots and flights

From `builder/web/tests/browser`:

```sh
npm run pretest:trailview                  # the page against the fixture
node trailview/shoot.mjs [--views FILE] [--out DIR] [--tiles FILE] [--features DIR] [--lab LAB] [--hud] [--gpu] [--audit]
node trailview/flight.mjs --gpu [--flights FILE] [--tiles FILE] [--features DIR] [--lab LAB] [--cracks N] [--audit N]
npm run test:trailview                     # the smoke
```

`shoot.mjs` writes one PNG per view of `views.json` after `waitForIdle()`. Two runs give the same
bytes. `flight.mjs` prints the GPU time per pass, the CPU selection time, the nodes, triangles and
pages over the flights of `flights.json`. Run it on the M1 Pro with `--gpu`: Chrome then draws
2560 × 1600 pixels and reports exact timestamps. `--tiles` serves a `planner-dem` MBTiles instead
of the fixture, and `--features` a folder of feature tiles `z/x/y.bin`; build the page with a config
whose `bounds` are its box. `--audit` prints `trailView.audit()` of each view; `--audit 10` adds
up the audit of every tenth frame of a flight, as over `audits.json`. `--lab terrain:rings` adds
the lab to each view. `--cracks 10` counts the red pixels of every tenth frame with
`debug=cracks`; such a flight looks down on known terrain only, as `cracks.json` does over the box.

## Checks

```sh
npx vitest run src/lib/trailview
npm run check
npm run check:wgsl          # cargo install naga-cli --version 30.0.1 --locked
```

`check:wgsl` composes each file with its `// #include NAME` lines, as `gpu/wgsl.ts` does.

## The fixture

`tests/browser/trailview/fixture/` holds the planner terrain of a box at Engelberg, from
Copernicus GLO-30 square N46 E008, its feature tiles with heights from that terrain, and its config.
To make it again, with OSM.pbf an extract that covers the box:

```sh
cargo run --release --locked -p obc-dem --features terrarium --bin planner-dem -- \
  --dem DIR --bounds 8.37,46.765,8.46,46.83 --output terrain.mbtiles
uv run --locked --group trailview python -m tools.planner_trailview_features --osm OSM.pbf \
  --terrain terrain.mbtiles --bounds 8.37,46.765,8.46,46.83 --output features.pmtiles
uv run --locked --group planner-maps python builder/web/tests/browser/trailview/make-fixture.py terrain.mbtiles features.pmtiles
```

## What bites

- SwiftShader times are not GPU times. Measure on a GPU.
- macOS lowers the GPU clock while a frame leaves the GPU time to spare, and a pass then measures
  much longer than its work. `flight.mjs --gpu` keeps the clock high with a compute load before each
  frame; a measurement without it is not comparable.
- Headless Chromium on Linux with no GPU shows a WebGPU canvas only with
  `--use-vulkan=swiftshader` (`SWIFTSHADER` in `harness.js`). Without it, the device is lost with
  "A valid external Instance reference no longer exists".
- Chrome rounds timestamps to 100 µs unless it runs with `--enable-webgpu-developer-features`.
- `OBC_CHROMIUM` names a Chromium for the scripts and the smoke when Playwright's own is absent.
- Vitest needs the generated WASM: `npm run build:wasm`.
- No shader uses NaN: compilers may assume that there is none. An unknown height is `NONE`.
- A finer node meets a coarser one only on the LOD plateau. Keep `RANGE.least` and the ring
  band of `lod.ts`; `lod.test.ts` holds both.
- The page pools and the water layers have a fixed size. A view that wants more of them lowers
  `quality` in the HUD.
- A stored page with stamps takes two layers: its raw heights for the syntheses below it, and a
  stamped copy that draws. Never let a synthesis read a stamped page.
- `bins.ts` mirrors the gates of `stamp-core.wgsl`; `bins.test.ts` holds their numbers equal.
- WebGPU allows 8 storage buffers per shader stage: the audit packs its node table into the stamp data.
- The audit counts only what the frame shows: a node at the frame edge can border culled nodes without pages.
