# OpenBikeComputer iOS companion

Keep this file as an on-ramp. The canonical wire contract is
[`../specs/obc-ble-interface-spec.md`](../specs/obc-ble-interface-spec.md); the compact
[`OBCProtocol.md`](OBCProtocol.md) records only iOS-facing mappings and deltas.

The app imports planned routes, pushes them to the device, and syncs recorded rides back. It is
built against `DeviceTransport`, so the same UI runs against CoreBluetooth on a device and the
deterministic mock in tests and the simulator.

## Architecture

View models depend only on `DeviceTransport`. CoreBluetooth stays in `OBCTransport/BLE/`, and
mock/dev-panel code stays in `OBCMock` behind `#if DEBUG`. Tests enforce both boundaries.

The dependency direction is:

```text
OBCDomain -> OBCTransport -> OBCMock
         \-> OBCFormats
         \-> OBCWeatherWire -> OBCWeather
OBCUI -> OBCDomain + OBCTransport
```

`OBCCompanion/` is the composition root and the only target that chooses a concrete transport.
`project.yml` is the Xcode project source of truth; never edit or commit the generated pbxproj.

## Build and test

Prerequisites are Xcode 26.x, an iOS simulator runtime and XcodeGen.

```sh
cd companion-ios
xcodegen generate

cd Packages/OBCKit
swift test
```

CI runs the package tests plus Debug and Release simulator builds. For a simulator or physical
device, generate the project and use the `OBCCompanion` scheme. Personal signing belongs in the
gitignored `project.local.yml`.

## Mock and captures

Debug uses `MockTransport` by default. The authoritative launch arguments live in
`OBCMock/MockLaunchOptions.swift`; scenario presets live in `OBCMock/Scenario.swift`. Useful entry
points include `-OBCScenario`, `-OBCFixtures`, `-OBCConnection`, `-OBCImportSample`,
`-OBCWeatherDemo`, `-OBCShowDevPanel`, and `-OBCShowUIGallery`.

`-OBCHideMockHUD`, `-OBCDisableAnimations`, and `-OBCHoldConfirmations` make automated captures
deterministic. Website captures are generated and checked by
`scripts/capture-website-screenshots.sh`; wait for asynchronously rendered elements instead of
assuming a delay.

## UI source of truth

The tracked SwiftUI implementation is authoritative. Reuse `OBCTheme` and the `OBCUI` component
kit; inspect the component gallery and screenshot tests for current states. Do not introduce
one-off colours or chrome metrics. Track previews intentionally use MapKit with a grid fallback.
Copy remains English-only until localization is adopted as a complete feature.

## Conventions

- Swift 6, async/await, `AsyncStream`, and `@Observable` view models.
- Decode formats at the edges into canonical domain models.
- Persist canonical models, never transport bytes.
- Surface device-write failures or reconcile them on reconnect; do not hide them with `try?`.
- Store and cancel open-ended stream tasks; capture `self` weakly and unwrap inside the loop.
- New test suites use Swift Testing; migrate XCTest only when substantially rewriting a suite.
- Keep one feature per folder under `OBCUI`.
- Never ship mock or developer-panel code in Release.
