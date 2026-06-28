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
| `firmware/obc-reader/` | `no_std + alloc` — pure OBCM **v5** parsing (header, style table, LOD table, per-LOD quadtree query + chunk decode). Dependency-light. |
| `firmware/obc-route/` | `no_std` — the OBCR route reader **and** the GPX → OBCR converter. |
| `firmware/obc-render/` | `no_std` — the **shared rendering path**: `Viewport` projection, meters-per-pixel LOD selection, painter z-ordering, even-odd scanline polygon fill, weighted polylines, text. Generic over an `embedded-graphics` `DrawTarget` so host and MCU run identical drawing code. |
| `firmware/obc-app/` | `no_std` — the **application + hardware-abstraction layer**: camera, camera mode (follow-user / free), screen stack, input model, route tracking. One per-frame entry point (`App::render_frame`) both hosts call. Builds for `thumbv8m.main-none-eabihf`. |
| `firmware/obc-sim/` | Desktop **simulator host** (eframe/egui, pure Rust — no SDL): renders `obc-app` into a framebuffer at the device's 240×320 / 64-color look, plus a control panel, GPX replay, and headless capture. |
| `firmware/obc-pack/` | The **map packer** (Rust): OSM `.osm.pbf` → `.obcm` — ingest, multipolygon assembly, land generation, quadtree build, streaming serialize. |
| `packer/config.json` | Feature selection + styling — which OSM tags to keep, their colors, z-order, and per-LOD detail. The read-only **factory default**. |
| `packer/palette.json` | The device's 64-color (RGB222) gamut, offered as the web builder's default color picker so the editor and the panel agree. |
| `packer/web_builder/` | **Web builder** (FastAPI): pick regions on a map, edit styles, and build an `.obcm` in the browser — shells out to `obc-pack`. |
| `OBCM_Spec.md` / `OBCR_Spec.md` | The binary map / route format specifications. |
| `firmware/docs/`, `packer/docs/` | Design notes and handover docs (UI spec, rendering pipeline, line-style plans, packer port stages…). |

The crate dependency direction is `obc-sim → obc-app → obc-render → obc-reader`
(with `obc-route` shared alongside). The real nRF54L firmware will become a
*second host* beside `obc-sim`, reusing `obc-app` / `obc-render` / `obc-reader` /
`obc-route` unchanged.

---

## Prerequisites

| For… | You need |
| :-- | :-- |
| Building anything Rust | A stable Rust toolchain (`rustup`). |
| The packer (`obc-pack`) | System **GEOS** (`brew install geos`) — linked for multipolygon area assembly. Optionally the [`osmium`](https://osmcode.org/osmium-tool/) CLI on `PATH` (only used to merge/sort when you pass multiple `.pbf` inputs). |
| The desktop simulator | Just Rust — the GUI is pure eframe/egui, **no SDL/Homebrew setup**. |
| The web builder (optional) | Python 3.13 + the deps in `packer/requirements.txt`. |
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
firmware/target/release/obc-pack region.osm.pbf packer/config.json region.obcm
```

```
usage: obc-pack <pbf...> <config.json> <out.obcm> [--chunk-size N] [--no-land]
```

- **Multiple `.pbf` inputs** are merged + sorted (via `osmium`) before ingest.
- `--chunk-size N` sets the quadtree chunk payload target (default `4096`).
- `--no-land` skips coastline/land-polygon generation.
- LOD tiers and feature styling come from `config.json`. When `natural.land` is
  configured, coastline/land polygons are generated automatically — the global
  land-polygon dataset is downloaded and cached under `~/.cache/obcm/` on first
  use.

### `config.json` — features, styles, and LODs

`config.json` is the single source of truth for *what* gets packed and *how it
looks*:

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
`OBC_PACK_BIN` env var). Features:

- **Region picker** — browse the Geofabrik tree and select one or more extracts;
  builds stream live progress over server-sent events.
- **Bounding-box build mode** — draw a crop box on the map and the selected PBFs
  are cropped to it before packing, so you can target a small area precisely.
- **Style editor** — edit colors / z-order / weights / per-LOD detail, with the
  color picker defaulting to the device's 64-color gamut (`palette.json`).
- **Persistent edits** — `config.json` is the read-only factory default; your
  edits are auto-saved to `user_config.json` (gitignored) and persist between
  sessions. **Restore defaults** discards them.
- **Stylesheets** — **Export** / **Import** the current styling as a standalone
  `.json`, independent of any `.obcm`.
- Feature/category fields autocomplete from a curated OSM tag catalog
  (`packer/web_builder/static/osm_catalog.json`); any freeform tag still works.

---

## Viewing & simulating

`obc-sim` renders `.obcm` maps through the exact code path the firmware will run.
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
Statistics) driven by an encoder + Back input model, plus a control panel for
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
3. **Save** — the finished ride is exported back to a `.gpx` in the tracks
   folder.

---

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
shared stack (`obc-reader`, `obc-route`, `obc-render`, `obc-app`) runs `no_std`
on the device, streaming maps/routes from a microSD card and driving the panel
over SPI.

**Working now:** OBCM v5 packing (CLI + web builder); the shared LOD-pyramid
renderer (quadtree query, polygon fill with holes, weighted lines, z-ordering,
RGB565 → RGB222 quantization); the on-device UI (screen stack + encoder/Back
input); route loading with live map-matching, ride logging, and GPX export; and
the nRF54L firmware booting into the full load → ride → save loop on the DK (see
[`firmware/obc-fw-nrf54l`](firmware/obc-fw-nrf54l)).

**Next:** the reflective **LS021B7DD02** panel driver (in bring-up, with the
waveform backend moving onto the nRF54L's FLPR coprocessor); Settings / Shutdown
screens and a Ride-control rework; and richer line styling (dashed / two-color
lines — a future OBCM v6 — and road casing).

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
