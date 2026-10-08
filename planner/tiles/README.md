# Planner tiles

Serve the map packs of grid planner releases from the `obc-maps` R2 bucket. The
Worker uses the upstream PMTiles reader. It caches tile responses at the
Cloudflare edge.

## Build and deploy

Install Node 24 or later. Run from the repository root:

```sh
npm ci --prefix planner/tiles
npm test --prefix planner/tiles
npm run build --prefix planner/tiles
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
npm run deploy --prefix planner/tiles
```

| Path | Result |
| --- | --- |
| `/releases/ID/basemap.json` | Vector TileJSON from the release's `maps/basemap.json` |
| `/releases/ID/basemap/Z/X/Y.mvt` | Vector tile |
| `/releases/ID/places.json` | Rider places TileJSON |
| `/releases/ID/places/Z/X/Y.mvt` | Rider places tile |
| `/releases/ID/overlays.json` | Route network and access TileJSON |
| `/releases/ID/overlays/Z/X/Y.mvt` | Route network and access tile |
| `/releases/ID/terrain.json` | Terrain TileJSON |
| `/releases/ID/terrain/Z/X/Y.webp` | Terrarium tile |
| `/releases/ID/LAYER.json` | TileJSON of a data layer of the release, such as `snow` |
| `/releases/ID/LAYER/Z/X/Y` | Data layer tile; a tile that is not MVT or WebP keeps its gzip encoding, `application/octet-stream` |
| `/releases/ID/routes/tiles/9-X-Y.json` | Route catalog cell of a grid release; 404 for a cell outside the grid |

The pack header gives the zoom levels and the tile type. A tile extension is
optional and must match the tile type. TileJSON tile URLs have no extension.

`ID` is the SHA-256 of `release.json`. Packs come from the canonical object pool
through its small public pointers. The
[release contract](../../specs/planner-release.md#canonical-grid-storage)
defines those paths. Queries and unknown paths
return 404. An absent tile returns 204. A tile without a pack is absent.
A release without `public/grid.json`, or an archive without a TileJSON pointer, returns 404.
Read failures return 503 with no cache. The domain root returns 404.

The service also serves release font, sprite, and device catalog paths.

Only `npm run deploy` applies `wrangler.toml`. It turns off `workers.dev` and preview URLs and
adds the `LIMITER` binding. The Worker answers `429` to a client address that exceeds the limit
on cache misses. Cache hits are never counted. A dashboard paste has no binding, and the Worker
then applies no limit. Counters are per data centre. Raise the limit when many riders share one
address. Configure the zone rules below for the public domains.

Offline payload downloads use the bucket's public domain. See the
[planner instructions](../../builder/web/src/components/planner/README.md).

## Cloudflare dashboard settings

These zone settings are not in `wrangler.toml`. Check them after an account change.

### What costs money

The tile Worker is the only part with a real per-request price. It serves tiles, fonts, sprites and
the device catalog. Cloudflare bills each Worker request, also when the edge cache answers it. A
request that misses the cache adds R2 reads. Egress from R2 is free. A request to `maps.` or
`updates.` costs one R2 read and no Worker request.
The VPS has a fixed price.

### Budget alert

Create the alert in Billing > Billable Usage. An alert only sends an email. It never stops usage.
Create two: one near the expected monthly spend and one at five times that figure.

### Rate limit rule

Create the rule in Security > WAF > Rate limiting rules. The free plan allows one rule, counted per
IP address over 10 seconds, with a 10 second block.

| Field | Value |
| --- | --- |
| Expression | `http.host in {"tiles.openbikecomputer.com" "maps.openbikecomputer.com"}` |
| Characteristic | IP address |
| Requests per 10 seconds | 3000 |
| Action | Block for 10 seconds |

The limit allows planner tile bursts. A blocked request never starts the Worker,
so Cloudflare does not bill it.

### Cache Rule for `maps.` and `updates.`

Cloudflare caches by file extension. It does not cache `.json`, `.pbf` or `.sqlite`, or any
extension that is not in its list, unless a Cache Rule allows it. Without the rule, every request
reads R2.

Create the rule in Caching > Cache Rules:

| Field | Value |
| --- | --- |
| Expression | `http.host in {"maps.openbikecomputer.com" "updates.openbikecomputer.com"}` |
| Cache eligibility | Eligible for cache |
| Edge TTL | Use cache-control header if present, bypass cache if not |

Uploads set `Cache-Control`. An object uploaded before it did stays uncached until it is uploaded
again.

### Check

Run each command twice. The second answer must read `HIT`. `DYNAMIC` means the object is not cached.

```sh
curl -sI https://maps.openbikecomputer.com/planner/catalog.json | grep -i cf-cache-status
curl -sI https://tiles.openbikecomputer.com/releases/ID/basemap/9/268/176.mvt | grep -i cf-cache-status
```

Open Workers & Pages > `obc-planner-tiles` > Settings > Domains & Routes. The only entry must be
`tiles.openbikecomputer.com`. A `workers.dev` entry skips the zone rules above.
