// The dev host's transport: HTTP against `python -m builder.server`. Only
// platform/dev.ts imports it, so it never reaches the other two bundles.

import type { Palette, SchemaPreviewMap, SchemaPreviewStatus } from "../platform/types";
import type { Preset, SchemaEnvelope } from "../config/model";

// Every fetch goes through API_BASE so a deployment can relocate the API with
// a build-time env var (VITE_API_BASE) instead of code changes.
export const API_BASE: string = import.meta.env.VITE_API_BASE ?? "/api";

async function getJson<T>(path: string): Promise<T> {
    const res = await fetch(API_BASE + path);
    if (!res.ok) {
        let detail = res.statusText;
        try {
            detail = (await res.json()).detail ?? detail;
        } catch {
            // non-JSON error body; keep statusText
        }
        throw new Error(detail);
    }
    return res.json() as Promise<T>;
}

async function errorDetail(res: Response): Promise<string> {
    try {
        const body = (await res.json()) as { detail?: unknown };
        if (typeof body.detail === "string") return body.detail;
    } catch {
        // Keep the HTTP status below for a non-JSON failure.
    }
    return `${res.status} ${res.statusText}`;
}

export const api = {
    presets: () => getJson<Preset[]>("/presets"),
    schema: () => getJson<SchemaEnvelope>("/schema"),
    palette: () => getJson<Palette>("/palette"),
    runtime: () => getJson<{ catalog_url: string }>("/runtime"),
    previewStatus: () => getJson<SchemaPreviewStatus>("/schema-preview/status"),

    async packPreview(config: Record<string, unknown>, signal: AbortSignal): Promise<SchemaPreviewMap> {
        const res = await fetch(`${API_BASE}/schema-preview`, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(config),
            signal,
        });
        if (!res.ok) throw new Error(await errorDetail(res));
        return {
            bytes: new Uint8Array(await res.arrayBuffer()),
            packDurationMs: Number(res.headers.get("X-OBC-Pack-Duration-Ms") ?? 0),
        };
    },

    /** 404 (no user_config.json on the server) is the common case, not an error. */
    async legacyConfig(): Promise<Record<string, unknown> | null> {
        const res = await fetch(`${API_BASE}/config/legacy`);
        return res.ok ? ((await res.json()) as Record<string, unknown>) : null;
    },
};
