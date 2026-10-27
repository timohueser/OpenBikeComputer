// The corners of the junctions of paved roads. Where two roads leave a shared vertex less than MAX_TURN
// apart, their kerb turns from the edge of the one to the edge of the other round an arc: the stamps paint
// its asphalt and the sidewalk round it and hold their ground, and the slabs follow it.
import { crossSlope, metresPerUnit, PROFILES, ROAD, type Line } from './bins';
import { LEFT, RIGHT } from './town';

/**
 * The kerb's radius, at least LEAST_M; the most that an end of its arc lies from the junction along each
 * road, which runs on within STRAIGHT_M of straight up to there. A corner changes the ground only inside
 * it, within about 20 m of its junction with a mesh cell, so every tile that stamps it holds the first
 * TANGENT_M of each road in its 40 m buffer and decides it alike.
 */
const FILLET_M = 5, LEAST_M = 1, TANGENT_M = 10, STRAIGHT_M = 0.3;
/** The widest angle between two roads' ways out of a junction that makes a corner. */
const MAX_TURN = (150 * Math.PI) / 180;
/** The surface codes of the roads with corners, main, road and oneway, and the code of a corner between two of them. */
const PAVED = [1, 2, 7], ROAD_CODE = 2;

/** A corner: the centre of its kerb's arc and its ends on the edges of the two roads, in world units, its radius running from the one end's to the other's; at each end the kerb's height and the sidewalk's width; its surface code, and the profile and half-width of its narrower road. */
export interface Corner { o: [number, number]; a: [number, number]; b: [number, number]; z: [number, number]; walk: [number, number]; code: number; profile: number; h: number }

/** A road's way out of its vertex v, step 1 or -1 along it: its direction in world units toward its point TANGENT_M on, and the metres that it runs on within STRAIGHT_M of that. */
interface Way { line: Line; v: number; step: number; ux: number; uy: number; angle: number; room: number }

const found = new WeakMap<Line[], Corner[]>();

export function cornersOf(lines: Line[]): Corner[] {
    let corners = found.get(lines);
    if (corners) return corners;
    const at = new Map<string, { line: Line; v: number }[]>();
    for (const line of lines) {
        if (line.kind !== ROAD || !PAVED.includes(line.code)) continue;
        for (let v = 0; v < line.z.length; v++) {
            const key = `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;
            (at.get(key) ?? at.set(key, []).get(key)!).push({ line, v });
        }
    }
    corners = [];
    for (const here of at.values()) {
        if (here.every(({ line }) => line === here[0].line)) continue;
        const out = here.flatMap(({ line, v }) => [-1, 1].filter(step => v + step >= 0 && v + step < line.z.length).map(step => wayOut(line, v, step))).sort((p, q) => p.angle - q.angle);
        out.forEach((p, i) => {
            const q = out[(i + 1) % out.length], turn = q.angle - p.angle + (i + 1 === out.length ? 2 * Math.PI : 0);
            const corner = turn < MAX_TURN ? cornerOf(p, q) : undefined;
            if (corner) corners!.push(corner);
        });
    }
    found.set(lines, corners);
    return corners;
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
function cornerOf(p: Way, q: Way): Corner | undefined {
    const x = p.line.xy[2 * p.v], y = p.line.xy[2 * p.v + 1], unit = metresPerUnit(y);
    // The normals toward the corner, and the centre of the arc of radius r as metres from the junction.
    const np = [-p.uy, p.ux], nq = [q.uy, -q.ux], det = np[0] * nq[1] - np[1] * nq[0];
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
    // An end of the arc on its road's edge beside the end on the straight way, with the kerb's height and the
    // sidewalk's width there, as the slab of that segment has them.
    const end = ({ line, v, step }: Way, n: number[]): [[number, number], number, number] => {
        const tx = x + (o[0] - n[0] * r) / unit, ty = y + (o[1] - n[1] * r) / unit;
        let s = Math.min(v, v + step), u = 0, dx = 0, dy = 0;
        for (; s >= 0 && s + 1 < line.z.length; s += step) {
            dx = line.xy[2 * s + 2] - line.xy[2 * s];
            dy = line.xy[2 * s + 3] - line.xy[2 * s + 1];
            u = ((tx - line.xy[2 * s]) * dx + (ty - line.xy[2 * s + 1]) * dy) / (dx * dx + dy * dy || 1);
            if (step > 0 ? u <= 1 || s + 2 === line.z.length : u >= 0 || s === 0) break;
        }
        u = Math.min(1, Math.max(0, u));
        const l = Math.hypot(dx, dy) || 1, side = dx * n[1] - dy * n[0] > 0 ? RIGHT : LEFT, sign = side === LEFT ? 1 : -1, across = (sign * line.h) / unit / l;
        const at: [number, number] = [Math.round(line.xy[2 * s] + u * dx + dy * across), Math.round(line.xy[2 * s + 1] + u * dy - dx * across)];
        const z = line.z[s] + (line.z[s + 1] - line.z[s]) * u - sign * line.h * crossSlope(line, s, u * l * unit, (1 - u) * l * unit);
        return [at, z, (line.sidewalks?.[s] ?? 0) & side ? Object.values(PROFILES)[line.profile].walk : 0];
    };
    const [a, za, wa] = end(p, np), [b, zb, wb] = end(q, nq), narrow = p.line.h <= q.line.h ? p.line : q.line;
    return {
        o: [Math.round(x + o[0] / unit), Math.round(y + o[1] / unit)], a, b, z: [za, zb], walk: [wa, wb],
        code: p.line.code === q.line.code ? p.line.code : ROAD_CODE, profile: narrow.profile, h: narrow.h,
    };
}

/** Whether a point lies in a corner: between the ends of its arc seen from its centre, and off both roads. */
export function inCorner({ o, a, b }: Corner, x: number, y: number): boolean {
    const ea = [a[0] - o[0], a[1] - o[1]], eb = [b[0] - o[0], b[1] - o[1]], e = [x - o[0], y - o[1]];
    const t = Math.atan2(ea[0] * e[1] - ea[1] * e[0], ea[0] * e[0] + ea[1] * e[1]) / Math.atan2(ea[0] * eb[1] - ea[1] * eb[0], ea[0] * eb[0] + ea[1] * eb[1]);
    return t >= 0 && t <= 1 && (e[0] - ea[0]) * ea[0] + (e[1] - ea[1]) * ea[1] <= 0 && (e[0] - eb[0]) * eb[0] + (e[1] - eb[1]) * eb[1] <= 0;
}
