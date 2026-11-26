# iOS companion

The iPhone app. A rider plans or imports a route, sends it to the device over Bluetooth, gets
recorded rides back, keeps offline maps for the planner and hears about new firmware.

The app does not define the wire format: [`specs/ble-interface.md`](../specs/ble-interface.md)
does, and [`OBCProtocol.md`](OBCProtocol.md) records the iOS-side mappings. The same Xcode
project also holds `OBCDevice`, a development shell that runs the firmware on a phone; its
sources are in [`sim/phone/app`](../sim/phone/app/README.md) and it links nothing from OBCKit.

## Where to start

| Path | What |
| --- | --- |
| `OBCCompanion/` | The composition root: the only target that chooses a transport and a planner provider. Open `OBCCompanionApp.swift`. |
| `Packages/OBCKit/` | The Swift package with the screens, the domain, the transport and the planner client. `Package.swift` lists the targets. |
| [`PlannerNative/`](PlannerNative/README.md) | Routing through `planner-service`, and the PMTiles reader. |
| [`PlannerSearch/`](PlannerSearch/README.md) | The search runtime of `planner/search`, in JavaScriptCore. |
| [`ReplayAssets/`](ReplayAssets/README.md) | The pinned Cesium runtime. `prepare.mjs` also writes the native map styles from the web planner sources. |
| `OBCCompanionUITests/`, `scripts/` | The XCUITests and the website screenshot capture. |
| `project.yml`, `project.local.yml` | The XcodeGen project and the personal signing team. |

```text
OBCDomain <- OBCProtocolV4 (+ OBCHost) <- OBCTransport <- OBCMock
OBCDomain <- OBCFormats
OBCDomain <- OBCPlanner (online client, offline installation and selection)
OBCUI uses OBCDomain, OBCTransport, OBCPlanner and MapLibre
```

The app links two Rust libraries, `OBCHost` (`sim/phone/host`) and `planner-service`
(`planner/service`), and bundles the JavaScript search of `planner/search`.

## Setup and build

Install Xcode 26.x, an iOS simulator runtime, XcodeGen, Node.js 22 or later and the Rust
toolchain. The screenshot capture also needs `webp` and a simulator named "iPhone 17 Pro".

```sh
obc ios-host --sim-only        # pack OBCHost.xcframework; Xcode needs it before it plans a build
cd companion-ios
xcodegen generate              # then use the OBCCompanion scheme
```

`obc ios-companion` builds Release with real Bluetooth and installs it on the paired iPhone;
`obc ios-device` does the same for `OBCDevice`. XcodeGen runs `npm ci` for the replay renderer
and the search runtime; the build phases compile `planner-service` for the selected iOS target.

## Test

```sh
obc ios-host --mac-only
npm ci --prefix companion-ios/ReplayAssets
cd companion-ios/Packages/OBCKit
swift test --disable-keychain --disable-netrc
```

CI runs the package tests with `xcodebuild` (suite `swift.obckit-host`), the Debug and Release
simulator builds, the `OBCDevice` build and the screenshot check
(`scripts/capture-website-screenshots.sh --check`); `tools/testing/suites.toml` has the exact
commands. All iOS suites run on macOS only.

## Mock, captures and local planner data

Debug uses `MockTransport` by default. The launch arguments are in
`OBCMock/MockLaunchOptions.swift`, the scenario presets in `OBCMock/Scenario.swift`. Useful
entry points: `-OBCScenario`, `-OBCFixtures`, `-OBCConnection`, `-OBCImportSample`.
`-OBCHideMockHUD`, `-OBCDisableAnimations` and `-OBCHoldConfirmations` make captures
deterministic. `scripts/capture-website-screenshots.sh` with no argument updates
`docs/assets/companion/`.

`-OBCPlannerCatalog URL` gives a Debug build another planner catalogue, such as the one that
`obc data serve` prints. It changes only the online source: installed offline maps come first
(`LocalFirstPlanner`), and offline downloads use the downloads service of the active release.
On a device, accept the Local Network alert (Settings › Privacy & Security › Local Network).

## Conventions

- The tracked SwiftUI code is the authority for the UI. Reuse `OBCTheme` and the `OBCUI`
  components; read the components and the screenshot tests for the current states. Do not add
  one-off colours or chrome metrics.
- List rows draw the track sketch, never a map. Detail pages use MapLibre with the published
  tiles and fall back to the sketch.
- Copy stays English-only until localization is a complete feature.
- Swift 6, async/await, `AsyncStream` and `@Observable` view models.
- Decode formats at the edges into canonical domain models. Persist canonical models, never
  transport bytes.
- Surface a failed device write, or reconcile it on reconnect. Do not hide it with `try?`.
- Store and cancel open-ended stream tasks; capture `self` weakly and unwrap inside the loop.
- New test suites use Swift Testing. Migrate an XCTest suite only when you rewrite most of it.
- One feature per folder under `OBCUI`.
- In a UI test, wait for an element that renders asynchronously. Do not assume a delay.

## Gotchas

- `project.yml` is the source of truth. The `.xcodeproj` is generated and ignored: run
  `xcodegen generate` after a clone and after each edit, and never commit the pbxproj.
- Xcode resolves `OBCHost.xcframework` before any script phase runs. Run `obc ios-host` once
  before the first build. The package tests need the macOS slice (`--mac-only`).
- There is no Intel simulator slice.
- `project.local.yml` is tracked with an empty team. For a phone build, set your team; the
  header of the file says how to keep the edit out of git.
- The app has no Associated Domains entitlement, because a personal team cannot sign it. A
  pairing link then opens the browser, not the app. A paid team adds the entitlement of
  `specs/ble-interface.md` §9.6 to `project.yml`.
- The map styles, `poi-kinds.json` and `contract.json` under `OBCUI/Resources/Map/` are
  generated and ignored. Edit the web planner sources (`builder/web/src/lib/planner/`).
- The Rust store client builds and decodes the v4 frames; Swift checks only the record framing.
- The Terminus glyphs are a copy of the firmware's; `PixelTextTests` fails when the copy drifts.

## Decisions

- View models depend on small capability protocols of `OBCTransport`, so the same screens run
  on CoreBluetooth and on the deterministic mock. Use the aggregate `DeviceTransport` only for
  aggregate consumers and for the wiring in the composition root.
- CoreBluetooth stays inside `OBCTransport/BLE/`, and the app entry point does not import it.
  Tests hold both (`testCoreBluetoothConfinedToTheBLEFolder`,
  `testAppCompositionRootNeverImportsCoreBluetooth`).
- No mock or developer-panel code ships in Release. `OBCMock` is all inside `#if DEBUG`; CI
  greps the Release binary for the marker `OBCMock:DEBUG-only`.
- `PlannerDataSource` supplies one release for maps, search, routes and viewport layers, and
  `obcPlannerSource` passes it to the map views. `OfflinePlanner` connects installed releases
  to native routing, search and map files. Offline maps are in Settings.
- OBCKit declares macOS only so that host tooling can compile and test it. The app is
  iPhone-only.
- Personal signing stays out of git, so a fresh clone builds for the simulator with no setup.
