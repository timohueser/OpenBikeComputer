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

With a Cloudflare deployment credential, run:

```sh
npm run deploy --prefix apps/planner-tiles
```

| Path | Result |
| --- | --- |
| `/releases/ID/basemap.json` | Vector TileJSON |
| `/releases/ID/basemap/Z/X/Y.mvt` | Vector tile, zoom 0–14 |
| `/releases/ID/terrain.json` | Terrain TileJSON |
| `/releases/ID/terrain/Z/X/Y.webp` | Terrarium tile, zoom 0–12 |

`ID` is the SHA-256 of `release.json`. The archives live at
`planner/releases/ID/maps/{basemap,terrain}.pmtiles`. Queries and unknown paths
return 404. An absent tile returns 204. An absent archive returns 404.
Read failures return 503 with no cache. The domain root returns 404.

Raw archive downloads use the bucket's public domain. See the
[planner instructions](../../builder/app/src/components/planner/README.md).
