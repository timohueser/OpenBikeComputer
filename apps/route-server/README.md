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
Each worker has its own router with a 64 MiB CH cache. A request has a 15-second
cooperative deadline. Disconnects cancel its work. The body limit is 64 KiB.
Put a public service behind TLS and an OS memory and CPU limit. Keep the package
read-only. Replace it by starting a new service instance on the new directory.

For Linux, install `route-server.service` after placing the executable in
`/opt/obc-routing/bin` and the package in `/opt/obc-routing/region`. The unit
uses a dynamic user, a 1536 MiB memory limit and two CPU cores at most.

## HTTP API

- `GET /health` reports process health.
- `GET /v1/region` reports package identity, coverage, profiles and attribution.
- `POST /v1/route` calculates a route.

```sh
curl http://127.0.0.1:8788/v1/route \
  -H 'Content-Type: application/json' \
  --data '{"points":[[7.849,47.997],[8.154,47.902]],"profile":"touring","alternatives":true}'
```

Coordinates are longitude, latitude. Supply 2 to 64 ordered points. Optional
`turnarounds` lists interior point indices where a reversal is deliberate.
Optional `pace` contains `cycling_kmh`, `walking_kmh` and `personal_multiplier`.
Omitted pace uses 19, 4.5 and 1 respectively.

The response contains `routes`. The first is the primary route. Each result
has an ID, reason, package identity, profile, cost, coordinates, nullable
heights, cumulative moving seconds, totals, directed attachments and leg road
slices. `surfaces[i]` describes the edge from `geometry[i]` to `geometry[i + 1]`. Each leg has inclusive geometry indices. Surface totals use this order:
unknown, paved, compacted, gravel, dirt, rough. Distances and heights are metres.
Time is seconds. A cost is a prepared preference value, not time.

Errors contain `code` and `message`. Codes are `invalid_request` (400),
`no_snap`, `no_path`, `missing_region` (422), `cancelled` (408), `busy`, `limit`
(503), and `invalid_data` (500). An error never contains a substitute route.

## Planner

Start this service, then start Vite from `builder/app`. Its `/routing` proxy
connects to the default local listener. Set `VITE_PLANNER_ROUTING_URL` at build
time for another endpoint. Build the isolated preview with:

```sh
VITE_PLANNER_PMTILES_URL=/planner/data/basemap.pmtiles \
  npm run --prefix builder/app build:planner
```

Serve `builder/app/dist/planner` under `/planner/` and open
`/planner/planner.html`. Serve a Protomaps basemap at the configured URL.
Proxy `/routing/*` to this service with that prefix removed. The preview needs
no user account. Search examples and the map renderer remain separate from
routing. See the [planner README](../../builder/app/src/components/planner/README.md).

```sh
obc test -p route-server
cargo clippy -p route-server --all-targets -- -D warnings
```
