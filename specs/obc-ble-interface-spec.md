# OBC BLE interface

The GATT surface the device serves to the companion app: the two SIG services, the custom OBC
Control service, its live characteristics, and the payload layouts of the objects those
characteristics carry. Pairing and encryption are §8.

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

A route object's payload is exactly the bytes of an OBCR v3 file
([`OBCR_Spec.md`](OBCR_Spec.md)); the device stores and serves it verbatim. An update package's
payload is exactly the bytes of an OBCU container ([`OBCU_Spec.md`](OBCU_Spec.md) §1).

### 7.2 `ride` — ride object v3

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
84 bytes at `object length − 84`; a full reader requires `object length == point_count × 20 + 84`.
Finalize appends this footer and performs one store commit that publishes the final length and CRC
and clears `RECORDING`. `specs/vectors/ride-v3.bin` pins three sample records — including sensor
sentinels and segment flags — and the footer.

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

### 7.7 `trip` — a trip object (v2)

A **trip** groups planned routes into one named unit. It references route object ids in ride order
and never contains route bytes.

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

- **Reference-only.** A stage is a route object id. A route referenced by no stored trip is a
  top-level route, and membership is exactly one level deep — a route lives in at most one trip, or
  standalone.
- **Dangling refs are tolerated on read.** A member route deleted individually does not invalidate
  the trip: the device serves the trip verbatim, dangling ids and all. The device never rewrites a
  stored trip. Dangling refs persist until the next trip upload replaces the object.
- **Uploads commit verbatim.** A trip upload that references unknown route ids is stored as sent;
  validation belongs to the client.
- **Recommended upload order: stages first, the trip object last.** An interrupted whole-trip push
  then never leaves a trip pointing at nothing, and re-running the push is idempotent.
- **Removing a trip removes only the trip object.** Its member routes become top-level routes; the
  wire has no cascading delete. A "delete trip *and* its routes" action is composed by the client as
  individual route removals plus the trip removal.

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
  action in Settings ▸ Bluetooth zeroes the bond slot, removes the peer from the host's bond table
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
