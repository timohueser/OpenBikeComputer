# OBC BLE Interface Specification (legacy wire v2)

> **Superseded for Device Object System v2.** The normative replacement is
> [`Device_Object_System_v2.md`](Device_Object_System_v2.md), using wire major 3. This document
> remains the authority only for the temporary legacy implementation during the coordinated
> cutover. Shipping DOS v2 peers do not translate or serve these descriptors.

The legacy wire contract between the OpenBikeComputer device (nRF54L
firmware, BLE peripheral) and the companion app (iOS, BLE central): advertising,
the GATT control plane, the L2CAP CoC data plane, and the byte layout of every
object that crosses the link. It sits next to [`OBCM_Spec.md`](OBCM_Spec.md)
(map format) and [`OBCR_Spec.md`](OBCR_Spec.md) (route format) and is the
historical source the firmware Track-A issues (epic #267) implemented.

> **Protocol v2** (epic #632) is the one coordinated wire break over v1: it
> **removes** the `objectStore` digest and reserved `diagnostics` characteristics
> and the descriptor's permanently-zero `offset`; folds the download announce into
> the `status` envelope (so `transferControl` is **write-only**); widens the
> `protocolVersion` read to carry a **store epoch**; and grows `routeList` entries
> (+content CRC) and the shared list header (+`total`). v1 is not served in
> parallel — a v1 peer reads `version = 2` first and surfaces its mismatch path
> (§1). The one-line "what changed and why" for each item lives in its section.

> **This document is canonical for legacy wire v2 only.** The iOS implementation notes
> ([`companion-ios/OBCProtocol.md`](../companion-ios/OBCProtocol.md)) defer to it:
> where they disagree, this spec wins and the notes are corrected. §9 lists the
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
> radio-specific. **§11 (Weather Request) straddles that line on purpose**: its
> lifecycle — a second advertised service, one authenticated read, a disconnect —
> is radio, while the bundle it produces is an ordinary §4 object type carrying no
> transport assumption.

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
   announces the transfer and the CoC carries exactly the object's payload
   bytes. Uploads normally verify the descriptor's whole-object CRC-32 while
   streaming straight to storage — no reassembly buffer. §6 defines the one
   transport-specific exception for USB-only map objects.
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

**v2 widened the read** from a bare `u16` to `version u16 · store_epoch u32`,
**E1 (#911) appended `obcm_version u8`**, and **WX3 (#1188) appended
`feature_bits u32`** — eleven little-endian bytes:

```
protocolVersion read (11 bytes, little-endian):
  version       u16   the protocol version (2)
  store_epoch   u32   the device's current store-epoch nonce
  obcm_version  u8    the OBCM map-format version this firmware's reader reads
  feature_bits  u32   the optional capability word — bit 0 = Weather Request (§11)
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
onto a fixed shape. Four lengths are defined:

| Bytes | Served by | Decodes to |
| --: | :-- | :-- |
| 11 | a device with a mounted store, since WX3 | version + epoch + map-format version + capability word |
| 7 | a firmware predating `feature_bits` (pre-#1188) | as above, `feature_bits` **absent** |
| 6 | a firmware predating `obcm_version` | version + epoch, both trailing fields **absent** |
| 2 | a device with **no mounted store** | version only, `store_epoch` **absent** |

A reader takes each field on "did at least this many bytes arrive", and **ignores
bytes past the fields it knows** — so a future trailing field breaks no shipped
peer. A field that did not arrive decodes to *absent* (`nil` / `None` / `null`),
**never to a fabricated default**: `store_epoch = 0` names a legal id era the
device never claimed, `obcm_version = 0` reads as "this device supports OBCM
v0" and would refuse every real map, and `feature_bits = 0` would record that a
device *told us* it has no weather when it never said anything at all. Absent
means *unknown*, and unknown has its own defined behaviour in every case (ack
fail-closed below; §6(c)'s no-known-target-firmware branch for the map version;
no weather capability for the capability word, §11.7).

A **partially delivered** trailing field decodes as absent, not as the bytes that
arrived: a read of 8, 9 or 10 bytes is a broken read of a `u32`, not a smaller
capability set, and treating it as data could claim a feature the device never
announced. Only a whole field counts as having arrived.

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

**`feature_bits` did not bump it either, for exactly that argument** (WX3, #1188).
Bytes 0–6 keep their meaning and their offsets, the new field is bytes 7–10, and
both directions of the mismatch are defined and harmless: an old app reads eleven
bytes, takes the seven it understands and loses nothing, while a new app reading
seven gets *absent* and offers no weather — which is precisely what an old
firmware is. Bumping would stop a pair that is fully interoperable in order to
announce a field that is allowed to be missing.

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

**Capability bits (`feature_bits`).** A `u32` of independent capability flags, of
which WX3 allocates exactly one: **bit `0` = the Weather Request contract (§11)**.
The word exists because weather is the first thing the app must decide *whether to
offer at all* before it does anything — the phone's whole weather lifecycle
(background scanning for a second service UUID, an HTTP fetch, an upload) is
wasted work against a device that cannot receive the answer, and there is nothing
else in this read from which the answer could be inferred. **Unknown bits are
ignored**, never rejected: that is how a later firmware announces something this
build was never going to act on. The word obeys the same positional rule the rest
of the read does — it can only be served when `obcm_version` is, since a `u32` at
byte 7 cannot be reached without inventing byte 6 — so a device with a capability
to announce but no map-format version to announce it beside serves the 6-byte
form rather than fabricating "supports OBCM v0".

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
- **One advertised UUID at a time — the weather swap** (WX3, #1188). A legacy
  advertisement cannot comfortably carry two 128-bit UUIDs alongside the flags,
  so while a weather refresh is due the device **swaps** the advertised UUID from
  OBC Control to **Weather Request** (§3.5) rather than listing both. Everything
  else about the advertisement is unchanged: same address, same intervals, same
  connectability. **Both services exist in GATT at all times**, whichever UUID is
  on air — advertising a service the connected database does not contain is the
  one thing this must never do, because a central that matched on it would connect
  and find nothing. The app therefore scans for **both** UUIDs and treats either
  as "the device is here"; the swapped UUID is what lets iOS wake the app in the
  background for a request it did not ask for.
- **The weather advertising window is a monotonic deadline, not a restartable
  timer.** The hint is lowered when — and only when — an authenticated read of
  `weatherRequestContext` has actually been served (§11.3), and it expires on a
  bounded budget measured from the moment the request was raised (the reference
  firmware: **60 s**). It is *not* restarted per connection: a stray central that
  connects and drops repeatedly would otherwise extend a bounded hint into a
  permanent secondary beacon and a battery bug. When the budget expires the device
  returns to advertising OBC Control; the request itself stays pending on its
  retry ladder (§11.3).
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

Four services. The SIG services are open (readable before pairing); the OBC
Control service is encrypted once bonding lands (§8), as is the one
characteristic of the Weather Request service (§3.5, §11).

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
| `0008` | `protocolVersion` | read | `version u16 · store_epoch u32 · obcm_version u8 · feature_bits u32` — §1, decoded **by length** (11 / 7 / 6 / 2 bytes). Readable **without** encryption |

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

### 3.5 Weather Request service (custom, WX3 #1188)

The secondary service the device advertises while a weather refresh is due (§2),
and the one small authenticated read the companion performs before it disconnects
again. The full contract — the exchange, the context layout, the bundle upload —
is §11; this is the GATT entry.

Its own **random 128-bit base**, deliberately *not* a block inside the OBC Control
base: iOS matches the advertisement on this UUID alone, so the two services have
to be independently advertisable, and a `3C92xxxx` block would have made one a
sub-range of the other on air.

| `XXXX` | Entity | Properties | Role |
|---|---|---|---|
| `0000` | **Weather Request service** | — | primary service |
| `0001` | `weatherRequestContext` | read | the 52-byte request context (§11.4). **Authenticated** — the value says where the rider is |

```
service                B3B60000-33B4-4F02-A5FF-E5954D54B5AA
weatherRequestContext  B3B60001-33B4-4F02-A5FF-E5954D54B5AA
```

This base has **never shipped in a released firmware**, so `0001` is a first
assignment and not a reuse; **no block of it is retired** — unlike `3C920003` /
`3C920006` (§3.3), there is nothing here to retire yet. Blocks of this base MUST
NOT be reused for a different entity once assigned, on the same rule §3.3 states
for the Control base.

The service is present in the GATT database **at all times**, not only while a
request is due (§2). Only what is *advertised* changes.

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
| `17` | `mapShard` | host → device (upload) | one OBCM shard of a volume set ([`OBCA_Spec.md` §5.1](OBCA_Spec.md)) — **USB only** |
| `18` | `mapSet` | host → device (upload) | the OBCS set manifest ([`OBCA_Spec.md` §5.2](OBCA_Spec.md)) — **USB only** |
| `19` | `terrainShard` | host → device (upload) | the set's OBCT terrain shard ([`OBCA_Spec.md` §5.1](OBCA_Spec.md)'s `terrain` role) — **USB only** |
| `20` | `weatherBundle` | app → device (upload) | one OBCW v1 weather bundle ([`OBCW_Spec.md`](OBCW_Spec.md)), singleton at `object_id = 0` — §11.5 |

`map` is the one type BLE could never have carried: a map is hundreds of
megabytes, so the type would have been dead weight until a USB bulk endpoint
existed (#889). It sits at `16` rather than at the next free number because
`11`–`15` are already spoken for; the byte is a `u8` and there is no reason to
crowd a reserved band. Like `fwImage`, the transfer layer is **format-blind** —
the payload is opaque bytes.

`mapShard` and `mapSet` (#1039) are the same argument at a larger scale, so they
join `map` in the USB-only band without a new one: a DACH-shaped **volume set**
is 7.6–8.9 GiB across ~8 files ([`OBCA_Spec.md` §5.1](OBCA_Spec.md)).
`terrainShard` (#1044) is the fourth: it is one more file of that same set. A
device MUST answer any of the four with `error` on the radio.

`weatherBundle` (`20`, WX3 #1188) continues that numbering thread rather than
quietly breaking it: `11`–`15` are still the sensors' (M4), `16`–`19` are now the
band the USB transport opened, so `20` is simply the next free value and nothing
is crowded — the byte is a `u8` and the free space above `20` is untouched.
(#1188's issue text
said `11`; the epic's handover comment on #1185 supersedes it for exactly the
reason `map` did not take `11` either.) What it does **not** inherit from its
neighbours is the reason they sit up there: a bundle is **~46 KiB** — a couple of
seconds on the CoC — so it is the first type since #889 that is neither USB-only
nor map-shaped. It rides the ordinary invisible-temp upload path, the ordinary
whole-object CRC-32 and the ordinary `transferResult`; **none of the five map
rules below apply to it**, and a device MUST NOT answer it with `error` on the
radio. That affordability is the whole reason the intermittent weather lifecycle
(§11) works at all. Its own two rules — singleton `object_id = 0`, and what
happens to a duplicate or stale bundle — are §11.5 and §11.6.

A map upload carries **four rules the other upload types do not** (#927). All of
them follow from one fact: a map is hundreds of megabytes, which makes it the
only object whose transfer is measured in minutes rather than frames.

1. **New-only.** `object_id` MUST be `0xFFFF`. A named id is answered
   `notFound` — for a map there is no id an upload may target. Replacing a map in
   place would mean destroying the stored bytes as the new ones arrive, which
   forfeits the "a failed upload never touches the old copy" guarantee below on the
   one object a device cannot rebuild for itself. Replacing a map is *upload the
   new one; the device retires the old one itself* — see rule 5. A host has no
   verb for it: `deleteObject` (§4.4) takes routes and trips, and no map
   enumeration crosses the wire at all.
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
   magic only after the upload's §6 integrity policy *and* the header have validated. An
   interrupted transfer therefore leaves a zero-magic file that every reader
   refuses and a boot sweep reclaims — the same durability the other types get
   from an invisible temp, reached without the copy.
5. **One uploaded map.** A device that loads a single map SHOULD retire the
   uploads that map supersedes, rather than accumulate copies no reader will ever
   open. In the reference firmware this happens **at the same boot that selects
   the new map**, and only once the selected map has opened: an upload commits
   while its predecessor is held open for the session, so the moment of the commit
   is precisely when the old file cannot be touched. Two consequences worth
   stating, because a host cannot see either: between the upload and the next
   restart the card carries **both** maps, so rule 2's guard can refuse a
   replacement that does not fit alongside the copy it is about to replace; and a
   map the rider placed on the card themselves is **never** retired — it carries
   no device-assigned id, and the rule is one *uploaded* map, not one file.

### Volume sets: several transfers, one map (#1039)

A **volume set** ([`OBCA_Spec.md` §5](OBCA_Spec.md)) is one logical map spread
over OBCM shards, optionally one OBCT **terrain shard**, and an OBCS manifest, so
it is the first object on this link whose correctness lives *between* transfers.
Each file is an ordinary upload — its own descriptor (including the real
whole-object CRC-32), its own commit — and the five map rules above apply to each
of them unchanged. Eight rules govern the sequence, and they are **normative**:

1. **The `object_id` of a `mapShard` is not an object id.** It carries the set's
   `Shard Count` in the **high** byte and this shard's `index` in the **low**
   one: `object_id = (shard_count << 8) | index`, with `1 ≤ shard_count ≤ 32` and
   `index < shard_count`. A shard has no durable id to target — §5.2 *derives*
   every filename from the set id and the index, and §5.4 makes the whole set one
   map with one identity — so the field is repurposed rather than the descriptor
   widened. A device MUST answer a pair outside those ranges with `notFound`.
   Restating `shard_count` in **every** shard rather than only the first buys two
   things, and it is worth being exact about the second:
   - a device can refuse an over-large set at the **first** announce (rule 4)
     rather than after the whole upload;
   - a `mapShard` whose `shard_count` **differs** from the set already in flight
     MUST be refused with `error`, so a host that starts sending a
     differently-shaped set mid-transfer is named rather than merged.

   What the announce **cannot** see is a switch between two sets with the *same*
   shard count: the pair names a file, not a set, and no field in the descriptor
   identifies which set a shard belongs to. That case is caught at the manifest's
   commit instead — rule 7 — which is later but is still before anything is a
   map. Closing it at the announce would need a set identifier on the wire, i.e.
   a descriptor change; it is left open deliberately, because §5.3 already
   obliges a host to have proven its own set before offering a byte of it, and
   the failure mode of a host that has not is an unmountable set rather than a
   damaged one.
2. **The manifest is new-only and last, and the device enforces it.** A `mapSet`
   upload sends `object_id = 0xFFFF`; a named id is answered `notFound`. A
   `mapSet` announced when **any** shard of the set in flight has not yet
   committed — or when no set is in flight at all — MUST be refused with `error`
   **before any byte streams**. §5.4 addresses the writer; this is the receiver's
   half of the same sentence, and it exists because a device cannot hold a host
   to a MUST it merely read. An announced `total_len` that is not
   `72 + 64 × Shard Count` is likewise refused at the descriptor, where
   `Shard Count` is the manifest's own field and therefore counts **every**
   record — see rule 8, which is where that word did real damage.

   The manifest a `mapSet` carries on this transport is **unbound**
   ([`OBCA_Spec.md` §5.2](OBCA_Spec.md)): every member `ObjectId` is `0`. This
   protocol writes files to a FAT card, which has no object identities to mint,
   and a device receiving a set here resolves its members by the derived §5.2
   filenames exactly as it did before v3. Binding belongs to the flat store's own
   object protocol, where the device answers each member's commit with the id it
   assigned.
3. **The terrain shard is its own type, and it precedes the manifest** (#1044).
   A set with elevation carries one OBCT raster
   ([`OBCA_Spec.md` §5.1](OBCA_Spec.md)'s `terrain` role, stored as
   `MS<id>.OBD`). It is uploaded as `terrainShard` (`19`), **not** as a
   `mapShard`: a shard's `object_id` is a `(shard_count, index)` pair naming one
   of the OBCM files the manifest's *leading* records describe, and a raster has
   no index, is not an OBCM file, and lands under a different name — sent as a
   shard it would consume an index the manifest never names. The rules:
   - `object_id` MUST be `0xFFFF`; a named id is answered `notFound`. There is at
     most **one** terrain shard per set, so there is nothing for an id to select.
     This refusal is checked **first**, ahead of the session rules below, for the
     same reason rule 1 answers a malformed part before a device's shard ceiling:
     a host that packed the field wrong is told *that*, not something about a set.
   - A `terrainShard` announced with **no set in flight** MUST be refused with
     `error`. The set id is minted by the first `mapShard`, so a raster arriving
     first names no set at all.
   - An announced `total_len` below one OBCT header is answered `error` — map
     rule 3 above, against the raster's format instead of OBCM's.
   - A device that **discards** a raster's transfer (failed validation, a card that
     refused the write) MUST stop counting it toward rule 8's length, because the
     discard removes the file — including one an earlier attempt had committed.
     The host is then refused at the *manifest's announce*, where it costs one
     descriptor and the raster can still be re-sent, rather than at the commit
     that would delete the whole set.
   - A host MUST send it after every `mapShard` of the set and **before** the
     `mapSet`. That is not house style: rule 8 makes the manifest's expected
     length depend on whether the raster has arrived, so a device can only be
     right about it if it has already seen the file.
   - A device whose set already holds 32 records MUST refuse it with
     `storageFull` — §5.2 caps a manifest at 32 records, so such a set has no
     room for a terrain one and no legal manifest could be written.
   - A **re-sent** terrain shard is legal and overwrites the file, exactly as a
     re-sent `mapShard` does. It is never a second record.
   - Its `transferResult` echoes the device-assigned **set id**, like the
     manifest's: a raster has no part to correlate against.
   - A set that carries **no** terrain sends no `terrainShard`, and nothing in
     this section changes for it.
4. **A device's own shard ceiling is announced-time, not commit-time.** A device
   that can hold fewer than 32 shards open MUST refuse a set whose declared
   `shard_count` exceeds its ceiling with `storageFull`, at the **first** shard —
   the same "this catalog cannot take another entry" meaning `storageFull`
   already carries for routes and trips. Refusing at the manifest instead would
   cost the rider the whole upload. The reference firmware's ceiling is **11**
   (its FAT handle budget); the format's is 32. A device with no id left to name
   a new set answers the same `storageFull` at the same moment, for the same
   reason: it is a catalog refusal, not a storage failure discovered mid-write.
5. **`transferResult` correlation.** A shard's result echoes its **part**
   (`object_id` = the same packed pair), because that is what a host correlates
   its transfer slot against and what says *which* file committed. A host MUST
   check it: a result naming a different part is not "this file committed", and
   continuing past it would write a manifest over a set the two sides disagree
   about. The **manifest's** result carries the **device-assigned set id** — the
   one moment a set's identity crosses the wire, and the answer to "what did my
   upload become".
6. **A staged set can be abandoned, and an `op=3` naming `mapSet` is how.** A set
   spans several descriptors, so an abort most often arrives when *nothing is in
   flight* — in the gap between two of them. A device MUST treat an `op=3` whose
   descriptor names **`mapSet`** as abandoning the set in flight, not merely as a
   confirmed no-op: the session closes and every file of the set is deleted,
   exactly as a dropped link would. Without it a host that cancelled would be
   told `aborted` while gigabytes stayed staged and every differently-shaped set
   went on being refused (rule 1) until the transport was torn down. The answer
   is `aborted`, as it already is.

   **The type is load-bearing, because an idle `op=3` has a second meaning.** A
   host also sends one to *quiesce* the byte pipe after an exchange the device
   had already closed — a reject it noticed late, a rider's cancel, a shard the
   device refused on CRC — and in that case it is about to **retry** (§4.2's
   idle-abort drain). A device MUST NOT abandon the set for such an abort: a
   descriptor naming `mapShard`, `terrainShard` (or any type other than
   `mapSet`) is a quiesce, and MUST leave the session and every staged file
   untouched — including the terrain band, which rule 3 lets a set carry or
   omit. A host MUST likewise name `mapSet` only when it means abandonment.

   Conflating the two is not a corner case, it is a map-shaped hole: a failed
   shard drops only itself (**Resume**, below), so a host that re-sends that one
   shard is doing exactly what this spec tells it to — and if the quiesce that
   preceded the retry deleted the set, the re-sent shard lands in nothing and the
   manifest seals a set with no files.
7. **A shard after a committed manifest begins a new set.** Once a `mapSet` has
   committed, the set it completed is closed: a later `mapShard` MUST NOT be
   added to it, because its manifest names exactly the files it names and any
   addition would make that manifest false. Such a shard opens a **new** set,
   with a new device-assigned id, and is staged and reclaimed like any other.
   This is also where a same-count set switch (rule 1) is caught: at the
   manifest's commit a device re-checks every shard against the manifest's own
   record of it, and MUST refuse a manifest that does not describe the files
   beside it — deleting the whole set rather than leaving it half-present.

   **The terrain record is checked here and only here.** A device MUST refuse a
   just-uploaded manifest whose `terrain` record does not match the raster on the
   card (absent, a different length, or not a readable OBCT). That is *not* in
   tension with [`OBCA_Spec.md` §5.3](OBCA_Spec.md)'s rule that a missing or
   unreadable terrain shard MUST NOT fail a **mount** — the two are different
   moments and must stay different. At mount the device is judging a card that has
   aged: a rider deleted the `.OBD` to reclaim space, a hand copy was truncated, a
   read glitched, a later OBCT version arrived. None of that makes the map a lie,
   and §5.3 requires it to mount flat. At commit the host built the manifest and
   the raster together seconds ago and was told the exact length to announce, so a
   disagreement is the two ends contradicting each other about this very transfer.
   A device MUST NOT let a stored set's terrain record affect whether that set
   lists or mounts.
8. **`Shard Count` counts every record, and the manifest's announced length
   follows from that** (#1044). Rule 2's `72 + 64 × Shard Count` uses the
   manifest's own field, and [`OBCA_Spec.md` §5.2](OBCA_Spec.md) is explicit
   that the field counts **every** record — the `terrain` one included. So the
   length a device expects at the `mapSet` announce is

   ```text
   72 + 64 × (mapShards committed + terrainShards committed)
   ```

   …computed from what **this upload session actually received**, not from the
   `shard_count` the descriptors carried. A device MUST compute it that way, and
   a host MUST have sent the terrain shard (rule 3) before the manifest that
   names it.

   The reason this is a numbered rule rather than a clause is that it was a real
   and expensive bug. A host that built a terrain-bearing manifest and skipped
   the raster announced one record more than a device counting only OBCM shards
   could expect; the manifest was refused with `error` at the **last** transfer
   of a multi-gigabyte upload, every shard already on the card was swept at the
   next boot, and — because an announce-time refusal never reaches the glass —
   the device sat on the previous shard's "Map installed" card while the host
   reported failure. The two ends must derive the number the same way or a set
   is lost after all of it has moved.

**Atomicity, and what an interrupted set leaves.** A device MUST NOT let a
half-received set be mountable, which §5.4 already guarantees (no manifest ⇒ no
map). What this section adds is the *cleanup* obligation: a device SHOULD
reclaim a set whose upload it abandoned, and MUST NOT delete files it cannot
prove are its own. The reference firmware does both with one mechanism — it
creates `MS{id}.OBS` holding four zero bytes *before the first shard streams* and
patches the `OBCS` magic in as the very last write of the set. A manifest whose
magic is **not whole** is therefore its own torn upload: all zeros (the
placeholder), a strict prefix of `OBCS` (the commit's four-byte write split by a
power cut), or shorter than four bytes at all (the token's create without its
write). A set arriving over a card reader is copied from a host that already
holds a finished manifest and writes it front to back, so its `.OBS` carries the
whole magic from its first block — the shapes above are, to within one block of a
copy that was itself interrupted, unreachable any other way, and a manifest in
one of them cannot be read as a map by anyone regardless. A dropped link, or an `op=3` abort **naming
`mapSet`** (rule 6), deletes the whole set immediately; a power cut is reclaimed
by the boot sweep. An `op=3` naming anything else is a quiesce and leaves the set
alone. Complete shard files with no manifest at all are left alone: §5.4
makes deleting orphans a MAY, and that shape is a rider mid-copy.

A device MUST NOT let a set's *id allocation* undo that care. Whatever scheme
assigns the `{id}` of the derived filenames, an id must be treated as taken while
**any** file naming it is on the card — a manifest-less pile of shards included,
since that is precisely the mid-copy shape the paragraph above protects. Minting
such an id and then clearing it (§5.4's replace rule) would delete, at the next
upload, the map the sweep deliberately spared.

**Resume.** Per **file**, and free: shards are independent files, so a shard
whose validation failed is re-sent on its own while the rest stand. This is the property
rule 6's `mapSet`-only abandonment exists to protect — a re-send is preceded by
the idle-abort quiesce (§4.2), and that abort must not take the set with it. Across a
**disconnect**, no — the set is gone, and resuming would need a device → host
query for "which shards of which set do you hold", which is a new §4.4 command
rather than a change to this section.

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
  object atomically (commit-then-swap; a failed upload never touches the old copy).
  **`map` is the exception**: it is new-only, because atomic replacement of a
  several-hundred-megabyte object is not something a device can offer — see the
  map rules above.
- Objects that exist once (`routeList`, `rideList`, `tripList`, `diagnostics`,
  `echo`, the `fwImage` staging slot, and `weatherBundle`) use object id `0`. A
  `fwImage` upload is a singleton stage: the app sends object id `0`, the device
  assigns no id and the `transferResult` echoes `0` (§7.6). `weatherBundle` is
  the same shape and is **not** new-only like `map`: there is exactly one weather
  bundle and an upload is *always* a replacement, so any id other than `0` is
  answered `notFound` (§11.5).
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
`total_len` + `crc32`, then streams the whole object over the active bulk channel
as raw bytes. The device sinks them to storage and applies §6's integrity policy.
When `total_len` bytes have arrived it notifies a `transferResult` (§4.3):
`committed` after the required checks pass; a checked CRC mismatch **rejects** the
object (`crcMismatch`) and never commits it. Uploads are **not resumable** (§1
principle 4): an interrupted upload (a dropped link or an `op=3` abort) is
discarded, and the app re-sends the object from the start.

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

**Abort with nothing in flight — the quiesce, and the one place a device may
empty its byte pipe.** An `op=3` also arrives when the device has *already*
closed the exchange: a descriptor-open reject the host noticed late, a cancel
that raced the device's verdict, or an integrity check the device refused. The host
is not confused — it is about to retry, and it needs the channel empty first.
This matters on an unframed, unacknowledged pipe where the sender does not wait
between chunks: bytes it queued for the abandoned exchange are still arriving,
neither end can recall them, and if the retry's descriptor arms while they land
they become that object's opening payload and fail its validation.

A device on a transport whose pipe it can read (a USB bulk endpoint) SHOULD
therefore **read and discard until the pipe is quiet, and only then answer**
`aborted`. It MUST bound that drain and answer regardless when the bound is hit.
A transport that closes and reopens its channel around a failed exchange (BLE's
CoC) has nothing to drain and answers immediately.

**Draining is an explicit act, at a termination the host already knows about.**
There are two such moments, and the difference between them is only whether the
answer has gone out yet:

- The **abort handshake**, *before* the answer — with or without a transfer in
  flight, since either way the host has stopped and is waiting for `aborted`
  before it does anything else.
- A **device-originated termination that left announced bytes unread**, *after*
  the answer: a refused announce, a storage failure mid-object. Here the host has
  not been told yet and refills as fast as a device could discard, so a device
  MUST NOT drain first — the terminal `transferResult` is what makes it stop, and
  queuing that behind a drain only delays it. Once the answer is out, a device on
  a readable pipe SHOULD empty it, bounded as above.

That second case is an obligation rather than tidying, because of the rule
below: with nothing armed the pipe is not being read at all, so the sender's
already-submitted writes are held by transport flow control and never settle. A
device that answers a refusal and then leaves the pipe full does not fail that
sender's upload — it stops it, in front of the very abort it would otherwise
send.

A transfer that consumed its full announced length leaves nothing behind, so a
commit refusal or a failed final flush needs no drain of its own.

There is a third moment, and it belongs to the transport rather than to the
protocol: **immediately after the channel is (re-)established**, before anything
can be armed on it. A device whose endpoint hardware stages a received packet
may still be holding one from the session that ended — a cable pulled mid-write
does not clear it — and it would otherwise become the *next* session's opening
bytes. A device SHOULD sweep there, and it is unambiguously safe: the peer
cannot have opened an exchange on a channel that has only just come up.

**Bytes for an announce that has not been accepted are not consumed.** A device
MUST NOT read and discard payload bytes while no transfer is armed. Senders are
permitted to pipeline — §4.2 has no upload-accepted handshake, so an upload's
first bytes may be on the wire before its descriptor has been classified — and a
device that eats them has silently destroyed the payload of a transfer it is
about to arm. There is no ordering a device can rely on to make that safe: its
own classification may be delayed behind unrelated work, and an object shorter
than that delay is lost in full rather than in part. Unclaimed bytes MUST instead
be left to the transport's flow control (a bulk endpoint NAKs; a CoC withholds
credit), where they wait until a transfer arms and reads them or until one of the
two drains above discards them.

Such an abort is a **pure quiesce**: apart from discarding an in-flight partial
it MUST change no stored state — see §5 rule 6 for the one descriptor that is
also an instruction (`mapSet`).

A descriptor that names an unknown type/id or arrives mid-transfer is answered
with a `transferResult` carrying `error` / `notFound` / `busy` (§4.3) and does not
disturb an active transfer.

**Storage-full reject (descriptor-open).** A **new**-route upload — `op=1`,
route type, `object_id = 0xFFFF` (or a route id the device doesn't hold) —
that would grow the catalog past its cap is rejected at the `transferControl`
write, **before the device consumes payload bytes**, with `transferResult` status
`storageFull` (§4.3); no partial file is created. Because v2 has no separate
upload-accepted handshake, the sender may already have queued raw payload bytes
when that asynchronous result arrives. Recovering from that is transport-shaped:
over BLE the sender resets the CoC as described above, which discards them. Over
a transport with no channel to reopen — a USB bulk endpoint — resetting cannot
un-queue a submitted transfer, so the sender MUST instead complete the idle-abort
handshake above (`op=3`, wait for `aborted`) before it retries, which is what
guarantees the pipe is empty when the retry's descriptor arms. A sender that
skips it is not merely unlucky: nothing clears the leftovers on their own — with
no transfer armed they are not read at all — so the retry inherits them as its
opening payload and fails validation for reasons nothing in the exchange
explains. What keeps such a sender from stopping outright is the device's
post-answer drain above; what keeps it from *converging* is skipping the
handshake, so the retry can fail the same way again.
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
  status            u8   0 = committed     stored + §6 integrity policy passed
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

msg = 5  weatherRequest (1 byte total):
  msg         u8   = 5; authenticated request context is ready to read
```

The **`downloadAnnounce`** (v2) is the device's answer to a download request
(§4.2): the announce moves off `transferControl` and onto this envelope so all
device → app control traffic shares one notify characteristic and one ordering
domain. Unknown `msg` values must be ignored by the app (forward compatibility).
`weatherRequest` is only a live-link hint; the authenticated context read remains the receipt
that consumes the request. A disconnected phone discovers the same request through the dedicated
advertised service UUID.

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
| `7` | `weatherUnchanged` | `request_id u32 · retry_after_s u16` | finish that live weather request after the phone conditionally checked both providers and found no revision newer than the held bundle. `retry_after_s` is `0...3600` and suppresses repeated manual probes only; a mismatched/non-live id answers `notFound`, malformed values answer `error` (§11.1) |
| `8`–`15` | — | — | reserved (identify/find-my-device, factory reset, …) |

**Next free command: `8`.** (`setClock` landed at `5` and `setRouteRetention`
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
request the on-device confirm flow must approve. Image **authenticity** is no longer
out of scope: since OBCU v2 (`OBCU_Spec.md` §1.3, epic #773) a staged container carries
an Ed25519 signature over a domain-separated message, and the device's armer verifies it
— and refuses an *unsigned* container — before an install can be armed
(`OBCU_Spec.md` §1.4). This is orthogonal to the link and identical on every delivery
path: a bonded phone, a USB cable, and a hand-copied card all end at the same scan. The
physical-confirmation gate above is unchanged and still the *authorization* step; the
signature answers a different question (are these bytes ours?) from the CRC-32 (did
they arrive intact?), and both are checked. A peer therefore cannot stage an installable
image it did not obtain from a release; the worst it can do is waste a transfer.

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

The descriptor always carries the real whole-object **CRC-32/IEEE**
(zlib/gzip/PNG): reflected, polynomial `0x04C11DB7` (reflected form
`0xEDB88320`), initial value `0xFFFFFFFF`, final XOR `0xFFFFFFFF`.
Check value: `CRC32("123456789") = 0xCBF43926`.

Receivers normally verify it once at commit. This is deliberately *not* a
per-chunk CRC — the link already covers each packet — and it catches errors
outside that link, end to end from the sender's encoding to the stored object.
It is mandatory for BLE uploads, every download receiver, `route`, `trip`,
`fwImage`, and `echo` on USB, and any future type unless its definition says
otherwise.

The one exception is an upload of the USB-only map-shaped types `map`,
`mapShard`, `terrainShard`, and `mapSet`. A device **MAY** omit the second serial
whole-object calculation for those types while retaining the descriptor and its
real `crc32`. Such a device MUST instead rely on the USB packet CRC/retry, the
storage transport's block CRC/ECC, the exact announced byte count, the
format-specific length/header validation, and the magic-last commit rules in
§4.1. The reference nRF54L firmware uses this policy: a checked failure is
reported as `error`, not `crcMismatch`, and an unreadable result cannot mount;
its no-map boot path keeps USB available so the host can replace it. This is a
receiver policy only — it changes no wire bytes, fixture, or protocol version.

---

## 7. Object layouts

### 7.1 `route` — an OBCR v3 file

A route object's payload is **exactly the bytes of an OBCR v3 file** — see
[`OBCR_Spec.md`](OBCR_Spec.md), including the waypoints section (categorized and
carrying a signed lateral offset since v3). The phone encodes imported GPX/TCX to
OBCR v3 (waypoints included — see the iOS implementation notes); the device writes the
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
  name_len         u16  ≤ 48 (UTF-8 bytes; matches the OBCR route-name cap)
  name             name_len bytes, UTF-8 — THE device name (Delta 1: rename = write
                   Config with a changed name; there is no separate rename command)
  units            u8   0 = metric · 1 = imperial
  weather_refresh  u8   how often the device raises a weather request (WX3, §11.8)
                        0 = Off · 1 = 15 · 2 = 30 · 3 = 60 · 4 = 120 minutes
                        ABSENT on a read  = device default (30 min) — NOT Off
                        ABSENT on a write = leave the stored value untouched
  [future fields append here; readers MUST ignore unknown trailing bytes]
```

The append-only rule is the version mechanism: fields are never reordered or
resized, only appended, and absent trailing fields mean "device default".

**On a *write*, an absent trailing field means "leave the stored value untouched"**
— not "reset it to the default". The two readings coincide on a factory-fresh
device and diverge on every configured one, so the distinction has to be stated
rather than inferred: an old app renaming the device writes the pre-WX3 3-byte
blob, and a device that read that as *the rider chose the default* would reset a
rider who had deliberately chosen `Off` back to 30-minute wakeups — a setting
change they never made, caused by a rename.

**`weather_refresh`** (WX3, #1188) is the first field to land under that rule, and
it is where "absent means device default" stops being a formality. **Absent is the
device default (30 minutes), explicitly not `Off`.** A shipped app that predates
the field renames the device by writing the 3-byte-plus-name blob it has always
written; a device that read that as "the rider chose Off" would silently disable
weather on a rename, and nothing in the app's UI would ever show why. So a blob
that stops after `units` leaves the stored setting untouched.

An **out-of-range** refresh byte is a different thing again, and it is handled
**by direction** (§11.8, which is normative for the rule). On a **write** it is
malformed and the whole write is rejected (an ATT error, as for any malformed
Config), not quietly defaulted: absent means the writer never mentioned refresh,
whereas a value of `9` means it asked for an interval this build cannot honour,
and storing 30 minutes for it would tell the rider their choice was applied when
it was discarded. On a **read** it is *not* an error — it is a newer device naming
an interval this reader predates, so the reader reports it as unknown (neither
`Off` nor the default) and keeps the rest of the blob. Rejecting the read instead
would mean a future fifth interval stopped a shipped app from so much as renaming
its device. The same enum crosses the wire in the request context (§11.4), under
the same rule, so the two never drift.

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
| `weatherRequestContext` (§3.5) | encrypted, LESC-authenticated link — it says where the rider is |
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

Protocol v2 (epic #632) is the one coordinated wire break. The iOS implementation
notes ([`companion-ios/OBCProtocol.md`](../companion-ios/OBCProtocol.md)) point back
to this list; each item below is a single-spot repin on both sides,
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
- **The volume-set types** `mapShard` (`17`), `mapSet` (`18`, §4.1, #1039) and
  `terrainShard` (`19`, #1044) are additive in the same sense `map` was, and for
  a stronger version of the same reason: a set is *larger* than the map BLE could
  not carry. Three `ObjectType` values out of the 237 still free, no descriptor
  change (a shard's part rides the existing `object_id`; a raster is new-only), no
  new status, no new command. A peer that does not know them simply never sends
  them — a host that never sends `terrainShard` simply assembles maps with no
  raster, which is a complete map with flat profiles
  ([`OBCC_Spec.md` §13](OBCC_Spec.md)).
- **`protocolVersion` read 6 → 7 bytes** (§1): a trailing `obcm_version u8` on a
  read that was already decoded by length. Bytes 0–5 keep their meaning and their
  offsets, absent trailing fields have defined "unknown" behaviour on both sides,
  and a bump would stop two peers that remain fully interoperable — the argument
  is written out in §1. Re-cuts the `version-read.bin` fixture and adds
  `version-read-noobcm.bin`.

**The Weather Request contract** (§11, WX3 #1188) is additive on v2 in the same
sense, and for the same reasons — four changes, none of which moves an existing
byte:

- **`protocolVersion` read 7 → 11 bytes** (§1, §3.3): a trailing `feature_bits
  u32`, bit `0` = weather. Bytes 0–6 keep their meaning and their offsets; a
  partial word (8–10 bytes) decodes as absent, and absent is exactly "this device
  has no weather", so the old-client path needs no special case. The two
  acceptance directions — an old app against the 11-byte read, a new app against
  the 7- and 6-byte reads — are the shape of the compatibility promise.
- **A new service** (§3.5): `B3B60000-…`, one authenticated `weatherRequestContext`
  read at `B3B60001-…`. A base of its own, never shipped before, nothing retired.
  Advertising **swaps** the advertised UUID while a request is due (§2) — both
  services are always in GATT.
- **Object type `20` `weatherBundle`** (§4.1, §11.5): one more `ObjectType` value
  from the free space above the USB band, singleton at `object_id = 0`, on the ordinary CoC upload
  path with the ordinary whole-object CRC and `transferResult`. No descriptor
  change, no new status, no new command. A peer that does not know it never sends
  it.
- **Config grows a trailing `weather_refresh u8`** (§7.3, §11.8) under the
  existing append-only rule — absent = *device default* (30 min), **not** `Off`,
  which is what keeps an old app's rename from disabling weather. The blob a
  writer produces with no refresh field is byte-identical to the pre-WX3 one,
  which is what keeps the existing Config fixture meaningful.

The iOS implementation handles those four on the codec side: the widened identity read
decoded by length, the second service scanned for and read, the type-`20` upload,
and the Config field written only when the rider set one. The shared
`specs/vectors/weather-request-*.bin` fixtures pin the context layout from both
languages.

The corresponding iOS implementation — `setClock`/`setRouteRetention` sent at the documented
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
endpoint exactly as it holds for a CoC — reliable, ordered, unframed — so the
channel carries exactly the object's payload bytes and the descriptor retains
its whole-object CRC-32. §6 permits a receiver to omit that redundant calculation
for the four USB-only map-shaped upload types; this changes policy, not the byte
stream. §4.1's one-transfer-at-a-time rule and principle #4's
restart-don't-resume are unchanged.

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
| 7 | host → device | mounted-card free-space read — no payload (USB only) |
| 1 | device → host | `status` (§4.3), verbatim, discriminator included |
| 2 | device → host | the §1 identity bytes (11 / 7 / 6 with a store, 2 without — the same length-driven read, verbatim) |
| 3 | device → host | device information: `len u8 · UTF-8` ×3, firmware · hardware · serial |
| 4 | device → host | the §7.3 config blob |
| 5 | device → host | mounted-card free bytes as little-endian `u64`; empty when no readable card is mounted |

Device → host selector 1 is the **sole unsolicited channel**, exactly as the
`status` CCCD is on BLE: one ordering domain for every device → host edge,
including a download's `downloadAnnounce`.

The free-space read is deliberately an envelope query rather than a new object
or command. It is USB-only UI telemetry, has no persistent effect, and is
answered from FAT32's cached FSInfo count (a bounded three-sector read, never a
FAT walk). A host uses it immediately before a map-set send; an empty reply is
"space unavailable", not zero bytes free.

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

## 11. Weather Request (WX3, issue #1188)

The device cannot fetch a forecast: it has no IP stack, and nothing in its power
budget would pay for one. The phone can, and already carries the network the
rider is paying for. This section is the contract by which a **disconnected**
device asks for a forecast and a **backgrounded** app answers — without either
side holding a BLE link across the HTTP that does the real work.

Everything here is **additive on protocol v2** (§1, §9): a new service, one new
object type, a trailing field on the identity read and another on Config. A peer
that knows none of it behaves exactly as it did before.

### 11.1 The exchange

1. A refresh comes due (§11.8) or the rider opens Weather. The device raises a
   request, fills the context attribute, and **swaps its advertised service UUID**
   from OBC Control to Weather Request (§2, §3.5).
2. The phone — scanning for both UUIDs, in the background — wakes on the match and
   connects.
3. It reads **one** `weatherRequestContext` (§11.4): where the rider is, where
   they are heading, and which bundle they already hold. Then it disconnects. The
   link is not held across the fetch.
4. The phone conditionally revalidates the small precipitation manifest and MET hourly response.
   If neither provider timestamp is newer than the held bundle's `generated_at`, it reconnects and
   sends `weatherUnchanged` (command `7`, §4.4): seven authenticated GATT bytes and no CoC. If
   either changed — or freshness/location cannot be proved — it builds an OBCW bundle
   ([`OBCW_Spec.md`](OBCW_Spec.md)) and uploads it as `weatherBundle` (object type `20`, §11.5)
   over the ordinary reliable CoC, stamping the context's `request_id` into the bundle header's
   `Request ID` field so the two connections can be correlated.

The shape is what makes it affordable on a phone's background budget: two short
connections with the network work outside both of them, and a payload small
enough that the upload is not an event either side has to plan around. That
payload grew with WXR5
(#1244): a corridor of the one uniform dataset is 162 x 162 cells in every frame,
so a bundle is tens of kB in practice and up to 256 KiB by the phone's producer
policy — **roughly 10-13 s on the CoC at the ~20-25 kB/s §11.1 estimates, against
§11.3's 60 s advertising window**, where a 46 KiB bundle was a couple of seconds.
It fits with room, and the headroom is one of the things the on-glass pass
measures rather than trusts. Nothing in the exchange is a new transfer mechanism —
step 4 is an ordinary §4.2 upload with an ordinary whole-object CRC-32 and an
ordinary `transferResult`.

### 11.2 The request id — what it is, and what it is not

`request_id` is a `u32` nonce, **monotonic per device boot** and **stable across
the retry ladder**: every attempt at one request carries the same id, so retries
of a request stay one request rather than becoming several the phone might answer
several times.

It **correlates**. It is **not** an authorisation token and **not** an upload
gate. A bundle carrying an unknown or stale request id is still accepted if it
validates and is newer than the active one (§11.6) — a fresher forecast is useful
no matter which request provoked it, and refusing one because the device has
since moved on would throw away work the phone has already paid for.

### 11.3 Advertising a pending request

While a request is pending, the advertised UUID is Weather Request's (§2). Two
rules govern when that stops, and both exist to prevent a specific failure:

- **The hint is lowered only by an authenticated read that was actually served.**
  Both halves matter. A connection that never authenticated must not consume the
  request — that is how a passer-by's scan would silently cost the rider a
  forecast — and neither must a read whose ATT response never reached the
  controller. An unbonded peer that connects to the advertisement gets an ATT
  security error (§8) and leaves the request exactly as pending as it found it.
- **The window is a monotonic deadline, not a restartable timer.** The budget runs
  from the moment the request was raised (reference firmware: **60 s**) and is
  never extended by a connection. Restarting it per connection would let a stray
  central that connects and drops repeatedly turn a bounded hint into a permanent
  secondary beacon — a battery bug that only appears in the field, next to
  somebody else's misbehaving phone.

When the budget expires the device returns to advertising OBC Control. **The
request itself does not expire with it**: it stays pending on the retry ladder
(the reference firmware's is **5 / 10 / 20 minutes**), and each step re-raises the
advertising hint with the *same* `request_id` (§11.2). The request is finished by either a valid
bundle being **accepted** — any upload §11.6 answers `committed`, the duplicate/stale
ignored-but-successful rows included — or a matching `weatherUnchanged` command being accepted
after the phone's conditional checks. Each is the phone's complete answer; an advertising window
closing is not.

### 11.4 `weatherRequestContext` — the request context (v1)

A read of the `B3B60001-…` characteristic (§3.5). **52 little-endian bytes**;
byte 1 declares that length.

```
weatherRequestContext v1 (52 bytes, little-endian):
   0  u8   version = 1
   1  u8   encoded_len = 52          the writer's own length (see decoding below)
   2  u16  validity                  which optional groups below are populated
   4  u16  reason                    why this request is due (advisory)
   6  u8   refresh                   0 = Off · 1 = 15 · 2 = 30 · 3 = 60 · 4 = 120 min
   7  u8   reserved = 0
   8  u32  request_id                §11.2 — echoed into the OBCW header
  12  i32  lat_udeg                  WGS84 microdegrees          (validity bit 0)
  16  i32  lon_udeg                  WGS84 microdegrees          (bit 0)
  20  i64  fix_utc                   UTC seconds of that fix     (bit 0)
  28  u16  bearing_deg               travel bearing, 0..359      (bit 1)
  30  u16  speed_deci_ms             ground speed, 0.1 m/s       (bit 2)
  32  u16  route_id                  the active route's id       (bit 4)
  34  u16  reserved = 0
  36  u32  bundle_generation         the held bundle's generation (bit 3)
  40  i64  bundle_generated_at       its generated_at, UTC secs   (bit 3)
  48  u32  bundle_crc32              its whole-bundle CRC-32      (bit 3)
```

| `validity` bit | Group |
| --: | :-- |
| `0` | position — `lat_udeg` · `lon_udeg` · `fix_utc` carry a real GPS fix |
| `1` | bearing — `bearing_deg` is a travel bearing the device believes |
| `2` | speed — `speed_deci_ms` is a trustworthy ground speed |
| `3` | active bundle — `bundle_generation` · `bundle_generated_at` · `bundle_crc32` describe a bundle the device has validated and selected |
| `4` | route — `route_id` names the active route object |

| `reason` bit | Why the request is due |
| --: | :-- |
| `0` | scheduled — the configured refresh interval elapsed during a ride |
| `1` | urgent — the rider opened Weather |
| `2` | retry — a previous attempt failed; this is a step on the ladder (§11.3) |
| `3` | no bundle — there is none at all, or the active one has expired |
| `4` | out of area — the rider has left the active bundle's covered corridor |
| `5` | hourly only — the active bundle contains no rain frames, so a rain-manifest identity alone cannot prove it complete |

The reference firmware uses a **2 km point-forecast reuse radius** around the centre of the active
bundle's 90 km rider-centred window. Opening Weather does not raise an urgent request while that
bundle is inside the radius and before the next possible quarter-hour publication. A bundle built
inside the publisher's two-minute processing grace is rechecked when that same grace ends; a build
after it is rechecked after the following quarter-hour grace. At or beyond 2 km, with an hourly-only
bundle, or whenever the proof is unavailable, the firmware raises normally and the phone performs
the full build. Near a dataset edge the clipped bundle centre can cause an early refresh; it cannot
cause reuse beyond the stated radius.

**Field widths mirror the OBCW header deliberately** — `i32` microdegrees, `i64`
UTC seconds, `u32` generation and CRC ([`OBCW_Spec.md` §3](OBCW_Spec.md)) — so a
value read here round-trips into a bundle header without narrowing. A `u32`
timestamp or a `i16` bearing would have been smaller and would have needed a
conversion at exactly the boundary where a mistake is invisible.

**Optional groups are guarded by flags, not sentinels.** No fix is *absent*, not
the equator; no bundle is *absent*, not generation `0`. This is the same rule §1
applies to the identity read's trailing fields, and it exists for the same reason:
a sentinel that is also a legal value eventually gets acted on. A device with no
fix still raises a well-formed request for diagnostics and retry, but the current
companion cannot fetch until the device supplies a fix; there is no phone-location
fallback in this protocol version.

**Decoding.** The read is **length-declared**:

- Fewer bytes than byte 1 declares → **rejected as truncated**, never
  half-decoded. That includes a read shorter than the 2-byte version/length prefix
  itself.
- A declared length **below 52** → rejected. v1 is the first version, so a writer
  claiming less is not an old writer, it is malformed.
- Bytes **past** this version's 52 → **ignored**, so a later firmware may append a
  field without breaking a shipped app. The `version` byte is reported as it
  arrived, not normalised away.
- **Unknown `validity` / `reason` bits and the reserved bytes are ignored, not
  rejected.** This is a deliberate difference from the OBCW header, which rejects
  nonzero reserved bytes ([`OBCW_Spec.md` §9](OBCW_Spec.md)): a bundle is a stored
  artifact validated once and trusted afterwards, whereas these bits are how a
  later firmware mentions something this build was never going to act on. Refusing
  the whole read over one would strand a rider's forecast on a byte nobody needed.
- An **out-of-range `refresh`** byte is **not** an error here. This is a device →
  phone read, so a value the reader does not know is a newer device, not a broken
  one: it is carried verbatim and reported as *unknown* — neither `Off` nor the
  default — exactly like the unrecognised bits above. The strict reading belongs
  to the one direction that has to *adopt* the value, a Config write (§11.8).
- The `reason` word is **advisory scheduling help**, never a gate on the ordinary full fetch: a
  phone that recognises none of the bits still performs it. A phone may use known bits to disable
  the no-change optimisation conservatively (`out of area` and `hourly only` do exactly that).

Before any request is raised, the attribute holds a structurally valid v1 value
with `validity = 0` and `reason = 0` — so a peer that reads it out of turn learns
"nothing is due" rather than the rider's last known coordinates.

### 11.5 `weatherBundle` — object type `20`

One OBCW v1 file, **app → device, upload only**, over the ordinary reliable CoC.
Its placement in the type space and why it is neither USB-only nor map-shaped is
§4.1.

- **Singleton.** `object_id` MUST be `0`. There is exactly one weather bundle, so
  the id selects nothing; any other value is answered **`notFound`** rather than
  quietly treated as `0`. It is *not* `0xFFFF`/new-only like `map`: "new-only"
  exists because a map cannot be replaced in place, whereas a bundle is **always**
  a replacement.
- **Ordinary transfer machinery.** The descriptor carries the real `total_len` and
  whole-object CRC-32 (§6), the payload streams through the same invisible temp
  every small object uses, and the close is an ordinary `transferResult` (§4.3).
  There is no held-back magic and no per-type announce rule.
- **Validation before commit.** The device verifies the descriptor's CRC-32 and
  then the container's own structure and CRC
  ([`OBCW_Spec.md` §9](OBCW_Spec.md)'s validation order). A CRC-failed transfer is
  `crcMismatch` and commits nothing (§4.2, unchanged); a bundle that arrives
  intact but does not validate as OBCW is answered **`error`** and likewise never
  selected. The two are deliberately different bytes: `crcMismatch` says *the wire
  corrupted your bytes, send them again*, and a retry is the right response;
  `error` says *these bytes arrived exactly as you sent them and they are not a
  bundle*, where a retry would reproduce the same failure and the fault is the
  phone's to fix. Answering the second case `crcMismatch` would hide a producer
  bug behind an infinite, blameless-looking retry ladder.
- **Where it lands** is a device convention rather than a wire one, but it is why
  an interrupted upload is harmless: the reference firmware holds **two** slots and
  writes into the inactive one, so the bundle the rider is looking at survives a
  torn transfer untouched. The slot rules are
  [`firmware/docs/WEATHER_STORAGE.md`](../firmware/docs/WEATHER_STORAGE.md).
- **No download direction.** The device never serves a bundle back: the phone
  built it and can rebuild it, and the only thing the device knows that the phone
  does not — *which* bundle it currently holds — already crosses the wire in the
  request context's bundle group (§11.4).

### 11.6 Duplicate, stale, and what an arriving bundle becomes

Once a bundle has passed CRC and OBCW structural validation, its fate is decided
by one rule — **newest valid generation wins** — compared with **RFC-1982-style
serial arithmetic** rather than `<`, so a generation counter that wraps does not
strand the device on a bundle from before the wrap. The decision is deliberately
**independent of the request id** (§11.2).

Serial arithmetic leaves two cases genuinely ambiguous — an **equal** generation,
and generations exactly **half the range** apart — and in both the later
`generated_at` decides. That tiebreak is not an embellishment: it is what
`obc_weather`'s slot selector already uses to pick a bundle at boot, and the two
must agree or the device can answer `committed` and then quietly boot the *old*
bundle. `obc-ble`'s classifier is tested against that selector directly across the
whole matrix rather than trusting a comment that says they match.

| Incoming vs. the active bundle | Disposition | `transferResult` |
| :-- | :-- | :-- |
| no valid bundle held | **commit** into the inactive slot and select it | `committed` |
| serially newer generation | **commit** | `committed` |
| equal (or half-range) generation, later `generated_at` | **commit** | `committed` |
| identical generation **and** `generated_at` | **duplicate — ignored** | `committed` (success) |
| serially older, or the same generation with an earlier `generated_at` | **stale — ignored** | `committed` (success) |

The two "ignored but successful" rows are the load-bearing part. A duplicate is
the phone doing its job twice — a lost ack, a re-run request — not an error;
failing it would send the phone back around the retry ladder to upload the very
same bytes again. A stale bundle is a phone whose HTTP path was slow while a
newer bundle landed from another attempt; answering an error there pushes it into
a retry loop it cannot win, because every retry produces the same too-old bundle.
Both did exactly what this contract asked of them, so both are told so. Only the
commit rows change what the rider sees.

### 11.7 Capability discovery

`FEATURE_WEATHER` is **bit `0`** of the identity read's `feature_bits` word (§1).
It covers the **whole** contract — the service, the context read, object type `20`
and the Config field — because the parts are useless apart: a phone that can read
a request but cannot upload the answer has nothing to offer, and a device that
accepts bundles but never asks for one is never asked. Later capabilities that are
genuinely separable take their own bits.

An **absent** word (a 7- or 6-byte read, or a partial one — §1) means the device
never told us, which is exactly a device without weather: the app does not scan
for the second UUID, does not offer weather in its UI, and behaves as it did
before WX3. Never a fabricated `0`, so a diagnostic cannot claim a firmware
generation answered when it did not. **Unknown bits are ignored** — an unknown
neighbour never masks a known one.

**A device sets the bit only when it implements the whole contract**, not when it
merely carries the layouts. Serving the 11-byte read and holding the service in
the GATT table is not the contract; accepting a type-`20` upload and honouring a
refresh interval is. A device that announced the bit while still answering every
bundle `error` would send a phone round the fetch-build-upload loop forever, at
its own expense, for a forecast that can never land — the one failure mode the
capability word exists to make impossible. Announcing zero optional contracts is
not a smaller truth than announcing one that does nothing; it is the only accurate
one, and it costs nothing, because a device that announces nothing is exactly the
old-client case every app already handles.

### 11.8 Refresh interval

How often the device raises a *scheduled* request (`reason` bit `0`). One enum,
used in two places: the trailing Config field (§7.3) the rider sets, and the
context's `refresh` byte (§11.4) so the phone can schedule its own work without a
second read.

| Value | Interval |
| --: | :-- |
| `0` | Off |
| `1` | 15 minutes |
| `2` | 30 minutes — **the device default** |
| `3` | 60 minutes |
| `4` | 120 minutes |

`Off` has **no** interval rather than a zero one: the wire carries the
discriminant and the minutes are derived, so nothing has to encode "never" as a
number.

**An unrecognised value is handled by direction, not uniformly.** This is the one
rule in §11 that is deliberately asymmetric, and the asymmetry is the point:

| Direction | Where | An unrecognised value |
| :-- | :-- | :-- |
| phone → device | a Config **write** (§7.3) | **rejected** — the write fails whole |
| device → phone | the context `refresh` byte (§11.4) | **unknown** — decoded, reported as unrecognised, never fatal |
| device → phone | a Config **read** (§7.3) | **unknown** — as above |

A device asked to *adopt* an interval it does not know must refuse: it cannot
honour the value, and storing anything else — the default, `Off`, the previous
setting — would tell the rider their choice was applied when it was discarded.

A reader must not. An unrecognised value arriving *from* a device is not a
malformed device, it is a **newer** one: this enum is append-only like everything
else here, and adding a fifth interval is an ordinary append. Under a
direction-blind reject that append would break every **shipped** app against new
firmware — the context read would fail, so weather would go permanently dead, and
the Config read would fail too, so the app could no longer read Config even to
rename the device. A trailing enum value would have become a breaking change,
which is exactly what the append-only discipline everywhere else in this document
exists to prevent. So a reader treats it as §11.4 treats an unrecognised `reason`
bit: carried verbatim, reported as unknown, ignored.

Unknown is its own state, and specifically **not** `Off` and **not** the default —
a phone that collapsed it to either would misreport the rider's own setting back
to them. Implementations therefore keep the **raw byte**, so a value they cannot
name still round-trips unchanged.

An **absent** Config field is likewise not `Off` — and what it *does* mean also
depends on direction: on a **read** it is the device default; on a **write** it is
*leave the stored value untouched* (§7.3).

## Reference implementation

Firmware: the `obc-ble` workspace crate (descriptor codec + transfer state
machine, lands with A5; the §11 context codec and advertising policy are its
`weather_request` module) and `obc-route` (OBCR v3). App:
`companion-ios/Packages/OBCKit` (`OBCTransport/Transfer`, `Codecs/`,
`BLE/GATT.swift`). Shared fixtures: [`specs/vectors/`](vectors/) —
routes with/without waypoints, a ride, a config blob, the route list, and
transfer-descriptor transcripts, asserted byte-exact from both languages.
