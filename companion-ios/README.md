# OpenBikeComputer iOS companion

The app imports planned routes, pushes them to the device, and syncs recorded rides back. The
canonical wire contract is [`../specs/ble-interface.md`](../specs/ble-interface.md);
[`OBCProtocol.md`](OBCProtocol.md) records the iOS-facing mappings.

## Setup

Install Xcode 26.x, an iOS simulator runtime, XcodeGen, Node.js 22 or later, and the Rust
toolchain. Personal signing belongs in `project.local.yml`: it is tracked with an empty team id,
and its header says how to keep your edit out of git.

## Build

XcodeGen installs the pinned replay renderer and builds native map styles and search with npm.
Map styles and POI kinds come from the web planner sources. App builds link `OBCHost` and compile
`planner-service` for the selected Rust iOS target. The host build script also connects the
package to the Rust framework.

For a simulator, prepare the framework before XcodeGen resolves the package, then use the
`OBCCompanion` scheme:

```sh
obc ios-host --sim-only
cd companion-ios
xcodegen generate
```

`obc ios-companion` builds Release with real Bluetooth and installs it on the paired iPhone;
`obc ios-device` does the same for the `OBCDevice` shell.

`project.yml` is the Xcode project source of truth; **never edit or commit the generated
pbxproj.**

## Test

Before package tests without XcodeGen, run `npm ci --prefix companion-ios/ReplayAssets` from the
repository root. Then, from the repository root, prepare the macOS slice:

```sh
obc ios-host --mac-only
cd companion-ios/Packages/OBCKit
swift test --disable-keychain --disable-netrc
```

CI runs the package tests plus Debug and Release simulator builds.

## Mock and captures

Debug uses `MockTransport` by default. The authoritative launch arguments are in
`OBCMock/MockLaunchOptions.swift`, and the scenario presets in `OBCMock/Scenario.swift`. Useful
entry points: `-OBCScenario`, `-OBCFixtures`, `-OBCConnection`, `-OBCImportSample`.

`-OBCHideMockHUD`, `-OBCDisableAnimations` and `-OBCHoldConfirmations` make automated captures
deterministic. Website captures come from `scripts/capture-website-screenshots.sh`. **Wait for an
asynchronously rendered element instead of assuming a delay.**

## Local planner data

`-OBCPlannerCatalog URL` (in `OBCPlanner/PlannerService.swift`) gives a Debug build another planner
catalogue, such as the one that `obc data serve` prints.

- It changes only the online source. Installed offline maps come first (`LocalFirstPlanner`).
  Offline downloads use the downloads service of the active release in that catalogue.
- On a device, accept the Local Network alert. To answer it again, use Settings › Privacy &
  Security › Local Network.

## Architecture

View models depend on capability-sized protocols declared by `OBCTransport`, so the same UI runs
against CoreBluetooth on a device and against the deterministic mock in tests and the simulator.
Use the broad `DeviceTransport` aggregate only for true aggregate consumers and composition-root
wiring. CoreBluetooth stays in `OBCTransport/BLE/`, and mock code stays in `OBCMock` behind
`#if DEBUG`. Tests enforce these boundaries.

```text
OBCDomain -> OBCTransport -> OBCMock
         \-> OBCFormats
OBCUI -> OBCDomain + OBCTransport + OBCPlanner + MapLibre (iOS)
OBCPlanner -> OBCDomain (online client, offline installation and selection)
```

`OBCCompanion/` is the composition root and the only target that chooses a concrete transport and
planner provider. `PlannerDataSource` supplies one release for maps, search, routes, and viewport
layers. `obcPlannerSource` passes that provider to map views. The app uses `LocalFirstPlanner`.
`OfflinePlanner` connects installed releases to native routing, search and map files. Offline maps
live in Settings, with a shortcut in the planner's map menu.
