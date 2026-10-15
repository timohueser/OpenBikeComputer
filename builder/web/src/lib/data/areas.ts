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
    | { kind: "planet" }
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
