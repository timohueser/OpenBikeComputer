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

/** The rectangles of region or Geofabrik path `id`: a preset by its kind, a union by its members. */
export function outline(areas: Areas | null, id: string | null, seen = new Set<string>()): Bbox[] {
    if (!areas || !id || seen.has(id)) return [];
    seen.add(id);
    const region = areas.regions.find((region) => region.id === id);
    if (region?.kind === "box") return [region.box];
    if (region?.kind === "union") return region.union.flatMap((member) => outline(areas, member, seen));
    const paths = region?.kind === "geofabrik" ? region.areas : [id];
    return paths.flatMap((path) => areas.areas.filter((area) => area.id === path).map((area) => area.bounds));
}

/** The smallest Geofabrik area whose bounds hold `box`: the extract that a box reads. */
export function extract(areas: Areas | null, box: Bbox): string | null {
    const holds = ({ bounds: b }: Areas["areas"][number]) => b.west <= box.west && b.south <= box.south && b.east >= box.east && b.north >= box.north;
    const size = ({ bounds: b }: Areas["areas"][number]) => (b.east - b.west) * (b.north - b.south);
    return (areas?.areas.filter(holds).sort((a, b) => size(a) - size(b))[0]?.id) ?? null;
}
