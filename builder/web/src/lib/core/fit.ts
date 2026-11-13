import { core } from "./bridge";
import type { GpxImport, GpxPoint, GpxWaypoint, Sensors } from "./gpx";

/** `unknown`: the file has no session, as after a power loss. It is written as `other`. */
export type FitSport = "road" | "gravel" | "mtb" | "bike" | "foot" | "other" | "unknown";

/** One recorded point of a ride, in microdegrees. An absent value has no key. */
export interface FitSample extends Sensors {
    /** Unix time in seconds. */
    timeS: number;
    lat: number;
    lon: number;
    /** True for the first sample of the ride and for the first sample after a pause. */
    segmentStart: boolean;
    elevationM?: number;
    speedMps?: number;
}

export interface FitRide { sport: FitSport; samples: FitSample[] }
/** A planned route, with the time of the whole course in seconds. */
export type FitCourse = GpxImport & { elapsedS: number };

export class FitError extends Error {
    constructor(message: string, readonly code: string) { super(message); this.name = "FitError"; }
}

/** The optional values after time, lat, lon and segmentStart, in the order of the bridge. */
const OPTIONAL = ["elevationM", "speedMps", "hrBpm", "cadenceRpm", "powerW", "temperatureC"] as const;
const VALUES = 4 + OPTIONAL.length;

type RawFit =
    | { sport: FitSport; samples: Float64Array }
    | { name: string; points: Float64Array; waypoints: Float64Array; names: (string | undefined)[]; elapsedS: number };

/** Reads a FIT activity as a ride, or a FIT course as a route. A course with no name gets `fallback`. */
export function readFit(bytes: Uint8Array, fallback: string): FitRide | FitCourse {
    let raw: RawFit;
    try { raw = core().obc_fit_read(bytes, fallback) as RawFit; } catch (cause) {
        if (cause instanceof Error && cause.name === "FitError") {
            throw new FitError(cause.message, (cause as Error & { code: string }).code);
        }
        throw cause;
    }
    if ("samples" in raw) {
        const v = raw.samples;
        const samples: FitSample[] = [];
        for (let at = 0; at < v.length; at += VALUES) {
            const sample: FitSample = { timeS: v[at], lat: v[at + 1], lon: v[at + 2], segmentStart: v[at + 3] === 1 };
            OPTIONAL.forEach((key, k) => { if (!Number.isNaN(v[at + 4 + k])) sample[key] = v[at + 4 + k]; });
            samples.push(sample);
        }
        return { sport: raw.sport, samples };
    }
    const points: GpxPoint[] = [];
    for (let at = 0; at < raw.points.length; at += 3) {
        const [lat, lon, ele] = raw.points.subarray(at, at + 3);
        points.push(Number.isNaN(ele) ? { lat, lon } : { lat, lon, ele });
    }
    const waypoints: GpxWaypoint[] = raw.names.map((name, i) => {
        const [lat, lon] = raw.waypoints.subarray(2 * i, 2 * i + 2);
        return name === undefined ? { lat, lon } : { lat, lon, name };
    });
    return { name: raw.name, points, waypoints, elapsedS: raw.elapsedS };
}

export function writeFitRide(ride: FitRide): Uint8Array {
    const values = new Float64Array(ride.samples.length * VALUES);
    ride.samples.forEach((s, i) => {
        values.set([s.timeS, s.lat, s.lon, s.segmentStart ? 1 : 0, ...OPTIONAL.map((key) => s[key] ?? NaN)], i * VALUES);
    });
    return core().obc_fit_write_ride(ride.sport, values);
}

/** Writes a course. A waypoint note is not written. */
export function writeFitCourse(course: FitCourse): Uint8Array {
    return core().obc_fit_write_course(
        course.name,
        Float64Array.from(course.points.flatMap((p) => [p.lat, p.lon, p.ele ?? NaN])),
        Float64Array.from(course.waypoints.flatMap((w) => [w.lat, w.lon])),
        course.waypoints.map((w) => w.name ?? ""),
        course.elapsedS,
    );
}
