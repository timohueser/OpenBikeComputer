# Verification record

Source changes were checked with the affected package and contract suites. The local runs did not mirror full CI.

- `cargo test --locked -p obc-app --lib --tests`: whole App library and integration suites, including 918 library tests.
- `cargo clippy --locked -p obc-app --all-targets -- -D warnings`: passed; the later position-only correction passed the whole App tests.
- The Route library and integration suites ran with one new distance-expectation failure. After correcting that test expectation, `cargo test --locked -p obc-route --test visit` passed all 12 Visit contracts. The original whole-package shell flags are not preserved in its log; the final CI covers the package.
- `cargo test --locked -p obc-host-core --lib`: 66 tests passed.
- `cargo test --locked -p obc-web-demo`: 16 tests passed.
- `cargo test --locked -p obc-host-core --test board_detour`: 17 actual board Detour/Visit contracts passed.
- `cargo clippy -p obc-app -p obc-route -p obc-host-core --all-targets -- -D warnings`: passed before the final App-only delta.
- Whole conformance suite after the loading animation changed wake counts (27 tests).
- `tools/obc suites check`: 80 suites, 347 execution units.
- `cargo fmt --all`, plus `cargo fmt` in all three standalone roots.
- `python3 docs/build_docs.py --check-links`.
- One local resource bundle at ae000874 against the recorded baseline. No base rebuild. Later ARM allocations are read from CI reports; memory/stack limits remain unchanged.
- `SIM=<pinned final binary> bash firmware/ui-snapshots.sh <output>` on final production source `9f6c93e6d`: one local sweep, 263 screens. Only `find-place.png` changed. The orchestrator and independent reviewer inspected it and the manifest was updated. No second local sweep. CI must match that manifest.
- Real Swiss offline simulator: five production input paths, exact original route, normal GPS ticks, network denied, saved-route CRC and geometry checks, preview reuse hashes.
- Physical nRF54LM20A: fixed Meiringen location, original route object2/rev1, progress1664m. Times use device button and first results-frame timestamps. These are not simulator wall times.

Broad on-road navigation, physical GPS reception, and power measurements remain outside this desk-test acceptance.
