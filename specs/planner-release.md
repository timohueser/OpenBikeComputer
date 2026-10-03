# Planner release

A release joins map, routing, search, model, and device catalogue data.
`release.json` is UTF-8 JSON. Its SHA-256 is the release ID. The release and
its files are immutable.

## Manifest

| Field | Value |
| --- | --- |
| `format` | `1` |
| `region` | Lowercase region ID, with letters, digits, and hyphens |
| `bounds` | `[west,south,east,north]` in degrees |
| `osm_sha256` | Hash of the common OSM PBF |
| `routing_package` | Hash of `routing/manifest.json` or `routing/blocks.json` |
| `profiles` | Sorted routing profile IDs |
| `attribution` | OSM source credit and licence |
| `terrain_attribution` | Elevation source credits |
| `terrain_bounds` | Bounds that include contour neighbour tiles |
| `sources` | Recipe hash, source identities, tool identities, and input provenance |
| `device_catalog_source` | Original device catalogue URL |
| `files` | Relative file names, each with `bytes` and `sha256` |
| `source_files` | Local source mirror names, each with `bytes` and `sha256` |
| `probe` | Regional route points, search query, and view for service deployment; absent from offline cutouts |

File paths stay inside the release directory. File hashes use lowercase
64-character hex. Maps, search, and routing have the same OSM hash and bounds.
The terrain inputs cover every routing elevation input. Search metadata contains
the OSM hash. The map manifest contains terrain input hashes.

## Objects and services

R2 stores release files under `planner/releases/ID/`. It stores source mirrors
under `planner/sources/`, without the local `sources/` path prefix. A source name
starts with its SHA-256. Source mirrors can be shared by releases.

`maps/` contains `basemap.pmtiles`, `places.pmtiles`, `overlays.pmtiles`,
`terrain.pmtiles`, map assets, and their manifest. When the region recipe names a
snow source, it also contains `snow.pmtiles` from the
[snow tile contract](planner-snow-tiles.md). When the recipe has a `climate` field,
it also contains `climate.pmtiles` from the
[climate tile contract](planner-climate-tiles.md). The release `files` hold exactly the
files of the map manifest and the manifest itself. `routing/` contains the three files in the
[route package contract](route-package.md), plus `overlays.sqlite`. The overlay
index stores the routing manifest identity and has the same OSM source.
`search/` contains `REGION.sqlite`
and `model/`. `device/catalog.json` is a snapshot. Its file references are absolute
URLs to the original immutable cell objects.

The overlay database has SQLite `user_version=2`. `features` stores stable IDs,
layer kinds, cycling and walking minimum zooms, geometry IDs, and attribute IDs.
`attributes` stores distinct JSON properties without the way ID or layer kind.
Route memberships are ordered relation IDs. `routes` stores each relation's JSON
properties once. `geometries` stores the way ID, a point count, and a Postcard `Vec<[i32;2]>`.
Each coordinate pair is longitude and latitude in microdegrees. The first pair
is absolute; each later pair is a difference from the previous pair. Decoding
uses checked addition. Shared geometry retains every source point. `bounds` is
an R-tree over feature IDs with longitude, latitude, and facet axes. The facet
is `32 * layer + minimum_zoom`. Cycling, hiking, and access have layer values
0, 1, and 2. The minimum is the lower non-null mode minimum, or 23 for an
invisible feature. The facet has equal lower and upper bounds. The query also
checks each mode's minimum zoom. A cutout retains every referenced geometry,
attribute, and route.

`places.pmtiles` holds the rider places of the basemap. It has gzip MVT tiles
at zoom 11 only, with extent 4096 and one `pois` layer. Each feature is one
point with the basemap feature ID and the basemap `kind`, `name`, and `name:en`
properties. The kinds are the `kinds` keys of the web planner's
[place categories](../builder/app/src/lib/planner/poi-kinds.json). The bake reads
the basemap's deepest zoom. Each place occurs once, in the tile that contains it.
A tile with no places is absent.

`overlays.pmtiles` holds the route networks and access restrictions of the
overlay index. It has gzip MVT tiles from zoom 6 to 14, with extent 4096. Its
metadata `routing_package` is the routing manifest identity of the overlay index.
In a grid release, `maps/overlays.json` names the grid routing package, the
SHA-256 of `routing/blocks.json`, which packs the same graph.

| Layer | Feature ID | Properties |
| --- | --- | --- |
| `cycling`, `hiking` | Way ID of the first way in the line | `rank`, `ref`, and `routes`: a JSON array of relation IDs in rank order. `hiking` adds `marker`, the first route `symbol` that is not empty. |
| `access` | Way ID | `cycling_status`, `walking_status`, `name`, `ref`, `conditional`; `riding`, `walking`, `pushing` and `tags` as JSON text; `cycling_minzoom` and `walking_minzoom`, the overlay index minimum zoom of each restricted mode |
| `routes` | Relation ID | The route properties that are not empty: `kind`, `network`, `rank`, `name`, `ref`, `website`, `symbol`, `symbol_text`. The point is the tile origin. |

A feature starts at the lower mode minimum zoom of its overlay index feature,
at most zoom 14. The planner shows access from the `MODE_minzoom` of its travel mode.
A line names only the routes that start at or below the tile zoom. Lines with
equal properties join where exactly two of them meet. The `routes` layer of a
tile holds each route that its lines name. A tile with no features is absent.

The tile API serves `/releases/ID/basemap.json`, vector tiles at
`/releases/ID/basemap/Z/X/Y.mvt`, `/releases/ID/places.json`, places tiles at
`/releases/ID/places/Z/X/Y.mvt`, `/releases/ID/overlays.json`, overlay tiles at
`/releases/ID/overlays/Z/X/Y.mvt`, Terrarium tiles at
`/releases/ID/terrain/Z/X/Y.webp`, and, for a release with snow,
`/releases/ID/snow.json` and snow tiles at `/releases/ID/snow/Z/X/Y`. Each
TileJSON holds its archive metadata. Snow tiles keep their gzip encoding. An absent tile returns 204. The raw archives
remain downloadable from R2.

Routing and search APIs have the prefix `/planner-api/releases/ID/`. The final
path component selects `routing` or `search`. A rollout serves the active
release and the previous release on separate VPS ports.

## Canonical grid storage

The online services and `deploy` serve grid releases only. A regional release
without `grid` is for local preview. A grid release adds `grid: {format: 2, zoom: 9, map_zoom: 11}`. Its `files`
entries retain logical paths and decoded `bytes` and `sha256`. Each also has
`transport: {bytes, sha256, encoding}`. Encoding is `identity` or `gzip`.
R2 stores each distinct transport once at `planner/releases/ID/objects/SHA256`.
The online services and offline installer consume this same pool.

`public/grid.json` contains `format: 2` and `map_zoom`. Each map pack, asset,
TileJSON, and device catalog has a small pointer at `public/LOGICAL_PATH.json`.
A pointer repeats the transport entry and adds `decoded_bytes`. The tile
service resolves a pack through this pointer. A tile without a pack is absent.
No regional map archive is required beside the pool. Grid assets use the tile
service origin.

The VPS materializes routing, search, and offline selection metadata. Search
uses `search/REGION.grid.json` to list cell files and coverage. The
[offline contract](planner-offline.md#grid-publication-and-selection) defines
cell selection and download manifests.

## Catalogue

`planner/catalog.json` has `format: 1`, `active`, and `previous`.
`active` contains the release `id`, manifest URL, region, bounds, attribution,
device catalogue URL, map asset URLs, tile URLs, and routing and search API
prefixes. `snow` is the snow TileJSON URL, only for a release with snow. Its `slot` is `0` or `1`. `previous` has the same shape or is `null`.

The publisher uploads and verifies all release files before the manifest.
The deployer verifies public services before changing the catalogue. The
catalogue cache lifetime is 30 seconds. Immutable objects have a one-year
cache lifetime. Rollback swaps `active` and `previous` after service checks.
A site build reads one active catalogue entry and uses it for every planner
endpoint and the map builder's device catalogue.

Finalization verifies the live services and site against `active`. It removes
inactive planner releases and source mirrors that `active` does not name.
It then sets `previous` to `null`. Device cell objects and terrain reference
objects remain outside planner cleanup. A completed rollout retains one
regional planner dataset in R2.
