# Planner tiles

Serve immutable regional PMTiles from the `obc-maps` R2 bucket. The Worker uses
the upstream PMTiles reader. It caches tile responses at the Cloudflare edge.

## Build and deploy

Install Node 24 or later. Run from the repository root:

```sh
npm ci --prefix apps/planner-tiles
npm test --prefix apps/planner-tiles
npm run build --prefix apps/planner-tiles
```

For dashboard deployment, create the `obc-planner-tiles` Worker. Paste
`dist/worker.js` into its code editor. Add an R2 binding named `BUCKET` for
`obc-maps`. Add the custom domain `tiles.openbikecomputer.com`.
The Worker needs no bucket access key.
Start with Workers Free for private tests. Check cold tile requests for CPU
limit failures before a public launch. Upgrade if the fixed limit is too low.
Workers Free has a fixed `10` ms limit. The configuration does not request
a paid CPU allowance.

With a Cloudflare deployment credential, run:

```sh
npm run deploy --prefix apps/planner-tiles
```

| Path | Result |
| --- | --- |
| `/releases/ID/basemap.json` | Vector TileJSON |
| `/releases/ID/basemap/Z/X/Y.mvt` | Vector tile |
| `/releases/ID/places.json` | Rider places TileJSON |
| `/releases/ID/places/Z/X/Y.mvt` | Rider places tile |
| `/releases/ID/overlays.json` | Route network and access TileJSON |
| `/releases/ID/overlays/Z/X/Y.mvt` | Route network and access tile |
| `/releases/ID/terrain.json` | Terrain TileJSON |
| `/releases/ID/terrain/Z/X/Y.webp` | Terrarium tile |
| `/releases/ID/snow.json` | Snow TileJSON with the archive metadata, when the release has snow |
| `/releases/ID/snow/Z/X/Y` | Snow tile: gzip-encoded bytes, `application/octet-stream` |
| `/releases/ID/climate.json` | Climate TileJSON with the archive metadata, when the release has climate |
| `/releases/ID/climate/Z/X/Y` | Climate tile: gzip-encoded bytes, `application/octet-stream` |

The archive header gives the zoom levels and the tile type. A tile extension is
optional and must match the tile type. TileJSON tile URLs have no extension.

`ID` is the SHA-256 of `release.json`. Grid archives use the canonical object pool and its small public pointers.
The [release contract](../../specs/planner-release.md#canonical-grid-storage)
defines those paths. Queries and unknown paths
return 404. An absent tile returns 204. A grid tile without a pack is absent.
An absent archive returns 404.
Read failures return 503 with no cache. The domain root returns 404.

The service also serves release font, sprite, and device catalog paths.

Only `npm run deploy` applies `wrangler.toml`. It turns off `workers.dev` and preview URLs and
adds the `LIMITER` binding. The Worker answers `429` to a client address that exceeds the limit
on cache misses. Cache hits are never counted. A dashboard paste has no binding, and the Worker
then applies no limit. Counters are per data centre. Raise the limit when many riders share one
address. The rate limit rule, the cache rule and the budget alert are in the
[Cloudflare runbook](../../ops/cloudflare.md).

Offline payload downloads use the bucket's public domain. See the
[planner instructions](../../builder/app/src/components/planner/README.md).
