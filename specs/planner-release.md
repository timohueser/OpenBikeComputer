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
| `routing_package` | Hash of `routing/manifest.json` |
| `profiles` | Sorted routing profile IDs |
| `attribution` | OSM source credit and licence |
| `terrain_attribution` | Elevation source credits |
| `terrain_bounds` | Bounds that include contour neighbour tiles |
| `sources` | Recipe hash, source identities, tool identities, and input provenance |
| `device_catalog_source` | Original device catalogue URL |
| `files` | Relative file names, each with `bytes` and `sha256` |
| `source_files` | Local source mirror names, each with `bytes` and `sha256` |
| `probe` | Regional route points, search query, and view used before activation |

File paths stay inside the release directory. File hashes use lowercase
64-character hex. Maps, search, and routing have the same OSM hash and bounds.
The terrain inputs cover every routing elevation input. Search metadata contains
the OSM hash. The map manifest contains terrain input hashes.

## Objects and services

R2 stores release files under `planner/releases/ID/`. It stores source mirrors
under `planner/sources/`, without the local `sources/` path prefix. A source name
starts with its SHA-256. Source mirrors can be shared by releases.

`maps/` contains `basemap.pmtiles`, `terrain.pmtiles`, map assets, and their
manifest. `routing/` contains the three files in the
[route package contract](route-package.md), plus `overlays.sqlite`. The overlay
index stores the routing manifest identity and has the same OSM source.
`search/` contains `REGION.sqlite`
and `model/`. `device/catalog.json` is a snapshot. Its file references are absolute
URLs to the original immutable cell objects.

The tile API serves `/releases/ID/basemap.json`, vector tiles at
`/releases/ID/basemap/Z/X/Y.mvt`, and Terrarium tiles at
`/releases/ID/terrain/Z/X/Y.webp`. An absent tile returns 204. The raw archives
remain downloadable from R2.

Routing and search APIs have the prefix `/planner-api/releases/ID/`. The final
path component selects `routing` or `search`. A rollout serves the active
release and the previous release on separate VPS ports.

## Catalogue

`planner/catalog.json` has `format: 1`, `active`, and `previous`.
`active` contains the release `id`, manifest URL, region, bounds, attribution,
device catalogue URL, map asset URLs, tile URLs, and routing and search API
prefixes. Its `slot` is `0` or `1`. `previous` has the same shape or is `null`.

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
