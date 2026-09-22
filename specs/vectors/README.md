# Shared wire-protocol test vectors (S0)

Binary fixtures pinning the byte layouts of
[`obc-ble-interface-spec.md`](../obc-ble-interface-spec.md), the OBCR route format
([`OBCR_Spec.md`](../OBCR_Spec.md)), the 20-byte ride-sample codec and complete ride-v3
object, and the OBCT terrain raster ([`OBCT_Spec.md`](../OBCT_Spec.md)), consumed by
**four** implementations:

- **Firmware**: `cargo test -p obc-vectors` (workspace `firmware/`) verifies every
  file byte-for-byte against builders written straight from the spec text, and
  loads the route vectors through the production `obc-route` reader.
- **App**: the `OBCKit` Swift tests pin the config, command, route, ride, trip and update
  codecs to the same files.
- **Browser**: two consumers.
  - The wasm conversion bridge (`apps/obc-web-convert`) must reproduce the route and
    finished-ride fixtures byte-for-byte from the same inputs —
    `builder/app/src/lib/convert/bridge.test.ts`.
  - The **USB protocol client** (`builder/app/src/lib/usb/`) pins the protocol-v4
    control and stream layouts under `flat-store-v4/` —
    `.../src/lib/usb/vectors.test.ts`.

A drift on any side fails that side's tests — the files are the contract.

## Files

| File | Layout | Content |
|---|---|---|
| `route-waypoints.obcr` | OBCR v4 | "Vector Loop", 9-point track at 48°N, 2 waypoints (`Brunnen` @ 0 m with ele 238, `<sym>Drinking Water</sym>` → category 1, 13 m left of travel; `Pass Summit` mid-route without ele, an unmapped `<type>Viewpoint</type>` → generic, on-route) |
| `route-plain.obcr` | OBCR v4 | the same track, no waypoints — must ride identically |
| `route-visit.obcr` | OBCR v4, visit descriptor ([`OBCR_Spec.md`](../OBCR_Spec.md) §6) | a valid 80-byte accepted-visit envelope |
| `route-visit-waypoint-overlap.obcr`, `route-visit-index-overlap.obcr` | OBCR v4 §1.1 envelope | each shares the descriptor's last four reserved-zero bytes with another section; both codecs MUST reject them |
| `track-log.obct` | sample-codec vector (five complete 20-byte records, no header) | shaped for sensor/signed-coordinate coverage only; it is not accepted as a ride or recovery input |
| `track-export.gpx` | GPX 1.1, `obc_route::track_to_gpx` | the export of finished `ride-v3.bin` as "Schauinsland & back" — the name's `&` pins XML escaping. Not spec-derived: the exporter's serialization *is* the contract, so this file is its output, and its value is cross-implementation |
| `ride-v3.bin` | ride object v3 (spec §7.2) | "Sensor Ride": three exact 20-byte recorded samples (including segment flags and mixed sensor presence), followed by the fixed 84-byte summary footer |
| `status-command-result.bin` | `status` msg 3 §4.3 | the answer to an accepted `installFw`: `cmd` 3, `ok`, `detail` 0. Pins the four-byte layout and the position of `detail` |
| `command-set-clock.bin` | `setClock` §4.4 cmd 5 | `utc` 1783598400 (2026-07-09T12:00:00Z) · `offset_min` 120 |
| `update-container-v1.bin` | OBCU container ([`OBCU_Spec.md`](../OBCU_Spec.md) §1), **unsigned/v1** | a full `UPDATE.BIN` / `fwImage` payload (§7.6, id 0): 64-byte header (`fw_version` `1.2.0+abc1234`, `image_len` 128) + a 128-byte raw image. Decoded by `obc-dfu` (`cargo test -p obc-dfu --test vectors`) and the iOS `OBCUHeader`. It is the shape of a fielded container and of the device-written rollback snapshot, and pairing it with the v2 file below pins the offset-compatibility guarantee across implementations |
| `update-container-v2.bin` | OBCU container (§1), **Ed25519-signed/v2** | the *same* header table and the *same* 128-byte image as v1 — `header_version` still `1` (§1.2) — with `sig_scheme`/`sig_len` in v1's reserved bytes `48..52` and a 64-byte signature trailer after the image. Signed with the committed **test** key (`firmware/obc-dfu/keys/test/`); signing is deterministic, so this is a stable file. A decoder must read every v1 field from it byte-identically |
| `trip-v3.bin` | trip object v3 (spec §7.7) | "Alpen Traverse", key `0x0123_4567_89AB_CDEF`, start date 20360 (Monday 2025-09-29), 3 days: route 7 ending on the line at 82,000 m, route 8 leaving the line at 73,600 m, and full-width route `0x1_0000_0063` joining at 400 m with `leave_m` `0xFFFF_FFFF`; 64-byte header + 16 bytes/day |
| `terrain-shard.obcd` | OBCT container ([`OBCT_Spec.md`](../OBCT_Spec.md) §4) | a 2 × 2 cell rectangle at ≈ 46.97°N / 7.98°E over a **plane** (`100 + 3·di + 5·dj` m), with the far cell **absent** (the `0` directory sentinel) and one `NODATA` sample. Posting is the v1 `2^9`; the cell is `2^14`, not the v1 `2^19`, because both are header data. A plane is an *oracle*: bilinear interpolation over one has a closed form, so a second implementation checks itself against arithmetic, and the differing coefficients (3 vs 5) catch a transposed lat/lon |

The `place-train-v15.bin` record pins the OBCM v15 service metadata: Train subtype 20,
source identity, explicit approach node and coordinate, and profile mask. Its 64 bytes come
from a separate spec builder and pass through the production metadata decoder.

## Regenerating

The builders live in `host/obc-vectors` (the route vectors go through the real
GPX→OBCR converter; everything else is built from spec constants). After a
**deliberate** spec change:

```bash
cargo run -p obc-vectors --example regenerate --locked
```

…then update `manifest.json` to match and flag the app side **and** the web
builder's two consumers — the conversion bridge and the USB protocol client. All
of them pin the same bytes.

Two builder inputs are **not** literals. `update-container-v2.bin`'s 64-byte trailer comes
from `obc_dfu::sign_image`; that signer is deterministic, so the fixture is a fixed file
rather than one that re-cuts on every regeneration.

`peak-section-v17.bin` is an authored OBCM §10 section with three summit SourceIds and two
article identities. Two summits share one English/German article; the third has French text only.
The reader suite uses these bytes for language fallback, generation changes, bounded binary
search, malformed indexes and valid-range content-reference swaps. The web assembly fixture
passes a separate authored catalogue through the ordinary cutter and native assembler.
