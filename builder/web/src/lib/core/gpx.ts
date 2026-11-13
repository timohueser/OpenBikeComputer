import { core } from "./bridge";

/** A coordinate in integer microdegrees, `lat, lon` — the catalog's order. */
export interface LatLon {
    lat: number;
    lon: number;
}

/** The sensor values of a point or a sample. An absent value has no key. */
export interface Sensors {
    hrBpm?: number;
    cadenceRpm?: number;
    powerW?: number;
    temperatureC?: number;
}

/** `time` is in Unix seconds. `segmentStart` marks the first point of the file and of each track and track segment. */
export type GpxPoint = LatLon & Sensors & { ele?: number; time?: number; segmentStart?: true };
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

// The five entities that `readGpx` reads.
const ENTITIES: Record<string, string> = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&apos;" };
const escape = (text: string) => text.replace(/[&<>"']/g, c => ENTITIES[c]);
const place = ({ lat, lon }: LatLon) => `lat="${(lat / 1e6).toFixed(6)}" lon="${(lon / 1e6).toFixed(6)}"`;
const text = (tag: string, value: string | undefined) => value ? `<${tag}>${escape(value)}</${tag}>` : "";

const number = (tag: string, value: number | undefined) => value === undefined ? "" : `<${tag}>${value}</${tag}>`;
const whole = (value: number | undefined) => value === undefined ? undefined : Math.round(value);

/** The shape of the device export. The schema types `hr` and `cad` as unsigned bytes and puts `atemp` first. */
function extensions(p: Sensors): string {
    const garmin = number("gpxtpx:atemp", p.temperatureC) + number("gpxtpx:hr", whole(p.hrBpm)) + number("gpxtpx:cad", whole(p.cadenceRpm));
    const body = (garmin && `<gpxtpx:TrackPointExtension>${garmin}</gpxtpx:TrackPointExtension>`) + number("power", p.powerW);
    return body && `<extensions>${body}</extensions>`;
}

const point = (p: GpxPoint, i: number) => `${i && p.segmentStart ? "</trkseg><trkseg>\n" : ""}<trkpt ${place(p)}>`
    + `${number("ele", p.ele)}${p.time === undefined ? "" : `<time>${new Date(p.time * 1000).toISOString()}</time>`}${extensions(p)}</trkpt>`;

/** A GPX 1.1 file with one track and a `wpt` for each waypoint; a waypoint's note is its `desc`. A point with
 * `segmentStart` after the first starts a new `trkseg`. */
export function writeGpx(route: GpxImport): string {
    return [
        '<?xml version="1.0" encoding="UTF-8"?>',
        '<gpx version="1.1" creator="OpenBikeComputer" xmlns="http://www.topografix.com/GPX/1/1"'
            + ' xmlns:gpxtpx="http://www.garmin.com/xmlschemas/TrackPointExtension/v1">',
        ...route.waypoints.map(w => `<wpt ${place(w)}>${text("name", w.name)}${text("desc", w.note)}</wpt>`),
        `<trk>${text("name", route.name)}<trkseg>`,
        ...route.points.map(point),
        "</trkseg></trk>",
        "</gpx>",
        "",
    ].join("\n");
}

export function readGpx(text: string, fallback: string): GpxImport {
    const imported = read<GpxImport>(() => core().obc_gpx_read(text, fallback));
    for (const waypoint of imported.waypoints) waypoint.name ??= undefined;
    return imported;
}
