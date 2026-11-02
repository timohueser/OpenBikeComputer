// The plan of a tile's junctions, a pure function of its lines. At each point that roads and paths share, the through
// roads are the hosts: they keep their shape. Each other way is a guest: a path stops at its host's outer edge, a
// driveway at the outer edge of the sidewalk that it crosses, and a side road at the host's carriageway edge, between
// two corners whose arcs turn the kerb from the one way to the other. A guest stands on its host's plane where it stops
// and returns to its own heights past it. The stamp and the corridors read this one plan: its guests, its corners, and
// the sidewalk that a side road takes round a corner from its host.
import { pointOn, spanOf } from '../stream/curve';
import { SIDE_BACK, SIDE_ON } from '../stream/features';
import { BRIDGE, crossSlope, isDeck, isPath, KERB_RISE_M, metresPerUnit, mitre, PATH, PAVED_CODES, PROFILES, RAIL, RAIL_CODE, ROAD, ROAD_CODE, SHOULDER_M, soil, SURFACES, TUNNEL, walkOf, WARP_M, type Line } from './bins';
import { halfAt, partnersOf } from './corners';
import { LEFT, RIGHT } from './town';

/** The plan reads and changes this many metres of each way out of a junction, so each tile whose buffer holds the junction plans it alike. */
export const WINDOW_M = 16;
/** A way's direction is toward its point this far on, as in the bake. A host's seam and a corner's edges run on this far behind its junction. */
const LOOK_M = 8, BEHIND_M = 0.5;
/** Two ways out of a point at this cosine or less run on into each other. */
const RUNS_ON_COS = -0.5;
/** A guest's return to its own heights: the most grade that it adds, its fewest metres and its chords. A change under SAME_M stays out. */
export const RETURN_GRADE = 0.15, RETURN_LEAST_M = 2, RETURN_CHORDS = 3, SAME_M = 0.01, STOP_CLEAR_M = 2, SQUEEZE_M = 0.5;
/** The most metres of a way that a corner takes, and its share of the way to the way's end or next junction near by. */
export const ROOM_M = 14;
const ROOM_SHARE = 0.45;
/** Corner radii: a paved road's edge or kerb, a track's; the least arc. A kerbed arc clears its sidewalk by WALK_CLEAR_M. */
const PAVED_R = 5, TRACK_R = 4, LEAST_R = 0.8, WALK_CLEAR_M = 0.6;
/** An arc tries a smaller radius by this factor while its ends leave a way's room. */
const SHRINK = 0.8;
/** The widest turn between two ways that makes a corner where neither is a side road, by their tangents, and where one is, by their directions. */
const MAX_TURN = (150 * Math.PI) / 180, MAX_SIDE_TURN = (172 * Math.PI) / 180;
/** A side road's carriageway needs this many metres past its stop. */
const MOUTH_ROOM_M = 1;
/** A sidewalk ends over this many metres, about two slabs: its kerb turns to its back edge at about 30°. */
export const WALK_END_M = 3;
/** Per segment, what `Line.inside` holds: a path or a driveway yields its core and its paint to every way; a side road's mouth only to the carriageways. */
export const INSIDE = 1, MOUTH = 2;

/** A way out of a shared vertex: its unit direction in metres east and south toward its point LOOK_M on, and its angle; the metres that a corner may take of it. */
export interface Arm { line: Line; v: number; step: 1 | -1; u: [number, number]; angle: number; through: boolean; room: number }

/** A shared point, `unit` metres per world unit there, with its arms sorted by angle. */
export interface Meeting { x: number; y: number; unit: number; arms: Arm[] }

/**
 * A guest: the way out of the junction at world point x, y, `step` along its line. A path stops at its host's outer
 * edge, a driveway at the outer edge of the sidewalk that it crosses, and a side road at the host's carriageway edge;
 * `seam` metres from the host's middle on the host line's `side` (1 its left). It stops `stop` metres along its own
 * line, where its whole core has left that edge, at height `z`, the host's plane under its middle. A side road holds
 * the plane of its mouth, `plane` per metre east and south, up to `held`, where its corners' arcs end.
 */
export interface Guest { kind: 'path' | 'driveway' | 'side'; line: Line; x: number; y: number; step: 1 | -1; host: Line; side: 1 | -1; seam: number; walk: boolean; stop: number; held: number; z: number; plane?: [number, number] }

/**
 * A corner between neighbouring arms p and q of the junction at x, y, the side of each line that faces it: the arc's
 * centre `o`, its ends `a` on p's edge and `b` on q's, and the point `m` where the edges meet, in world units; its radius
 * in metres, 0 where it stays sharp, and the metres from the centre to `m`; the half-width `h` of the narrower way. `ends`
 * and `meet` are metres along each line from its first vertex.
 */
export interface Corner { x: number; y: number; p: Arm; q: Arm; sp: 1 | -1; sq: 1 | -1; o: [number, number]; a: [number, number]; b: [number, number]; m: [number, number]; r: number; apex: number; h: number; ends: [number, number]; meet: [number, number]; oneLine: boolean }

export interface JunctionPlan { guests: Guest[]; corners: Corner[] }

const smooth = (t: number) => t * t * (3 - 2 * t);
const nameOf = (line: Line, v: number) => `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;
const className = (line: Line) => Object.keys(PROFILES)[line.profile];
const shoulderOf = (line: Line) => SHOULDER_M[(Object.keys(SURFACES) as (keyof typeof SURFACES)[])[(line.code & 15) - 1]] ?? 0;
/** A line with a corridor: a road, a track or a path on the ground, or a deck. */
const corridor = (line: Line) => ((line.kind === ROAD || line.kind === RAIL || line.kind === PATH) && !line.gallery) || isDeck(line);
/** A way of the junctions: a road or a path, and a road's bridge, tunnel or gallery. */
const counts = (line: Line) => ((line.kind === ROAD || line.kind === PATH) && !line.gallery) || ((line.kind === BRIDGE || line.kind === TUNNEL || !!line.gallery) && !isPath(line) && line.code < RAIL_CODE);
/** The segment of an arm next to its junction. */
const firstSegment = ({ v, step }: Pick<Arm, 'v' | 'step'>) => (step > 0 ? v : v - 1);
/** The sidewalk's width on side `side` (1 left) of segment s. */
const walkAt = (line: Line, s: number, side: 1 | -1) => ((line.sidewalks?.[s] ?? 0) & (side > 0 ? LEFT : RIGHT) ? walkOf(line) : 0);

/** Metres along a line at each vertex. */
export function alongOf({ xy, z }: Line): number[] {
    const out = [0];
    for (let v = 1; v < z.length; v++) out.push(out[v - 1] + Math.hypot(xy[2 * v] - xy[2 * v - 2], xy[2 * v + 1] - xy[2 * v - 1]) * metresPerUnit(xy[2 * v + 1]));
    return out;
}

/** The point of a line `metres` along it from its first vertex: x, y and its segment with the share along that. */
export function pointAlong({ xy }: Line, along: number[], metres: number): { x: number; y: number; v: number; u: number } {
    let v = 0;
    while (v + 2 < along.length && along[v + 1] < metres) v++;
    const u = Math.max(0, Math.min(1, (metres - along[v]) / ((along[v + 1] - along[v]) || 1)));
    return { x: xy[2 * v] + u * (xy[2 * v + 2] - xy[2 * v]), y: xy[2 * v + 1] + u * (xy[2 * v + 3] - xy[2 * v + 1]), v, u };
}

/** A line's height `metres` along it, on its chords as the stamp holds it. */
const heightAt = (line: Line, along: number[], metres: number) => {
    const { v, u } = pointAlong(line, along, metres);
    return line.z[v] + u * (line.z[v + 1] - line.z[v]);
};

/** Cuts a line at these metres along it: the cuts become vertices on its curve, and each piece of a segment keeps the segment's marks. */
export function cutLine(line: Line, cuts: number[], along = alongOf(line)): void {
    const wanted = [...new Set(cuts)].sort((a, b) => a - b), plan = line.plan;
    const out = { xy: [line.xy[0], line.xy[1]], z: [line.z[0]], anchors: [line.anchors?.[0] ?? true], sideWays: line.sideWays && [line.sideWays[0]], sidewalks: line.sidewalks && ([] as number[]), inside: line.inside && ([] as number[]), plan: plan && { tangent: [plan.tangent[0]], grade: [plan.grade[0]], bank: [plan.bank[0]], widen: [plan.widen[0]] } };
    const push = (x: number, y: number, z: number, anchor: boolean, side: number, v: number, on?: { tangent: number; grade: number }, u = 1) => {
        out.xy.push(x, y);
        out.z.push(z);
        out.anchors.push(anchor);
        out.sideWays?.push(side);
        if (line.sidewalks) out.sidewalks!.push(line.sidewalks[v]);
        if (line.inside) out.inside!.push(line.inside[v] ?? 0);
        if (plan && out.plan) {
            const mix = (p: number[]) => p[v] + u * (p[v + 1] - p[v]);
            out.plan.tangent.push(on?.tangent ?? plan.tangent[v + 1]);
            out.plan.grade.push(on?.grade ?? plan.grade[v + 1]);
            out.plan.bank.push(mix(plan.bank));
            out.plan.widen.push(mix(plan.widen));
        }
    };
    for (let v = 0, c = 0; v + 1 < line.z.length; v++) {
        const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), metres = along[v + 1] - along[v];
        const span = plan && spanOf(bx - ax, by - ay, plan.tangent[v], plan.tangent[v + 1], line.z[v], line.z[v + 1], plan.grade[v], plan.grade[v + 1], metres);
        while (c < wanted.length && wanted[c] <= along[v] + 1e-3) c++;
        for (; c < wanted.length && wanted[c] < along[v + 1] - 1e-3; c++) {
            const u = (wanted[c] - along[v]) / metres, on = span && pointOn(span, u), mix = (p: number, q: number) => p + u * (q - p);
            // On the chord: what reads the line's plan, in a tile with its junction or without, finds the same line.
            push(mix(ax, bx), mix(ay, by), on?.z ?? mix(line.z[v], line.z[v + 1]), true, 0, v, on ?? { tangent: 0, grade: 0 }, u);
        }
        push(bx, by, line.z[v + 1], line.anchors?.[v + 1] ?? true, line.sideWays?.[v + 1] ?? 0, v);
    }
    Object.assign(line, out);
}

/**
 * Brings a line from height `z` at `at` metres along it back to its own heights, toward `dir`, within `room` metres:
 * over RETURN_LEAST_M at least, at RETURN_GRADE at most, in RETURN_CHORDS chords, so the stamp's straight segments
 * follow its curve. Where the room is too short, the change shrinks; a guest's, which must stand on its host where it
 * stops, takes all the room instead, at least SQUEEZE_M. Returns the height that the line takes at `at`.
 */
export function returnTo(line: Line, at: number, dir: 1 | -1, z: number, room: number, squeeze = false): number {
    const own = heightAt(line, alongOf(line), at);
    if (!squeeze && room < RETURN_LEAST_M) return own;
    const change = squeeze ? z - own : Math.max(-RETURN_GRADE * room, Math.min(RETURN_GRADE * room, z - own));
    const over = Math.max(RETURN_LEAST_M, Math.abs(change) / RETURN_GRADE), fits = squeeze ? Math.max(SQUEEZE_M, Math.min(over, room)) : over;
    if (Math.abs(change) < SAME_M) return own;
    cutLine(line, Array.from({ length: RETURN_CHORDS + 1 }, (_, k) => at + (dir * fits * k) / RETURN_CHORDS));
    alongOf(line).forEach((metres, v) => {
        const t = (metres - at) * dir / fits;
        if (t < -1e-3 || t > 1 + 1e-3) return;
        const u = Math.min(1, Math.max(0, t));
        line.z[v] += change * (1 - smooth(u));
        // The curve between two points holds their grades, so it is the same cubic as the return's.
        if (line.plan) line.plan.grade[v] -= (dir * change * 6 * u * (1 - u)) / fits;
    });
    return own + change;
}

/** A unit direction out of vertex v of a line, toward its point LOOK_M on, in metres east and south. */
export function direction({ xy, z }: Line, v: number, step: 1 | -1, unit: number): [number, number] {
    let left = LOOK_M, px = xy[2 * v], py = xy[2 * v + 1];
    for (let k = v + step; k >= 0 && k < z.length && left > 0; k += step) {
        const piece = Math.hypot(xy[2 * k] - px, xy[2 * k + 1] - py) * unit, u = Math.min(1, left / (piece || 1));
        [px, py, left] = [px + u * (xy[2 * k] - px), py + u * (xy[2 * k + 1] - py), left - piece];
    }
    const dx = (px - xy[2 * v]) * unit, dy = (py - xy[2 * v + 1]) * unit, l = Math.hypot(dx, dy) || 1;
    return [dx / l, dy / l];
}

/** The metres from an arm's junction to its line's end or to its next shared vertex, if either lies within `most`. */
function reachOf({ line, v, step }: Pick<Arm, 'line' | 'v' | 'step'>, shared: (name: string) => boolean, unit: number, most: number): number {
    const last = line.z.length - 1;
    let d = 0;
    for (let k = v + step; k >= 0 && k <= last && d < most; k += step) {
        d += Math.hypot(line.xy[2 * k] - line.xy[2 * k - 2 * step], line.xy[2 * k + 1] - line.xy[2 * k + 1 - 2 * step]) * unit;
        if (k === 0 || k === last || shared(nameOf(line, k))) return d;
    }
    return Infinity;
}

/** The meetings of a tile's lines: points of two ways or more, with their arms sorted by angle. Planning and drawing read the same. */
export function meetingsOf(lines: Line[]): Meeting[] {
    const at = new Map<string, { line: Line; v: number }[]>();
    for (const line of lines) {
        if (!counts(line)) continue;
        for (let v = 0; v < line.z.length; v++) {
            // A deck is a way of a junction at its ends only: what crosses its inner vertices passes under it.
            if (!(line.anchors?.[v] ?? true) || (isDeck(line) && v > 0 && v + 1 < line.z.length)) continue;
            (at.get(nameOf(line, v)) ?? at.set(nameOf(line, v), []).get(nameOf(line, v))!).push({ line, v });
        }
    }
    const shared = (name: string) => (at.get(name)?.length ?? 0) > 1, out: Meeting[] = [];
    for (const here of at.values()) {
        if (here.length < 2) continue;
        const { line: first, v: at0 } = here[0], x = first.xy[2 * at0], y = first.xy[2 * at0 + 1], unit = metresPerUnit(y);
        const arms = here.flatMap(({ line, v }) => ([[-1, SIDE_BACK], [1, SIDE_ON]] as const).filter(([step]) => v + step >= 0 && v + step < line.z.length).map(([step, bit]): Arm => {
            const u = direction(line, v, step, unit), d = reachOf({ line, v, step }, shared, unit, WINDOW_M);
            return { line, v, step, u, angle: Math.atan2(u[1], u[0]), through: !((line.sideWays?.[v] ?? 0) & bit), room: d < WINDOW_M ? Math.min(ROOM_M, ROOM_SHARE * d) : ROOM_M };
        }));
        arms.sort((p, q) => p.angle - q.angle);
        out.push({ x, y, unit, arms });
    }
    return out;
}

/** The metres from a host's middle to its outer edge on `side` of segment s: a sidewalk's outer edge, else the shoulder's. */
function seamOf(host: Line, s: number, side: 1 | -1): { seam: number; walk: boolean } {
    const walk = walkAt(host, s, side);
    return { seam: host.h + (walk || shoulderOf(host)), walk: walk > 0 };
}

/** A host's outer edge on `side` from its junction out: per segment of its line within WINDOW_M, the segment's chord moved out by its seam, the first one run back by BEHIND_M. */
function seamLines({ line, v, step }: Arm, side: 1 | -1, unit: number) {
    const out: { s: number; a: number[]; b: number[]; seam: number; walk: boolean }[] = [];
    for (let k = v, metres = 0; k + step >= 0 && k + step < line.z.length && metres < WINDOW_M; k += step) {
        const s = Math.min(k, k + step), [ax, ay, bx, by] = line.xy.slice(2 * s, 2 * s + 4), l = Math.hypot(bx - ax, by - ay) || 1, { seam, walk } = seamOf(line, s, side);
        // Left of the line's own direction: y runs south.
        const nx = (side * (by - ay) / l) * seam / unit, ny = (side * -(bx - ax) / l) * seam / unit, back = k === v ? BEHIND_M / unit / l : 0;
        const [px, py, qx, qy] = step > 0 ? [ax - (bx - ax) * back, ay - (by - ay) * back, bx, by] : [ax, ay, bx + (bx - ax) * back, by + (by - ay) * back];
        out.push({ s, a: [px + nx, py + ny], b: [qx + nx, qy + ny], seam, walk });
        metres += l * unit;
    }
    return out;
}

/** Where segment p-q first crosses segment a-b: the shares along each. */
function crossing(p: number[], q: number[], a: number[], b: number[]): { t: number; u: number } | undefined {
    const r = [q[0] - p[0], q[1] - p[1]], s = [b[0] - a[0], b[1] - a[1]], cross = r[0] * s[1] - r[1] * s[0];
    if (Math.abs(cross) < 1e-12) return undefined;
    const t = ((a[0] - p[0]) * s[1] - (a[1] - p[1]) * s[0]) / cross, u = ((a[0] - p[0]) * r[1] - (a[1] - p[1]) * r[0]) / cross;
    return t >= 0 && t <= 1 && u >= 0 && u <= 1 ? { t, u } : undefined;
}

/** The side of a host's line that a way out of their junction in direction g lies on: 1 its left. */
const sideOfHost = (host: Arm, g: number[], unit: number) => {
    const [hx, hy] = direction(host.line, host.v, host.step, unit);
    return (host.step * (g[0] * hy - g[1] * hx) > 0 ? 1 : -1) as 1 | -1;
};

/**
 * Where a path's or a driveway's middle, or the line `across` metres left of it, first crosses a host's outer edge,
 * within WINDOW_M: metres along the guest from its junction, the host, its side and the host's plane there.
 */
function stopOf(guest: Arm, hosts: Arm[], unit: number, across = 0): { host: Line; side: 1 | -1; seam: number; walk: boolean; metres: number } | undefined {
    const { line, v, step } = guest, g = direction(line, v, step, unit);
    const seams = hosts.map(host => {
        const side = sideOfHost(host, g, unit);
        return { host, side, lines: seamLines(host, side, unit) };
    });
    let k = v, metres = 0;
    for (; k + step >= 0 && k + step < line.z.length && metres < WINDOW_M; k += step) {
        const [px, py, qx, qy] = [...line.xy.slice(2 * k, 2 * k + 2), ...line.xy.slice(2 * (k + step), 2 * (k + step) + 2)], l = Math.hypot(qx - px, qy - py) * unit;
        // Left of the way out: y runs south.
        const nx = (across * (qy - py)) / (l || 1), ny = (across * -(qx - px)) / (l || 1), p = [px + nx, py + ny], q = [qx + nx, qy + ny];
        let best: { t: number; found: NonNullable<ReturnType<typeof stopOf>> } | undefined;
        for (const { host, side, lines } of seams) for (const { a, b, seam, walk } of lines) {
            const hit = crossing(p, q, a, b);
            if (!hit || (best && hit.t >= best.t)) continue;
            best = { t: hit.t, found: { host: host.line, side, seam, walk, metres: metres + hit.t * l } };
        }
        if (best) return best.found;
        metres += l;
    }
    // A way that ends inside its hosts, as a mapped crossing does, is inside to its end, at its own height.
    const [ex, ey] = line.xy.slice(2 * k, 2 * k + 2), within = seams.some(({ host: { line: h }, lines }) => lines.some(({ s, seam }) => {
        const [ax, ay, bx, by] = h.xy.slice(2 * s, 2 * s + 4), dx = bx - ax, dy = by - ay, l = Math.hypot(dx, dy) || 1, t = ((ex - ax) * dx + (ey - ay) * dy) / l / l;
        return t >= 0 && t <= 1 && Math.abs((ex - ax) * dy - (ey - ay) * dx) / l * unit < seam;
    }));
    return metres < WINDOW_M && within ? { host: seams[0].host.line, side: seams[0].side, seam: 0, walk: false, metres } : undefined;
}

/** A host's plane under a world point: at its foot on the host's nearest segment, held level past the carriageway's edge; and that foot's distance. */
function planeUnder(host: Line, x: number, y: number, unit: number): { z: number; away: number } {
    let least = Infinity, z = 0;
    for (let s = 0; s + 1 < host.z.length; s++) {
        const [ax, ay, bx, by] = host.xy.slice(2 * s, 2 * s + 4), dx = bx - ax, dy = by - ay, l = Math.hypot(dx, dy) || 1, u = Math.max(0, Math.min(1, ((x - ax) * dx + (y - ay) * dy) / l / l));
        const d = Math.hypot(ax + u * dx - x, ay + u * dy - y);
        if (d >= least) continue;
        // Left of the host's line: y runs south.
        const left = (((x - ax) * dy - (y - ay) * dx) / l) * unit, mix = (w: number[]) => w[s] + u * (w[s + 1] - w[s]);
        [least, z] = [d, mix(host.z) + (host.plan ? mix(host.plan.bank) : 0) * Math.max(-host.h, Math.min(host.h, left))];
    }
    return { z, away: least * unit };
}

/** Whether a driveway out of a junction crosses a host's sidewalk: a host has one on its side, and the driveway has none of its own near. */
function crossesWalk(guest: Arm, hosts: Arm[], unit: number): boolean {
    const g = direction(guest.line, guest.v, guest.step, unit);
    const own = (line: Line, v: number, step: number) => {
        for (let k = v, metres = 0; k + step >= 0 && k + step < line.z.length && metres < WINDOW_M; k += step) {
            if (line.sidewalks?.[Math.min(k, k + step)]) return true;
            metres += Math.hypot(line.xy[2 * (k + step)] - line.xy[2 * k], line.xy[2 * (k + step) + 1] - line.xy[2 * k + 1]) * unit;
        }
        return false;
    };
    return !own(guest.line, guest.v, guest.step) && hosts.some(host => walkAt(host.line, firstSegment(host), sideOfHost(host, g, unit)) > 0);
}

/** An arm's carriageway edge on side `side` of its line, `out` metres past it: its vertices moved along their mitres, in metres about the junction, with their metres from it. */
function edgeOf({ line, v, step }: Arm, side: 1 | -1, out: number, { x, y, unit }: Meeting, partners: ReturnType<typeof partnersOf>): { points: number[][]; metres: number[] } {
    const points: number[][] = [], metres: number[] = [];
    for (let k = v, d = 0; k >= 0 && k < line.z.length; k += step) {
        if (k !== v) d += Math.hypot(line.xy[2 * k] - line.xy[2 * k - 2 * step], line.xy[2 * k + 1] - line.xy[2 * k + 1 - 2 * step]) * unit;
        // Left of the line's tangent: y runs south.
        const [tx, ty] = mitre(line, k), a = side * (halfAt(line, k, partners) + out);
        points.push([(line.xy[2 * k] - x) * unit + ty * a, (line.xy[2 * k + 1] - y) * unit - tx * a]);
        metres.push(d);
        if (d >= WINDOW_M) break;
    }
    const [p, q] = points, l = Math.hypot(q[0] - p[0], q[1] - p[1]) || 1;
    return { points: [[p[0] - (q[0] - p[0]) * BEHIND_M / l, p[1] - (q[1] - p[1]) * BEHIND_M / l], ...points], metres: [-BEHIND_M, ...metres] };
}

type Edge = ReturnType<typeof edgeOf>;

/** Where edge B first crosses edge A, by the metres along A: the metres along each edge. */
function edgesCross(A: Edge, B: Edge): { a: number; b: number } | undefined {
    for (let i = 0; i + 1 < A.points.length; i++) {
        let best: { a: number; b: number } | undefined, least = Infinity;
        for (let k = 0; k + 1 < B.points.length; k++) {
            const hit = crossing(A.points[i], A.points[i + 1], B.points[k], B.points[k + 1]);
            if (!hit || hit.t >= least) continue;
            least = hit.t;
            best = { a: A.metres[i] + hit.t * (A.metres[i + 1] - A.metres[i]), b: B.metres[k] + hit.u * (B.metres[k + 1] - B.metres[k]) };
        }
        if (best) return best;
    }
    return undefined;
}

/** The point of an edge `metres` along it. */
function pointAt({ points, metres }: Edge, at: number): number[] {
    let k = 0;
    while (k + 2 < metres.length && metres[k + 1] < at) k++;
    const t = (at - metres[k]) / ((metres[k + 1] - metres[k]) || 1);
    return points[k].map((value, i) => value + t * (points[k + 1][i] - value));
}

/**
 * The corner from arm p to the next arm q round their meeting; none where their edges do not meet near it. Its centre
 * lies on both edges moved out by its radius, so the arc meets each edge square and fits a way that curves near the junction.
 */
function cornerOf(meeting: Meeting, p: Arm, q: Arm, partners: ReturnType<typeof partnersOf>): Corner | undefined {
    // Across the line at a step forward is the way's left, and angles grow to its right: y runs south.
    const sp = p.step > 0 ? -1 : 1, sq = q.step > 0 ? 1 : -1, { unit } = meeting, along = [alongOf(p.line)[p.v], alongOf(q.line)[q.v]];
    const [ep, eq] = [(out: number) => edgeOf(p, sp, out, meeting, partners), (out: number) => edgeOf(q, sq, out, meeting, partners)];
    // A line that runs on through its vertex has one edge on this side: there is no corner to turn.
    const oneLine = p.line === q.line && p.v === q.v, met = oneLine ? { a: 0, b: 0 } : edgesCross(ep(0), eq(0));
    if (!met || Math.max(met.a, met.b) > ROOM_M) return undefined;
    const paved = PAVED_CODES.includes(p.line.code) && PAVED_CODES.includes(q.line.code);
    const walk = Math.max(walkAt(p.line, firstSegment(p), sp), walkAt(q.line, firstSegment(q), sq)), least = walk > 0 ? walk + WALK_CLEAR_M : LEAST_R;
    const world = ([e, s]: number[]): [number, number] => [Math.round(meeting.x + e / unit), Math.round(meeting.y + s / unit)];
    const corner = (o: number[], r: number, a: number, b: number): Corner => ({
        x: meeting.x, y: meeting.y, p, q, sp, sq, o: world(o), a: world(pointAt(ep(0), a)), b: world(pointAt(eq(0), b)), m: world(pointAt(ep(0), met.a)), r, apex: Math.hypot(...pointAt(ep(0), met.a).map((value, i) => value - o[i])), h: Math.min(p.line.h, q.line.h), oneLine,
        ends: [along[0] + p.step * Math.max(0, a), along[1] + q.step * Math.max(0, b)], meet: [along[0] + p.step * Math.max(0, met.a), along[1] + q.step * Math.max(0, met.b)],
    });
    // A bridge's edge has its kerb or its parapet: no arc rounds it.
    if (!oneLine && p.line.kind !== BRIDGE && q.line.kind !== BRIDGE) for (let r = Math.max(paved ? PAVED_R : TRACK_R, least); ; r = Math.max(r * SHRINK, least)) {
        const offset = [ep(r), eq(r)], hit = edgesCross(offset[0], offset[1]), last = r === least;
        if (hit && (last || (hit.a <= p.room && hit.b <= q.room)) && hit.a >= 0 && hit.b >= 0) return corner(pointAt(offset[0], hit.a), r, hit.a, hit.b);
        if (last) break;
    }
    return corner(pointAt(ep(0), met.a), 0, met.a, met.b);
}

/** The cosine of the angle between two ways out of their junction, by their tangents there: a bend's chords hide them. */
function tangentCos(p: Arm, q: Arm): number {
    const [a, b] = [mitre(p.line, p.v), mitre(q.line, q.v)];
    return p.step * q.step * (a[0] * b[0] + a[1] * b[1]);
}

/**
 * A junction's side roads and corners: at three road arms or more with two through and a surface to join, the corners
 * between neighbouring ways, none between two side roads and none where the through pair runs on in about one curve. A
 * side road with a corner on each side stops where both its edges have met the host's carriageway edge, the farther
 * of its corners' meets, if its carriageway runs on past that. A corner stays where each of its arms is through or such a side road.
 */
function cornersAt(meeting: Meeting, crossings: Arm[], partners: ReturnType<typeof partnersOf>): { corners: Corner[]; sides: Map<Arm, number> } {
    const roads = meeting.arms.filter(({ line }) => line.kind !== PATH), pair = roads.filter(({ through }) => through), sides = new Map<Arm, number>();
    // A pair of soil has no surface to join: the ruts cross, and the ground shows between.
    if (roads.length < 3 || pair.length !== 2 || roads.filter(({ line }) => corridor(line)).length < 2 || pair.every(({ line }) => soil(line))) return { corners: [], sides };
    const ways = roads.filter(arm => !crossings.includes(arm)), found = ways.flatMap((p, i) => {
        const q = ways[(i + 1) % ways.length], turn = q.angle - p.angle + (i + 1 === ways.length ? 2 * Math.PI : 0);
        if ((!p.through && !q.through) || !corridor(p.line) || !corridor(q.line)) return [];
        if (p.through && q.through ? tangentCos(p, q) <= Math.cos(MAX_TURN) : turn >= MAX_SIDE_TURN) return [];
        return cornerOf(meeting, p, q, partners) ?? [];
    });
    for (const arm of ways) {
        const before = found.find(({ q }) => q === arm), after = found.find(({ p }) => p === arm);
        // A deck that leaves as a side starts at the road's edge.
        if (arm.through || isDeck(arm.line) || !before || !after) continue;
        const along = alongOf(arm.line), stop = Math.max(...[before.meet[1], after.meet[0]].map(m => Math.abs(m - along[arm.v])));
        if (stop + MOUTH_ROOM_M <= (arm.step > 0 ? along[along.length - 1] - along[arm.v] : along[arm.v])) sides.set(arm, stop);
    }
    return { corners: found.filter(({ p, q }) => (p.through || sides.has(p)) && (q.through || sides.has(q))), sides };
}

/**
 * The plane of a junction's mouth: through the host's middle at the junction and its carriageway edges where the two
 * edges of a side road meet them, so a mouth between two through ways of different grades lies on one plane. Gives its
 * rise per metre east and south, and its height at a point. Where the three points lie on one line, the host's plane holds.
 */
function junctionPlane({ x, y, unit, arms }: Meeting, meets: { at: [number, number]; z: number }[], at: { x: number; y: number }): { plane: [number, number]; z: number } {
    const centre = arms.filter(({ through, line }) => through && line.kind === ROAD).map(({ line, v }) => line.z[v]), z0 = centre.reduce((a, b) => a + b, 0) / (centre.length || 1);
    const [[ax, ay], [bx, by]] = meets.map(({ at: [px, py] }) => [(px - x) * unit, (py - y) * unit]), det = ax * by - ay * bx;
    const [da, db] = meets.map(({ z }) => z - z0), plane: [number, number] = Math.abs(det) > 1e-6 ? [(da * by - db * ay) / det, (db * ax - da * bx) / det] : [0, 0];
    return { plane, z: z0 + plane[0] * (at.x - x) * unit + plane[1] * (at.y - y) * unit };
}

/**
 * Where a side road has no sidewalk on a corner's side and its host has one there, the side road takes the host's
 * sidewalk round the arc: from its junction to WALK_END_M past the arc's end, where it ends, within its room before
 * its next junction; or on into its own sidewalk where that starts within three such ends. A class with no sidewalk
 * takes the host's width (`Line.walk`). Gives the metres along the line from the junction to where it ends, and the side.
 */
function stubOf(corner: Corner, meeting: Meeting, shared: (name: string) => boolean): { line: Line; from: number; to: number; side: 1 | -1 } | undefined {
    if (corner.r <= 0 || corner.p.through === corner.q.through) return undefined;
    const [host, hostSide, end, guest, side, guestEnd] = corner.p.through ? [corner.p, corner.sp, corner.ends[0], corner.q, corner.sq, corner.ends[1]] : [corner.q, corner.sq, corner.ends[1], corner.p, corner.sp, corner.ends[0]];
    const width = walkAt(host.line, pointAlong(host.line, alongOf(host.line), end).v, hostSide), along = alongOf(guest.line), from = along[guest.v], arc = Math.abs(guestEnd - from);
    if (!width) return undefined;
    const own = (metres: number) => walkAt(guest.line, pointAlong(guest.line, along, from + guest.step * metres).v, side) > 0;
    let to = Math.min(arc + WALK_END_M, reachOf(guest, shared, meeting.unit, 2 * WINDOW_M) - STOP_CLEAR_M);
    for (let m = 0.05; m < arc + 3 * WALK_END_M; m += 0.25) if (own(m)) {
        if (m <= arc) return undefined;
        to = m;
        break;
    }
    if (to < arc + 1) return undefined;
    if (!Object.values(PROFILES)[guest.line.profile].walk) guest.line.walk = Math.max(guest.line.walk ?? 0, width);
    return { line: guest.line, from, to: from + guest.step * to, side };
}

/**
 * Plans a tile's junctions and applies the plan to its lines: each guest is cut at its stop, its segments up to the
 * stop are marked inside, and it returns to its own heights past the stop; a side road takes its host's sidewalk round
 * each corner where it has none. Steps keep their own heights.
 */
export function junctionPlan(lines: Line[]): JunctionPlan {
    const partners = partnersOf(lines), guests: Guest[] = [], corners: Corner[] = [], stubs: NonNullable<ReturnType<typeof stubOf>>[] = [];
    const sharedNames = new Map<string, number>();
    for (const line of lines) if (counts(line)) line.z.forEach((_, v) => sharedNames.set(nameOf(line, v), (sharedNames.get(nameOf(line, v)) ?? 0) + 1));
    const shared = (name: string) => (sharedNames.get(name) ?? 0) > 1;
    for (const meeting of meetingsOf(lines)) {
        const { x, y, unit, arms } = meeting, hosts = arms.filter(({ through, line }) => through && line.kind === ROAD);
        if (!hosts.length) continue;
        const dirs = new Map(arms.map(arm => [arm, arm.u]));
        // A way that runs on from a road that ends, or across its end, is no guest of it.
        const runsOn = (arm: Arm) => arms.some(other => other !== arm && dirs.get(other)![0] * arm.u[0] + dirs.get(other)![1] * arm.u[1] < RUNS_ON_COS);
        const crossings = arms.filter(arm => !arm.through && arm.line.kind === ROAD && arm.line.driveway && crossesWalk(arm, hosts, unit));
        const { corners: found, sides } = cornersAt(meeting, crossings, partners);
        corners.push(...found);
        for (const corner of found) {
            const stub = stubOf(corner, meeting, shared);
            if (stub) stubs.push(stub);
        }
        for (const [arm, metres] of sides) {
            const [before, after] = [found.find(({ q }) => q === arm)!, found.find(({ p }) => p === arm)!], hostOf = ({ p, q }: Corner) => (p.through ? p : q);
            const along = alongOf(arm.line), stop = along[arm.v] + arm.step * metres, held = arm.step > 0 ? Math.max(stop, before.ends[1], after.ends[0]) : Math.min(stop, before.ends[1], after.ends[0]);
            const { plane, z } = junctionPlane(meeting, [before, after].map(corner => ({ at: corner.m, z: planeUnder(hostOf(corner).line, corner.m[0], corner.m[1], unit).z })), pointAlong(arm.line, along, held));
            const host = hostOf(before);
            guests.push({ kind: 'side', line: arm.line, x, y, step: arm.step, host: host.line, side: sideOfHost(host, arm.u, unit), seam: host.line.h, walk: false, stop, held, z, plane });
        }
        for (const arm of arms) {
            const path = arm.line.kind === PATH;
            if (!(path || crossings.includes(arm)) || (hosts.length < 2 && runsOn(arm))) continue;
            // A guest stops where its whole core has left the host: past there its faces lie on no face of the host.
            const core = arm.line.h + shoulderOf(arm.line);
            let stop = stopOf(arm, hosts, unit);
            if (!stop) continue;
            for (const edge of [stopOf(arm, hosts, unit, core), stopOf(arm, hosts, unit, -core)]) if (edge && edge.metres > stop.metres) stop = edge;
            const along = alongOf(arm.line), metres = along[arm.v] + arm.step * stop.metres, middle = pointAlong(arm.line, along, metres);
            // A way that ends inside its host keeps its own height.
            const z = stop.seam ? planeUnder(stop.host, middle.x, middle.y, unit).z + (stop.walk ? KERB_RISE_M : 0) : heightAt(arm.line, along, metres);
            guests.push({ kind: path ? 'path' : 'driveway', line: arm.line, x, y, step: arm.step, host: stop.host, side: stop.side, seam: stop.seam, walk: stop.walk, stop: metres, held: metres, z });
        }
    }
    for (const { line, from, to, side } of stubs) {
        cutLine(line, [to]);
        const along = alongOf(line);
        line.sidewalks ??= new Array(line.z.length - 1).fill(0);
        for (let s = 0; s + 1 < along.length; s++) if ((along[s] + along[s + 1]) / 2 > Math.min(from, to) && (along[s] + along[s + 1]) / 2 < Math.max(from, to)) line.sidewalks[s] |= side > 0 ? LEFT : RIGHT;
    }
    // Any line that shares a vertex holds its heights there: a deck, a tunnel or a gallery too.
    const heldAt = new Map<string, number>();
    for (const line of lines) line.z.forEach((_, v) => heldAt.set(nameOf(line, v), (heldAt.get(nameOf(line, v)) ?? 0) + 1));
    // A line that is a guest at its next junction too returns in its half of the way there.
    const visits = new Map<Line, Set<string>>();
    for (const { line, x, y } of guests) (visits.get(line) ?? visits.set(line, new Set()).get(line)!).add(`${x},${y}`);
    for (const guest of guests) settle(guest, heldAt, (line, k) => !!visits.get(line)?.has(nameOf(line, k)));
    return { guests, corners };
}

/**
 * Cuts a guest at its stop, marks it inside up to there, and gives it its height `z` at `held` and its return past that.
 * Up to `held` it runs straight from the junction's height; a side road lies there with no bank of its own, on the
 * plane of its mouth, and takes its bank back over WARP_M, as the junction's planes give back its slope across.
 */
function settle(guest: Guest, shared: Map<string, number>, guestAt: (line: Line, v: number) => boolean): void {
    const { line, x, y, step, stop, held } = guest;
    cutLine(line, [stop, held]);
    const along = alongOf(line), v = line.xy.findIndex((value, i) => i % 2 === 0 && value === x && line.xy[i + 1] === y) / 2, junction = along[v];
    // A return ends STOP_CLEAR_M short of the next shared vertex, whose heights another way reads there. A line's other end says nothing: the tile may cut it there.
    let room = WINDOW_M - Math.abs(held - junction);
    for (let k = v + step; k >= 0 && k < line.z.length && Math.abs(along[k] - junction) < 2 * WINDOW_M; k += step) {
        if ((shared.get(nameOf(line, k)) ?? 0) < 2) continue;
        const metres = Math.abs(along[k] - junction);
        room = Math.min(room, (guestAt(line, k) ? metres / 2 : metres - STOP_CLEAR_M) - Math.abs(held - junction));
        break;
    }
    line.inside ??= new Array(line.z.length - 1).fill(0);
    for (let s = Math.min(v, v + step); s >= 0 && s + 1 < line.z.length; s += step) {
        if ((along[s] + along[s + 1]) / 2 > Math.max(junction, stop) || (along[s] + along[s + 1]) / 2 < Math.min(junction, stop)) break;
        line.inside[s] = guest.kind === 'side' ? MOUTH : INSIDE;
    }
    if (className(line) === 'steps') {
        guest.z = heightAt(line, along, stop);
        return;
    }
    returnTo(line, held, step, guest.z, room, true);
    if (guest.kind === 'side' && line.plan) {
        cutLine(line, [held + step * WARP_M]);
        const metres = alongOf(line), into = (k: number) => (metres[k] - held) * step;
        for (let k = 0; k < line.z.length; k++) if ((metres[k] - junction) * step > 1e-3 && into(k) < WARP_M - 1e-3) line.plan.bank[k] *= smooth(Math.max(0, into(k)) / WARP_M);
    }
    // Toward its junction the guest runs straight from the junction's height, so a mouth's curve holds no bulge over the stamp's chords.
    const after = alongOf(line), s = after.findIndex(m => Math.abs(m - held) < 1e-3), [a, b] = [Math.min(v, s), Math.max(v, s)], rise = (line.z[b] - line.z[a]) / ((after[b] - after[a]) || 1);
    for (let k = a + 1; k < b; k++) line.z[k] = line.z[a] + rise * (after[k] - after[a]);
    // A line that runs on through the junction keeps its grade there.
    if (line.plan) for (let k = a; k <= b; k++) if (k !== s && (k !== v || v === 0 || v === line.z.length - 1)) line.plan.grade[k] = rise;
    guest.z = heightAt(line, after, stop);
}

/**
 * A corner's heights for the stamp, on its lines as they stand after the plan and the junctions' planes: at each end
 * the kerb's height, the slope of its way's surface per metre east and south, and the sidewalk's width; its surface
 * code, and the profile of the narrower way.
 */
export function cornerStamp({ y, p, q, sp, sq, a, b }: Corner, partners: ReturnType<typeof partnersOf>) {
    const unit = metresPerUnit(y), end = ({ line }: Arm, side: 1 | -1, foot: [number, number]) => {
        let best = { s: 0, u: 0, d: Infinity };
        for (let s = 0; s + 1 < line.z.length; s++) {
            const [ax, ay, bx, by] = line.xy.slice(2 * s, 2 * s + 4), dx = bx - ax, dy = by - ay, u = Math.max(0, Math.min(1, ((foot[0] - ax) * dx + (foot[1] - ay) * dy) / (dx * dx + dy * dy || 1)));
            const d = Math.hypot(ax + u * dx - foot[0], ay + u * dy - foot[1]);
            if (d < best.d) best = { s, u, d };
        }
        const { s, u } = best, dx = line.xy[2 * s + 2] - line.xy[2 * s], dy = line.xy[2 * s + 3] - line.xy[2 * s + 1], l = Math.hypot(dx, dy) || 1;
        const h = halfAt(line, s, partners) + (halfAt(line, s + 1, partners) - halfAt(line, s, partners)) * u, tilt = crossSlope(line, s, u * l * unit, (1 - u) * l * unit), grade = (line.z[s + 1] - line.z[s]) / (l * unit);
        // Up the segment by its grade, and down to its left by its tilt.
        return { z: line.z[s] + (line.z[s + 1] - line.z[s]) * u - side * h * tilt, grad: [(grade * dx - tilt * dy) / l, (grade * dy + tilt * dx) / l] as [number, number], walk: walkAt(line, s, side) };
    };
    const ea = end(p, sp, a), eb = end(q, sq, b);
    return { z: [ea.z, eb.z], grad: [ea.grad, eb.grad], walk: [ea.walk, eb.walk], code: p.line.code === q.line.code ? p.line.code : ROAD_CODE, profile: (p.line.h <= q.line.h ? p.line : q.line).profile };
}
