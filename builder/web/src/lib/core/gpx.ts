import { core } from "./bridge";
import type { LatLon } from "../catalog/corridor";

export type GpxPoint = LatLon & { ele?: number };
export type GpxWaypoint = LatLon & { name?: string; note?: string };
export interface GpxRoute { name: string; points: LatLon[]; distanceKm: number }
export interface GpxImport { name: string; points: GpxPoint[]; waypoints: GpxWaypoint[] }
export const MAX_ROUTE_POINTS = core().obc_builder_constants()[5];

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

export function parseGpx(text: string, fallback: string): GpxRoute {
    return read(() => core().obc_gpx_parse(text, fallback));
}
