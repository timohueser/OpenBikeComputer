# cairn

`cairn check` compares the code with the system docs in `docs/design/`. It uses the Python standard
library, the glob rule of `tools/test_plan.py` and `cargo metadata`.

CI gates only quality, with `cairn check`.

```sh
obc cairn check            # print the report
obc cairn check --update   # remove gone entries from the baseline, and lower sizes that shrank
obc cairn check --base origin/develop   # also fail when the baseline grew since the merge base
obc cairn map              # rewrite the generated block of docs/design/README.md, and docs/design/map.json
obc cairn leaves rendering-display   # print the leaves of a system, for the skill's `document` command
obc cairn model code-review   # print the model and effort of a role in this harness
obc cairn report           # print the weekly report; --publish writes it to its issue, --drift runs step 2
python3 -m unittest discover -s tools/cairn/tests
```

## Files

| File | Holds |
| --- | --- |
| `docs/design/README.md` | the system map. `cairn map` rewrites the block between its two comments. `check` fails when the block is stale. Two merged pull requests can leave it stale on `develop`; `obc cairn map` fixes it. |
| `docs/design/map.json` | the same map as data: for each system its purpose, dependencies, line count and violations, then the edges and the cycles. `cairn map` writes it, and the system explorer on the docs site shows it. Agents read it directly. `check` fails when it is stale, but ignores `lines`, `violations` and `problems`, because most code changes alter them. So those three can lag in git; the site deploy writes the file again. |
| `docs/design/<system>.md` | the header of one system or shell: `kind`, `area` (systems only), `purpose`, `owns`, `depends_on`, `test_depends_on`, `status` |
| `.cairn/config.json` | the source file types, the adapters, the runtime table for R2, the tooling folder for R4, the globs of test files, the `specs` folder, the size thresholds of a change, the leaf limits, the skill budget, the model roles and tiers, and the report settings |
| `.cairn/baseline.json` | the known violations, by ID, with a size |
| `report.py` | `cairn report` |
| `skill/cairn/` | the agent skill: `SKILL.md` routes to one file in `reference/` for each command. `.claude/skills/cairn` and `.agents/skills/cairn` link to it. |
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
writes. This rule applies also while no doc is approved. The CI job uses `--base`.

With `--base`, a system is approved when its doc has `status: approved` at the merge base or at
HEAD, so a change cannot set a doc back to draft to hide its own violation.

`firmware/tools/check_dependencies.py` is a separate check. It enforces the production direction
between its own crate groups, which do not follow the system cut.

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
| `tiers.<harness>` | the model of each tier for `claude-code`, `codex` and `api`. An `api` model is a family prefix such as `claude-sonnet`, or an exact model ID, which is a pin. |
| the harness | `--harness`, else `claude-code` when `CLAUDECODE=1`. Codex sets no such variable. An unknown harness or the tier `inherit` prints `inherit`. |

## Report

| Part | Source |
| --- | --- |
| Skips | the pull requests merged in the last 7 days with `Mode: skip: <reason>`, and their reasons, read through `gh api` |
| Size estimates | each `change #N` pull request merged in the last 7 days: the `**Size:**` line of issue N, outside code blocks and HTML comments, and the size of the merge commit against its first parent. System ownership and the config are today's, not those of the merge. An unreadable size is `unknown`, with the reason. |
| Ratchet, files with no owner | the analysis of `cairn check` |
| Drift, step 1 | For each doc with `status: approved`: the `Merge pull request #N` commits on the first-parent line of HEAD after the last change to the doc. A commit counts when it changes files of the system outside `tests`, `report.ignore` and the map. A `change #N` gives the TL;DR of issue N. Any other pull request gives its diff on those files. `report.max_input_chars` caps the input of a system. |
| Drift, step 2 | Runs only with `--drift`, because it spends tokens. It needs `ANTHROPIC_API_KEY`. It sends one Message Batch for the `drift` role in the `api` tier. A family prefix resolves to its newest model ID through the Models API. The fixed instructions and `docs/design/glossary.md` come first, with a cache breakpoint. |
| Budget | The token-counting endpoint counts each request before it joins the batch. The worst case is the shared prefix at the one-hour cache write price (twice the input price), the rest at the input price, and `report.max_tokens` at the output price. The batch stops at `report.budget_usd`, and the report names the skipped systems. The report also gives the real cost from the usage of the results. |
| `report.batch_prices` | The Message Batches prices in USD per million tokens, by exact model ID. Step 2 refuses a model with no price. |
| `report.timeout_minutes` | The longest wait for the batch. Then the batch is canceled, and the report still reads the results of the requests that ran. While it waits, a network error, HTTP 429 or HTTP 5xx tries again. Any other HTTP error stops the run and names the batch. |
| Mentions | Each `@` in a title, a reason or a finding gets a zero-width space after it, so the issue notifies nobody. |
| `--publish` | Replaces the body of the open issue labelled `cairn-report`, or creates the label and the issue and pins it. Pinning uses GraphQL; a failed pin adds a note to the body. |
| Exit code | 1 when a part fails |

### Size of a change

The size of a merge is the largest size that one of its signals gives:

| Signal | Size |
| --- | --- |
| systems and shells touched | S up to `size.S.systems`, M up to `size.M.systems`, else L |
| changed lines outside the `tests` globs | S up to `size.S.lines`, M up to `size.M.lines`, else L |
| a changed file under `specs/` | at least M |
| a new system doc | L |

A changed system doc touches only its own system. The map counts for no system and no lines. A
`tests` glob with no `/` matches the file name only. A pull request that cites an L issue counts as
"against an epic", because it is one part of that epic. The report does not see a new dependency
between systems or a breaking contract change.
