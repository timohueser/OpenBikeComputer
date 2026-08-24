# Developing OpenBikeComputer

The complete test taxonomy, suite-granularity rule, registry fields, exception policy, and change
selection table live in [`docs/testing.md`](docs/testing.md). In short: unit, component, and
contract suites are the fast hermetic tier; fixture, end-to-end, live, and hardware suites use
explicit cadences; selection is per suite, never per test function.

## Verification is proportional to the change

The normal development loop is **scoped verification**, not the complete repository gate. Every
developer and coding agent should inspect the files it changed, exercise the smallest command that
covers those files, and report exactly what it ran. CI remains the cross-repository backstop.

### 1. Focused checks — the default

Run the directly affected tests while iterating and the affected package before handoff:

```sh
obc test -p obc-weather
obc test -p obc-app weather_alert
cargo clippy -p obc-weather --all-targets -- -D warnings
```

`obc test` deliberately requires a scope. It does not silently expand to the workspace. Multiple
affected packages may be supplied with repeated `-p` arguments.

When the change spans packages, ask the registry which suites it selects instead of guessing:

```sh
obc test affected --base origin/develop --dry-run
obc test affected --base origin/develop
obc test unit --surface formats
```

`affected` is the same selection CI runs. Every form prints the selected suites and one reason each,
and `--dry-run` executes nothing.

Run `obc suites check` after changing test sources, validation commands, workflows, registries, or
test policy. Use `obc suites list` and `obc suites explain SUITE_ID` to inspect the derived
inventory; counts and durations do not belong in the registry.

For a non-Rust surface, use that surface's native focused command from its README or package
scripts. Do not run Rust gates for a Swift-, documentation-, or frontend-only change.

### 2. External-fixture checks — when captured data is part of the behavior

Tests backed by maps, routes, rides, or captured weather products are opt-in:

```sh
obc test fixtures -p obc-wx-bake
obc test fixtures -p obc-route nav_uses_grimsel
```

This syncs the `test` fixture profile, enables the `external-fixtures` feature, and still requires
an explicit Cargo scope. Use it when changing a decoder, fixture-backed behavior, the fixture
registry, or the associated scenario. Ordinary package work should stay in tier 1.

### 3. Surface gates — when a whole development surface changed

`obc check` runs only the explicitly named gates:

```sh
obc check fmt
obc check device
obc check frontend
obc check docs
obc check fmt clippy device
```

The `clippy` and `test` gates cover the complete host workspace, so prefer package-scoped Cargo
commands during development. `frontend`, `board`, `docs`, `deny`, and `wasm` are independent
surfaces; include one only when the change can affect it. Each gate prints the registry suites it
reproduces, and `obc check full` names the required suites it does not — it is not CI parity.

### 4. Full gates — exceptional and explicit

```sh
obc test full
obc check full
```

Use a full run when the change is genuinely cross-cutting: workspace manifests or lockfiles,
shared format/protocol contracts, foundational crates with many reverse dependencies, CI/dev-tool
or feature-resolution changes, a release candidate, or an explicit request. A task ending, a PR
being opened, or another agent also working in the repository is not by itself a reason to run it.
Concurrent full runs are allowed; the discipline is to start one only when its coverage is needed.

## Reclaiming stale development state

`obc clean` is the repository-owned cleanup command. It is always a dry run unless `--apply` is
present:

```sh
obc clean
obc clean --days 3
obc clean --apply
obc clean --include-builds --days 14 --apply
```

It inventories registered worktrees, their build artifacts, prunable Git metadata, and old OBC
test scratch paths. Existing worktrees are eligible only when they are linked worktrees, clean,
unlocked, merged into the configured base, not the current worktree, and older than the threshold.
The main checkout, current checkout, dirty worktrees, and unmerged work are never removed. The
default seven-day threshold also protects newly created or recently committed worktrees.

`--include-builds` additionally makes old `target/` directories in retained worktrees eligible.
Those artifacts are reproducible but expensive to rebuild, so this is opt-in even with `--apply`.
Fixture packages are managed separately with `obc fixtures prune`; cleanup never conflates the two
caches.
