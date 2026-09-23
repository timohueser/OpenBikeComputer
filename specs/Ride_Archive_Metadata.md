# Ride archive metadata

The device keeps proof that a client holds the exact finalized ride bytes. This proof
controls the synced indicator. It does not authorize deletion. The device never deletes
routes or rides automatically.

One current Metadata object stores the proof rows. Its object identity and revision follow
the flat-store contract. The private payload is `OBRM`, version 2, with a 32-byte header
and 40-byte rows. Integers use little-endian encoding.

| Header offset | Bytes | Value |
| --: | --: | :-- |
| 0 | 4 | ASCII `OBRM` |
| 4 | 2 | version `2` |
| 6 | 2 | header length `32` |
| 8 | 2 | row length `40` |
| 10 | 2 | row count, at most `128` |
| 12 | 2 | checkpoint version: `0` when absent, otherwise `1` |
| 14 | 2 | checkpoint length: `0` when absent, otherwise `96` |
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
unknown versions, nonzero reserved bytes, and invalid lengths are errors.
Absence of a row means no proof. A zero timestamp still proves archive possession.
There is no background timestamp update.

The [two-ride vector](vectors/ride-archive-metadata/two-rides.bin) binds two exact source tuples
on StoreId `42` repeated 16 times.

## Navigator checkpoint

The checkpoint starts at `32 + row_count * 40`. The Metadata header StoreId binds
both route fingerprints to the same card. This is only a recovery offer for the
route guidance was following, whether the rider selected it or accepted it as an
Assistant plan; it does not restore navigation or start Recorder by itself.
Absence preserves ordinary boot behavior.

| Checkpoint offset | Bytes | Value |
| --: | --: | :-- |
| 0 | 8 | followed Route ObjectId, nonzero |
| 8 | 8 | followed Route Revision, nonzero |
| 16 | 8 | followed payload length, nonzero |
| 24 | 4 | followed payload CRC-32 |
| 28 | 4 | matched progress, metres |
| 32 | 28 | optional original Route ObjectId/Revision/length/CRC; all zero when absent |
| 60 | 4 | route occurrence anchor |
| 64 | 4 | signed longitude, microdegrees |
| 68 | 4 | signed latitude, microdegrees |
| 72 | 1 | phase: `0` Following, `1` Outbound, `2` AtStop, `3` Returning |
| 73 | 1 | unresolved avoidance: `0` or `1` |
| 74 | 1 | selection: `0` an accepted plan, `1` the rider's own route choice |
| 75 | 1 | zero |
| 76 | 4 | lower progress bound, metres |
| 80 | 4 | upper progress bound, metres |
| 84 | 12 | zero |

Progress must be within the stored bounds. Coordinates must be geographic.
Visit phases require an original fingerprint. An absent original must have all
28 bytes zero. A selection has no original and phase `0`, and its progress is not
a measured anchor. Unknown phases or nonzero reserved bytes are invalid.

Checkpoint edits validate their exact current Route targets separately from
ride proof rows. They use the current complete image and the same Metadata-head
CAS, allocation, publication, and verified readback. Archive mutations
preserve the checkpoint bytes; checkpoint mutations preserve all valid proof rows.
A stale draft cannot refresh its authority through another writer's load.

Recovery checks the exact current source heads, lengths, and CRCs. It reads and
checks source payload CRCs with bounded block scratch, closes temporary holds, and
completes the media sync barrier before offering Resume. Route removal or
replacement must refuse while the checkpoint depends on that exact object.
Clearing or completing the journey releases its original dependency.

Acceptance adds a route-only `ASSISTANT_ACCEPTED` catalog amendment
to the same batch that replaces Metadata. The amendment preserves the exact source tuple and
payload extents. Readback checks both the Metadata bytes and the accepted catalog entry.
The commit is atomic: recovery cannot expose a checkpoint without its acceptance flag.
Clear preserves the flag; a new route payload cannot inherit it.

Explicit cleanup skips both checkpoint source IDs and the live active route. Explicit removal
or replacement of either checkpoint source is refused. Invalid Metadata makes cleanup fail
closed. It does not prevent unrelated Ride payload mutations. No automatic expiry is added.

## Trip progress records

The trip progress records follow the checkpoint, or the rows when there is no checkpoint. The
record count is the remaining payload length divided by 96, at most `16`. Records are in write
order, each with a unique nonzero trip key. `obc-ble-interface-spec.md` §7.7 has the rules. A
record never holds a route: route removal and replacement ignore it.

| Record offset | Bytes | Value |
| --: | --: | :-- |
| 0 | 8 | trip key, nonzero |
| 8 | 8 | position day route ObjectId |
| 16 | 8 | position day route Revision |
| 24 | 4 | position metres |
| 28 | 2 | position day, from 0 |
| 30 | 2 | last finished day; `0xFFFF` when none |
| 32 | 64 | finish dates of days 0 to 31, `u16` days since 1970-01-01; 0 = none |

A remaining length that is not a multiple of 96, more than 16 records, a zero key or a duplicate
key is an error. Proof-row and checkpoint edits preserve the records.

## Size

The image holds all 128 ride proof rows, the optional checkpoint and 16 progress records: at most
6,784 bytes. It has no route proof rows.

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
