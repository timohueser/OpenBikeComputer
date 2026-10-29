# OBC protocol — iOS implementation notes

The normative wire contract is
[`../specs/ble-interface.md`](../specs/ble-interface.md). It owns every UUID,
byte layout, state transition and security rule, and it wins over anything here. The ride archive
rules are in [`../specs/ride-archive-receipts.md`](../specs/ride-archive-receipts.md). Shared
fixtures under `specs/vectors/` pin the codecs byte for byte.

This file records only the choices an iOS contributor needs while reading the implementation.

## Versioning and store identity

The client checks `protocolVersion` (`ble-interface.md` §3.3). It issues `LIST` before its first
object operation and takes the exact 128-bit `StoreId` from that response. Durable phone state is
scoped to `(device serial, StoreId)`, so a reset or a card swap cannot alias an old object id.
**A version mismatch is surfaced, never decoded optimistically.**

## Transport

The app discovers DIS, BAS and OBC Control (`ble-interface.md` §3). It follows the §3.1
firmware-revision dialect: **a bare Git hash is never parsed as a release version**, so it never
produces an automatic update offer.

Bulk objects move over one encrypted L2CAP CoC. `L2CAPByteChannel` owns bytes, `BLEChannel`
preserves v4 stream-record boundaries, `TransferClient` owns request ids and the
announce/stream/result lifetime, and `BLETransport` keeps only live-link records and radio facts.

One `objectControl` Write Request announces an operation. `PUT` and `GET` stream framed records on
the live CoC under that request id, then receive exactly one indicated result. **Transfers never
resume.** After a broken link the client opens a fresh link, repeats `LIST`, and reconciles named
mutations with `STATUS`; a lost create uses its catalog fingerprint. The Rust store client
(`obc_client_*` through `OBCHost`) builds and decodes the v4 frames, and `firmware/obc-link` pins
them to the checked-in `specs/vectors/flat-store-v4/` bytes. Swift checks only the framing of each
request and stream record at the link.

## Object formats

- Routes are OBCR v5 files. The device stores the OBCR bytes verbatim and never parses XML.
- Ride bytes are recorded samples plus an OBRF footer. The phone decodes them to `Ride` before
  archiving or exporting GPX.
- Route, ride and trip catalogs are v4 `LIST` entries. Catalog flag bit 3 (`assistantAccepted`)
  marks an accepted Assistant route; the decoder accepts it and rejects undefined flag bits.
- Trips reference route objects (`ble-interface.md` §7.7). Upload the stages first and the trip
  last. Deleting a trip does not implicitly delete its routes.
- Firmware images are signed OBCU containers carried as update objects.

The wire codecs are under `OBCTransport/Codecs/`; interchange-file parsing is in `OBCFormats`.

## Ride archives

`ride-archive-receipts.md` owns the archive commit, the receipt, its deadline and the retry rules.
The coordinator sends ARCHIVE_RIDE; **only the matching ARCHIVE_RIDE response confirms proof**.
Rides stay on the device until the user deletes them.

## Names and imports

- **The device name lives in Config.** Renaming is a `Config` object write; there is no rename
  command (`ble-interface.md` §7.3). The app truncates the UTF-8 name to the 48-byte cap only at a
  Character boundary.
- **The phone accepts GPX and TCX.** Either decodes into `ImportedRoute`, which is encoded as OBCR
  before upload. `RouteSource` and the import UI expose the same two-format boundary.

## Swift type map

| Type | Contract role |
| --- | --- |
| `OBCProtocol` | Pinned version and feature-bit constants |
| `DeviceInfo` | DIS plus the v4 StoreId learned through `LIST` |
| `DeviceConfig` | Append-only Config blob: device name and units |
| `RouteBlob` / `RouteDetail` | Opaque OBCR upload and decoded route-object detail |
| `Ride` / `RidePoint` | Canonical decoded ride and export input |
| `ImportedRoute` / `RoutePoint` | Canonical GPX and TCX import model |
| `TransferProgress` / `TransferOutcome` | Whole-object transfer lifecycle |
| `DeviceError` | Typed protocol, radio, CRC and storage failures |
| `OBCUHeader` / `StagedFirmware` | Validated firmware-update container |

`OBCDomain` holds transport-free values. `OBCTransport` holds the interface and the codecs, and
`OBCTransport/BLE` is the real radio. Tests should normally exercise codecs and semantic behavior
without hardware.
