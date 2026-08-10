# obc-wx-bake event packs

A **event pack** is one real past weather event frozen on disk: the raw bytes the archives served,
the tree the real baker makes of them, and what the radar actually saw over the following two
hours. Packs exist so the simulator and the test suite can run against real radar instead of
synthetic blobs.

```text
wx-events/<event-id>/
  event.json    the pack manifest: window, bake parameters, and per-member provenance
  upstream/     raw archive bytes, byte-identical to what the archive served
  service/      the baked tree: wx/v1/manifest.json + wx/v1/<product>/<gen>/f*.obcg
  truth/        the OBSERVED frames at the truth offsets — ground truth for later scoring
```

Capture, inspect and check a pack with `obc-wx-pack` (a second binary of this crate — see
`src/bin/obc-wx-pack.rs`):

```sh
cargo run --release --bin obc-wx-pack -- capture <event-id> --at <rfc3339> \
      [--out <dir>] [--title <t>] [--region <r>] [--bbox <s,w,n,e>] \
      [--truth-offsets 15,30,...|none] [--store-truth-upstream]
cargo run --release --bin obc-wx-pack -- show   <pack-dir>
cargo run --release --bin obc-wx-pack -- verify <pack-dir>   # digests, then a full re-bake
cargo run --release --bin obc-wx-pack -- rebake <pack-dir>
cargo run --release --bin obc-wx-pack -- fetch  <pack-dir>   # materialize recorded-only members
```

## The two rules that make a pack trustworthy

**`service/` is not a hand-made artifact.** It is what `cycle::run_cycle` writes when its
`Upstream` is a `FixtureUpstream` loaded from `upstream/` — the same offline seam
`tests/cycle.rs` and `tests/us_gfs_cycle.rs` already use. `tests/event_pack.rs` re-bakes every
checked-in pack and byte-compares, so a pack that stops reproducing is a baker regression, loudly.

**The archive is not the upstream.** The baker asks for
`https://noaa-mrms-pds.s3.amazonaws.com/...`, a bucket that retains days rather than years. A 2020
observation therefore comes from Iowa State's MTArchive mirror, and every member records *both*
URLs: `url`, the canonical key the baker requests and the replay serves it under, and
`archive_url`, where the bytes actually came from. The rewrite lives in `src/pack/archive.rs`.

Provenance discipline is `tests/fixtures/README.md`'s, applied per member inside `event.json`:
exact retrieval URL, byte range where one was used, length, sha256, and licence.

## Size discipline

Full-domain packs are hundreds of megabytes, so two things keep a checked-in pack small:

* `--bbox` crops the **baked** output (`src/pack/crop.rs`). The crop window is aligned outward to
  each frame's tile edge, so every retained tile's payload bytes are identical to the uncropped
  object's — a cropped pack is a *subset* of the real bake, not a different bake. The raw
  `upstream/` bytes are never cropped; they must stay byte-identical to what the archive served.
* Members with `"stored": false` are recorded with full provenance but not checked in.
  `obc-wx-pack fetch` materializes them from `archive_url` and refuses anything whose sha256 has
  moved.

---

## `us-derecho-2020-08-10` — the 10 August 2020 Midwest derecho

The 2020-08-10 derecho crossing Iowa, captured at its 18:52 UTC peak. One cycle of the composed
US product (MRMS observation + HRRR subhourly forecast), cropped to Iowa and the Mississippi
valley, plus a two-hour observed ladder. 1.28 MB on disk.

| | |
|---|---|
| cycle wall clock (`--at`) | `2020-08-10T18:52:00Z` |
| product reference time | `2020-08-10T18:52:00Z` (the MRMS observation the cycle discovered) |
| HRRR run selected | 2020-08-10 18Z, subhourly `wrfsubhf01..04` |
| crop (`--bbox`) | `40.5,-96.5,43.5,-90.0` |
| service | 9 frames + manifest, 113,632 bytes |
| truth | 8 observed frames, 241,181 bytes |
| upstream checked in | 12 members, 898,268 bytes |
| upstream recorded only | 8 members, 4,291,446 bytes |

Frame offsets are the real ones: the 18Z HRRR run's 15-minute steps land 8, 23, 38 … 113 minutes
ahead of an 18:52 observation, and nothing is re-spaced onto a round cadence. The observation
frame is 704 x 320 cells at 1 km and 36.9 % wet; the +113 min forecast is 224 x 128 at 3 km.

### Truth ladder

Requested at +15 min steps. MRMS `PrecipRate` publishes every two minutes on even minutes, so odd
requests floor onto the cadence; `event.json` records both numbers and each frame's own OBCG
header carries the real instant.

| requested | actual | valid at | bytes |
|---|---|---|---|
| +15 | +14 | 2020-08-10T19:06:00Z | 35,672 |
| +30 | +30 | 2020-08-10T19:22:00Z | 36,389 |
| +45 | +44 | 2020-08-10T19:36:00Z | 35,686 |
| +60 | +60 | 2020-08-10T19:52:00Z | 32,583 |
| +75 | +74 | 2020-08-10T20:06:00Z | 30,561 |
| +90 | +90 | 2020-08-10T20:22:00Z | 26,718 |
| +105 | +104 | 2020-08-10T20:36:00Z | 23,510 |
| +120 | +120 | 2020-08-10T20:52:00Z | 20,062 |

### Upstream provenance

Retrieved 2026-08-10 UTC. Terms for every member: NOAA Open Data Dissemination (public-use U.S.
government data, no endorsement implied), <https://www.noaa.gov/information-technology/open-data-dissemination>.
The full record — including the HEAD probes the baker's discovery made and the object lengths its
range arithmetic was bounded by — is in `event.json`; this table is the readable tour.

**MRMS PrecipRate (CONUS observation, 1 km / 2 min).** Canonical key
`https://noaa-mrms-pds.s3.amazonaws.com/CONUS/PrecipRate_00.00/20200810/MRMS_PrecipRate_00.00_20200810-HHMMSS.grib2.gz`,
retrieved from
`https://mtarchive.geol.iastate.edu/2020/08/10/mrms/ncep/PrecipRate/PrecipRate_00.00_20200810-HHMMSS.grib2.gz`.

- `upstream/mrms/MRMS_PrecipRate_00.00_20200810-185200.grib2.gz` — 451,427 bytes,
  sha256 `039f88506120178b…` (the cycle's anchor; **checked in**).
- the eight truth observations at 190600, 192200, 193600, 195200, 200600, 202200, 203600 and
  205200 — 465,326 / 492,794 / 521,572 / 542,146 / 550,238 / 559,850 / 573,534 / 585,986 bytes,
  **recorded only**. `obc-wx-pack fetch` restores them; their sha256s are in `event.json`.

**HRRR subhourly PRATE (CONUS forecast, 3 km / 15 min).** Objects
`https://noaa-hrrr-bdp-pds.s3.amazonaws.com/hrrr.20200810/conus/hrrr.t18z.wrfsubhfFF.grib2`
(`.idx` for the indexes) — NOAA's Big Data bucket is its own archive, so no rewrite. The objects
are 181–198 MB and are never checked in; only the 40–56 KB `PRATE` messages the `.idx` selection
resolves to, as explicit byte ranges. Upstream object lengths (what the range arithmetic is bounded
by): `f01` 181,258,425, `f02` 191,174,984, `f03` 197,759,552. `f04` is HEAD-probed for run
completeness — `hrrr.t18z.wrfsubhf04.grib2.idx`, 10,223 bytes — and never read: no published lead
lives in it, which is the request accounting `tests/us_gfs_cycle.rs` already pins.

- `upstream/hrrr/hrrr.t18z.wrfsubhf01.grib2.idx` — 10,015 bytes.
- `upstream/hrrr/hrrr.t18z.wrfsubhf02.grib2.idx` — 10,116 bytes.
- `upstream/hrrr/hrrr.t18z.wrfsubhf03.grib2.idx` — 10,220 bytes.
- `upstream/hrrr/hrrr.t18z.wrfsubhf01.grib2@157147064-157187055` — 39,992 bytes (+60 min).
- `upstream/hrrr/hrrr.t18z.wrfsubhf02.grib2@23028282-23076333` — 48,052 bytes (+75 min).
- `upstream/hrrr/hrrr.t18z.wrfsubhf02.grib2@70526706-70579753` — 53,048 bytes (+90 min).
- `upstream/hrrr/hrrr.t18z.wrfsubhf02.grib2@118367784-118422756` — 54,973 bytes (+105 min).
- `upstream/hrrr/hrrr.t18z.wrfsubhf02.grib2@166419807-166475282` — 55,476 bytes (+120 min).
- `upstream/hrrr/hrrr.t18z.wrfsubhf03.grib2@23551850-23605739` — 53,890 bytes (+135 min).
- `upstream/hrrr/hrrr.t18z.wrfsubhf03.grib2@72814187-72869548` — 55,362 bytes (+150 min).
- `upstream/hrrr/hrrr.t18z.wrfsubhf03.grib2@122274042-122329738` — 55,697 bytes (+165 min).

Each file name states its own byte window, and `event.json` carries every sha256 —
`obc-wx-pack verify` re-checks the lot.

### Reproducing it

```sh
cargo run --release --bin obc-wx-pack -- capture us-derecho-2020-08-10 \
  --at 2020-08-10T18:52:00Z --title "2020-08-10 Midwest derecho" --region conus \
  --bbox 40.5,-96.5,43.5,-90.0 --out wx-events
```

Both archives serve immutable objects, so a fresh capture reproduces these exact digests. The one
thing that is *not* reproducible from a wall clock alone: run discovery replays against an archive
that already holds the whole day, so `--at` must be a moment when the run the cycle selects had
genuinely been published. 18:52 UTC is such a moment — the 18Z HRRR subhourly set completes around
18:50.
