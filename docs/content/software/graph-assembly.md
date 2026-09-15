---
title: Graph assembly evaluation
description: Why assembly retains one navigation graph and how a cell-order alternative was evaluated.
copy: ai
---

# Graph assembly evaluation

The assembler retains one navigation graph in one OBCM file.
A bounded experiment did not establish the required speed improvement.
The production reader, planner, format, and assembly algorithm are unchanged.
Further optimization remains open. The retained experiments evaluate specific
candidates; they do not establish an optimal graph layout or navigation algorithm.

## Current work

The assembler copies each surviving edge geometry record without changing its bytes.
It rebuilds graph identities, adjacency, and spatial indexes.
Node and edge bookkeeping uses bounded sorts and scratch streams.
Component pruning uses a local union-find for each cell, then joins local components through
boundary nodes. It does not use a whole-map node union-find.
See [src:host/obcm-assemble/src/nav.rs] and [src:host/obcm-assemble/src/prune.rs].

The measured baseline attributed 50.0% of native assembly time and 44.2% of browser
assembly time to navigation work. This justified a small experiment; it did not establish
that a new layout would be faster.
The [measurement record](https://github.com/timohueser/OpenBikeComputer/tree/develop/host/obcm-assemble/dev/navigation)
contains the pinned inputs, build identity, commands, and limits.

## The alternative that was tested

The experiment kept the existing reader and single graph.
It assigned dense node identities to cell interiors in grid order, followed by boundary
nodes in coordinate order. It resolved edge endpoints with one cell's mapping and a
boundary-node table instead of two global sort joins.
Global pruning still used the original input collection order.
This kept its component tie-break separate from deterministic output numbering.

This experiment retained cell ordering, not complete baked cell blocks.
It rebuilt node records, adjacency, edge placement, and both spatial indexes.
It did not add a cell directory, alias reader, bake summary, or new firmware table.

| Pass | Tested alternative |
| :-- | :-- |
| Collect serialized nodes and edges | Retained |
| Unify boundary nodes | Retained |
| Remove duplicate edges | Retained |
| Label and prune global components | Retained |
| Sort all nodes to assign identities | Replaced by cell and boundary counts plus streamed mapping passes |
| Join both endpoints through global sorts | Replaced by per-cell lookup and boundary lookup |
| Lay out edge records and create snap anchors | Retained; endpoint order keys made the sort records larger |
| Rebuild adjacency and node/snap indexes | Retained |
| Copy output and validate the complete map | Retained |

Changing node order first changed which arcs survived the degree limit at a boundary
junction. The corrected experiment carried the original endpoint coordinate and digest
keys into edge ordering. Its edge-sort record grew from 51 to 67 bytes.
A focused case with 26 alternating spokes across two cells checks the retained directed
arcs at the limit of 24.

The actual cutter, assembler, reader, and planner passed the existing multi-cell route
oracle with the corrected experiment. The checks also covered pruning, missing cells,
duplicate ascents, and complete-map validation. Reversing the connected fixture's cell
inputs produced the same candidate bytes.
These checks support the tested cases; they do not prove every possible graph equivalent.

## Decision and measurement limits

The criterion required at least 10% lower median total assembly time in both native and
browser runs, with three fixed samples, non-overlapping time ranges, full validation,
at most 5% growth in peak memory or output size, and unchanged route behavior.

The candidate's native totals were 19.038, 18.431, and 10.142 seconds.
Three subsequent runs of the original binary took 4.526, 4.411, and 4.459 seconds.
Candidate time varied strongly, and its unchanged collection, write, and verification
phases also slowed. The runs did not establish the required speed improvement.
They do not prove that the new graph ordering caused the entire elapsed-time difference.
No browser candidate run or further timing campaign was performed.

The repeatable size and logical I/O observations were:

| Observation | Current assembly | Candidate |
| :-- | --: | --: |
| Output bytes | 247,837,696 | 247,837,696 |
| Peak native allocator-owned bytes | 51,346,355 | 51,346,325 |
| Scratch bytes written | 607,409,419 | 486,912,301 |
| Scratch bytes read | 689,279,449 | 597,272,435 |
| Peak live scratch bytes | 152,769,123 | 173,419,901 |

Lower scratch traffic alone is insufficient for adoption. Live scratch grew by 13.5%.
Scratch storage is separate from allocator-owned memory.
The output passed full validation and was deterministic across all three candidate runs.
Its hash differs from the current output because its node identities differ.
The [experiment record](https://github.com/timohueser/OpenBikeComputer/tree/develop/host/obcm-assemble/dev/ng3)
retains the replay patch, raw results, checks, and exact limits.
Experimental production changes were removed.

## What remains unmeasured

Full cell-block preservation was not implemented or timed.
The source review identified additional work for such a design:

- Give coincident boundary nodes one graph identity, including four-cell corners.
  Equal interior coordinates must remain separate nodes.
- Retain opposite road stubs and remove only duplicate full edges, including their snap
  anchors. Preserve the selected duplicate's directional ascents.
- Apply the component threshold across selected cells. Two fragments below the local
  threshold can form a retained global component. An absent cell contributes no connection.
- Omit pruned records from the output. Compacting a partly pruned cell can change its node
  records, edge placement, references, and indexes.
- Query neighboring selected cells during snapping and apply the same canonical identities
  during planning and route emission. Bound directory access and firmware memory.
- Include all copied bytes, changed records, bake metadata, validation, and reader work in
  the comparison. Existing edge geometry copying is not a new saving.

These are design requirements, not measured proof that every cell-block layout is too
complex or too slow. The current no-adopt decision applies to the tested cell-order
alternative. A future cell-block experiment needs its own bounded design and evidence.

## Follow-up: browser input reads

A later comparison used the same shipping worker and pinned input with its existing
read-block option. Reducing the block from 64 KiB to 4 KiB reduced median total
assembly time from 19.278 to 10.853 seconds across three fixed pairs. All six
outputs passed full validation and had the same independent digest. Logical input
read bytes fell by 92.2%, while input call count increased. Verification alone
became slower because the option also changes its read cache.

This result identifies input caching as a smaller implementation opportunity.
The production default remains unchanged. The Chromium profile used persistent
OPFS on a memory-backed host filesystem; physical storage performance and other
workloads remain unmeasured. The
[follow-up record](https://github.com/timohueser/OpenBikeComputer/tree/develop/host/obcm-assemble/dev/followup)
contains all samples, source identities, and limits.
