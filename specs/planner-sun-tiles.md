# Planner sunlight index

The sunlight layer reads `sun.pmtiles` and the terrain archive from the same
release. The archive stores height bounds and map-scale horizon profiles.
The client calculates shadows for one date and clock time. No rider schedule
enters the calculation.

## Archive

The archive uses PMTiles v3, uncompressed lossless WebP tiles, and XYZ tile
addresses at zooms 0 through 12. Height-bound tiles at zooms 0 through 10 have
512 × 512 RGBA pixels. Zooms 8 through 10 append horizon profiles below those
pixels. Zooms 11 and 12 contain horizon profiles only.
The archive header covers the terrain context. The metadata `bounds` field
sets the visible region. Terrain extends at least `distance_m` beyond it.

| Metadata | Value |
| --- | --- |
| `sun_format` | 3 |
| `dem_zoom` | 12 |
| `index_zoom` | 10 |
| `bound_step` | 16 metres |
| `horizon_min_zoom` | 8 |
| `horizon_zoom` | 12 |
| `horizon_samples` | 32 or 64 grid cells along each tile edge |
| `horizon_directions` | 36 or 72, evenly spaced clockwise from north |
| `horizon_step` | 90 / 254 degrees per encoded step |
| `distance_m` | Positive search distance, at most 30000 metres |
| `timezone` | Region IANA time zone |
| `terrain_sha256` | SHA-256 of the terrain PMTiles archive |
| `attribution` | Terrain source attribution |
| `bounds` | Visible west, south, east, north |
| `coverage` | Terrain archive west, south, east, north |

## Height bounds

Native vertices are the pixel centres of the zoom-12, 512-pixel terrain grid.
A longitude and latitude project to this grid as `(world_x - 0.5,
world_y - 0.5)`. The client uses bilinear interpolation between these vertices.

At zoom z, let L = 12 − z. Pixel i,j of index tile x,y bounds the closed native
rectangle from `(512x + i, 512y + j) × 2^L` to
`(512x + i + 1, 512y + j + 1) × 2^L`. The maximum includes both far edges.
These values are block bounds, not heights at lower-zoom pixel centres.

The bound rounds upward in `bound_step` metres. This affects culling only;
native terrain intersections retain their source heights. R,G encode `height + 32768` in
big-endian order. B is zero. A is 255. R=255,G=255 means unknown, not a height.
A missing vertex makes every bound that contains it unknown. An unknown bound
cannot cull a ray. A missing index tile has the same meaning.

## Client calculation

### Horizon profiles

Each zoom-12 profile sits at a grid cell centre. Its observer sits one metre
above the bilinear terrain surface. Each bearing stores the largest geometric
elevation angle of terrain within `distance_m`. The search includes the
interior peak of each bilinear patch and Earth curvature. A negative horizon
is zero. The angle rounds upward in `horizon_step` units. A value of 255 means
unknown. A missing terrain segment makes that bearing unknown.

Each tile has `horizon_samples` pixels across and
`horizon_samples × horizon_directions / 3` pixels down. RGB holds three
consecutive bearings. Each vertical plane holds the next three bearings for
every grid cell. The first bearing stores its value. Each later bearing stores
its difference from the preceding bearing, modulo 256. Alpha is 255.

Each coarser level averages groups of four cells and rounds upward. An unknown
child makes its parent bearing unknown. At zooms 8 through 10, the RGB plane
pixels follow the height bounds in row-major order, in a 512-pixel-wide image.
The extra rows have alpha 255. The tile has `512 + horizon_samples² ×
horizon_directions / 1536` rows. Its first 512 rows retain the height bounds.

The map uses profiles one zoom above its raster tile, up to zoom 12. Terrain
shading starts at map zoom 7. Routes use zoom 12.
The client interpolates between neighbouring bearings and grid centres.
These profiles are overview estimates. A 32-cell tile represents about 200 m
in Baden-Württemberg. Changing the time reuses the decoded profiles.

### Point inspection

The observer sits one metre above the bilinear surface. The solar centre uses
geometric elevation. A ray includes Earth curvature with radius 6371008.8 m.
A confirmed intersection means terrain shade. An incomplete ray means unknown.
A solar centre at or below the horizon means night for the shading calculation.
Sunrise and sunset use the separate −0.833° solar elevation convention.

For point inspection, the worker skips a block only when its known upper bound is below the ray at
block entry. At a native leaf it evaluates the bilinear patch along the ray,
including an interior maximum. It does not sample only the segment endpoints.

The map samples tile pixels at the chosen instant. Below map zoom 7, it shows
global night from the solar position, with transparent daytime pixels. It reads
no terrain profiles at those zooms. At map zoom 7 and above, pixels outside
the visible bounds are transparent. The interface identifies the day/night overview.
Point inspection uses five-minute bins for the local date. A missing
spring clock-change bin is unknown. A repeated autumn hour uses one of its
UTC instants. The interface identifies the region time zone.

Completed raster tiles and decoded height tiles have bounded caches. Jobs stop
when cancelled. A hidden layer starts no map work. Panning with the same date
and time can reuse completed tiles. Trees, buildings, clouds and atmospheric
refraction do not enter the terrain shadow model.
