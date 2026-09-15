# Integrated aligned node-reference candidate

## Decision: retain the production format

The real changed-format producer, reader, planner, and full validator work.
Direct references remove 98.8% of dense-route index-cache accesses, and the
fixed host route batches are about 19–37% faster. Navigation bytes grow 1.63%
and planner workspace stays unchanged. These are useful results, but the
candidate also increases deterministic producer I/O. Complete browser assembly
is approximately 19% slower in the first campaign. One bounded verification
cache change does not remove that cost and regresses render-only assembly.
Physical-device timing and energy benefit remain unmeasured.

We retain the production format under the requested condition that a speedup
must not have a significant drawback. This is a conservative decision with
active-host timing limits. It does not prove that direct references are
unsuitable or that the current graph format is optimal. The complete candidate
and both fixed campaigns are retained below. No new follow-up issue or format
compatibility path is introduced. The separate 4 KiB browser input-cache default
is already implemented independently.


This experiment writes actual changed-format maps. It uses version 250. The
experimental reader also accepts version 16 so that the same pinned source
cells can be used. This input adapter is not a production compatibility policy.

See [replay instructions](REPLAY.md), [source provenance](provenance.json),
[validation logs](validation/), and the [unexecuted cutover inventory](CUTOVER.md).

## Layout

The reference unit is `max(16, 2^header_offset_scale)` bytes. Every node record
starts at a multiple of that unit relative to the node chunk run. The record
keeps its dense producer ID. Each neighbor stores a 32-bit node reference:
`node_run_relative_byte_offset / unit`. The reader uses the reference as the
planner key and exposes dense identity separately to host validation.

Each record and its alignment padding fit in one 512-byte chunk. All alignment
and tail padding bytes are `0xFF`. The tree builder uses the padded size before
it splits leaves and packs chunks. Producers refuse missing placements. Direct
lookup walks preceding record lengths within one chunk to prove the requested
aligned address is a record boundary. It rejects padding, interior payload,
invalid degree, truncated records and source read failures.

The last two u32 values are reserved for the planner's virtual nodes. Parsing
refuses a node chunk run longer than `(u32::MAX - 1) * unit`; complete chunks
make any final partial chunk unavailable. The two reserved units plus chunk
rounding lose 512 bytes at units 16 through 256, and 1024 bytes at unit 512,
relative to the nominal address range. Default scale 4 has a node-run reach
just below 64 GiB. Scale 9 raises the node-run reach with the whole-map header
to just below 2 TiB. This is distinct from the unchanged edge-pool limit. The
current assembler's separate 4 GiB variable-record scratch-offset ceiling also
remains; this experiment does not establish country-scale producer acceptance.

## Producer and validator

The packer assigns references after actual spatial packing. Its existing
resident graph permits a dense placement vector. The assembler uses an
external sort to write a four-byte placement table per dense node. Its existing
output walk resolves neighbor references through a fixed 64 KiB scratch cache.
No completed map is copied or rewritten by the assembler.

The validator scans all physical node records, including alignment padding,
then uses the existing dense coordinate bands, edge claims and component
union-find. Each neighbor reference is resolved through the real reader and a
fixed tile cache before the dense target and coordinates enter those checks.
Each physical dense identity must be unique. The old duplicate-record digest
array and hashing pass are removed. The spatial walk must reach every stored node. Both target reads and the
physical record scan are included in full assembly cost.

`aligned_fixture` is a separate fixture preparation adapter. It rebuilds the
node tree with the actual packer, retains exact edge/profile/snap bytes, and
appends a new nav section to the pinned route map. Its whole-map copy is not
part of the real producer measurement. The old unreachable section retained
in these route fixtures must not be used for output-size comparisons.

## Decision criteria

Correct routes, full validation, unchanged planner workspace and step limits
are required. Navigation-section growth must stay within 15%. Dense host route
batches must improve by at least 10%, with disjoint batch-mean ranges. Other
route means may regress by at most the larger of 5% and 0.1 ms. Complete native
and browser assembly medians may regress by at most 15%. This explicitly
revises the earlier 5% assembly policy: map production happens once, while
routing happens repeatedly. A result outside these limits needs a separate
tradeoff decision. Host results do not establish physical-device benefit.

The validator also retains its existing `total_nodes < 2^31` union-find limit.
The reference address range is not a promise that every graph up to that range
can be produced or validated by the current host implementation.

Padding overhead above the default unit is unmeasured. At unit 512, each node
uses one whole chunk. Equal address reach does not imply equal packing efficiency.

## Fixed acceptance result

The integrated candidate is commit `d37842992cc6bf72ad523d1bf8d5910f59702285`.
All six route cases keep the pinned OBCR bytes and outcomes. Three alternating
pairs use 250 fresh plans per process after 10 warmups: 9,000 measured plans in
total. All optional navigation metrics are disabled in both binaries. The table
uses the median of three batch means, in milliseconds.

| Route | Baseline | Candidate | Reduction |
| --- | ---: | ---: | ---: |
| Monaco dense | 1.632 | 1.231 | 24.58% |
| Grimsel road | 0.225 | 0.169 | 25.13% |
| Grimsel MTB | 0.133 | 0.090 | 32.12% |
| Interior edge | 0.235 | 0.157 | 33.16% |
| Meiringen cell crossing | 2.500 | 1.581 | 36.77% |
| Monaco detour | 3.565 | 2.904 | 18.56% |

Every candidate batch-mean range is below its baseline range. Planner workspace
stays at 81,080 bytes. Dense-route index-cache accesses fall from 26,319
(26,299 hits and 20 fills) to 311 (298 hits and 13 fills), a 98.8% reduction.
Source reads remain similar: 1,628 versus 1,631. Direct references remove the
repeated index traversal; this does not imply fewer physical storage reads.
The search keeps its 12 cache-miss step budget and 64-settle
cap. Timing outliers do not establish better worst-step latency: for example,
the largest MTB step rises from 0.070 to 0.218 ms, and the largest detour step
rises from 0.422 to 0.506 ms. These are host results. No physical device timing,
energy, or storage-latency measurement was made.

Complete assembly uses the same pinned 37 map cells and four terrain cells.
Both variants run full validation and independent SHA-256 output readback.
The default browser policy is 4 KiB input reads and 64 KiB verification reads
for both variants. Medians use three alternating complete assemblies.

| Complete producer | Baseline | Candidate | Change |
| --- | ---: | ---: | ---: |
| Native assembly | 4.917 s | 5.492 s | +11.69% |
| Actual browser assembly | 11.033 s | 13.102 s | +18.75% |
| Browser request | 11.298 s | 13.307 s | +17.79% |
| Navigation section | 98,674,256 B | 100,278,864 B | +1.63% |
| Whole map | 247,837,696 B | 249,442,304 B | +0.65% |
| Browser WASM capacity | 69,140,480 B | 69,140,480 B | 0% |
| Logical scratch peak | 152,769,123 B | 159,043,521 B | +4.11% |

**The browser producer fails the fixed 15% regression screen.** Native production,
route behavior, host route speed, workspace, and size pass their screens. This
failed screen contributes to the retain decision. No samples were removed from this campaign.
Process checks detected no concurrent compiler; six browser sample-boundary process
inventories are retained outside the repository with the local run artifacts.
Other desktop work was possible. These checks do not prove an idle host; timing
percentages are approximate observations. Exact bytes, calls, hashes, and
workspace measurements provide stronger evidence than a near-threshold timing.

The new target lookups add real I/O. Native verification reads increase from
1,103,026,810 to 1,352,343,676 bytes. Browser verification host-read bytes increase
from 2,203,309,628 to 5,752,608,316 bytes because the shared 64 KiB verification
cache now also serves random node lookups. Scratch reads increase from
689,279,449 to 1,763,827,745 bytes. Browser verification median time increases
from 1.542 to 2.627 seconds. The removed physical-record digest array reduces the
validator peak, but the collection phase still determines overall producer
memory. That validator change is part of the measured candidate; all measured
cost differences must not be attributed only to direct references.

Raw timings, per-variant I/O counters, hashes, source identifiers, and invariant
results are in `results/`. The first full correctness assembly overlapped builds
and is excluded from acceptance. The separate six-case correctness pass is also
excluded. The acceptance campaign itself has no exclusions or retries.

## Verification and implementation size

Whole package suites passed for `obc-formats`, `obc-reader`, `obc-route`, and
`obcm-assemble`. The packer's whole `nav_round_trip` suite passed all 20 cases.
After the final empty-index guard, both reader and assembler package suites
passed again, including all 17 verifier refusal cases and all 15 producer
oracle cases. The producer oracle exercises new-format packer output as the
assembler input. `obc suites check` passed (68 registered suites).

The eight changed runtime source files add 425 lines and remove 152 lines
(net +273), compared with `773758a8`. This includes the fixture preparation
helper inside the packer source. Test sources, benchmark examples, retained
measurement scripts, and the separate browser-cache change are excluded from
that count. The validator alone adds 76 lines and removes 125 lines. Direct
lookup adds a reader entry point; the planner changes its existing settle path.
No selectable planner backend or completed-map rewrite was introduced.

The candidate remains isolated. No production format specification, public
catalog, external fixture package, embedded demo map, or protocol version
vector has been changed. A hard cutover would need these artifacts rebuilt
and published together. The current experiment does not claim those deployment
steps or physical-board resource checks are complete.

## Bounded verification-cache follow-up

The first candidate's verification host-read amplification justified one
additional change: reduce only the browser verification block from 64 KiB to
4 KiB. The memory estimate derives from this constant and updates with it.
Input blocks stay at 4 KiB. No adaptive policy or block-size sweep was added.
The changed runtime is `09bbdd320490625e5f65694dff9fd82a8b348a24`; the retained
harness is `e4f18fca5a465edb1119fea7c4a531b4953a0c07`. The whole web-assemble
unit suite passed all 20 cases. Native and route code did not change, so their
measurements were not repeated.

The fixed follow-up uses three alternating pairs on the pinned graph workload
and three on render-only input. Each candidate is compared with the shipping
v16 baseline at 4 KiB input and 64 KiB verification. All samples are retained.
The renderer's files have separate version-specific hashes. A second readback
hash normalizes only header byte 4 and proves that all other bytes agree.

| Browser workload | Baseline median | Candidate median | Change |
| --- | ---: | ---: | ---: |
| Complete pinned assembly | 10.756 s | 13.360 s | +24.21% |
| Pinned verification phase | 1.590 s | 2.506 s | +57.61% |
| Complete render-only assembly | 1.345 s | 1.432 s | +87 ms / +6.47% |
| Render-only verification phase | 0.331 s | 0.393 s | +62 ms |

The smaller window reduces candidate verification host-read bytes from
5,752,608,316 to 1,592,361,980, while calls rise from 87,587 to 385,595. It does
not remove the producer cost. Both the 15% pinned producer screen and the
render-only screen of `max(5%, 50 ms)` fail. Other desktop activity can influence
these approximate timings; there was no detected compiler overlap. This is the
only follow-up cache size tested. No further tuning or favorable reruns were
performed. The original 64 KiB verification candidate and all its evidence
remain available; the 4 KiB follow-up is not selected.

Raw follow-up data is in `results/browser-verify4.json`; derived values are in
`results/verify4-summary.json`. `browser.py` reproduces the fixed two-workload
campaign. The new readback hash and selectable experiment root are harness-only
changes after assembly timing; they do not alter the worker or format code.
