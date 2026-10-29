# cairn

`cairn check` compares the code with the system docs in `docs/design/`. It uses the Python standard
library, the glob rule of `tools/test_plan.py` and `cargo metadata`.

```sh
obc cairn check            # print the report
obc cairn check --update   # remove gone entries from the baseline, and lower sizes that shrank
obc cairn check --base origin/develop   # also fail when the baseline grew since the merge base
obc cairn check --pr --base origin/develop   # check only the diff against the mode
obc cairn map              # rewrite the generated block of docs/design/README.md
obc cairn mode change 3012 # declare the mode of the current branch
obc cairn mode             # print the mode and the `Mode:` line for the pull request body
obc cairn hash issue.md    # print the approval hash of an issue body (- reads stdin)
obc cairn model code-review   # print the model and effort of a role in this harness
obc cairn report           # print the weekly report; --publish writes it to its issue
python3 -m unittest discover -s tools/cairn/tests
```

## Files

| File | Holds |
| --- | --- |
| `docs/design/README.md` | the system map. `cairn map` rewrites the block between its two comments. `check` fails when the block is stale. Two merged pull requests can leave it stale on `develop`; `obc cairn map` fixes it. The mode gate does not count the map. |
| `docs/design/<system>.md` | the header of one system or shell: `kind`, `area` (systems only), `purpose`, `owns`, `depends_on`, `test_depends_on`, `status` |
| `.cairn/config.json` | the source file types, the adapters, the runtime table for R2, the tooling folder for R4, the size limit of a fix, the globs of test files, the size thresholds of a change, the model roles and tiers, and the report settings |
| `.cairn/baseline.json` | the known violations, by ID, with a size |
| `mode.py` | the modes, the approval hash, the rules of the mode gate and the edit hook |
| `report.py` | `cairn report` |
| `.github/workflows/cairn-report.yml` | runs `cairn report --publish` each week. Its environment `cairn-report` holds the `ANTHROPIC_API_KEY` secret. |
| `adapters/cargo.py` | the Rust crates and their path dependencies. Each tracked `Cargo.toml` with a `[workspace]` table is a Cargo root. Every other tracked `Cargo.toml` outside `ignore` must be a member of a root. |

`owns` uses the glob rule of the test plan: `*` also matches `/`, and `**/` also matches no folder.

## Rules

| Rule | The check reports |
| --- | --- |
| Ownership | a tracked source file that no doc owns, or that two docs own. A system beats a shell: a shell owns only the files that no system owns. |
| Dependency | a production edge between two systems that `depends_on` does not list, or a test edge that neither `depends_on` nor `test_depends_on` lists. The system that owns a crate's entry file (`src/lib.rs` or `src/main.rs`) owns all of its edges. |
| Cycle | each production edge inside a cycle between systems. A cycle is a violation even when the docs declare its edges. |
| R1 | a Rust crate that holds files of a system other than the owner of its entry file, with the number of those files |
| R2 | a system with code under two roots of the same runtime in the `runtimes` table. The longest matching root applies. The check ignores a file under no root. |
| R3 | each shell's own lines: the lines of the files that no system owns. A shell's doc states its target size. |
| R4 | a file under `tooling.path` whose owner is outside `tooling.area` |

## Ratchet

A violation that is not in the baseline is new. An R1, R3 or R4 violation whose size (files or
lines) is above its baseline size is also new. A new violation fails only when it touches a system
whose doc has `status: approved`. An unowned file touches no system, so it fails when any doc has
`status: approved`. While no doc is approved, the check only reports, but a stale map fails.

A baseline entry that shrank or disappeared must be recorded with `--update`. For an approved
system it fails like a new violation. For a draft system the report only lists it. A gone entry touches the systems that its ID names. `--update` never adds an entry and never raises a size. `--accept` writes the first
baseline and refuses when the file exists.

`--base REF` fails when the baseline has an entry or a size that the baseline at the merge base
does not have. When the merge base has no baseline, the baseline must equal the one `--accept`
writes. This rule applies also while no doc is approved. The CI job uses it.

`firmware/tools/check_dependencies.py` is a separate check. It enforces the production direction
between its own crate groups, which do not follow the system cut.

## Mode gate

`cairn mode` writes the mode to the local git config as `branch.<name>.cairnMode`. Each worktree
of the branch reads it, and nothing is committed. `--pr` runs only the mode gate, not the ratchet.
It reads the mode from the `Mode:` line of the pull request body when `GITHUB_EVENT_PATH` is set,
and from the branch otherwise. The diff is HEAD against the merge base with `--base`. The workflow
`.github/workflows/cairn-mode.yml` runs it on each pull request, also when the body changes.

| `Mode:` line | The diff must |
| --- | --- |
| `fix` | touch one system or shell, add no dependency between systems, change no doc header and no file under `specs/`, and change at most `fix.max_lines` lines outside the `fix.tests` globs |
| `fix, over limit: <reason>` | do the same, with no size limit |
| `change #N` | touch only the systems on the `**Systems:**` line of issue N, outside code blocks and HTML comments, by slug or title. The first `Status:` line of the issue is `Status: approved`, and an `Approved: <hash>` line holds the hash that `cairn hash` gives for its body. N must be an issue, not a pull request. A new dependency `a -> b` changes `docs/design/a.md` too. The size of the diff is not larger than the `**Size:**` line of issue N. |
| `prototype` | not exist: the gate always fails, because a prototype never merges |
| `override: <reason>` | nothing: the gate passes, and the body keeps the reason |

A changed system doc touches only its own system. A violation fails only when the diff touches a
system whose doc has `status: approved` at the base or at HEAD; otherwise the gate only reports
it. A missing `Mode:` line is a violation.

The gate reads the base from a temporary git worktree: its dependency graph, its approvals, and the
`fix`, `size` and `specs` keys of its config. When the base config has no `fix` or `size`, the gate
uses the defaults in `mode.py`, which equal the config of this repository, never the HEAD config. A
change therefore cannot loosen the rules it is checked against. When the base cannot be read, a
`fix` or `change` gets a violation that says so. A test dependency that becomes a production
dependency is a new dependency.

A `fix.tests` glob with no `/` matches the file name only. `change #N` reads the issue through
`gh api`, which uses `GH_TOKEN` and `GH_REPO` in CI.

### Size of a change

The size of the diff is the largest size that one of its signals gives:

| Signal | Size |
| --- | --- |
| systems and shells touched | S up to `size.S.systems`, M up to `size.M.systems`, else L |
| changed lines outside the `fix.tests` globs | S up to `size.S.lines`, M up to `size.M.lines`, else L |
| a changed file under `specs/` | at least M |
| a new dependency between systems | at least M |
| a new system doc | L |

The gate takes the letter after `**Size:**` on its line in the issue, outside code blocks and HTML
comments. An issue with no such line is a violation. A diff larger than the issue is a violation
that names both sizes and the signals that decide. The way out is to shrink the pull request, amend
the issue and approve it again, or override with a reason. The gate cannot see a breaking contract
change, so the people who size the issue make it L.

### Epics

`change #N` asks GitHub for the parent of issue N through the sub-issue relation
(`GET /repos/{owner}/{repo}/issues/N/parent`). When N has a parent, the parent is the epic: the
epic, not issue N, must be approved and unchanged, and each system on the Systems line of N must be
on the Systems line of the epic. The epic must be an issue, not a pull request. The epic lists `#N`
as an item (`- #N`, `- [ ] #N`) outside code blocks and HTML comments, also unclosed ones, so its
approval covers the list. A sub-issue is one pull request, so its `**Size:**` line is S or M.

The approval hash is the first 12 hex digits of SHA-256 over the body without its `Status:` and
`Approved:` lines, with trailing whitespace removed from each line and from the end.

## Edit hook

`.claude/settings.json` runs `cairn.py hook` before each Edit, Write, MultiEdit and NotebookEdit
tool call. It blocks an edit of a source file inside the repository while the branch has no
declared mode, and asks the agent to get the mode from the user. A detached HEAD passes. Edits made
through Bash bypass the hook; AGENTS.md and the mode gate cover them.

## Models

| Key or input | Gives |
| --- | --- |
| `roles.<role>` | the `tier` (`strong`, `balanced`, `fast` or `inherit`) and the `effort` of a job |
| `tiers.<harness>` | the model of each tier for `claude-code`, `codex` and `api`. An `api` model is a family prefix, or `{"family": ..., "pin": "<model ID>"}`. |
| the harness | `--harness`, else `claude-code` when `CLAUDECODE=1`. Codex sets no such variable. An unknown harness or the tier `inherit` prints `inherit`. |

## Report

| Part | Source |
| --- | --- |
| Overrides | the pull requests merged in the last 7 days with `Mode: override: <reason>`, read through `gh api` |
| Ratchet, files with no owner | the analysis of `cairn check` |
| Drift, step 1 | for each doc with `status: approved`, each `Merge pull request #N` commit on the first-parent line of HEAD after the last change of the doc that touches files of the system outside `fix.tests` and the map. A `change` gives the TL;DR of its issue, another mode its diff on those files. `report.max_input_chars` caps the input of a system. |
| Drift, step 2 | one Message Batch for the `drift` role in the `api` tier, skipped without `ANTHROPIC_API_KEY`. The family resolves to its newest model ID through the Models API, unless `pin` is set. The fixed instructions and `docs/design/glossary.md` come first, with a cache breakpoint. |
| Budget | the token-counting endpoint counts each request before it joins the batch. The worst case is the input at twice the input price (a one-hour cache write) plus `report.max_tokens` of output. The batch stops at `report.budget_usd`, and the report names the skipped systems. |
| `report.batch_prices` | the Message Batches price in USD per million tokens of the newest model of each family. Change it when a family gets a new model. |
| `report.timeout_minutes` | the longest wait for the batch. The batch is then canceled. |
| `--publish` | replaces the body of the open issue labelled `cairn-report`, or creates the label and the issue and pins it. Pinning uses GraphQL; a failed pin adds a note to the body. |
| Exit code | 1 when a part cannot be read |
