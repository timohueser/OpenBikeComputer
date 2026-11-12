import type { Component } from "svelte";
import type { Preset, SchemaEnvelope } from "../config/model";

export type PlatformName = "web" | "dev";

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

    /** Resolved catalog URL and its raw root document. */
    catalog(options?: { refresh?: boolean }): Promise<{ url: string; body: string }>;
    readonly catalogFetch: typeof fetch;

    readonly styleEditor: StyleEditorService | null;

    readonly siteNav?: { readonly home: string; readonly docs: string; readonly blog: string; readonly github: string };
}
