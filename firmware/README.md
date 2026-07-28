# OBC firmware (Rust)

The OpenBikeComputer Rust workspace: the device application and a desktop
simulator share **one** rendering path for `.obcm` maps. This file is the
**build / test / dev-loop guide** — for *how the system works* (crate graph,
render pipeline, formats, UI) read the docs site:
<https://timohueser.github.io/OpenBikeComputer/>. Per-crate roles are tabulated
in the [repo README](../README.md#repository-layout).

The host workspace (`Cargo.toml`, at the repo root) builds the shared `no_std` crates
(`obc-formats`, `obc-ports`, `obc-map-scene`, `obc-reader`, `obc-route`, `obc-render`, `obc-app`), the desktop simulator
(`obc-sim`), the website's wasm demo host (`obc-web-demo`, plus the host glue
both simulator hosts share in `obc-host-core`), the web builder's wasm
conversion bridge (`obc-web-convert`), the map packer (`obc-pack`),
and the test/host helpers.

Three crates are **`exclude`d** from that workspace and built on their own, each
because it drags a toolchain the rest has no use for:

| Crate | Why it stands alone |
| :-- | :-- |
| [`obc-fw-nrf54l`](obc-fw-nrf54l/README.md) | the real board: its own MCU target + `.cargo/config.toml` |
| [`obc-boot`](obc-boot/README.md) | the 32 KB bootloader, same target, its own link script |
| [`obc-desktop`](obc-desktop/README.md) | the Tauri app: a platform webview (WebKitGTK on Linux) |

Each has its own `Cargo.lock` and needs its own `fmt` / `clippy` / `test`
invocation — and its own CI job.

## Prerequisites

| For… | You need |
| :-- | :-- |
| Anything Rust | A stable toolchain (`rustup`). |
| The desktop simulator | Just Rust — the GUI is pure eframe/egui, **no SDL/Homebrew**. |
| The packer (`obc-pack`) | System **GEOS ≥ 3.14** (`brew install geos`) — its only native dependency. Multi-`.pbf` merge, `--bbox` and the land-dataset download all run in-process; no `osmium`, `curl` or `unzip`. |
| The desktop app (`obc-desktop`) | **Not** GEOS — it builds a vendored copy in, so it needs **CMake** and a C++ compiler instead. Plus Node for the frontend it embeds. Linux also wants WebKitGTK — see [its README](obc-desktop/README.md). |
| Compiling the shared crates for the device | `rustup target add thumbv8m.main-none-eabihf`. |

## Build

```sh
# From this directory. Builds the simulator + shared crates + packer for the host.
cargo build --release        # → target/release/{obc-sim, obc-pack}

# Confirm the shared stack still compiles for the nRF54L application core:
cargo build -p obc-app --target thumbv8m.main-none-eabihf
```

The board crate is built from **inside** its own directory (its target is
discovered from `.cargo/config.toml` by the working directory, not by
`--manifest-path` — building it via `--manifest-path` from here
silently targets the host and fails):

```sh
cd obc-fw-nrf54l && cargo build --release    # see that crate's README to flash

# The BLE build (companion-app link): the same firmware with the nrf-sdc + TrouBLE stack
# folded in. The board crate README has the pins, flashing, and on-glass verify.
cd obc-fw-nrf54l && cargo build --release --no-default-features --features ble
```

The host-tested, radio-free BLE core (`obc-ble`) is a normal workspace member, so the
`cargo test` below already exercises it. The wire contract those bytes cross to the phone
is [`obc-ble-interface-spec.md`](../obc-ble-interface-spec.md); the concepts are on the docs
site under [the companion link](https://timohueser.github.io/OpenBikeComputer/software/companion-link/).

## Test

```sh
cargo test            # the whole host workspace
cargo test -p obc-pack    # just the packer (fixtures under ../packer/tests/corpus/)
```

`cargo test` does **not** touch the excluded board crate.

### Render benchmark + pixel-hash tripwire

`obc-bench` renders seven fixed scenes (riding / mid / overview × north-up /
rotated, plus route) through the real reader → renderer pipeline over a deterministic
fixture and prints per-stage timings plus a frame hash per scene. CI re-renders
them and fails if any hash drifts from the committed golden file — timings are
printed but never gated.

```sh
cargo run -p obc-bench --release                                  # the timing/hash table
cargo run -p obc-bench --release -- --check obc-bench/hashes.txt  # what CI runs
cargo run -p obc-bench --release -- --repeat 9                    # stable local timing sample
```

A pure refactor must leave the hashes untouched. An **intentional** rendering
change regenerates the golden file in the same PR (that's the review signal):

```sh
cargo run -p obc-bench --release -- --write-hashes obc-bench/hashes.txt
```

One-off runs against a real map:
`cargo run -p obc-bench --release -- --map ../freiburg.obcm --mpp 4 --heading 35`.

The frozen firmware resource numbers, dependency-direction contract, benchmark
reference host, and repeatable on-device capture procedure live in
[`docs/ARCHITECTURE_RESOURCE_BASELINE.md`](docs/ARCHITECTURE_RESOURCE_BASELINE.md). Read it
before approving a resource-baseline change: report-only firmware is diagnostic
and must never be flashed as the shipping artifact.

## Format

`rustfmt.toml` is committed (`max_width = 120`, `use_small_heuristics = "Max"`),
so let rustfmt own style — don't hand-format. Formatting takes **four
invocations**, and CI checks all of them (the workspace is a *virtual* manifest,
so `--all` is required or it formats nothing; the three excluded crates above are
skipped by `--all` and each needs its own):

```sh
cargo fmt --all                                    # the workspace
cargo fmt --manifest-path obc-fw-nrf54l/Cargo.toml # the board crate, separately
cargo fmt --manifest-path obc-boot/Cargo.toml      # the bootloader, separately
cargo fmt --manifest-path obc-desktop/Cargo.toml   # the desktop app, separately
```

## Run the simulator

`obc-sim` renders `.obcm` maps (which must be **v5**) through the exact code the
firmware runs. `../freiburg.obcm` is a current sample.

```sh
# Interactive: device look (240×320, 64 colors), 3× window scale. Drag to pan,
# scroll to zoom.
./target/release/obc-sim ../freiburg.obcm

./target/release/obc-sim ../freiburg.obcm --size 480x640 --scale 2  # bigger window
./target/release/obc-sim ../freiburg.obcm --true-color             # skip 64-color quantization
./target/release/obc-sim ../freiburg.obcm --gpx ../kandel.gpx      # replay a GPX as a fake GPS
./target/release/obc-sim ../freiburg.obcm --png out.png            # headless one-frame render
./target/release/obc-sim ../freiburg.obcm --screenshot gui.png     # capture the live GUI's first frame
```

Run `obc-sim --help` for the full flag set (routes/tracks folders, `--import`,
`--physical`/`--calibrate`, `--script`/`--boot`, headless `--center`/`--zoom`).
Packing maps and the web builder are covered in the [repo README](../README.md).

## Run the web demo (`obc-web-demo`)

The landing page's live demo is the same shared crates compiled to wasm behind a
small `obc_demo_*` API (no egui/wgpu — the page's JS owns the frame loop). Trunk
drives the build from the site config (`rustup target add wasm32-unknown-unknown`
+ `cargo install trunk` once):

```sh
# From the repo root. Dev server with rebuild-on-change:
trunk serve --config docs/Trunk.toml           # http://127.0.0.1:8080/

# The shipped, wasm-opt'd binary (what CI + Pages deploy build):
trunk build --release --config docs/Trunk.toml # → docs/dist/
```

The demo core is target-independent, so its unit tests run in the plain
`cargo test` above — no browser needed for the logic.

## Build the conversion bridge (`obc-web-convert`)

The hosted web builder converts routes client-side: `obc-web-convert` is a thin
wasm host over `obc-route`'s `gpx_to_obcr` / `track_to_gpx`, so a dropped GPX
becomes the *same* `.obcr` the device and the CLI produce. It is a normal
workspace member (its conversion core is target-independent and covered by the
`cargo test` above); only the shipping artifact is wasm.

Unlike the demo this is a **library** consumed by Vite, not an app Trunk
bundles, so it builds with `wasm-pack` (`cargo install wasm-pack` once):

```sh
# From packer/web_builder/frontend — writes src/lib/convert/pkg/ (gitignored).
npm run build:wasm
```

The frontend needs that output before `npm run check`, `npm test` or
`npm run build` will work: the TypeScript wrapper imports the generated
bindings, and the vitest suite converts the checked-in `protocol-vectors/`
fixtures through the wasm module and compares them **byte-for-byte** against the
native converter's checked-in output. CI does the same in its `wasm-convert`
job, which also enforces the bundle-size budget:

```sh
python3 tools/wasm_size_guard.py --pkg ../packer/web_builder/frontend/src/lib/convert/pkg
```

## Firmware update images (OBCU)

Field firmware updates (epic #615) ship as an **OBCU** container — a 64-byte header
plus the raw app image — dropped on the SD card as `/UPDATE.BIN`. The byte format is
[`OBCU_Spec.md`](../OBCU_Spec.md); the shared codec + boot-decision logic live in
`obc-dfu` (a `no_std` workspace member, host-tested by the `cargo test` above). The
producer is `obc-mkimage`.

The pipeline is **objcopy → wrap**. Strip the board ELF to a raw binary (vector table
first), then wrap it:

```sh
# From the board crate — its .cargo/config.toml selects the nRF54L target (see its README).
# cargo-binutils provides `cargo objcopy`; `-O binary` emits the raw image in LMA order.
cd obc-fw-nrf54l
cargo objcopy --release -- -O binary app.bin
# (equivalently, on the ELF: llvm-objcopy -O binary target/<triple>/release/obc-fw-nrf54l app.bin)

# Wrap into an OBCU container tagged with the build's git describe.
cargo run -p obc-mkimage -- wrap \
    --bin app.bin \
    --version "$(git describe --always --dirty)" \
    --out UPDATE.BIN

# Inspect: decode + verify both CRCs (non-zero exit if invalid).
cargo run -p obc-mkimage -- inspect UPDATE.BIN
```

`wrap` refuses an image over the app-slot limit (`MAX_IMAGE_LEN`, 1,480,000 bytes)
and **warns** if the binary's first word isn't a plausible initial stack pointer in
RAM (`0x2000_0000 … 0x2004_0000`) — a raw `.bin` starts with the vector table, so a
failed check usually means an ELF or a wrong-section-order strip slipped through.

Installing a staged `UPDATE.BIN` on the device is the app-side armer (S4, #619): copy
the file to the card root and trigger the install — from the S5 UI once it lands, or
today over the debug VCOM link (`dfu-install`; recipe in the
[board README](obc-fw-nrf54l/README.md#triggering-a-firmware-update-over-the-vcom-dfu-install-s4-619)).
The armer validates the file, snapshots the running image to `/ROLLBACK.BIN`, arms the
boot-state page, and resets into `obc-boot`, which does the actual flash
([its README](obc-boot/README.md) has the LED codes).
