// What `obc data sources --json` and `source prepare --dry-run --json` write.
import type { Holder } from "./shapes";

export type SourceState = "blocked" | "stale" | "ok" | "unused";

export interface SourceRow {
    id: string;
    kind: "data" | "asset" | "tool";
    group?: string;
    licence?: string;
    licence_url?: string;
    attribution?: string;
    obligations?: string;
    fetch: { kind: string; url?: string | null };
    version: string;
    refresh: number | "manual";
    copy: boolean;
    credential?: { env?: string[]; file?: string | null };
    credential_missing: boolean;
    /** By target: its policy, and each request that its serving release reads. */
    targets: Record<
        string,
        {
            refresh: number | "manual";
            state: SourceState;
            requests: {
                serving: string | null;
                observation: { result: { state: string; value?: string }; checked_at: number | null };
                state: SourceState;
                reason: string | null;
                age_days: number | null;
            }[];
        }
    >;
    /** A prepared source, by target: its complete versions, and the run that prepares it there. */
    prepared: Record<string, { versions: { version: string; bytes: number; time: string }[]; lock: Holder | null }>;
}

export interface PrepareReview {
    id: string;
    source: string;
    target: string;
    version: string;
    abandoned: string | null;
    inputs: { source: string; version: string | null; bytes: number | null }[];
    cost: { wall_ms: number | null; peak_rss_bytes: number | null; store_bytes: number; upload_bytes: number | null; unknown: string[] };
    machine: { refusals: string[] };
    /** The layers of each target that are stale once the version is complete. */
    stale: { target: string; layers: string[]; error: string | null }[];
    args: string[];
}

/** The states, worst first. */
export const ORDER: SourceState[] = ["blocked", "stale", "ok", "unused"];

/** The state that a person acts on first: the worst of `states`. */
export const worst = (states: SourceState[]): SourceState => ORDER.find((state) => states.includes(state)) ?? "unused";

/** A prepared source of a family, such as `terrain-reference/ch`, is listed under its family. */
export const family = (id: string): string => id.split("/")[0];

/** `90 days` or `manual`. */
export const policy = (refresh: number | "manual"): string => (refresh === "manual" ? "manual" : `${refresh} days`);
