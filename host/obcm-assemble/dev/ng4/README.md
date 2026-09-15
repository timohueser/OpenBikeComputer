# Integrated direct-reference cost probe

This experiment isolates final placement and reference resolution inside the real
assembler. It does not produce or validate a direct-reference map. The v16 output
and full current validator remain unchanged.

The replay patch reads the final node placement plan, sorts an 8-byte mapping
record per node by dense ID, and writes a 4-byte-per-node scratch table. The normal output walk then resolves each node ID and every neighbor ID through
a fixed 64 KiB cache. The resolved references are consumed by a checksum; the
written records remain v16. There is no extra record-read pass. No whole-map copy, post-write rewrite, or
second full output validation is part of this probe.

References use 26 chunk bits and six ordinal bits. Ordinals 0 through 38 cover
all records that fit in a 512-byte chunk, including 39 degree-zero records.
Other ordinals are invalid, including the two planner sentinel IDs. The node
run has a 32 GiB ceiling. This is an experiment allocation, not a new format
contract. The probe refuses any dropped or missing dense node placement.

## Replay

Apply `probe.patch` to the `source_commit` in `results.json` in a disposable
checkout. Build with the current toolchain:

```sh
cargo build --release -p obcm-assemble --features mem-profile
```

The same binary runs baseline mode without `OBC_NG4_PROBE`, and probe mode with
`OBC_NG4_PROBE=1`. This switch exists only in the retained replay patch.

```sh
python3 host/obcm-assemble/dev/ng4/run.py target/release/obcm-assemble /tmp/obc-ng-inputs/data /tmp/obc-ng4-replay
```

The runner first checks NG1's pinned inputs, then runs three alternating pairs.
The extra mapping phase appears as `nav-profile integrated_reference_probe`.
Reference resolution is included in write time. Normal native full verification
and independent output SHA-256 checks remain enabled. Use a fresh output path.

## What remains outside this probe

- Emitting and fully validating direct-reference wire records.
- Browser cost, physical device cost, and large-map capacity acceptance.
- Changes to the reader, planner, format, published cells, or obc-pack.
- A production speed claim or adoption decision.

A lower probe cost than the old post-layout converter does not by itself prove
that a complete candidate meets the 5% assembly regression limit.

For the separate alignment arithmetic, use the fully verified pinned output:

```sh
python3 host/obcm-assemble/dev/ng4/alignment.py /tmp/obc-ng-native-final/native-0.obcm
```

This reports payload padding and which existing chunks would overflow. It does
not rebuild the spatial tree or provide a candidate output-size measurement.
