# Replay the isolated candidate

This is an experiment, not a shipping format patch. Use clean worktrees at the
recorded base. Do not apply these patches to current production code. Version
250 and legacy v16 input acceptance exist only to compare identical pinned
inputs. The production format and browser defaults are unchanged by this evidence.

`candidate.patch` contains the actual producer, reader, planner, full validator,
focused validation tests, route batch harness, and the measured browser input
cache dependency. It applies to commit
`773758a82af59c728ff51b4b699b5a07d1e47609`.
`verify4.patch` applies after it and contains only the bounded verification-cache
change and the post-timing normalized readback harness.

The patches omit this evidence directory. Copy it into each replay worktree at
`host/obcm-assemble/dev/aligned/` to run its drivers. All source and binary hashes
are in `provenance.json`. Different build paths or toolchains can produce a
different executable hash; the recorded hashes identify the binaries actually
measured, not a claim of reproducible compiler output.

## Preparation

1. Follow root `CONTRIBUTING.md` for Rust and GEOS. Follow the builder README for
   Node, WebAssembly, and browser dependencies.
2. Create separate baseline and candidate worktrees at the exact base from
   `provenance.json`.
3. In the baseline, apply only the benchmark example from `candidate.patch`:
   `git apply --include=host/obc-bench/examples/nav_cost.rs PATH/candidate.patch`.
   This adds the same 10-warmup/250-plan batch wrapper without changing routing.
4. In the candidate, apply all of `candidate.patch` with `git apply`.
5. Obtain the pinned NG1 inputs and fixed route fixture packages using the
   manifests and instructions in `../navigation/README.md` and
   `../../../obc-bench/dev/navigation/README.md`. The drivers verify all input
   hashes before measurement.

In each worktree, build native producer and route executables:

```sh
cargo build --release -p obcm-assemble --features mem-profile
cargo build --release -p obc-bench --example nav_cost
```

The recorded native baseline binary was built from NG1 commit
`55be89b653f2094cc67c192e3fd2fa31673a8cd8`; the relevant assembler runtime did
not change before the experiment base. The recorded route baseline is the NG4
fused-entry candidate, which is production-equivalent to the experiment base
with optional metrics disabled. Its original provenance is in
`../../../obc-bench/dev/navigation/probes/provenance.json`.

Prepare the three candidate route maps with the actual packer adapter:

```sh
cargo run --release -p obc-pack --example aligned_fixture -- OLD_MAP NEW_MAP
```

Repeat for the Monaco, Grimsel, and Meiringen map paths listed in the route
manifest, preserving their directory names under the candidate map root. The
adapter resolves every produced neighbor reference and coordinate. It preserves
edge/profile/snap bytes. Its appended old section is intentionally excluded from
production size measurement.

## Native and route measurements

Run from the candidate root, with absolute paths substituted:

```sh
python3 host/obcm-assemble/dev/aligned/native.py BASELINE_BINARY CANDIDATE_BINARY INPUT_DIRECTORY NEW_OUTPUT_DIRECTORY
python3 host/obcm-assemble/dev/aligned/batch.py BASELINE_NAV_BINARY CANDIDATE_NAV_BINARY NEW_OUTPUT_DIRECTORY --candidate-maps CANDIDATE_MAP_DIRECTORY
```

The fixed route driver runs 9,000 measured fresh plans. It verifies each pinned
OBCR hash and cross-variant workspace/outcome/output-size/snap-type invariants.
It retains deterministic counters separately for each variant, because direct
references intentionally change index work and read counts.

## Browser measurements

The actual browser baseline is the shipping worker with 4 KiB input and 64 KiB
verification, WASM digest `1de090a375e8dac1fe142932f0ef74a15bbd9650bc3b7e36c168ea3a845fcc69`.
It was built from browser-cache commit `c986bde6`; the later merged changes were
documentation and comment changes. Use that baseline for both browser campaigns.
Build candidate WASM with `npm run build:wasm:assemble` from `builder/app`.

For the first campaign, invoke `../navigation/browser.mjs` in each variant with
`INPUT_DIRECTORY OUTPUT.json default pinned`, alternating baseline/candidate,
candidate/baseline, baseline/candidate. Set `OBC_BROWSER_PROFILE_ROOT` to an
existing directory on the regular filesystem. Each run uses a fresh persistent
Chromium profile, the shipping worker and OPFS, full validation, and independent
output SHA-256 readback. Input staging and readback are outside assembly timing.

Preserve that WASM package, then apply `verify4.patch` to the candidate and rebuild
its WASM. Run the retained two-workload follow-up driver:

```sh
python3 host/obcm-assemble/dev/aligned/browser.py BASELINE_ROOT CANDIDATE_ROOT INPUT_DIRECTORY NEW_OUTPUT_DIRECTORY
```

This runs three alternating pairs per workload. Render-only inputs omit the
network cells and mark them known-empty. The post-timing normalized digest changes
only header version byte 4, proving the files otherwise match across formats.
No native or route rerun is needed for this browser-only constant change.

## Validation already run

Whole suites, with logs retained in `validation/`:

```sh
cargo test -p obcm-assemble -p obc-reader -p obc-route -p obc-formats
cargo test -p obcm-assemble -p obc-reader -p obc-route -p obc-pack --test nav_round_trip
cargo test -p obcm-assemble -p obc-reader
cargo test -p obc-web-assemble --lib
obc suites check
```

The third command checks the final malformed-empty-index refusal. The last
package command checks the one-constant browser follow-up. Root formatting ran
on the prototype. Physical-board resource builds, UI snapshots, external fixture
rebakes, public catalog publication, and a production-format full CI run were
not performed because the candidate was not adopted. Prototype clippy was not run. No public documentation
behavior changed in this evidence-only result. The artifacts-only branch runs
Python syntax checks and the current suite-registry check. Replay validation
applies both patches to a temporary Git index at the recorded base and compares
every changed file blob with the measured source commit; all 19 blobs match
after each patch. Raw patches and test logs preserve their original whitespace.
