# OBCM viewer (Rust)

A Rust workspace for viewing `.obcm` maps, sharing the rendering path with the
target firmware (nRF5340 + LS021B7DD02, via `embedded-graphics`).

- **`obcm/`** — `no_std + alloc` reader for the OBCM format (header, styles,
  quadtree query, chunk decode). The same crate is intended to compile for the
  MCU. Currently parses format **v2**; the v3 LOD table
  ([design](../docs/superpowers/specs/2026-06-16-obcm-lod-design.md)) is an
  additive parse step.
- **`obcm-sim/`** — desktop simulator built on `embedded-graphics-simulator`
  (SDL2). It renders through the *same* `embedded-graphics` primitives the
  firmware will use, and defaults to the device's 240×320 / 64-color (RGB222)
  look so the preview matches the panel.

## Building

Requires Rust and SDL2 (`brew install sdl2`). On Apple Silicon the linker needs
the Homebrew lib path:

```sh
export LIBRARY_PATH="/opt/homebrew/lib:$LIBRARY_PATH"
export DYLD_LIBRARY_PATH="/opt/homebrew/lib:$DYLD_LIBRARY_PATH"
cargo build --release
```

## Running

```sh
# Interactive, simulating the device (240x320, 64 colors), 3x window scale:
./target/release/obcm-sim ../monaco.obcm

# Larger window / different simulated resolution:
./target/release/obcm-sim ../monaco.obcm --size 480x640 --scale 2

# Full-color (skip the 64-color quantization) to compare:
./target/release/obcm-sim ../monaco.obcm --true-color

# Headless: render one frame to PNG (no window):
./target/release/obcm-sim ../monaco.obcm --png out.png
```

Interactive controls: drag to pan, scroll to zoom, `Esc`/`Q` to quit.

## Status / next steps

- Parses and renders v2 maps (filled polygons with holes via even-odd scanline
  fill, weighted polylines, z-ordering, RGB565→RGB222 quantization).
- **Next:** v3 LOD table parse + m/px level selection; allocation-free chunk
  decode (visitor API + `heapless`) for the MCU; the actual nRF5340 front-end
  (embassy + LS021B7DD02 driver) reusing the `obcm` crate and rendering code.
