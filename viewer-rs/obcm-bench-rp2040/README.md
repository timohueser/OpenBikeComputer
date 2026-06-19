# obcm-bench-rp2040

Headless render-timing harness for the OBCM map pipeline on a **Raspberry Pi Pico / RP2040**.
No SD card, no display, no input — it bakes one cropped map tile into flash, runs
`MapRenderer::render` into a **counting null `DrawTarget`** (zero framebuffer RAM) at a few
representative zooms, and streams the per-frame microseconds over **USB-CDC serial**.

## Why this exists / how to read the numbers

The real target is the nRF54L (Cortex-M33 **with FPU**). The RP2040 is Cortex-**M0+** with **no
FPU** — the f32-heavy render path runs in software float, so expect it to be **~3–10× slower** on
the float-bound work than the nRF54L will be. Treat the numbers as a **conservative floor** and a
"shape" probe (how cost scales across LOD/zoom). "Fast enough even here" ⇒ strong green light.

This crate is **excluded** from the `viewer-rs` workspace (it builds for `thumbv6m-none-eabi`, not
the host) and pins its target via `.cargo/config.toml`. The pipeline crates (`obcm-render`,
`obcm-reader`) are unmodified `no_std`/no-heap — that's the whole point.

## One-time setup (macOS)

```bash
rustup target add thumbv6m-none-eabi
cargo install flip-link elf2uf2-rs     # flip-link = linker (stack-overflow guard); elf2uf2-rs = flasher
brew install tio                       # serial reader (or use `screen`)
```

## Build + flash

```bash
# 1. Put the Pico in BOOTSEL: hold the BOOTSEL button while plugging in USB.
#    A drive named "RPI-RP2" mounts.
# 2. From this directory:
cargo run --release        # builds → elf2uf2-rs -d copies the .uf2 to RPI-RP2 → Pico reboots & runs
```

If `elf2uf2-rs -d` can't find the drive, flash manually:
```bash
cargo build --release
elf2uf2-rs target/thumbv6m-none-eabi/release/obcm-bench-rp2040 bench.uf2
# then drag bench.uf2 onto the RPI-RP2 volume in Finder
```

## Read the timings

After it reboots, the Pico re-enumerates as a USB serial port:
```bash
ls /dev/tty.usbmodem*
tio /dev/tty.usbmodem*          # or: screen /dev/tty.usbmodem... 115200   (quit screen: Ctrl-A k)
```
Output repeats once per second, e.g.:
```
riding    4123 us | lod 2 | chunks   6 | drawn  812 | px  58210 (+76800 solid)
```
`us` = wall-clock render time (RP2040 1 µs timer). `px` = rasterized pixels consumed (proves the
rasterizer ran); `solid` = clear/fill area (counted, not iterated — see `null_target.rs`).
USB logs emitted before you attach the terminal are lost; it loops forever, so just reattach.

## Regenerating the baked tile

`fixtures/fr_small.obcm` is git-ignored (large + regenerable). It is a ~3×3 km crop of central
Freiburg, packed with the repo's Python pipeline. To rebuild it from the source `.pbf`:

```bash
osmium extract --bbox=7.83,47.982,7.87,48.008 --strategy=complete_ways --overwrite \
  -o /tmp/fr_small.osm.pbf <freiburg-regbez>.osm.pbf
cd <repo root>
.venv/bin/python obcm_pack.py /tmp/fr_small.osm.pbf config.json \
  viewer-rs/obcm-bench-rp2040/fixtures/fr_small.obcm
```
Keep the tile **< ~1.8 MB** so it fits the 2 MB flash alongside ~52 KB of code. The camera presets
in `src/main.rs` are centered on (7.85, 47.995) — keep them inside the crop bbox.

## Footprint (verified, `--release`, flip-link)

| Region | Used | Budget | Note |
| :-- | :-- | :-- | :-- |
| RAM `.bss` (renderer scratch) | ~199 KB | — | `MapRenderer` static, via `new_const()` |
| RAM `data+bss` total | ~206 KB | 264 KB | ~58 KB left for stack |
| Flash `text` (code + 1.6 MB tile) | ~1.64 MB | 2.0 MB | tile dominates; shrink bbox if over |

A full 240×320 framebuffer (150 KB RGB565) does **not** fit alongside the full scratch — that's why
this harness uses a null target. To measure pixel-store cost too, feature-gate smaller `MAX_*`
constants in `obcm-render` and add a small real framebuffer.

## Phasing

- **Phase 1 (this):** base map only — `MapRenderer::render`.
- **Phase 2:** full frame incl. route line + breadcrumb + chevrons via `App::render_frame`
  (add `obcm-app` + `obcm-route`, bake a `.obcr`, set a fixed `progress_m`).
