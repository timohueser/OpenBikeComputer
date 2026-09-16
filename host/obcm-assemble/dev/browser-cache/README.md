# Browser input cache default

## Fixed comparison

Compare the old default (64 KiB input and verification blocks) with the new
default (4 KiB input, 64 KiB verification). Both caches retain sixteen shared
slots. No `readBlockBytes` override is sent in either group.

Before measurement, fix three pairs in this order: baseline/candidate,
candidate/baseline, baseline/candidate. Use the actual shipping worker, persistent
Chromium, OPFS profiles on `/home` (btrfs), and the pinned navigation inputs.
Input staging and independent output read-back stay outside engine timing.
The host filesystem is warm; logical I/O is not physical device traffic.

Acceptance requires at least 10% lower pinned median engine total with disjoint
sample ranges, identical fully verified output hashes, and no increased WASM
capacity. Verification and render-only median total must not regress by more
than the larger of 5% or 50 ms. Keep all samples and disclose host noise.

A second assembly uses only the same rendering and terrain objects. It marks the
network squares known-empty in a **synthetic derived selection**, not as a claim
about the published network data. This removes graph work to isolate bulk source
copying. The result must pass full verification and retain its exact digest
across cache policies. It does not represent sequential small-record reads.

The separate `sequential.patch` adds a diagnostic export around the unchanged
`BlockCache` and `JsReads` shim. It is absent from shipping code and WASM.
`sequential.mjs` scans one synthetic 32 MiB sync-OPFS file in 17-byte records,
512-byte records, and 256 KiB bulk reads. It checks an aggregate checksum over all bytes
and requires exactly one source-length of logical reads. Each width has the
same three paired cache sizes. This diagnostic exposes the sequential call
tradeoff; it does not predict a whole-map speedup.

## Reproduce

Use `../navigation/README.md` for dependencies and pinned input download. Build
baseline and candidate WASM separately and save their complete generated `pkg`
directories. Restore the appropriate `pkg` before each sample. Run:

```sh
OBC_BROWSER_PROFILE_ROOT=/path/on/regular/filesystem node host/obcm-assemble/dev/navigation/browser.mjs INPUT OUTPUT.json default pinned
OBC_BROWSER_PROFILE_ROOT=/path/on/regular/filesystem node host/obcm-assemble/dev/navigation/browser.mjs INPUT OUTPUT.json default sequential
```

For the diagnostic, apply `sequential.patch` to the candidate source, rebuild
WASM, then run:

```sh
node host/obcm-assemble/dev/browser-cache/sequential.mjs OUTPUT.json /path/on/regular/filesystem
```

Restore the shipping source and WASM after the diagnostic. Do not combine its
binary with the normal assembly comparison.

## Source identity and excluded runs

`provenance.json` maps each WASM hash to its source. The baseline WASM was built
at `55be89b653f2094cc67c192e3fd2fa31673a8cd8`; its assembly runtime is equivalent
to the parent of this change. Intervening changes in that dependency path are
comments and native tests. The candidate runtime is from `c986bde6`. Both builds
use the same release configuration and `wasm-opt -Oz`.

Raw `source_commit` identifies the checkout carrying the measurement harness.
It does not replace the separate source identity of a reused WASM binary.
Candidate test additions and later comment fixes do not change its WASM runtime.

An initial three-sample attempt overlapped an unrelated Cargo build. The whole
attempt is retained under `confounded/` and excluded. A later competing Clippy
build overlapped sample 3 after a clean first pair. Only that identified overlap
is retained under `confounded-later/` and excluded; clean samples 1–2 remain.
The campaign resumes its fixed order after a quiet interval. A process check at
each sample boundary prevents starting a sample during a known build. No
unrelated process is stopped. No sample is excluded because of its result.

Run `python3 host/obcm-assemble/dev/browser-cache/compare.py` to recompute assembly
medians and adoption guards from the twelve main raw files. The command never
loads either excluded directory.

## Results and decision

Adopt the 4 KiB input default with separate 64 KiB verification. All predeclared
assembly guards pass. Every pinned pair improved total time (33.6%, 42.9%,
38.8%); no comparison uses the excluded samples.

| Pinned engine phase | Baseline median | Candidate median | Change |
| --- | ---: | ---: | ---: |
| Total | 19.580 s | 11.450 s | -41.5% |
| Navigation | 8.385 s | 4.521 s | -46.1% |
| Write | 10.142 s | 5.287 s | -47.9% |
| Full verification | 1.676 s | 1.645 s | -1.8% |

Total ranges are 18.705–22.768 s and 11.183–15.123 s; they do not intersect.
Phase medians are calculated separately and need not add to the total median.
The first retained candidate has a 3.600 s verification outlier; it remains in
the statistics. The later candidate verification times are 1.645 and 1.625 s.
The completed clean first pair precedes a pause for unrelated builds; the other
pairs follow that pause. This is one host comparison with visible timing noise,
not a universal speedup percentage.

All six pinned outputs pass full verification and independent SHA-256 read-back:
`feb9775a380b5aea0736b01c14a7af7ea0d3cd025a9e8b74c0d629a164bf5e72`,
247,837,696 bytes. Input calls rise from 1,774,769 to 2,172,442 (+22.4%), but
logical input bytes fall from 116,438,699,584 to 9,066,983,061 (-92.2%). Median
input host-call time falls from 7.714 to 2.129 s. Verification cache traffic is
unchanged: 33,429 calls and 2,203,309,628 bytes. Scratch traffic and peak logical
scratch remain unchanged (152,769,123 bytes peak).

WASM capacity falls from 70,123,520 to 69,140,480 bytes. The default memory
estimate falls by the same 983,040 bytes, from 149,757,132.8 to 148,774,092.8.
These are linear-memory capacity and a model estimate, not browser process-memory
measurements. Input/verification slot counts, full validation, and failure policy
stay fixed.

### Sequential tradeoff

The synthetic render/terrain-only assembly has a median total of 1.295 → 1.276 s
(-1.5%); its ranges overlap. Verification is 318 → 321 ms (+3 ms), within the
50 ms allowance. All six 147,605,504-byte outputs have SHA-256
`9e8b350a342320c30adc5e8665460a3f65e64c09c5fd67519e8a9c4e0ca28469`.
WASM capacity falls from 23,330,816 to 22,151,168 bytes.

The separate 32 MiB sequential source diagnostic shows the cost that bulk copying
alone cannot expose:

| Read width | 64 KiB cache median | 4 KiB cache median | Absolute change | Relative change |
| --- | ---: | ---: | ---: | ---: |
| 17 bytes | 47.7 ms | 49.0 ms | +1.3 ms | +2.7% |
| 512 bytes | 12.9 ms | 19.3 ms | +6.4 ms | +49.6% |
| 256 KiB | 10.1 ms | 10.4 ms | +0.3 ms | +3.0% |

Small sequential reads need 8,192 host calls instead of 512 (16×), with the same
32 MiB of logical bytes. Bulk reads bypass either cache and need 128 calls.
The 512-byte sample ranges do not overlap: this diagnostic has a real relative
cost, although the absolute cost is small on this host. The other diagnostic
ranges overlap. No claim is made for slower devices or universal source access
patterns. The measured complete-map benefit and bounded memory justify the
default, with this tradeoff explicit.

## Verification

Passed on the shipping candidate:

```sh
./tools/obc test -p obc-web-assemble
cargo clippy -p obc-web-assemble --all-targets -- -D warnings
npm test --prefix builder/app -- src/lib/assemble/bridge.test.ts src/lib/assemble/assemble.worker.test.ts src/lib/cells/store.test.ts
./tools/obc suites check
cargo fmt --all --check
cargo fmt --manifest-path firmware/obc-fw-nrf54l/Cargo.toml --check
cargo fmt --manifest-path firmware/obc-boot/Cargo.toml --check
cargo fmt --manifest-path apps/obc-desktop/Cargo.toml --check
python3 docs/build_docs.py --check-links
python3 host/obcm-assemble/dev/browser-cache/compare.py
```

The Rust package passed 50 tests (the fixture regenerator remains ignored).
The three complete Vitest files passed 83 tests, including typed input/read
failures, cancellation, worker admission and storage ownership. These use actual
WASM for bridge checks and mocked seams for worker policy; the twelve assembly
runs use the actual shipping worker in Chromium with sync OPFS and full
verification. The sequential replay is a diagnostic binary, not shipping code.

The initial diagnostic build hit the host's `/tmp` quota. Its existing target
cache was copied to `/home`, where the build passed. No measured artifact was
produced by the failed build. Native checks also use an isolated target on
`/home`; the user's working-tree target was not used.

No full local CI sweep, fixture regeneration, UI snapshot sweep, board resource
build, country-scale run, physical-device benchmark, or browser process-memory
acceptance was run. The existing broader acceptance issues retain that scope.
