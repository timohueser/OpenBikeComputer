# Cell-order assembly experiment

Decision: retain the current production assembly. The tested alternative did not establish
NG1's required 10% native and browser speed improvement. Full cell-block preservation was
not implemented or measured; no general claim about its performance follows from this run.

## Reproduce

The source is commit `55be89b653f2094cc67c192e3fd2fa31673a8cd8` plus
`candidate.patch`. `decision.json` pins the patch SHA-256. The patch includes the actual
assembler replacement and the focused correctness cases. No candidate reader or format is
installed. The patch retains unused original sorting functions for review; a shipping
implementation would remove them and update the OBCA numbering contract.

From a checkout containing this report and the NG1 measurement files:

```sh
python3 host/obcm-assemble/dev/navigation/fetch.py /tmp/obc-ng3-inputs
export OBC_NG3_REPORT_ROOT="$PWD"
git worktree add --detach /tmp/obc-ng3-replay 55be89b653f2094cc67c192e3fd2fa31673a8cd8
cd /tmp/obc-ng3-replay
git apply "$OBC_NG3_REPORT_ROOT/host/obcm-assemble/dev/ng3/candidate.patch"
./tools/obc test -p obcm-assemble
cargo build --locked -p obcm-assemble --release --features mem-profile
python3 "$OBC_NG3_REPORT_ROOT/host/obcm-assemble/dev/navigation/native.py" "$PWD/target/release/obcm-assemble" /tmp/obc-ng3-inputs /tmp/obc-ng3-candidate-results
```

Use another checkout of the same base without the patch to build the comparison binary.
Run the same native harness with the same input directory and a separate output directory.
It verifies all pinned input metadata and hashes before running exactly three assemblies
with full validation, a 16 MiB sort budget, and output digest read-back.
The raw JSON's `source_commit` identifies the caller checkout. For the recorded comparison,
the executable source is the base commit above; for the candidate it is that base plus the
pinned patch. Binary SHA-256 values in each report identify the executed files.

## Scope and correctness

The candidate replaces global node sorting and two endpoint sort joins with cell-order
numbering and endpoint lookup using one cell's mapping plus a boundary-node table.
Interior identities follow canonical cell order; boundary identities form a sorted suffix.
Component pruning retains the original input collection order and its existing tie-break.
No whole-map node table is resident. The mapping contains 24 bytes per input node on scratch.

Edge geometry bytes still copy unchanged, as in production. All node records, adjacency,
edge placement, and node/snap indexes are rebuilt. No complete cell blocks are preserved.
There is no extra bake work or metadata and no device code or resident-state change.

An initial cell-order emission key selected different directed arcs at a 26-spoke boundary
junction. The retained patch fixes this by carrying both endpoint digests and sorting on
coordinates and digests. The remaining edge-sort record grows from 51 to 67 bytes.
The corrected package suite passed: 83 library tests, 14 original oracle cases, and all
pinning, sort-budget, and refusal cases. The added input-permutation oracle also passed.
The oracle invokes the real cutter, assembler, complete-map validator, reader, and planner
for routes across longitude and latitude cell seams. Existing component tests compare
hierarchical pruning with a flat graph oracle.

The strengthened cap case alternates 26 spokes between two cells, uses a square bbox, and
checks the geographic endpoints of the 24 retained arcs. It does not assume a candidate
node numbering. The permutation case compares output bytes for the same connected fixture
in forward and reversed cell order. These are focused case evidence, not a proof for every
possible graph. There is no candidate four-cell-corner routing or route-latency measurement.

## Recorded comparison

`candidate.json` and `comparison-baseline.json` retain all six executed commands, binary
hashes, phase times, memory/I/O ledgers, output hashes, and full verification summaries.
The baseline executable SHA-256 is
`b57afa8e8a7331e772473ccc9de86aa710cb49ecc8832a6d60ec1ea4d2bee083`.
The candidate executable SHA-256 is
`84b85995d1344743e9c757262aa7dd2a205f1beb09b9a13626379346ee086c32`.
Both use the same release profile, empty Rust flags, and `mem-profile` feature.

| Observation | Comparison baseline | Candidate |
| --- | ---: | ---: |
| Total seconds, three runs | 4.526 / 4.411 / 4.459 | 19.038 / 18.431 / 10.142 |
| Median navigation seconds | 2.169 | 8.413 |
| Median write seconds | 0.965 | 3.262 |
| Median verification seconds | 1.307 | 5.256 |
| Output bytes | 247,837,696 | 247,837,696 |
| Native allocator peak bytes | 51,346,355 | 51,346,325 |
| Scratch writes / bytes | 244 / 607,409,419 | 216 / 486,912,301 |
| Scratch reads / bytes | 1,356,390 / 689,279,449 | 1,362,987 / 597,272,435 |
| Peak live scratch bytes | 152,769,123 | 173,419,901 |

Input reads match at 2,758,507 calls / 341,256,925 bytes; verification reads match at
1,971,277 calls / 1,103,026,810 bytes; output writes match at 239 calls / 247,837,696 bytes.
The candidate writes 19.8% fewer scratch bytes and reads 13.3% fewer, but keeps 13.5% more
live scratch. Scratch storage is separate from native allocator peak.

The candidate's large time spread and slowdown in unchanged phases prevent attributing
the full elapsed difference to the algorithm. No additional candidate samples were taken.
Three subsequent baseline runs confirmed the original binary and retained its normal
scale of execution. This does not establish a reliable candidate speedup. The conjunctive
adoption gate is unmet, so no browser candidate run was made. Browser capacity, browser
process memory, physical SD traffic, and device routing overhead were not measured here.

The candidate has 1,010,635 nodes, 1,317,371 edges, 693 pruned nodes, 503 pruned edges,
and no truncated adjacency or dropped nodes. Full validation passes in every run.
Candidate output SHA-256 is
`88ae41b8759b2110d0fc35897d9a1e08b11789a4705888283d92a7385813cbe0` in all three runs.
The baseline output hash is the NG1 hash. The wire version stays v16, but the experimental
node ordering differs from the current OBCA numbering contract. Graph and route comparisons permit different node identities.

## Decision boundaries

The smaller candidate was tested before introducing a cell directory or boundary-reader
machinery. Its replacement helper is 161 lines in the retained patch, plus call-site and
comparison changes. The original global node-sort and join implementations remain unused
in the replay patch; they are not counted as new required production machinery.

A full block-preserving design still needs a separate prototype for seam ownership,
pruned-record removal, neighboring-cell snapping, deterministic layout, and bounded
reader access. Source analysis alone does not reject all such designs. The conceptual
pass inventory and the current production behavior are documented in
`docs/content/software/graph-assembly.md`.
