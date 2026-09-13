# Card retention metadata

This contract defines the payload of flat-store kind `9` (`Metadata`). The board and flat-store
host use route rows for usage stamps and route expiry. `RetentionMachine` remains the only
retention policy owner. Ride rows do not establish a device archive receipt: rides remain unsynced
until the receipt policy has a complete device persistence path.

## Ownership and identity

The card has at most one live metadata object. A retained revision of that same object is
not a second head. Distinct live metadata object IDs are an error. Only the device writes
or removes metadata. Protocol v4 `LIST`, `GET`, and `STATUS` can read it; remote `PUT` and
`REMOVE` return `invalidRequest / badCombination`.

The header binds the payload to the mounted `StoreId`. Each row binds one source object to
its full `ObjectId`, `Revision`, payload length, and payload CRC. A recording or retained
source revision cannot validate a row. A card replacement, source replacement, or missing
source invalidates the corresponding scope. Metadata bytes alone do not prove that a
phone has an archive: future device admission must validate the exact archive receipt.

## Bytes

All integers are little-endian. The payload CRC is the catalog entry's standard CRC-32.
There is no padding or trailing data after the last row.

| Header offset | Bytes | Value |
| --: | --: | :-- |
| 0 | 4 | ASCII `OBRM` |
| 4 | 2 | version `1` |
| 6 | 2 | header length `32` |
| 8 | 2 | row length `40` |
| 10 | 2 | row count |
| 12 | 4 | zero |
| 16 | 16 | mounted `StoreId` |

| Row offset | Bytes | Value |
| --: | --: | :-- |
| 0 | 8 | nonzero source `ObjectId` |
| 8 | 8 | nonzero source `Revision` |
| 16 | 8 | source payload length |
| 24 | 4 | source payload CRC-32 |
| 28 | 4 | UTC seconds: last route use or confirmed ride archive time |
| 32 | 2 | source kind: route `1` or finalized ride `3` |
| 34 | 1 | route retention selection; zero for a ride |
| 35 | 5 | zero |

Route retention values are `0` forever, `1` one day, `2` one week, `3` two weeks,
`4` one month, and `5` two months. Zero timestamps mean no recorded time; a zero ride
time cannot authorize expiry. Absence of a ride row means unsynced. Policy integration
must preserve that protection.

Rows have strictly increasing object IDs, with no duplicates. At most 64 route rows and
128 ride rows fit in 7,712 bytes. The 32-entry ride menu does not limit reconciliation.
Unknown versions, kinds, selections, nonzero reserved bytes, invalid counts, duplicates,
and trailing bytes are errors. Capacity refusal does not discard an existing row.

The fixed [route-and-ride vector](vectors/retention-metadata/route-and-ride.bin) contains
StoreId `42` repeated 16 times and two rows. The route has ID `0x100000001`, revision
`0x200000003`, length `0x300000004`, CRC `0x12345678`, time `0x65000000`, selection `5`.
The ride has ID `0x100000002`, revision `7`, length `123456`, CRC `0xabcdef01`, time
`0x66000000`, selection `0`.

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
generation; identity and payload CRC must validate before any row becomes policy evidence.
Metadata publication checks read-handle capacity before commit. Ordinary capacity pressure returns
`Busy` without publication or a remount fence.

## Runtime admission

The runtime publishes a loaded scope only after its catalog and route metadata reads succeed.
The scope contains the complete StoreId and catalog sequence. A route stamp or automatic removal
carries that captured scope unchanged to the serialized writer. The writer checks it before mutation,
and metadata publication checks its captured metadata head and exact source row.

A successful metadata write invalidates the resident policy snapshot and orders a complete reload.
It does not change the resident timestamp directly. Write and catalog-read failures wait thirty
seconds before retry. Unsupported work is disabled by object family. Uncertain publication parks
policy and catalog refresh until App restart and a fresh mount. The same card identity and sequence
can survive recovery, so those values alone do not release that park.

Before an automatic removal, the retention owner rechecks clock trust, recording state, active route,
and expiry against the loaded snapshot. The board waits for that admitted writer operation to finish
before it processes another App transition. Rider-requested deletion remains a separate intent.
