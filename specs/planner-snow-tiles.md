# Planner snow tiles

The planner snow archive of one region, `snow.pmtiles`: the onset and melt-out of the longest snow
period of each season. The prepared source [`snow`](obc-data.md#snow) holds its tiles of zoom 6 to
12, one file per season and zoom 6 block. `planner/snow` joins the seasons of the blocks of a
region into the archive.

- Writers: `tools/planner_snow.py`.
- Readers: the web planner snow layer (`builder/web/src/lib/planner/layers/snow.ts`),
  `tools/trailview/snow.py`.
- Related: [planner release](planner-release.md), [planner climate tiles](planner-climate-tiles.md).

## Archive

| Field | Value |
| --- | --- |
| Format | PMTiles v3 |
| Tiles | Web Mercator XYZ, 256 × 256 pixels |
| Tile type | `unknown` (0) |
| Tile compression | gzip |
| Bounds | The region bounds |
| Zoom levels | 0 to 12; 0 to 8 when the archive has no tile of HR-WSI |

The archive has the tiles of zoom 6 to 12 of the blocks of the region that meet the bounds, and
their parents of zoom 0 to 5. A pixel outside the bounds keeps its data.

A tile of zoom 9 to 12 is in the archive only where HR-WSI has data in it or in a tile below it.
A reader that lacks a tile of zoom 9 to 12 reads the nearest ancestor that the archive has, down
to zoom 8, and takes the part of it that covers the tile. A missing tile of zoom 8 or less is no
data.

## Seasons

Season s runs from 1 September of year s to 31 August of year s+1. The day index is 0 on 1
September. A season has 365 or 366 days. The day step is ⌊day index ÷ 2⌋, from 0 to 182.

The longest snow period of a season is its longest run of consecutive snow days. When two runs have
the same length, the earliest run counts. Onset is its first day. Melt-out is its last day.

Each season is made from its own days only, so a new season does not change the seasons before it.

## Tile body

The body is planar bytes. For each season, from the first season in order:

| Offset in season block | Bytes | Content |
| --- | --- | --- |
| 0 | 65,536 | Onset, one byte per pixel |
| 65,536 | 65,536 | Melt-out, one byte per pixel |

Pixels are in row order. Row 0 is the north edge and column 0 is the west edge. The body length is
`seasons` × 131,072 bytes.

## Byte values

| Value | Meaning |
| --- | --- |
| 0–182 | Day step of the onset or melt-out |
| 253 | No snow in this season |
| 254 | Snow on every day of the season |
| 255 | No data |

A sentinel (253, 254, 255) is in both planes of the pixel.

There is snow on day step d of season s when onset_s ≤ d ≤ meltout_s, or when the pixel has 254. The
share of seasons with snow on a date is the count of seasons with snow divided by the count of
seasons that are not 255 at that pixel.

## Metadata

The archive metadata is a JSON object:

| Key | Value |
| --- | --- |
| `first_season` | Integer year of the first season |
| `seasons` | Integer count of seasons |
| `step_days` | `2` |
| `source` | The finest source of the archive: `copernicus-hr-wsi` when it has a tile of zoom 9 or more, else `nasa-modis` |
| `resolution_m` | Integer resolution of that source in metres |
| `attribution` | Text to show with the layer |

## Sources

| `source` | Data | `resolution_m` | Max zoom |
| --- | --- | --- | --- |
| `nasa-modis` | MODIS Terra and Aqua daily snow cover (MOD10A1, MYD10A1, collection 6.1) | 500 | 8 |
| `copernicus-hr-wsi` | HR-WSI Snow Phenology S2 yearly rasters | 20 | 12 |

A pixel reads `copernicus-hr-wsi` where it has data and `nasa-modis` elsewhere.

`nasa-modis` snow days:

- A day has a clear observation when Terra or Aqua is clear. Terra comes first.
- A clear day is a snow day when NDSI ≥ 0.10 or the detector is saturated.
- A day with no clear observation takes the state of the nearest clear day of its season. On a
  tie, the earlier clear day counts.
- A season with no clear day at a pixel is no data.

`copernicus-hr-wsi` takes onset (SCO), melt-out (SCM) and duration (SCD) from the yearly rasters.
Its day index also starts at 0 on 1 September. A season without a raster is no data.

## Zoom levels

Each source makes its own tiles. In each season, a pixel of the archive takes the first source
in which it is not 255: `copernicus-hr-wsi`, then `nasa-modis`.

The resampling steps use the blend rule. The rule takes weighted input pixels
for each season:

1. The pixel is 255 when the weight of 255 inputs is more than half of the total.
2. Else the pixel takes the class with the largest weight among dated (0–182), 254 and 253, in this
   order on a tie.
3. A dated pixel takes the weighted mean of the dated inputs, for onset and for melt-out separately,
   rounded half up.

| Zoom | `copernicus-hr-wsi` | `nasa-modis` |
| --- | --- | --- |
| 12 | Bilinear | Bilinear |
| 8 to 11 | 2 × 2, smoothed | Bilinear |
| 6 and 7 | 2 × 2, smoothed | 2 × 2 |

- Bilinear: the inputs are the four source pixels around the pixel centre with bilinear weights.
  A source pixel outside the source grid is 255.
- 2 × 2: the inputs are the 2 × 2 child pixels of the source, with equal weights. A child tile
  that the source lacks is 255.
- Smoothed: each child pixel is smoothed first: it takes the blend of its 3 × 3 neighbourhood in
  its tile, with equal weights and the median of the dated inputs instead of the mean. A
  neighbour outside the tile repeats the edge pixel.

A season has a tile of zoom 9 to 12 where HR-WSI has data in it or in a tile below it, and a
tile of zoom 6 to 8 where it has data or a tile below it. The archive has a tile when a season
has it. A season that lacks a tile of zoom 9 to 12 takes the part of the nearest ancestor of zoom
8 or more that the season has, each pixel repeated 2^k times in each direction for an ancestor k
zooms up. Else the season is 255 in the tile.

Zoom 0 to 5 take the 2 × 2 child pixels of the archive with equal weights, without smoothing.

## Forest

`nasa-modis`: a MODIS pixel is 255 in all seasons when its mean tree canopy cover is more than 75 %
in Hansen Global Forest Change `treecover2000`.

`copernicus-hr-wsi` has no forest mask.
