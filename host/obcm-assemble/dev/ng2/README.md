# Direct node reference experiment

This directory retains the bounded candidate for issue #1774. It is an experiment,
not a supported map format or a shipping format switch. `candidate.patch` changes
the shared reader and planner in a disposable checkout. The patch accepts version
250 instead of version 16. The converter writes version 250. Do not publish these
maps or use this patch in a device build.

## Reference and layout

The candidate uses `u32 = chunk << 5 | ordinal`. The chunk range is
`0..2^27`; the ordinal range is `0..30` (exclusive). Ordinals 30 and 31
are invalid in every chunk. Thus both planner virtual identities, `0xffffffff`
and `0xfffffffe`, are impossible physical nodes. Chunk byte addressing uses
checked `u64` arithmetic and the current directory bounds. The node run can
reach 64 GiB. This equals the current edge pool limit and default scaled-offset
reach, but it does not prove equal graph capacity: node bytes can exceed edge
bytes. Production adoption requires a separate capacity argument or a wider
reference allocation to preserve the larger header scales in #1420.
It does not restrict the absolute file offset of that run to 32 bits. The
header can address a larger overall file at a larger scale; this candidate
does not claim that its node run can exceed 64 GiB.

A node is still `13 + 17 * degree` bytes. Degree 24 is still 421 bytes.
The edge reference allocation alone is not proof that a node fits: a node with
zero neighbors is 13 bytes, so 39 such nodes can fit in a v16 chunk. The
prototype refuses chunks with more than 30 records. It never drops excess
records. A production cutover must handle this case in both producers' leaf
construction and bin packing, including co-located records at the split floor.
Nodes with at least one neighbor are at least 30 bytes, so at most 17 fit.

Resolution first rejects reserved ordinals and out-of-directory chunks. It
reads one chunk through the caller's existing navigation cache. It walks at
most 30 record headers, checks degree and each record end, and requires the
selected record's own identity to match the reference. The planner also checks
that the target coordinates match the coordinates used to discover that node. It yields no node for
an absent or malformed target, as the previous settle path does. A source
failure remains an error. Coordinates, neighbor facts, edge bytes, routing
weights, cache size, read budget and settle cap do not change.

## Producer work and limits

`convert.rs` takes the actual output of `obc-pack` or `obcm-assemble` after its
full normal verification. It does these additional steps:

1. Count records in the final node chunks and check candidate capacity.
2. Create a file-backed table of 12 bytes per source node: reference and
   coordinates. Scan final placement again and write the table by dense ID.
   Reject duplicate or non-dense source IDs.
3. Copy the whole source file to the output, then rewrite only node and
   neighbor IDs in each node chunk. Resolve each neighbor through the table
   and check its coordinate deltas. Change the header version to 250.
4. Read every output node and resolve every neighbor against its actual
   target record. Check target range, record boundary, identity and coordinates.

The converter holds fixed 8 KiB buffers and one chunk at a time, plus a second
chunk for validation. Its only variable allocations are two maximum 30-element
lists of record positions during target verification (480 bytes on this host). The table is on disk; no map-sized array lives in
the reader or planner. All explicit file reads and writes have counters,
including the complete copy and target validation. These are logical file
operations; they do not measure physical storage commands. Output `sync_all`
is part of conversion time. The reported conversion time includes layout;
verification time is separate. Scratch bytes are the table length. Peak
live storage also includes the original full output and the candidate copy.

This late remap is deliberately simple. It adds staging and random lookup
I/O after existing full-graph assembly sorts. A future integrated producer
could instead spill final placement references, externally sort them by dense
ID, join adjacency targets against them, sort patch locations, and rewrite
records in output order. That needs additional passes and must be measured;
its cost is not represented as an achieved optimization here. The native
file-backed converter does not prove browser-worker conversion cost.

## Reproduction

Use the pinned NG1 manifest and harness in `host/obc-bench/dev/navigation`.
The decision report records exact input digests and commands. Build the converter:

```sh
rustc --edition=2021 -O host/obcm-assemble/dev/ng2/convert.rs -o /tmp/ng2-convert
/tmp/ng2-convert INPUT.obcm OUTPUT.ng2-obcm SCRATCH_TABLE
```

Both output paths must be new. Keep the source input. The source must be a
fully validated v16 file; the converter only validates the navigation rewrite.
Apply the replay patch only in a disposable checkout at the report's pinned
source, then build that checkout's actual planner harness. `--candidate` on
the NG1 runner permits the converted input digests and records them in the
result. Candidate route output hashes must be compared with baseline hashes;
equal length alone is insufficient evidence of preserved routing behavior.

The replay patch includes focused checks in the reader's existing `extremes`
suite for reserved IDs, missing targets, record-boundary corruption, identity
mismatch, failed reads and cache reset on replacement. Run the whole suite:

```sh
cargo test -p obc-reader --test extremes
cargo clippy -p obc-reader -p obc-route --lib -- -D warnings
```

No public documentation changes: the shipping format and behavior are unchanged.

For the existing generated-map route oracles, `test-producer.patch` also remaps
the packer's final chunks with a host-only hash table. This adapter lets the
unchanged route suite generate candidate maps. It is not the bounded producer
used for the measured assembly comparison. Do not apply this adapter when
producing the v16 baseline input for `convert.rs`.
