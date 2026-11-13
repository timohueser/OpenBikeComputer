# Route engine

Coordinate routing over immutable regional packages. No dependency on OBC firmware, HTTP, a map
renderer or an itinerary model. The default build needs Rust and `std`; the same core compiles for
native iOS and WebAssembly. Run browser queries in a worker. The [route API](../../specs/route-api.md)
defines snapping, direction continuity, alternatives, pace and request limits.

## Run

```rust,no_run
use planner_router::{data::RoutingData, Control, Request, Router};

let package = planner_router::open(std::path::Path::new("/data/freiburg"))?;
let budget = package.default_budget();
let mut router = Router::new(package, budget);
let request: Request = serde_json::from_str(r#"{
  "points": [[7.849,47.997],[8.154,47.902]],
  "profile": "touring", "alternatives": true
}"#)?;
let result = router.routes(&request, &Control::default())?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

`open` chooses the engine by the folder. A folder with `manifest.json` or `blocks.json` is format 9:
a complete package or a grid selection, read into one `data::Selection`. Each other folder is an
install of format 10 (`directory.bin`, `install.bin`), read into one `region::Region`. For
application storage, supply a `package::Source`: it returns a complete object by SHA-256 and must not change objects during a package's lifetime. `Selection::verify`
checks the full object closure before installation. A missing object returns `MissingRegion`. No
query needs a network service.

A local query without the HTTP service writes the wire answer:

```sh
cargo run --release -p planner-router --example query -- /data/freiburg < request.json
```

`compare` routes one request set on format 9 and on an install of format 10 of one bake, and
prints the times and the search work of each request. It exits with 1 on a difference:

```sh
cargo run --release -p planner-router --example compare -- OLD NEW 48 1 --max-km 40
```

## Test

```sh
obc test -p planner-router
obc test -p planner-router-build
cargo clippy -p planner-router --all-targets -- -D warnings
cargo check -p planner-router --target wasm32-unknown-unknown
cargo check -p planner-router --target aarch64-apple-ios
```

The builder tests compare prepared coordinate queries with independent arrival-road Dijkstra. They
also check turn continuity, partial endpoints, package failures, terrain and page boundaries.
Physical phone performance and complete-country preparation need separate validation.

## Reference

The old engine (format 9) is `data::Selection`, `package`, `blocks`, `search`, `queue`,
`landmarks`, `table`, `storage`, `directory` and the graph of `base`. The new engine (format 10) is
`cells`, `install`, `host`, `region`, `overlay` and `overlay/unpack`. `open`, `router`, `snap` and
the other modules serve both.

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
| `directory` | Native packed storage of format 9 |
| `open` | Native: `open` and `Opened`, the data of either format |
| `compare` | Native: the request set and the check that compare the two engines |
| `cells` | Format 10: read views of the cell directory and of the parts of a cell, the road record |
| `install` | Format 10, native only: installs a set of cells as files, checks the objects, reads the files again |
| `host` | Format 10: lends the bytes of the installed parts. `Maps` (native) maps each file at its first use; `Memory` (WebAssembly, tests) holds the parts |
| `region` | Format 10: the request data of an install through a host: snapping, roads, costs, turns, closures, and the cells with a cost table |
| `overlay` | Format 10: the search from both ends. Labels in a hash table by road ID, the reverse turns of an open cell built when the backward side enters it |
| `overlay/unpack` | Format 10: the roads of the chosen legs. Level by level, it announces what the steps read, then finds the path of each step in its cell |

A cell is closed for a search when it has a cost table and holds no level-1 cell of a seed road.
The search crosses the highest closed cell of a junction in one step with the table, and uses the
turns at each other junction.

- Snapping uses only roads that the prepared profile permits. A disconnected leg returns `NoPath`;
  it does not retry other roads.
- Missing terrain stays unknown and uses level slope for moving time.
- Alternative discovery is bounded, not exhaustive. All routes share one query budget.
  Cancellation, a limit or a lack of memory keeps the routes already found; no alternatives is a
  valid answer.
- Corridor probes apply to two-point requests. Optional landmarks guide the same search without
  changing its result.
- Keep itinerary state and learned pace outside this crate. Preserve a chosen route when the
  itinerary changes.
- `elapsed` is moving time along the route geometry, not an arrival clock time.

## Bounds

One router runs one query at a time. The constructor takes an estimated routing working-set budget
in bytes. `default_budget` is the costliest profile's routing bytes plus 256 MiB for the search
queues, at least 768 MiB; native hosts use it. The estimate counts the decoded graph, the junction
mapping, the label blocks and, for each prepared profile, its cost columns and its landmark
columns. It reserves 64 MiB for geometry, index caches and decode scratch. An estimate above the
budget returns `Limit` when the profile does not fit alone, else `Memory`. This is not an allocator or process RAM guarantee: manifest memory, source
mappings, results and host serialization add memory. Search labels allocate visited blocks and
keep a small reuse pool. Prepared profiles share identical turn columns.

On format 10 the budget is the search limit: the most bytes of the labels, the queues and the
reverse turns of one search, 64 MiB by default. Above it a search returns `Limit`.

`Selection::fork` gives a worker its own page caches. Forks share the graph, the junction mapping,
the closures and one cache of prepared profiles. That cache is budgeted like one router: the shared
data plus one router's label blocks and 16 MiB of queue headroom fit the largest router budget, so
each further router adds only its own blocks and queues. At most three profiles stay per router.
An evicted profile that a router still holds counts and comes back without a load. A cold load
never blocks other routers' reads. `routing_bytes` is one router's own blocks plus the
shared data. When a search queue lacks room or a profile does not fit, the router drops the cached
profiles that no router holds and tries once before it answers `Memory`.

The router retains 32 snap results. The geometry
cache has a 32 MiB ceiling. A decoded page is at most 8 MiB. Default request limits are 64 points,
8,192 batched attachment queries, and 250,000 each for the steps of one search path, the roads of
the chosen legs of a route and the points of an answer. `Control` can lower these limits, cap the
search queues and supply a cancellation callback. These are service budgets.
