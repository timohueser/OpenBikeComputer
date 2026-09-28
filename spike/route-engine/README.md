# Trip router experiments

Standalone Rust experiments for app, browser and server routing. These files do not depend on the device router or its map format. Do not merge this spike into the product workspace.

## Run

Run these commands from the repository root with a stable Rust toolchain. The import command reads local OSM PBF files. Optional elevation inputs are local Copernicus Float32 geographic DSM tiles.

```sh
cargo build --manifest-path spike/route-engine/Cargo.toml --release --features builder
spike/route-engine/target/release/trip-router import GRAPH west south east north DEM_DIRECTORY_OR_- INPUT.osm.pbf
spike/route-engine/target/release/trip-router benchmark GRAPH OUTPUT_DIRECTORY touring start_lat start_lon end_lat end_lon 1024
```

Import accepts multiple PBF paths. Use extracts from the same snapshot. Overlapping objects use the first input copy. The benchmark accepts `touring`, `gravel`, `road`, and `hiking`. Add `--ch-only` to skip the partition experiment. Use a fresh output directory for each graph and profile. Preparation runs before each benchmark.

To run the browser query harness:

```sh
cd spike/route-engine
wasm-pack build --target web --out-dir web/pkg --features web
python3 experiments/serve.py --directory . --port 8767 --delay-ms 0
```

Open `/web/?base=/data/OUTPUT/ch/&start=RANK&startroad=ID&end=RANK&endroad=ID` on that local server. Use the four seed values in the benchmark JSON. This harness measures summary fetching and unpacking. It excludes endpoint selection and geometry fetching. The page reports WASM linear memory, not total browser memory. To test request overhead, increase `--delay-ms`. This adds server delay to each GET over HTTP/1; it does not simulate WAN bandwidth or a complete network path.

The command writes JSON to stdout and progress to stderr. Generated graphs and pages belong under the ignored `data/` directory. Retain the source data attribution and ODbL terms when distributing OSM-derived graphs.

## Server benchmark

Run these commands from this directory. Use a completed touring benchmark and its JSON output. The fixture command checks subpaths of its route and writes their required geometry pages.

```sh
cargo run --release --features builder --example server_fixtures -- GRAPH OUTPUT_DIRECTORY BENCHMARK.json data/requests.json
cargo run --release --features server --bin trip-router-server -- OUTPUT_DIRECTORY touring 8790 4 512 128 10000
```

In another terminal:

```sh
python3 experiments/server_load.py http://127.0.0.1:8790/route data/requests.json --requests 128 --concurrency 4 --output data/load.json
```

The server arguments after the profile are port, worker count, graph cache MiB, geometry cache MiB and maximum query duration in milliseconds. It binds to localhost. Each process serves one prepared graph and profile. Decoded pages are shared; route answers are not cached. Cache exhaustion returns an error. Restart the process to clear its caches.

`POST /route` accepts `start` and `end` seeds with `node`, `road` and zero `cost`. These are prepared graph-state endpoints, not coordinates. Optional `pace` changes ETA only. The response contains traversed road IDs, geometry, totals and cache metrics. The server supports gzip responses. It does not generate alternatives. The HTTP adapter is for local experiments, not public deployment.

The load driver warms each fixture once by default. Use `--warmup 0` for an empty application-cache run. It reports warmup separately and checks repeated answers and expected costs. `--health-url URL` stops new requests if the monitored service fails. Keep results under `data/`. These fixtures cover subpaths of one corridor; they do not represent a public traffic distribution.

## Endpoint, alternative and representation experiments

Run these commands from this directory with a prepared touring corpus. `REQUESTS.json` is the output of `server_fixtures`.

```sh
cargo run --release --features builder --example snap_experiment -- GRAPH OUTPUT_DIRECTORY REQUESTS.json data/snap.json
cargo run --release --features builder --example route_alternatives -- GRAPH
cargo run --release --features builder --example compact_graph -- GRAPH touring start_lat start_lon end_lat end_lon 20
```

The alternatives example defaults to two Luxembourg pairs. Supply four latitude/longitude values after `GRAPH` for another pair. It uses resident reference searches, not accelerated alternatives. The compact example compares selective turn states with a full arrival-state oracle and prepares native CH. Its endpoints are free junctions, so its costs differ from the main benchmark's fixed incoming-road endpoints. It does not export the paged format.

When filtering PBF input with `osmium tags-filter`, retain `w/highway`, `w/route=ferry`, `r/type=restriction`, `r/type=restriction:bicycle` and `r/type=restriction:foot`, plus their referenced objects. Use one consistent snapshot and a complete region. The importer still excludes unsupported semantics.

## Check

```sh
cargo test --manifest-path spike/route-engine/Cargo.toml --all-features
cargo clippy --manifest-path spike/route-engine/Cargo.toml --all-features --all-targets -- -D warnings
cargo check --manifest-path spike/route-engine/Cargo.toml --features web --target wasm32-unknown-unknown --lib
cargo check --manifest-path spike/route-engine/Cargo.toml --target aarch64-apple-ios --lib
cargo fmt --manifest-path spike/route-engine/Cargo.toml --check
cargo fmt --all --check
obc suites check
python3 -m unittest discover -s spike/route-engine/experiments
python3 spike/route-engine/experiments/alternatives.py
```

## Interfaces and limits

| Module | Interface |
| --- | --- |
| `model` | Directed roads, prepared profile costs, separate pace estimates |
| `osm`, `elevation` | Host import; never included in the default runtime |
| `build` | Host CH preparation and exact reference search |
| `search`, `storage` | Resumable CH query; caller supplies compressed pages |
| `server` | Fixed-profile host with shared graph and geometry caches |
| `snap` | Directed polyline candidates, partial-road costs and shaping continuity |
| `compact` | Host-only selective turn-state representation and native CH experiment |
| `partition` | Exact one-level partition baseline on a resident weighted graph |
| `experiments/alternatives.py` | Candidate selection and regret experiments |

Profile weights and pace curves are demonstration values. The runtime search accepts prepared costs, not live preference changes. Use `state_graph_with_costs` to supply costs from additional data during preparation. New cost fields need a new prepared metric. Each request must use pages from one graph and metric; the experimental page format does not authenticate that identity. The server also requires its `PROFILE` argument to match the prepared costs. Its manifest does not enforce that match.

The importer excludes unsupported conditional and via-way restrictions conservatively. It does not implement all regional access laws. Read the graph warnings before interpreting any route. Elevation samples come from source geometry vertices, are unsmoothed, and have no bridge or tunnel correction. Missing elevation is explicit.

The core and HTTP benchmark use fixed incoming-road states. The separate `snap` experiment attaches coordinates with partial-road costs and preserves direction through shaping points. It scans a resident source graph to build local index windows and incidence tables; it is not a packaged endpoint index. Exactness is conditional on its retained candidate set. A wider snap band or truncated candidate set can change the answer. The reference search checks optimality only inside the supplied graph and cost model. Profile quality is not validated.

The CH adapter uses ordinary metric-dependent contraction. The partition baseline uses one level and coordinate cuts. It is not a full CRP implementation. Payload counters include selected compressed graph pages, endpoint records, and road geometry. They exclude map rendering, HTTP overhead, and alternatives. CH timings include local file reads and decoding. Partition timings use resident data. These are not browser, network, or iPhone benchmarks.

The Python experiment selects from a finite synthetic candidate set. The Rust alternatives example generates real paths from fixed objectives and bounded via probes. Neither proves a global Pareto or regret bound.
