# OBC BLE Interface Specification (legacy wire v2)

> **Object transfers use protocol v4.** The normative contract is
> [`FLAT_Store_Protocol.md`](FLAT_Store_Protocol.md). This document remains the authority for the
> live command, status and config characteristics. Its retired object surface is not served.
>
> ⚠️ **The radio no longer speaks this document's object surface** (FS7.5-c3a, epic #1256). On BLE
> the object surface is **protocol v4**, whose normative contract is
> [`FLAT_Store_Protocol.md`](FLAT_Store_Protocol.md) §5.1. Concretely, and these are the lines a
> client implementer will otherwise trust:
>
> - **`transferControl` (`3C920005`) is retired on BLE.** Its replacement is `objectControl`
>   (`3C920009`): one Write Request carries one complete v4 control frame, one *confirmed
>   indication* carries its response. There is no 12-byte descriptor and no `transferResult`.
> - **`protocolVersion` (`3C920008`) is two bytes, `u16` = 4.** The `VersionRead` widening —
>   `version · store_epoch · obcm_version` — is retired with the store epoch itself:
>   a v4 client learns the card's identity from the `StoreId` every `LIST` page carries (§3).
> - **`command` (`…0001`), `status` (`…0002`) and `config` (`…0004`) are unchanged** and this
>   document remains their authority. They were never part of the object surface.
>
> ⚠️ **And the cable no longer speaks it either** (FS7.5-c3b, epic #1256). USB is protocol v4 too,
> bound by [`FLAT_Store_Protocol.md`](FLAT_Store_Protocol.md) §5.2, so this document's USB binding
> is **retired in full**. The two clauses a client implementer will otherwise trust:
>
> - **The `selector u8` envelope is gone.** Both bulk endpoint pairs carry §3 frames, each record
>   framed as `record_length u32` + frame bytes + zero alignment padding, and a record spans
>   packets freely. The retired "one frame is one USB transfer" rule and its seven-selector table
>   describe nothing that exists.
> - **The identity read (selector 4) and the device-information read (selector 5) are not
>   replaced by frames.** USB binding v5 is settled by descriptor matching (`bInterfaceProtocol = 5`,
>   `bcdDevice = 0x0500`) before a record moves, and the three device-information strings are one EP0
>   vendor request (§5.2.1 of that document). There is **no** identity read on this link any more.
> - **Object types `17`–`19` (`mapShard` / `mapSet` / `terrainShard`) are deleted from the tree**,
>   reader side included, and their values are not reissued. The "Volume sets" section of §4.1 is
>   history rather than a contract; a map is one OBCM object.
>
> So this document is now *the radio, minus its object surface*: §2 (advertising), §3 (the GATT
> table), §5 (the CoC), §8 (pairing) and the `command` / `status` / `config` characteristics of §4.
> Everything it says about USB, and everything it says about how an object crosses a wire, has a
> successor elsewhere.

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
> where they disagree, this spec wins and the notes are corrected.

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
| 7 | a device with a map reader | version, epoch, and map-format version |
| 6 | a firmware predating `obcm_version` | version + epoch, map-format version **absent** |
| 2 | a device with **no mounted store** | version only, `store_epoch` **absent** |

A reader takes only complete fields and ignores bytes beyond the fields it knows. A read of
three, four, or five bytes leaves `store_epoch` absent. Missing fields remain unknown; the
reader does not supply a default value.

**Why appending `obcm_version` did not bump `protocol_version`.** A bump is a hard
stop — the mismatch path above disables sync in both directions, by design, because
a bump means the wire is no longer mutually intelligible. That is the wrong signal
here. The field is optional by construction: a peer that predates it reads seven
bytes, takes the six it understands, and loses nothing it had; a peer that expects
it against an older device reads six, gets *absent*, and takes the defined unknown
branch. Neither side is wrong and neither needs to stop, so bumping would break a
pair that is fully interoperable in order to announce a field that is allowed to be
missing. A trailing field appended to an existing layout whose length is
self-describing is additive,
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

## 3. GATT control plane

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
exactly one place in the firmware, so this characteristic and the USB device-information read
([`FLAT_Store_Protocol.md`](FLAT_Store_Protocol.md) §5.2.1) always carry identical bytes.

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

## 4. Transfers, status, and commands

### 4.1 Object model

Every bulk payload is a typed **object**:

| `type` | Object | Direction | Payload |
|---|---|---|---|
| `1` | `route` | app → device (upload), device → app (detail read) | an OBCR v3 file, §7.1 |
| `2` | `ride` | device → app | ride object v3, §7.2 |
| `3` | `config` | — | reserved on the CoC; Config crosses GATT (§3.3) |
| `4` | `diagnostics` | device → app | diagnostics blob, §7.5 |
| `5` | `fwImage` | app → device (upload) | a complete `UPDATE.BIN` OBCU update image, §7.6 |
| `6` | `routeList` | device → app | list object, §7.4 |
| `7` | `rideList` | device → app | list object, §7.4 |
| `8` | `echo` | both | dev/test only: device streams back what it received (A5's loopback) |
| `9` | `trip` | app → device (upload), device → app (detail read) | trip object v2, §7.7 |
| `10` | `tripList` | device → app | list object, §7.4 |
| `11`–`15` | — | — | reserved (sensors, M4) |
| `16` | `map` | host → device (upload) | an `.obcm` map — **USB only** ([`FLAT_Store_Protocol.md`](FLAT_Store_Protocol.md) §5.2), see below |
| `17` | `mapShard` | host → device (upload) | one OBCM shard of a volume set ([`OBCA_Spec.md` §5.1](OBCA_Spec.md)) — **USB only**; **retired**, see below |
| `18` | `mapSet` | host → device (upload) | the OBCS set manifest ([`OBCA_Spec.md` §5.2](OBCA_Spec.md)) — **USB only**; **retired**, see below |
| `19` | `terrainShard` | host → device (upload) | the set's OBCT terrain shard ([`OBCA_Spec.md` §5.1](OBCA_Spec.md)'s `terrain` role) — **USB only**; **retired**, see below |

> **Types `17`–`19` are retired by OBCM v14 / issue #1420, and they have left the tree**: FS7.5b
> took the producers, FS7.5-c3b the readers — the descriptor's set-part field in `obc-ble`, the
> board's set machinery and the three `transfer-set-*.bin` wire vectors are all deleted. A map is one
> OBCM object with its
> terrain raster inside it
> ([`OBCM_Spec.md` §1.3](OBCM_Spec.md)), so a map transfer is an ordinary single-object `PUT` of
> kind *map* under protocol major **4** — [`FLAT_Store_Protocol.md`](FLAT_Store_Protocol.md) is the
> normative contract for that, and it needs no map-shaped opcode at all. Nothing about the wire is
> redesigned here: this note marks what dies. The three numbers stay spent (they are not reissued to
> anything else), the "Volume sets" section below stops being normative with them, and type `16`
> `map` is the shape the single transfer already had.

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
device MUST answer any of the four with `error` on the radio. (Retired with the set, above: a
DACH-shaped map is those same 7.6–8.9 GiB as **one** object, the argument for the USB-only band is
unchanged by that, and only `map` is left to make it.)

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
also an instruction (`mapSet`). That exception retires with the set (`mapSet` is gone under OBCM
v14), leaving the quiesce rule with no exception at all.

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

  msg         u8   = 5; authenticated request context is ready to read
```

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
| `5` | `setClock` | `utc u32 · offset_min i16` | the phone stamps the device's UTC clock + local offset on **every connect** . Stamps the wall-clock set-point, **persists** the offset, and marks the clock *trusted* for the boot . Sent immediately after encryption, **before** `ackRides`. Validated → `error` on a malformed length, `utc < 1577836800`, or `\|offset_min\| > 840`; no store-revision bump (the clock is not an object). See below |
| `6`–`15` | — | — | reserved (identify/find-my-device, factory reset, …) |

**Next free command: `6`.**

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
- An acknowledgment records durable possession. Repeated acknowledgment is idempotent.

**Synced** means that a durable copy exists off the device. The flag is display information.
It does not authorize automatic deletion.

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
changes nothing. **`installFw` — install the staged update (M4).** After a `fwImage` upload
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

**`setClock` — stamp the trusted wall clock .** The device has no RTC: at boot its
clock resumes from a persisted set-point, stale by however long the device was off, and that stale
clock is **untrusted** — nothing is stamped or deleted from it. Exactly two sources establish a
*trusted* clock for the boot: a GPS fix (which carries full UTC) and this command. `setClock` is a
7-byte write — `cmd u8 = 5 · utc u32 · offset_min i16`, all little-endian:

- **`utc`** is the phone's current time in **unix seconds** (UTC). The device sets its wall-clock
  UTC set-point from it (seconds-resolution: the display's minute rolls at the true instant).
- **`offset_min`** is the phone's current **local UTC offset in minutes**, with **DST already
  applied** (`+02:00` → `120`). The phone is the timezone oracle — the device holds no tz tables and
  runs no DST math; route age arithmetic is pure UTC, and the offset only shifts the *displayed* hour.
  The offset is **persisted** (it survives reboots and seeds the boot display clock) and silently
  refreshed by every connect, so a rider crossing time zones need only reconnect the app.

On a valid write the device stamps the clock, persists the offset, marks the clock **trusted** for
the boot, and answers `commandResult(ok)`. The clock is **not an object** — there is **no
store-revision bump** and no `storeChanged`. Validation answers `commandResult` `error` (§4.3) for a
**malformed length** (not exactly 7 bytes), a **`utc < 1577836800`** (before 2020-01-01 — an
obviously-bogus phone clock), or an **`offset_min` beyond ±840** (±14 h, the real-world −12:00…+14:00
span). A device that predates the command answers `unknown` (§4.4 compat), which the app reads as
"clock sync is unsupported" and degrades gracefully.

**Ordering — sent before `ackRides`.** The app sends `setClock` on **every connect, immediately
after encryption and before the first `ackRides`** (or any reconcile write). This is what lets ride
`synced_at` stamping (#638 S3) assume a trusted clock: the `ackRides` that first flags a ride synced
runs *after* the clock is trusted, so the timestamp it stamps is real. (`setClock` itself needs no
identity read — it establishes local time, not id-scoped state — but it shares the same
post-encryption prologue as the version+epoch read and the ack, §1.)

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

> **The exception below is retired with the USB binding.** Protocol v4 verifies the declared
> length and a whole-payload CRC-32 before every commit and runs the kind's validator, maps included
> ([`FLAT_Store_Protocol.md`](FLAT_Store_Protocol.md) §3.6); a mismatch is `checksumFailure` and
> nothing is published. The trade this clause made was worth making against a filesystem that could
> not commit atomically, and the flat store's single durable commit is what made it unnecessary.

The one exception is an upload of the USB-only map-shaped types `map`,
`mapShard`, `terrainShard`, and `mapSet` — of which only `map` survives OBCM v14, and it survives
carrying the terrain bytes the other two used to carry, so the exception's *reason* (these objects
are gigabytes) applies more strongly rather than less. A device **MAY** omit the second serial
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

### 7.2 `ride` — ride object v3

A ride payload is the sample stream the device recorded, followed by one fixed summary footer.
There is no leading header and no finish-time conversion. Protocol-v4 `GET` serves the stored bytes
unchanged.

Each sample is the existing 20-byte little-endian record:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 4 | longitude, `i32` microdegrees |
| 4 | 4 | latitude, `i32` microdegrees |
| 8 | 2 | elevation, `i16` metres |
| 10 | 2 | flags; bit 0 is `segment_start`, all other bits are zero |
| 12 | 4 | monotonic timestamp, `u32` milliseconds |
| 16 | 1 | heart rate, bpm; `0xFF` = absent/stale |
| 17 | 1 | cadence, rpm; `0xFF` = absent/stale |
| 18 | 2 | power, watts; `0xFFFF` = absent/stale |

The final 84 bytes are the summary footer:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 4 | magic `OBRF` (`4F 42 52 46`) |
| 4 | 1 | version, `3` |
| 5 | 1 | UTF-8 name length, `0..=48` |
| 6 | 2 | footer length, `84` |
| 8 | 4 | start time, Unix seconds |
| 12 | 4 | total distance, metres |
| 16 | 4 | moving time, seconds |
| 20 | 2 | average speed, cm/s |
| 22 | 2 | climb, metres |
| 24 | 4 | point count |
| 28 | 1 | average heart rate; `0xFF` = absent |
| 29 | 1 | maximum heart rate; `0xFF` = absent |
| 30 | 1 | average cadence; `0xFF` = absent |
| 31 | 1 | reserved, zero |
| 32 | 2 | average power; `0xFFFF` = absent |
| 34 | 2 | maximum power; `0xFFFF` = absent |
| 36 | 48 | UTF-8 name followed by zero padding |

The footer is last because the flat-store payload pages are write-once. A list row reads precisely
84 bytes at `object length − 84`; a full reader requires
`object length == point_count × 20 + 84`. Finalize appends this footer and performs one store commit
that publishes the final length/CRC and clears `RECORDING`. `specs/vectors/ride-v3.bin` pins three
sample records—including sensor sentinels and segment flags—and the footer.

### 7.3 `config` — the Config object

Crosses GATT on the `config` characteristic (§3.3), whole-blob on both read
and write. Maximum encoded size **128 bytes**.

```
Config v1:
  name_len         u16  ≤ 48 (UTF-8 bytes; matches the OBCR route-name cap)
  name             name_len bytes, UTF-8 — THE device name (Delta 1: rename = write
                   Config with a changed name; there is no separate rename command)
  units            u8   0 = metric · 1 = imperial
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
(`routeList` 76 bytes, `rideList` 72, `tripList` 76), so the entry size is carried
per-list in the header's `entry_len` byte; readers step by it, never a constant.

```
List header (6 bytes):
  version     u8   = 2
  entry_len   u8   the entry size (76 routeList · 72 rideList · 76 tripList) — readers skip by it
  count       u16  entries actually in this object (after the MAX_RIDES / MAX_ROUTES / MAX_TRIPS cap)
  total       u16  full catalog size BEFORE the cap
```

**`total`** (v2, epic #632 item 7) makes the >`MAX_RIDES` (or >`MAX_ROUTES` /
>`MAX_TRIPS`) truncation visible on the wire: the object is **truncated iff `total > count`**
(the device dropped `total - count` entries in FAT order), and the app surfaces a
one-line warning instead of silently answering "up to date". When nothing was
dropped `total == count`.

`routeList` entry (**76 bytes**) — from the stored OBCR header and content CRC:

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
has no additional fields. Both entries are 76 bytes:

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

### 7.7 `trip` — a trip object (v2)

A **trip** groups planned routes into one named unit (one folder on the device,
one card in the app). It is a tiny metadata object that **references route object
ids** in ride order — it never contains route bytes. Routes stay byte-identical
OBCR v3 files (§7.1); membership edits never touch a route payload. The reference
firmware stores each trip as `TP{id}.OBT` beside the `RT{id}.OBR` route files (no
FAT subdirectories); trip ids come from a separate device counter (§4.1).

```
trip object v2 (56-byte header + 8 bytes/stage, little-endian):
  version      u8   = 2
  reserved     u8   = 0
  stage_count  u16
  name_len     u8   ≤ 48
  name         char[48]  UTF-8, zero-padded
  reserved     u8[3]  = 0
  stages       stage_count × u64   flat-store ObjectIds, ride order
```

The object length is fully determined by its header: `56 + 8·stage_count` bytes.

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

## Reference implementation
