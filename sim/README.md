# Simulators

The device core (`obc-app`, `obc-render`, map reading, navigation, card storage) away from the
board: in a desktop window or as one PNG, on the landing page as WebAssembly, and on an iPhone.
Also the host code these shells share, GPX replay, a simulated device for link tests, the render
bench, and a window that sends test input to a real board.

No device code lives here. The firmware is under `firmware/`; the BLE sensor simulator for a
second development kit is `firmware/obc-sensor-sim`. All crates are members of the root
workspace.

## Where to start

| Crate | What | Open first |
| --- | --- | --- |
| [`desktop/`](desktop/README.md) (`obc-sim`) | The desktop simulator: a window, or a headless `--png` run. `obc shot` renders through it. | `src/main.rs` |
| [`web-demo/`](web-demo/README.md) (`obc-web-demo`) | The landing-page demo. | `src/demo.rs` (the core, tested natively); `src/main.rs` is the wasm shim |
| [`phone/host/`](phone/host/README.md) (`obc-ios-host`) | The device core for the iPhone, as a static library with a C ABI. | `src/lib.rs`, `src/ffi.rs` |
| [`phone/app/`](phone/app/README.md) | The SwiftUI shell `OBCDevice`. Its scheme is in `companion-ios/project.yml`. | `HostController.swift` |
| `host-core/` (`obc-host-core`) | The host code that desktop, web and phone share: the `HostLoop`, stores, frames, terrain. | `src/lib.rs` |
| `replay/` (`obc-replay`) | A GPX player and a barometer that implement the `obc-ports` traits. | `src/lib.rs` |
| `flat-device/` (`obc-flat-device`) | The real `obc-link` engine over a real flat store on a simulated card. | `src/lib.rs` |
| `bench/` (`obc-bench`) | Fixed render scenes and route cases, gated against `golden.txt`. | `src/main.rs` |
| `feeder/` (`obc-usb-host`) | A window that streams GPS, sensors and buttons to a board built with `debug-uart`. | `src/main.rs` |

## Run and test

```sh
obc sim                      # the grimsel scenario; obc sim monaco-upahead; obc sim MAP.obcm TRACK.gpx
obc sim grimsel -- --help    # simulator flags go after --
obc shot NAME                # one UI frame; --list, --check, --accept, --vs REF
obc test -p obc-sim          # any crate of the area
obc test fixtures -p obc-sim # the tests that read fixture packages
obc uart TRACK.gpx           # the feeder; obc debug builds and flashes debug-uart first
bash tools/ci/test.sh bench-golden   # the bench gate
```

`obc sim` syncs the fixtures of its scenario on first use (see
[`fixtures/README.md`](../fixtures/README.md)). Linux needs `libasound2-dev`.

A UI frame is a row of `firmware/ui-frames.toml`; `firmware/ui-snapshots.sha256` holds one PNG
digest for each. The suite `ci.ui-snapshots` runs `obc shot --check`. After an intended change of
the screen, run `obc shot --accept` and commit the digests.

The web demo builds with `trunk build --config docs/Trunk.toml`; its browser suite
(`web.demo-browser`) serves the existing `docs/dist` and never builds it. The phone host builds
with `obc ios-host`; see [`companion-ios/README.md`](../companion-ios/README.md).

## Gotchas

- Tests of `host-core` compile board firmware source: `tests/board_ride.rs`, `board_detour.rs`
  and `factory_demo_ride.rs` include files of `firmware/obc-fw-nrf54l/src/` with `#[path]`. They
  are the host tests of those board files. After an edit there, run `obc test -p obc-host-core`.
- `host-core` must stay free of a GUI, and free of the file system on wasm. `builder/wasm` and
  `host/obcm-assemble` use it too, so a change reaches the web planner.
- `flat-device` is the device behind the TypeScript link suites of `builder/web`, and
  `obc-link`, `builder/usb` and `builder/wasm` use it. It has exactly two test hooks and no
  policy of its own. A test that needs a third hook should ask the engine a different question.
- `desktop/assets/grimsel-demo.obcm` is the map of the web demo and the phone, built into the
  wasm binary. It is not the map of `obc sim`, which comes from the fixture registry. Do not add
  screenshots or fixtures to `assets/`.
- A named `obc sim` scenario supplies its own track and rejects a second one.
- A headless `--png` run must not write the developer's settings file, and never opens audio.
- `obc test -p obc-bench` runs the unit tests only. The golden gate is the `bench-golden`
  section. After an intended change of pixels or read counts, regenerate with
  `cargo run -p obc-bench --release -- --write-golden sim/bench/golden.txt` in the same change.
- The feeder talks to the J-Link serial port of the development kit, not to the device's USB.
  The firmware must have the `debug-uart` feature, which replaces the real sensors.
- The feeder sends a place search to Nominatim only on a press of Search, never for
  autocompletion.
- The wasm build overflows its stack when it boxes the map cache or the route table from the
  stack. Use the `new_boxed` constructors.
- Xcode reads `OBCHost.xcframework` before its script phases run; `obc ios-host` must run once
  first.

## Decisions

- One host core for desktop, web and phone, so the hosts cannot drift. Delete, rescan,
  navigation and track sequencing exist once, in `HostLoop`.
- The bench gates pixels and map read counts, never time: shared CI machines are too noisy for
  time. The read counts catch a cache change that leaves the pixels identical. It never runs on
  a real map in CI, because real maps are not byte-stable.
- Replay is a separate `std` crate. The device never parses GPX, and through the `obc-ports`
  traits the app cannot tell replay from hardware. Replay derives course and speed and
  throttles its output, so a fast host does not flood the matcher or the recorder.
- The desktop simulator runs the device's real row-diff display path next to a full-frame
  comparison, so a diff bug fails in CI. It scans update packages with the real scanner.
- In the web demo, JavaScript owns the frame loop and the canvas, and Rust owns the state. On
  the phone, Swift owns the display loop, and the host renders only when the app reports a change.
- One `flat-device` serves the Rust and the TypeScript harnesses, so both test the same device.
  Framing and retries stay in the adapter of each side.
- The feeder reuses the gesture thresholds and the wire codec of the device
  (`obc-platform::debug_link`), so input from the bench cannot drift from real input.
