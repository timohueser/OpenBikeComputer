# WX1 server-side weather source contracts

Status: **GO with a GFS-only worldwide floor; IMERG Early is an explicit v1 NO-GO**

Issue: [#1186](https://github.com/timohueser/OpenBikeComputer/issues/1186)

Evidence captured: 2026-08-09 UTC from a macOS host in Germany

This decision record implements the revised server-based architecture in
[#1185](https://github.com/timohueser/OpenBikeComputer/issues/1185). Provider
archives and GRIB/HDF5 decoding belong on the self-hosted stateless baker. The
phone downloads only provider-neutral baked weather objects, except for the
separately scoped MET Norway Locationforecast point forecast. Nothing in this
spike is a production baker, an object format, or an iOS provider adapter.

## Frozen decisions

| Region/tier | Upstream fact | Source | WX1 result | Safe fallback |
| --- | --- | --- | --- | --- |
| Germany, tier 1 | 1 km rain observation/nowcast, five-minute native steps to +120 min | DWD RV raw HDF5 tar | **GO** | ICON-EU, then GFS |
| CONUS, tier 1 | observed two-minute precipitation rate | NOAA MRMS `PrecipRate_00.00` | **GO** | HRRR, then GFS |
| CONUS, tier 2 | forecast precipitation rate at 15-minute steps through +2 h | NOAA HRRR subhourly `PRATE` | **GO** | GFS |
| Europe, tier 2 | hourly forecast accumulation | DWD ICON-EU regular-lat-lon `TOT_PREC` | **GO** | GFS |
| Worldwide, tier 3 | hourly forecast accumulation/floor | NOAA GFS 0.25-degree `APCP` | **GO** | keep the last complete GFS run; otherwise publish unavailable |
| Worldwide observation candidate | half-hourly precipitation estimate | NASA GPM IMERG Early V07B | **NO-GO for v1** | GFS-only; do not fabricate observation frames |
| Phone-only hourly point forecast | temperature, precipitation, condition, wind | MET Norway Locationforecast 2.0 `complete` | **GO with optional gust/probability** | retain timestamped cache or show unavailable |

The fallback column is a loss of quality, not permission to relabel one product
as another. Observation, nowcast, and forecast provenance must survive through
normalization and baking. Missing data is never dry weather.

> **Superseded in part, 2026-08-11 (WXR5 [#1244](https://github.com/timohueser/OpenBikeComputer/issues/1244)
> and WXR7 [#1246](https://github.com/timohueser/OpenBikeComputer/issues/1246), under epic
> [#1248](https://github.com/timohueser/OpenBikeComputer/issues/1248)).**
> The *sources* above and their GO/NO-GO verdicts stand unchanged; what no longer
> exists is the **ladder**. The tier column described a choice a client made — by
> region, bbox containment and freshness — and both clients deleted that code in
> #1244, after which #1246 deleted the producer that published the products and
> the spec sections that described them. The baker mosaics every source above onto one canonical 0.01 degree
> lattice with a fixed priority order, and downstream there is one dataset with no
> tier, no product id and no fallback to select. Read the tier column as the
> baker's **priority order** for overlapping sources, not as anything a phone or a
> device can see. The last sentence is the part that survives untouched and got
> stronger: missing data is never dry weather, and neither is an expired one.

## Architecture boundary

WX1 proved, with a disposable Rust contract validator, that a host can
decompress and validate the selected upstream bytes. That spike was deleted
once WX6 landed the last adapter it covered; the contracts it measured now live
in the production baker `host/obc-wx-bake` (`src/grib.rs` pins the Section-3
bytes and templates, `src/source/*` the per-source rules), which preserves these
module boundaries:

```text
source/{dwd_rv,mrms,icon_eu,hrrr,gfs}
    fetch immutable run/object metadata
    select exact fields
    decode provider bytes
    normalize native grids + provenance
                  |
                  v
policy/timeline   choose regional precedence and coherent valid times
                  |
                  v
bake              crop/resample/quantize into the provider-neutral object
                  |
                  v
publish           atomically upload immutable objects + latest manifest to R2
```

Each source module owns its endpoint, archive/index selection, units, missing
sentinels, native projection, and fail-closed schema checks. It returns an
internal normalized grid carrying at least source product, run/reference time,
valid interval, geometry, units, missing mask, and quality class. Timeline
policy consumes that normalized contract; it must not branch on GRIB template
numbers or provider JSON. Publishing consumes baked bytes; it must not know how
NOAA or DWD encodes precipitation.

Consequences:

- no DWD, NOAA, NASA, GRIB, HDF5, bzip2, or provider archive code belongs in
  Swift or device firmware;
- no source adapter writes directly to R2;
- no single broad `WeatherService` hides unlike facts behind optional fields;
- a complete run is validated and published atomically; mixed-run timelines
  are rejected;
- upstream schema drift fails the run and keeps the previous complete output;
- MET is the one explicit phone-side provider exception and remains outside
  `obc-wx-bake`.

The spike caps compressed inputs at 16 MiB, decompressed GRIB/HDF5 inputs at
256 MiB, and decoded grids at 30 million points before accepting data. The
production baker may tighten those limits or stream/chunk more aggressively;
it must not silently loosen them.

## DWD RV: German rain nowcast

Use the maintained raw OpenData product:

```text
https://opendata.dwd.de/weather/radar/composite/rv/composite_rv_LATEST.tar
https://opendata.dwd.de/weather/radar/composite/rv/composite_rv_YYYYMMDD_HHMM.tar
```

The captured immutable run contained 25 complete HDF5 members, leads 000..120
in five-minute increments. Validate every member before selecting the nine
published valid times +0, +15, ..., +120 minutes. Do not interpolate the
discarded intermediate frames.

Pinned member contract:

- ODIM quantity `ACRR`, five-minute liquid precipitation accumulation in mm;
- 1,100 x 1,200 grid, native 1,000 m x 1,000 m cells;
- exact stereographic projection recorded in the Rust validator;
- exact ODIM corners `(LL 3.5669946350,45.6964253774)`,
  `(UL 1.4633015103,55.8620871082)`, `(UR 18.7316164547,55.8454385633)`,
  and `(LR 16.5808693486,45.6846057814)`; a shifted/flipped grid fails;
- `gain=0.0009999999317806213` and
  `offset=-0.0009999999317806213` for the captured run;
- encoded `nodata=4294967295` means missing;
- encoded `undetect=0` means dry;
- every other value is `encoded * gain + offset` mm/5 min and must be finite
  and nonnegative.

The filename run (`YYYYMMDD_HHMM`) must equal the root ODIM `what/date,time`.
For lead `L`, `dataset1/what/enddate,endtime` must equal run + `L` minutes and
the internal start must be exactly five minutes earlier. Member names must be
one coherent run with leads 000..120/5. A renamed member, duplicate lead, or
internally stale forecast therefore fails even if the HDF5 values decode.

The full 11:30 UTC tar was 2,017,280 bytes and was last modified at 11:33:25,
an observed publication delay of 3 min 25 s. Its f000 member contained 28,048
positive cells, 621,815 missing cells, and a convective maximum of
4.192999713956145 mm/5 min. Validating all 25 members in an optimized Rust build
took 0.35 s wall / 0.06 s CPU and 18.2 MB peak RSS on the capture host.

The previous WCS/GeoTIFF and WMS approaches are retired. WMS is styled imagery,
not numeric data; WCS is not the server contract and must not re-enter the
phone. The raw tar's `ETag`, `Last-Modified`, `Content-Length`, and range support
may be used for conditional discovery. Once an immutable run name is known,
cache it by run name and never mutate its decoded result.

## NOAA MRMS: CONUS observation

Use NOAA Open Data Dissemination (NODD), not a scraped map service:

```text
https://noaa-mrms-pds.s3.amazonaws.com/
  CONUS/PrecipRate_00.00/YYYYMMDD/
  MRMS_PrecipRate_00.00_YYYYMMDD-HHMMSS.grib2.gz
```

Pinned contract:

- two-minute nominal cadence, observation only;
- GRIB discipline/category/parameter `209/6/1` (`PrecipRate_00.00`);
- regular latitude/longitude grid template 3.0;
- exact 7,000 x 3,500 / 24,500,000-point Section-3 geometry: first point
  `54.995,230.005`, last `20.005001,299.994998`, increments `0.01/0.01`
  degrees, scanning mode `0x00`, including the captured Earth shape/flags;
- PNG representation template 5.41;
- unit mm/hour;
- `-1` is missing and `-3` is no coverage; neither is dry;
- finite `0` is dry and positive values are rain.

The captured 02:00 UTC object was 456,264 bytes and appeared at 02:02:44, an
observed delay of 2 min 44 s. It decoded to 8,357,311 missing/no-coverage cells,
15,816,149 dry cells, 326,540 positive cells, and a 185.3 mm/h convective
maximum. Full-field decoding took 0.59 s wall / 0.29 s CPU and 160.9 MB peak
RSS. This is the high-water mark for the current spike and sets a minimum
production-memory review: WX5 must either provision safely above concurrent
decodes or tile/stream the normalized result.

MRMS is CONUS-only in this contract. Alaska, Hawaii, Puerto Rico, and other
regional products require a separate measured source decision. Do not clamp
coordinates onto the CONUS grid.

## NOAA HRRR: CONUS +2-hour forecast

Use NODD subhourly objects and their text indexes:

```text
https://noaa-hrrr-bdp-pds.s3.amazonaws.com/
  hrrr.YYYYMMDD/conus/hrrr.tCCz.wrfsubhfFF.grib2[.idx]
```

Select only the exact `PRATE:surface:<N> min fcst` records. Use f01 for 15, 30,
45, and 60 minutes and f02 for 75, 90, 105, and 120 minutes. Parse strictly
increasing `.idx` offsets, require one exact match per selector, obtain the
full object length for the final range, and use HTTP Range to fetch complete
GRIB messages. An ambiguous or out-of-bounds range is a contract failure.

Pinned field contract:

- discipline/category/parameter `0/1/7` (`PRATE` at surface);
- Lambert conformal grid template 3.30;
- exact 1,799 x 1,059 / 1,905,141-point Section-3 geometry: first point
  `21.138123,237.280472`, `LaD=38.5`, `LoV=262.5`, `Latin1=Latin2=38.5`,
  `Dx=Dy=3,000 m`, projection-centre `0x00`, scanning mode `0x40`, and the
  captured Earth shape/remaining projection fields;
- product template 4.0;
- complex packing with spatial differencing, representation template 5.3;
- unit kg/m2/s, numerically mm/s; multiply by 3,600 only when an mm/hour rate
  is required;
- finite, nonnegative values only.

The selected `<N> min fcst` value is passed to the validator and must equal
the GRIB PDT 4.0 valid time minus its Section-1 reference time. Index text
alone is not accepted as temporal identity.

The captured f002 object was 186,047,054 bytes and appeared at 00:53:49 after
the 00Z reference time. The selected +120-minute message was 42,861 bytes,
with 26,061 positive cells. The eight first-two-hour message ranges totaled
330,351 bytes for the captured run. Decoding the selected field took 0.02 s
and 30.4 MB peak RSS.

Do not present HRRR as an observation, and do not present a frozen MRMS field as
one either. MRMS and HRRR are two sources at two priority ranks (#1246 deleted
the composed product that used to hold both), and neither is blended into a
fictional single model run.

What changed with the mosaic is where the honesty lives. A published frame's
`valid_at` is its place on the 15-minute cadence, not the measurement time of
whatever painted it, and MRMS contributes one frame — so the anchor's field also
paints +15 and +30, inside the 30-minute skew window. Those two frames carry
**Forecast**: an observation carried forward is a persistence nowcast, and only
the anchor may claim to be measured weather (`OBCG_Spec.md` §3.2). The
generation states `max_source_skew_s` once, so how old the radar under a cell
may be is a number a consumer reads rather than assumes.

## DWD ICON-EU: European forecast

Use DWD's regular-lat-lon single-level `TOT_PREC` files:

```text
https://opendata.dwd.de/weather/nwp/icon-eu/grib/CC/tot_prec/
  icon-eu_europe_regular-lat-lon_single-level_YYYYMMDDCC_FFF_TOT_PREC.grib2.bz2
```

Nominal cycles are 00, 06, 12, and 18 UTC. Select the newest run only after all
lead files required by the publication window exist and validate. Retaining
f000..f011 safely covers the first hours while the next six-hourly cycle is
still being published; the captured set totaled 4,134,603 compressed bytes.

Pinned contract:

- `TOT_PREC`, discipline/category/parameter `0/1/52`;
- cumulative liquid precipitation in mm from reference time to valid time;
- exact 1,377 x 657 / 904,689-point Section-3 geometry from
  `29.5,336.5` to `70.5,62.5`, `Di=Dj=0.0625` degrees, scanning mode `0x40`,
  including the captured Earth shape/flags;
- product template 4.8;
- CCSDS/AEC representation template 5.42;
- bzip2 outer compression;
- de-accumulate only consecutive leads from the same run and identical grid.

For PDT 4.8 the generic forecast-time field is only the accumulation start.
The validator therefore parses the template's explicit interval-end timestamp,
statistical range unit/length, range count, and increment semantics. The CLI
lead must equal `interval_end - reference`; de-accumulation additionally
requires identical interval starts and exactly one hour between ends. Thus
f001/f001, a renamed lead, a shifted grid, and a skipped lead all fail.

Independent packing of cumulative fields produced 9,005 negative f002-f001
differences of exactly 1/4096 mm. The two captured packing increments were
1/4096 and 1/2048 mm. Treat as dry roundoff only a negative difference no
larger than half the sum of the two fields' decoded packing increments. Any
larger decrease, run mismatch, skipped lead, or geometry change fails the run;
it is not clamped. The captured delta had 262,086 positive cells and a
12.145752 mm maximum. Decode plus de-accumulation took 0.08 s and 25.7 MB peak
RSS.

## EUMETNET OPERA: European radar (addendum, 2026-08-10, WXR6 #1245)

Amendment, not a rewrite: WX1 recorded no European tier-1 radar because none was
surveyed. [#1245](https://github.com/timohueser/OpenBikeComputer/issues/1245)
surveyed one and it is a **GO**, so the frozen table above should be read as
having gained a `Europe, tier 1` row — which, per the supersession note, is a
position in the baker's priority order and not a ladder a client walks.

Objects are anonymous on CloudFerro, CC BY 4.0 (each object states its own
licence in `GDAL_METADATA`):

```text
https://s3.waw3-1.cloudferro.com/openradar-24h/
  YYYY/MM/DD/OPERA/COMP/OPERA@YYYYMMDDTHHMM@0@{DBZH,RATE,ACRR}.{h5,tiff}
```

Two products are used, both as observation frames, both read from the **COG**
rather than the ODIM HDF5 twin (`openradar-archive`, which reaches back to 2012,
carries only the HDF5s; the live 24-hour bucket carries both):

- **CIRRUS `DBZH`** — 3,800 x 4,400 cells of 1 km, every 5 minutes, measured
  publication lag 4.1 min. Column-maximum reflectivity (`product = MAX`).
- **NIMBUS `RATE`** — 1,900 x 2,200 cells of 2 km, every 15 minutes, measured lag
  10 min. Already mm/h, near-surface (`product = PPI`); its metadata declares
  `zr_a = 200.0`, `zr_b = 1.6`.

`ACRR` is rejected: same 2 km grid, and it is a **one-hour** accumulation, which
smears a moving shower across an hour of track.

**Reflectivity to rain rate.** Marshall-Palmer `Z = 200 R^1.6` is a *surface*
relation, and it is what OPERA itself applies to the near-surface PPI. CIRRUS is
a column maximum, so applying it unchanged overstates surface rain: measured over
the 149,527 cells where both products saw an echo in the 2026-08-10T00:00 pair,
the median CIRRUS/NIMBUS rate ratio is **2.2**, a full intensity band. The
reflectivity path therefore divides by that measured ratio — equivalently
`a_eff = 200 x 2.2^1.6 = 706.2`, or -5.48 dBZ — as an **empirical calibration,
not physics**. Settling it properly means splitting the ratio by regime
(stratiform vs convective at 30 dBZ) over a full day and scoring both products
against gauge-adjusted `dwd-rv`; that measurement is not done, and until it is,
the scalar is one number from one frame pair.

Pinned contract, verified against the live objects:

- classic TIFF, `Compression = 8`, `Predictor = 1`, 512 x 512 tiles,
  `SamplesPerPixel = 2` (value + `pl.imgw.quality.qi_total`), both `float32`;
- LAEA/WGS-84, `+proj=laea +lat_0=55.0 +lon_0=10.0 +x_0=1950000.0
  +y_0=-2100000.0 +units=m +ellps=WGS84`;
- **the grid's north-west corner is model (0, 0)**, which is what the false origin
  is for and what the ODIM corner attributes say: `LL` to `UR` spans exactly
  3,800,000 x 4,400,000 m, i.e. exactly 3,800 x 4,400 cells of 1 km, so those are
  outer corners. The COG's `ModelTiepoint` instead reports that corner *minus half
  of each product's own pixel* (-500.0002714 / -1000.0002714, with a bit-identical
  residual tail across the two files) — a converter that read the ODIM corners as
  pixel centres. The baker pins the grid and requires the tiepoint to equal
  `corner - half a pixel`, so an upstream fix fails the bake rather than moving
  the field 500 m. Following the tiepoint instead would also put the two products'
  rasters 500 m apart, at which point a NIMBUS cell is not an aggregate of CIRRUS
  cells at all;
- `GDAL_NODATA = -9999000` means **no radar coverage**; a `NaN` sample is ODIM
  `undetect`, meaning covered with nothing detected. The two are different facts
  and only the second is dry.

Coverage is static in shape and not to the cell: 50.34 / 50.34 / 50.21 / 50.22 %
of the domain over four frames spanning 18 hours on 2026-08-10, with the union
and intersection of those masks differing by 21,936 cells (0.13 % of the domain).
It is therefore read per frame from the nodata sentinel, never from a committed
mask. Structurally uncovered: central and southern Italy, all of Greece, Albania,
North Macedonia, southern Bulgaria, Ukraine east of Lviv, Belarus — which is why
"OPERA lands" must never be read as "Europe is covered".

## NOAA GFS: worldwide v1 floor

Use NODD objects and indexes:

```text
https://noaa-gfs-bdp-pds.s3.amazonaws.com/
  gfs.YYYYMMDD/CC/atmos/gfs.tCCz.pgrb2.0p25.fFFF[.idx]
```

Cycles are nominally 00, 06, 12, and 18 UTC. A cycle is selectable only after
all forecast hours needed for one publication are present and validate. Never
choose a run from wall-clock arithmetic alone.

Select the exact consecutive index span for the currently duplicated
`APCP:surface:0-N hour acc fcst` entries. The captured index advertised two
indistinguishable records. Fetch both complete messages and require their
decoded fields to be identical; do not pick an undocumented first or second
occurrence.

Pinned contract:

- discipline/category/parameter `0/1/8` (`APCP` at surface);
- global 0.25-degree regular latitude/longitude grid, 1,038,240 points;
- exact 1,440 x 721 Section-3 geometry from `90,0` to `-90,359.75`,
  `Di=Dj=0.25` degrees and scanning mode `0x00`, including the captured Earth
  shape/flags;
- product template 4.8;
- complex packing representation template 5.3;
- cumulative kg/m2, numerically mm of liquid precipitation;
- finite, nonnegative fields.

Hourly de-accumulation is run-scoped:

```text
hour 1 of run R = cumulative(R, f001) - zero
hour N of run R = cumulative(R, fNNN) - cumulative(R, f(N-1))
```

Both operands must have the same reference time and grid, and forecast hours
must be consecutive. The validator parses the PDT 4.8 interval end/range and
requires the caller's selected forecast hour to equal the byte-derived lead;
the `.idx` label is not trusted as temporal identity. At a run transition,
validate and publish the new run as
a complete unit; never subtract the prior run's last field. A decrease is a
contract failure, not dry weather. The Rust tests pin the zero baseline and
reject cross-run subtraction.

The captured f003 object was 539,185,590 bytes; its exact two-message APCP span
was 640,466 bytes and appeared at 09:32:31 for the 06Z run. That value is one
fixture, not an upper bound: the same run's f004/f005/f006 spans were 688,950,
723,372, and 753,828 bytes. All 24 selected spans totaled 12,299,954 bytes,
with f006 as the high-water span. The live reproduction recomputes and records
the total/high-water hour for every selected run, fails above 15,500,000 bytes,
and reports 3,200,046 bytes of headroom for this capture. Four daily cycles are
therefore budgeted at no more than 62.0 MB before small index requests.
Decoding the captured f003 duplicates and proving equality took 0.04 s and
25.8 MB peak RSS.

## IMERG Early: explicit v1 NO-GO

NASA documents IMERG Early V07B as a half-hourly 0.1-degree near-real-time
product with approximately four-hour latency. It is scientifically suitable
as a later worldwide observation layer, but the current evidence does not
prove an unattended operational fetch:

- NASA PPS registration and credentials are required;
- this spike had no approved service credentials and therefore did not perform
  or pretend to perform an authenticated download;
- unattended credential renewal, rate behavior, redistribution of transformed
  baked output, and exact HDF5 decode/latency remain unmeasured.

Therefore IMERG Early is an explicit **NO-GO for v1**. Worldwide v1 uses a
GFS-only forecast floor. It must not synthesize an observation timestamp,
backdate GFS, or emit an empty IMERG frame that looks like dry weather.

Re-open this decision only after one-time PPS registration yields stable
non-interactive credentials, a live Rust capture verifies the exact V07B file
contract and real publication times, and NASA redistribution/attribution for
the transformed output is recorded. Loss of future IMERG access must continue
to fall back honestly to GFS-only.

## MET Norway: phone-only point forecast

MET remains the only direct phone provider in this architecture:

```text
GET https://api.met.no/weatherapi/locationforecast/2.0/complete
    ?lat=LATITUDE_4DP&lon=LONGITUDE_4DP&altitude=GROUND_METRES
User-Agent: OpenBikeComputer/VERSION https://github.com/timohueser/OpenBikeComputer
Accept-Encoding: gzip
```

Round coordinates to at most four decimals and always identify the app. Map
the first 24 `timeseries` records:

| Canonical field | Provider field | Availability |
| --- | --- | --- |
| temperature | `instant.details.air_temperature` | required |
| precipitation amount | `next_1_hours.details.precipitation_amount` | required |
| condition | `next_1_hours.summary.symbol_code` | required for an accepted record |
| wind direction/speed | `instant.details.wind_from_direction`, `wind_speed` | required |
| gust | `instant.details.wind_speed_of_gust` | optional |
| precipitation probability | `next_1_hours.details.probability_of_precipitation` | optional |

Validate advertised units. Never infer missing gust or probability. Oslo
supplied both optional fields in all 24 captured hours; Manila supplied neither
in any of the 24 hours. The provider-neutral OBCW model already represents
unavailable values, so this geographic difference is not a launch blocker.

Timestamps must be canonical UTC RFC3339 seconds and exactly one hour apart.
An optional key may be absent, but when present it must be a finite numeric
value in range; strings, objects, and null are malformed rather than silently
treated as unavailable.

Freeze this `symbol_code` mapping to WX2 `OBCWeatherCondition`; `_day`,
`_night`, and `_polartwilight` variants keep the same semantic condition:

| MET code family | OBC condition |
| --- | --- |
| `clearsky*` | `clear` |
| `fair*` | `mostlyClear` |
| `partlycloudy*` | `partlyCloudy` |
| `cloudy`, `fog` | `overcast`, `fog` respectively |
| `lightrain` | `drizzle` |
| `rain`, `heavyrain` | `rain` |
| rain `*showers*` without thunder | `showers` |
| `*sleet*` without thunder | `sleet` |
| `*snow*` without thunder | `snow` |
| every documented `*andthunder*` rain/sleet/snow variant | `thunderstorm` |
| new/unknown nonempty code | `unavailable` (never guessed) |

An empty/non-string `symbol_code` is malformed and rejects the record.

MET has no accepted mapping to WX2 `hail` or `wind`; those remain unavailable.

Honor `Expires` and revalidate with `If-Modified-Since` using the exact
`Last-Modified` value. Retain a visibly timestamped cache on network/provider
failure. The direct request discloses the phone IP and requested coordinates to
MET; WX4 must keep the privacy declaration and provider attribution aligned.

## Capacity and monthly cost gate

The measured source-ingress budget is approximately:

| Source | Captured bytes per selection | Nominal selections/day | Projected ingress/day |
| --- | ---: | ---: | ---: |
| DWD RV full tar | 2,017,280 | 288 | 581 MB |
| MRMS full object | 456,264 | 720 | 329 MB |
| ICON-EU f000..f011 | 4,134,603 | 4 | 16.5 MB |
| HRRR eight byte ranges | 330,351 | 24 | 7.9 MB |
| GFS first 24 APCP spans | 12,299,954 captured; <=15,500,000 enforced | 4 | <=62.0 MB |

The total is about 1.0 GB/day of upstream ingress before HTTP metadata. This is
small enough for a modest always-on host, but MRMS's 161 MB spike RSS requires
a deliberate concurrency/memory policy.

The epic's steady-state infrastructure ceiling is EUR 10/month. Freeze the WX1
cost gate as:

- stateless baker/VPS allocation: **at most EUR 7/month**; WX18 must select and
  record the actual plan, included transfer, taxes, and region before launch;
- Cloudflare R2: Standard storage, not Infrequent Access, because weather
  expires within roughly 48 hours and IA has a minimum-storage-duration cost;
- keep the rolling published weather set below 1 GB and operations inside the
  free allowances where feasible;
- no upstream source in the selected v1 set has a per-request fee.

At the R2 prices checked on 2026-08-09 (Standard storage USD 0.015/GB-month,
10 GB-month free, free internet egress, and monthly request allowances), the
projected weather-only R2 charge is USD 0. The frozen combined projection is
therefore at most EUR 7/month plus any exchange-rate/tax difference verified in
WX18, leaving at least EUR 3/month headroom. A plan above that value or a
storage/operation forecast outside the free allowances re-opens the gate.

## Failure, cache, and publication rules

- Fetch mutable aliases/index listings only for discovery. Persist the resolved
  immutable run/object name and response metadata.
- Retry timeouts and 5xx responses with bounded exponential backoff and jitter;
  honor `Retry-After`. A 4xx, changed MIME/schema/template, ambiguous index, or
  invalid numeric field is not retried as successful weather.
- Validate every source component needed for a publication before updating the
  latest manifest. Keep the previous complete manifest on partial failure.
- Expired data retains its true timestamps and becomes stale/unavailable under
  later product policy. It never rolls forward merely because the baker failed.
- Bound concurrent large decodes. In particular, do not run multiple full MRMS
  decodes on a small host until WX5 proves the memory budget.
- Collect per-source fetch bytes, status, publication lag, decode duration,
  peak/working memory, selected run, and validation failure reason. Do not log
  credentials or route/user coordinates.

## License and attribution manifest inputs

The baker must carry these source facts into the later manifest rather than
leaving attribution to UI guesswork:

| Source | Terms | Attribution input |
| --- | --- | --- |
| DWD RV / ICON-EU | DWD Open Data, CC BY 4.0 | `Source: Deutscher Wetterdienst (DWD); modified/quantized by OpenBikeComputer` plus DWD legal and CC BY links |
| NOAA MRMS / HRRR / GFS | NOAA NODD public-use U.S. government data | `Source: NOAA/NCEP <product>; modified/quantized by OpenBikeComputer; no NOAA endorsement is implied` |
| EUMETNET OPERA CIRRUS / NIMBUS | CC BY 4.0 (stated in each object's own `GDAL_METADATA`) | `Source: EUMETNET OPERA <composite> (CC BY 4.0); modified/quantized by OpenBikeComputer`, and for CIRRUS the Marshall-Palmer conversion and its column-max calibration are named in the same string |
| NASA IMERG, if later approved | NASA Earth science data policy and GPM citation rules | exact product/citation and transformation notice; not present in v1 manifests |
| MET Locationforecast | NLOD 2.0 or CC BY 4.0 | `Data from MET Norway` (phone-side attribution, not an R2 baker source) |

The exact fixture retrieval URLs/ranges, content hashes, full-vs-subset status,
and terms are in
[`host/obc-wx-bake/tests/fixtures/README.md`](../../host/obc-wx-bake/tests/fixtures/README.md).

## Reproduction and checked evidence

Run immutable fixture verification:

```bash
cargo test -p obc-wx-bake
```

Run a live host reproduction against the real upstreams (it discovers current
runs, bounds every object before download, requires exact 206 `Content-Range`
for byte-range reads, and fails the cycle on any contract surprise):

```bash
cargo run --release -p obc-wx-bake -- cycle --store /tmp/obc-weather-evidence
```

The MET Locationforecast contract has no baker-side reproduction: MET is a
phone-only provider (WX4), and this record's MET evidence stands as captured.

Checked-in tests cover:

- DWD raw projection/extents, internal time/name identity, native resolution,
  scale, dry/missing values, and a convective field;
- MRMS PNG packing, dry/missing/no-coverage distinctions, and severe rain;
- HRRR exact index/lead selection, full Lambert geometry, and representation
  5.3 spatial differencing;
- ICON-EU CCSDS decoding, exact geometry/interval cadence, and the tightly
  bounded packing-roundoff rule;
- GFS exact geometry/interval lead, representation 5.3, duplicate-record
  equality, and run-boundary-safe cumulative
  de-accumulation;
- deterministic fixture cycles per product: byte-stable published trees,
  corrupt upstream publishing nothing, unchanged upstream moving no bytes, and
  every published cell equal to the quantized nearest-neighbour source cell.

The MET condition mapping above is implemented and tested on the phone side
(WX4), not in the baker.

## Follow-up gates

- WX5/WX6 own the production baker, normalized domain types, timeline policy,
  baked byte format, R2 keys/manifests, atomic publishing, and end-to-end
  deterministic vectors.
- WX13 must render source/license/modified-data attribution from manifest data
  and keep MET's direct-phone attribution visible where required.
- WX18 must verify the actual VPS/R2 invoice model and operational alerts under
  the EUR 10/month cap.
- IMERG remains closed until the explicit re-open criteria above are met.
- Any Alaska/Hawaii or other non-CONUS high-resolution source is a new measured
  decision, not an implicit extension of MRMS/HRRR.

There is no UI in WX1 and no physical-iPhone acceptance requirement for the
server decoders. Phone behavior remains in the separately scoped MET adapter
and later provider-neutral object consumption work.
