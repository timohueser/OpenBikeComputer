# Route service

```sh
cargo build --release -p route-server
target/release/route-server /data/routes/freiburg --verify
target/release/route-server /data/routes/freiburg --build-overlays
target/release/route-server /data/routes/freiburg
```

| Variable | Default | Purpose |
| --- | --- | --- |
| `ROUTE_LISTEN` | `127.0.0.1:8788` | Listen address |
| `ROUTE_WORKERS` | `2` | Concurrent queries, from 1 to 8 |
| `ROUTE_ORIGIN` | Unset | One allowed browser origin; omit for same-origin proxy |

The service does not queue requests. Extra requests receive `503 busy`.
Each worker has its own router with a 768 MiB routing budget plus its landmark
cache. Workers share one immutable graph and road-to-junction mapping. Each
retains up to three profile cost sets within its budget. A request has a 15-second
cooperative deadline. Disconnects cancel its work. The body limit is 64 KiB.
Put a public service behind TLS and an OS memory and CPU limit. Keep the package
read-only. Replace it by starting a new service instance on the new directory.

For Linux, install `route-server.service` after placing the executable in
`/opt/obc-routing/bin` and the package in `/opt/obc-routing/region`. The unit
uses a dynamic user, a 2048 MiB memory limit and two CPU cores at most.

## HTTP API

- `GET /health` reports process health.
- `GET /v1/region` reports package identity, coverage, profiles and attribution.
- `POST /v1/route` calculates a route.
- `GET /v1/overlays?bbox=7.9,48,8.1,48.1&zoom=12&layers=cycling,hiking,access`
  returns GeoJSON from the package's OSM snapshot. Select one or more layers.

Overlay requests accept zoom 6 to 22 and bounds up to 30 degrees wide and high.
The response includes regional coverage bounds and the routing package ID.
Discard cached features when that ID changes. Local routes appear from zoom 10,
regional routes from 8, and national or international routes from 6.
Construction and conditional access appear from 10; other access restrictions
appear from 13, directional rules from 14, and pushing sections from 15.
Access uses `mode=cycling` by default; `mode=walking` selects pedestrian rules.
Responses distinguish construction, no access, private, limited, conditional,
directional, pushing and bicycle bans. Dense requests fail with a zoom-in message.
Prepare `overlays.sqlite` before service startup. A runtime-only package must
receive its compiled overlay index from the preparation host. Its package identity must match
the routing manifest. The service reads the viewport through a disk spatial index
with a 4 MiB cache. Two overlay requests can run independently of routing workers.
Geometry is simplified within half a map pixel at the requested zoom. Feature
`routes` contains IDs from the response's `routes` dictionary. Successful responses
permit caching for one hour.
Route relations retain names, references, websites, trail symbols, network levels and overlapping memberships.
Proposed routes are omitted. Access markings are snapshot data, not live closures.

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
slices. `surfaces[i]` and `pushing[i]` describe the edge from `geometry[i]` to
`geometry[i + 1]`. Pushing is true where the bicycle must be pushed. Each leg has inclusive geometry indices. Surface totals use this order:
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
