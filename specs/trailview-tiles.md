# TrailView tiles

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
north edge and column 0 is the west edge. A tile that the archive does not hold has no forest
cell, or no forest reference covers it. The release packs the archive in PMTiles per zoom 11
parent: `maps/tiles/trailview-canopy/<z>-<x>-<y>.pmtiles`, with the TileJSON
`maps/trailview-canopy.json`.

| Metadata | Value |
| --- | --- |
| `canopy_format` | `1`. A reader reads only the format that it knows |
| `attribution` | Text to show with the layer |

Decode a tile without colour conversion (`createImageBitmap` with `colorSpaceConversion:
'none'`). The code of a cell is red + 256 × green; blue is 0. Every bit of the code of a cell that
is not a forest cell is 0. A forest cell has a canopy height of 3 m or more and a cover of 5 % or
more.

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
