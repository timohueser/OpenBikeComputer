# EchoHarness — the BLE soak + fault-injection rig (A5 → A9)

A macOS command-line tool that drives the firmware's BLE data planes from a terminal — the oracle that
**isn't the iOS app**, so a failure localizes to one side of the link. It began as the A5 L2CAP-CoC echo
loopback ([#273](https://github.com/timohueser/OpenBikeComputer/issues/273)) and is now the **A9
reliability rig** ([#277](https://github.com/timohueser/OpenBikeComputer/issues/277)): scripted soak +
fault injection, run until the golden path survives everything we can throw at it. **This is the
regression net for every future BLE change.**

It reuses the *actual* app transport code from `OBCKit/OBCTransport` — the pinned `GATT` UUID map, the
`L2CAPByteChannel` + `BLEChannel` byte plane, the `TransferControl` / `StatusMessage` / `RouteList`
descriptors, and `CRC32` — so the bytes on the wire are exactly the app's. Only two things are
harness-owned: the `CBCentralManager` bring-up (scan → connect → discover → read PSM → open CoC), and
the **fault injection** — dropping the link mid-transfer, flipping bytes, storming reconnects — because
`BLETransport`'s semantic `DeviceTransport` API has no verb for "echo this" or "fail now".

## The core principle: both ledgers must agree

Every A9 scenario asserts on **two** ledgers and fails if they disagree:

1. **Harness-side** — what the harness observed (bytes byte-identical, the expected `transferResult`
   status, the routeList contents).
2. **Device-side** — the device's own telemetry counters, read back from the **diagnostics blob**
   (spec §7.5, object type 4): `link_connects` / `link_disconnects`, `boot_count` (a reboot ⇒ the
   watchdog fired or the firmware crashed), `routes` / `rides`, `sd`, and the `stack_hw` / `stack_total`
   high-water (the "stack + RAM numbers posted" DoD, read over the link with no RTT).

## Prerequisites

- A Mac with Bluetooth.
- The device running a **`ble` build**, powered + advertising:
  `cargo run --release --no-default-features --features ble` in `firmware/obc-fw-nrf54l`.
- **Paired once.** The CoC requires an encrypted link, so on the first run macOS pops a pairing dialog —
  type the 6-digit passkey shown on the device glass. After that the OS reconnects from the stored bond
  with no dialog, and the storm/drop scenarios reconnect silently.
- One or more real **`.obcr`** route files (varied sizes, incl. a waypoint-bearing one) for the
  scenarios that upload. The app produces these; keep a couple around.

## The scenarios (one command each)

```sh
cd companion-ios/EchoHarness

# ── Golden-path soak — the epic's headline gate ──
# N complete uploads, verify-by-list-read after each, ledgers reconciled (one connection, no reboot,
# no stray drops, the catalog tracking the uploads). Default 50 = the DoD. Pass several files for size
# variety; --no-cleanup accumulates instead of deleting each after verify.
swift run echo-harness soak route.obcr route-waypoints.obcr --count 50

# ── Drop / restart matrix ──
# Kill the link at randomized points (incl. first + last chunk) during upload AND download → the device
# discards the upload partial (routeCount unchanged), the re-upload is byte-identical, a mid-download
# drop recovers by whole re-request. Reconciles: the device saw exactly the drops induced, never rebooted.
swift run echo-harness drop-matrix route.obcr --iterations 20

# ── CRC / corruption injection ──
# Flipped-byte echo → crcMismatch; flipped-byte upload → crcMismatch (nothing committed); malformed
# descriptor → error. Store clean after each. (v2 dropped the descriptor's offset field — transfers
# restart, never resume — so the old non-zero-offset fault class no longer has a wire field to carry.)
swift run echo-harness corruption route.obcr

# ── Connect / disconnect storm (bonding active) ──
# N connect→disconnect cycles; the device's connects/disconnects counters must track the harness exactly.
swift run echo-harness storm --iterations 50

# ── Concurrency probes ──
# A second transfer opened while one is active → busy; a command answered while a transfer is parked
# (control plane stays responsive); 5 back-to-back reconnects with no settle time.
swift run echo-harness concurrency route.obcr

# ── Trip object lifecycle (TR4, #653) ──
# The whole type-9 trip lifecycle over the real byte layer: upload 2 routes + a trip that references them,
# read the tripList (type 10) back, replace the trip reordered by id (its content fingerprint moves), delete
# one member route (the device tolerates the dangling stage and never rewrites the stored trip — stageCount
# holds, the live totals shrink to the resolvable stage, the stored-bytes crc is unchanged), then delete the
# trip → storeChanged(type = trip), the surviving member becoming a top-level route. Pass one file to cycle
# it as both stages, or two for distinct stages.
swift run echo-harness trip-soak route.obcr route-2.obcr

# ── Session churn (the 2026-07-13 instability report) ──
# What the companion app does across a testing session, on ONE connection, with numbers: each cycle
# uploads a route (commit + timed storeChanged), verifies by routeList read, reads the rideList (the
# sync path's read), and deletes the route (command ack + timed storeChanged — the edge the app's
# badge-clear rides on). Every --abort-every cycles an extra upload is aborted mid-bytes (op 3) first,
# proving the transfer engine recycles with no disconnect to clean up behind it. Every wait is bounded,
# so a lost notify fails the run loudly with its cycle number; the report prints min/avg/max latency
# per edge. Crank --iterations for an overnight number-gathering run.
swift run echo-harness churn route.obcr --iterations 30 --abort-every 5

# ── Read the device diagnostics blob directly ──
swift run echo-harness diagnostics --verbose
```

The A5/A6/A7 bring-up subcommands (`echo`, `upload`, `list`, `detail`, `delete`, `abort-test`) are still
here — see `swift run echo-harness --help`. Every scenario exits non-zero on the first failed assertion,
printing the invariant that broke and the two ledgers, so a red run is self-diagnosing.

## The epic's definition-of-done gates (A9)

Run these on hardware (they need a board; the long ones aren't in CI):

| DoD gate | Command |
|---|---|
| **50 consecutive golden-path uploads, zero intervention** | `soak route.obcr --count 50` |
| Full drop/restart matrix green, 100 % recovered | `drop-matrix route.obcr --iterations 20` |
| Fault-injection suite green | `corruption route.obcr` |
| Connect/disconnect storm, counters coherent | `storm --iterations 50` |
| Concurrency probes green | `concurrency route.obcr` |
| Stack high-water + RAM numbers posted | any scenario prints them; `diagnostics` reads them directly |

The **24 h bonded idle soak** ("reconnects on demand at the end, counters coherent, no reboot") is a
wall-clock endurance run, not a harness loop: leave a bonded phone/`storm` idle, then `diagnostics` at the
end and confirm `boot_count` didn't move and a reconnect still works.

## What the firmware side of A9 added

Driven by what a rig like this finds, the `ble` build gained (all in `firmware/obc-fw-nrf54l`):

- A **hardware watchdog** fed by the status loop, gated on the input-plane heartbeat — the last-resort net
  under a synchronous SD wedge in the data plane (which blocks the whole thread-mode task). See the
  watchdog-policy note in [`ble/lifecycle.rs`](../../firmware/obc-fw-nrf54l/src/ble/lifecycle.rs).
- **Every host notify `HOST_OP_TIMEOUT`-bounded**, so a peer that stops draining its ATT queue can't
  stall a plane past the link's supervision timeout — no error path ends in a wedged, non-advertising
  state.
- The **stack high-water** surfaced in the diagnostics blob, so the soak posts the RAM numbers with no RTT.
