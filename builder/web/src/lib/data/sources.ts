// What `obc data sources --json` and `source prepare --dry-run --json` write.
import type { Holder } from "./shapes";

export type SourceState = "blocked" | "stale" | "ok" | "unused";

export interface SourceRow {
    id: string;
    kind: "data" | "asset" | "tool";
    group?: string;
    /** The parts of a bake that read it: `device`, `planner`, `search` or `trailview`. */
    used_by?: string[];
    /** What it is, in a few words. */
    use?: string;
    /** West, south, east, north in degrees: where it has data. */
    extent?: [number, number, number, number] | null;
    licence?: string;
    licence_url?: string;
    attribution?: string;
    obligations?: string;
    fetch: { kind: string; url?: string | null };
    /** Hosts that the fetch reaches besides the host of `fetch.url`; `*.` is any subdomain. */
    hosts?: string[];
    version: string;
    refresh: number | "manual";
    copy: boolean;
    credential?: { env?: string[]; file?: string | null };
    credential_missing: boolean;
    /** Whether `source refresh` moves the source. */
    refreshable: boolean;
    /** The versions in this machine's store, the one fetched last first. */
    snapshots: { version: string; bytes: number }[];
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
    /** A prepared source, by target: its complete versions, and the run that prepares it there.
     *  `error` says why they cannot be read. */
    prepared: Record<string, { versions: PreparedVersion[]; lock: Holder | null; error: string | null }>;
}

export interface PreparedVersion {
    version: string;
    bytes: number;
    time: string;
    /** The region that it covers; absent for all of the source. */
    region?: string;
}

export interface PrepareReview {
    id: string;
    source: string;
    target: string;
    version: string;
    abandoned: string | null;
    scope: { region: string; definitions: Record<string, string> } | null;
    languages: string[];
    inputs: { source: string; version: string | null; bytes: number | null; download_bytes: number | null; params: [string, string][] }[];
    /** The files of the version at most; `null` when the producer cannot tell before the run. */
    files: number | null;
    /** Whether the version publishes its article text; `null` for a source without text. */
    publish_text: boolean | null;
    cost: { wall_ms: number | null; peak_rss_bytes: number | null; store_bytes: number; upload_bytes: number | null; unknown: string[] };
    machine: { refusals: string[] };
    /** The layers of each target that are stale once the version is complete. */
    stale: { target: string; layers: string[]; error: string | null }[];
    args: string[];
}

/** A reference of a family, such as `terrain-reference/ch`, is listed under its family. */
export const family = (id: string): string => id.split("/")[0];

/** `90 days` or `manual`. */
export const policy = (refresh: number | "manual"): string => (refresh === "manual" ? "manual" : `${refresh} days`);

/** A reference: `obc data source prepare` makes its versions. Every other source is a download. */
export const isReference = (row: SourceRow): boolean => row.fetch.kind === "prepared";

/** The headings of Downloads, in order, as `group` in data/sources.toml names them. */
export const GROUPS = ["OpenStreetMap", "Elevation", "Wikimedia", "Climate and snow", "Land cover and imagery", "Map styles and tools"];

/** The parts of a bake, as `used_by` names them. */
export const PARTS: Record<string, string> = { device: "Bike computer maps", planner: "Planner", search: "Search", trailview: "Trail View" };

/** The hosts that a fetch of `row` reaches. */
export function hostsOf(row: SourceRow): string[] {
    const url = row.fetch.url ? [new URL(row.fetch.url).host] : [];
    return [...new Set([...url, ...(row.hosts ?? []).map((host) => host.replace(/^\*\./, ""))])];
}

/** The age in days of a date version such as `2026-10-08`; `null` for another version. */
export function daysOld(version: string, now = Date.now()): number | null {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(version)) return null;
    return Math.max(0, Math.floor((now - Date.parse(`${version}T00:00:00Z`)) / 86_400_000));
}

/** The reference families, as the References screen names them. A `regional` reference is made
 *  area by area: a version of `terrain` adds an area to the one before. */
export const FAMILIES = [
    { family: "wiki", title: "Wiki", about: "Facts, article text and photos of landmarks and peaks, from the Wikimedia dumps", regional: true, add: "Prepare a newer version" },
    {
        family: "terrain",
        title: "Terrain",
        about: "The national LiDAR models, merged best first; each version adds an area to the one before",
        regional: true,
        add: "Add an area",
    },
    { family: "forest-reference", title: "Forest", about: "Canopy height and leaf type, for the Trail View forest", regional: false, add: "Prepare a newer version" },
];

/** Whether `prod` takes a version of an area of reference `id`. */
export const regional = (id: string): boolean => !!FAMILIES.find((at) => at.family === family(id))?.regional;

/** Where a reference is: what a person acts on first. */
export type Coverage = "lock" | "preparing" | "r2" | "local" | "none";

export const COVERAGE: Record<Coverage, { label: string; color: string }> = {
    r2: { label: "On R2", color: "var(--ready)" },
    local: { label: "On this computer only", color: "var(--stale)" },
    preparing: { label: "Being prepared", color: "var(--running)" },
    lock: { label: "Lock left by a stopped run", color: "var(--missing)" },
    none: { label: "Not prepared", color: "var(--ink-faint)" },
};

/** The coverage of `row` from its locks and the versions that `counts`; `r2` tells which targets
 *  are R2 buckets. */
function placed(row: SourceRow, r2: (target: string) => boolean, counts: (version: PreparedVersion) => boolean): Coverage {
    const on = Object.entries(row.prepared);
    if (on.some(([, at]) => at.lock && (at.lock.died || at.lock.stale))) return "lock";
    if (on.some(([, at]) => at.lock)) return "preparing";
    const held = on.filter(([, at]) => at.versions.some(counts)).map(([target]) => target);
    return held.some(r2) ? "r2" : held.length ? "local" : "none";
}

export const coverage = (row: SourceRow, r2: (target: string) => boolean): Coverage => placed(row, r2, () => true);

/** The boxes of a coverage map: the extent of each member in the state of its versions of all of
 *  it, and above it each regional version that `bounds` finds a box for. */
export function coverageCells(
    rows: SourceRow[],
    r2: (target: string) => boolean,
    bounds: (region: string) => [number, number, number, number] | null,
): { box: [number, number, number, number]; state: Coverage; label: string }[] {
    const whole = rows.flatMap((row) => {
        const state = placed(row, r2, (version) => !version.region);
        // A reference of the whole world shows the areas of its versions only.
        const world = row.extent && row.extent[2] - row.extent[0] >= 360;
        return row.extent && !world ? [{ box: row.extent, state, label: `${row.use ?? row.id} · ${COVERAGE[state].label}` }] : [];
    });
    const regional = rows.flatMap((row) =>
        Object.entries(row.prepared).flatMap(([target, at]) =>
            at.versions.flatMap((version) => {
                const box = version.region ? bounds(version.region) : null;
                const state: Coverage = r2(target) ? "r2" : "local";
                return box ? [{ box, state, label: `${row.use ?? row.id} · ${version.region} · ${COVERAGE[state].label}` }] : [];
            }),
        ),
    );
    return [...whole, ...new Map(regional.map((cell) => [cell.label, cell])).values()];
}
