// The dev host's transport: HTTP against `python -m builder.server`. Only
// platform/dev.ts imports it, so it never reaches the other two bundles.

import type { Palette } from "../platform/types";
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

export const api = {
    presets: () => getJson<Preset[]>("/presets"),
    schema: () => getJson<SchemaEnvelope>("/schema"),
    palette: () => getJson<Palette>("/palette"),

    /** 404 (no user_config.json on the server) is the common case, not an error. */
    async legacyConfig(): Promise<Record<string, unknown> | null> {
        const res = await fetch(`${API_BASE}/config/legacy`);
        return res.ok ? ((await res.json()) as Record<string, unknown>) : null;
    },
};
