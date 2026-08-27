<div align="center">

# OpenBikeComputer

[![CI](https://img.shields.io/github/actions/workflow/status/timohueser/OpenBikeComputer/ci.yml?branch=develop&style=flat-square&label=CI)](https://github.com/timohueser/OpenBikeComputer/actions/workflows/ci.yml?query=branch%3Adevelop)
[![Software: GPL-3.0-only](https://img.shields.io/badge/software-GPL--3.0--only-3c6e47?style=flat-square)](LICENSE)
[![Hardware: CERN-OHL-S-2.0](https://img.shields.io/badge/hardware-CERN--OHL--S--2.0-d46a28?style=flat-square)](LICENSE.hardware)

**An open-source GPS computer for bikepacking.**

[Live browser demo](https://openbikecomputer.com/#demo) ·
[Documentation](https://openbikecomputer.com/software/architecture/) ·
[Contributing](CONTRIBUTING.md)

[<img src="docs/assets/og-card.png" width="100%" alt="OpenBikeComputer concept render with the device showing an offline map of the Grimsel Pass">](https://openbikecomputer.com/#demo)

</div>

> [!NOTE]
> This project is developed with extensive use of AI coding tools. Don't get out your "AI Slop"
> pitchforks just yet though. Any text you read in the software or documentation that sounds AI
> generated will be replaced once we get closer to releasing this project. The goal is that all
> user-facing text is 100% human-generated!

OpenBikeComputer is built for long rides away from mobile service and power sockets. It combines
offline vector maps, route navigation, ride recording, and weather updates with a sunlight-readable
reflective display.

The physical device, desktop simulator, and browser demo use the same Rust application and render
path. The project is an active prototype, not a finished consumer product, but the software already
runs both on a desktop and on the nRF54LM20 development platform.

<table>
  <tr>
    <td align="center" width="50%">
      <img src="docs/assets/companion/route-on-device.webp" width="280" alt="The companion app confirms that a Grimsel Pass route is on the bike computer">
    </td>
    <td align="center" width="50%">
      <img src="docs/assets/companion/ride-detail.webp" width="280" alt="A recorded Grimsel Pass ride in the companion app">
    </td>
  </tr>
  <tr>
    <td align="center"><sub>Send a route to the bike computer.</sub></td>
    <td align="center"><sub>Bring the recorded ride back.</sub></td>
  </tr>
</table>

## Try the simulator

The simulator is the best way to explore the project without hardware. It runs the real device
application in a desktop window and downloads the map, route, terrain, and ride data for the Grimsel
Pass demo on first use.

You need [Rust](https://rustup.rs/), Git, and Python 3.11 or newer. Then run:

```sh
git clone https://github.com/timohueser/OpenBikeComputer.git
cd OpenBikeComputer
cargo install just
./tools/obc sim
```

The first build can take a few minutes. The fixture download is cached for later runs. Run
`./tools/obc setup` once to install the shorter `obc` command, or see the
[simulator guide](apps/obc-sim/README.md) for controls, other scenarios, and headless rendering.

## Roadmap

This is the current direction, not a release schedule.

| Stage | Status |
| --- | --- |
| Shared application, offline maps, routing, ride recording, and weather | Available in the simulator and browser demo |
| iOS companion and BLE/USB transfer flows | Working with the development platform |
| nRF54LM20 development-kit prototype with the 240 × 320 reflective display | Running on hardware |
| Custom PCB and enclosure | In development |
| Integrated, field-tested bike computer | Next major milestone |
| Reproducible hardware build, assembly, and flashing guide | Planned when the custom hardware is ready |

## Current development platform

The hardware build uses the Nordic nRF54LM20 development kit and a Sharp LS021B7DD02 240 × 320
reflective memory LCD. It reads maps and routes from microSD and connects to the companion software
over BLE and USB. The custom PCB and enclosure are still under development.

Board-specific wiring, build, flash, and on-device test instructions remain in the
[`obc-fw-nrf54l` guide](firmware/obc-fw-nrf54l/README.md). They are not part of the newcomer setup
because the current hardware is not generally available.

## Repository layout

| Path | Purpose |
| --- | --- |
| `firmware/` | Device application, rendering, protocols, storage, board image, and bootloader |
| `host/` | Host tools, map and weather bakers, fixtures, and test support |
| `apps/` | Desktop simulator, desktop shell, and browser/WebAssembly hosts |
| `builder/` | Svelte map builder, presets, and maintainer server |
| `companion-ios/` | SwiftUI companion app and shared iOS package |
| `specs/` | Normative binary, wire, and vector contracts |
| `fixtures/` | Scenario registry, source provenance, and fixture builders |
| `docs/` | Public documentation, website, and project blog |
| `hardware/` | KiCad schematics, PCB layouts, footprints, and component models |
| `ops/` | Service configuration, probes, and runbooks |
| `tools/` | The `obc` development command and repository tooling |

The root Cargo workspace contains the shared `firmware/`, `host/`, and `apps/` crates. The nRF54L
board image, bootloader, and Tauri desktop app use standalone Cargo roots so that their platform
dependencies do not burden normal host builds. The
[architecture guide](https://openbikecomputer.com/software/architecture/) explains the boundaries
and the shared render path.

## Other open bike computers

OpenBikeComputer is part of a small but inventive open-source hardware community. These projects
take different approaches to many of the same problems:

- [IceNav](https://github.com/jgauchia/IceNav-v3) — an ESP32-based GPS navigator with multi-GNSS
  support and both rendered and vector offline maps.
- [OpenTrailPaper](https://github.com/RaemondBW/OpenTrailPaper) — an ESP32-S3 e-paper computer
  with offline maps, ride recording, and an iOS companion.
- [Pedal Guru](https://github.com/juliannojungle/pedal.guru) — a work-in-progress bike computer
  project for Raspberry Pi, RP2040, and ESP32 platforms.
- [Pi Zero Bikecomputer](https://github.com/hishizuka/pizero_bikecomputer) — a Raspberry Pi GPS
  and ANT+ computer with offline maps, navigation, and FIT ride logging.
- [bike-computer-32](https://github.com/lspr98/bike-computer-32) — a low-cost ESP32-C3 mini-map
  with offline OpenStreetMap data, GNSS positioning, and GPX track overlays.

## Contributing

Contributions are welcome. Start with [`CONTRIBUTING.md`](CONTRIBUTING.md), which explains the
repository workflow and how to run focused checks. Large external test assets are managed through
the fixture registry described in [`fixtures/README.md`](fixtures/README.md).

## License

Software is available under [GPL-3.0-only](LICENSE). Hardware design sources are available under
[CERN-OHL-S-2.0](LICENSE.hardware). Third-party notices are listed in
[`THIRD-PARTY.md`](THIRD-PARTY.md).
