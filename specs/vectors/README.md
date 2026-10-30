# Shared wire-protocol test vectors

Binary fixtures pinning the byte layouts of
[`ble-interface.md`](../ble-interface.md), the OBCR route format
([`obcr-route.md`](../obcr-route.md)), the 20-byte ride-sample codec and complete ride-v6
object, and the OBCT terrain raster ([`obct-terrain.md`](../obct-terrain.md)), consumed by
three implementations:

- **Rust**: `cargo test -p obc-vectors` verifies every file it builds byte-for-byte against
  builders written straight from the spec text, and loads the route vectors through the
  production `obc-route` reader. The rows below name the other Rust readers.
- **App**: the `OBCKit` Swift tests pin the command, route, ride, trip and update codecs to the
  same files.
- **Browser**: the wasm conversion bridge (`builder/wasm`) must reproduce the
  route and finished-ride fixtures byte-for-byte from the same inputs —
  `builder/web/src/lib/convert/bridge.test.ts`.

The files are the contract: a drift on any side fails that side's tests.

## Files

| File | Layout | Content |
|---|---|---|
| `route-waypoints.obcr` | OBCR v5 | "Vector Loop", 9-point track at 48°N, 2 waypoints (`Brunnen` @ 0 m with ele 238, `<sym>Drinking Water</sym>` → category 1, 13 m left of travel; `Pass Summit` mid-route without ele, an unmapped `<type>Viewpoint</type>` → generic, on-route) |
| `route-plain.obcr` | OBCR v5 | the same track, no waypoints — must ride identically |
| `route-visit.obcr` | OBCR v5, visit descriptor ([`obcr-route.md`](../obcr-route.md) §6) | a valid 80-byte accepted-visit envelope |
| `route-visit-waypoint-overlap.obcr`, `route-visit-index-overlap.obcr` | OBCR v5 §1.1 envelope | each shares the descriptor's last four reserved-zero bytes with another section; both codecs MUST reject them |
| `eta.csv` | [`obcr-route.md`](../obcr-route.md) §1.2 estimate | one row per case: bike type, distance, ascent and the expected whole seconds, for every bike type. `obc-formats` and the `OBCKit` Swift tests read it |
| `track-log.obct` | sample-codec vector (five complete 20-byte records, no header) | shaped for sensor/signed-coordinate coverage only; it is not accepted as a ride or recovery input |
| `track-export.gpx` | GPX 1.1, `obc_route::track_to_gpx` | the export of finished `ride-v6.bin` as "Schauinsland & back" — the name's `&` pins XML escaping. Not spec-derived: the exporter's serialization is the contract, and this file is its output |
| `ride-v6.bin` | ride object v6 (spec §7.2) | "Sensor Ride": three exact 20-byte recorded samples (including segment flags and mixed sensor presence), followed by the fixed 154-byte summary footer with 95 m of descent and 756 kJ. The ride started on day index 1 of 3 of the `trip-v3.bin` trip ("Alpen Traverse", key `0x0123_4567_89AB_CDEF`), on a Gravel bike, with max HR 185, FTP 250 and the demo flag set |
| `status-command-result.bin` | `status` msg 3 §4.3 | the answer to an accepted `installFw`: `cmd` 3, `ok`, `detail` 0. Pins the four-byte layout and the position of `detail` |
| `config-v1.bin` | `config` §7.3 | name `OBC Tourer`, `units` 0 (metric). `obc-ble` decodes and re-encodes it (`firmware/obc-ble/tests/cases/vectors.rs`) |
| `command-set-clock.bin` | `setClock` §4.4 cmd 5 | `utc` 1783598400 (2026-07-09T12:00:00Z) · `offset_min` 120 |
| `update-container-v1.bin` | OBCU container ([`obcu-update.md`](../obcu-update.md) §1), **unsigned/v1** | a full `UPDATE.BIN` / `fwImage` payload (§7.6, id 0): 64-byte header (`fw_version` `1.2.0+abc1234`, `image_len` 128) + a 128-byte raw image. Decoded by `obc-dfu` (`cargo test -p obc-dfu --test vectors`) and the iOS `OBCUHeader`. It is the shape of a fielded container and of the device-written rollback snapshot. With the v2 file below, it pins the offset compatibility across implementations |
| `update-container-v2.bin` | OBCU container (§1), **Ed25519-signed/v2** | the *same* header table and the *same* 128-byte image as v1 — `header_version` still `1` (§1.2) — with `sig_scheme`/`sig_len` in v1's reserved bytes `48..52` and a 64-byte signature trailer after the image. Signed with the committed **test** key (`firmware/obc-dfu/keys/test/`); signing is deterministic. A decoder must read every v1 field from it byte-identically |
| `trip-v3.bin` | trip object v3 (spec §7.7) | "Alpen Traverse", key `0x0123_4567_89AB_CDEF`, start date 20360 (Monday 2025-09-29), 3 days: route 7 ending on the line at 82,000 m, route 8 leaving the line at 73,600 m, and full-width route `0x1_0000_0063` joining at 400 m with `leave_m` `0xFFFF_FFFF`; 64-byte header + 16 bytes/day |
| `terrain-shard.obcd` | OBCT container ([`obct-terrain.md`](../obct-terrain.md) §4) | a 2 × 2 cell rectangle at ≈ 46.97°N / 7.98°E over a **plane** (`100 + 3·di + 5·dj` m), with the far cell **absent** (the `0` directory sentinel) and one `NODATA` sample. Posting is the v1 `2^9`; the cell is `2^14`, not the v1 `2^19`, because both are header data. Bilinear interpolation over a plane has a closed form for a second implementation to check against; the differing coefficients (3 vs 5) catch a transposed lat/lon |
| `route-answer.json` | route answer ([`route-api.md`](../route-api.md)) | two routes before (`routes`) and after (`answer`) encoding. The primary route has six points, one unknown height, two legs and four edge channels: two `surfaces` runs, three `pushing` runs, three `closures` runs and three `sac_scale` runs. The corridor route has five points, one leg and a `via` point. The `planner-router` encoder test and the decoder tests of the web planner and `OBCKit` read it |
| `signed-routes.json` | route catalog records ([`route-catalog.md`](../route-catalog.md)) and signed-route search queries | 13 records around one start at 47.87°N 8.15°E: hiking and foot routes graded T1 to T5, an ungraded loop, an S2 mountain-bike loop, a bicycle loop, a long route with two stages whose start is 30 km away, a route across a cell edge, and a loop that reaches a cell outside the offline download. A 14th record, a loop that turns back at its own start, lies outside every query and serves only its plan. `grid` lists the cells that have a file; the loader answers null for every other cell. Each query has the cells that the search loads and the expected matches in order. The distance model is a flat projection at the start latitude on a sphere of R = 6371 km. The cell box is the start ± the radius on each axis, and each box edge is at least 0.05 tiles from a tile edge. A route matches when its distance is not more than the radius. `distance_m` is rounded to the metre. Each plan has the plan points and the turnarounds as plan indices (index 0 when a loop turns back at its start). The web planner search test reads it; the phone port reads the same file |
| `trailview-features.json`, `trailview-features.bin` | TrailView feature tile ([`trailview-features.md`](../trailview-features.md)) | one tile decoded and its bytes: a line into the buffer, a line with two parts, solved heights and two heights that are not solved, a line with an alignment in codes that runs against its stroke and a part without one, a line with an alignment and no heights, an area of two polygons with a hole, a point outside the tile square, a non-ASCII tag value and a feature without tags. The encoder test of the bake (`host/obc-trailview-features/src/tiles.rs`) and the engine decoder test (`builder/web/src/lib/trailview/stream/features.test.ts`) read both |
| `place-train-v15.bin` | OBCM POI record ([`obcm-map.md`](../obcm-map.md) §7.3) | one 64-byte Train record, subtype 20, with its source identity, an explicit approach node and coordinate, and a profile mask. `obc-vectors` passes it through the production metadata decoder |
| `landmark-section-v16.bin` | OBCM landmark section (`obcm-map.md` §9) | one landmark, Wikidata item 123 `Burg` at 8°E 47°N, with a German article bundle, no OSM identity and no photo. `obc-vectors` passes the record through the production record decoder |
| `peak-section-v17.bin` | OBCM peak section (`obcm-map.md` §10) | three summit SourceIds and two article identities. Two summits share one English/German article; the third has French text only. The `obc-reader` peak suite (`firmware/obc-reader/tests/cases/peaks.rs`) uses these bytes for language fallback, generation changes, bounded binary search, malformed indexes and valid-range content-reference swaps |
| `flat-store-v4/` | protocol v4 records ([`flat-store-protocol.md`](../flat-store-protocol.md) §3.11) | one JSON file per record in `controls/`, `streams/`, `errors/` and `negative/`, each listed with its SHA-256 in the directory's own `manifest.json`. The `obc-link` unit tests check it against its producer; the `OBCKit` `FlatStoreVectorTests` read it |
| `ride-archive-metadata/two-rides.bin` | `OBRM` v2 ([`ride-archive-metadata.md`](../ride-archive-metadata.md)) | two proof rows on StoreId `42` repeated 16 times. The `obc-storage` metadata tests build the same bytes and refuse corrupted copies; the `obc-link` `flat_engine` test serves it as a device-owned Metadata object |

## Build

The builders live in `host/obc-vectors` (the route vectors go through the real
GPX→OBCR converter; everything else is built from spec constants). After a
**deliberate** spec change:

```bash
cargo run -p obc-vectors --example regenerate --locked
```

Then update `manifest.json` to match and flag the app side **and** the web
builder's two consumers — the conversion bridge and the USB protocol client. All
of them pin the same bytes.

One builder input is **not** a literal. `update-container-v2.bin`'s 64-byte trailer comes
from `obc_dfu::sign_image`; that signer is deterministic, so the fixture does not change on
regeneration.

`flat-store-v4/` has its own producer:
`cargo run -p obc-link --features std --example flat_vectors --locked`. `two-rides.bin` has no
producer; the `obc-storage` metadata test states its bytes.
