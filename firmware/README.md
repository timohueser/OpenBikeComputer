# OBC firmware (Rust)

This directory holds the crates the device image reaches: the shared `no_std` render path, the
platform adapters, the board crate and the bootloader. The device application and the desktop
simulator share one rendering path for `.obcm` maps.

This file is the build, test and dev-loop guide for all the Rust in the repo. For how the system
works, read the docs site: <https://openbikecomputer.com/>. Per-crate roles are tabulated in the
[repo README](../README.md#repository-layout).

The workspace is rooted at the repo root (`../Cargo.toml`) and spans `firmware/`, `../host/` and
`../apps/` — one `Cargo.lock`, one `target/`. Only `firmware/` is device-reachable; that is the
rule `firmware/tools/check_dependencies.py` enforces. Dev-dependencies cross the boundary on
purpose, because a dev-dep never enters the `no_std` build, so `cargo test` wants GEOS.

Three crates are excluded from the workspace and built from **inside their own directory**, each
with its own `Cargo.lock`, `fmt`, `clippy`, `test` and CI job:

| Crate | Why it stands alone |
| :-- | :-- |
| [`obc-fw-nrf54l`](obc-fw-nrf54l/README.md) | the board: its own MCU target and `.cargo/config.toml` |
| [`obc-boot`](obc-boot/README.md) | the 32 KB bootloader, same target, its own link script |
| [`obc-desktop`](../apps/obc-desktop/README.md) | the Tauri app: a platform webview |

## Prerequisites

| For… | You need |
| :-- | :-- |
| Anything Rust | A stable toolchain (`rustup`). |
| The desktop simulator | Just Rust — the GUI is pure eframe/egui. |
| The packer (`obc-pack`) | System **GEOS ≥ 3.14** (`brew install geos`), its only native dependency. |
| The desktop app (`obc-desktop`) | **CMake** and a C++ compiler (it vendors GEOS), plus Node. Linux also wants WebKitGTK — see [its README](../apps/obc-desktop/README.md). |
| Compiling the shared crates for the device | `rustup target add thumbv8m.main-none-eabihf`. |

## Build

```sh
# From anywhere inside the repo. Builds the simulator, the shared crates and the packer.
cargo build --release        # → target/release/{obc-sim, obc-pack}

# Confirm the shared stack still compiles for the nRF54L application core.
cargo build -p obc-app --target thumbv8m.main-none-eabihf
```

The board crate is built from inside its own directory. Its target comes from
`.cargo/config.toml`, which cargo finds by working directory, so building it through
`--manifest-path` from here silently targets the host and fails:

```sh
cd firmware/obc-fw-nrf54l && cargo build --release    # see that crate's README to flash
```

## Test

```sh
cargo test                # the whole host workspace
cargo test -p obc-pack    # just the packer
```

`cargo test` does not touch the three excluded crates.

The frozen resource numbers are in [`tools/resource_baseline.json`](tools/resource_baseline.json),
enforced by [`tools/resource_guard.py`](tools/resource_guard.py). A build with
`--features resource-report` is diagnostic and must never be flashed or packaged.

### Render benchmark and golden gate

`obc-bench` renders seven fixed scenes through the real reader and renderer over a deterministic
fixture, and prints per-stage timings, a frame hash and the map read path's counters. `--check`
also runs the nine route-corridor cases and fails if any frame hash or read counter drifts from
`host/obc-bench/golden.txt`. Timings are printed but never gated.

```sh
cargo run -p obc-bench --release                                       # the table
cargo run -p obc-bench --release -- --check host/obc-bench/golden.txt  # what CI runs
cargo run -p obc-bench --release -- --repeat 9                         # stable timing sample
cargo run -p obc-bench --release -- --corridor                         # the corridor matrix alone
cargo run -p obc-bench --release -- --write-golden host/obc-bench/golden.txt
```

A pure refactor must leave the golden file untouched. An intentional rendering or cache change
regenerates it in the same pull request, with the reason stated.

## Format

`rustfmt.toml` is committed, so let rustfmt own style. Formatting takes four invocations, and CI
checks all of them: the workspace is a *virtual* manifest, so `--all` is required or it formats
nothing, and `--all` skips the three excluded crates.

```sh
cargo fmt --all                                             # the workspace
cargo fmt --manifest-path firmware/obc-fw-nrf54l/Cargo.toml
cargo fmt --manifest-path firmware/obc-boot/Cargo.toml
cargo fmt --manifest-path apps/obc-desktop/Cargo.toml
```

`obc fmt` runs all four.

## Run the simulator

`obc-sim` renders a packed `.obcm` map through the exact code the firmware runs. Pack one with
`obc pack <region.osm.pbf>`.

```sh
./target/release/obc-sim map.obcm                      # device look, 240×320, 3× window scale
./target/release/obc-sim map.obcm --size 480x640 --scale 2
./target/release/obc-sim map.obcm --gpx ride.gpx       # replay a GPX as a fake GPS
./target/release/obc-sim map.obcm --png out.png        # headless one-frame render
```

See the [simulator guide](../apps/obc-sim/README.md) or `obc-sim --help` for the rest.

## Run the web demo (`obc-web-demo`)

The landing page's live demo is the same shared crates compiled to wasm. Trunk drives the build
(`rustup target add wasm32-unknown-unknown` and `cargo install trunk` once):

```sh
trunk serve --config docs/Trunk.toml            # http://127.0.0.1:8080/
trunk build --release --config docs/Trunk.toml  # → docs/dist/, what CI and Pages deploy
```

## Build the web builder's wasm bridges

The hosted builder has no backend, so `obc-web-convert` (GPX → `.obcr`) and `obc-web-assemble`
(OBCA cells → one map) run as wasm in the tab. They are libraries consumed by Vite, so they build
with `wasm-pack` (`cargo install wasm-pack` once):

```sh
# From builder/app — writes src/lib/{convert,assemble}/pkg/ (gitignored).
npm run build:wasm            # both; :convert / :assemble build one
```

The frontend needs that output before `npm run check`, `npm test` or `npm run build` will work.
CI's `wasm-bridges` job does the same and enforces the per-module bundle-size budgets:

```sh
# From the repo root.
python3 firmware/tools/wasm_size_guard.py --module convert
python3 firmware/tools/wasm_size_guard.py --module preview
python3 firmware/tools/wasm_size_guard.py --module assemble
```

## Firmware update images (OBCU)

A field update is an OBCU container a client uploads as an update-package object. The byte format
is [`OBCU_Spec.md`](../specs/OBCU_Spec.md); the codec and boot decision live in `obc-dfu`, and the
producer is `obc-mkimage`. The pipeline is objcopy, then wrap and sign:

```sh
# From the board crate, whose .cargo/config.toml selects the nRF54L target.
# cargo-binutils provides `cargo objcopy`; -O binary emits the raw image in LMA order.
cd obc-fw-nrf54l
cargo objcopy --release -- -O binary app.bin

# Wrap and sign. On a dev machine the committed test seed is the right key; CI passes
# --sign-seed-env instead.
cargo run -p obc-mkimage -- wrap \
    --bin app.bin \
    --version "$(git describe --always --dirty)" \
    --out UPDATE.BIN \
    --sign-seed ../obc-dfu/keys/test/obcu-test.seed

# Decode and verify both CRCs and the signature. Non-zero exit if invalid.
cargo run -p obc-mkimage -- inspect UPDATE.BIN
```

`wrap` refuses an image over `MAX_IMAGE_LEN` and warns if the binary's first word is not a
plausible initial stack pointer, which usually means an ELF or a wrong section order slipped
through.

**Signing is not optional on the device.** Without a seed, `wrap` emits an unsigned container and
the armer rejects it. `obc-mkimage sign` attaches the trailer to an already-wrapped container, so
an artifact can be built on one machine and signed on the one that holds the key; `keygen` makes a
keypair. Keys, the `OBCU_SIGNING_SEED` secret and the **rotation still owed before the first real
release** are in [`obc-dfu/keys/README.md`](obc-dfu/keys/README.md).

To install, upload the container and confirm on the device, or trigger the armer over the debug
VCOM link ([board README](obc-fw-nrf54l/README.md#driving-it-from-a-host-debug-uart)). The armer
validates the object, writes the running image into a rollback reserve, arms the boot-state page
and resets into `obc-boot` ([its README](obc-boot/README.md) has the LED codes).

## Terrain tiles (OBCT)

`obc-dem` turns Copernicus GLO-30 GeoTIFF into the `.obcd` artifact carried beside a map
([`OBCT_Spec.md`](../specs/OBCT_Spec.md)). It has no native dependency.

```sh
# The tiles a box needs, from the AWS Open Data mirror (~44 MB each).
cargo run --release -p obc-dem -- fetch \
    --bbox 46.48261,8.15034,46.72070,8.46007 --out /tmp/dem

# One .obcd per terrain cell — what a bakery publishes and a catalog names.
cargo run --release -p obc-dem -- bake --sources /tmp/dem \
    --bbox 46.48261,8.15034,46.72070,8.46007 --out cells/

# One .obcd over the whole box — the sidecar a rider carries beside a map.
cargo run --release -p obc-dem -- bake --sources /tmp/dem \
    --bbox 46.48261,8.15034,46.72070,8.46007 --cell-log2 16 --shard grimsel.obcd
```

**`--bbox` is latitude first** (`min_lat,min_lon,max_lat,max_lon`), the opposite of
`obc-pack --bbox`. For an Alpine box both numbers are plausible on either axis, so nothing catches
the mix-up.

`--posting-log2` and `--cell-log2` default to the v1 baked pairing. Both are OBCT header data, so
a different pairing is a re-bake, not a format change.

`fetch` is the only thing that touches the network. A bake is a pure function of a tile directory
and a box, and byte-identical output for identical inputs is a contract pinned by a digest test. A
source void becomes `NODATA` and nothing is ever inpainted.

Anything derived from GLO-30 must carry the Copernicus credit; `bake` prints it, and
`obc_elevation::COPERNICUS_ATTRIBUTION` is its single copy in the repo.

For a published catalog, use `obc bake terrain`, which drives this crate as a library over the
curated coverage (see the root [README](../README.md#baking-and-publishing-the-catalog)). The two
committed terrain sidecars are regenerated by
[`fixtures/build-map-package.sh terrain`](../fixtures/build-map-package.sh).
