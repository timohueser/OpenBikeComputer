# EchoHarness — the A5 CoC echo rig

A macOS command-line tool that drives the firmware's **A5 L2CAP CoC echo loopback**
([issue #273](https://github.com/timohueser/OpenBikeComputer/issues/273)) from a terminal — the
data-plane oracle that **isn't the iOS app**, so a failure localizes to one side of the link.

It reuses the *actual* app transport code from `OBCKit/OBCTransport`: the pinned `GATT` UUID map, the
`L2CAPByteChannel` + `BLEChannel` byte plane, the `TransferControl` / `StatusMessage` descriptors, and
`CRC32`. Only the `CBCentralManager` bring-up (scan → connect → discover → read PSM → open CoC) is
harness-owned, because `BLETransport`'s semantic `DeviceTransport` API has no echo verb — echo is a
dev/test object type (S0 §4.1, type 8).

This is a **seed**: the [A9 soak rig](https://github.com/timohueser/OpenBikeComputer/issues/277) grows
from it (induced disconnects + offset-resume, the 50-consecutive-upload gate).

## What it does per object

1. Generate a random payload and its CRC-32/IEEE.
2. Write a `TransferControl(op: .upload, type: .echo, totalLen, crc32)` to the `transferControl`
   characteristic — arming the device's data plane.
3. Stream the payload up the CoC **and** read it back down **concurrently** (a one-direction-at-a-time
   drive would deadlock on the CoC's bidirectional credit flow).
4. Assert the bytes came back **byte-identical** and the device notified `transferResult: committed`.

`--corrupt` flips one byte per object (leaving the announced CRC intact) and instead asserts the
device rejects it with the S0 `crcMismatch` status (§6) — the CRC that the on-air link CRC can't catch.

## Run it

Needs a Mac with Bluetooth, and the device powered + advertising (a `ble` build:
`cargo run --release --no-default-features --features ble` in `firmware/obc-fw-nrf54l`). macOS will
prompt for Bluetooth permission on the first run.

```sh
cd companion-ios/EchoHarness

swift run echo-harness                          # 100 × 32 KB, the default smoke run
swift run echo-harness --count 1000 --size 32768   # the A5 definition-of-done run
swift run echo-harness --count 50 --corrupt        # CRC fault injection → crcMismatch
swift run echo-harness --help
```

Exit code is non-zero if any object fails. Each object prints its round-trip time + throughput; the
run ends with an aggregate kB/s (the number that sizes route/ride transfer times).

## Not here yet (A9)

Induced mid-transfer disconnects + offset-resume, and the scripted 50-upload reliability gate, land
when this grows into the A9 soak harness.
