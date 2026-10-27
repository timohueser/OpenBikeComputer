# TrailView tiles

TrailView reads the raster layers of the planner release: the ground and the colour of each
texel, and the ruleset that the ground indexes. The layers are the optional planner layers
`trailview-rules`, `trailview-ground` and `trailview-colour` of [obc-data.md](obc-data.md). The
bakes are `tools/trailview/rules.py`, `tools/trailview/ground.py` and `tools/trailview/colour.py`.

## Archives

Each archive is a tile pyramid over the region bounds.

| Field | Value |
| --- | --- |
| Format | PMTiles v3 |
| Tiles | Web Mercator XYZ, 512 × 512 texels |
| Tile type | `webp` (4); `unknown` (0) in a colour archive, see [Colour](#colour) |
| Tile compression | none |
| Bounds | The region bounds |

Texel (column, row) of tile z/x/y covers that cell of the 512 × 512 grid of the tile. Row 0 is
the north edge and column 0 is the west edge. The value of a texel is the value at its centre.

A tile that the archive does not hold has no data: the engine draws its parent. The release packs
each archive in PMTiles per zoom 11 parent: `maps/tiles/<kind>/<z>-<x>-<y>.pmtiles`, with the
TileJSON `maps/<kind>.json`. The kind is `trailview-ground` for the ground, and
`trailview-colour-<archive>` for the colour.

Decode a tile without colour conversion and without premultiplied alpha (`createImageBitmap`
with `colorSpaceConversion: 'none'` and `premultiplyAlpha: 'none'`): the bytes are data. A 2D canvas
premultiplies alpha, so read the pixels through a WebGL texture, not `getImageData`.

The archive metadata is a JSON object:

| Key | Value |
| --- | --- |
| `trailview_format` | `3` in the ground archive, `2` in a colour archive. A reader reads only the format that it knows |
| `archive` | The name of the archive: `ground`, or `q1` to `q4` |
| `variant` | Only in a colour archive: the variant of the table in [Colour](#colour) |
| `attribution` | Text to show with the layer |

## Ground

`trailview-ground` has one archive, `ground`, as lossless WebP at zooms 0 to 13. A texel is four
bytes:

| Channel | Byte |
| --- | --- |
| Red | The material byte: the material code and two flags, see below |
| Green | The barren share |
| Blue | The lithology: the index of the lithology in `lithologies` of `materials.json`; 0 is unknown |
| Alpha | The region id: the index in `ids` of `rules.json`; 255 is no data |

The material byte:

| Bits | Meaning |
| --- | --- |
| 0–5 | The index of the material in `materials` of `materials.json` |
| 6 | Barren: the highest NDVI that a quarter of the year measured is below 0.25 |
| 7 | No data: the texel is outside the region bounds. The other bits are 0 |

The barren share is round(255 × (1 − smoothstep(0.15, 0.35, NDVI))) of the highest NDVI that a
quarter of the year measured, and 0 where no quarter looked or the texel has no data. A quarter that
saw snow counts as NDVI −1.

A material code above 63 needs a new format. The ruleset check rejects more than 255 region ids, so
a region id is 0 to 254; more ids need a new format, with two bytes.

The material and the barren share come from the sources at zoom 13, the lithology and the region
at zoom 10. A coarser texel takes the most common byte of its four children, the first in reading
order on a tie. A material or region texel with no data counts only where all four have no data,
and a missing child has no data. A coarser barren share is the rounded mean of the children with
data. At zooms 11 to 13, a lithology or region texel is the zoom 10 texel that holds its centre.
A tile is in the archive where its material has data.

The material chain refines a baseline. Each step changes a texel only where the step names the
material more precisely: the table `REFINES` of `ground.py`.

1. ESA WorldCover, its class as a material. Finer than its 10 m pixels, a shore takes the
   majority of water within half a pixel.
2. Sentinel-2 heath: open ground that is as red as it is green while its NDVI is high and near
   the year's highest.
3. OSM forests with a `leaf_type`.
4. CORINE Land Cover.
5. OSM natural and landuse areas, and kinds of wetland.

The lithology is the rock of the unit of EGDI's 1:1 000 000 map, and of IGME 5000 where EGDI
names no rock. A term that the tables `EGDI` and `IGME5000` of `ground.py` do not list is unknown.

The region is the ecoregion of `ecoregions.json` that holds the texel centre, or the nearest one
within 25 km, else `world`. From there the texel steps into the subregion whose `where` holds it,
the smallest on a tie, until none does. The place of the texel is the subdivision of
`countries.json` that holds its centre, else the country, else the nearest place within 5 km in the
tile. The texel takes the id of the region and its subdivision, else of the region and its
country, else of the region and `"*"`.

## Colour

`trailview-colour` has four archives from the Sentinel-2 quarterly mosaics of one year, `q1` to
`q4`. Quarter q holds the months 3q − 2 to 3q. A texel has the colour of the quarter and its NDVI
code.

The colour is the visible reflectance `refl` of the red, green and blue bands, as
round(255 × refl^(1/2.2)).

| Code | Meaning |
| --- | --- |
| 0 | Fewer than 3 valid looks in the quarter: the engine paints the palette |
| 1 | The quarter saw snow. The colour is that of the nearest quarter without snow, the previous one first, or black |
| 2–255 | NDVI = (code − 1) / 127 − 1 |

The NDVI has half the resolution of the texels: a texel with a code of 2 or more takes the mean code
m of the texels with a code of 2 or more in its aligned 2 × 2 block, which starts at even texel
coordinates. m is rounded half to even and stored as 128 + 4 × floor((m − 128) / 4) + 2. So a stored
code is a multiple of 4 plus 2, and it is below 128 when m is below 128. Codes 0 and 1 stay as they are. This is the last step of the bake, on
each tile of each zoom.

A tile is two images, one after the other:

1. The NDVI tile: a lossless WebP of 256 × 256 pixels, one pixel per aligned 2 × 2 block of texels.
   Red is the code that the measured texels of the block share, or 0 where no texel is measured.
   Green holds a mark of 2 bits for each of the four texels, in reading order from bit 0: north-west,
   north-east, south-west, south-east. A mark is 0 or 1, the code of the texel, or 2: the texel
   has the code in red. The length of the image is 8 + the RIFF size in bytes 4 to 7, little
   endian.
2. The colour: an RGB image of 512 × 512 texels in the codec of the variant.

In the first and the last quarter, a texel darker than 0.025 in visible reflectance is terrain
shadow, unless it is snow in shade. Where the third quarter looked, it takes the chroma of the third
quarter at its own brightness.

The archives hold zoom 9 to the last zoom of the variant. A coarser texel is the mean colour, in
linear light, of its children that looked, without the black of a child with code 1; and the mean
NDVI code of those that measured one. It is 1 where more children saw snow than measured an NDVI.

| Variant | Codec | Last zoom | Chroma at 47° N | NDVI at 47° N |
| --- | --- | --- | --- | --- |
| `webp-13` | lossy WebP, 4:2:0 | 13 | 13 m | 13 m |
| `webp-12` | lossy WebP, 4:2:0 | 12 | 26 m | 26 m |
| `webp-11` | lossy WebP, 4:2:0 | 11 | 52 m | 52 m |
| `avif-12` | AVIF, 4:2:0 | 12 | 26 m | 26 m |
| `avif-11` | AVIF, 4:2:0 | 11 | 52 m | 52 m |
| `avif444-12` | AVIF, 4:4:4 | 12 | 13 m | 26 m |

`colour` of `[trailview]` in `data/planner.toml` selects the variant of the release.

## Snow days

`trailview-snow` holds the snow of [the planner snow archive](planner-snow-tiles.md) by day, so a
view reads the snow of its date only. It has one archive per band of day steps,
`trailview-snow-00` to `trailview-snow-45`. Band b holds the day steps 4b to 4b + 3 and shows day
step min(4b + 2, 182). A date reads the band of its day step, ⌊day step ÷ 4⌋.

The archives differ from the table in [Archives](#archives): a tile is 512 × 512 pixels, tile type
`webp` (4), lossless. Tile z/x/y holds the four tiles of zoom z + 1 of the snow
archive that it covers, so its zooms are one coarser than those of the snow archive. A pixel is one
byte, which decodes as red, green and blue:

| Bits | Meaning |
| --- | --- |
| 4–7 | The seasons with data at the pixel: not 255 in the snow archive. 0 is no data |
| 0–3 | The seasons with snow on the band's day step, as the snow archive defines it |

A pixel that the snow archive has no tile for has no data. A tile where no season has data is not
in the archive. A snow archive with more than 15 seasons needs a new format. Each archive has the
metadata of [Archives](#archives), with `trailview_format` `3`, `archive` `snow-00` to `snow-45`,
and:

| Key | Value |
| --- | --- |
| `band` | The band |
| `band_steps` | `4` |
| `day_step` | The day step that the band shows |
| `first_season`, `seasons`, `source` | As in the snow archive |

## Rules

`trailview-rules` is one file, `trailview/rules.json`, from `data/trailview/`. The bake rejects a
ruleset with a problem.

| Key | Value |
| --- | --- |
| `format` | `2` |
| `materials` | `materials.json` as it is |
| `ids` | Each region id as `[region, place]`: per region by name, `"*"` and then each country or subdivision that a value by place of the region names, by code. The index is the region id |
| `regions` | The files of `regions/` by name |
| `packs`, `styles` | The files of `packs/` and `styles/` by name |
| `overlays` | The files of `overlays/` by region name |

The bundle keeps each file as it is, without `about` and `where`. The engine resolves a region id
with these rules:

- A file inherits what it does not say from the file that it `extends`. Objects merge, and any
  other value replaces.
- `look.ramps` of a cover comes whole from the nearest region of the chain that names it.
- A value by place, `{"es-ga": …, "es": …, "*": …}`, takes the entry of the place of the region id,
  else of its country, else `"*"`. The values by place are `style` of the pack and `look.roadLine`.
- In a pack, `trees`, `ground` and `herds` are keyed by species. A child adds or changes one, and
  `null` drops one.
- A list of ground (`on` and `off` of a ground plant, `on` of a herd and of a look's patch) names
  covers, materials by id, and the `tags` of materials in `materials.json` as `#tag`.

An overlay holds more look keys of a region, as `{"look": {…}}`. The engine merges the overlays
of the chain of a region over its look, root first, with the same merge. The key table
`OVERLAY_LOOK` of `rules.py` lists the keys that an overlay may set.

## Canopy

`trailview-canopy` holds the forest of each cell of zoom 12 at 256 × 256 cells, 26 m at 47° N:
the canopy height, the cover, a class of the stems per hectare and the leaf type. It holds no tree
position: the engine places trees that match these values. The layer is the optional planner layer
`trailview-canopy` of [obc-data.md](obc-data.md). Its bake, `tools/planner_trailview_canopy.py`,
reads the tiles of the prepared source [`forest`](obc-data.md#forest).

| Field | Value |
| --- | --- |
| Format | PMTiles v3 |
| Tiles | Web Mercator XYZ, zoom 12 only, 256 × 256 cells |
| Tile type | `webp` (4), lossless |
| Tile compression | none |
| Bounds | The region bounds |

Cell (column, row) of tile 12/x/y covers that cell of the 256 × 256 grid of the tile. Row 0 is the
north edge and column 0 is the west edge. A tile that the archive does not hold is unknown: no
`forest` lacks it. The release packs the archive in PMTiles per zoom 11 parent:
`maps/tiles/trailview-canopy/<z>-<x>-<y>.pmtiles`, with the TileJSON `maps/trailview-canopy.json`.

| Metadata | Value |
| --- | --- |
| `canopy_format` | `1`. A reader reads only the format that it knows |
| `archive` | `trailview-canopy` |
| `attribution` | Text to show with the layer |

Decode a tile without colour conversion (`createImageBitmap` with `colorSpaceConversion:
'none'`). The code of a cell is red + 256 × green; blue is 0. The code 65535 is a cell that
`forest` does not know. The code 0 is a known cell that is not a forest cell. A forest cell has a
canopy height of 3 m or more and a cover of 5 % or more; its code is the bits below, and its cover
bits are never 15.

| Bits | Meaning |
| --- | --- |
| 0–6 | Canopy height h, 1 to 127: h × 0.5 m, the 95th percentile of the canopy pixels. A higher canopy has 127 |
| 7–10 | Cover c, 1 to 10: c × 10 %, the share of the cell where the canopy is 3 m high or more |
| 11–13 | Stems class, the trees 5 m high or more per hectare: see the table below |
| 14–15 | Leaf type: 0 unknown, 1 broadleaved, 2 coniferous, 3 mixed |

| Stems class | Stems per hectare |
| --- | --- |
| 0 | Fewer than 50 |
| 1 | 50 to 99 |
| 2 | 100 to 149 |
| 3 | 150 to 199 |
| 4 | 200 to 299 |
| 5 | 300 to 399 |
| 6 | 400 to 599 |
| 7 | 600 or more |

The leaf type comes from the conifer share of the cell: 25 % or less is broadleaved, 75 % or
more is coniferous, and a share in between is mixed. A cell without a share is unknown.
