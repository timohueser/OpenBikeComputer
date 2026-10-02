# Offline planner bundle

A bundle contains the runtime files from one [planner release](planner-release.md).
It omits source mirrors. It preserves the exact release manifest and every runtime
file. It does not combine routing graphs or change coverage.

A geographic cutout is a new release. It rebuilds routing for whole intersecting
roads and retains all source profiles. Its declared bounds limit route endpoints.
It omits source OSM tables after routing and overlays are compiled.
Map and overlay selection covers both these bounds and the complete retained road
geometry. Terrain adds its tile neighbours. Search includes its address and context
dependencies. Intersecting overlay features keep complete geometry and properties.
The source release identity and geometry envelope are recorded in `sources.extraction`.
Old and new graphs do not combine into a regional union.

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

## Local installation

| Path | Content |
| --- | --- |
| `objects/SHA256` | Verified decoded file bytes |
| `downloads/SHA256` | Incomplete or complete transport bytes pending verification |
| `releases/ID/` | Original runtime file layout, linked to verified objects |
| `active.json` | Active release `release` ID and `region` |

The installer locks one destination at a time. It resumes transport downloads by
byte offset. An HTTP 206 response must match the requested range and complete
object size. An HTTP 200 response restarts the object. A short download stays
available for resumption. A complete object with a wrong hash is discarded.

Activation follows verification of the complete decoded runtime closure. Files
and directories are synchronized before the atomic activation write. A failure
keeps the previous activation. Old releases and verified objects stay installed.
This layout does not provide eviction or combine separately prepared regions.

## Size fields

The pack and install commands return JSON. `transfer_bytes` includes both
manifests and each distinct transport object. `installed_bytes` counts the logical
release files and release manifest. `unique_installed_bytes` deduplicates equal
runtime files by their decoded hashes.

An install also reports bytes received in this run as `downloaded_bytes`, and
file bytes already retained as `retained_before_bytes`. `stored_bytes` counts
distinct installed file inodes, including retained releases, objects, downloads,
and activation metadata. `peak_install_bytes` includes transient decoded files
and activation writes. `peak_added_bytes` and `added_stored_bytes` subtract the
initial retained total. `download_cache_bytes` counts pending transport files.
These are file byte counts. They exclude filesystem allocation overhead and
caches made by the application.

## Grid publication and selection

A grid release has `grid: {format: 2, zoom: 9, map_zoom: 11}`. Selection cells
use Web Mercator XYZ coordinates at zoom 9. Cell IDs are `9-X-Y`. A requested
rectangle selects every intersecting cell, clipped to the release bounds.
The returned coverage contains the complete requested rectangle.

The publisher creates routing page packs, search databases, overlay databases,
and PMTiles packs once. Map packs group each tile under its ancestor at
`min(tile_zoom, 11)`. Original compressed tile payloads remain unchanged.
A routing pack contains pages with the same cell consumers, up to 16 MiB.
Search and overlays retain whole intersecting records and their dependencies.

`offline/catalog.json` has `format: 3`. It lists cell bounds, logical file
names, map packs, and routing cell descriptors. Each routing descriptor has a manifest path and
SHA-256, source adjacency ranges, and retained geometry bounds. `shared` maps
each map asset path of a selection to its publication file. Files carry the
decoded and transport hashes above.
The service selects these objects and writes only the small selection manifests.
It does not rebuild routing, SQLite databases, or map payloads.

A selection keeps every glyph range path, `maps/assets/fonts/STACK/RANGE.pbf`.
All range paths of one font stack share `offline/fonts/STACK.pbf`. This file
joins the stack's range files in range order. It keeps only the ranges that
contain a character of a basemap label field or a route network reference in
the release. The label fields are `name`, `name:en`, `name2`, `name3`, their
`pgf:` forms, `ref`, `ref:en`, `shield_text`, and `addr_housenumber`. MapLibre
reads only the glyphs of the requested range from the file. A missing range
file stops the labels of each tile that requests it.

The selected release has `offline.format: 2`, `id`, `zoom`, `map_zoom`,
`source_routing`, and `cells`. Each cell has `id` and `bounds`.
`routing/layers.json` lists every required overlay cell ID. A missing listed
cell is an error. `routing/blocks.json` follows the
[routing selection contract](route-package.md#grid-selections).
Map selection covers retained road geometry; terrain includes tile neighbours.

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

The iOS installer uses the object and release layout above. `maps.json` replaces
`active.json` with a list of installed maps. Each entry contains `id`, `name`,
`region`, `bounds`, and `installedBytes`. `pending.json` holds one resumable
download. `downloads/SHA256.resume` holds opaque URLSession resume data.

The app checks free space before transfer. Its estimate includes filesystem
allocation, missing decoded objects, the four largest temporary compressed
objects, and installation metadata. At most four downloads run at once. Compressed objects are decoded
and removed individually. Verification precedes the atomic library update.
The library is excluded from device backups. Deletion retains shared objects
that another map or the pending download needs.

Each download prohibits cellular, expensive and constrained networks unless
the user allows them. Maps, routes, search and overlays first use a complete
installed release that covers the request. Routing graphs are not combined.
A failed local request falls back to the online service. A valid empty local
search result does not need a network request.
