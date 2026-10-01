# Native offline planner host

This module hosts the shared web planner in one persistent `WKWebView`. It is
a reusable composition boundary and has a standalone iPhone target. It is not
a screen in the Companion app. The Companion composition root can retain an
`OfflinePlannerHost` and present `OfflinePlannerView` without copying planner
logic. Follow [the iOS on-ramp](../../companion-ios/CLAUDE.md) for that integration.

Use iOS 17 or later, Swift 6, XcodeGen, Node.js, and the Rust iOS target. Install
the locked dependencies for `builder/app` and `apps/planner-search`. Follow the
[search setup](../planner-search/README.md) to prepare the model and interpreter.
Run from the repository root:

```sh
python3 host/route-engine/examples/phone/prepare-parser.py --python apps/planner-search/.venv/bin/python --model MODEL_DIRECTORY
IPHONEOS_DEPLOYMENT_TARGET=17.0 cargo build --release -p route-server --target aarch64-apple-ios
node apps/planner-native/build.mjs target/planner-native/web
node apps/planner-search/native-build.mjs target/planner-native/search
xcodegen generate --spec host/route-engine/examples/phone/project.yml
xcodebuild -quiet -project host/route-engine/examples/phone/PlannerBenchmark.xcodeproj -scheme PlannerHostBenchmark -configuration Release -destination 'generic/platform=iOS' -derivedDataPath target/planner-host/build DEVELOPMENT_TEAM="$TEAM" -allowProvisioningUpdates build
```

The preparation script verifies pinned CPython and ONNX Runtime archives,
builds the locked tokenizer crate, and stages the shared Python decoder. The
app includes the complete Python standard library. Keep `target/planner-parser`
until the build ends. The host target links only the production route-server
static library. It shares the benchmark app's bundle identifier and data container.
The app carries `app/native-third-party-licenses.txt`. The shared JavaScript
builds carry their own `third-party-licenses.txt`. Keep these files with the app
and assets.

Install `target/planner-host/build/Build/Products/Release-iphoneos/PlannerHostBenchmark.app`
with `devicectl`. Copy `target/planner-native/web` to `Documents/planner-web` and
`target/planner-native/search` to `Documents/planner-search-runtime`. Install a
complete release with the [shared installer](../../tools/planner_offline.py).
Copy or create that installation in the app container. Its `active.json` selects
the immutable release. Do not pass a loose routing directory.

Launch the app with `--installation INSTALLATION_DIRECTORY`, relative to Documents.
The first run creates a route and a named version with the real UI. Copy
`Documents/host-result.json`, `host-device.json`, `host-draft.json`, and
`host-screen.png`. Relaunch with `--terminate-existing` and `--restore` to compare
the current draft and saved versions across process restart. Keep the installation
argument on both launches. Add `--hold` to keep the screen awake until the app
closes. Capture all processes with Instruments for memory;
the device report covers the native process only. The map, parser, database,
router, and overlay providers remain resident after the run.

With a full Baden-Württemberg release, add `--full-bw` for day and long routes
and broad-to-detail map views with both route networks. Route summaries retain
missing-elevation counts and complete totals. `--routing-memory-mib N` overrides
the routing provider's complete memory budget. The default is 768 MiB plus the
decoded landmark cache. It excludes the model, search, and WebKit;
measure their combined memory separately.

For an attach-based Instruments capture, launch with `--wait-for-trace --hold`.
The process waits before opening any providers. Once recording starts, copy a
small file to `Documents/host-start`. The app consumes that marker and starts
the measured workload. Use an instrument that samples all processes to include
the WebKit processes created after the marker.

The host uses a stable loopback port and the default persistent WebKit data
store. Keep the port stable when reopening the same draft. Content rules and
the page CSP restrict web resources to that origin. User-activated external
links open in the system browser. Native providers read local files. This does
not change the phone's network settings. The host currently opens German
releases and evaluates their calendar in `Europe/Berlin`, as the release recipe
requires. The phone's time zone can differ.

Call `stop()` when disposing of the host. Retain it across SwiftUI redraws.
The API preserves search and routing JSON contracts; frontend planning,
versions, map styling, and terrain decoding use the same source as the web app.

```sh
bash apps/planner-native/test-host.sh
bash apps/planner-native/test-http.sh
npm test --prefix apps/planner-search
npm exec --prefix builder/app -- vitest run --root builder/app src/lib/planner/ src/components/planner/
```
