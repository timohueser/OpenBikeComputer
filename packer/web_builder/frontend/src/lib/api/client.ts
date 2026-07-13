import { API_BASE } from "../constants";
import type { Preset, SchemaEnvelope } from "../config/model";

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

export interface RegionFeature {
    type: "Feature";
    properties: { id: string; name: string; parent: string | null; has_children: boolean };
    geometry: { type: "Polygon" | "MultiPolygon"; coordinates: unknown };
}

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
    palette: () => getJson<{ columns: number; colors: string[] }>("/palette"),
    job: (id: string) => getJson<JobSnapshot>(`/jobs/${id}`),

    async startJob(req: JobRequest): Promise<string> {
        const res = await fetch(API_BASE + "/jobs", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(req),
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
