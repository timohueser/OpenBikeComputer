# WX1 server-source fixtures

These are immutable source-contract captures for
[#1186](https://github.com/timohueser/OpenBikeComputer/issues/1186). The
host-only Rust spike decodes them; none is linked into the companion app or
device firmware. All retrieval timestamps are UTC.

## Byte-for-byte provider material

| File | Retrieval and exact source | Retained material | Bytes | SHA-256 |
| --- | --- | --- | ---: | --- |
| `dwd-rv-20260809-1130-f000.h5` | 2026-08-09T11:34:01Z; member `composite_rv_20260809_1130_000-hd5` of [DWD run tar](https://opendata.dwd.de/weather/radar/composite/rv/composite_rv_20260809_1130.tar). The full 2,017,280-byte tar had SHA-256 `2dd0ffb1da15faf4562f11615611f618e9aae476e1d6d0c4e16cba74942064bb`. | One **complete HDF5 member** extracted without modification from the full tar; it is a lawful subset of that archive. | 82,070 | `58e99eb26009795b89d5f696bcf5ccaad2eda2ba98c839d9374d00b56a127d7c` |
| `mrms-conus-20260808-020000.grib2.gz` | 2026-08-09T11:38:30Z; [exact NOAA NODD object](https://noaa-mrms-pds.s3.amazonaws.com/CONUS/PrecipRate_00.00/20260808/MRMS_PrecipRate_00.00_20260808-020000.grib2.gz). | **Complete object**, byte for byte. | 456,264 | `49e728bf1c058233afbfa095e6922baceb226f9e1fdcf3be0451a310a0571730` |
| `icon-eu-20260809T06-f001.grib2.bz2` | 2026-08-09T11:39:35Z; [exact DWD ICON-EU f001 object](https://opendata.dwd.de/weather/nwp/icon-eu/grib/06/tot_prec/icon-eu_europe_regular-lat-lon_single-level_2026080906_001_TOT_PREC.grib2.bz2). | **Complete object**, byte for byte. | 300,675 | `b5a0b2db80bc3aa3b22cda9ed6c282aaa0551a96aac4a9b4c632b83acd0589ff` |
| `icon-eu-20260809T06-f002.grib2.bz2` | 2026-08-09T11:37:08Z; [exact DWD ICON-EU f002 object](https://opendata.dwd.de/weather/nwp/icon-eu/grib/06/tot_prec/icon-eu_europe_regular-lat-lon_single-level_2026080906_002_TOT_PREC.grib2.bz2). | **Complete object**, byte for byte. Paired with f001 to pin cumulative-field de-accumulation. | 315,543 | `c289a5358e9023d695a906f15912319c9faa28bb79f17e57b3ec4ef42b538f2a` |
| `hrrr-conus-20260808T00-f002.idx` | 2026-08-09T11:37:38Z; [exact NOAA NODD index](https://noaa-hrrr-bdp-pds.s3.amazonaws.com/hrrr.20260808/conus/hrrr.t00z.wrfsubhf02.grib2.idx). | **Complete text index**, byte for byte. | 11,829 | `7e224d6bd8fc3bcff0b424b4bfa229e81988aca4797f7d393629cb3890527141` |
| `hrrr-conus-20260808T00-prate-f002-t120.grib2` | 2026-08-09T11:38:06Z; [NOAA NODD HRRR f002 object](https://noaa-hrrr-bdp-pds.s3.amazonaws.com/hrrr.20260808/conus/hrrr.t00z.wrfsubhf02.grib2), whose complete size was 186,047,054 bytes. | **Exact HTTP byte-range subset** `165672006-165714866`, selected uniquely by `:PRATE:surface:120 min fcst:`. It contains one complete GRIB2 message and no rewritten bytes. | 42,861 | `6c83587aaf7e60fa37bc453d338e547dee27f70ca18ff7c78c2808e1089677b5` |
| `gfs-global-20260809T06-f003.idx` | 2026-08-09T11:38:07Z; [exact NOAA NODD index](https://noaa-gfs-bdp-pds.s3.amazonaws.com/gfs.20260809/06/atmos/gfs.t06z.pgrb2.0p25.f003.idx). | **Complete text index**, byte for byte. | 40,471 | `0d8fe2f26bdbd09f6d3432816fb972c9e8e2b92175aeee109dbcb6999e31e7ad` |
| `gfs-global-20260809T06-apcp-f003.grib2` | 2026-08-09T11:38:08Z; [NOAA NODD GFS f003 object](https://noaa-gfs-bdp-pds.s3.amazonaws.com/gfs.20260809/06/atmos/gfs.t06z.pgrb2.0p25.f003), whose complete size was 539,185,590 bytes. | **Exact HTTP byte-range subset** `427163736-427804201` for the two consecutive `:APCP:surface:0-3 hour acc fcst:` entries. It contains two complete GRIB2 messages; tests require their decoded fields to be identical. | 640,466 | `75b9c6c172cc7e63a47af5148db495bd49913f41c85a04c8d3be74ba577f39e0` |

## Deterministic MET extracts

These two files are lawful, deterministic JSON subsets rather than
byte-for-byte responses. Each stores the exact request coordinates, capture
timestamp, identifying User-Agent, response byte counts, cache metadata, and
the SHA-256 of the full decoded provider response in its `provenance` object.
It then retains the first 24 canonical hourly records used by the contract
test.

| File | Retrieval and exact request | Bytes | SHA-256 |
| --- | --- | ---: | --- |
| `met-locationforecast-oslo-24h.json` | 2026-08-09T08:35:28Z; [Locationforecast complete](https://api.met.no/weatherapi/locationforecast/2.0/complete?lat=59.9139&lon=10.7522&altitude=23). Full decoded response SHA-256: `244b71b1136f70a89d882c217f1878717eb85b962de057b446cdee06c17362e5`. | 8,083 | `55c8f8fb22719a9bc1c5d11c30872786afa290d38834537619478dee70e065c5` |
| `met-locationforecast-manila-24h.json` | 2026-08-09T09:46:14Z; [Locationforecast complete](https://api.met.no/weatherapi/locationforecast/2.0/complete?lat=14.5995&lon=120.9842&altitude=16). Full decoded response SHA-256: `9e63ec334ddd7a36c33caf4a28d49b2e0ea8373317eec20e37f1e6e8c074f81d`. | 7,009 | `42d7808344f1b4f853f8feb43482ffa79979125ecf9d4f0a423d4836e4832165` |

## Upstream terms and required attribution

- DWD's [official legal notice](https://www.dwd.de/DE/service/rechtliche_hinweise/rechtliche_hinweise.html)
  permits freely accessible geodata under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/)
  with source attribution; the [Open Data FAQ](https://www.dwd.de/DE/leistungen/opendata/faqs_opendata.html)
  confirms the same terms for Open Data. Use
  `Source: Deutscher Wetterdienst (DWD); modified/quantized by
  OpenBikeComputer` for baked output and retain the CC BY link.
- NOAA NODD data is [made available for unrestricted public
  use](https://www.noaa.gov/information-technology/open-data-dissemination).
  Use `Source: NOAA/NCEP <product>; modified/quantized by OpenBikeComputer; no
  NOAA endorsement is implied`. Do not present modified output as an original
  NOAA product.
- MET Locationforecast data is offered under [NLOD 2.0 or CC BY
  4.0](https://docs.api.met.no/doc/License.html). Use `Data from MET Norway`.

The provider decisions, decoding contracts, and fallback rules are recorded in
[`docs/decisions/WX1-weather-source-contracts.md`](../../../../docs/decisions/WX1-weather-source-contracts.md).
