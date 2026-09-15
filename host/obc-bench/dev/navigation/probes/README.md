# Navigation table probe experiment

## Change

The planner searched for a node ID, then searched the same collision chain again
when it inserted a new node. The candidate uses one bounded search to return the
existing entry or initialize the first free slot. It keeps slot order, heap order,
coordinates, route costs, retries and full-table salvage behavior. A full table
still requires a search for an existing ID before a new ID can be refused.

The map format and cache do not change. The replay patches add an optional
`nav-metrics` feature that counts table slot probes and absent IDs in a full table. These counts cover the lifetime of the
workspace, including search retries. The production change omits this diagnostic
feature and keeps only the simpler table operation.

## Other opportunities

The current implementation has not been shown to be optimal. The existing NG1
raw evidence identifies several smaller experiments before a new graph format:

1. Stop a settled-node record scan at the requested ID. The current reader decodes
   every node header in the selected chunk. Monaco has 23,728 decoded headers for
   2,599 settles. These counts include snap work and repeat work across retries;
   their ratio is not a search-only per-settle measurement. Retain byte-wise
   decoding, chunk length checks and point/leaf boundary behavior.
2. Reuse a known point-to-leaf mapping in the bounded route cache. Monaco visits
   26,319 quadtree nodes but fills only 20 index windows. This can remove CPU work;
   the visit count is not an SD command count. A cache entry must be invalidated
   with its map and preserve split-boundary and corrupt-input behavior.
3. Avoid repeated edge projections during snapping. Endpoints, long-edge anchors
   and overlapping lookup windows can name the same edge. Monaco snap accounts
   for 431 of 1,628 logical source reads (26.5%). Count repeated projections before
   adding a bounded edge-ID set; overflow must fall back to complete scanning and
   unique-candidate queries must retain their ambiguity rules.
4. Measure hash collision chains and full-table misses before changing the hash
   function or search policy. Monaco retries once and Meiringen twice. A stronger
   epsilon can reduce work while changing route quality and reach; this is a
   separate behavior decision, not an equivalent optimization.

The audit paths are `firmware/obc-route/src/nav.rs` (`entry`, `settle`,
`relax_virtual_edge`, `NAV_EPSILON_LADDER`),
`firmware/obc-reader/src/reader/nav.rs` (`decode_nav_chunk`,
`for_each_nav_node_cached`, `nav_edge_candidate_cached`), and the route-private
cache in `firmware/obc-reader/src/reader/nav/cache.rs`.

The prior NG2 direct-reference prototype reduced Monaco quadtree visits from
26,319 to 307 and decoded headers from 23,728 to 3,951, but search reads fell only
from 1,062 to 1,041 (2.0%) and search steps from 90 to 88. For Meiringen, search
reads fell from 1,497 to 1,301 (13.1%) and search steps from 131 to 116 (11.5%).
The dense host speedup mostly removed cached traversal work; it does not imply
an equal reduction in card commands or device wall time. These values are from
`host/obcm-assemble/dev/ng2/paired.json`, not a new run.

## Physical-device evidence

`firmware/obc-fw-nrf54l/src/ride.rs::nav_finish` already reports route wall time,
phase time, logical cache fills, settles, epsilon and stack high-water. A build
with `sd-bench` also reports actual card command counts and card time by phase.
A matched device experiment should retain map hashes, profile, endpoints, firmware
ELF/hash, cache reset policy and SD card identity, and compare complete route time
as well as the slowest planner step. Host input is in RAM and cannot establish
SD or device latency.

The read-only `./tools/obc board doctor` command found no serial candidates or
native device in this session, and `probe-rs` was not installed. No image was
flashed, no board was reset and no device measurement was made.

## Initial result

The candidate removes repeated work, but this sample does not establish a speedup.
All six baseline/candidate total-time ranges overlap. There are no outcome or OBCR
hash differences. Settles, retries, logical reads and the normal 81,080-byte host
workspace remain equal. Metrics increase that host workspace to 81,112 bytes in
both diagnostic builds; they are disabled for timing and normal firmware.

| Case | Table probes, baseline → candidate | Change | Uninstrumented median total, ms |
| --- | --- | --- | --- |
| Monaco dense | 772,756 → 601,842 | -22.1% | 2.164 → 2.364 |
| Grimsel Road | 3,914 → 2,589 | -33.9% | 0.491 → 0.459 |
| Grimsel MTB | 1,001 → 681 | -32.0% | 0.355 → 0.314 |
| Interior edge | 3,917 → 2,591 | -33.9% | 0.479 → 0.271 |
| Meiringen cell crossing | 526,113 → 431,191 | -18.0% | 2.161 → 2.171 |
| Monaco detour | 734,044 → 575,506 | -21.6% | 5.799 → 6.122 |

Monaco dense baseline total ranges from 1.614 to 3.164 ms; candidate from 2.090 to
2.920 ms. The higher candidate median is not proof of a regression, but it does
not exclude one. No timing sweep or repeated attempt to reach a target was made.
This unresolved noise led to the fixed cold-batch follow-up below. The initial
results remain in `paired.json`; they are not silently replaced.

The full-table behavior remains a separate cost: Monaco has 136 absent-ID probes
in a full table, Meiringen 150, and detour 155. Each scans all 1,536 slots in both
variants. These account for 208,896, 230,400 and 238,080 slot checks respectively.
A compact negative-membership filter could avoid some absent-ID scans, but its
false-positive rate, memory and CPU cost must be measured; it is not implemented.

## Fixed cold-batch follow-up

The follow-up was declared before it ran: 10 unreported warmups and 250 measured
plans per process; three pairs in baseline/candidate, candidate/baseline,
baseline/candidate order. Map bytes are loaded once. Every iteration constructs a
fresh planner, scratch table, cache and sink. Warmups warm host execution, not the
next plan's graph cache. All table and reader metrics are disabled in both builds.

The harness compares every iteration's complete OBCR bytes, outcome and workspace
with its first result, outside timing. The driver then checks the output hash
against NG1 and checks every plan's remaining invariant fields. All 9,000 measured
plans pass. `batch.json` records individual times, per-batch means, medians and
95th percentiles; the table uses the median of the three batch means. Invariant
fields are stored once per case. Numeric arrays in the retained JSON use compact
formatting; `provenance.json` retains the original raw artifact digests. No values
or individual times were discarded from these timing artifacts.

| Case | Baseline, ms | Candidate, ms | Change |
| --- | --- | --- | --- |
| Monaco dense | 1.7042 | 1.5878 | -6.83% |
| Grimsel Road | 0.2220 | 0.2159 | -2.76% |
| Grimsel MTB | 0.1334 | 0.1299 | -2.61% |
| Interior edge | 0.2172 | 0.2192 | +0.93% |
| Meiringen cell crossing | 2.5017 | 2.2847 | -8.67% |
| Monaco detour | 3.9150 | 4.0294 | +2.92% |

Dense candidate batch means range from 1.5776 to 1.6297 ms, below the baseline
range of 1.6780 to 2.1433 ms. Dense search mean improves by 7.81%. This supports a
modest dense host improvement. Every other case's batch-mean ranges overlap,
including Meiringen by about 0.002 ms, so their aggregate changes remain less clear.

Individual-step outliers remain: Meiringen maximum is 0.187 → 0.395 ms and detour
0.758 → 2.962 ms. The experiment does not establish an improvement or absence of
regression in worst-step latency. Source-level settle and probe bounds are equal;
this is not a physical-device timing measurement. Do not equate the 22.1% fewer
table probes with a 22.1% route-time reduction.

## Replay

Use two isolated checkouts of the base commit in `provenance.json`. Apply
`baseline-instrumentation.patch` in the baseline and `candidate.patch` in the
candidate. The patches are alternatives and both include the optional counter
feature and harness output. The measured local source commit in the raw reports
was not published; `provenance.json` gives replay patch and exact source hashes.
The production commit removes the diagnostic counters from the otherwise equal
entry logic. Those fields and increments were already disabled in the timed
binaries and the whole-package test run. No runtime logic or memory layout
changed during their removal. Build each with:

```sh
cargo build --release -p obc-bench --example nav_cost
```

Copy the executables to stable paths. Run the existing paired harness from the
candidate checkout, with the same fixture directory for both variants:

```sh
python3 host/obcm-assemble/dev/ng2/compare.py BASELINE_BINARY CANDIDATE_BINARY \
  "$HOME/.cache/openbikecomputer/fixtures/by-id" /tmp/nav-probe-pairs
```

For the batch follow-up, also apply `batch-harness.patch` after either variant
patch and rebuild without metrics. Then run:

```sh
python3 host/obc-bench/dev/navigation/probes/batch.py \
  BASELINE_BINARY CANDIDATE_BINARY /tmp/nav-probe-batches
```

The earlier single-plan harness verifies input hashes, outcomes and exact route
bytes across three pairs per case. The checked-in `paired.json` is the result of this command. Build separately
with `--features nav-metrics` for operation counts; `counts.json` records one
observation per variant/case and excludes elapsed times. Do not use counter-build
timing as a speed claim: each counted table probe adds an increment in a hot loop.

## Verification

The candidate passed these focused checks, with the existing GEOS environment
loaded for the host packer test dependency:

```sh
./tools/obc test -p obc-route
cargo clippy -p obc-route -p obc-bench --all-targets --features obc-bench/nav-metrics -- -D warnings
./tools/obc suites check
cargo fmt --all
cargo fmt --manifest-path firmware/obc-fw-nrf54l/Cargo.toml
cargo fmt --manifest-path firmware/obc-boot/Cargo.toml
cargo fmt --manifest-path apps/obc-desktop/Cargo.toml
```

The route package passed 238 tests. Existing complete suites cover table
exhaustion, salvage of a tracked goal, all epsilon rungs, virtual edge endpoints,
profiles and route emission. No test sources were changed. Tests ran before the
counter-source removal, with those optional counters disabled. The final removal
was checked with `cargo check -p obc-route --lib`. The batch harness also passed:

```sh
cargo clippy -p obc-bench --all-targets --features nav-metrics -- -D warnings
```

Both replay variants plus the batch patch were applied to isolated base files and
matched the recorded source and harness hashes. The fixed paired measurements
also checked the six pinned real-map route hashes.

No full workspace suite, UI snapshot sweep, physical-board build or run, resource
baseline rebuild, or Miri campaign was run for this bounded host investigation.
No decoder, unsafe access, record layout, wake policy or scheduling policy changed.
