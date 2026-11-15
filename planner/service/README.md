# Route service

The HTTP service and the phone library around the [route engine](../router/README.md). The
[route API](../../specs/route-api.md) specifies requests, answers and errors.

## Build and run

```sh
cargo build --release -p planner-service
target/release/planner-service /data/route-cells/freiburg --scrub
target/release/planner-service /data/route-cells/freiburg
target/release/planner-service --formats
```

The argument is an install of the [route package](../../specs/route-package.md), which
`--install` writes; the service fails at its start when the install does not open. `--scrub` reads
each installed file again and names those whose SHA-256 differs. `--formats` prints the format of
the routing package that the service reads: `{"planner/routing":10}`. `obc data serve` starts no
release in another format of `planner/routing`, and starts one service for each planner release
that it serves, on `<planner folder>/route-cells/`. The planner calls the `routing` prefix of its
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
`cancelled`. The budget of `planner_router_open` is the search limit of one search; 0 takes the
engine's default of 64 MiB.

`planner_routing_plan` and `planner_routing_install` take a JSON request: the paths `package`,
`directory` and `target`, the `areas` of all maps of one routing package as `[west, south, east,
north]`, and for the install the folder `objects`. The plan answers the objects that `target`
lacks and the installed bytes. The install calls its progress callback from its workers, one call
at a time; a callback that returns false stops it with `cancelled`, and a later call goes on. An
install with no area deletes `target`.

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
| `ROUTE_WORKERS` | CPUs of the process | Concurrent requests, at least 1; heavy requests use at most half |
| `ROUTE_ORIGIN` | Unset | One allowed browser origin; omit for same-origin proxy |

- `GET /health` reports process health.
- `GET /v1/region` reports package identity, coverage, profiles and attribution.
- `POST /v1/route` calculates a route.
- `POST /v1/shape` finds the plan points of a route that follows a line.

A route request with `alternatives` or `alternatives_only`, and a shape request, can use at most
`max(1, workers / 2)` workers. With 1 worker, a light request waits behind a heavy request. A
request waits up to 1 second for its worker, then receives `503 busy`. A request that exceeds a
fixed limit receives `422 limit`; a worker with no memory for it at that moment answers `503 busy`. Workers share the maps of
the installed files and use the engine's default search limit; see
[engine bounds](../router/README.md#bounds). A route request has a 15-second cooperative deadline,
a shape request 30 seconds. Disconnects cancel its work. The body limit is 64 KiB. SIGTERM or SIGINT stops it after its open requests
finish. A panic or another internal failure answers `500 internal` and writes the cause to
standard error. A worker that panicked gets a new router.

## Pitfalls

- Put a public service behind TLS and an OS memory and CPU limit.
- The default worker count is the number of CPUs of the process, which includes a systemd
  `CPUQuota`.
- Keep the install read-only while the service runs. To replace it, start a new service instance on
  the new install.
