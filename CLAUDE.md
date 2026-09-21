## Overview
We are building an open source bikepacking GPS computer. The brain of the OpenBikeComputer is the **NRF54LM20**.

We want to build software that feels obvious and intuitive, both for the user and the developers working on it.
We never want to preserve complexity, just because it already exist, instead we strive to build the simplest possible systems. The ideal feature implementation is one that you look at and think: "Wow, I did not think we could implement this so simply and straightforward".

## Coding Preferences
The goal for this codebase is to make this a robust and extendable open source mono-repo, with clear architectural seams and boundaries. We put a great deal of thought and effort into building our features and modules in ways that makes them easy to understand and reason about.

- **Keep things simple, adhere to YAGNI and DRY principles** This is a large codebase and we don't want it to grow uncontrollably. Make sure large LOC additions you make are well justified.
- Don't be afraid to tell me if you notice a system we built forces you to write verbose or "work around" code. We can always take a step back and reevaluate even large architectural decisions.
- Prefer small, well-bounded modules over growing monoliths.
- Make the smallest change that meets the request. Do not add adjacent features without approval.
- Test are good! But avoid endless smoke thests and too many "regression tests". Write focused, high quality tests, never add tests just for the sake of adding them.
- **Keep code and comments concise, current, and focused on the present behavior.** A comment states an invariant, a non-obvious reason or a gotcha. It never says what the code plainly does, what the code used to do, or which issue changed it.
- None of this project is deployed to consumers yet, there is no need to keep backwards compatability, or write migration systems if we change any datastructure or file format. Breaking changes are fine at this stage of developement. "This will break the old format" is never an argument against making a change.

## Repository layout

The layout table is in [README.md](README.md#repository-layout). The root Cargo workspace holds
the `firmware/`, `host/` and `apps/` crates. Keep device-reachable dependencies in `firmware/`.
The nRF54L board image, the bootloader and the Tauri desktop app are standalone Cargo roots; build
and test them from their own directories. The nearest README has the surface's setup;
`companion-ios/CLAUDE.md` is the iOS on-ramp.

## Build and verification

- Run `obc` inside the task's checkout and check the printed root. `obc help TASK` describes a task.
- Test the crates you changed, never the whole workspace by default:

  ```sh
  obc test -p <crate>
  cargo clippy -p <crate> --all-targets -- -D warnings
  ```

- `obc test fixtures -p <crate>` is for captured external data only. For a non-Rust surface, run
  that surface's own focused command from its README. Select whole suites, never single tests.
- For a change across packages, `obc test affected --base origin/develop` is the selection CI
  runs; `--dry-run` prints the plan. `obc ready` runs the gates a change selects before a push.
- `obc test full` or `obc check full` only for cross-cutting changes: workspace manifests, shared
  contracts, foundational crates, CI or tooling, releases.
- `obc suites check` after changing test sources, suite commands, workflows or test policy.
  [docs/testing.md](docs/testing.md) explains the plan documents.
- `cargo fmt --all` for the workspace, and `cargo fmt` in each standalone Cargo root.
- Report the exact checks you ran and the ones you left out. `obc clean` removes stale state;
  read its dry run before `--apply`.

## How reviews work

A pull request gets one review round; a re-review covers the delta. Do not demonstrate a test
against deliberately broken code unless the reviewer asks for one specific case. Do not mirror
the CI suite locally before a push; run the affected suites, CI is the gate. `obc ci` prints a red
run's failures; never read a raw CI log. Resource figures come from one head build compared
against `resource_baseline.json`; never rebuild the base.

## What gets recorded where

Every durable artifact answers one question. If it answers two, it is in the wrong place.

| Kind | Answers | Lives in |
| --- | --- | --- |
| Contract | what must the bytes be? | `specs/` |
| Guide | how does the product work, and why? | `docs/content/`, listed in `nav.json` |
| README | how do I build, run and flash this, and what will bite me? | next to the code |
| Rule | what must we not break? | the guard that enforces it, with `GOVERNS` and `RULE`, or one sentence here |
| Requirement | what must it do for a rider? | the verification console |
| Record | what happened, measured how? | the pull request, and `CHANGELOG.md` generated from it |

There is no other home. A design study, an experiment log, a measurement, a version history or a
"what we tried" belongs in the pull request that did it. Do not create a notes, evidence, scratch
or plans file; GitHub issues are the plan and git history is the archive.

A date, an issue or pull-request number, or a measurement from a past build is the signature of a
record. It does not appear in a contract, a guide, a README, a guard or a comment. The changelog
is generated (`obc changelog --update`); never write it by hand.

A rule is a check or it is one sentence in this file. A number in a gate says where it came from:
measured on a named build, or chosen by the owner for a stated reason. `obc governs PATH` lists
every guard, suite, contract, coverage component and budget that reaches a file. `obc prose
--check` budgets guide pages, READMEs, guard docstrings and this file; an over-budget file must
not grow.

## Documentation

- Write documentation, issues and pull requests in ASD-STE100 Simplified Technical English: short
  sentences, present tense, one meaning per word.
- A guide page says why and links the spec with `[text](src:path)`; it never restates a spec
  table. A README is instructions and reference tables, not explanation.
- On a pull request, say whether public docs changed. If they did, the change is its own `docs:`
  commit. Pages under `docs/content/` carry `copy: ai | mixed | human`; do not rewrite human-owned
  prose, add a `copy-review` note beside it instead ([docs/README.md](docs/README.md)).
- After editing public docs, run `python3 docs/build_docs.py --check-links`.

## System requirements and release evidence

The [verification console](https://releases.openbikecomputer.com) holds the system requirements
(`SYS-nnn`) and, for each, a coverage plan: acceptance criteria, the tests that are evidence for
each criterion, and the gaps. The owner writes requirements and approves plans. Agents propose
plans and suggest requirements, with the agent credential only; never invent a requirement or
claim a test is linked.

`obc req SYS-003` prints a requirement, its coverage state and its criteria numbered 1..n; bare
`obc req` lists every other subcommand. Always `propose --check` before `propose`, and
`suggest --check` before `suggest`. The agent token is at
`~/.config/openbikecomputer/verification-agent.token`.

- **Every pull request** carries one line: `Requirements: SYS-012, SYS-030` or
  `Requirements: none`. List a requirement when the change alters behavior it describes or adds or
  renames a test its plan cites. That line is the whole per-PR duty.
- **When a listed requirement's plan is affected**, propose the update after the change lands:
  new criteria, evidence for new tests, a gap where a test is missing. Proposing never approves.
- **When a change needs a requirement that does not exist**, or a requirement no longer describes
  the product, `obc req suggest` it with a title, a statement and the reason. The owner writes the
  requirement by hand.
- **Alert the owner** when requested behavior contradicts a requirement, or when a test cited as
  evidence was deleted or hollowed out.

[apps/obc-verification/README.md](apps/obc-verification/README.md) has the API and the release flow.
