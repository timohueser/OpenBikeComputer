/** Makes the summary of a route or a ride: its line simplified to about 1,000 points, its bounds and its numbers. */
import type { Bike, RideRecord, RideSummary, RouteRecord, Summary } from './records';

/** One recorded point of a ride: Unix seconds and microdegrees. `segmentStart` marks the first point and the first
 * point after a pause. */
export interface RideSample {
    timeS: number;
    lat: number;
    lon: number;
    elevationM?: number | null;
    segmentStart: boolean;
}

/** FNV-1a of `text`, as hex. */
function hash(text: string): string {
    let value = 0x811c9dc5;
    for (let i = 0; i < text.length; i++) value = Math.imul(value ^ text.charCodeAt(i), 0x01000193);
    return (value >>> 0).toString(16).padStart(8, '0');
}

/** A hash of the summary line, and for a ride of its start time, as hex. A new name keeps it. */
export const lineKey = (item: RouteRecord | RideRecord) => hash(`${item.summary?.line.join(',')}@${'key' in item ? item.summary.start : ''}`);

/** A hash of what the device file of a route is made from: its name and its plan (`trip`), which give the line, the
 * waypoints, the days and the bike. A route whose key differs from the key in `sent` changed since it was sent. */
export const sentKey = (route: RouteRecord) => hash(JSON.stringify([route.name, route.trip]));

/** The points of the summary line as [longitude, latitude]. */
export function lineCoordinates({ line }: Summary): [number, number][] {
    return Array.from({ length: line.length / 2 }, (_, i) => [line[2 * i], line[2 * i + 1]]);
}

/** Longitude, latitude, and the height in metres or null. */
export type Point = readonly [number, number, (number | null)?];
/** The numbers that the caller knows; `summarize` finds the others from the points. */
export type Numbers<S extends Summary> = Omit<S, 'line' | 'heights' | 'bounds' | 'highestM'>;

const maxPoints = 1000;
// The group rule of rides allows 50 m between two lines, so a simplified line stays within 40 m of its points.
const firstToleranceM = 10, lastToleranceM = 40;
const metresPerDegree = 6_371_008.8 * Math.PI / 180;

// Start values, to test on real rides.
const climbStepM = 5, movingMps = 2 / 3.6;

const round = (value: number, scale: number) => Math.round(value * scale) / scale;

/** The great-circle distance between two samples. */
export function metres(a: RideSample, b: RideSample): number {
    const rad = Math.PI / 180e6, dLat = (b.lat - a.lat) * rad, dLon = (b.lon - a.lon) * rad;
    const h = Math.sin(dLat / 2) ** 2 + Math.cos(a.lat * rad) * Math.cos(b.lat * rad) * Math.sin(dLon / 2) ** 2;
    return 2 * metresPerDegree * 180 / Math.PI * Math.asin(Math.min(1, Math.sqrt(h)));
}

/** For each sample, the seconds of the step that ends at it when that step moves: at 2 km/h or more, and not to a
 * segment start. Else 0. The moving time of a ride is their sum. */
export function movingSteps(samples: RideSample[]): number[] {
    return samples.map((sample, i) => {
        if (!i || sample.segmentStart) return 0;
        const seconds = sample.timeS - samples[i - 1].timeS;
        return seconds > 0 && metres(samples[i - 1], sample) / seconds >= movingMps ? seconds : 0;
    });
}

/** The summary of a recorded ride. The length skips the step to a segment start. The climb counts a rise when the
 * height is 5 m above the last low. The moving time is the sum of `movingSteps`. `samples` holds at least one sample. */
export function rideSummary(samples: RideSample[], bike: Bike): RideSummary {
    let lengthM = 0, climbM = 0, low: number | null = null;
    samples.forEach((sample, i) => {
        const height = sample.elevationM;
        if (height != null) {
            if (low === null || height < low) low = height;
            else if (height - low >= climbStepM) { climbM += height - low; low = height; }
        }
        if (i && !sample.segmentStart) lengthM += metres(samples[i - 1], sample);
    });
    const start = samples[0].timeS, movingS = movingSteps(samples).reduce((sum, seconds) => sum + seconds, 0);
    return summarize<RideSummary>(samples.map(s => [s.lon / 1e6, s.lat / 1e6, s.elevationM]), {
        lengthM: Math.round(lengthM), climbM: Math.round(climbM), seconds: Math.round(samples.at(-1)!.timeS - start),
        movingS: Math.round(movingS), start: Math.floor(start), bike,
    });
}

/** Douglas–Peucker at 10 m. While more than 1,000 points remain, the tolerance doubles, to 40 m at most; at 40 m the
 * line keeps its points. `points` holds at least one point. */
export function summarize<S extends Summary>(points: readonly Point[], numbers: Numbers<S>): S {
    if (!points.length) throw new Error('A summary needs at least one point.');
    const n = points.length, lon = new Float64Array(n), lat = new Float64Array(n), cos = new Float64Array(n);
    let west = Infinity, south = Infinity, east = -Infinity, north = -Infinity, highestM: number | null = null;
    points.forEach(([x, y, height], i) => {
        lon[i] = x; lat[i] = y; cos[i] = Math.cos(y * Math.PI / 180);
        west = Math.min(west, x); east = Math.max(east, x); south = Math.min(south, y); north = Math.max(north, y);
        if (height != null && (highestM === null || height > highestM)) highestM = height;
    });
    let tolerance = firstToleranceM, kept = simplify(lon, lat, cos, tolerance);
    while (kept.length > maxPoints && tolerance < lastToleranceM) kept = simplify(lon, lat, cos, tolerance = Math.min(2 * tolerance, lastToleranceM));
    return {
        ...numbers,
        line: kept.flatMap(i => [round(lon[i], 1e5), round(lat[i], 1e5)]),
        heights: kept.map(i => points[i][2] == null ? null : Math.round(points[i][2]!)),
        bounds: [west, south, east, north],
        highestM: highestM === null ? null : Math.round(highestM),
    } as S;
}

/** The indices of the points that Douglas–Peucker keeps. A point's distance from a segment is measured in degrees of
 * latitude, with the longitude scaled at the latitude of the segment's first point. */
function simplify(lon: Float64Array, lat: Float64Array, cos: Float64Array, toleranceM: number): number[] {
    const n = lon.length, keep = new Uint8Array(n), limit = (toleranceM / metresPerDegree) ** 2;
    keep[0] = keep[n - 1] = 1;
    const stack = n > 2 ? [0, n - 1] : [];
    while (stack.length) {
        const last = stack.pop()!, first = stack.pop()!, k = cos[first];
        const bx = (lon[last] - lon[first]) * k, by = lat[last] - lat[first], length = bx * bx + by * by;
        let far = -1, farthest = limit;
        for (let i = first + 1; i < last; i++) {
            const px = (lon[i] - lon[first]) * k, py = lat[i] - lat[first];
            const t = length ? Math.max(0, Math.min(1, (px * bx + py * by) / length)) : 0;
            const d = (px - t * bx) ** 2 + (py - t * by) ** 2;
            if (d > farthest) { far = i; farthest = d; }
        }
        if (far >= 0) { keep[far] = 1; stack.push(first, far, far, last); }
    }
    const kept: number[] = [];
    for (let i = 0; i < n; i++) if (keep[i]) kept.push(i);
    return kept;
}
