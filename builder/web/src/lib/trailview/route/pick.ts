import { NEAR } from '../frame/camera';
import { CURVATURE } from '../frame/frame';

/** The lift of the ribbon of route.wgsl above the surface. */
export const LIFT_M = 0.3;
/** A route is under the pointer when a segment between two of its pick points passes this near, in CSS pixels. */
const REACH_PX = 12;
/** `onScreen` cuts a segment through the near plane here: `onImage` drops a point at NEAR or nearer, so a cut exactly at NEAR can round to behind the eye. */
const CUT = 2 * NEAR;

/** A point of the frame at a height, about the eye as route.wgsl draws its ribbon: LIFT_M above it, with the curvature term. */
export function aboutEye(eye: readonly number[], east: number, north: number, height: number): [number, number, number] {
    const x = east - eye[0], y = north - eye[1];
    return [x, y, height + LIFT_M - eye[2] - CURVATURE * (x * x + y * y)];
}

/** Clip w of a point about the eye: its distance along the view axis. */
function clipW(m: Float32Array, [x, y, z]: readonly number[]): number {
    return m[3] * x + m[7] * y + m[11] * z + m[15];
}

/** Where a point about the eye lies on the image, from 0 to 1 to the right and down, or undefined behind the near plane. */
export function onImage(m: Float32Array, p: readonly number[]): [number, number] | undefined {
    const ahead = clipW(m, p), [x, y, z] = p;
    if (!(ahead > NEAR)) return undefined;
    return [((m[0] * x + m[4] * y + m[8] * z + m[12]) / ahead + 1) / 2, (1 - (m[1] * x + m[5] * y + m[9] * z + m[13]) / ahead) / 2];
}

/**
 * The pick points of a route on a screen of `width` × `height` CSS pixels, as `nearestRoute` takes them,
 * with the view projection `m` about `eye`. A segment through CUT keeps its part in front of it: the cut
 * adds a point between its two ends.
 */
export function onScreen(m: Float32Array, eye: readonly number[], width: number, height: number, points: Float64Array): Float32Array {
    const out = new Float32Array(2 * points.length);
    let n = 0;
    const put = (p: readonly number[]) => {
        const at = onImage(m, p);
        out[n++] = at ? at[0] * width : NaN;
        out[n++] = at ? at[1] * height : NaN;
        out[n++] = at ? clipW(m, p) : NaN;
    };
    for (let i = 0, last: number[] | undefined, lastW = NaN; i < points.length; i += 3) {
        const p = aboutEye(eye, points[i], points[i + 1], points[i + 2]), w = clipW(m, p);
        if (last && (lastW > CUT) !== (w > CUT)) {
            const t = (CUT - lastW) / (w - lastW);
            put(last.map((value, j) => value + t * (p[j] - value)));
        }
        put(p);
        last = p;
        lastW = w;
    }
    return out.subarray(0, n);
}

/**
 * The number of the route under the pointer at (x, y), or null for none. Entry k is route k + 1: x and y
 * in CSS pixels and clip w of each pick point (`onScreen`), NaN where a segment ends. Of the routes with
 * a segment within REACH_PX, the bold route wins, then the route whose nearest place on such a segment
 * is nearest to the eye, then the lower number.
 */
export function nearestRoute(routes: readonly (Float32Array | undefined)[], x: number, y: number, bold: number | null): number | null {
    let best: { route: number; distance: number } | undefined;
    routes.forEach((points, k) => {
        const distance = points && nearest(points, x, y);
        if (distance === undefined) return;
        if (k + 1 === bold) best = { route: bold, distance: -Infinity };
        else if (!best || distance < best.distance) best = { route: k + 1, distance };
    });
    return best?.route ?? null;
}

/** The least clip w of the nearest places on the segments within REACH_PX of (x, y), or undefined. */
function nearest(points: Float32Array, x: number, y: number): number | undefined {
    let least: number | undefined;
    for (let i = 3; i < points.length; i += 3) {
        const ax = points[i - 3], ay = points[i - 2], dx = points[i] - ax, dy = points[i + 1] - ay;
        const t = Math.min(1, Math.max(0, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy || 1)));
        // A point behind the eye makes every comparison false.
        if (!(Math.hypot(ax + t * dx - x, ay + t * dy - y) <= REACH_PX)) continue;
        // 1 / w is linear on the screen, so this is the clip w of the place.
        const distance = 1 / ((1 - t) / points[i - 1] + t / points[i + 2]);
        if (least === undefined || distance < least) least = distance;
    }
    return least;
}
