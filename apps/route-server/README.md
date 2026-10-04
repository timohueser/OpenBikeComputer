# Route service

```sh
cargo build --release -p route-server
target/release/route-server /data/routes/freiburg --verify
target/release/route-server /data/routes/freiburg
```

| Variable | Default | Purpose |
| --- | --- | --- |
| `ROUTE_LISTEN` | `127.0.0.1:8788` | Listen address |
| `ROUTE_WORKERS` | `2` | Concurrent queries, from 1 to 8 |
| `ROUTE_ORIGIN` | Unset | One allowed browser origin; omit for same-origin proxy |

The service does not queue requests. Extra requests receive `503 busy`.
Each worker has its own router. The budget is the engine's default for the
package: the costliest profile's routing bytes plus 256 MiB for the search
queues, at least 768 MiB. The native host uses the same rule when it is given
no budget. Workers share one immutable graph, the road-to-junction mapping, the
closures and one cache of prepared profiles (cost and landmark columns),
budgeted like one router: each further worker adds only its own label blocks and
queues. A route request has a
15-second cooperative deadline, a shape request 30 seconds. Disconnects cancel
its work. The body limit is 64 KiB.
Put a public service behind TLS and an OS memory and CPU limit. Keep the package
read-only. Replace it by starting a new service instance on the new directory.

For Linux, install `route-server.service` after placing the executable in
`/opt/obc-routing/bin` and the package in `/opt/obc-routing/region`. The unit
uses a dynamic user, a 2048 MiB memory limit and two CPU cores at most.

## HTTP API

- `GET /health` reports process health.
- `GET /v1/region` reports package identity, coverage, profiles and attribution.
- `POST /v1/route` calculates a route. The [route API](../../specs/route-api.md)
  specifies the request, the answer and the errors.
- `POST /v1/shape` finds the plan points of a route that follows a line. The
  route API specifies it too.

```sh
curl http://127.0.0.1:8788/v1/route \
  -H 'Content-Type: application/json' \
  --data '{"points":[[7.849,47.997],[8.154,47.902]],"profile":"touring","alternatives":true}'
```

## Planner

Start this service, then start Vite from `builder/app`. Its `/routing` proxy
connects to the default local listener. Set `VITE_PLANNER_ROUTING_URL` at build
time for another endpoint. Build the isolated preview with:

```sh
VITE_PLANNER_PMTILES_URL=/planner/data/basemap.pmtiles \
  VITE_PLANNER_PLACES_URL=/planner/data/places.pmtiles \
  VITE_PLANNER_OVERLAYS_URL=/planner/data/overlays.pmtiles \
  npm run --prefix builder/app build:planner
```

Serve `builder/app/dist/planner` under `/planner/` and open
`/planner/planner.html`. Serve a Protomaps basemap, its places archive and the overlay archive at the configured URLs.
Proxy `/routing/*` to this service with that prefix removed. The preview needs
no user account. Search examples and the map renderer remain separate from
routing. See the [planner README](../../builder/app/src/components/planner/README.md).

```sh
obc test -p route-server
cargo clippy -p route-server --all-targets -- -D warnings
```
