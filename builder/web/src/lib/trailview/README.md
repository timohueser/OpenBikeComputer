# TrailView

TrailView is the 3D view of the planner. The engine is TypeScript with raw WebGPU and WGSL, and
it uses no framework. The page is `builder/web/trailview.html`. The terrain is naive: one mesh per
Terrarium tile of the planner terrain, only to give pixels.

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
| `gpu/` | The device and its pools, the GPU time per pass |
| `terrain/` | The naive terrain and its shader |
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
| `debug` | `normals` or `lod` |
| `lab` | `name:variant`, a list with commas |

Ctrl+C copies the view as an address, and Ctrl+V shows a pasted address. W A S D or the arrows
move, Q and E sink and rise in flight, Shift runs, F walks or flies, a drag looks and the wheel
zooms. H hides the HUD.

## Labs

A lab is an entry in `LABS` of `labs.ts`, with its variants and the default first. The engine
builds the variant that `?lab=` selects.

## Shots and flights

From `builder/web/tests/browser`:

```sh
npm run pretest:trailview                  # the page against the fixture
node trailview/shoot.mjs [--views FILE] [--out DIR] [--hud] [--gpu]
node trailview/flight.mjs --gpu [--flights FILE]
npm run test:trailview                     # the smoke
```

`shoot.mjs` writes one PNG per view of `views.json` after `waitForIdle()`. Two runs give the same
bytes. `flight.mjs` prints the GPU time per pass over the flights of `flights.json`. Run it on
the M1 Pro with `--gpu`: Chrome then draws 2560 × 1600 pixels and reports exact timestamps.

## Checks

```sh
npx vitest run src/lib/trailview
npm run check
npm run check:wgsl          # cargo install naga-cli --version 30.0.1 --locked
```

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
- Headless Chromium on Linux with no GPU shows a WebGPU canvas only with
  `--use-vulkan=swiftshader` (`SWIFTSHADER` in `harness.js`). Without it, the device is lost with
  "A valid external Instance reference no longer exists".
- Chrome rounds timestamps to 100 µs unless it runs with `--enable-webgpu-developer-features`.
- `OBC_CHROMIUM` names a Chromium for the scripts and the smoke when Playwright's own is absent.
- Vitest needs the generated WASM: `npm run build:wasm`.
