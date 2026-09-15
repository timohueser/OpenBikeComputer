# Guided Find and Visit integration

Code commit `34acf4eb` updates the web tour and the existing simulator presentation journey.
The baseline is `1430ea5b`. Shared seam fix `c54f2e5b` is included as `c707aaba`.

## Production behavior

The old tour selected Grimsel Hospiz, which has no mapped bike approach. The production detail
correctly refused Visit. The new tour selects Train through shared Find Place and reviews Gletsch,
OSM node `420041304`, from the real offline map. It freezes replay while the rider compares costs.
It waits for measured choices and the immutable Preview, then sends Select to accept the route.
The final caption requires Accepted and Map. A failed or stale route cannot advance that caption.

The complete web journey checks that the original remains active before acceptance, the accepted
Metadata checkpoint contains the exact candidate fingerprint, and Recorder is still active.
It also checks that a reset cannot take the planner while review is in flight. Its UI clock uses
the page's 420 ms input pacing and 2600 ms dwells, so acceptance must keep a fresh real replay fix.
The simulator uses the same demo GPX timebase as the page. The ordinary climb GPX placed the old
1500 s test baseline about 8 km earlier. Both are tracked source files; no coordinate or route
answer is injected into Find or Visit.

The simulator reads the external Grimsel map and uses the normal HostLoop, source catalog,
planner, immutable output, and checkpoint path. Each Find/detail/preview/accepted-map dwell checks
that the presented frame equals the rendered framebuffer on every row.

## Validation

The build target was `/Users/timo/Documents/OSM-agents/ra06-find-place/target`.

- `./tools/obc test -p obc-web-demo`: 16 tests pass.
- `./tools/obc test fixtures -p obc-sim`: 66 simulator tests and 11 dirty-parity tests pass.
- `cargo clippy -p obc-web-demo -p obc-sim --all-targets --features obc-sim/external-fixtures -- -D warnings`: passes.
- `cargo check -p obc-web-demo --target wasm32-unknown-unknown`: passes.
- `node --check /tmp/ra06-tour-page.js`, using the exact script from `docs/index.html`: passes.
- `./tools/obc suites check`: passes.
- `cargo fmt --all`, formatting in each standalone Cargo root, and `git diff --check`: pass.
- `python3 docs/build_docs.py --check-links`: passes.

Logs are `/Users/timo/Documents/OSM-agents/ra06-tour-{web-tests,sim-fixtures,clippy,wasm,registry,docs}.log`.
The shared route suite and seam contract evidence are in [the Visit evidence](ra05-evidence.md).

This support change adds no rendering layout or device-state bypass. Normal Assistant menu entry
is a separate integration change. The browser end-to-end journey remains the CI gate; it was not
repeated locally. No UI sweep, resource image, full CI mirror, or device test was run. Hardware
acceptance remains pending without a connected device.

## Review delta

Commit `5bd3f163` bounds both native Find and Preview waits to 750 frames at 16 ms, the page's
12 s timeout. The whole 16-test web suite passes with these limits. The same journey also checks
the refused-navigation-reset latch at playback end after acceptance: the queue remains empty,
reset remains Failed, and Recorder keeps the same session. This timing avoids moving the frozen
origin before acceptance. Registry and format checks pass. Logs are
`/Users/timo/Documents/OSM-agents/ra06-tour-review-{delta,registry}.log`.
