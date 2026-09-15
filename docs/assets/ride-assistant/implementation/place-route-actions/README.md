# Place route actions: validation record

This record covers [PR #1806](https://github.com/timohueser/OpenBikeComputer/pull/1806). The reviewed production source is `77fe2b79fe4036f1530190e065e071753ba58ac2`. Commit `bca6b4cab` updates the exact allocation record only. Hardware acceptance is pending.

The place preview switches between **Add detour** and **Route here**. Route here replaces the goal and has no original route continuation. Generated routes stay out of saved Routes while the stored objects needed for navigation and recovery remain available. Independent adversarial review and reviews of the fixes covered this source.

## Focused checks

These commands ran from the workspace root, with the shared native target at `/Users/timo/Documents/OSM-agents/ride-assistant-speed/target`:

```sh
cargo test --locked -p obc-app --lib --tests
cargo test --locked -p obc-host-core --lib
cargo test --locked -p obc-host-core --test board_detour
cargo clippy --locked -p obc-app -p obc-host-core --all-targets -- -D warnings
python3 tools/suite_registry.py check
python3 docs/build_docs.py --check-links
```

Results: App 922 library tests and all 16 integration suites passed; host 73 library tests passed; board executor contracts 17 passed. Scoped Clippy, suite registry and documentation links passed. The board contracts run the actual board executor on a host; they do not establish physical-device acceptance.

The root logs are `.artifacts/place-actions/app-final-tests.log` and `clippy-final.log`; owner logs are `/tmp/place-mode-host.log`, `/tmp/place-cancel-board.log` and `/tmp/place-mode-suites.log`. [CI run 35027121846](https://github.com/timohueser/OpenBikeComputer/actions/runs/35027121846) also recorded 3262 passing Rust tests on source `77fe2b79f`. Its old snapshot manifest and exact allocation record still needed the recorded updates below; that run is not an all-green gate.

## UI and real offline simulator

One local snapshot sweep rendered 263 frames on source `77fe2b79f`:

```sh
SIM="$PWD/target/release/obc-sim" bash firmware/ui-snapshots.sh .artifacts/place-actions/ui-sweep
python3 firmware/tools/ui_snapshot_manifest.py update firmware/ui-snapshots.sha256 .artifacts/place-actions/ui-sweep
python3 firmware/tools/ui_snapshot_manifest.py check firmware/ui-snapshots.sha256 .artifacts/place-actions/ui-sweep
```

All 263 frames passed the updated manifest check. Only `visit-preview.png` changed: its destination action label is **Route here**. The reviewed hash is `9a0a2f1b932185c9963e2fe8745e23c6b4544db679ae62dc744152d795ff4546`, recorded in [the manifest](../../../../../firmware/ui-snapshots.sha256). The [CI UI artifact](https://github.com/timohueser/OpenBikeComputer/actions/runs/35027121846/artifacts/10419659670) retains the source-77 frames. No second local sweep was run.

The separate [real Swiss simulator evidence](simulator/README.md) retains six production flows, exact commands, source identity, input hashes and route geometry checks. Those named captures use source `b9807e09c`; later source-77 changes handle pending ownership and errors. The evidence confirms direct destination geometry, reverse mode switching, acceptance with `original: None`, hidden generated routes, preservation of the original route, and complete cancellation cleanup. [Pixel measurements](simulator/arrow-pixels.json) confirm that both mode arrows are present in the direct and detour captures.

## Resources

The authoritative ARM report from [CI run 35027121846](https://github.com/timohueser/OpenBikeComputer/actions/runs/35027121846), on source `77fe2b79f`, measured:

| Measurement | Bytes |
| --- | ---: |
| App allocation | 52,720 |
| Linked resident RAM | 308,496 |
| `.uninit` | 132,096 |
| Flash | 1,712,324 |
| Residual main stack | 50,928 |

The resource check reported exact allocation drift from the earlier recorded App size. Commit `bca6b4cab` records the measured 52,720 bytes in [resource_baseline.json](../../../../../firmware/tools/resource_baseline.json). Resource and stack ceilings remain unchanged. Corrected-baseline CI is still the merge gate; the failed earlier exact-match check is not reported as a pass. No base image was rebuilt for comparison.

## Hardware acceptance

Physical replay is in progress on the real Swiss map. It found a board-specific stall while a RouteMode request waits for deferred work. The fix and its physical verification are pending. Simulator and host results do not close that finding. The final device source, flash identity, replay logs and acceptance result must be added here after the replay completes.

This documentation update did not run tests, builds, a resource measurement or another snapshot sweep. It records completed checks and leaves hardware acceptance open.
