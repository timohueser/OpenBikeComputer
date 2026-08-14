# EchoHarness

EchoHarness is the macOS BLE soak and fault-injection client. It reuses the companion app's real
GATT, L2CAP, descriptor and CRC implementations, while owning central-manager setup and deliberate
faults such as disconnects and corruption.

Every scenario reconciles what the harness observed with the device diagnostics ledger. A run
fails if bytes, status, stored objects, link counters, reboot count or stack high-water disagree.

## Prerequisites

- A Mac with Bluetooth.
- A device running the `ble` firmware build and paired once with macOS.
- One or more `.obcr` route files for upload scenarios.

## Scenarios

```sh
cd companion-ios/EchoHarness

swift run echo-harness soak route.obcr route-waypoints.obcr --count 50
swift run echo-harness drop-matrix route.obcr --iterations 20
swift run echo-harness corruption route.obcr
swift run echo-harness storm --iterations 50
swift run echo-harness concurrency route.obcr
swift run echo-harness trip-soak route.obcr route-2.obcr
swift run echo-harness churn route.obcr --iterations 30 --abort-every 5
swift run echo-harness diagnostics --verbose
```

`soak` verifies repeated upload/list round trips. `drop-matrix` interrupts uploads and downloads.
`corruption` exercises CRC and malformed-descriptor failures. `storm` checks bonded reconnects,
`concurrency` probes busy/control-plane behavior, `trip-soak` covers the trip lifecycle, and `churn`
repeats the companion session pattern while timing its notification edges.

The lower-level `echo`, `upload`, `list`, `detail`, `delete`, and `abort-test` commands remain
available; run `swift run echo-harness --help` for their arguments. Every scenario exits non-zero
on the first broken invariant and prints both ledgers.

These are hardware checks and do not run in CI. For an endurance check, leave a bonded client idle,
then run `diagnostics` and confirm that `boot_count` did not move and reconnect still succeeds.
