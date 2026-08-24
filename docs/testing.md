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
by relabeling it and do not add test-level selection. The initial measured inventory records the
following temporary conflicts:

- `obc-storage` mixes fast storage tests with expensive crash matrices.
- `obc-wx-bake` mixes fast codecs, captured fixtures, large bakes, and manual generators.
- `obc-dem`, `obc-link`, `obc-render`, `obc-display`, `obc-vectors`, `obcm-assemble`, and
  `obc-web-assemble` contain an ignored live, timing, exhaustive, or generator path beside required
  tests.
- the Rust aggregate command still combines fast and external-fixture execution.
- the Swift host command combines fast model tests, captured fixtures, real sleeps, and one
  expensive exhaustive codec test.
- required XCUITest currently runs two screenshot methods while the rest of the application suite
  has no scheduled full route.

These are inventory findings, not permission to hide a conflict. The linked issues in the registry
keep every temporary state accountable.

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
obc test fixtures -p obc-wx-bake canonical_mosaic
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

## Exceptions, quarantines, and sleeps

Budget exceptions, quarantines, cadence conflicts, and real-sleep exceptions are temporary. Each
must state a concrete reason and reference an open repository issue. A missing or malformed issue
reference fails the checker. Normal validation stays offline; issue closure is reviewed when the
exception changes and will be automated by the later quarantine-hygiene delivery step.

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
