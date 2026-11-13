# Planner snow tiles

The three snow archives of one region: `snow.pmtiles` and `snow-levels.pmtiles`, the runs of bands
in which a share of the years had snow, and `snow-seasons.pmtiles`, the onset and melt-out of each
season. They come from the per-year tiles: the onset and melt-out of the longest snow period of
each season. The prepared source [`snow`](obc-data.md#snow) holds the per-year tiles of zoom 6 to
12, one file per season and zoom 6 block. `planner/snow` joins the seasons of the blocks of a
region and makes the three archives.

- Version: `snow_format` 1 of each archive. A reader reads only `snow_format` 1.
- Writers: `tools/planner_snow.py`.
- Readers: the web planner snow layer (`builder/web/src/lib/planner/layers/`), the TrailView ground
  stream (`builder/web/src/lib/trailview/stream/ground.ts`), `tools/planner_verify.py`.
- Related: [planner release](planner-release.md), [planner climate tiles](planner-climate-tiles.md).

## Seasons

Season s runs from 1 September of year s to 31 August of year s+1. The day index is 0 on 1
September. A season has 365 or 366 days. The day step is ⌊day index ÷ 2⌋, from 0 to 182.

The longest snow period of a season is its longest run of consecutive snow days. When two runs have
the same length, the earliest run counts. Onset is its first day. Melt-out is its last day.

Each season is made from its own days only, so a new season does not change the seasons before it.

## Tile body

A per-year tile has 256 × 256 pixels. Its body is planar bytes. For each season, from the first
season in order:

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

## Bands

Band b, from 0 to 45, holds the day steps 4b to 4b + 3. The day of band b is the day step
min(4b + 2, 182). A date reads the band ⌊day step ÷ 4⌋. Example: 1 March is day step 90, so band
22, whose day is day step 90.

## Levels

For a pixel, k is the count of seasons with data, and c(b) is the count of seasons with snow on the
day of band b. A level holds band b when k ≥ 1 and:

| Level | Holds band b when | Archive |
| --- | --- | --- |
| any | c(b) ≥ 1 | `snow-levels`, image 1 |
| quarter | c(b) ≥ 1 and 4 c(b) ≥ k | `snow-levels`, image 2 |
| half | c(b) ≥ 1 and 2 c(b) ≥ k | `snow` |
| three quarters | 4 c(b) ≥ 3 k | `snow-levels`, image 3 |
| every | c(b) = k | `snow-levels`, image 4 |

## Runs

The run of a level is its longest run of consecutive bands that it holds, the earliest on a tie. A
run stores its first and its last band. A level that holds no band stores 253 in both bytes. A
pixel with k = 0 stores 255 in both bytes. Each level keeps its own run. So in a pixel with more
than one run, a higher level can hold a band that a lower level does not.

## `snow` and `snow-levels`

| Field | Value |
| --- | --- |
| Format | PMTiles v3 |
| Tiles | Web Mercator XYZ, 512 × 512 pixels |
| Tile type | `webp` (4) |
| Tile compression | None |
| Bounds | The region bounds |
| Zoom levels | 0 to 11; 0 to 7 when the region has no per-year tile of HR-WSI |

Tile z/x/y covers the four per-year tiles of zoom z + 1, each as a quarter of its pixels. Example:
`snow` tile 11/1072/722 covers the per-year tiles 12/2144/1444, 12/2145/1444, 12/2144/1445 and
12/2145/1445. Where the join lacks one of these per-year tiles of zoom 9 to 12, that quarter takes
its part of the per-year tile z/x/y, each pixel twice in each direction. Where it lacks one of zoom
8 or less, the quarter is no data, 255 in both bytes. A tile is in the archive when the join has
one of its four per-year tiles and a pixel of the tile has data.

A run image is a lossless WebP: red is the first band, green is the last band, blue is 0. A `snow`
tile is one run image. A `snow-levels` tile is four run images one after the other, in the order
of the levels table. The length of each image is 8 + the RIFF size in bytes 4 to 7, little endian,
as in the [colour tile](trailview-tiles.md#colour).

## `snow-seasons`

| Field | Value |
| --- | --- |
| Format | PMTiles v3 |
| Tiles | Web Mercator XYZ, 64 × 64 pixels |
| Tile type | `unknown` (0) |
| Tile compression | gzip |
| Bounds | The region bounds |
| Zoom levels | 2 to 14; 2 to 10 when the region has no per-year tile of HR-WSI |

The body is the [tile body](#tile-body) of a per-year tile with 4,096 pixels per plane: for each
season, the onset plane and then the melt-out plane, 8,192 bytes. A tile where no season has data
is not in the archive.

- A tile of zoom 14 holds the pixels of its part of the per-year tile of zoom 12. Example: tile
  14/8576/5776 holds the north-west 64 × 64 pixels of the per-year tile 12/2144/1444.
- Below zoom 14, a pixel takes, for each season, the worst case of its 2 × 2 pixels one zoom
  deeper, by the first rule that applies: 254 when one of them is 254; the earliest onset and the
  latest melt-out of those that are dated; 253 when one of them is 253; else 255.
- A quarter of a tile of zoom z whose tile one zoom deeper is not in the archive takes its part of
  the per-year tile of zoom z − 2. So a tile at the edge of HR-WSI keeps the MODIS data of its
  other quarters.

`snow` and `snow-levels` come from the per-year tiles, which keep the mean of their pixels and not
the worst case.

## Metadata

The metadata of each archive is a JSON object:

| Key | Value |
| --- | --- |
| `snow_format` | `1` |
| `first_season` | Integer year of the first season |
| `seasons` | Integer count of seasons |
| `source` | The finest source of the per-year tiles: `copernicus-hr-wsi` when the region has a per-year tile of zoom 9 or more, else `nasa-modis` |
| `resolution_m` | Integer resolution of that source in metres |
| `attribution` | Text to show with the layer |
| `step_days` | `2`; in `snow-seasons` only |

## Readers

- A reader of `snow` or `snow-levels` that lacks a tile of zoom 8 to 11 reads the nearest
  ancestor that the archive has, down to zoom 7.
- TrailView reads `snow` only. A pixel has snow on a date where its run holds the band of the
  date, and data where its bytes are not 255.
- The planner map draws 6 classes and a hatch for no data, on the band of the date. The hatch is
  where `snow` has 255. Else the class is the highest level that holds the band while each lower
  level holds it too, in the order any, quarter, half, three quarters, every; none when `any`
  does not hold it.
- A route has samples: the points that the planner samples along the route for its profile.
  Before any read, the planner picks the finest zoom of `snow-seasons`, at most its max zoom, at
  which the samples fall in at most 25 tiles; else zoom 2. A tile of zoom 11 to 14 that the
  archive lacks then reads its ancestor of zoom 10. A point on the route uses the route samples.
- A pin on the map samples one point, so it reads one tile at the max zoom of `snow-seasons`.

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

Each source makes its own tiles. In each season, a pixel of a per-year tile takes the first source
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
tile of zoom 6 to 8 where it has data or a tile below it. A season file holds each ancestor of its
tiles down to zoom 6.

The join of a region has the per-year tiles of zoom 6 to 12 of its blocks that meet the bounds,
and their parents of zoom 0 to 5. A pixel outside the bounds keeps its data. The join has a tile
when a season has it. A season that lacks a tile of zoom 9 to 12 takes the part of the nearest
ancestor of zoom 8 or more that the season has, each pixel repeated 2^k times in each direction
for an ancestor k zooms up. Else the season is 255 in the tile. The per-year tiles of zoom 0 to 5
take the 2 × 2 child pixels one zoom deeper with equal weights, without smoothing.

## Forest

`nasa-modis`: a MODIS pixel is 255 in all seasons when its mean tree canopy cover is more than 75 %
in Hansen Global Forest Change `treecover2000`.

`copernicus-hr-wsi` has no forest mask.
