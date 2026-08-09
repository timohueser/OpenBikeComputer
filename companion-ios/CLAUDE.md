# OpenBikeComputer iOS Companion — notes for Claude

> **Keep this file short.** It's an on-ramp, not a spec. Do NOT add issue links,
> per-screen tables, exhaustive component lists, or feature history — those live
> in the source (see the pointers below). Only add something here if an agent
> genuinely cannot get started without it.

A bonded **BLE companion app** for the OBC bike computer: import planned routes
(share sheet / Files), push them to the device, sync tracked rides back. Built
**entirely against a `DeviceTransport` abstraction**, so the app is developed and
UI-tested against a mock before the firmware BLE stack exists.

Wire contract: [`OBCProtocol.md`](OBCProtocol.md) (a *mirror* — the firmware
`obc-ble-interface-spec.md` is canonical and wins any conflict).

## Architecture — the golden rule

> **View models depend only on `DeviceTransport`.** CoreBluetooth appears **only**
> inside `OBCTransport/BLE/`. Mock/dev-panel code lives **only** in `OBCMock`
> behind `#if DEBUG` and must never ship in Release.

Both rules are **test-enforced** (`swift test`): `CoreBluetoothSeamTests` fails if
`import CoreBluetooth` leaks outside `OBCTransport/BLE/` (or the app composition
root); `OBCMock` compiles to an empty module in Release.

Layers (lower may not import higher): `OBCDomain` → `OBCTransport` → `OBCMock`;
`OBCUI` sits on `OBCDomain` + `OBCTransport`; `OBCFormats` on `OBCDomain` only.
The app target is the **only** place that picks a concrete transport
(`OBCCompanionApp.makeTransport()`).

```
companion-ios/
  project.yml            XcodeGen source of truth — EDIT THIS, never the pbxproj
  project.local.yml      personal DEVELOPMENT_TEAM (see Signing)
  OBCCompanion.xcodeproj  gitignored — run `xcodegen generate` after cloning
  OBCCompanion/          app target = composition root only (picks the transport)
  Packages/OBCKit/       local SwiftPM package — builds/tests without the app:
    OBCDomain            pure Sendable value types (Route/Ride/Waypoint/…)
    OBCTransport         DeviceTransport + BondStore + LibraryStore + Codecs;
                         BLE/ = the only place CoreBluetooth is allowed
    OBCFormats           file formats at the edges (GPX/TCX import, ride export)
    OBCMock  (#DEBUG)    MockTransport + MockControl + Scenario presets + fixtures
    OBCUI                SwiftUI component kit (OBCTheme) + feature screens
  OBCCompanionUITests/   XCUITest target — launch-arg driven
```

## Build / test / run

Prereqs: **Xcode 26.x** with a modern iOS simulator runtime installed
(`xcodebuild -downloadPlatform iOS` if `list_sims` is empty), and
**XcodeGen** (`brew install xcodegen`). Min deployment target iOS 17.0.

**First step after cloning (and after editing `project.yml`):**
```bash
cd companion-ios && xcodegen generate      # .xcodeproj is gitignored
```
The pbxproj is deliberately **not** committed (this is the canonical statement
of that policy): committing it baked personal signing state into shared history
and could silently drift from `project.yml` with nothing to catch it.

**Unit tests — host, no simulator (fast, do this first):**
```bash
cd companion-ios/Packages/OBCKit && swift test
```
CI (the repo's `ci.yml`) runs this same `swift test` plus a simulator app build
(Debug + Release mock-seam check) on every PR touching `companion-ios/**`.

**App build/run — via XcodeBuildMCP** (preferred for agents). Set session
defaults once, then the build tools take no path args:
```
session_set_defaults { projectPath: "companion-ios/OBCCompanion.xcodeproj",
                       scheme: "OBCCompanion", simulatorName: "<from list_sims>" }
build_run_sim {}        // build + boot + install + launch
test_sim {}             // app + XCUITests on the sim
```
`session_show_defaults` prints the active set. For a physical iPhone:
`list_devices {}` → `session_set_defaults { deviceId }` → `build_run_device {}`.

Raw equivalents (CI / no MCP): `xcodebuild build|test -scheme OBCCompanion
-destination 'platform=iOS Simulator,name=<sim>'`. Any CI workflow's first step
must be `xcodegen generate`.

**Signing (physical device only):** set `DEVELOPMENT_TEAM` in
**`project.local.yml`** (merged via `project.yml`'s `include:`) — never
`project.yml` or the pbxproj. Then `git update-index --skip-worktree
companion-ios/project.local.yml` and re-run `xcodegen generate`.

## Running against the mock

Debug defaults to `MockTransport` (no Bluetooth in the simulator). Scenarios,
fixtures, and connection state are chosen via **launch arguments** — the
authoritative list is [`MockLaunchOptions.swift`](Packages/OBCKit/Sources/OBCMock/MockLaunchOptions.swift)
and the scenario presets are [`Scenario.swift`](Packages/OBCKit/Sources/OBCMock/Scenario.swift).
Common ones: `-OBCScenario <name>`, `-OBCFixtures default|empty|large|trips|website`,
`-OBCConnection <state>`, `-OBCImportSample gpx|tcx|bad|grimsel`, `-OBCTransport ble`
(force real BLE, device only). These names are **stable automation API** —
XCUITests depend on them.

The Debug-only, non-UI Weather Request transport harness (WX3) is
`-OBCTransport ble -OBCWeatherRequestHarness`. Pair once in a normal BLE run first; the harness then
runs one bounded authenticated read of the weather-request context without the ordinary foreground
link, and logs it. No UI, no scheduler, no bundle upload.

`-OBCHideMockHUD` keeps the mock transport but removes its Debug status tag for clean automated
captures; `-OBCDisableAnimations` runs the UI unanimated so a capture can't catch a transition
mid-flight, and `-OBCHoldConfirmations` parks the timed confirmation states (sync check, synced
line, the upload sheet's self-dismiss) so a shot of one isn't a race. The landing-page captures and their CI drift check live in
`scripts/capture-website-screenshots.sh` — that gate compares pixels, so anything the capture
screenshots must be *waited for*, never assumed (see `WebsiteScreenshotTests`).

Dev control panel (Debug): shake the sim (⌃⌘Z) or launch with
`-OBCShowDevPanel` for live `MockControl` knobs; `-OBCShowUIGallery` opens the
component gallery.

## Design source of truth

**`project/OBC Companion App.dc.html`** (repo root) is canonical for layout,
copy, and states — **read the HTML/CSS directly**, don't screenshot it. Tokens
live in `project/_ds/…/tokens/` and are mapped in
[`OBCTheme`](Packages/OBCKit/Sources/OBCUI/Theme/OBCTheme.swift). **Use the
`OBCUI` component kit; never restyle ad hoc or introduce colors outside the
tokens.** One deliberate deviation: track previews use a real MapKit basemap
(grid fallback when offline / no geometry). Copy is **English-only** for now —
verbatim from the design source; don't introduce localization machinery
piecemeal (if that ever flips, it's its own issue).

## Conventions

- **SwiftUI + async/await + `AsyncStream`**; `@Observable` view models (not
  `ObservableObject`). Swift 6 language mode everywhere — keep domain types
  `Sendable`.
- **Formats at the edges, canonical models in the middle.** Files decode into
  `ImportedRoute`, device bytes into `Ride`; switching a format must stay a
  one-conformer change. Never parse/encode outside `OBCFormats`/`Codecs`.
- **The `LibraryStore` persists canonical models, never wire bytes.** Screens
  read the store first, then reconcile with the device.
- **Device-bound writes must surface failure or self-heal on reconnect** —
  never a silent `try?` across the link (the H3 rename's `DeviceNameReconciler`
  is the pattern). Phone-local library writes stay best-effort.
- **Open-ended stream loops in view models** (`for await` over `transport.state`
  etc.): `[weak self]` + `guard let self` **inside** the loop, store the `Task`,
  cancel it in `deinit` (cancellation only — no main-actor state in `deinit`).
- **New test suites use Swift Testing** (`import Testing`, `@Test`/`#expect`);
  existing XCTest suites migrate only when substantially rewritten anyway —
  never as drive-by churn. Both frameworks coexist in the same test target
  under `swift test`; `CRC32Tests` is the template (incl. `@Test(arguments:)`).
- **One feature per folder** under `OBCUI` (view + its view model).
- **Never hand-edit the pbxproj** — it's regenerated; edit `project.yml`.

## What NOT to do

- No blocking full-screen spinner — use inline / skeleton states.
- No new colors outside the OBC tokens.
- No cloud / account language — this app is phone ↔ device only.
- Never ship mock/panel code in Release (the seam is tested — keep it that way).
