// The desktop host's transport: Tauri commands. Only platform/desktop.ts and
// the desktop build tracker import it, so it never reaches the other two
// bundles — the same containment `api/client.ts` has on the dev side.
//
// Every function here is one `invoke()`. The names are the Rust command names in
// firmware/obc-desktop/src/main.rs, and the argument shapes are what serde
// deserializes there; that is the whole contract, and it is worth keeping in one
// file so a rename is one place on each side.

import { invoke, type Channel } from "@tauri-apps/api/core";
import type { Palette, RegionFeature, StoragePlace } from "../platform/types";
import type { Preset, SchemaEnvelope } from "../config/model";

/** The catalog manifest, plus the URL it came from — §2 resolves preview
 *  references against the manifest's own location. */
export interface FetchedCatalog {
    url: string;
    body: string;
}

/** `build_active`'s answer: the running (or most recent) build. */
export interface JobSnapshot {
    id: string;
    state: "running" | "done" | "error" | "cancelled";
}

/** What a build reports. Mirrors the dev server's SSE events, deliberately —
 *  see `lib/build/phases.ts`. */
export type BuildEvent =
    | { type: "status"; status: string; detail: string }
    | { type: "progress"; phase: string; region: string; pct: number }
    | { type: "log"; line: string }
    | { type: "done"; path: string; filename: string; size: number }
    | { type: "error"; message: string }
    | { type: "cancelled" };

// --- USB (D4 #909) ------------------------------------------------------------
//
// The Rust side moves bytes and nothing else; the protocol is C3's TS client, the
// same one the hosted site runs. See firmware/obc-desktop/src/usb/ for the plane
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
    regions: () => invoke<{ features: RegionFeature[] }>("regions").then((fc) => fc.features),
    presets: () => invoke<Preset[]>("presets"),
    schema: () => invoke<SchemaEnvelope>("schema"),
    palette: () => invoke<Palette>("palette"),
    catalog: () => invoke<FetchedCatalog>("catalog"),

    buildActive: () => invoke<JobSnapshot | null>("build_active"),
    buildCancel: (id: string) => invoke<boolean>("build_cancel", { id }),

    storagePlaces: () => invoke<StoragePlace[]>("storage_info"),
    storageClear: (id: string) => invoke<number>("storage_clear", { id }),

    saveStyle: (name: string, body: string) => invoke<string>("save_style", { name, body }),
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
};
