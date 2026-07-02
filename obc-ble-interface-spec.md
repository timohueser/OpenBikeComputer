# OBC BLE Interface Specification (v1)

The normative wire contract between the OpenBikeComputer device (nRF54L
firmware, BLE peripheral) and the companion app (iOS, BLE central): advertising,
the GATT control plane, the L2CAP CoC data plane, and the byte layout of every
object that crosses the link. It sits next to [`OBCM_Spec.md`](OBCM_Spec.md)
(map format) and [`OBCR_Spec.md`](OBCR_Spec.md) (route format) and is the
canonical source the firmware Track-A issues (epic #267) implement.

> **This document is canonical.** The iOS mirror
> ([`companion-ios/OBCProtocol.md`](companion-ios/OBCProtocol.md)) defers to it:
> where they disagree, this spec wins and the mirror is corrected. §9 lists the
> deliberate divergences from the mirror's provisional values so the app-side
> repin is a checklist, not a diff hunt.

All multi-byte integers are **little-endian** (matching OBCM/OBCR). Shared
binary test vectors pinning these layouts live in
[`protocol-vectors/`](protocol-vectors/) and are consumed by both `cargo test`
(firmware) and `swift test` (app).

## Design principles

1. **Two planes.** GATT carries small, typed control state (identity, config,
   transfer orchestration, notifications). Bulk bytes move over a single L2CAP
   connection-oriented channel. Nothing large ever crosses GATT — the 512-byte
   ATT attribute cap is a hard wall, not a soft budget.
2. **The CoC is a raw byte pipe.** The BLE Link Layer already CRCs and
   retransmits every packet, so the channel is reliable and ordered. Bulk
   transfer therefore has **no per-chunk framing**: a control-plane descriptor
   announces the transfer, the CoC carries exactly the object's payload bytes,
   and one whole-object CRC-32 is verified at commit. The MCU sinks bytes
   straight to storage with a running CRC — no reassembly buffer.
3. **Objects are files the device already speaks.** A route crosses the wire as
   OBCR v2 bytes and is written to SD verbatim; a ride crosses as the compact
   ride object (§7.2). The phone does all format conversion (GPX/TCX → OBCR);
   the device never parses XML.
4. **Resumable by offset, always.** Every transfer can restart from the
   receiver's durable byte count. A drop costs the un-flushed tail, never the
   transfer.
5. **Versioned once.** A single `protocol_version` covers this whole contract.
   Object layouts carry their own version bytes where they live (OBCR header,
   ride object) so they can evolve without a protocol bump.

---

## 1. Protocol version

`protocol_version` is an unsigned 16-bit integer, **currently `1`**, exposed as
a read-only GATT characteristic (§3.3). It covers everything in this document:
UUIDs, descriptor layouts, object types, and status codes.

- The app reads it on every connect, before any other OBC Control traffic.
- On mismatch the app **surfaces and stops** (banner / disabled sync) — it must
  never trap or attempt a best-effort decode. The device ignores traffic it
  can't parse and answers unknown commands with `unknown` (§4.4).
- Additive, compatible changes (a new object type id, a new command, a new
  waypoint type) do **not** bump the version; changing an existing layout does.

## 2. Advertising

- **Device name**: `OBC-XXXX`, where `XXXX` is the last four uppercase hex
  digits of the serial number (§3.1). This is the *factory* name; the
  user-facing name lives in the Config object (§7.3) and, when set, replaces
  the factory name in the advertisement (truncated to fit — the Config field is
  authoritative, the advertised string is display-only).
- **Payload**: AD Flags (LE General Discoverable, BR/EDR unsupported) + the
  128-bit OBC Control service UUID (Complete List). The device name goes in the
  scan response if it doesn't fit the primary PDU.
- **Intervals**: *fast* advertising at **40 ms** for **30 s** after power-on and
  after every disconnect, then *slow* advertising at **1000 ms** indefinitely.
- **Policy — "always just works"**: the device advertises connectable whenever
  it is powered and unconnected. There is no advertising timeout and no
  "pairing mode" gate for reconnection. Once bonded (A8), the device still
  advertises generally but rejects pairing and bonded-data access from any peer
  other than the bonded phone (§8); a fresh pairing requires the user to clear
  the bond on the device.

## 3. GATT control plane

Three services. The SIG services are open (readable before pairing); the OBC
Control service is encrypted once bonding lands (§8).

### 3.1 Device Information Service — `0x180A` (SIG)

| Characteristic | UUID | Value |
|---|---|---|
| Firmware Revision String | `0x2A26` | UTF-8 semver of the firmware, e.g. `0.4.0` |
| Hardware Revision String | `0x2A27` | UTF-8 board id, e.g. `nrf54l15-dk`, `obc-lm20-r1` |
| Serial Number String | `0x2A25` | 16 uppercase hex digits — the nRF `FICR.DEVICEID` |

### 3.2 Battery Service — `0x180F` (SIG)

| Characteristic | UUID | Value |
|---|---|---|
| Battery Level | `0x2A19` | `u8` percent, read + notify |

### 3.3 OBC Control service (custom)

Base UUID (random; **not** derived from the SIG base): the 16-bit block
`XXXX` in `3C92XXXX-9916-4EBA-ABC2-342FE08F6B10` selects the entity.

| `XXXX` | Entity | Properties | Role |
|---|---|---|---|
| `0000` | **OBC Control service** | — | primary service |
| `0001` | `command` | write | small imperative commands (§4.4) |
| `0002` | `status` | notify | typed device → app messages (§4.3) |
| `0003` | `objectStore` | read + notify | store digest: revision + object counts (§4.5) |
| `0004` | `config` | read + write | the Config object (§7.3), whole-blob |
| `0005` | `transferControl` | write + notify | open / resume / abort a CoC transfer (§4.2) |
| `0006` | `diagnostics` | read | reserved — diagnostics cross the CoC (§7.5); reads return 0 bytes |
| `0007` | `psm` | read | `u16` — the dynamic L2CAP PSM the app opens the CoC on |
| `0008` | `protocolVersion` | read | `u16` — §1. Readable **without** encryption |

Concrete UUIDs, for the record:

```
service          3C920000-9916-4EBA-ABC2-342FE08F6B10
command          3C920001-9916-4EBA-ABC2-342FE08F6B10
status           3C920002-9916-4EBA-ABC2-342FE08F6B10
objectStore      3C920003-9916-4EBA-ABC2-342FE08F6B10
config           3C920004-9916-4EBA-ABC2-342FE08F6B10
transferControl  3C920005-9916-4EBA-ABC2-342FE08F6B10
diagnostics      3C920006-9916-4EBA-ABC2-342FE08F6B10
psm              3C920007-9916-4EBA-ABC2-342FE08F6B10
protocolVersion  3C920008-9916-4EBA-ABC2-342FE08F6B10
```

The `config` characteristic carries the Config object (§7.3) directly — it is
the one object small enough (≤ 128 bytes, §7.3) to live on GATT, and reading /
writing it whole keeps rename (Delta 1) a plain characteristic write.

### 3.4 Connection parameters

The device requests, and the app accepts where the OS allows: **2M PHY**, data
length extension (**251-byte PDUs**), ATT MTU **247**. The L2CAP CoC MPS is
aligned to the PDU so one SDU chunk of **244 bytes** rides in one packet.
These are preferences, not requirements — the protocol is correct at any
negotiated MTU, just slower.

---

## 4. Transfers, status, and commands

### 4.1 Object model

Every bulk payload is a typed **object**:

| `type` | Object | Direction | Payload |
|---|---|---|---|
| `1` | `route` | app → device (upload), device → app (detail read) | an OBCR v2 file, §7.1 |
| `2` | `ride` | device → app | ride object v1, §7.2 |
| `3` | `config` | — | reserved on the CoC; Config crosses GATT (§3.3) |
| `4` | `diagnostics` | device → app | diagnostics blob, §7.5 |
| `5` | `firmware` | — | **reserved** for OTA (M4); no layout in this spec |
| `6` | `routeList` | device → app | list object, §7.4 |
| `7` | `rideList` | device → app | list object, §7.4 |
| `8` | `echo` | both | dev/test only: device streams back what it received (A5's loopback) |
| `9`–`15` | — | — | reserved (sensors, M4) |

**Object ids** are `u16`, assigned by the device, stable for the life of the
stored object, and enumerated by the list objects. Conventions:

- `0xFFFF` on an upload means "new" — the device assigns an id and reports it
  in the `transferResult` (§4.3). Uploading to an existing id replaces that
  object atomically (commit-then-swap; a failed CRC never touches the old copy).
- Objects that exist once (`routeList`, `rideList`, `diagnostics`, `echo`) use
  object id `0`.

At most **one transfer is in flight at a time** — the CoC carries exactly one
object's bytes between a `transferControl` open and its `transferResult`. A
second open while one is active is answered with `busy`.

### 4.2 `transferControl` — the transfer descriptor

One fixed **16-byte** descriptor shape serves both directions and abort.
Written by the app; notified by the device to announce a download's size + CRC.

```
TransferControl (16 bytes, little-endian):
  op         u8    1 = upload (app → device)
                   2 = download (device → app)
                   3 = abort
  type       u8    object type (§4.1)
  object_id  u16
  total_len  u32   upload: full object size · download request / abort: 0
  crc32      u32   upload: whole-object CRC-32 (§6) · download request / abort: 0
  offset     u32   byte offset to start from (0 = fresh) — the resume anchor
```

**Upload (app → device).** The app writes `op=1` with the object's real
`total_len` + `crc32`, then streams `object[offset…]` over the CoC as raw
bytes. The device sinks them to storage, CRC-ing as it writes. When
`total_len` bytes have arrived it verifies the CRC and notifies a
`transferResult` (§4.3): `committed` on match — a mismatch **rejects** the
object (`crcMismatch`), never commits it. A resume after a drop re-writes the
descriptor with `offset` = the `committed_offset` the device reported; the CRC
still covers the **whole object**, so the device keeps its running CRC state
(or re-reads the committed prefix) across the resume.

**Download (app → device request, device → app announce).** The app writes
`op=2` with `total_len = crc32 = 0` and the wanted `offset`. The device
answers with a `transferControl` **notification** — the same 16 bytes, `op=2`,
with `total_len` and `crc32` filled in — then streams `object[offset…]` over
the CoC. The app CRCs as it reads and rejects on mismatch. End of object =
`total_len − offset` bytes received; the device additionally notifies a
`transferResult` (`committed`) as the explicit close. A download resume is a
new `op=2` write with a nonzero `offset`.

**Abort (`op=3`).** Either side stops cleanly: the app writes `op=3`
(type/object_id echo the active transfer), the device drains and discards, and
notifies `transferResult` with `aborted` and the durable `committed_offset`.
An aborted upload's partial bytes may be kept for a later resume but are never
visible as a committed object.

A descriptor that names an unknown type/id, a nonsensical offset (past
`total_len` or past the stored object), or arrives mid-transfer is answered
with a `transferResult` carrying `error` / `notFound` / `busy` (§4.3) and does
not disturb an active transfer.

### 4.3 `status` — typed device → app notifications

Every `status` notification is one message: a `u8` discriminator + fixed body.

```
msg = 1  transferResult (8 bytes total):
  msg               u8   = 1
  object_id         u16       for a fresh upload (0xFFFF), the ASSIGNED id
  status            u8   0 = committed     stored + CRC verified
                         1 = crcMismatch   rejected, nothing committed
                         2 = aborted       §4.2 op=3, either side
                         3 = error         storage / internal failure
                         4 = notFound      unknown object type/id
                         5 = busy          a transfer is already active
  committed_offset  u32       durable byte count — the resume anchor

msg = 2  storeChanged (6 bytes total):
  msg       u8   = 2
  type      u8   which store changed: route (1) or ride (2)
  revision  u32  the new store revision (§4.5)

msg = 3  commandResult (4 bytes total):
  msg     u8   = 3
  cmd     u8   echoes the command byte (§4.4)
  status  u8   0 = ok · 1 = unknown command · 2 = not found · 3 = busy · 4 = error
  detail  u8   command-specific, 0 unless documented
```

Unknown `msg` values must be ignored by the app (forward compatibility).

### 4.4 `command` — small imperatives

A write of `cmd u8` + fixed args. Every command is answered with a
`commandResult` (§4.3).

| `cmd` | Command | Args | Effect |
|---|---|---|---|
| `1` | `deleteObject` | `type u8 · object_id u16` | delete a stored route (`1`) or ride (`2`); bumps the store revision |
| `2`–`15` | — | — | reserved (identify/find-my-device, factory reset, …) |

### 4.5 `objectStore` — the store digest

A 10-byte read + notify value — the cheap "did anything change" signal that
replaces polling the (CoC-sized) lists:

```
ObjectStore (10 bytes, little-endian):
  revision     u32  bumped on ANY store change (upload committed, delete,
                    ride tracked). Monotonic per boot; not persisted.
  route_count  u16
  ride_count   u16
  reserved     u16  = 0
```

The device notifies it on every change (alongside the `storeChanged` status
message, which additionally says *which* store moved). The app's sync flow:
read the digest, and if the revision moved since the last fetch, download the
relevant list object(s).

---

## 5. Data plane — L2CAP CoC

- The device opens an LE credit-based connection-oriented channel server on a
  **dynamic PSM** and publishes the PSM in the `psm` characteristic (§3.3).
  The app reads it and opens the channel (`CBL2CAPChannel`).
- The channel carries **only object payload bytes** as announced by the active
  `transferControl` descriptor — no framing, no interleaving (§4.1: one
  transfer at a time).
- Flow control is the CoC's native credit scheme; the device grants credits as
  it drains its sink. Neither side pads or aligns: a receiver must accept any
  segmentation of the byte stream.
- If the channel drops mid-transfer, the transfer stays resumable (§4.2); the
  app re-opens the CoC (re-reading `psm`) and resumes by offset.

## 6. CRC-32

Whole-object end-to-end check, verified once at commit. **CRC-32/IEEE**
(zlib/gzip/PNG): reflected, polynomial `0x04C11DB7` (reflected form
`0xEDB88320`), initial value `0xFFFFFFFF`, final XOR `0xFFFFFFFF`.
Check value: `CRC32("123456789") = 0xCBF43926`.

This is deliberately *not* a per-chunk CRC — the Link Layer already covers the
air. It covers what the link can't: encode bugs, storage errors, resume-logic
mistakes, end to end from phone encode to device flash (and back).

---

## 7. Object layouts

### 7.1 `route` — an OBCR v2 file

A route object's payload is **exactly the bytes of an OBCR v2 file** — see
[`OBCR_Spec.md`](OBCR_Spec.md), including the v2 waypoints section. The phone
encodes imported GPX/TCX to OBCR v2 (waypoints included — Delta 2 in the
mirror); the device writes the payload to SD verbatim and serves it back
verbatim.

**Route detail read (app screen E2) is pinned as: download the route object.**
There is no separate detail codec — the app decodes waypoints and the
elevation profile from the OBCR bytes it (in the upload direction) encoded
itself. One layout, one truth.

### 7.2 `ride` — ride object v1

The compact tracked-ride layout (ratified from the app's B7 codec, byte-for-
byte). Coordinates are stored as **degrees × 1e7** (units of 10⁻⁷ °) and the
point order is `lat, lon` — this object is *not* OBCR and deliberately keeps
the layout the app already pins; the extra digit over OBCR's microdegrees
costs nothing at 14 bytes/point and buys a ~1 cm grid.

```
Header (23 bytes + name):
  version      u8   = 1
  name_len     u16  · name UTF-8 (name_len bytes follow immediately)
  start_time   u32  unix seconds
  distance     u32  meters
  moving_time  u32  seconds
  avg_speed    u16  cm/s
  climb        u16  meters
  point_count  u32

Point record (14 bytes × point_count):
  t_offset  u32  seconds since start_time
  lat       i32  degrees × 1e7
  lon       i32  degrees × 1e7
  ele       i16  meters · INT16_MIN (-32768) = no elevation
```

The byte length is fully determined: `23 + name_len + 14 × point_count` —
a decoder must reject a payload whose length disagrees.

### 7.3 `config` — the Config object

Crosses GATT on the `config` characteristic (§3.3), whole-blob on both read
and write. Maximum encoded size **128 bytes**.

```
Config v1:
  name_len  u16  ≤ 48 (UTF-8 bytes; matches the OBCR route-name cap)
  name      name_len bytes, UTF-8 — THE device name (Delta 1: rename = write
            Config with a changed name; there is no separate rename command)
  units     u8   0 = metric · 1 = imperial
  [future fields append here; readers MUST ignore unknown trailing bytes]
```

The append-only rule is the version mechanism: fields are never reordered or
resized, only appended, and absent trailing fields mean "device default".

### 7.4 `routeList` / `rideList` — list objects

Downloaded over the CoC (they outgrow the 512-byte ATT cap fast). Shared
shape: a 4-byte header + fixed 72-byte entries, so entry `k` is at
`4 + 72k` — O(1) indexing, no string scanning.

```
List header (4 bytes):
  version     u8   = 1
  entry_len   u8   = 72 (readers use this, not a constant, to skip entries)
  count       u16
```

`routeList` entry (72 bytes) — from the stored OBCR header:

```
  object_id       u16
  reserved        u16  = 0
  byte_len        u32  stored file size (upload/detail sizing)
  distance_m      u32
  ascent_m        u32
  point_count     u32
  waypoint_count  u16
  name_len        u8   ≤ 48
  name            char[48]  UTF-8, zero-padded
  reserved        u8   = 0
```

`rideList` entry (72 bytes) — from the stored ride-object header:

```
  object_id      u16
  reserved       u16  = 0
  byte_len       u32  stored file size
  start_time     u32  unix seconds
  distance_m     u32
  moving_time_s  u32
  avg_speed_cms  u16
  climb_m        u16
  name_len       u8   ≤ 47
  name           char[47]  UTF-8, zero-padded
```

### 7.5 `diagnostics`

An opaque UTF-8 text blob: the device's diagnostic ring buffer (boot count,
last panic message, storage stats) rendered as text. No binary layout is
pinned — it is a human-readable debugging artifact, not an API. Downloaded
over the CoC like any object (object id `0`); may be empty (`total_len = 0`).

---

## 8. Security (lands with A8)

- **Pairing**: LE Secure Connections, **passkey display** — the device shows a
  6-digit code on its screen, typed into the phone's pairing dialog
  (MITM-protected). One bonded peer at a time.
- **Encryption requirements** once a bond exists:

| Surface | Requirement |
|---|---|
| DIS, BAS, `protocolVersion` | none (open — lets the app identity/version-check before pairing) |
| every other OBC Control characteristic | encrypted, LESC-authenticated link |
| the L2CAP CoC | encrypted link (opening it plaintext is refused) |

- Before A8 lands, bring-up builds run everything plaintext; the levels above
  become mandatory the moment bonding ships and are re-verified there.
- Bonded devices reject SMP pairing attempts from other peers; clearing the
  bond (device-side UI) returns to open pairing.

---

## 9. Divergences from the iOS mirror (the repin list)

The app was built against `companion-ios/OBCProtocol.md`'s provisional values.
This spec ratifies most of them; the deliberate changes, each a single-spot
repin by design:

1. **Custom UUIDs** (`GATT.swift`): the `0BC0000x-0000-1000-8000-00805F9B34FB`
   placeholders were derived from the Bluetooth SIG base UUID, which custom
   services must not use. Replaced by the random `3C92xxxx-…` base (§3.3); the
   final 16-bit block keeps the mirror's `000N` indexing, plus `0000` for the
   service itself.
2. **`TransferStart` 15 → 16 bytes** (`TransferDescriptor.swift`): a leading
   `op` byte (§4.2) folds download-announce and abort — both named but
   byte-less in the mirror — into the one descriptor. Field order after `op`
   is unchanged.
3. **`TransferResult` gains status codes** and rides inside the `status`
   message envelope (§4.3): the mirror's raw 7-byte notify becomes `msg=1` + the
   same 7 bytes, and `notFound` / `busy` join the status enum.
4. **`RideList` characteristic → `objectStore` digest** (§4.5): full lists
   don't fit a 512-byte GATT attribute, so both lists are CoC objects (§7.4)
   and the characteristic slot (`…0003`) becomes the change-signal digest.
5. **Diagnostics move to the CoC** (§7.5), for the same 512-byte reason; the
   `diagnostics` characteristic slot is kept but reserved. The app's
   `readDiagnostics()` becomes a download of object type `4`.
6. **Ratified as-is**: the ride object (`ProvisionalRideCodec`, §7.2), the
   Config blob (`ProvisionalConfigCodec` + append-only rule, §7.3),
   CRC-32/IEEE (§6), the 244-byte chunk preference (§3.4), `protocol_version
   = 1` (§1), device name in Config / Delta 1 (§7.3), and phone-side GPX+TCX
   conversion / Delta 2 (§7.1).

## Reference implementation

Firmware: the `obc-ble` workspace crate (descriptor codec + transfer state
machine, lands with A5) and `obc-route` (OBCR v2). App:
`companion-ios/Packages/OBCKit` (`OBCTransport/Transfer`, `Codecs/`,
`BLE/GATT.swift`). Shared fixtures: [`protocol-vectors/`](protocol-vectors/) —
routes with/without waypoints, a ride, a config blob, and transfer-descriptor
transcripts (including a resume), asserted byte-exact from both languages.
