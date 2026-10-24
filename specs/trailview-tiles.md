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
| Tile type | `webp` (4), or `avif` (5) for a colour variant |
| Tile compression | none |
| Bounds | The region bounds |

Texel (column, row) of tile z/x/y covers that cell of the 512 × 512 grid of the tile. Row 0 is
the north edge and column 0 is the west edge. The value of a texel is the value at its centre.

A tile that the archive does not hold has no data: the engine draws its parent. The release packs
each archive in PMTiles per zoom 11 parent: `maps/tiles/<kind>/<z>-<x>-<y>.pmtiles`, with the
TileJSON `maps/<kind>.json`. The kind is `trailview-material`, `trailview-lithology` and
`trailview-region` for the ground, and `trailview-colour-<archive>` for the colour.

Decode a tile without colour conversion and without premultiplied alpha (`createImageBitmap`
with `colorSpaceConversion: 'none'` and `premultiplyAlpha: 'none'`): the bytes are data. A 2D canvas
premultiplies alpha, so read the pixels through a WebGL texture, not `getImageData`.

The archive metadata is a JSON object:

| Key | Value |
| --- | --- |
| `trailview_format` | `2` in a ground archive, `1` in a colour archive. A reader reads only the format that it knows |
| `layer` | `material`, `lithology` or `region`; absent in a colour archive |
| `archive` | The name of the archive: the layer, or `q1` to `q4` |
| `variant` | Only in a colour archive: the variant of the table in [Colour](#colour) |
| `attribution` | Text to show with the layer |

## Ground

`trailview-ground` has three archives, as lossless WebP.

- A lithology or region texel is one byte: red, green and blue are equal to it.
- A material texel is two bytes: red holds the material byte, green holds the barren share, and blue
  is 0.

| Archive | Zooms | Byte |
| --- | --- | --- |
| `material` | 0 to 13 | The material code and two flags, see below |
| `lithology` | 0 to 10 | The index of the lithology in `lithologies` of `materials.json`; 0 is unknown |
| `region` | 0 to 10 | The region id: the index of the region in `regions` of `rules.json`; 255 is no data |

The material byte:

| Bits | Meaning |
| --- | --- |
| 0–5 | The index of the material in `materials` of `materials.json` |
| 6 | Barren: the highest NDVI that a quarter of the year measured is below 0.25 |
| 7 | No data: the texel is outside the region bounds. The other bits are 0 |

The barren share is round(255 × (1 − smoothstep(0.15, 0.35, NDVI))) of the highest NDVI that a
quarter of the year measured, and 0 where no quarter looked or the texel has no data. A quarter that
saw snow counts as NDVI −1.

A material code above 63 needs a new format. The ruleset check rejects more than 255 regions, so
a region id is 0 to 254; more regions need a new format, with two bytes.

A tile of the last zoom of an archive comes from the sources. A coarser texel takes the most
common byte of its four children, the first in reading order on a tie. A material or region texel
with no data counts only where all four have no data, and a missing child has no data. A coarser
barren share is the rounded mean of the children with data.

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
the smallest on a tie, until none does.

## Colour

`trailview-colour` has four archives from the Sentinel-2 quarterly mosaics of one year, `q1` to
`q4`. Quarter q holds the months 3q − 2 to 3q. A texel is RGBA: the colour of the quarter, and its
NDVI code in alpha.

The colour is the visible reflectance `refl` of the red, green and blue bands, as
round(255 × refl^(1/2.2)). A WebP variant keeps the alpha lossless. An AVIF variant codes the alpha
at the quality of the colour, so its codes 0 and 1 are not exact.

| Alpha | Meaning |
| --- | --- |
| 0 | Fewer than 3 valid looks in the quarter: the engine paints the palette |
| 1 | The quarter saw snow. The colour is that of the nearest quarter without snow, the previous one first, or black |
| 2–255 | NDVI = (alpha − 1) / 127 − 1 |

In the first and the last quarter, a texel darker than 0.025 in visible reflectance is terrain
shadow, unless it is snow in shade. Where the third quarter looked, it takes the chroma of the third
quarter at its own brightness.

The archives hold zoom 9 to the last zoom of the variant. A coarser texel is the mean colour, in
linear light, of its children that looked, without the black of a child with alpha 1; and the mean
NDVI code of those that measured one. It is 1 where more children saw snow than measured an NDVI.

| Variant | Codec | Last zoom | Chroma at 47° N | NDVI at 47° N |
| --- | --- | --- | --- | --- |
| `webp-13` | lossy WebP, 4:2:0 | 13 | 13 m | 6.5 m |
| `webp-12` | lossy WebP, 4:2:0 | 12 | 26 m | 13 m |
| `webp-11` | lossy WebP, 4:2:0 | 11 | 52 m | 26 m |
| `avif-12` | AVIF, 4:2:0 | 12 | 26 m | 13 m |
| `avif-11` | AVIF, 4:2:0 | 11 | 52 m | 26 m |
| `avif444-12` | AVIF, 4:4:4 | 12 | 13 m | 13 m |

`colour` of `[trailview]` in `data/planner.toml` selects the variant of the release.

## Rules

`trailview-rules` is one file, `trailview/rules.json`, from `data/trailview/`. The bake rejects a
ruleset with a problem.

| Key | Value |
| --- | --- |
| `format` | `1` |
| `materials` | `materials.json` as it is |
| `regions` | The files of `regions/` sorted by name, each with `id`, its name. The index is the region id |
| `packs`, `styles` | The files of `packs/` and `styles/` by name |
| `overlays` | The files of `overlays/` by region name |

The bundle keeps each file as it is, without `about` and `where`. The engine resolves a region as
the prototype's `regions.ts` does:

- A file inherits what it does not say from the file that it `extends`. Objects merge, and any
  other value replaces.
- `look.ramps` of a cover comes whole from the nearest region of the chain that names it.
- A value by place, `{"es-ga": …, "es": …, "*": …}`, takes the entry of the subdivision, else of
  the country, else `"*"`. The release has no layer of the place yet, so the engine takes `"*"`.
- In a pack, `trees`, `ground` and `herds` are keyed by species. A child adds or changes one, and
  `null` drops one.

An overlay holds the look keys of the engine that the prototype does not have, as
`{"look": {…}}`. The engine merges the overlays of the chain of a region over its look, in the
same way. The key table `OVERLAY_LOOK` of `rules.py` lists the keys that an overlay may set.

## Canopy

`trailview-canopy` holds the forest of each cell of zoom 12 at 256 × 256 cells, 26 m at 47° N:
the canopy height, the cover, a class of the stems per hectare and the leaf type. It holds no tree
position: the engine places trees that match these values. The layer is the optional planner layer
`trailview-canopy` of [obc-data.md](obc-data.md). Its bake, `tools/planner_trailview_canopy.py`,
reads the prepared sources [`forest-reference/<key>`](obc-data.md#forest-referencekey). A cell
takes its values from the first of them, in the order of `data/sources.toml`, that knows the cell.

| Field | Value |
| --- | --- |
| Format | PMTiles v3 |
| Tiles | Web Mercator XYZ, zoom 12 only, 256 × 256 cells |
| Tile type | `webp` (4), lossless |
| Tile compression | none |
| Bounds | The region bounds |

Cell (column, row) of tile 12/x/y covers that cell of the 256 × 256 grid of the tile. Row 0 is the
north edge and column 0 is the west edge. A tile that the archive does not hold is unknown: no
forest reference has it. The release packs the archive in PMTiles per zoom 11 parent:
`maps/tiles/trailview-canopy/<z>-<x>-<y>.pmtiles`, with the TileJSON `maps/trailview-canopy.json`.

| Metadata | Value |
| --- | --- |
| `canopy_format` | `1`. A reader reads only the format that it knows |
| `archive` | `trailview-canopy` |
| `attribution` | Text to show with the layer |

Decode a tile without colour conversion (`createImageBitmap` with `colorSpaceConversion:
'none'`). The code of a cell is red + 256 × green; blue is 0. The code 65535 is a cell that no
forest reference knows. The code 0 is a known cell that is not a forest cell. A forest cell has a
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
