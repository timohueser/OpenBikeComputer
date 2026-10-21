# TrailView

TrailView is the 3D view of the planner. The engine is TypeScript with raw WebGPU and WGSL, and
it uses no framework. The page is `builder/web/trailview.html`. The terrain streams with LOD from
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

The page reads the `active` entry of the planner catalogue, as the planner does.
`VITE_PLANNER_CONFIG` replaces the entry. A browser without WebGPU shows a notice. The ground needs
`trailview_rules` and the `trailview-*` archives in `layers` of the entry; without them the ground
is the palette of the `world` region.

## Layout

| Path | Holds |
| --- | --- |
| `engine.ts` | The frame loop, the view, `waitForIdle()`, the recovery from a lost device |
| `frame/` | The local metric frame, the walk and fly camera, the input |
| `gpu/` | The device and its pools, the GPU time per pass, the WGSL `// #include` |
| `stream/` | The stored terrain and its decode worker, the feature codec, the ground layers and the worker that builds their pages |
| `terrain/` | The LOD rules and the node selection, the page pools, `drawn-height.wgsl`, the mesh, the page compute, the probe |
| `ground/` | The ground pages, `look.wgsl` (the ground shading of the prototype), the rules packing and the per-page compute |
| `light/` | The light keys and weather presets, the look at the eye, `shade.wgsl` (toon bands, haze, `finish()`), the sky |
| `rules/` | The regions of `rules.json`, resolved |
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
| `debug` | `normals`, `lod`, `cracks` (red clear colour), `surface` (red where drawnHeight leaves the mesh by 5 cm), the ground codes `material`, `lithology`, `region`, `snow` (white where snow lies on the date), or `albedo` (the ground colour without light) |
| `lab` | `name:variant`, a list with commas |

Ctrl+C copies the view as an address, and Ctrl+V shows a pasted address. W A S D or the arrows
move, Q and E sink and rise in flight, Shift runs, F walks or flies, a drag looks and the wheel
zooms. H hides the HUD.

## Labs

A lab is an entry in `LABS` of `labs.ts`, with its variants and the default first. The engine
builds the variant that `?lab=` selects. The HUD has a menu per lab; a change loads the page again
at the same view. The terrain has no lab: CDLOD by distance, bicubic synthesized levels, and 16-bit
pages with an offset and a scale per page.

## Shots and flights

From `builder/web/tests/browser`:

```sh
npm run pretest:trailview                  # the page against the fixture
node trailview/shoot.mjs [--views FILE] [--out DIR] [--tiles FILE] [--ground DIR] [--colour V] [--lab LAB] [--hud] [--gpu]
node trailview/flight.mjs --gpu [--flights FILE] [--tiles FILE] [--ground DIR] [--colour V] [--lab LAB] [--cracks N]
node trailview/skyline.mjs [--tiles FILE] [--lab LAB] [--gpu]
npm run test:trailview                     # every browser test; CI adds -- --grep-invert @gpu
```

`shoot.mjs` writes one PNG per view of `views.json` after `waitForIdle()`. Two runs give the same
bytes. `flight.mjs` prints the GPU time per pass, the CPU selection time, the nodes, triangles and
pages over the flights of `flights.json`. The `ground` pass runs only when ground pages arrive:
read its `amortized` time. Run it on the M1 Pro with `--gpu`: Chrome then draws
2560 × 1600 pixels and reports exact timestamps. `--tiles` serves a `planner-dem` MBTiles instead
of the fixture; build the page with a config whose `bounds` are its box. `--lab NAME:VARIANT`
adds the lab to each view. `--cracks 10` counts the red pixels of every tenth frame with
`debug=cracks`; such a flight looks down on known terrain only, as `cracks.json` does over the box.

`--ground DIR` serves the bakes of `tools/trailview` and `tools/planner_snow.py` as the tile service
does: `DIR/rules.json`, `DIR/{material,lithology,region,snow}.pmtiles` and
`DIR/colour/<variant>/{q1,q2,q3,q4,ndvimax}.pmtiles`; `--colour` picks the variant (`webp-12`). Build
the page with a config whose `layers` name `https://tiles.test/<name>.json` and whose
`trailview_rules` is `https://tiles.test/trailview/rules.json`.

`skyline.mjs` draws each photograph of `peak-view-photos` from 1.6 m above its `ground_m` and
prints the RMS of the drawn skyline against the photograph's, per quarter-degree column. It exits 1
when one is past its `rms_limit_deg`.

## Checks

```sh
npx vitest run src/lib/trailview
npm run check
npm run check:wgsl          # cargo install naga-cli --version 30.0.1 --locked
```

`check:wgsl` composes each file with its `// #include NAME` lines, as `gpu/wgsl.ts` does.

## The fixture

`tests/browser/trailview/fixture/` holds the planner terrain of a box at Engelberg, from
Copernicus GLO-30 square N46 E008, and its config. To make it again:

```sh
cargo run --release --locked -p obc-dem --features terrarium --bin planner-dem -- \
  --dem DIR --bounds 8.37,46.765,8.46,46.83 --output terrain.mbtiles
uv run --locked --group planner-maps python builder/web/tests/browser/trailview/make-fixture.py terrain.mbtiles
```

## What bites

- SwiftShader times are not GPU times. Measure on a GPU.
- macOS lowers the GPU clock while a frame leaves the GPU time to spare, and a pass then measures
  much longer than its work. `flight.mjs --gpu` keeps the clock high with a compute load before each
  frame; a measurement without it is not comparable.
- A browser test that draws for minutes on SwiftShader takes `{ tag: '@gpu' }`. Pull requests skip
  it, and `test-weekly.yml` runs it each Monday.
- Headless Chromium on Linux with no GPU shows a WebGPU canvas only with
  `--use-vulkan=swiftshader` (`SWIFTSHADER` in `harness.js`). Without it, the device is lost with
  "A valid external Instance reference no longer exists".
- Chrome rounds timestamps to 100 µs unless it runs with `--enable-webgpu-developer-features`.
- `OBC_CHROMIUM` names a Chromium for the scripts and the smoke when Playwright's own is absent.
- Vitest needs the generated WASM: `npm run build:wasm`.
- No shader uses NaN: compilers may assume that there is none. An unknown height is `NONE`.
- A finer node meets a coarser one only on the LOD plateau. Keep `RANGE.least` of `lod.ts`;
  `lod.test.ts` holds it.
- The page pools have a fixed size. A view that wants more pages lowers `quality` in the HUD.
- The ground pool has a fixed size too. A view that wants more ground pages draws the farthest of
  them from their cover, two levels coarser and never without the satellite colour; the HUD counts
  them as short.
- A new date builds every ground page again: the rules, the two Sentinel-2 quarters about the date
  and the snow are part of a page. The tile bodies of a view are more than the 96 MiB that the
  ground keeps, so most of them come again.
- The noise of the ground is in frame metres about the first view. Far from it, float32 coarsens
  the finest grains. A view that starts at another place draws another grain pattern at a spot, so
  compare screenshots of one first view.
