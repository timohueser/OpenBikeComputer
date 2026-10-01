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

## Download service

The HTTPS prefix is `/planner-offline`. Bounds use west, south, east, north.

| Request | Result |
| --- | --- |
| `GET /catalog` | `format: 1`, source `bounds`, and `regions` |
| `POST /jobs` | JSON with either `bounds` or a `region` ID; returns a job |
| `GET /jobs/ID` | Job `id`, `state`, and optional failure `message` |
| `GET /bundles/ID/bundle.json` | Bundle manifest for a ready job |
| `GET /bundles/ID/release.json` | Original release manifest |
| `GET /bundles/ID/objects/SHA256` | Verified transport bytes; supports HEAD and byte ranges |

A job state is `preparing`, `ready`, or `failed`. Its ID hashes the source release
ID and bounds. Only one preparation runs at a time. Repeated selections reuse a
complete cached bundle. A failed job can be requested again. Error responses
contain a `message`. Selections outside source coverage are rejected.

A region has `id`, `name`, nullable `parent`, `bounds`, `available`, and `rings`.
Rings contain longitude-latitude pairs. The hierarchy preserves all matching
region choices at a point. A region download selects its enclosing rectangle.
Regions outside source coverage remain visible but unavailable.

## iOS library

The iOS installer uses the object and release layout above. `maps.json` replaces
`active.json` with a list of installed maps. Each entry contains `id`, `name`,
`region`, `bounds`, and `installedBytes`. `pending.json` holds one resumable
download. `downloads/SHA256.resume` holds opaque URLSession resume data.

The app checks free space before transfer and before each object. Its estimate
includes filesystem allocation, missing decoded objects, the largest temporary
compressed object, and installation metadata. Compressed objects are decoded
and removed individually. Verification precedes the atomic library update.
The library is excluded from device backups. Deletion retains shared objects
that another map or the pending download needs.

Each download prohibits cellular, expensive and constrained networks unless
the user allows them. Maps, routes, search and overlays first use a complete
installed release that covers the request. Routing graphs are not combined.
A failed local request falls back to the online service. A valid empty local
search result does not need a network request.
