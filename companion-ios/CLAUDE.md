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
    RootView.swift             placeholder root (real screen stack = B3+)
    Info.plist                 NSBluetoothAlwaysUsageDescription (B6 adds UTI/Share)
    Assets.xcassets            AppIcon (empty) + AccentColor (= --forest)
  Packages/
    OBCKit/                    local SwiftPM package — builds/tests WITHOUT the app
      Sources/
        OBCDomain/             pure value types (DeviceInfo, …). No framework deps.
        OBCTransport/          DeviceTransport protocol (+ BLETransport, BLEChannel → B1)
        OBCMock/       #DEBUG   MockTransport + MockControl + fixtures (→ B1M)
        OBCUI/                  SwiftUI component kit + feature views (→ B11)
      Tests/
        OBCTransportTests/     domain/transport/codec unit tests (host, `swift test`)
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
`OBCMock`; `OBCUI` sits beside them on `OBCDomain`. The app target sits on top of
all four and is the *only* target allowed to choose a concrete transport.

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
  (GATT / L2CAP CoC, framing, CRC, codecs).
- **`MockTransport`** (fake, `#if DEBUG`, **B1M**) — fixture-backed, driven by a
  `MockControl` fault-injection surface that can reproduce every design state.

The mock-exclusion **seam is real and tested**: `OBCMock` is entirely behind
`#if DEBUG`, so a Release build compiles it to an empty module. Prove it:

```bash
cd companion-ios/Packages/OBCKit
swift build -c debug   && grep -rq "OBCMock:DEBUG-only" .build/debug   && echo "debug: marker present ✓"
swift build -c release && ! grep -rq "OBCMock:DEBUG-only" .build/release && echo "release: marker absent ✓"
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
so the mock *is* the default dev target. `OBCCompanionApp.makeTransport()`
returns `MockTransport()` under `#if DEBUG`.

Once **B1P** lands the dev control panel + launch-arg hooks, scenarios are
selectable without a rebuild via launch args (`launchArgs` in the MCP run tools,
or the scheme's arguments):

```
-OBCScenario happyPath      -OBCConnection outOfRange
```

Planned scenario → screen mapping (authoritative table lives in **B1M**,
[#238](https://github.com/timohueser/OpenBikeComputer/issues/238)):

| launch arg | drives | design screens |
|---|---|---|
| `-OBCScenario happyPath` | connected, routes present | C1/C2, E1–E3 |
| `-OBCScenario empty` | no routes | S1 |
| `-OBCConnection outOfRange` | disconnected / searching | S4, D-series |
| `-OBCScenario syncError` | ride sync failure | H9/H10 |

(These args are **not wired yet** — B1P. Listed here so the seam is documented.)

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

The Swift `Color`/`Font` theme that maps these tokens is **B11**. Until then,
`OBCUI.OBCTheme` exposes only `tint`/`parchment`/`ink` as a placeholder — don't
grow it ad hoc; do it properly in B11.

---

## Conventions

- **SwiftUI + async/await + `AsyncStream`.** Transport surfaces streams
  (connection state, progress); view models consume them.
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
