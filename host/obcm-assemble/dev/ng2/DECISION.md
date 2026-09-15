# Decision: retain the current navigation format

The bounded post-layout candidate fails NG1's producer-cost gate. Retain the
shipping format and spatial settle path. No production cutover is selected.
The direct lookup result is useful: it removes repeated reader work and meets
the dense-route speed target. It does not establish an acceptable complete
producer/reader design.

This decision applies to this candidate. An integrated producer with different
layout/remap passes is unmeasured. Browser conversion and physical device
latency are also unmeasured. No board resource claim is made.

## Inputs and method

The baseline is NG1 source `55be89b6`, measured on Linux x86-64 with the frozen
[route manifest](../../../obc-bench/dev/navigation/inputs.json). Candidate source
is that baseline plus [candidate.patch](candidate.patch). The patch uses an
isolated version 250 and changes ordinary expansion in the actual shared
planner. The measured maps come from the retained v16 producers and the
file-backed [converter](convert.rs). The layout and capacity limits are in
[README.md](README.md).

There are exactly three baseline/candidate pairs for each of six cases, in
that order, in one idle host window. Each process starts a fresh planner and
caller-owned cache. Inputs are loaded into host memory before route timing.
There was no parameter sweep or repeat after the result. The large native
conversion ran once, after those pairs, on the fully verified NG1 output.
Route-map conversion during preparation was concurrent with builds; its times
are not used in this decision.

[paired.json](paired.json) retains all samples, route output hashes, input
hashes, phase times, source calls and bytes, cache operations, settles, retries,
workspace and maximum step time. [provenance.json](provenance.json) records the
source and binary identities. The source commit in the raw paired report is
the content-identical cherry-pick `2a98ee2c`; the retained patch identifies all
experimental changes.

## Routing result

Times below are medians in milliseconds. Search is the planner's Search phase.

| Case | Total baseline | Total candidate | Search baseline | Search candidate |
| --- | ---: | ---: | ---: | ---: |
| Monaco dense | 3.1505 | 2.2505 | 2.5432 | 1.6596 |
| Grimsel road | 0.4593 | 0.3427 | 0.2518 | 0.1319 |
| Grimsel MTB | 0.3106 | 0.2445 | 0.1440 | 0.0725 |
| Interior edge | 0.4742 | 0.3483 | 0.2607 | 0.0914 |
| Meiringen cell crossing | 2.2090 | 1.4614 | 2.0587 | 1.3110 |
| Monaco detour | 5.0924 | 5.6002 | 4.5024 | 4.9850 |

The dense case improves total time by 28.6% and search time by 34.7%. Its
three-sample total ranges do not overlap: baseline 3.1431–3.1825 ms, candidate
1.7627–2.3856 ms. This passes the frozen 10% total and 20% search targets.

Two other observed guards fail. The detour median rises by 10.0%, above the
5% allowance. Its samples are noisy and overlap, so this does not establish a
stable detour slowdown. The interior case's maximum step rises from 0.0723 to
0.1954 ms, above the permitted 0.1 ms increase. These are recorded guard
failures, not results that were rerun until they passed.

All six cases have identical OBCR hashes between baseline and candidate,
including the detour and partial-edge projection. Settles and retries are
unchanged. Snap and emit source reads are unchanged in every case. Workspace
is 81,096 bytes in both measurement builds, including the same optional
counters. File size and navigation-section size change by zero bytes.

The following counters cover each complete plan. They are stable across the
three samples.

| Case | Decoded junctions, baseline → candidate | Quadtree visits, baseline → candidate | Source bytes, baseline → candidate |
| --- | ---: | ---: | ---: |
| Monaco dense | 23,728 → 3,951 | 26,319 → 307 | 833,536 → 822,784 |
| Grimsel road | 4,472 → 632 | 4,537 → 93 | 125,952 → 125,440 |
| Grimsel MTB | 2,907 → 454 | 2,780 → 93 | 90,112 → 89,088 |
| Interior edge | 4,471 → 631 | 4,536 → 92 | 125,952 → 125,440 |
| Meiringen cell crossing | 45,455 → 5,045 | 55,266 → 132 | 955,392 → 855,040 |
| Monaco detour | 23,204 → 3,879 | 25,451 → 307 | 854,528 → 846,848 |

The large CPU-work reduction gives only a 1.3% source-byte reduction on Monaco
dense. Cache hits and logical reads do not establish physical SD latency.
The raw report retains graph/index hits and fills separately.

## Producer result

The input is NG1's verified native output with SHA-256
`feb9775a380b5aea0736b01c14a7af7ea0d3cd025a9e8b74c0d629a164bf5e72`:
1,010,635 nodes, 2,633,471 directed neighbor entries, 116,320 node chunks,
98,674,256 navigation bytes and 247,837,696 total bytes. The original assembler
used the pinned 37 map cells, four terrain cells and 16 MiB sort budget with
full verification enabled. Its three-run median is 4,864.574 ms.

| Additional stage | Measured time |
| --- | ---: |
| Placement scan and file-backed reference table | 528.820 ms |
| Complete conversion, including placement, copy, rewrite and output sync | 1,460.126 ms |
| Full candidate target verification | 692.302 ms |
| Total additional work | 2,152.428 ms |

The permitted 5% increase is 243.229 ms. Conversion alone exceeds it by a
factor of six. Including target verification, the added work is 44.2% of
baseline total assembly time. The component sum is 7,017.002 ms; this is the
sum of measured stages, not a timing of one uninterrupted pipeline. No extra
base assembly was run to repeat this clear refusal.

[conversion.log](conversion.log) retains all logical I/O. The converter adds
426,505,313 source-read bytes, 55,856,892 reference-table-read bytes,
24,255,240 table-write bytes, 1,407,892,992 candidate-read bytes and
307,393,537 candidate-write bytes. Repeated 12-byte lookups and 512-byte target
reads are included. The final table is 12,127,620 bytes. The staged candidate
copy also needs 247,837,696 bytes while the source remains present. Original
assembly scratch does not need to remain live during conversion.

[conversion.time](conversion.time) records a maximum process RSS of 2,120 KiB.
This is process resident memory, not allocator peak, WASM capacity or browser
memory. The native converter has no map-sized resident array. The temporary
reference table is a host file. Device workspace does not grow.

Native rejection is sufficient to retain this candidate. No browser converter
was implemented or measured. Its missing result is not presented as a pass.
The 30-record prototype refusal and the 64 GiB node-run capacity limitation
also require work before any production adoption; matching the old edge limit
alone does not preserve the larger-map contract.

## Reproduce the retained experiment

Obtain the pinned inputs and native output through NG1's
[route instructions](../../../obc-bench/dev/navigation/README.md) and
[assembly instructions](../navigation/README.md). In a disposable checkout at
`55be89b6`, apply this directory's `candidate.patch`, then build the actual
candidate harness:

```sh
git apply /path/to/ng2/candidate.patch
cargo build --release -p obc-bench --example nav_cost --features nav-metrics
```

Build a baseline harness at `55be89b6` without the patch. Build the converter
as specified in README.md. Convert each route map into the same package/file
layout below `/tmp/ng2-maps`, using new output and table paths. Then run:

```sh
python3 host/obcm-assemble/dev/ng2/compare.py /path/to/baseline/nav_cost /path/to/candidate/nav_cost /tmp/ng2-maps /tmp/ng2-paired
/usr/bin/time -v -o /tmp/ng2-conversion.time /tmp/ng2-convert /tmp/ng-native/native-0.obcm /tmp/ng2-native.obcm /tmp/ng2-native.table > /tmp/ng2-conversion.log
```

The raw command paths are retained in the time report. The compiler identity,
source hashes and output hashes are in provenance.json. The converter source
was formatted after its measured binary was built; only formatting changed.
Its recorded binary hash is the binary used for the measurement.

## Verification and production scope

The candidate passed the complete reader `extremes` suite (15 checks) and
route `nav` suite (49 checks), using `test-producer.patch` for generated route
maps. The existing oracles cover route cost bounds against Dijkstra, profile
prohibitions, directional climb, terrain completeness, partial-edge geometry,
retries, exhaustion and step budgets. Added focused checks cover reserved and
missing references, malformed boundaries, identity and coordinate mismatch,
failed fills and map replacement. Exact command:

```sh
source "$HOME/.obc-geos-env.sh"
cargo test -p obc-reader --test extremes -p obc-route --test nav
```

The first combined suite build failed because GEOS was not in the shell
environment. It passed after loading the existing repository GEOS setup.
`cargo clippy -p obc-reader -p obc-route --lib -- -D warnings` passed during
candidate preparation. Workspace formatting and registry validation are
recorded in the PR.

The final change adds zero shipping production lines and removes zero.
Only the experiment, replay patches and evidence remain. No runtime format
switch, alternate reader or resident lookup table is merged. No cutover child
is needed for a retain decision. UI snapshots, board resources, wake profiles,
full CI mirroring and browser candidate measurements were deliberately omitted.
No public documentation changed because shipping behavior did not change.
