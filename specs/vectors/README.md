# Shared wire-protocol test vectors (S0)

Binary fixtures pinning the byte layouts of
[`obc-ble-interface-spec.md`](../obc-ble-interface-spec.md), the OBCR v3
waypoint extension ([`OBCR_Spec.md`](../OBCR_Spec.md)), the 20-byte ride-sample codec and complete
ride-v3 object, and the OBCT terrain raster
([`OBCT_Spec.md`](../OBCT_Spec.md)), consumed by **four** implementations:

- **Firmware**: `cargo test -p obc-vectors` (workspace `firmware/`) verifies every
  file byte-for-byte against builders written straight from the spec text, and
  loads the route vectors through the production `obc-route` reader.
- **App**: the `OBCKit` Swift tests pin their codecs (`ProvisionalRideCodec`,
  `ProvisionalConfigCodec`, `TransferDescriptor`, the OBCR route encoder) to the
  same files.
- **Browser**: two consumers, for two different reasons.
  - The wasm conversion bridge (`apps/obc-web-convert`, #896) must reproduce
    the route and finished-ride fixtures byte-for-byte from the same inputs —
    `builder/app/src/lib/convert/bridge.test.ts`. That is what keeps
    client-side conversion honest: the file a visitor downloads is the file the
    device would have written.
  - The **USB protocol client** (`builder/app/src/lib/usb/`, #902)
    pins every control-plane and object layout here, and round-trips the object
    fixtures over a loopback transport —
    `.../src/lib/usb/vectors.test.ts`. USB is a second *transport*, not a second
    protocol, so it agrees with these bytes or it is wrong.

A drift on any side fails that side's tests — the files are the contract.

> Object types `17`–`19` (`mapShard` / `terrainShard` / `mapSet`) are retired and MUST NOT be
> re-issued. A map is one object, so no producer makes a volume-set transfer.

## Files

| File | Layout | Content |
|---|---|---|
| `route-waypoints.obcr` | OBCR v3 | "Vector Loop", 9-point track at 48°N, 2 waypoints (`Brunnen` @ 0 m with ele 238, `<sym>Drinking Water</sym>` → category 1, 13 m left of travel; `Pass Summit` mid-route without ele, an unmapped `<type>Viewpoint</type>` → generic, on-route) |
| `route-plain.obcr` | OBCR v3 | the same track, no waypoints — must ride identically |
| `track-log.obct` | sample-codec vector (five complete 20-byte records, no header) | shaped for sensor/signed-coordinate coverage only; it is not accepted as a ride or recovery input |
| `track-export.gpx` | GPX 1.1, `obc_route::track_to_gpx` | the export of finished `ride-v3.bin` as "Schauinsland & back" — the name's `&` pins XML escaping. Not spec-derived: the exporter's serialization *is* the contract, so this file is its output, and its value is cross-implementation |
| `ride-v3.bin` | ride object v3 (spec §7.2) | "Sensor Ride": three exact 20-byte recorded samples (including segment flags and mixed sensor presence), followed by the fixed 84-byte summary footer |
| `version-read-noobcm.bin` | `protocolVersion` read §1 | the **6-byte** read a firmware predating `obcm_version` serves. The epoch is present (the ack gate is open); the trailing field must decode to *unknown*, never `0` — `obcm_version` 0 would read as "supports OBCM v0" and refuse every real map |
| `version-read-nostore.bin` | `protocolVersion` read §1 | the **2-byte** read a device with no mounted card serves: `version u16` = 2 and nothing else. A reader must take it as "no epoch" — never epoch `0`, which is a legal era — and fail its ack closed. No epoch also means no room for the `obcm_version` after it |
| `transfer-upload-start.bin` | `transferControl` §4.2 | fresh route upload, id `0xFFFF` (new); **12-byte v2 descriptor** (no `offset`); `total_len`/`crc32` are the **actual** length + CRC-32 of `route-waypoints.obcr` |
| `transfer-download-request.bin` | `transferControl` §4.2 | download request for the `rideList` object (12 bytes) |
| `transfer-abort.bin` | `transferControl` §4.2 | abort of the active upload (12 bytes) |
| `status-download-announce.bin` | `status` msg 4 §4.3 | the download announce — `msg` byte + the 12-byte descriptor (`op` = download, id 7, size + CRC of `route-waypoints.obcr`); protocol v2 moves the announce off `transferControl` |
| `status-transfer-result.bin` | `status` msg 1 §4.3 | `committed`, assigned id 7, all bytes durable |
| `status-transfer-storage-full.bin` | `status` msg 1 §4.3 | `storageFull` (6) — new-route upload (id `0xFFFF`) rejected at descriptor-open time, catalog full, nothing committed |
| `status-store-changed.bin` | `status` msg 2 §4.3 | route store changed, revision 42 |
| `status-command-result-ack.bin` | `status` msg 3 §4.3 | the answer to an `ackRides`: `ok`, `detail` = 3 newly-flagged rides |
| `command-ack-rides.bin` | `ackRides` §4.4 cmd 2 | `count` 3 · ride ids 3, 5, 9 |
| `command-set-clock.bin` | `setClock` §4.4 cmd 5 | `utc` 1783598400 (2026-07-09T12:00:00Z) · `offset_min` 120 |
| `route-list.bin` | `routeList` object §7.4 | Three routes (ids 7, 8, and 9), with content CRCs; a 6-byte header and 76-byte entries. |
| `update-container-v1.bin` | OBCU container ([`OBCU_Spec.md`](../OBCU_Spec.md) §1), **unsigned/v1** | a full `UPDATE.BIN` / `fwImage` payload (§7.6, id 0): 64-byte header (`fw_version` `1.2.0+abc1234`, `image_len` 128) + a 128-byte raw image. Decoded by `obc-dfu` (`cargo test -p obc-dfu --test vectors`) and the iOS `OBCUHeader`. Kept even though a v2 device refuses to *install* it: it is still the shape of a fielded container and of the device-written `ROLLBACK.BIN`, and pairing it with the v2 file below is what pins the offset-compatibility guarantee across implementations |
| `update-container-v2.bin` | OBCU container (§1), **Ed25519-signed/v2** (#997) | the *same* header table and the *same* 128-byte image as v1 — `header_version` still `1`, deliberately (§1.2) — with `sig_scheme`/`sig_len` in v1's reserved bytes `48..52` and a 64-byte signature trailer after the image. Signed with the committed **test** key (`firmware/obc-dfu/keys/test/`); signing is deterministic, so this is a stable file. A decoder must read every v1 field from it byte-identically |
| `trip-v2.bin` | trip object v2 (spec §7.7) | "Alpen Traverse", 3 stages referencing route ids 7 + 8 **plus one deliberately dangling id (`0x1_0000_0063`)** — pins full-width ObjectId read-tolerance; 56-byte header + 8 bytes/stage |
| `terrain-shard.obcd` | OBCT container ([`OBCT_Spec.md`](../OBCT_Spec.md) §4) — the filename says "shard", a role that retires with `OBCA_Spec.md` §5; the **bytes are unaffected**, since an assembly's raster is the same container wherever it is carried, and the file keeps its name | a 2 × 2 cell rectangle at ≈ 46.97°N / 7.98°E over a **plane** (`100 + 3·di + 5·dj` m), with the far cell **absent** (the `0` directory sentinel) and one `NODATA` sample. Posting is the v1 `2^9`; the cell is `2^14` — deliberately not the v1 `2^19`, because a v1 cell is 2 MiB of raster and the point of both being header data is that a small one is equally legal. A plane is an *oracle*: bilinear interpolation over one has a closed form, so a second implementation checks itself against arithmetic rather than a reference table, and the differing coefficients (3 vs 5) catch a transposed lat/lon |
| `trip-list.bin` | `tripList` object §7.4 | one entry for the trip above: **6-byte v2 header** + a **76-byte** entry mirroring `routeList` (trailing whole-object `crc32`); `total_distance_m`/`total_ascent_m` (4414 / 152) summed over the two **resolvable** stages, `stage_count` 3 counts every stored stage (dangling included) |

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

Two builder inputs are **not** literals. `update-container-v2.bin`'s 64-byte trailer
comes from `obc_dfu::sign_image` — a signature is the one thing a builder cannot
transcribe from spec prose — and that signer is deterministic on purpose, so the
fixture is a fixed file rather than one that re-cuts on every regeneration. The
other:

`version-read.bin`'s `obcm_version` comes
from `obc_formats::obcm::VERSION`. That is deliberate — the fixture's job is to be
the bytes a current device serves, and a device that reads OBCM v14 saying "13"
would be a lie three implementations agreed on. (That example is the v13→v14 bump, which FS7.5b
made: the spec moved first, the constant followed, and the note at the top of this file is what
stopped the gap from being silent while the two were apart.) So an OBCM format bump
fails
`cargo test -p obc-vectors`, and the regeneration walks you past the Swift and TS
assertions on that number, which is exactly the review this change wants.

That walk is only a review if the suites actually run: the Swift assertion sat a
version behind for the whole of v12 because `specs/vectors/**` was missing from
CI's iOS path filter (#1105 added it). A pin nobody runs is a comment.

`peak-section-v17.bin` is an authored OBCM §10 section with three summit SourceIds and two
article identities. Two summits share one English/German article; the third has French text only.
The reader suite uses these bytes for language fallback, generation changes, bounded binary
search, malformed indexes and valid-range content-reference swaps. The web assembly fixture
passes a separate authored catalogue through the ordinary cutter and native assembler.
