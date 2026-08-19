# Shared wire-protocol test vectors (S0)

Binary fixtures pinning the byte layouts of
[`obc-ble-interface-spec.md`](../obc-ble-interface-spec.md), the OBCR v3
waypoint extension ([`OBCR_Spec.md`](../OBCR_Spec.md)), the device's own
recorded-track log, and the OBCT terrain raster
([`OBCT_Spec.md`](../OBCT_Spec.md)), consumed by **four** implementations:

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

> **FS7.5-c3b removed three files** (OBCM v14, [#1420](https://github.com/timohueser/OpenBikeComputer/issues/1420)).
> `transfer-set-shard.bin`, `transfer-set-terrain.bin` and `transfer-set-manifest.bin` pinned the
> `mapShard` / `terrainShard` / `mapSet` object types of a volume-set upload. A map is one object
> now (`OBCA_Spec.md` §5 is superseded), so they described a transfer no producer will make. They
> outlived FS7.5b because the board still parsed set manifests off cards written before the cut;
> that reader — `obc-formats/src/obcs.rs` and the `sd.rs` machinery behind it — went with the USB
> cutover, and the fixtures went with it. They were not regenerated; they are gone, and the three
> object-type values `17`–`19` are not re-issued to anything else.
>
> `version-read.bin` and `version-read-features.bin` were the other two pending files, and FS7.5b
> re-cut both: their `obcm_version` byte is self-sourced from `obc_formats::obcm::VERSION`, which is
> now `14`.

## Files

| File | Layout | Content |
|---|---|---|
| `route-waypoints.obcr` | OBCR v3 | "Vector Loop", 9-point track at 48°N, 2 waypoints (`Brunnen` @ 0 m with ele 238, `<sym>Drinking Water</sym>` → category 1, 13 m left of travel; `Pass Summit` mid-route without ele, an unmapped `<type>Viewpoint</type>` → generic, on-route) |
| `route-plain.obcr` | OBCR v3 | the same track, no waypoints — must ride identically |
| `track-log.obct` | recorded track log (flat 20-byte records, no header — `obc-formats/src/track.rs`) | 5 points shaped for coverage, not plausibility: two `<trkseg>`s, sensor presence walking all → one-absent → none → power-only → all-zero, one point at negative lat/lon/elevation, plus a **7-byte partial trailing record** (what a power-loss leaves) that the exporter must ignore |
| `track-export.gpx` | GPX 1.1, `obc_route::track_to_gpx` | the export of `track-log.obct` as "Schauinsland & back" — the name's `&` pins XML escaping. Not spec-derived: the exporter's serialization *is* the contract, so this file is its output, and its value is cross-implementation |
| `ride-v1.bin` | ride object v1 (spec §7.2) | "Höhenweg", 3 points, the last without elevation |
| `ride-v2.bin` | ride object v2 (spec §7.2, epic #707) | "Sensor Ride", 3 points, BLE-sensor summary + per-point hr/cad/pwr with mixed present/absent (0xFF/0xFFFF sentinels) — the app must accept v1 **and** v2 |
| `config-v1.bin` | Config v1 (spec §7.3) | name "OBC Tourer", metric, **no** trailing refresh byte — what an app predating WX3 writes to rename the device. The absent field must read as *unspecified* (device default), never `Off` |
| `config-weather-refresh.bin` | Config §7.3 + the WX3 (#1188) trailing field | name "OBC Alpine", **imperial**, `weather_refresh` = `3` (60 min). The pair with `config-v1.bin` is the fixture: the same object one appended byte apart, so an off-by-one — which reads the shorter file correctly — fails here. Imperial on purpose, so the new byte follows a *nonzero* `units` rather than a zero a misaligned reader could take for padding |
| `config-weather-refresh-unknown.bin` | Config §7.3 + a WX3 trailing field naming an interval **no version defines** | name "OBC Horizon", metric, `weather_refresh` = `200`. One blob, read twice, two right answers (§11.8, #1214): as a phone → device **write** it is **rejected** — a device cannot honour an interval it does not know, and storing the default, `Off`, or the previous value would all tell the rider their choice was applied when it was discarded. As a device → phone **read** it is **unknown, never fatal** — a value arriving *from* a device is a *newer* device, and a direction-blind reject would mean appending a fifth interval broke every shipped app, down to renaming it. Unknown is its own state: not `Off`, not the default, and distinguishable from *absent*, so the raw byte is kept and round-trips verbatim. Metric on purpose — an off-by-one reader lands on that zero and decodes a perfectly *known* `Off`, so the misalignment is a wrong answer rather than another "unknown" the tolerant path would have swallowed. The 11-byte name is a third distinct length across the three Config fixtures |
| `version-read.bin` | `protocolVersion` read §1 | the **7-byte** identity read: `version u16` = 2 · `store_epoch u32` = `0xA1B2C3D4` · `obcm_version u8` = 14 (re-cut by FS7.5b, see the note above). The last byte is the OBCM **map-format** version the device's reader reads — a different number in a different sequence from the protocol version beside it — and it is **self-sourced** from `obc_formats::obcm::VERSION`, so an OBCM bump re-cuts this file on purpose. Since WX3 this is what a device *without* the weather contract serves: the capability word is absent, so the phone offers no weather |
| `version-read-features.bin` | `protocolVersion` read §1, WX3 #1188 | the **11-byte** read: the layout above plus `feature_bits u32` = `0x1` (`weather`). Its `obcm_version` is 14, re-cut by FS7.5b — see the note above. The only read that entitles a phone to look for the Weather Request service. The word is an **append** — the protocol version underneath it does not move — and the epoch (`0xC0DEF00D`) deliberately differs from the other three reads' so a consumer that opens the wrong file fails on the epoch, not only on the feature bit. A read of 8–10 bytes is a broken `u32`, not a smaller capability set: it decodes as *absent* |
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
| `command-set-route-retention.bin` | `setRouteRetention` §4.4 cmd 6 | route id 7 · retention `3` (2 weeks) |
| `route-list.bin` | `routeList` object §7.4 | three catalog entries — the two stored route fixtures (ids 7 + 8, fields from their OBCR headers, each with its whole-object `crc32`) plus a synthetic id 9 with no file behind it; **6-byte v2 header** (`total` = `count` = 3) + **84-byte entries** (the 76-byte v2 core + the auto-expiry tail), covering all three retention states: a live countdown, a clock not yet started, and `Never` |
| `update-container-v1.bin` | OBCU container ([`OBCU_Spec.md`](../OBCU_Spec.md) §1), **unsigned/v1** | a full `UPDATE.BIN` / `fwImage` payload (§7.6, id 0): 64-byte header (`fw_version` `1.2.0+abc1234`, `image_len` 128) + a 128-byte raw image. Decoded by `obc-dfu` (`cargo test -p obc-dfu --test vectors`) and the iOS `OBCUHeader`. Kept even though a v2 device refuses to *install* it: it is still the shape of a fielded container and of the device-written `ROLLBACK.BIN`, and pairing it with the v2 file below is what pins the offset-compatibility guarantee across implementations |
| `update-container-v2.bin` | OBCU container (§1), **Ed25519-signed/v2** (#997) | the *same* header table and the *same* 128-byte image as v1 — `header_version` still `1`, deliberately (§1.2) — with `sig_scheme`/`sig_len` in v1's reserved bytes `48..52` and a 64-byte signature trailer after the image. Signed with the committed **test** key (`firmware/obc-dfu/keys/test/`); signing is deterministic, so this is a stable file. A decoder must read every v1 field from it byte-identically |
| `trip-v1.bin` | trip object v1 (spec §7.7) | "Alpen Traverse", 3 stages referencing route ids 7 + 8 (the two `route-list.bin` entries) **plus one deliberately dangling id (99)** — pins read-tolerance; 56-byte header + 2 bytes/stage |
| `terrain-shard.obcd` | OBCT container ([`OBCT_Spec.md`](../OBCT_Spec.md) §4) — the filename says "shard", a role that retires with `OBCA_Spec.md` §5; the **bytes are unaffected**, since an assembly's raster is the same container wherever it is carried, and the file keeps its name | a 2 × 2 cell rectangle at ≈ 46.97°N / 7.98°E over a **plane** (`100 + 3·di + 5·dj` m), with the far cell **absent** (the `0` directory sentinel) and one `NODATA` sample. Posting is the v1 `2^9`; the cell is `2^14` — deliberately not the v1 `2^19`, because a v1 cell is 2 MiB of raster and the point of both being header data is that a small one is equally legal. A plane is an *oracle*: bilinear interpolation over one has a closed form, so a second implementation checks itself against arithmetic rather than a reference table, and the differing coefficients (3 vs 5) catch a transposed lat/lon |
| `weather-request-context-full.bin` | `weatherRequestContext` v1 (§11, WX3 #1188) | the 52-byte read a rider mid-ride produces: every validity bit set (position at Freiburg 47.999008°N / 7.842104°E, bearing 342°, 7.1 m/s, active route id 7 — the one `route-list.bin` catalogs), `reason` = `scheduled`, refresh 30 min. The `bundle_*` group identifies a bundle that **exists**: generation, `generated_at` and the whole-object CRC-32 of `weather-dwd-96x96-9f.obcw`, and `fix_utc` is exactly one 30-minute interval past it, so the scheduled reason is arithmetic the same file supports |
| `weather-request-context-empty.bin` | `weatherRequestContext` v1 (§11) | the resting value the attribute holds between requests, so an out-of-turn read gets a valid "nothing is due" instead of the last ride's coordinates. **Not** all-zeroes: version `1` and the default 30-minute refresh are still stated — an all-zero attribute would decode as layout version 0 with weather switched off, and neither is true |
| `weather-request-context-no-fix.bin` | `weatherRequestContext` v1 (§11) | the opposite corner: `reason` = `urgent \| no_bundle`, no validity bits, a nonzero `request_id`, and a *scheduled* refresh of `Off`. Two rules in one file — absence is a **cleared flag**, so the zero coordinates must never put a rider at 0°N 0°E holding generation 0; and `Off` configures the schedule, not the right to ask. The request remains useful for diagnostics/retry, but the current companion returns `noPosition` until the device supplies a fix. |
| `weather-request-context-unknown-refresh.bin` | `weatherRequestContext` v1 (§11.8, #1214) | the day a firmware appends a fifth refresh interval, this is the byte every phone already in the field receives: `refresh` = `9`, which v1 never defined. A read may **never** treat it as fatal — an unrecognised value here is newer firmware, not a malformed device, exactly as an unrecognised `reason` bit is. It decodes as *unknown* (not `Off`, not the default) and the raw byte round-trips verbatim. The file is `weather-request-context-full.bin` at every offset but two — the refresh byte and the request-id nonce — so the rule is checkable by byte comparison: an interval a build cannot name costs it the schedule and nothing else, not the fix, the route or the bundle identity |
| `weather-request-context-southern.bin` | `weatherRequestContext` v1 (§11) | sign coverage, shaped for **coverage not plausibility** like `track-log.obct`: no other fixture carried a negative coordinate or a pre-1970 time, so until this one a mirror could read `lat_udeg`/`lon_udeg` as `u32` and both timestamps as `u64` and pass the whole suite. El Chaltén, Patagonia — southern *and* western — with a fix at 1938-04-24T22:13:20Z and an older bundle an hour before it. Read unsigned those become ≈ 4245°N and a clock 585 billion years ahead: visibly impossible rather than subtly wrong. The two `i64`s sit at different offsets, so one correct sign extension cannot cover for the other, and the bundle group runs the trap the other way — `generation` and `crc32` both have their top bit set, so a *signed* read gets `-2` and `-2147483647` |
| `trip-list.bin` | `tripList` object §7.4 | one entry for the trip above: **6-byte v2 header** + a **76-byte** entry mirroring `routeList` (trailing whole-object `crc32`); `total_distance_m`/`total_ascent_m` (4414 / 152) summed over the two **resolvable** stages, `stage_count` 3 counts every stored stage (dangling included) |

### OBCW weather vectors

The eight positive `.obcw` files pin [`OBCW_Spec.md`](../OBCW_Spec.md): hourly-only dry,
96 × 96 × nine-frame DWD shape, coarse native model times, a genuine four-hour-latent observation
before the current hourly base, all-no-data, raw4, RLE4, and the exact 262,144-byte
producer-policy boundary (raised from 65,536 by WXR5 #1244; it is a phone policy, not a format
limit). The DWD-shaped raw object is 46,480 bytes (45.39 KiB).

The thirteen `weather-invalid-*` files isolate truncation, a bad section offset, section overlap,
nonzero hourly flags/reserved bytes, a reserved intensity nibble, RLE expansion beyond 256 cells,
a compressible tile mislabeled raw4, noncanonical split RLE runs, CRC mismatch, and timestamp
disorder, including nonpositive and after-ceiling frame times. Except for truncation and the
deliberate CRC mismatch, their CRCs are recomputed so structural validation cannot hide behind the
integrity check.

`manifest.json` records each positive's internal CRC, SHA-256, shape, semantic seed and exact
producer/consumer paths. Rust builds them through the `obc-formats` authority and reads them
through the allocator-free `obc-weather` seam. The independent Swift `OBCWeatherWire` codec
decodes and re-encodes every positive byte-for-byte and rejects every negative.

### OBCG grid vectors

The ten positive `grid-*.obcg` files pin [`OBCG_Spec.md`](../OBCG_Spec.md): an all-dry
sentinel-only object; one tile per §5 codec — raw4 on pseudo-random cells, RLE4 winning a tie
against deflate4, RLE4 winning outright, and deflate4 on 64 × 64 upsampled coarse data; the same
deflate4 tile again with the padding bits of its final byte flipped (a second legal byte image,
there to prove a decoder must *not* reject it); a 256 × 256 frame at tile edge 256, WXR1's
production geometry and the only payload longer than 255 bytes; an explicit all-no-data tile
(unavailable is never the dry sentinel); a 3 × 3-tile object across five directory pages with
last-page padding (the corridor request-accounting target); and edge-tile no-data padding. Cells
the Rust tests pin are the cells the Swift decoder must reproduce — OBCG is decoder-mirrored, not
re-encoded, because its only producer is the Rust baker — and each positive's codec id is pinned
too, because §5's choice rule is what keeps RLE4 alive where deflate4 loses.

Every vector is a window of the **one published lattice** — 10,000 × 10,000 µdeg cells,
`cell_size_m = 1113` — and none of them carries provenance: #1246 deleted the product id and tier
from the header, so bytes 12–13 are reserved and zero in every file above.

The twenty-five `grid-invalid-*` files isolate truncation, all four CRC scopes (header, object,
page, tile), a shifted payload offset, an aliased/overlapping payload, impossible dimensions, a
non-power-of-two tile edge, zero entries per page, a codec id outside the closed set, overlong
and noncanonical RLE, a compressible raw4 payload, five deflate4 failures (a truncated stream, a
stream that over-inflates past the tile's raw4 size, one that under-inflates, one whose match
distance reaches before the start of the tile's image, and a valid stream that fails to beat the
canonical raw4/RLE4 length), an encoded all-dry full tile (the sentinel is mandatory there), a dry
sentinel at a partial edge tile (forbidden — padding is no-data, never dry), and a nonzero dry
sentinel, reserved byte, nonzero bytes 12–13 (the deleted provenance pair, which is a malformed
object rather than a code a reader must tolerate), and a double source-class flag. Except for
truncation and the deliberate stale-CRC files, every CRC covering a corrupted byte is recomputed so
structural validation cannot hide behind an integrity check.

### The shared weather manifest (`wx-manifest-v2.json`)

`wx-manifest-v2.json` is the first **manifest** ever cross-pinned between the two client
implementations. Until WXR4 (#1243) only the `.obcg`/`.obcw` byte vectors were shared: the Swift
suite synthesised its own manifests, so the two parsers of the one document every rider reads first
could drift without a test noticing.

It is a complete [`OBCG_Spec.md` §10](../OBCG_Spec.md) v2 document over the production canonical
lattice — 36,000 × 18,000 cells at 0.01°, a 6 × 4 grid of 6,144 × 4,608-cell shards, nine frames at
15 minutes. Its object lengths and CRCs are deterministic placeholders rather than a recording of a
live bake, because what this fixture pins is the *document* contract; the bytes are pinned by the
`grid-*.obcg` vectors above. It carries deliberate presence holes — f0 omits shards (2,0) and (3,0),
f120 omits (5,3) — so the present / dry / out-of-domain trichotomy is exercised rather than assumed,
and exactly one shard (f0's (3,2)) is observed. Coordinates throughout are microdegrees in the
−180..180 / −90..90 convention; `west > east` means an antimeridian crossing and every other
spelling of an out-of-range coordinate is an error rather than a clamp
([`OBCG_Spec.md` §10.2a](../OBCG_Spec.md)).

Three obligations, recorded in `manifest.json`'s `wx_manifest_v2` block:

- the baker parses it back through its own `deny_unknown_fields` **writer** model, so a field the
  fixture invents or misspells fails loudly instead of being silently ignored by the two lenient
  readers;
- both clients derive the **same shard key set from the same bbox** — the `bbox_equivalence` cases
  are the pinned answers, and that equivalence is what replaced product selection. The ten cases are
  chosen to be the geometry a second implementation can get wrong while passing everything else: an
  exact shard boundary (the half-open rule), a southern-hemisphere corridor, an antimeridian wrap
  (`west > east`), the polar band outside `covered_rows`, and three bboxes that must be **refused**
  rather than clamped — wholly off the lattice, a 0..360 longitude, and one reaching past a pole.
  Each case pins the shard set, the composed f0 keys, and the plan's outcome, because "no objects" is
  several different answers and only one of them is about rain;
- a listed-but-missing shard is an error, a bitmap-absent shard is dry, and a shard off the grid is
  out of domain. A 404 is never dry, in either language.

WXR5 (#1244) added two more blocks beside those, both driven from `manifest.json` by both suites so
neither language can quietly test a different list.

**`rejection_equivalence`** — 28 hostile mutations of the fixture, each with a verdict both clients
must reach. `bbox_equivalence` pins what the two readers *compute*; this pins what they **refuse**,
which is where two JSON stacks actually rot apart. It exists because a review ran a corpus like it
through both readers and found five documents they answered differently and three that crashed one
of them outright — an unbounded `width` overflowing a shard-grid division, a shard count overflowing
its own multiplication, and a `present` string with a non-ASCII character being sliced by byte. A
manifest is the first thing a phone fetches from a network nobody controls, so *does not crash* is
the floor and *answers identically* is the contract. The cases also pin the type coercions the
document/entry strictness split could not state: an integral float is not an integer version, an
explicit `null` is not an absent key, a space is not the `T` in RFC 3339, and a `+` is not a hex
digit. Each case is `{name, why, patch, verdict}` where the patch is a list of `{op, path, value}`
over RFC 6901 pointers, and each suite applies it with its own small walker — deliberately
hand-rolled on both sides, because a JSON-Patch library against a hand walker is the asymmetry that
makes a "cross-language" fixture test one language.

**`resample_equivalence`** — nine latitudes of WXR5's uniform east-west resample, pinned by output
rather than by arithmetic: source columns, output window, exact bundle length, and an FNV-1a 64 of
frame 0's decoded cells. The hash is the load-bearing column. At the raw4 worst case every tile is
128 bytes whatever is in it, so byte lengths alone would let a half-cell drift in the
nearest-neighbour column map move which cells a rider sees while every length still matched. FNV-1a
is chosen because it is four lines in any language: a hash needing a library is a hash one of the
two suites quietly skips.

Because §5 leaves compressed bytes to the encoder, the deflate4 fixtures' root of trust is the
exact `miniz_oxide` version the workspace lock pins (`=0.8.9` in `firmware/obc-formats` and
`host/obc-vectors`). Moving it is a fixture regeneration in the same commit, never a lockfile
refresh: a different compressor produces a different — equally valid — object, which every
sha256 here would report as a failure.

`manifest.json` restates each fixture's expected decoded values (plus the pinned
protocol version, the service/characteristic UUIDs — including the Weather
Request service's own 128-bit base — the `feature_bits` assignments, and the
CRC-32 check value) so a test suite can assert against data instead of
hard-coding.

## Regenerating

The builders live in `host/obc-vectors` (the route vectors go through the real
GPX→OBCR converter; everything else is built from spec constants). After a
**deliberate** spec change:

```bash
cargo test -p obc-vectors regenerate -- --ignored
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
