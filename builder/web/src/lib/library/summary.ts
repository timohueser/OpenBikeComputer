/** Makes the summary of a route or a ride: its line simplified to about 1,000 points, its bounds and its numbers. */
import type { Summary } from './records';

/** Longitude, latitude, and the height in metres or null. */
export type Point = readonly [number, number, (number | null)?];
/** The numbers that the caller knows; `summarize` finds the others from the points. */
export type Numbers<S extends Summary> = Omit<S, 'line' | 'heights' | 'bounds' | 'highestM'>;

const maxPoints = 1000;
// The group rule of rides allows 50 m between two lines, so a simplified line stays within 40 m of its points.
const firstToleranceM = 10, lastToleranceM = 40;
const metresPerDegree = 6_371_008.8 * Math.PI / 180;

const round = (value: number, scale: number) => Math.round(value * scale) / scale;

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
