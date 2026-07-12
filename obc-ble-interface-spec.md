# OBC BLE Interface Specification (v2)

The normative wire contract between the OpenBikeComputer device (nRF54L
firmware, BLE peripheral) and the companion app (iOS, BLE central): advertising,
the GATT control plane, the L2CAP CoC data plane, and the byte layout of every
object that crosses the link. It sits next to [`OBCM_Spec.md`](OBCM_Spec.md)
(map format) and [`OBCR_Spec.md`](OBCR_Spec.md) (route format) and is the
canonical source the firmware Track-A issues (epic #267) implement.

> **Protocol v2** (epic #632) is the one coordinated wire break over v1: it
> **removes** the `objectStore` digest and reserved `diagnostics` characteristics
> and the descriptor's permanently-zero `offset`; folds the download announce into
> the `status` envelope (so `transferControl` is **write-only**); widens the
> `protocolVersion` read to carry a **store epoch**; and grows `routeList` entries
> (+content CRC) and the shared list header (+`total`). v1 is not served in
> parallel — a v1 peer reads `version = 2` first and surfaces its mismatch path
> (§1). The one-line "what changed and why" for each item lives in its section.

> **This document is canonical.** The iOS mirror
> ([`companion-ios/OBCProtocol.md`](companion-ios/OBCProtocol.md)) defers to it:
> where they disagree, this spec wins and the mirror is corrected. §9 lists the
> v1 → v2 wire changes so the app-side repin is a checklist, not a diff hunt.

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
4. **Interrupted transfers restart, not resume — in both directions.** Objects
   are small — a route or a ride is tens of kB, a couple of seconds on the wire —
   so a dropped or aborted transfer is simply re-sent (or re-requested) whole
   rather than continued from a durable offset. The device discards a partial
   upload on any interruption and the app re-sends it from byte 0. Multi-object
   flows resume at **whole-object granularity**: a dropped ride sync keeps the
   rides that fully landed and re-requests the rest from byte 0 (§7.2). (Offset
   resume was in the S0 draft; the descriptor carried a permanently-`0` `offset`
   field through v1 for shape stability — **v2 removes it** (§4.2), since a suffix
   can't be verified against the whole-object CRC anyway. If a large object type
   ever lands, resume returns with it.)
5. **Versioned once.** A single `protocol_version` covers this whole contract.
   Object layouts carry their own version bytes where they live (OBCR header,
   ride object) so they can evolve without a protocol bump.

---

## 1. Protocol version & store epoch

`protocol_version` is an unsigned 16-bit integer, **currently `2`**, exposed
first in the `protocolVersion` read (§3.3). It covers everything in this
document: UUIDs, descriptor layouts, object types, and status codes.

**v2 widens the read** from a bare `u16` to `version u16 · store_epoch u32` — six
little-endian bytes:

```
protocolVersion read (6 bytes, little-endian):
  version      u16   the protocol version (2)
  store_epoch  u32   the device's current store-epoch nonce
```

- The app reads it on every connect, before any other OBC Control traffic. It is
  an **open** (pre-pairing) read (§8), so the app knows both the version and the
  epoch *before* `ackRides` or any reconcile write fires.
- On version mismatch the app **surfaces and stops** (banner / disabled sync) —
  it must never trap or attempt a best-effort decode. A **v1 peer** reads the
  first `u16 = 2` and takes exactly that mismatch path; **there is no dual-version
  serving** — the device speaks v2 only. The device ignores traffic it can't
  parse and answers unknown commands with `unknown` (§4.4).
- Additive, compatible changes (a new object type id, a new command, a new
  waypoint type) do **not** bump the version; changing an existing layout does.

**Store epoch.** `store_epoch` is a `u32` TRNG nonce the device mints once per
id-era and persists in its own RRAM line; it changes only on an **id-era reset**.
Its purpose and the app-side keying are epic #632 item 5; the mint rule lives with
the device implementation (V3). The essentials the wire depends on:

- **Every durable app↔device link keys on bare `u16` object ids** (the ride
  synced-set + delete tombstones, the route `deviceObjectID` links). Ids mint at
  `max(card-scan max + 1, RRAM floor)`: **SD filenames guard stored ids, the RRAM
  floor guards deleted ids.** The **era events** — the only two ways an id can be
  re-issued to a *different* object — are **floor loss** (a full-chip reflash /
  factory reset / a torn id-marks write) and a **namespace reset** (a fresh or
  reformatted card). Either mints a fresh epoch.
- The **never-reuse guarantee** is therefore *within an epoch*: while the epoch
  holds, an id is never re-assigned to a different object. Across an epoch change
  the id space legitimately reopens, and the new nonce makes that visible so the
  app can scope its state to `(device serial, store epoch)` and never silently
  alias months-old ids.
- **Ack fail-closed contract.** The version+epoch read **gates** `ackRides` and
  every reconcile write: a connection whose identity read failed sends no ack and
  reconciles nothing (library browsing is unaffected). V5 implements it; it exists
  so a failed read can never stamp synced-flags or badges under an unknown era.

A random nonce leaks nothing beyond what the open DIS (§3.1) already exposes.

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
- **The Bluetooth switch** (#455): the rider can turn the radio **off** in
  Settings ▸ Bluetooth. Off = advertising stops and any live connection is
  dropped; the device vanishes from scans until the switch is turned back on,
  which resumes the normal lifecycle (fast → slow, exactly as after a boot).
  The stored bond is **retained** across the off state and across reboots with
  the radio off. The switch itself persists in the device settings.
- **Policy — "always just works"**: while the Bluetooth switch is on, the
  device advertises connectable whenever it is powered and unconnected. There
  is no advertising timeout and no "pairing mode" gate for reconnection. The
  device uses a **stable static random address** (derived from `FICR.DEVICEID`)
  and never rotates it — so the phone, which stores that identity at bonding,
  silently reconnects on any contact. Once bonded (A8/#455), the device still
  advertises generally; bonded-data access (the gated OBC Control
  characteristics + the CoC) is denied to any peer that isn't
  LESC-authenticated, and **while a bond is stored, new pairing attempts are
  rejected outright** (§8) — the on-device **Forget phone** action
  (Settings ▸ Bluetooth, hold-guarded) is the only way to clear the bond and
  re-open pairing. *(This reverses the original A8 rule, under which a fresh
  passkey pairing replaced the stored bond.)*

## 3. GATT control plane

Three services. The SIG services are open (readable before pairing); the OBC
Control service is encrypted once bonding lands (§8).

### 3.1 Device Information Service — `0x180A` (SIG)

| Characteristic | UUID | Value |
|---|---|---|
| Firmware Revision String | `0x2A26` | UTF-8 version of the **running** image, e.g. `0.4.0+abc1234`; after a confirmed DFU it reflects the newly-installed image (the app's device-version display, §7.6) |
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
| `0002` | `status` | notify | typed device → app messages (§4.3) — the **sole** device → app channel |
| `0004` | `config` | read + write | the Config object (§7.3), whole-blob |
| `0005` | `transferControl` | write | open / abort a CoC transfer (§4.2) — **write-only, no CCCD** |
| `0007` | `psm` | read | `u16` — the dynamic L2CAP PSM the app opens the CoC on |
| `0008` | `protocolVersion` | read | `version u16 · store_epoch u32` — §1. Readable **without** encryption |

**Five characteristics** (v2 dropped two from v1's seven). The `0003` and `0006`
blocks — v1's `objectStore` digest and reserved `diagnostics` — are **retired and
never reassigned**: the digest double-signalled a change `storeChanged` (§4.3)
already carries and its per-boot `revision` was a latent client trap, and
`diagnostics` returned 0 bytes (real diagnostics cross the CoC as object type 4,
§7.5). `transferControl` loses its CCCD: it is written to *open* a transfer, and a
download's announce now rides `status` (§4.3 `msg = 4`), so **all** device → app
control traffic flows through one notify characteristic — one subscription, one
ordering domain.

Concrete UUIDs, for the record:

```
service          3C920000-9916-4EBA-ABC2-342FE08F6B10
command          3C920001-9916-4EBA-ABC2-342FE08F6B10
status           3C920002-9916-4EBA-ABC2-342FE08F6B10
config           3C920004-9916-4EBA-ABC2-342FE08F6B10
transferControl  3C920005-9916-4EBA-ABC2-342FE08F6B10
psm              3C920007-9916-4EBA-ABC2-342FE08F6B10
protocolVersion  3C920008-9916-4EBA-ABC2-342FE08F6B10
```

(`3C920003` and `3C920006` are retired — see above — and MUST NOT be reused.)

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
| `2` | `ride` | device → app | ride object v1 or v2, §7.2 |
| `3` | `config` | — | reserved on the CoC; Config crosses GATT (§3.3) |
| `4` | `diagnostics` | device → app | diagnostics blob, §7.5 |
| `5` | `fwImage` | app → device (upload) | a complete `UPDATE.BIN` OBCU update image, §7.6 |
| `6` | `routeList` | device → app | list object, §7.4 |
| `7` | `rideList` | device → app | list object, §7.4 |
| `8` | `echo` | both | dev/test only: device streams back what it received (A5's loopback) |
| `9`–`15` | — | — | reserved (sensors, M4) |

**Object ids** are `u16`, assigned by the device, **stable for the life of the
stored object — including across device reboots** — and enumerated by the list
objects. Durability is what lets the phone persist the id an upload committed
under and later reconcile ("is my copy still on the device?") or replace that
object in place — and, for rides, what the app's synced-set and delete
tombstones key on. Ids mint at `max(card-scan max + 1, RRAM floor)`: the
reference firmware encodes the id in the stored filename (routes `RT{id}.OBR`,
rides `RD{id}.ORD`) — **SD filenames guard stored ids** — and an RRAM floor
guards **deleted** ids. **Within a store epoch an id is never re-issued to a
different object.** The two era events that legitimately reopen the id space —
floor loss (reflash / factory reset / torn id-marks write) and namespace reset
(fresh / reformatted card) — each mint a fresh `store_epoch` (§1), so the app
scopes id-keyed state per epoch and an era change never silently aliases a stale
id. Conventions:

- `0xFFFF` on an upload means "new" — the device assigns an id and reports it
  in the `transferResult` (§4.3). Uploading to an existing id replaces that
  object atomically (commit-then-swap; a failed CRC never touches the old copy).
- Objects that exist once (`routeList`, `rideList`, `diagnostics`, `echo`, and
  the `fwImage` staging slot) use object id `0`. A `fwImage` upload is a
  singleton stage: the app sends object id `0`, the device assigns no id and the
  `transferResult` echoes `0` (§7.6).
- Ids `0xFF00`–`0xFFFE` are a **session-scoped** band for objects that exist on
  storage without a device-assigned identity (side-loaded dev files). They are
  valid transfer targets within a connection but must never be persisted by the
  app — they may name a different object after a reboot.

At most **one transfer is in flight at a time** — the CoC carries exactly one
object's bytes between a `transferControl` open and its `transferResult`. A
second open while one is active is answered with `busy`.

### 4.2 `transferControl` — the transfer descriptor

One fixed **12-byte** descriptor shape serves both directions and abort. In v2 it
is **write-only** (no CCCD): the app writes it to *open* a transfer; the device
never notifies it. A download's announce rides the `status` envelope instead
(§4.3 `msg = 4`).

```
TransferControl (12 bytes, little-endian):
  op         u8    1 = upload (app → device)
                   2 = download (device → app)
                   3 = abort
  type       u8    object type (§4.1)
  object_id  u16
  total_len  u32   upload: full object size · download request / abort: 0
  crc32      u32   upload: whole-object CRC-32 (§6) · download request / abort: 0
```

**v2 drops the `offset` field** (v1's trailing `u32`, always `0`): transfers
restart, never resume (§1 principle 4), so the byte and its `error`-on-nonzero
reject were dead weight.

**Upload (app → device).** The app writes `op=1` with the object's real
`total_len` + `crc32`, then streams the whole object over the CoC as raw bytes.
The device sinks them to storage, CRC-ing as it writes. When `total_len` bytes
have arrived it verifies the CRC and notifies a `transferResult` (§4.3):
`committed` on match — a mismatch **rejects** the object (`crcMismatch`), never
commits it. Uploads are **not resumable** (§1 principle 4): an interrupted upload
(a dropped link or an `op=3` abort) is discarded, and the app re-sends the object
from the start.

**Download (app → device request, device → app announce).** The app writes
`op=2` with `total_len = crc32 = 0`. The device answers with a **`downloadAnnounce`
status notification** (§4.3 `msg = 4`) — the same 12 descriptor bytes, `op=2`,
with `total_len` and `crc32` filled in — then streams the whole object over the
CoC. The app CRCs as it reads and rejects on mismatch. End of object = `total_len`
bytes received; the device additionally notifies a `transferResult` (`committed`)
as the explicit close, on the same `status` characteristic. An interrupted
download is re-requested whole (a fresh `op=2`, §1 principle 4).

**Abort (`op=3`).** Either side stops cleanly: the app writes `op=3`
(type/object_id echo the active transfer), the device drains and **discards** the
partial, and notifies `transferResult` with `aborted` (`committed_offset = 0` —
nothing is retained).

A descriptor that names an unknown type/id or arrives mid-transfer is answered
with a `transferResult` carrying `error` / `notFound` / `busy` (§4.3) and does not
disturb an active transfer.

**Storage-full reject (descriptor-open).** A **new**-route upload — `op=1`,
route type, `object_id = 0xFFFF` (or a route id the device doesn't hold) —
that would grow the catalog past its cap is rejected at the `transferControl`
write, **before any bytes stream**, with `transferResult` status `storageFull`
(§4.3); no CoC opens and no partial file is created. **Replace-by-id uploads of
an existing route are exempt** — they reuse a catalog slot rather than growing
it, so updating a stored (or actively-navigated) route never hits the cap. The
`object_id` in the reject echoes the request (`0xFFFF` for a fresh upload). The
app surfaces this as "delete routes on the device"; the reference cap is 64
routes.

**`fwImage` staging (M4).** A `fwImage` upload (§7.6) stages a firmware update
image to the card over the existing transfer machinery unchanged — whole-object
CRC-32 at commit, no partial resume (an update is ~900 KB ≈ a large route). Two
`fwImage`-specific rules ride the same descriptor path: (1) an announced
`total_len` past the device's update-slot ceiling is rejected at the
`transferControl` write with `error`, **before any bytes stream** — the ~900 KB
would otherwise transfer only to fail at commit; and (2) a CRC-verified commit
promotes the staged bytes to `/UPDATE.BIN` in the card root, **overwriting any
existing `UPDATE.BIN`**. A torn or CRC-failed transfer leaves no visible
`UPDATE.BIN` (the same commit-then-swap invisibility routes use). Staging does
**not** install — installation is the separate, physically-confirmed `installFw`
command (§4.4).

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
                         6 = storageFull   the route catalog is full — a NEW-route
                                           upload was rejected at descriptor-open
                                           time (§4.2), before any bytes streamed
  committed_offset  u32       durable byte count: total_len on `committed`, else 0
                              (a download's explicit close reports its total_len)

msg = 2  storeChanged (6 bytes total):
  msg       u8   = 2
  type      u8   which store changed: route (1) or ride (2)
  revision  u32  a monotonic-per-boot counter bumped on any change to that store —
                 the cheap "refetch the list" signal (it is the sole change signal
                 in v2; the v1 objectStore digest is gone)

msg = 3  commandResult (4 bytes total):
  msg     u8   = 3
  cmd     u8   echoes the command byte (§4.4)
  status  u8   0 = ok · 1 = unknown command · 2 = not found · 3 = busy · 4 = error
  detail  u8   command-specific, 0 unless documented

msg = 4  downloadAnnounce (13 bytes total):
  msg         u8   = 4
  descriptor  12   the 12-byte TransferControl (§4.2), op = 2 (download), with
                   total_len + crc32 filled in for the object about to stream
```

The **`downloadAnnounce`** (v2) is the device's answer to a download request
(§4.2): the announce moves off `transferControl` and onto this envelope so all
device → app control traffic shares one notify characteristic and one ordering
domain. Unknown `msg` values must be ignored by the app (forward compatibility).

### 4.4 `command` — small imperatives

A write of `cmd u8` + fixed args. Every command is answered with a
`commandResult` (§4.3).

| `cmd` | Command | Args | Effect |
|---|---|---|---|
| `1` | `deleteObject` | `type u8 · object_id u16` | delete a stored route (`1`); bumps the store revision. Ride (`2`) deletion over the link is **reserved** — the reference firmware answers `notFound`: rides are deleted only on the device itself (its Rides screen), and the app hides synced rides locally (tombstones) so a re-sync can't resurrect them |
| `2` | `ackRides` | `count u8 · count × object_id u16` | the app's **ride-possession ack**: the device marks every listed ride id it still stores as synced ("downloaded at least once"). `commandResult.detail` = the newly-flagged count (saturating at 255); a flag change bumps the **ride** store revision. See below |
| `3` | `installFw` | none (`cmd` byte only) | ask the device to install the staged `UPDATE.BIN` — runs the on-device scan + **on-glass confirm** flow (see below). The command only *requests*; it never waits for the human and never installs on its own |
| `4` | `forgetBond` | none (`cmd` byte only) | ask the device to dissolve **its** side of the bond, so an app-side "Forget device" doesn't leave the pair wedged. The device answers `commandResult(ok)` **first**, then clears the bond + drops the link and returns to open-pairing advertising. **Honoured only on the bonded, authenticated link** (see below) |
| `5`–`15` | — | — | reserved (identify/find-my-device, factory reset, …) |

**Next free command: `5`.** (Heads-up for auto-expiry #638: its draft `setClock`/
`setRouteRetention` table predates `installFw`/`forgetBond` taking `3`/`4`, so it
renumbers to `5`/`6` when it lands.)

**`ackRides` — possession reconciliation.** The device keeps a per-ride
"synced" flag (it drives the delete-guard cue on the device's Rides screen).
Setting it only when a ride download completes leaves the flag an *event
inference* — any divergence between the app's library and the device's record
(rides synced before the device tracked the flag, a record lost with a
reflashed card, an app reinstall) would be permanent, because a ride the app
already holds is never downloaded again. `ackRides` converts the flag into
*reconciled state*: the app's library is the ground truth for "the phone has
this ride", and the app sends the device-namespace ride ids it holds on every
connect (and after edits, as it likes). Rules:

- **Monotonic**: the device only sets flags from an ack, never clears them —
  the flag means "downloaded at least once", not "still held by the phone".
  (A phone-side delete keeps the ride's tombstone, so its id stays in the
  ack list; ids never reuse, so a stale flag can't mislabel a future ride.)
- **Idempotent and order-free**: re-acking a flagged ride changes nothing, so
  the app may chunk a long list across several `command` writes (the
  reference firmware accepts ≤ 31 ids per write — a 64-byte value) and
  re-send the whole list every connect.
- **Unknown ids are ignored**, answered `ok`: the app may hold rides the
  device has since deleted. `error` is answered only for a malformed write
  (`count` promising more ids than the write carries).

**`installFw` — install the staged update (M4).** After a `fwImage` upload
(§7.6) lands `/UPDATE.BIN` on the card, the app sends `installFw` to ask the
device to install it. The command returns as soon as the request is **accepted**
— it does *not* wait for the human. The device then runs its on-device flow:
scan + validate the staged image, show a **confirm card**, and install only on a
physical **encoder press** by the rider. The reply codes map onto the existing
`commandResult` status vocabulary (§4.3) — no new status byte:

| `installFw` outcome | `commandResult.status` | Meaning |
|---|---|---|
| `ok` | `ok` (0) | request accepted — the device opens its on-glass check → confirm flow promptly (it may briefly wait for the screen to be free, e.g. an active pairing card) |
| `noStaged` | `notFound` (2) | no `UPDATE.BIN` on the card to install |
| `busy` | `busy` (3) | a ride is recording, or an install request is already pending |
| `invalid` | `error` (4) | the staged image is already known-unusable |

Precedence when several apply: **`busy` > `noStaged` > `invalid` > `ok`**. The
device answers from **cheaply-knowable** edge state only: `busy` (recording /
pending) and `noStaged` (a card-root existence check) are cheap; the full
multi-second CRC scan is **not** run inside the command handler, so the reference
firmware never returns `invalid` here — it accepts (`ok`) and lets the
on-device scan surface a bad image on the confirm card. `invalid` is reserved for
a device that *can* cheaply reject a stage. A device that predates the command
answers `unknown` (§4.4 compat), which the app reads as "this device can't be
updated over BLE".

**Security posture — no silent installs, ever.** Staging a `fwImage` over BLE is
authenticated only by the bonded, encrypted link (§8): a paired phone can drop an
image on the card, nothing more. **Installing** is gated on **physical
confirmation at the device** — the encoder press on the confirm card, symmetric
with the pairing-passkey pattern (the phone can request, only the rider at the
device acts). `installFw` therefore never arms or reboots on its own; it posts a
request the on-device confirm flow must approve. Image authenticity beyond the
link is out of scope for v1 (CRC-32 integrity only, no signature — matching the
SD-sideload contract): physical possession of the card is already root on an open
device, so the install-time gate is the human, not a cryptographic signature
(`OBCU_Spec.md` reserves header bytes for a future signature scheme).

**`forgetBond` — dissolve the device-side bond (#756).** The app's "Forget device" clears only the
phone's own bond record; the device keeps its bond, and the reject-when-bonded posture (§8) then
refuses *every* new pairing until the rider also runs **Forget phone** on the device — so a one-sided
app forget leaves the pair wedged (a pairing attempt while bonded is rejected outright). `forgetBond`
closes that gap: the bonded phone asks the device to clear its side too, and the device runs the same
machinery **Forget phone** does — zero the RRAM bond slot, drop the peer from the host bond table +
resolving list, lower the `paired` flag — then returns to open-pairing advertising, so the next pair
is a clean passkey flow with no on-device step.

**Security posture — the bonded link is the whole gate.** `forgetBond` is honoured **only over the
authenticated, encrypted link**: the `command` characteristic is one of the gated OBC Control
characteristics (§3.3), which carry the `authenticated` (LESC-MITM) access permission — an unbonded
peer gets Insufficient Authentication on the write and never reaches the handler (§8). So only the
bonded phone can issue it, and a bonded phone dissolving *its own* bond is fully consistent with
reject-when-bonded: a stranger can never clear the rider's bond (that still requires either the
bonded phone or physical possession via **Forget phone**), and the command mints no replacement bond
— it only clears. **Ordering is fixed:** the device notifies `commandResult(ok)` on `status`
**before** it clears the bond and disconnects, so the phone always gets its ack; the forget +
link-drop follow the ack, never race ahead of it.

### 4.5 Change signalling

The `storeChanged` status message (§4.3 `msg = 2`) is the **sole** change signal:
notified on every store change, it names which store (route / ride) moved and
carries a monotonic-per-boot `revision`. The app's sync flow: on `storeChanged`
(or on connect), download the relevant list object (§7.4). *(v1 additionally
carried a 10-byte `objectStore` read/notify digest on characteristic `0003`; v2
removes it — it double-signalled the same change and its per-boot `revision`
tripped clients that persisted a last-seen value. The characteristic block is
retired, §3.3.)*

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
- If the channel drops mid-transfer, the device discards the partial (§4.2); the
  app re-opens the CoC (re-reading `psm`) and re-sends the object from the start.

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

### 7.2 `ride` — ride object (v1 / v2)

The compact tracked-ride layout (ratified from the app's B7 codec, byte-for-
byte). Coordinates are stored as **degrees × 1e7** (units of 10⁻⁷ °) and the
point order is `lat, lon` — this object is *not* OBCR and deliberately keeps
the layout the app already pins; the extra digit over OBCR's microdegrees
costs nothing and buys a ~1 cm grid.

**v2 (epic #707)** adds recorded BLE-sensor data — a per-ride heart-rate /
cadence / power summary in the header and per-point `hr`/`cad`/`pwr` samples.
It is a pure **additive object version** (§1 point 5): the version byte goes
`1 → 2`, the header and point record each grow a fixed sensor tail, and there
is **no `protocolVersion` bump**. **A device may serve either version and the
app MUST accept both** — a device that has never seen a sensor keeps writing
v1, and old v1 rides already on the card must still list, download and delete.

```
Header (v1: 23 bytes + name  ·  v2: 31 bytes + name):
  version      u8   = 1 or 2
  name_len     u16  · name UTF-8 (name_len bytes follow immediately)
  start_time   u32  unix seconds
  distance     u32  meters
  moving_time  u32  seconds
  avg_speed    u16  cm/s
  climb        u16  meters
  point_count  u32
  -- v2 only, the per-ride sensor summary: --
  avg_hr       u8   bpm    · 0xFF   = no HR data this ride
  max_hr       u8   bpm    · 0xFF   = no HR data
  avg_cad      u8   rpm    · 0xFF   = no cadence data
  pad          u8   = 0    (reserved)
  avg_pwr      u16  watts  · 0xFFFF = no power data
  max_pwr      u16  watts  · 0xFFFF = no power data

Point record (v1: 14 bytes · v2: 18 bytes, × point_count):
  t_offset  u32  seconds since start_time
  lat       i32  degrees × 1e7
  lon       i32  degrees × 1e7
  ele       i16  meters · INT16_MIN (-32768) = no elevation
  -- v2 only, the per-point sensor samples: --
  hr        u8   bpm · 0xFF   = absent (no strap / stale)
  cad       u8   rpm · 0xFF   = absent
  pwr       u16  watts · 0xFFFF = absent
```

The byte length is fully determined **per version**: v1
`23 + name_len + 14 × point_count`, v2 `31 + name_len + 18 × point_count` —
a decoder reads the version byte first, then rejects a payload whose length
disagrees for that version.

Sensor values are **raw** (no zones / smoothing / NP / TSS); an absent value —
a quantity with no sensor, or a per-point sample whose strap had dropped or
gone stale (>5 s) — encodes as its sentinel (`0xFF` for the `u8` fields, `0xFFFF`
for the `u16` fields), and decodes back to "no data". `pad` is a reserved `0`
byte keeping the `u16` sensor fields 2-byte aligned.

The reference firmware stores each tracked ride as **exactly these bytes**
(`/tracks/RD{id}.ORD`, encoded once at ride Finish), so a ride download is a
verbatim file stream — the §7.1 discipline in the device → app direction.
`protocol-vectors/ride-v1.bin` and `ride-v2.bin` pin the two layouts (the v2
fixture mixes present and absent sensor fields across its header and points).

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

The Config object carries **no firmware-version field** (issue #622): the running
image's version is the DIS **Firmware Revision String** (§3.1, `0x2A26`), which
the app already reads on connect and which reflects the newly-installed image
after a confirmed DFU. Duplicating it here would only risk the two disagreeing.

### 7.4 `routeList` / `rideList` — list objects

Downloaded over the CoC (they outgrow the 512-byte ATT cap fast). Shared shape: a
**6-byte header** + fixed entries, so entry `k` is at `6 + entry_len·k` — O(1)
indexing, no string scanning. In v2 the two list types **differ in entry length**
(`routeList` 76 bytes, `rideList` 72), so the entry size is carried per-list in the
header's `entry_len` byte; readers step by it, never a constant.

```
List header (6 bytes):
  version     u8   = 2
  entry_len   u8   the entry size (76 routeList · 72 rideList) — readers skip by it
  count       u16  entries actually in this object (after the MAX_RIDES / MAX_ROUTES cap)
  total       u16  full catalog size BEFORE the cap
```

**`total`** (v2, epic #632 item 7) makes the >`MAX_RIDES` (or >`MAX_ROUTES`)
truncation visible on the wire: the object is **truncated iff `total > count`**
(the device dropped `total - count` entries in FAT order), and the app surfaces a
one-line warning instead of silently answering "up to date". When nothing was
dropped `total == count`.

`routeList` entry (**76 bytes**) — from the stored OBCR header:

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
  crc32           u32  whole-object CRC-32 (§6) of the stored OBCR bytes · 0 = unknown
```

**`crc32`** (v2, epic #632 item 6) is the whole-object CRC-32 the device computes
at upload commit, persisted in a `/routes` sidecar; a side-loaded file not yet
fingerprinted reads `0` (unknown), filled lazily at first list build. It lets the
app verify *what* a linked id points at (identity-verified badges) and adopt an
identical unlinked copy by content. `rideList` entries are **unchanged** (72
bytes) — which is why entry length is now per-list.

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

An opaque UTF-8 text blob: the device's runtime diagnostics (boot count, uptime,
the BLE link counters, storage stats, and the stack high-water) rendered as text.
No binary layout is pinned — it is a human-readable debugging artifact, not an API.
Downloaded over the CoC like any object (object id `0`); may be empty
(`total_len = 0`). The A9 soak rig reads it after every scenario and reconciles
these counters with its own observations.

### 7.6 `fwImage` — a firmware update image

A `fwImage` object's payload is **exactly the bytes of an `UPDATE.BIN` OBCU
container** — a 64-byte header (magic, raw-image length + CRC-32, `git describe`
version string, header CRC-32) followed by the raw application image. The
container format is normative in [`OBCU_Spec.md`](OBCU_Spec.md); the transfer
layer stays **format-blind** — the container is self-describing and its internals
are opaque to the protocol (§1 principle 2), exactly as a route's OBCR bytes are.
The device writes the payload to the card verbatim and hands it to the bootloader
unchanged.

- **Direction**: app → device only (upload). There is no download direction — the
  running firmware's version is read from DIS (§3.1), not by fetching the image.
- **Singleton stage**: the app uploads with object id `0`; the device assigns no
  id and the `transferResult` echoes `0`. There is one staging slot on the card.
- **Commit**: a CRC-verified commit promotes the staged bytes to `/UPDATE.BIN` in
  the card root, **replacing any existing `UPDATE.BIN`**. A torn or CRC-failed
  transfer never becomes a visible `UPDATE.BIN` (§4.2).
- **Size**: an announced object past the device's update-slot ceiling is rejected
  at announce with `error`, before any bytes stream (§4.2).
- **Install is separate**: staging never installs. Installation is the
  physically-confirmed `installFw` command (§4.4). This mirrors the SD-sideload
  contract — the same `/UPDATE.BIN` a user could copy onto the card by hand.

**The running firmware version is not in a CoC object.** The connected device's
running version is the **DIS Firmware Revision String** (§3.1, `0x2A26`), read
over an open characteristic before or after pairing; after a confirmed update it
reflects the newly-installed image on the next connect. The app displays that —
there is no `fwImage` metadata object and no version field duplicated into the
Config object (§7.3).

---

## 8. Security (A8)

- **Pairing**: LE Secure Connections, **passkey display** — the device is
  `DisplayOnly`, so it shows a 6-digit code on its screen that the rider types
  into the phone's pairing dialog (LESC passkey *entry*, MITM-protected). One
  bonded peer at a time.
- **Encryption requirements** once a bond exists:

| Surface | Requirement |
|---|---|
| DIS, BAS, `protocolVersion` | none (open — lets the app identity/version-check before pairing) |
| every other OBC Control characteristic | encrypted, LESC-authenticated link |
| the L2CAP CoC | encrypted link (opening it plaintext is refused) |

  The gated characteristics carry an `authenticated` (LESC-MITM) access
  permission; an unbonded peer discovers the service but gets Insufficient
  Authentication on every gated read / write / subscribe, and the CoC accept is
  refused on an unencrypted link.

- **Bond store**: the single peer's keys (LTK, peer identity + IRK, security
  level) persist in the device's RRAM settings carve, so a bond survives power
  cycles **and firmware reflashes** (the carve sits above the application image;
  a normal firmware download leaves it intact). At boot the device re-arms the
  bond so the phone's rotating RPA reconnect resolves against the stored peer IRK
  and re-encrypts with the stored LTK — no dialog, no interaction.

- **Single-peer policy — reject-when-bonded** (#455, reverses the original A8
  rule): exactly one bond slot, and **while it is occupied the device refuses
  every new pairing attempt** — whether from a stranger or from a peer claiming
  the bonded identity. A stored bond can only be cleared by the rider: the
  hold-guarded **Forget phone** action in Settings ▸ Bluetooth zeroes the bond
  slot, removes the peer from the host's bond table + resolving list, and drops
  the connection if that peer is connected. After Forget, the next pairing is
  open again (passkey display, as at first pairing). Physical possession is
  thus still the gate — but it now guards the *clear* step instead of the
  replace step, so a stranger who can see the screen can no longer silently
  evict the rider's phone by pairing.
- **Reject mechanics + what the rejected phone sees**: the pairing link is not
  bondable while a bond is stored (a completed pairing could never persist
  keys), and the device refuses the attempt at its first SMP surface — it
  suppresses the passkey display and **drops the link**. The stranger's phone
  surfaces a generic OS pairing failure; the device screen shows nothing
  (locked: the "this device is already paired to another phone" message is
  app-side only). **No distinguishable SMP failure reason crosses the wire**:
  the host stack auto-answers the SMP Pairing Request before the application
  sees it (no hook to answer with a chosen reason code such as
  `Pairing Not Supported`, 0x05), and iOS would not surface an SMP reason code
  to the app anyway — CoreBluetooth reports only a generic pairing/connection
  failure. The app must infer "already bonded elsewhere" from context, not
  from a code (see the iOS epic's already-bonded UX issue).
  If the phone forgets the device **while offline** (app H2 + iOS Settings) its
  re-pair attempt is rejected like any other until the rider runs Forget phone on
  the device. A forget **while connected** avoids that wedge: the app sends
  `forgetBond` (§4.4 cmd 4) over the bonded link, so the device clears its own
  bond and the next pair is open again with no on-device step.

- **Reconnect policy**: the device keeps a **stable static random address** and
  does **not** enable device-side privacy/RPA — the phone stores that identity
  and reconnects on any adv contact, which is what CoreBluetooth's background
  reconnect keys on. The reverse direction (identifying the phone behind its
  rotating RPA) uses the stored peer IRK in the controller resolving list, not a
  filter accept-list. Net: bonded + powered + in range ⇒ connected + encrypted,
  no user interaction.

---

## 9. Changes from v1 (the repin list)

Protocol v2 (epic #632) is the one coordinated wire break. The iOS mirror
([`companion-ios/OBCProtocol.md`](companion-ios/OBCProtocol.md)) is updated to
match in the same change; each item below is a single-spot repin on both sides,
pinned by the shared `protocol-vectors/` fixtures:

1. **`protocolVersion` read widened** `u16` → `version u16 · store_epoch u32`
   (§1, §3.3): the app reads the store epoch alongside the version on every
   connect and scopes id-keyed state to `(serial, epoch)`. The version+epoch read
   gates `ackRides` and reconcile (ack fail-closed, §1).
2. **`TransferControl` 16 → 12 bytes** (§4.2): the permanently-`0` `offset` field
   and its `error`-on-nonzero reject are gone (transfers restart, not resume).
3. **Download announce folds into `status`** (§4.3 `msg = 4`, §4.2): the announce
   moves off `transferControl`, which becomes **write-only, no CCCD**. All
   device → app control traffic is now one notify characteristic — one
   subscription, one ordering domain (the split-CCCD failure mode is gone).
4. **`objectStore` digest removed** (`…0003` retired, §3.3 / §4.5): `storeChanged`
   (§4.3 `msg = 2`) is the sole change signal.
5. **`diagnostics` characteristic removed** (`…0006` retired, §3.3): it returned 0
   bytes; real diagnostics cross the CoC as object type `4` (§7.5) — unchanged.
6. **`routeList` entry 72 → 76 bytes** (§7.4): a trailing whole-object `crc32`
   (`0` = unknown) lets the app verify linked-route identity and adopt by content.
   `rideList` entries are unchanged (72), so entry length is now **per-list**.
7. **List header 4 → 6 bytes** (§7.4): `version 2 · entry_len · count u16 ·
   total u16` — `total` surfaces the >`MAX_RIDES`/`MAX_ROUTES` truncation on the
   wire (truncated iff `total > count`).
8. **Unchanged in v2** (already ratified in v1): the custom `3C92xxxx-…` UUID base
   (§3.3), the ride object (§7.2), the Config blob + append-only rule (§7.3),
   CRC-32/IEEE (§6), the 244-byte chunk preference (§3.4), the `fwImage` object
   type (id `5`, §7.6) and `installFw` / `forgetBond` commands (§4.4), and the
   `transferResult` / `commandResult` status envelopes (§4.3).

## Reference implementation

Firmware: the `obc-ble` workspace crate (descriptor codec + transfer state
machine, lands with A5) and `obc-route` (OBCR v2). App:
`companion-ios/Packages/OBCKit` (`OBCTransport/Transfer`, `Codecs/`,
`BLE/GATT.swift`). Shared fixtures: [`protocol-vectors/`](protocol-vectors/) —
routes with/without waypoints, a ride, a config blob, the route list, and
transfer-descriptor transcripts, asserted byte-exact from both languages.
