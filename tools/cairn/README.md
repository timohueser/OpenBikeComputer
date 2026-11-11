# cairn

`cairn check` compares the code with the system docs in `docs/design/`. It uses the Python standard
library, the glob rule of `tools/test_plan.py` and `cargo metadata`.

CI gates only quality, with `cairn check`.

```sh
obc cairn check            # print the report
obc cairn check --update   # remove gone entries from the baseline
obc cairn check --base origin/develop   # also fail when the baseline grew since the merge base
obc cairn map              # rewrite the generated block of docs/design/README.md, and docs/design/map.json
obc cairn leaves rendering-display   # print the leaves of a system, for the skill's `document` command
obc cairn model code-review   # print the model and effort of a role in this harness
obc cairn report           # print the weekly report; --publish writes it to its issue
python3 -m unittest discover -s tools/cairn/tests
```

## Files

| File | Holds |
| --- | --- |
| `docs/design/README.md` | the system map. `cairn map` rewrites the block between its two comments. `check` fails when the block is stale. Two merged pull requests can leave it stale on `develop`; `obc cairn map` fixes it. |
| `docs/design/map.json` | the same map as data: for each system its purpose, dependencies, line count and violations, then the edges and the cycles. `cairn map` writes it, and the system explorer on the docs site shows it. Agents read it directly. `check` fails when it is stale, but ignores `lines`, `violations` and `problems`, because most code changes alter them. So those three can lag in git; the site deploy writes the file again. |
| `docs/design/<system>.md` | the header of one system or shell: `kind`, `area` (systems only), `purpose`, `owns`, `depends_on`, `test_depends_on`, `status` |
| `.cairn/config.json` | the source file types, the adapters, the tooling folder for R4, the globs of test files, the leaf limits, the skill budget, and the model roles and tiers |
| `.cairn/baseline.json` | the IDs of the known violations |
| `report.py` | `cairn report` |
| `skill/cairn/` | the agent skill: `SKILL.md` routes to one file in `reference/` for each command. `.claude/skills/cairn` and `.agents/skills/cairn` link to it. |
| `.github/workflows/cairn-report.yml` | runs `cairn report --publish` each week |
| `adapters/cargo.py` | the Rust crates and their path dependencies. Each tracked `Cargo.toml` with a `[workspace]` table is a Cargo root. Every other tracked `Cargo.toml` outside `ignore` must be a member of a root. |
| `adapters/imports.py` | the relative imports of the web source files with the `extensions` of its settings: `from`, `import`, `import()` and `new URL()`. Many systems share one npm package, so an edge joins two files. An import of a package or through an alias is not an edge. |

`owns` uses the glob rule of the test plan: `*` also matches `/`, and `**/` also matches no folder.

## Rules

| Rule | The check reports |
| --- | --- |
| Ownership | a tracked source file that no doc owns, or that two docs own. A system beats a shell: a shell owns only the files that no system owns. |
| Dependency | a production edge between two systems that `depends_on` does not list, or a test edge that neither `depends_on` nor `test_depends_on` lists. The system that owns a crate's entry file (`src/lib.rs` or `src/main.rs`) owns all of its edges. A file edge counts for the owners of the two files, and an edge from a file that matches the `tests` globs is a test edge. A doc lists what its system may use: an edge that the owner did not intend stays out of the doc and in the baseline. |
| Cycle | each production edge inside a cycle between systems. A cycle is a violation even when the docs declare its edges. |
| R1 | a Rust crate that holds files of a system other than the owner of its entry file, with the number of those files |
| R4 | a file under `tooling.path` whose owner is outside `tooling.area` |

## Ratchet

A violation whose ID is not in the baseline is new. The ID of an R1 or R4 violation names the
crate or the system, not the number of files, so more files under a known ID are not new. A new
violation fails only when it touches a system whose doc has `status: approved`. An unowned file
touches no system, so it fails when any doc has `status: approved`. Three faults fail for every
doc, approved or not: a stale map, a lost name in a Rules table, and a skill file over its budget.

A baseline entry whose violation is gone must be removed with `--update`. For an approved system
it fails like a new violation. For a draft system the report only lists it. A gone entry touches
the systems that its ID names. `--update` never adds an entry. `--accept` writes the first
baseline and refuses when the file exists.

`--base REF` fails when the baseline has an entry that the baseline at the merge base does not
have. One change may add entries: a change to `adapters` in the config, because a new adapter finds
violations that the check could not see before. In that change the baseline may also hold the
violations of HEAD: delete the file and run `--accept`. When the merge base has no baseline, the baseline must equal the one `--accept`
writes. This rule applies also while no doc is approved. The CI job uses `--base`.

With `--base`, a system is approved when its doc has `status: approved` at the merge base or at
HEAD, so a change cannot set a doc back to draft to hide its own violation.

`firmware/tools/check_dependencies.py` is a separate check. It enforces the production direction
between its own crate groups, which do not follow the system cut.

## Rules tables

`check` reads the `## Rules` section of each doc. In the last column of its table, a name between
backticks in snake case with a `_` is a test or a function. `check` fails when no file that the same
cell links holds the name. So a change that renames or removes a cited test must update the doc.

## Leaves

`leaves` cuts the files that a system owns into leaves of at most `leaf_lines` lines and
`leaf_files` files. It leaves out the files that match the `tests` globs or an adapter's `ignore`
globs: the doc describes the system, and the draft of `document` links the tests as evidence for
the rules. A leaf stays inside one crate. A folder that is too large is cut by its entries, and
small entries of one folder share a leaf. A file `x.rs` stays with its folder `x/`. A file larger
than the line limit is a leaf of its own.

`skill/cairn/NOTICE` names the source and licence of the adapted Goldfish prompts.

The output names every repository crate on one line. Each leaf gives its `level`, the folder at
which it was cut, and the repository crates that its crate uses, from the cargo adapter. A file in
no crate has no such list.

## Skill budget

`check` fails when a skill file is over its budget in `skill`: `router_tokens` for `SKILL.md`,
`reference_tokens` for each file in `reference/`. A token is four bytes. `tools/prose.py` does not
budget the skill files; it checks only their links.

`skill` is optional; `leaves` needs `leaf_lines` and `leaf_files`.

## The `Mode:` line

The pull request template has a `Mode:` line: `change #N`, `fix` or `skip: <reason>`. It is a record
for the weekly report, and nothing checks it. The report reads the first `Mode:` line outside code
blocks and HTML comments, and does not count an unknown value.

## Models

| Key or input | Gives |
| --- | --- |
| `roles.<role>` | the `tier` (`strong`, `balanced`, `fast` or `inherit`) and the `effort` of a job |
| `tiers.<harness>` | the model of each tier for `claude-code` and `codex` |
| the harness | `--harness`, else `claude-code` when `CLAUDECODE=1`. Codex sets no such variable. An unknown harness or the tier `inherit` prints `inherit`. |

## Report

The report reads git and GitHub and costs no tokens.

| Part | Source |
| --- | --- |
| Skips | the pull requests merged in the last 7 days with `Mode: skip: <reason>`, and their reasons, read through `gh api` |
| Ratchet, files with no owner | the analysis of `cairn check` |
| System docs to check | For each doc with `status: approved`: the `Merge pull request #N` commits on the first-parent line of HEAD after the last change to the doc. A commit counts when it changes files of the system outside `tests` and the map. |
| Mentions | Each `@` in a title or a reason gets a zero-width space after it, so the issue notifies nobody. |
| `--publish` | Replaces the body of the open issue labelled `cairn-report`, or creates the label and the issue and pins it. Pinning uses GraphQL; a failed pin adds a note to the body. |
| Exit code | 1 when a part fails |
