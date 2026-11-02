// The plan of a tile's junctions, a pure function of its lines. At each point that roads and paths share, the through
// pair of roads are the hosts: they keep their shape. A path, or a driveway across a sidewalk, is a guest: it stops where
// its middle line crosses its host's seam, at the host's height there, and returns to its own heights past it. Between
// the junction and the stop it is inside: the stamp gives it no record, and the corridor no faces, but a fill to the seam.
// The stamp and the corridors read the one plan.
import { pointOn, spanOf } from '../stream/curve';
import { SIDE_BACK, SIDE_ON } from '../stream/features';
import { DRIVEWAYS, KERB_RISE_M, metresPerUnit, PATH, PROFILES, ROAD, SHOULDER_M, SURFACES, walkOf, type Line } from './bins';
import { LEFT, RIGHT } from './town';

/** The plan reads and changes this many metres of each way out of a junction, so each tile whose buffer holds the junction plans it alike. */
export const WINDOW_M = 16;
/** A way's direction is toward its point this far on, as in the bake. A host's seam runs on this far behind its junction. */
const LOOK_M = 8, BEHIND_M = 0.5;
/** Two ways out of a point at this cosine or less run on into each other. */
const RUNS_ON_COS = -0.5;
/** A guest's return to its own heights: the most grade that it adds, its fewest metres and its chords. A change under SAME_M stays out. */
export const RETURN_GRADE = 0.15, RETURN_LEAST_M = 2, RETURN_CHORDS = 3, SAME_M = 0.01;

/**
 * A guest: the way out of the junction at world point x, y, `step` along its line. Its middle crosses the seam of its
 * host on the host line's `side` (1 its left) `stop` metres along its own line, `seam` metres from the host's middle: a
 * sidewalk's outer edge where `walk`, else the shoulder's or the carriageway's. There it stands at height `z`. A
 * driveway that crosses no sidewalk keeps its mouth: it takes only the height and the return, and nothing is inside.
 */
export interface Guest { line: Line; x: number; y: number; step: 1 | -1; host: Line; side: 1 | -1; seam: number; walk: boolean; stop: number; z: number; mouth: boolean }

export interface JunctionPlan { guests: Guest[] }

const smooth = (t: number) => t * t * (3 - 2 * t);
const nameOf = (line: Line, v: number) => `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;
const className = (line: Line) => Object.keys(PROFILES)[line.profile];
const shoulderOf = (line: Line) => SHOULDER_M[(Object.keys(SURFACES) as (keyof typeof SURFACES)[])[(line.code & 15) - 1]] ?? 0;

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
    const out = { xy: [line.xy[0], line.xy[1]], z: [line.z[0]], anchors: [line.anchors?.[0] ?? true], sideWays: line.sideWays && [line.sideWays[0]], sidewalks: line.sidewalks && ([] as number[]), inside: line.inside && ([] as boolean[]), plan: plan && { tangent: [plan.tangent[0]], grade: [plan.grade[0]], bank: [plan.bank[0]], widen: [plan.widen[0]] } };
    const push = (x: number, y: number, z: number, anchor: boolean, side: number, v: number, on?: { tangent: number; grade: number }, u = 1) => {
        out.xy.push(x, y);
        out.z.push(z);
        out.anchors.push(anchor);
        out.sideWays?.push(side);
        if (line.sidewalks) out.sidewalks!.push(line.sidewalks[v]);
        if (line.inside) out.inside!.push(!!line.inside[v]);
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
 * follow its curve. Where the room is too short, the change shrinks. Returns the height that the line takes at `at`.
 */
export function returnTo(line: Line, at: number, dir: 1 | -1, z: number, room: number): number {
    const own = heightAt(line, alongOf(line), at);
    if (room < RETURN_LEAST_M) return own;
    const change = Math.max(-RETURN_GRADE * room, Math.min(RETURN_GRADE * room, z - own)), over = Math.max(RETURN_LEAST_M, Math.abs(change) / RETURN_GRADE);
    if (Math.abs(change) < SAME_M) return own;
    cutLine(line, Array.from({ length: RETURN_CHORDS + 1 }, (_, k) => at + (dir * over * k) / RETURN_CHORDS));
    const along = alongOf(line);
    along.forEach((metres, v) => {
        const t = (metres - at) * dir / over;
        if (t < -1e-3 || t > 1 + 1e-3) return;
        const u = Math.min(1, Math.max(0, t));
        line.z[v] += change * (1 - smooth(u));
        // The curve between two points holds their grades, so it is the same cubic as the return's.
        if (line.plan) line.plan.grade[v] -= (dir * change * 6 * u * (1 - u)) / over;
    });
    return own + change;
}

/** A way out of a shared vertex. */
interface Arm { line: Line; v: number; step: 1 | -1; through: boolean }

/** A unit direction out of vertex v of a line, toward its point LOOK_M on, in metres east and south. */
function towards({ xy, z }: Line, v: number, step: 1 | -1, unit: number): [number, number] {
    let left = LOOK_M, px = xy[2 * v], py = xy[2 * v + 1];
    for (let k = v + step; k >= 0 && k < z.length && left > 0; k += step) {
        const piece = Math.hypot(xy[2 * k] - px, xy[2 * k + 1] - py) * unit, u = Math.min(1, left / (piece || 1));
        [px, py, left] = [px + u * (xy[2 * k] - px), py + u * (xy[2 * k + 1] - py), left - piece];
    }
    const dx = (px - xy[2 * v]) * unit, dy = (py - xy[2 * v + 1]) * unit, l = Math.hypot(dx, dy) || 1;
    return [dx / l, dy / l];
}

/** The metres from a host's middle to its seam on `side` of segment s: a sidewalk's outer edge, else the shoulder's. */
function seamOf(host: Line, s: number, side: 1 | -1): { seam: number; walk: boolean } {
    const walk = (host.sidewalks?.[s] ?? 0) & (side > 0 ? LEFT : RIGHT) ? walkOf(host) : 0;
    return { seam: host.h + (walk || shoulderOf(host)), walk: walk > 0 };
}

/** A host's seam on `side` from its junction out: per segment of its line within WINDOW_M, the segment's chord moved out by its seam, the first one run back by BEHIND_M. */
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

/** Where a guest's middle first crosses a host's seam, within WINDOW_M: metres along the guest from its junction, the host, its side and the host's plane there. */
function stopOf(guest: Arm, hosts: Arm[], unit: number): Omit<Guest, 'line' | 'x' | 'y' | 'step' | 'stop' | 'z' | 'mouth'> & { metres: number; plane: number } | undefined {
    const { line, v, step } = guest, g = towards(line, v, step, unit);
    const seams = hosts.map(host => {
        // The guest's side of the host's line, whose own direction `step` turns.
        const [hx, hy] = towards(host.line, host.v, host.step, unit), side = (host.step * (g[0] * hy - g[1] * hx) > 0 ? 1 : -1) as 1 | -1;
        return { host, side, lines: seamLines(host, side, unit) };
    });
    let k = v, metres = 0;
    for (; k + step >= 0 && k + step < line.z.length && metres < WINDOW_M; k += step) {
        const p = line.xy.slice(2 * k, 2 * k + 2), q = line.xy.slice(2 * (k + step), 2 * (k + step) + 2), l = Math.hypot(q[0] - p[0], q[1] - p[1]) * unit;
        let best: { t: number; found: NonNullable<ReturnType<typeof stopOf>> } | undefined;
        for (const { host, side, lines } of seams) for (const { s, a, b, seam, walk } of lines) {
            const hit = crossing(p, q, a, b);
            if (!hit || (best && hit.t >= best.t)) continue;
            // The host's plane at the crossing's foot on its segment, held level past the carriageway's edge.
            const h = host.line, [ax, ay, bx, by] = h.xy.slice(2 * s, 2 * s + 4), cx = p[0] + hit.t * (q[0] - p[0]), cy = p[1] + hit.t * (q[1] - p[1]);
            const u = Math.max(0, Math.min(1, ((cx - ax) * (bx - ax) + (cy - ay) * (by - ay)) / ((bx - ax) ** 2 + (by - ay) ** 2 || 1))), mix = (w: number[]) => w[s] + u * (w[s + 1] - w[s]);
            best = { t: hit.t, found: { host: h, side, seam, walk, metres: metres + hit.t * l, plane: mix(h.z) + (h.plan ? mix(h.plan.bank) : 0) * side * h.h } };
        }
        if (best) return best.found;
        metres += l;
    }
    // A way that ends inside its hosts, as a mapped crossing does, is inside to its end, at its own height.
    const [ex, ey] = line.xy.slice(2 * k, 2 * k + 2), within = seams.some(({ host: { line: h }, lines }) => lines.some(({ s, seam }) => {
        const [ax, ay, bx, by] = h.xy.slice(2 * s, 2 * s + 4), dx = bx - ax, dy = by - ay, l = Math.hypot(dx, dy) || 1, t = ((ex - ax) * dx + (ey - ay) * dy) / l / l;
        return t >= 0 && t <= 1 && Math.abs((ex - ax) * dy - (ey - ay) * dx) / l * unit < seam;
    }));
    return metres < WINDOW_M && within ? { host: seams[0].host.line, side: seams[0].side, seam: 0, walk: false, metres, plane: line.z[k] } : undefined;
}

/** Whether a driveway out of a junction crosses a host's sidewalk: a host has one on its side, and the driveway has none of its own near. */
function crossesWalk(guest: Arm, hosts: Arm[], unit: number): boolean {
    const g = towards(guest.line, guest.v, guest.step, unit);
    const own = (line: Line, v: number, step: number) => {
        for (let k = v, metres = 0; k + step >= 0 && k + step < line.z.length && metres < WINDOW_M; k += step) {
            if (line.sidewalks?.[Math.min(k, k + step)]) return true;
            metres += Math.hypot(line.xy[2 * (k + step)] - line.xy[2 * k], line.xy[2 * (k + step) + 1] - line.xy[2 * k + 1]) * unit;
        }
        return false;
    };
    return !own(guest.line, guest.v, guest.step) && hosts.some(host => {
        const [hx, hy] = towards(host.line, host.v, host.step, unit), side = host.step * (g[0] * hy - g[1] * hx) > 0 ? LEFT : RIGHT;
        return ((host.line.sidewalks?.[Math.min(host.v, host.v + host.step)] ?? 0) & side) !== 0;
    });
}

/**
 * Plans a tile's junctions and applies the plan to its lines: each guest is cut at its stop, its segments up to the
 * stop are marked inside, and it returns to its own heights past the stop. Steps keep their own heights.
 */
export function junctionPlan(lines: Line[]): JunctionPlan {
    const counted = lines.filter(({ kind, gallery, z }) => (kind === ROAD || kind === PATH) && !gallery && z.length > 1), at = new Map<string, { line: Line; v: number }[]>();
    for (const line of counted) line.z.forEach((_, v) => (at.get(nameOf(line, v)) ?? at.set(nameOf(line, v), []).get(nameOf(line, v))!).push({ line, v }));
    const guests: Guest[] = [];
    for (const [name, here] of at) {
        if (here.length < 2) continue;
        const arms: Arm[] = here.flatMap(({ line, v }) => ([[-1, SIDE_BACK], [1, SIDE_ON]] as const).filter(([step]) => v + step >= 0 && v + step < line.z.length)
            .map(([step, bit]) => ({ line, v, step, through: line.kind === ROAD && !((line.sideWays?.[v] ?? 0) & bit) })));
        const hosts = arms.filter(({ through }) => through);
        if (!hosts.length) continue;
        const [x, y] = name.split(',').map(Number), unit = metresPerUnit(y), dirs = new Map(arms.map(arm => [arm, towards(arm.line, arm.v, arm.step, unit)]));
        // A way that runs on from a road that ends, or across its end, is no guest of it.
        const runsOn = (arm: Arm) => arms.some(other => other !== arm && dirs.get(other)![0] * dirs.get(arm)![0] + dirs.get(other)![1] * dirs.get(arm)![1] < RUNS_ON_COS);
        for (const arm of arms) {
            const type = className(arm.line), path = arm.line.kind === PATH;
            if (arm.through || (!path && !DRIVEWAYS.includes(type)) || (hosts.length < 2 && runsOn(arm))) continue;
            const stop = stopOf(arm, hosts, unit);
            if (!stop) continue;
            const along = alongOf(arm.line), metres = along[arm.v] + arm.step * stop.metres, mouth = !path && !crossesWalk(arm, hosts, unit);
            guests.push({ line: arm.line, x, y, step: arm.step, host: stop.host, side: stop.side, seam: stop.seam, walk: stop.walk && !mouth, stop: metres, z: stop.plane + (stop.walk && !mouth ? KERB_RISE_M : 0), mouth });
        }
    }
    // Any line that shares a vertex holds its heights there: a deck, a tunnel or a gallery too.
    const shared = new Map<string, number>();
    for (const line of lines) line.z.forEach((_, v) => shared.set(nameOf(line, v), (shared.get(nameOf(line, v)) ?? 0) + 1));
    for (const guest of guests) settle(guest, shared);
    return { guests };
}

/** Cuts a guest at its stop, marks it inside up to there, and gives it its height and its return. */
function settle(guest: Guest, shared: Map<string, number>): void {
    const { line, x, y, step, stop } = guest;
    cutLine(line, [stop]);
    const along = alongOf(line), v = line.xy.findIndex((value, i) => i % 2 === 0 && value === x && line.xy[i + 1] === y) / 2, junction = along[v];
    // A return keeps to its half of the way to the next shared vertex. A line's other end says nothing: the tile may cut it there.
    let room = WINDOW_M - Math.abs(stop - junction);
    for (let k = v + step; k >= 0 && k < line.z.length && Math.abs(along[k] - junction) < 2 * WINDOW_M; k += step) {
        if ((shared.get(nameOf(line, k)) ?? 0) < 2) continue;
        room = Math.min(room, Math.abs(along[k] - junction) / 2 - Math.abs(stop - junction));
        break;
    }
    if (!guest.mouth) {
        line.inside ??= new Array(line.z.length - 1).fill(false);
        for (let s = Math.min(v, v + step); s >= 0 && s + 1 < line.z.length; s += step) {
            if ((along[s] + along[s + 1]) / 2 > Math.max(junction, stop) || (along[s] + along[s + 1]) / 2 < Math.min(junction, stop)) break;
            line.inside[s] = true;
        }
    }
    if (className(line) === 'steps') {
        guest.z = heightAt(line, along, stop);
        return;
    }
    guest.z = returnTo(line, stop, step, guest.z, room);
}
