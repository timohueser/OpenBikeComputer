# OBC protocol — iOS implementation notes

The normative wire contract is
[`../specs/obc-ble-interface-spec.md`](../specs/obc-ble-interface-spec.md). It owns every UUID,
byte layout, state transition and security rule, and it wins over anything here. The ride archive
rules are in [`../specs/Ride_Archive_Contract.md`](../specs/Ride_Archive_Contract.md). Shared
fixtures under `specs/vectors/` pin the Swift and Rust codecs byte for byte.

This file records only the choices an iOS contributor needs while reading the implementation.

## Versioning and store identity

Protocol version `4`. `protocolVersion` is exactly one little-endian `u16`. The client issues
`LIST` before its first object operation and takes the exact 128-bit `StoreId` from that response.
Durable phone state is scoped to `(device serial, StoreId)`, so a reset or a card swap cannot
alias an old object id. **A version mismatch is surfaced, never decoded optimistically.**

## Transport

The app discovers DIS (`0x180A`), BAS (`0x180F`) and OBC Control (`3C920000-…`). The DIS Firmware
Revision String is either an installed OBCU version or a bare Git hash; **a hash is never parsed
as a release version**, so it never produces an automatic update offer.

Bulk objects move over one encrypted L2CAP CoC. `L2CAPByteChannel` owns bytes, `BLEChannel`
preserves v4 stream-record boundaries, `TransferClient` owns request ids and the
announce/stream/result lifetime, and `BLETransport` keeps only live-link records and radio facts.

One `objectControl` Write Request announces an operation. `PUT` and `GET` stream framed records on
the live CoC under that request id, then receive exactly one indicated result. **Transfers never
resume.** After a broken link the client opens a fresh link, repeats `LIST`, and reconciles named
mutations with `STATUS`; a lost create uses its catalog fingerprint. The checked-in
`specs/vectors/flat-store-v4/` bytes are the codec oracle.

## Object formats

- Routes are OBCR v3 files. GPX and TCX conversion happens on the phone, and the device stores the
  OBCR bytes verbatim. The device never parses XML.
- Ride bytes are recorded samples plus an OBRF footer. The phone decodes them to `Ride` before
  archiving or exporting GPX.
- Route, ride and trip catalogs are v4 `LIST` entries. Catalog flag bit 3 (`assistantAccepted`)
  marks an accepted Assistant route; the decoder accepts it and rejects undefined flag bits.
- Trips hold route object ids, not route bytes. Upload the stages first and the trip last.
  Deleting a trip does not implicitly delete its routes.
- Firmware images are signed OBCU containers carried as update objects.

The wire codecs are under `OBCTransport/Codecs/`; interchange-file parsing is in `OBCFormats`.

## Ride archives

The catalog and download path preserve StoreId, ObjectId, the served Revision, the verified length
and the CRC. The archive publishes the canonical summary, samples, source and local sync state in
one transaction, and **file and directory persistence barriers must succeed before a local archive
receipt is returned**. An in-memory save cannot produce a receipt.

The coordinator sends ARCHIVE_RIDE after a durable commit, and for a revalidated matching archive
even when no download is needed. **Only the matching ARCHIVE_RIDE response confirms proof**;
STATUS and local save counts cannot. The device uses archive proof for the synced indicator, and
rides stay on the device until the user deletes them.

A ten-second receipt-response deadline cancels the parked receive. A timeout or a failed receipt
leaves the local archive intact and the device confirmation pending in the sync banner; resume or
reconnect revalidates and retries without downloading saved rides. A changed source is terminal
for that receipt.

## Two deltas from the spec

- **The device name lives in Config.** Renaming is a `Config` object write; there is no rename
  command. The UTF-8 name is capped at 48 bytes and truncated only at a Character boundary.
- **The phone accepts GPX and TCX.** Either decodes into `ImportedRoute`, which is encoded as OBCR
  before upload. `RouteSource` and the import UI expose the same two-format boundary.

## Swift type map

| Type | Contract role |
| --- | --- |
| `OBCProtocol` | Pinned version and feature-bit constants |
| `DeviceInfo` | DIS plus the v4 StoreId learned through `LIST` |
| `DeviceConfig` | Append-only Config blob, including name and raw refresh byte |
| `RouteBlob` / `RouteDetail` | Opaque OBCR upload and decoded route-object detail |
| `Ride` / `RidePoint` | Canonical decoded ride and export input |
| `ImportedRoute` / `RoutePoint` | Canonical GPX and TCX import model |
| `TransferProgress` / `TransferOutcome` | Whole-object transfer lifecycle |
| `DeviceError` | Typed protocol, radio, CRC and storage failures |
| `OBCUHeader` / `StagedFirmware` | Validated firmware-update container |

`OBCDomain` holds transport-free values. `OBCTransport` holds the interface and the codecs, and
`OBCTransport/BLE` is the real radio. Tests should normally exercise codecs and semantic behavior
without hardware.
