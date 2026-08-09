# Weather source fixtures

These are the small, legally redistributable captures for WX1. Full request
templates, measurements, licenses, and failure semantics are in
[`docs/decisions/WX1-weather-source-contracts.md`](../../../../../../docs/decisions/WX1-weather-source-contracts.md).

## DWD RV

`dwd-rv-convective-rain.json` and `dwd-rv-dry-nodata.json` are deterministic
extracts from numeric WCS GeoTIFF responses for reference/valid time
`2026-08-09T08:20:00Z`. They retain the bbox, raster transform, counts, sentinel
examples, sample values, response size, and SHA-256 of the original GeoTIFF.
They are extracts, not byte-identical copies. The source response was DWD's
`dwd__Niederschlagsradar` coverage. DWD geodata is CC BY 4.0.

Attribution: **Quelle: Deutscher Wetterdienst**

License/source: <https://www.dwd.de/DE/service/rechtliche_hinweise/rechtliche_hinweise.html>

## MET Norway

`met-locationforecast-oslo-24h.json` is a deterministic extraction of the first
24 hourly records returned by Locationforecast 2.0 `complete` for Oslo
(`59.9139,10.7522`, altitude 23 m). It preserves every field selected by the
contract and the full response's headers, sizes, and SHA-256. It is not a
byte-identical response. MET data is NLOD 2.0 / CC BY 4.0.

Attribution: **Data from MET Norway**

License/source: <https://docs.api.met.no/doc/License.html>

## NOAA/NCEP GFS

`gfs-manila-apcp-f006.grib2.b64` reversibly base64-encodes the exact 430-byte
NOMADS response for GFS run `2026-08-09T00Z`, f006, surface APCP, bbox
`14.17..15.03 N, 120.53..121.43 E`. After decoding, its SHA-256 is
`be6705b5d5a3e56b5a11cf42295ea1804e5b4a49b237082d877df3612e385566`.
It contains two byte-identical cumulative messages, a real service behavior the
decoder de-duplicates. NOAA/NCEP U.S. government data is public domain in the
United States; do not imply NOAA endorsement.

Attribution: **Forecast data: NOAA/NCEP GFS**

Source: <https://www.ncei.noaa.gov/products/weather-climate-models/global-forecast>

## Regeneration

Run from the repository root:

```bash
tools/weather-source-spike/reproduce.sh /tmp/obc-weather-evidence
```

Provider retention is short, so the script captures the latest coherent run;
it will not recreate the historical timestamps above after they age out. The
stored response hashes and extracts are the permanent provenance for those
captures.
