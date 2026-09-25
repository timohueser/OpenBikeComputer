# OBC BLE interface

The GATT surface the device serves to the companion app: the two SIG services, the custom OBC
Control service, its live characteristics, and the payload layouts of the objects those
characteristics carry. Pairing and encryption are §8. The QR link that starts pairing is §9.

Object transfers are **not** here. They are protocol v4, whose normative contract is
[`FLAT_Store_Protocol.md`](FLAT_Store_Protocol.md); §5.1 of that document binds it to the
`protocolVersion`, `psm` and `objectControl` characteristics below.

All multi-byte integers are **little-endian**. Shared binary test vectors pinning these layouts
live in [`specs/vectors/`](vectors/) and are read by both `cargo test` and `swift test`.

## 3. GATT control plane

### 3.1 Device Information Service — `0x180A` (SIG)

| Characteristic | UUID | Value |
|---|---|---|
| Firmware Revision String | `0x2A26` | UTF-8 version of the **running** image — see the dialect below |
| Hardware Revision String | `0x2A27` | UTF-8 board id, e.g. `nrf54l15-dk`, `obc-lm20-r1` |
| Serial Number String | `0x2A25` | 16 uppercase hex digits — the nRF `FICR.DEVICEID` |

**The firmware-revision dialect.** The value is the version the running image was *wrapped* with,
in this preference order:

1. the **installed image's OBCU version string, verbatim** — the `fw_version` field of the container
   the device installed ([`OBCU_Spec.md`](OBCU_Spec.md) §1). For a released build that is the
   release tag, e.g. `v1.3.0`;
2. otherwise the **build's bare git short hash**, e.g. `ca9b336` — a device flashed over SWD, which
   has never installed a container and has no version to report.

Case 1 is the only string that can be compared against a published release. Hosts parse it as a
release version with an optional leading `v`.

Case 2 is deliberately **not parseable as a version**: a host that cannot read a running version
MUST NOT offer an automatic update. A development device stays on whatever its owner flashed and
gets back onto the release track through the manual install path.

The value is at most 32 bytes (the OBCU `fw_version` field width). This characteristic and the USB
device-information read ([`FLAT_Store_Protocol.md`](FLAT_Store_Protocol.md) §5.2.1) always carry
identical bytes.

### 3.2 Battery Service — `0x180F` (SIG)

| Characteristic | UUID | Value |
|---|---|---|
| Battery Level | `0x2A19` | `u8` percent, read + notify |

### 3.3 OBC Control service (custom)

Base UUID (random; **not** derived from the SIG base): the 16-bit block `XXXX` in
`3C92XXXX-9916-4EBA-ABC2-342FE08F6B10` selects the entity. The device advertises the service UUID.

| UUID | Entity | Properties | Role |
|---|---|---|---|
| `3C920000-9916-4EBA-ABC2-342FE08F6B10` | **OBC Control service** | — | the advertised primary service |
| `3C920001-9916-4EBA-ABC2-342FE08F6B10` | `command` | write | small imperative commands (§4.4) |
| `3C920002-9916-4EBA-ABC2-342FE08F6B10` | `status` | notify | device → app messages (§4.3) |
| `3C920004-9916-4EBA-ABC2-342FE08F6B10` | `config` | read + write | the Config object (§7.3), whole-blob |
| `3C920007-9916-4EBA-ABC2-342FE08F6B10` | `psm` | read | `u16` — the dynamic L2CAP PSM of the protocol-v4 stream channel |
| `3C920008-9916-4EBA-ABC2-342FE08F6B10` | `protocolVersion` | read, open | `u16` = `4`, the protocol-v4 major |
| `3C920009-9916-4EBA-ABC2-342FE08F6B10` | `objectControl` | write + indicate | the protocol-v4 control channel |

The last three are bound by [`FLAT_Store_Protocol.md`](FLAT_Store_Protocol.md) §5.1, which owns
their byte rules.

`3C920003`, `3C920005` and `3C920006` are retired and **MUST NOT** be reused. A retired UUID never
takes a new meaning.

### 3.4 Connection parameters

The device requests, and the app accepts where the OS allows: **2M PHY**, data length extension
(**251-byte PDUs**), ATT MTU **247**. The L2CAP CoC MPS is aligned to the PDU so one SDU chunk of
**244 bytes** rides in one packet. These are preferences, not requirements — the protocol is
correct at any negotiated MTU, just slower.

## 4. Commands and status

### 4.3 `status` — typed device → app notifications

Every `status` notification is one message: a `u8` discriminator and a fixed body. One message is
defined:

```
msg = 3  commandResult (4 bytes total):
  msg     u8   = 3
  cmd     u8   echoes the command byte (§4.4)
  status  u8   0 = ok · 1 = unknown command · 2 = not found · 3 = busy · 4 = error
  detail  u8   command-specific, 0 unless documented
```

Discriminators `1`, `2` and `4` carried the retired object surface and are not served. An app
ignores a `msg` value it does not know.

### 4.4 `command` — small imperatives

A write of `cmd u8` and fixed args. Every command is answered with a `commandResult` (§4.3). An
unknown command is answered `unknown command` (`1`), which an app reads as "this device does not
support it".

| `cmd` | Command | Args | Effect |
|---|---|---|---|
| `3` | `installFw` | none | ask the device to install the staged update package — runs the on-device check and on-glass confirm |
| `4` | `forgetBond` | none | ask the device to dissolve **its** side of the bond |
| `5` | `setClock` | `utc u32 · offset_min i16` | stamp the device's UTC clock and local offset |

Commands `1` (`deleteObject`) and `2` (`ackRides`) are retired: object removal is protocol v4
`REMOVE` and ride possession is `ARCHIVE_RIDE`
([`FLAT_Store_Protocol.md`](FLAT_Store_Protocol.md) §3.7, §3.12). The device answers them
`unknown command`. **Next free command: `6`.**

**`installFw`.** The command only *requests*; it never waits for the rider and never installs on its
own. The device answers from cheaply-knowable edge state, then opens its on-device flow: validate
the staged image, show a confirm card, and install only on a physical **Select press**.

| Outcome | `commandResult.status` | Meaning |
|---|---|---|
| accepted | `ok` (0) | the device opens its on-glass check and confirm flow |
| busy | `busy` (3) | a ride is recording, or an install request is already pending |

Precedence: **`busy` > `ok`**. The two answers are the whole set, because "can the device act now"
is the only question the command handler can answer cheaply. Whether a package is staged, and
whether it is a valid one, are the on-device scan's own answers a moment later, and the multi-second
image scan MUST NOT run inside the command handler. A second answer given here would be a second
truth about the same fact, so the device accepts and the confirm card reports what the scan found.

**Security posture — no silent installs.** Staging a package is authenticated only by the bonded,
encrypted link (§8). **Installing** is gated on physical confirmation at the device. `installFw`
therefore never arms and never reboots on its own. Image authenticity is separate and is checked on
every delivery path: the armer verifies the container's Ed25519 signature, and refuses an unsigned
container, before an install can be armed ([`OBCU_Spec.md`](OBCU_Spec.md) §1.3, §1.4).

**`forgetBond`.** The bonded phone asks the device to clear its side of the bond, so an app-side
"Forget device" does not leave the pair wedged under the reject-when-bonded posture (§8). The device
runs the same machinery **Forget phone** does: zero the bond slot, drop the peer from the host bond
table and resolving list, lower the `paired` flag, then return to open-pairing advertising.

**Ordering is fixed:** the device notifies `commandResult(ok)` **before** it clears the bond and
disconnects, so the phone always gets its ack.

`forgetBond` is honoured **only over the authenticated, encrypted link** — the `command`
characteristic carries the `authenticated` permission (§8), so an unbonded peer never reaches the
handler. The command mints no replacement bond; it only clears.

**`setClock`.** The device has no RTC: at boot its clock resumes from a persisted set-point and is
**untrusted**. Exactly two sources establish a *trusted* clock for the boot: a GPS fix and this
command. The write is 7 bytes — `cmd u8 = 5 · utc u32 · offset_min i16`:

- **`utc`** is the phone's current time in **unix seconds** (UTC). The device sets its wall-clock
  UTC set-point from it, at seconds resolution.
- **`offset_min`** is the phone's current **local UTC offset in minutes**, with **DST already
  applied** (`+02:00` → `120`). The phone is the timezone oracle; the device holds no timezone
  tables and runs no DST math. Date arithmetic is pure UTC and the offset only shifts the displayed
  hour. The offset is **persisted** and is refreshed by every connect.

On a valid write the device stamps the clock, persists the offset, marks the clock **trusted** for
the boot, and answers `commandResult(ok)`. The clock is not an object, so nothing about the store
changes. Validation answers `error` for a **malformed length** (not exactly 7 bytes), a
**`utc < 1577836800`** (before 2020-01-01), or an **`offset_min` beyond ±840** (±14 h).

**Ordering — sent before any reconcile write.** The app sends `setClock` on every connect,
immediately after encryption. A timestamp the device stamps later in the session is then real.

## 7. Object layouts

A route object's payload is exactly the bytes of an OBCR file
([`OBCR_Spec.md`](OBCR_Spec.md)); the device stores and serves it verbatim. An update package's
payload is exactly the bytes of an OBCU container ([`OBCU_Spec.md`](OBCU_Spec.md) §1).

### 7.2 `ride` — ride object v6

A ride payload is the sample stream the device recorded, followed by one fixed summary footer.
There is no leading header and no finish-time conversion. Protocol-v4 `GET` serves the stored bytes
unchanged.

Each sample is a 20-byte record:

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

The final 154 bytes are the summary footer:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 4 | magic `OBRF` (`4F 42 52 46`) |
| 4 | 1 | version, `6` |
| 5 | 1 | UTF-8 name length, `0..=48` |
| 6 | 2 | footer length, `154` |
| 8 | 4 | start time, Unix seconds |
| 12 | 4 | total distance, metres |
| 16 | 4 | moving time, seconds |
| 20 | 2 | average speed, cm/s |
| 22 | 2 | climb, metres |
| 24 | 2 | descent, metres |
| 26 | 4 | point count |
| 30 | 1 | average heart rate; `0xFF` = absent |
| 31 | 1 | maximum heart rate; `0xFF` = absent |
| 32 | 1 | average cadence; `0xFF` = absent |
| 33 | 1 | reserved, zero |
| 34 | 2 | average power; `0xFFFF` = absent |
| 36 | 2 | maximum power; `0xFFFF` = absent |
| 38 | 4 | energy, kJ; `0xFFFF_FFFF` = absent |
| 42 | 48 | UTF-8 name followed by zero padding |
| 90 | 8 | trip key, `u64`; `0` = no trip |
| 98 | 1 | day index, 0-based |
| 99 | 1 | day count |
| 100 | 1 | bike type, `0..=3` (Road, Gravel, MTB, Touring) |
| 101 | 1 | UTF-8 trip name length, `0..=48` |
| 102 | 48 | UTF-8 trip name followed by zero padding |
| 150 | 1 | maximum heart rate limit, bpm; `0` = not set |
| 151 | 1 | reserved, zero |
| 152 | 2 | FTP limit, watts; `0` = not set |

- **Climb and descent.** The device counts both with the same dead band, from the same altitude
  samples.
- **Energy.** Each power sample adds its watts × the time since the previous sample, at most 2 s,
  while the ride records and is not paused. The total is rounded down to whole kJ. A ride without
  power data writes the absent value, so `0` is a ride with power data and no work.
- **Bike type.** The bike type that is current when the ride starts
  ([`OBCR_Spec.md`](OBCR_Spec.md) §1.2). The phone can change its own copy. The device copy does
  not change.
- **Trip.** A ride is on a trip when it starts on a day of a stored trip (§7.7). The footer holds the trip key, the index of that day and the trip's day
  count. The rider sees Day 1 for day index 0. A reader requires `day index < day count`.
- **Trip name.** The device writes the name of the trip when the ride is saved, so the name stays
  after the trip is deleted. It is empty when the device no longer holds a trip with that key.
- **No trip.** With trip key 0, the day index, the day count and every trip-name byte are zero.
- **Effort limits.** The rider's maximum heart rate and FTP settings when the ride starts. A
  continued ride keeps them, and a later settings change does not reach the ride. A reader takes
  the effort zones of the ride from these limits, never from the current settings. A limit of `0`
  gives that metric no zones. The zone edges are fixed percentages of the limit, so the footer
  holds no edges. A value `v` against limit `L` is in the highest zone whose edge it reaches:

  | Metric | Z2 | Z3 | Z4 | Z5 |
  | :-- | :-- | :-- | :-- | :-- |
  | Heart rate | `100·v ≥ 60·L` | `100·v ≥ 70·L` | `100·v ≥ 80·L` | `100·v ≥ 90·L` |
  | Power | `100·v ≥ 55·L` | `100·v > 75·L` | `100·v > 90·L` | `100·v > 105·L` |

The footer is last because the flat-store payload pages are write-once. A list row reads precisely
154 bytes at `object length − 154`; a full reader requires
`object length == point_count × 20 + 154`. A reader rejects any other footer length.
Finalize appends this footer and performs one store commit that publishes the final length and CRC
and clears `RECORDING`. `specs/vectors/ride-v6.bin` pins three sample records — including sensor
sentinels and segment flags — and a footer on a trip day with both effort limits set.

### 7.3 `config` — the Config object

Crosses GATT on the `config` characteristic (§3.3), whole-blob on both read and write. Maximum
encoded size **128 bytes**.

```
Config v1:
  name_len         u16  ≤ 48 (UTF-8 bytes; matches the OBCR route-name cap)
  name             name_len bytes, UTF-8 — THE device name (rename = write Config with a
                   changed name; there is no separate rename command)
  units            u8   0 = metric · 1 = imperial
  [future fields append here; readers MUST ignore unknown trailing bytes]
```

The append-only rule is the version mechanism: fields are never reordered or resized, only
appended, and absent trailing fields mean "device default".

The Config object carries **no firmware-version field**: the running image's version is the DIS
Firmware Revision String (§3.1).

### 7.7 `trip` — a trip object (v3)

A **trip** is a named list of days in ride order. Each day references one route object, the day
route. The trip never contains route bytes. The phone writes the trip object.

```
trip object v3 (64-byte header + 16 bytes/day, little-endian):
  version      u8   = 3
  reserved     u8   = 0
  day_count    u16
  name_len     u8   ≤ 48
  name         char[48]  UTF-8, zero-padded
  reserved     u8   = 0
  start_date   u16  days since 1970-01-01; 0 = no start date
  trip_key     u64  nonzero
  days         day_count × 16 bytes, ride order:
    route      u64  flat-store ObjectId of the day route
    join_m     u32  metres along the day route where it joins the main line
    leave_m    u32  metres along the day route where it leaves the main line
```

The object length is fully determined by its header: `64 + 16·day_count` bytes. A reader rejects
any other length, which also rejects a torn write.

- **Trip key.** The phone chooses the key and keeps it for the life of the trip. A re-upload of the
  same trip writes the same key. Device progress and rides refer to the key, not to the object id.
  Key 0 means "no trip" in those records, so a reader rejects a trip object with key 0. Reversing a
  trip makes a new trip with a new key, so its progress starts empty.
- **Day names and stats.** The display name of a day is the OBCR name of its route. The name is
  the day's own name, such as the name of the route or file it came from. A day without one is
  named after its number and end place ("Day 2 Ulrichen"). The day number itself comes from the
  day's position in the trip. Distance and climb come from the route's OBCR header. The trip
  object repeats neither.
- **Main line.** A day that starts on the main line has `join_m = 0`. A day that ends on the main
  line has `leave_m` at or past the end of its route; readers clamp `leave_m` to the route length.
  Other values mark an out-and-back spur to a stop off the line. The device skips the spur when it
  joins the rest of one day to the next day. Unless a transfer lies between them, the `leave_m` of
  day N−1 and the `join_m` of day N name the same point on the main line.
- **Transfer.** A day end is a transfer when the next day's route starts more than
  `TRANSFER_MIN_M` = 200 m, straight line, from the last point of the day's route. Readers derive
  it from the two routes; the trip object has no field for it. The device never joins the rest of
  a day to the next day across a transfer.
- **Reference-only.** A day route is a route object id. A route that no stored trip references is a
  top-level route. Membership is one level deep: a route is in at most one trip, or standalone.
- **Dangling refs are tolerated on read.** A day route deleted individually does not invalidate
  the trip: the device serves the trip verbatim, dangling ids and all. The device never rewrites a
  stored trip. Dangling refs persist until the next trip upload replaces the object.
- **Uploads commit verbatim.** A trip upload that references unknown route ids is stored as sent;
  validation belongs to the client.
- **Recommended upload order: day routes first, the trip object last.** An interrupted whole-trip
  push then never leaves a trip pointing at nothing, and re-running the push is idempotent.
- **Removing a trip removes only the trip object.** Its day routes become top-level routes; the
  wire has no cascading delete. A "delete trip *and* its routes" action is composed by the client as
  individual route removals plus the trip removal.

#### Trip progress

The device keeps one progress record per trip key. The record never crosses the wire. The device
writes it at Finish of a ride on a trip day, and keeps it in the ride-archive Metadata object
([`Ride_Archive_Metadata.md`](Ride_Archive_Metadata.md)) with the navigator checkpoint.

- **Start record.** The fresh start of a ride on a trip day, and the continuation of that ride
  after a reset, write a start record for the trip: position day 0, the route ObjectId of day 0,
  0 m, no last finished day, and all finish dates 0. The device writes no start record when the
  trip's record is already the last record.
- **A start record never replaces a record.** When the Metadata object holds a record for the
  trip key, the write moves that record to the end, byte for byte. Its Revision stays, so its
  metres read as 0 when its route has another Revision now. Only when the object holds no record
  for the key does the start record go in, with the Revision the store holds for the route of
  day 0. The store tells a start record by its missing last finished day; a Finish always writes
  one. The store applies this rule, so a start is safe before the device has read the records.

| Field | Meaning |
| :-- | :-- |
| position day | the day that contains the last matched position |
| position route | the route ObjectId and Revision of that day when the record was written |
| position metres | metres into that day's route |
| last finished day | the last day the rider finished; none before the first Finish |
| finish dates | for each day, the date of its Finish in days since 1970-01-01; 0 = none |

- **Re-upload.** A re-upload of the same trip key keeps the record. The position metres count
  only while the trip names the same route ObjectId, at the same Revision, for the position day.
  Otherwise they read as 0, and the last finished day stays.
- **Fewer days.** A position day or a last finished day at or past `day_count` reads as none. A
  re-upload with fewer days thus never reads as a done trip.
- **Bound.** The Metadata object holds at most 16 progress records, in write order. A write moves
  its record to the end. Before a write, the device drops each record whose key no stored trip
  holds. When 16 records remain, it drops the first one.
- **No route hold.** A progress record never blocks a route replace or remove, unlike the navigator
  checkpoint. The Revision check voids a stale position instead.

#### Day rules

Days count from 0 in the object; the rider sees Day 1 for day 0.

- **Next day.** `next = max(last finished + 1, position day)`. Without a record, day 0 is next.
  When `next` is past the last day, the trip is done. Example: the rider stops 20 km before the end
  of Day 2 and finishes. Day 3 is next, and the device loads the rest of Day 2 plus Day 3. When the
  rider instead rides 20 km into Day 3, Day 3 is next with 20 km less to ride.
  A ride on the rest of Day 2 plus Day 3 that ends before it joins Day 3 finishes Day 2, not
  Day 3. The position stays on Day 2, so Day 3 is still next.
- **Rode on.** A Finish after the rider arrived at the end of the loaded route and rode on past
  it writes the position at that end, unless the last fix lies within 50 m of the route of the
  next day. Then the position is the point of that route nearest the fix, in the next day. A later
  point of the route counts as nearer only when it is more than 8 m nearer than an earlier one, so
  the outbound leg of an out-and-back wins. The device projects that one fix once, when it writes
  the record.
- **Active trip.** The trip of the latest record, while it has a next day. A start record moves the
  trip's record to the end, so the trip of a started ride is active before its Finish.
- **Ticks.** A day is ticked when it is at or before the last finished day, or when it is before
  the position day.
- **Day dates.** Dates follow the rides. For day `k`, take the latest day `j ≤ k` with a finish
  date: `date(k) = date(j) + (k − j)`. Without such a day, `date(k) = start_date + k`. Without a
  start date either, day `k` has no date and no weekday. The progress record stores finish dates
  for days 0 to 31 only. A later day stores no date, so its date follows the last stored one.
- **Trip to go.** The rest of the loaded day, plus the distance and climb of the routes of all
  later days.

## 8. Security

- **Pairing**: LE Secure Connections, **passkey display** — the device is `DisplayOnly`, so it shows
  a 6-digit code on its screen that the rider types into the phone's pairing dialog (LESC passkey
  entry, MITM-protected). One bonded peer at a time.
- **Encryption requirements** once a bond exists:

| Surface | Requirement |
|---|---|
| DIS, BAS, `protocolVersion` | none (open — the app version-checks before pairing) |
| every other OBC Control characteristic | encrypted, LESC-authenticated link |
| the L2CAP CoC | encrypted link (opening it plaintext is refused) |

  The gated characteristics carry an `authenticated` (LESC-MITM) access permission; an unbonded peer
  discovers the service but gets Insufficient Authentication on every gated read, write or
  subscribe, and the CoC accept is refused on an unencrypted link.

- **Bond store**: the single peer's keys (LTK, peer identity and IRK, security level) persist in the
  device's RRAM settings carve, so a bond survives power cycles **and firmware reflashes** (the
  carve sits above the application image). At boot the device re-arms the bond so the phone's
  rotating RPA reconnect resolves against the stored peer IRK and re-encrypts with the stored LTK.

- **Single-peer policy — reject-when-bonded**: exactly one bond slot, and **while it is occupied the
  device refuses every new pairing attempt** — from a stranger and from a peer claiming the bonded
  identity alike. A stored bond can only be cleared by the rider: the hold-guarded **Forget phone**
  action in Settings ▸ Connections zeroes the bond slot, removes the peer from the host's bond table
  and resolving list, and drops the connection if that peer is connected. After Forget, the next
  pairing is open again. Physical possession guards the *clear* step, so a stranger who can see the
  screen cannot silently evict the rider's phone by pairing.
- **Reject mechanics.** The pairing link is not bondable while a bond is stored, and the device
  refuses the attempt at its first SMP surface: it suppresses the passkey display and drops the
  link. **No distinguishable SMP failure reason crosses the wire** — the host stack auto-answers the
  SMP Pairing Request before the application sees it, and iOS does not surface an SMP reason code to
  the app. The app infers "already bonded elsewhere" from context, not from a code. A phone that
  forgets the device **while offline** is rejected like any other until the rider runs Forget phone
  on the device; a forget **while connected** uses `forgetBond` (§4.4) and needs no on-device step.

- **Reconnect policy**: the device keeps a **stable static random address** and does **not** enable
  device-side privacy. The phone stores that identity and reconnects on any advertising contact.
  Identifying the phone behind its rotating RPA uses the stored peer IRK in the controller resolving
  list, not a filter accept-list.

## 9. Pairing link

During first-use setup the device shows a QR code. The code holds a universal link. The phone
camera opens the app from the link, and the app finds the one device that shows the code. Pairing
then runs as §8 specifies. The link carries no secret: the passkey stays the only proof that the
rider holds the device.

### 9.1 The link

```
https://openbikecomputer.com/pair/?s=<serial>
```

| Part | Bytes |
|---|---|
| scheme and host | `https://openbikecomputer.com`, lowercase |
| path | `/pair/` |
| query | one item, `s=<serial>` |
| `<serial>` | the Serial Number String of §3.1: 16 uppercase hex digits, `[0-9A-F]{16}` |

Example: `https://openbikecomputer.com/pair/?s=0123456789ABCDEF`. The link is 53 ASCII bytes. It has
no fragment and no percent-encoding.

**Why the serial.** The app can use only what it sees before it pairs:

- The device address is not available. CoreBluetooth gives an app an opaque identifier per phone,
  never the address.
- The factory name `OBC-XXXX` holds only the last four digits of the serial. Two devices can have
  the same factory name.
- The serial is the full 64-bit factory device id, and the open DIS serves it before pairing (§8).

The serial is not secret. The device gives it to every central in range.

### 9.2 The QR code

| Property | Value |
|---|---|
| Symbol | QR Code model 2 (ISO/IEC 18004) |
| Version | 4 (33 × 33 modules) |
| Error correction | level M |
| Segment | one byte-mode segment with the 53 link bytes |
| Mask | the mask that the standard penalty rule selects |
| Module | 5 × 5 px or larger |
| Quiet zone | 4 modules or more on each side |
| Colors | dark modules on a light background, in each theme |

Version 4 at level M holds 62 bytes in byte mode. At 5 px per module the code and its quiet zone
use 205 × 205 px, which fits the 240 px panel width.

### 9.3 Device rules

- The device shows the code only while its bond slot is empty (§8). A bonded device refuses every
  new pairing, so its code would be of no use.
- While the device shows the code, it advertises the OBC Control service UUID (§3.3) and its
  factory name `OBC-XXXX`, where `XXXX` is the last four digits of the serial. First-use setup
  follows a factory reset, which clears the name that the rider set.

### 9.4 App rules

1. **Validate.** The link is valid when the host is `openbikecomputer.com`, the path is `/pair/`,
   and the query has exactly one `s` item that matches `[0-9A-F]{16}`. The app ignores query items
   with other names. For an invalid link, the app tells the rider that the code is not an OBC
   pairing code, and it does not scan.
2. **Scan.** The app scans for the OBC Control service UUID. A candidate is a device whose
   advertised local name is `OBC-` followed by the last four digits of `s`.
3. **Match.** The app connects to a candidate and reads the Serial Number String (§3.1). It does no
   gated operation first. When the value equals `s`, the app starts pairing (§8). When it does not,
   the app disconnects and ignores that device for the rest of the scan.
4. **Not found.** When the scan window ends without a match, the app tells the rider that it did
   not find the OBC and offers a new scan.

The app never pairs with a device whose serial it did not match to `s`. After a match, a pairing
failure is as §8 specifies: a wrong passkey and a bonded device look the same to the app.

### 9.5 When the app is not installed

The phone opens the link in the browser. `https://openbikecomputer.com/pair/` serves one static page
for each query. The page:

- tells the rider that the code is for the OBC companion app, and how to install the app;
- tells the rider to scan the code on the OBC again after the install, because iOS does not give
  the link to an app that it installs later;
- does not store or send `s`.

### 9.6 Associated domains

A universal link opens the app only when the app and the domain each name the other.

- **App entitlement.** `com.apple.developer.associated-domains` holds
  `applinks:openbikecomputer.com`.
- **Domain file.** `https://openbikecomputer.com/.well-known/apple-app-site-association`, with no
  file extension. The server sends it over HTTPS with a valid certificate, with status 200, with no
  redirect, and with `Content-Type: application/json`.

```json
{
  "applinks": {
    "details": [
      {
        "appIDs": ["<TEAM_ID>.com.openbikecomputer.companion"],
        "components": [{ "/": "/pair/" }]
      }
    ]
  }
}
```

`<TEAM_ID>` is the Apple Developer team id that signs the release app. The component matches only
the path `/pair/`, so every other page of the site opens in the browser. The file does not limit
the query, because the app validates it (§9.4). iOS gets the file through the Apple CDN when it
installs or updates the app, so a change to the file does not reach a phone immediately.
