# cairn

`cairn check` compares the code with the system docs in `docs/design/`. It uses the Python standard
library, the glob rule of `tools/test_plan.py` and `cargo metadata`.

```sh
obc cairn check            # print the report
obc cairn check --update   # remove gone entries from the baseline, and lower sizes that shrank
obc cairn check --base origin/develop   # also fail when the baseline grew since the merge base
obc cairn check --pr --base origin/develop   # also check the diff against the mode
obc cairn mode change 3012 # declare the mode of the current branch
obc cairn mode             # print the mode and the `Mode:` line for the pull request body
obc cairn hash issue.md    # print the approval hash of an issue body (- reads stdin)
python3 -m unittest discover -s tools/cairn/tests
```

## Files

| File | Holds |
| --- | --- |
| `docs/design/<system>.md` | the header of one system or shell: `kind`, `area` (systems only), `purpose`, `owns`, `depends_on`, `test_depends_on`, `status` |
| `.cairn/config.json` | the source file types, the adapters, the runtime table for R2, the tooling folder for R4, the size limit of a fix and the globs of test files |
| `.cairn/baseline.json` | the known violations, by ID, with a size |
| `mode.py` | the modes, the approval hash, the rules of the mode gate and the edit hook |
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
`status: approved`. While no doc is approved, the check only reports.

A baseline entry that shrank or disappeared must be recorded with `--update`. For an approved
system it fails like a new violation, so that the violation cannot grow back unseen. For a draft
system the report only lists it. A gone entry touches the systems that its ID names. `--update` never adds an entry and never raises a size. `--accept` writes the first
baseline and refuses when the file exists.

`--base REF` fails when the baseline has an entry or a size that the baseline at the merge base
does not have. When the merge base has no baseline, the baseline must equal the one `--accept`
writes. This rule applies also while no doc is approved. The CI job uses it.

`firmware/tools/check_dependencies.py` is a separate check. It enforces the production direction
between its own crate groups, which do not follow the system cut.

## Mode gate

`cairn mode` writes the mode to the local git config as `branch.<name>.cairnMode`. Each worktree
of the branch reads it, and nothing is committed. `--pr` reads the mode from the `Mode:` line of
the pull request body when `GITHUB_EVENT_PATH` is set, and from the branch otherwise. The diff is
HEAD against the merge base with `--base`.

| `Mode:` line | The diff must |
| --- | --- |
| `fix` | touch one system or shell, add no dependency between systems, change no doc header and no file under `specs/`, and change at most `fix.max_lines` lines outside the `fix.tests` globs |
| `fix, over limit: <reason>` | do the same, with no size limit |
| `change #N` | touch only the systems on the `**Systems:**` line of issue N, by slug or title. The issue has `Status: approved` and an `Approved: <hash>` line that `cairn hash` gives for its body. A new dependency `a -> b` changes `docs/design/a.md` too. |
| `prototype` | not exist: the gate always fails, because a prototype never merges |
| `override: <reason>` | nothing: the gate passes, and the body keeps the reason |

A changed doc touches its system. A violation fails only when the diff touches a system whose doc
has `status: approved`; otherwise the gate only reports it. A missing `Mode:` line is a violation.
The dependency rule reads the graph of the base from a temporary git worktree. `change #N` reads
the issue through the REST API with `GITHUB_TOKEN` in CI, and through `gh api` locally.

The approval hash is the first 12 hex digits of SHA-256 over the body without its `Status:` and
`Approved:` lines, with trailing whitespace removed from each line and from the end.

## Edit hook

`.claude/settings.json` runs `cairn.py hook` before each Edit, Write, MultiEdit and NotebookEdit
tool call. It blocks an edit of a source file inside the repository while the branch has no
declared mode, and asks the agent to get the mode from the user. A detached HEAD passes.
