// A corner between two neighbouring ways of a junction: the arc that turns the kerb from the edge of the one to the
// edge of the other. It fits the ways' own curves: its centre lies on both edges offset by its radius, so it meets
// each edge square, and a way that curves away near the junction bends no kerb.
import { PAVED_CODES, type Line } from '../terrain/bins';
import { cornerAt } from '../terrain/corners';
import { inTown, outward, ROOM_M, sideAt, type Meeting, type Way } from './arms';
import { isDeck } from './decks';

/** Corner radii: a town street's kerb, a rural road's edge, a track's; the least arc. */
const TOWN_R = 5, RURAL_R = 7, TRACK_R = 4, LEAST_R = 0.8;
/** A kerbed corner's arc clears its sidewalk by this much, so that the sidewalk turns round it. */
const WALK_CLEAR_M = 0.6;
/** An arc tries a smaller radius by this factor while its ends leave a way's room. */
const SHRINK = 0.8;
/** Edges may meet this far behind the junction along a way: on it the arms' edges run on straight. */
const BEHIND_M = 0.5;

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
    /** Metres along each line at the arc's ends, and where the edges meet. */
    ends: [number, number];
    meet: [number, number];
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
    return { points: [[p[0] - (q[0] - p[0]) * BEHIND_M / l, p[1] - (q[1] - p[1]) * BEHIND_M / l], ...points], metres: [-BEHIND_M, ...list.map(s => Math.abs(s.along - way.along))] };
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

/** The corner from way p to the next way q round their meeting; none where their edges do not meet near it. The stamp holds the ground under its own arcs, so a corner that it rounds takes its radius. */
export function cornerOf(lines: Line[], meeting: Meeting, p: Way, q: Way): Corner | undefined {
    // Across the line at a step forward is the way's left, and angles grow to its right: y runs south.
    const sp = p.step > 0 ? -1 : 1, sq = q.step > 0 ? 1 : -1;
    const [ep, eq] = [(out: number) => edgeOf(p, sp, out, meeting), (out: number) => edgeOf(q, sq, out, meeting)];
    // A line that runs on through its vertex has one edge on this side: there is no corner to turn.
    const oneLine = p.line === q.line && p.v === q.v, met = oneLine ? { a: 0, b: 0 } : crossing(ep(0), eq(0));
    if (!met || Math.max(met.a, met.b) > ROOM_M) return undefined;
    const paved = PAVED_CODES.includes(p.line.code) && PAVED_CODES.includes(q.line.code), town = inTown(p) || inTown(q), [mx, my] = pointAt(ep(0), met.a);
    const walk = Math.max(sideAt(p, sp).walk, sideAt(q, sq).walk), least = walk > 0 ? walk + WALK_CLEAR_M : LEAST_R, stamped = cornerAt(lines, meeting.x + mx / meeting.unit, meeting.y + my / meeting.unit);
    const wanted = stamped ? Math.hypot(stamped.a[0] - stamped.o[0], stamped.a[1] - stamped.o[1]) * meeting.unit : paved ? (town ? TOWN_R : RURAL_R) : TRACK_R;
    const corner = (o: number[], r: number, a: number, b: number): Corner => ({
        meeting, p, q, sp, sq, o, r, ends: [p.along + p.step * Math.max(0, a), q.along + q.step * Math.max(0, b)], meet: [p.along + p.step * Math.max(0, met.a), q.along + q.step * Math.max(0, met.b)],
        feet: [pointAt(ep(0), a), pointAt(eq(0), b)], apex: Math.hypot(...pointAt(ep(0), met.a).map((value, i) => value - o[i])), walk, road: (p.through ? p : q).line, oneLine,
    });
    // A deck's edge has its kerb or its parapet: no arc rounds it.
    const sharp = oneLine || isDeck(p.line) || isDeck(q.line);
    for (let r = Math.max(wanted, least); !sharp; r = Math.max(r * SHRINK, least)) {
        const offset = [ep(r), eq(r)], hit = crossing(offset[0], offset[1]), last = r === least;
        if (hit && (last || (hit.a <= p.room && hit.b <= q.room)) && hit.a >= 0 && hit.b >= 0) return corner(pointAt(offset[0], hit.a), r, hit.a, hit.b);
        if (last) break;
    }
    return corner(pointAt(ep(0), met.a), 0, met.a, met.b);
}

/** Whether a point in metres about the meeting lies on a corner's fillet, or its kerb and sidewalk. */
export function onCorner({ o, r, feet: [a, b], apex, walk }: Corner, x: number, y: number): boolean {
    const cross = (p: number[], q: number[]) => (p[0] - o[0]) * (q[1] - o[1]) - (p[1] - o[1]) * (q[0] - o[0]), turn = cross(a, b), point = [x, y], d = Math.hypot(x - o[0], y - o[1]);
    return r > 0 && d > r - walk && d < apex && cross(a, point) * turn > 0 && cross(point, b) * turn > 0;
}

/** The point of an edge `metres` along it. */
function pointAt({ points, metres }: Edge, at: number): number[] {
    let k = 0;
    while (k + 2 < metres.length && metres[k + 1] < at) k++;
    const t = (at - metres[k]) / ((metres[k + 1] - metres[k]) || 1);
    return points[k].map((value, i) => value + t * (points[k + 1][i] - value));
}
