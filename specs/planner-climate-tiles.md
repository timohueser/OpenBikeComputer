# Planner climate tiles

The planner climate layer reads one archive per region: `climate.pmtiles`. It
holds rain, temperature and wind for each week of ten years. The bake is
`tools/planner_climate.py`.

## Archive

| Field | Value |
| --- | --- |
| Format | PMTiles v3 |
| Tile type | `unknown` (0) |
| Tile compression | gzip |
| Bounds | The region bounds |
| Zoom levels | 8 (overview) and 9 (detail) |

The tiles are blocks of grid cells. They are not Web Mercator tiles.

## Cells

The grid is the ERA5-Land grid of 0.1° × 0.1° cells. Cell column i, from 0 to
3599, has its centre at longitude −180 + 0.1 i. Cell row j, from 0 to 1800, has
its centre at latitude 90 − 0.1 j. A point is in column ⌊10 (lon + 180) + 0.5⌋
mod 3600 and row ⌊10 (90 − lat) + 0.5⌋.

| Zoom | Columns × rows | Tile x holds columns | Tile y holds rows |
| --- | --- | --- | --- |
| 8 | 24 × 16 | 24x to 24x + 23 | 16y to 16y + 15 |
| 9 | 12 × 8 | 12x to 12x + 11 | 8y to 8y + 7 |

The zoom 8 tile x, y holds the same cells as the zoom 9 tiles 2x to 2x + 1 and
2y to 2y + 1.

The archive holds the cells that touch the bounds. Every other cell, and every
cell without source data (sea), has the missing code in every plane. A tile
without a cell with data is absent.

## Tile body

The body is a sequence of planes. A plane has one value per cell for each of its
indexes. For each index, from 0 in order, the cells follow in row order: row 0 is
the north edge and column 0 is the west edge. A 2-byte value is little-endian.

Zoom 9, the detail tile, has 96 cells and these planes:

| Plane | Indexes |
| --- | --- |
| `orography` | 1 |
| `lapse` | 12: month − 1 |
| `wet_days` | 520: 52 × year + week |
| `rain` | 520: 52 × year + week |
| `tmax` | 520: 52 × year + week |
| `tmin` | 520: 52 × year + week |
| `wind` | 520: 52 × year + week |

The detail body is 250,944 bytes.

Zoom 8, the overview tile, has 384 cells and these planes:

| Plane | Indexes |
| --- | --- |
| `orography` | 1 |
| `lapse` | 12: month − 1 |
| `rose` | 192: 16 × (month − 1) + sector |
| `wet_share` | 52: week |
| `rain` | 52: week |
| `tmax` | 52: week |
| `tmin` | 52: week |
| `wind` | 52: week |

The overview body is 178,944 bytes.

## Codes

A value takes the nearest code, with a half rounded up, and is clamped to the
codes of its plane.

| Plane | Type | Missing | Codes | Value |
| --- | --- | --- | --- | --- |
| `wet_days` | uint8 | 255 | 0–254 | code days |
| `wet_share` | uint8 | 255 | 0–100 | code % of days |
| `rain` | uint8 | 255 | 0–100 | code mm |
| | | | 101–254 | 100 + 5 (code − 100) mm |
| `tmax`, `tmin` | int8 | −128 | −127–127 | 0.5 code °C |
| `wind` | uint8 | 255 | 0–254 | 0.2 code m/s |
| `orography` | int16 | −32768 | −32767–32767 | code m |
| `lapse` | int8 | −128 | −127–127 | 0.1 code K/km |
| `rose` | uint8 | 255 | 0–200 | 0.5 code % |

## Time

Year 0 is `first_year`. A day is a local solar day: local time is UTC plus
⌊lon ÷ 15 + 0.5⌋ hours, with the longitude of the cell centre.

Week w of a year holds the days 7w to 7w + 6 of the year, from day 0 on
1 January. Week 51 also holds days 364 and 365, so it has 8 days, or 9 days in a
leap year.

## Values

Daily values:

| Value | Rule |
| --- | --- |
| Maximum temperature | Maximum of the hourly 2 m temperatures at local 00:00 to 23:00 |
| Minimum temperature | Minimum of the same 24 values |
| Rain | Sum of the 24 hourly precipitation totals that end at local 01:00 to 24:00 |
| Daytime wind | Mean of the 10 hourly 10 m wind speeds at local 09:00 to 18:00 |

Detail planes, for each year and week:

| Plane | Value |
| --- | --- |
| `wet_days` | Days with rain ≥ 1 mm |
| `rain` | Sum of daily rain |
| `tmax` | Mean of the daily maximum temperatures |
| `tmin` | Mean of the daily minimum temperatures |
| `wind` | Mean of the daily daytime wind |

A week with a missing hour is missing.

Overview planes, for each week, use the decoded detail values of the years that
are not missing:

| Plane | Value |
| --- | --- |
| `wet_share` | 100 × the sum of `wet_days` ÷ the sum of the days of the week |
| `rain`, `tmax`, `tmin`, `wind` | Mean of the detail values |

Static planes:

| Plane | Value |
| --- | --- |
| `orography` | ERA5-Land model height of the cell: the geopotential ÷ 9.80665 m/s² |
| `lapse` | Change of temperature with height, from the monthly means below |
| `rose` | Share of the daytime hourly samples of the month, in all years, with wind from the sector |

The temperatures of a cell are at its orography. The temperature at height h is
T + `lapse` × (h − `orography`) ÷ 1000.

`lapse`: the monthly mean of a cell is the mean, over the days of the month in all
years, of (maximum + minimum temperature) ÷ 2. A least-squares fit
T = a + b × height + c × lon + d × lat through the 5 × 5 cells around the cell gives
b. When the heights of these cells differ by less than 100 m, b is −6.5 K/km.

`rose`: the wind comes from the direction atan2(−u, −v), clockwise from north.
Sector s holds the directions from 22.5 s − 11.25° up to, but not including,
22.5 s + 11.25°.

## Metadata

The archive metadata is a JSON object:

| Key | Value |
| --- | --- |
| `first_year` | Integer year of year 0 |
| `years` | `10` |
| `source` | `era5-land` |
| `attribution` | Text to show with the layer |
| `inputs` | Object: `doi`, `orography_sha256`, `chunks` |

## Source

The source is ERA5-Land hourly data, DOI 10.24381/cds.e2161bac, licence CC BY
4.0. The bake reads the ECMWF ARCO geo-chunked Zarr stores of ERA5-Land: `t2m`,
`u10`, `v10` and `tp`. `tp` holds the precipitation of each hour. The bake uses
final data only. The orography is the ERA5-Land geopotential file
`geo_1279l4_0.1x0.1.grib2_v4_unpack.nc` with the SHA-256 in `orography_sha256`.

`chunks` has one SHA-256 per variable. It is the SHA-256 of one line
`T.Y.X SHA256` for each Zarr chunk that the bake reads, sorted as text, each line
ended by a line feed. `T.Y.X` is the Zarr chunk key and `SHA256` is the SHA-256 of
its compressed bytes. A chunk that the store omits has the SHA-256 of no bytes.
