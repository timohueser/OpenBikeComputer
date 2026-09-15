# OBC protocol — iOS implementation notes

The normative wire contract is
[`../specs/obc-ble-interface-spec.md`](../specs/obc-ble-interface-spec.md). It owns every UUID,
byte layout, state transition and security rule. If these notes disagree with it, the spec wins.
Shared fixtures under `specs/vectors/` pin the Swift and Rust codecs byte-for-byte.

This file records only the choices an iOS contributor needs while navigating the implementation.

## Versioning and store identity

The current protocol version is `4`. `protocolVersion` is exactly one little-endian `u16`. The
client issues `LIST` before its first object operation and takes the exact 128-bit `StoreId` from
that response. Durable phone state is scoped to `(device serial, StoreId)`, so a reset or card swap
cannot alias an old object id. A version mismatch is surfaced, never decoded optimistically.

## Transport

### Control plane

The app discovers DIS (`0x180A`), BAS (`0x180F`), OBC Control (`3C920000-…`).
The DIS Firmware Revision String is either an installed OBCU version or a bare Git hash. Hashes are
not parsed as release versions and therefore never produce an automatic update offer.

### Data plane

Bulk objects move over one encrypted L2CAP CoC. `L2CAPByteChannel` owns bytes and `BLEChannel`
preserves v4 stream-record boundaries. `TransferClient` owns request ids, announce/stream/result
lifetimes, and recovery. `BLETransport` retains only live-link records and radio facts.

### Bulk transfers

One `objectControl` Write Request announces a v4 operation. `PUT` and `GET` stream framed records on
the live CoC under that request id, then receive exactly one indicated result. Transfers never
resume. After a broken link the client opens a fresh link, repeats `LIST`, and reconciles named
mutations with `STATUS`; a lost create uses its catalog fingerprint as the v4 contract requires.
The checked-in `specs/vectors/flat-store-v4/` bytes are the codec oracle.

## Object formats

- Routes are OBCR v3 files. GPX/TCX conversion happens on the phone and the device stores the OBCR
  bytes verbatim.
- Ride bytes contain recorded samples and an OBRF footer. The phone decodes them to `Ride` before
  archiving or exporting GPX.
- Route, ride and trip catalogs are v4 `LIST` entries.
- Trips contain route object ids, not route bytes. Upload stages first and the trip last; deleting a
  trip does not implicitly delete its routes.
- Firmware images are signed OBCU containers carried as update objects.

The wire codecs live under `OBCTransport/Codecs/`; interchange-file parsing lives in `OBCFormats`.

## Ride archives

The catalog and download path preserve StoreId, ObjectId, served Revision, verified length and CRC.
The archive publishes canonical summary, samples, source and local sync state in one transaction.
File and directory persistence barriers must succeed before a local archive receipt is returned.
A changed revision or missing archive remains eligible for download. Phone deletion records are
separate from archive proof. The coordinator sends ARCHIVE_RIDE after a durable commit and for
revalidated matching archives, including when no download is needed. Reconnect reconciles receipts
only; manual sync can download missing rides. An in-memory save cannot produce a receipt.

The existing transfer FIFO and request correlation own this exchange. A real link loss permits one
restore and exact-source retry. A ten-second receipt-response deadline cancels its parked receive.
A timeout or failed receipt leaves the local archive intact and device confirmation pending in the
existing sync banner. Resume or reconnect revalidates and retries without downloading saved rides.
Unsupported and refused receipts remain visible; a changed source is terminal for that receipt.
Only the matching ARCHIVE_RIDE response confirms proof. STATUS and local save counts cannot do so.
Timestamp zero is valid. The device uses archive proof for the synced indicator.
Rides remain on the device until the user deletes them. See the [archive contract](../specs/Ride_Archive_Contract.md).

## Delta 1 — device name lives in Config

Renaming is a `Config` object write; there is no rename command. The UTF-8 name is capped at 48 bytes
and truncated only at a Character boundary.

## Delta 2 — GPX and TCX import

The phone accepts GPX and TCX, decodes either into `ImportedRoute`, and encodes OBCR before upload.
The device never parses XML. `RouteSource` and the import UI expose the same two-format boundary.

## Swift type map

| Type | Contract role |
| --- | --- |
| `OBCProtocol` | pinned version and feature-bit constants |
| `DeviceInfo` | DIS plus the v4 StoreId learned through `LIST` |
| `DeviceConfig` | append-only Config blob, including name and raw refresh byte |
| `RouteBlob` / `RouteDetail` | opaque OBCR upload and decoded route-object detail |
| `Ride` / `RidePoint` | canonical decoded ride and export input |
| `ImportedRoute` / `RoutePoint` | canonical GPX/TCX import model |
| `TransferProgress` / `TransferOutcome` | whole-object transfer lifecycle |
| `DeviceError` | typed protocol, radio, CRC and storage failures |
| `OBCUHeader` / `StagedFirmware` | validated firmware-update container |

`OBCDomain` contains transport-free values. `OBCTransport` contains the interface and codecs;
`OBCTransport/BLE` is the real radio implementation. Tests should normally exercise codecs and
semantic behavior without hardware; protocol-v4 vector and board-composition suites cover the
transport contract until a v4 on-device soak harness is introduced.
