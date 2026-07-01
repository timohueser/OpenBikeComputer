# OBC wire-protocol contract — iOS mirror (B-S0)

The protocol surface the companion app codes against: the GATT control plane, the
L2CAP CoC data plane, the typed object model, and the two design-surfaced deltas.

> ### Divergence policy — read first
>
> **This file is a mirror, not the freeze.** The wire contract is owned by the
> **firmware `S0` freeze** and **`obc-ble-interface-spec.md`** (the canonical
> source of truth for services, transport, and security). If this document and
> firmware `S0` disagree, **firmware `S0` wins and this file is corrected** — never
> the reverse. Its only job is to let `B1`
> ([#237](https://github.com/timohueser/OpenBikeComputer/issues/237)) build against
> a fixed definition and to track the two iOS-surfaced deltas below.
>
> Concrete values that firmware `S0` owns and this file must **not** invent: the
> custom 128-bit service/characteristic UUIDs, the exact byte widths and endianness
> of the frame header fields, and the CRC-32 polynomial/seed. They are named here
> by role; pin the numbers from the spec when it lands.

**Execution-plan sources** (until the spec file lands in the repo): the BLE
implementation epic brief §3 (shared protocol contract) and §4 (milestones), and
the firmware Track-A issues it decomposes into — `A4` (GATT: DIS + BAS + OBC
Control), `A5` (L2CAP CoC + framing), `A8` (LESC pairing + bonding). This app
epic only *references* the contract; it does not define it (epic
[#234](https://github.com/timohueser/OpenBikeComputer/issues/234)).

---

## Versioning

A single `protocol_version` (unsigned) is exposed by the device via **DIS / OBC
Control** and pinned app-side as `OBCProtocol.version`
([OBCProtocol.swift](Packages/OBCKit/Sources/OBCDomain/OBCProtocol.swift), currently `1`).

**Mismatch behavior — surface, don't crash.** On connect, `B1` reads the device's
reported version into `DeviceInfo.protocolVersion` and compares it to
`OBCProtocol.version`. A mismatch is reported as
`DeviceError.protocolMismatch(expected:found:)` and surfaced in the UI (a banner /
disabled sync) — it must **never** trap, force-unwrap, or silently proceed with an
incompatible decode. Bump `OBCProtocol.version` only in lockstep with a firmware
`S0` change (guarded by `ProtocolContractTests.testProtocolVersionIsPinned`).

---

## Transport — two planes

### Control plane = GATT

| Service | UUID | Role | App use |
|---|---|---|---|
| **DIS** — Device Information | `0x180A` (SIG) | fw / hw / serial + `protocol_version` | `deviceInfo()` → `DeviceInfo` |
| **BAS** — Battery | `0x180F` (SIG) | battery % (notify) | `battery` stream → top bar |
| **OBC Control** | custom 128-bit *(firmware `S0`)* | command + bulk-transfer orchestration | see below |

**OBC Control characteristics** (custom 128-bit UUIDs assigned by firmware `S0`):

| Characteristic | Properties | Role |
|---|---|---|
| `Command` | write | device commands (enter pairing mode, start/select transfer, delete) |
| `Status` | notify | device → app status / progress / result |
| `RideList` | read + notify | enumerable tracked rides (id, start time, size, name) → `[RideSummary]` |
| `Config` | read + write | device config blob incl. **device name** (see *Delta 1*) → `DeviceConfig` |
| `TransferControl` | write + notify | begin / offset-resume / cancel of a CoC object transfer |
| `Diagnostics` | read | ring-buffer / crash-log reader → `readDiagnostics()` |
| **`PSM`** | read | the dynamically-assigned L2CAP CoC PSM the app opens the channel on |

The app-facing characteristics require an **encrypted** link (LESC bond, firmware
`A8`); the phone is the only bonded peer.

### Data plane = L2CAP CoC

All bulk payloads move over a **single L2CAP connection-oriented channel** with
**credit-based flow control**. The device (peripheral) publishes the channel and
advertises its dynamic PSM via the GATT `PSM` characteristic; the app (central)
reads the PSM, then opens a `CBL2CAPChannel`. Prefer **2M PHY + DLE** (251-byte
PDU); align the L2CAP MTU to the PDU.

**One framing protocol, typed objects.** Every object is chunked behind a frame
header:

```
Frame header (widths/endianness pinned by firmware S0):
  type       enum   { route, ride, config_blob, diagnostics, firmware(reserved) }
  object_id  id     which object this frame belongs to
  total_len  len    full object size in bytes
  offset     len    byte offset of this chunk within the object   ← resume anchor
  chunk_len  len    bytes in this chunk
  crc32      u32    checksum (validated before commit)
```

- **Resumable** — a dropped transfer restarts at the last committed `offset`
  (`TransferProgress.offset`, `TransferHandle.resume()`).
- **Cancelable** — an abort message over `TransferControl` + channel teardown;
  clean on both ends.
- **CRC validated before commit on both ends** — a failing object is **rejected,
  never committed** (`DeviceError.crcMismatch`). App and device both check.

`firmware` is **reserved** for a future OTA type — no codec in this epic.

**B1 lands this framing** in `OBCTransport`: `Frame.swift` (header codec),
`CRC32.swift`, and `TransferAssembler.swift` (reassembly), driven by `BLEChannel`
over a `ByteChannel` — the L2CAP CoC (`L2CAPByteChannel`) on the real path, an
in-memory pipe in tests. The concrete field widths, little-endian layout, and
CRC-32/IEEE variant are **provisional**, centralized in `FrameFormat` / `CRC32`
for a single-spot repin once firmware `S0` freezes the numbers. Likewise the
custom GATT UUIDs live provisionally in `BLE/GATT.swift`.

---

## Object formats

Routes and rides both cross the wire as **compact binary**, never XML:

- **Routes** — the phone converts the imported track to the compact binary route
  format; **the device never parses XML**. See *Delta 2*.
- **Rides** — stored and transferred as compact binary; any FIT/GPX conversion
  happens on the phone.

The byte layout of each object is owned by firmware `S0` (and, for routes, the
existing on-device map/route formats). `B1`'s `BLEChannel` owns the codecs; this
epic pins only the transport + object *types*, not the codecs.

---

## Delta 1 — device name lives in `Config`

The device name is a field of the wire **`Config`** object. Renaming the device
(H3) is a **`writeConfig`** with a changed `name` — there is **no** separate
rename command. This is a hard requirement on the contract, mirrored in
[`DeviceConfig.name`](Packages/OBCKit/Sources/OBCDomain/DeviceConfig.swift).

## Delta 2 — GPX **and** TCX import

The phone accepts **both GPX and TCX** and converts each to the compact binary
route format before upload; the device never parses either XML dialect. The share
sheet / Files import (H5) names both formats and rejects anything else. Mirrored
in [`RouteSource`](Packages/OBCKit/Sources/OBCDomain/Route.swift) (`.gpx` / `.tcx`).

> **Flag to the firmware track:** both deltas must also be reflected in
> `obc-ble-interface-spec.md` when it is written — the `Config` object must carry a
> name field, and the route object's documented provenance must acknowledge the
> GPX/TCX-on-phone conversion. Tracked here so the app and spec don't drift.

---

## Swift type map

The domain types `B1` finalizes live in `OBCKit`'s `OBCDomain` module (minimal
`Sendable` value types — **no codecs**; those are `B1`/`BLEChannel`):

| Type | File | Contract role |
|---|---|---|
| `OBCProtocol.version` | `OBCProtocol.swift` | the pinned `protocol_version` |
| `DeviceInfo` | `DeviceInfo.swift` | DIS mirror (name, fw/hw, serial, protocolVersion) |
| `DeviceConfig` | `DeviceConfig.swift` | `Config` blob — **incl. `name`** (Delta 1) |
| `RouteSummary` / `RouteBlob` | `Route.swift` | route metadata + opaque binary payload |
| `RouteSource` | `Route.swift` | GPX / TCX (Delta 2) |
| `RideSummary` | `Ride.swift` | enumerable tracked ride (`RideList`) |
| `Waypoint` | `Waypoint.swift` | route waypoint (W1) — rides in `RouteBlob` |
| `Coordinate` / `TrackPreview` | `Geo.swift` | normalized polyline for `GPSTrackPreview` (B11) |
| `TransferProgress` | `TransferProgress.swift` | CoC transfer progress + resume `offset` |
| `DeviceError` | `DeviceError.swift` | typed failures incl. `crcMismatch`, `protocolMismatch`, radio states |
| `ConnectionState` | `ConnectionState.swift` | link lifecycle for `DeviceTransport.state` |

`RouteID` / `RideID` are thin `String` wrappers in the same files. **B1
([#237](https://github.com/timohueser/OpenBikeComputer/issues/237)) is landed:**
the finalized `DeviceTransport` protocol + `TransferHandle` live in `OBCTransport`,
the real conformer in `OBCTransport/BLE/` (`BLETransport`, `BLEChannel`,
`L2CAPByteChannel`, `GATT`), and the framing/codec + domain types are unit-tested
without hardware. The **real-path** (live GATT/CoC) is gated on firmware `A4`/`A5`.
