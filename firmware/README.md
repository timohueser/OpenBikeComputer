# OBC firmware (Rust)

The OpenBikeComputer Rust workspace: the device application and a desktop
simulator share **one** rendering path for `.obcm` maps. This file is the
**build / test / dev-loop guide** — for *how the system works* (crate graph,
render pipeline, formats, UI) read the docs site:
<https://timohueser.github.io/OpenBikeComputer/>. Per-crate roles are tabulated
in the [repo README](../README.md#repository-layout).

The host workspace (`firmware/Cargo.toml`) builds the shared `no_std` crates
(`obc-reader`, `obc-route`, `obc-render`, `obc-app`), the desktop simulator
(`obc-sim`), the map packer (`obc-pack`), and the test/host helpers. The
**board crate** — `obc-fw-nrf54l` — is **`exclude`d** from the workspace (it has
its own MCU target + `.cargo/config.toml`) and is built on its own; see
[`obc-fw-nrf54l/README.md`](obc-fw-nrf54l/README.md) for the real-hardware target.

## Prerequisites

| For… | You need |
| :-- | :-- |
| Anything Rust | A stable toolchain (`rustup`). |
| The desktop simulator | Just Rust — the GUI is pure eframe/egui, **no SDL/Homebrew**. |
| The packer (`obc-pack`) | System **GEOS** (`brew install geos`); optionally `osmium` on `PATH` for multi-`.pbf` input. |
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

## Format

`rustfmt.toml` is committed (`max_width = 120`, `use_small_heuristics = "Max"`),
so let rustfmt own style — don't hand-format. Formatting takes **two
invocations**, and CI checks both (the workspace is a *virtual* manifest, so
`--all` is required or it formats nothing; the board crate is excluded, so
`--all` skips it):

```sh
cargo fmt --all                                    # the workspace
cargo fmt --manifest-path obc-fw-nrf54l/Cargo.toml # the board crate, separately
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
