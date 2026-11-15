# Route engine

Coordinate routing over installs of the regional [route package](../../specs/route-package.md). No dependency on OBC firmware, HTTP, a map
renderer or an itinerary model. The default build needs Rust and `std`; the same core compiles for
native iOS and WebAssembly. Run browser queries in a worker. The [route API](../../specs/route-api.md)
defines snapping, direction continuity, alternatives, pace and request limits.

## Run

```rust,no_run
use planner_router::{Control, Request, Router};

let region = planner_router::open(std::path::Path::new("/data/freiburg"), false)?;
let mut router = Router::new(region);
let request: Request = serde_json::from_str(r#"{
  "points": [[7.849,47.997],[8.154,47.902]],
  "profile": "touring", "alternatives": true
}"#)?;
let result = router.routes(&request, &Control::default())?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

`open` reads an install (`directory.bin`, `package.json`, `install.bin`, `cells/`) into one
`region::Region` on a `host::Maps`. Another host supplies the parts through `host::Host`. The
installer checks each object. A road of a cell that the install lacks returns `MissingRegion`. No
query needs a network service.

A local query without the HTTP service writes the wire answer:

```sh
cargo run --release -p planner-router --example query -- /data/freiburg < request.json
```

`compare` routes one request set on two installs, with `--layouts` of two layouts of one bake, and
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

The builder tests compare coordinate queries on a package with closed cells with independent
arrival-road Dijkstra. They also check turn continuity, partial endpoints, package failures and
terrain.
Physical phone performance and complete-country preparation need separate validation.

## Reference

| Module | Responsibility |
| --- | --- |
| `model` | Directed roads, prepared profiles, surface data, separate pace |
| `snap` | Nearest accessible directed road attachments |
| `geometry` | Nearest points of segments, points along lines |
| `cost` | Prepared road costs and partial-road prefixes |
| `closures` | The kinds of possible closures of a road |
| `router` | Ordered points, direction continuity, route pieces and leg totals |
| `answer` | The wire answer of [the route API](../../specs/route-api.md) |
| `compare` | Native: the request set and the check that compare two installs, and the table for the download level |
| `cells` | Read views of the cell directory and of the parts of a cell, the road record |
| `install` | Native: installs a set of cells as files, checks the objects, reads the files again |
| `host` | Lends the bytes of the installed parts. `Maps` (native) maps each file at its first use; `Memory` (WebAssembly, tests) holds the parts |
| `region` | The request data of an install through a host: snapping, roads, costs, turns, closures, and the cells with a cost table |
| `overlay` | The search from both ends. Labels in a hash table by road ID, the reverse turns of an open cell built when the backward side enters it |
| `overlay/unpack` | The roads of the chosen legs. Level by level, it announces what the steps read, then finds the path of each step in its cell |

A cell is closed for a search when it has a cost table and holds no level-1 cell of a seed road.
The search crosses the highest closed cell of a junction in one step with the table, and uses the
turns at each other junction.

- Snapping uses only roads that the prepared profile permits. A disconnected leg returns `NoPath`;
  it does not retry other roads.
- Missing terrain stays unknown and uses level slope for moving time.
- Alternative discovery is bounded, not exhaustive. All routes share one query budget.
  Cancellation, a limit or a lack of memory keeps the routes already found; no alternatives is a
  valid answer.
- Corridor probes apply to two-point requests.
- Keep itinerary state and learned pace outside this crate. Preserve a chosen route when the
  itinerary changes.
- `elapsed` is moving time along the route geometry, not an arrival clock time.

## Bounds

One router runs one query at a time. The search limit of a region is the most bytes of the labels,
the queues and the reverse turns of one search: 64 MiB after `open`, and `set_search_limit`
changes it. Above it a search returns `Limit`. A failed allocation returns `Memory`. This is not a
process RAM guarantee: the maps of the installed files, results and host serialization add memory.
`Region::fork` gives a worker its own search space; forks share the directory and the host.

The router retains 32 snap results. Default request limits are 64 points,
8,192 batched attachment queries, and 250,000 each for the steps of one search path, the roads of
the chosen legs of a route and the points of an answer. `Control` can lower these limits, cap the
search queues and supply a cancellation callback. These are service budgets.
