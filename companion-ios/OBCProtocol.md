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

The app discovers DIS (`0x180A`), BAS (`0x180F`), OBC Control (`3C920000-…`) and Weather Request
(`B3B60000-…`). `GATT.swift` owns UUIDs; `OBCProtocolV4` owns control and stream records.
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
- Ride bytes still use the previous decoder behind the v4 GET path. FS8 will replace it after the
  footer layout is fixed; the phone continues to decode to `Ride` before exporting GPX.
- Route, ride and trip catalogs are v4 `LIST` entries.
- Trips contain route object ids, not route bytes. Upload stages first and the trip last; deleting a
  trip does not implicitly delete its routes.
- Firmware images are signed OBCU containers carried as update objects.
- Weather bundles are OBCW objects. A create requests object id zero; the store assigns the id.

The wire codecs live under `OBCTransport/Codecs/`; interchange-file parsing lives in `OBCFormats`.

## Weather Request

The device advertises the secondary Weather Request service while a request is pending. iOS wakes,
connects to the known bonded peripheral, performs one authenticated 52-byte context read, then
disconnects before HTTP work. The resulting OBCW bundle returns later through the ordinary object
upload path.

Optional context groups are controlled by validity bits, never coordinate or timestamp sentinels.
Unknown reason/validity bits and an unknown refresh byte are tolerated on reads. The request id
correlates work but is not authorization; the device may accept a valid newer bundle raised by an
older request.

One CoreBluetooth manager arbitrates foreground and weather intents. Foreground work wins. A
weather operation may reuse but must not tear down a foreground connection. The standing watch is
rider-controlled and persisted; the bounded read and upload legs use absolute deadlines.

## Delta 1 — device name lives in Config

Renaming is a `Config` object write; there is no rename command. The UTF-8 name is capped at 48 bytes
and truncated only at a Character boundary.

`weather_refresh` is an optional trailing raw byte. On read, an absent value means the device default;
on write, absence means preserve the stored choice. Unknown values survive a round trip and are not
written as a guessed interval. The companion reports this device-owned setting but does not edit it.

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
| `WeatherRequestContext` | authenticated §11 request read |

`OBCDomain` contains transport-free values. `OBCTransport` contains the interface and codecs;
`OBCTransport/BLE` is the real radio implementation. Tests should normally exercise codecs and
semantic behavior without hardware; protocol-v4 vector and board-composition suites cover the
transport contract until a v4 on-device soak harness is introduced.
