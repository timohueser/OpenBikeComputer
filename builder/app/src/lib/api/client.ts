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
    async publishedCatalog(): Promise<{ url: string; body: string }> {
        const res = await fetch(`${API_BASE}/catalog/root`);
        if (!res.ok) throw new Error(await errorDetail(res));
        const url = res.headers.get("X-OBC-Catalog-Url");
        if (!url) throw new Error("The maintainer host omitted the catalog URL.");
        return { url, body: await res.text() };
    },

    async catalogFetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
        const url = input instanceof Request ? input.url : String(input);
        return fetch(`${API_BASE}/catalog/object?url=${encodeURIComponent(url)}`, init);
    },
    previewStatus: () => getJson<SchemaPreviewStatus>("/schema-preview/status"),

    async packPreview(config: Record<string, unknown>, signal: AbortSignal): Promise<SchemaPreviewMap> {
        const res = await fetch(`${API_BASE}/schema-preview`, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(config),
            signal,
        });
        if (!res.ok) throw new Error(await errorDetail(res));
        let diagnostics: string[] = [];
        const encoded = res.headers.get("X-OBC-Pack-Diagnostics");
        if (encoded) {
            try {
                const parsed: unknown = JSON.parse(atob(encoded.replaceAll("-", "+").replaceAll("_", "/")));
                if (Array.isArray(parsed) && parsed.every((line) => typeof line === "string")) diagnostics = parsed;
            } catch {
                // Diagnostics are advisory; never reject a valid map for a bad header.
            }
        }
        return {
            bytes: new Uint8Array(await res.arrayBuffer()),
            packDurationMs: Number(res.headers.get("X-OBC-Pack-Duration-Ms") ?? 0),
            diagnostics,
        };
    },

    /** 404 (no user_config.json on the server) is the common case, not an error. */
    async legacyConfig(): Promise<Record<string, unknown> | null> {
        const res = await fetch(`${API_BASE}/config/legacy`);
        return res.ok ? ((await res.json()) as Record<string, unknown>) : null;
    },
};
