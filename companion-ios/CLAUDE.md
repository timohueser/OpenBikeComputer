# OpenBikeComputer iOS companion

An on-ramp. The canonical wire contract is
[`../specs/obc-ble-interface-spec.md`](../specs/obc-ble-interface-spec.md);
[`OBCProtocol.md`](OBCProtocol.md) records the iOS-facing mappings and deltas.

The app imports planned routes, pushes them to the device, and syncs recorded rides back.

## Architecture

View models depend on capability-sized protocols declared by `OBCTransport`, so the same UI runs
against CoreBluetooth on a device and against the deterministic mock in tests and the simulator.
Use the broad `DeviceTransport` aggregate only for true aggregate consumers and composition-root
wiring. CoreBluetooth stays in `OBCTransport/BLE/`, and mock and dev-panel code stays in `OBCMock`
behind `#if DEBUG`. Tests enforce these boundaries.

```text
OBCDomain -> OBCTransport -> OBCMock
         \-> OBCFormats
OBCUI -> OBCDomain + OBCTransport
OBCRouting -> OBCDomain + OBCCompanionCore (Rust; its own package, only the app links it)
```

`OBCCompanion/` is the composition root and the only target that chooses a concrete transport.
`project.yml` is the Xcode project source of truth; **never edit or commit the generated
pbxproj.**

## Build and test

Needs Xcode 26.x, an iOS simulator runtime, XcodeGen and the Rust toolchain.

```sh
obc companion-core   # the Rust router the app links; run it again after a Rust change
cd companion-ios
xcodegen generate

cd Packages/OBCKit
swift test
cd ../OBCRouting
swift test
```

CI runs the package tests plus Debug and Release simulator builds. For a simulator, generate the
project and use the `OBCCompanion` scheme. `obc ios-companion` builds Release with real Bluetooth
and installs it on the paired iPhone; `obc ios-device` does the same for the `OBCDevice` shell.
Personal signing belongs in the gitignored `project.local.yml`.

## Mock and captures

Debug uses `MockTransport` by default. The authoritative launch arguments are in
`OBCMock/MockLaunchOptions.swift`, and the scenario presets in `OBCMock/Scenario.swift`. Useful
entry points: `-OBCScenario`, `-OBCFixtures`, `-OBCConnection`, `-OBCImportSample`,
`-OBCShowDevPanel`, `-OBCShowUIGallery`.

`-OBCHideMockHUD`, `-OBCDisableAnimations` and `-OBCHoldConfirmations` make automated captures
deterministic. Website captures come from `scripts/capture-website-screenshots.sh`. **Wait for an
asynchronously rendered element instead of assuming a delay.**

## UI source of truth

The tracked SwiftUI implementation is authoritative. Reuse `OBCTheme` and the `OBCUI` component
kit, and read the component gallery and the screenshot tests for the current states. Do not
introduce one-off colours or chrome metrics. List rows draw the track sketch, never a map;
detail pages use MapKit and fall back to the sketch. Copy stays English-only until localization is a complete feature.

## Conventions

- Use ASD-STE100 Simplified Technical English for documentation, issues and pull requests.
- Swift 6, async/await, `AsyncStream`, and `@Observable` view models.
- Decode formats at the edges into canonical domain models. Persist canonical models, never
  transport bytes.
- Surface device-write failures or reconcile them on reconnect; do not hide them with `try?`.
- Store and cancel open-ended stream tasks; capture `self` weakly and unwrap inside the loop.
- New test suites use Swift Testing; migrate an XCTest suite only when substantially rewriting it.
- Keep one feature per folder under `OBCUI`.
- Never ship mock or developer-panel code in Release.
