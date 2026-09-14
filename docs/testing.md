# Testing policy

`testing/suites.toml` is the machine-readable inventory of maintained test and validation
suites. `testing/coverage-policy.toml` is the coverage-policy scaffold. Both files are parsed and
checked by `tools/suite_registry.py`; test counts, durations, and Cargo dependency edges are
derived and never copied into either registry.

## Test levels

| Level | Mechanical meaning |
| --- | --- |
| Unit | Hermetic module or pure-behavior test |
| Component | Hermetic multi-module test in one process |
| Contract | Stable format, protocol, vector, build, resource, or artifact check |
| Fixture | Hermetic test over captured external or production-shaped data |
| End-to-end | Shipping entry point in a real browser, application, simulator, or process |
| Live | Test against a live external service |
| Hardware | Test against a physical target |

Unit, component, and contract suites form the **fast hermetic tier**. The checker enforces the
mechanical properties that affect execution: a fast or fixture command cannot visibly contact a
live service, fixture suites declare their fixture sets, real sleeps need a reason and an open
issue, and live or hardware suites never run for an unrelated pull request. It does not try to
prove the subjective boundary between unit and component.

“E2E” means only the end-to-end level. A host model test, in-process flow, headless component, or
shared-vector test is not E2E even when an older test name contains that abbreviation.

## Suite granularity

Selection is always at suite granularity, never at test-function granularity. A registry entry is
one execution unit with one cadence. A new test belongs in the suite whose mechanical level and
cadence match it; if none does, add a suite.

A file or binary that mixes the fast tier with fixture, end-to-end, live, or hardware work must be
split into separate execution units. Until that source split can be made, the registry records a
`cadence_conflict` with its reason and an open issue. Do not make a mixed binary look homogeneous
by relabeling it and do not add test-level selection.

The current conflicts and their owners live in `testing/suites.toml`. Use `obc suites list` and
`obc suites explain SUITE_ID` to inspect them. A declared cadence does not prove that a workflow
executes it; unresolved execution routes remain explicit conflicts until the implementation lands.

## Suite registry fields

Every `[[suite]]` entry uses these fields:

| Field | Meaning |
| --- | --- |
| `id` | Stable suite identifier used by commands and reports |
| `surface` | Product or development surface |
| `level` | One level from the table above |
| `command` | One repository-root command for local and CI use |
| `fixtures` | Named captured or production-shaped inputs required by the suite |
| `pull_request` | `always`, `affected`, or `never` |
| `scheduled` | `none`, `nightly`, `weekly`, `manual`, or `release` |
| `extra_triggers` | Paths whose edge is not available from a build graph |
| `platforms` | Supported platform restriction, when one exists |
| `coverage_component` | Coverage-policy component fed by the suite, when applicable |
| `ownership` | Stable Cargo package, Swift target/package, test-root pattern, or workflow route |
| `budget_exception` | Temporary reason and open issue for a suite over budget |
| `quarantine` | Temporary reason and open issue for quarantined behavior |
| `cadence_conflict` | Mixed-cadence source that still needs to split, with an open issue |
| `sleep_exception` | Approved bounded real sleep, with a reason and open issue |

Ownership is routing information. It is not a copied source-file inventory. Cargo targets and
library harnesses come from `cargo metadata`; files under the declared non-Rust roots come from
filesystem, Swift package, and workflow discovery. An ownership pattern may not cross a product
surface or cadence boundary.

The registry must not contain Rust dependencies, reverse dependencies, test counts, durations, or
source-file lists. Cargo supplies its graph. Result artifacts added by the later measurement step
supply counts and durations. The drift checker rejects fields that try to store those facts.

## Coverage-policy fields

Each `[[component]]` in `testing/coverage-policy.toml` has a stable `id`, included production-path
patterns, optional excluded generated or platform-only paths with replacement evidence, and a
planned `enforcement` class. Format and protocol codecs, CRC, storage, DFU, and boot are planned
`ratchet` components. Application, UI, and tool components are `report`.

There is deliberately no accepted baseline yet. The coverage delivery step will measure and
review baselines; this scaffold does not invent a number or enforce coverage tooling.

## Commands

`tools/suite_registry.py` is the only selection implementation. `obc` and the CI workflow are thin
entry points into it, so a developer and CI answer "which suites does this change require" with one
answer. CI calls the Python entry point directly because `just` is not installed on the runners;
`obc test affected` is the same call with the same output.

```sh
obc suites check                         # registry drift, discovery, and command resolution
obc suites list [--json]
obc suites explain rust.obc-storage
obc suites select --base REF [--head REF] [--format text|json]
obc suites validate-filters              # plan and workflow describe the same jobs
```

`check` is the always-run CI policy command. It parses both registries, derives Cargo metadata and
every supported non-Rust source, validates command and trigger resolution from the repository root,
and requires exactly one registry owner for every discovered execution unit and required CI command.
It does not contact a live service.

`select` reads changed paths from Git, derives Rust package and reverse-dependency edges from Cargo
metadata, applies only the extra cross-language edges declared by the registry, and prints every
selected and non-selected suite with its reason and its CI jobs. Unknown production paths and
selected suites without an executable CI route are errors; a selection error never degrades to
"run everything".

### Running suites locally

```sh
obc test affected --base origin/develop [--head REF] [--dry-run]
obc test unit|component|contract|fixtures|e2e [--surface NAME] [--dry-run]
obc test -p obc-weather                  # focused package work, no registry involved
obc test fixtures -p obc-wx-bake
obc test full                            # cross-cutting changes only
```

Every form prints the selected suite IDs with one reason each before it runs anything, and
`--dry-run` prints that plan and executes nothing. `affected` runs each selected suite's registry
command in registry order and stops at the first failure. `fixtures` means the `fixture` level and
`e2e` means `end-to-end`; those are the only two aliases. A suite whose `platforms` exclude the
current host is reported as skipped with that restriction, never as passed. `obc test fixtures`
keeps its scoped meaning whenever a Cargo scope is present, and that path needs neither the
registry, Git, nor Cargo metadata.

### Reproducing CI gates locally

`obc check <gates>` runs the primitive commands of the named gates and prints the registry suites
those gates reproduce; a gate that resolves to no registry suite fails before any work starts.
`obc check full` runs every gate the registry declares and then names each suite required on a pull
request that the run did not reproduce, with the reason. It makes no unqualified CI-parity claim.

## Rust CI result artifacts

The existing `cargo nextest run` commands in `test` and each `test-weather` matrix leg use
`NEXTEST_PROFILE=ci`. The profile in `.config/nextest.toml` writes native JUnit XML to
`target/nextest/ci/junit.xml`. Each test case retains its binary and test identity, result, and
elapsed duration. Failed tests also retain their output. Test selection, retries, and failure
handling keep their existing settings; a failed run can contain only the tests completed before
it stopped. Filtered and ignored tests are absent from the report, not recorded as passes.

CI uploads the report after test success or failure. Artifact names are `rust-test-ATTEMPT` and
`rust-test-weather-INDEX-ATTEMPT`, where `INDEX` is the matrix job index and `ATTEMPT` is the GitHub
run attempt. The two canonical mosaic partitions have distinct indices. Open a workflow run's
**Artifacts** section, or download all its Rust reports with:

```sh
gh run download RUN_ID --pattern 'rust-test-*' --dir test-results
```

Each command removes an old report before it starts. A skipped job uploads nothing. A build or
fixture setup failure remains a failure and may produce no report; the upload step reports a
missing file as an error. CI does not create an empty report or run the tests again for reporting.

These artifacts cover only the nextest invocations. Cargo doctests, other Cargo test commands,
and serial XCTest results still use their existing logs. Python and Swift Testing results are
separate, as described below. This is not a coverage baseline or a complete cross-language result set.

## Python CI result artifacts

The four Python suites write XML from their existing test invocations. Install the test
requirements in your Python environment before local execution:

```sh
python3 -m pip install -r tools/requirements-test.txt
python3 -m pip install -r builder/requirements-dev.txt
```

The repository, firmware, and weather tool suites use pinned `unittest-xml-reporting` 4.0.0
with standard unittest discovery. Their registry commands write to
`.artifacts/python/repository-tools/`, `.artifacts/python/firmware-tools/`, and
`.artifacts/python/weather-probe/`. Builder uses pytest's native `--junitxml` option and writes
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
`python-weather-probe-ATTEMPT`, and `python-builder-ATTEMPT`. Download them with:

```sh
gh run download RUN_ID --pattern 'python-*-ATTEMPT' --dir test-results
```

CI and local registry commands remove stale reports before execution. CI uploads after an
executed test step succeeds or fails. Setup failures, skipped steps and cancelled runs publish no
report for that step. Missing expected output fails the upload. Collection or test failures keep
their nonzero exit status and can leave partial results. No tests run again to produce reports.
These are result artifacts, not coverage measurements; the test exit status and CI log remain
authoritative.

## Web CI result artifacts

The existing builder and rain-radar Vitest commands keep their default console output and also
write native JUnit XML. Each report records file and test identities, outcomes, and elapsed
durations. Skipped tests retain their skipped status. This does not add test execution or coverage.

CI uploads the reports after test success or failure. Artifact names are `web-builder-ATTEMPT`
and `web-rain-radar-ATTEMPT`, where `ATTEMPT` is the GitHub run attempt. Download both with:

```sh
gh run download RUN_ID --pattern 'web-*-ATTEMPT' --dir test-results
```

Each step removes its old report before it starts. A skipped step uploads nothing. Setup or
collection failures remain failures and can produce an incomplete report or no report; a missing
file makes the upload step fail. A cancelled run does not upload these reports. CI does not create
an empty report or run tests again to obtain results. These reports cover the two Vitest suites,
which use Node and simulated DOM environments; they are not real-browser evidence.

## Web demo browser journey

`web.demo-browser` is an affected end-to-end suite for the shipping landing page. It runs in
Chromium after the existing Trunk build in the `wasm` CI job. The journey uses the real page
controls and existing WASM observations to save and view a ride, reset for route upload, save
again and reload. It checks rendering and rejects page errors, failed resets and stalled states.
It does not measure exact saved-object counts or content; native tests own those assertions.

The job requires the browser test step to succeed and publishes `web-demo-browser-ATTEMPT`,
with native JUnit and diagnostics, plus a screenshot and trace on failure. See the
[web demo README](../apps/obc-web-demo/README.md) for setup, reproduction and evidence limits.

## Swift Testing CI result artifact

The existing `ios-unit` command adds `--xunit-output` without changing how tests run. With both
frameworks enabled, SwiftPM writes Swift Testing results to `ios-unit-swift-testing.xml`.
The current serial XCTest run does not produce xUnit XML; its results remain in the CI log.

The native report records Swift Testing function identities and durations. Parameterized cases
are grouped under their test function. Failure totals count recorded issues, which can differ
from the number of failed functions; known issues are not failures. Native skipped entries retain
their status. Do not combine these fields into an invented count of executed parameter cases.

CI removes stale reports before the command and uploads `ios-swift-testing-ATTEMPT` after test
success or failure. Download it with `gh run download RUN_ID --name ios-swift-testing-ATTEMPT`.
A build failure can produce no report; a terminated runner can leave incomplete XML. Missing
output makes the upload fail. Skipped or cancelled steps upload nothing. The test exit status and
CI log remain authoritative; an artifact is not proof that the run passed. No converter, extra
test invocation or coverage collection is added. This is not a result set for all Swift tests.

## Exceptions, quarantines, and sleeps

Budget exceptions, quarantines, cadence conflicts, and real-sleep exceptions are temporary. Each
must state a concrete reason and reference an open repository issue. A missing or malformed issue
reference fails the checker. Normal validation stays offline. To check issue state, run
`./tools/obc suites check-issues --repo OWNER/REPO` with authenticated `gh` access. This explicit
online maintenance command checks each distinct issue once, with a 20-second request timeout. It
fails on closed issues, pull requests, or API errors and names each owning suite and field.

The Test exception issue health workflow runs this command each Monday at 07:17 UTC and on manual
dispatch, with read-only permissions. It reads the same registry; it is separate from offline suite
validation and does not claim that all scheduled test suites have execution routes.

Do not retry a flaky test automatically. A quarantined behavior stays visible in the registry and
its issue. Prefer observed state, a controllable clock, or a protocol signal to a fixed sleep; use
a small bounded sleep only when its exception explains why.

## Change selection

This table is implemented by `suite_registry.py select`, the shared local and CI selection core.
CI's `selection` job publishes the plan and the list of workflow jobs it requires; every gated job
starts only when that list names it, and the aggregate `ci` job evaluates the same plan. There is no
path-filter selector — a suite's CI jobs are derived from the workflow commands the registry says it
owns and, for Cargo packages, from the workflow steps that compile them.

| Change type | Required pull-request work |
| --- | --- |
| Test registry, selector, workflow, root manifest, lockfile, or toolchain | Full relevant build and fast-test graph |
| One Rust crate | Unit and component tests for the crate; affected contracts and reverse-dependent suites |
| Shared format, protocol, or vector | All affected Rust, Swift, and web contract consumers |
| Fixture or fixture loader | Owner suite and each consumer contract suite |
| Web-only source | Web unit and component tests |
| WebAssembly producer or bridge | Build bridge, web contract tests, and browser smoke |
| Critical browser, worker, storage, or download flow | Browser smoke suite |
| Swift package source | Affected Swift targets |
| iOS application composition or UI | Swift host tests and XCUITest smoke |
| Desktop application composition | Affected platform build and desktop launch smoke |
| Python tool or service implementation | Matching Python suite |
| Rain-radar demo | Demo tests |
| Documentation only | Documentation and generated-policy checks unless it produces a shared artifact |
| Live-service or hardware path | Hermetic contracts on the pull request; scheduled, manual, or release evidence as required |

The aggregate gate reports pass, fail, not selected, selected but not run, or blocked by an upstream
failure for every suite. A skipped selected job is a failure, never evidence that the suite passed,
and a failed or cancelled `selection` job fails the gate because no plan can then be trusted.
