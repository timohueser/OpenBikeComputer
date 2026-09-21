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
- **Keep code and comments concise, current, and focused on the present behavior.**. Keep revision history and references to PRs, issues etc. out of the comments. Do not annotate every line or every behaviour.
- None of this project is deployed to consumers yet, there is no need to keep backwards compatability, or write migration systems if we change any datastructure or file format. Breaking changes are fine at this stage of developement. "This will break the old format" is never an argument against making a change.

## Repository layout

| Path | Purpose |
| --- | --- |
| `firmware/` | Device-reachable `no_std` application, rendering, protocols, storage, board image, and bootloader. |
| `host/` | Host-only tools, bakers, fixtures, test oracles, and shared host support. |
| `apps/` | Simulator, desktop shell, and web/WebAssembly hosts. |
| `builder/` | Svelte map-builder UI, presets, and maintainer server. |
| `companion-ios/` | SwiftUI companion and shared iOS package. |
| `specs/` | Normative binary, wire, and vector contracts. |
| `fixtures/` | Scenario registry, input provenance, and fixture builders. |
| `docs/` | Public conceptual documentation and blog source. |
| `hardware/`, `tools/` | Hardware design and repository tooling. |

The root Cargo workspace contains the shared `firmware/`, `host/`, and `apps/` crates. Keep
device-reachable dependencies in `firmware/`; keep host policy and native-heavy dependencies out
of it. The nRF54L board image, bootloader, and Tauri desktop app are standalone Cargo roots; build
and test them from their own directories using their READMEs.
Use the nearest README for surface-specific setup. `companion-ios/CLAUDE.md` is the iOS on-ramp.


## Build and verification

- Run `obc` inside the task's checkout; check the printed root. Outside OBC it uses the installed
  checkout. Read [active plans](docs/plans.md), use `obc docs review` for related prose, and see
  [simulator diagnostics](apps/obc-sim/README.md#journey-diagnostics) to inspect a scripted journey.

- Read [CONTRIBUTING.md](CONTRIBUTING.md) and [docs/testing.md](docs/testing.md) before choosing
  verification. Use focused checks for the changed package or surface:

  ```sh
  obc test -p <crate>
  cargo clippy -p <crate> --all-targets -- -D warnings
  ```

- Use `obc test fixtures -p <crate>` only for captured external data. For non-Rust work, run the
  affected surface's native focused command. Select whole suites, never individual test
  functions.
- For a change spanning packages, let the plan choose: `obc test affected --base origin/develop`
  (add `--dry-run` to see the plan first). It is the same selection CI runs, and it prints one
  reason per suite.
- Run `obc test full` or `obc check full` only for cross-cutting changes, such as workspace or
  feature-resolution changes, shared contracts, foundational crates, CI/tooling, or releases.
- Run `obc suites check` after changing test sources, validation commands, workflows, the plan
  documents, or test policy.
- Format the workspace with `cargo fmt --all`; also run `cargo fmt` in each standalone Cargo root
  (`firmware/obc-fw-nrf54l`, `firmware/obc-boot`, and `apps/obc-desktop`).
- Report the exact checks run and deliberately omitted. Use `obc clean` for stale state; inspect
  its dry run before using `--apply`.

## Verification budget

Verification must cost less than implementation. These caps bind every agent — Claude,
Codex, or other — and only the owner can raise them:

- Run the UI snapshot sweep at most once per pull request, on the final head, and only when
  the change touches rendering, screens, or i18n. Reviewers spot-check named frames; they do
  not re-run the sweep.
- Measure resources with one head build compared against `resource_baseline.json`. Do not
  rebuild the base; the baseline file is the recorded base. The exact-match gates stay.
- Do not demonstrate tests against deliberately broken code ("mutants") as a routine step.
  A reviewer may request one targeted demonstration when they doubt a specific test; it is
  never repeated in later rounds.
- Run wake-profile isolations only when the change touches wake or scheduling behavior.
- Do not mirror the full CI suite locally before a push. Run the affected suites; CI is the gate.
- Reviews get one round by default. A re-review covers only the delta. Small pre-approved
  errands land on green CI without a further round.

## Documentation

- Use ASD-STE100 Simplified Technical English for documentation, issues, and pull requests.
- Put conceptual architecture and behavior in `docs/content/`; put exact byte and wire contracts
  in `specs/`; keep build, run, and flash instructions in the relevant README.
- On a PR, check whether code changes make public docs stale. If they do, update the docs in a
  separate `docs:` commit; otherwise state that no public documentation changed.
- Respect the `copy` ownership in each `docs/content/` page. Agents can rewrite `copy: ai` prose.
  They must not change prose in a `copy: human` page or inside a `human-copy` block on a
  `copy: mixed` page. If protected prose is stale, add a non-rendered `copy-review` note with the
  current facts and source, and report it in the pull request. See `docs/README.md` and run
  `obc docs` for the current queue.
- After editing public docs, run:

  ```sh
  python3 docs/build_docs.py --check-links
  ```

## System requirements and release evidence

The [verification console](https://releases.openbikecomputer.com) holds the system requirements
(`SYS-nnn`), and for each one a coverage plan: acceptance criteria, the tests that are evidence
for each criterion, and the gaps that remain. The owner writes requirement prose and approves
plans. Agents propose plans. Read the console with the agent credential; never write with an
owner session, and never invent a requirement or claim a test is linked.

To look up a requirement, run `obc req SYS-003` (`--json` for the raw record). It prints the
statement, the coverage state, and the criteria numbered 1..n, so "criterion 2 of SYS-003" is the
second one in that listing. `obc req` with no arguments lists the rest: `list` with filters,
`proposal SYS-003`, `tests <query>` to search the CI catalogue, `changed --since rN` for the
requirements a revision added or reworded, and `propose plan.json` to validate a coverage plan and
submit it. Always `propose --check` first; it catches what a reviewer would send back. The agent
token is stored at `~/.config/openbikecomputer/verification-agent.token`.

- **When you implement or test behavior that a requirement describes**, say so in the pull
  request in one line: `Requirements: SYS-012, SYS-030` or `Requirements: none`. List a
  requirement when the change adds, removes, or alters behavior it describes, or adds or renames a
  test its plan cites. This line is the whole per-PR duty; do not review requirements on every PR.
- **When a listed requirement's plan is affected**, propose the update with
  `POST /api/coverage-proposals` (see the README) after the change lands: new criteria for new
  behavior, evidence for tests you added, a gap where a test is still missing. Proposing never
  approves.
- **Alert the owner** when requested behavior contradicts a requirement, when a change needs a
  requirement that does not exist, or when a test cited as evidence was deleted or hollowed out.
  The console does not block development; the alert is the duty.

### Occasional requirements check

Run this when asked, or when a milestone lands, not per pull request. It takes one session.

1. Read the current revision and every plan from the console. Read `git log --since` the last
   check (the previous check's report names its end commit).
2. For each requirement touched by those commits, answer three questions: does the prose still
   describe the product; does the plan still name the right tests, with no cited test removed or
   emptied; is there new user-visible behavior that no requirement covers.
3. Report the findings as a short list grouped by those three questions, name the end commit,
   and propose plan updates for the second group. Requirement prose changes are for the owner.

See [the application README](apps/obc-verification/README.md) for the API and release flow.
