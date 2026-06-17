# OBCM viewer (Rust)

A Rust workspace for viewing `.obcm` maps, sharing the rendering path with the
target firmware (nRF5340 + LS021B7DD02, via `embedded-graphics`).

- **`obcm/`** — `no_std + alloc` crate, the shared building block for both the
  simulator and the firmware. Two halves:
  - **reader** (`reader.rs`, `color.rs`): parses the OBCM **v3** LOD-pyramid
    format ([spec](../OBCM_Spec.md)) — header, styles, LOD table, per-LOD
    quadtree query + chunk decode, and `select_lod_for_mpp`. Dependency-light;
    the `render` feature is off here.
  - **renderer** (`render.rs`, feature `render`): the *shared rendering path* —
    `Viewport` projection, meters-per-pixel LOD selection, painter z-ordering,
    even-odd scanline polygon fill and line drawing — written generically over an
    `embedded-graphics` `DrawTarget` so the host and the MCU run identical drawing
    code. Compiles `no_std`.
- **`obcm-app/`** — `no_std` *application + hardware-abstraction layer* shared by
  the simulator and the firmware. Owns *what the device is doing* — the camera,
  the camera mode (follow-user / free), and the last known user fix — behind a
  small HAL: a `LocationSource` (GPS / control panel / GPX replay) and an
  `InputSource` (buttons). `App::render_frame` is the single per-frame entry point
  both hosts call; it drives `obcm`'s `Viewport` + `MapRenderer`. Builds for the
  nRF5340 bare-metal target (`thumbv8m.main-none-eabihf`).
- **`obcm-sim/`** — thin desktop *host shell* on **eframe/egui** (pure Rust, no
  SDL): the device-screen window renders `obcm-app` into an in-house `Framebuffer`
  (`DrawTarget`) blitted to a GPU texture at integer scale; mouse drag pans and
  scroll zooms (Free mode). It also owns the host-only `LocationSource`
  (`SimLocationSource`), PNG output, and the color policy. Defaults to the
  device's 240×320 / 64-color (RGB222) look so the preview matches the panel.

The dependency direction is `obcm-sim → obcm-app → obcm`; the firmware will be a
second host beside `obcm-sim`, reusing `obcm-app` and `obcm` unchanged.

## Building

Just Rust — the GUI host is pure Rust (eframe/egui), so **no SDL2/Homebrew setup
is needed** anymore:

```sh
cargo build --release
```

To check the shared crates still compile for the device, build them for the
nRF5340 application core:

```sh
rustup target add thumbv8m.main-none-eabihf
cargo build -p obcm-app --target thumbv8m.main-none-eabihf
```

## Running

Maps must be **v3** (`.obcm` files packed before the v3 migration won't load).
`../luxemburg.obcm` is a current v3 sample.

```sh
# Interactive, simulating the device (240x320, 64 colors), 3x window scale:
./target/release/obcm-sim ../luxemburg.obcm

# Larger window / different simulated resolution:
./target/release/obcm-sim ../luxemburg.obcm --size 480x640 --scale 2

# Full-color (skip the 64-color quantization) to compare:
./target/release/obcm-sim ../luxemburg.obcm --true-color

# Headless: render one frame to PNG (no window):
./target/release/obcm-sim ../luxemburg.obcm --png out.png

# Capture the live GUI's first composited frame, then exit (good for verifying
# the on-screen result without a window manager in the loop):
./target/release/obcm-sim ../luxemburg.obcm --screenshot gui.png
```

Interactive controls: drag to pan, scroll to zoom.

## Status / next steps

- Parses and renders **v3** LOD-pyramid maps: per-LOD quadtree query + chunk
  decode, meters-per-pixel layer selection, filled polygons with holes (even-odd
  scanline fill), weighted polylines, z-ordering, RGB565→RGB222 quantization.
  The full rendering path is shared (`obcm::render`) and compiles `no_std`.
- App + HAL layer (`obcm-app`) in place: a `LocationSource`-driven camera with
  follow/free modes, behind one `App::render_frame` entry point both hosts share.
  The eframe host drives it; the shared stack builds for the nRF5340 target.
- **Next (control panel):** a second egui viewport with center / heading / zoom
  controls writing into `SimLocationSource`, a Follow toggle, then a user-position
  marker and GPX replay (another `LocationSource`), then virtual buttons
  (`InputSource`).
- **Firmware:** allocation-free chunk decode (visitor API + `heapless`) for the
  MCU, then the nRF5340 front-end (embassy + LS021B7DD02 driver) as a second host
  beside `obcm-sim` — a real GPS `LocationSource`, GPIO `InputSource`, and a
  panel `DrawTarget`, reusing `obcm-app`/`obcm` unchanged.
