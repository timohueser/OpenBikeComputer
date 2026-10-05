# Route engine

This crate supplies coordinate routing over immutable regional packages. It has
no dependency on OBC firmware, HTTP, a map renderer, or an itinerary model.
The default build needs Rust and `std`. The same core compiles for native iOS
and WebAssembly. Run browser queries in a worker.

## Use

```rust,no_run
use route_engine::{data::RoutingData, Control, Request, Router};

let package = route_engine::open(std::path::Path::new("/data/freiburg"))?;
let budget = package.default_budget();
let mut router = Router::new(package, budget);
let request: Request = serde_json::from_str(r#"{
  "points": [[7.849,47.997],[8.154,47.902]],
  "profile": "touring", "alternatives": true
}"#)?;
let result = router.routes(&request, &Control::default())?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

`open` reads a complete package or a grid selection (`blocks.json` with its
packs) into one `data::Selection`. Supply `package::Source` for application
storage. It returns a complete object by SHA-256. It must not change objects
during a package's lifetime. Use `Selection::verify` to check the full object
closure before installation. A missing object returns `MissingRegion`. No query
needs a network service.

Run a local query without the HTTP service. It writes the wire answer:

```sh
cargo run --release -p route-engine --example query -- /data/freiburg < request.json
```

## API reference

| Module | Responsibility |
| --- | --- |
| `model` | Directed roads, prepared profiles, surface data, separate pace |
| `package` | Manifest, object identity, bounded page caches |
| `data` | One selection of a package: snapping, prepared profiles, the memory estimate |
| `blocks` | Grid selections: compact road ids and the union graph |
| `snap` | Nearest accessible directed road attachments |
| `geometry` | Nearest points of segments, points along lines, road page columns |
| `cost` | Prepared road costs and partial-road prefixes |
| `base` | Shared road-state topology and exact profile cost columns |
| `search` | Exact bidirectional search with optional feasible potentials |
| `landmarks` | Compressed junction bounds and the potential of each request |
| `router` | Ordered points, direction continuity, route pieces and leg totals |
| `answer` | The wire answer of [the route API](../../specs/route-api.md) |
| `directory` | Native packed storage and `open` |

The [route API](../../specs/route-api.md) defines snapping, direction continuity,
alternatives, pace, and request limits. Snapping uses only roads permitted by the
prepared profile. A disconnected leg returns `NoPath`; it does not retry other roads.
Missing terrain stays unknown and uses level slope for moving time.

Alternative discovery is bounded, not exhaustive. All routes share one query budget.
Cancellation or a limit keeps the routes already found; no alternatives is a valid
answer. Corridor probes apply to two-point requests. Optional landmarks guide the
same search without changing its result.

Keep itinerary state and learned pace outside this crate. Preserve a chosen route when
the itinerary changes. Use `elapsed` for moving time along its geometry; it is not an
arrival clock time.

## Bounds and checks

One router runs one query at a time. The constructor takes an estimated routing
working-set budget in bytes. `default_budget` is the costliest profile's routing
bytes plus 256 MiB for the search queues, at least 768 MiB; native hosts use
it. The estimate counts the decoded graph, the junction
mapping, the label blocks and, for each prepared profile, its cost columns and
its landmark columns. It reserves 64 MiB for geometry, index caches and decode
scratch. An estimate above the budget returns `Limit`. This is not an allocator
or process RAM guarantee. Manifest memory, source mappings, results and host
serialization add memory. Search labels allocate visited blocks and retain a
small reuse pool. Prepared profiles share identical turn columns.
`Selection::fork` gives a worker its own page caches; forks share the graph, the
junction mapping, the closures and one cache of prepared profiles. That cache is
budgeted like one router: the shared data plus one router's label blocks and
16 MiB of queue headroom fit the largest router budget, so each further router
adds only its own blocks and queues; at most three profiles stay per router. An
evicted profile a router still holds counts, and comes back without a load. A
cold load never blocks the other routers' reads. `routing_bytes` is one router's
own blocks plus the shared data.

The router retains its last primary route and 32 snap results. A request that
differs from the last one only in its alternatives flags reuses that route and
spends no queries on it. The geometry cache has a 32 MiB ceiling. A decoded page is at
most 8 MiB. Default request limits are 64 points, 8,192 batched attachment
queries, and 250,000 geometry vertices. `Control` can lower these limits, cap
the search queues, and supply a cancellation callback. These are service budgets.

```sh
obc test -p route-engine
obc test -p route-build
cargo clippy -p route-engine --all-targets -- -D warnings
cargo check -p route-engine --target wasm32-unknown-unknown
cargo check -p route-engine --target aarch64-apple-ios
```

The builder tests compare prepared coordinate queries with independent
arrival-road Dijkstra. They also check turn continuity, partial endpoints,
package failures, terrain, and page boundaries. Physical phone performance
and complete-country preparation require separate validation.
