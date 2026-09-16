# EPIC — Test system: one selection plan, fewer test binaries, the real device under test

Rebuild how this repository decides what to test, compiles it and runs it, without rewriting the
tests themselves. The tests are good; the system around them is not. The device the phone and the
browser talk to is faked separately in three languages and only the Rust fake uses production code.
The headless host assembly is hand-written three times (simulator, web demo, iPhone host; the
desktop app runs no `App` loop at all). CI computes which crates a change affects and then compiles
and runs the whole workspace anyway. Test binaries fan out to 101 integration executables for 36
packages.

The design, its evidence and its rejected alternatives live in the plan document. It is
`docs/assets/test-system/implementation/plan.md`, landed and corrected in #1817, alongside
`epic.md`, the ten subsystem surveys and the seven reviews it rests on. Every child is derived from that document. Do not
re-derive the design here, and do not implement a section that has no open child.

## How this epic was produced

Ten parallel subsystem surveys, one build-time measurement pass on the owner's machine, an
independent review by a second agent, and a re-audit against current `develop` on 2026-09-16. The
design decisions below are settled. The corrections below are not negotiable either: they are
places where the repository moved after the plan was written.

## Design fixed by review

1. Keep inline unit tests and the existing App scenario tests where they are. No relocation quota,
   no new App seeding APIs, no `obc-app` dependency on `obc-host-core`.
2. Consolidate ordinary Rust integration tests to one target per package. Separate targets only for
   a different execution route or required process isolation.
3. Cargo's graph decides Rust selection. Only cross-language, tool, fixture and job relationships
   live in a small explicit Python map.
4. The same repository scripts run locally and in CI.
5. Keep `HostLoop`, CoreHarness, `Frames`, `Planner` and the focused refusal helpers. They express
   real differences in execution; a universal harness would have to grow or weaken them.
6. Add exactly one reusable test library: the real flat engine and store behind a bounded adapter,
   native Rust plus a wasm binding for the existing TypeScript tests and the browser harness.
7. Keep the existing screenshot script, PNG encoding, manifest and checks. Give them one explicit
   final-head route. Do not rewrite them as a Rust framework in this epic.
8. Add the missing composition checks in the packages that already own those seams. No new general
   system-test crate.
9. Delete expired scaffolding in separate bounded changes. Keep every uncertain candidate.
10. Hardware acceptance stays in its current issues. No board predicate extraction, no hardware CI
    slot, no native upload sender here.

## The repository moved under the plan

The plan was written against `d42e2e3a`. `develop` is 764 commits ahead. These rows are wrong in
the document and are overridden here. TS-0 fixes them in the document itself.

| Plan content | What is true now | Binding instruction |
|---|---|---|
| Delete `testing/coverage-policy.toml`; defer the coverage ratchet | TS5 shipped in #1784 / #1789. The policy file carries real `enforcement = "ratchet"` components, `testing/coverage-baseline.json` holds measured baselines with CI evidence, and `tools/coverage_report.py` runs per pull request under llvm-cov instrumentation. | **Keep the files and the ratchet.** Deleting them discards accepted work. Separately: llvm-cov instrumentation is a new, unmeasured cost inside the `test` job. Measure it in TS-B and report it; do not remove it without the owner. |
| Delete the LOC ledger tools | #1786 / #1787 made `tools/loc_ledger.py --storage-total --check-budget` the FS11 acceptance command. | **Keep `loc_ledger.py`.** Only the per-merge delta reporting and `loc_report.py` may go, and only with FS11's owner agreeing. |
| Add a Linux desktop launch smoke; this "closes #994" | Already delivered as `apps/obc-desktop/e2e/launch.py` under `xvfb-run -a dbus-run-session` (#1727 / #1729), registered as `e2e.desktop-linux-launch`. | **Drop the row.** Do not add a second launch path. #994 keeps its real remainder: Windows launch, and physical device enumeration and route upload. |
| Add Playwright and a browser smoke; "is Playwright acceptable?" is an open decision | Answered yes and shipped: `web.demo-browser`, a Playwright suite at `apps/obc-web-demo/tests/browser/`, registered at end-to-end level. | **The decision is closed.** What remains is the **builder** assembly and download journey, which is a different product from the demo. Reuse the existing Playwright setup; do not introduce a second browser stack. |
| The UI sweep runs unconditionally in the `test` job | It is already gated on `ci.ui-snapshots` being selected. | The remaining lever is only to take it off the `test` job's serial path. Claim less than the plan does. |
| The registry is a passive inventory that can be deleted late | `tools/suite_registry.py` (1,652 lines, nine subcommands) is load-bearing at runtime in seven places: `cargo-filter` builds the nextest filter for both `test` job steps and for `obc test full` / `obc check test`; `select` feeds the `selection` job; `run --affected` / `run --level` are `obc test affected|unit|…`; `run --scheduled weekly` drives `test-weekly.yml`; `gates` supplies the `obc check` gate vocabulary and its `--unreproduced` report; `check` and `validate-filters` run in the `fixture-registry` job and `check` also validates `testing/coverage-policy.toml`; `check-issues` drives `test-exception-health.yml`; and `tools/ci_aggregate.py` imports `workflow_jobs()` for its upstream-failure closure. Only `list` and `explain` are documentation-only. | **TS-C is about twice the cut the plan assumes.** Every one of those consumers needs a replacement or a deliberate retirement in the cutover, including the coverage-policy validation and the two weekly workflows. Only about 19 to 24 of the 83 registry rows are pure Cargo restatement; 59 rows carry facts Cargo cannot see (cross-language triggers, `specs/**` fan-out, cross-workspace edges from `firmware/obc-fw-nrf54l/src/*.rs` to `obc-host-core`, platform restrictions, target-level cadence). |
| One ordinary integration target per package, universally | **Eight** `fixtures.*` suites exist: five Rust ones that select with `--test <name>` (each backed by a `[[test]] required-features = ["external-fixtures"]` entry), and three Python ones (`assistant-inputs`, `landmark-content`, `assistant-places`) on the manual cadence. `required-features` is a target attribute, so those five cannot merge into a package's ordinary `main.rs`. The allocator target `host/obcm-assemble/tests/sort_budget.rs` must also stay alone, and `firmware/obc-app/tests/overlay_plane.rs` swaps the panic hook and needs serial execution once merged. | **Carve the five Rust fixture targets and the allocator target as named exceptions** in TS-A. Do not break their routes; `testing/suites.toml` names them in ten `targets` / `exclude_targets` places, so the registry edit lands in the same change. |
| The host inventory | `apps/obc-ios-host` landed: a host running the real `App::run_pass` through the production `HostLoop` on an iPhone over a real card file, with `rust.obc-ios-host`, `ci.ios-host-portability` and `ci.ios-device-build`. There are now three rendering hosts (simulator, web demo, iPhone); `apps/obc-desktop` is a Tauri shell with no `obc-app` dependency and no frame step. | Treat the iPhone host as a first-class host wherever hosts are enumerated. Account for `ci.ios-device-build` in the selection design: its triggers include `host/obc-host-core/**`, so many host-core edits now pull a macOS job. |
| Registry-waste inventory: twelve `cadence_conflict` entries, orphan weekly cadences | `cadence_conflict` is now zero. `test-weekly.yml` actually runs the weekly cadence, and `manual` is an explicit named class. The `live` level now has a user. | Restate the case for TS-C on its real remaining waste: three hand-rolled workflow parsers (`scan_workflow`, `workflow_jobs`, the dead `coarse_filters`), shell-command and Trunk-HTML scraping to learn which job compiles which package, two separate `cargo metadata` passes, and the ~250 to 300 lines of TOML that restate Cargo. |
| Heavy route: "the existing render exhaustive ignored-library suite" (`obc-render/src/rain.rs`) and "the display timing probe" (`ls021/rowdiff.rs:342`) | `rain.rs` was deleted with weather. `rowdiff.rs:342` is inside an ordinary fast test. The whole repository has **three** `#[ignore]` tests (two Copernicus downloads in `host/obc-dem`, one 549 MB captured-source check in `host/obc-pack`), and its actual opt-out mechanism is Cargo examples on the `manual.*` cadence (`manual.obc-display` = `examples/row_hash_timing.rs`, three vector generators) plus `required-features`. | **There is no heavy recipe list to maintain.** Drop `obc test heavy NAME` from the design. Generators and the timing probe keep their explicit commands; the two live downloads keep `live.copernicus`; the weekly iOS suite keeps `test-weekly.yml`. TS-B routes those existing items; it does not build a heavy tier for zero recipes. |
| `sim-peak-view` may leave the test sync profile "after confirming no selected test consumes it" | Confirmed: no test reads it. Its only reader is `apps/obc-sim/src/peak_view.rs` at runtime, yet it sits in the `test` profile, so `obc test fixtures` and `ci.rust-fixture-tests` download 103 MB nothing asserts on. | **Remove it from the `test` profile in TS-B.** It stays in the `sim` profile. |
| "A missing fixture fails with the exact setup command" is a rule to add | `host/obc-fixtures` already prints the sync hint, but only when `OBC_REQUIRE_FIXTURES` is set; without it, `file()` returns `None` and all 16 call sites `.expect(...)` that `None` with a message carrying no hint. The documented "return early on `None`" path is unreachable. | TS-B makes bounded fixture tests unconditional: `file()` always fails with the sync command, the env variable and the `None` branch go, and the 16 `.expect` sites become plain reads. |
| Move `COPERNICUS_ATTRIBUTION`, "update its production and test references", drop the app's `obc-dem` dev-dependency (`obc-app/Cargo.toml:42`) | The dev-dependency is at `Cargo.toml:41`. The app's only use of `obc-dem` is one `#[cfg(test)]` unit test in `src/screen/settings/about.rs`, not anything under `tests/`. `obc-elevation` is already a regular dependency of the app. | Move the constant to `obc-elevation` and fix that one unit test. No `tests/` edit is involved. |
| obc-route tests: "replace the packer's writer/model use with the existing handwritten testkit" and drop the `obc-pack` dev-dependency | Only `tests/nav.rs` and `tests/detour.rs` use `obc-pack`. The writer use is replaceable by `obcm-testkit`, but four call sites use `obc_pack::nav::integrate_edge_ascent`, which is production ascent logic under test, not a writer. | Move those four ascent assertions into `obc-pack`'s own tests (or keep the dev-dependency and say so). Do not re-implement ascent integration in the testkit. |

All measured figures the plan quotes must be re-taken before they are cited again. `obc-app` fell
from 22 integration targets to 16, the workspace from 110 integration binaries to 101, the golden
manifest from 317 frames to 263, the sweep from 233 simulator launches to 196. One figure moved the
other way: the golden manifest changed 24 times in three days, so the sweep's churn argument is
stronger, not weaker.

### Measured 2026-09-16, current shape

These replace the plan's build figures as the baseline TS-A and TS-B compare against.

CI, warm `develop` cache, pull-request runs 35070441391 and 35068175326:

| Job or step | Duration |
| --- | --- |
| `test` job, whole | 302 s and 304 s |
| `cargo nextest` fast tier (workspace, all features, llvm-cov instrumented) | 97 s and 99 s |
| `cargo nextest` captured-fixture tier | 14 s |
| UI snapshot manifest check (when selected) | 66 s |
| builder pytest preparation (`pip` + release `obc-pack` build) | 33 s; the tests themselves 2 s |
| `embedded` release build | 190 s |
| `ios-app` | 1,069 s (14 to 18 min in the runs sampled) |

The `test` job already meets the six-minute target with the sweep inside it. The pull-request wall
clock is set by `ios-app` whenever it is selected, and that job is #1788's, not this epic's. What
TS-A and TS-B can win on CI is the fast-tier compile-and-run and the sweep's 66 s off the serial
path, and both must be reported as measured deltas, not assumed.

Owner's machine, `cargo test -p obc-app`, warm cache:

| Command | Wall time |
| --- | --- |
| `--lib --no-run`, nothing changed | 0.5 s |
| `--lib --no-run` after touching `src/lib.rs` | 2.3 s |
| `--lib` run (922 tests) | 1.0 s |
| `--no-run` for the whole package (17 binaries), nothing changed | 8.2 s |
| `--no-run` for the whole package after touching `src/lib.rs` | 3.7 s |

The library-only loop is already under the plan's ten-second target. Consolidating the app's 16
integration targets is worth a few seconds per iteration, not the 12.7 s versus 2.9 s the plan
measured at the old shape. The consolidation still earns its place on simplicity (one target, one
`common` shim, one filter term per package) but must not be sold as a speed win until TS-A measures
it at the merged shape.

## Settled: share the frame seam, keep the hosts

The plan decided against a shared host-side application assembly. That decision was made on three
reasons which have all since stopped holding: it claimed the simulator's storage differs, which is
untrue (`apps/obc-sim` aliases `obc_host_core::FlatRouteStore as RouteStore`); it claimed the
assembly was not stable enough to share, but a third rendering copy has since been written by hand
and is driven by its own tests in about twenty lines; and it claimed sharing would force a change
to `HostLoop::execute`, which has since been split into `serve_effects`, `serve_derived` and a free
`deliver` for unrelated reasons.

**The owner's decision, 2026-09-16: extract only the part that is provably identical in every host,
and leave everything host-specific alone.** Verified against source before adopting.

Extract into `obc-host-core`, beside the existing `photo::Preparer`, which is the precedent for
shared host frame machinery:

- The render pair: `App::render_scene_map_photo_timed` followed by `App::render_overlay`, with the
  shared `rgb565_to_device64` conversion. `apps/obc-web-demo` and `apps/obc-ios-host` inline this
  identically, comments included; `apps/obc-sim/src/map_file.rs` has already factored the same pair
  into a local helper, which collapses into the shared one.
- The render-on-demand predicate, `plan.render.map || plan.render.overlay || !ready ||
  app.photo_pending()`, identical in the web demo and the iPhone host.
- The re-open-active-route step that must run before rendering, because the executor may have
  committed new geometry under the frame.
- The single-loop hold-cancel consumption, `app.take_hold_cancel()`, identical in both and carrying
  the same rule.

**Second verification pass, 2026-09-16, against source.** The list above is true for the web demo
and the iPhone host, with two expression-level differences (the web demo inlines the route match
that the iPhone host has as `active_route()`; the iPhone host holds `Option<peak_view::Runtime>`
so its panorama argument is `as_ref().and_then(..)`). The simulator is not the same: its
`map_file::render_base_frame` is generic over the draw target and colour function, passes a
`StdClock` where the two hosts pass `NoopClock`, returns `RenderStats`, takes the `FramePhoto` from
its caller, and the simulator GUI has **no** render-on-demand predicate and no `ready` state (it
repaints every frame). The route re-open exists in all three over the same `FlatRouteStore`; the
hold-cancel line exists in all three with the same rule (the iPhone copy lost the comment's third
line). What is therefore provably identical across all three hosts is the render pair itself when
the clock and the photo are parameters, plus the route re-open. The predicate and the hold-cancel
line are one statement each. TS-G's exact extraction surface is fixed in its child issue against
these facts.

Do not extract, and do not let this grow to cover: the pacing and clock source, which is a display
link, a browser animation frame and a replay player respectively; sensors; store and platform
types; the `pass` and `execute` argument lists, which genuinely differ per host; or the board, which
is asynchronous `no_std` and shares nothing here.

The test of whether this stayed narrow is simple. If the shared piece acquires a switch, a mode or a
capability flag to serve a second caller, it has become the framework this project does not want.
Stop and put that behaviour back in the host that needs it.

## Existing owners to reuse

| Responsibility | Existing seam |
| --- | --- |
| One application step | `App::run_pass(PassInputs) -> PassPlan`, `firmware/obc-app/src/device_core/pass.rs` |
| Serving effects and returning outcomes | `HostLoop`, `HostPlatform`, `host/obc-host-core/src/dispatch.rs` |
| Protocol engine and its policies | `obc_link::flat::{Engine, Policy, OpenPolicy, Ceilings}` |
| Card and fault injection | `obc_storage::flat::{BlockDevice, SparseDisk, FaultPlan, When, FaultOnce}`; the real card behind `HostStore` and `ObjectSource` in `host/obc-host-core/src/flat_store.rs` (there is no `StoreSource`); the cut-point recipe in `firmware/obc-storage/src/flat/crash.rs` (`finalisation_retry_does_not_rewrite_an_intact_tail`) |
| Existing protocol harness | `firmware/obc-link/tests/flat_harness/mod.rs` (`finish_recording` is the FS8 journal path; `link_lost()` reconnects immediately and is not an unplug) |
| Mounting map bytes on a card and rendering them | `obc_host_core::FlatMap::{from_bytes_in, open_in}` and the counted-source render oracle in `host/obc-host-core/src/flat_map/tests.rs`; the card-to-App-frame precedent in `host/obc-host-core/src/dispatch/find_tests.rs` |
| Pass ordering evidence | the pass-stage trace in `firmware/obc-app/src/device_core/pass.rs` (`pass_trace()` and its order tests); this is what survives when `feeders.rs` goes |
| Web transport seam the fake plugs into | `BytePipe` / `DeviceLink` in `builder/app/src/lib/usb/pipe.ts`; `loopbackLink` stays, only `MockDevice` is replaced |
| Independent byte oracles | `host/obcm-testkit`, the assembler oracle, `build_obcr`, `SyntheticDem` |
| Screens and goldens | `firmware/ui-snapshots.sh`, `firmware/tools/ui_snapshot_manifest.py` |
| Browser journeys | the existing Playwright setup under `apps/obc-web-demo/tests/browser/` |
| Selection and aggregation | `tools/suite_registry.py` graph and closure code, `tools/ci_aggregate.py` |

## Child plan and dependencies

No child is open yet. Each must carry its own files, commands, acceptance and deletion scope,
refined against current source, before implementation starts.

| ID | Issue | Deliverable | Depends on |
| --- | --- | --- | --- |
| TS-0 | #1817 | Plan document under `docs/assets/test-system/implementation/`, corrected twice against source | — |
| TS-A | #1819 | One integration target in each of the seven packages with five or more (75 of 101 binaries); fixture and allocator targets carved by name; the four ascent assertions move to `obc-pack` and `obc-route` drops its packer dev-dependency; before/after measurement | — |
| TS-B | #1820 | Explicit routes for the three ignored tests and five manual commands (no heavy tier); fixture loader always fails with the sync command; `sim-peak-view` out of the `test` profile; the `test` job runs the affected package set; sweep, builder pytest and the seven guard jobs reshaped; `obc test -p` on nextest; `COPERNICUS_ATTRIBUTION` moved; llvm-cov cost measured | TS-A |
| TS-C1 | #1821 | Delete the registry's consumer-less parts (`list`, `explain`, dead `coarse_filters`, second metadata pass, Trunk/shell scraping, `test_cost.py`); exception machinery shrunk to a small script; byte-identical selection before and after | TS-B |
| TS-C2 | #1822 | `tools/test_plan.py` replaces the selector and the pure-Cargo registry rows in one cutover behind a differential harness; five fail-closed rules tested; PyYAML structural workflow check | TS-C1 |
| TS-D | #1823 | `host/obc-flat-device` native plus wasm; harness assembly shared; about 157 TypeScript tests and the dev harness ported under the three rulings; `MockDevice` deleted, `loopbackLink` kept; two PRs | — |
| TS-G | #1824 | `obc_host_core::frame::render` and `active_route` extracted; three hosts collapse onto them; one final-head sweep with zero manifest changes | — |
| TS-E1 | #1825 | Interrupted recording through storage recovery and GET to the pinned GPX, in the link suite | TS-D |
| TS-E2 | #1826 | Assembled map through the card to a rendered App frame, in the assembler oracle | TS-G |
| TS-E3 | #1827 | Builder browser assembly and download journey, download half, on a shared fixture catalog | TS-B |
| TS-F | #1828 | Deletions (s6b soak, BLE repro bin, `feeders.rs`, `loc_report.py`, ledger delta mode), release routes, contributor docs, final line-count report | TS-C2, TS-D, TS-E1, TS-E2, TS-E3 |
| TS-H | #1829 | Captured Komoot and bikepacking.com waypoint imports to device presentation. **Owner-gated and deliberately last**: the owner captures the bikepacking.com route and settles redistribution first | TS-F, owner |

TS-G is a production refactor, not a test change, and carries its own acceptance: every host renders the same frames it rendered before, and the shared piece has no per-host switch. TS-A and TS-B carry no design risk; their value is measured, not assumed (see the table above). TS-C2 is the only step that changes how CI decides anything; it lands as one cutover, never as two selectors running side by side. Execution order: TS-A, TS-D and TS-G in parallel; then TS-B and TS-E1 and TS-E2; then TS-C1, TS-E3; then TS-C2; then TS-F; TS-H when the owner supplies the fixtures.

## Decisions recorded 2026-09-16

Taken by the owner on the second audit (comment below the body); binding for the children.

- **D1** Consolidation covers the seven packages with five or more targets only.
- **D2** No heavy tier and no `obc test heavy`. Existing manual commands, the live Copernicus route and the weekly workflow are the routes.
- **D3** TS-C lands as C1 (delete consumer-less parts) then C2 (selector cutover behind a differential harness, deleted after use). Fail-closed rules are explicit requirements with tests.
- **D4** TS-D rulings: the no-space assertion changes to the engine's `busy`; chosen object ids are rewritten to captured ids; exactly two adapter test hooks (request trace, stop answering); the dev harness is ported in the same child.
- **D5** The UI sweep stays automatic and required when `ci.ui-snapshots` is selected, as its own CI job off the `test` job's serial path. No manual-only route.
- **D6** TS-G extracts the generic render function (clock and photo as parameters) and the route re-open helper; the one-line predicate and hold-cancel stay in the hosts. Three rendering hosts.
- **D7** Provider-import fixtures are the owner's to capture; TS-H is last and owner-gated. The epic's closing report must remind the owner.
- **D8** The builder browser journey is download-only; OPFS in headless Chromium is proven first.
- **D9** This epic does not reduce pull-request wall time while `ios-app` is selected; that is #1788.

## Shared implementation rules

- Preserve assertions. A consolidation or a port that cannot keep a behaviour is a defect to
  resolve or a documented discrepancy, never a dropped assertion.
- Package flags narrow compilation; target filters narrow execution. Say which one a change uses.
- Never select individual test functions in CI.
- Generators, live-service downloads and physical procedures are explicitly invoked and excluded
  from every verification route. No blanket ignored-test run.
- A missing fixture or tool fails with the exact setup command. Tests never start a download.
- A local run that cannot cover every platform prints the unsupported jobs and returns a distinct
  incomplete status. It never reports full verification.
- Report actual added, deleted and moved line counts per pull request under the existing
  tracked-file convention. No projected total, and no manufactured deletion to meet one.
- Follow the standing verification budget in CLAUDE.md: whole suites, focused checks, at most one
  final-head sweep and one head resource build, one review round.

## Completion gates

- Scoped and affected commands use the same plan and scripts as CI. Unknown ownership and missing
  selected work fail.
- Ordinary Rust integration targets are consolidated with named exceptions, and the application's
  focused command still runs its retained integration tests.
- Cross-language, vector and artifact consumers are selected without a source edit in their own
  language.
- Existing conformance, failure, storage, vector, dirty and resource evidence is intact, and the
  composition checks are live.
- TypeScript device flows run against the real protocol engine and store; the hand-written
  `MockDevice` is gone.
- UI verification has one explicit final-head route with preserved golden semantics.
- The obsolete selection scaffolding is removed and the contributor instructions describe commands
  that exist.
- The coverage ratchet and the FS11 counting command still work unchanged.

Targets, not predictions: an application library-only edit reaches a result within 10 seconds on the
measured machine (already true: 2.3 s build plus 1.0 s run); a leaf-package pull request within 3
minutes on a warm CI cache; ordinary selected jobs within 6 minutes (the `test` job is at 5 today).
No target may regress. Record compile, execution and setup separately. A missed target is examined
at its measured bottleneck, never met by weakening assertions or promoting work to a manual route.

## Intentional limits

No proof of physical board equivalence. No Windows USB acceptance. No Swift-to-firmware runtime
bridge. No large-map empirical memory gate. No coverage change beyond keeping what TS5 accepted. No
unified test language. No in-process screenshot rewrite. No cron beyond the weekly route that
already exists. Each is separate work, named in its own issue, not a hidden dependency here.

## Relationship to #1448 and #1449

#1449's TS5 (coverage) and TS7 (test health) are complete. Its only remaining obligation is TS6,
which owes four things, dispositioned as follows. #1449 closes once this epic is open.

| TS6 obligation | Disposition |
|---|---|
| Real-browser builder assembly and download | **Absorbed here**, in TS-E, on the Playwright setup that already exists. The shipped Chromium journey (#1728 / #1731) covered the web demo, a different product; it does not close this. |
| Captured waypoint-bearing provider imports | **Absorbed here**, in TS-E. Retains the #953 obligation: Komoot and bikepacking.com routes with waypoints, through actual import and device presentation, on captured fixtures with provenance. Basic GPX decoding does not satisfy it. State of the fixtures on 2026-09-16: one captured Komoot export exists (`companion-ios/Packages/OBCKit/Sources/OBCMock/Fixtures/sample-import.gpx`, five waypoints, exercised only by the Swift decoder); **no bikepacking.com fixture exists anywhere**; `.gitignore` blanket-ignores `*.gpx` with three negations. Capturing a bikepacking.com route and settling its redistribution terms is the owner's, not an agent's. |
| Windows release launch | **Stays in #994.** Its remainder is Windows launch plus physical device enumeration and route upload: platform and hardware acceptance, not test-system work. |
| #1177 orchestration and observation | **Stays in #1177.** This epic explicitly does not build the native upload sender. #1177's acceptance text still names a registry `cadence` field that no longer exists and needs rewording against the current workflow-based cadences. |

#1448 gates final integrated acceptance on TS6 and on physical evidence this epic does not produce.
After the disposition above, **no test-system work blocks #1448**; what remains under it is physical
acceptance owned by #1383, #1393, #1262, #1392, #1420, #1501 and #1166.

Two issues opened after the plan need coordinating rather than absorbing. #1788 (Swift simulator
cost and incidental waits) overlaps TS-B on the cost half only; its waits half is out of scope, and
its stated constraint against changing the screenshot gate conflicts with taking that gate off pull
requests, which is an owner decision. #1815 item 1.3 builds a text-recording surface over
`App::render_frame` for copy fitting; under decision 7 it does not collide with this epic and stays
where it is.
