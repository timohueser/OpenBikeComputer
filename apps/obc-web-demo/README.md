# Web demo

This host runs the shared firmware application and renderer in the landing page. The page owns the
frame loop and the controls. The host embeds the tracked Grimsel map, route and ride inputs from
`apps/obc-sim/assets/`.

## Build and test

Needs Rust with the `wasm32-unknown-unknown` target, Trunk 0.21.14, Node.js 22 and Python 3.11 or
newer. Run from the repository root:

```sh
NO_COLOR=true trunk build --config docs/Trunk.toml
npm ci --prefix apps/obc-web-demo/tests/browser
cd apps/obc-web-demo/tests/browser
npx playwright install chromium --only-shell
cd ../../../..
npm test --prefix apps/obc-web-demo/tests/browser
```

On Linux, use `npx playwright install --with-deps chromium --only-shell` to get the system
libraries too. The lock file pins Playwright and its Chromium headless shell revision. The suite
runs on Linux and macOS.

**The suite never builds a second copy of the demo**: it serves the existing `docs/dist` from a
loopback server on port 4178 and shuts it down when the tests finish. `OBC_BROWSER_PORT` selects
another free port for another worktree, and an existing server on the selected port is an error.

Each run clears `.artifacts/web-demo/` and writes JUnit and browser diagnostics there. A failed
test also keeps a screenshot and a Playwright trace. CI uploads `web-demo-browser-ATTEMPT`:

```sh
gh run download RUN_ID --name web-demo-browser-ATTEMPT --dir test-results
```

## What the journey covers

The `web.demo-browser` suite selects Ride log, follows the real Save to Home, opens Rides and Ride
detail through the device controls, switches to Load route, waits for reset completion and Route
received, saves another ride, then reloads and completes a fresh demo. It checks readiness, screen
and reset-status exports, nonblank canvas output, and a changed frame after navigation. Errors,
panics, failed resets and stalled transitions fail the suite. One worker, bounded state waits, no
retries.

It covers page, WASM, rendering, controls and reset integration. It asserts no saved-object count,
identity or sample byte: the native Demo and storage tests own those.

## Guided Visit

The Add a stop chapter pauses the captured ride at its mid-climb position. The `find` command
opens the shared Find Place screen; the tour selects Train, waits for a measured choice, opens
Gletsch, and waits for the complete Visit preview. Select accepts that exact route and returns to
the map, and the Recorder continues through acceptance.

The page waits on `obc_demo_find_ready()` and `obc_demo_visit_status()` as well as the screen
name. **A planning screen alone cannot advance a preview or acceptance caption.**
