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
> of the transfer descriptors, and the CRC-32 polynomial/seed. They are named here
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

**Control-plane descriptor + raw byte stream.** The CoC is a **reliable, ordered**
channel (the BLE Link Layer already CRCs and retransmits every packet), so bulk
transfer carries **no per-chunk framing**. Instead:

1. A fixed **`TransferStart`** descriptor opens the transfer over the GATT
   `TransferControl` characteristic — *before* any payload byte:

   ```
   TransferStart (15 bytes, little-endian):
     type          u8    { route, ride, config_blob, diagnostics, firmware(reserved) }
     object_id     u16
     total_len     u32   full object size
     crc32         u32   whole-object CRC-32/IEEE, verified at commit
     resume_offset u32   byte offset to start from (0 = fresh)   ← resume anchor
   ```

2. The **CoC carries the raw payload bytes** of `object[resume_offset…]` — nothing
   else. The receiver sinks them straight to storage, updating a running CRC (no
   reassembly buffer — the point on a RAM-limited MCU).

3. A fixed **`TransferResult`** descriptor closes it over `Status`
   (`object_id u16 · status u8 {committed, crcMismatch, aborted, error} · committed_offset u32`).

- **CRC once, end-to-end.** One whole-object CRC verified at commit — a mismatch
  **rejects** the object (`DeviceError.crcMismatch`), never commits it. This is the
  *end-to-end* check the link CRC can't give (encode bugs, storage errors); it is
  **not** a redundant per-packet CRC.
- **Resumable** — a dropped upload restarts from `committed_offset` (the device's
  durable byte count, reported in `TransferResult`); byte-exact, no re-sent frame.
- **Cancelable** — abort over `TransferControl` + channel teardown; clean both ends.

`firmware` is **reserved** for a future OTA type — no codec in this epic.

> **Flag to the firmware / `S0` track.** This is the **iOS recommendation** for the
> data plane, chosen to be cheap on the nRF54L (no per-chunk headers to parse, no
> validate buffer, one CRC pass, byte-exact resume). It supersedes an earlier
> per-frame `{type, object_id, total_len, offset, chunk_len, crc32}` design. `S0`
> should ratify it; if it diverges, `S0` wins and this file + the code are corrected.

**B1 lands this** in `OBCTransport`: `Transfer/TransferDescriptor.swift` (the two
descriptors), `Transfer/CRC32.swift` (whole-object + streaming `Hasher`), driven by
`BLEChannel` (raw streaming, progress/cancel/resume) over a `ByteChannel` — the
L2CAP CoC (`L2CAPByteChannel`) on the real path, an in-memory pipe in tests. Field
widths, the CRC-32/IEEE variant, and the custom GATT UUIDs (`BLE/GATT.swift`) are
**provisional**, centralized for a single-spot repin once `S0` freezes them.

---

## Object formats

Routes and rides both cross the wire as **compact binary**, never XML:

- **Routes** — the phone converts the imported track to the compact binary route
  format; **the device never parses XML**. See *Delta 2*.
- **Rides** — stored and transferred as compact binary; any GPX/FIT conversion
  happens on the phone (device bytes → canonical `Ride` → an `OBCFormats`
  `RideFileEncoder`), never straight from the wire bytes.

The byte layout of each object is owned by firmware `S0` (and, for routes, the
existing on-device map/route formats). The device object codecs live in
`OBCTransport/Codecs/` (`BLEChannel` only moves bytes; the interchange *file*
formats live in `OBCFormats`); this epic pins only the transport + object
*types*, not the codecs.

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
| `RouteDetail` | `Route.swift` | detail read for E2 (waypoints + elevation profile) — **provisional**, wire mapping lands with `S0` |
| `RouteSource` | `Route.swift` | GPX / TCX (Delta 2) |
| `RideSummary` | `Ride.swift` | enumerable tracked ride (`RideList`) |
| `RideDetail` | `Ride.swift` | detail read for E3 (elevation profile) — **provisional** like `RouteDetail` |
| `Ride` / `RidePoint` | `Ride.swift` | canonical full ride — device ride codec decodes into it; exports encode from it |
| `ImportedRoute` / `RoutePoint` | `ImportedRoute.swift` | canonical parsed route — every import format decodes into it |
| `Waypoint` | `Waypoint.swift` | route waypoint (W1) — rides in `RouteBlob` |
| `Coordinate` / `TrackPreview` | `Geo.swift` | normalized polyline for `GPSTrackPreview` (B11) |
| `TransferProgress` | `TransferProgress.swift` | CoC transfer progress + resume `offset` |
| `TransferOutcome` | `TransferProgress.swift` | terminal transfer state (`TransferHandle.outcome`) — a drop stays unresolved/resumable |
| `DeviceError` | `DeviceError.swift` | typed failures incl. `crcMismatch`, `protocolMismatch`, radio states |
| `ConnectionState` | `ConnectionState.swift` | link lifecycle for `DeviceTransport.state` |

`RouteID` / `RideID` are thin `String` wrappers in the same files. **B1
([#237](https://github.com/timohueser/OpenBikeComputer/issues/237)) is landed:**
the finalized `DeviceTransport` protocol + `TransferHandle` live in `OBCTransport`,
the real conformer in `OBCTransport/BLE/` (`BLETransport`, `BLEChannel`,
`L2CAPByteChannel`, `GATT`), and the framing/codec + domain types are unit-tested
without hardware. The **real-path** (live GATT/CoC) is gated on firmware `A4`/`A5`.
