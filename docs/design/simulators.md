---
system: simulators
kind: shell
purpose: "Runs the device core on a computer, a phone or a web page, and sends test input to a real board."
owns:
  - sim/desktop/**
  - sim/web-demo/**
  - sim/phone/**
  - sim/feeder/**
  - sim/bench/**
depends_on:
  - board-inputs
  - card-storage
  - data-engine
  - device-link
  - device-runtime
  - firmware-update
  - foundations
  - host-runtime
  - map-reading
  - navigation
  - rendering-display
  - repository-tooling
test_depends_on: []
status: approved
---

# Simulators

## What it does

The simulators run the device's own code away from the board. They link the device runtime, the renderer, map reading, navigation and card storage through the shared host code of host runtime.

- The desktop simulator ([`obc-sim`](../../sim/desktop/src/main.rs)) shows the device in a window, or writes one frame to a PNG file. A developer starts it with `obc sim`.
- The web demo ([`obc-web-demo`](../../sim/web-demo/README.md)) runs the device on the landing page of the project site, compiled to WebAssembly.
- The iPhone app `OBCDevice` ([`sim/phone/app`](../../sim/phone/app/README.md)) runs the device on a phone, with the phone's GPS, compass and barometer. Its Rust part is [`sim/phone/host`](../../sim/phone/host/README.md).
- The USB feeder ([`sim/feeder`](../../sim/feeder/src/main.rs)) runs no device code. It sends a recorded track, sensor values and button presses to a real board over USB.
- The bench ([`sim/bench`](../../sim/bench/src/main.rs)) renders fixed scenes and measures the time of each step.

## Rules that must hold

| Rule | Check |
| --- | --- |
| The pixels of the bench scenes and their map read counts change only on purpose. | `check_golden` in [`golden.rs`](../../sim/bench/src/golden.rs), against [`golden.txt`](../../sim/bench/golden.txt) |

## Decisions

- **The bench gates pixels and read counts, not time.** It prints the time of each step, but never fails on it, because shared CI machines are noisy. The pixel hashes and the read counts are the same on each run, so `--check` fails on any change.
