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

export interface StoragePlace {
    readonly id: string;
    readonly label: string;
    readonly note: string;
    readonly path: string;
    readonly bytes: number;
    readonly files: number;
}

export interface DiskStorage {
    places(): Promise<StoragePlace[]>;
}

/** Native grouped output; browser hosts use their downloader instead. */
export interface MapOutputSession {
    readonly path: string;
    write(name: string, bytes: Uint8Array): Promise<string>;
    finish(): Promise<void>;
}

export type StyleEditorModule = { default: Component<Record<string, never>> };
export type LoadStyleEditor = () => Promise<StyleEditorModule>;

/**
 * Host services used by the shared app. Build-time aliases select one platform,
 * so absent hosts do not enter the bundle. `null` means unavailable by design.
 */
export interface Platform {
    readonly name: PlatformName;
    readonly caps: Caps;

    /** Whether `device()` uses browser WebUSB rather than a native driver. */
    readonly usbViaWebUsb: boolean;

    presets(): Promise<Preset[]>;
    /** Resolved catalog URL and its raw root document. */
    catalog(): Promise<{ url: string; body: string }>;
    readonly catalogFetch: typeof fetch;
    readonly openMapOutput: ((name: string) => Promise<MapOutputSession>) | null;

    readonly device: (() => Promise<DeviceSession>) | null;
    readonly rides: (() => Promise<RideLibrary>) | null;

    readonly schema: (() => Promise<SchemaEnvelope>) | null;
    readonly palette: (() => Promise<Palette>) | null;
    readonly schemaPreview: SchemaPreviewService | null;

    readonly storage?: DiskStorage;
    readonly revealFile?: (path: string) => Promise<void>;
    readonly siteNav?: { readonly docs: string; readonly simulator: string; readonly github: string };
}
