# Developing OpenBikeComputer

The routes, the suite-granularity rule, the plan documents, the exception policy and the change
selection table live in [`docs/testing.md`](docs/testing.md). In short: a suite declares one of
four routes — `ordinary` (the change selects it), `required` (it runs whenever one of its CI jobs
starts), `manual` (its own command only) and `live` (it contacts a live service). Captured
fixtures are ordinary work behind an explicit sync, `test-weekly.yml` names its two commands
directly, and physical procedures stay in their owning issue. Selection is per suite, never per
test function.

## Selecting a checkout

`obc` is a global entry point for humans and agents. Inside an OBC checkout or linked worktree,
it uses that checkout's tools. Outside OBC, including inside another project, it uses the checkout
where `obc` was installed. Main-checkout use stays the same. Relative input paths still start at
the caller's current directory. Each invocation prints the selected checkout on stderr.

Run commands inside the task's worktree and check that printed path. With an older installed
wrapper, use `./tools/obc` from the worktree root until the installed checkout has this change.

## Task groups

Each `obc` task declares a group. `obc` lists every group but `agent`, so the everyday list stays
short, and the last line points at the rest. `obc --agent` lists the agent tasks and `obc --all`
lists all of them. `obc help TASK`, or `obc TASK --help`, prints the whole comment block above
the recipe. Every task runs by name, whatever its group. Bash completion offers the tasks
outside the `agent` group; `OBC_COMPLETE_ALL=1` offers all of them.

A task belongs to `agent` when an automation is its main user. Give each new task a
`[group('...')]` attribute in `tools/justfile` and a comment block whose last line is the
one-sentence summary the listing shows; the lines above it are the help text.

For concurrent browser sessions, set `OBC_BROWSER_PORT` to a free port before the builder or web
demo browser suite. For a separate builder frontend, start the backend with `obc web --port 8001`
and Vite with `OBC_BUILDER_PORT=8001 npm run dev -- --port 5174 --strictPort` from `builder/app`.
Ports must be distinct for simultaneous sessions; the defaults stay unchanged.

## Verification is proportional to the change

The normal development loop is **scoped verification**, not the complete repository gate. Every
developer and coding agent should inspect the files it changed, exercise the smallest command that
covers those files, and report exactly what it ran. CI remains the cross-repository backstop.

Before a push, `obc ready` assembles that list for you. It reads the changed paths, prints one line
and one reason for each gate — the skipped gates included — runs the gates it selects, and ends
with a pull-request skeleton. `obc ready --dry-run` prints the plan and stops. A gate whose work is
a declared suite is skipped and names the suite, because `obc test affected` already runs it. The
format gate writes: if it rewrites a file, the command names the file and stops, because the tree
is no longer the tree you were about to push. Commit the file and run `obc ready` again.

A foundation input such as `Cargo.toml`, or a change to the test policy, selects the graph as a
whole. That run is CI's, so `obc ready` does not start it. It keeps the selected suites that
build nothing — the Python checks and guards — gives each one a line, and names every other suite
as left to CI. A check that costs a fraction of a second then stays visible instead of hiding
behind a run of the complete suite. Those suites are the ones CI runs too, so install their
reporters first with `pip install -r tools/requirements-test.txt`; a missing module stops the
run at that gate.

```sh
obc ready --dry-run
obc ready --base origin/develop
```

### 1. Focused checks — the default

Run the directly affected tests while iterating and the affected package before handoff:

```sh
obc test -p obc-app
cargo clippy -p obc-app --all-targets -- -D warnings
```

`obc test` deliberately requires a scope. It does not silently expand to the workspace. Multiple
affected packages may be supplied with repeated `-p` arguments.

When the change spans packages, ask the plan what it selects instead of guessing:

```sh
obc test affected --base origin/develop --dry-run
obc test affected --base origin/develop
```

`affected` is the same selection CI runs. It prints the selected units and one reason each, and
`--dry-run` executes nothing.

Run `obc suites check` after changing test sources, validation commands, workflows, the plan
documents, or test policy. `testing/suites.toml` holds only what Cargo cannot see; packages,
dependency edges, test binaries, counts and durations are all derived.

For a non-Rust surface, use that surface's native focused command from its README or package
scripts. Do not run Rust gates for a Swift-, documentation-, or frontend-only change.

### 2. External-fixture checks — when captured data is part of the behavior

Tests backed by maps, routes, or rides are opt-in:

```sh
obc test fixtures -p obc-route
```

This syncs the `test` fixture profile, enables the `external-fixtures` feature, and still requires
an explicit Cargo scope. Use it when changing a decoder, fixture-backed behavior, the fixture
catalog, or the associated scenario. Ordinary package work stays with the focused checks above.

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
surfaces; include one only when the change can affect it. Each gate prints the suites it
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

`obc test full` runs ordinary binaries, captured-fixture binaries and doctests in separate
commands.
`obc check test` runs `tools/ci/test.sh`, the same file the CI `test` job runs: the fast tier,
doctests, the default-feature formats shape and the render contract. Every command here, focused
ones included, needs the CI-pinned runner (`cargo install cargo-nextest --version 0.9.143
--locked`). `obc test -p <crate>` runs that scope on nextest and then its doctests; add `--lib`
for the library target alone.

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

Cargo never removes build artifacts it no longer uses, so every `target/` grows with each
dependency bump, feature set, and toolchain update. `--include-builds` sweeps the artifacts that
cargo has not rewritten within the threshold from every `target/`, the main checkout and locked
agent worktrees included. The sweep is safe by construction: cargo recompiles whatever is missing,
so a swept artifact costs a rebuild, never a stale binary. It holds each profile directory's
`.cargo-lock` while it removes entries and skips a directory where a build is running.

`--include-builds` additionally makes old `target/` directories in retained worktrees eligible.
Those artifacts are reproducible but expensive to rebuild, so this is opt-in even with `--apply`.
Fixture packages are managed separately with `obc fixtures prune`; cleanup never conflates the two
caches.
