# Route engine

This crate supplies coordinate routing over immutable regional packages. It has
no dependency on OBC firmware, HTTP, a map renderer, or an itinerary model.
The default build needs Rust and `std`. The same core compiles for native iOS
and WebAssembly. Run browser queries in a worker.

## Use

```rust,no_run
use route_engine::{directory::Directory, Control, Request, Router};

let package = Directory::open(std::path::Path::new("/data/freiburg"))?;
let mut router = Router::new(package, 64 * 1024 * 1024);
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

Run a local query without the HTTP service:

```sh
cargo run --release -p route-engine --example query -- /data/freiburg < request.json
```

## API reference

| Module | Responsibility |
| --- | --- |
| `model` | Directed roads, prepared profiles, surface data, separate pace |
| `package` | Manifest, object identity, bounded geometry and endpoint caches |
| `snap` | Nearest accessible directed road attachments and partial costs |
| `search` | Resumable bidirectional CH query and shortcut reconstruction |
| `router` | Ordered points, direction continuity, geometry and totals |
| `directory` | Optional native file adapter |

The primary route minimizes the selected integer metric over retained snap
candidates. Exactness is within the installed graph. The snap window is 250 m;
retain roads within 3 m of the nearest distance, up to eight candidates. The
response reports truncation. A shape point shares its directed attachment
between both legs. Only an explicit `turnarounds` entry permits a reversal at
that point. Road junction turns still obey the prepared access and turn rules.

Alternatives use prepared goals and bounded corridor probes. They must pass a
base-cost cap and a material benefit or separation test. Discovery is not
exhaustive. Corridor probes currently apply to two-point requests. An empty
alternative set is valid. The primary route remains first.

Pace changes moving seconds, not costs or geometry. Keep learned personal pace
on the rider's device. The engine accepts a time multiplier but does not learn
from or store rides. Missing terrain stays unknown. Moving time uses a level
slope where terrain is missing. No result is an arrival clock time.

Keep day ends, manual lines, imported lines, versions, and undo outside this
crate. Preserve a chosen result instead of recalculating it when the itinerary
changes. Use `elapsed` for time at positions along the geometry.

## Bounds and checks

One router runs one query at a time. It retains up to 256 leg paths with at most
65,536 road slices. Geometry and endpoint caches each have a 32 MiB ceiling and
16-page limit. The caller sets the CH cache size. A decoded page is at most
8 MiB. Default query limits are 64 points, 250,000 labels, 8,192 attachment-pair
queries, and 250,000 geometry vertices. `Control` can lower these limits and
supplies a cancellation callback. These are service budgets, not device
performance claims.

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
