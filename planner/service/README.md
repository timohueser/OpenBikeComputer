# Route service

The HTTP service and the phone library around the [route engine](../router/README.md). The
[route API](../../specs/route-api.md) specifies requests, answers and errors.

## Build and run

```sh
cargo build --release -p planner-service
target/release/planner-service /data/routes/freiburg --verify
target/release/planner-service /data/routes/freiburg
target/release/planner-service --formats
```

`--formats` prints the format of the routing package that the service reads, such as
`{"planner/routing":8}`. `obc data serve` starts no release in another format, and starts one
service for each planner release that it serves. The planner calls the `routing` prefix of its
config; see the [planner README](../../builder/web/src/components/planner/README.md).

```sh
curl http://127.0.0.1:8788/v1/route \
  -H 'Content-Type: application/json' \
  --data '{"points":[[7.849,47.997],[8.154,47.902]],"profile":"touring","alternatives":true}'
```

## Phone library

The iOS Companion links this crate as a static library without the default `http` feature, so the
library contains no HTTP stack. [`tools/build-planner-ios.sh`](../../tools/build-planner-ios.sh)
builds it. `planner_router_call` answers `route` and `shape` with the same code as
`POST /v1/route` and `POST /v1/shape`. The host serializes the calls on a handle.
`planner_router_cancel` is safe from any thread: it stops the call in progress, which then answers
`cancelled`.

```sh
cargo build --release -p planner-service --lib --no-default-features --target aarch64-apple-ios
```

## Test

```sh
obc test -p planner-service
cargo clippy -p planner-service --all-targets -- -D warnings
cargo clippy -p planner-service --all-targets --no-default-features -- -D warnings
```

## Reference

| Variable | Default | Purpose |
| --- | --- | --- |
| `ROUTE_LISTEN` | `127.0.0.1:8788` | Listen address |
| `ROUTE_WORKERS` | `2` | Concurrent queries, from 1 to 8 |
| `ROUTE_ORIGIN` | Unset | One allowed browser origin; omit for same-origin proxy |

- `GET /health` reports process health.
- `GET /v1/region` reports package identity, coverage, profiles and attribution.
- `POST /v1/route` calculates a route.
- `POST /v1/shape` finds the plan points of a route that follows a line.

A request waits up to 1 second for a free worker, then receives `503 busy`. Workers share immutable
routing data and use the engine's default memory budget; see
[engine bounds](../router/README.md#bounds). A route request has a 15-second cooperative deadline,
a shape request 30 seconds. Disconnects cancel its work. The body limit is 64 KiB. The service
loads the `touring` profile before it listens. SIGTERM or SIGINT stops it after its open requests
finish. A panic or another internal failure answers `500 internal` and writes the cause to
standard error. A worker that panicked gets a new router.

## Pitfalls

- Put a public service behind TLS and an OS memory and CPU limit.
- Keep the package read-only. To replace it, start a new service instance on the new directory.
