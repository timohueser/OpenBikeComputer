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
The optional final arguments set iterations and the routing memory budget in
MiB; without one, the package's default budget applies. Timing excludes
JSON serialization. Append `retained` to keep one router and change the first
coordinate on each iteration. This mode measures fresh requests with retained
caches. Reports include the budget. HTTP timing includes serialization and
response transfer.
Compare equal request corpora, profile lists, and input identities.

The file audit verifies all runtime hashes and counts complete gzip output.
Its transfer figure is a codec measurement, not a bundle format. It excludes
source mirrors and includes every runtime index and manifest.

## Physical iPhone

The benchmark app measures the components that the Companion app ships. Install
XcodeGen and the Rust iOS target. Set `TEAM` and `DEVICE` to the signing team and
paired device. The benchmark app uses its own data container.

```sh
IPHONEOS_DEPLOYMENT_TARGET=17.0 cargo build --release -p route-engine --example phone_benchmark --target aarch64-apple-ios
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
pressure and power conditions separately. Routing results do not include map or
search costs.

Launch each workload in a new process with `--terminate-existing`. Full routes
use the default workload. Pass `--package RELATIVE_PATH` to use another routing
directory below Documents. Copy each report before the next launch. Use
`--retained --hold` for the same fresh-request workload with an awake screen.

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
The shared evaluator checks holidays, school holidays, solar times, and daylight
saving transitions. It evaluates Berlin hours in the regional calendar, whatever
the device time zone. Device settings stay unchanged.
