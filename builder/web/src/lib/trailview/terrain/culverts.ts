// A stream and the ways that pass over it. A short culvert joins the stream on either side of it. A long culvert, or
// one that runs along a way, buries the stream between its ends. A line that ends just short of a way runs on to it:
// OSM often stops a stream at a road's ditch. Where a stream crosses a road or a railway, or meets a buried culvert,
// its ribbon ends at a mouth's wall, front along the stream, where the wall clears every way. A foot way has no wall.
import type { Feature } from '../stream/features';
import { metresPerUnit, PATH, RAIL, RIVER, ROAD, STREAM, walkOf, type Line } from './bins';
import { cornerAt, cornersOf } from './corners';
import { along, gridOf, LEFT, near, RIGHT } from './town';

/** A ribbon stops this far past a way's edge or its sidewalk, as GIVE_WAY_M of water.wgsl: the widest shoulder, 0.6 m, a mouth's wall behind its origin, and 0.06 m. */
export const GIVE_WAY_M = 1.2;
/** A mouth's wall: its half-width and how far it lies behind its origin, as culvertMouth of models/amenities.ts. */
export const MOUTH_HALF_M = 1.3, MOUTH_BACK_M = 0.54;
/** A wall stands at least this far past every way's edge: the widest shoulder, SHOULDER_M of bins.ts. */
const WALL_CLEAR_M = 0.6;
/** A culvert of a stream up to this long can carry its ribbon: 56 of the 63 of the fixture have up to 52 m, the next 72 m. */
const CULVERT_M = 60;
/** A culvert with this many metres along a way buries the stream: on the fixture, 11 have 3 m or more and the others 0.5 m at most. */
const ALONG_M = 2;
/** How near a way a culvert runs along it, past the edge; and how straight, the cosine of the angle between them. */
const BESIDE_M = 3, BESIDE_COS = 0.7;
/** A line's end this near a way, past its edge, runs on to the way: a few metres, about what a road's ditch and its verge take. */
const RUN_ON_M = 5;
/** How far `waysOf` reads past a way's edge. */
const GAP_REACH_M = 8;
/** A mouth is looked for at this step along a stream, up to WALK_M from a crossing or a buried culvert's end. */
const STEP_M = 0.1, WALK_M = 16;
/** A stretch of ribbon shorter than this between walls, or past a wall to the line's end, stays buried. */
const LEAST_RIBBON_M = 3;

/** A mouth of a culvert, where a ribbon ends: its world point, and the unit direction of its front, along the stream away from the culvert, in world axes. */
export interface CulvertMouth { x: number; y: number; ux: number; uy: number }

/** The metres from a point to the nearest way on the ground, past its edge or its sidewalk; the foot on that way; and the way's unit direction. */
export interface WayGap { metres: number; x: number; y: number; ux: number; uy: number; line: Line }

/** What a stream asks of a tile's ways on the ground. */
interface Ways { gap: (x: number, y: number) => WayGap | undefined; crossings: (ax: number, ay: number, bx: number, by: number) => number[] }

const known = new WeakMap<Line[], Ways>();

/**
 * A tile's ways on the ground: a bridge and a tunnel leave the ground to the stream, and a ford lets it pass over.
 * `gap` reads the nearest within GAP_REACH_M of a world point. `crossings` gives the shares along a segment where it
 * crosses the middle of a road or a railway.
 */
export function waysOf(lines: Line[]): Ways {
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
    const gap = (x: number, y: number) => {
        let best: WayGap | undefined;
        near(grid, x, y, x, y, 0, s => {
            const [line, v] = segments[s], [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), t = along(ax, ay, bx, by, x, y), l = Math.hypot(bx - ax, by - ay) || 1;
            const px = ax + t * (bx - ax), py = ay + t * (by - ay), ux = (bx - ax) / l, uy = (by - ay) / l;
            // Left of a way whose y runs south, as clearOf of bins.ts.
            const walked = (line.sidewalks?.[v] ?? 0) & ((x - px) * uy - (y - py) * ux > 0 ? LEFT : RIGHT);
            const metres = Math.hypot(x - px, y - py) * metresPerUnit(y) - line.h - (line.plan?.widen[v] ?? 0) - (walked ? walkOf(line) : 0);
            if (metres < GAP_REACH_M && metres < (best?.metres ?? Infinity)) best = { metres, x: px, y: py, ux, uy, line };
        });
        return best;
    };
    const crossings = (ax: number, ay: number, bx: number, by: number) => {
        const shares: number[] = [], dx = bx - ax, dy = by - ay;
        near(grid, ax, ay, bx, by, 0, s => {
            const [line, v] = segments[s];
            if (line.kind === PATH) return;
            const [cx, cy, ex, ey] = line.xy.slice(2 * v, 2 * v + 4), wx = ex - cx, wy = ey - cy, d = dx * wy - dy * wx;
            if (!d) return;
            const t = ((cx - ax) * wy - (cy - ay) * wx) / d, u = ((cx - ax) * dy - (cy - ay) * dx) / d;
            if (t >= 0 && t < 1 && u >= 0 && u < 1) shares.push(t);
        });
        return shares;
    };
    const ways = { gap, crossings };
    known.set(lines, ways);
    return ways;
}

/** The metres of a run of world points that lie along a way, beside it. */
function metresAlong(xy: number[], gapAt: Ways['gap']): number {
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
 * it keeps the level before it. Returns the names of the points where the other culverts end, as `pointName` gives them.
 */
export function joinCulverts(list: Feature[], lines: Line[], world: (xy: ArrayLike<number>, v: number) => number[]): Set<string> {
    const last = (xy: number[]) => xy.length / 2 - 1, same = (a: number[], v: number, b: number[], w: number) => a[2 * v] === b[2 * w] && a[2 * v + 1] === b[2 * w + 1];
    const endingAt = (xy: number[], v: number) => lines.find(line => line.kind === STREAM && same(line.xy, last(line.xy), xy, v));
    const startingAt = (xy: number[], v: number) => lines.find(line => line.kind === STREAM && same(line.xy, 0, xy, v));
    const gapAt = waysOf(lines).gap, buried = new Set<string>();
    for (const { kind, tags, parts } of list) {
        if (kind !== 'stream' || (tags.tunnel ?? 'no') === 'no') continue;
        for (const { xy: part, z } of parts) {
            if (z?.some(Number.isFinite)) continue;
            let xy = Array.from({ length: part.length / 2 }, (_, v) => world(part, v)).flat(), metres = 0;
            for (let v = 0; v < last(xy); v++) metres += Math.hypot(xy[2 * v + 2] - xy[2 * v], xy[2 * v + 3] - xy[2 * v + 1]) * metresPerUnit(xy[2 * v + 1]);
            if (metres > CULVERT_M || metresAlong(xy, gapAt) >= ALONG_M) {
                for (const v of [0, last(xy)]) buried.add(pointName(xy[2 * v], xy[2 * v + 1]));
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
    return buried;
}

/** Runs each end of a stream that no other water meets and no culvert takes on to a way ahead of it within RUN_ON_M: the ribbon stops at the way. */
function runOnToWays(lines: Line[], buried: Set<string>, gapAt: Ways['gap']): void {
    const meets = new Map<string, number>();
    for (const { kind, xy } of lines) {
        if (kind !== RIVER && kind !== STREAM) continue;
        for (let i = 0; i < xy.length; i += 2) meets.set(pointName(xy[i], xy[i + 1]), (meets.get(pointName(xy[i], xy[i + 1])) ?? 0) + 1);
    }
    for (const line of lines) {
        if (line.kind !== STREAM) continue;
        for (const start of [true, false]) {
            const n = line.z.length, [v, w] = start ? [0, 1] : [n - 1, n - 2], x = line.xy[2 * v], y = line.xy[2 * v + 1], gap = gapAt(x, y), name = pointName(x, y);
            if (!gap || gap.metres >= RUN_ON_M || gap.metres < 0 || meets.get(name)! > 1 || buried.has(name)) continue;
            // Only a way ahead of the line: a stream that starts beside a road and leaves it starts there.
            const ox = x - line.xy[2 * w], oy = y - line.xy[2 * w + 1], fx = gap.x - x, fy = gap.y - y;
            if (ox * fx + oy * fy <= BESIDE_COS * Math.hypot(ox, oy) * Math.hypot(fx, fy)) continue;
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

/** A stretch of a stream under a way or a culvert, in metres along its line, and whether a wall stands at each end. */
interface Cut { from: number; to: number; wallFrom: boolean; wallTo: boolean }

/** Runs each stream on to the ways ahead of it, ends its ribbon at a mouth on each side of a road or a railway that it crosses and at each buried culvert, and returns the mouths. */
export function standMouths(lines: Line[], buried: Set<string>): CulvertMouth[] {
    // A corner's shape holds for the lines as they were joined.
    cornersOf(lines);
    const { gap, crossings } = waysOf(lines), mouths: CulvertMouth[] = [];
    runOnToWays(lines, buried, gap);
    for (const line of lines.filter(({ kind }) => kind === STREAM)) {
        const metres = [0];
        for (let v = 0; v + 1 < line.z.length; v++) metres.push(metres[v] + Math.hypot(line.xy[2 * v + 2] - line.xy[2 * v], line.xy[2 * v + 3] - line.xy[2 * v + 1]) * metresPerUnit(line.xy[2 * v + 1]));
        const total = metres[metres.length - 1];
        /** The point `s` metres along, its height, and the unit direction of its segment. */
        const at = (s: number) => {
            let v = 0;
            while (v + 2 < metres.length && metres[v + 1] < s) v++;
            const t = (s - metres[v]) / (metres[v + 1] - metres[v] || 1), [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), l = Math.hypot(bx - ax, by - ay) || 1;
            return { x: ax + (bx - ax) * t, y: ay + (by - ay) * t, z: line.z[v] + (line.z[v + 1] - line.z[v]) * t, ux: (bx - ax) / l, uy: (by - ay) / l };
        };
        /** Whether a wall with its origin at `s` and its front toward `sign` along the line stands clear of every way. */
        const clearAt = (s: number, sign: number) => {
            const { x, y, ux, uy } = at(s), unit = metresPerUnit(y), fx = sign * ux, fy = sign * uy;
            if ((gap(x, y)?.metres ?? Infinity) < GIVE_WAY_M) return false;
            for (const across of [-MOUTH_HALF_M, 0, MOUTH_HALF_M]) for (const back of [0, MOUTH_BACK_M]) {
                const px = x + (-fy * across - fx * back) / unit, py = y + (fx * across - fy * back) / unit;
                if ((gap(px, py)?.metres ?? Infinity) < WALL_CLEAR_M || cornerAt(lines, px, py)) return false;
            }
            return true;
        };
        /** The first clear place for a wall from `s` toward `sign` along the line. */
        const walk = (s: number, sign: number) => {
            for (let d = 0; d <= WALK_M; d += STEP_M) {
                const here = s + sign * d;
                if (here < 0 || here > total) return undefined;
                if (clearAt(here, sign)) return here;
            }
            return undefined;
        };
        const cuts: Cut[] = [];
        // Ends: a buried culvert's, and one on a road or a railway.
        for (const [s, sign, v] of [[0, 1, 0], [total, -1, line.z.length - 1]]) {
            const x = line.xy[2 * v], y = line.xy[2 * v + 1], under = gap(x, y), end = buried.has(pointName(x, y));
            if (!end && !(under && under.metres < 0 && under.line.kind !== PATH)) continue;
            const place = walk(s, sign);
            if (place === undefined) continue;
            cuts.push(sign > 0 ? { from: 0, to: place, wallFrom: false, wallTo: true } : { from: place, to: total, wallFrom: true, wallTo: false });
        }
        for (let v = 0; v + 1 < line.z.length; v++) {
            for (const t of crossings(line.xy[2 * v], line.xy[2 * v + 1], line.xy[2 * v + 2], line.xy[2 * v + 3])) {
                const s = metres[v] + t * (metres[v + 1] - metres[v]), from = walk(s, -1), to = walk(s, 1);
                cuts.push({ from: from ?? s, to: to ?? s, wallFrom: from !== undefined, wallTo: to !== undefined });
            }
        }
        if (!cuts.length) continue;
        cuts.sort((a, b) => a.from - b.from);
        const merged: Cut[] = [];
        for (const cut of cuts) {
            const last = merged[merged.length - 1];
            if (last && cut.from - last.to < LEAST_RIBBON_M) {
                if (cut.to >= last.to) [last.to, last.wallTo] = [cut.to, cut.wallTo];
            } else merged.push({ ...cut });
        }
        const first = merged[0], final = merged[merged.length - 1];
        if (first.from < LEAST_RIBBON_M) [first.from, first.wallFrom] = [0, false];
        if (total - final.to < LEAST_RIBBON_M) [final.to, final.wallTo] = [total, false];
        const pieces: Line[] = [];
        let start = 0;
        for (const cut of [...merged, { from: total, to: total }]) {
            if (cut.from > start) {
                const a = at(start), b = at(cut.from), inner = metres.flatMap((m, v) => (m > start && m < cut.from ? [v] : []));
                const xy = [a.x, a.y, ...inner.flatMap(v => [line.xy[2 * v], line.xy[2 * v + 1]]), b.x, b.y];
                pieces.push({ ...line, xy, z: [a.z, ...inner.map(v => line.z[v]), b.z], anchors: line.anchors && Array<boolean>(inner.length + 2).fill(true) });
            }
            start = cut.to;
        }
        for (const { from, to, wallFrom, wallTo } of merged) {
            if (wallFrom) {
                const { x, y, ux, uy } = at(from);
                mouths.push({ x, y, ux: -ux, uy: -uy });
            }
            if (wallTo) {
                const { x, y, ux, uy } = at(to);
                mouths.push({ x, y, ux, uy });
            }
        }
        lines.splice(lines.indexOf(line), 1, ...pieces);
    }
    return mouths;
}

/** The name of a world point of a line. */
function pointName(x: number, y: number): string {
    return `${x},${y}`;
}
