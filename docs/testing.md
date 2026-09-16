# Testing policy

`tools/test_plan.py` decides what a change must test. Cargo's own graph supplies every Rust
package, every dependency edge and the split between the two test tiers, so none of that is
written down. `testing/suites.toml` holds only what Cargo cannot see: the path triggers of
Cargo packages that other languages or tools reach, and the suites no Cargo package owns.
`testing/coverage-policy.toml` defines coverage ownership and exclusions. The planner checks
both files.

## Routes

A route says when a piece of verification runs. There are four, and `route` in
`testing/suites.toml` names one of them:

| Route | Meaning |
| --- | --- |
| `ordinary` | The change selects it |
| `required` | It runs whenever one of its CI jobs starts |
| `manual` | Explicitly invoked only: generators, probes, captured-source checks and the weekly application suite |
| `live` | It contacts a live service; explicitly invoked only |

There is no `weekly` route. A schedule belongs to the workflow that holds it: `test-weekly.yml`
names the two commands it runs each Monday, and the suites they run are `manual`.

Captured fixtures are not a route either. They are ordinary work whose Cargo targets are gated
on `required-features = ["external-fixtures"]`, run in CI after an explicit sync, and fail with
the exact `obc fixtures sync` command to run instead of skipping.

Physical work has no route and no registry row. A procedure that needs a board, a card or a
cable lives in its owning issue, and no workflow claims it.

Every Rust package is on the ordinary route unless `testing/suites.toml` says otherwise.
Its ordinary test binaries are every test target Cargo reports; its captured-fixture
binaries are the targets gated on `required-features = ["external-fixtures"]`. Examples are
generators and probes, so no tier compiles them.

## Suite granularity

Selection is always at suite granularity, never at test-function granularity. A plan unit is
one execution unit with one route. A new test belongs in the unit whose route matches it; if
none does, add a suite.

A file or binary that mixes ordinary work with captured-fixture, live or manual work must be
split into separate execution units. Do not make a mixed binary look homogeneous and do not add
test-level selection.

## Explicit commands and tiers

The Rust ordinary and captured-fixture runs use separate nextest invocations and result files.
`cargo-filter --tier fast|fixtures` derives expressions over whole Cargo binaries from
`cargo metadata`. It does not select test functions. A suite on the `manual` or `live` route can
name a `package` and its `targets`, and those targets then belong to no tier. Captured
navigation, POI, altitude, terrain and simulator scenarios have separate binaries. Live
Copernicus tests and the captured assistant places check run only through their explicit
commands; their ignored status is a second guard against a broad Cargo command contacting a
live service.

Run a manual suite through the command `testing/suites.toml` declares for it. Captured Rust
suites use the `obc test fixtures -p` wrapper to prepare inputs before they run locally.

Vector and assembly writers are Cargo examples. They run only through the commands
`testing/suites.toml` gives `manual.obc-link`, `manual.obc-vectors` and `manual.obc-web-assemble`.
`manual.obc-display` runs the row-hash timing probe. These commands can write fixtures or print
measurements; they are not required test passes.

There is no expensive tier. Every test that is not on the ordinary route has one explicit route,
and this table is the complete list:

| Item | Route |
| --- | --- |
| `host/obc-dem` `decode` and `real_tile` (ignored, live Copernicus download) | `live.copernicus`, explicit command only |
| `host/obc-pack` `assistant_places` (ignored, pinned 549 MB source) | `fixtures.assistant-places`, through `fixtures/verify-assistant-places.py` |
| `manual.obc-display`, `manual.obc-vectors`, `manual.obc-link`, `manual.obc-web-assemble` | their own explicit commands; a generator writes, it never verifies |
| `ios.application-weekly`, the weekly `obc-storage` run | `test-weekly.yml` |

A captured fixture is not on this list. Bounded fixture suites are ordinary work: they run in CI
after an explicit sync, and a missing package fails with the exact `obc fixtures sync` command
rather than skipping. No environment variable turns that failure on.

The required iOS suite owns `WebsiteScreenshotTests.swift`. The separate weekly application suite
owns the other XCUITest classes. `test-weekly.yml` runs the weekly iOS and storage work each
Monday and on manual dispatch; it names the two commands directly:

```sh
companion-ios/scripts/test-application.sh
cargo test -p obc-storage --locked
```

The iOS application command needs Xcode 26.5, XcodeGen and an iPhone 17 Pro simulator. Set
`OBC_TEST_DEVICE` to use another available simulator. It runs each application class once, without
parallel test execution or retries. The weekly result artifact is `ios-application-ATTEMPT`.
Required screenshot runs retain `ios-screenshots-ATTEMPT`. Both contain the native `.xcresult`
bundle contents. Restore the bundle suffix when downloading, then open it in Xcode to inspect
identities, outcomes and durations:

```sh
gh run download RUN_ID --name ios-screenshots-ATTEMPT --dir ios-screenshots.xcresult
gh run download RUN_ID --name ios-application-ATTEMPT --dir ios-application.xcresult
```

Weekly storage runs retain the original Cargo output as `storage-weekly-ATTEMPT`. The log contains
native test identities, outcomes and aggregate durations; it has no per-case durations or coverage.
A missing expected bundle
fails the upload; setup failure can leave no bundle. The screenshot script also accepts
`OBC_XCRESULT_PATH` for local retention. A workflow declaration alone does not establish a passing
run. TS6 still owns the remaining critical application journeys.

The UI snapshot sweep is its own `ui-snapshots` job, off the `test` job's serial path. The
sweep step runs only when `ci.ui-snapshots` is selected, and that suite still selects only on
its own rendering, screen and snapshot-input triggers. A broad coverage or policy change does
not run a sweep.

## The plan documents

`testing/suites.toml` has two kinds of entry. A `[[package]]` entry adds facts to a Cargo
package Cargo itself cannot supply:

| Field | Meaning |
| --- | --- |
| `name` | The Cargo package |
| `route` | `ordinary` by default; `required` runs it whenever one of its jobs starts |
| `triggers` | Paths whose edge no build graph carries |
| `fixtures` | True when a change under `fixtures/` reaches the package |
| `command` | The local command for a standalone Cargo root, which the workspace run misses |
| `platforms` | Supported platform restriction, when one exists |

A `[[suite]]` entry is a piece of verification no Cargo package owns:

| Field | Meaning |
| --- | --- |
| `id` | Stable suite identifier used by commands and reports |
| `route` | One route from the table above |
| `command` | One repository-root command for local and CI use |
| `jobs` | The CI jobs that execute it; an `ordinary` or `required` suite needs at least one |
| `triggers` | Paths that select it, including its own test sources |
| `fixtures` | Named captured or production-shaped inputs required by the suite |
| `platforms` | Supported platform restriction, when one exists |
| `foundation` | True when a manifest, lockfile or toolchain change must select it |
| `ci_only` | True when no `obc check` gate can reproduce it locally |
| `package`, `targets` | Cargo test targets this suite owns, which then belong to no tier |
| `budget_exception` | Temporary reason and open issue for a suite over budget |
| `sleep_exception` | Approved bounded real sleep, with a reason and open issue |

Neither entry may contain Rust dependencies, reverse dependencies, test counts, durations or
source-file lists. Cargo supplies its graph. Result artifacts supply counts and durations.

The job table lives in `tools/test_plan.py`. Each row names a CI job, the jobs it needs, and the
Cargo product roots or individual packages it compiles. Only the builders whose package no
`cargo` argument list names are written out: Trunk for the web demo and `wasm-pack` for the four
bridges. `tools/tests/test_selection_plan.py` pins both against the build script and the Trunk
page, and it runs in the unconditional `guards` job, so that pin is checked on every pull
request.

## Coverage-policy fields

Each `[[component]]` in `testing/coverage-policy.toml` has a stable `id`, included production-path
patterns, excluded paths with replacement evidence, and an `enforcement` class. Native collectors
write raw reports during the maintained test run. `tools/coverage_report.py` groups measured lines
by component, writes `components.json` and `summary.md`, and checks the accepted Rust baseline in
`testing/coverage-baseline.json`. No tests run again for conversion.

Only format/protocol codecs, CRC, storage, DFU and boot have a no-decrease line-coverage gate.
The comparison uses exact covered/total fractions, with no rounding tolerance. A missing or empty
critical component and unmeasured critical production function source fail. Other components are
informational. There is no repository-wide or changed-line percentage gate.

The boot component measures the actual shared boot policy in `obc-dfu` (`engine.rs`, `state.rs`, and
`blobstage.rs`). These files also belong to DFU. The boot image's MMIO adapters and reset entry are
explicit exclusions. The boot build and resource guards give static replacement evidence;
physical install, rollback and power-cut acceptance remain separate obligations. A host percentage
does not establish MMIO execution coverage.
The separate informational board-image row lists all nRF54L image source as excluded from host
execution, with board build/resource guards and the remaining physical obligations as evidence.

Generated bridges, test sources and test-support packages are excluded. A pinned Rust syntax parser
identifies `#[cfg(test)]` and `#[test]` item ranges and external test modules; it removes these lines
from component counts without changing the raw LLVM report. Files with declarations and no function
bodies are listed separately when LLVM has no executable mapping. Every summary lists excluded and
unmeasured files. Unmeasured informational source stays visible and does not become a zero or a pass.

### Native coverage artifacts

| Maintained invocation | Collector | Coverage artifact |
| --- | --- | --- |
| Workspace nextest | cargo-llvm-cov 0.9.1, current Rust LLVM | `coverage-rust-ATTEMPT` |
| Repository and firmware Python | coverage.py 7.16.1 around xmlrunner | `coverage-repository-tools-ATTEMPT`, `coverage-firmware-tools-ATTEMPT` |
| Builder Python | pytest-cov 7.1.0, coverage.py 7.16.1 | `coverage-builder-ATTEMPT` |
| Builder Vitest | Vitest and V8 provider 3.2.6 | `coverage-web-ATTEMPT` |
| OBCKit package | Xcode 26.5, native xccov | `ios-tests-coverage-ATTEMPT` |
| Desktop Linux release tests | cargo-llvm-cov 0.9.1 | `coverage-desktop-ATTEMPT` |

Artifacts contain native LCOV or xccov JSON plus component counts and a Markdown summary. V8 also
keeps its native JSON. Python tool artifacts keep their coverage database. The source SHA and tool
versions identify each measurement. Components that span languages have separate contributions in
each native artifact; do not add overlapping percentages. Desktop coverage is measured on Linux;
macOS and Windows native test results remain available, but their platform-only execution coverage
is unmeasured. OBCKit coverage does not include the iOS application composition root.

Install `tools/requirements-coverage.txt` to summarize a local native report:

```sh
python3 tools/coverage_report.py --scope rust --lcov .artifacts/coverage/rust/lcov.info \
  --output .artifacts/coverage/rust --tool cargo-llvm-cov=0.9.1 --tool "$(rustc --version)"
```

For downloaded reports, `--source-prefix ORIGINAL_CHECKOUT` maps the original absolute paths to
this checkout. Use `--measurement-sha ORIGINAL_SHA` to retain the measured source identity. Check
out that production source before replaying a report; coverage line numbers belong to that source.
The output records the reporting policy's SHA separately. Replaying reports runs no tests.

A baseline change requires actual successful run evidence, its source SHA and tool versions, and
review of its counts and exclusions. CI never writes or lowers the baseline. Collect a new measured
proposal when source ownership or native tool versions change. Raw reports and CI test outcomes
remain authoritative; reports from failed runs can be partial and are not acceptance evidence.

## Commands

`tools/test_plan.py` is the only selection implementation. `obc` and the CI workflow are thin
entry points into it, so a developer and CI answer "which work does this change require" with one
answer. CI calls the Python entry point directly because `just` is not installed on the runners;
`obc test affected` is the same call with the same output.

```sh
obc suites check                         # plan drift and command resolution
obc suites select --base REF [--head REF] [--format text|json] [--release]
obc suites validate-filters              # the job table and the workflow describe the same jobs
```

`check` is the always-run CI policy command. It reads `cargo metadata`, parses both plan
documents, and validates every trigger, platform, route, job name, carved target and command from
the repository root. It does not contact a live service.

`select` reads changed paths from Git, derives Rust package and reverse-dependency edges from
Cargo metadata, applies the non-Cargo edges the plan documents declare, and prints every selected
and non-selected unit with its reason and its CI jobs. Unknown production paths and selected
suites without an executable CI route are errors; a selection error never degrades to "run
everything".

`select --release` is what the release workflow passes. A release candidate is verified whole
rather than by its diff, so it requires every unit that has a CI route. It still leaves out the
snapshot sweep, which keeps its rendering-input budget, and every `manual` and `live` route.

`validate-filters` parses `.github/workflows/ci.yml` with PyYAML and requires that every job in
the table exists with a runner image, that every plan-gated job gates on its own name, that every
job reaches the `ci` aggregate's `needs`, and that the workflow's `needs` graph is the table's.
PyYAML is pinned in `tools/requirements-test.txt` and installed in the `guards` job; selection
itself is standard library only.

### Running suites locally

```sh
obc test affected --base origin/develop [--head REF] [--dry-run]
obc test -p obc-app                      # focused package work, no plan involved
obc test -p obc-app --lib                # the library target alone, no doctests
obc test fixtures -p obc-route
obc test full                            # cross-cutting changes only
```

`affected` prints the selected unit IDs with one reason each before it runs anything, and
`--dry-run` prints that plan and executes nothing. It runs one nextest invocation over the
selected root-workspace packages — package flags narrow compilation, the ordinary tier filter
narrows execution — and then each selected suite's own command, stopping at the first failure. An
empty package set produces no Cargo invocation; it never expands to the workspace. It also reads
the working tree, so uncommitted and untracked source counts. A suite whose `platforms` exclude
the current host is reported as skipped with that restriction, never as passed. `obc test
fixtures -p` keeps its scoped meaning and needs neither the plan, Git nor Cargo metadata.

A focused `obc test -p PACKAGE` runs that scope on nextest, the runner CI uses, and then the
same scope's doctests. `--lib` runs the library test target alone and no doctests; Cargo rejects
it on a package with no library.

### Reproducing CI gates locally

`obc check <gates>` runs the primitive commands of the named gates and prints the units those
gates reproduce; a gate the job table does not know fails before any work starts. A gate names
the CI jobs it re-runs, and it reproduces a unit when it runs every job that unit routes to.
`obc check full` runs every gate and then names each suite required on a pull request that the
run did not reproduce, with the reason. It makes no unqualified CI-parity claim.

## Rust CI result artifacts

`tools/ci/test.sh` is the body of the `test` job, one section per CI step, and `obc check test`
runs the same file. Its two nextest steps stay `--workspace --all-features` with the tier filter
expression, and they are deliberately not narrowed to the affected packages. The per-pull-request
coverage ratchet in `tools/coverage_report.py` reads one LCOV report over the whole workspace and
fails any critical file it never compiled, so a narrowed package set would fail the ratchet
rather than save time. Moving that ratchet from pull requests to develop pushes would remove the
constraint; that is the owner's call and has not been made. Package-level narrowing is therefore
only on the local `obc test affected` route, where no ratchet reads the result. Only the two
nextest sections run under llvm-cov instrumentation, because their report is that evidence.

The two `cargo nextest run` commands in `test` use `NEXTEST_PROFILE=ci` for fast binaries
and `NEXTEST_PROFILE=fixtures` for captured fixtures. The profiles in `.config/nextest.toml` write
native JUnit XML to `target/nextest/ci/junit.xml` and `target/nextest/fixtures/junit.xml`. Each test case retains its binary and test identity, result, and
elapsed duration. Failed tests also retain their output. Test selection, retries, and failure
handling keep their existing settings; a failed run can contain only the tests completed before
it stopped. Filtered and ignored tests are absent from the report, not recorded as passes.

CI uploads the report after test success or failure. The artifact name is `rust-test-ATTEMPT`,
and `rust-fixtures-ATTEMPT`, where `ATTEMPT` is the GitHub run attempt. Open a workflow run's
**Artifacts** section, or download all its Rust reports with:

```sh
gh run download RUN_ID --pattern 'rust-*' --dir test-results
```

Each command removes an old report before it starts. A skipped job uploads nothing. A build or
fixture setup failure remains a failure and may produce no report; the upload step reports a
missing file as an error. CI does not create an empty report or run the tests again for reporting.

These JUnit artifacts cover nextest invocations. Workspace doctests and the default-feature
`obc-formats` Cargo command retain their native text in `coverage-rust-ATTEMPT` (`doctests.log` and
`formats-default.log`). Those logs contain native identities and outcomes, with aggregate durations;
Cargo does not emit individual case durations for these commands. No synthetic durations or passes
are added. Desktop uses nextest on Linux, macOS and Windows and publishes
`desktop-tests-PLATFORM-ATTEMPT`. Swift's native result bundle is described below.

## Python CI result artifacts

The maintained Python suites write XML from their existing test invocations. Install the test
requirements in your Python environment before local execution:

```sh
python3 -m pip install -r tools/requirements-test.txt
python3 -m pip install -r builder/requirements-dev.txt
```

The repository and firmware tool suites use pinned `unittest-xml-reporting` 4.0.0
with standard unittest discovery. Their declared commands write to
`.artifacts/python/repository-tools/` and `.artifacts/python/firmware-tools/`. Builder uses pytest's native `--junitxml` option and writes
`.artifacts/python/builder.xml`. Builder still needs its existing `obc-pack` executable; use
`OBC_PACK_BIN` to select a built binary. CI builds it before the test step.

Each testcase retains its class/function identity, outcome and elapsed duration. Expected
unittest failures appear as skips with type `XFAIL`; unexpected successes appear as errors with
type `UnexpectedSuccess` and fail the command. Failed, errored and skipped subtests retain their
parameter identities. Successful subtests are grouped under their parent test, not counted as
separate successes. Skipped subtests can be in a separate XML file. Subtest times are not separate
measurements and can be zero; XML entry totals can differ from unittest's method count. The small
reporter contract in `tools/tests/test_python_reports.py` checks these native semantics.

CI publishes `python-repository-tools-ATTEMPT`, `python-firmware-tools-ATTEMPT`,
and `python-builder-ATTEMPT`. Download them with:

```sh
gh run download RUN_ID --pattern 'python-*-ATTEMPT' --dir test-results
```

CI and local suite commands remove stale reports before execution. CI uploads after an
executed test step succeeds or fails. Setup failures, skipped steps and cancelled runs publish no
report for that step. Missing expected output fails the upload. Collection or test failures keep
their nonzero exit status and can leave partial results. No tests run again to produce reports.
Coverage is collected in the same invocation and published separately. The test exit status and CI
log remain authoritative.

## Web CI result artifacts

The builder Vitest command keeps its default console output and also
writes native JUnit XML. Each report records file and test identities, outcomes, and elapsed
durations. Skipped tests retain their skipped status. V8 collects coverage in the same invocation.

The same registered builder suite executes the unchanged whole `sha256.test.ts` file separately
without V8 instrumentation. Its real 600 MB length-boundary assertion and 60-second timeout remain.
`npm test` keeps the complete uninstrumented suite in one invocation. CI runs
`npm run test:components -- --coverage` and
`npm run test:sha256` with separate native JUnit outputs; `web-sha256-ATTEMPT` contains all four SHA
cases. Other instrumented production callers still contribute SHA coverage. There is no test-name
filter, duplicate test execution, retry, or injected hash state.

CI uploads the reports after test success or failure. Artifact names are `web-builder-ATTEMPT`
where `ATTEMPT` is the GitHub run attempt. Download it with:

```sh
gh run download RUN_ID --pattern 'web-*-ATTEMPT' --dir test-results
```

Each step removes its old report before it starts. A skipped step uploads nothing. Setup or
collection failures remain failures and can produce an incomplete report or no report; a missing
file makes the upload step fail. A cancelled run does not upload these reports. CI does not create
an empty report or run tests again to obtain results. These reports cover the builder Vitest suite,
which uses Node and a simulated DOM. The `web.builder-browser` journey below is the builder's
real-browser evidence.

## Builder browser journey

`web.builder-browser` is an affected end-to-end suite for the map builder. The `web-browser` CI job
runs it in Chromium on the `dist/web` build that ships. `tools/fixture_catalog.py` publishes the
`obc-web-assemble` bridge fixture as a digest-pinned catalog on loopback, and serves the build on
the same origin. The journey selects the region, lets the application verify the cells into OPFS,
assembles them in the real worker with the real WebAssembly bridge, and downloads the map. The
downloaded bytes must be the same as the checked-in `expected/map.obcm`, which the command-line
assembler wrote from the same cells.

The journey also makes sure that the run used the storage path that ships, that the application
requested every published object, and that no request went to a host other than loopback. A
console, page or worker error is a failure. The suite covers the download half only. The upload
half to a device is not in it.

The job requires the browser test step to succeed and publishes `web-builder-browser-ATTEMPT`,
with native JUnit and diagnostics, plus a screenshot and trace on failure.

## Web demo browser journey

`web.demo-browser` is an affected end-to-end suite for the shipping landing page. It runs in
Chromium after the existing Trunk build in the `wasm` CI job. The journey uses the real page
controls and existing WASM observations to save and view a ride, reset for route upload, save
again and reload. It checks rendering and rejects page errors, failed resets and stalled states.
It does not measure exact saved-object counts or content; native tests own those assertions.

The job requires the browser test step to succeed and publishes `web-demo-browser-ATTEMPT`,
with native JUnit and diagnostics, plus a screenshot and trace on failure. See the
[web demo README](../apps/obc-web-demo/README.md) for setup, reproduction and evidence limits.

## Swift CI results and coverage

The `ios-unit` job runs the complete OBCKit package once with Xcode 26.5 on the macOS host.
`xcodebuild test` disables parallel testing and collects coverage in the native
`ios-unit.xcresult` bundle. The declared command uses the same package scheme and serial test setting
for local execution. No iOS simulator or physical device is required.

The bundle contains XCTest and Swift Testing results, including test identities, outcomes,
durations, and failure details. CI exports the native result summary and test tree with
`xcresulttool`; it does not convert console output or run tests again. Keep the native hierarchy
when reading parameterized Swift Testing results. A test function and its argument cases are
not separate totals to add together.

`xccov` exports native file coverage to `xccov.json`. The component summary uses covered and
executable line counts from that report. The artifact also records the actual Xcode build and
Swift version in `toolchain.txt`. This measures package code executed on macOS; it does not
measure the iOS app or prove physical BLE behavior.

CI uploads `ios-tests-coverage-ATTEMPT` after test success or failure. Download it with:

```sh
gh run download RUN_ID --name ios-tests-coverage-ATTEMPT
```

Open the `.xcresult` bundle in Xcode to inspect the complete run. The JSON exports let other
tools read the results without Xcode. CI clears stale output before execution. Result export
and coverage export run as separate steps, so a coverage export failure does not prevent
result export. A build failure can produce an incomplete bundle or no test results; export
failures remain CI failures. Skipped or cancelled test steps do not publish results. The test
exit status and CI log remain authoritative; an uploaded artifact does not prove a passing run.

## Exceptions

A suite over budget declares a `budget_exception`; a suite that waits on real time declares a
`sleep_exception`. Both are temporary. Each must state a concrete reason and reference an open
repository issue, so the exception expires with that issue.

`tools/test_exceptions.py --repo OWNER/REPO` checks them. It validates every exception block
offline, then asks GitHub once per distinct issue, with a 20-second request timeout, whether that
issue is still open. It fails on closed issues, pull requests, or API errors and names each owning
suite and field. It needs authenticated `gh` access.

The Test exception issue health workflow runs it each Monday at 07:17 UTC and on manual dispatch,
with read-only permissions. It is separate from offline suite validation and does not claim that
all scheduled test suites have execution routes.

Do not retry a flaky test automatically. Prefer observed state, a controllable clock, or a protocol
signal to a fixed sleep; use a small bounded sleep only when its exception explains why.

## Change selection

This table is implemented by `test_plan.py select`, the shared local and CI selection core.
CI's `selection` job publishes the plan and the list of workflow jobs it requires; every gated job
starts only when that list names it, and the aggregate `ci` job evaluates the same plan. There is
no path-filter selector — a Cargo package reaches the jobs the job table says compile it, and
every other suite names its jobs in `testing/suites.toml`.

Five rules fail closed, each with its own test. A changed tracked path that no owner claims is an
error. "No owner" is judged over source and policy files: a path under `docs/`, `artifacts/`,
`.claude/` or `.repowise/`, a test file, and any path whose suffix is not one of the code and
policy suffixes the planner lists are all outside the rule, so a new `hardware/notes.txt` selects
nothing and reports nothing. A selected suite with no CI route is an error. A selection error publishes no plan, so the
`selection` job exits nonzero and the aggregate fails on the missing plan. An empty root package
set produces no Cargo invocation. A change to a manifest, the lockfile, the toolchain, the Cargo
configuration, the planner, the workflow, `tools/ci/**` or `testing/suites.toml` selects the whole
relevant graph.

A deleted path is the one case that does not fail closed: its owner may have been deleted with it
and the base tree's Cargo graph is not available, so an unowned deleted path selects the whole
graph rather than nothing.

| Change type | Required pull-request work |
| --- | --- |
| Plan documents, selector, workflow, root manifest, lockfile, or toolchain | Full relevant build and ordinary-test graph |
| One Rust crate | Ordinary tests for the crate; affected contracts and reverse-dependent suites |
| Shared format, protocol, or vector | All affected Rust, Swift, and web contract consumers |
| Fixture or fixture loader | Owner suite and each consumer contract suite |
| Web-only source | Web unit and component tests |
| WebAssembly producer or bridge | Build bridge, web contract tests, and browser smoke |
| Critical browser, worker, storage, or download flow | Browser smoke suite |
| Swift package source | Affected Swift targets |
| iOS application composition or UI | Swift host tests and XCUITest smoke |
| Desktop application composition | Affected platform build and desktop launch smoke |
| Python tool or service implementation | Matching Python suite |
| Documentation only | Documentation and generated-policy checks unless it produces a shared artifact |
| Agent prose (`CLAUDE.md`, `AGENTS.md`) | Documentation and policy validation only; it decides nothing, so it builds no platform |
| Live-service or hardware path | Hermetic contracts on the pull request; scheduled, manual, or release evidence as required |

The aggregate gate reports pass, fail, not selected, selected but not run, or blocked by an upstream
failure for every suite. A skipped selected job is a failure, never evidence that the suite passed,
and a failed or cancelled `selection` job fails the gate because no plan can then be trusted.

## Timing guidance

The guidance stays informational: required PR elapsed time at most 10 minutes, p95 at most
20 minutes, cross-surface cost at most 40 runner-minutes, a required binary/file at most 30 seconds,
and a unit suite near two seconds. Explain any remaining exception in its open owner issue.
