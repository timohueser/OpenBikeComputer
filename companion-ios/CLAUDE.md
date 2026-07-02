# OpenBikeComputer iOS Companion — notes for Claude

A bonded **BLE companion app** for the OBC bike computer: import planned routes
(share sheet / Files), push them to the device, sync tracked rides back. Built
**entirely against a `DeviceTransport` abstraction** so the whole app is
developed, demoed, and UI-tested against a mock **before the firmware BLE stack
exists**.

This file is the on-ramp: an agent handed only the repo + this file can build,
run on a simulator, and run on a physical iPhone without further guidance. The
epic + per-issue specs live as GitHub issues under
[epic #234](https://github.com/timohueser/OpenBikeComputer/issues/234) (B0–B11);
this app scaffold was **B0** ([#235](https://github.com/timohueser/OpenBikeComputer/issues/235)).

---

## Project map

```
companion-ios/
  project.yml                 XcodeGen source of truth — EDIT THIS, not the pbxproj
  OBCCompanion.xcodeproj      generated (committed) — regenerate: `xcodegen generate`
  OBCCompanion/               app target = composition root ONLY
    OBCCompanionApp.swift      @main; the one place that picks a DeviceTransport
    RootView.swift             launch gate (B2) + the main screen's NavigationStack
                               (B3) + the B4 detail routing and the import edge
                               (RouteImporter → E1 cover); upload = B5 placeholder
    Info.plist                 NSBluetoothAlwaysUsageDescription + the GPX
                               document type (share sheet / "open with OBC" →
                               RootView.onOpenURL → E1; TCX joins with B6)
    Assets.xcassets            AppIcon (empty) + AccentColor (= --forest)
  Packages/
    OBCKit/                    local SwiftPM package — builds/tests WITHOUT the app
      Sources/
        OBCDomain/             pure value types (DeviceInfo, Route/Ride, Waypoint,
                               TrackPreview, …). No framework deps.
        OBCTransport/          DeviceTransport (Tier 1) + TransferHandle + RideDownload
                               + AsyncMulticast + BondStore (the B2 "have we bonded"
                               record — UserDefaults-backed; CB owns the real bond);
          Transfer/            control-plane descriptors + CRC-32 (pure, host-tested)
          Codecs/              device object layouts ↔ domain types (S0-owned bytes:
                               Config blob now; route encoder / ride decoder when
                               S0 pins them). Pure — a device-format change lands here.
          BLE/                 real conformer — BLETransport, BLEChannel (raw CoC
                               streaming), ByteChannel, L2CAPByteChannel, GATT.
                               **CoreBluetooth lives ONLY here.**
        OBCFormats/            interchange file formats (phone-side edge):
                               RouteFileDecoder + RouteImporter with GPXRouteDecoder
                               (landed with B4; TCX + share sheet = B6),
                               RideFileEncoder + RideExporter (ride export, B7).
                               Registries over the canonical ImportedRoute / Ride.
        OBCMock/       #DEBUG   MockTransport + MockControl + Scenario presets +
                               MockBondStore (bond bit from the scenario) +
          Fixtures/            editable JSON fixture sets (default/empty/large) (B1M)
        OBCUI/                  SwiftUI component kit (B11) + feature screens:
          Launch/              B2 launch + pairing flow (LaunchFlowModel state
                               machine + the A/D1–D5/H7/H8 screens)
          Main/                B3 main screen (MainScreenModel + MainScreenView:
                               C1/C2 compact lists, top-bar sync states, pull-down-
                               to-reveal search, swipe-to-delete → H1)
          Detail/              B4 route detail (RouteDetailModel + RouteDetailView:
                               ONE profile layout, three dressings — E2 planned /
                               E3 tracked / E1 import via ImportLandingView — plus
                               the W1 waypoints push and H12 rename)
      Tests/
        OBCTransportTests/     domain/transport/codec unit tests (host, `swift test`);
                               incl. CoreBluetoothSeamTests (enforces the CB seam)
        OBCMockTests/          mock/fixture tests
  OBCCompanionUITests/         XCUITest target — launch-arg driven (→ B1P)
  OBCProtocol.md               wire-contract mirror (B-S0): GATT + CoC + deltas
  CLAUDE.md                    ← you are here
```

The wire protocol the app codes against is pinned in
[`OBCProtocol.md`](OBCProtocol.md) (**B-S0**,
[#236](https://github.com/timohueser/OpenBikeComputer/issues/236)) — GATT control
plane, L2CAP CoC data plane, the typed object model, and the two deltas (device
name in `Config`; GPX **+** TCX import). It's a **mirror**: the firmware `S0`
freeze + `obc-ble-interface-spec.md` are canonical and win on any conflict.

**Layering (lower may not import higher):** `OBCDomain` → `OBCTransport` →
`OBCMock`; `OBCUI` sits on `OBCDomain` + `OBCTransport` (feature view models
consume the `DeviceTransport`/`BondStore` protocols — never `OBCMock`, never
CoreBluetooth); `OBCFormats` sits on `OBCDomain` only. The app target sits on
top and is the *only* target allowed to choose a concrete transport
(and, later, the concrete format registries).

Why SPM-first: the domain + transport layers build and test on the Mac host
(`swift test`) with **no simulator and no app target** — fast, and it keeps the
BLE/UI concerns from leaking into the core.

---

## The golden rule

> **View models depend only on `DeviceTransport`.** CoreBluetooth appears **only**
> inside `BLETransport` (in `OBCTransport`). Mock and dev-panel code lives **only**
> inside `#if DEBUG` (in `OBCMock`). Mock code must **never** ship in Release.

Everything downstream of the composition root (`OBCCompanionApp.makeTransport()`)
sees only the `any DeviceTransport` protocol — never `CBCentralManager`, never
`MockTransport`. Two conformers:

- **`BLETransport`** (real, **B1**) — CoreBluetooth + the `BLEChannel` byte layer
  (GATT / L2CAP CoC, framing, CRC, codecs). Its **live path is gated on firmware
  `A4`/`A5`**; the framing/codec layer beneath it is fully host-tested.
- **`MockTransport`** (fake, `#if DEBUG`, **B1M**) — fixture-backed, driven by a
  `MockControl` fault-injection surface that can reproduce every design state.

The CB seam is **test-enforced**: `CoreBluetoothSeamTests` (`swift test`) fails if
`import CoreBluetooth` appears anywhere outside `OBCTransport/BLE/` or in the app's
composition root.

The mock-exclusion **seam is real and tested**: `OBCMock` is entirely behind
`#if DEBUG`, so a Release build compiles it to an empty module (the editable
JSON fixtures still ship as inert bundle data — it's the *code* that's gated).
Prove it by `strings`-grepping the compiled objects (plain `grep -r` on `.build`
misses it — `.build/debug` is a symlink and the marker lives in a binary `.o`):

```bash
cd companion-ios/Packages/OBCKit
marker() { find ".build/$1/OBCMock.build" -name '*.o' -exec strings {} \; | grep -c "OBCMock:DEBUG-only"; }
swift build -c debug   >/dev/null && echo "debug:   $(marker debug)   (expect ≥1)"
swift build -c release >/dev/null && echo "release: $(marker release) (expect 0)"
```

(Or grep the built `.app` binary — see the seam note under *Build/run*.)

---

## Build / test / run

### Toolchain prerequisites (verified on this machine)

- **Xcode 26.6** (iOS **26.5** SDK). `xcodebuild -version`.
- **XcodeGen** — `brew install xcodegen` (used to (re)generate the `.xcodeproj`).
- **A modern iOS simulator runtime.** Xcode 26 will **not** pair its iOS 26.5
  simulator SDK with old runtimes (e.g. iOS 17.5) — if the only runtime is old,
  `xcodebuild -showdestinations` lists *no* simulators. Install the matching one:
  ```bash
  xcodebuild -downloadPlatform iOS      # ~8.5 GB; run in an interactive Terminal
  ```
  (Or Xcode ▸ Settings ▸ Components.) The app's **minimum deployment target is
  iOS 17.0** — that's just the floor; we build/run on the newest sim + real HW.

### XcodeBuildMCP (preferred path for agents)

Build/test/run go through **XcodeBuildMCP**, configured at the **repo root** in
[`../.mcp.json`](../.mcp.json):

```json
{ "mcpServers": { "XcodeBuildMCP": {
  "command": "npx", "args": ["-y", "xcodebuildmcp@2.6.2", "mcp"],
  "env": { "XCODEBUILDMCP_ENABLED_WORKFLOWS": "simulator,device,project-discovery" } } } }
```

- ⚠️ **Adding/editing `.mcp.json` requires restarting Claude Code** — MCP servers
  load at session start. After the first launch, approve the server when prompted.
- The `mcp` subcommand + `XCODEBUILDMCP_ENABLED_WORKFLOWS` matter: the default set
  is simulator-only; `device` unlocks `build_run_device` / `launch_app_device` /
  `list_devices`, which B0 requires.

**XcodeBuildMCP is session-defaults driven** — set the project/scheme/simulator
once, then the build tools take no path args. Paths are relative to the repo root
(the server's cwd).

1. **Set session defaults (once per session):**
   ```
   session_set_defaults {
     projectPath: "companion-ios/OBCCompanion.xcodeproj",
     scheme: "OBCCompanion",
     configuration: "Debug",
     simulatorName: "iPhone 17 Pro",     // pick a real one from list_sims
     persist: true                        // writes companion-ios/.xcodebuildmcp/config.yaml
   }
   ```
   `session_show_defaults` prints the active set; `list_sims` / `list_schemes`
   discover valid values. (`persist: true` saves them so later sessions skip this.)
2. **List simulators:** `list_sims {}`
3. **Build + boot + install + launch on sim (one step):** `build_run_sim {}`
   - Compile-only: `build_sim {}` · boot without building: `boot_sim {}`
   - Install/launch a prebuilt app: `install_app_sim { appPath }` →
     `launch_app_sim { launchArgs: [...] }`
4. **Screenshot the sim:** `screenshot {}`  · **semantic UI tree:** `snapshot_ui {}`
5. **Run tests on the sim (app + XCUITests):** `test_sim {}`
6. **Unit tests without a sim** (domain/transport/mock) — run on the host with
   `swift test` (below); the package targets aren't in the app scheme.
7. **Physical iPhone** (needs the device unlocked, trusted, and a signing team —
   see *Signing*):
   ```
   list_devices {}
   session_set_defaults { deviceId: "<id from list_devices>" }
   build_run_device {}            // build + install + launch; launch_app_device to relaunch
   ```
   Runtime logs from `launch_app_sim` / `build_run_sim` are captured to a file
   whose path the tool returns — read it to tail logs.

### Raw `xcodebuild` / `swift` (ground truth — what the MCP wraps)

These are the verified commands; use them in CI or when the MCP isn't loaded.

```bash
# Unit tests — host, no simulator (fast). VERIFIED ✓
cd companion-ios/Packages/OBCKit && swift test

# (Re)generate the Xcode project after editing project.yml. VERIFIED ✓
cd companion-ios && xcodegen generate

# Build the app for a simulator (Debug). Needs a modern iOS runtime installed.
cd companion-ios
xcodebuild build -scheme OBCCompanion -configuration Debug \
  -destination 'platform=iOS Simulator,name=iPhone 17 Pro'

# Boot + install + launch on a simulator by hand
xcrun simctl boot "iPhone 17 Pro"; open -a Simulator
xcrun simctl install booted <path>/OBCCompanion.app
xcrun simctl launch booted com.openbikecomputer.companion

# Prove the mock-exclusion seam on the built .app (grep the Mach-O binaries with
# `strings`, not `grep -r` — and note Debug hides the code in a `.debug.dylib`).
seam() { find "$1" -type f \( -name OBCCompanion -o -name '*.dylib' \) \
  -exec strings {} \; | grep -c "OBCMock:DEBUG-only"; }
seam .../Debug-iphonesimulator/OBCCompanion.app     # → 1  (mock compiled in)
seam .../Release-iphonesimulator/OBCCompanion.app   # → 0  (mock excluded)
```

### Signing (physical device only)

Simulator builds need no signing. For a real iPhone set your team once in
`project.yml` (then `xcodegen generate`):

```yaml
settings:
  base:
    DEVELOPMENT_TEAM: "YOURTEAMID"   # Xcode ▸ Settings ▸ Accounts, or `security find-identity`
```

`CODE_SIGN_STYLE` is already `Automatic`. First device run: unlock the phone,
trust the Mac, and (free accounts) approve the profile in Settings ▸ General ▸
VPN & Device Management.

---

## Running against the mock

**Debug defaults to `MockTransport`** — there is no Bluetooth in the simulator,
so the mock *is* the default dev target. `OBCCompanionApp.makeTransport()` wires
the mock under `#if DEBUG` (booted into whatever the launch arguments ask for);
`-OBCTransport ble` forces the real `BLETransport` on a Debug device build.

**B1M** ([#238](https://github.com/timohueser/OpenBikeComputer/issues/238)) landed
the fixture-backed mock: a live `MockControl` fault-injection surface, editable JSON
fixture sets (`OBCMock/Fixtures/*.json`), and named `Scenario` presets that reproduce
each design state with no device and no firmware. Realism comes from **latency +
throughput + faults**, never wire bytes — the mock serves domain objects straight
from fixtures. Select a scenario **programmatically** today:

```swift
RootView(transport: MockTransport(scenario: .outOfRange))   // or: control.apply(.syncDrop)
```

`Scenario` → design screens (the authoritative table; source of truth is
[`Scenario.swift`](Packages/OBCKit/Sources/OBCMock/Scenario.swift)):

| Scenario | Reproduces |
|---|---|
| `happyPath` | C1 / C2 / E2 / F / F₂ |
| `emptyLibrary` | S1 |
| `coldRead` | S2 (skeletons) |
| `readError` | S3 |
| `outOfRange` | S4 + disconnected banner |
| `noDevice` | D1→D4 pairing flow; H4 on import (A = any bonded scenario + `-OBCConnection disconnected`) |
| `pairingTimeout` / `pairingRejected` | D5 |
| `bluetoothOff` / `permissionDenied` | H8 / H7 |
| `syncUpToDate` / `syncDrop` | H9 / H10 |
| `uploadDrop` | F interrupted → resume |
| `unsupportedFile` | H5 |

`loadFixtures("empty" | "large")` swaps the library (S1 / search). `unsupportedFile`
(H5) and `syncUpToDate` (H9) are pure UI-layer states — their preset is a happy link
and the UI branches on `scenario`; the rest are transport-driven.

Each preset also carries a **bond bit** (`ScenarioPreset.bonded`, served through
`MockBondStore` → the B2 launch branch): the pairing-family scenarios
(`noDevice`, `pairingTimeout`, `pairingRejected`, `bluetoothOff`,
`permissionDenied`) boot **unpaired** into D1; everything else boots bonded
straight toward main. H7/H8 are reached from D1 via *Start pairing* (that's when
the app first touches the radio, matching the real permission-prompt timing).

### Launch arguments (B1P, [#239](https://github.com/timohueser/OpenBikeComputer/issues/239))

**These names are stable automation API** (XCUITests + screenshot scripts depend
on them — parsing lives in
[`MockLaunchOptions.swift`](Packages/OBCKit/Sources/OBCMock/MockLaunchOptions.swift),
consumed once at the composition root). Unknown values degrade to defaults, never
crash. Env fallbacks in parentheses apply when the argument is absent.

| launch arg | values | effect |
|---|---|---|
| `-OBCScenario <name>` (`OBC_SCENARIO`) | any `Scenario` rawValue | boot into that scenario |
| `-OBCFixtures <name>` (`OBC_FIXTURES`) | `default` / `empty` / `large` | override the fixture set |
| `-OBCConnection <state>` (`OBC_CONNECTION`) | `disconnected` / `connecting` / `connected` / `outOfRange` | override the initial link state |
| `-OBCTransport ble` (`OBC_TRANSPORT`) | `ble` / `mock` | force the real `BLETransport` (device only) |
| `-OBCShowDevPanel` (`OBC_SHOW_DEV_PANEL=1`) | flag | present the dev panel at launch |
| `-OBCShowUIGallery` (`OBC_SHOW_UI_GALLERY=1`) | flag | present the B11 component gallery at launch |
| `-OBCImportSample` (`OBC_IMPORT_SAMPLE=1`) | flag | boot into the E1 import landing with the bundled sample GPX (`OBCMock/Fixtures/sample-import.gpx`, a real Komoot export) through the real decoder |

### Dev control panel + HUD (Debug only)

**Shake the device** (sim: Device ▸ Shake, ⌃⌘Z) — or launch with
`-OBCShowDevPanel` — to open the **Mock control** panel: live `MockControl`
knobs (scenario preset, connection, radio, bonded, battery, latency/throughput,
one-shot faults, synthetic events, fixture swap) you can flip while clicking
through the app. The panel + the status HUD live in `OBCMock`
([`MockControlPanel.swift`](Packages/OBCKit/Sources/OBCMock/MockControlPanel.swift));
the app-side host (shake hook, sheet, overlay) is `OBCCompanion/DevMockOverlay.swift`.
B8 adds the second entry point (a hidden Settings row).

The **HUD** (bottom-right capsule) shows `scenario · connection` with
accessibility ids `mockScenarioTag` / `mockConnectionTag` — what the XCUITests
assert. `OBCCompanionUITests/ScenarioLaunchTests` launches every scenario by
argument and checks the tag (plus fixture-name, connection-override, and
panel-presentation smoke tests); `PairingFlowTests` walks the B2 launch/pairing
flow end to end per scenario, `MainScreenTests` walks the B3 main-screen
states (C1/C2, SYNC, S4, H6, H11→H1), and `RouteDetailTests` walks the B4
detail dressings (E2/E3/W1/H12/H1 + E1 via `-OBCImportSample`) — all attach a
screenshot of each design screen to the result bundle. Run them with
`test_sim {}` / `xcodebuild test`. The B3 landing anchor the pairing tests wait
for is `main.screen`; the detail anchor is `detail.screen` (⚠️ it sits on a
ScrollView, so query it with `descendants(matching: .any)`, not `otherElements`).

---

## Design source of truth

**`project/OBC Companion App.dc.html`** (repo root, not under `companion-ios/`) is
canonical for layout, copy, and states. **Read the HTML/CSS directly** for exact
dimensions and colors — do **not** screenshot it. Design tokens are in
`project/_ds/openbikecomputer-design-system-*/tokens/`.

Screen IDs are canonical and grouped by letter:

| letter | area | ids |
|---|---|---|
| **C** | main screen (connected) | C1, C2 |
| **D** | launch · pairing / bonding | D1–D5 |
| **E** | route detail (planned / import / tracked) | E1–E3 |
| **F** | upload sheet | F, F2 |
| **G** | settings | G |
| **S** | app states (empty / loading / error / disconnected) | S1–S4 |
| **H** | edge cases & confirmations | H1–H12 |
| **I** | import (share sheet + Files) | I1, I2 |
| **W** | waypoints | W1 |

### Design tokens

Base OBC palette (`tokens/colors.css`) — the field-guide colourway:
`--parchment #ece8cf` (base), `--ink #24331c` (text), `--forest #3c6b39`
(primary/tint), `--coral #cf6a2a`, `--amber #e3ad33`. The **iOS additions** live
in the `:root` of `OBC Companion App.dc.html`:

- `--tint: var(--forest)` — the app tint.
- `--track-stroke #d99a1f`, `--track-halo #f4ecc9` — route stroke + casing.
- `--track-start: var(--forest)`, `--track-end: var(--coral)` — track gradient.
- Metrics: **44px** nav bar · **54px** status bar · **13px** control radius.

### The component kit (B11, [#240](https://github.com/timohueser/OpenBikeComputer/issues/240))

`OBCUI` holds the full token mapping + the §9 component kit — **use it, don't
restyle ad hoc**, and don't introduce colors outside
[`OBCTheme`](Packages/OBCKit/Sources/OBCUI/Theme/OBCTheme.swift):

- **Theme/** — `OBCTheme` (all tokens + 44/54/13 chrome metrics + radii),
  `Font.obcSerif/obcMono` (serif = Iowan Old Style, ships with iOS — Spectral is
  only the web stand-in), `OBCFormat` (the canonical stat-line strings:
  "62.4 km · 840 m ↑ · 3h 20m" — pinned by unit tests, don't format inline).
- **Components/** — `TrackPreviewView` (renders `OBCDomain.TrackPreview`;
  basemap-free, halo + stroke + forest/coral node dots, waypoint `Marker`s),
  `RouteCard`/`RouteCardFullBleed`, `DeviceTopBar` (+ `OBCBatteryIndicator`,
  `OBCIconButton`, `OBCSpinner`, `OBCSyncButtonState`), `OBCSegmentedControl`,
  `OBCGroupedSection`/`OBCListRow`/`OBCIconTile`/`OBCSoonBadge`, buttons as
  `ButtonStyle`s (`.obcPrimary/.obcGhost/.obcWarm/.obcDestructive`),
  `OBCProgressBar`, `OBCSearchField`, `OBCSheetContainer`, `OBCSkeleton`/`RouteCardSkeleton`,
  `OBCInlineBanner`/`OBCToast` (`.obcToast`), `OBCEmptyStateView`,
  `ElevationProfileView`, `OBCStatStrip`/`OBCStatGrid`, `OBCDisclosureRow`,
  `WaypointRow`/`WaypointsListView`, `OBCConnectedServicesBlock`,
  `OBCImportButton` (opens the Files picker directly, filtered by decoder
  extensions — deliberately no intermediate menu),
  `.obcSwipeToDelete` (always confirms), `.obcRenameAlert` /
  `.obcDestructiveConfirm` (native presentations; pairing prompts stay
  system-blue — see `OBCSystemPairing`), `OBCNavigationChrome.apply()` +
  `OBCLargeTitleBar`.
- **Gallery/** — `OBCComponentGallery` (`#if DEBUG`): every component with
  design data. Launch with `-OBCShowUIGallery` for screenshot review;
  `GalleryLaunchTests` smoke-tests it. Like the mock, it must never reach a
  Release binary (verify with the strings recipe above).

Every component file carries a `#Preview` with design sample data.

---

## Conventions

- **SwiftUI + async/await + `AsyncStream`.** Transport surfaces streams
  (connection state, progress); view models consume them.
- **File formats at the edges, canonical models in the middle.** Route files
  decode into `ImportedRoute`, device ride bytes decode into `Ride`, and every
  export encodes *from* `Ride` through the `RideExporter` registry (`OBCFormats`).
  Never parse a file or generate an export anywhere else — switching the
  tracked-file format (GPX → FIT) must stay a one-conformer change at the
  composition root.
- **Observation (`@Observable`) for view models.** Not `ObservableObject`.
- **One feature per folder** under `OBCUI` (a view + its view model together).
- **Strict concurrency is on** (`SWIFT_STRICT_CONCURRENCY: complete`, package
  targets use `.enableExperimentalFeature("StrictConcurrency")`). Keep domain
  types `Sendable` value types.
- **Unit-test the transport/codec logic** in `OBCKit` (`swift test`) — that's the
  layer that must be right before the firmware exists. UI flows get XCUITests
  (B1P), driven by launch args.
- **Never hand-edit the pbxproj.** Change `project.yml` and regenerate.

---

## What NOT to do

- **No basemap under tracks.** Routes render on parchment, not a map tile layer.
- **No blocking full-screen spinner.** Use inline / skeleton states (see S1–S4).
- **No new colors** outside the OBC tokens above.
- **No cloud / account language** anywhere — this app is phone ↔ device only, no
  sign-in, nothing leaves the phone (see the Bluetooth usage string).
- **Never ship mock code in Release.** Mock/panel code stays inside `#if DEBUG`
  / `OBCMock`. The seam is tested — keep it that way.
