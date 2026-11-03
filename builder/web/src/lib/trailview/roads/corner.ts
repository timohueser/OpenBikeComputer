// A corner between two neighbouring ways of a junction, as the junction plan makes it (terrain/junction-plan.ts), on
// the ways' stations: the arc that turns the kerb from the edge of the one to the edge of the other. The plan says
// which ways meet in a corner and its radius; the arc's centre lies on both station edges moved out by that radius,
// so each end column of its faces is its way's own section, within ROAD_SAG of the stamp's arc.
import type { Line } from '../terrain/bins';
import { EDGE_BEHIND_M, type Corner as PlanCorner } from '../terrain/junction-plan';
import { outward, stationAlong, type Meeting, type Way } from './arms';
import { sideOf, MAT } from './section';
import { stationOn } from './stations';


export interface Corner {
    meeting: Meeting;
    /** The two ways, in the order of their angles, and the side of each line that faces the corner. */
    p: Way;
    q: Way;
    sp: 1 | -1;
    sq: 1 | -1;
    /** The arc's centre in metres about the meeting, and its radius; 0 where the corner stays sharp. */
    o: number[];
    r: number;
    /** Metres along each line's stations at the arc's ends, and where the edges meet. */
    ends: [number, number];
    meet: [number, number];
    /**
     * A sharp corner's join: the metres along each way past the meet within which its side may reach the other's.
     * There each way's side gives way to the other's (junctions.ts `heldAt`).
     */
    join?: number;
    /** The arc's ends in metres about the meeting, how far from its centre the edges meet, and the sidewalk's width inside the arc. */
    feet: number[][];
    apex: number;
    walk: number;
    /** The through road whose surface its faces take. */
    road: Line;
    /** Its two ways are one line through its vertex, so it stays sharp and nothing is cut. */
    oneLine: boolean;
}

/** A way's edge on side `side` of its line, `out` metres past it: its points in metres about the meeting, and their metres from the junction. */
function edgeOf(way: Way, side: 1 | -1, out: number, { x, y, unit }: Meeting): { points: number[][]; metres: number[] } {
    const list = outward(way), points = list.map(s => {
        const a = side * (s.h + out);
        return [(s.x + s.ax * a - x) * unit, (s.y + s.ay * a - y) * unit];
    });
    const [p, q] = points, l = Math.hypot(q[0] - p[0], q[1] - p[1]) || 1;
    return { points: [[p[0] - (q[0] - p[0]) * EDGE_BEHIND_M / l, p[1] - (q[1] - p[1]) * EDGE_BEHIND_M / l], ...points], metres: [-EDGE_BEHIND_M, ...list.map(s => Math.abs(s.along - way.along))] };
}

type Edge = ReturnType<typeof edgeOf>;

/** Where edge B first crosses edge A, by the metres along A: the metres along each edge. */
function crossing(A: Edge, B: Edge): { a: number; b: number } | undefined {
    for (let i = 0; i + 1 < A.points.length; i++) {
        const [p, q] = [A.points[i], A.points[i + 1]], r = [q[0] - p[0], q[1] - p[1]];
        let best: { a: number; b: number } | undefined, least = Infinity;
        for (let k = 0; k + 1 < B.points.length; k++) {
            const [c, d] = [B.points[k], B.points[k + 1]], s = [d[0] - c[0], d[1] - c[1]], cross = r[0] * s[1] - r[1] * s[0];
            if (Math.abs(cross) < 1e-9) continue;
            const t = ((c[0] - p[0]) * s[1] - (c[1] - p[1]) * s[0]) / cross, u = ((c[0] - p[0]) * r[1] - (c[1] - p[1]) * r[0]) / cross;
            if (t < 0 || t > 1 || u < 0 || u > 1 || t >= least) continue;
            least = t;
            best = { a: A.metres[i] + t * (A.metres[i + 1] - A.metres[i]), b: B.metres[k] + u * (B.metres[k + 1] - B.metres[k]) };
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

/** A sharp corner's join reaches at most as far as at this sine of half its angle. */
const JOIN_SIN = 0.1;

const toUnit = ([x, y]: number[]) => {
    const l = Math.hypot(x, y) || 1;
    return [x / l, y / l];
};

/** An edge's direction away from the junction at `at` metres along it. */
function directionAt({ points, metres }: Edge, at: number): number[] {
    let k = 0;
    while (k + 2 < metres.length && metres[k + 1] < at) k++;
    return toUnit([points[k + 1][0] - points[k][0], points[k + 1][1] - points[k][1]]);
}

/** A plan's corner on the ways of a meeting, at the plan's radius on the ways' station edges; where those do not meet at it, the plan's own points. */
export function cornerOf(corner: PlanCorner, meeting: Meeting, p: Way, q: Way, road: Line): Corner {
    const { x, y, unit } = meeting, about = ([px, py]: number[]) => [(px - x) * unit, (py - y) * unit], on = (way: Way, metres: number) => stationAlong(way.run.stations, way.line, metres);
    const { sp, sq, r, oneLine } = corner, [ep, eq] = [(out: number) => edgeOf(p, sp, out, meeting), (out: number) => edgeOf(q, sq, out, meeting)];
    // A side's sidewalk at the arc's end: the width past the carriageway's edge where its kerb stands.
    const walkAt = (way: Way, side: 1 | -1, metres: number) => {
        const s = stationOn(way.run.stations, metres), { parts, edge } = sideOf(way.line, side, s.v, s.h);
        return parts.some(({ mat }) => mat === MAT.KERB) ? edge - s.h : 0;
    };
    const met = oneLine ? { a: 0, b: 0 } : crossing(ep(0), eq(0)), hit = r > 0 && met ? crossing(ep(r), eq(r)) : undefined;
    const ends: [number, number] = hit ? [p.along + p.step * Math.max(0, hit.a), q.along + q.step * Math.max(0, hit.b)] : [on(p, corner.ends[0]), on(q, corner.ends[1])];
    const meet: [number, number] = met ? [p.along + p.step * Math.max(0, met.a), q.along + q.step * Math.max(0, met.b)] : [on(p, corner.meet[0]), on(q, corner.meet[1])];
    const o = hit ? pointAt(ep(r), hit.a) : about(corner.o), m = met ? pointAt(ep(0), met.a) : about(corner.m);
    // A sharp corner's join reaches as far from the meet as the wider side lies from the bisector.
    const [dp, dq] = met ? [directionAt(ep(0), met.a), directionAt(eq(0), met.b)] : [p.u, q.u];
    const outer = (way: Way, side: 1 | -1, metres: number) => {
        const s = stationOn(way.run.stations, metres);
        return (sideOf(way.line, side, s.v, s.h).parts.at(-1)?.a ?? s.h) - s.h;
    };
    const half = Math.max(JOIN_SIN, Math.sqrt((1 - (dp[0] * dq[0] + dp[1] * dq[1])) / 2));
    const walk = Math.max(walkAt(p, sp, ends[0]), walkAt(q, sq, ends[1]));
    // A sharp corner with a kerb joins its sides; verges and bands of the ground meet on the ground.
    const join = r > 0 || oneLine || !walk ? undefined : Math.max(outer(p, sp, ends[0]), outer(q, sq, ends[1])) / half;
    return {
        meeting, p, q, sp, sq, o, r, ends, meet, join, feet: hit ? [pointAt(ep(0), hit.a), pointAt(eq(0), hit.b)] : [about(corner.a), about(corner.b)], apex: Math.hypot(m[0] - o[0], m[1] - o[1]),
        walk, road, oneLine,
    };
}

/** Whether a point in metres about the meeting lies on a corner's fillet, or its kerb and sidewalk. */
export function onCorner({ o, r, feet: [a, b], apex, walk }: Corner, x: number, y: number): boolean {
    const cross = (p: number[], q: number[]) => (p[0] - o[0]) * (q[1] - o[1]) - (p[1] - o[1]) * (q[0] - o[0]), turn = cross(a, b), point = [x, y], d = Math.hypot(x - o[0], y - o[1]);
    return r > 0 && d > r - walk && d < apex && cross(a, point) * turn > 0 && cross(point, b) * turn > 0;
}
