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
each archive in PMTiles per zoom 11 parent: `maps/tiles/trailview-<archive>/<z>-<x>-<y>.pmtiles`,
with the TileJSON `maps/trailview-<archive>.json`.

The archive metadata is a JSON object:

| Key | Value |
| --- | --- |
| `trailview_format` | `1`. A reader reads only the format that it knows |
| `layer` | `material`, `lithology` or `region`; absent in a colour archive |
| `archive` | The name of the archive: the layer, or `q1` to `q4` and `ndvimax` |
| `variant` | Only in a colour archive: the variant of the table in [Colour](#colour) |
| `attribution` | Text to show with the layer |

## Ground

`trailview-ground` has three archives. Each texel is one byte, as lossless WebP. Decode it
without colour conversion (`createImageBitmap` with `colorSpaceConversion: 'none'`): the red
channel holds the byte, and green and blue are equal to it.

| Archive | Zooms | Byte |
| --- | --- | --- |
| `material` | 0 to 13 | The material code and two flags, see below |
| `lithology` | 0 to 10 | The index of the lithology in `lithologies` of `materials.json`; 0 is unknown |
| `region` | 0 to 10 | The region id: the index of the region in `regions` of `rules.json` |

The material byte:

| Bits | Meaning |
| --- | --- |
| 0–5 | The index of the material in `materials` of `materials.json` |
| 6 | Barren: the highest NDVI that a quarter of the year measured is below 0.25 |
| 7 | No data: the texel is outside the region bounds. The other bits are 0 |

A material code above 63 needs a new format. A region id above 255 needs a new format, with
two bytes.

A tile of the last zoom of an archive comes from the sources. A coarser texel takes the most
common byte of its four children, the first in reading order on a tie. A material texel with no
data counts only where all four have no data.

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

`trailview-colour` has five archives from the Sentinel-2 quarterly mosaics of one year. Quarter
q holds the months 3q − 2 to 3q.

| Archive | Content |
| --- | --- |
| `q1` to `q4` | RGBA: the colour of the quarter, and its NDVI code in alpha |
| `ndvimax` | One byte, lossless as in [Ground](#ground): the highest NDVI code of the four quarters |

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
linear light, of its children that looked, and the mean NDVI code of those that measured one. It
is 1 where more children saw snow than measured an NDVI.

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
  the country, else `"*"`.
- In a pack, `trees`, `ground` and `herds` are keyed by species. A child adds or changes one, and
  `null` drops one.

An overlay holds the look keys of the engine that the prototype does not have, as
`{"look": {…}}`. The engine merges the overlays of the chain of a region over its look, in the
same way. The key table `OVERLAY_LOOK` of `rules.py` lists the keys that an overlay may set.
