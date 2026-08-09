# WX1 weather source contracts

Status: **DWD/GFS accepted; hourly provider blocked**

Issue: [#1186](https://github.com/timohueser/OpenBikeComputer/issues/1186)
Evidence captured: 2026-08-09 UTC from a macOS host in Germany

This record narrows the provider-selection risks for #1185 and pins the host-
validated contracts. It does not claim a physical-iPhone network or energy
measurement; that remaining evidence is spelled out under
[On-device evidence still required](#on-device-evidence-still-required).

## Decisions

| Need | Decision | Endpoint/product | Result |
| --- | --- | --- | --- |
| German rain nowcast | DWD stable RV alias through numeric WCS GeoTIFF | `dwd__Niederschlagsradar` | **GO** |
| Worldwide hourly weather candidate | MET Norway Locationforecast 2.0 `complete` | `/weatherapi/locationforecast/2.0/complete` | **NO-GO for a direct native client**; reconsider with a proxy or written provider confirmation |
| Worldwide rain fallback | NOAA/NCEP GFS 0.25-degree APCP through NOMADS bbox filter | `gfs.tCCz.pgrb2.0p25.fFFF` | **GO** |
| Retired German assumptions | DWD RADVOR RE/RQ | retired after 2026-02-28 | **NO-GO** |
| Germany-wide client download | DWD RV OpenData HDF5 tar | `composite_rv_LATEST.tar` | **NO-GO for routine fetch; verification/disaster fallback only** |
| Styled rain pixels | DWD WMS PNG | `dwd:Niederschlagsradar` | **NO-GO for data; presentation/diagnostics only** |

MET's Terms of Service say browsers and mobile apps should not contact the API
directly. Their low-volume exception is written specifically for simple cross-
origin JavaScript requests and direct image links, not a native Locationforecast
JSON client setting an identifying User-Agent. The terms also require prior
agreement above 20 requests/second across all installations. Therefore MET does
not satisfy this epic's zero-backend native-client invariant without written
provider confirmation. A different hourly provider or an approved proxy design
must be selected before an hourly adapter or WX4 integration is implemented.

The accepted DWD and GFS launch paths need no API key, paid account, OBC server,
or scraping.

WX4 remains blocked on an explicit product/architecture decision for hourly
conditions. This spike does not silently select one of these alternatives:

1. Obtain written MET confirmation that OBC's identified, cache-respecting
   native request pattern is permitted, including its aggregate traffic plan.
2. Change the zero-backend scope and introduce an OBC caching proxy for MET,
   with an operational owner, privacy review, capacity budget, and failure plan.
3. Extend the direct NOAA GFS subset to hourly temperature and wind semantics.
   GFS does not supply MET's precipitation probability or canonical condition
   symbol contract, so those fields must remain unavailable unless the product
   requirements are explicitly changed; they may not be guessed from APCP or
   substituted with fake precision.

## Architectural boundary for WX2+

Do not model DWD, MET, and GFS as interchangeable implementations of one large
`WeatherService`. They provide different facts:

- `HourlyForecastSource` returns point/route hourly conditions.
- `PrecipitationGridSource` returns a georeferenced grid with native cell size,
  run time, valid interval, and missing cells.
- A `WeatherRepository` owns caches and regional policy: DWD inside supported
  coverage, GFS elsewhere, and the separately approved provider for hourly
  conditions.

Add the approved provider clients and policies in a dedicated `OBCWeather` SwiftPM
target, depending on `OBCDomain` and `OBCFormats`. `OBCFormats` owns only wire
decoding, including the audited GRIB2 subset added by this spike. `OBCUI` must
consume the repository protocol and canonical models, never provider JSON,
GeoTIFF, GRIB2, or `URLSession` directly. The app composition root chooses the
repository. This keeps provider replacement out of view models and avoids
turning the BLE-specific `DeviceTransport` abstraction into a general network
client.

Preserve provider provenance on canonical values. A merged timeline may not
silently mix fields from different runs or providers.

## DWD Germany: RV through WCS

### Why RV, not RE/RQ

DWD states that RADVOR ended after 2026-02-28 and RE/RQ are no longer supplied.
The maintained RV product is a 1 km x 1 km Germany composite of precipitation
accumulated over five minutes, updated every five minutes, with forecasts to
+120 minutes. The popular `dwd:Niederschlagsradar` layer is DWD's forward alias
to the currently preferred rain-radar product (RV at capture time).

Official product/discontinuation references:

- [DWD RADVOR retirement notice](https://www.dwd.de/DE/leistungen/radvor/radvor.html)
- [DWD popular radar layers](https://www.dwd.de/DE/leistungen/radarprodukte/radarlayer.html)
- [DWD RV product description](https://www.dwd.de/DE/leistungen/radarprodukte/radarkomposit_rv.pdf?__blob=publicationFile&v=2)
- [DWD 2025 HDF5 migration notice](https://www.dwd.de/DE/leistungen/opendata/neuigkeiten/opendata_august2025_1.html)

### Discovery and request contract

Discover the current run and advertised valid-time domain first:

```text
GET https://maps.dwd.de/geoserver/dwd/wcs
    ?service=WCS
    &version=2.0.1
    &request=DescribeCoverage
    &coverageId=dwd__Niederschlagsradar
```

Read the `wcsgs:DimensionDomain` named `REFERENCE_TIME`; use its `default` as
the run. A coherent frame request must pin both the valid time and that vendor
dimension:

```text
GET https://maps.dwd.de/geoserver/dwd/wcs
    ?service=WCS
    &version=2.0.1
    &request=GetCoverage
    &coverageId=dwd__Niederschlagsradar
    &subset=Lat(LAT_MIN,LAT_MAX)
    &subset=Long(LON_MIN,LON_MAX)
    &subset=time("VALID_TIME_UTC")
    &subset=REFERENCE_TIME("RUN_TIME_UTC")
    &format=image/tiff;application=geotiff
```

All `subset` and `format` values must be URL encoded. The stable WMS alias is
documented publicly; WCS exposure of the same alias was verified through live
GetCapabilities/DescribeCoverage. Because DWD does not separately promise the
WCS alias forever, discovery must fail closed if it disappears instead of
silently switching products.

For nine useful frames, request run +0, +15, +30, +45, +60, +75, +90, +105,
and +120 minutes. The +0 field covers the preceding five-minute observation
interval; each forecast field covers the five minutes ending at its valid time.

### Numeric and grid semantics

The selected response is a one-band, 32-bit floating-point GeoTIFF in EPSG:4326.
The source product is a 1,100 x 1,200 stereographic grid with 1,000 m cells. The
WCS crop keeps approximately one output sample per native cell and the live
convective maximum matched the raw HDF5 value exactly, demonstrating
nearest-cell values rather than interpolated presentation colors.

The live `2026-08-09T08:20Z` raw HDF5 analysis recorded:

```text
quantity=ACRR
start=2026-08-09T08:15:00Z
end=2026-08-09T08:20:00Z
gain=0.0009999999317806213
offset=-0.0009999999317806213
undetect=0
nodata=4294967295
```

The physical field is five-minute accumulation in millimetres. WCS returns the
gain/offset-applied values. Convert only after missing-value handling:

```text
4294967296.0 (GeoTIFF GDAL_NODATA) -> missing
-999.0                                 -> missing
-0.001 (ODIM undetect after offset)    -> dry, 0 mm
value >= 0                             -> millimetres per 5 minutes
display rate                           -> value * 12 mm/hour
```

Do not treat missing as dry. Do not bilinearly resample or store/render cells as
more precise than 1 km. Keep the returned transform and cell footprint.

DWD's WCS DescribeCoverage incorrectly advertised the band unit as
`W.m-2.Sr-1` during the capture. That generic metadata is rejected for unit
selection; the RV product contract, raw ODIM quantity/gain/offset, and captured
cross-check above are authoritative. A change to any of those assertions must
stop decoding and re-open this decision.

### WMS and raw OpenData comparison

WMS has the documented alias and a convenient time dimension, but its PNG
output applies a presentation style. Reverse-mapping colors would lose numeric
precision and couple the app to a legend. It is not selected.

The maintained raw path is:

```text
https://opendata.dwd.de/weather/radar/composite/rv/composite_rv_LATEST.tar
```

At capture it contained 25 HDF5 files named
`composite_rv_20260809_0820_PPP-hd5`, with `PPP` from `000` to `120` in
five-minute steps. This path is the reference used to audit WCS semantics. It
requires a Germany-wide tar and an HDF5 decoder, so it loses to the route crop
on bandwidth and iOS complexity. The old RADOLAN file assumption is invalid;
DWD retired old composite formats after 2025-11-30.

### Caching, retry, budget, and failure

The WCS responses observed no `ETag`, `Last-Modified`, `Expires`, or
`Cache-Control`. Cache immutably by `(coverage, reference_time, valid_time,
bbox)` and discover a new run no more often than every five minutes while the
weather surface is active. A newer run must replace the whole nine-frame set;
never mix frames from two references.

The raw OpenData response did provide `Last-Modified`, `ETag`, `Content-Length`,
and byte ranges. At 08:25 UTC the 08:20 bundle was 768,000 bytes and had
`Last-Modified: 08:23:12`, an observed publication delay of 3 min 12 s. WCS was
confirmed by 08:24:40, an upper-bound delay of 4 min 40 s.

DWD publishes no request-rate allowance and no SLA. Limit a refresh to four
concurrent frame requests. Retry timeouts/5xx at most twice with exponential
backoff and jitter; honor `Retry-After` on 429. On failure, retain a visibly
timestamped previous complete run. A 4xx/OGC exception, changed content type,
or changed raster contract is not rain and is not retried as success.

Measured 96 km route crop, bbox `52.5016..53.3656 N,
6.8560..8.2932 E`, HTTP/2:

| Request | Bytes | Total latency |
| --- | ---: | ---: |
| One GeoTIFF frame | 50,586 | 0.100-1.862 s |
| Nine frames, sequential | 455,274 | 3.266 s |
| Nine-frame median | 50,586 | 0.159 s |
| Raw Germany RV tar at same run | 768,000 | 0.152 s |

The route crop returned 104 x 104 values. A rain cell held 5.564 mm/5 min
(66.768 mm/h), providing the convective fixture.

### License and attribution

DWD open geodata and geodata services are CC BY 4.0 and require a source note.
Show, adjacent to the radar surface:

> Quelle: Deutscher Wetterdienst

Link the attribution to [DWD legal notices](https://www.dwd.de/DE/service/rechtliche_hinweise/rechtliche_hinweise.html)
and provide the [CC BY 4.0 license](https://creativecommons.org/licenses/by/4.0/)
in the app's data-sources screen. If OBC averages or otherwise changes the data,
use DWD's prescribed change wording, for example “Datenbasis: Deutscher
Wetterdienst, Einzelwerte gemittelt”.

## MET Norway hourly candidate: no-go for direct native use

### Request and fields

Use `complete`, not `compact`, because precipitation probability and gusts are
part of this contract:

```text
GET https://api.met.no/weatherapi/locationforecast/2.0/complete
    ?lat=LATITUDE_4DP
    &lon=LONGITUDE_4DP
    &altitude=GROUND_METRES
User-Agent: OpenBikeComputer/VERSION https://github.com/timohueser/OpenBikeComputer
Accept-Encoding: gzip
```

Coordinates must be rounded/truncated to at most four decimal places. Altitude
is optional but should be supplied when route ground elevation is known.

For each of the first 24 `properties.timeseries` records, pin:

| Canonical field | JSON path | Unit/meaning |
| --- | --- | --- |
| time | `time` | UTC instant and start of the next-hour period |
| temperature | `data.instant.details.air_temperature` | degrees Celsius |
| precipitation amount | `data.next_1_hours.details.precipitation_amount` | mm over `[time,time+1h)` |
| precipitation probability | `data.next_1_hours.details.probability_of_precipitation` | percent over the same hour |
| condition | `data.next_1_hours.summary.symbol_code` | canonical MET Weathericon 2 code |
| wind from | `data.instant.details.wind_from_direction` | degrees where wind comes from; 0 north, 90 east |
| wind speed | `data.instant.details.wind_speed` | m/s at 10 m, 10-minute average |
| wind gust | `data.instant.details.wind_speed_of_gust` | m/s at 10 m, maximum 3-second gust |

Validate the corresponding units in `properties.meta.units`. If any required
path is absent in the first 24 hours, omit/fail that record; do not substitute a
6- or 12-hour aggregate. Gust is documented as unavailable beyond roughly 60
hours, which does not affect this 24-hour contract.

Preserve the raw `symbol_code` as the canonical condition. UI mapping strips
only `_day`, `_night`, and `_polartwilight` for grouping, retains the phase for
icons, maps intensity prefixes (`light`, no prefix, `heavy`) separately, and
retains `andthunder` as an orthogonal thunder flag. Unknown codes render as
unknown while preserving the provider code. Do not reverse-engineer symbols
from cloud/precipitation fields; MET says the supplied symbol is a computed
period value and Weathericon filenames map directly to `symbol_code`.

Official references:

- [Locationforecast 2.0 documentation](https://api.met.no/weatherapi/locationforecast/2.0/documentation)
- [Locationforecast data model](https://docs.api.met.no/doc/locationforecast/datamodel.html)
- [Locationforecast FAQ and period semantics](https://docs.api.met.no/doc/locationforecast/FAQ.html)

### Caching, traffic gate, failure, and privacy

The Oslo capture returned 86,517 decoded JSON bytes / 6,795 gzip bytes in
0.130 s, with:

```text
Last-Modified: Sun, 09 Aug 2026 08:27:39 GMT
Expires: Sun, 09 Aug 2026 08:59:17 GMT
provider meta.updated_at: 2026-08-09T07:30:33Z
```

An exact `If-Modified-Since` request returned 304, zero body bytes, in 0.110 s.
Cache until `Expires`, then revalidate using the exact `Last-Modified` value.
Never perform HEAD before GET. Do not refresh while the app is unused, and do
not poll mobile background weather more often than ten minutes.

Treat 203 as deprecation requiring attention; 304 reuses the cache; 400 is a
request bug; 403 commonly means missing identification; 429 stops traffic and
backs off; 5xx retains a timestamped cached forecast. Check status and content
type before decoding.

MET records the user's IP address and requested coordinates in Oslo when a
client connects directly. If MET confirms a direct native adapter, the privacy
policy and App Store privacy disclosure must describe that transfer before it
ships.

The direct-client prohibition and aggregate-volume gate come from MET's
official [Terms of Service](https://docs.api.met.no/doc/TermsOfService.html).
Do not implement the direct adapter based only on the successful host probe.
Written confirmation would also need to address the 20 requests/second
aggregate threshold and any subsequent terms change.

### License and attribution

Locationforecast data is offered under NLOD 2.0 and CC BY 4.0. Show:

> Data from MET Norway

Link to [MET Weather API](https://api.met.no/) and include the
[licensing policy](https://docs.api.met.no/doc/License.html) plus
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) in data sources.
Weathericon artwork is MIT licensed; using only the code does not bundle icons.

## NOAA/NCEP GFS global precipitation

### Run and request selection

GFS cycles nominally run at 00, 06, 12, and 18 UTC. Never select from wall-clock
math alone. Starting with the newest nominal cycle not in the future, require
HTTP 200 for the index of the highest forecast hour needed:

```text
https://nomads.ncep.noaa.gov/pub/data/nccf/com/gfs/prod/
  gfs.YYYYMMDD/CC/atmos/gfs.tCCz.pgrb2.0p25.fFFF.idx
```

At 08:26 UTC the 06Z indexes were still unavailable (HTTP 403), so the complete
00Z run was correctly selected. Pin the chosen date/cycle for every timeline
field. Forecast hours are f001 through f120 hourly, then f123 through f384 in
three-hour steps, as observed in the live NOMADS inventory and described by
NCEP's product inventory.

Exact filter template:

```text
GET https://nomads.ncep.noaa.gov/cgi-bin/filter_gfs_0p25.pl
    ?file=gfs.tCCz.pgrb2.0p25.fFFF
    &lev_surface=on
    &var_APCP=on
    &subregion=
    &leftlon=WEST
    &rightlon=EAST
    &toplat=NORTH
    &bottomlat=SOUTH
    &dir=/gfs.YYYYMMDD/CC/atmos
```

Official references:

- [NOMADS GFS 0.25-degree filter](https://nomads.ncep.noaa.gov/cgi-bin/filter_gfs_0p25.pl)
- [NCEP GFS file inventory](https://www.nco.ncep.noaa.gov/pmb/products/gfs/)
- [NCEI GFS description](https://www.ncei.noaa.gov/products/weather-climate-models/global-forecast)

### Accumulation and decode contract

`APCP` is accumulated total precipitation at the surface in kg/m2, numerically
equivalent to millimetres of liquid water. The filter can return two APCP
messages. For f001-f006 they were byte-identical 0-to-hour cumulative fields;
later hours include an interval bucket beside the 0-to-hour cumulative field.

Select product template 4.8 records whose accumulation `startStep == 0`.
De-duplicate exact copies and reject conflicting copies. For two fields from the
same cycle and grid:

```text
rate_mm_per_hour = (cumulative_mm(t1) - cumulative_mm(t0)) / (t1 - t0 hours)
```

Use zero as the earlier cumulative field for f001. A negative difference, run
mismatch, geometry mismatch, or missing endpoint is invalid; do not clamp it
into believable dry weather.

The checked-in pure-Swift decoder accepts only the audited NOMADS subset:

- GRIB edition 2, meteorological discipline 0;
- grid template 3.0, regular latitude/longitude;
- product template 4.8, category 1 / parameter 8, surface accumulation;
- data template 5.0, simple packing including signed binary/decimal scales;
- no bitmap or an inline bitmap (missing bitmap cells become `nil`);
- GFS scanning modes 0 or 64.

Any other template fails closed. The decoder returns source scanning order,
native 0.25-degree increments, and optional values. It does not interpolate.
At the equator the cell spacing is roughly 28 km; it varies with latitude and
must never be drawn or sampled as a 1 km forecast.

The 96 km Manila test bbox (`14.17..15.03 N, 120.53..121.43 E`) returned a
3 x 4 grid. f006 was 430 bytes of internally simple-packed GRIB2, 0.599 s total
with 0.598 s time to first byte. Fetching f001-f024 sequentially measured 24
requests, 10,301 bytes, and 13.156 s (median 0.522 s). Production may use at
most four concurrent requests and caches the immutable selected run.

The filter response observed `Cache-Control: no-cache, private,
max-age=14400` and `Expires`, but no response validator. The source `.idx` had
`Last-Modified`. Probe indexes for cycle completeness, cache filtered files by
cycle/hour/bbox, and never infer that a non-200 response is zero rain. GFS is a
global complete model field; a GRIB bitmap zero is the only cell-level no-data
signal supported by this decoder.

### License and attribution

NOAA/NCEP GFS is U.S. government data in the public domain in the United States.
Do not imply NOAA endorsement. Show:

> Forecast data: NOAA/NCEP GFS

Link to the [NCEI GFS product page](https://www.ncei.noaa.gov/products/weather-climate-models/global-forecast).
NOAA asks users to acknowledge NOAA as the source; see the
[NCEI archive policy](https://www.ncei.noaa.gov/index.php/archive).

## Captured fixtures and provenance

Fixtures live in
`companion-ios/Packages/OBCKit/Tests/OBCFormatsTests/Fixtures/` and are copied
only into the test bundle.

| Fixture | What it retains | Original response evidence |
| --- | --- | --- |
| `dwd-rv-convective-rain.json` | Transform, response summary, sentinels, and a 7 x 7 numeric window with 5.564 mm/5 min cell | 50,586 B GeoTIFF, SHA-256 `a4b60cfa1656bf39622956029c59d2ba65999e492c1f1d2dda5d326c018d6cfd` |
| `dwd-rv-dry-nodata.json` | dry/zero values, GDAL no-data, and `-999` invalid cells | 50,586 B GeoTIFF, SHA-256 `5b82455d132d1173c53a6bec5242fe0a15b546f2469a4b1cb8c6c485e9d92791` |
| `met-locationforecast-oslo-24h.json` | exact 24 canonical records plus cache/provenance headers | 86,517 B JSON, SHA-256 `244b71b1136f70a89d882c217f1878717eb85b962de057b446cdee06c17362e5` |
| `gfs-manila-apcp-f006.grib2.b64` | exact 430-byte GRIB2 response, base64 encoded for reviewability | SHA-256 `be6705b5d5a3e56b5a11cf42295ea1804e5b4a49b237082d877df3612e385566` after decoding |

The DWD and MET JSON files are deliberately small, deterministic extracts, not
mislabelled byte-for-byte provider responses. They retain the original response
hash, request coordinates/times, transform, and captured fields needed for the
contract tests. The GFS fixture is exact apart from reversible base64 encoding.
All are redistributable under the provider terms above and carry attribution in
the fixture README.

Run live reproduction:

```bash
tools/weather-source-spike/reproduce.sh /tmp/obc-weather-evidence
```

Run fixture and decoder verification:

```bash
cd companion-ios/Packages/OBCKit
swift test --filter 'GRIB2PrecipitationDecoderTests|WeatherFixtureContractTests'
```

## On-device evidence still required

The decoder is Foundation-only Swift in the iOS 17-compatible `OBCFormats`
target, passes host Swift 6 tests against a non-constant, simple-packed NOAA
fixture, and compiles in the full app for an arm64/x86_64 iOS 17 simulator
destination. That proves a viable source-level iOS path; it is not a physical-
device measurement. The paired iPhone was reported offline by Xcode during this
capture.

Before closing #1186, run a Release build on a physical iPhone over Wi-Fi and
cellular and attach:

1. direct `URLSession` status/headers for one DWD frame and GFS f006 using the
   production User-Agent; include MET only after written direct-use confirmation
   or measure the approved replacement/proxy;
2. wall-clock DNS/connect/TTFB/download/decode timings from `URLSessionTaskMetrics`;
3. response bytes, peak resident memory, and decoded grid checksum;
4. a 24-hour GFS timeline and nine-frame DWD run without mixed references;
5. an Instruments Energy Log for one refresh and a cancelled/background case;
6. confirmation that ATS, IPv6-only networking, and cellular constrained mode
   work without a server or key.

Until that attachment exists, this spike should use `Refs #1186`, not an
automatic closing keyword.

## Documents and product declarations made stale by eventual implementation

- The app privacy policy and App Store privacy answers must add provider network
  access generally. Add the coordinates/IP transfer to MET Norway only if that
  path is explicitly approved.
- The in-app About/Data Sources screen must add DWD/GFS and the selected hourly
  provider's attribution strings, license links, modification notices,
  timestamps, and the NOAA non-endorsement.
- `companion-ios/CLAUDE.md` and the SwiftPM layer diagram become stale when the
  proposed `OBCWeather` target is introduced; update them in that adapter PR.
- Any weather UI specification must state provider, run/valid time, stale state,
  and native spatial resolution; no current UI document does.
