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
- None of this project is deployed to consumers yet, there is no need to keep backwards compatability, or write migration systems if we change any datastructure or file format. Breaking changes are fine at this stage of developement. "This will break the old format" is never an argument against making a change. The one exception is the settings blob in RRAM: released devices will receive firmware updates that change settings, so its versioned codec stays. It may be simplified, never deleted.

## Memory

This file and the `README.md` of each top-level directory are the memory of the agents and
people who work here. This file loads into every session, so each line costs every reader
attention. An area page is read when work starts in that area. A page holds what the code cannot
show: what the area is for, where to start, how to run and test it, what will bite you, and why
it is the way it is. A page states the present.

| Area | Page |
| --- | --- |
| The device crates, the board image, the bootloader | [firmware/README.md](firmware/README.md) |
| Host tools: `obc data`, the map bakers, test oracles | [host/README.md](host/README.md) |
| The simulators, the host core, replay, the render bench | [sim/README.md](sim/README.md) |
| The web app, its WebAssembly bridges, the USB transport | [builder/README.md](builder/README.md) |
| The planner services: router, search, tiles | [planner/README.md](planner/README.md) |
| The iPhone app | [companion-ios/README.md](companion-ios/README.md) |
| The registry of sources, regions and targets | [data/README.md](data/README.md) |
| The contracts: formats, protocols, test vectors | [specs/README.md](specs/README.md) |
| Large test data and scenarios | [fixtures/README.md](fixtures/README.md) |
| The site, the guide, the blog | [docs/README.md](docs/README.md) |
| The electronics and the parts list | [hardware/README.md](hardware/README.md) |
| The `obc` command, test selection, CI, the guards | [tools/README.md](tools/README.md) |

The READMEs below the top level have the build, flash and run instructions of their code.
[PRODUCT.md](PRODUCT.md) holds the principles that decide a design question. The root Cargo
workspace lists its crates in `Cargo.toml`; the board image, the bootloader and the sensor
simulator are standalone Cargo roots.

The pull request is the record: what happened, what was measured, what was tried. It may end
with a `Learned:` section: what the next agent should know. Git history and pull request bodies
are searched when needed. So:

- Do not create a notes, evidence, scratch or plans file. GitHub issues are the plan, and git
  history is the archive.
- A comment, a guard, a spec, a guide page and a README hold no date, no issue number and no
  measurement of a past build. A page may link the pull request that made a decision.
- A rule is a check, or it is one sentence in a page. A number in a gate says where it came
  from: measured on a named build, or chosen by the owner for a stated reason.
- `obc paths` holds the pages to the tree: the line caps, and each path, task and link that a
  page names.
- Before a large change, the plan is in its issue and a fresh reader reviews it there (the
  skill `plan-review`).

## Build and verification

- Run `obc` inside the task's checkout and check the printed root. `obc help TASK` describes a task.
- Verify a change with two commands:

  ```sh
  obc test -p <crate>   # the inner loop: one package's tests, then its doctests
  obc ready             # before a push: format, lint, docs and the affected suites
  ```

  `obc ready` prints each gate it runs or skips, with the reason. A change that reaches the whole
  graph is left to CI. [docs/testing.md](docs/testing.md) explains the plan.
- Run a command that no task covers with `obc exec`: a raw shell lacks the tools of `obc.local`,
  GEOS and the Cargo target of the checkout.
- For a non-Rust surface, run that surface's own command from its README. Select whole suites,
  never single tests.
- Prove claims with tests and measurements, and keep each run short. A run that takes minutes, in
  CI or local, committed or temporary, must earn its time: take the smallest sample that answers
  the question, and do not repeat it without a reason. Before a run over 5 minutes becomes routine
  or repeats, report to the owner what it costs and why it is needed.
- Report exact checks run and left out. `obc clean` removes old test scratch and old cargo
  artifacts in `$CARGO_TARGET_DIR`; read its dry run before `--apply`.
- Obtain explicit owner permission before offloading builds or working data to another machine to
  bypass local memory or disk limits.
- Never print `tools/obc.local` or the files in `~/.config/openbikecomputer/`: they hold operator
  settings and secrets. List the names with `grep -oE '^\s*(export\s+)?[A-Za-z0-9_]+=' FILE`.
- Production: publish only with a review ID that the owner approved
  (`obc data update prod --yes --expect ID`), and never run two publications or deployments at
  the same time. [data/README.md](data/README.md) has what follows a production update.

## How reviews work

A pull request gets one review round; a re-review covers the delta. Do not demonstrate a test
against deliberately broken code unless the reviewer asks for one specific case. Do not mirror
the CI suite locally before a push; run the affected suites, CI is the gate. `obc ci` prints a red
run's failures; never read a raw CI log. Resource figures come from one head build compared
against `resource_baseline.json`; never rebuild the base.

Use `obc pr list|view` for PR metadata and `body|comments|checks` when needed; for narrower GitHub reads, select `gh --json` fields or filter connector results before they enter context.

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

[tools/verification/README.md](tools/verification/README.md) has the API and the release flow.
