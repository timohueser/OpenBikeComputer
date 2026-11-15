# Offline planner bundle

A bundle carries one [planner release](planner-release.md) that the download service selects from a
grid publication: the exact release manifest and every runtime file, without source mirrors. This
contract also defines the grid selection, the download service API and the iOS library.

- Version: bundle manifest `format` `1`; `offline/catalog.json` and `offline/cells/CELL.json`
  `format: 5`; selected release `offline.format: 4`; service catalog `format: 2`; selection
  format `7`.
- Writers: the download service (`tools/planner_downloads.py`); the step `planner/offline/grid`
  (`tools/planner_grid_offline.py`, offline selection metadata).
- Readers: the companion app (`OBCPlanner/OfflineMapStore.swift`).
- Related: [planner release](planner-release.md), [route package](route-package.md), [route
  catalog](route-catalog.md).

## Transfer layout

| Path | Content |
| --- | --- |
| `bundle.json` | UTF-8 JSON bundle manifest |
| `release.json` | Exact original release manifest bytes |
| `objects/SHA256` | Identity or gzip transport bytes with this SHA-256 |

The bundle manifest has these fields:

| Field | Value |
| --- | --- |
| `format` | `1` |
| `release` | Original release manifest `bytes` and `sha256` |
| `files` | Every runtime path from the release, with no omissions or extra paths |

Each file entry repeats the release's `bytes` and `sha256`. Its `transport` object has `bytes`,
`sha256`, and `encoding`. Encoding is `identity` or `gzip`. Decoded bytes match the release entry.
Repeated content uses one object. Paths are relative, contain no `..`, and cannot replace
`release.json`.

## Grid publication and selection

A grid release has `grid: {format: 3, zoom: 9}`. Selection cells use Web Mercator XYZ
coordinates at zoom 9. Cell IDs are `9-X-Y`. A requested rectangle selects every intersecting cell.
The service rejects a rectangle that is not inside the release bounds. The returned coverage
contains the complete requested rectangle.

The publisher creates search databases and PMTiles packs once. A map pack
`maps/tiles/<kind>/<z>-<x>-<y>.pmtiles` holds the tiles of square `z/x/y` from zoom `z` to the last
zoom of the level range of `z`:

| Tile zooms | Base level |
| --- | --- |
| 0–6 | 0 |
| 7–8 | 7 |
| 9 and deeper | 9 |

Each tile goes into the pack of its square at the base level of its zoom. When the PMTiles file of
a pack is larger than 15 MiB, the pack keeps only its one tile at its own zoom. Its four squares one
zoom deeper get packs for its deeper tiles, by the same rule. A pack with no tiles has no file. A
pack that holds one tile cannot split: if it is larger than 15 MiB, the bake fails. Map packs are
stored without compression, so the limit is the stored size. A cold range read straight from
`maps.` waits until Cloudflare has read the pack from R2 up to the range, at about 17–25 MB/s. So the
last tile of a cold 15 MiB pack waits about 1 s.

The pack of a tile is the deepest pack that exists whose square holds the tile, at a zoom from the
tile zoom back to the base level of the tile zoom. A tile that this pack does not hold is absent.

Original compressed tile payloads remain unchanged. Terrain is the exception: its tiles are encoded
again with the same pixels, and its zoom 13 tiles have packs of their own ([terrain
tiles](planner-release.md#terrain-tiles)). Search retains whole intersecting records
and their dependencies. Each cell also has its [route catalog](route-catalog.md) file.

The routing of an area is not part of the grid. It is each download cell of the [route
package](route-package.md#installed-files-and-the-installer) whose `box` meets the area, grown on
each side by the half-sizes of the snap window of 1 km at the corner of the area nearest to a pole.
That window is the largest of any point of the area, so no point of the area is `missing_region`.
Long roads do not count. The phone selects these cells from `routing/directory.bin`.

The download service reads `offline/catalog.json` when it starts, and the cell files of each
request. An entry is a step index entry: the decoded `bytes` and `sha256`, and the `transport`
above.

| `offline/catalog.json` field | Value |
| --- | --- |
| `format` | `5`, the format of the layer `planner/index` |
| `zoom` | `9` |
| `release` | The header of the [release manifest](planner-release.md#manifest), with `format: 1` and without `grid`, `indexes` and `files` |
| `cells` | The sorted IDs of the cells |
| `shared` | Each map asset path of a selection, and `routing/package.json` and `routing/directory.bin` of the route package, mapped to its publication file |
| `assets` | Each publication file that `shared` names, mapped to its entry |

`offline/cells/CELL.json` describes one cell:

| Field | Value |
| --- | --- |
| `format` | `5` |
| `id`, `bounds` | The cell ID and its bounds, clipped to the release bounds |
| `files` | The search databases and the route catalog file of the cell, each mapped to its entry |
| `packs` | The map packs that an area of the cell takes, each mapped to its entry |

The `packs` of a cell are the basemap, places, overlay and terrain packs of its `bounds`. Terrain
adds one ring of tiles. They omit the zoom 13 terrain
packs. The service selects these objects and writes only the selection manifests. It does not
rebuild routing, SQLite databases, or map payloads.

A selection keeps every glyph range path, `maps/assets/fonts/STACK/RANGE.pbf`. All range paths of
one font stack share `offline/fonts/STACK.pbf`. This file joins the stack's range files in range
order. It keeps only the ranges that contain a character of a basemap label field or a route network
reference in the release. The label fields are `name`, `name:en`, `name2`, `name3`, their `pgf:`
forms, `ref`, `ref:en`, `shield_text`, and `addr_housenumber`. The ranges also cover the upper-case
form of each text. Text with a character from U+0600 to U+08FF adds the Arabic presentation forms,
U+FB00 to U+FEFF. A missing range file stops the labels of each tile that requests it.

The selected release has `offline` with `format: 4`, `id`, `zoom`, and `cells`. Each cell has
`id`, `bounds`, and `files`: the cell's search databases and route catalog file. Its `region` is
`area-` and the first 24 hex digits of the selection ID. A selection takes the union of the `packs`
of its cells and the two files of the route package in `shared`. Its `routing_package` is the
SHA-256 of `routing/directory.bin`, the identity of the route package. It has no
`routing/blocks.json`. The TileJSON bounds and `terrain_bounds` of a selection are the box of its
cells, so a route can leave the map of its area. `maps/basemap.json`, `maps/overlays.json` and `maps/terrain.json` are TileJSON for these
packs. Their tile URLs use the host `offline.openbikecomputer.invalid`, which the app serves from
the installed packs. The app finds the pack of a tile by the rule above, from the files of the
bundle.

## Download service

A client calls the service that `downloads` of the active release in the planner pointer names.
Without `downloads`, it calls `/planner-offline` of the API origin. A job `source` has the scheme of
the service. Bounds use west, south, east, north.

| Request | Result |
| --- | --- |
| `GET /catalog` | `format: 2`, source `bounds`, and selection `zoom` |
| `POST /jobs` | JSON `bounds`; returns `id` and `state: ready` once the selection metadata exists |
| `GET /bundles/ID/bundle.json` | Selected bundle manifest |
| `GET /bundles/ID/release.json` | Selected release manifest |
| `GET /bundles/ID/objects/SHA256` | Transport bytes or HTTP 307 to the immutable object pool, also for each cell object of the route package |

The ID hashes the selection format (`7`), the publication catalog, the bounds of the grid cells
that the request overlaps, the SHA-256 of each of their cell files, and the URL of the object pool.
A cell of the catalog without its cell file is an error. Repeated selections reuse metadata.
Cancellation stops the client request. Error responses contain `message`. Selections outside the
release bounds are rejected.

Objects support HEAD and byte ranges. The service stores the publication origin with each cached
selection. Least recently used selections can expire from its bounded metadata cache. A missing
selection returns 404; the client must request a new selection. The companion bundle omits the
server language model and device catalog.

## iOS library

| Path | Content |
| --- | --- |
| `objects/SHA256` | Verified decoded file bytes |
| `downloads/SHA256` | Incomplete or complete transport bytes pending verification |
| `downloads/SHA256.resume` | Opaque URLSession resume data |
| `releases/ID/` | Original runtime file layout, linked to verified objects |
| `routing/DIRECTORY/` | The install of the route package whose `directory.bin` has the SHA-256 `DIRECTORY`, with the union of the areas of the maps of that package |
| `maps.json` | List of installed maps |
| `pending.json` | One resumable download |

Each `maps.json` entry contains `id`, `name`, `region`, `bounds`, `routing` (the SHA-256 of
`routing/directory.bin`), and `installedBytes`.

Before each change of `maps.json`, the app installs the union of the areas of the maps of the
routing package that the change gives; with no area left, the install folder goes. A map enters
`maps.json` only after its install is complete. The cell objects of the route package stay in
`objects/` only until their install. The `installedBytes` of a map count the routing of its own
area, so two maps that share cells both count them.

The app checks free space before transfer. Its estimate includes filesystem allocation, missing
decoded objects, the four largest temporary compressed objects, installation metadata, each missing
cell object of the route package and its installed part, and for a new routing package the copies
of `package.json` and `directory.bin` in its install folder. At most
four downloads run at once. Compressed objects are decoded and removed individually. Verification
precedes the atomic library update. The library is excluded from device backups. Deletion retains
shared objects that another map or the pending download needs.

Each download prohibits cellular, expensive and constrained networks unless the user allows them.
The preparation of a download is the exception: it fetches `routing/package.json` and
`routing/directory.bin` on any network, to plan the cells of the area.
Maps, routes and search first use a complete installed release that covers the request. A route
uses the install of the first map that holds its points. A failed local request falls back to the
online service. A long road of a download cell that the area does not install is no candidate on the
phone, where the server can snap to it. A valid empty
local search result does not need a network request.
