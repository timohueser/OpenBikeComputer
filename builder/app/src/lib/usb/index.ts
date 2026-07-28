/**
 * The USB stack, in one import (C3, issue #902).
 *
 * Four layers:
 *
 * ```text
 *   session.svelte.ts   reactive shell — what a Svelte component holds
 *   client.ts           the object model: identity, lists, transfers, commands
 *   transport.ts        the one byte USB adds: which control characteristic a frame is
 *   pipe.ts             two byte pipes — WebUSB, nusb (`../desktop/usb.ts`) or loopback underneath
 * ```
 *
 * The bottom layer really is swappable in isolation — that is what `BytePipe` is for, and the
 * loopback and WebUSB implementations prove it. `transport.ts` is a weaker seam: it owns the frame
 * *encoding* alone, but the assumption that control messages carry a leading selector is shared with
 * `client.ts`'s dispatch. Its header says exactly what moves if #889 ratifies something else.
 *
 * `loopback.ts` is deliberately **not** re-exported here: it is a full simulated device, and the
 * hosted bundle has no business carrying one. Tests and dev harnesses import it by path, which also
 * keeps `platform/bundle.test.ts` honest about what ships.
 */

export { Crc32 } from "./crc32";
export {
    DEFAULT_CHUNK_SIZE,
    DEFAULT_TIMEOUT_MS,
    DeviceError,
    ProtocolClient,
    blobSource,
    bytesSource,
    type ClientOptions,
    type DeviceErrorCode,
    type ObjectSource,
    type SendHooks,
    type TransferOptions,
    type UploadResult,
} from "./client";
export {
    ROUTE_ENTRY_LEN,
    RIDE_ENTRY_LEN,
    TRIP_ENTRY_LEN,
    decodeRideList,
    decodeRideObject,
    decodeRouteList,
    decodeTripList,
    decodeTripObject,
    encodeTripObject,
    type RideListEntry,
    type RideObject,
    type RidePoint,
    type RouteListEntry,
    type TripListEntry,
    type TripObject,
} from "./objects";
export { PipeError, type BytePipe, type DeviceLink, type PipeErrorCode } from "./pipe";
export {
    Command,
    CommandStatus,
    DecodeError,
    MAX_RETENTION,
    NEW_OBJECT_ID,
    ObjectType,
    Op,
    PROTOCOL_VERSION,
    SINGLETON_OBJECT_ID,
    TransferStatus,
    type DeviceConfig,
    type VersionRead,
} from "./protocol";
export type { DeviceSession, DeviceState, DeviceStatus, DeviceWatcher } from "./session";
export type { DeviceInfo } from "./transport";
export { OBC_USB_FILTERS, WebUsbWatcher, webUsb, type WatcherOptions } from "./webusb";
