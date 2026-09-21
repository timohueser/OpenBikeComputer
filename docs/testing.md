# Testing

`tools/test_plan.py` decides what a change must test. Cargo's graph supplies every Rust package,
every dependency edge and the split between the fast and the captured-fixture tier.
`testing/suites.toml` holds only what Cargo cannot see; `testing/coverage-policy.toml` holds
coverage ownership. `obc suites check` validates both and runs on every pull request.

## Commands

```sh
obc test -p obc-app                       # one package on nextest, then its doctests
obc test -p obc-app --lib                 # the library target alone
obc test fixtures -p obc-route            # the captured-fixture tier, after a fixture sync
obc test affected --base origin/develop   # the selection CI runs; --dry-run prints it
obc test full                             # cross-cutting changes only
obc check fmt clippy device docs          # named CI gates; obc check full runs them all
obc suites check                          # plan drift and command resolution
obc suites select --base REF [--release]  # the plan as text or json
```

`obc test` always needs a scope; it never expands to the workspace on its own. `affected` reads
the working tree, prints every selected unit with a reason, and stops at the first failure. A
suite whose `platforms` exclude the host is reported as skipped, never as passed. Every Cargo
command needs the CI-pinned runner: `cargo install cargo-nextest --version 0.9.143 --locked`.
The Python suites need `pip install -r tools/requirements-test.txt`.

## Routes

`route` in `testing/suites.toml` says when a unit runs:

| Route | Meaning |
| --- | --- |
| `ordinary` | the change selects it (the default for every Rust package) |
| `required` | it runs whenever one of its CI jobs starts |
| `manual` | its own command only: generators, probes, captured-source checks, the weekly suites |
| `live` | it contacts a live service; its own command only |

Selection is per suite, never per test function. A binary that mixes ordinary work with
captured-fixture, live or manual work is split into separate units. Captured fixtures are ordinary
work gated on `required-features = ["external-fixtures"]`; a missing package fails with the exact
`obc fixtures sync` command. Physical procedures have no route; they live in their issue.
`test-weekly.yml` names the two commands it runs each Monday; their suites are `manual`.

## The plan documents

A `[[package]]` entry adds what Cargo cannot say about a package:

| Field | Meaning |
| --- | --- |
| `name` | the Cargo package |
| `route` | `ordinary` by default |
| `triggers` | paths whose edge no build graph carries |
| `fixtures` | true when a change under `fixtures/` reaches the package |
| `command` | the local command for a standalone Cargo root |
| `platforms` | supported platforms, when restricted |

A `[[suite]]` entry is verification no Cargo package owns:

| Field | Meaning |
| --- | --- |
| `id` | stable identifier used by commands and reports |
| `route` | one of the four routes |
| `command` | one repository-root command, for local and CI use |
| `jobs` | the CI jobs that execute it (`ordinary` and `required` need at least one) |
| `triggers` | paths that select it, including its own test sources |
| `fixtures` | captured or production-shaped inputs it needs |
| `platforms`, `foundation`, `ci_only` | platform restriction; selected by manifest or toolchain changes; not reproducible locally |
| `package`, `targets` | Cargo test targets this suite owns, which then belong to no tier |
| `sleep_exception` | a bounded real sleep, with a reason and an open issue |

Neither entry lists dependencies, test counts, durations or source files; Cargo and the result
artifacts supply those. The CI job table is in `tools/test_plan.py`; `obc suites validate-filters`
checks that it and `.github/workflows/ci.yml` describe the same jobs.

Selection fails closed: a changed tracked source or policy path that no unit owns is an error, a
selected suite with no CI route is an error, and a selection error publishes no plan, so the `ci`
aggregate fails. A manifest, lockfile, toolchain, planner, workflow, `tools/ci/**` or
`testing/suites.toml` change selects the whole relevant graph. A deleted path selects the whole
graph, because its owner may be gone with it.

## Exceptions

A suite that waits on real time declares `sleep_exception = { reason, issue }`; the exception
expires with the issue. `tools/test_exceptions.py --repo OWNER/REPO` checks each block and asks
GitHub once per issue whether it is open; `test-exception-health.yml` runs it weekly. Do not
retry a flaky test; prefer observed state, a controllable clock or a protocol signal to a sleep.

## Coverage

Each `[[component]]` in `testing/coverage-policy.toml` names its production paths, its
exclusions with replacement evidence, and an `enforcement` class. Only the format and protocol
codecs, CRC, storage, DFU and boot have a no-decrease line-coverage gate, compared as exact
fractions against `testing/coverage-baseline.json`. Everything else is informational; there is no
repository-wide percentage gate. CI never writes the baseline; a baseline change needs a measured
run, its source SHA and its tool versions in the pull request.

The `test` job's two nextest steps stay `--workspace --all-features`, because the coverage ratchet
reads one report over the whole workspace and fails any critical file it never compiled. Only the
local `obc test affected` route narrows by package.

## CI artifacts

Every job uploads its native results after success or failure; a skipped step uploads nothing,
and a missing expected file fails the upload. Download with
`gh run download RUN_ID --pattern '<prefix>*' --dir test-results`.

| Artifact | Holds |
| --- | --- |
| `rust-test-ATTEMPT`, `rust-fixtures-ATTEMPT` | nextest JUnit XML for the fast and fixture tiers |
| `coverage-rust-ATTEMPT` | LCOV, component counts, `doctests.log`, `formats-default.log` |
| `python-repository-tools-ATTEMPT`, `python-firmware-tools-ATTEMPT`, `python-builder-ATTEMPT` | unittest and pytest XML |
| `coverage-repository-tools-ATTEMPT`, `coverage-firmware-tools-ATTEMPT`, `coverage-builder-ATTEMPT` | coverage.py databases and summaries |
| `web-builder-ATTEMPT`, `web-sha256-ATTEMPT`, `coverage-web-ATTEMPT` | Vitest JUnit XML and V8 coverage |
| `web-builder-browser-ATTEMPT`, `web-demo-browser-ATTEMPT` | the two Chromium journeys, with a screenshot and trace on failure |
| `ios-tests-coverage-ATTEMPT`, `ios-screenshots-ATTEMPT`, `ios-application-ATTEMPT` | `.xcresult` bundles; restore the suffix and open in Xcode |
| `desktop-tests-PLATFORM-ATTEMPT`, `coverage-desktop-ATTEMPT` | desktop nextest results; coverage measured on Linux |

The exit status and the CI log stay authoritative; an artifact does not prove a passing run.

## Browser journeys

`web.builder-browser` runs the map builder in Chromium against a digest-pinned loopback catalog,
assembles a map through the real WebAssembly bridge and requires the bytes to equal
`expected/map.obcm`. `web.demo-browser` drives the landing page's save, view, reset and reload
controls. For simultaneous runs in separate worktrees, set `OBC_BROWSER_PORT` to a free port;
the defaults are 4180 and 4178.
