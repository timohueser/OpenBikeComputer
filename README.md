# OBC — OpenBikeComputer

<img width="483" height="692" alt="image" src="https://github.com/user-attachments/assets/a222e908-a12b-4e26-a53c-6c227f30005a" />


A from-scratch bikepacking computer: a custom binary map format, a Rust map
packer, a web-based map builder, and the device firmware + a desktop simulator
that share **one** rendering path. The target hardware is an **nRF54L** driving a
**LS021B7DD02** memory LCD (240×320, 64 colors), but everything here runs and is
developed on the desktop today.

**Docs & live demo:** the conceptual guide — system architecture, the render
pipeline, the data formats, the UI, the display protocol — lives at
<https://timohueser.github.io/OpenBikeComputer/>, which also runs the firmware's
own render path **live in your browser** (compiled to wasm).

```
  .osm.pbf  ──►  obc-pack  ──►  *.obcm (v5)  ─┐
 (OSM data)    (Rust packer)   (binary map)   │
                                              ├─►  obc-app  ──►  obc-sim   (desktop simulator, today)
  *.gpx     ──►  obc-route ──►  *.obcr        │   (shared        └─►  nRF54L firmware (on the DK)
 (a route)    (route import)  (binary route) ─┘    app + render)
```

The whole project is built around two compact binary formats — **OBCM** (`.obcm`,
maps: a self-contained LOD pyramid of quadtree-indexed geometry) and **OBCR**
(`.obcr`, routes: geometry + map-matching + an elevation profile) — designed to
be read *directly* off flash by a microcontroller, with no JSON, reparsing, or
heap churn. How they work: the
[data-formats guide](https://timohueser.github.io/OpenBikeComputer/software/formats/);
the normative byte layouts: [`OBCM_Spec.md`](OBCM_Spec.md) /
[`OBCR_Spec.md`](OBCR_Spec.md).

---

## Repository layout

| Path | What it is |
| :-- | :-- |
| `firmware/` | Rust workspace — the device app, the desktop simulator, the shared reader/renderer, and the map packer. See [`firmware/README.md`](firmware/README.md). |
| `firmware/obc-formats/` | Dependency-free `no_std` persistent-format authority — versions, fixed layouts, flags, sentinels, primitive codecs, and the shared byte-I/O seam. |
| `firmware/obc-ports/` | Dependency-free `no_std` semantic ports — fixes, sensor/input/settings traits, clocks, and recorded-track points shared without depending upward on app, platform, or host policy. |
| `firmware/obc-map-scene/` | Dependency-light `no_std` streamed map-scene seam — neutral bounds/styles plus allocation-free candidate/decode visitors shared by map sources and the renderer. |
| `firmware/obc-reader/` | `no_std + alloc` — pure OBCM **v5** parsing (header, style table, LOD table, per-LOD quadtree query + chunk decode). Dependency-light. |
| `firmware/obc-route/` | `no_std` — the OBCR route reader **and** the GPX → OBCR converter. |
| `firmware/obc-render/` | `no_std` — the **shared rendering path**: `Viewport` projection, meters-per-pixel LOD selection, painter z-ordering, even-odd scanline polygon fill, weighted polylines, text. Generic over an `embedded-graphics` `DrawTarget` so host and MCU run identical drawing code. |
| `firmware/obc-app/` | `no_std` — the **application layer**: camera, camera mode (follow-user / free), screen stack, input model, route tracking, plus compatibility re-exports of `obc-ports`. One per-frame entry point (`App::render_frame`) both hosts call. Builds for `thumbv8m.main-none-eabihf`. |
| `firmware/obc-sim/` | Desktop **simulator host** (eframe/egui, pure Rust — no SDL): renders `obc-app` into a framebuffer at the device's 240×320 / 64-color look, plus a control panel, GPX replay, and headless capture. |
| `firmware/obc-web-demo/` | The website's **live-demo host**: the same shared crates compiled to wasm behind a small `obc_demo_*` API — the landing page's JS owns the frame loop and canvas, no GUI framework in the tree. |
| `firmware/obc-host-core/` | Host glue **shared by the two simulator hosts** (desktop + web): GPX replay stepping, the frame-interleaved route planner, in-memory stores. |
| `firmware/obc-pack/` | The **map packer** (Rust): OSM `.osm.pbf` → `.obcm` — ingest, multipolygon assembly, land generation, quadtree build, streaming serialize. |
| `packer/presets/` | Style presets — complete packer configs (features + LODs + marker, plus a `_meta` block). `default.json` ("Bikepacking") is the read-only factory default; `minimal.json` and `high-detail.json` ship alongside. |
| `packer/palette.json` | The device's 64-color (RGB222) gamut, offered as the web builder's default color picker so the editor and the panel agree. |
| `packer/web_builder/` | **Web builder** (FastAPI): pick regions on a map, edit styles, and build an `.obcm` in the browser — shells out to `obc-pack`. |
| `firmware/obc-ble/` | `no_std` — the **BLE data-plane core** (epic #267): the S0 control-plane descriptor codecs, CRC-32, list objects, and the whole-object transfer state machine. Radio-free and host-tested; the board crate drives the L2CAP bytes through it. |
| `firmware/obc-dfu/` | `no_std` — the **SD-staged DFU core** (epic #615): the `OBCU` update-image container + boot-state page codecs, the bootloader's install engine (verify → flash → readback → trial/rollback), and the app-side armer. Host-tested with mock IO; both `obc-boot` and the board crate are thin drivers over it. |
| `firmware/obc-mkimage/` | Host tool (`wrap` / `inspect`) — prepends the 64-byte `OBCU` header to a raw app image to make an `UPDATE.BIN`, and decodes + CRC-verifies one. The release pipeline's image producer. |
| `firmware/obc-boot/` | The **32 KB nRF54L bootloader** — reads the `BOOT_STATE` page, runs `obc-dfu`'s install engine, flashes the app slot via RRAMC (LED codes, no display). Workspace-excluded + standalone like the board crate; flashed once. |
| `companion-ios/` | The **iOS companion app** (SwiftUI + the `OBCKit` package): import GPX/TCX, encode OBCR, and sync routes/rides with the device over BLE. |
| `protocol-vectors/` | Shared binary fixtures pinning the BLE wire contract — asserted byte-exact by both `cargo test` and `swift test`. |
| `OBCM_Spec.md` / `OBCR_Spec.md` / `OBCU_Spec.md` / `obc-ble-interface-spec.md` | The binary map / route / firmware-update-image format specifications and the BLE wire contract. |
| `firmware/docs/`, `packer/docs/` | Design notes and handover docs (UI spec, rendering pipeline, line-style plans, packer port stages…). |

The crate dependency direction includes `obc-sim → obc-app → obc-render → obc-map-scene`,
with `obc-reader → obc-map-scene` independently adapting streamed OBCM data. The
dependency-light `obc-formats`, `obc-map-scene`, and `obc-ports` foundations sit beneath the
reader/route, renderer, and app/route layers respectively.
The nRF54L firmware and the website's
`obc-web-demo` are *sibling hosts* beside `obc-sim`, reusing
`obc-app` / `obc-render` / `obc-reader` / `obc-route` unchanged.

---

## Prerequisites

| For… | You need |
| :-- | :-- |
| Building anything Rust | A stable Rust toolchain (`rustup`). |
| The packer (`obc-pack`) | System **GEOS** (`brew install geos`) — linked for multipolygon area assembly. Optionally the [`osmium`](https://osmcode.org/osmium-tool/) CLI on `PATH` — *only* to merge/sort when you pass multiple `.pbf` inputs. A single-input build, `--bbox` included, needs no osmium. |
| The desktop simulator | Just Rust — the GUI is pure eframe/egui, **no SDL/Homebrew setup**. |
| The web builder (optional) | Python 3.13 + the deps in `packer/requirements.txt`, and **Node 22+** for the one-time UI build (`npm ci && npm run build` in `packer/web_builder/frontend/`). |
| Checking the shared crates build for the device | `rustup target add thumbv8m.main-none-eabihf`. |

---

## Building

```sh
# The whole firmware workspace (simulator + shared crates + packer):
cd firmware && cargo build --release
```

This produces, in `firmware/target/release/`:

- `obc-sim` — the desktop simulator
- `obc-pack` — the map packer

To confirm the shared stack still compiles for the device target:

```sh
cargo build -p obc-app --target thumbv8m.main-none-eabihf
```

---

## Packing a map

Download an OSM extract (e.g. from [Geofabrik](https://download.geofabrik.de/)),
then:

```sh
firmware/target/release/obc-pack region.osm.pbf packer/presets/default.json region.obcm
```

```
usage: obc-pack <pbf...> <config.json> <out.obcm> [--bbox W,S,E,N] [--chunk-size N] [--no-land]
```

- **Multiple `.pbf` inputs** are merged + sorted (via `osmium`) before ingest.
- `--bbox W,S,E,N` crops the source to a box (degrees) **during ingest** — no
  `osmium extract` step and no temporary cropped `.pbf`. Ways crossing the
  boundary are kept whole (osmium's `complete_ways` strategy), so the map and
  the nav graph don't fray at the edge.
- `--chunk-size N` sets the quadtree chunk payload target (default `4096`).
- `--no-land` skips coastline/land-polygon generation.
- LOD tiers and feature styling come from `config.json`. When `natural.land` is
  configured, coastline/land polygons are generated automatically — the global
  land-polygon dataset is downloaded and cached under `~/.cache/obcm/` on first
  use.

### The config — features, styles, and LODs

The config JSON (any preset under `packer/presets/`, or your own file) is the
single source of truth for *what* gets packed and *how it looks*
(`obc-pack schema` prints its JSON Schema):

- **`lods`** — the LOD pyramid. Each tier has a `max_mpp` (meters-per-pixel
  ceiling, `null` = the coarsest/overview tier) and a `simplify` tolerance.
- **`features`** — an OSM `key → value → style` tree. Each style sets `color`
  (RGB565, chosen to land on the 64-color RGB222 grid), `z_index` (paint order),
  `weight` (line thickness), `min_lod` (the finest tier it first appears in), and
  `priority` (drop order when a chunk overflows).

### Web builder

For an interactive flow — select regions on a map, tweak styles against the live
device palette, watch build progress:

```sh
# from the repo root
.venv/bin/python -m packer.web_builder        # http://localhost:8000
```

It drives the `obc-pack` binary you built above (override its path with the
`OBC_PACK_BIN` env var). The UI is a Svelte app compiled once with Node
(`cd packer/web_builder/frontend && npm ci && npm run build`). Features:

- **Region picker** — click regions or search the Geofabrik tree; builds stream
  live progress over server-sent events and finish with a download link.
- **Bounding-box build mode** — draw a crop box on the map and the selected PBFs
  are cropped to it before packing, so you can target a small area precisely.
- **Style presets** — pick Bikepacking / Minimal / High detail on the main page
  (the files under `packer/presets/`); the **advanced editor** exposes every
  knob: per-feature colors / z-order / weights / per-LOD detail, LOD tiers, and
  output settings, with the color picker defaulting to the device's 64-color
  gamut (`palette.json`).
- **Your edits live in the browser** (localStorage) as "Custom — based on
  &lt;preset&gt;"; **Reset to preset** re-applies the shipped version. Nothing
  is stored server-side.
- **Export / Import** — the exported `.json` is a complete packer config,
  directly usable with the `obc-pack` CLI; old stylesheet exports import fine.
- Feature/category fields autocomplete from a curated OSM tag catalog; any
  freeform tag still works.

Downloads, caches, and the build queue are env-configurable (all optional):

| Variable | Default | Meaning |
| :-- | :-- | :-- |
| `OBCM_CACHE_DIR` | `~/.cache/obcm` | Geofabrik index, PBF downloads, land polygons. |
| `OBCM_OUTPUT_DIR` | `<cache>/builds` | Per-job build outputs, served by the download endpoint. |
| `OBC_PACK_BIN` | `firmware/target/{release,debug}/obc-pack` | Path to the packer binary. |
| `OBCM_MAX_CONCURRENT_JOBS` | `1` | Parallel packs (obc-pack is memory-hungry). |
| `OBCM_KEEP_JOBS` | `20` | Finished builds kept before the sweeper evicts by count/age. |

For frontend development, run the API (`.venv/bin/python -m packer.web_builder
--no-browser`) and `npm run dev` in `packer/web_builder/frontend/` side by side —
Vite proxies `/api` to port 8000.

---

## Viewing & simulating

`obc-sim` renders `.obcm` maps through the exact code path the firmware runs.
Maps must be **v5**.

```sh
# Interactive, simulating the device (240×320, 64 colors), 3× window scale:
firmware/target/release/obc-sim region.obcm

# Larger window / different simulated resolution:
firmware/target/release/obc-sim region.obcm --size 480x640 --scale 2

# Full color (skip the 64-color quantization) for comparison:
firmware/target/release/obc-sim region.obcm --true-color
```

**Interactive controls:** drag to pan, scroll to zoom, Esc/Q to quit. The
simulator boots to the device's Home screen and drives the full on-device UI —
a screen stack (Home, Map with a pan mode, Menu, Route menu, Ride control,
Statistics) driven by the four-button input model, plus a control panel for
feeding it a simulated GPS.

Useful flags:

| Flag | Purpose |
| :-- | :-- |
| `--gpx TRACK.gpx` | Preload a GPX and replay it as a simulated GPS (or drag a file onto the window). |
| `--routes-dir DIR` | Folder of `.obcr` routes the Route menu lists (device-SD stand-in; default `routes/`). |
| `--tracks-dir DIR` | Folder for saved ride `.gpx` + the in-progress log (default `tracks/`). |
| `--import GPX` | Convert a GPX into an `.obcr` route in the routes folder and exit (what the device does on a USB drop). |
| `--physical` / `--calibrate` | Render at the panel's true physical size — calibrate once with a ruler. |
| `--palette` | Show the device's 64-color gamut and nothing else (color test). |
| `--png OUT.png` | Headless: render one frame to PNG (no window). |
| `--screenshot OUT.png` | Launch the GUI, capture its first composited frame, then exit. |
| `--center LON,LAT` `--zoom MULT` | Aim the headless camera at a spot / zoom level (e.g. to inspect a chunk boundary). |
| `--script TOKENS` `--boot` | Drive a gesture script before a headless render (e.g. walk Home → Route menu → Map). |

The GUI host is pure Rust (eframe/egui) — no SDL or system libraries to install.
See [`firmware/README.md`](firmware/README.md) for the full flag reference.

---

## Routes & tracks

The route workflow mirrors what the device will do:

1. **Import** — a `.gpx` route is converted to a compact `.obcr` (`obc-route`,
   shared with the firmware). On the device this happens on a USB drop; in the
   simulator use `--import` or drag-and-drop.
2. **Ride** — pick a route from the Route menu. The Map draws it, and the app
   live **map-matches** the current fix (progress along route / off-route),
   tracks actually-ridden distance / time / climb (barometer), shows a zoomable
   elevation profile, and records a breadcrumb.
3. **Save** — the finished ride is stored durably: the simulator exports a
   `.gpx` to the tracks folder; the device writes the compact ride object the
   companion app syncs (and exports to GPX phone-side).

---

## The companion app

Routes are usually planned on a phone and rides are worth keeping, so a SwiftUI
**iOS companion app** ([`companion-ios/`](companion-ios)) syncs with the device
over **Bluetooth Low Energy** — push a route, pull a ride, rename the device,
read diagnostics. The phone does all the format conversion (GPX/TCX → OBCR) and
the device writes the bytes to its card verbatim. How the link is shaped — the
GATT control plane, the L2CAP data plane, pairing and reconnect — is
[the companion link](https://timohueser.github.io/OpenBikeComputer/software/companion-link/);
the normative byte contract is
[`obc-ble-interface-spec.md`](obc-ble-interface-spec.md), and its host-tested
core (descriptor codecs, CRC-32, the transfer state machine) is
[`firmware/obc-ble/`](firmware/obc-ble), which rides the normal `cargo test`.

The firmware's BLE support is a build-time variant of the board crate:

```sh
cd firmware/obc-fw-nrf54l
cargo build --release --no-default-features --features ble
```

Flashing, the dependency pins, and the on-glass verify steps live in the
[board crate README](firmware/obc-fw-nrf54l/README.md).

## Firmware updates

The device updates itself in the field — no probe needed. An update ships as a
single **`UPDATE.BIN`** file (an [`OBCU`](OBCU_Spec.md) container: a 64-byte
header plus the raw app image). The trust model — verify before erase, a single
trial boot with rollback — is the
[firmware updates](https://timohueser.github.io/OpenBikeComputer/software/firmware-updates/)
docs page.

**Installing one (on the device):**

1. Copy `UPDATE.BIN` to the **root** of the device's SD card (from any computer),
   or push it from the companion app over BLE.
2. On the device: **Settings → System → "Install update from card"**.
3. Confirm on the glass. The device validates the image, snapshots the running
   firmware, and reboots into the bootloader to flash it (its LED takes over while
   the display is off). If the new image doesn't come up healthy, the next boot
   rolls back automatically.

**Cutting a release (maintainers):** push a `v*` tag. The
[`release` workflow](.github/workflows/release.yml) builds the shipping firmware
(`obc-fw-nrf54l` `--features ble`) and the `obc-boot` bootloader, wraps the app
into `UPDATE.BIN` tagged with the version, gates on `obc-mkimage inspect`, and
attaches `UPDATE.BIN`, both ELFs, and a `SHA256SUMS.txt` to the GitHub release:

```sh
git tag v0.4.0
git push origin v0.4.0        # → the release + UPDATE.BIN appear on the Releases page
```

To dry-run the pipeline without tagging — validate that a candidate `UPDATE.BIN`
builds and passes `inspect` — trigger the workflow manually (**Actions → Release →
Run workflow**, with a version string, or `gh workflow run release.yml -f
version=v0.4.0-rc1`); it uploads the same artifacts as a downloadable bundle and
publishes no release. Building `UPDATE.BIN` by hand (the `objcopy → wrap`
pipeline) is in the [firmware README](firmware/README.md#firmware-update-images-obcu).

## Testing

```sh
cargo test -p obc-pack --manifest-path firmware/Cargo.toml   # the packer
cd firmware && cargo test                                    # the whole workspace
```

The `obc-pack` tests use fixtures under `packer/tests/corpus/` — the committed
`tiny/tiny.osm` plus a `config.json`. Regenerate the binary fixtures with
`packer/tests/corpus/build_corpus.sh` (needs `osmium`).

---

## Status & roadmap

The full app runs on the desktop simulator **and on the nRF54L15-DK** today: the
shared stack (`obc-map-scene`, `obc-reader`, `obc-route`, `obc-render`, `obc-app`) runs `no_std`
on the device, streaming maps/routes from a microSD card and driving the panel
over SPI.

**Working now:** OBCM v5 packing (CLI + web builder); the shared LOD-pyramid
renderer (quadtree query, polygon fill with holes, weighted lines, z-ordering,
RGB565 → RGB222 quantization); the on-device UI (screen stack + the four-button
input); route loading with live map-matching, ride logging, and ride saving; the
nRF54L firmware booting into the full load → ride → save loop on the DK (see
[`firmware/obc-fw-nrf54l`](firmware/obc-fw-nrf54l)); and the reflective
**LS021B7DD02** panel driver, its waveform backend running on the nRF54L's FLPR
coprocessor with partial / dirty-row updates.

**Next:** Settings / Shutdown screens and a Ride-control rework; and richer line
styling (dashed / two-color lines — a future OBCM v6 — and road casing).

> The packer was originally a Python pipeline; it has been ported to Rust
> (`firmware/obc-pack`) and the Python pipeline removed. The port's design notes
> live in `packer/docs/`.

---

## License

The **software** in this repository — the firmware, the `obc-*` crates, the Rust
packer, and the web map builder — is licensed under the **GNU GPL v3.0** (see
[`LICENSE`](LICENSE)). Use it for anything, commercial or not, as long as
derivatives stay open source under the same terms.

The **hardware** design files (the 3D-printed case, and any future board designs)
are licensed under **CERN-OHL-S v2** — strongly reciprocal; see
[`LICENSE.hardware`](LICENSE.hardware). It's the open-hardware counterpart to the
GPL: build it, modify it, even sell it, but share the editable source under the
same licence.
