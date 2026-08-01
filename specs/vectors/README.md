# Shared wire-protocol test vectors (S0)

Binary fixtures pinning the byte layouts of
[`obc-ble-interface-spec.md`](../obc-ble-interface-spec.md), the OBCR v3
waypoint extension ([`OBCR_Spec.md`](../OBCR_Spec.md)), and the device's own
recorded-track log, consumed by **four** implementations:

- **Firmware**: `cargo test -p obc-vectors` (workspace `firmware/`) verifies every
  file byte-for-byte against builders written straight from the spec text, and
  loads the route vectors through the production `obc-route` reader.
- **App**: the `OBCKit` Swift tests pin their codecs (`ProvisionalRideCodec`,
  `ProvisionalConfigCodec`, `TransferDescriptor`, the OBCR route encoder) to the
  same files.
- **Browser**: two consumers, for two different reasons.
  - The wasm conversion bridge (`apps/obc-web-convert`, #896) must reproduce
    the route and track fixtures byte-for-byte from the same inputs —
    `builder/app/src/lib/convert/bridge.test.ts`. That is what keeps
    client-side conversion honest: the file a visitor downloads is the file the
    device would have written.
  - The **USB protocol client** (`builder/app/src/lib/usb/`, #902)
    pins every control-plane and object layout here, and round-trips the object
    fixtures over a loopback transport —
    `.../src/lib/usb/vectors.test.ts`. USB is a second *transport*, not a second
    protocol, so it agrees with these bytes or it is wrong.

A drift on any side fails that side's tests — the files are the contract.

## Files

| File | Layout | Content |
|---|---|---|
| `route-waypoints.obcr` | OBCR v3 | "Vector Loop", 9-point track at 48°N, 2 waypoints (`Brunnen` @ 0 m with ele 238, `<sym>Drinking Water</sym>` → category 1, 13 m left of travel; `Pass Summit` mid-route without ele, an unmapped `<type>Viewpoint</type>` → generic, on-route) |
| `route-plain.obcr` | OBCR v3 | the same track, no waypoints — must ride identically |
| `track-log.obct` | recorded track log (flat 20-byte records, no header — `obc-formats/src/track.rs`) | 5 points shaped for coverage, not plausibility: two `<trkseg>`s, sensor presence walking all → one-absent → none → power-only → all-zero, one point at negative lat/lon/elevation, plus a **7-byte partial trailing record** (what a power-loss leaves) that the exporter must ignore |
| `track-export.gpx` | GPX 1.1, `obc_route::track_to_gpx` | the export of `track-log.obct` as "Schauinsland & back" — the name's `&` pins XML escaping. Not spec-derived: the exporter's serialization *is* the contract, so this file is its output, and its value is cross-implementation |
| `ride-v1.bin` | ride object v1 (spec §7.2) | "Höhenweg", 3 points, the last without elevation |
| `ride-v2.bin` | ride object v2 (spec §7.2, epic #707) | "Sensor Ride", 3 points, BLE-sensor summary + per-point hr/cad/pwr with mixed present/absent (0xFF/0xFFFF sentinels) — the app must accept v1 **and** v2 |
| `config-v1.bin` | Config v1 (spec §7.3) | name "OBC Tourer", metric |
| `version-read.bin` | `protocolVersion` read §1 | the **full** identity read: `version u16` = 2 · `store_epoch u32` = `0xA1B2C3D4` · `obcm_version u8` = 11. The last byte is the OBCM **map-format** version the device's reader reads — a different number in a different sequence from the protocol version beside it — and it is **self-sourced** from `obc_formats::obcm::VERSION`, so an OBCM bump re-cuts this file on purpose |
| `version-read-noobcm.bin` | `protocolVersion` read §1 | the **6-byte** read a firmware predating `obcm_version` serves. The epoch is present (the ack gate is open); the trailing field must decode to *unknown*, never `0` — `obcm_version` 0 would read as "supports OBCM v0" and refuse every real map |
| `version-read-nostore.bin` | `protocolVersion` read §1 | the **2-byte** read a device with no mounted card serves: `version u16` = 2 and nothing else. A reader must take it as "no epoch" — never epoch `0`, which is a legal era — and fail its ack closed. No epoch also means no room for the `obcm_version` after it |
| `transfer-upload-start.bin` | `transferControl` §4.2 | fresh route upload, id `0xFFFF` (new); **12-byte v2 descriptor** (no `offset`); `total_len`/`crc32` are the **actual** length + CRC-32 of `route-waypoints.obcr` |
| `transfer-download-request.bin` | `transferControl` §4.2 | download request for the `rideList` object (12 bytes) |
| `transfer-set-shard.bin` | `transferControl` §4.2 | a **volume-set shard** upload (§4.1, #1039): type `17` `mapShard`, and the one `object_id` on this wire that is not an object id — the packed part `(shard_count << 8) \| index` = `0x0802`, shard 2 of 8. Count in the *high* byte; the fixture exists because that is the half of the rule three implementations would otherwise each re-derive from prose. `total_len`/`crc32` are `route-waypoints.obcr`'s, as the other descriptors' are |
| `transfer-set-manifest.bin` | `transferControl` §4.2 | the **set manifest** upload: type `18` `mapSet`, new-only (`0xFFFF`), and written **last** (`OBCA_Spec.md` §5.4). `total_len` = `72 + 56 × 8` = 520, the length a manifest's shard count fixes and a device checks at the announce |
| `transfer-abort.bin` | `transferControl` §4.2 | abort of the active upload (12 bytes) |
| `status-download-announce.bin` | `status` msg 4 §4.3 | the download announce — `msg` byte + the 12-byte descriptor (`op` = download, id 7, size + CRC of `route-waypoints.obcr`); protocol v2 moves the announce off `transferControl` |
| `status-transfer-result.bin` | `status` msg 1 §4.3 | `committed`, assigned id 7, all bytes durable |
| `status-transfer-storage-full.bin` | `status` msg 1 §4.3 | `storageFull` (6) — new-route upload (id `0xFFFF`) rejected at descriptor-open time, catalog full, nothing committed |
| `status-store-changed.bin` | `status` msg 2 §4.3 | route store changed, revision 42 |
| `status-command-result-ack.bin` | `status` msg 3 §4.3 | the answer to an `ackRides`: `ok`, `detail` = 3 newly-flagged rides |
| `command-ack-rides.bin` | `ackRides` §4.4 cmd 2 | `count` 3 · ride ids 3, 5, 9 |
| `command-set-clock.bin` | `setClock` §4.4 cmd 5 | `utc` 1783598400 (2026-07-09T12:00:00Z) · `offset_min` 120 |
| `command-set-route-retention.bin` | `setRouteRetention` §4.4 cmd 6 | route id 7 · retention `3` (2 weeks) |
| `route-list.bin` | `routeList` object §7.4 | three catalog entries — the two stored route fixtures (ids 7 + 8, fields from their OBCR headers, each with its whole-object `crc32`) plus a synthetic id 9 with no file behind it; **6-byte v2 header** (`total` = `count` = 3) + **84-byte entries** (the 76-byte v2 core + the auto-expiry tail), covering all three retention states: a live countdown, a clock not yet started, and `Never` |
| `update-container-v1.bin` | OBCU container ([`OBCU_Spec.md`](../OBCU_Spec.md) §1), **unsigned/v1** | a full `UPDATE.BIN` / `fwImage` payload (§7.6, id 0): 64-byte header (`fw_version` `1.2.0+abc1234`, `image_len` 128) + a 128-byte raw image. Decoded by `obc-dfu` (`cargo test -p obc-dfu --test vectors`) and the iOS `OBCUHeader`. Kept even though a v2 device refuses to *install* it: it is still the shape of a fielded container and of the device-written `ROLLBACK.BIN`, and pairing it with the v2 file below is what pins the offset-compatibility guarantee across implementations |
| `update-container-v2.bin` | OBCU container (§1), **Ed25519-signed/v2** (#997) | the *same* header table and the *same* 128-byte image as v1 — `header_version` still `1`, deliberately (§1.2) — with `sig_scheme`/`sig_len` in v1's reserved bytes `48..52` and a 64-byte signature trailer after the image. Signed with the committed **test** key (`firmware/obc-dfu/keys/test/`); signing is deterministic, so this is a stable file. A decoder must read every v1 field from it byte-identically |
| `trip-v1.bin` | trip object v1 (spec §7.7) | "Alpen Traverse", 3 stages referencing route ids 7 + 8 (the two `route-list.bin` entries) **plus one deliberately dangling id (99)** — pins read-tolerance; 56-byte header + 2 bytes/stage |
| `trip-list.bin` | `tripList` object §7.4 | one entry for the trip above: **6-byte v2 header** + a **76-byte** entry mirroring `routeList` (trailing whole-object `crc32`); `total_distance_m`/`total_ascent_m` (4414 / 152) summed over the two **resolvable** stages, `stage_count` 3 counts every stored stage (dangling included) |

`manifest.json` restates each fixture's expected decoded values (plus the pinned
protocol version, UUIDs, and the CRC-32 check value) so a test suite can assert
against data instead of hard-coding.

## Regenerating

The builders live in `host/obc-vectors` (the route vectors go through the real
GPX→OBCR converter; everything else is built from spec constants). After a
**deliberate** spec change:

```bash
cd firmware && cargo test -p obc-vectors regenerate -- --ignored
```

…then update `manifest.json` to match and flag the app side **and** the web
builder's two consumers — the conversion bridge and the USB protocol client. All
of them pin the same bytes.

Two builder inputs are **not** literals. `update-container-v2.bin`'s 64-byte trailer
comes from `obc_dfu::sign_image` — a signature is the one thing a builder cannot
transcribe from spec prose — and that signer is deterministic on purpose, so the
fixture is a fixed file rather than one that re-cuts on every regeneration. The
other:

`version-read.bin`'s `obcm_version` comes
from `obc_formats::obcm::VERSION`. That is deliberate — the fixture's job is to be
the bytes a current device serves, and a device that reads OBCM v11 saying "10"
would be a lie three implementations agreed on. So an OBCM format bump fails
`cargo test -p obc-vectors`, and the regeneration walks you past the Swift and TS
assertions on that number, which is exactly the review this change wants.
