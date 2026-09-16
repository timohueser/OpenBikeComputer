# OpenBikeComputer test system — plan v3 (corrected)

> **Read this first.** This plan was written against `develop` at `d42e2e3a` on 2026-09-13. It was
> landed in the repository on 2026-09-16, by which time `develop` was 764 commits ahead. The
> corrections below were applied to the text; the epic (#1816) carries the authoritative list and
> the reasons. Struck content is marked **[REMOVED 2026-09-16]** in place rather than deleted
> silently, so a reader can see what stopped being true.
>
> Applied corrections: weather no longer exists in this repository (`ee5424579` removed 435 files
> and 80,732 lines), so every weather route, target and composition check is gone; the coverage
> policy and its baselines shipped and must be kept, not deleted; `tools/loc_ledger.py` is now
> FS11's acceptance command and must be kept; the Linux desktop launch smoke and a Playwright
> browser suite already shipped; the UI sweep is already conditional; `tools/suite_registry.py` is
> load-bearing at runtime in CI. All measured figures quoted below predate a 27% shrink in
> `obc-app`'s test targets and must be re-taken before they are cited again.


Date: 2026-09-13. Evidence base: develop `d42e2e3a`. This replaces v2 as the proposed implementation plan. It incorporates the independent review and makes the design choices below; it is not an implementation or a claim that new checks have passed. [HANDOVER.md:12](docs/assets/test-system/implementation/README.md:12) [review-codex.md:3](docs/assets/test-system/implementation/reviews/review-codex.md:3)

**Build fewer test programs. Run the affected consumers. Keep the tests that already prove useful behavior. Add complete flows at the real product boundaries.** Do not build a universal testing framework.

## 1. The design, settled

1. Keep inline unit tests and existing App scenario tests. Do not relocate a fixed number of tests or add App seeding APIs to make relocation possible. **[PARTLY SUPERSEDED 2026-09-16: the owner settled that the provably identical frame seam is extracted into `obc-host-core`. See #1816. The rest of this decision stands: no relocation quota, no App seeding APIs.]**
2. Consolidate ordinary Rust integration tests into one target per package. Separate targets are allowed for a different execution route or required isolation.
3. Use Cargo's graph for Rust selection. Keep only cross-language, tool, fixture, and job relationships in a small explicit Python map.
4. Run the same repository scripts locally and in CI. Preserve useful `obc` entry points; remove taxonomy commands when their replacement lands.
5. Keep `HostLoop`, CoreHarness, Frames, Planner, and the focused refusal helpers where their distinct behavior is needed. Share identical fixture-building or byte-buffer helpers only when that removes actual duplication.
6. Add one reusable protocol test device: the real flat engine and store, with a native Rust interface and a wasm binding for the existing TS tests and browser harness.
7. Keep the existing screenshot script, PNG encoding, manifest, and checks. Give it one explicit final-head execution route. Do not rewrite it as a Rust framework in this epic.
8. Add the missing map, storage/ride and browser composition checks in existing owning packages. No new general system-test crate. **[CORRECTED 2026-09-16: weather removed from the repository; the Linux desktop smoke already shipped.]**
9. Delete expired or redundant scaffolding in separate, bounded changes. Keep uncertain deletion candidates until their evidence exists.
10. Keep hardware acceptance in its current issues. Do not extract board predicates, add a hardware CI slot, or build a native upload sender in this epic.

Why these choices: consolidation addresses the measured compile/launch cost without rewriting assertions. A normal HostLoop answers effects, while several existing tests deliberately delay or refuse them. The simulator uses folder stores; the memory ride stores do not retain recording bytes. A universal Device would either grow extra behavior or change the tests. [HANDOVER.md:43](docs/assets/test-system/implementation/README.md:43) [host/obc-host-core/src/dispatch.rs:402](host/obc-host-core/src/dispatch.rs:402) [firmware/obc-app/src/app.rs:4340](firmware/obc-app/src/app.rs:4340) [firmware/obc-app/src/app.rs:7222](firmware/obc-app/src/app.rs:7222) [apps/obc-sim/src/main.rs:754](apps/obc-sim/src/main.rs:754) [host/obc-host-core/src/stores.rs:53](host/obc-host-core/src/stores.rs:53)

### What v3 removes from v2

| Removed requirement | Replacement |
|---|---|
| ~~New App-level `Device`, `testing` feature, and public route/recovery seeds~~ **[PARTLY SUPERSEDED 2026-09-16]** | No universal Device and no seeds, as written. But the identical render pair, render-on-demand predicate, route re-open and hold-cancel consumption are extracted into `obc-host-core`; see #1816 |
| Estimated 150 scenarios moved and 100 reclassified | No relocation quota; move a test only when its new home is clearly simpler |
| Retargeting app helpers to host-core | App helpers stay host-independent; no app → host-core dev-dependency |
| In-process sweep and a new scenario table | Current script, isolated execution route, same PNGs and manifest |
| All goldens use a new environment-variable convention | Existing explicit check/update commands; verification never regenerates |
| New vectors manifest generator and timing-diff utility | Existing vector checks and native test reports |
| Board predicate extraction and future HIL scaffolding | Existing host tests, board build/resource gates, separate physical acceptance |
| Per-target Swift selection and method-pruning project | Existing package suite; focused CI caching; uncertain deletions deferred |
| A mandatory selector size or net LOC forecast | Remove redundant responsibilities, then report the actual diff |

These removals address the concrete cycle, store, sweep, and relocation problems in the independent review. CoreHarness and the current Swift scripted links remain intentional seams. [review-codex.md:9](docs/assets/test-system/implementation/reviews/review-codex.md:9) [review-codex.md:17](docs/assets/test-system/implementation/reviews/review-codex.md:17) [review-codex.md:34](docs/assets/test-system/implementation/reviews/review-codex.md:34) [review-codex.md:81](docs/assets/test-system/implementation/reviews/review-codex.md:81) [plan-v2.md:172](docs/assets/test-system/implementation/plan-v2-superseded.md:172) [plan-v2.md:200](docs/assets/test-system/implementation/plan-v2-superseded.md:200)

## 2. What runs, and how to run it

A test's location is not a taxonomy to enforce. Unit tests check local logic; system tests exercise a composition. Both belong in the normal affected run when they are bounded and deterministic.

There are three execution routes:

| Route | Contents | Entry point |
|---|---|---|
| Ordinary verification | Unit, component, contract and bounded system tests, plus selected captured-fixture tests | Scoped package command, affected plan, or full ordinary CI |
| Explicit expensive or special verification | Heavy hermetic tests; final-head UI sweep; artifact generation as separate named commands | Named recipe or workflow, never implicit in ordinary verification |
| Physical acceptance | Board timing, power/reset, real USB/BLE, and phone behavior | Existing issue-specific local procedures |

Heavy tests and the UI sweep are distinct named operations. Generators and live-service probes are not heavy tests. They never run merely because someone requested all expensive tests.

### Local command contract

The following is the final interface after the selector cutover; until then, use the existing commands from CONTRIBUTING.

| Command | Defined behavior |
|---|---|
| `obc test -p PACKAGE` | That root-workspace package's ordinary nextest targets, plus its doctests. No reverse dependencies. Includes its bounded captured tests after explicit fixture sync. |
| `obc test -p PACKAGE --lib` | Only that package's library test target. No integration targets or doctests. Error if it has no library. This is the app unit-loop measurement command. |
| `obc test -p PACKAGE FILTER` | Local investigation only; nextest name filter. Before handoff, run the affected whole suites. No filtered CI gate. |
| `obc test affected --base REF [--dry-run]` | One plan for changed files, reverse-dependent packages, and non-Cargo consumers. Execute runnable jobs in prerequisite order. |
| `obc test full [--dry-run]` | Full ordinary verification, including captured PR tests; excludes heavy, sweep, generators, live probes, and physical procedures. |
| `obc check JOB` | Run the same script as the named CI job. Keep this useful scoped entry point. |
| ~~`obc test heavy NAME`~~ | **[REMOVED 2026-09-16: there is no heavy recipe list. The repository has three `#[ignore]` tests (two Copernicus downloads, one captured-source check) and five `manual.*` Cargo-example commands; the exhaustive render suite this row was written for died with weather. Those items keep their existing explicit commands and `test-weekly.yml`.]** |
| `obc check sweep` | Current UI sweep and manifest check on the final rendering head; see §6. |
| `obc fixtures sync PROFILE` | Explicit fixture preparation. Tests never start a download. |

Print the selected packages/jobs, reasons, exact commands, and fixture requirements before execution. A missing fixture or tool fails with the exact setup command. Do not automatically substitute a narrower test command after failure.

A local run cannot perform every platform's checks. Preflight the whole plan, print unsupported jobs as **NOT RUN: requires PLATFORM**, execute the supported jobs, and return a distinct nonzero incomplete status if any required jobs remain. Never print “full verification passed” for a partial host run. CI assigns each job a supported runner and must complete the whole selected plan. Existing policy already distinguishes platform skips from passes. [docs/testing.md:121](docs/testing.md:121)

Doctests remain explicit because nextest does not replace their current Cargo step. Keep the separate default-feature `obc-formats` test when formats or a consumer that can unify its codec feature is selected. Do not call a multi-package Cargo invocation the device feature shape. [ci.yml:284](.github/workflows/ci.yml:284) [ci.yml:285](.github/workflows/ci.yml:285) [ci.yml:289](.github/workflows/ci.yml:289) [review-build.md:7](docs/assets/test-system/implementation/reviews/review-build.md:7)

## 3. Rust test layout and build cost

### Consolidate without changing assertions

For each affected root-workspace package:

- Keep its library and binary unit tests in place.
- Move ordinary external test modules under `tests/cases/`, with `tests/main.rs` declaring them. Keep shared external support under `tests/common/`.
- Remove the old top-level test source files after moving them. Do not leave them as auto-discovered targets beside `main.rs`.
- Update module imports, including `common::` references. Preserve test bodies, seeds, payload sizes, assertions, and relevant cfg attributes during this mechanical step.
- Preserve independent handwritten-byte oracles. Do not replace them with a call to the production writer under test.
- Keep `obcm-assemble`'s allocator measurement in its own target. Carve the `fixtures.*` suites that select with `--test <name>` as named targets so their routes survive. Use nextest locally and in CI for merged targets with process-global state. **[CORRECTED 2026-09-16: the five Rust fixture targets are `[[test]] required-features = ["external-fixtures"]` entries, which is a target attribute and cannot merge; `firmware/obc-app/tests/overlay_plane.rs` swaps the panic hook and must run serially inside a merged binary; consolidation applies to the seven packages with five or more targets (75 of 101 binaries), the rest are left as they are.]**

The previous build review identifies the allocator and panic-hook constraints. The measured app comparison was 12.7 seconds of build for the full package versus 2.9 seconds for the library plus one selected integration target. That is evidence for fewer binaries, not a measured result for the final merged suite. [review-build.md:16](docs/assets/test-system/implementation/reviews/review-build.md:16) [HANDOVER.md:43](docs/assets/test-system/implementation/README.md:43)

Do not add `obc-host-core` to `obc-app` dev-dependencies. `obc-app/tests/main.rs` continues to run external App tests. The in-crate support include can stay while inline and external tests both consume it; one shared source file is preferable to a new dependency cycle. [firmware/obc-app/tests/common/mod.rs:8](firmware/obc-app/tests/common/mod.rs:8) [host/obc-host-core/Cargo.toml:12](host/obc-host-core/Cargo.toml:12)

### Separate execution from compilation honestly

Ordinary Rust execution uses `cargo nextest run --locked -p ...`, with a target-level filter excluding named heavy targets and the captured-fixture targets. Package flags narrow compilation. Target filters narrow execution; excluded targets in a selected package can still compile. This plan accepts that residual compile cost rather than inventing a feature system to eliminate it. [review-build.md:10](docs/assets/test-system/implementation/reviews/review-build.md:10)

Reserve `heavy_*` integration-target names for heavy hermetic suites. A captured-fixture target is named `captured`; its owning job selects it explicitly. Do not select individual test functions in CI. Any existing ignored library probe admitted to the heavy route must have a recipe for its whole ignored-library suite and an audit that no generator or network test shares that selection.

Remove `external-fixtures` only after each gated test has an ordinary, captured, or explicit maintainer route. Bounded fixture tests become unconditional and fail with a sync hint when missing. **[CORRECTED 2026-09-16: today `host/obc-fixtures::file()` prints the hint only under `OBC_REQUIRE_FIXTURES`; all 16 call sites `.expect()` its `None`, so a plain local run panics without the hint. Make `file()` always fail with the sync command and drop the env variable. `sim-peak-view` (103 MB) is in the `test` profile but no test reads it; drop it from that profile.]** Generators and live downloads stay explicitly invoked and excluded. The existing feature-gated and ignored tests mix these purposes, so deletion of the flags must not become a blanket `--ignored` run. [firmware/obc-route/tests/nav.rs:1066](firmware/obc-route/tests/nav.rs:1066) [host/obc-dem/tests/decode.rs:35](host/obc-dem/tests/decode.rs:35)

### Dependency cuts

Move `COPERNICUS_ATTRIBUTION` to `obc-elevation`, update its production and test references, and remove the app's `obc-dem` dev-dependency. This is an explicit small production change for dependency cost. For route tests, replace the packer's writer/model use with the existing handwritten testkit while preserving the named navigation/ascent assertions. Do not create `obcm-write` in this epic. The assembler oracle keeps its full packer dependency because comparing independent production paths is its purpose. [firmware/obc-app/Cargo.toml:41](firmware/obc-app/Cargo.toml:41) **[CORRECTED 2026-09-16: the app's only `obc-dem` use is one `#[cfg(test)]` unit test in `src/screen/settings/about.rs`; `obc-elevation` is already a regular dependency. In `obc-route`, only `tests/nav.rs` and `tests/detour.rs` use the packer, and four of those sites call `obc_pack::nav::integrate_edge_ascent`, which is ascent logic under test, not a writer: move those assertions to `obc-pack` rather than re-implementing them in the testkit.]** [review-build.md:13](docs/assets/test-system/implementation/reviews/review-build.md:13) [host/obcm-assemble/Cargo.toml:60](host/obcm-assemble/Cargo.toml:60)

## 4. One selector, explicit cross-language connections

### Representation

Replace the Rust suite rows and workflow-command regex discovery with `tools/test_plan.py`. It contains the selection algorithm and a small job table. Keep execution scripts under `tools/ci/`; keep aggregate evaluation separate and small. No new TOML registry, generated workflow system, source-history database, result converter, or persistent timing service.

The job table holds only facts Cargo cannot provide: job ID, script, supported platform, prerequisite jobs/artifacts, extra path triggers, and explicit package-to-product roots. Heavy recipes are a separate small named table in the same module. Do not copy Cargo package lists, dependency edges, test counts, or timings into it.

Use a plan object containing the head revision, changed paths, root-workspace package set, selected job IDs, and reasons. Scripts receive package arguments as an argument array. No shell evaluation of a generated command string. The same plan object feeds local execution and CI.

### Selection algorithm

1. Obtain tracked changes relative to the merge base. Locally include staged/unstaged tracked changes and detect relevant untracked source files; in CI use the PR base/head. Handle both paths of renames and deletions.
2. Read Cargo metadata for the root workspace and the board, boot, and desktop roots. Include normal, build, and dev-dependency edges. Keep workspace membership so standalone packages never become root `-p` arguments. Use unioned base/head ownership when deletions or moves need it.
3. Map changed Rust sources to their owning packages and take the reverse-dependency closure. Select only existing head packages for compilation. A manifest/lockfile/toolchain change selects the full relevant build and ordinary-test graph; a root Cargo change selects all Rust product roots.
4. Add explicit non-Cargo consumer edges from the table below. Then close selected jobs over prerequisites. A selected consumer always has its artifact producer selected.
5. Validate ownership and routes before execution. Unknown source roots, missing scripts, unknown jobs, or missing package routes are errors. A recognized directory alone is not proof that all consumers were selected.
6. Deduplicate packages/jobs, sort deterministically, and print the reasons. An empty root package set means no Rust invocation, never an implicit whole-workspace invocation.

Reuse the existing multi-manifest graph and closure code where useful. The current implementation already records workspace membership and job prerequisites; preserve those properties while removing command parsing. [tools/suite_registry.py:620](tools/suite_registry.py:620) [tools/suite_registry.py:633](tools/suite_registry.py:633) [tools/suite_registry.py:914](tools/suite_registry.py:914) [tools/suite_registry.py:941](tools/suite_registry.py:941)

### Required non-Cargo edges

These are normative routes. Preserve any existing extra trigger not explicitly retired in the cutover; account for it in the cutover review before deleting its registry row.

| Change | Required consumers |
|---|---|
| Rust package | Its ordinary tests and reverse-dependent ordinary suites; fmt/clippy; any reached product-root jobs below |
| Board/boot dependency closure or their sources/config | Their standalone build/resource jobs, plus reached host contracts |
| Desktop dependency closure, Rust source, or desktop config | Desktop build/tests; Linux launch smoke after §5 lands; desktop frontend and wasm artifacts as prerequisites |
| Web-convert, web-assemble, skin-preview, or flat-device closure | Corresponding wasm build and web contract/browser tests; desktop when the changed producer is shipped by desktop |
| Web-demo closure or Trunk inputs | Web-demo wasm build and relevant docs artifact checks |
| `builder/app/` source/config/package files | Web verification and wasm prerequisites; desktop frontend/build for shared frontend code; no Python builder job solely for a TS-only edit |
| `builder/server/`, presets, Python requirements/tests, or packer closure | Builder Python contracts, including the real packer where required |
| `specs/` contracts and vectors | Rust contract consumers, Swift package tests, web contracts/browser smoke, and affected generated artifact checks; use conservative all-contract-consumer routing initially |
| Fixture catalog, loader, or source inputs | Their consumers in Rust, web, Python, and Swift where applicable; fixture policy; heavy remains separate |
| `companion-ios/Packages/OBCKit/` sources or manifest | Existing Swift package suite and iOS app build |
| iOS app composition/project files | iOS app build; existing Swift package suite for composition dependencies |
| Python tools/services | Their existing owning unittest/pytest suites; downstream artifact consumers where applicable |
| Rain-radar source | Its existing Vitest suite |
| Docs prose only | Docs/link checks; generated product inputs under docs additionally select their real consumers |
| Lockfiles, license tooling, `about.toml`, third-party notice | Relevant deny checks and the separate third-party license generation check |
| Test runner, job scripts, workflow, selection policy | Full relevant ordinary verification; never automatic physical work or generators |
| Agent prose only | Policy/docs validation; do not build every platform merely because CLAUDE.md or AGENTS.md changed |

Cargo cannot derive the vector→Swift/web connections. These triggers exist today and must survive. Preserve license generation separately from dependency denial. [testing/suites.toml:533](testing/suites.toml:533) [testing/suites.toml:634](testing/suites.toml:634) [testing/suites.toml:811](testing/suites.toml:811) [docs/testing.md:224](docs/testing.md:224)

### CI jobs and ownership

Keep the current build/resource commands and target matrices unless a row explicitly changes them. Extract commands to scripts without adding a wrapper around every shell line.

| Job/group | Responsibility |
|---|---|
| `selection` | Compute and validate the plan; publish package and job sets. Keep this job short. |
| `guards` | Run existing ownership/dependency/fixture guards and their tooling tests. Merge jobs, not guard implementations. Keep the named scripts and useful failure messages. |
| `fmt`, `clippy`, `deny` | Existing formatting/dependency checks; clippy uses affected root packages, standalone roots retain their own checks. |
| `test` | Affected ordinary Rust nextest targets, doctests, default-shape formats check when selected, existing bench pixel/read-counter golden check when its package closure is selected. |
| `builder-python` | Existing native suite with its own dependency triggers. Builder preflight must fail if its required binary/corpus is absent, not pass by skipping tests. |
| `embedded`, `boot`, `device`, `wasm` | Existing board/boot/device-shape/wasm compilation and exact resource gates. Preserve sensors/display coverage through their host suites and device builds. |
| `wasm-bridges` | Build required bridge artifacts once; enforce existing shipping size budgets; publish the exact artifacts to web/desktop consumers. |
| `web` | Svelte check, Vitest, `build:all`, bundle guard against those outputs, and the small browser smoke. No nested bundle-guard builds. |
| `desktop` | Linux build and native tests on affected PRs; launch smoke after it lands. macOS/Windows build legs on develop push. Consume desktop frontend artifacts from the web producer. |
| `ios-unit`, `ios-app` | Existing Swift package suite; app build and mock-boundary check. Cache SwiftPM/DerivedData. Screenshot asset regeneration stays an explicit maintainer operation. |
| `docs`, `licenses` | Existing docs/link and third-party-notice checks. |
| `ci` | Fail unless selection and every planned ordinary job succeeded. Print job/result, not invented per-test counts or wall times. |

Do not add guard rule-table infrastructure: merging existing guard invocations removes runner setup duplication without translating their logic. Keep the bench golden in the normal Rust job to reuse build state; use its current release profile initially, with no unmeasured profile conversion. Move the sweep and builder pytest out of that job. Existing commands and artifacts are visible in the workflow. [ci.yml:297](.github/workflows/ci.yml:297) [ci.yml:304](.github/workflows/ci.yml:304) [ci.yml:313](.github/workflows/ci.yml:313) [ci.yml:321](.github/workflows/ci.yml:321) [ci.yml:335](.github/workflows/ci.yml:335) [ci.yml:787](.github/workflows/ci.yml:787)

The aggregate fails on missing/malformed selection, selected-but-missing, skipped, failed, or cancelled jobs. An unselected job is “not selected,” never “passed.” A build failure stays a failure even without a JUnit file. Keep native report uploads and ordinary console output; use the CI UI for durations. [tools/ci_aggregate.py:78](tools/ci_aggregate.py:78) [tools/ci_aggregate.py:100](tools/ci_aggregate.py:100) [docs/testing.md:154](docs/testing.md:154)

Develop pushes run the full ordinary package set and populate shared caches. PR jobs may restore it, but every job still declares/builds its required shape; caches are performance aids, not evidence. Keep each producer's profile/target/features consistent with consumers, and key separate shapes where necessary. Splitting jobs can lower elapsed time while increasing runner minutes; measure the implemented change rather than promising a net saving. [review-build.md:19](docs/assets/test-system/implementation/reviews/review-build.md:19)

### Selection tests that must survive

Reuse the existing exact-set change-class tests. Add only the missing classes needed by this design: renamed/deleted package paths, standalone reverse dependencies, wasm producer→browser, shared vectors→all clients, empty package selection, unknown ownership, and artifact prerequisite closure. Keep aggregate tests for missing plans and failed/skipped/cancelled selected jobs. One test module may contain table-driven cases; there is no “one test only” restriction. [tools/tests/test_suite_registry.py:789](tools/tests/test_suite_registry.py:789) [tools/tests/test_suite_registry.py:821](tools/tests/test_suite_registry.py:821) **[CORRECTED 2026-09-16: the exact-set change-class tests are `ShippedRoutingTests::test_every_suite_routes_to_the_job_that_executes_it` and `test_selected_job_set_per_change_class`; the earlier line numbers pointed at the two `gate_claims` tests.]** [tools/tests/test_ci_aggregate.py:50](tools/tests/test_ci_aggregate.py:50)

Use a single parsed-YAML structural check, not regex command walkers: all declared ordinary jobs have a workflow route; each conditional job gates on its own ID; all required jobs reach the aggregate; dependencies agree with the explicit prerequisite map. Declare PyYAML as a pinned test-only dependency in `tools/requirements-test.txt`; install it in tooling setup locally and in the guards job. Selection itself stays standard-library-only. Do not write a YAML parser. This checks routing structure, not the meaning of arbitrary shell commands.

Do the cutover in one bounded change once scripts and target splits are stable: adapt the table-driven tests, switch local and CI entry points together, delete superseded registry/parser code, and update documentation. Do not keep two production selectors running indefinitely.

## 5. System coverage with existing seams

Keep all existing crash matrices, conformance assertions, dirty/no-repaint checks, and independent oracles. A helper stays when it expresses a meaningful difference in execution. A test can stay inline and still be a valuable composition test. The existing population is 255 App/harness test definitions plus 34 pass tests; neither number is a relocation target. [review-codex.md:81](docs/assets/test-system/implementation/reviews/review-codex.md:81)

Add these bounded checks. Each has an owner, an input, an observable result, and a route.

| Check / home | Implementation and acceptance |
|---|---|
| Assembled map through card to App / existing assembler oracle | Reuse the oracle's packed/assembled output. Add host-core and App as dev-dependencies, mount through the existing flat-map adapter, and use its `ObjectSource` (`obc_host_core::flat_store::ObjectSource`; there is no `StoreSource`) for the App's reader. Render a map screen at a nonempty known viewport. Compare against the direct-source App frame and assert positive map reads/content; an empty Home frame cannot pass. Keep the existing routing oracle. Ordinary assembler suite. |
| Interrupted recording through recovery and GET / link engine suite | Reuse the real sparse card, journal, fault plan, remount, and engine GET helpers. Journal a valid ride-v3 object, then cut finalization after its tail reaches the payload and before the catalog commit. Reboot, assert the recovered recording, retry the amend commit, and GET the finalized ride. Compare its samples/totals with the expected object using the existing ride reader. Export those GET bytes through `obc_route::track_to_gpx` and assert expected points and segment boundaries without duplicates. Add `obc-route` as a dev-dependency; no new exporter or App recovery API. Ordinary link suite. |
| Browser worker/OPFS/client composition / existing dev-harness | Add Playwright and one small browser spec. Use local fixture responses, the real worker/wasm artifacts, OPFS, and the wasm flat device. Assert nonempty valid output and the uploaded object's bytes/digest. Refuse unexpected network access; fail on worker, console/page, or adapter errors. Bound readiness and completion by signals/timeouts, no retries. Ordinary web job. |
| Linux desktop launch / desktop root | Add a narrow `--smoke` mode that launches the real embedded frontend with temporary app data and a loopback fixture catalog. Require a frontend acknowledgement after the real catalog UI has loaded and a native IPC round trip succeeds; then exit successfully. Under xvfb, fail on timeout, premature exit, missing acknowledgement, or errors. A live process alone is not success. Ordinary Linux desktop job. |

The map adapter already has a counted-source/pixel oracle. The flat harness already journals a real recording. The storage retry test provides the cut point and amend sequence; the route crate supplies GPX export. [firmware/obc-storage/src/flat/crash.rs:1018](firmware/obc-storage/src/flat/crash.rs:1018) [firmware/obc-route/src/track.rs:28](firmware/obc-route/src/track.rs:28) Reuse those pieces rather than adding a cross-product runner. [host/obc-host-core/src/flat_map/tests.rs:24](host/obc-host-core/src/flat_map/tests.rs:24) [host/obc-host-core/src/flat_map/tests.rs:63](host/obc-host-core/src/flat_map/tests.rs:63) [host/obcm-assemble/Cargo.toml:60](host/obcm-assemble/Cargo.toml:60) [apps/obc-sim/Cargo.toml:19](apps/obc-sim/Cargo.toml:19) [host/obc-wx-client/src/http.rs:230](host/obc-wx-client/src/http.rs:230) [apps/obc-sim/src/weather_store.rs:190](apps/obc-sim/src/weather_store.rs:190) [apps/obc-sim/src/weather_store.rs:220](apps/obc-sim/src/weather_store.rs:220) [firmware/obc-link/tests/flat_harness/mod.rs:352](firmware/obc-link/tests/flat_harness/mod.rs:352) **[CORRECTED 2026-09-16: the FS8 journal path is `finish_recording` at :352; :348 closes `seed_recording`. `crash.rs:1018` is `finalisation_retry_does_not_rewrite_an_intact_tail`. The recovery in this check is the store's `recovered_ride()`, not `App::offer_recovered_ride`. `obc-link` has no `obc-route` dev-dependency yet and no `ByteSink`; add both, reuse `specs/vectors/ride-v3.bin`, and note `track_to_gpx` emits no `<time>`.]**

For the browser spec, install the browser version supported by the pinned Playwright package in setup and cache its download. Do not assume a mutable hosted-runner Chrome installation matches the test dependency. Keep the browser harness/dev-only engine out of shipped web and desktop entry graphs; the existing bundle guard is the place to enforce that boundary. Use the smallest existing complete fixture; do not reduce existing throughput-test payloads. [builder/app/package.json:24](builder/app/package.json:24) [builder/app/vite.config.ts:5](builder/app/vite.config.ts:5) [plan-v2.md:284](docs/assets/test-system/implementation/plan-v2-superseded.md:284)

The Linux smoke proves Linux frontend/IPC startup without hardware. It does not close #994's Windows and real-device upload requirements. The new browser smoke is not a large-input memory measurement. #1503 remains separate, and must distinguish client/worker memory from the sparse simulated card's retained data. [issue-994.md:30](issue issue-994.md:30) [issue-994.md:41](issue issue-994.md:41) [issue-1503.md:3](issue issue-1503.md:3) [firmware/obc-storage/src/flat/sim.rs:99](firmware/obc-storage/src/flat/sim.rs:99)

RideRecovery screen coverage uses the existing typed `App::offer_recovered_ride` / `offer_damaged_ride` APIs in the existing App recovery tests. Add a focused rendered-state assertion there if missing. No public bytes parser or simulator flag is needed for this screen check. [firmware/obc-app/src/app.rs:2220](firmware/obc-app/src/app.rs:2220) [firmware/obc-app/src/app.rs:2243](firmware/obc-app/src/app.rs:2243)

## 6. Screenshots and golden artifacts

Keep `firmware/ui-snapshots.sh`, its current PNG encoder, the sha256 manifest, expected-screen assertions, and duplicate/missing/identical-frame checks. Keep mutable state isolated per invocation. The script uses different maps and prepared ride/route data; this epic does not reimplement that setup. [firmware/ui-snapshots.sh:18](firmware/ui-snapshots.sh:18) [firmware/ui-snapshots.sh:48](firmware/ui-snapshots.sh:48) [firmware/ui-snapshots.sh:61](firmware/ui-snapshots.sh:61) [firmware/tools/ui_snapshot_manifest.py:13](firmware/tools/ui_snapshot_manifest.py:13) [firmware/tools/ui_snapshot_manifest.py:52](firmware/tools/ui_snapshot_manifest.py:52)

### One final-head sweep

The standing budget permits a sweep at most once per PR, on the final head, and only for rendering/screens/i18n work. Therefore remove the unconditional sweep from ordinary CI and do not embed it in a Cargo test target. [CLAUDE.md:69](CLAUDE.md:69) [ci.yml:321](.github/workflows/ci.yml:321)

Provide `obc check sweep` and a separate manual `ui-sweep` workflow running the same script. The workflow accepts a full revision, checks out that exact revision, builds the simulator, renders once, checks the existing manifest, and uploads the PNGs and revision with its result. It has its own status name; it must not overwrite the ordinary `ci` aggregate status with a sweep-only run.

A rendering PR selects one execution venue: local or that workflow. The final review records the successful check and exact head; it does not rerun the sweep. Do not run both venues. If the rendering head changes afterwards, the earlier evidence is stale; follow the standing owner-controlled verification budget rather than silently treating it as current.

For an intentional golden change, render once, inspect those generated frames, and update the manifest from that same output directory. Check the manifest against those same files; this is not another render. CI never updates goldens. A final patch that only records the reviewed manifest can cite the rendered source revision and the manifest-only diff explicitly.

The ordinary CI summary states “UI sweep is a separate final-head review check” where relevant. It must not claim to include visual acceptance. No workflow state tracker, label bot, or per-PR sweep database is added.

Other goldens retain their existing explicit generators. Do not standardize their commands merely for appearance. Test commands check; generator commands write. Never use a blanket ignored-test command that could regenerate multiple artifacts. Preserve independent vector bytes, the existing manifest, and existing consumer checks. Do not claim that Rust currently verifies that manifest; adding a manifest generator is outside this epic. [firmware/tools/ui_snapshot_manifest.py:11](firmware/tools/ui_snapshot_manifest.py:11) [plan-v2.md:317](docs/assets/test-system/implementation/plan-v2-superseded.md:317) [survey-critic.md:71](docs/assets/test-system/implementation/surveys/survey-critic.md:71)

## 7. Heavy, release, and hardware evidence

~~Maintain a small explicit list of hermetic heavy recipes: the existing render exhaustive ignored-library suite; the display timing probe as a measurement recipe; and the retained iOS UI suite on a supported simulator.~~ **[REMOVED 2026-09-16: `firmware/obc-render/src/rain.rs` was deleted with weather; `rowdiff.rs:342` is an ordinary fast test; the timing probe is `firmware/obc-display/examples/row_hash_timing.rs`, already routed as `manual.obc-display`; the iOS application suite already runs weekly through `test-weekly.yml`. There is no heavy list to maintain.]** Preserve their native exit status and results. Do not invent a shared scenario format or a result converter.

The release-heavy workflow runs correctness recipes on the exact release revision on their required platforms before publishing. Make the release publisher depend on successful completion, not merely an artifact from an older branch head. Timing probes produce evidence without unstable shared-runner timing thresholds. Generators, external-service downloads, and physical procedures are excluded. Failures stop publication. Keep existing release version/key/image verification unchanged. [release.yml:3](.github/workflows/release.yml:3)

No new cron is added; the existing weekly route stays. **[CORRECTED 2026-09-16: the live weather freshness probe was removed with the weather subsystem.]**

No board source changes are required by this plan. Keep existing board resource checks and host-side driver/display tests. Physical reset, watchdog, wake, stack, USB/BLE, and phone evidence remains in #1262/#1393/#994 and their existing procedures. Do not claim that host success substitutes for these observations. Cable and BLE sessions remain separate. [issue-1262.md:9](issue issue-1262.md:9) [issue-1262.md:109](issue issue-1262.md:109) [issue-994.md:41](issue issue-994.md:41) [plan-v2.md:152](docs/assets/test-system/implementation/plan-v2-superseded.md:152)

The future smoke-upload sender, board predicate extraction, and generic Swift-over-real-BLE suite are follow-ups only when a concrete acceptance task needs them. Preserve the Swift `TransferLink` boundary and scripted actors now. [plan-v2.md:200](docs/assets/test-system/implementation/plan-v2-superseded.md:200) [plan-v2.md:389](docs/assets/test-system/implementation/plan-v2-superseded.md:389)

## 8. Protocol device: the only new reusable test library

Create `host/obc-flat-device` for the real `obc-link` engine over `obc-storage`'s simulated card. It has a native Rust library surface and target-specific wasm bindings. It has no App, UI, host-loop, or phone-process dependency.

Share the engine/store assembly with the existing Rust flat harness through a native-only `obc-link` dev-dependency on `obc-flat-device`. Keep this out of normal/build dependencies and enable wasm bindings only for wasm targets. This follows the existing test-only adapter edge to `obc-storage`; the production dependency guard excludes dev-dependencies. [firmware/obc-link/Cargo.toml:21](firmware/obc-link/Cargo.toml:21) [firmware/tools/check_dependencies.py:57](firmware/tools/check_dependencies.py:57) Keep handwritten request builders and Rust-only assertion conveniences in the native test support. Native protocol tests must not require a JS runtime. The existing harness already separates the engine/store from client-built bytes, and the existing web-assemble crate shows the native-library/wasm-binding packaging pattern. [firmware/obc-link/tests/flat_harness/mod.rs:5](firmware/obc-link/tests/flat_harness/mod.rs:5) [firmware/obc-link/tests/flat_harness/mod.rs:109](firmware/obc-link/tests/flat_harness/mod.rs:109) [apps/obc-web-assemble/Cargo.toml:8](apps/obc-web-assemble/Cargo.toml:8) [apps/obc-web-assemble/Cargo.toml:33](apps/obc-web-assemble/Cargo.toml:33)

### Required adapter behavior

- Accept complete control/stream records and return one bounded engine reaction at a time. The existing TS pipe continues to handle packet slicing/reassembly and asynchronous channel capacity.
- Wait for channel capacity before polling more stream output. Keep control/cancel input serviceable while a stream write waits. Serialize access to the engine; do not drain an entire download into a vector before giving JS a turn.
- Map `Send`, `Close`, and `SendAndReboot` explicitly. A wasm trap, channel failure, or adapter exception fails the test.
- Model link-up, link-down, and reboot independently. Browser unplug calls link-down only; the Rust helper that immediately reconnects is not the unplug implementation.
- Allow card faults at actual storage operations, preserve the durable card, and recreate mounted store/engine state for reboot. Do not mimic power loss by merely clearing the JS connection.
- Keep expected protocol refusals separate from unexpected adapter failures. Preserve request logging only where existing tests assert sent/unsent commands.

These rules preserve the current cancellation and unplug tests. The existing Rust convenience pump accumulates records, and `link_lost()` reconnects immediately, so exposing those helpers directly would be incorrect. [firmware/obc-link/tests/flat_harness/mod.rs:229](firmware/obc-link/tests/flat_harness/mod.rs:229) [firmware/obc-link/tests/flat_harness/mod.rs:280](firmware/obc-link/tests/flat_harness/mod.rs:280) [builder/app/src/lib/usb/client.test.ts:454](builder/app/src/lib/usb/client.test.ts:454) [builder/app/src/lib/device/flows.test.ts:204](builder/app/src/lib/device/flows.test.ts:204)

### Existing mock options, decided

| Existing behavior | Replacement |
|---|---|
| Store ID / sequence / seed IDs | Format and mutate the real card. Tests use returned identities and before/after sequence assertions. |
| `cardBytes` as payload allowance | Valid card geometry plus occupied extents. Compare before/after catalog state and preserve no-space required-byte assertions; do not invent an invalid tiny real card. |
| Paged LIST / stream payload | Real record ceilings. Preserve pagination/cursor properties, adapting exact page counts only to the actual ceiling contract. |
| Unformatted/unreadable catalog | Blank card or real corruption of the relevant stored copies; verify the intended mount state. |
| ARM allowed | A small explicit test Policy; default remains refusal. This proves protocol behavior, not board signature/battery/update policy. |
| Device information | Small vendor-lane response stub; no simulated device policy. |
| `faults` | Unexpected adapter failures propagate to the test; not a counter of expected protocol errors. |
| `requestLog` | Retain command tracing required by existing negative assertions. |
| `sinkUploads` | No replacement in normal flows. Large-client-memory evidence is separately scoped under #1503. |

The TS capacity, seed, and fault meanings are defined in loopback.ts; the real card is block/extent based. OpenPolicy accepts payloads and refuses ARM by default. [builder/app/src/lib/usb/loopback.ts:316](builder/app/src/lib/usb/loopback.ts:316) [builder/app/src/lib/usb/loopback.ts:372](builder/app/src/lib/usb/loopback.ts:372) [builder/app/src/lib/usb/loopback.ts:400](builder/app/src/lib/usb/loopback.ts:400) [firmware/obc-storage/src/flat/layout.rs:82](firmware/obc-storage/src/flat/layout.rs:82) [firmware/obc-link/src/flat/store.rs:184](firmware/obc-link/src/flat/store.rs:184)

Build it first with wasm-pack and run a native engine test target plus a minimal existing TS round trip. Then retarget the existing flow suites. **[CORRECTED 2026-09-16: the loopback device drives about 157 tests across eight files (`client`, `webusb`, `desktop/usb`, `rides`, `flows`, `library`, `dashboard`, `manage`) plus the builder dev harness, not only `flows` and `client`. `loopbackLink` and its channels stay as the transport substrate under the fake `USBDevice` / Tauri bridge tests; only `MockDevice` is replaced. Three semantic differences need an owner ruling before the adapter is written: the real engine answers `busy` to a full reservation table where the mock answers `noSpace` with a byte context; object ids are assigned by the store and cannot be chosen by `seed`; a hung-device timeout and the `requestLog` assertions need two explicit adapter hooks.]** Keep MockDevice during the bounded migration only; remove it when all its consumers use the bridge. Do not retain a permanent backend-selection flag. A failed port must be resolved by preserving the tested behavior or documenting a real fake/protocol discrepancy, not by dropping the assertion.

Use the same generated wasm package in Vitest and the browser harness. Build once in CI and pass the artifact to consumers; local setup uses the same build script. Add the new module to both existing wasm entry points by making the npm entry point call the repository bridge script, not by maintaining a second command list. Keep the test-device module out of shipping bundles. [builder/build-wasm-bridges.sh:7](builder/build-wasm-bridges.sh:7) [builder/app/package.json:12](builder/app/package.json:12)

## 9. Deletions and scope limits

Do not make cleanup depend on harness relocation. Each deletion carries its own source removal, command/reference cleanup, and focused validation.

| Delete in this epic | Preserved condition / evidence |
|---|---|
| ~~LOC ledger/report tools~~ **[CORRECTED 2026-09-16: keep `loc_ledger.py`]** | It is now FS11's acceptance command (`--storage-total --check-budget`, #1786/#1787). Only the per-merge delta reporting and `loc_report.py` may go, with FS11's owner agreeing. Keep `obc clean` and `obc docs`. The supplied census counts 1,657 tool/test lines, excluding recipe edits. [review-loc.md:13](docs/assets/test-system/implementation/reviews/review-loc.md:13) |
| Expired s6b soak and its tests | Death trigger met in the supplied evidence. Keep physical acceptance in its owning issues; Link is recoverable from git. Supplied count: 1,233 lines. [review-loc.md:10](docs/assets/test-system/implementation/reviews/review-loc.md:10) |
| ~~Nowcast reporting/cost scaffolding~~ **[ALREADY DONE 2026-09-16]** | Banked by the weather removal. [review-loc.md:44](docs/assets/test-system/implementation/reviews/review-loc.md:44) |
| `feeders.rs` migration table | Keep the independent trace-based feeder coverage gate. [review-loc.md:19](docs/assets/test-system/implementation/reviews/review-loc.md:19) |
| Closed-issue BLE central repro binary | Preserve flat-store bench and wiring diagnostic tools. [plan-v2.md:302](docs/assets/test-system/implementation/plan-v2-superseded.md:302) |
| Registry duplication and workflow regex walkers. **[CORRECTED 2026-09-16: the coverage policy is no longer a scaffold — TS5 shipped real ratchets, `testing/coverage-baseline.json` and `tools/coverage_report.py`. Keep all of it.]** | Only after the new routes and tests are active; update the policy/issue wording so no removed gate is claimed as completed. [plan-v2.md:258](docs/assets/test-system/implementation/plan-v2-superseded.md:258) [plan-v2.md:333](docs/assets/test-system/implementation/plan-v2-superseded.md:333) |
| TS MockDevice | Only after all native/TS/browser consumers and failure semantics pass through the real engine. §8 is the deletion gate. |

Defer these uncertain or independent changes: bench_ingest and its board ingest mode; display_test; XCUITest method pruning; OBCMock pacing tests; S0 envelope/buffer removal; Swift legacy product read paths; resource-baseline prose cleanup; production packer split; vector generation redesign. They have extra evidence, product-wire, or method-level review requirements that are unnecessary for the new test system. Do not count them as v3 savings. [review-loc.md:10](docs/assets/test-system/implementation/reviews/review-loc.md:10) [review-loc.md:25](docs/assets/test-system/implementation/reviews/review-loc.md:25) [review-loc.md:28](docs/assets/test-system/implementation/reviews/review-loc.md:28) [review-loc.md:38](docs/assets/test-system/implementation/reviews/review-loc.md:38) [review-loc.md:40](docs/assets/test-system/implementation/reviews/review-loc.md:40) [plan-v2.md:297](docs/assets/test-system/implementation/plan-v2-superseded.md:297)

Keep `dirty.rs` and its no-repaint/polling assertions. Keep the sha256 and throughput-shaped web tests intact. These are coverage, not cleanup candidates. [plan-v2.md:284](docs/assets/test-system/implementation/plan-v2-superseded.md:284) [plan-v2.md:307](docs/assets/test-system/implementation/plan-v2-superseded.md:307)

No projected net LOC total is promised. Report actual added/deleted/moved test and infrastructure lines in each implementation PR using the existing tracked-file, brace-matched convention. Do not add a ledger or manufacture deletions to meet a target. The supplied baseline is approximately 165k test/infrastructure versus 285k product lines under that convention; it is a census, not an implementation forecast. [HANDOVER.md:48](docs/assets/test-system/implementation/README.md:48) [review-loc.md:7](docs/assets/test-system/implementation/reviews/review-loc.md:7)

## 10. Implementation sequence and gates

Each milestone is a complete change or a small sequence with one clear acceptance boundary. Start from current source, confirm the cited paths still hold, and preserve unrelated work. No milestone requires a new universal harness.

### A. Consolidate binaries and cut the app's attribution dependency

Change test module placement and imports; switch the local runner to nextest; preserve the allocator exception and carve the `fixtures.*` named targets. Keep current selection infrastructure during this step. Move the attribution constant in a separate small commit.

Acceptance: the affected whole package suites and doctests pass; Cargo discovers one ordinary integration target for each consolidated package, plus named exceptions; assertions/payloads unchanged; app still has no host-core dev-dependency. Run affected package clippy and required formatting. Use one final-head app timing against the supplied baseline; do not rebuild the old head. [review-build.md:16](docs/assets/test-system/implementation/reviews/review-build.md:16) [HANDOVER.md:43](docs/assets/test-system/implementation/README.md:43)

### B. Establish execution routes and extract CI scripts

Assign all fixture-gated/ignored tests, remove obsolete fixture features, and make missing fixtures fail. Remove sim-peak-view from the test sync profile only after confirming no selected test consumes it. Update existing registry commands and target ownership for these splits while that registry is authoritative. Extract job scripts and feed the current selector's affected root package set into nextest. Move builder pytest and the sweep out of the Rust test job. Merge guard jobs by invoking existing scripts.

Acceptance: dry-run examples show bounded ordinary, captured, heavy, generator, and physical routes; ordinary execution runs no generator/live test; doctests remain; builder prerequisites cannot skip silently; app `-p` still includes its integration suite. Add no parallel selector. Keep `obc suites check` while the old registry remains authoritative. [plan-v2.md:286](docs/assets/test-system/implementation/plan-v2-superseded.md:286) [ci.yml:284](.github/workflows/ci.yml:284) [survey-critic.md:77](docs/assets/test-system/implementation/surveys/survey-critic.md:77)

### C. Replace registry discovery with the explicit plan

Implement §4's table and algorithm, migrate its tests, switch local/CI callers together, then delete superseded registry and parser responsibilities. Keep a small `obc suites check` entry point for ownership/routing validation under the new planner, so the standing verification command remains useful; remove obsolete list/explain/taxonomy subcommands. No compatibility implementation of the old selector remains.

Acceptance: exact package/job-set cases and aggregate failure cases pass; every existing suite/extra trigger is retained, deliberately retired, or assigned an explicit special route; no unknown path is silently accepted. Validate workflow structure and script existence. Run `obc suites check` under its new implementation. CI, not a duplicate local full run, supplies the cross-platform gate.

### D. Real protocol device and browser smoke

Land the native/wasm assembly and bounded adapter; port existing TS tests; remove MockDevice; add the small browser spec. Add artifact prerequisite and nonshipping-bundle checks at the same time.

Acceptance: native engine suites, existing TS client/flow suites, and browser smoke pass, including cancellation while streaming, unplug/retry, no-space, corrupt payload, paging/stale cursor, replace, and expected refusal cases. No JS runtime in native tests; no whole-download buffering in the adapter; no test device in shipping bundles. Run affected bridge builds and native frontend checks once for the final change.

### E. Fill the remaining product compositions

Add the map and recovered-ride checks in §5, plus the builder browser journey and captured waypoint-bearing provider imports absorbed from #1449 TS6. **[CORRECTED 2026-09-16: the Linux desktop smoke already shipped as `apps/obc-desktop/e2e/launch.py`.]** Implement each against the existing source seam and keep helpers local. Add the required selector edges in the same PR as each new test.

Acceptance: the test asserts the actual output and exercised path, not merely a successful exit or non-null value. Fixture identity, fixed clock, temp storage, and expected error paths are explicit. Linux launch success requires real frontend/native readiness. Record #994/#1503 limitations rather than closing them with partial evidence.

### F. Remove dead infrastructure and finish policy/release routes

Land §9's safe deletions independently, complete cache/job cleanup, wire release-heavy before publication, and update contributor/testing documentation and applicable issue acceptance text. Keep screenshot generation explicit and separate. No new hardware or memory harness is required to finish v3.

Acceptance: no references to deleted commands/files; old coverage/exception policy no longer claims nonexistent enforcement; release tests target the release revision; known physical and memory gaps remain named. Report the actual LOC delta and timing results without a persistent reporting system.

### Verification discipline for every milestone

Use focused whole suites and affected selection, appropriate native checks, and `obc suites check` for test/tool/workflow changes. Format the root workspace and applicable standalone roots. Follow protected public-copy ownership and use a separate docs commit when public docs become stale; run `python3 docs/build_docs.py --check-links` when public docs change. No routine mutant tests, repeated sweep, rebuilt resource base, or full local CI rehearsal. Run wake/profile evidence only for actual wake/scheduling changes—which this plan does not introduce. [CLAUDE.md:41](CLAUDE.md:41) [CLAUDE.md:64](CLAUDE.md:64) [CLAUDE.md:84](CLAUDE.md:84)

If a milestone requires a new production API or broad framework beyond the changes named here, stop expanding that milestone: preserve the existing test seam and file the concrete missing capability separately. A clean build and a smaller diff do not justify weaker assertions.

## 11. Completion criteria and intentional limits

V3 is complete when:

- Scoped and affected commands use the same plan and scripts as CI; unknown ownership and missing selected work fail.
- Ordinary Rust integration targets are consolidated with named exceptions, and the app's focused command still runs its retained integration tests.
- Cross-language/vector/artifact consumers are selected even without a source edit in their own language.
- Existing conformance, failure, storage, vector, dirty, and resource evidence remains intact; the §5 composition checks are live.
- TS device flows and browser smoke use the real protocol engine/store through a bounded adapter; the handwritten MockDevice is gone.
- UI verification has one explicit final-head route and preserved golden semantics. Heavy correctness checks gate the exact release revision; generators/live/physical work cannot run accidentally.
- The obsolete selection/exception scaffolding and the safe cleanup rows are removed; contributor instructions describe the commands that actually exist.

Performance targets, not predictions: app library-only edit-to-result within 10 seconds on the measured machine after warm dependency setup; a leaf-package PR within 3 minutes on the supported CI cache; ordinary selected jobs within 6 minutes. Record compile, execution, and setup separately. A missed target triggers examination of the measured bottleneck, not fewer assertions or arbitrary promotion to heavy. These targets refine v2's proposed acceptance limits. [plan-v2.md:378](docs/assets/test-system/implementation/plan-v2-superseded.md:378)

Intentional limits: no proof of physical board equivalence, no Windows USB acceptance, no Swift-to-firmware runtime bridge, no large-map empirical memory gate, no coverage ratchet, no unified test DSL, no in-process screenshot rewrite. Those are explicit separate work items rather than hidden dependencies of this redesign. [issue-1262.md:109](issue issue-1262.md:109) [issue-994.md:41](issue issue-994.md:41) [issue-1503.md:45](issue issue-1503.md:45)

**Plan-author verification:** source/document inspection and citation/count validation only. No implementation, builds, tests, dependency resolution, branches, worktrees, issue updates, or public site changes were performed while writing v3. Only this plan file was created.
