# Remaining direct-reference designs

The post-layout converter is not the only producer design. The assembler knows
final node placement before it writes the output. It can build a small mapping
stream at that point and resolve references during its normal write. The probe
measures that work. It does not validate a changed wire format.

## Dense identity and file location are different fields

A direct reference need not replace the node's dense identity. The current
validator uses dense identities for its coordinate bands, duplicate bitmap,
edge claims and component union-find. It still allocates a whole-graph component
array and done bitmap. Simply using sparse packed locators as those array
indices would increase memory and is not a valid cutover.

One option keeps the current dense ID in each node and replaces each neighbor
ID with a locator. The reader can compute a node's own locator from the chunk
and ordinal during traversal. The planner can use that locator as its search
key. The validator can resolve a neighbor locator to the target's dense ID.
That preserves dense indexing but introduces target reads in validation.

The pinned graph has 2,633,471 directed neighbors. One uncached 512-byte target
read per neighbor is 1,348,337,152 bytes. Current full verification reads
1,103,026,810 bytes in total. These figures cannot simply be subtracted: the
new target reads may replace some current verification walks, and caching can
reduce them. A complete validator must measure its own work.

Adding a separate four-byte locator to every neighbor while retaining its ID
is less attractive: a degree-24 record grows from 421 to 517 bytes and no
longer fits one chunk. It also adds 10,533,884 raw bytes on the pinned graph.

## Capacity and device RAM

| Allocation | Node-run reach | Records per chunk | Consequence |
| --- | ---: | ---: | --- |
| 32 bits: 27 chunk + 5 ordinal | 64 GiB | 32 representable | Cannot hold all 39 possible degree-zero records without repacking |
| 32 bits: 26 chunk + 6 ordinal | 32 GiB | 64 representable | Covers 39 records and reserves impossible ordinals for sentinels |
| 40 bits: 34 chunk + 6 ordinal | 8 TiB | 64 representable | Needs wider planner keys or another lookup design |
| 32 bits: byte offset divided by 16 | 64 GiB | At most 32 aligned starts | Adds record padding; keeps current planner key width |

Only ordinals below the actual physical maximum are valid. The earlier
experiment reserved two ordinals in every chunk and therefore refused more
than 30 records. A 6-bit ordinal avoids that refusal without reducing packing.

The one-map contract in issue #1420 permits maps beyond 64 GiB by raising the
header offset scale. It also calls for a DACH-class acceptance map; it does not
state a measured DACH node-run size. A 32 GiB node ceiling must therefore be an
explicit new capacity decision or have a proof against the required coverage.
Matching the edge pool's 64 GiB ceiling does not prove equal node capacity.
The current assembler's `ByteSpill::push` uses a u32 byte offset and separately
refuses record spills beyond 4 GiB. That existing implementation limit is not
permission to reduce the broader contract.

A 40-bit neighbor locator adds one byte per neighbor. The node keeps its dense
ID, so its maximum record grows from 421 to 445 bytes. Raw growth on the pinned
graph is 2,633,471 bytes, or 2.7% of the navigation section. Chunk packing could
add more; this is not a measured output-size result.

Wider planner keys are expensive. Current `NavEntry` is 24 bytes with a u32
key. Replacing that field with u64 makes the C layout 32 bytes: 12,288 extra
bytes for 1,536 entries. A separate packed extra byte still costs at least
1,536 bytes. Both exceed the frozen 512-byte workspace allowance. A wider
reference is not a free way to preserve file capacity.

## A simpler next candidate: aligned record offsets

A u32 locator can name `node_run_relative_byte_offset / 16`. Align every node
record start to 16 bytes and keep the one-chunk record boundary. This reaches
64 GiB with a 32-bit planner key and points directly to the record offset.
Reserve the last two units so the planner sentinel values cannot name nodes.
The reader must still reject padding and malformed record boundaries; alignment
alone is not proof that a target is a real record.

The pinned map's actual degree histogram gives a small raw alignment cost:

| Measurement | Bytes |
| --- | ---: |
| Current record payload | 57,907,262 |
| Payload with each record rounded to 16 bytes | 59,988,464 |
| Added alignment padding | 2,081,202 |
| Current padded node chunks | 59,555,840 |

The padding is 2.11% of the current navigation section. Of 1,010,635 nodes,
561,023 have degree three; their existing 64-byte records need no padding.
A maximum-degree record grows from 421 to 432 bytes including tail padding,
so it still fits one chunk. Degree-zero records use 16 bytes and at most 32
fit, without the earlier candidate's 30-record refusal.

This does not predict final output size. With current chunk membership,
44,392 of 116,320 chunks would exceed 512 bytes; the largest would need 592
bytes. Spatial leaves and bin packing must be rebuilt with aligned lengths.
`alignment.py` and `alignment.json` retain the exact arithmetic and input hash.

This is the simplest next layout candidate identified here. It avoids widening
planner keys and the 32 GiB ceiling of the 26/6 allocation. It still needs the
full node-capacity argument, integrated producer, verifier and actual reader
measurement. The 64 GiB edge ceiling alone is not that argument.

## Cell-local references and preserved blocks

A cell directory could assign each copied node run a global chunk base. A
local packed neighbor reference becomes a global reference by adding that
base. With the 26/6 allocation, this arithmetic keeps a u32 planner key and
needs no per-node global remap. It still has the same aggregate 32 GiB node
run ceiling. The device must know which cell owns a settled chunk; a directory
search or cached range supplies the base needed to interpret its neighbors.
A fixed cell-index/local-chunk bit split instead places separate ceilings on
cell count and cell size.

Keeping global dense IDs as `cell_base + local_dense_id` preserves a u32 key
without a fixed chunk ceiling. But it does not locate variable-length records.
The device must use a cell-local spatial lookup or an ID-to-placement table.
The table can be on disk with a bounded cache; its extra reads are a cost to
measure, not a reason to reject the design without testing it.

Seams need a canonical node and merged adjacency. Pruned nodes, duplicate
seam edges and capped adjacency need exceptions to the copied content. A
small measured seam count makes a sparse overlay plausible; it does not prove
that overlay state can stay resident at larger coverage. A bounded on-disk
overlay is a possible design. Existing edge geometry is already copied, so
copying whole cell blocks must save enough indexing and join work to repay
its directory, overlay, validation and device lookup costs. The cell-order
experiment did not test this design and cannot reject it.
