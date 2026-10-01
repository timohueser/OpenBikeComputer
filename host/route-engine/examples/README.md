# Routing measurements

Run these commands from the repository root. Use immutable packages and a
Release build. Keep reports in the pull request. Extend the sample corpus for the workload under test.

```sh
python3 tools/planner_bench.py corpus PACKAGE host/route-engine/examples/requests.json > /tmp/requests.json
cargo run --release -p route-engine --example benchmark -- PACKAGE /tmp/requests.json > /tmp/routes.json
python3 tools/planner_bench.py summary /tmp/routes.json
python3 tools/planner_bench.py server http://127.0.0.1:8788 /tmp/requests.json --concurrency 2
python3 tools/planner_bench.py audit RELEASE_DIRECTORY
```

The native runner records complete route time, object reads, costs, geometry
and metadata fingerprints, and failures. It runs each case with a fresh router
and then with the same router. The operating system file cache stays uncontrolled.
The optional final arguments set iterations and routing memory budget MiB. Timing excludes
JSON serialization. Append `retained` to keep one router and change the first
coordinate on each iteration. This mode measures fresh requests with retained
caches. Set `ROUTE_BENCH_INDEX_MEMORY=1` to add the decoded landmark cache to the
base memory allowance. Reports include the resulting budget. HTTP timing
includes serialization and response transfer.
Compare equal request corpora, profile lists, and input identities.

The file audit verifies all runtime hashes and counts complete gzip output.
Its transfer figure is a codec measurement, not a bundle format. It excludes
source mirrors and includes every runtime index and manifest.

## Physical iPhone

Use the [native planner host](../../../apps/planner-native/README.md) for a
persistent session with the complete web UI, local providers, and restart checks.
The commands below measure individual components.

Install XcodeGen and the Rust iOS target. Set `TEAM` and `DEVICE` to the signing
team and paired device. The benchmark app uses its own data container. Restore
the [search environment](../../../apps/planner-search/README.md) and pass its
interpreter and model directory to the preparation script. It verifies the
pinned Python and ONNX Runtime archives, builds the tokenizer, and copies the
shared decoder and installer. Keep `target/planner-parser` until the app build ends.

```sh
IPHONEOS_DEPLOYMENT_TARGET=17.0 cargo build --release -p route-engine -p route-server --example phone_benchmark --example phone_overlay_benchmark --target aarch64-apple-ios
python3 host/route-engine/examples/phone/prepare-parser.py --python apps/planner-search/.venv/bin/python --model MODEL_DIRECTORY
xcodegen generate --spec host/route-engine/examples/phone/project.yml
xcodebuild -quiet -project host/route-engine/examples/phone/PlannerBenchmark.xcodeproj -scheme PlannerBenchmark -configuration Release -destination 'generic/platform=iOS' -derivedDataPath target/planner-phone/build DEVELOPMENT_TEAM="$TEAM" -allowProvisioningUpdates build
xcrun devicectl device install app --device "$DEVICE" target/planner-phone/build/Build/Products/Release-iphoneos/PlannerBenchmark.app
xcrun devicectl device copy to --device "$DEVICE" --domain-type appDataContainer --domain-identifier com.openbikecomputer.PlannerBenchmark --source PACKAGE --destination Documents/routing
xcrun devicectl device copy to --device "$DEVICE" --domain-type appDataContainer --domain-identifier com.openbikecomputer.PlannerBenchmark --source /tmp/requests.json --destination Documents/requests.json
xcrun devicectl device process launch --device "$DEVICE" com.openbikecomputer.PlannerBenchmark
```

After the app reports completion, copy `Documents/result.json` and
`Documents/device.json` with `devicectl device copy from`. The second report
records the physical device, OS, thermal state, process peak RSS, and physical
memory samples at 20 ms intervals. Sampling can miss short peaks. Record memory
pressure and power conditions separately. Routing results do not include map,
search, model initialization, or installation costs.

Launch each workload in a new process with `--terminate-existing`. Full routes
use the default workload. Pass `--package RELATIVE_PATH` to use a routing
directory below Documents, including an active installed release. Copy each
report before the next launch. Use `--retained --index-memory --hold` for the
same fresh-request workload with the host memory allowance and an awake screen.

For the parser, copy `MODEL_DIRECTORY` to `Documents/parser/model` and
`target/planner-parser/parser-reference.json` to `Documents/parser/parser-reference.json`.
Append `--parser` to the launch command. The report separates cold initialization
from two passes over the held-out requests. It compares every decoded request
with the host and verifies all model hashes before initialization.

For lexical search and reverse lookup, install the locked search dependencies
and build the shared JavaScript bundle and reference:

```sh
npm exec --prefix apps/planner-search -- esbuild apps/planner-search/phone-benchmark.mjs --bundle --format=iife --global-name=PlannerSearchBenchmark --target=safari17 --outfile=/tmp/search-benchmark.js
node apps/planner-search/phone-reference.mjs SEARCH.sqlite /tmp/search-reference.json
```

Copy the database to `Documents/search/baden-wuerttemberg.sqlite`. Copy both
generated files to `Documents/search`, then launch with `--search`. IDs, strings,
and result order must match. The report retains exact fingerprint differences
and allows small floating-point differences between JavaScript engines.

For opening hours, bundle `phone-hours.mjs` into `/tmp/hours-benchmark.js` with
the same esbuild options. Pass `/tmp/hours-reference.json --hours` to the reference
command, copy both files to `Documents/search`, and launch with `--hours`.
Also build `node apps/planner-search/calendar-bundle.mjs /tmp/calendar.js` and copy
that file to `Documents/search`. The shared evaluator checks holidays, school
holidays, solar times, and daylight saving transitions. It evaluates Berlin hours
in an isolated JavaScript realm with a New York host calendar. Device settings
stay unchanged. The Temporal adapter and the evaluator use separate Date bindings.

For complete smart search, build the native JavaScript resource and the HTTP
server reference. The model, database, and calendar files above must be present.

```sh
node apps/planner-search/native-build.mjs /tmp/search-native
node apps/planner-search/smart-reference.mjs SEARCH.sqlite MODEL_DIRECTORY apps/planner-search/.venv/bin/python /tmp/search-native/smart-reference.json
```

Copy the generated directory contents to `Documents/search`, then launch with `--smart`. This retains
a [native search session](../../../apps/planner-search/native/README.md) for the
complete request corpus. Replies include region, attribution, places, route
changes, hours, and reverse labels. Timing fields are excluded from comparison.
Foundation HTTP(S) is denied. This does not change device network settings.
Save the thermal state with the results and repeat after a process restart.

For durable installation, copy a complete transport bundle to
`Documents/offline-bundle` and launch with `--install`. The shared Python installer
resumes a retained prefix, verifies and activates the release, and repeats the
install to check reuse. Each run uses a fresh directory and reports its path.
Remove that directory after its report is saved. Reports use `Documents/result.json` and `Documents/device.json`
for every workload. Installation measures local bundle reads; it excludes
network transfer latency.

For offline map rendering, install the locked builder dependencies and build the
benchmark assets. Pass complete BW and cutout map directories:

```sh
npm ci --prefix builder/app
node host/route-engine/examples/phone/maps/build.mjs BW/maps CUTOUT/maps target/planner-phone/map-benchmark
xcrun devicectl device copy to --device "$DEVICE" --domain-type appDataContainer --domain-identifier com.openbikecomputer.PlannerBenchmark --source target/planner-phone/map-benchmark --destination Documents/map-benchmark
xcrun devicectl device copy to --device "$DEVICE" --domain-type appDataContainer --domain-identifier com.openbikecomputer.PlannerBenchmark --source BW/maps --destination Documents/maps
xcrun devicectl device copy to --device "$DEVICE" --domain-type appDataContainer --domain-identifier com.openbikecomputer.PlannerBenchmark --source CUTOUT/maps --destination Documents/map-cutout
xcrun devicectl device process launch --terminate-existing --device "$DEVICE" com.openbikecomputer.PlannerBenchmark --maps
```

Copy `Documents/map-result.json` after completion. The map harness uses the
planner style, icons, MapLibre, PMTiles, and contour worker. It serves local files
through bounded loopback Range reads. Its content policy blocks external requests.
It measures viewport readiness, pan, zoom, frame intervals, errors, and rendered
contours. OS caches remain uncontrolled. Native process RSS excludes WebContent
and GPU processes; use device process tracing for complete map RAM.
