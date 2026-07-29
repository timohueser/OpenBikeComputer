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
> ([`companion-ios/OBCProtocol.md`](../companion-ios/OBCProtocol.md)) defers to it:
> where they disagree, this spec wins and the mirror is corrected. §9 lists the
> v1 → v2 wire changes so the app-side repin is a checklist, not a diff hunt.

> **The title says BLE; most of the document does not.** BLE came first and named
> the file, but only §2 (advertising), §3 (the GATT table), §5 (the CoC) and §8
> (pairing) are actually about the radio. §1 (identity), §4 (the object model,
> descriptors, status envelope and **commands**), §6 (CRC) and §7 (object layouts)
> are the transport-free contract, and USB (§10) binds the same bytes to a
> different wire. Rules stated in those sections — including which peers may send
> `ackRides` and what `synced` means (§4.4) — apply to **every** transport unless
> they say otherwise. The file has not been renamed because the URL is load-bearing
> across the firmware, the app and the web builder; §10 is the map of what is
> radio-specific.

All multi-byte integers are **little-endian** (matching OBCM/OBCR). Shared
binary test vectors pinning these layouts live in
[`specs/vectors/`](vectors/) and are consumed by both `cargo test`
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
   OBCR v3 bytes and is written to SD verbatim; a ride crosses as the compact
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

## 1. Protocol version, store epoch & map-format version

`protocol_version` is an unsigned 16-bit integer, **currently `2`**, exposed
first in the `protocolVersion` read (§3.3). It covers everything in this
document: UUIDs, descriptor layouts, object types, and status codes.

**v2 widened the read** from a bare `u16` to `version u16 · store_epoch u32`, and
**E1 (#911) appended `obcm_version u8`** — seven little-endian bytes:

```
protocolVersion read (7 bytes, little-endian):
  version       u16   the protocol version (2)
  store_epoch   u32   the device's current store-epoch nonce
  obcm_version  u8    the OBCM map-format version this firmware's reader reads
```

- The app reads it on every connect, before any other OBC Control traffic. It is
  an **open** (pre-pairing) read (§8), so the app knows the version, the epoch and
  the map format *before* `ackRides` or any reconcile write fires.
- On version mismatch the app **surfaces and stops** (banner / disabled sync) —
  it must never trap or attempt a best-effort decode. A **v1 peer** reads the
  first `u16 = 2` and takes exactly that mismatch path; **there is no dual-version
  serving** — the device speaks v2 only. The device ignores traffic it can't
  parse and answers unknown commands with `unknown` (§4.4).
- Additive, compatible changes (a new object type id, a new command, a new
  waypoint type, **a trailing field on a length-driven read**) do **not** bump the
  version; changing an existing layout does.

**The read is decoded by length, and that *is* its version mechanism.** This has
been true since the store-less short read (#776) — it is not a special case bolted
onto a fixed shape. Three lengths are defined:

| Bytes | Served by | Decodes to |
| --: | :-- | :-- |
| 7 | a device with a mounted store | version + epoch + map-format version |
| 6 | a firmware predating `obcm_version` | version + epoch, `obcm_version` **absent** |
| 2 | a device with **no mounted store** | version only, `store_epoch` **absent** |

A reader takes each field on "did at least this many bytes arrive", and **ignores
bytes past the fields it knows** — so a future trailing field breaks no shipped
peer. A field that did not arrive decodes to *absent* (`nil` / `None` / `null`),
**never to a fabricated default**: `store_epoch = 0` names a legal id era the
device never claimed, and `obcm_version = 0` reads as "this device supports OBCM
v0" and would refuse every real map. Absent means *unknown*, and unknown has its
own defined behaviour in both cases (ack fail-closed below; §6(c)'s
no-known-target-firmware branch for the map version).

**Why appending `obcm_version` did not bump `protocol_version`.** A bump is a hard
stop — the mismatch path above disables sync in both directions, by design, because
a bump means the wire is no longer mutually intelligible. That is the wrong signal
here. The field is optional by construction: a peer that predates it reads seven
bytes, takes the six it understands, and loses nothing it had; a peer that expects
it against an older device reads six, gets *absent*, and takes the defined unknown
branch. Neither side is wrong and neither needs to stop, so bumping would break a
pair that is fully interoperable in order to announce a field that is allowed to be
missing. The precedent is `routeList`'s 76 → 84-byte entry (§7.4, §9): a trailing
field appended to an existing layout whose length is self-describing is additive,
and additive changes do not bump. What *would* require a bump is changing or
reordering a field already defined here — which this does not do: bytes 0–5 keep
their meaning and their offsets, and the new field is byte 6.

**Map-format version (`obcm_version`).** The OBCM version (`OBCM_Spec.md`) the
running firmware's map reader reads — `10` at time of writing. It is a **different
number in a different sequence** from `protocol_version` beside it: one is this
wire contract, the other is the on-card map file format, and neither is derivable
from the other. Nor is it derivable from the DIS firmware-revision string (§3.1),
which is a release string that maps to a format version only through a table that
exists nowhere. The reference firmware sources the byte from
`obc_formats::obcm::VERSION` — the same constant its reader validates every
`.obcm` header against — so what a device *claims* to read and what it *does* read
cannot drift.

Its consumer is `OBCC_Spec.md` §6(c): a host offering map artifacts MUST NOT offer
one whose `obcm_version` the connected device cannot read, and SHOULD show it as
unsupported *with the reason* rather than hiding it. The reader supports exactly
one version at a time (earlier maps are repacked), so this is a single `u8`, not a
range. A host that reads *absent* — an older firmware, or the store-less short read
— takes §6(c)'s branch for no known target firmware: offer the download, stating
the version. Guessing would mean either refusing a map that works or offering one
that doesn't.

A store-less device serves 2 bytes even though it knows its OBCM version. The
fields are positional and `store_epoch` has no absent encoding, so byte 6 cannot be
reached without inventing bytes 2..6; a 3-byte `version · obcm_version` form would
make byte 2 mean two different things depending on the total length, which is
decodable but is the kind of positional special case that outlives the reason for
it. Nothing is lost: a device with no card has nowhere to put a map.

**Store epoch.** `store_epoch` is a `u32` TRNG nonce that names a store's **id
era**. It is **card-resident** — persisted as **`EPOCH.OBE`** in the card root, so
the card carries its own era name — and changes only on an **id-era reset**. The
file is a fixed 12-byte record — `magic "OBCE" · version u8 · pad u8 · epoch u32 ·
crc16`, little-endian, CRC-16 over the first 10 bytes; the host-tested codec in
`obc-app::settings` is the layout's authoritative home, as with the other card
sidecars. An absent, short, torn (CRC-failed), or foreign-version read decodes to
**no epoch**, and the device mints a fresh era onto the card. A boot whose epoch
*persist* fails serves **no epoch** that session (the version-only short read
below) and retries the mint next boot — a store with no *proven* era name is never
given one on the wire. Its purpose and the app-side keying are epic #632 item 5;
the mint rule lives with the device implementation (V3, card-resident move #776).
The essentials the wire depends on:

- **Every durable app↔device link keys on bare `u16` object ids** (the ride
  synced-set + delete tombstones, the route `deviceObjectID` links). Ids mint at
  `max(card-scan max + 1, RRAM floor)`: **SD filenames guard stored ids, the RRAM
  floor guards deleted ids.** The **era events** — the only ways the device can
  re-issue an id it minted to a *different* object under the *same* epoch — are a
  lost RRAM floor (a full-chip reflash, a factory reset, or a torn id-marks write:
  each mints a fresh epoch onto the card) **or** an absent/torn card epoch file
  (mints a fresh one). Because the epoch lives on the card, a **card swap is a
  store transplant**: the served epoch is the *new* card's (swap back and the old
  era returns), so the same device never conflates two cards' id spaces. A lost
  floor still reopens only the deleted-id band on the card that was in (filenames
  guard stored ids); a fresh/reformatted card reopens all of it and, having no
  epoch file, mints a fresh era anyway.
- The **never-reuse guarantee** is therefore *within an epoch, for objects the
  device itself minted*: while the epoch holds, the device never re-assigns such
  an id to a different object. Across an epoch change the id space legitimately
  reopens, and the new nonce makes that visible so the app can scope its state to
  `(device serial, store epoch)` and never silently alias months-old ids. The
  card-resident epoch **closes** the former residual hole (#776): a card written by
  a **different device** presents *its own* epoch, so on this device it reads as a
  distinct `(serial, epoch)` scope — no foreign ids alias under a shared era.
- **No mounted store ⇒ no epoch.** A device with no card has nothing to name and
  nothing to prove, so it serves only the **2-byte version** (`version` alone, no
  `store_epoch`, and therefore no `obcm_version` after it). The app treats the
  absent epoch as a **failed identity read** — ack fail-closed (below) — never as
  epoch `0` (a legal value). The full shape is served whenever a store is mounted.
- **Ack fail-closed contract.** The version+epoch read **gates** `ackRides` and
  every reconcile write: a connection whose identity read failed — including the
  short version-only read above — sends no ack and reconciles nothing (library
  browsing is unaffected). V5 implements it; it exists so a failed read can never
  stamp synced-flags or badges under an unknown era.

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
| Firmware Revision String | `0x2A26` | UTF-8 version of the **running** image — see the dialect below; after a confirmed DFU it reflects the newly-installed image (the app's device-version display, §7.6) |
| Hardware Revision String | `0x2A27` | UTF-8 board id, e.g. `nrf54l15-dk`, `obc-lm20-r1` |
| Serial Number String | `0x2A25` | 16 uppercase hex digits — the nRF `FICR.DEVICEID` |

**The firmware-revision dialect** (#996, epic #773). The value is the version
the running image was *wrapped* with, in this preference order:

1. the **installed image's OBCU version string, verbatim** — the `fw_version`
   field of the container the device installed (`OBCU_Spec.md` §1), which the
   bootloader handoff page records as the installed image (§2). For a released
   build that is the release tag, e.g. `v1.3.0`;
2. otherwise the **build's bare git short hash**, e.g. `ca9b336` — a device
   flashed over SWD, which has never installed a container and therefore has no
   version to report.

Case 1 is the one that matters to a host: it is the only string that can be
compared against a published release ("is `v1.3.0` newer than this?"). Hosts
parse it as a release version with an optional leading `v`.

Case 2 is deliberately **not parseable as a version**, and the consequence is
locked, not incidental: a host that cannot read a running version must never
offer an automatic update. A development device stays on whatever its owner
flashed, and gets back onto the release track through the manual install path.
The value is ≤ 32 bytes (the OBCU `fw_version` field width) and is assembled in
exactly one place in the firmware, so this characteristic and the USB
device-information frame (§10) always carry identical bytes.

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
| `0008` | `protocolVersion` | read | `version u16 · store_epoch u32 · obcm_version u8` — §1, decoded **by length** (7 / 6 / 2 bytes). Readable **without** encryption |

**Six characteristics** (v2 drops two of v1's eight). The `0003` and `0006`
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
| `1` | `route` | app → device (upload), device → app (detail read) | an OBCR v3 file, §7.1 |
| `2` | `ride` | device → app | ride object v1 or v2, §7.2 |
| `3` | `config` | — | reserved on the CoC; Config crosses GATT (§3.3) |
| `4` | `diagnostics` | device → app | diagnostics blob, §7.5 |
| `5` | `fwImage` | app → device (upload) | a complete `UPDATE.BIN` OBCU update image, §7.6 |
| `6` | `routeList` | device → app | list object, §7.4 |
| `7` | `rideList` | device → app | list object, §7.4 |
| `8` | `echo` | both | dev/test only: device streams back what it received (A5's loopback) |
| `9` | `trip` | app → device (upload), device → app (detail read) | trip object v1, §7.7 |
| `10` | `tripList` | device → app | list object, §7.4 |
| `11`–`15` | — | — | reserved (sensors, M4) |
| `16` | `map` | host → device (upload) | an `.obcm` map — **USB only** (§10), see below |

`map` is the one type BLE could never have carried: a map is hundreds of
megabytes, so the type would have been dead weight until a USB bulk endpoint
existed (#889). It sits at `16` rather than at the next free number because
`11`–`15` are already spoken for; the byte is a `u8` and there is no reason to
crowd a reserved band. Like `fwImage`, the transfer layer is **format-blind** —
the payload is opaque bytes.

A map upload carries **four rules the other upload types do not** (#927). All of
them follow from one fact: a map is hundreds of megabytes, which makes it the
only object whose transfer is measured in minutes rather than frames.

1. **New-only.** `object_id` MUST be `0xFFFF`. A named id is answered
   `notFound` — for a map there is no id an upload may target. Replacing a map in
   place would mean destroying the stored bytes as the new ones arrive, which
   forfeits the "a failed CRC never touches the old copy" guarantee below on the
   one object a device cannot rebuild for itself. Replacing a map is *upload the
   new one, then delete the old one*.
2. **A free-space guard at announce.** A device SHOULD refuse with `storageFull`
   when the announced length plus a reserve it keeps for ride logs and sidecars
   exceeds free space, **before any byte streams** — a transfer that fails at
   byte 300,000,000 has cost the rider minutes. A device that cannot *measure*
   free space allows the transfer rather than refusing every one.
3. **An announced length below one OBCM header** is answered `error`.
4. **The commit point is the format magic, patched in last.** Because staging a
   map in a temp file and copying it would double both the write time and the
   free space required, the reference firmware streams a map straight into its
   final file with the leading 4-byte `OBCM` magic held back, and writes that
   magic only after the whole-object CRC *and* the header have validated. An
   interrupted transfer therefore leaves a zero-magic file that every reader
   refuses and a boot sweep reclaims — the same durability the other types get
   from an invisible temp, reached without the copy.

**Where an uploaded map lands** is a device convention, not a wire one, but the
reference firmware's is worth stating because it follows the `RT{id}.OBR` rule
this section already describes: a received map is `MP{id}.OBM` in the card root
(`OBM` is an 8.3-safe twin of `.obcm`; the FAT layer creates short names only),
so the durable object id is guarded by the filename like every other stored
object. Which map the renderer streams from is recorded separately on the card,
and a committed upload becomes that selection — it takes effect at the device's
next boot, since the map's parsed tables are read once at startup.

**The device is never told a map's name.** The descriptor has no field for one,
the payload is opaque, and the OBCM header (`OBCM_Spec.md` §1) carries no name,
build date or source-snapshot date. A device can therefore enumerate the maps it
holds — id, filename, size, OBCM version, bounding box, all derivable from the
card — but not describe where any of them came from. Closing that gap needs a
new §4.4 command, not a change to this section.

**Object ids** are `u16`, assigned by the device, **stable for the life of the
stored object — including across device reboots** — and enumerated by the list
objects. Durability is what lets the phone persist the id an upload committed
under and later reconcile ("is my copy still on the device?") or replace that
object in place — and, for rides, what the app's synced-set and delete
tombstones key on. Ids mint at `max(card-scan max + 1, RRAM floor)`: the
reference firmware encodes the id in the stored filename (routes `RT{id}.OBR`,
rides `RD{id}.ORD`, trips `TP{id}.OBT`) — **SD filenames guard stored ids** — and an RRAM floor
guards **deleted** ids. **Within a store epoch, an id the device minted is never
re-issued to a different object.** The era events that legitimately reopen the id
space are a lost RRAM floor (reflash / factory reset / torn id-marks write) or an
absent/torn **card-resident** epoch file; each mints a fresh `store_epoch` (§1) —
so the app scopes id-keyed state per epoch and an era change never silently aliases
a stale id. Because the epoch rides the card, a card swap transplants the era, and
a card written by a *different* device presents *its own* epoch — a distinct
`(serial, epoch)` scope on this device, which is what **closes** the former
foreign-card hole (#776). Conventions:

- `0xFFFF` on an upload means "new" — the device assigns an id and reports it
  in the `transferResult` (§4.3). Uploading to an existing id replaces that
  object atomically (commit-then-swap; a failed CRC never touches the old copy).
  **`map` is the exception**: it is new-only, because atomic replacement of a
  several-hundred-megabyte object is not something a device can offer — see the
  map rules above.
- Objects that exist once (`routeList`, `rideList`, `tripList`, `diagnostics`,
  `echo`, and the `fwImage` staging slot) use object id `0`. A `fwImage` upload is
  a singleton stage: the app sends object id `0`, the device assigns no id and the
  `transferResult` echoes `0` (§7.6).
- Ids `0xFF00`–`0xFFFE` are a **session-scoped** band for objects that exist on
  storage without a device-assigned identity (side-loaded dev files). They are
  valid transfer targets within a connection but must never be persisted by the
  app — they may name a different object after a reboot.
- **Trip** objects (type `9`, §7.7) draw their ids from a **separate device
  counter** — a trip id is never shared with a route or ride id — under the same
  durability rules (stable across reboots, `0xFFFF` on upload = "new",
  replace-by-id atomic). A trip **references** route object ids and never contains
  route bytes; a route referenced by no stored trip is a top-level route (§7.7).

At most **one transfer is in flight at a time** — the CoC carries exactly one
object's bytes between a `transferControl` open and its `transferResult`. A
second open while one is active is answered with `busy`. The terminal result is
the ownership boundary: the device clears its active gate **before** notifying
that result, and the app holds its local transfer slot until it has consumed a
correlated close (matching object id and committed byte count).

The CoC is an unframed stream, so an exchange that does not reach that close is
not reusable. After cancellation, timeout, a mismatched/late answer, or an upload
descriptor-open reject, the app closes and reopens the CoC **before** handing its
slot to another descriptor. This also discards bytes the upload sender may have
queued before its asynchronous reject arrived. A device treats that channel drop
as an implicit abort, discards the partial, clears the gate, and sends no late
`transferResult` for the dead exchange.

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
nothing is retained). Closing the CoC is the implicit-abort/reset form: the
device performs the same discard but sends no result for a channel the peer has
already abandoned.

A descriptor that names an unknown type/id or arrives mid-transfer is answered
with a `transferResult` carrying `error` / `notFound` / `busy` (§4.3) and does not
disturb an active transfer.

**Storage-full reject (descriptor-open).** A **new**-route upload — `op=1`,
route type, `object_id = 0xFFFF` (or a route id the device doesn't hold) —
that would grow the catalog past its cap is rejected at the `transferControl`
write, **before the device consumes payload bytes**, with `transferResult` status
`storageFull` (§4.3); no partial file is created. Because v2 has no separate
upload-accepted handshake, the sender may already have queued raw CoC bytes when
that asynchronous result arrives; it resets the CoC as described above.
**Replace-by-id uploads of
an existing route are exempt** — they reuse a catalog slot rather than growing
it, so updating a stored (or actively-navigated) route never hits the cap. The
`object_id` in the reject echoes the request (`0xFFFF` for a fresh upload). The
app surfaces this as "delete routes on the device"; the reference cap is 64
routes.

The same descriptor-open reject guards **new-trip uploads** (`op=1`, trip type
`9`, `object_id = 0xFFFF` or a trip id the device doesn't hold): a trip that would
grow the trip catalog past its cap is refused with `storageFull` before any bytes
are consumed, and **replace-by-id uploads of an existing trip are exempt**. The
reference cap is **16 trips**. (A trip references route ids only, so its bytes are
tiny — the cap bounds the trip *count*, independent of the 64-route cap.)

**Fresh-upload dedup (idempotent retry).** A **new**-object upload (`op=1`,
route or trip type, `object_id = 0xFFFF`) whose verified whole-object CRC-32
**and** byte length match an object the device already stores (same type) is
answered `committed` with the **existing** object's id — nothing new is stored,
no catalog slot is consumed, and the store revision does not move. This makes an
upload retry convergent: if the link dies between the device's commit and the
app's `transferResult` (the ack is lost), the app re-sends the identical bytes
as a new object, and without this rule the device minted a silent same-content
twin. Content identity is the CRC (the same fingerprint `routeList` /
`tripList` serve, §7.4); the app treats the result exactly like any commit and
links to the reported id. Replace-by-id uploads are not deduplicated — they
target a specific object. The dedup applies at commit time, so the retry's
bytes still stream; a client that wants to skip the bytes entirely should
reconcile against the list CRCs first.

**`fwImage` staging (M4).** A `fwImage` upload (§7.6) stages a firmware update
image to the card over the existing transfer machinery unchanged — whole-object
CRC-32 at commit, no partial resume (an update is ~900 KB ≈ a large route). Two
`fwImage`-specific rules ride the same descriptor path: (1) an announced
`total_len` past the device's update-slot ceiling is rejected at the
`transferControl` write with `error`, **before any bytes are consumed** — the ~900 KB
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
                         6 = storageFull   the named catalog is full — a NEW-object
                                           upload (route past its 64-route cap, trip
                                           past its 16-trip cap) was rejected at
                                           descriptor-open time (§4.2), before any
                                           bytes streamed
  committed_offset  u32       durable byte count: total_len on `committed`, else 0
                              (a download's explicit close reports its total_len)

msg = 2  storeChanged (6 bytes total):
  msg       u8   = 2
  type      u8   which store changed: route (1), ride (2), or trip (9) — the values
                 mirror the object-type numbers (§4.1)
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

Each `storeChanged` store keeps **its own** monotonic-per-boot revision: a trip
upload or delete bumps the **trip** store, never the route store. A UI-composed
cascade ("delete trip & routes", §7.7 — individual route deletes plus the trip
delete) therefore emits **both** a route and a trip `storeChanged`. Unknown
`storeChanged.type` values must be ignored by the app (forward compatibility,
the same posture as unknown `msg` values).

### 4.4 `command` — small imperatives

A write of `cmd u8` + fixed args. Every command is answered with a
`commandResult` (§4.3).

| `cmd` | Command | Args | Effect |
|---|---|---|---|
| `1` | `deleteObject` | `type u8 · object_id u16` | delete a stored route (`1`) or trip (`9`); bumps that store's revision. A trip delete is **non-cascading** (§7.7): it removes only the stored trip object — its member routes become top-level routes — and an unknown trip id answers `notFound`. Ride (`2`) deletion over the link is **reserved** — the reference firmware answers `notFound`: rides are deleted only on the device itself (its Rides screen), and the app hides synced rides locally (tombstones) so a re-sync can't resurrect them |
| `2` | `ackRides` | `count u8 · count × object_id u16` | the app's **ride-possession ack**: the device marks every listed ride id it still stores as synced ("downloaded at least once"). `commandResult.detail` = the newly-flagged count (saturating at 255); a flag change bumps the **ride** store revision. See below |
| `3` | `installFw` | none (`cmd` byte only) | ask the device to install the staged `UPDATE.BIN` — runs the on-device scan + **on-glass confirm** flow (see below). The command only *requests*; it never waits for the human and never installs on its own |
| `4` | `forgetBond` | none (`cmd` byte only) | ask the device to dissolve **its** side of the bond, so an app-side "Forget device" doesn't leave the pair wedged. The device answers `commandResult(ok)` **first**, then clears the bond + drops the link and returns to open-pairing advertising. **Honoured only on the bonded, authenticated link** (see below) |
| `5` | `setClock` | `utc u32 · offset_min i16` | the phone stamps the device's UTC clock + local offset on **every connect** (auto-expiry #638). Stamps the wall-clock set-point, **persists** the offset, and marks the clock *trusted* for the boot — the retention sweep's safety gate. Sent immediately after encryption, **before** `ackRides`. Validated → `error` on a malformed length, `utc < 1577836800`, or `\|offset_min\| > 840`; no store-revision bump (the clock is not an object). See below |
| `6` | `setRouteRetention` | `object_id u16 · retention u8` | the phone sets a stored route's **retention level** (`0` never · `1` 1 day · `2` 1 week · `3` 2 weeks · `4` 1 month · `5` 2 months, auto-expiry #638) **without re-uploading** the route. Writes the level in the device's retention store **without touching `last_used`** (changing retention never resets the usage clock) and bumps the **route** store revision **only on a real change** (the app sees the fresh `expires_at` in the next `routeList`). Additive on protocol v2 — no `protocolVersion` bump. See below |
| `7`–`15` | — | — | reserved (identify/find-my-device, factory reset, …) |

**Next free command: `7`.** (`setClock` landed at `5` and `setRouteRetention`
at `6`, not the `3`/`4` epic #638's draft table drew: that draft predates
`installFw`/`forgetBond` taking `3`/`4`, so #638's two commands slid to `5`/`6`.)

**`ackRides` — possession reconciliation.** The device keeps a per-ride
"synced" flag (it drives the delete-guard cue on the device's Rides screen).
Setting it only when a ride download completes leaves the flag an *event
inference* — any divergence between a peer's library and the device's record
(rides synced before the device tracked the flag, a record lost with a
reflashed card, an app reinstall) would be permanent, because a ride the peer
already holds is never downloaded again. `ackRides` converts the flag into
*reconciled state*: the peer's library is the ground truth for "a copy of this
ride exists over there", and the peer sends the device-namespace ride ids it
holds on every connect (and after edits, as it likes). Rules:

- **Monotonic**: the device only sets flags from an ack, never clears them —
  the flag means "synced at least once", not "still held by the acking peer".
  (A phone-side delete keeps the ride's tombstone, so its id stays in the
  ack list; ids never reuse, so a stale flag can't mislabel a future ride.)
- **Idempotent and order-free**: re-acking a flagged ride changes nothing, so
  a peer may chunk a long list across several `command` writes (the
  reference firmware accepts ≤ 31 ids per write — a 64-byte value) and
  re-send the whole list every connect.
- **Unknown ids are ignored**, answered `ok`: a peer may hold rides the
  device has since deleted. `error` is answered only for a malformed write
  (`count` promising more ids than the write carries).
- **First sync, not last**: flagging a ride records its `synced_at` once. A
  second ack of an already-flagged ride does **not** re-stamp it, so a
  reconnect can never push an auto-expiry countdown anchor forward (#638).

**What `synced` means, and who is allowed to say it.** `synced` means **a durable
copy of this ride exists off the device** — *not* "the phone has it". The
distinction became load-bearing the moment USB (§10) gave the device a second peer,
because the flag is what unlocks deleting the ride, and auto-expiry (#638) counts
from its `synced_at`. Saying it when no durable copy exists loses a rider's ride.

The three sinks, and what each may do:

| Sink | Transport | Acks? |
| :-- | :-- | :-- |
| Companion app | BLE | **Yes** — and *heals*: it re-sends its whole library every connect, which is what repairs a record the device lost |
| Desktop app | USB | **Yes, after `fsync`** — the ack follows the durable write, never the successful transfer |
| Browser | USB (WebUSB) | **Never, on any path** — a file the browser handed to a download is cancellable, overwritable and not yet anywhere; it is not durable, so it is not a sync |

The browser's rule is structural rather than disciplinary: the hosted tier's ride
path is handed a two-method read surface with no `command` on it, so `ackRides` is
not reachable from that code at all.

**Two acking sinks need no coordination** — no per-sink field, no ownership, no new
command — because the ack is add-only and idempotent. A desktop ack and a phone
heal **merge to the same flags in either order**: the phone acking a library that
never held a ride the desktop already fsynced does not un-flag it (the phone's
silence is not evidence), and the desktop re-acking a ride the phone flagged
changes nothing. The `synced_at` stamp is **first-ack-wins**: the ride keeps the
instant it was first flagged with, whichever sink got there first, so no re-ack can
extend a ride's life. (In the reference firmware the question does not arise: both
handlers flag with an unset stamp because the ack path holds no trusted-clock
handle, and the retention sweep sets the one anchor afterwards — §4.4 `setClock`.)

**`installFw` — install the staged update (M4).** After a `fwImage` upload
(§7.6) lands `/UPDATE.BIN` on the card, the app sends `installFw` to ask the
device to install it. The command returns as soon as the request is **accepted**
— it does *not* wait for the human. The device then runs its on-device flow:
scan + validate the staged image, show a **confirm card**, and install only on a
physical **Select press** by the rider. The reply codes map onto the existing
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
confirmation at the device** — the Select press on the confirm card, symmetric
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

**`setClock` — stamp the trusted wall clock (auto-expiry #638).** The device has no RTC: at boot its
clock resumes from a persisted set-point, stale by however long the device was off, and that stale
clock is **untrusted** — nothing is stamped or deleted from it. Exactly two sources establish a
*trusted* clock for the boot: a GPS fix (which carries full UTC) and this command. `setClock` is a
7-byte write — `cmd u8 = 5 · utc u32 · offset_min i16`, all little-endian:

- **`utc`** is the phone's current time in **unix seconds** (UTC). The device sets its wall-clock
  UTC set-point from it (seconds-resolution: the display's minute rolls at the true instant).
- **`offset_min`** is the phone's current **local UTC offset in minutes**, with **DST already
  applied** (`+02:00` → `120`). The phone is the timezone oracle — the device holds no tz tables and
  runs no DST math; expiry arithmetic is pure UTC, and the offset only shifts the *displayed* hour.
  The offset is **persisted** (it survives reboots and seeds the boot display clock) and silently
  refreshed by every connect, so a rider crossing time zones need only reconnect the app.

On a valid write the device stamps the clock, persists the offset, marks the clock **trusted** for
the boot, and answers `commandResult(ok)`. The clock is **not an object** — there is **no
store-revision bump** and no `storeChanged`. Validation answers `commandResult` `error` (§4.3) for a
**malformed length** (not exactly 7 bytes), a **`utc < 1577836800`** (before 2020-01-01 — an
obviously-bogus phone clock), or an **`offset_min` beyond ±840** (±14 h, the real-world −12:00…+14:00
span). A device that predates the command answers `unknown` (§4.4 compat), which the app reads as
"this device predates expiry support" and degrades gracefully.

**Ordering — sent before `ackRides`.** The app sends `setClock` on **every connect, immediately
after encryption and before the first `ackRides`** (or any reconcile write). This is what lets ride
`synced_at` stamping (#638 S3) assume a trusted clock: the `ackRides` that first flags a ride synced
runs *after* the clock is trusted, so the timestamp it stamps is real. (`setClock` itself needs no
identity read — it establishes local time, not id-scoped state — but it shares the same
post-encryption prologue as the version+epoch read and the ack, §1.)

**`setRouteRetention` — set a route's expiry policy (auto-expiry #638).** Retention is mutable
device-local state, never baked into the byte-pinned OBCR route file (§7.1): it travels as this
command and lives in an SD sidecar (route id → retention + `last_used`). `setRouteRetention` is a
4-byte write — `cmd u8 = 6 · object_id u16 · retention u8`, all little-endian:

- **`object_id`** names a stored route. An id the device does not hold answers `commandResult`
  `notFound` (2).
- **`retention`** is the level enum: `0` never · `1` 1 day · `2` 1 week · `3` 2 weeks · `4` 1 month
  (30 d) · `5` 2 months (60 d). A value **above `5`** answers `error` (4), as does a write that is not
  exactly 4 bytes. The device sanitises any unknown stored/wire byte to `Never` on read, so a
  forward-compat value can never surprise-delete a route.

On a valid write to a known route the device writes the level into its retention sidecar **without
touching `last_used`** — changing retention never resets the usage clock, so a route mid-countdown
keeps its anchor — and answers `commandResult(ok)`. A **real** change bumps the **route** store
revision and fires `storeChanged(route)` (§4.3 `msg = 2`), so the app re-reads the `routeList` and
sees the route's new `expires_at` (§7.4). **Idempotence:** setting the level a route already has is
`ok` with **no** revision bump and no `storeChanged` — only a real change moves the store.

The app sends it **(a)** right after a route upload's `transferResult` commits — the result carries
the assigned id — so a freshly-uploaded route gets its chosen retention without a second upload, and
**(b)** any time the user edits retention for an on-device route. The device stamps a route's
`last_used = now` at **upload commit** (when the clock is trusted), so a retention set right after an
upload yields `expires_at = upload_time + retention`; an upload under an untrusted clock leaves
`last_used` unstarted (`0`) and the retention sweep starts the clock on its next pass (the safe
fallback — nothing deletes on sight). A device that predates the command answers `unknown` (§4.4
compat), which the app reads as "this device predates expiry support" and degrades gracefully.

### 4.5 Change signalling

The `storeChanged` status message (§4.3 `msg = 2`) is the **sole** change signal:
notified on every store change, it names which store (route / ride / trip) moved and
carries a monotonic-per-boot `revision`. The app's sync flow: on `storeChanged`
(or on connect), download the relevant list object (§7.4). Changes that arrive
while a list is in flight are coalesced into a follow-up read; they do not cancel
the opened transfer. Notifications remain best-effort BLE edges, so the app also
performs a low-cadence catalog audit while connected (the reference app uses 60 s)
to converge after a dropped edge. *(v1 additionally
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

### 7.1 `route` — an OBCR v3 file

A route object's payload is **exactly the bytes of an OBCR v3 file** — see
[`OBCR_Spec.md`](OBCR_Spec.md), including the waypoints section (categorized and
carrying a signed lateral offset since v3). The phone encodes imported GPX/TCX to
OBCR v3 (waypoints included — Delta 2 in the mirror); the device writes the
payload to SD verbatim and serves it back verbatim. The device **rejects** a v1/v2
payload at commit, so an app build that still encodes v2 must be updated with this
bump rather than silently uploading routes the device won't open.

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
`specs/vectors/ride-v1.bin` and `ride-v2.bin` pin the two layouts (the v2
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

### 7.4 `routeList` / `rideList` / `tripList` — list objects

Downloaded over the CoC (they outgrow the 512-byte ATT cap fast). Shared shape: a
**6-byte header** + fixed entries, so entry `k` is at `6 + entry_len·k` — O(1)
indexing, no string scanning. The list types **differ in entry length**
(`routeList` 84 bytes, `rideList` 72, `tripList` 76), so the entry size is carried
per-list in the header's `entry_len` byte; readers step by it, never a constant.

```
List header (6 bytes):
  version     u8   = 2
  entry_len   u8   the entry size (84 routeList · 72 rideList · 76 tripList) — readers skip by it
  count       u16  entries actually in this object (after the MAX_RIDES / MAX_ROUTES / MAX_TRIPS cap)
  total       u16  full catalog size BEFORE the cap
```

**`total`** (v2, epic #632 item 7) makes the >`MAX_RIDES` (or >`MAX_ROUTES` /
>`MAX_TRIPS`) truncation visible on the wire: the object is **truncated iff `total > count`**
(the device dropped `total - count` entries in FAT order), and the app surfaces a
one-line warning instead of silently answering "up to date". When nothing was
dropped `total == count`.

`routeList` entry (**84 bytes**) — from the stored OBCR header, plus the auto-expiry
tail (offsets `76..84`, epic #638 S4):

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
  crc32           u32  whole-object CRC-32 (§6) of the stored OBCR bytes · 0 = unknown   (offset 72)
  expires_at      u32  unix seconds the route auto-deletes at · 0 = never / not yet started  (offset 76)
  retention       u8   the stored retention enum value (0 never … 5 = 2 months)          (offset 80)
  reserved        u8[3]  = 0                                                               (offset 81)
```

**`crc32`** (v2, epic #632 item 6) is the whole-object CRC-32 the device computes
at upload commit, persisted in a `/routes` sidecar; a side-loaded file not yet
fingerprinted reads `0` (unknown), filled lazily at first list build. It lets the
app verify *what* a linked id points at (identity-verified badges) and adopt an
identical unlinked copy by content. A stored route whose genuine CRC-32 happens to
be `0` (probability 2⁻³²) is indistinguishable from "unknown" and is served — and
read — as unknown; the consequence is merely "no badge until re-upload", the
conservative direction, so implementations do **not** special-case it. `rideList`
entries are **unchanged** (72 bytes) — which is why entry length is per-list.

**`expires_at` / `retention`** (auto-expiry #638 S4) report the route's device
truth so the app can show a countdown. **`retention`** is the level set by
`setRouteRetention` (§4.4 cmd 6). **`expires_at`** is computed **at list-encode
time** — `last_used + retention days`, or `0` when the route is `Never` or its clock
has not started (`last_used == 0`). Both are **device-computed volatile state** — an
`expires_at` that merely ticked, or a retention edit, is *not* a change of route
content — so they sit deliberately **after** the content `crc32` and are **outside
its coverage**: the `crc32` fingerprints only the stored OBCR bytes, so a route
whose expiry moved never spuriously reads as "content changed". The 76-byte v2 core
(offsets `0..76`) is **byte-identical** to before; the tail is appended via the
`entry_len` mechanism — the format's designed additive path (list `version` stays
`2`; a reader steps by `entry_len` and decodes the prefix it knows), so growing the
entry needs **no** `protocolVersion` bump (§1).

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

`tripList` entry (**76 bytes**) — from the stored trip object (§7.7). It mirrors
`routeList`'s **v2 core**: the same trailing whole-object `crc32`, so the app's
identity / outdated-copy machinery works on trips exactly as on routes (a stage
reorder changes neither `byte_len` nor `name`, so only the `crc32` reveals it). It
carries **no** auto-expiry tail — trips have no per-object retention — which is why
`tripList` stays 76 bytes while `routeList` grew to 84:

```
  object_id         u16
  reserved          u16  = 0
  byte_len          u32  stored trip file size
  total_distance_m  u32  summed over resolvable stages (device-computed)
  total_ascent_m    u32  summed over resolvable stages
  stage_count       u16  as stored (incl. dangling refs)
  reserved          u16  = 0
  name_len          u8   ≤ 48
  name              char[48]  UTF-8, zero-padded
  reserved          u8[3]  = 0
  crc32             u32  whole-object CRC-32 (§6) of the stored trip bytes · 0 = unknown
```

**`total_distance_m` / `total_ascent_m`** are summed by the device over the trip's
**resolvable** stages — a dangling stage ref (a member route deleted individually,
§7.7) contributes nothing — while **`stage_count`** counts every stage as stored,
dangling refs included, so `stage_count` can exceed the number of stages the totals
drew from. **`crc32`** has the same semantics as the `routeList` `crc32`: computed
at upload commit, `0` = unknown for a side-loaded trip not yet fingerprinted.

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
  at announce with `error`, before the device consumes payload bytes (§4.2).
- **Install is separate**: staging never installs. Installation is the
  physically-confirmed `installFw` command (§4.4). This mirrors the SD-sideload
  contract — the same `/UPDATE.BIN` a user could copy onto the card by hand.

**The running firmware version is not in a CoC object.** The connected device's
running version is the **DIS Firmware Revision String** (§3.1, `0x2A26`), read
over an open characteristic before or after pairing; after a confirmed update it
reflects the newly-installed image on the next connect. The app displays that —
there is no `fwImage` metadata object and no version field duplicated into the
Config object (§7.3).

### 7.7 `trip` — a trip object (v1)

A **trip** groups planned routes into one named unit (one folder on the device,
one card in the app). It is a tiny metadata object that **references route object
ids** in ride order — it never contains route bytes. Routes stay byte-identical
OBCR v3 files (§7.1); membership edits never touch a route payload. The reference
firmware stores each trip as `TP{id}.OBT` beside the `RT{id}.OBR` route files (no
FAT subdirectories); trip ids come from a separate device counter (§4.1).

```
trip object v1 (56-byte header + 2 bytes/stage, little-endian):
  version      u8   = 1
  reserved     u8   = 0
  stage_count  u16
  name_len     u8   ≤ 48
  name         char[48]  UTF-8, zero-padded
  reserved     u8[3]  = 0
  stages       stage_count × u16   route object ids, ride order
```

The object length is fully determined by its header: `56 + 2·stage_count` bytes.

**Semantics:**

- **Reference-only.** A stage is a route object id; the trip carries no route
  bytes. A route referenced by no stored trip is a top-level route, and membership
  is exactly one level deep — a route lives in at most one trip, or standalone.
- **Dangling refs are tolerated on read.** A member route deleted individually
  (over the link or on the device) does **not** invalidate the trip: the device
  serves the trip verbatim, dangling ids and all. **The device never rewrites a
  stored trip** — dangling refs persist until the next trip **upload** replaces
  the object by id, and that upload arrives compacted because the **app** (which
  owns validation) builds it from resolvable stages. The `tripList` totals (§7.4)
  sum only resolvable stages, while its `stage_count` counts every stored stage.
- **Uploads commit verbatim.** A trip upload referencing unknown route ids is
  stored as sent — validation is the app's job, not the device's.
- **Recommended upload order: stages first, the trip object last.** An interrupted
  whole-trip push then never leaves a trip pointing at nothing, and re-running the
  push is idempotent (each stage replace-by-id, the trip object replace-by-id).
- **Protocol-level delete removes only the trip object.** `deleteObject` (§4.4)
  with the trip type frees the trip and **leaves its member routes as top-level
  routes** — it never cascades. A "delete trip *and* its routes" action is a UI
  decision, composed by the initiating side as individual route deletes **plus**
  the trip delete; the wire has no cascading delete.

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
([`companion-ios/OBCProtocol.md`](../companion-ios/OBCProtocol.md)) is updated to
match in the same change; each item below is a single-spot repin on both sides,
pinned by the shared `specs/vectors/` fixtures:

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

**Post-v2 additive (no version bump, §1).** Auto-expiry (#638) layers additive
changes on v2, not part of the v1→v2 break above:

- `setClock` (§4.4 cmd `5`, S2) — the phone stamps the trusted clock every connect.
- `setRouteRetention` (§4.4 cmd `6`, S4) — the phone sets a route's retention level.
- **`routeList` entry 76 → 84 bytes** (§7.4, S4): the auto-expiry tail
  (`expires_at u32 · retention u8 · reserved u8[3]`) appended **after** the content
  `crc32` (outside its coverage — device-computed volatile state), via the
  `entry_len` mechanism. The 76-byte v2 core is byte-identical; `rideList` (72) and
  `tripList` (76) are untouched.

The USB transport (§10, #889) and the identity read's `obcm_version` byte (§1, E1
#911) are additive on v2 for the same reason each of the above is:

- **USB is a second transport, not a second protocol** — it re-binds §3's GATT
  routing to a leading selector byte and carries §4's bytes unchanged, so nothing
  in this document's object model moved.
- **`protocolVersion` read 6 → 7 bytes** (§1): a trailing `obcm_version u8` on a
  read that was already decoded by length. Bytes 0–5 keep their meaning and their
  offsets, absent trailing fields have defined "unknown" behaviour on both sides,
  and a bump would stop two peers that remain fully interoperable — the argument
  is written out in §1. Re-cuts the `version-read.bin` fixture and adds
  `version-read-noobcm.bin`.

Their iOS mirror repin — `setClock`/`setRouteRetention` sent at the documented
times, the 84-byte `routeList` entry decoded by `entry_len`, and the
`command-set-clock.bin` / `command-set-route-retention.bin` / regenerated
`route-list.bin` fixtures pinned — **landed in S6 (#646)**, the epic's iOS
transport sub-issue. The iOS `routeList` decoder is `entry_len`-driven (it reads
the 76-byte core it knows and fills the expiry tail when the entry carries it), so
a pre-expiry 76-byte device and an 84-byte device both decode.

## 10. Transport binding — USB (issue #889)

Everything above is written against BLE because BLE came first, but only §2
(advertising), §3 (the GATT table), §5 (the CoC) and §8 (pairing) are actually
*about* the radio. The object model, the descriptors (§4.2), the status envelope
(§4.3), the commands (§4.4), the object layouts (§7) and the CRC (§6) are
transport-free, and `specs/vectors/` pins them for **every** transport.

The nRF54LM20 exposes the same contract over USB, as a **second transport, not a
second protocol**. One vendor-specific interface (class `0xFF`), four bulk
endpoints, all at the high-speed-mandated 512 bytes:

| Endpoint | Replaces | Carries |
| :-- | :-- | :-- |
| `0x81` / `0x01` | the GATT control plane (§3.3) | one control frame per transfer |
| `0x82` / `0x02` | the L2CAP CoC (§5) | the unframed object stream, byte for byte |

**The bulk plane needs no translation at all.** Principle #2 holds for a USB bulk
endpoint exactly as it holds for a CoC — reliable, ordered, unframed — so §4.2's
"the channel carries exactly the object's payload bytes, one whole-object CRC-32
at commit" is unchanged, as are §4.1's one-transfer-at-a-time rule and principle
#4's restart-don't-resume.

**The control plane needs exactly one byte.** GATT carries "which
characteristic" in the transport; USB has one endpoint pair, so that routing
becomes a leading **`selector u8`**, and the rest of the frame is *the exact
bytes the corresponding characteristic carries*. One frame is one USB transfer,
and a frame must be strictly shorter than the endpoint's max packet (a frame
exactly filling a packet would need a ZLP to be delimited).

| Selector | Direction | Payload |
| --: | :-- | :-- |
| 1 | host → device | `command` (§4.4) |
| 2 | host → device | `transferControl` (§4.2), the 12-byte descriptor |
| 3 | host → device | `config` write (§7.3) |
| 4 | host → device | identity read (§1) — no payload |
| 5 | host → device | device-information read (§3.1) — no payload |
| 6 | host → device | `config` read (§7.3) — no payload |
| 1 | device → host | `status` (§4.3), verbatim, discriminator included |
| 2 | device → host | the §1 identity bytes (7 with a store, 2 without — the same length-driven read, verbatim) |
| 3 | device → host | device information: `len u8 · UTF-8` ×3, firmware · hardware · serial |
| 4 | device → host | the §7.3 config blob |

Device → host selector 1 is the **sole unsolicited channel**, exactly as the
`status` CCCD is on BLE: one ordering domain for every device → host edge,
including a download's `downloadAnnounce`.

**Every §4.4 command is reachable over USB, `ackRides` included**, and not as a
per-command decision: selector 1 carries the `command` bytes into the *same*
transport-free handler the GATT write reaches, so the command set is whatever §4.4
says it is on both wires. What differs is not the plumbing but the *policy* — who is
allowed to ack, and when — which is written down with the command (§4.4, "What
`synced` means"): a USB peer acks only after its own durable write, and the browser
never acks at all.

Two device-side rules the host may rely on:

- A download whose `total_len` is an exact multiple of the endpoint's max packet
  is followed by a **zero-length packet**, so an object's end is always marked by
  a short packet or a ZLP. A host reading one max packet per transfer never needs
  this; one reading several does.
- The one-transfer gate (§4.1) is **shared across transports**, because the
  resource it arbitrates is the device's single upload temp and open download
  source, not the wire. A transfer in flight on BLE answers a USB
  `transferControl` with `busy`, and vice versa.

Security differs, and deliberately: §8's pairing/encryption gate is a BLE
mechanism with no USB analogue. Physical possession of the cable is the USB
plane's authentication, which is the same posture every other wired peripheral
takes. `forgetBond` over USB still clears the *radio's* bond — it is a device
command, not a transport one.

## Reference implementation

Firmware: the `obc-ble` workspace crate (descriptor codec + transfer state
machine, lands with A5) and `obc-route` (OBCR v3). App:
`companion-ios/Packages/OBCKit` (`OBCTransport/Transfer`, `Codecs/`,
`BLE/GATT.swift`). Shared fixtures: [`specs/vectors/`](vectors/) —
routes with/without waypoints, a ride, a config blob, the route list, and
transfer-descriptor transcripts, asserted byte-exact from both languages.
