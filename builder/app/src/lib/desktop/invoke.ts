// The desktop host's transport: Tauri commands. Only the desktop platform and
// native device/library adapters import it, so it never reaches the other two
// bundles — the same containment `api/client.ts` has on the dev side.
//
// Every function here is one `invoke()`. The names are the Rust command names in
// apps/obc-desktop/src/main.rs, and the argument shapes are what serde
// deserializes there; that is the whole contract, and it is worth keeping in one
// file so a rename is one place on each side.

import { invoke, type Channel } from "@tauri-apps/api/core";
import type { StoragePlace } from "../platform/types";

/** The catalog manifest, plus the URL relative references resolve against. */
export interface FetchedCatalog {
    url: string;
    body: string;
}

export interface OpenedMapOutput {
    id: number;
    path: string;
}

// --- USB (D4 #909) ------------------------------------------------------------
//
// The Rust side moves bytes and nothing else; the protocol is C3's TS client, the
// same one the hosted site runs. See apps/obc-desktop/src/usb/ for the plane
// split and why the two binary calls below do not go through JSON.

/** Which endpoint pair a call means — `DeviceLink`'s two members, by name. */
export type UsbPlane = "control" | "bulk";
/** Which half of a pair to cancel. Omitted means both. */
export type UsbDir = "in" | "out";

/** A device the backend is willing to open. `id` is opaque: never parse it. */
export interface UsbDeviceSummary {
    id: string;
    vendorId: number;
    productId: number;
    product: string | null;
    serialNumber: string | null;
}

/** Hot-plug, as the backend reports it. `watchFailed` is not a device problem —
 *  it means the OS notification stream itself is gone, and a watch that died
 *  quietly is indistinguishable from "nothing is ever plugged in". */
export type UsbEvent =
    | { type: "connected"; device: UsbDeviceSummary }
    | { type: "disconnected"; id: string }
    | { type: "watchFailed"; message: string };

/** What `usb_open` hands back: the handle every later call carries, plus the two
 *  planes' packet sizes (512 on the LM20's high-speed core, by USB rule). */
export interface UsbLinkInfo {
    handle: number;
    deviceId: string;
    interfaceNumber: number;
    controlPacketSize: number;
    bulkPacketSize: number;
    product: string | null;
    serialNumber: string | null;
}

/** A transport failure in `PipeError`'s own vocabulary — `closed`, `aborted` or
 *  `device-error`. Tauri rejects with the serialized `Err` value, so this arrives
 *  as a plain object rather than an `Error`. */
export interface UsbFault {
    code: string;
    message: string;
}

/** A file, as a transfer descriptor needs it (§4.2): length and whole-object CRC. */
export interface UsbFileDigest {
    len: number;
    crc32: number;
}

/** How far a native file send has got. */
export interface UsbSendProgress {
    sent: number;
    total: number;
}

export const desktop = {
    catalog: () => invoke<FetchedCatalog>("catalog"),
    /** Raw response path: catalog cells can be hundreds of MB, so a JSON byte
     *  array is not an acceptable transport. Rust restricts this to the
     *  configured catalog origin. */
    catalogGet: (url: string) => invoke<ArrayBuffer>("catalog_get", { url }),
    mapOutputBegin: (name: string) => invoke<OpenedMapOutput>("map_output_begin", { name }),
    mapOutputWrite: (id: number, name: string, bytes: Uint8Array) =>
        invoke<string>("map_output_write", bytes, {
            headers: { "output-id": String(id), filename: name },
        }),
    mapOutputFinish: (id: number) => invoke<void>("map_output_finish", { id }),
    mapOutputDiscard: (id: number) => invoke<void>("map_output_discard", { id }),

    storagePlaces: () => invoke<StoragePlace[]>("storage_info"),

    revealFile: (path: string) => invoke<void>("reveal_file", { path }),

    usbWatch: (onEvent: Channel<UsbEvent>) => invoke<UsbDeviceSummary[]>("usb_watch", { onEvent }),
    usbList: () => invoke<UsbDeviceSummary[]>("usb_list"),
    usbOpen: (deviceId: string) => invoke<UsbLinkInfo>("usb_open", { deviceId }),
    usbClose: (handle: number) => invoke<void>("usb_close", { handle }),

    /** One transfer off a plane's IN endpoint. Resolves to an `ArrayBuffer`: the
     *  command returns `tauri::ipc::Response`, which is Tauri's raw path. */
    usbRead: (handle: number, plane: UsbPlane) => invoke<ArrayBuffer>("usb_read", { handle, plane }),

    /**
     * One transfer onto a plane's OUT endpoint.
     *
     * The bytes are the *whole* invoke body — that is what makes them raw rather
     * than a JSON array of numbers (roughly 4 bytes of text per byte) — so the
     * handle and the plane have nowhere to go but headers.
     */
    usbWrite: (handle: number, plane: UsbPlane, bytes: Uint8Array) =>
        invoke<void>("usb_write", bytes, { headers: { handle: String(handle), plane } }),

    usbCancel: (handle: number, plane: UsbPlane, dir?: UsbDir) =>
        invoke<void>("usb_cancel", { handle, plane, dir: dir ?? null }),
    usbReset: (handle: number, plane: UsbPlane) => invoke<void>("usb_reset", { handle, plane }),

    usbFileDigest: (path: string) => invoke<UsbFileDigest>("usb_file_digest", { path }),
    usbSendFile: (handle: number, path: string, onProgress: Channel<UsbSendProgress>) =>
        invoke<number>("usb_send_file", { handle, path, onProgress }),

    // --- the ride library (E2 #912) -----------------------------------------
    //
    // `ridesImport` is the one command in this file whose *timing* is part of a
    // contract: it resolves after the ride object, the GPX and the index have
    // each been fsynced, and `pullRides` sends `ackRides` only afterwards. See
    // apps/obc-desktop/src/rides.rs.

    ridesIndex: () => invoke<RideIndexView>("rides_index"),
    ridesImport: (request: RideImportRequest) => invoke<RideImported>("rides_import", { request }),
    ridesAckSet: (serial: string, epoch: number) => invoke<number[]>("rides_ack_set", { serial, epoch }),
    /** The stored ride object. Raw, like `usbRead` — a JSON number array would be 4× the bytes. */
    ridesRead: (key: string) => invoke<ArrayBuffer>("rides_read", { key }),
    ridesWriteGpx: (key: string, gpx: string) => invoke<string>("rides_write_gpx", { key, gpx }),
    /** Opens the OS folder chooser and moves the library. `null` when it was dismissed. */
    ridesChooseFolder: () => invoke<string | null>("rides_choose_folder"),
};

// --- the ride library's payloads ---------------------------------------------
//
// Field for field what `serde` reads and writes in apps/obc-desktop/src/rides.rs.
// `lib/device/library.ts` owns the app-facing shapes; these are the wire ones, and
// `lib/desktop/library.ts` is the (thin) translation between them.

export interface RideIndexEntry {
    key: string;
    serial: string;
    epoch: number;
    objectId: number;
    name: string;
    startTime: number;
    distanceM: number;
    movingTimeS: number;
    climbM: number;
    points: number;
    bytes: number;
    crc32: number;
    importedAt: number;
    /** Basenames — what the index stores, because the folder can move. */
    rideFile: string;
    gpxFile: string;
    track: Array<[number, number]>;
    /** The basenames joined to the current folder, by the side that owns a path separator. */
    ridePath: string;
    gpxPath: string;
    /** Recomputed against the filesystem on every read, never trusted from the index. */
    present: boolean;
    gpxPresent: boolean;
}

export interface RideIndexView {
    folder: string;
    isDefault: boolean;
    rides: RideIndexEntry[];
}

export interface RideImportRequest {
    serial: string;
    epoch: number;
    objectId: number;
    name: string;
    startTime: number;
    distanceM: number;
    movingTimeS: number;
    climbM: number;
    points: number;
    crc32: number;
    track: Array<[number, number]>;
    object: number[];
    gpx: string;
}

export interface RideImported {
    ride: RideIndexEntry;
    imported: boolean;
}
