# Test system redesign — outcome

Epic [#1816](https://github.com/timohueser/OpenBikeComputer/issues/1816) landed in eleven pull
requests. One child, TS-H, is not done. The design documents, subsystem surveys and adversarial
reviews behind the epic are in the repository history; the epic issue is the authoritative scope.

| Child | PR | Added | Deleted | Moved |
| --- | ---: | ---: | ---: | ---: |
| TS-A — one integration target per package | [#1837](https://github.com/timohueser/OpenBikeComputer/pull/1837) | 260 | 123 | 25,678 |
| TS-G — one shared frame render for three hosts | [#1833](https://github.com/timohueser/OpenBikeComputer/pull/1833) | 145 | 148 | — |
| TS-E2 — assembled map through the card to a frame | [#1843](https://github.com/timohueser/OpenBikeComputer/pull/1843) | 144 | 31 | — |
| TS-D part 1 — real flat engine behind one device | [#1838](https://github.com/timohueser/OpenBikeComputer/pull/1838) | 1,463 | 212 | — |
| TS-D part 2 — every device suite on the real engine | [#1840](https://github.com/timohueser/OpenBikeComputer/pull/1840) | 2,014 | 1,263 | — |
| TS-E1 — interrupted recording through recovery | [#1845](https://github.com/timohueser/OpenBikeComputer/pull/1845) | 190 | 0 | — |
| TS-B — execution routes and one guards job | [#1841](https://github.com/timohueser/OpenBikeComputer/pull/1841) | 391 | 256 | — |
| TS-C1 — delete the registry's consumer-less parts | [#1847](https://github.com/timohueser/OpenBikeComputer/pull/1847) | 331 | 588 | — |
| TS-E3 — builder journey in a real browser | [#1848](https://github.com/timohueser/OpenBikeComputer/pull/1848) | 705 | 67 | — |
| TS-C2 — the registry becomes `tools/test_plan.py` | [#1852](https://github.com/timohueser/OpenBikeComputer/pull/1852) | 2,153 | 3,536 | — |
| TS-F — dead infrastructure, policy and docs | — | 186 | 2,789 | — |

## The settled harness decision

Share only the frame seam that is provably identical in every host. Leave everything host-specific
alone. The exact surface, and the explicit list of what must not be extracted, are in #1816.

## Measured

- The `test` job: 302 s before the epic; 153 s measured on TS-B's head when it shipped (#1841);
  177-194 s on `develop` push runs after every child landed, measured 2026-09-16. All three are
  measurements, not estimates. The saving is the snapshot sweep and the builder pytest
  leaving that job's serial path, not a narrowed package set.
- `obc-app` went from 17 ordinary integration executables to 1. Across the seven consolidated
  packages the count went from 73 to 8. Wall time per rebuild did not change; CPU halved, and
  nextest execution is about 1.2 s slower because each test process now loads a larger binary.
- The selector went from 1,668 lines of `suite_registry.py` plus 1,020 lines of its tests plus
  1,137 lines of `suites.toml` to 1,030 lines of `test_plan.py`, 567 lines of tests and 552
  lines of `suites.toml`.

## Deviations recorded on the epic

- **TS-B kept `--workspace` for the instrumented nextest steps.** The per-pull-request coverage
  ratchet walks every critical component's files and fails the job when one was not compiled, so
  a narrowed CI run fails every leaf-crate change. Moving that ratchet to `develop` pushes is the
  owner's call and has not been made.
- **D4 was wrong about the no-space answer.** A byte-exhausted card answers `noSpace` with the
  required byte count; `busy` is only the full-reservation-table answer. The TypeScript tests
  keep `no-space` against a real one-extent card.

## Still open

**TS-H ([#1829](https://github.com/timohueser/OpenBikeComputer/issues/1829)) is not done and is
owner-gated.** The captured Komoot and bikepacking.com waypoint imports through to device
presentation need the owner to capture a bikepacking.com route with waypoints and to settle its
redistribution terms before the work can start. No other epic child covers it.
