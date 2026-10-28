// What `obc data sources --json` and `source prepare --dry-run --json` write.
import { list, size, type Holder } from "./shapes";

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
    credential?: { env?: string[]; file?: string | null };
    credential_missing: boolean;
    /** Whether `source refresh` moves the source. */
    refreshable: boolean;
    /** The versions in this machine's store, the one fetched last first. */
    snapshots: { version: string; bytes: number }[];
    /** For a source fetched with params: the newest fetch of each set of params in this store. */
    requests: StoredRequest[];
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
    /** A prepared source: the archive root of its home (`null` without one), its complete versions
     *  there, and the run that prepares it. `error` says why they cannot be read. `null` for another
     *  source. */
    prepared: { root: string | null; versions: PreparedVersion[]; lock: Holder | null; error: string | null } | null;
}

export interface StoredRequest {
    params: [string, string][];
    version: string;
    bytes: number;
    /** When its newest file was retrieved; `null` for a fetch without files. */
    fetched: string | null;
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
    /** The archive root of the home of the source. */
    root: string;
    version: string;
    abandoned: string | null;
    scope: { region: string; definitions: Record<string, string> } | null;
    languages: string[];
    inputs: { source: string; version: string | null; bytes: number | null; download_bytes: number | null; params: [string, string][] }[];
    /** The files of the version at most; `null` when the producer cannot tell before the run. */
    files: number | null;
    /** Whether the version publishes its article text; `null` for a source without text. */
    publish_text: boolean | null;
    cost: { wall_ms: number | null; peak_rss_bytes: number | null; store_bytes: number; archive_bytes: number | null; unknown: string[] };
    machine: { refusals: string[] };
    /** The layers of each target that are stale once the version is complete. */
    stale: { target: string; layers: string[]; error: string | null }[];
    args: string[];
}

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

/** What the first param of a request names, singular and plural. */
const NOUNS: Record<string, [string, string]> = { area: ["area", "areas"], bbox: ["box", "boxes"], tile: ["tile", "tiles"] };

const days = (n: number) => `${n} ${n === 1 ? "day" : "days"}`;

/** The age in days of `request`: since its fetch, else since its date version. */
export const requestAge = (request: StoredRequest, now = Date.now()): number | null => daysOld(request.fetched?.slice(0, 10) ?? request.version, now);

/** `16 areas · 8–16 days old · 6.3 GB`: the requests of a source in the store, and the `bytes` of
 *  its versions there. Requests can share files, so their sizes do not add up. */
export function requestsLine(requests: StoredRequest[], bytes: number, now = Date.now()): string {
    const key = requests[0].params[0]?.[0];
    const [one, many] = NOUNS[key] ?? ["request", "requests"];
    const values = new Set(requests.flatMap((request) => request.params.filter(([name]) => name === key).map(([, value]) => value)));
    const count = NOUNS[key] ? values.size : requests.length;
    const ages = requests.map((request) => requestAge(request, now)).filter((age) => age !== null);
    const [young, old] = [Math.min(...ages), Math.max(...ages)];
    const age = ages.length ? [`${young === old ? days(old) : `${young}–${days(old)}`} old`] : [];
    return [`${count} ${count === 1 ? one : many}`, ...age, size(bytes)].join(" · ");
}

/** What a request fetches: area names, a box as W,S,E,N, tiles as a count with the first few. */
export function requestLabel(request: StoredRequest, areaName: (id: string) => string): string {
    return [...new Set(request.params.map(([name]) => name))]
        .map((name) => {
            const values = request.params.filter(([at]) => at === name).map(([, value]) => value);
            if (name === "area") return list(values.map(areaName));
            if (name === "bbox") return `box ${values.join("; ")}`;
            if (name === "tile" && values.length > 3) return `${values.length} tiles: ${values.slice(0, 3).join(", ")}, …`;
            return `${NOUNS[name]?.[values.length === 1 ? 0 : 1] ?? name} ${values.join(", ")}`;
        })
        .join(" · ");
}

/** The references, as the References screen names them. Each version covers an area. A version of
 *  a reference of national models, the downloads `<models><country>`, adds its area to the one before. */
export const REFERENCES: { id: string; title: string; about: string; add: string; models?: { prefix: string; noun: string } }[] = [
    { id: "wiki", title: "Wiki", about: "Facts, article text and photos of landmarks and peaks, from the Wikimedia dumps", add: "Prepare a newer version" },
    {
        id: "terrain",
        title: "Terrain",
        about: "The national LiDAR terrain models, merged best first",
        add: "Add an area",
        models: { prefix: "dtm-", noun: "national terrain model" },
    },
    {
        id: "forest",
        title: "Forest",
        about: "Canopy height and leaf type for the Trail View forest, from the national canopy models, else a global one",
        add: "Add an area",
        models: { prefix: "canopy-", noun: "canopy model" },
    },
    { id: "climate", title: "Climate", about: "Daily ERA5-Land temperature, rain and wind for the climate layer", add: "Add an area" },
];

export const referenceOf = (id: string) => REFERENCES.find((reference) => reference.id === id);

/** The national models of reference `id` in `rows`, with their countries: `dtm-de-by` is of `DE`. */
export function modelsOf(id: string, rows: SourceRow[]): { row: SourceRow; country: string }[] {
    const prefix = referenceOf(id)?.models?.prefix;
    if (!prefix) return [];
    return rows
        .filter((row) => row.id.startsWith(prefix) && row.id !== `${prefix}global`)
        .map((row) => ({ row, country: row.id.slice(prefix.length).split("-")[0].toUpperCase() }));
}

/** The model of reference `id` in `rows` that covers every country, such as `canopy-global`. */
export function globalModelOf(id: string, rows: SourceRow[]): SourceRow | undefined {
    const prefix = referenceOf(id)?.models?.prefix;
    return prefix ? rows.find((row) => row.id === `${prefix}global`) : undefined;
}

/** Where a reference is: what a person acts on first. */
export type Coverage = "lock" | "preparing" | "archive" | "none";

export const COVERAGE: Record<Coverage, { label: string; color: string }> = {
    archive: { label: "In the archive", color: "var(--ready)" },
    preparing: { label: "Being prepared", color: "var(--running)" },
    lock: { label: "Lock left by a stopped run", color: "var(--missing)" },
    none: { label: "Not prepared", color: "var(--ink-faint)" },
};

/** The coverage of `row` from its lock and versions. */
export function coverage(row: SourceRow): Coverage {
    const lock = row.prepared?.lock;
    if (lock && (lock.died || lock.stale)) return "lock";
    if (lock) return "preparing";
    return row.prepared?.versions.length ? "archive" : "none";
}

/** The boxes of the coverage map of reference `row`: the extent of each of its `models`, not
 *  prepared, and above them the area of each version that `bounds` finds a box for. */
export function coverageCells(
    row: SourceRow,
    models: SourceRow[],
    bounds: (region: string) => [number, number, number, number] | null,
): { box: [number, number, number, number]; state: Coverage; label: string }[] {
    const available = models.flatMap((model) => (model.extent ? [{ box: model.extent, state: "none" as Coverage, label: `${model.use ?? model.id} · ${COVERAGE.none.label}` }] : []));
    const made = (row.prepared?.versions ?? []).flatMap((version) => {
        const box = version.region ? bounds(version.region) : null;
        return box ? [{ box, state: "archive" as Coverage, label: `${version.region} · ${COVERAGE.archive.label}` }] : [];
    });
    return [...available, ...new Map(made.map((cell) => [cell.label, cell])).values()];
}
