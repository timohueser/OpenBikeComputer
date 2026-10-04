# Offline planner bundle

A bundle carries one [planner release](planner-release.md) that the download
service selects from a grid publication. It preserves the exact release manifest
and every runtime file. It omits source mirrors.

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

Each file entry repeats the release's `bytes` and `sha256`. Its `transport` object
has `bytes`, `sha256`, and `encoding`. Encoding is `identity` or `gzip`. Decoded
bytes match the release entry. Repeated content uses one object. Paths are relative,
contain no `..`, and cannot replace `release.json`.

## Grid publication and selection

A grid release has `grid: {format: 2, zoom: 9, map_zoom: 11}`. Selection cells
use Web Mercator XYZ coordinates at zoom 9. Cell IDs are `9-X-Y`. A requested
rectangle selects every intersecting cell, clipped to the release bounds.
The returned coverage contains the complete requested rectangle.

The publisher creates routing page packs, search databases, and PMTiles packs
once. Map packs group each tile under its ancestor at `min(tile_zoom, 11)`.
Original compressed tile payloads remain unchanged.
A routing pack contains pages with the same cell consumers, up to 16 MiB.
Search retains whole intersecting records and their dependencies.
Each cell also has its [route catalog](route-catalog.md) file.

`offline/catalog.json` has `format: 3`. It lists cell bounds, logical file
names, map packs, and routing cell descriptors. Its map packs omit the online
places archive. Each routing descriptor has a manifest path and SHA-256, source
adjacency ranges, and retained geometry bounds. `shared` maps
each map asset path of a selection to its publication file. Files carry the
decoded and transport hashes above.
The service selects these objects and writes only the small selection manifests.
It does not rebuild routing, SQLite databases, or map payloads.

A selection keeps every glyph range path, `maps/assets/fonts/STACK/RANGE.pbf`.
All range paths of one font stack share `offline/fonts/STACK.pbf`. This file
joins the stack's range files in range order. It keeps only the ranges that
contain a character of a basemap label field or a route network reference in
the release. The label fields are `name`, `name:en`, `name2`, `name3`, their
`pgf:` forms, `ref`, `ref:en`, `shield_text`, and `addr_housenumber`. The
ranges also cover the upper-case form of each text. Text with a character from
U+0600 to U+08FF adds the Arabic presentation forms, U+FB00 to U+FEFF. MapLibre
reads only the glyphs of the requested range from the file. A missing range
file stops the labels of each tile that requests it.

The selected release has `offline.format: 2`, `id`, `zoom`, `map_zoom`,
`source_routing`, and `cells`. Each cell has `id`, `bounds`, and `files`: the
cell's search databases and route catalog file.
`routing/blocks.json` follows the [routing selection contract](route-package.md#grid-selections).
Map selection takes basemap, overlay and terrain packs. It covers retained road
geometry; terrain includes tile neighbours. `maps/basemap.json`,
`maps/overlays.json` and `maps/terrain.json` are TileJSON for these packs. Their
tile URLs use the host `offline.openbikecomputer.invalid`, which the app serves
from the installed packs.

## Download service

The HTTPS prefix is `/planner-offline`. Bounds use west, south, east, north.

| Request | Result |
| --- | --- |
| `GET /catalog` | `format: 1`, source `bounds`, and selection `zoom` |
| `POST /jobs` | JSON `bounds`; returns `id`, `state: ready`, and `progress: 1` |
| `GET /jobs/ID` | `id` and `state`, either `ready` or `failed` |
| `GET /bundles/ID/bundle.json` | Selected bundle manifest |
| `GET /bundles/ID/release.json` | Selected release manifest |
| `GET /bundles/ID/objects/SHA256` | Transport bytes or HTTP 307 to the immutable object pool |

The ID hashes the publication catalog, rounded bounds, and selection format.
Repeated selections reuse metadata. There is no queued preparation worker or
reservation. Cancellation stops the client request. Error responses contain
`message`. Selections outside source coverage are rejected.

Objects support HEAD and byte ranges. The service stores the publication origin
with each cached selection. Generated manifests stay in its bounded metadata
cache. Least recently used selections can expire. A missing selection returns
404; the client must request a new selection. The companion bundle omits the
server language model and device catalog.

## iOS library

| Path | Content |
| --- | --- |
| `objects/SHA256` | Verified decoded file bytes |
| `downloads/SHA256` | Incomplete or complete transport bytes pending verification |
| `downloads/SHA256.resume` | Opaque URLSession resume data |
| `releases/ID/` | Original runtime file layout, linked to verified objects |
| `maps.json` | List of installed maps |
| `pending.json` | One resumable download |

Each `maps.json` entry contains `id`, `name`, `region`, `bounds`, and
`installedBytes`.

The app checks free space before transfer. Its estimate includes filesystem
allocation, missing decoded objects, the four largest temporary compressed
objects, and installation metadata. At most four downloads run at once. Compressed objects are decoded
and removed individually. Verification precedes the atomic library update.
The library is excluded from device backups. Deletion retains shared objects
that another map or the pending download needs.

Each download prohibits cellular, expensive and constrained networks unless
the user allows them. Maps, routes and search first use a complete installed
release that covers the request. Routing graphs are not combined.
A failed local request falls back to the online service. A valid empty local
search result does not need a network request.
