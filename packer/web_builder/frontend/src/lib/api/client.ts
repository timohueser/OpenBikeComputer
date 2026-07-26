// The dev host's transport: HTTP against `python -m packer.web_builder`. Only
// platform/dev.ts imports it, so it never reaches the other two bundles.

import type { BuildRequest, Palette, RegionFeature } from "../platform/types";
import type { Preset, SchemaEnvelope } from "../config/model";

// Every fetch goes through API_BASE so a deployment can relocate the API with
// a build-time env var (VITE_API_BASE) instead of code changes.
export const API_BASE: string = import.meta.env.VITE_API_BASE ?? "/api";

// The region shape is declared at the platform seam now (every host serves
// regions), but it is still this client's `/regions` response type, so it stays
// re-exported here alongside the other wire shapes below.
export type { RegionFeature };

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

/** The POST /jobs wire body — snake_case, and this module's business alone. */
export interface JobRequest {
    region_ids: string[];
    config: unknown;
    chunk_size?: number;
    output_name: string;
    bbox?: [number, number, number, number];
}

export interface JobSnapshot {
    id: string;
    state: "queued" | "running" | "done" | "error";
    created_at: number;
    output: string;
    size?: number;
    download_url?: string;
    error?: string;
}

export const api = {
    regions: () =>
        getJson<{ features: RegionFeature[] }>("/regions").then((fc) => fc.features),
    presets: () => getJson<Preset[]>("/presets"),
    schema: () => getJson<SchemaEnvelope>("/schema"),
    palette: () => getJson<Palette>("/palette"),
    job: (id: string) => getJson<JobSnapshot>(`/jobs/${id}`),

    /** 404 (no user_config.json on the server) is the common case, not an error. */
    async legacyConfig(): Promise<Record<string, unknown> | null> {
        const res = await fetch(`${API_BASE}/config/legacy`);
        return res.ok ? ((await res.json()) as Record<string, unknown>) : null;
    },

    async startJob(req: BuildRequest): Promise<string> {
        const body: JobRequest = {
            region_ids: req.regionIds,
            config: req.config,
            ...(req.chunkSize != null ? { chunk_size: req.chunkSize } : {}),
            output_name: req.outputName,
            ...(req.bbox ? { bbox: req.bbox } : {}),
        };
        const res = await fetch(API_BASE + "/jobs", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(body),
        });
        if (!res.ok) {
            let detail = res.statusText;
            try {
                detail = (await res.json()).detail ?? detail;
            } catch {
                // non-JSON error body; keep statusText
            }
            throw new Error(detail);
        }
        return (await res.json()).job_id as string;
    },
};
