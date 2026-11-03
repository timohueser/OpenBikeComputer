// The plan of a tile's junctions, a pure function of its lines. At each point that roads and paths share, the through
// roads are the hosts: they keep their shape. Each other way is a guest: a path stops at its host's outer edge, a
// driveway at the outer edge of the sidewalk that it crosses, and a side road at the host's carriageway edge, between
// two corners whose arcs turn the kerb from the one way to the other. A guest stands on its host's plane where it stops
// and returns to its own heights past it. The stamp and the corridors read this one plan: its guests, its corners, and
// the sidewalk that a side road takes round a corner from its host.
import { pointOn, spanOf } from '../stream/curve';
import { SIDE_BACK, SIDE_ON } from '../stream/features';
import { BRIDGE, crossSlope, isDeck, isPath, KERB_RISE_M, metresPerUnit, mitre, PATH, PAVED_CODES, PROFILES, RAIL, RAIL_CODE, ROAD, ROAD_CODE, SHOULDER_M, soil, SURFACES, TUNNEL, walkOf, WARP_M, type Line } from './bins';
import { boxOf, halfAt, inCorner, partnersOf } from './corners';
import { gridOf, LEFT, near, RIGHT, type Grid } from './town';

/** The plan reads and changes this many metres of each way out of a junction, so each tile whose buffer holds the junction plans it alike. */
export const WINDOW_M = 16;
/** A way's direction is toward its point this far on, as in the bake. A host's seam and a corner's edges run on this far behind its junction. */
const LOOK_M = 8, BEHIND_M = 0.5;
/** Two ways out of a point at this cosine or less run on into each other. */
const RUNS_ON_COS = -0.5;
/**
 * A guest's return to its own heights: the most grade that it adds, and where the room is too short for that, the most for a
 * guest, which must stand on its host where it stops; its fewest metres and its chords. A change under SAME_M stays out.
 */
export const RETURN_GRADE = 0.15, GUEST_GRADE = 0.25, RETURN_LEAST_M = 2, RETURN_SHORTEST_M = 0.5, RETURN_CHORDS = 3, SAME_M = 0.01, STOP_CLEAR_M = 2;
/** The most metres of a way that a corner takes, and its share of the way to the way's end or next junction near by. */
export const ROOM_M = 14;
const ROOM_SHARE = 0.45;
/** Corner radii: a paved road's edge or kerb, a track's; the least arc. A kerbed arc clears its sidewalk by WALK_CLEAR_M. */
const PAVED_R = 5, TRACK_R = 4, LEAST_R = 0.8, WALK_CLEAR_M = 0.6;
/** An arc tries a smaller radius by this factor while its ends leave a way's room. */
const SHRINK = 0.8;
/** A corner's edges run on straight this far behind its junction: a side road that leaves at a wide angle meets the through road's edge behind it. */
export const EDGE_BEHIND_M = ROOM_M;
/** The widest turn between two ways that makes a corner where neither is a side road, by their tangents, and where one is, by their directions. */
const MAX_TURN = (150 * Math.PI) / 180, MAX_SIDE_TURN = (172 * Math.PI) / 180;
/** A side road's carriageway needs this many metres past its stop. */
const MOUTH_ROOM_M = 1;
/** A mouth's plane takes the through ways' slope where its points' angle at the junction has a sine under this. */
const ONE_LINE_SIN = 0.2;
/** A sidewalk ends over this many metres, about two slabs: its kerb turns to its back edge at about 30°. */
/** A side road's own sidewalk where its host has none ends this far past the arc's end, so the arc's end column stands on its plain section. */
const AFTER_ARC_M = 0.05;
export const WALK_END_M = 3;
/** A path or a driveway stops this far past where its whole core has left its host: its fill ramps from its own surface, its bed and its lift, to the host's edge. */
export const FILL_M = 0.5;
/** Per segment, what `Line.inside` holds: a path or a driveway yields its core and its paint to every way; a side road's mouth only to the carriageways. */
export const GUEST_INSIDE = 1, GUEST_MOUTH = 2;

/** A way out of a shared vertex: its unit direction in metres east and south toward its point LOOK_M on, and its angle; the metres that a corner may take of it. */
export interface Arm { line: Line; v: number; step: 1 | -1; u: [number, number]; angle: number; through: boolean; room: number }

/** A shared point, `unit` metres per world unit there, with its arms sorted by angle. */
export interface Meeting { x: number; y: number; unit: number; arms: Arm[] }

/**
 * A guest: the way out of the junction at world point x, y, `step` along its line. A path stops at its host's outer
 * edge, a driveway at the outer edge of the sidewalk that it crosses, and a side road at the host's carriageway edge;
 * `seam` metres from the host's middle on the host line's `side` (1 its left). It stops `stop` metres along its own
 * line, where its whole core has left that edge, at height `z`, the host's plane under its middle; a side road on the
 * plane of its mouth, `plane` per metre east and south.
 */
export interface Guest { kind: 'path' | 'driveway' | 'side'; line: Line; x: number; y: number; step: 1 | -1; host: Line; side: 1 | -1; seam: number; walk: boolean; stop: number; z: number; plane?: [number, number] }

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
 * follow its curve. Where the room is too short, the return takes all of it, at least RETURN_SHORTEST_M, at `steep` at
 * most, and the change shrinks past that. Returns the height that the line takes at `at`.
 */
export function returnTo(line: Line, at: number, dir: 1 | -1, z: number, room: number, steep = RETURN_GRADE): number {
    const own = heightAt(line, alongOf(line), at), gentle = Math.max(RETURN_LEAST_M, Math.abs(z - own) / RETURN_GRADE);
    if (gentle > room && steep === RETURN_GRADE && room < RETURN_LEAST_M) return own;
    const over = gentle <= room ? gentle : Math.max(RETURN_SHORTEST_M, room), most = gentle <= room ? Infinity : steep * over, change = Math.max(-most, Math.min(most, z - own));
    if (Math.abs(change) < SAME_M) return own;
    cutLine(line, Array.from({ length: RETURN_CHORDS + 1 }, (_, k) => at + (dir * over * k) / RETURN_CHORDS));
    alongOf(line).forEach((metres, v) => {
        const t = (metres - at) * dir / over;
        if (t < -1e-3 || t > 1 + 1e-3) return;
        const u = Math.min(1, Math.max(0, t));
        line.z[v] += change * (1 - smooth(u));
        // The curve between two points holds their grades, so it is the same cubic as the return's.
        if (line.plan) line.plan.grade[v] -= (dir * change * 6 * u * (1 - u)) / over;
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
    return { points: [[p[0] - (q[0] - p[0]) * EDGE_BEHIND_M / l, p[1] - (q[1] - p[1]) * EDGE_BEHIND_M / l], ...points], metres: [-EDGE_BEHIND_M, ...metres] };
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
function cornerOf(meeting: Meeting, p: Arm, q: Arm, partners: ReturnType<typeof partnersOf>, clear: (corner: Corner) => boolean): Corner | undefined {
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
    if (oneLine || p.line.kind === BRIDGE || q.line.kind === BRIDGE) return corner(pointAt(ep(0), met.a), 0, met.a, met.b);
    const arc = (r: number) => {
        const offset = [ep(r), eq(r)], hit = edgesCross(offset[0], offset[1]);
        return hit && { o: pointAt(offset[0], hit.a), ...hit };
    };
    // An open corner, whose arc would end behind the junction, turns no kerb: the ways' bands meet on the ground.
    let r = Math.max(paved ? PAVED_R : TRACK_R, least), hit = arc(r);
    for (; ; r = Math.max(r * SHRINK, least), hit = arc(r)) {
        if (hit && (r === least || (hit.a <= p.room && hit.b <= q.room)) && hit.a >= 0 && hit.b >= 0) {
            const round = corner(hit.o, r, hit.a, hit.b);
            if (clear(round)) return round;
            break;
        }
        if (r === least || (hit && Math.min(hit.a, hit.b) < 0)) break;
    }
    return corner(pointAt(ep(0), met.a), 0, met.a, met.b);
}

/** Every segment of a tile's lines, and their boxes grown by the line's half-width in a grid. */
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

/** Whether the core of a line but `own`, the junction's ways and the ways that meet them, reaches into a corner off both its roads, at points about a metre apart along and across each segment: a stream, a railway or a road that passes. */
function reached({ list, grid }: Segments, corner: Corner, own: Set<Line>): boolean {
    const unit = metresPerUnit(corner.o[1]), [x0, y0, x1, y1] = boxOf(corner);
    let hit = false;
    near(grid, x0, y0, x1, y1, 0, k => {
        const [line, v] = list[k];
        if (hit || own.has(line)) return;
        const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), dx = bx - ax, dy = by - ay, l = Math.hypot(dx, dy) || 1;
        const along = Math.ceil(l * unit), across = Math.max(1, Math.ceil(2 * line.h));
        for (let i = 0; i <= along && !hit; i++) for (let j = 0; j <= across && !hit; j++) {
            const t = i / along, side = ((2 * j) / across - 1) * line.h / unit / l;
            hit = inCorner(corner, ax + t * dx - dy * side, ay + t * dy + dx * side, 0);
        }
    });
    return hit;
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
function cornersAt(meeting: Meeting, crossings: Arm[], partners: ReturnType<typeof partnersOf>, segments: Segments, own: Set<Line>): { corners: Corner[]; sides: Map<Arm, number> } {
    const roads = meeting.arms.filter(({ line }) => line.kind !== PATH), pair = roads.filter(({ through }) => through), sides = new Map<Arm, number>();
    // A pair of soil has no surface to join: the ruts cross, and the ground shows between.
    if (roads.length < 3 || pair.length !== 2 || roads.filter(({ line }) => corridor(line)).length < 2 || pair.every(({ line }) => soil(line))) return { corners: [], sides };
    const ways = roads.filter(arm => !crossings.includes(arm)), found = ways.flatMap((p, i) => {
        const q = ways[(i + 1) % ways.length], turn = q.angle - p.angle + (i + 1 === ways.length ? 2 * Math.PI : 0);
        if ((!p.through && !q.through) || !corridor(p.line) || !corridor(q.line)) return [];
        if (p.through && q.through ? tangentCos(p, q) <= Math.cos(MAX_TURN) : turn >= MAX_SIDE_TURN) return [];
        return cornerOf(meeting, p, q, partners, round => !reached(segments, round, own)) ?? [];
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
 * edges of a side road meet them, so a mouth between two through ways of different grades lies on one surface. Where
 * the three points nearly lie on one line (their angle at the junction under ONE_LINE_SIN), the mean slope of the
 * through ways holds. Gives its rise per metre east and south, and its height at a point.
 */
function junctionPlane({ x, y, unit, arms }: Meeting, meets: { at: [number, number]; z: number }[], at: { x: number; y: number }): { plane: [number, number]; z: number } {
    const through = arms.filter(({ through, line }) => through && line.kind === ROAD), z0 = through.reduce((sum, { line, v }) => sum + line.z[v], 0) / (through.length || 1);
    const [[ax, ay], [bx, by]] = meets.map(({ at: [px, py] }) => [(px - x) * unit, (py - y) * unit]), [da, db] = meets.map(({ z }) => z - z0), det = ax * by - ay * bx;
    let plane: [number, number] = [(da * by - db * ay) / det, (db * ax - da * bx) / det];
    if (!(Math.abs(det) >= ONE_LINE_SIN * Math.hypot(ax, ay) * Math.hypot(bx, by))) {
        // A through way's slope: its grade along its tangent and its bank, a rise to its left: y runs south.
        const slopes = through.map(({ line, v }) => {
            const [tx, ty] = mitre(line, v), grade = line.plan?.grade[v] ?? 0, bank = line.plan?.bank[v] ?? 0;
            return [grade * tx + bank * ty, grade * ty - bank * tx];
        });
        plane = [0, 1].map(k => slopes.reduce((sum, g) => sum + g[k], 0) / (slopes.length || 1)) as [number, number];
    }
    return { plane, z: z0 + plane[0] * (at.x - x) * unit + plane[1] * (at.y - y) * unit };
}

/**
 * A side road's sidewalk on a corner's side matches its host's round the arc. Where the host has one there and the
 * side road none, the side road takes the host's: from its junction to WALK_END_M past the arc's end, where it ends,
 * within its room before its next junction, or on into its own sidewalk where that starts within three such ends. A
 * class with no sidewalk takes the host's width (`Line.walk`). Where the side road has one and the host none, the side
 * road's sidewalk ends after the arc. Gives the stretch of the side road's line, in metres from its first vertex, that
 * takes a sidewalk on `side` or loses it.
 */
function stubOf(corner: Corner, meeting: Meeting, shared: (name: string) => boolean): { line: Line; from: number; to: number; side: 1 | -1; walk: boolean } | undefined {
    if (corner.r <= 0 || corner.p.through === corner.q.through) return undefined;
    const [host, hostSide, end, guest, side, guestEnd] = corner.p.through ? [corner.p, corner.sp, corner.ends[0], corner.q, corner.sq, corner.ends[1]] : [corner.q, corner.sq, corner.ends[1], corner.p, corner.sp, corner.ends[0]];
    const width = walkAt(host.line, pointAlong(host.line, alongOf(host.line), end).v, hostSide), along = alongOf(guest.line), from = along[guest.v], arc = Math.abs(guestEnd - from);
    const own = (metres: number) => walkAt(guest.line, pointAlong(guest.line, along, from + guest.step * metres).v, side) > 0;
    if (!width) return own(arc) ? { line: guest.line, from, to: from + guest.step * (arc + AFTER_ARC_M), side, walk: false } : undefined;
    let to = Math.min(arc + WALK_END_M, reachOf(guest, shared, meeting.unit, 2 * WINDOW_M) - STOP_CLEAR_M);
    for (let m = 0.05; m < arc + 3 * WALK_END_M; m += 0.25) if (own(m)) {
        if (m <= arc) return undefined;
        to = m;
        break;
    }
    if (to < arc + 1) return undefined;
    if (!Object.values(PROFILES)[guest.line.profile].walk) guest.line.walk = Math.max(guest.line.walk ?? 0, width);
    return { line: guest.line, from, to: from + guest.step * to, side, walk: true };
}

/**
 * Plans a tile's junctions and applies the plan to its lines: each guest is cut at its stop, its segments up to the
 * stop are marked inside, and it returns to its own heights past the stop; a side road takes its host's sidewalk round
 * each corner where it has none. Steps keep their own heights.
 */
export function junctionPlan(lines: Line[]): JunctionPlan {
    const partners = partnersOf(lines), segments = segmentsOf(lines), guests: Guest[] = [], corners: Corner[] = [], stubs: NonNullable<ReturnType<typeof stubOf>>[] = [];
    const sharedNames = new Map<string, number>();
    for (const line of lines) if (counts(line)) line.z.forEach((_, v) => sharedNames.set(nameOf(line, v), (sharedNames.get(nameOf(line, v)) ?? 0) + 1));
    const shared = (name: string) => (sharedNames.get(name) ?? 0) > 1, linesAt = new Map<string, Line[]>();
    for (const line of lines) line.z.forEach((_, v) => (linesAt.get(nameOf(line, v)) ?? linesAt.set(nameOf(line, v), []).get(nameOf(line, v))!).push(line));
    for (const meeting of meetingsOf(lines)) {
        const { x, y, unit, arms } = meeting, hosts = arms.filter(({ through, line }) => through && line.kind === ROAD);
        if (!hosts.length) continue;
        const dirs = new Map(arms.map(arm => [arm, arm.u]));
        // A way that runs on from a road that ends, or across its end, is no guest of it.
        const runsOn = (arm: Arm) => arms.some(other => other !== arm && dirs.get(other)![0] * arm.u[0] + dirs.get(other)![1] * arm.u[1] < RUNS_ON_COS);
        const crossings = arms.filter(arm => !arm.through && arm.line.kind === ROAD && arm.line.driveway && crossesWalk(arm, hosts, unit));
        const own = new Set(arms.flatMap(({ line }) => line.xy.flatMap((_, i) => (i % 2 ? [] : linesAt.get(`${line.xy[i]},${line.xy[i + 1]}`) ?? []))));
        const { corners: found, sides } = cornersAt(meeting, crossings, partners, segments, own);
        corners.push(...found);
        for (const corner of found) {
            const stub = stubOf(corner, meeting, shared);
            if (stub) stubs.push(stub);
        }
        for (const [arm, metres] of sides) {
            const [before, after] = [found.find(({ q }) => q === arm)!, found.find(({ p }) => p === arm)!], hostOf = ({ p, q }: Corner) => (p.through ? p : q);
            const along = alongOf(arm.line), stop = along[arm.v] + arm.step * metres;
            const { plane, z } = junctionPlane(meeting, [before, after].map(corner => ({ at: corner.m, z: planeUnder(hostOf(corner).line, corner.m[0], corner.m[1], unit).z })), pointAlong(arm.line, along, stop));
            const host = hostOf(before);
            guests.push({ kind: 'side', line: arm.line, x, y, step: arm.step, host: host.line, side: sideOfHost(host, arm.u, unit), seam: host.line.h, walk: false, stop, z, plane });
        }
        for (const arm of arms) {
            const path = arm.line.kind === PATH;
            if (!(path || crossings.includes(arm)) || (hosts.length < 2 && runsOn(arm))) continue;
            // A guest stops where its whole core has left the host: past there its faces lie on no face of the host.
            const core = arm.line.h + shoulderOf(arm.line);
            let stop = stopOf(arm, hosts, unit);
            if (!stop) continue;
            for (const edge of [stopOf(arm, hosts, unit, core), stopOf(arm, hosts, unit, -core)]) if (edge && edge.metres > stop.metres) stop = edge;
            const along = alongOf(arm.line), end = arm.step > 0 ? along[along.length - 1] - along[arm.v] : along[arm.v];
            const metres = along[arm.v] + arm.step * Math.min(stop.metres + (stop.seam ? FILL_M : 0), end), middle = pointAlong(arm.line, along, metres);
            // A way that ends inside its host keeps its own height.
            const z = stop.seam ? planeUnder(stop.host, middle.x, middle.y, unit).z + (stop.walk ? KERB_RISE_M : 0) : heightAt(arm.line, along, metres);
            guests.push({ kind: path ? 'path' : 'driveway', line: arm.line, x, y, step: arm.step, host: stop.host, side: stop.side, seam: stop.seam, walk: stop.walk, stop: metres, z });
        }
    }
    for (const { line, from, to, side, walk } of stubs) {
        cutLine(line, [to]);
        const along = alongOf(line), bit = side > 0 ? LEFT : RIGHT;
        line.sidewalks ??= new Array(line.z.length - 1).fill(0);
        for (let s = 0; s + 1 < along.length; s++) if ((along[s] + along[s + 1]) / 2 > Math.min(from, to) && (along[s] + along[s + 1]) / 2 < Math.max(from, to)) line.sidewalks[s] = walk ? line.sidewalks[s] | bit : line.sidewalks[s] & ~bit;
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
 * Cuts a guest at its stop, marks it inside up to there, and gives it its height and its return. Up to the stop it runs
 * straight from the junction's height; a side road lies there with no bank of its own, and takes its bank back over
 * WARP_M, as the junction's planes give back its slope across.
 */
function settle(guest: Guest, shared: Map<string, number>, guestAt: (line: Line, v: number) => boolean): void {
    const { line, x, y, step, stop } = guest;
    cutLine(line, [stop]);
    // A line that closes on itself, or passes its junction twice, holds the point at more than one vertex: the guest's is the nearest behind its stop.
    const along = alongOf(line), at = along.map((_, k) => k).filter(k => line.xy[2 * k] === x && line.xy[2 * k + 1] === y && (stop - along[k]) * step >= 0);
    const v = at.reduce((best, k) => (Math.abs(stop - along[k]) < Math.abs(stop - along[best]) ? k : best), at[0]), junction = along[v];
    // A return ends STOP_CLEAR_M short of the next shared vertex, whose heights another way reads there. A line's other end says nothing: the tile may cut it there.
    let room = WINDOW_M - Math.abs(stop - junction);
    for (let k = v + step; k >= 0 && k < line.z.length && Math.abs(along[k] - junction) < 2 * WINDOW_M; k += step) {
        if ((shared.get(nameOf(line, k)) ?? 0) < 2) continue;
        const metres = Math.abs(along[k] - junction);
        room = Math.min(room, (guestAt(line, k) ? metres / 2 : metres - STOP_CLEAR_M) - Math.abs(stop - junction));
        break;
    }
    line.inside ??= new Array(line.z.length - 1).fill(0);
    for (let s = Math.min(v, v + step); s >= 0 && s + 1 < line.z.length; s += step) {
        if ((along[s] + along[s + 1]) / 2 > Math.max(junction, stop) || (along[s] + along[s + 1]) / 2 < Math.min(junction, stop)) break;
        line.inside[s] = guest.kind === 'side' ? GUEST_MOUTH : GUEST_INSIDE;
    }
    if (className(line) === 'steps') {
        guest.z = heightAt(line, along, stop);
        return;
    }
    guest.z = returnTo(line, stop, step, guest.z, room, GUEST_GRADE);
    if (guest.kind === 'side' && line.plan) {
        cutLine(line, [stop + step * WARP_M]);
        const metres = alongOf(line), into = (k: number) => (metres[k] - stop) * step;
        for (let k = 0; k < line.z.length; k++) if ((metres[k] - junction) * step > 1e-3 && into(k) < WARP_M - 1e-3) line.plan.bank[k] *= smooth(Math.max(0, into(k)) / WARP_M);
    }
    // Toward its junction the guest runs straight from the junction's height, so a mouth's curve holds no bulge over the stamp's chords.
    const after = alongOf(line), s = after.findIndex(m => Math.abs(m - stop) < 1e-3), [a, b] = [Math.min(v, s), Math.max(v, s)], rise = (line.z[b] - line.z[a]) / ((after[b] - after[a]) || 1);
    for (let k = a + 1; k < b; k++) line.z[k] = line.z[a] + rise * (after[k] - after[a]);
    // A line that runs on through the junction keeps its grade there.
    if (line.plan) for (let k = a; k <= b; k++) if (k !== s && (k !== v || v === 0 || v === line.z.length - 1)) line.plan.grade[k] = rise;
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
