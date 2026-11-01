// A stream and the ways that pass over it. A short culvert joins the stream on either side of it, whose ribbon then
// stops at the way (ribbon_fragment of water.wgsl). A long culvert, or one that runs along a way, buries the stream
// between its ends. A line that ends just short of a way runs on to it: OSM often stops a stream at a road's ditch.
import type { Feature } from '../stream/features';
import { metresPerUnit, PATH, RAIL, RIVER, ROAD, STREAM, walkOf, type Line } from './bins';
import { along, gridOf, LEFT, near, RIGHT } from './town';

/** A ribbon stops this far past a way's edge or its sidewalk, as GIVE_WAY_M of water.wgsl: past a shoulder, with room for a mouth's wall. */
export const GIVE_WAY_M = 0.8;
/** A culvert of a stream up to this long can carry its ribbon: 56 of the 63 of the fixture have up to 52 m, the next 72 m. */
const CULVERT_M = 60;
/** A culvert with this many metres along a way buries the stream: on the fixture, 11 have 3 m or more and the others 0.5 m at most. */
const ALONG_M = 2;
/** How near a way a culvert runs along it, past the edge; and how straight, the cosine of the angle between them. */
const BESIDE_M = 3, BESIDE_COS = 0.7;
/** A line's end this near a way, past its edge, runs on to the way: a few metres, about what a road's ditch and its verge take. */
const RUN_ON_M = 5;
/** How far `wayGaps` reads past a way's edge. */
export const GAP_REACH_M = 8;

/** The end of a culvert that buries a stream, where a ribbon ends: its world point, and the unit direction out of the culvert, in world axes. */
export interface CulvertEnd { x: number; y: number; ux: number; uy: number }

/** The metres from a point to the nearest way on the ground, past its edge or its sidewalk; the foot on that way; and the way's unit direction. */
export interface WayGap { metres: number; x: number; y: number; ux: number; uy: number }

const known = new WeakMap<Line[], (x: number, y: number) => WayGap | undefined>();

/** Reads the nearest way on the ground within GAP_REACH_M of a world point. A bridge and a tunnel leave the ground to the stream, and a ford lets it pass over. */
export function wayGaps(lines: Line[]): (x: number, y: number) => WayGap | undefined {
    const found = known.get(lines);
    if (found) return found;
    const segments: [Line, number][] = [], boxes: number[][] = [];
    for (const line of lines) {
        if ((line.kind !== ROAD && line.kind !== PATH && line.kind !== RAIL) || line.ford) continue;
        for (let v = 0; v + 1 < line.z.length; v++) {
            const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), pad = (line.h + (line.plan?.widen[v] ?? 0) + walkOf(line) + GAP_REACH_M) / metresPerUnit(ay);
            segments.push([line, v]);
            boxes.push([Math.min(ax, bx) - pad, Math.min(ay, by) - pad, Math.max(ax, bx) + pad, Math.max(ay, by) + pad]);
        }
    }
    const grid = gridOf(boxes);
    const gaps = (x: number, y: number) => {
        let best: WayGap | undefined;
        near(grid, x, y, x, y, 0, s => {
            const [line, v] = segments[s], [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), t = along(ax, ay, bx, by, x, y), l = Math.hypot(bx - ax, by - ay) || 1;
            const px = ax + t * (bx - ax), py = ay + t * (by - ay), ux = (bx - ax) / l, uy = (by - ay) / l;
            // Left of a way whose y runs south, as clearOf of bins.ts.
            const walked = (line.sidewalks?.[v] ?? 0) & ((x - px) * uy - (y - py) * ux > 0 ? LEFT : RIGHT);
            const metres = Math.hypot(x - px, y - py) * metresPerUnit(y) - line.h - (line.plan?.widen[v] ?? 0) - (walked ? walkOf(line) : 0);
            if (metres < GAP_REACH_M && metres < (best?.metres ?? Infinity)) best = { metres, x: px, y: py, ux, uy };
        });
        return best;
    };
    known.set(lines, gaps);
    return gaps;
}

/** The metres of a run of world points that lie along a way, beside it. */
function metresAlong(xy: number[], gapAt: ReturnType<typeof wayGaps>): number {
    let sum = 0;
    for (let v = 0; v + 1 < xy.length / 2; v++) {
        const [ax, ay, bx, by] = xy.slice(2 * v, 2 * v + 4), l = Math.hypot(bx - ax, by - ay) * metresPerUnit(ay) || 1, steps = Math.ceil(l / 0.5);
        for (let k = 0; k < steps; k++) {
            const gap = gapAt(ax + ((bx - ax) * k) / steps, ay + ((by - ay) * k) / steps);
            if (gap && gap.metres < BESIDE_M && Math.abs((bx - ax) * gap.ux + (by - ay) * gap.uy) / (l / metresPerUnit(ay)) > BESIDE_COS) sum += l / steps;
        }
    }
    return sum;
}

/**
 * Joins each short culvert of a stream that runs along no way to the stream at its ends, so that the stream runs on
 * to the way that passes over it. A culvert has no water level, and a ribbon needs one only for its width on screen:
 * it keeps the level before it. Returns the ends of the other culverts, where a stream meets them.
 */
export function joinCulverts(list: Feature[], lines: Line[], world: (xy: ArrayLike<number>, v: number) => number[]): CulvertEnd[] {
    const last = (xy: number[]) => xy.length / 2 - 1, same = (a: number[], v: number, b: number[], w: number) => a[2 * v] === b[2 * w] && a[2 * v + 1] === b[2 * w + 1];
    const endingAt = (xy: number[], v: number) => lines.find(line => line.kind === STREAM && same(line.xy, last(line.xy), xy, v));
    const startingAt = (xy: number[], v: number) => lines.find(line => line.kind === STREAM && same(line.xy, 0, xy, v));
    const gapAt = wayGaps(lines), ends: CulvertEnd[] = [];
    for (const { kind, tags, parts } of list) {
        if (kind !== 'stream' || (tags.tunnel ?? 'no') === 'no') continue;
        for (const { xy: part, z } of parts) {
            if (z?.some(Number.isFinite)) continue;
            let xy = Array.from({ length: part.length / 2 }, (_, v) => world(part, v)).flat(), metres = 0;
            for (let v = 0; v < last(xy); v++) metres += Math.hypot(xy[2 * v + 2] - xy[2 * v], xy[2 * v + 3] - xy[2 * v + 1]) * metresPerUnit(xy[2 * v + 1]);
            if (metres > CULVERT_M || metresAlong(xy, gapAt) >= ALONG_M) {
                for (const [v, w] of [[0, 1], [last(xy), last(xy) - 1]]) {
                    if (!endingAt(xy, v) && !startingAt(xy, v)) continue;
                    const dx = xy[2 * v] - xy[2 * w], dy = xy[2 * v + 1] - xy[2 * w + 1], l = Math.hypot(dx, dy) || 1;
                    ends.push({ x: xy[2 * v], y: xy[2 * v + 1], ux: dx / l, uy: dy / l });
                }
                continue;
            }
            // OSM draws a culvert as it likes, and the bake turns only the ways that it solves.
            if (!endingAt(xy, 0) && !startingAt(xy, last(xy))) xy = Array.from({ length: xy.length / 2 }, (_, v) => [xy[xy.length - 2 * v - 2], xy[xy.length - 2 * v - 1]]).flat();
            const up = endingAt(xy, 0), down = startingAt(xy, last(xy));
            if (!up && !down) continue;
            const line = up ?? { ...down!, xy: xy.slice(0, 2), z: [down!.z[0]], anchors: [true] }, level = line.z[line.z.length - 1];
            const append = (more: number[], heights: number[]) => {
                line.xy.push(...more);
                line.z.push(...heights);
                line.anchors?.push(...heights.map(() => true));
            };
            append(xy.slice(2), Array<number>(last(xy)).fill(level));
            if (down && down !== line) {
                append(down.xy.slice(2), down.z.slice(1).map(z => Math.min(z, level)));
                lines.splice(lines.indexOf(down), 1);
            }
            if (!up) lines.push(line);
        }
    }
    return ends;
}

/** Runs each end of a stream that no other water meets and no culvert takes on to a way ahead of it within RUN_ON_M: the ribbon stops at the way. */
export function runOnToWays(lines: Line[], ends: CulvertEnd[]): void {
    const meets = new Map<string, number>(), name = (x: number, y: number) => `${x},${y}`, gapAt = wayGaps(lines);
    for (const { kind, xy } of lines) {
        if (kind !== RIVER && kind !== STREAM) continue;
        for (let i = 0; i < xy.length; i += 2) meets.set(name(xy[i], xy[i + 1]), (meets.get(name(xy[i], xy[i + 1])) ?? 0) + 1);
    }
    const taken = new Set(ends.map(({ x, y }) => name(x, y)));
    for (const line of lines) {
        if (line.kind !== STREAM) continue;
        for (const start of [true, false]) {
            const n = line.z.length, [v, w] = start ? [0, 1] : [n - 1, n - 2], x = line.xy[2 * v], y = line.xy[2 * v + 1], gap = gapAt(x, y);
            if (!gap || gap.metres >= RUN_ON_M || meets.get(name(x, y))! > 1 || taken.has(name(x, y))) continue;
            // Only ahead of the line: a stream that starts beside a road and leaves it starts there.
            if ((x - line.xy[2 * w]) * (gap.x - x) + (y - line.xy[2 * w + 1]) * (gap.y - y) <= 0) continue;
            if (start) {
                line.xy.unshift(gap.x, gap.y);
                line.z.unshift(line.z[0]);
                line.anchors?.unshift(true);
            } else {
                line.xy.push(gap.x, gap.y);
                line.z.push(line.z[n - 1]);
                line.anchors?.push(true);
            }
        }
    }
}
