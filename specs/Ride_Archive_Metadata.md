# Ride archive metadata

The device keeps proof that a client holds the exact finalized ride bytes. This proof
controls the synced indicator. It does not authorize deletion. The device never deletes
routes or rides automatically.

One current Metadata object stores the proof rows. Its object identity and revision follow
the flat-store contract. The private payload is `OBRM`, version 1, with a 32-byte header
and 40-byte rows. Integers use little-endian encoding.

| Header offset | Bytes | Value |
| --: | --: | :-- |
| 0 | 4 | ASCII `OBRM` |
| 4 | 2 | version `1` |
| 6 | 2 | header length `32` |
| 8 | 2 | row length `40` |
| 10 | 2 | row count, at most `128` |
| 12 | 4 | zero |
| 16 | 16 | physical StoreId |

| Row offset | Bytes | Value |
| --: | --: | :-- |
| 0 | 8 | source ObjectId |
| 8 | 8 | source revision |
| 16 | 8 | source payload length |
| 24 | 4 | source payload CRC-32 |
| 28 | 4 | archive timestamp; zero when unknown |
| 32 | 2 | kind `3` (Ride) |
| 34 | 6 | zero |

Rows have strictly increasing nonzero object IDs and revisions. Duplicate IDs, other kinds,
unknown versions, nonzero reserved bytes, invalid lengths, and trailing bytes are errors.
Absence of a row means no proof. A zero timestamp still proves archive possession.
There is no background timestamp update.

The [two-ride vector](vectors/ride-archive-metadata/two-rides.bin) binds two exact source tuples
on StoreId `42` repeated 16 times. The codec tests verify the complete bytes and malformed rows.

## Load, reconcile, and publish

Only absence creates an empty default. Invalid payloads, duplicate heads, wrong card
identity, and media failure have distinct typed errors. Load checks the complete payload
CRC and closes its temporary read handle. A loaded draft captures its metadata base head;
a later load cannot refresh an older draft's publication authority.

Reconciliation scans the complete catalog and checks that the scan succeeded before it
removes stale rows. A failed scan changes no rows. Publication checks the captured metadata
head and every remaining source row again. A supplied target must match the exact current
source head captured before the stamp decision. No target permits reconciliation to publish
an empty image. A draft edit or successful reconciliation is not a durable outcome.

Publication allocates fresh payload space and commits removal of the old metadata head and
insertion of the next revision in one batch. It never amends the authoritative payload.
Existing readers can finish reading the old bytes. Success requires exact committed
readback, then advances the draft's base revision.

A payload or catalog-body failure before publication cancels the allocation and permits retry.
An uncertain final gate write or synchronization, or a failed committed readback, returns
`RemountRequired`. The generic store blocks mutations and fresh catalog reads. No durable success
is reported. Existing pinned readers can finish. Remount can recover the old or the complete new
generation. Policy reads validate identity and payload CRC, reconcile against the complete catalog,
and complete a media sync barrier before exposing any rows. A live-medium remount can expose an
unflushed gate; a failed barrier fences the store and returns no policy rows.
Metadata publication checks read-handle capacity before commit. Ordinary capacity pressure returns
`Busy` without publication or a remount fence.

## Archive and display

`ARCHIVE_RIDE` checks the exact current finalized source tuple, then writes a proof row
with timestamp zero. A duplicate preserves the row and repeats the durability barrier.
The reader overlays proof on the visible ride summaries. Manual deletion remains available.
