import { core } from "./bridge";

/** A coordinate in integer microdegrees, `lat, lon` — the catalog's order. */
export interface LatLon {
    lat: number;
    lon: number;
}

/** `time` is in Unix seconds. `segmentStart` marks the first point of the file and of each track and track segment. */
export type GpxPoint = LatLon & { ele?: number; time?: number; segmentStart?: true };
export type GpxWaypoint = LatLon & { name?: string; note?: string };
/** `kind` is the `<type>` of the first track, such as `cycling`. */
export interface GpxImport { name: string; points: GpxPoint[]; waypoints: GpxWaypoint[]; kind?: string }

export class GpxError extends Error {
    constructor(message: string) { super(message); this.name = "GpxError"; }
}

function read<T>(fn: () => string): T {
    try { return JSON.parse(fn()) as T; } catch (cause) {
        if (cause instanceof Error && cause.name === "GpxError") throw new GpxError(cause.message);
        throw cause;
    }
}

export function readGpx(text: string, fallback: string): GpxImport {
    const imported = read<GpxImport>(() => core().obc_gpx_read(text, fallback));
    for (const waypoint of imported.waypoints) waypoint.name ??= undefined;
    return imported;
}
