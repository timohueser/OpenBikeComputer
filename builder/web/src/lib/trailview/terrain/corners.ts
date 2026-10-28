// The corners of the junctions of paved roads. Where two roads leave a shared vertex less than MAX_TURN
// apart, their kerb turns from the edge of the one to the edge of the other round an arc: the stamps paint
// its asphalt and the sidewalk round it and hold their ground, and the slabs follow it. A corner that
// another line's band reaches into stays sharp, so its paint covers no other way or water.
import { crossSlope, metresPerUnit, PAVED_CODES, RAIL, ROAD, ROAD_CODE, walkOf, type Line } from './bins';
import { gridOf, LEFT, near, RIGHT, type Grid } from './town';

/**
 * The kerb's radius, at least LEAST_M; the most that an end of its arc lies from the junction along each
 * road, which runs on within STRAIGHT_M of straight up to there. A corner changes the ground only inside
 * it, within about 20 m of its junction with a mesh cell, so every tile that stamps it holds the first
 * TANGENT_M of each road in its 40 m buffer and decides it alike.
 */
const FILLET_M = 5, LEAST_M = 1, TANGENT_M = 10, STRAIGHT_M = 0.3;
/** The widest angle between two roads' ways out of a junction that makes a corner; the steepest grade of its kerb, about the steepest street's. */
const MAX_TURN = (150 * Math.PI) / 180, MAX_GRADE = 0.25;

/**
 * A corner: the centre of its kerb's arc and its ends on the edges of the two roads, in world units, its radius
 * running from the one end's to the other's; at each end the kerb's height, the slope of that road's surface
 * there per metre east and south, and the sidewalk's width; its surface code, and the profile and half-width of
 * its narrower road.
 */
export interface Corner { o: [number, number]; a: [number, number]; b: [number, number]; z: [number, number]; grad: [number, number][]; walk: [number, number]; code: number; profile: number; h: number }

/** A road's way out of its vertex v, step 1 or -1 along it: its direction in world units toward its point TANGENT_M on, and the metres that it runs on within STRAIGHT_M of that. */
interface Way { line: Line; v: number; step: number; ux: number; uy: number; angle: number; room: number }

const found = new WeakMap<Line[], { corners: Corner[]; grid: Grid }>();
const paired = new WeakMap<Line[], Map<Line, [Line?, Line?]>>();
/** Two ends of roads that meet at most this straight run on into each other: the cosine of the angle between their ways out. */
const RUNS_ON = 0.7;

/** Per road, the roads that its start and its end run on into: where OSM splits a road and no road or track passes through, the two ends that run most nearly straight on. */
export function partnersOf(lines: Line[]): Map<Line, [Line?, Line?]> {
    const known = paired.get(lines);
    if (known) return known;
    const key = (line: Line, v: number) => `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;
    const ends = new Map<string, { line: Line; end: 0 | 1 }[]>(), inner = new Set<string>();
    for (const line of lines) {
        if (line.kind !== ROAD && line.kind !== RAIL) continue;
        const n = line.z.length;
        for (let v = 1; v + 1 < n; v++) inner.add(key(line, v));
        if (line.kind === ROAD) for (const end of [0, 1] as const) ends.set(key(line, end ? n - 1 : 0), [...(ends.get(key(line, end ? n - 1 : 0)) ?? []), { line, end }]);
    }
    const partner = new Map<Line, [Line?, Line?]>();
    for (const [at, here] of ends) {
        if (inner.has(at) || here.length < 2) continue;
        const away = ({ line, end }: (typeof here)[number]) => {
            const n = line.z.length, [v, w] = end ? [n - 1, n - 2] : [0, 1], dx = line.xy[2 * w] - line.xy[2 * v], dy = line.xy[2 * w + 1] - line.xy[2 * v + 1], l = Math.hypot(dx, dy) || 1;
            return [dx / l, dy / l];
        };
        let best: [number, number] | undefined, least = RUNS_ON;
        for (let i = 0; i < here.length; i++) for (let k = i + 1; k < here.length; k++) {
            const [p, q] = [away(here[i]), away(here[k])], cos = p[0] * q[0] + p[1] * q[1];
            if (cos < least) [least, best] = [cos, [i, k]];
        }
        if (!best) continue;
        for (const [a, b] of [[best[0], best[1]], [best[1], best[0]]]) (partner.get(here[a].line) ?? partner.set(here[a].line, [undefined, undefined]).get(here[a].line)!)[here[a].end] = here[b].line;
    }
    paired.set(lines, partner);
    return partner;
}

/** A road's half-width at its vertex v: halfway to its partner's at an end that runs on into one, so the paint's and the slab's edges run on across the joint. */
export function halfAt(line: Line, v: number, partners: Map<Line, [Line?, Line?]>): number {
    const other = v === 0 ? partners.get(line)?.[0] : v === line.z.length - 1 ? partners.get(line)?.[1] : undefined;
    return other ? (line.h + other.h) / 2 : line.h;
}

export function cornersOf(lines: Line[]): Corner[] {
    return cornersIn(lines).corners;
}

/** The corner of a tile's lines that a point lies in, if any. */
export function cornerAt(lines: Line[], x: number, y: number): Corner | undefined {
    const { corners, grid } = cornersIn(lines);
    let hit: Corner | undefined;
    near(grid, x, y, x, y, 0, k => (hit ??= inCorner(corners[k], x, y) ? corners[k] : undefined));
    return hit;
}

function cornersIn(lines: Line[]): { corners: Corner[]; grid: Grid } {
    const known = found.get(lines);
    if (known) return known;
    const at = new Map<string, { line: Line; v: number }[]>();
    for (const line of lines) {
        if (line.kind !== ROAD || !PAVED_CODES.includes(line.code)) continue;
        for (let v = 0; v < line.z.length; v++) {
            const key = `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;
            (at.get(key) ?? at.set(key, []).get(key)!).push({ line, v });
        }
    }
    const all = segmentsOf(lines), partners = partnersOf(lines), corners: Corner[] = [];
    for (const here of at.values()) {
        if (here.every(({ line }) => line === here[0].line)) continue;
        const out = here.flatMap(({ line, v }) => [-1, 1].filter(step => v + step >= 0 && v + step < line.z.length).map(step => wayOut(line, v, step))).sort((p, q) => p.angle - q.angle);
        out.forEach((p, i) => {
            const q = out[(i + 1) % out.length], turn = q.angle - p.angle + (i + 1 === out.length ? 2 * Math.PI : 0);
            const corner = turn < MAX_TURN ? cornerOf(p, q, partners) : undefined;
            if (corner && !reached(all, corner, p.line, q.line)) corners.push(corner);
        });
    }
    const grid = gridOf(corners.map(boxOf));
    found.set(lines, { corners, grid });
    return { corners, grid };
}

/** Every segment of every line, and their boxes in a grid. */
interface Segments { list: [Line, number][]; grid: Grid }

function segmentsOf(lines: Line[]): Segments {
    const list: [Line, number][] = [], boxes: number[][] = [];
    for (const line of lines) for (let v = 0; v + 1 < line.z.length; v++) {
        const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), pad = line.h / metresPerUnit(ay);
        list.push([line, v]);
        boxes.push([Math.min(ax, bx) - pad, Math.min(ay, by) - pad, Math.max(ax, bx) + pad, Math.max(ay, by) + pad]);
    }
    return { list, grid: gridOf(boxes) };
}

/** Whether the band of a line but the corner's own two reaches into it off both roads, at points about a metre apart along and across each segment. */
function reached({ list, grid }: Segments, corner: Corner, ...own: Line[]): boolean {
    const unit = metresPerUnit(corner.o[1]), [x0, y0, x1, y1] = boxOf(corner);
    let hit = false;
    near(grid, x0, y0, x1, y1, 0, k => {
        const [line, v] = list[k];
        if (hit || own.includes(line)) return;
        const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), dx = bx - ax, dy = by - ay, l = Math.hypot(dx, dy) || 1;
        const along = Math.ceil(l * unit), across = Math.max(1, Math.ceil(2 * line.h));
        for (let i = 0; i <= along && !hit; i++) for (let j = 0; j <= across && !hit; j++) {
            const t = i / along, side = ((2 * j) / across - 1) * line.h / unit / l;
            hit = inCorner(corner, ax + t * dx - dy * side, ay + t * dy + dx * side, 0);
        }
    });
    return hit;
}

function wayOut(line: Line, v: number, step: number): Way {
    const x = line.xy[2 * v], y = line.xy[2 * v + 1], unit = metresPerUnit(y), n = line.z.length;
    let k = v + step, length = 0, tx = line.xy[2 * k], ty = line.xy[2 * k + 1];
    for (; k >= 0 && k < n; k += step) {
        const piece = Math.hypot(line.xy[2 * k] - line.xy[2 * k - 2 * step], line.xy[2 * k + 1] - line.xy[2 * k + 1 - 2 * step]) * unit;
        const u = Math.min(1, (TANGENT_M - length) / (piece || 1));
        tx = line.xy[2 * k - 2 * step] + u * (line.xy[2 * k] - line.xy[2 * k - 2 * step]);
        ty = line.xy[2 * k + 1 - 2 * step] + u * (line.xy[2 * k + 1] - line.xy[2 * k + 1 - 2 * step]);
        if ((length += piece) >= TANGENT_M) break;
    }
    const l = Math.hypot(tx - x, ty - y) || 1, ux = (tx - x) / l, uy = (ty - y) / l;
    let room = 0;
    for (k = v + step; k >= 0 && k < n && room < TANGENT_M; k += step) {
        const dx = (line.xy[2 * k] - x) * unit, dy = (line.xy[2 * k + 1] - y) * unit;
        if (Math.abs(dx * uy - dy * ux) > STRAIGHT_M) break;
        room = Math.min(TANGENT_M, dx * ux + dy * uy);
    }
    return { line, v, step, ux, uy, angle: Math.atan2(uy, ux), room };
}

/** The corner from way p to way q, which turns toward it, with the arc's radius cut to keep its ends where the roads run straight from the junction. */
function cornerOf(p: Way, q: Way, partners: Map<Line, [Line?, Line?]>): Corner | undefined {
    const x = p.line.xy[2 * p.v], y = p.line.xy[2 * p.v + 1], unit = metresPerUnit(y);
    // The normals toward the corner, and the centre of the arc of radius r as metres from the junction.
    const np = [-p.uy, p.ux], nq = [q.uy, -q.ux], det = np[0] * nq[1] - np[1] * nq[0];
    // Ways that overlap meet in no corner.
    if (!det) return undefined;
    const centre = (r: number) => {
        const dp = p.line.h + r, dq = q.line.h + r;
        return [(dp * nq[1] - dq * np[1]) / det, (dq * np[0] - dp * nq[0]) / det];
    };
    const along = (r: number, way: Way, n: number[]) => {
        const o = centre(r);
        return (o[0] - n[0] * r) * way.ux + (o[1] - n[1] * r) * way.uy;
    };
    let r = FILLET_M;
    for (const [way, n] of [[p, np], [q, nq]] as const) {
        const room = way.room, a0 = along(0, way, n), slope = along(1, way, n) - a0;
        if (a0 + slope * r > room) r = (room - a0) / slope;
    }
    if (!(r >= LEAST_M)) return undefined;
    const o = centre(r);
    // An end of the arc: the foot of its centre on its road's edge, where the arc meets the edge square and
    // the road's slab stops across it; with the kerb's height, the surface's slope and the sidewalk's width
    // there, as that slab and the road's core have them.
    const end = ({ line, v, step }: Way, n: number[]): [[number, number], number, [number, number], number] => {
        const ox = x + o[0] / unit, oy = y + o[1] / unit;
        let s = Math.min(v, v + step), u = 0, dx = 0, dy = 0;
        for (; s >= 0 && s + 1 < line.z.length; s += step) {
            dx = line.xy[2 * s + 2] - line.xy[2 * s];
            dy = line.xy[2 * s + 3] - line.xy[2 * s + 1];
            u = ((ox - line.xy[2 * s]) * dx + (oy - line.xy[2 * s + 1]) * dy) / (dx * dx + dy * dy || 1);
            if (step > 0 ? u <= 1 || s + 2 === line.z.length : u >= 0 || s === 0) break;
        }
        u = Math.min(1, Math.max(0, u));
        const h = halfAt(line, s, partners) + (halfAt(line, s + 1, partners) - halfAt(line, s, partners)) * u;
        const l = Math.hypot(dx, dy) || 1, side = dx * n[1] - dy * n[0] > 0 ? RIGHT : LEFT, sign = side === LEFT ? 1 : -1, across = (sign * h) / unit / l;
        const at: [number, number] = [Math.round(line.xy[2 * s] + u * dx + dy * across), Math.round(line.xy[2 * s + 1] + u * dy - dx * across)];
        const tilt = crossSlope(line, s, u * l * unit, (1 - u) * l * unit), grade = (line.z[s + 1] - line.z[s]) / (l * unit);
        const z = line.z[s] + (line.z[s + 1] - line.z[s]) * u - sign * h * tilt;
        // Up the segment by its grade, and down to its left by its tilt.
        return [at, z, [(grade * dx - tilt * dy) / l, (grade * dy + tilt * dx) / l], (line.sidewalks?.[s] ?? 0) & side ? walkOf(line) : 0];
    };
    const [a, za, ga, wa] = end(p, np), [b, zb, gb, wb] = end(q, nq), narrow = p.line.h <= q.line.h ? p.line : q.line;
    // Roads whose kerbs climb past MAX_GRADE from one end to the other lie at two levels, as at a ramp: no corner.
    if (Math.abs(za - zb) > MAX_GRADE * Math.hypot(a[0] - b[0], a[1] - b[1]) * unit) return undefined;
    return {
        o: [Math.round(x + o[0] / unit), Math.round(y + o[1] / unit)], a, b, z: [za, zb], grad: [ga, gb], walk: [wa, wb],
        code: p.line.code === q.line.code ? p.line.code : ROAD_CODE, profile: narrow.profile, h: narrow.h,
    };
}

/**
 * The height of a corner's ground at a point: each road's surface carried on from its end to the point, by the
 * point's distances past each road's sidewalk from the corner's two sides: the first road's, from the centre to
 * its end and on along its edge to the roads' corner, and the second's. So the ground is each road's own within
 * its sidewalk's width of its side, where that road's core holds it too.
 */
export function heightAt({ o, a, b, z, grad, walk }: Corner, x: number, y: number): number {
    const unit = metresPerUnit(a[1]), ea = [a[0] - o[0], a[1] - o[1]], eb = [b[0] - o[0], b[1] - o[1]], e = [x - o[0], y - o[1]];
    const s = ((eb[0] - ea[0]) * eb[0] + (eb[1] - ea[1]) * eb[1]) / (-ea[1] * eb[0] + ea[0] * eb[1]), c = [ea[0] - s * ea[1], ea[1] + s * ea[0]];
    const gap = (p: number[], q: number[]) => {
        const t = Math.min(1, Math.max(0, ((e[0] - p[0]) * (q[0] - p[0]) + (e[1] - p[1]) * (q[1] - p[1])) / ((q[0] - p[0]) ** 2 + (q[1] - p[1]) ** 2 || 1)));
        return Math.hypot(e[0] - p[0] - t * (q[0] - p[0]), e[1] - p[1] - t * (q[1] - p[1]));
    };
    const da = Math.max(0, Math.min(gap([0, 0], ea), gap(ea, c)) * unit - walk[0]), db = Math.max(0, Math.min(gap(c, eb), gap(eb, [0, 0])) * unit - walk[1]);
    const w = da + db > 0 ? da / (da + db) : 0.5;
    const za = z[0] + (grad[0][0] * (x - a[0]) + grad[0][1] * (y - a[1])) * unit;
    return za + (z[1] + (grad[1][0] * (x - b[0]) + grad[1][1] * (y - b[1])) * unit - za) * w;
}

/** The box of a corner as inCorner has it: its arc's centre, its ends moved its reach onto the roads, and where the edges through those meet. */
export function boxOf({ o, a, b, h }: Corner): number[] {
    const reach = h / metresPerUnit(o[1]), out = (end: number[]) => {
        const e = [end[0] - o[0], end[1] - o[1]], k = 1 + reach / Math.hypot(e[0], e[1]);
        return [o[0] + k * e[0], o[1] + k * e[1]];
    };
    const [pa, pb] = [out(a), out(b)], da = [o[1] - pa[1], pa[0] - o[0]], eb = [pb[0] - o[0], pb[1] - o[1]];
    const t = ((pb[0] - pa[0]) * eb[0] + (pb[1] - pa[1]) * eb[1]) / (da[0] * eb[0] + da[1] * eb[1]);
    const xs = [o[0], pa[0], pb[0], pa[0] + t * da[0]], ys = [o[1], pa[1], pb[1], pa[1] + t * da[1]];
    return [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)];
}

/**
 * Whether a point lies in a corner: between the ends of its arc seen from its centre, and off both roads or
 * less than the narrower road's half-width onto them, where the arc is the nearest kerb. So no road's edge
 * that the arc replaces paints or holds a slab where a road bends inside the corner.
 */
export function inCorner({ o, a, b, h }: Corner, x: number, y: number, onto = h): boolean {
    const ea = [a[0] - o[0], a[1] - o[1]], eb = [b[0] - o[0], b[1] - o[1]], e = [x - o[0], y - o[1]], reach = onto / metresPerUnit(o[1]);
    const t = Math.atan2(ea[0] * e[1] - ea[1] * e[0], ea[0] * e[0] + ea[1] * e[1]) / Math.atan2(ea[0] * eb[1] - ea[1] * eb[0], ea[0] * eb[0] + ea[1] * eb[1]);
    const past = (end: number[]) => ((e[0] - end[0]) * end[0] + (e[1] - end[1]) * end[1]) / Math.hypot(end[0], end[1]);
    return t >= 0 && t <= 1 && past(ea) <= reach && past(eb) <= reach;
}
