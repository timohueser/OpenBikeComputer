---
system: planner-tile-server
kind: shell
purpose: "Serves the map tiles of a planner release from the storage bucket or from a local folder."
owns:
  - planner/tiles/**
depends_on: []
test_depends_on: []
status: approved
---

# Planner tile server

## What it does

The route planner reads its map tiles from this server: the base map, the places, the route networks and the terrain. The server is a Cloudflare Worker ([`worker.mjs`](../../planner/tiles/src/worker.mjs)). For each request, it finds the tile archive (a PMTiles file) of the release in the R2 storage bucket, and reads only the bytes of the tile. It keeps each answer in the cache of the network edge. The [release contract](../../specs/planner-release.md) defines the paths in the bucket.

[`local.mjs`](../../planner/tiles/src/local.mjs) serves the same tiles from a folder on this machine, on the loopback address. `obc data serve` starts it.

An operator deploys the Worker with `obc data deploy tiles`. The planner map baking writes the archives; this server only reads them.

## Rules that must hold

| Rule | Check |
| --- | --- |
| A request that is not valid reads nothing from the bucket. | `invalid requests perform no bucket access` in [`worker.test.mjs`](../../planner/tiles/test/worker.test.mjs) |
| The rate limit counts only requests that the cache does not answer. | `a client over its limit is refused on a cache miss only` in [`worker.test.mjs`](../../planner/tiles/test/worker.test.mjs) |
| The local server gives the same bytes as the Worker. | `Local files serve the same grid bytes over loopback without an R2 cache` in [`worker.test.mjs`](../../planner/tiles/test/worker.test.mjs) |

## Decisions

- **Only a cache miss counts against the limit.** Only a miss reads the bucket, so the limit of a client address counts only misses.
- **No tile deeper than zoom 16.** No archive has deeper tiles, so the Worker refuses a deeper request before it reads the bucket.
