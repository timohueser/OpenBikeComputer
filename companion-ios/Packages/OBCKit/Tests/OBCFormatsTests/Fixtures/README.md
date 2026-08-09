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

`dwd-rv-raw-wcs-correspondence.json` pins a same-run 09:35 UTC cross-check. It
maps every WCS pixel center back into DWD's stereographic HDF5 grid and retains
20 reviewable positive, dry, and missing samples. All 9,974 comparable WCS
cells mapped to 9,974 distinct raw cells and matched the raw gain/offset value
within `1e-6`; ten were positive rain values. This demonstrates the observed
nearest-source-cell behavior across the full crop without pretending the
EPSG:4326 response grid is itself DWD's native projection.

Attribution: **Quelle: Deutscher Wetterdienst**

License/source: <https://www.dwd.de/DE/service/rechtliche_hinweise/rechtliche_hinweise.html>

## MET Norway

`met-locationforecast-oslo-24h.json` is a deterministic extraction of the first
24 hourly records returned by Locationforecast 2.0 `complete` for Oslo
(`59.9139,10.7522`, altitude 23 m). It preserves every field selected by the
contract and the full response's headers, sizes, and SHA-256. It is not a
byte-identical response. MET data is NLOD 2.0 / CC BY 4.0.

`met-locationforecast-manila-24h.json` is the corresponding non-Nordic
availability probe for Manila (`14.5995,120.9842`, altitude 16 m). All first 24
records have temperature, wind speed/direction, precipitation amount, and a
symbol, but none has precipitation probability or wind gust; those units are
also absent from `meta.units`. This fixture prevents the Oslo schema from being
misrepresented as a worldwide guarantee. Its decoded source SHA-256 is
`9e63ec334ddd7a36c33caf4a28d49b2e0ea8373317eec20e37f1e6e8c074f81d`.

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
