# Ride archive and persistence receipts

This contract defines the iOS archive boundary and the device archive-proof boundary.
[ARCHIVE_RIDE](FLAT_Store_Protocol.md#312-archive_ride) carries proof into
[card archive metadata](Ride_Archive_Metadata.md). iOS delivers and retries these receipts; the board uses validated proof for its synced indicator.

## Source identity

A ride source is the tuple `(StoreId, ObjectId, Revision, payloadLength, payloadCRC32)`:

| Field | Meaning |
| :-- | :-- |
| `StoreId` | All 128 bits of the mounted store identity, encoded as 32 lowercase hexadecimal digits in the client library. |
| `ObjectId` | The nonzero card-local `u64` object identity. |
| `Revision` | The nonzero `u64` revision served by GET. |
| `payloadLength` | The verified `u64` byte length of the served object. |
| `payloadCRC32` | The verified CRC-32/IEEE of those bytes. |

The library ride key also contains the device serial. The source's store and object must match
that key. Neither the serial nor ObjectId alone identifies the archived device content.

The iOS catalog carries this source for each finalized ride. Download requests pin its revision.
The client checks the expected StoreId with LIST before and after GET, including after a restored
link. It checks the served revision, length and CRC against the requested source. A mismatch fails
the download. The canonical archive is decoded from the returned payload; an earlier catalog's
display fields do not replace the downloaded fields.

[GET](FLAT_Store_Protocol.md#35-get) completion is transport delivery. It is not client storage
proof.

## Atomic local archive

`FileLibraryStore.archiveRide` receives a decoded ride with its verified source. It commits the
summary, all canonical samples, source record and local downloaded flag as one generation.
Samples preserve timestamps, coordinates, optional elevation and sensor values, and segment starts.
Wire bytes are not the library schema.

Each `rides/<library-key>/summary.json` is a version 3 manifest. It contains the canonical summary,
its source, the downloaded flag, and the name, length and CRC of one immutable
`points-<UUID>.json` file. That points file contains version 3 canonical point records. The manifest
is the only publication point. A list reads manifests without reading track samples. A local
summary edit keeps the source and points reference; it does not rewrite samples.

The production persistence anchor is the existing `Library` directory inside the app container.
The archive can create `Application Support/OBCLibrary` and its descendants below that anchor.
A custom `FileLibraryStore(directory:)` uses the directory's existing parent as its anchor; the
caller must supply a durable, accessible parent. Directory creation and barriers stay inside this
boundary. They never traverse ancestors outside the app container.

The commit order is:

1. Encode both records before changing the archive. Refuse an unreadable manifest or an unknown
   reserved path. Do not overwrite unsupported archives.
2. Create missing directories and sync their parents. Write a new points file and complete its
   `F_FULLFSYNC` barrier. Sync the directory path before publishing a reference to the new file.
3. Write a temporary manifest and complete its `F_FULLFSYNC` barrier.
4. Rename the manifest atomically to `summary.json`. Sync the archive directory and its ancestors through the persistence anchor,
   then complete `F_FULLFSYNC` on the published manifest.
5. Return a local `RideArchiveReceipt` with the source tuple. Unreferenced transaction files can
   then be removed.

A write, rename or barrier failure throws an error and returns no receipt. Before publication,
the previous generation remains visible. After publication, a failed final barrier is uncertain:
the complete new generation can be visible, but visibility alone is not proof of persistence.
The old point file remains until a later successful commit. A failed batch keeps earlier commits.

After reopen, `archivedRideSource` checks the manifest, point length and point CRC. It completes
file and directory barriers through the same anchor again before returning the source. This can make an uncertain
visible generation durable. A failed check or barrier returns no source, so the ride remains
eligible for download. An unreferenced temporary file is never an archive.

These guarantees depend on the filesystem and storage device honoring the requested barriers.

## Client sync and deletion

For device sources, sync compares the entire source tuple with the current complete archive.
A different revision or different verified content remains eligible for download. Old download
markers and missing archives cannot suppress it. Explicit phone deletion and trash records can
exclude a ride from a download, but they cannot supply an archive source or receipt.

The downloaded flag is part of the archive manifest. Sync does not write a second success marker.
An in-memory preview can record a completed local sync, but it returns no durable receipt.
The iOS coordinator sends each new durable receipt through ARCHIVE_RIDE. Before deciding that no
new download is needed, it also revalidates matching existing archives and sends their receipts.
Reconnect runs this receipt reconciliation without downloading missing rides. Explicit sync can
download missing or replaced sources. Both paths use the same coordinator task and identity gate.

The transfer client serializes receipts with other object operations. It checks cancellation after
queue admission and the exact StoreId before sending. Only a matching opcode and RequestId can
confirm proof; timestamp zero is a successful confirmation. A real link loss permits one restore
and exact-source retry with a new RequestId and renewed StoreId check. STATUS is not proof.
A receipt response has a ten-second deadline. Expiry cancels its parked receive and leaves
confirmation pending; it does not assume that the write failed or immediately retry the request.

A failed receipt keeps the phone archive and earlier batch successes. The existing sync banner
and Resume action expose pending confirmation, unsupported devices and device refusals. Resume
or a later reconnect revalidates the archives and retries without downloading them again. A
changed or absent source is terminal for that receipt; a fresh catalog decides any later download.
There is no persistent receipt queue or local device-acknowledgment mirror. Sync counts describe
local saves, never device proof. The device synced indicator uses only validated durable proof.

## Device archive proof

The wire rules are [`ARCHIVE_RIDE`](FLAT_Store_Protocol.md#312-archive_ride); the stored rows and
the synced indicator are [card archive metadata](Ride_Archive_Metadata.md). The device accepts proof
only for the exact current finalized Ride source on the mounted card, and a source that has since
changed or vanished is terminal for that receipt: the device never recreates the ride or its proof.
