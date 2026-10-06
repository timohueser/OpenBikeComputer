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
| Zoom levels | 0 to the max zoom |

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
| `source` | The finest source of the archive: `nasa-modis` or `copernicus-hr-wsi` |
| `resolution_m` | Integer resolution of that source in metres |
| `attribution` | Text to show with the layer |

## Sources

| `source` | Data | `resolution_m` |
| --- | --- | --- |
| `nasa-modis` | MODIS Terra and Aqua daily snow cover (MOD10A1, MYD10A1, collection 6.1) | 500 |
| `copernicus-hr-wsi` | HR-WSI Snow Phenology S2 yearly rasters | 20 |

An archive reads `copernicus-hr-wsi` where it has data and `nasa-modis` elsewhere.
A region outside the `extent` of `hr-wsi`, or where HR-WSI has no product, reads `nasa-modis` only.

The max zoom is the zoom whose pixel size at the middle latitude of the bounds is
nearest to `resolution_m` in log scale. In the Alps, it is 12 for
`copernicus-hr-wsi` and 8 for `nasa-modis`.

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
with bilinear weights. A source pixel outside the source grid is 255. In each
season, a pixel takes the first source in which it is not 255:
`copernicus-hr-wsi`, then `nasa-modis`.

At a lower zoom, the inputs are the 2 × 2 child pixels, with equal weights. When
`source` is `copernicus-hr-wsi`, each child pixel is smoothed first: it takes the blend of its
3 × 3 neighbourhood in its tile, with equal weights and the median of the dated
inputs instead of the mean. A neighbour outside the tile repeats the edge pixel. The smoothing
applies to every pixel of the tile, also to a pixel that `nasa-modis` fills.

## Forest

`nasa-modis`: a MODIS pixel is 255 in all seasons when its mean tree canopy
cover is more than 75 % in Hansen Global Forest Change `treecover2000`.

`copernicus-hr-wsi` has no forest mask.
