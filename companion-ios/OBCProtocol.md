# OBC wire-protocol contract — iOS mirror (B-S0)

The protocol surface the companion app codes against: the GATT control plane, the
L2CAP CoC data plane, the typed object model, and the two design-surfaced deltas.

> ### Divergence policy — read first
>
> **This file is a mirror, not the freeze.** The wire contract is owned by
> **[`obc-ble-interface-spec.md`](../obc-ble-interface-spec.md)** (repo root — the
> firmware `S0` freeze, landed in PR #279): the canonical source of truth for
> services, transport, and security. If this document and that spec disagree,
> **the spec wins and this file is corrected** — never the reverse.
>
> **Status: reconciled with `S0`.** The custom UUIDs, descriptor layouts, and
> CRC-32 parameters below are the frozen values (spec §9 was the repin checklist);
> the shared fixtures in [`protocol-vectors/`](../protocol-vectors/) pin them
> byte-exactly on both sides (`ProtocolVectorTests` here, `obc-vectors` in the
> firmware workspace). The remaining firmware Track-A issues — `A4` (GATT), `A5`
> (L2CAP CoC + transfer engine), `A8` (LESC pairing + bonding) — implement this
> contract; this app epic only *references* it (epic
> [#234](https://github.com/timohueser/OpenBikeComputer/issues/234)).

---

## Versioning

A single `protocol_version` (`u16`, currently **1**) is exposed by the device on
the OBC Control `protocolVersion` characteristic (readable without encryption)
and pinned app-side as `OBCProtocol.version`
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
| **DIS** — Device Information | `0x180A` (SIG) | fw / hw / serial | `deviceInfo()` → `DeviceInfo` |
| **BAS** — Battery | `0x180F` (SIG) | battery % (notify) | `battery` stream → top bar |
| **OBC Control** | `3C920000-9916-4EBA-ABC2-342FE08F6B10` | command + bulk-transfer orchestration | see below |

**OBC Control characteristics** — base `3C92XXXX-9916-4EBA-ABC2-342FE08F6B10`,
the 16-bit `XXXX` block selects the characteristic (spec §3.3; constants in
`BLE/GATT.swift`). The earlier `0BC0…` placeholders sat on the Bluetooth SIG base
UUID, which custom services must not use — `S0` replaced them:

| `XXXX` | Characteristic | Properties | Role |
|---|---|---|---|
| `0001` | `command` | write | small imperatives: `deleteObject` (cmd 1: `type u8 · id u16`) — spec §4.4 |
| `0002` | `status` | notify | typed device → app messages (`StatusMessage`: transferResult / storeChanged / commandResult) — spec §4.3 |
| `0003` | `objectStore` | read + notify | 10-byte store digest (revision + route/ride counts); **full lists are CoC objects** — they outgrow the 512-byte ATT attribute cap |
| `0004` | `config` | read + write | the Config object incl. **device name** (see *Delta 1*) → `DeviceConfig` |
| `0005` | `transferControl` | write + notify | open / abort a CoC object transfer (§ below) |
| `0006` | `diagnostics` | read | **reserved** — diagnostics cross the CoC as object type 4 |
| `0007` | `psm` | read | the dynamically-assigned L2CAP CoC PSM the app opens the channel on |
| `0008` | `protocolVersion` | read | `u16` LE, readable without encryption — the connect-time version check |

The app-facing characteristics require an **encrypted, LESC-authenticated** link
(firmware `A8`); the phone is the only bonded peer. DIS/BAS/`protocolVersion` stay
open so the app can identity/version-check before pairing.

**Pairing / bonding (A8, canonical spec §8).** The device is `DisplayOnly`: it
shows a 6-digit **passkey** on its screen, iOS raises the system pairing dialog
(`OBCSystemPairing`), the rider types it — LESC passkey entry, MITM-protected.
Pairing *is* `connect()`: reading a gated characteristic / opening the CoC on the
unencrypted link makes iOS pair, and the encrypted link completing is what
resolves the connect. A declined / wrong passkey surfaces as a pairing failure
(→ D5). One bonded peer; a fresh passkey pairing replaces the stored bond.

**Reconnect.** The device keeps a **stable** static address, so once bonded, iOS
reconnects silently on any contact (no dialog) — power-cycle either side, walk
away and back. The app persists only a `BondRecord` ("we paired, with `<name>`")
for the launch greeting; CoreBluetooth owns the real crypto bond.

**Forget (H2).** `BondStore.clear()` drops the app's record, but iOS keeps the CB
bond until the user also removes it in **Settings ▸ Bluetooth** — the H2 copy
says so. After a true forget, the next contact re-pairs with a fresh passkey (the
device replaces its single bond).

### Data plane = L2CAP CoC

All bulk payloads move over a **single L2CAP connection-oriented channel** with
**credit-based flow control**. The device (peripheral) publishes the channel and
advertises its dynamic PSM via the GATT `PSM` characteristic; the app (central)
reads the PSM, then opens a `CBL2CAPChannel`. Prefer **2M PHY + DLE** (251-byte
PDU); align the L2CAP MTU to the PDU.

**Control-plane descriptor + raw byte stream.** The CoC is a **reliable, ordered**
channel (the BLE Link Layer already CRCs and retransmits every packet), so bulk
transfer carries **no per-chunk framing**. Instead (spec §4.2/§4.3, mirrored in
`Transfer/TransferDescriptor.swift`):

1. A fixed **`TransferControl`** descriptor opens the transfer over the GATT
   `transferControl` characteristic — *before* any payload byte. One 16-byte shape
   serves both directions and abort:

   ```
   TransferControl (16 bytes, little-endian):
     op         u8    1 = upload (app → device) · 2 = download · 3 = abort
     type       u8    { route 1, ride 2, config(reserved) 3, diagnostics 4,
                        firmware(reserved) 5, routeList 6, rideList 7, echo 8 }
     object_id  u16   0xFFFF on upload = "new" (device assigns the id)
     total_len  u32   upload: full object size · download request / abort: 0
     crc32      u32   upload: whole-object CRC-32/IEEE · download request / abort: 0
     offset     u32   always 0 — transfers restart, not resume (shape stability only)
   ```

   For a **download** the device answers with the same 16 bytes as a
   *notification* — `total_len` and `crc32` filled in — before the payload flows.

2. The **CoC carries the raw payload bytes** of the whole object — nothing
   else. The receiver sinks them straight to storage, updating a running CRC (no
   reassembly buffer — the point on a RAM-limited MCU).

3. A **`transferResult`** message closes it over `status` (a typed envelope:
   `msg u8 = 1 · object_id u16 · status u8 {committed, crcMismatch, aborted,
   error, notFound, busy} · committed_offset u32`; for a fresh upload the result
   carries the **assigned** id). `status` also carries `storeChanged` (msg 2) and
   `commandResult` (msg 3) messages — unknown discriminators are ignored.

- **CRC once, end-to-end.** One whole-object CRC verified at commit — a mismatch
  **rejects** the object (`DeviceError.crcMismatch`), never commits it. This is the
  *end-to-end* check the link CRC can't give (encode bugs, storage errors); it is
  **not** a redundant per-packet CRC.
- **Restart, not resume** — an interrupted transfer is re-sent / re-requested
  whole (spec §1 principle 4); the device discards partial uploads. Multi-object
  flows (the B7 ride sync) resume at whole-object granularity: rides that fully
  landed are kept, the rest are re-requested from byte 0.
- **Cancelable** — abort over `TransferControl` + channel teardown; clean both ends.

`firmware` is **reserved** for a future OTA type — no codec in this epic; `echo`
is the `A5` dev/test loopback. At most **one transfer is in flight at a time**.

> **Ratified.** `S0` adopted this descriptor + raw-stream design (it supersedes the
> earlier per-frame `{type, object_id, total_len, offset, chunk_len, crc32}` idea),
> widening the descriptor by one leading `op` byte so the download announce and
> abort — named but byte-less here before the freeze — share the one shape.

**B1 lands this** in `OBCTransport`: `Transfer/TransferDescriptor.swift`
(`TransferControl`, the `StatusMessage` envelope, `ObjectStoreDigest`),
`Transfer/CRC32.swift` (whole-object + streaming `Hasher`; CRC-32/IEEE, check
value `0xCBF43926`), driven by `BLEChannel` (raw streaming,
progress/cancel/resume) over a `ByteChannel` — the L2CAP CoC (`L2CAPByteChannel`)
on the real path, an in-memory pipe in tests. Field widths, the CRC variant, and
the GATT UUIDs (`BLE/GATT.swift`) are **frozen** and pinned byte-exactly against
the shared `protocol-vectors/` fixtures by `ProtocolVectorTests`.

---

## Object formats

Routes and rides both cross the wire as **compact binary**, never XML:

- **Routes** — an **OBCR v2 file, verbatim** (`OBCR_Spec.md`, incl. the v2
  waypoints section): the phone encodes imported GPX/TCX to OBCR before upload;
  **the device never parses XML** (see *Delta 2*) and stores/serves the blob
  byte-for-byte. The E2 **route detail read is pinned as "download the route
  object"** — the app decodes waypoints + the elevation profile from the same
  OBCR bytes it encodes; there is no separate detail codec.
- **Rides** — the **ride object v1** (spec §7.2; `RideObjectCodec`, ratified
  byte-for-byte from this app's B7 codec): any GPX/FIT conversion happens on the
  phone (device bytes → canonical `Ride` → an `OBCFormats` `RideFileEncoder`),
  never straight from the wire bytes.
- **Lists** — `routeList`/`rideList` are CoC objects with fixed 72-byte entries
  (spec §7.4); **diagnostics** is a CoC text blob (spec §7.5).

The byte layout of each object is owned by the spec. The device object codecs
live in `OBCTransport/Codecs/` (`BLEChannel` only moves bytes; the interchange
*file* formats live in `OBCFormats`).

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

> **Resolved:** `S0` ratified both deltas — the `Config` object carries the name
> field (spec §7.3, append-only layout) and the route object's provenance is the
> phone-side GPX/TCX → OBCR conversion (spec §7.1).

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
| `RouteDetail` | `Route.swift` | detail read for E2 (waypoints + elevation profile) — pinned: decoded from the downloaded OBCR v2 route object |
| `RouteSource` | `Route.swift` | GPX / TCX (Delta 2) |
| `RideSummary` | `Ride.swift` | enumerable tracked ride (a `rideList` object entry) |
| `RideDetail` | `Ride.swift` | detail read for E3 (elevation profile) — pinned: decoded from the downloaded ride object |
| `Ride` / `RidePoint` | `Ride.swift` | canonical full ride — device ride codec decodes into it; exports encode from it |
| `ImportedRoute` / `RoutePoint` | `ImportedRoute.swift` | canonical parsed route — every import format decodes into it |
| `Waypoint` | `Waypoint.swift` | route waypoint (W1) — rides in `RouteBlob` |
| `Coordinate` / `TrackPreview` | `Geo.swift` | normalized polyline for `GPSTrackPreview` (B11) |
| `TransferProgress` | `TransferProgress.swift` | CoC transfer progress (bytes done / total) |
| `TransferOutcome` | `TransferProgress.swift` | terminal transfer state (`TransferHandle.outcome`) — a drop stays unresolved/resumable |
| `DeviceError` | `DeviceError.swift` | typed failures incl. `crcMismatch`, `protocolMismatch`, radio states |
| `ConnectionState` | `ConnectionState.swift` | link lifecycle for `DeviceTransport.state` |

`RouteID` / `RideID` are thin `String` wrappers in the same files. **B1
([#237](https://github.com/timohueser/OpenBikeComputer/issues/237)) is landed:**
the finalized `DeviceTransport` protocol + `TransferHandle` live in `OBCTransport`,
the real conformer in `OBCTransport/BLE/` (`BLETransport`, `BLEChannel`,
`L2CAPByteChannel`, `GATT`), and the framing/codec + domain types are unit-tested
without hardware. The **real-path** (live GATT/CoC) is gated on firmware `A4`/`A5`.
