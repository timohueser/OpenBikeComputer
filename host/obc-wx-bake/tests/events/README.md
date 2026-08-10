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

**Nothing in a pack is a hand-made artifact, and nothing has to be fetched.** `service/` is what
`cycle::run_cycle` writes when its `Upstream` is a `FixtureUpstream` loaded from `upstream/` — the
same offline seam `tests/cycle.rs` and `tests/us_gfs_cycle.rs` already use. `truth/` is the same
deal through `mrms::bake_observation`. `tests/event_pack.rs` re-derives **both** from the pack's
own bytes and byte-compares, so a pack that stops reproducing is a baker regression, loudly. It
also checks the converse both ways: no request the replay makes is unaccounted for by a member,
and no member goes unread.

That second half only became true once the truth ladder's raw MRMS observations were checked in
(`--store-truth-upstream`, +4.3 MB). Before that, `truth/` was eight *baked artifacts* whose
sources lived on a single free mirror — so a change to the observation lattice or the quantization
table would have meant going back to MTArchive to re-derive them. For a fixture whose whole purpose
is durability that was the wrong trade, and 4.3 MB in git is the cheaper half of it by a wide
margin. **Ship US packs with `--store-truth-upstream`.**

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
reports a 2021 re-upload. The guard covers the **service** half only — `truth/` is by definition
what happened afterwards.

The two lag constants in `src/pack/archive.rs` are this guard's **axiom**, and they deserve the
scrutiny: the guard reads them and so does the leakage test, so a constant that is too small is
invisible to CI by construction. They are therefore *worst observed plus margin*, never calibrated
to a measurement — a ceiling equal to the worst sample is a coincidence, not a ceiling.

| source | worst observed | constant | why the margin |
|---|---|---|---|
| MRMS `PrecipRate` | +3:01 over 73 consecutive objects (2026-08-10) | **240 s** | an earlier 180 s "rounded up" past a real sample |
| HRRR subhourly set | +62:56 over 13 runs (2026-08-09/10) | **75 min** | 2 of 13 runs past +62, and in both the *last* file written was a middle one — the tail is non-monotonic write order, with no reason to stop just past the worst sample. These are 2026 numbers applied to 2020 HRRRv3 runs whose latency is unrecoverable. |

The asymmetry decides the direction: treating an object as unpublished too long only makes a
capture more conservative — it falls back to an older observation or run, which the real service
does whenever an upstream is late. Treating one as published too early is the whole failure mode.

The margins are **compile-time assertions** in `src/pack/archive.rs`, not tests, because a test
could not catch this: the leakage test calls the same `published_at` the guard does, so it keeps
passing while a shaved constant quietly lets data in. Trimming `MRMS_PUBLICATION_LAG_SECONDS` back
to 180 fails the build with *"must clear the worst observed publication by a real margin, not equal
it"* — which is feedback that arrives before a capture does. (The constants also have an upper
bound: both must stay inside the adapters' own backwards-discovery reach, or the guard would turn
every capture into "nothing was ever published".)

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
  moved. **The shipped pack has none** — durability beat 4.3 MB, see above — but the mechanism
  stays for a pack that is genuinely too large to carry.

`--bbox` is validated for magnitude as well as ordering, because `(degrees * 1e6).round() as i64`
*saturates*: `--bbox 40.5,-96.5,1e30,-90` used to crop half of CONUS silently instead of being
refused. (`crop::window` is also independently overflow-free — it works in `i128` — since a release
build has `overflow-checks` off.)

## The basemap convention — US packs stay on one map

The bakery is DACH-first (`host/obc-bake/regions.toml`). It carries exactly **one** non-European
region, `north-america/us/iowa`, and it exists for these packs: a frozen storm with no map under it
is not something the simulator can show.

**Every US event pack crops to ground Iowa covers, so one state map serves all of them.** This is a
convention, held by three things rather than by hope:

* every pack records `coverage_udeg` — what its baked frames actually answer for — and
  `basemap_region`, the map that ground needs;
* `obc-wx-pack capture` prints the coverage and how far it reaches past the basemap;
* `tests/event_pack.rs` holds the **requested** `--bbox` to within one observation tile
  (0.64 degrees) of Iowa's box, and the resulting coverage to *that request plus one tile*.

The two-part budget is deliberate, because coverage exceeds the state for two independent reasons
and it would be dishonest to blame both on tiling. Of this pack's worst overhang — +0.200 degrees
east — **+0.140 is the requested window itself** (its east edge is 90.000 W against Iowa's
90.140 W) and only ~0.060 is tile alignment. Holding the request directly is what actually catches
drift; the tile term only forgives what the format forces.

`US_BASEMAP_BBOX` is hand-copied from the published state bounds and **nothing ties it to the
Geofabrik extract** — this crate never fetches the `.poly`. It is a tripwire for "did a pack wander
to another state", not a survey marker.

A pack that wants Kansas is a conversation about a second basemap, not a second line added quietly.
`--basemap <id>` exists for that conversation's outcome, not to route around it.

---

## `us-derecho-2020-08-10` — the 10 August 2020 Midwest derecho

The 2020-08-10 derecho crossing Iowa, captured at its peak. One cycle of the composed US product
(MRMS observation + HRRR subhourly forecast), cropped to Iowa and the Mississippi valley, plus a
two-hour observed ladder. **5,505,817 bytes on disk, and nothing to fetch.**

| | |
|---|---|
| capture instant (`--at`) | `2020-08-10T18:52:00Z` |
| product reference time | `2020-08-10T18:48:00Z` — the newest observation that existed at 18:52 |
| HRRR run selected | 2020-08-10 **17Z**, subhourly `wrfsubhf01..04` |
| crop (`--bbox`) | `40.5,-96.5,43.5,-90.0` |
| coverage | 40.480 N, 96.660 W .. 43.680 N, 89.940 W — basemap `north-america/us/iowa` |
| service | 9 frames + manifest, 108,242 bytes |
| truth | 8 observed frames, 245,673 bytes |
| upstream | 20 members, 5,126,306 bytes, **all checked in** |

### Why 18:48 and 17Z, when the capture is clocked at 18:52

Because that is what the service would have had, and the pack is worthless if it is not.

* **MRMS** takes about three minutes to publish, so the guard's 240 s puts the newest available
  observation at 18:48. The 18:52 and 18:50 objects were still in the pipeline.
* **HRRR** subhourly runs complete around the top of the following hour, and the guard's 75 min
  puts the 18Z run out of reach at 18:52. `select_run` requires all four `wrfsubhf` objects, so the
  newest complete run was 17Z.

Both selections are stable under the margins rather than balanced on them: raising the constants
from 180 s / 65 min to 240 s / 75 min left the baked bytes **byte-identical**, which is what a
conservative guard should do.

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
  sha256 `fb78ef1e5baf…` (the cycle's anchor).
- the eight truth observations at 190200, 191800, 193200, 194800, 200200, 201800, 203200 and
  204800 — 464,823 / 488,065 / 512,256 / 539,054 / 547,051 / 559,673 / 570,840 / 578,683 bytes,
  4,260,445 in total. All checked in, so `truth/` re-derives offline.

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
  --bbox 40.5,-96.5,43.5,-90.0 --store-truth-upstream --out wx-events
```

Both archives serve immutable objects, so a fresh capture reproduces these exact digests — the
whole pack, `event.json` included, `diff -r`s clean against the checked-in one.
