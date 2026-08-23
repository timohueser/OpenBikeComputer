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
| `ops/`, `hardware/`, `tools/` | Operations runbooks, hardware design, and repository tooling. |

The root Cargo workspace contains the shared `firmware/`, `host/`, and `apps/` crates. Keep
device-reachable dependencies in `firmware/`; keep host policy and native-heavy dependencies out
of it. The nRF54L board image, bootloader, and Tauri desktop app are standalone Cargo roots; build
and test them from their own directories using their READMEs.
Use the nearest README for surface-specific setup. `companion-ios/CLAUDE.md` is the iOS on-ramp.


## Build and verification

- Read [CONTRIBUTING.md](CONTRIBUTING.md) and [docs/testing.md](docs/testing.md) before choosing
  verification. Use focused checks for the changed package or surface:

  ```sh
  obc test -p <crate>
  cargo clippy -p <crate> --all-targets -- -D warnings
  ```

- Use `obc test fixtures -p <crate>` only for captured external data. For non-Rust work, run the
  affected surface's native focused command. Unit, component, and contract suites are the fast,
  hermetic tier; select whole suites, never individual test functions.
- Run `obc test full` or `obc check full` only for cross-cutting changes, such as workspace or
  feature-resolution changes, shared contracts, foundational crates, CI/tooling, or releases.
- Run `obc suites check` after changing test sources, validation commands, workflows, registries,
  or test policy.
- Format the workspace with `cargo fmt --all`; also run `cargo fmt` in each standalone Cargo root
  (`firmware/obc-fw-nrf54l`, `firmware/obc-boot`, and `apps/obc-desktop`).
- Report the exact checks run and deliberately omitted. Use `obc clean` for stale state; inspect
  its dry run before using `--apply`.

## Documentation

- Put conceptual architecture and behavior in `docs/content/`; put exact byte and wire contracts
  in `specs/`; keep build, run, and flash instructions in the relevant README.
- On a PR, check whether code changes make public docs stale. If they do, update the docs in a
  separate `docs:` commit; otherwise state that no public documentation changed.
- After editing public docs, run:

  ```sh
  python3 docs/build_docs.py --check-links
  ```
