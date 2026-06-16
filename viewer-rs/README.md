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
- **`obcm-sim/`** — thin desktop *host shell* on `embedded-graphics-simulator`
  (SDL2): window, pan/zoom event loop, PNG output, and the color policy. All map
  drawing is delegated to `obcm::render`. Defaults to the device's 240×320 /
  64-color (RGB222) look so the preview matches the panel.

## Building

Requires Rust and SDL2 (`brew install sdl2`). On Apple Silicon the linker needs
the Homebrew lib path:

```sh
export LIBRARY_PATH="/opt/homebrew/lib:$LIBRARY_PATH"
export DYLD_LIBRARY_PATH="/opt/homebrew/lib:$DYLD_LIBRARY_PATH"
cargo build --release
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
```

Interactive controls: drag to pan, scroll to zoom, `Esc`/`Q` to quit.

## Status / next steps

- Parses and renders **v3** LOD-pyramid maps: per-LOD quadtree query + chunk
  decode, meters-per-pixel layer selection, filled polygons with holes (even-odd
  scanline fill), weighted polylines, z-ordering, RGB565→RGB222 quantization.
  The full rendering path is shared (`obcm::render`) and compiles `no_std`.
- **Next:** allocation-free chunk decode (visitor API + `heapless`) and a
  scratch-buffer scanline fill for the MCU; the actual nRF5340 front-end (embassy
  + LS021B7DD02 driver) implementing a `DrawTarget` and calling `MapRenderer::render`.
