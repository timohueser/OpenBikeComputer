// What `obc data areas --json` writes: the presets of `data/regions/` and the areas of the cached
// Geofabrik index. Bounds are degrees.
import type { DegPoint } from "../map/coverageMap";

export interface Bbox {
    west: number;
    south: number;
    east: number;
    north: number;
}

export type Region = { id: string; name: string; countries: string[] } & (
    | { kind: "geofabrik"; areas: string[] }
    | { kind: "box"; box: Bbox }
    | { kind: "union"; union: string[] }
);

export interface Areas {
    regions: Region[];
    /** The version of the cached index; `null` while the store has none. */
    index: string | null;
    areas: { id: string; name: string; parent: string | null; countries: string[]; bounds: Bbox }[];
}

export const corners = ({ west, south, east, north }: Bbox): [DegPoint, DegPoint] => [
    [south, west],
    [north, east],
];

/** The smallest box that holds every box of `boxes`, or `null` for none. */
export function union(boxes: Bbox[]): Bbox | null {
    if (!boxes.length) return null;
    return {
        west: Math.min(...boxes.map((box) => box.west)),
        south: Math.min(...boxes.map((box) => box.south)),
        east: Math.max(...boxes.map((box) => box.east)),
        north: Math.max(...boxes.map((box) => box.north)),
    };
}

/** `W,S,E,N` in degrees, as `--box` takes it. */
export function parseBox(value: string): Bbox | null {
    const [west, south, east, north] = value.split(",").map(Number);
    return [west, south, east, north].every(Number.isFinite) ? { west, south, east, north } : null;
}

/** The bounds of `id`: a preset, Geofabrik areas joined by `,`, or the areas of a union. */
export function boundsOf(id: string, areas: Areas): Bbox | null {
    const region = areas.regions.find((region) => region.id === id);
    if (region?.kind === "box") return region.box;
    const parts = region?.kind === "geofabrik" ? region.areas : region?.kind === "union" ? region.union : id.split(",");
    const found = parts.map((part) => (part === id ? null : boundsOf(part, areas)) ?? areas.areas.find((area) => area.id === part)?.bounds ?? null);
    return found.every(Boolean) ? union(found as Bbox[]) : null;
}

/** The name of `id` as a person reads it. */
export function nameOf(id: string, areas: Areas | null): string {
    const named = (part: string) => areas?.regions.find((region) => region.id === part)?.name ?? areas?.areas.find((area) => area.id === part)?.name ?? part;
    return new Intl.ListFormat("en").format(id.split(",").map(named));
}

/** About `52 × 88 km`: the size of `box` on the ground. */
export function extent({ west, south, east, north }: Bbox): string {
    const km = (degrees: number) => Math.round(degrees * 111.32);
    return `${km((east - west) * Math.cos((((south + north) / 2) * Math.PI) / 180))} × ${km(north - south)} km`;
}

/** What a search finds, at most `limit`: an exact name first, then a name that starts with the
 *  query, then one with a word that does, then the rest; saved areas before Geofabrik areas of the
 *  same rank. */
export function search(query: string, areas: Areas, limit = 12): { id: string; name: string; detail: string }[] {
    const words = query.trim().toLowerCase();
    if (!words) return [];
    // The last part of an id only: `central-america/belize` is not a hit for "central".
    const rank = (id: string, name: string) => {
        const [text, last] = [name.toLowerCase(), id.split("/").at(-1)!.toLowerCase()];
        if (text === words || last === words) return 0;
        if (text.startsWith(words) || last.startsWith(words)) return 1;
        if (` ${text}`.includes(` ${words}`) || `-${last}`.includes(`-${words}`)) return 2;
        return text.includes(words) || last.includes(words) ? 3 : null;
    };
    const parent = (id: string | null) => (id ? (areas.areas.find((area) => area.id === id)?.name ?? id) : null);
    const hits = [
        ...areas.regions.map((region) => ({ id: region.id, name: region.name, detail: "Saved area" })),
        ...areas.areas
            .filter((area) => !areas.regions.some((region) => region.id === area.id))
            .map((area) => ({ id: area.id, name: area.name, detail: parent(area.parent) ?? "Geofabrik" })),
    ].flatMap((hit) => {
        const at = rank(hit.id, hit.name);
        return at === null ? [] : [{ hit, at }];
    });
    return hits
        .sort((a, b) => a.at - b.at)
        .slice(0, limit)
        .map(({ hit }) => hit);
}
