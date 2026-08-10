# obc-wx-bake event packs

An **event pack** is one real past weather event frozen on disk: the raw bytes the archives served,
the tree the real baker makes of them, and what the radar actually saw over the following two
hours. Packs exist so the simulator and the test suite can run against real radar instead of
synthetic blobs.

```text
wx-events/<event-id>/
  event.json    the pack manifest: window, bake parameters, coverage, and per-member provenance
  upstream/     raw archive bytes, byte-identical to what the archive served
  service/      the baked tree: wx/v1/manifest.json + wx/v1/<product>/<gen>/f*.obcg
  truth/        the OBSERVED frames at the truth offsets — ground truth for later scoring
```

Capture, inspect and check a pack with `obc-wx-pack` (a second binary of this crate — see
`src/bin/obc-wx-pack.rs`):

```sh
cargo run --release --bin obc-wx-pack -- capture <event-id> --at <rfc3339> \
      [--out <dir>] [--title <t>] [--region <r>] [--basemap <regions.toml id>] \
      [--bbox <s,w,n,e>] [--truth-offsets 15,30,...|none] [--store-truth-upstream]
cargo run --release --bin obc-wx-pack -- show   <pack-dir>
cargo run --release --bin obc-wx-pack -- verify <pack-dir>   # digests, then a full re-bake
cargo run --release --bin obc-wx-pack -- rebake <pack-dir>
cargo run --release --bin obc-wx-pack -- fetch  <pack-dir>   # materialize recorded-only members
```

## The three rules that make a pack trustworthy

**`service/` is not a hand-made artifact.** It is what `cycle::run_cycle` writes when its
`Upstream` is a `FixtureUpstream` loaded from `upstream/` — the same offline seam `tests/cycle.rs`
and `tests/us_gfs_cycle.rs` already use. `tests/event_pack.rs` re-bakes every checked-in pack and
byte-compares, so a pack that stops reproducing is a baker regression, loudly. It also checks the
converse both ways: no request the replay makes is unaccounted for by a member, and no member goes
unread.

**The archive is not the upstream.** The baker asks for
`https://noaa-mrms-pds.s3.amazonaws.com/...`, a bucket that retains days rather than years. A 2020
observation therefore comes from Iowa State's MTArchive mirror, and every member records *both*
URLs: `url`, the canonical key the baker requests and the replay serves it under, and
`archive_url`, where the bytes actually came from. The rewrite lives in `src/pack/archive.rs`.

**A pack must not contain the future.** An archive holds the whole day, so a naive replay lets run
discovery select a run and an observation that had not been published yet at the capture instant —
and a pack like that ships a model baseline with an extra hour of assimilation and radar the device
could not have had. `AsOf` (in `src/pack/capture.rs`) makes any object whose *own key* says it was
not published yet a 404 for discovery, so the production `discover_latest` / `select_run` fallback
paths reach the honest answer with no adapter change. It needs no response header, which matters:
MTArchive reports its own 2020-08-11 ingest time for a 2020-08-10 object, and NOAA's HRRR bucket
reports a 2021 re-upload. The lags are measured constants in `src/pack/archive.rs`
(MRMS +180 s, HRRR run complete +65 min), and the guard covers the **service** half only —
`truth/` is by definition what happened afterwards.

Provenance discipline is `tests/fixtures/README.md`'s, applied per member inside `event.json`:
exact retrieval URL, byte range where one was used, length, sha256, and licence.

## Size discipline

Full-domain packs are hundreds of megabytes, so two things keep a checked-in pack small:

* `--bbox` crops the **baked** output (`src/pack/crop.rs`). The crop window is aligned outward to
  each frame's tile edge, so every retained tile's payload bytes are identical to the uncropped
  object's — a cropped pack is a *subset* of the real bake, not a different bake. The raw
  `upstream/` bytes are never cropped; they must stay byte-identical to what the archive served.
  The crop also re-verifies the composed product's lattice nesting, since it moves every origin.
* Members with `"stored": false` are recorded with full provenance but not checked in.
  `obc-wx-pack fetch` materializes them from `archive_url` and refuses anything whose sha256 has
  moved.

## The basemap convention — US packs stay on one map

The bakery is DACH-first (`host/obc-bake/regions.toml`). It carries exactly **one** non-European
region, `north-america/us/iowa`, and it exists for these packs: a frozen storm with no map under it
is not something the simulator can show.

**Every US event pack crops to ground Iowa covers, so one state map serves all of them.** This is a
convention, held by three things rather than by hope:

* every pack records `coverage_udeg` — what its baked frames actually answer for — and
  `basemap_region`, the map that ground needs;
* `obc-wx-pack capture` prints the coverage and how far it reaches past the basemap;
* `tests/event_pack.rs` fails if a pack's coverage reaches more than one observation tile
  (0.64 degrees) past Iowa's bounding box, which is the honest tolerance: crops align outward to
  whole tiles, so a window hugging a state border always overshoots it a little.

A pack that wants Kansas is a conversation about a second basemap, not a second line added quietly.
`--basemap <id>` exists for that conversation's outcome, not to route around it.

---

## `us-derecho-2020-08-10` — the 10 August 2020 Midwest derecho

The 2020-08-10 derecho crossing Iowa, captured at its peak. One cycle of the composed US product
(MRMS observation + HRRR subhourly forecast), cropped to Iowa and the Mississippi valley, plus a
two-hour observed ladder. 1,245,380 bytes on disk.

| | |
|---|---|
| capture instant (`--at`) | `2020-08-10T18:52:00Z` |
| product reference time | `2020-08-10T18:48:00Z` — the newest observation that existed at 18:52 |
| HRRR run selected | 2020-08-10 **17Z**, subhourly `wrfsubhf01..04` |
| crop (`--bbox`) | `40.5,-96.5,43.5,-90.0` |
| coverage | 40.480 N, 96.660 W .. 43.680 N, 89.940 W — basemap `north-america/us/iowa` |
| service | 9 frames + manifest, 108,242 bytes |
| truth | 8 observed frames, 245,673 bytes |
| upstream checked in | 12 members, 865,861 bytes |
| upstream recorded only | 8 members, 4,260,445 bytes |

### Why 18:48 and 17Z, when the capture is clocked at 18:52

Because that is what the service would have had, and the pack is worthless if it is not.

* **MRMS** takes about three minutes to publish (measured against the live bucket on 2026-08-10:
  +2:49, +2:52, +2:58, +3:01 for four consecutive objects). At 18:52:00Z the 18:52 and 18:50
  observations were still in the pipeline; 18:48 was the newest one out.
* **HRRR** subhourly runs complete around the top of the following hour — the 2026-08-10 11Z set
  landed at +53:38, +55:49, +56:51, +58:53, and 2026-08-09's 18Z run wrote `wrfsubhf01`'s index at
  **+62:21**, *after* `wrfsubhf04`'s. `select_run` requires all four, so at 18:52 the 18Z run was
  still incomplete and the newest complete one was 17Z.

Both facts are visible in `event.json` rather than asserted here: three HEAD probes are recorded
with `"object_length": null` — MRMS 18:52, MRMS 18:50, and `hrrr.t18z.wrfsubhf04.grib2.idx` — and
those are the fallback happening.

A 112-minute-old model run is not an unlucky draw, either: HRRR publishes hourly and completes
around +60 min, so the newest complete run is **always** between 60 and 119 minutes old. 18:52 sits
at the older end of an ordinary range. Moving `--at` to ~19:05 would have bought the 18Z run at 65
minutes old — equally ordinary, and it would have hidden the guard's effect. The capture instant
stayed where it was so the fallback is part of what the pack demonstrates.

The forward frames therefore keep the 17Z run's +120..+225 leads at their own valid times, which
land 12, 27, 42 … 117 minutes ahead of an 18:48 observation. Nothing is re-spaced onto a round
cadence. The observation frame is 704 x 320 cells at 1 km and 36.92 % wet; the +117 min forecast is
224 x 128 at 3 km and 8.57 %.

### Truth ladder

Requested at +15 min steps. MRMS `PrecipRate` publishes every two minutes on even minutes, so odd
requests floor onto the cadence; `event.json` records both numbers and each frame's own OBCG header
carries the real instant. A ladder whose requests are closer together than the cadence is refused
outright rather than silently losing a rung.

| requested | actual | valid at | bytes |
|---|---|---|---|
| +15 | +14 | 2020-08-10T19:02:00Z | 36,437 |
| +30 | +30 | 2020-08-10T19:18:00Z | 36,199 |
| +45 | +44 | 2020-08-10T19:32:00Z | 35,768 |
| +60 | +60 | 2020-08-10T19:48:00Z | 33,395 |
| +75 | +74 | 2020-08-10T20:02:00Z | 30,669 |
| +90 | +90 | 2020-08-10T20:18:00Z | 27,660 |
| +105 | +104 | 2020-08-10T20:32:00Z | 24,414 |
| +120 | +120 | 2020-08-10T20:48:00Z | 21,131 |

The ladder is on the observation's own lattice — same origin, same cell size, same dimensions as
frame 0 — so scoring is a cell comparison with no resampling in between. Wet fraction falls from
37.90 % to 18.26 % across the two hours as the storm leaves the window eastward.

### Upstream provenance

Retrieved 2026-08-10 UTC. Terms for every member: NOAA Open Data Dissemination (public-use U.S.
government data, no endorsement implied), <https://www.noaa.gov/information-technology/open-data-dissemination>.
The full record — including the HEAD probes the baker's discovery made, the three the as-of guard
suppressed, and the object lengths its range arithmetic was bounded by — is in `event.json`; this
table is the readable tour.

**MRMS PrecipRate (CONUS observation, 1 km / 2 min).** Canonical key
`https://noaa-mrms-pds.s3.amazonaws.com/CONUS/PrecipRate_00.00/20200810/MRMS_PrecipRate_00.00_20200810-HHMMSS.grib2.gz`,
retrieved from
`https://mtarchive.geol.iastate.edu/2020/08/10/mrms/ncep/PrecipRate/PrecipRate_00.00_20200810-HHMMSS.grib2.gz`.

- `upstream/mrms/MRMS_PrecipRate_00.00_20200810-184800.grib2.gz` — 448,150 bytes,
  sha256 `fb78ef1e5baf…` (the cycle's anchor; **checked in**).
- the eight truth observations at 190200, 191800, 193200, 194800, 200200, 201800, 203200 and
  204800 — 464,823 / 488,065 / 512,256 / 539,054 / 547,051 / 559,673 / 570,840 / 578,683 bytes,
  **recorded only**. `obc-wx-pack fetch` restores them; their sha256s are in `event.json`.

**HRRR subhourly PRATE (CONUS forecast, 3 km / 15 min).** Objects
`https://noaa-hrrr-bdp-pds.s3.amazonaws.com/hrrr.20200810/conus/hrrr.t17z.wrfsubhfFF.grib2`
(`.idx` for the indexes) — NOAA's Big Data bucket is its own archive, so no rewrite. The objects
are 187–198 MB and are never checked in; only the 45–50 KB `PRATE` messages the `.idx` selection
resolves to, as explicit byte ranges. Upstream object lengths (what the range arithmetic is bounded
by): `f02` 186,909,295, `f03` 192,621,021, `f04` 197,621,511. `f01` is HEAD-probed for run
completeness and never read — no published lead lives in it, which is the request accounting
`tests/us_gfs_cycle.rs` already pins.

- `upstream/hrrr/hrrr.t17z.wrfsubhf02.grib2.idx` — 10,114 bytes.
- `upstream/hrrr/hrrr.t17z.wrfsubhf03.grib2.idx` — 10,215 bytes.
- `upstream/hrrr/hrrr.t17z.wrfsubhf04.grib2.idx` — 10,220 bytes.
- `upstream/hrrr/hrrr.t17z.wrfsubhf02.grib2@162616821-162662264` — 45,444 bytes (+120 min).
- `upstream/hrrr/hrrr.t17z.wrfsubhf03.grib2@23066514-23113886` — 47,373 bytes (+135 min).
- `upstream/hrrr/hrrr.t17z.wrfsubhf03.grib2@71546481-71593450` — 46,970 bytes (+150 min).
- `upstream/hrrr/hrrr.t17z.wrfsubhf03.grib2@119513228-119562347` — 49,120 bytes (+165 min).
- `upstream/hrrr/hrrr.t17z.wrfsubhf03.grib2@167720684-167770594` — 49,911 bytes (+180 min).
- `upstream/hrrr/hrrr.t17z.wrfsubhf04.grib2@23587278-23636767` — 49,490 bytes (+195 min).
- `upstream/hrrr/hrrr.t17z.wrfsubhf04.grib2@72266095-72315311` — 49,217 bytes (+210 min).
- `upstream/hrrr/hrrr.t17z.wrfsubhf04.grib2@121873450-121923086` — 49,637 bytes (+225 min).

Each file name states its own byte window, and `event.json` carries every sha256 —
`obc-wx-pack verify` re-checks the lot.

### Reproducing it

```sh
cargo run --release --bin obc-wx-pack -- capture us-derecho-2020-08-10 \
  --at 2020-08-10T18:52:00Z --title "2020-08-10 Midwest derecho" --region conus \
  --bbox 40.5,-96.5,43.5,-90.0 --out wx-events
```

Both archives serve immutable objects, so a fresh capture reproduces these exact digests — the
whole pack, `event.json` included, `diff -r`s clean against the checked-in one.
