# EPIC — Test system: one selection plan, fewer test binaries, the real device under test

Rebuild how this repository decides what to test, compiles it and runs it, without rewriting the
tests themselves. The tests are good; the system around them is not. The device the phone and the
browser talk to is faked separately in three languages and only the Rust fake uses production code.
The headless host assembly is now hand-written four times. CI computes which crates a change
affects and then compiles the whole workspace anyway. Test binaries fan out far enough that the
first-launch cost of freshly linked executables exceeds the compile.

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
| The weather bakery, its client, the device reader, the four-leg CI matrix, the weather target map, the airmass cadence question, and the "published weather through client to screen" composition check | **Weather was removed entirely** in `ee5424579`: 435 files, 80,732 deletions. `firmware/obc-weather`, `host/obc-wx-bake`, `host/obc-wx-client`, `ops/` and the rain-radar demo no longer exist. | **Delete every weather section.** There is no weather composition check to write and no airmass decision to make. The two nowcast deletion rows are already banked. |
| Delete `testing/coverage-policy.toml`; defer the coverage ratchet | TS5 shipped in #1784 / #1789. The policy file carries real `enforcement = "ratchet"` components, `testing/coverage-baseline.json` holds measured baselines with CI evidence, and `tools/coverage_report.py` runs per pull request under llvm-cov instrumentation. | **Keep the files and the ratchet.** Deleting them discards accepted work. Separately: llvm-cov instrumentation is a new, unmeasured cost inside the `test` job. Measure it in TS-B and report it; do not remove it without the owner. |
| Delete the LOC ledger tools | #1786 / #1787 made `tools/loc_ledger.py --storage-total --check-budget` the FS11 acceptance command. | **Keep `loc_ledger.py`.** Only the per-merge delta reporting and `loc_report.py` may go, and only with FS11's owner agreeing. |
| Add a Linux desktop launch smoke; this "closes #994" | Already delivered as `apps/obc-desktop/e2e/launch.py` under `xvfb-run -a dbus-run-session` (#1727 / #1729), registered as `e2e.desktop-linux-launch`. | **Drop the row.** Do not add a second launch path. #994 keeps its real remainder: Windows launch, and physical device enumeration and route upload. |
| Add Playwright and a browser smoke; "is Playwright acceptable?" is an open decision | Answered yes and shipped: `web.demo-browser`, a Playwright suite at `apps/obc-web-demo/tests/browser/`, registered at end-to-end level. | **The decision is closed.** What remains is the **builder** assembly and download journey, which is a different product from the demo. Reuse the existing Playwright setup; do not introduce a second browser stack. |
| The UI sweep runs unconditionally in the `test` job | It is already gated on `ci.ui-snapshots` being selected. | The remaining lever is only to take it off the `test` job's serial path. Claim less than the plan does. |
| The registry is a passive inventory that can be deleted late | `tools/suite_registry.py` is now load-bearing at runtime: `cargo-filter` builds the nextest filter for both of the `test` job's nextest steps, and `run --scheduled weekly` drives `test-weekly.yml`. | **TS-C is a bigger cut than the plan assumes.** Both live call sites need replacements in the same change. |
| One ordinary integration target per package, universally | Five new `fixtures.*` suites select with `--test <name>`, i.e. per-target selection. | **Carve them as named targets explicitly** in TS-A, the way the allocator target is carved. Do not break their routes. |
| The host inventory | `apps/obc-ios-host` landed: a fourth host running the real `App::run_pass` through the production `HostLoop` on an iPhone over a real card file, with `rust.obc-ios-host`, `ci.ios-host-portability` and `ci.ios-device-build`. | Treat the iPhone host as a first-class host wherever hosts are enumerated. Account for `ci.ios-device-build` in the selection design: its triggers include `host/obc-host-core/**`, so many host-core edits now pull a macOS job. |
| Registry-waste inventory: twelve `cadence_conflict` entries, orphan weekly cadences | `cadence_conflict` is now zero. `test-weekly.yml` actually runs the weekly cadence, and `manual` is an explicit named class. The `live` level now has a user. | Restate the case for TS-C on its real remaining waste, not on the old inventory. |

All measured figures the plan quotes must be re-taken before they are cited again. `obc-app` fell
from 22 integration targets to 16, the workspace from 110 integration binaries to 101, the golden
manifest from 317 frames to 263, the sweep from 233 simulator launches to 196. The build lever is
still real, but its size is unproven at the new shape. One figure moved the other way: the golden
manifest changed 24 times in three days, so the sweep's churn argument is stronger, not weaker.

## Settled: share the frame seam, keep the hosts

The plan decided against a shared host-side application assembly. That decision was made on three
reasons which have all since stopped holding: it claimed the simulator's storage differs, which is
untrue; it claimed the assembly was not stable enough to share, but a fourth copy has since been
written by hand and is driven by its own tests in about twenty lines; and it claimed sharing would
force a change to `HostLoop::execute`, which has since been split into `serve_effects` and a free
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
| Card and fault injection | `obc_storage::flat::{BlockDevice, SparseDisk, FaultOnce, When}`; the real card in `host/obc-host-core/src/flat_store.rs` |
| Existing protocol harness | `firmware/obc-link/tests/flat_harness/mod.rs` |
| Independent byte oracles | `host/obcm-testkit`, the assembler oracle, `build_obcr`, `SyntheticDem` |
| Screens and goldens | `firmware/ui-snapshots.sh`, `firmware/tools/ui_snapshot_manifest.py` |
| Browser journeys | the existing Playwright setup under `apps/obc-web-demo/tests/browser/` |
| Selection and aggregation | `tools/suite_registry.py` graph and closure code, `tools/ci_aggregate.py` |

## Child plan and dependencies

No child is open yet. Each must carry its own files, commands, acceptance and deletion scope,
refined against current source, before implementation starts.

| ID | Deliverable | Depends on |
| --- | --- | --- |
| TS-0 | **Done in #1817.** Plan document landed under `docs/assets/test-system/implementation/` with every correction above applied in place | — |
| TS-A | Re-take the build measurements at the current shape, then one ordinary integration target per package, with the allocator and `fixtures.*` targets carved by name; local runner on nextest; move `COPERNICUS_ATTRIBUTION` and drop the app's `obc-dem` dev-dependency | — |
| TS-B | Execution routes: fixture-gated and ignored tests assigned, CI job scripts extracted, sweep and builder pytest off the Rust test job's serial path, guard jobs merged, llvm-cov cost measured | TS-A |
| TS-C | Replace registry discovery with the explicit selection plan, including both live `suite_registry.py` call sites; migrate its tests; switch local and CI callers in one cutover; delete the superseded responsibilities | TS-B |
| TS-D | `host/obc-flat-device`: the real engine and store behind a bounded adapter, native plus wasm; port the TypeScript flow suites; remove `MockDevice` | TS-B |
| TS-G | Extract the shared frame seam above into `obc-host-core`; collapse the simulator's local helper and both inlined copies onto it | TS-A; lands before TS-E |
| TS-E | The missing composition checks: assembled map through the card to a rendered App frame; interrupted recording through recovery and GET to GPX; the builder browser assembly and download journey; captured waypoint-bearing provider imports | TS-B, TS-D, TS-G |
| TS-F | Remove dead infrastructure, finish policy and release routes, update contributor and testing documentation | TS-C, TS-D, TS-E |

TS-G is a production refactor, not a test change, and carries its own acceptance: every host renders the same frames it rendered before, and the shared piece has no per-host switch. TS-A and TS-B are the measured wins and carry no design risk. TS-C is the only step that changes how
CI decides anything; it lands as one cutover, never as two selectors running side by side.

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
measured machine; a leaf-package pull request within 3 minutes on a warm CI cache; ordinary selected
jobs within 6 minutes. Record compile, execution and setup separately. A missed target is examined
at its measured bottleneck, never met by weakening assertions or promoting work to heavy.

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
| Captured waypoint-bearing provider imports | **Absorbed here**, in TS-E. Retains the #953 obligation: Komoot and bikepacking.com routes with waypoints, through actual import and device presentation, on captured fixtures with provenance. Basic GPX decoding does not satisfy it. |
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
