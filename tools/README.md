# Tools

The developer workflow: the `obc` command, test selection and `obc ready`, the CI scripts, the
guards, fixture and licence tooling, and the verification console. An agent that changes how the
repository builds, tests or gates starts here.

This directory also holds Python that other areas start. The planner bake scripts
(`planner_*.py`), `wiki/`, `trailview/`, `forest_reference.py` and `clms.py` are steps of
`obc data`: the engine in `host/obc-data-steps` runs them as `python -m tools.<module>` with the
request on standard input. Their product logic belongs to `host/` and `data/`.

## Where to start

| Topic | Files |
| --- | --- |
| The `obc` command | `obc` (the wrapper), `justfile` (every task), `obc-dev.sh` (the shell helpers of the recipes), `tasks.py` (the listing) |
| Test selection | `test_plan.py` (the selector and the table of CI jobs), `testing/suites.toml` (what Cargo cannot see), `ready.py` (`obc ready`), `ci/*.sh` (the body of each CI job), `ci_aggregate.py` (the final CI gate) |
| Guards | `check_paths.py` (pages, links, named tasks), `check_data_sources.py`, `check_obc_data_licence.py`, `check_fixture_policy.py`; the firmware guards are in `firmware/tools/` |
| Reading GitHub | `ci_log.py` (`obc ci`), the `pr` recipe, `verification/req.py` (`obc req`) |
| Fixtures | `fixtures.py` (see [`fixtures/README.md`](../fixtures/README.md)), `fixture_catalog.py` and `loopback_port.mjs` for the browser suites |
| Steps of `obc data` | `step_request.py`, `step_pool.py`, `data_registry.py`; `planner_runtime.py` is the hub of the planner bake |
| Boards and machines | `board.py`, `install-geos.sh`, `install-riscv.sh`, [`vps/`](vps/README.md) |
| Licences | `licenses/` (`gen-third-party.sh`, `deny.toml`) |
| The weekly dream job | `dream/` (`collect.py`, `brief.md`, `publish.py`), [`dream.yml`](../.github/workflows/dream.yml) |
| Verification console | [`verification/`](verification/README.md), a SvelteKit app; [`verification/ops/`](verification/ops/README.md) is the server runbook |

[`docs/testing.md`](../docs/testing.md) explains routes, the fields of `suites.toml`, fixtures
and CI artifacts. This page does not repeat it.

## Run and test

```sh
obc                          # the tasks by group; obc --agent, obc --all, obc help TASK
obc test -p CRATE            # one package on nextest, then its doctests
obc ready                    # before a push: format, lint, docs and the suites the change selects
obc test affected --base origin/develop --dry-run   # what a change selects, with the reason
obc suites check             # the test plan against Cargo, the tree and ci.yml
obc paths                    # the page check
obc check NAME               # run tools/ci/NAME.sh as CI does
obc exec CMD                 # any command with the environment of a task
obc ci                       # the failures of the last CI run of the branch
obc pr view                  # also list, body, comments, checks
obc clean                    # a dry run; --apply removes old scratch and cargo artifacts
uv run --locked --group dev --group planner-snow --group forest-reference --group route-sources \
  python -m unittest discover -s tools/tests    # the tests of this directory
```

`obc` needs `just`; the plan tools need `uv`; `obc ci` and `obc pr` need `gh`. `obc doctor`
checks the toolchain, and `obc setup` links `obc` into `~/.local/bin`.

## How `obc` works

`tools/obc` selects the checkout that holds the current directory and prints its root, so one
installed `obc` serves every worktree. Each recipe calls `obc_init`, which sources
`tools/obc.local` (operator settings; a linked worktree reads the file of the main checkout),
and gives each checkout its own Cargo target under `$CARGO_TARGET_DIR/checkouts/`. A shared
target would give one checkout the build of another, because Cargo treats an older source as
fresh. `obc exec` gives a raw command the same environment and GEOS.

## How a change selects tests

`test_plan.py` reads `cargo metadata` of the four Cargo roots. Each Rust package is a unit, and
a changed package selects the packages that depend on it. `suites.toml` adds what Cargo cannot
see: suites that are not Rust, and path triggers. The same selector runs locally and in CI.

- A `required` suite runs whenever one of its CI jobs starts. The job `guards` is unconditional,
  so its suites run on every pull request: the guards and the tests of `tools/`.
- A change of a policy path (the workflows, `testing/`, `ci/`, `test_plan.py`, the justfile,
  `obc`, `docs/testing.md`) or of a Rust foundation file (`Cargo.lock`) selects the whole graph.
  `obc ready` then runs only the suites that build nothing and leaves the rest to CI.
- Selection fails closed. A selector error publishes no plan, and the final job `ci` fails on a
  missing plan and on a selected suite that did not run.

## Gotchas

- A new source file outside every Cargo package and every trigger fails with "changed
  production path has no owner". Add a trigger in `suites.toml`; do not skip the check.
- A deleted file that a trigger names fails `obc suites check`. A CI job exists in three
  places that change together: `JOBS` in `test_plan.py`, the job in `ci.yml`, and the `needs`
  of the job `ci`.
- `obc ready` and several suites call `obc` by name. It must be on PATH; `./tools/obc ready`
  alone is not enough.
- Uncommitted and untracked files are part of the selection.
- The format gate of `obc ready` writes files and then stops. Commit them and run it again.
- `obc test -p` uses default features; the fast tier of CI uses `--all-features`. A test behind
  a feature can pass locally because it did not run.
- `obc check test` is the whole-workspace CI body, not a focused run.
- Never print `tools/obc.local` or the files in `~/.config/openbikecomputer/`. Secrets belong
  in `~/.config/openbikecomputer/credentials.env`, not in `obc.local`: every task exports the
  variables of `obc.local`.
- The scripts here import each other as the package `tools`. Run them from the repository root,
  in the uv group that `pyproject.toml` gives them.
- `obc pr comments` holds the conversation only. Review threads on lines need the GitHub
  connector.
- `obc clean --days` below 2 can remove Cargo units that are in use.
- `obc data update prod`, `obc data deploy` and the console's `publish` act on production. See
  [`data/README.md`](../data/README.md) and [`planner/README.md`](../planner/README.md).
- `check_paths.py` reads backticked paths in `AGENTS.md` and the READMEs. A path with two or
  more segments that starts at a real directory must exist; a build output must be ignored by
  git. A rename can so fail on a page far from the change: fix the page in the same change.
- The dream step has the API key only, and no shell and no network. `tests/test_dream_workflow.py`
  holds these guards.
- The next dream run finds its start from the head branch dream/pages and the line
  `Dream window: <since> to <through>` in its body. Do not rename either.
- The concurrency group `dream` keeps one waiting run. A third start replaces the waiting one.
- A dream step that reaches its timeout writes no result, so its report has no cost. The
  Console of the API key shows it.
- A new workflow that publishes goes into `PUBLICATION_WORKFLOWS` of `test_plan.py`. Otherwise
  each change of it selects the whole graph.

## Decisions

- Cargo's graph is the truth about Rust; `suites.toml` holds only what Cargo cannot see.
- One selector for the local run and for CI, so the two cannot disagree.
- A selection of the whole graph is CI's work: it does not say that one machine should compile
  the whole repository again.
- Each script of `ci/` is the one definition of the steps of its CI job; `obc check NAME` runs the
  same script.
- `obc ci` exists because the raw log of a red job is about a megabyte. Never read a raw log.
- The page check runs on every pull request, not only on a change of a page: a page names
  paths of the whole tree, so a rename anywhere can break one.
- A README change selects the suite `ci.docs` only, apart from the unconditional guards.
- Licences are per artifact: the host tools ship as source, and the web bundle writes its own
  notices.
- Requirement links (`obc req`, the `Requirements:` line of a pull request) belong to the
  verification console; [`verification/README.md`](verification/README.md) has the flow.
