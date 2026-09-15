# Place route actions: validation record

This record covers [PR #1806](https://github.com/timohueser/OpenBikeComputer/pull/1806). The final production source is `cc5bf0ebaf38ee9df07fe07982e4ac14168a4814`. The physical bench replay passed. Road riding and power-loss acceptance remain pending.

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

The final ARM report from [CI run 35030359323](https://github.com/timohueser/OpenBikeComputer/actions/runs/35030359323), on integration checkout `2385a32b72e83df9c15af1d5a4da6eca2e072e84` for PR source `cc5bf0eba`, passed the resource gates and measured:

| Measurement | Bytes |
| --- | ---: |
| App allocation | 52,720 |
| Linked resident RAM | 308,496 |
| `.uninit` | 132,096 |
| Flash | 1,712,836 |
| Residual main stack | 50,928 |

Commit `bca6b4cab` records the measured 52,720-byte App allocation in [resource_baseline.json](../../../../../firmware/tools/resource_baseline.json). The final report matches all 35 exact allocation entries. Resource and stack ceilings remain unchanged. One local head resource bundle was run; later shipping measurements came from CI. No base image was rebuilt for comparison.

## Final lifecycle and hardware checks

The final source fixes two board transitions found during replay. A pending preview-mode change advances after release even when no map redraw is due. A navigation stop that encounters a changed catalog revision before submission refreshes its scope and retains the latest selection intent. Actual write errors and uncertain outcomes retain their existing behavior.

The whole host suite reproduced the recorder-discard scope race, then passed with the fix. Final App 922 library tests, host 73 library tests, scoped Clippy and the suite registry passed. The source adds no persistent state beyond the allocation already recorded above. Independent adversarial review and reviews of the fixes reported no remaining findings.

The [final physical replay](hardware/final/README.md) verifies the exact flashed source, acceptance, concurrent recording discard, route-free search, preview cancellation and search re-entry. The [earlier physical replay](hardware/README.md) retains both route-mode directions and the saved Routes capture. The final board is left without a recording or active route, with the Swiss map and fixed Meiringen GPS feed available.

Remaining hardware acceptance belongs to [#1748](https://github.com/timohueser/OpenBikeComputer/issues/1748): real-motion arrival and rejoin with recording, card/power failure and recovery, and broader regional coverage. The separate recording-start warning remains in [#1810](https://github.com/timohueser/OpenBikeComputer/issues/1810). Bench and simulator evidence do not close these checks.

Deliberately omitted: a local full CI mirror, a base resource rebuild, another local UI sweep, mutation testing, and unrelated wake-profile isolation. Final source [CI run 35030359323](https://github.com/timohueser/OpenBikeComputer/actions/runs/35030359323) passed. PR #1806 merged as `36257ee4b78ef2a641769fec8ff39973d77bdb14`.
