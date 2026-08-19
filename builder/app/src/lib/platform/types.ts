import type { Component } from "svelte";
import type { Preset, SchemaEnvelope } from "../config/model";
import type { RideLibrary } from "../device/library";
import type { DeviceSession } from "../usb/session";

export type PlatformName = "web" | "desktop" | "dev";

export interface Palette {
    columns: number;
    colors: string[];
}

export interface SchemaPreviewStatus {
    available: boolean;
    label: string;
    configured: boolean;
    detail: string;
    bbox: string;
}

export interface SchemaPreviewMap {
    bytes: Uint8Array;
    packDurationMs: number;
    diagnostics: string[];
}

export interface SchemaPreviewService {
    status(): Promise<SchemaPreviewStatus>;
    pack(config: Record<string, unknown>, signal: AbortSignal): Promise<SchemaPreviewMap>;
}

/** Capability flags are the only supported way for the UI to gate host features. */
export interface Caps {
    readonly rideLibrary: boolean;
    readonly deviceUsb: boolean;
    readonly deviceDashboard: boolean;
}

export type { DeviceSession, RideLibrary };

/**
 * A place on disk to put the assembled map — a native folder on the desktop, a
 * picked directory on a browser that has `showDirectoryPicker`. Hosts without
 * either export `openMapOutput: null` and the UI falls back to its downloader.
 *
 * **Why this exists at all, now that a map is one file.** A browser can save one
 * file without any of this: the assembled `.obcm` is an OPFS-backed `Blob`, so the
 * plain downloader streams it to disk without the tab's heap ever holding it. What
 * the downloader cannot do is choose *where*. A country-scale map is ~9 GiB, and
 * "save to Downloads, then copy it to the card" means writing those 9 GiB twice and
 * needing the room for both at once. A picked directory is the card itself, and the
 * write happens once. That is the whole argument, and at this size it is decisive.
 *
 * The session is opened once and written once — a map is one file — but `write`
 * still names it, because the destination is a *folder* and the file needs a name in
 * it. The name comes from the caller: the assembler names nothing.
 *
 * `openMapOutput` must be called **under the user gesture that starts the run**: the
 * browser implementation opens a directory picker, and a picker not backed by a
 * fresh activation is refused by the browser. A dismissed picker rejects with an
 * `AbortError`, which callers treat as "changed my mind", not as a failure.
 */
export interface MapOutputSession {
    readonly path: string;
    /** Accepts a `Blob` so the OPFS-backed map can stream to disk without entering
     *  the tab's heap; hosts that need contiguous bytes (the desktop IPC) do their
     *  own conversion — the residency is theirs to own. */
    write(name: string, body: Uint8Array | Blob): Promise<string>;
    finish(): Promise<void>;
    discard(): Promise<void>;
}

export type StyleEditorModule = { default: Component<Record<string, never>> };

export interface StyleEditorService {
    load(): Promise<StyleEditorModule>;
    presets(): Promise<Preset[]>;
    schema(): Promise<SchemaEnvelope>;
    palette(): Promise<Palette>;
    readonly preview: SchemaPreviewService;
}

/**
 * Host services used by the shared app. Build-time aliases select one platform,
 * so absent hosts do not enter the bundle. `null` means unavailable by design.
 */
export interface Platform {
    readonly name: PlatformName;
    readonly caps: Caps;

    /** Whether `device()` uses browser WebUSB rather than a native driver. */
    readonly usbViaWebUsb: boolean;

    /** Resolved catalog URL and its raw root document. */
    catalog(): Promise<{ url: string; body: string }>;
    readonly catalogFetch: typeof fetch;
    readonly openMapOutput: ((name: string) => Promise<MapOutputSession>) | null;

    readonly device: (() => Promise<DeviceSession>) | null;
    readonly rides: (() => Promise<RideLibrary>) | null;

    readonly styleEditor: StyleEditorService | null;

    readonly siteNav?: { readonly docs: string; readonly simulator: string; readonly github: string };
}
