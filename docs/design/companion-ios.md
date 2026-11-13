---
system: companion-ios
kind: shell
purpose: "Runs the iPhone app that plans routes, sends them to the device and gets rides back."
owns:
  - companion-ios/**
depends_on: []
test_depends_on: []
status: approved
---

# Companion iOS

## What it does

The companion app runs on the rider's iPhone. The rider plans or imports a route and sends it to the device over Bluetooth. The app gets the recorded rides back from the device. It also keeps offline maps for the route planner, and tells the rider when a new firmware version is published.

The app entry point [`OBCCompanionApp.swift`](../../companion-ios/OBCCompanion/OBCCompanionApp.swift) is the only place that selects how the app talks to the device: the real Bluetooth link, or a test double (`MockTransport`) in Debug builds. The screens, the device protocol and the planner client are in the Swift package [`OBCKit`](../../companion-ios/Packages/OBCKit/Package.swift). The app links two Rust libraries: the client of the device's object protocol (`OBCHost`), and the offline router and search (`planner-service`).

The app does not define the wire format. The [Bluetooth contract](../../specs/ble-interface.md) defines it.

## Rules that must hold

| Rule | Check |
| --- | --- |
| Only the Bluetooth folder of `OBCTransport` imports CoreBluetooth. | `testCoreBluetoothConfinedToTheBLEFolder` in [`CoreBluetoothSeamTests.swift`](../../companion-ios/Packages/OBCKit/Tests/OBCTransportTests/CoreBluetoothSeamTests.swift) |
| The app entry point does not import CoreBluetooth. | `testAppCompositionRootNeverImportsCoreBluetooth` in [`CoreBluetoothSeamTests.swift`](../../companion-ios/Packages/OBCKit/Tests/OBCTransportTests/CoreBluetoothSeamTests.swift) |
| A Release build contains no test-double code. | The step `Mock marker absent from Release` in [`ci.yml`](../../.github/workflows/ci.yml), with `testDebugBuildMarkerIsCompiledIn` in [`MockTransportTests.swift`](../../companion-ios/Packages/OBCKit/Tests/OBCMockTests/MockTransportTests.swift) |

## Decisions

- **Screens depend on small protocols, not on Bluetooth.** Each view model uses a protocol that `OBCTransport` declares for one capability. So the same screens run on CoreBluetooth on a phone, and on the test double in tests and in the simulator.
