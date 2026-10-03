# Planner snow tiles

The planner snow layer reads one archive per region: `snow.pmtiles`. The bake is
`tools/planner_snow.py`.

## Archive

| Field | Value |
| --- | --- |
| Format | PMTiles v3 |
| Tiles | Web Mercator XYZ, 256 × 256 pixels |
| Tile type | `unknown` (0) |
| Tile compression | gzip |
| Bounds | The region bounds |
| Zoom levels | 0 to the max zoom of the source |

A missing tile is no data. A pixel outside the region bounds is no data.

## Seasons

Season s runs from 1 September of year s to 31 August of year s+1. The day index
is 0 on 1 September. A season has 365 or 366 days. The day step is
⌊day index ÷ 2⌋, from 0 to 182.

The longest snow period of a season is its longest run of consecutive snow days.
When two runs have the same length, the earliest run counts. Onset is its first
day. Melt-out is its last day.

## Tile body

The body is planar bytes. For each season, from the first season in order:

| Offset in season block | Bytes | Content |
| --- | --- | --- |
| 0 | 65,536 | Onset, one byte per pixel |
| 65,536 | 65,536 | Melt-out, one byte per pixel |

Pixels are in row order. Row 0 is the north edge and column 0 is the west edge.
The body length is `seasons` × 131,072 bytes.

## Byte values

| Value | Meaning |
| --- | --- |
| 0–182 | Day step of the onset or melt-out |
| 253 | No snow in this season |
| 254 | Snow on every day of the season |
| 255 | No data |

A sentinel (253, 254, 255) is in both planes of the pixel.

There is snow on day step d of season s when onset_s ≤ d ≤ meltout_s, or when the
pixel has 254. The share of seasons with snow on a date is the count of seasons
with snow divided by the count of seasons that are not 255 at that pixel.

## Metadata

The archive metadata is a JSON object:

| Key | Value |
| --- | --- |
| `first_season` | Integer year of the first season |
| `seasons` | Integer count of seasons |
| `step_days` | `2` |
| `source` | `nasa-modis` or `copernicus-hr-wsi` |
| `resolution_m` | Integer source resolution in metres |
| `attribution` | Text to show with the layer |

## Sources

| `source` | Data | `resolution_m` | Max zoom |
| --- | --- | --- | --- |
| `nasa-modis` | MODIS Terra and Aqua daily snow cover (MOD10A1, MYD10A1, collection 6.1) | 500 | 9 |
| `copernicus-hr-wsi` | HR-WSI Snow Phenology S2 yearly rasters | 20 | 13 |

`nasa-modis` snow days:

- A day has a clear observation when Terra or Aqua is clear. Terra comes first.
- A clear day is a snow day when NDSI ≥ 0.10 or the detector is saturated.
- A day with no clear observation takes the state of the nearest clear day. On a
  tie, the earlier clear day counts.
- A season with no clear day at a pixel is no data.
- When the source ends before the last season ends, a pixel is no data for that
  season when its snow period at the source end can still become the longest.

`copernicus-hr-wsi` takes onset (SCO), melt-out (SCM) and duration (SCD) from the
yearly rasters. Its day index also starts at 0 on 1 September. A season without
a raster is no data.

## Zoom levels

Both resampling steps use the blend rule. The rule takes weighted input pixels
for each season:

1. The pixel is 255 when the weight of 255 inputs is more than half of the total.
2. Else the pixel takes the class with the largest weight among dated (0–182), 254
   and 253, in this order on a tie.
3. A dated pixel takes the weighted mean of the dated inputs, for onset and for
   melt-out separately, rounded half up.

At the max zoom, the inputs are the four source pixels around the pixel centre
with bilinear weights. A source pixel outside the source grid is 255.

At a lower zoom, the inputs are the 2 × 2 child pixels, with equal weights.

## Forest

At the max zoom, a pixel is 255 in all seasons when more than half of its area
is tree cover (class 10) in ESA WorldCover 2021.
