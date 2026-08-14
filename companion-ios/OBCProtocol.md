# OBC protocol — iOS implementation notes

The normative wire contract is
[`../specs/obc-ble-interface-spec.md`](../specs/obc-ble-interface-spec.md). It owns every UUID,
byte layout, state transition and security rule. If these notes disagree with it, the spec wins.
Shared fixtures under `specs/vectors/` pin the Swift and Rust codecs byte-for-byte.

This file records only the choices an iOS contributor needs while navigating the implementation.

## Versioning and store epoch

The current protocol version is `2`. The `protocolVersion` characteristic is append-only and is
decoded by length:

| Bytes | Fields |
| ---: | --- |
| 2 | `version u16` only: no mounted store |
| 6 | plus `store_epoch u32` |
| 7 | plus `obcm_version u8` |
| 11 | plus `feature_bits u32` |

Missing fields remain `nil`; they are never fabricated as zero. Unknown trailing bytes and feature
bits are ignored. A partial `u32` capability word is absent, not a smaller word.

The store epoch names the card's object-id era. Durable phone state is scoped to `(device serial,
store epoch)` so a reset, card swap or recycled `u16` id cannot alias old state. A short identity read
does not provide an epoch and therefore gates off acknowledgements and reconciliation. A protocol
version mismatch is surfaced as `DeviceError.protocolMismatch`, never decoded optimistically.

Feature bit 0 announces the complete Weather Request contract. `obcm_version` is the map-file
version the running reader accepts and is independent of the protocol and firmware versions.

## Transport

### Control plane

The app discovers DIS (`0x180A`), BAS (`0x180F`), OBC Control (`3C920000-…`) and Weather Request
(`B3B60000-…`). `GATT.swift` owns UUIDs; typed codecs under `OBCTransport/Codecs/` own payloads.
The DIS Firmware Revision String is either an installed OBCU version or a bare Git hash. Hashes are
not parsed as release versions and therefore never produce an automatic update offer.

### Data plane

Bulk objects move over one encrypted L2CAP CoC. `L2CAPByteChannel` owns the stream,
`BLEChannel` owns whole-object transfer orchestration, and semantic features use capability-sized
protocols from `OBCTransport`. The app never exposes CoreBluetooth types above `OBCTransport/BLE/`;
the composition root alone chooses the concrete `DeviceTransport` aggregate.

### Bulk transfers

Transfers restart from byte zero; they do not resume at offsets. The app writes a descriptor,
streams exactly the declared length, verifies CRC-32/IEEE and waits for the typed terminal status.
Downloads are announced through `status`. A disconnect leaves the operation unresolved so policy
above the byte plane can retry the whole object. The complete descriptor, status and command layouts
are spec §§4–6.

## Object formats

- Routes are OBCR v3 files. GPX/TCX conversion happens on the phone and the device stores the OBCR
  bytes verbatim.
- Rides use the spec §7.2 ride object. The phone decodes to `Ride` before exporting GPX.
- Route, ride and trip lists use the v2 six-byte list header. Decoders honor `entry_len`, skip future
  tails and surface truncation when `total > count`.
- Trips contain route object ids, not route bytes. Upload stages first and the trip last; deleting a
  trip does not implicitly delete its routes.
- Firmware images are signed OBCU containers carried as `fwImage`.
- Weather bundles are OBCW objects carried as `weatherBundle` with object id zero.

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
| `DeviceInfo` | DIS plus store epoch, OBCM version and feature bits |
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
semantic behavior without hardware, with EchoHarness reserved for on-device transport checks.
