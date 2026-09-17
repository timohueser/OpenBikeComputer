# Web demo

This host runs the shared firmware application and renderer in the landing page. The page owns
the frame loop and controls. The host embeds the tracked Grimsel map, route and ride inputs from
`apps/obc-sim/assets/`.

## Build and browser test

Run from the repository root. Install Rust, the `wasm32-unknown-unknown` target, Trunk 0.21.14,
Node.js 22 and Python 3.11 or newer. Then run:

```sh
NO_COLOR=true trunk build --config docs/Trunk.toml
npm ci --prefix apps/obc-web-demo/tests/browser
cd apps/obc-web-demo/tests/browser
npx playwright install chromium --only-shell
cd ../../../..
npm test --prefix apps/obc-web-demo/tests/browser
```

On Linux, use `npx playwright install --with-deps chromium --only-shell` to install system
libraries too. The package lock pins Playwright 1.63.0 and its Chromium headless shell revision
1243 (153.0.8010.12). The suite runs on Linux and macOS. It starts a loopback server on port 4178,
serves the existing `docs/dist`, and shuts the server down when tests finish. It never builds a
second copy of the demo. Set `OBC_BROWSER_PORT` to use a different free port in another worktree.
An existing server on the selected port is an error.

The `web.demo-browser` end-to-end suite selects Ride log and follows the real Save to Home. It
uses the device controls to open Rides and Ride detail, switches to Load route, waits for reset
completion and Route received, saves another ride, then reloads and completes a fresh demo. It
checks existing readiness, screen and reset-status exports, nonblank canvas output and a changed
frame after navigation. Errors, panics, failed resets and stalled transitions fail the suite.
There is one worker, bounded state waits, and no test retries.

CI runs this journey after the existing Trunk build in the `wasm` job. It also runs when the Rust
dependency graph selects that job. A skipped journey cannot pass the job. Each test invocation
clears `.artifacts/web-demo/` and writes JUnit and browser diagnostics there. Failed tests also
retain a screenshot and Playwright trace. CI uploads `web-demo-browser-ATTEMPT` after an executed
test step succeeds or fails; missing evidence fails the upload. Download with:

```sh
gh run download RUN_ID --name web-demo-browser-ATTEMPT --dir test-results
```

This journey covers page, WASM, rendering, controls and reset integration. It does not assert
exact saved-object counts, identities or sample bytes. The native Demo and storage tests own
those assertions. Companion images are recorded screenshots; no phone or board is used.

## Guided Visit

The Add a stop chapter pauses the captured ride at its mid-climb position. The `find` command
opens the shared Find Place screen. The tour selects Train, waits for a measured choice, opens
Gletsch, and waits for the complete Visit preview. Select accepts that exact route and returns
to the map. The Recorder continues through acceptance.

The page waits on `obc_demo_find_ready()` and `obc_demo_visit_status()` as well as the screen name.
A planning screen alone cannot advance a preview or acceptance caption. The native journey uses
the page's input pacing and dwells. The simulator presentation journey uses the same
`grimsel-climb-demo.gpx` timebase and the external Grimsel map, with frame equality checks.
