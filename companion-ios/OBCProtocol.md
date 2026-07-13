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
> **Status: protocol v2** (epic #632 — the one coordinated wire break over v1).
> This mirror describes the **v2** surface; the shared fixtures in
> [`protocol-vectors/`](../protocol-vectors/) pin it byte-exactly on both sides
> (`ProtocolVectorTests` here, `obc-vectors` in the firmware workspace), and
> **spec §9 is the v1 → v2 repin checklist**. The v2 changes: the widened
> `protocolVersion` read (version + store epoch), a 12-byte `TransferControl`
> (no `offset`), the download announce folded into `status` (so `transferControl`
> is write-only), `objectStore` + `diagnostics` characteristics dropped,
> `routeList` entries at 76 bytes (+content CRC), and a 6-byte list header
> (+`total`). The iOS transport re-pin that makes the Swift codecs match is
> tracked as V4 (#768); until it lands, the app shows the version-mismatch banner
> against a v2 device — itself a verification of the compat path.

---

## Versioning & store epoch

The `protocolVersion` characteristic read is widened in v2 to
`version u16 · store_epoch u32` (6 bytes LE, readable without encryption) when a
store is mounted. The `protocol_version` is **currently `2`**; `store_epoch` is a
`u32` TRNG nonce naming the store's **id era**. It is **card-resident** — it lives
on the SD card, so the card carries its own era name — and the device changes it
only on an id-era reset: a lost RRAM id floor (a full-chip reflash, a factory
reset, or a torn id-marks line) **or** an absent/torn card epoch file. Because the
epoch rides the card, a **card swap transplants the era** (swap back and the old
one returns), and a card written by a *different* device presents *its own* epoch —
so on this device it reads as a distinct `(serial, epoch)` scope. That **closes**
the former foreign-card residual hole (#776). Because it is the pre-pairing read
the app performs first on every connect, the app knows the epoch **before** it acks
or reconciles anything.

**No store ⇒ no epoch (short read).** A device with no mounted card serves only the
**2-byte version** (`version` alone). The app treats the absent epoch as a **failed
identity read** — ack fail-closed (below), never epoch `0` (a legal value). The
full 6-byte shape returns whenever a store is mounted.

**Store epoch — why the app needs it.** Every durable link keys on bare `u16`
object ids (ride synced-set + tombstones, route `deviceObjectID` links). A device
reset (or a swapped card) can reopen the id space and silently **alias** months-old
phone state, so the app scopes all id-keyed state to `(device serial, store
epoch)`; an era change then makes old entries archival by construction. **Ack
fail-closed:** the version+epoch read gates `ackRides` and all reconcile writes — a
connection whose identity read *failed* (including the short version-only read
above) sends no ack and reconciles nothing (library browsing is unaffected). The
composite-key scoping is V5 (#769); this section is the wire fact it stands on.

**Mismatch behavior — surface, don't crash.** On connect the app reads the
device's version (the first `u16` of the read) into `DeviceInfo.protocolVersion`
and compares it to the pinned app version. A mismatch is reported as
`DeviceError.protocolMismatch(expected:found:)` and surfaced in the UI (a banner /
disabled sync) — it must **never** trap, force-unwrap, or silently proceed with an
incompatible decode. A v1 app against a v2 device reads `version = 2` and takes
exactly this path (there is no dual-version serving). Bump the pinned app version
only in lockstep with a firmware wire change.

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
`BLE/GATT.swift`). **Six characteristics in v2** (two of v1's eight dropped) — the
`0003` (`objectStore`) and `0006` (`diagnostics`) blocks are **retired and must
not be reused**:

| `XXXX` | Characteristic | Properties | Role |
|---|---|---|---|
| `0001` | `command` | write | small imperatives: `deleteObject` (cmd 1: `type u8 · id u16` — routes (1) and trips (9); a trip delete is **non-cascading**, members become top-level, unknown id → `notFound`), `ackRides` (cmd 2: `count u8 · count × id u16` — the ride-possession ack, below), `installFw` (cmd 3: no args — request installing the staged `/UPDATE.BIN`, S7 below), `forgetBond` (cmd 4: no args — dissolve the device-side bond, below) — spec §4.4 |
| `0002` | `status` | notify | typed device → app messages (`StatusMessage`: transferResult / storeChanged / commandResult / **downloadAnnounce**) — the **sole** device → app channel, spec §4.3 |
| `0004` | `config` | read + write | the Config object incl. **device name** (see *Delta 1*) → `DeviceConfig` |
| `0005` | `transferControl` | **write** | open / abort a CoC object transfer (§ below) — **write-only, no CCCD** in v2 |
| `0007` | `psm` | read | the dynamically-assigned L2CAP CoC PSM the app opens the channel on |
| `0008` | `protocolVersion` | read | `version u16 · store_epoch u32` LE, readable without encryption — the connect-time identity check |

*(v2 dropped `0003` `objectStore` — the change signal is `storeChanged` alone — and
`0006` `diagnostics`, which returned 0 bytes; real diagnostics cross the CoC as
object type 4. Full lists never fit a 512-byte ATT attribute anyway, so they were
always CoC objects.)*

The app-facing characteristics require an **encrypted, LESC-authenticated** link
(firmware `A8`); the phone is the only bonded peer. DIS/BAS/`protocolVersion` stay
open so the app can identity/version-check before pairing.

**Pairing / bonding (A8 + #455, canonical spec §8).** The device is
`DisplayOnly`: it shows a 6-digit **passkey** on its screen, iOS raises the
system pairing dialog (`OBCSystemPairing`), the rider types it — LESC passkey
entry, MITM-protected. Pairing *is* `connect()`: reading a gated characteristic
/ opening the CoC on the unencrypted link makes iOS pair, and the encrypted
link completing is what resolves the connect. A declined / wrong passkey
surfaces as a pairing failure (→ D5). One bonded peer — and **while a bond is
stored the device rejects every new pairing attempt** (spec §8, reversing the
old replace-on-pairing rule): the device suppresses its passkey and drops the
link, so the attempt surfaces as a **generic** pairing/connection failure. No
distinguishable SMP reason reaches the app (the device's host stack can't
attach one, and CoreBluetooth wouldn't surface it), so the "already paired to
another phone" copy must key on context, not a code (#461). The rider clears
the bond on the device (Settings ▸ Bluetooth ▸ **Forget phone**, hold-guarded)
to re-open pairing. The device also has a Bluetooth **off** switch: off = no
advertising, live link dropped — the app simply sees the device disappear
(bond retained; reconnect resumes when it's back on).

**Reconnect.** The device keeps a **stable** static address, so once bonded, iOS
reconnects silently on any contact (no dialog) — power-cycle either side, walk
away and back. The app persists only a `BondRecord` ("we paired, with `<name>`")
for the launch greeting; CoreBluetooth owns the real crypto bond.

**Forget (H2).** `BondStore.clear()` drops the app's record, but iOS keeps the CB
bond until the user also removes it in **Settings ▸ Bluetooth** — the H2 copy
says so. A phone-side forget alone would leave the device still holding its bond
and rejecting the fresh pairing attempt (#455) — so a **connected** forget also
sends `forgetBond` (cmd 4, spec §4.4) over the bonded link: the device dissolves
*its* side of the bond too and re-opens pairing, no on-device step needed. The
transport sends it best-effort — `commandResult(ok)` first, then the device drops
the link — and the app clears its local record whether the command succeeds or
times out. **Offline** the device is unreachable, so the old wedge stands: the
rider must run **Forget phone** on the device before re-pairing (the not-connected
H2 copy keeps that guidance). Either way, iOS's own CB bond is still the user's to
remove in Settings ▸ Bluetooth.

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

1. A fixed **`TransferControl`** descriptor opens the transfer — the app **writes**
   it to the GATT `transferControl` characteristic *before* any payload byte
   (v2: the characteristic is write-only). One 12-byte shape serves both directions
   and abort:

   ```
   TransferControl (12 bytes, little-endian):
     op         u8    1 = upload (app → device) · 2 = download · 3 = abort
     type       u8    { route 1, ride 2, config(reserved) 3, diagnostics 4,
                        fwImage 5, routeList 6, rideList 7, echo 8,
                        trip 9, tripList 10 }
     object_id  u16   0xFFFF on upload = "new" (device assigns the id)
     total_len  u32   upload: full object size · download request / abort: 0
     crc32      u32   upload: whole-object CRC-32/IEEE · download request / abort: 0
   ```

   (v2 dropped v1's trailing `offset u32` — it was always 0; transfers restart, not
   resume.) For a **download** the device answers with a **`downloadAnnounce`**
   status message (`msg = 4`: the `msg` byte + these same 12 descriptor bytes, `op =
   download`, `total_len`/`crc32` filled) before the payload flows — the announce
   rides `status`, not `transferControl`, so all device → app control traffic shares
   one CCCD and one ordering domain.

2. The **CoC carries the raw payload bytes** of the whole object — nothing
   else. The receiver sinks them straight to storage, updating a running CRC (no
   reassembly buffer — the point on a RAM-limited MCU).

3. A **`transferResult`** message closes it over `status` (a typed envelope:
   `msg u8 = 1 · object_id u16 · status u8 {committed 0, crcMismatch 1, aborted 2,
   error 3, notFound 4, busy 5, storageFull 6} · committed_offset u32`; for a
   fresh upload the result carries the **assigned** id). `status` also carries
   `storeChanged` (msg 2: `type u8 {route 1, ride 2, trip 9 — mirrors the object-type
   numbers} · revision u32` — each store keeps its **own** monotonic-per-boot
   revision, so a UI-composed "delete trip & routes" cascade emits **both** a route
   and a trip signal; unknown store types are ignored), `commandResult` (msg 3), and
   the **`downloadAnnounce`** (msg 4) messages — unknown discriminators are ignored,
   and an unknown *status* code decodes as a generic device error (forward compat).
   `storageFull` covers whichever catalog the upload targeted (routes: cap 64,
   trips: cap 16).

- **CRC once, end-to-end.** One whole-object CRC verified at commit — a mismatch
  **rejects** the object (`DeviceError.crcMismatch`), never commits it. This is the
  *end-to-end* check the link CRC can't give (encode bugs, storage errors); it is
  **not** a redundant per-packet CRC.
- **Restart, not resume** — an interrupted transfer is re-sent / re-requested
  whole (spec §1 principle 4); the device discards partial uploads. Multi-object
  flows (the B7 ride sync) resume at whole-object granularity: rides that fully
  landed are kept, the rest are re-requested from byte 0.
- **Cancelable** — abort over `TransferControl` + channel teardown; clean both ends.
- **Storage-full reject (descriptor-open).** A **new**-route upload (`op=1`, route
  type, `object_id = 0xFFFF` or a route id the device doesn't hold) that would grow
  the catalog past its cap (64 routes) is rejected at the `TransferControl` write,
  **before any bytes stream**, with `transferResult` status `storageFull` (6) — no
  channel opens, no partial file. **Replace-by-id uploads of an existing route are
  exempt** (they reuse a slot). The app surfaces this as "delete routes on the
  device".

`fwImage` (type 5, S7) carries a firmware update — the whole OBCU `UPDATE.BIN`
container (app → device, singleton `object_id = 0`); the transfer layer stays
format-blind, and a CRC-verified commit promotes it to `/UPDATE.BIN` on the card.
Installing is the separate, physically-confirmed `installFw` command (below), never
part of staging. `echo` is the `A5` dev/test loopback. At most **one transfer is in
flight at a time**.

> **Ratified.** `S0` adopted this descriptor + raw-stream design (it supersedes the
> earlier per-frame `{type, object_id, total_len, offset, chunk_len, crc32}` idea),
> with one leading `op` byte so upload, download, and abort share the one shape;
> **v2** trims it to 12 bytes by dropping the never-used `offset`.

**The transport codecs live** in `OBCTransport`: `Transfer/TransferDescriptor.swift`
(`TransferControl`, the `StatusMessage` envelope), `Transfer/CRC32.swift`
(whole-object + streaming `Hasher`; CRC-32/IEEE, check value `0xCBF43926`), driven
by `BLEChannel` (raw streaming, progress/cancel/resume) over a `ByteChannel` — the
L2CAP CoC (`L2CAPByteChannel`) on the real path, an in-memory pipe in tests. Field
widths, the CRC variant, and the GATT UUIDs (`BLE/GATT.swift`) are pinned
byte-exactly against the shared `protocol-vectors/` fixtures by
`ProtocolVectorTests`. **V4 (#768) re-pins these Swift codecs to the v2 shapes** —
the single notify surface, the 12-byte descriptor, the widened version read, and
the `routeList` `crc32`.

**Synced-ride reconciliation — `ackRides` (cmd 2, spec §4.4).** Rides are deleted
**only on the device** (its Rides screen), where a per-ride *"synced"* flag drives
a delete-guard cue ("this ride isn't on your phone yet"). To keep that flag
honest, the app **owns the ground truth** for "the phone holds this ride" and
sends it back: on every connect (and after edits, at will) it writes an
`ackRides` command — `count u8 · count × device-namespace ride id u16` — and the
device **sets** (never clears) the synced flag for each id it still stores.
`commandResult.detail` returns the newly-flagged count. This makes the flag
*reconciled state* rather than an inference from download events: rides synced
before the device tracked the flag, a reflashed card, or an app reinstall all
self-heal on the next connect, instead of the device permanently believing a
ride the phone already holds was never synced. `deleteObject` for a ride
(type 2) stays **reserved** — the app tombstones synced rides locally so a
re-sync can't resurrect them.

**Firmware delivery — `fwImage` + `installFw` (S7, spec §7.6 / §4.4).** The app
imports an `UPDATE.BIN` (a Files pick — the whole OBCU container, `OBCU_Spec.md`
§1), validates its 64-byte header **and both CRC-32s** before offering it, then
streams it as a `fwImage` (type 5, `object_id 0`) exactly like a route upload
(progress, cancel, whole-object restart).

The **size/tail contract** matches the SD sideload path (`OBCU_Spec.md`
§1.1 / §2.3) on both bounds. **Ceiling:** the header's `Image Len` is the raw
image, capped at `MAX_IMAGE_LEN` = 1,480,000; the streamed `fwImage` is the
*whole container*, so its `total_len` is `64 + Image Len`, and the device's
announce guard rejects at the **container** ceiling `MAX_IMAGE_LEN + 64` =
1,480,064 — not the raw cap, so an image in the top 64 bytes of the range still
stages. **Tail:** any bytes in the picked file past `64 + Image Len` are FAT
cluster slack and **ignored** (the armer accepts `file_len >= 64 + Len`); the app
trims to exactly `64 + Image Len` and streams only those bytes — a genuinely
*short* file (that can't hold header + image) is still rejected.

On commit the app sends `installFw`
(cmd 3, no args); its `commandResult.status` maps to plain UI copy: `ok`(0) →
"confirm on the device", `notFound`(2) → "the device doesn't see the update",
`busy`(3) → "finish the current ride first", `error`(4) → "the device rejected
it", `unknownCommand`(1) → "can't be updated over Bluetooth". **Installing is
gated on a physical confirm at the device** — `installFw` only requests it — after
which the device reboots and, on reconnect, DIS `0x2A26` reports the new version.
The running firmware version is **only** the DIS Firmware Revision String, never a
CoC object (there is no firmware download direction).

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
- **Lists** — `routeList`/`rideList` are CoC objects behind a **6-byte v2 header**
  (`version 2 · entry_len · count u16 · total u16`, spec §7.4). `total` is the full
  catalog size before the device's `MAX_RIDES`/`MAX_ROUTES` cap, so the list is
  **truncated iff `total > count`** — the app surfaces a one-line warning instead
  of silently answering "up to date". Entry length is now **per-list**: `routeList`
  entries are **76 bytes** (a trailing whole-object `crc32`, `0` = unknown — the
  content fingerprint for identity-verified route badges + adopt-by-content, V6
  #770), `rideList` entries stay 72. A stored route whose genuine CRC-32 happens
  to be `0` (probability 2⁻³²) is indistinguishable from "unknown" and is read as
  unknown — merely "no badge until re-upload", the conservative direction; don't
  special-case it. **diagnostics** is a CoC text blob (spec §7.5).
- **Trips** — a **trip object v1** (type 9, spec §7.7) groups routes: a 56-byte
  header (`version 1 · stage_count u16 · name ≤ 48`) + `stage_count × u16` route
  object **ids** in ride order. It **references** routes, never carries route bytes;
  a route in no trip is top-level, and membership is one level deep (a route is in
  ≤ 1 trip or standalone). Trip ids come from a **separate device counter** (never a
  route/ride id), `0xFFFF` = new, replace-by-id atomic, cap **16** (`storageFull` at
  descriptor-open for new trips, replace-by-id exempt). **Dangling stage refs are
  tolerated on read** (a member route deleted individually doesn't invalidate the
  trip); **the device never rewrites a stored trip** — dangling refs persist until
  the next trip **upload** replaces the object by id, and that upload arrives
  compacted because the **app** (which owns validation) builds it from resolvable
  stages. **Upload order is stages-first, trip-object-last** so an interrupted push
  never dangles and a re-run is idempotent. **Protocol delete is non-cascading** —
  deleting the trip object frees the trip and leaves member routes top-level; a
  "delete trip & routes" is composed by the initiating UI as route deletes + the
  trip delete (and emits both a route and a trip `storeChanged`). `tripList`
  (type 10) is a CoC list behind the same **6-byte v2 header**; its **76-byte**
  entries mirror `routeList` — `object_id · byte_len · total_distance_m ·
  total_ascent_m · stage_count · name ≤ 48` plus a trailing whole-object `crc32`
  (`0` = unknown) — with the totals summed over resolvable stages and `stage_count`
  counting every stored stage (dangling included). The `crc32` is the same content
  fingerprint routes use, so `OnDeviceState.determine` detects an outdated trip (a
  stage reorder changes neither `byte_len` nor `name`).

The byte layout of each object is owned by the spec. The device object codecs
live in `OBCTransport/Codecs/` (`BLEChannel` only moves bytes; the interchange
*file* formats live in `OBCFormats`).

---

## Delta 1 — device name lives in `Config`

The device name is a field of the wire **`Config`** object. Renaming the device
(H3) is a **`writeConfig`** with a changed `name` — there is **no** separate
rename command. This is a hard requirement on the contract, mirrored in
[`DeviceConfig.name`](Packages/OBCKit/Sources/OBCDomain/DeviceConfig.swift). The
name is capped at **48 UTF-8 bytes** (spec §7.3): the codec truncates on a
Character boundary at encode and the rename UI limits to the same, so an
over-long name can't overflow the `u16` length field into a corrupt blob.

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
| `FirmwareInstallResult` | `FirmwareInstall.swift` | mapped `installFw` request outcome (S7) |
| `OBCUHeader` / `StagedFirmware` | `OBCTransport/Firmware/FirmwareImage.swift` | OBCU update-container header + validated update (both CRCs) — the `fwImage` payload (S7) |

`RouteID` / `RideID` are thin `String` wrappers in the same files. **B1
([#237](https://github.com/timohueser/OpenBikeComputer/issues/237)) is landed:**
the finalized `DeviceTransport` protocol + `TransferHandle` live in `OBCTransport`,
the real conformer in `OBCTransport/BLE/` (`BLETransport`, `BLEChannel`,
`L2CAPByteChannel`, `GATT`), and the framing/codec + domain types are unit-tested
without hardware. The **real-path** (live GATT/CoC) now runs against the shipped
firmware `A4`/`A5` stack (with pairing gated on `A8`).
