# OBC firmware (Rust)

The OpenBikeComputer (OBC) firmware, as a Rust workspace: the device application
and a desktop simulator share one rendering path for `.obcm` maps (target hardware
nRF54L + LS021B7DD02, via `embedded-graphics`).

- **`obc-reader/`** — `no_std + alloc` crate, the pure parsing logic. Parses the OBCM **v5**
  LOD-pyramid format ([spec](../OBCM_Spec.md)) — header, styles, LOD table, per-LOD
  quadtree query + chunk decode, and `select_lod_for_mpp`. Dependency-light.
- **`obc-render/`** — `no_std` crate, the *shared rendering path* —
  `Viewport` projection, meters-per-pixel LOD selection, painter z-ordering,
  even-odd scanline polygon fill and line drawing — written generically over an
  `embedded-graphics` `DrawTarget` so the host and the MCU run identical drawing
  code.
- **`obc-app/`** — `no_std` *application + hardware-abstraction layer* shared by
  the simulator and the firmware. Owns *what the device is doing* — the camera,
  the camera mode (follow-user / free), and the last known user fix — behind a
  small HAL: a `LocationSource` (GPS / control panel / GPX replay) and an
  `InputSource` (buttons). `App::render_frame` is the single per-frame entry point
  both hosts call; it drives `obc-render`'s `Viewport` + `MapRenderer`. Builds for the
  nRF54L bare-metal target (`thumbv8m.main-none-eabihf`).
- **`obc-sim/`** — thin desktop *host shell* on **eframe/egui** (pure Rust, no
  SDL): the device-screen window renders `obc-app` into an in-house `Framebuffer`
  (`DrawTarget`) blitted to a GPU texture at integer scale; mouse drag pans and
  scroll zooms (Free mode). It also owns the host-only `LocationSource`
  (`SimLocationSource`), PNG output, and the color policy. Defaults to the
  device's 240×320 / 64-color (RGB222) look so the preview matches the panel.

The dependency direction is `obc-sim → obc-app → obc-render → obc-reader`; the firmware will be a
second host beside `obc-sim`, reusing `obc-app`, `obc-render`, and `obc-reader` unchanged.

## Building

Just Rust — the GUI host is pure Rust (eframe/egui), so **no SDL2/Homebrew setup
is needed** anymore:

```sh
cargo build --release
```

To check the shared crates still compile for the device, build them for the
nRF54L application core:

```sh
rustup target add thumbv8m.main-none-eabihf
cargo build -p obc-app --target thumbv8m.main-none-eabihf
```

## Running

Maps must be **v5** (`.obcm` files in an earlier format version won't load).
`../freiburg.obcm` is a current v5 sample.

```sh
# Interactive, simulating the device (240x320, 64 colors), 3x window scale:
./target/release/obc-sim ../freiburg.obcm

# Larger window / different simulated resolution:
./target/release/obc-sim ../freiburg.obcm --size 480x640 --scale 2

# Full-color (skip the 64-color quantization) to compare:
./target/release/obc-sim ../freiburg.obcm --true-color

# Headless: render one frame to PNG (no window):
./target/release/obc-sim ../freiburg.obcm --png out.png

# Capture the live GUI's first composited frame, then exit (good for verifying
# the on-screen result without a window manager in the loop):
./target/release/obc-sim ../freiburg.obcm --screenshot gui.png
```

Interactive controls: drag to pan, scroll to zoom.

## Status

The full app runs on the desktop simulator; the shared stack (`obc-reader`,
`obc-route`, `obc-render`, `obc-app`) compiles `no_std` for the device target.

- **Map render** (`obc-render`) — per-LOD quadtree query + chunk decode,
  meters-per-pixel layer selection, even-odd polygon fill with holes, view-clipped
  weighted polylines, z-ordering, RGB565→RGB222 quantization.
- **App + HAL** (`obc-app`) — a `LocationSource`/`InputSource`-driven app behind one
  `App::render_frame` entry point: follow/free + heading-up camera, a screen stack
  (Home, Map with a pan mode, Menu, Route menu, Ride control, Statistics), an
  encoder + Back gesture recognizer, and a user-position marker.
- **Routes & tracking** (`obc-route`, the OBCR route format) — load a `.obcr`
  route, live map-matching (progress / off-route), actually-ridden distance / time
  / climb (barometer), a zoomable elevation profile, a recorded breadcrumb, and a
  ride log exported back to GPX.
- **Host** (`obc-sim`) — an eframe/egui control panel, GPX replay as a simulated
  GPS, 1:1 physical-size preview, and PNG / scripted headless capture.

**Next:** Settings / Shutdown screens and a Ride-control rework; richer line
styling (dashed / two-color lines — a future OBCM v6 — and road casing); then the
real device front-end (embassy + LS021B7DD02 driver, GPS / GPIO / storage) as a
second host beside `obc-sim`, deferred until the hardware is in hand.
