# OBC firmware (Rust)

This directory holds the crates the **device image actually reaches** — the
shared `no_std` render path, the platform adapters, the board crate and the
bootloader. The device application and a desktop simulator share **one**
rendering path for `.obcm` maps.

This file is the **build / test / dev-loop guide** for all the Rust in the repo,
not just this directory — for *how the system works* (crate graph, render
pipeline, formats, UI) read the docs site:
<https://openbikecomputer.com/>. Per-crate roles are tabulated
in the [repo README](../README.md#repository-layout).

The workspace is rooted at the **repo root** (`../Cargo.toml`), and spans three
trees — one `Cargo.lock`, one `target/`:

| Tree | Holds | Reached by the device image? |
| :-- | :-- | :-- |
| `firmware/` | `obc-crc`, `obc-formats`, `obc-ports`, `obc-map-scene`, `obc-reader`, `obc-route`, `obc-render`, `obc-app`, `obc-ble`, `obc-dfu`, the platform adapters | **yes** — that is the rule |
| `../host/` | the packer (`obc-pack`), the bakery (`obc-bake`), the terrain baker (`obc-dem`), the cell assembler (`obcm-assemble`), `obc-mkimage`, `obc-bench`, the oracles (`obcm-testkit`, `obc-vectors`), `obc-host-core`, `obc-replay`, `obc-usb-host` | no |
| `../apps/` | `obc-sim`, `obc-web-demo`, `obc-web-convert`, `obc-web-assemble`, `obc-skin-preview`, `obc-desktop` | no |

Dev-dependencies cross that boundary on purpose — `obc-render` and `obc-reader`
test against `obcm-testkit`, `obc-route` against `obc-pack` — because a dev-dep
never enters the `no_std` build. `cargo test` therefore wants GEOS.

Three crates are **`exclude`d** from the workspace and built on their own, each
because it drags a toolchain the rest has no use for:

| Crate | Why it stands alone |
| :-- | :-- |
| [`obc-fw-nrf54l`](obc-fw-nrf54l/README.md) | the real board: its own MCU target + `.cargo/config.toml` |
| [`obc-boot`](obc-boot/README.md) | the 32 KB bootloader, same target, its own link script |
| [`obc-desktop`](../apps/obc-desktop/README.md) | the Tauri app: a platform webview (WebKitGTK on Linux) |

Each has its own `Cargo.lock` and needs its own `fmt` / `clippy` / `test`
invocation — and its own CI job.

## Prerequisites

| For… | You need |
| :-- | :-- |
| Anything Rust | A stable toolchain (`rustup`). |
| The desktop simulator | Just Rust — the GUI is pure eframe/egui, **no SDL/Homebrew**. |
| The packer (`obc-pack`) | System **GEOS ≥ 3.14** (`brew install geos`) — its only native dependency. Multi-`.pbf` merge, `--bbox` and the land-dataset download all run in-process; no `osmium`, `curl` or `unzip`. |
| The desktop app (`obc-desktop`) | **Not** GEOS — it builds a vendored copy in, so it needs **CMake** and a C++ compiler instead. Plus Node for the frontend it embeds. Linux also wants WebKitGTK — see [its README](../apps/obc-desktop/README.md). |
| Compiling the shared crates for the device | `rustup target add thumbv8m.main-none-eabihf`. |

## Build

```sh
# From the repo root (or anywhere inside it — cargo walks up to the workspace).
# Builds the simulator + shared crates + packer for the host.
cargo build --release        # → target/release/{obc-sim, obc-pack}

# Confirm the shared stack still compiles for the nRF54L application core:
cargo build -p obc-app --target thumbv8m.main-none-eabihf
```

The board crate is built from **inside** its own directory (its target is
discovered from `.cargo/config.toml` by the working directory, not by
`--manifest-path` — building it via `--manifest-path` from here
silently targets the host and fails):

```sh
cd firmware/obc-fw-nrf54l && cargo build --release    # see that crate's README to flash

# The BLE build (companion-app link): the same firmware with the nrf-sdc + TrouBLE stack
# folded in. The board crate README has the pins, flashing, and on-glass verify.
cd firmware/obc-fw-nrf54l && cargo build --release --no-default-features --features ble
```

The host-tested, radio-free BLE core (`obc-ble`) is a normal workspace member, so the
`cargo test` below already exercises it. The wire contract those bytes cross to the phone
is [`obc-ble-interface-spec.md`](../specs/obc-ble-interface-spec.md); the concepts are on the docs
site under [the companion link](https://openbikecomputer.com/software/companion-link/).

## Test

```sh
cargo test            # the whole host workspace
cargo test -p obc-pack    # just the packer (fixtures under builder/tests/corpus/)
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
cargo run -p obc-bench --release -- --check host/obc-bench/hashes.txt  # what CI runs
cargo run -p obc-bench --release -- --repeat 9                    # stable local timing sample
```

A pure refactor must leave the hashes untouched. An **intentional** rendering
change regenerates the golden file in the same PR (that's the review signal):

```sh
cargo run -p obc-bench --release -- --write-hashes host/obc-bench/hashes.txt
```

One-off runs against a real map:
`cargo run -p obc-bench --release -- --map freiburg.obcm --mpp 4 --heading 35`.

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
cargo fmt --manifest-path firmware/obc-fw-nrf54l/Cargo.toml # the board crate, separately
cargo fmt --manifest-path firmware/obc-boot/Cargo.toml      # the bootloader, separately
cargo fmt --manifest-path apps/obc-desktop/Cargo.toml       # the desktop app, separately
```

## Run the simulator

`obc-sim` renders `.obcm` maps (which must be **v12**) through the exact code the
firmware runs. `freiburg.obcm` in the repo root is a current sample.

```sh
# Interactive: device look (240×320, 64 colors), 3× window scale. Drag to pan,
# scroll to zoom.
./target/release/obc-sim freiburg.obcm

./target/release/obc-sim freiburg.obcm --size 480x640 --scale 2  # bigger window
./target/release/obc-sim freiburg.obcm --true-color             # skip 64-color quantization
./target/release/obc-sim freiburg.obcm --gpx kandel.gpx            # replay a GPX as a fake GPS
./target/release/obc-sim freiburg.obcm --png out.png            # headless one-frame render
./target/release/obc-sim freiburg.obcm --screenshot gui.png     # capture the live GUI's first frame
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

## Build the web builder's wasm bridges (`obc-web-convert`, `obc-web-assemble`)

The hosted web builder has no backend, so two things it needs run as wasm in
the tab, each a thin host over a shared crate:

- **`obc-web-convert`** — `obc-route`'s `gpx_to_obcr` / `track_to_gpx`, so a
  dropped GPX becomes the *same* `.obcr` the device and the CLI produce.
- **`obc-web-assemble`** — `obcm-assemble`, so downloaded OBCA cells become one
  map in the tab, verified (spec §4.8) before anything leaves it. The only one
  wrapping a `host/` crate rather than a firmware one.

Both are normal workspace members — their cores are target-independent and
covered by the `cargo test` above; only the shipping artifacts are wasm.

Unlike the demo these are **libraries** consumed by Vite, not apps Trunk
bundles, so they build with `wasm-pack` (`cargo install wasm-pack` once):

```sh
# From builder/app — writes src/lib/{convert,assemble}/pkg/ (gitignored).
npm run build:wasm            # both; :convert / :assemble build one
```

The frontend needs that output before `npm run check`, `npm test` or
`npm run build` will work: the TypeScript wrappers import the generated
bindings, and two vitest suites push checked-in fixtures through the wasm
modules and compare **byte-for-byte** against the native tools' checked-in
output — `specs/vectors/` for the converter, and the cell tree in
`apps/obc-web-assemble/tests/fixture/` for the assembler. CI does the same in
its `wasm-bridges` job, which also enforces the per-module bundle-size budgets:

```sh
# From the repo root; --pkg overrides the module's default location.
python3 firmware/tools/wasm_size_guard.py --module convert
python3 firmware/tools/wasm_size_guard.py --module preview
python3 firmware/tools/wasm_size_guard.py --module assemble
```

## Firmware update images (OBCU)

Field firmware updates (epic #615) ship as an **OBCU** container — a 64-byte header,
the raw app image, and an Ed25519 signature trailer (OBCU v2, #997) — dropped on the SD
card as `/UPDATE.BIN`. The byte format is
[`OBCU_Spec.md`](../specs/OBCU_Spec.md); the shared codec + boot-decision logic live in
`obc-dfu` (a `no_std` workspace member, host-tested by the `cargo test` above). The
producer is `obc-mkimage`.

The pipeline is **objcopy → wrap+sign**. Strip the board ELF to a raw binary (vector
table first), then wrap and sign it:

```sh
# From the board crate — its .cargo/config.toml selects the nRF54L target (see its README).
# cargo-binutils provides `cargo objcopy`; `-O binary` emits the raw image in LMA order.
cd obc-fw-nrf54l
cargo objcopy --release -- -O binary app.bin
# (equivalently, on the ELF: llvm-objcopy -O binary target/<triple>/release/obc-fw-nrf54l app.bin)

# Wrap into a signed OBCU container tagged with the build's git describe. On a dev machine
# the committed test seed is the right key (the firmware trusts it until the release key is
# rotated — see firmware/obc-dfu/keys/README.md); CI passes --sign-seed-env instead.
cargo run -p obc-mkimage -- wrap \
    --bin app.bin \
    --version "$(git describe --always --dirty)" \
    --out UPDATE.BIN \
    --sign-seed ../obc-dfu/keys/test/obcu-test.seed

# Inspect: decode + verify both CRCs AND the signature (non-zero exit if invalid).
cargo run -p obc-mkimage -- inspect UPDATE.BIN
```

`wrap` refuses an image over the app-slot limit (`MAX_IMAGE_LEN`, 1,480,000 bytes)
and **warns** if the binary's first word isn't a plausible initial stack pointer in
RAM (`0x2000_0000 … 0x2004_0000`) — a raw `.bin` starts with the vector table, so a
failed check usually means an ELF or a wrong-section-order strip slipped through.

**Signing is not optional on the device.** Without `--sign-seed`/`--sign-seed-env`,
`wrap` emits a v1/unsigned container, warns loudly, and the armer **rejects** it
("This update file is not signed for this device") — the signature would be pointless if
an unsigned wrapper still installed. `obc-mkimage sign --in … --out …` attaches the
trailer to an already-wrapped container, so an artifact can be built on one machine and
signed on the one that holds the key; `keygen` makes a keypair. Everything about keys —
the file format, the `OBCU_SIGNING_SEED` CI secret, and the **rotation still owed before
the first real release** — is in [`obc-dfu/keys/README.md`](obc-dfu/keys/README.md).

Installing a staged `UPDATE.BIN` on the device is the app-side armer (S4, #619): copy
the file to the card root and trigger the install — from the S5 UI once it lands, or
today over the debug VCOM link (`dfu-install`; recipe in the
[board README](obc-fw-nrf54l/README.md#triggering-a-firmware-update-over-the-vcom-dfu-install-s4-619)).
The armer validates the file, snapshots the running image to `/ROLLBACK.BIN`, arms the
boot-state page, and resets into `obc-boot`, which does the actual flash
([its README](obc-boot/README.md) has the LED codes).

## Terrain tiles (OBCT)

`obc-dem` turns the source DEM into the terrain artifact carried **beside** a map:
Copernicus GLO-30 GeoTIFF in, `.obcd` out ([`OBCT_Spec.md`](../specs/OBCT_Spec.md),
epic #1068). It is a plain host tool with **no native dependency at all** — the
GeoTIFF decode is pure Rust and the download is `ureq`/rustls, which is what keeps
libGEOS the last one in the tree (#907).

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

**`--bbox` is latitude first** (`min_lat,min_lon,max_lat,max_lon`) — the opposite
of `obc-pack --bbox`, because this tool selects grid *cells* and every grid
expression in the platform puts latitude first. For an Alpine box both numbers are
plausible on either axis, so nothing catches the mix-up; read the flag.

`--posting-log2` / `--cell-log2` default to the v1 baked pairing (`2^9` µdeg
posting, `2^19` µdeg cell — a 2 MiB block). Both are OBCT *header data*, so a
different pairing is a re-bake and not a format change; the committed sim sidecars
use a `2^16` cell for that reason.

For a *published* catalog you do not run `obc-dem bake` by hand: `obc bake terrain`
drives this crate as a library over the curated coverage and lays the cells out as
`cells/terrain/<i>/<j>.obcd` with the sidecars and known-empty runs the catalog
needs (see the root [README](../README.md#baking-and-publishing-the-catalog) and
`OBCC_Spec.md` §13). `obc-dem` on its own stays the way to build a one-off shard.

`fetch` is the only thing that touches the network — a bake is a pure function of
a tile directory and a box, and **byte-identical output for identical inputs is a
contract**, pinned by a digest test. A source void becomes `NODATA`, an uncovered
lattice point becomes `NODATA`, and nothing is ever inpainted.

Anything derived from GLO-30 must carry the Copernicus credit; `bake` prints it,
and `obc_dem::COPERNICUS_ATTRIBUTION` is its single copy in the repo.

The two committed terrain sidecars are regenerated by
[`apps/obc-sim/assets/repack.sh terrain`](../apps/obc-sim/assets/repack.sh), the
same provenance script as the `.obcm` fixtures.
