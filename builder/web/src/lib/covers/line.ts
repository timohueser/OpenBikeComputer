/** Reads the point list of a cover: the stretches of the line that show, the place of the mark and the point under the pointer. */
import type { CoverPoints, Summary } from '../library/records';
import { kilometres } from '../planner/geo';
import type { StillPoints } from '../trailview/still';

/** A visible or a hidden stretch shorter than this, in pixels of the image, joins the stretch before it, so trees and houses do not cut the line into dashes. */
const JOIN_PX = 16;
/** Distance code c of 1 to 127 is NEAREST_M · 2^((c − 1) / STEPS) metres: 9 % for each step, from 10 m to 550 km. */
const NEAREST_M = 10, STEPS = 8, VISIBLE = 128;

export function pack({ x, y, distance, visible }: StillPoints): CoverPoints {
    // Floats keep a point far outside the image on its line: a clamp of each axis would turn the segment to it.
    const points = { x: Float32Array.from(x), y: Float32Array.from(y), eye: new Uint8Array(x.length) };
    for (let i = 0; i < x.length; i++) {
        if (![x[i], y[i], distance[i]].every(Number.isFinite)) continue;
        const code = 1 + Math.round(STEPS * Math.log2(Math.max(distance[i], NEAREST_M) / NEAREST_M));
        points.eye[i] = Math.min(code, 127) | (visible[i] ? VISIBLE : 0);
    }
    return points;
}

export const placed = (points: CoverPoints, i: number) => (points.eye[i] & 127) !== 0;
export const visibleAt = (points: CoverPoints, i: number) => points.eye[i] >= VISIBLE;
/** Metres from the eye, NaN for a point with no place. */
export const distanceAt = (points: CoverPoints, i: number) => placed(points, i) ? NEAREST_M * 2 ** (((points.eye[i] & 127) - 1) / STEPS) : NaN;

/** The points `from` to `to` of the line. A segment is visible when its end point is. Two runs that follow each other share a point. */
export interface Run { from: number; to: number; visible: boolean }

/** The visible and the hidden stretches, after the rule of JOIN_PX. A point with no place ends a stretch, and the line has a gap there. */
export function runs(points: CoverPoints): Run[] {
    const segments: (Run & { px: number })[] = [];
    for (let i = 1; i < points.x.length; i++) {
        if (!placed(points, i - 1) || !placed(points, i)) continue;
        const visible = visibleAt(points, i), px = Math.hypot(points.x[i] - points.x[i - 1], points.y[i] - points.y[i - 1]), last = segments.at(-1);
        if (last?.to === i - 1 && last.visible === visible) { last.to = i; last.px += px; }
        else segments.push({ from: i - 1, to: i, visible, px });
    }
    const joined: typeof segments = [];
    for (const run of segments) {
        const last = joined.at(-1);
        if (last?.to === run.from && (run.px < JOIN_PX || run.visible === last.visible)) { last.to = run.to; last.px += run.px; }
        else joined.push(run);
    }
    return joined.map(({ from, to, visible }) => ({ from, to, visible }));
}

/** The metres along the route at each point of the summary line, from 0 to `lengthM`: the simplified line is shorter than the route. */
export function metresAlong({ line, lengthM }: Summary): Float64Array {
    const metres = new Float64Array(line.length / 2);
    for (let i = 1; i < metres.length; i++) metres[i] = metres[i - 1] + kilometres(line.slice(2 * i - 2, 2 * i), line.slice(2 * i, 2 * i + 2));
    const scale = metres.at(-1)! > 0 ? lengthM / metres.at(-1)! : 0;
    return metres.map(m => m * scale);
}

/** The place in the image of the point `metres` along the route, or undefined where the line has a gap. It is visible
 * when its segment is. */
export function along(points: CoverPoints, summary: Summary, metres: number): { x: number; y: number; distance: number; visible: boolean } | undefined {
    const at = metresAlong(summary);
    let i = 1;
    while (i < at.length - 1 && at[i] < metres) i++;
    if (!placed(points, i - 1) || !placed(points, i)) return undefined;
    const t = at[i] > at[i - 1] ? Math.max(0, Math.min(1, (metres - at[i - 1]) / (at[i] - at[i - 1]))) : 0;
    const mix = (a: number, b: number) => a + t * (b - a);
    return { x: mix(points.x[i - 1], points.x[i]), y: mix(points.y[i - 1], points.y[i]), distance: mix(distanceAt(points, i - 1), distanceAt(points, i)), visible: visibleAt(points, i) };
}

/** The metres along the route of the point of the line that is nearest to pixel (x, y) of the image; undefined when no
 * segment has a place within `within` pixels of the image. A hidden stretch counts as line. */
export function nearest(points: CoverPoints, summary: Summary, x: number, y: number, within = Infinity): number | undefined {
    const at = metresAlong(summary);
    let best: number | undefined, closest = Infinity;
    for (let i = 1; i < at.length; i++) {
        if (!placed(points, i - 1) || !placed(points, i)) continue;
        const ax = points.x[i - 1], ay = points.y[i - 1], dx = points.x[i] - ax, dy = points.y[i] - ay, length = dx * dx + dy * dy;
        const t = length ? Math.max(0, Math.min(1, ((x - ax) * dx + (y - ay) * dy) / length)) : 0;
        const away = (ax + t * dx - x) ** 2 + (ay + t * dy - y) ** 2;
        if (away < closest) [closest, best] = [away, at[i - 1] + t * (at[i] - at[i - 1])];
    }
    return closest <= within * within ? best : undefined;
}
