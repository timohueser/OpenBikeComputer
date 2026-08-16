# OBC — OpenBikeComputer

OpenBikeComputer is a from-scratch bikepacking computer: offline vector maps, route navigation,
ride recording, weather and field updates on an nRF54L driving a 240×320 reflective memory-LCD.
The firmware, desktop simulator and browser demo share the same Rust application and renderer.

<img width="483" height="692" alt="OpenBikeComputer prototype" src="https://github.com/user-attachments/assets/a222e908-a12b-4e26-a53c-6c227f30005a" />

The conceptual guide and live firmware demo are at
[openbikecomputer.com](https://openbikecomputer.com/). Normative binary and wire contracts live in
[`specs/`](specs/).

## Start here

Install stable Rust, then build the host workspace:

```sh
cargo build --release
```

The two common outputs are:

- `target/release/obc-sim` — the desktop device simulator;
- `target/release/obc-pack` — the OSM PBF to OBCM map packer.

The repository-owned `obc` command wraps development tasks and checks. Run `obc doctor` to inspect
optional host dependencies. Verification is deliberately scoped; see
[`CONTRIBUTING.md`](CONTRIBUTING.md) before running tests.

```sh
obc test -p obc-pack
obc check docs
```

To prove the shared application still compiles for the device target:

```sh
rustup target add thumbv8m.main-none-eabihf
cargo build -p obc-app --target thumbv8m.main-none-eabihf
```

The board image and desktop app are standalone Cargo roots with their own prerequisites and
commands:

- [`firmware/obc-fw-nrf54l/README.md`](firmware/obc-fw-nrf54l/README.md)
- [`apps/obc-desktop/README.md`](apps/obc-desktop/README.md)

## Maps, routes and simulation

Pack an OSM extract with the shipped schema:

```sh
target/release/obc-pack region.osm.pbf builder/presets/schema.json region.obcm
```

Run the result in the simulator:

```sh
target/release/obc-sim region.obcm
```

The [simulator guide](apps/obc-sim/README.md) covers headless PNGs, GPX replay, the interactive
control panel, and every CLI option.

The hosted and desktop builders select digest-pinned cells from the published catalog and assemble
the same OBCM bytes in a WebAssembly worker. Maintainer baking, incremental planet updates and R2
publication are documented in the [packer and routing guide](https://openbikecomputer.com/software/packer-routing/)
and exposed through `obc bake help`.

Routes enter as GPX/TCX on a host and become compact OBCR files before reaching the device. The
device reads maps, routes, terrain and weather directly from binary formats designed for bounded,
streaming access. Readable tours live on the docs site; exact layouts live in:

- [`OBCM_Spec.md`](specs/OBCM_Spec.md) — maps;
- [`OBCR_Spec.md`](specs/OBCR_Spec.md) — routes;
- [`OBCT_Spec.md`](specs/OBCT_Spec.md) — terrain;
- [`OBCW_Spec.md`](specs/OBCW_Spec.md) and [`OBCG_Spec.md`](specs/OBCG_Spec.md) — weather;
- [`OBCC_Spec.md`](specs/OBCC_Spec.md) and [`OBCA_Spec.md`](specs/OBCA_Spec.md) — catalogs and cells.

## Companion and updates

The SwiftUI iOS companion imports routes, synchronizes rides, manages firmware updates and services
device weather requests. Its package is developed against a deterministic mock and uses the same
wire vectors as the firmware. See [`companion-ios/CLAUDE.md`](companion-ios/CLAUDE.md) for the focused
build/test on-ramp.

BLE and USB bind the same object protocol. The canonical contract is
[`specs/obc-ble-interface-spec.md`](specs/obc-ble-interface-spec.md) (legacy wire v2 — superseded for
DOS v2 by the [`specs/Device_Object_System_v2.md`](specs/Device_Object_System_v2.md) suite); the
conceptual flow is in the
[companion-link guide](https://openbikecomputer.com/software/companion-link/).

Firmware updates are signed OBCU containers named `UPDATE.BIN`. Release builds, signing, install,
trial boot and rollback are documented in the
[firmware-update guide](https://openbikecomputer.com/software/firmware-updates/) and
[`firmware/README.md`](firmware/README.md#firmware-update-images-obcu).

## Repository layout

| Path | Purpose |
| --- | --- |
| `firmware/` | `no_std` application, readers, renderer, protocol cores, storage and board images |
| `host/` | map/weather bakers, assemblers, fixtures, command-line tools and shared host glue |
| `apps/` | desktop simulator, Tauri desktop app and WebAssembly hosts |
| `builder/` | shared Svelte map/device UI, presets and maintainer server |
| `companion-ios/` | SwiftUI companion app and the OBCKit package |
| `specs/` | normative formats, wire contract and executable vectors |
| `fixtures/` | scenario registry, tracked source provenance and reproducible fixture builders |
| `docs/` | public site source, landing page, blog and authoring tools |
| `ops/` | deployed weather-service installation, probes and runbook |
| `tools/` | the `obc` workflow command and host setup helpers |

One Cargo workspace spans the shared `firmware/`, `host/` and `apps/` crates. The nRF54L board
image, bootloader and Tauri desktop shell remain standalone because their toolchains and platform
dependencies should not burden ordinary host tests.

The direction of dependencies is enforced: device-reachable crates live under `firmware/`; host
policy and heavy native dependencies stay above them. The public architecture guide explains the
boundaries and shared render path in detail:
[openbikecomputer.com/software/architecture](https://openbikecomputer.com/software/architecture/).

## Current development platform

The full application runs in the desktop simulator and on the Nordic nRF54LM20 development kit. The
device streams maps and routes from microSD, drives the LS021 panel through the FLPR coprocessor,
uses real GPS and altimeter inputs, and exposes the companion protocol over BLE and USB.

The production PCB and enclosure are still under development. Current wiring, board configuration,
flashing and on-glass verification belong in the
[`obc-fw-nrf54l` README](firmware/obc-fw-nrf54l/README.md), while panel behavior belongs in the
[display-protocol guide](https://openbikecomputer.com/hardware/display-protocol/).

## Contributing and license

Read [`CONTRIBUTING.md`](CONTRIBUTING.md) for proportional verification and safe cleanup. Large test
fixtures are acquired through `obc fixtures`; see [`fixtures/README.md`](fixtures/README.md).

Software is GPL-3.0 under [`LICENSE`](LICENSE). Hardware design sources are CERN-OHL-S-2.0 under
[`LICENSE.hardware`](LICENSE.hardware). Third-party notices are collected in
[`THIRD-PARTY.md`](THIRD-PARTY.md).
