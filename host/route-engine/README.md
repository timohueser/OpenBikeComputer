# Route engine

This crate supplies coordinate routing over immutable regional packages. It has
no dependency on OBC firmware, HTTP, a map renderer, or an itinerary model.
The default build needs Rust and `std`. The same core compiles for native iOS
and WebAssembly. Run browser queries in a worker.

## Use

```rust,no_run
use route_engine::{directory::Directory, Control, Request, Router};

let package = Directory::open(std::path::Path::new("/data/freiburg"))?;
let mut router = Router::new(package, 768 * 1024 * 1024);
let request: Request = serde_json::from_str(r#"{
  "points": [[7.849,47.997],[8.154,47.902]],
  "profile": "touring", "alternatives": true
}"#)?;
let result = router.routes(&request, &Control::default())?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Supply `package::Source` for application storage. It returns a complete object
by SHA-256. It must not change objects during a package's lifetime. Use
`Package::verify` to check the full object closure before installation. A
missing object returns `MissingRegion`. No query needs a network service.

Run a local query without the HTTP service. It writes the wire answer:

```sh
cargo run --release -p route-engine --example query -- /data/freiburg < request.json
```

## API reference

| Module | Responsibility |
| --- | --- |
| `model` | Directed roads, prepared profiles, surface data, separate pace |
| `package` | Manifest, object identity, bounded geometry and cost caches |
| `snap` | Nearest accessible directed road attachments |
| `cost` | Prepared road costs and partial-road prefixes |
| `osm` | Source tags and relation membership, outside query caches |
| `base` | Shared road-state topology and exact profile cost columns |
| `search` | Exact bidirectional search with optional feasible potentials |
| `landmarks` | Compressed junction bounds and selection for each request |
| `router` | Ordered points, direction continuity, geometry and totals |
| `answer` | The wire answer of [the route API](../../specs/route-api.md) |
| `directory` | Optional native file adapter |

The primary route minimizes the selected metric over retained attachments.
The snap radius is 250 m. A point with no road that near uses the nearest
road within 1 km (`snap::REACH_M`). The first attempt retains up to eight roads
within 3 m of the nearest distance. If a leg cannot connect, a retry uses 16
candidates, minimizes total snap distance first, then route cost. Most points
get a 50 m band. The two points of the leg that failed get a band to 1 km, and
each further failed leg widens its two points in one more retry. Points already
within 3 m of a road keep the narrow band. The route answer reports truncation
only, not the retry. All attempts share the query budgets. A shape point keeps its direction between
legs. Only an explicit `turnarounds` entry permits reversal there. Prepared
access and turn rules apply.

Alternatives use prepared goals and bounded corridor probes. They must pass a
base-cost cap and a material benefit or separation test. Discovery is not
exhaustive. Corridor probes currently apply to two-point requests. An empty
alternative set is valid. The primary route remains first, except with
`alternatives_only`, which leaves it out.

Optional landmark columns guide long searches. Small searches finish before
loading these columns. The same search applies with or without this index.

Pace changes moving seconds, not costs or geometry. Keep learned personal pace
on the rider's device. The engine accepts a time multiplier but does not learn
from or store rides. Missing terrain stays unknown. Moving time uses a level
slope where terrain is missing. No result is an arrival clock time.

Keep day ends, manual lines, imported lines, versions, and undo outside this
crate. Preserve a chosen result instead of recalculating it when the itinerary
changes. Use `elapsed` for time at positions along the geometry.

## Bounds and checks

One router runs one query at a time. The constructor takes an estimated routing
working-set budget in bytes. Native hosts use 768 MiB plus the decoded landmark cache by default. The engine
checks decoded graph, active profile, label blocks and queue allocations. It
reserves 64 MiB for geometry, index caches and decode scratch. An estimate above
the budget returns `Limit`. This is not an allocator or process RAM guarantee.
Manifest memory, source mappings, results and host serialization add memory.
Search labels allocate visited blocks and retain a small reuse pool.
Cached profiles share identical turn columns.
Up to three profile cost sets stay cached within the budget. Use
`Package::fork` to share topology across workers with independent query caches.

The router retains up to 256 leg choices with at most 65,536 road slices and
32 snap results. The geometry cache has a 32 MiB ceiling. A decoded page is at
most 8 MiB. Default request limits are 64 points, 8,192 batched attachment
queries, and 250,000 geometry vertices. `Control` can lower these limits, add a
label limit, and supply a cancellation callback. These are service budgets.

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
