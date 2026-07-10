# Shared wire-protocol test vectors (S0)

Binary fixtures pinning the byte layouts of
[`obc-ble-interface-spec.md`](../obc-ble-interface-spec.md) and the OBCR v2
waypoint extension ([`OBCR_Spec.md`](../OBCR_Spec.md)), consumed by **both**
sides of the link:

- **Firmware**: `cargo test -p obc-vectors` (workspace `firmware/`) verifies every
  file byte-for-byte against builders written straight from the spec text, and
  loads the route vectors through the production `obc-route` reader.
- **App**: the `OBCKit` Swift tests pin their codecs (`ProvisionalRideCodec`,
  `ProvisionalConfigCodec`, `TransferDescriptor`, the OBCR route encoder) to the
  same files.

A drift on either side fails that side's tests — the files are the contract.

## Files

| File | Layout | Content |
|---|---|---|
| `route-waypoints.obcr` | OBCR v2 | "Vector Loop", 9-point track at 48°N, 2 waypoints (`Brunnen` @ 0 m with ele 238, `Pass Summit` mid-route without ele) |
| `route-plain.obcr` | OBCR v2 | the same track, no waypoints — must ride identically |
| `ride-v1.bin` | ride object v1 (spec §7.2) | "Höhenweg", 3 points, the last without elevation |
| `config-v1.bin` | Config v1 (spec §7.3) | name "OBC Tourer", metric |
| `transfer-upload-start.bin` | `transferControl` §4.2 | fresh route upload, id `0xFFFF` (new); `total_len`/`crc32` are the **actual** length + CRC-32 of `route-waypoints.obcr` |
| `transfer-upload-resume.bin` | `transferControl` §4.2 | a non-zero-offset upload descriptor — pins the `offset` byte layout (uploads restart, not resume; the device rejects this) |
| `transfer-download-request.bin` | `transferControl` §4.2 | download request for the `rideList` object |
| `transfer-abort.bin` | `transferControl` §4.2 | abort of the active upload |
| `status-transfer-result.bin` | `status` msg 1 §4.3 | `committed`, assigned id 7, all bytes durable |
| `status-transfer-storage-full.bin` | `status` msg 1 §4.3 | `storageFull` (6) — new-route upload (id `0xFFFF`) rejected at descriptor-open time, catalog full, nothing committed |
| `status-store-changed.bin` | `status` msg 2 §4.3 | route store changed, revision 42 |
| `object-store.bin` | `objectStore` §4.5 | revision 42 · 3 routes · 5 rides |
| `route-list.bin` | `routeList` object §7.4 | both stored route fixtures as catalog entries (ids 7 + 8, fields from their OBCR headers) |
| `update-container-v1.bin` | OBCU container ([`OBCU_Spec.md`](../OBCU_Spec.md) §1) | a full `UPDATE.BIN` / `fwImage` payload (§7.6, id 0): 64-byte header (`fw_version` `1.2.0+abc1234`, `image_len` 128) + a 128-byte raw image. Decoded by `obc-dfu` (`cargo test -p obc-dfu --test vectors`) and the iOS `OBCUHeader` |

`manifest.json` restates each fixture's expected decoded values (plus the pinned
protocol version, UUIDs, and the CRC-32 check value) so a test suite can assert
against data instead of hard-coding.

## Regenerating

The builders live in `firmware/obc-vectors` (the route vectors go through the real
GPX→OBCR converter; everything else is built from spec constants). After a
**deliberate** spec change:

```bash
cd firmware && cargo test -p obc-vectors regenerate -- --ignored
```

…then update `manifest.json` to match and flag the app side — its tests pin the
same bytes.
